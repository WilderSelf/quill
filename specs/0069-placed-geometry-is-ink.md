# 0069 — A placed part reports the ink it draws

**Milestone:** M5 (closeout) · **Status:** implemented

## Why

`PlacedBlock` is a record of what was drawn. Eight producers in `layout-engine` fill in its `frame`,
and until this increment five of them reported the **slot** the content was laid into rather than the
ink that came out of it:

| producer | reported | should report |
|---|---|---|
| body and heading paragraphs | the whole column (`frame.rect.w_pt`) | the paragraph's ink |
| list markers | the gutter (`style.indent.first_pt`) | the marker's glyphs |
| table cells | the column's inner measure | the cell's text |
| component text sections | the panel's inner measure | the section's text |
| the contents list's own title | the whole frame measure | the title's advance |

Three already reported ink — master statics (spec 0047), the contents page number (spec 0041), and
spec 0067's tab segments and leaders. So the rule was not missing, it was *contradicted*, and every
consumer of placed geometry had to take the union of two incompatible meanings.

Measured, the gap is not marginal. In the `simple_table` fixture at a 432 pt frame, the wide column
is 318 pt and the ink in it is **32.4 pt** — the reported box was ten times the drawn one:

| fixture | measure | ink | ink / measure |
|---|---|---|---|
| table, column 0 (`Roll`) | 102.0 | 21.6 | 21% |
| table, column 1 (`Goblins`) | 318.0 | 32.4 | 10% |
| stat block, `Small humanoid, chaotic` | 420.0 | 124.2 | 30% |
| stat block, `AC: 15` | 420.0 | 27.0 | 6% |
| `Document::sample()` heading | 432.0 | 158.4 | 37% |
| `Document::sample()` body (justified, 3 lines) | 432.0 | 432.0 | 100% |

The last row is the one that says the change is targeted rather than sweeping: a justified paragraph
spends the difference between its natural width and its measure on its spaces *by construction*, so
its ink **is** its measure and nothing moves. What moves is ragged text, short lines, and cells.

### The five reasons, in the order they decide it

Recorded in full in `docs/roadmap.md`'s decisions log; restated here because a spec that only points
at a log is not a spec.

1. **Alignment is not recoverable downstream.** `PlacedBlock` carries no alignment field, so a
   right-aligned segment in a wide slot is indistinguishable from a left-aligned one. Under slot
   semantics its reported box is nowhere near its glyphs.
2. **It is already the repo's stated intent.** Spec 0047 says a placed static "reports where the line
   *actually* sits rather than the box it was aligned in — which is what spec 0050's geometry
   preflight will need to ask." Five producers contradicted it.
3. **Both of preflight's consumers want ink.** The safe-area check asks whether ink lands in the
   margin; `under_dpi` divides pixels by the *drawn* width.
4. **False positives are the expensive failure mode**, which spec 0050 states outright. A short
   ragged line in a wide column puts no ink near the trim, and reporting it teaches a user to skim
   the report — and so to skim the real finding.
5. It makes spec 0052's `/Link` hot areas correct as a side effect — **but not in this increment**.
   The entry title is the only part in the workspace carrying `link_page`, and it is the one place
   the rule is deliberately not applied yet (see *One stated exception*), so the hot area still spans
   the clip column and runs across the dot leader. Spec 0070 collects it, and asserts it.

### Why this is only honest after spec 0068

Under ink semantics `w_pt` is the **measured** advance. While the PDF drew a different glyph run than
it measured — up to 4.90 pt wider over a 62-character line, per 0068's own table — a right-edge
safety violation inside that band would have gone **unreported**. A false negative on a press check is
the one outcome `CLAUDE.md`'s "prefer a visible failure over silent press-corruption" rule forbids
outright, and shipping this first would have traded a class of false positives for a class of false
negatives. Spec 0068 made measured and drawn the same number, so the trade disappears and the
reported box is the drawn box. That is the whole reason for the ordering, and it is why this spec
could not have been written a week earlier.

## What

### One rule, on `PlacedBlock`

> **A placed part's `frame` is the bounding box of the ink that part actually draws**: `x_pt` is the
> left edge of its leftmost glyph, `x_pt + w_pt` the right edge of its rightmost, and `h_pt` follows
> the same rule vertically — the line boxes drawn, never the space reserved around them.

It lives as a doc comment on the enum, and every producer is checked against it. Nothing is lost by
it: the slot is a frame's geometry plus the stylesheet, both available without going through placed
output. What is *not* recoverable downstream is the ink.

