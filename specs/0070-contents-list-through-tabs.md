# 0070 — The generated contents list is a tabbed paragraph

**Milestone:** M5 (closeout) · **Size:** small · **Status:** implemented

## Why

Spec 0067 shipped tab stops and leaders, and spec 0041's hand-rolled contents list went on standing
beside it. `measure_toc` computed a per-level indent, clipped the title to a column, measured the
page number, subtracted a gap at each end, divided by the width of a full stop and laid that many
dots into the space between — sixty-odd lines that spell out, longhand, *one right tab stop with a
dot leader*.

Two mechanisms that produce the same geometry is the state spec 0054 ended for components, by the
same move: retire the hand-written measurement, route the shape through the general interpreter, and
make **byte-identical geometry** the acceptance criterion rather than a code-tidiness argument. A
generalization that produces different output is a rewrite, and a rewrite of working press geometry
is not worth doing.

It also fixes a defect nobody had filed. See *The link nobody filed*.

## What

### The entry is a tabbed line

Each entry is laid as

```
"{clipped title}\t{page number}"
```

against **one right tab stop** at `width - indent`, carrying a `.` leader with
`gap_pt: TOC_LEADER_GAP_PT`, through `quill_text_layout::lay_tabs` — spec 0067's mechanism, not a
copy of it. The whole line is offset by the level indent, so the stop stays the column it names.

The arithmetic is the same arithmetic, which is the point:

| spec 0041, by hand | spec 0067's mechanism |
|---|---|
| `dx = indent` | segment 0 at `x = 0`, offset by `indent` |
| `leader_x = indent + title_w + GAP` | `fill_leader` starts at `pen + gap`, offset by `indent` |
| `leader_end = width - number_w - GAP` | `fill_leader` ends at `x - gap`, where `x = stop - number_w` |
| `dots = floor((leader_end - leader_x) / dot_w)` | a whole number of repetitions, clipped to the gap |
| `dx = width - number_w` (right-aligned) | `TabAlign::Right` ⇒ `x = stop - w`, offset by `indent` |

### One entry point, two callers

`lay_tabs` → placed geometry now happens in exactly one place, `tab_parts`. Both callers go through
it: an authored paragraph whose style names stops (`measure_tabbed`, spec 0067) and the generated
contents list. `tab_parts` takes an `origin_dx_pt` — added to each segment's resolved `x_pt` rather
than folded into the stops — which is what lets a contents entry carry its level indent without the
stop meaning something different per level.

### The ellipsis clipping stays, and did not generalize

A long chapter name is truncated with an ellipsis rather than wrapped, and that stayed in
`measure_toc` deliberately. **It is not a tab rule.** It is the contents list's own "an entry does
not wrap": a contents list is *scanned*, and a two-line entry whose page number sits beside the first
line reads as two entries. A price list, a bibliography or a specification sheet — the other things
0067's stops are for — want no part of it, and 0067 already names a wrapping tabbed paragraph as a
non-goal for its own reasons. Generalizing the clip would have meant inventing a paragraph-level
"truncate rather than wrap" property that exactly one caller wants, which is the shape `CLAUDE.md`
calls a defect.

## The equivalence claim, and the three widths that break it

**Every x position and every leader's dot count is byte-identical to spec 0041's output.** Asserted
on the *bits* of each `f32`, not to a tolerance — see *Test strategy*.

**Three widths deliberately move**, and this **inverts the criterion the known issue originally
stated** ("byte-identical, or the generalization is wrong"). It inverts because the question that
blocked the increment has since been answered: spec 0069 settled that **`w_pt` on a placed part is
ink**, the bounding box of what the part actually draws. Under that rule the mechanism's widths are
the correct ones and `measure_toc`'s are the defect, so a *fully* byte-identical result would have
meant the re-expression had preserved a bug in order to keep a number green. The criterion the known
issue stated was written before the decision existed; keeping it would have been deferring to a
digest instead of to a rule.

