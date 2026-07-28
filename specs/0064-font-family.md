# 0064 — The font family, and the overrides that move a glyph

**Milestone:** M5 · **Status:** implemented

## Why

Spec 0063 gave a paragraph runs, and gave a run an `InlineStyle` carrying `size_pt`, `color`,
`tracking_pt` and `baseline_shift_pt`. Only `color` took effect. The other three were declared so
that the format would not move twice, and left inert because all three change what a box *measures*,
which is the line breaker's input rather than its output.

Weight and italic were not even declared, and the reason is stated in `InlineStyle`'s own doc
comment: they were not a model gap. `quill-fonts` had exactly one face. `Font::bundled()` parsed one
embedded `SourceSerif4-Regular.ttf`; nothing indexed faces by weight or style; `shape`, `measure_run`
and `ascent_pt` took a size and no face selector; `export-pdf` subset one program and emitted one
resource name `F0`. Bold was not a missing field — it was a missing font family, missing multi-face
subsetting, and a resource dictionary with exactly one entry.

The three missing faces landed first, as their own change: Source Serif 4 Bold, Italic and Bold
Italic, static instances at the same optical size as the regular, same OFL, provenance recorded in
`tools/fonts/build-faces.py`. This spec is everything that turns four font programs into a family a
paragraph can use.

## What

### The model names a face; it does not hold one

`core-model` gains one small `Copy` type:

```rust
pub struct Weight(pub u16);   // 400 = regular, 700 = bold; the OS/2 usWeightClass scale
```

A numeric weight rather than a `Bold` flag, because the general mechanism is the one a family with a
Light and a Semibold can also use — and because it is the scale the faces themselves are labelled on.
`Weight::REGULAR` and `Weight::BOLD` are constants, not variants.

`InlineStyle` gains `weight: Option<Weight>` and `italic: Option<bool>`, beside the four fields 0063
declared. `ParagraphStyle` gains `weight: Weight` and `italic: bool`, defaulting to regular and
upright, so that a *paragraph* has a face to be the thing a run overrides. Both are additive optional
fields with serde defaults, so `FORMAT_VERSION` stays **4**: a v4 document written before this spec
reads back with every new field at its default, which is exactly the document it was.

`core-model` still knows nothing about `quill-fonts`. A model can name a face; it cannot hold one.

### `RunFormat`: what the breaker needs to know about a run

`text-layout` depends on no other quill crate, and this spec does not change that. It gains a plain
value type:

```rust
pub struct RunFormat {
    pub size_pt: f32,
    pub weight: u16,
    pub italic: bool,
    pub tracking_pt: f32,
}
```

and `RunMetrics` gains one method beside `measure_run`:

```rust
fn measure_format(&self, text: &str, fmt: RunFormat) -> f32 {
    self.measure_run(text, fmt.size_pt) + /* tracking, per glyph */
}
```

defaulted, so every existing implementation — `MonospaceRunMetrics`, and every test double — keeps
working unchanged and measures the way it always did. `measure_run` is not removed and not
deprecated: it is what a single-format paragraph still goes through, which is what makes the
byte-identity criterion below reachable at all rather than merely likely.

`baseline_shift_pt` is deliberately **not** in `RunFormat`. It moves a glyph vertically without
changing any advance, so it is a drawing property, not a measuring one. Putting it in the breaker's
input would make it invalidate line breaking for a change that cannot move a break.

### The item stream splits where the format changes, and nowhere else

`break_runs_shrinkable` and `justify_runs_indented` take a `formats: &[RunFormat]` parallel to
`runs: &[&str]`. From those the breaker derives **format segments**: maximal byte ranges of the
concatenated paragraph over which `RunFormat` is constant. Two adjacent runs that differ only in
colour are one segment.

- A box is measured at its segment's format, and is **split at a segment boundary** into adjacent
  boxes, each measured at its own. Adjacent boxes with no glue or penalty between them are not a
  break opportunity, so splitting adds no legal break — it only stops a single `measure_format` call
  from spanning two faces.
- Inter-word glue is measured at the format of the segment the space falls in.
- A hyphen inserted at a discretionary break is measured at the format of the segment its preceding
  segment ends in — the face the hyphen would be drawn in.

Two consequences follow, and both are the intended behaviour rather than a limitation:

- **A word straddling a face change is still one word.** It is hyphenated as one, and cannot be
  broken at the face change, because a face change emits no penalty. That is 0063's rule, unchanged.
- **No kern pair is applied across a face change.** A pair is a fact about one font program; the same
  two characters in two different programs have no pair. Splitting the measurement at the boundary is
  what makes the measured width equal the drawn width there.

