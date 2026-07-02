# 0017 — Knuth-Plass optimal line breaking

- **Milestone:** M1
- **Status:** in-progress (increment 1 — the optimal breaker, ragged output — implemented;
  increment 2 = justified rendering, deferred)
- **Crates:** `quill-text-layout` (owner), `quill-layout-engine`, `quill-export-pdf`

## Problem

Line breaking is **greedy** (`break_by_width`, spec 0015/0016): it packs each line as full as it
can, then breaks. Greedy is locally optimal and globally poor — overfilling an early line strands
words onto later ones, producing uneven line lengths (ragged text) and, once text is justified,
uneven inter-word spacing ("rivers", loose/tight lines). Press-quality justification — the reason
this product exists — is exactly what greedy cannot deliver.

Knuth-Plass **total-fit** chooses *all* breakpoints in a paragraph together to minimize a global
cost (the sum of per-line *demerits*), so the whole paragraph is balanced rather than each line
being maximally stuffed. It is the standard algorithm (TeX) and is a named follow-up in both spec
0015 and spec 0016. Shaping-aware measurement (spec 0016) now exists, so the widths KP optimizes
over are real; KP is the next quality upgrade on that seam.

## Architectural decision (optimize the algorithm first, justify the rendering second)

The valuable end state — evenly-spaced **justified** text — has two separable parts: (1) *choosing*
better breakpoints, and (2) *rendering* the resulting lines with adjusted inter-word spacing so each
line fills the frame. Part 2 is the only part that emits new PDF operators and so carries press-output
risk. Following the same risk-ordering as spec 0016 ("measure with shaping, but draw as before"),
this spec lands the algorithm first with **unchanged, still-ragged rendering**, then adds justified
rendering as a second increment:

- **Increment 1 — the optimal breaker, ragged output.** Add `break_paragraph` to `text-layout`
  implementing Knuth-Plass total-fit over a box/glue/penalty model, returning the same
  `Vec<String>` line shape `break_by_width` returns. `lay_out` switches to it. Text is still drawn
  **left-aligned / ragged-right** by the writer (no new operators); the only output change is *which
  words fall on which line*. Lowest-risk: a pure, unit-testable algorithm change with no new PDF
  content-stream constructs.
- **Increment 2 — justified rendering (named, deferred below).** Carry each line's computed
  adjustment ratio out of layout so the writer stretches/shrinks inter-word space to fill the frame
  (justification); the paragraph's last line stays ragged. This is where a new spacing operator and
  a golden-file regeneration land.

Measurement stays on the **spec-0016 `RunMetrics` seam** — no new trait, no dependency change. The
box/glue widths KP needs are derived from `RunMetrics::measure_run` (below). The acyclic crate seam
(`export-pdf → layout-engine → text-layout`) is unchanged: `text-layout` owns the algorithm,
`layout-engine` calls it, `export-pdf` supplies the shaper.

## Behavior (increment 1)

### The box / glue / penalty model

A paragraph is turned into a sequence of items measured under `RunMetrics` at `size_pt`:

- **Box** — one word. Width = `measure_run(word, size_pt)`.
- **Glue** — one inter-word space, with a natural width and elasticity. Natural width
  `w = measure_run(" ", size_pt)` (the stub returns `em_ratio · size_pt`; the rustybuzz shaper
  returns the space glyph's advance). Elasticity follows the classic interword defaults:
  **stretch = w/2**, **shrink = w/3**.
- **Penalty** — a forced break at end of paragraph (the paragraph-final glue has effectively
  infinite stretch, so the last line is neither stretched nor penalized for being short).

Increment 1 has **no hyphenation penalties** (no intra-word breakpoints); boxes are whole words, as
in greedy breaking. Line width is modeled additively as `Σ box widths + Σ glue widths`; the small
cross-word kerning at a joining space that `break_by_width` captured by measuring the whole string is
folded into glue and is negligible at body sizes (a fidelity note, revisited if it ever matters).

### Fitness of a line (adjustment ratio, badness, demerits)

For a candidate line covering a run of boxes with total natural width `W` (boxes + interior glue
naturals) against target `max_width_pt = L`, with total stretch `Y` and total shrink `Z` of its
interior glue:

- **Adjustment ratio** `r`: `0` if `Y=Z=0`; `(L−W)/Y` if `W ≤ L` (stretch, `r ≥ 0`); `(L−W)/Z` if
  `W > L` (shrink, `r < 0`).
- **Feasible** iff `r ≥ −1` (a line cannot shrink past its available shrink). A line with `r < −1`
  is infeasible and cannot be part of a breaking.
- **Badness** `b(r) = 100 · |r|³` (clamped to a ceiling — `BADNESS_CEIL = 10000`, TeX's
  "infinitely bad"). The `r = 0` when `Y = Z = 0` rule covers a single word that *fills the frame
  exactly* (`W = L`). A single word that is **underfull** (`W < L`) has no glue to stretch and so
  cannot be justified: it is the *near-infinite single-word case* the ceiling exists for — its
  badness is `BADNESS_CEIL`, not `0`. This is what keeps total-fit from stranding a lone short word
  on an interior line (the last line, being free, may still be a short single word).
- **Demerits** of a line `= (LINE_PENALTY + b(r))²`, with `LINE_PENALTY` a named constant
  (default `10`, TeX's `\linepenalty`).

**Total-fit:** among all breakings in which *every* line is feasible, `break_paragraph` returns the
one minimizing the **sum of line demerits**. Determinism tie-break (equal total demerits): fewest
lines, then the lexicographically earliest break-index sequence — so identical input always yields
identical lines regardless of active-node visit order.

### `break_paragraph` (text-layout)

```rust
pub fn break_paragraph(
    text: &str,
    max_width_pt: f32,
    size_pt: f32,
    metrics: &impl RunMetrics,
) -> Vec<String>
```

Same signature shape as `break_by_width`, same normalization and degenerate behavior:

- Whitespace normalized to single spaces between words; empty text → no lines.
- A single word wider than `max_width_pt` is emitted **alone** (it overflows; `r < −1` is
  unavoidable there — breaking oversized words is hyphenation, deferred). More generally, if *no*
  fully-feasible breaking exists (some word forces an overflow line), `break_paragraph` **falls back
  to the greedy `break_by_width` result** rather than failing — laying text out (even overflowing)
  is always recoverable; refusing to lay out is not. (`break_by_width` is retained for this fallback
  and for parity tests.)
- Text that fits on one line → one line (`r ≥ 0`, no break taken).

`break_by_width` is **not removed**: it is the documented fallback and the parity oracle.

### Wiring

- `layout-engine::lay_out(doc, metrics)` calls `break_paragraph` instead of `break_by_width`; block
  height is still `lines.len() · BODY_LINE_HEIGHT_PT`. No signature change.
- `export-pdf` is unchanged this increment: `EmbeddedFont`/`ShapingContext` construction and the
  drawn content stream are identical — the writer still draws each line left-aligned at the frame's
  left edge. Only the *contents* of `lines` differ (better breakpoints).

## Inputs / outputs

- **Input:** a `Document` plus a `RunMetrics` implementation (the rustybuzz-backed shaper in export;
  the monospace stub in tests).
- **Output:** `Vec<LaidOutPage>` whose lines are chosen by total-fit rather than greedily. Export
  bytes are structurally unchanged except where a different breakpoint moves a word to another line
  (and thus shifts pagination). **No new PDF operators this increment** — justified spacing is
  increment 2.

## Acceptance criteria

- **Optimal beats greedy on a crafted paragraph.** Under `MonospaceRunMetrics { em_ratio: 0.6 }` at
  `BODY_FONT_SIZE_PT`, a paragraph is constructed where greedy overfills the first line and strands a
  later line (higher total demerits), and `break_paragraph` returns the balanced breaking. Assert the
  exact returned lines equal the hand-computed total-fit optimum, and that its total demerits are
  strictly lower than the greedy breaking's.
- **Degenerate parity with `break_by_width`:** empty/whitespace text → no lines; a single over-wide
  word → emitted alone; text that all fits → exactly one line. (These match greedy so the two agree
  on the easy cases.)
- **No-feasible-breaking fallback:** a paragraph containing an unbreakable over-wide word still lays
  out (equals the `break_by_width` result), never panics, never returns empty for non-empty input.
- **Deterministic:** the same `(text, max_width_pt, size_pt, metrics)` yields identical lines across
  runs (tie-break rule pinned above).
- `lay_out` uses `break_paragraph`; the full workspace is green (`fmt`,
  `clippy --all-targets --all-features -D warnings`, `build`, `test`).
- Export output remains valid under the existing **Ghostscript golden-file gate** (drawn content
  still left-aligned; only line grouping/pagination may differ — regenerate the golden if breakpoints
  move).

## Non-goals (named follow-up increments)

- **Justified rendering (increment 2 of this spec).** Carry each line's adjustment ratio out of
  layout and have the writer stretch/shrink inter-word space (word-spacing operator, e.g. `Tw`, or
  positioned `TJ`) so justified lines fill the frame; last line of a paragraph stays ragged. This is
  the increment that emits a new PDF operator and regenerates the golden. Alignment mode
  (justified vs. ragged-left/right/centered) selection is part of this step.
- **Hyphenation** — intra-word breakpoints (hyphenation penalties in the item stream) and breaking
  oversized single words. Its own spec; KP's penalty slot is designed for it but it is not populated
  here.
- **Optimal breaking across threaded frames / columns / pages** — KP here optimizes one paragraph
  against one frame width; multi-column and cross-frame optimization ride with text threading later
  in M1.
- **Leading from font metrics** — line height stays `BODY_LINE_HEIGHT_PT`.
- **`\looseness` / forced line counts, complex-script/bidi reordering** — increment 1 is LTR,
  single default script, no looseness control.

## Performance note

Total-fit with active-node pruning (drop nodes whose partial line is already infeasible, `r < −1`)
is near-linear in the number of words per paragraph — well within budget for body paragraphs. KP runs
**per paragraph**, so it composes with the incremental/dependency-tracked layout engine (M1): only a
reflowed paragraph's break run is recomputed, never the whole document. The 500-page perf gate
applies once the perf harness lands; the algorithm's per-paragraph scope keeps it local.