`h_pt` moves with `w_pt`, to the same rule, because a box that is ink horizontally and slot
vertically is worse than either — no reader of the rectangle can know which question it answers. In
practice this means a paragraph's box is `lines × leading` and no longer carries the style's
space-after, which draws nothing. Panel parts were already measured that way, so this makes the two
agree rather than introducing a third convention.

### The case a naive reading gets wrong

**A multi-line paragraph is an ink bounding box, not an advance.** `Line::indent_pt` is added at
*draw* time and differs per line under a hanging indent (spec 0048), so no single line's box is the
paragraph's:

```
x_pt = measure_left + min over lines of indent_pt
w_pt = max over lines of (indent_pt + advance) - min over lines of indent_pt
```

A tab segment is the degenerate case — one line at zero indent — which is exactly why the general
rule has to be *stated*: the definition spec 0067 could get away with does not cover a paragraph.

Three new functions in `quill-text-layout` are the one place this is computed, next to
`natural_width` and `span_offsets` for the same reason those are shared:

- `line_advance` — a line's natural width **plus the justification it spends on its spaces**.
  `natural_width` answers "how wide is this text"; an ink box asks "how far does this line reach",
  and for a justified line the two differ by the whole stretch.
- `indent_base` — the leftmost inset of a line list.
- `ink_box` — the pair.

**The painters subtract `indent_base`.** `render/src/paint.rs` and `export-pdf/src/writer.rs` both
drew each line at `frame.x_pt + line.indent_pt`; they now draw it at
`frame.x_pt + line.indent_pt - indent_base(lines)`. The frame's `x_pt` gained exactly what each
line's inset gives back, so **no glyph moves** — which is what makes a geometry redefinition safe to
land without a content-stream change. Both call the same shared helper, on the same one-derivation
principle the crate already applies to `span_offsets`.

### The measure stays on the measurement

`PanelPart` now carries both: `dx_pt`/`w_pt` are the measure the run was broken to, and
`ink_dx_pt`/`ink_w_pt` are what reaches placed output. That split is not bookkeeping — it is how the
containment invariants below survive (see *What this costs*).

### `PlacedBlock::Image` keeps drawn-extent semantics exactly

`under_dpi` (`export-pdf/src/lib.rs`) divides a pixel count by `frame.w_pt`; anything but the drawn
width silently mis-reports resolution, which is a press-safety regression rather than a cosmetic one.
An image is the one variant where the old rule and the new agree, and it is untouched.

### One stated exception, and its expiry

**Expired: spec 0070 shipped and the exception is gone.** The entries are now a tabbed line laid
against one right stop, and their widths are ink like everything else. The paragraph below is what
this spec committed to, kept because the commitment is what made the exception safe to write.

The contents list's **entries** — title, leader, page number — still report their columns. Spec 0070
deletes that arithmetic outright in favour of one right tab stop with a dot leader, and its
equivalence claim is that every x and every dot count stays byte-identical to spec 0041's while the
three widths move individually and justified *there*. Moving them here would collide with it. The
list's own **title** is not an entry and moves now. The exception is written at the producer, naming
0070, so it cannot quietly become permanent.

## Acceptance criteria

- [x] One stated rule on `PlacedBlock`, in a doc comment, that every producer is checked against.
- [x] The five slot-semantics producers report ink; the three that already did stop being exceptions.
- [x] A multi-line paragraph under a hanging indent reports `min(indent)` / `max(indent + advance)`,
      asserted against a fixture that *has* a short line — otherwise the bounding box is untested and
      a per-line implementation would pass.
- [x] No glyph moves. `SAMPLE_EXPORT_DIGEST` does not move, and the render op lists do not change.
- [x] `PlacedBlock::Image` keeps drawn-extent semantics; `under_dpi` is untouched.
- [x] The containment assertions are re-expressed **against the measure at the producer** and are
      shown still to fail against the defect they were written for — a cell broken to too wide a
      measure. Reintroducing the defect and re-running is the check, per spec 0064's lesson.
- [x] The safe-area check gains a test that fails under slot semantics and passes under ink, on a
      preset with a **real safety margin** — `generic`'s `safety_pt == 0.0` short-circuits `intrudes`,
      so the same test on the default preset asserts nothing at all.
- [x] `component_parity`'s 20 constants are re-derived by the file's own rule, never pasted from a
      failing run, and the move is bounded by a third digest set.
- [x] Performance budgets still met (`cargo bench -p quill-testdoc`).

## Test strategy

### The digest move is bounded, not observed

`component_parity`'s module docs say it is never correct to paste in whatever number the test
printed. The re-derivation here is a third digest, `EXPECTED_EXTENT_FREE`: the `Debug` rendering with
`x_pt`, `w_pt` and `h_pt` textually stripped out of every placed rectangle. Those ten constants were
computed on the **pre-0069** engine, with the post-0069 test file compiled against it, and all ten
still match on the post-0069 engine.