Where every run shares one format — every document that exists today — there is exactly one segment,
the split never fires, and the item stream is the one 0063 produced, byte for byte.

### Spec 0051's pruning and spec 0060's rule, re-stated for per-box sizes

Spec 0051 retires a line start once `(wsum[e] − zsum[e]) − (wsum[s] − zsum[s]) > l_for(s)`, on the
argument that every item contributes `width − shrink ≥ 0` and so the tightest achievable line is
monotone non-decreasing in the line's end.

That argument is **per item**, not per paragraph: a box contributes `w ≥ 0` whatever size measured it,
a glue contributes `g − g/3 > 0` whatever size measured *it*, and a penalty contributes `0`. Per-box
sizes change the value of `w`; they do not change its sign. The measure `l_for(s)` is a property of
the *line* (first or rest), and per-run size does not vary it. The pruning is therefore sound
unchanged, and is not narrowed. `crates/testdoc/tests/line_break_equivalence.rs` continues to assert
the output is what the unpruned breaker produces, now over a mixed-format paragraph as well.

Spec 0060's rule — a line drawn at its natural width may not exceed its measure — is likewise
untouched in form: `natural` is still the sum of the line's item widths, each now measured at its own
format. The one place that needed care is the *ragged* predicate in `justify_runs_indented`, which
asks whether any single word overflows the measure: it now measures each word at its own segment's
format rather than at the paragraph's size, because a word set in 18 pt bold is what would overflow.

### Leading is a property of the paragraph, not of the tallest run in it

`Measured::first_baseline_offset` keeps computing `space_before_pt + ascent_pt(style.font_size_pt)`,
and a block's `leading_pt` stays its paragraph style's. A run set larger does **not** move the line it
sits on, and does not move the lines after it.

This is a decision, not an omission, and it is the one spec 0058 requires: a gridded line's position
is set by the paragraph's leading, or the grid is not a grid. It is also what makes the bundled family
safe — the four faces were built to share one set of vertical metrics precisely so that emphasising a
word cannot move a line, on screen or on the page. A superscript that wants to sit above the line uses
`baseline_shift_pt`, which is what it is for.

### `FontFamily`: selecting a face, and saying so when it cannot

`quill-fonts` gains:

```rust
pub struct FontFamily { /* one or more faces, each with its weight and slant */ }

impl FontFamily {
    pub fn bundled() -> FontFamily;                       // the four shipped faces
    pub fn single(font: Font) -> FontFamily;              // a user-supplied program (spec 0004)
    pub fn select(&self, weight: u16, italic: bool) -> Selection;
}
```

`select` returns the face asked for, or the nearest the family has, together with whether it had to
substitute. The nearest is defined and asserted: prefer the requested slant, then the smallest
absolute weight difference, then the lighter of a tie. A family with one face answers every request
with that face.

**A substitution is announced once per export on stderr**, naming what was asked for and what was
used — `bold italic requested; the family has only regular` — and once, not once per run, because a
warning printed per run is a warning nobody reads. Silence would be the failure this repository
exists to avoid: a document that asked for bold and got regular, with nothing said, is a press file
that is quietly not what its author wrote.

### Measuring and drawing through the family

`quill-fonts` gains a `FamilyMetrics` that implements `RunMetrics` over a `FontFamily`: `measure_run`
measures in the regular face exactly as `Font` does today (so every existing caller is unchanged), and
`measure_format` selects the face, shapes in it, and adds `tracking_pt` **per shaped glyph** —
per glyph and not per character, because that is what a PDF `Tc` adds, and measuring by a different
unit than the one the page is drawn with is the drift this workspace has one shaper to prevent.

`text-layout` gains one shared helper, so the writer and the painter cannot disagree about where a
span starts:

```rust
pub fn span_offsets(line: &Line, formats: &[RunFormat], metrics: &impl RunMetrics) -> Vec<f32>
```

It accumulates each span's x from the widths of the spans before it, measuring **consecutive spans of
equal format as one string** — so a single-format line is measured exactly as one call on the whole
prefix, which is what the screen painter does today, and a face change is measured either side of the
boundary, which is what the breaker did.

### The export embeds the faces it uses, and only those

`collect_doc_chars` becomes per-face: it walks the document resolving each run's face and records
which characters that face must carry. `export-pdf` then builds one `EmbeddedFont` per **used** face,
allocates its own Type0/CIDFont/descriptor/FontFile chain, and names it `F0`, `F1`, … in a stable
order. A document that uses no bold embeds no bold program; a document that uses only the regular face
gets exactly the single `/F0` resource dictionary it gets today.