| part | reported before | reports now | why it moves |
|---|---|---|---|
| the entry **title** | `title_max`, the clip column | its measured advance | The column is a *clip*, not the ink. It is the frame measure minus three constants, recoverable from the frame and the stylesheet without going through placed output; the advance is not. It is also the rectangle spec 0052's `/Link` hot area is emitted from — see below. |
| the **leader** | `leader_end - leader_x`, the gap | `dot_w × dots` | The dots are drawn; the gap is not. The leader deliberately stops short of the number by up to one dot's width (a partial glyph at the end of a contents entry is the thing a reader notices, spec 0067), so the gap systematically over-reports the ink by that remainder. |
| the list's **own title** | the whole frame measure | its measured advance | **Already moved by spec 0069**, which took the list's own heading — it is not an entry — while explicitly leaving the entries to this increment. Verified here rather than changed. |

The page number is the control: it was one of the three producers already reporting ink before 0069
made it the rule, and it does not move. Nor does any `y_pt`, `h_pt`, text run, or the number or order
of placed parts.

## The link nobody filed

The contents entry title is **the only part in the workspace carrying `link_page`**, so spec 0052's
`/Link` hot area was emitted from `title_max`. In a screen export the clickable region spanned the
whole clip column — past the end of the title, across the dot leader, and up to the page-number
column. Measured on the `linked_doc` fixture: the title's ink ends at **99.3 pt**, the leader starts
at **103.3 pt** and draws **315.3 pt** of dots, and the annotation's `/Rect` ended at **402 pt** —
so roughly three quarters of the hot area was leader.

The comment at the producer said a link whose hot area has drifted off its own text is
"structurally impossible", because the candidate is emitted from the *same* rectangle as the run it
belongs to. That was true vertically and false horizontally: sharing the rectangle only makes the hot
area right if the rectangle is the ink. Moving the title to ink makes the claim true in both axes,
and the comment now says which increment made it so.

**No test covered it, and the two that look like they do could not.** Both
`a_screen_export_links_a_contents_entry_to_the_page_its_heading_is_on` (`export-pdf`) and
`every_contents_entry_carries_a_link_candidate_over_its_own_title_run` (`layout-engine`) compare the
link rect to the *title run's frame*. The two are the same rectangle by construction, so they move
together and both pass under either semantics. A new test names something the defect cannot move with
it: the **leader's own origin**, and the title's advance **measured straight from the export font**.

## Acceptance criteria

- [x] Every x position and every leader's dot count is byte-identical to spec 0041's output,
      asserted against a transcription of the arithmetic 0070 deletes rather than against a golden
      captured after the change.
- [x] The three widths move, each asserted against what it now claims to be, each justified above.
- [x] The `/Link` hot area covers the title's text and stops short of the dot leader, and the test
      is shown to fail against the old geometry.
- [x] The ellipsis clipping stays in the contents list.
- [x] `SAMPLE_EXPORT_DIGEST` does not move — `Document::sample()` has no `Block::Toc`.
- [x] `component_parity`'s constants do not move — its corpus has no `Toc` fixture.
- [x] Performance budgets still met (`cargo bench -p quill-testdoc`).

## Test strategy

### The oracle is the code being deleted

`toc_reference_geometry` in `layout-engine`'s tests is a deliberate transcription of spec 0041's
entry arithmetic — indent, clip column, leader origin, dot count, right-aligned number. The
equivalence claim is exactly "the mechanism puts every part where *that* put it", and a golden
captured after the change would have proved nothing.

Comparison is on `f32::to_bits`, not a tolerance. "Byte-identical" is the claim; a tolerance would
let the two arithmetics drift apart by a hair each time either is touched, which is how an
equivalence quietly stops being one.