That turns twenty new hexadecimal constants into a claim: the increment moved the extent of placed
rectangles and demonstrably nothing else — not a character of text, not a line break, not a `y_pt`,
not a colour, not a run table, not the number or order of placed blocks. It is the same discipline
`digest_geometry` applies to a struct that grew a field, pointed at a value that changed instead.
It is also checked and reported *first*, because if it moves, the change is not the change it claims
to be and neither of the other two digests can tell you that.

### The containment invariants move to the producer

Two invariants were expressed *through* `w_pt` and would have weakened to "the text that happened to
be there fits":

- no run overruns its panel's right padding;
- `table_columns_land_at_exact_fractions_of_the_measure`, exact widths 102.0 / 318.0.

Both are now asserted on the **measurement** — a new `measure_at` test helper runs `measure_block`
directly, which is where a column width is known — plus a structural check that each part's ink lies
inside the measure it was broken to. A test that has to reconstruct its input from its output was
asking the wrong object.

Proven rather than assumed, by reintroducing each defect and re-running:

| defect reintroduced | result |
|---|---|
| cell measure not reduced by the cell padding (`columns.push((x + pad, w))`) | `table_columns_land_at_exact_fractions_of_the_measure` fails: `cell 0 column width: 108` |
| section broken to `width` instead of `inner_w` | `a_stat_block_wider_than_its_frame_still_wraps_inside_the_padding` fails: `a run was broken to a measure that overruns the panel's right padding: 6 + 432 > 426` |

An honest note on the second: with *that* fixture the ink-only check would also have caught it, because
the long action line happened to fill its 432 pt measure exactly. The four other sections in the same
panel reported 46.8, 124.2, 32.4 and 27.0 pt of ink against a 426 pt measure, so a panel of only short
sections would have passed the ink check with the measure wrong. That is precisely the weakening the
re-expression exists to prevent, and it is why the invariant is not left to the fixture's luck.

### The safe-area test is proven non-vacuous

A document with `inside: 60, outside: 12` on a 432 pt trim puts the text frame 12 pt from the
fore-edge, inside a 36 pt safety margin, and the paragraph in it is two characters long. The test
asserts no `SafeArea` finding, and asserts in the same breath that the **slot** it was laid into does
produce one — so the check is live and it is the placed geometry that changed.

Verified by sabotage, because a preflight test that passes for the wrong reason is worse than none:

| build | preset | result |
|---|---|---|
| ink (shipped) | `safety_pt: 36` | passes |
| slot semantics restored | `safety_pt: 36` | **fails** — the test distinguishes the two |
| slot semantics restored | `PodPreset::generic()` (`safety_pt: 0`) | **passes** — vacuous, as warned |

The third row is the trap the acceptance criteria name: `intrudes` early-returns when the live-area
margins are non-positive, so the same test written against the default preset asserts nothing at all
and would have shipped looking green.

## Risks

- **A geometry redefinition that moves a glyph is a silent press change.** Closed by the two painters
  subtracting `indent_base` from the same shared helper the frame added it with, and by
  `SAMPLE_EXPORT_DIGEST` — which did not move, exported through the same content-stream template
  spec 0068 established. Text drawing never read `w_pt`, and the indent rebase cancels, so the
  content stream is byte-identical.
- **Twenty digest constants moving at once is where a real defect hides.** Closed by
  `EXPECTED_EXTENT_FREE`, above.
- **The two consumers disagree about what they want.** They do not — both want ink — but they want it
  for opposite reasons, and `under_dpi` would be broken by a *narrower* box exactly as the safe-area
  check is broken by a wider one. Images are excluded explicitly rather than by omission.

## Non-goals

- **The contents list's entries**, which spec 0070 takes; see the stated exception above.
- **A glyph-tight vertical box.** `h_pt` is the line boxes drawn (`lines × leading`), not an
  ascent-to-descent bound over the actual glyphs. That is the workspace's existing notion of the
  space type occupies, it is what a panel part has always reported, and a tighter one would need
  per-line ascent and descent that no consumer asks for.
- **Recording the slot on `PlacedBlock`.** A second rectangle beside the first would let a consumer
  pick the wrong one, which is the state this increment is ending. The slot stays where it is known:
  on the frame, the stylesheet, and — for a component — on `PanelPart`, which is a measurement and
  not placed output.
- **Weight and slant on a declared component's runs.** Component text is measured *and* drawn at
  regular/upright, so the ink box is measured in the format it is drawn in. Making both read the
  style is spec 0064's follow-on, not this.