In the content stream, a line whose spans share one format is emitted exactly as it is emitted today —
one `set_font`, one `Tj` or one `TJ`. A line whose spans differ switches, at each span boundary and
only where the value actually changes:

- `Tf` — the span's face resource and its size;
- `Tc` — `tracking_pt`, reset to 0 when the next span has none;
- `Ts` — `baseline_shift_pt`, reset to 0 likewise;
- the fill colour, as 0063 already does.

No text matrix is set per span: PDF advances the text position by the glyphs shown, and the widths it
advances by are the same widths the family measured with.

The justification adjustment `−1000 · space_adjust_pt / size` is computed **per span**, from that
span's own size, because the unit is a thousandth of the current font size and a span set at another
size would otherwise spend the wrong amount of space.

### The screen paints the same thing

`PaintOp::Text` gains the face selector, `tracking_pt` and `baseline_shift_pt`; the rasterizer selects
the face from the same `FontFamily` and applies the same per-glyph tracking. `paint_page` takes a
`&FontFamily` where it took a `&Font`, and computes span offsets through the shared helper rather than
its own prefix measurement.

### The cache learns which fields move a glyph

`session.rs`'s `content_fingerprint` hashes a run's `text` and nothing else about it into the
measurement-affecting content hash, with `color` folded into the tail that invalidates the placed
result without invalidating the measurement. That split is right and now needs the other four fields
put on the correct side of it:

- `size_pt`, `tracking_pt`, `weight`, `italic` — into the **measurement** hash. Every one changes an
  advance.
- `baseline_shift_pt` — into the **tail**, beside colour. It moves a glyph without moving a break.

Missing this is exactly the failure `MeasureKey`'s own doc comment names: a key missing a dimension
the measurement depends on returns a stale layout and the document is silently wrong.

### `quill import` learns the two constructs it refused

`import.rs` supported six constructs completely and said so; emphasis was not among them, and
`**bold**` reached the page as four literal asterisks. It now parses `**bold**` / `__bold__` and
`*italic*` / `_italic_`, nested (`***both***`), into runs carrying `weight`/`italic` overrides. The
posture statement becomes eight constructs, and the two new ones are enumerated with the same
precision as the rest: an unmatched delimiter is literal text, not an error and not a silent
swallow.

## Acceptance criteria

- **A document using no metric-bearing override exports byte-identically to what 0063 produced.**
  `SAMPLE_EXPORT_DIGEST` does not move. This is the criterion that separates the intended change
  from a bug, and it is the one the whole increment stands on.
- A `FontFamily` selects a face by weight and slant, with the nearest-match rule asserted per branch,
  and announces a substitution once per export on stderr rather than silently setting regular.
- Per-run `size_pt` and `tracking_pt` reach the breaker: a box measures at its own run's size, glue
  measures at the size of the run it sits in, and a mixed-size paragraph is asserted against a
  hand-computed width.
- Spec 0051's pruning is re-argued for per-box sizes and asserted over a mixed-format paragraph;
  spec 0060's natural-width rule holds with a mixed-format last line.
- `baseline_shift_pt` is emitted as `Ts` and reset; `tracking_pt` as `Tc`.
- The export subsets every face actually used and only those: a document using no bold embeds no bold
  program, asserted by counting `/FontFile2` streams.
- A paragraph mixing regular and bold measures as the sum of the two runs' measurements at the join,
  and no kern pair is applied across the faces.
- The baseline grid (0058) is unmoved: a gridded line's position is set by the paragraph's leading,
  not by its tallest run, asserted on a mixed-size paragraph.
- `quill import` gains `**bold**` and `*italic*`, and its posture statement is updated in the same
  change.
- The incremental cache invalidates a measurement on a size, tracking, weight or italic edit, and
  invalidates only the placed result on a colour or baseline-shift edit — asserted with a work
  counter, not a timing.
- `benches/budgets.toml`: single-face shaping cost unchanged; a documented budget for the mixed-face
  case.

## Non-goals

- **A second family.** The mechanism selects a face within *a* family; which family a document uses is
  spec 0004's user-font work, and the type is shaped so that adding it is a constructor, not a
  redesign.
- **Optical sizing.** The bundled faces are pinned at one optical size. `opsz` is a variable-font axis
  and every bundled face is a static instance; instancing at run time is a different feature with a
  different dependency.
- **Small caps, drop caps, OpenType feature control.** All are run properties and all are downstream
  of this one, but none of them is a face selection.
- **Naming a run treatment.** `strong` and `emphasis` as *names* are spec 0065; this spec gives them
  something to resolve to.