One subtlety is worth recording because it *could* have bitten. Floating-point addition is not
associative, and the old leader origin was `(indent + title_w) + gap` while the mechanism computes
`indent + (title_w + gap)`. Over a random scan of 200,000 plausible operand triples the two differ by
one ulp (~3 × 10⁻⁵ pt at a 400 pt x) in about 0.2% of cases; they do not differ on any value either
fixture produces, and the dot count — the quantity a reader would actually see change — did not
differ on a single one of the 200,000. The bit-exact assertion is what would report it if a future
metrics change moved a case into that band, rather than letting it pass as "close enough".

### Verified against both metrics, before and after

The full placed geometry of a contents list was dumped bit-exact — `x`, `y`, `w`, `h` and text of
every part, plus every link rect — on a build of `main` and on this one, and diffed:

- **`MonospaceRunMetrics`**, a seven-entry fixture spanning four indent levels (0/12/24/36 pt), page
  numbers of one and two digits, and a title long enough to be clipped: every `x`, `y`, `h` and every
  dot string identical; `w` moved on exactly the title and the leader of each entry (and on the link
  rect, which is the title's).
- **The bundled font through `lay_out_for_press`**, `linked_doc`: same result. This is the one that
  matters for the associativity note above, because its advances are not round numbers.

### The `/Link` test proves itself by sabotage

`a_contents_links_hot_area_covers_its_title_and_stops_short_of_the_dot_leader` finds the annotation
by its **destination**, not by its rect — matching on the rect would make the assertion circular —
then asserts the `/Rect` starts where the title does, ends where the title's ink ends (measured from
the export font family, not read back out of the geometry under test), and does not reach the
leader's origin. It also asserts the leader is over 100 pt long, so "running across it" is a visible
defect rather than a rounding question.

Reintroducing the old geometry (`title.w_pt = title.ink_w_pt = title_max`) and re-running:

| test | under the defect |
|---|---|
| `a_contents_links_hot_area_covers_its_title_and_stops_short_of_the_dot_leader` | **fails** — `the hot area must end where the title's ink ends: 402 vs 99.3` |
| `a_contents_entry_reports_the_ink_it_draws_rather_than_the_columns_it_was_given` | **fails** — an entry title reports its advance |
| `a_screen_export_links_a_contents_entry_to_the_page_its_heading_is_on` | **passes** — as predicted |
| `every_contents_entry_carries_a_link_candidate_over_its_own_title_run` | **passes** — as predicted |
| `every_contents_x_and_dot_count_is_what_the_hand_rolled_arithmetic_produced` | **passes** — no x moved |

The last three rows are the finding: this is a defect the existing suite was structurally unable to
see, and the equivalence test is correctly blind to it too, because the defect is in a width and the
equivalence claim is about positions.

## Two behaviour changes in degenerate cases, stated rather than discovered

Both are unreachable with the bundled faces and no fixture hits either, but the mechanism's rule now
governs them and it differs from the hand-rolled one:

- **A face whose full stop measures zero.** `measure_toc` clamped the dot width to `0.01 pt` and
  would have drawn tens of thousands of dots; `fill_leader` draws none. Drawing nothing is the
  correct answer to "how many of a zero-width glyph fit".
- **An entry whose clipped title still overruns the stop** (possible only if `width - indent` is
  under ~31 pt, where the clip column bottoms out at its 1 pt floor). `measure_toc` put the number at
  `width - number_w` regardless, overlapping the title; `lay_tabs` butts it against the pen, because
  "two stretches of text in the same place is the one outcome no rule here may produce" (spec 0067).

## Non-goals

- **Tab stops on the contents list's paragraph styles.** The stop a contents entry needs is derived
  from the frame measure and the entry's level, not authored, and putting `toc-1`…`toc-6` stops in
  the default stylesheet would give every existing document six style entries it never asked for.
  When user-controlled contents formatting arrives it can name stops; the derivation stays the
  default.
- **Generalizing the ellipsis clip**, above.
- **Anything else that carries `link_page`.** Contents entries remain the only producer; a
  cross-reference is M6.
