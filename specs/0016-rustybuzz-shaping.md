# 0016 — Rustybuzz text shaping (kerning/ligature-aware measurement)

- **Milestone:** M1
- **Status:** in-progress (increment 1 of a multi-part spec; itself landing in two PRs)
- **Crates:** `quill-text-layout` (owner), `quill-layout-engine`, `quill-export-pdf`

> **Increment 1 lands in two PRs** (per CLAUDE.md "one atomic change per plan/turn"):
> **1a — measurement seam (parity, no dependency):** introduces the `RunMetrics` trait +
> `MonospaceRunMetrics` stub, rewrites `break_by_width` to measure whole candidate lines through
> `measure_run`, and wires `lay_out`/`export` through `RunMetrics`. The export font implements
> `RunMetrics` as the per-char sum of its advances, so output is byte-identical to spec 0015.
> **1b — rustybuzz shaper:** adds `rustybuzz` and replaces the export font's `measure_run` body with
> real shaping (kerning/ligatures), plus the kern-fixture acceptance test below. The acceptance
> criteria that name `rustybuzz` and the kern pair are satisfied by **1b**.

## Problem

Line breaking (spec 0015) measures a line as the **sum of per-char nominal advances**
(`CharMetrics::advance_pt(ch, size)` over each `char`). That ignores everything a shaper does:
**kerning** (the `AV` pair is tighter than `A`+`V` in isolation), **ligatures** (`fi` is one glyph,
not two advances), and the fact that a glyph's advance depends on its neighbours, not just its
codepoint. For press-quality justification the measured width of a run must be the width the run will
actually occupy when drawn — otherwise justified lines are subtly wrong and the M1 Knuth-Plass work
built on top inherits the error.

The pure-Rust stack already names the tool: `rustybuzz` (a HarfBuzz port, permissive-licensed) does
the shaping. It is a listed `text-layout` responsibility in `CLAUDE.md` but is not yet wired in.

## Architectural decision (measure the run, keep the seam)

Shaping measures **runs, not characters**: the unit is a shaped run (a string + font + size), and its
width is the sum of the shaped glyph advances rustybuzz returns. So this increment introduces
**run measurement** alongside the existing per-char `CharMetrics`, without disturbing the export path:

- **New `RunMetrics` capability in `text-layout`** — `measure_run(text, size_pt) -> f32` returns the
  shaped advance width of `text` in points. `break_by_width` measures candidate lines with
  `measure_run` instead of summing `advance_pt`. The per-char `CharMetrics` trait stays (it is the
  fallback/stub seam and is still what a monospace test double implements).
- **`rustybuzz`-backed implementation lives where the font bytes already are** (`export-pdf`, which
  owns `EmbeddedFont`) — it builds a `rustybuzz::Face` from the same font bytes it embeds and shapes
  with it. `text-layout` defines the trait; `export-pdf` implements it; `layout-engine` measures
  through it. This preserves the existing acyclic seam (`export-pdf → layout-engine → text-layout`),
  exactly as spec 0015 did for `CharMetrics`.
- **Measurement only — output is unchanged this increment.** The PDF content stream still draws text
  the way it does today (per-glyph from the subset). Carrying *shaped glyph ids and positions* into
  the content stream — and the **shaping-GID (original font space) ↔ subset-GID (subsetter remaps)
  reconciliation** that requires — is deferred to a named follow-up. Measurement can improve line
  breaking with zero risk to press output; wiring shaped glyphs into output is the riskier, separable
  step.

## Behavior (increment 1)

### `RunMetrics` (text-layout)

```rust
pub trait RunMetrics {
    /// Total shaped advance width of `text` at `size_pt`, in points.
    fn measure_run(&self, text: &str, size_pt: f32) -> f32;
}
```

- A `MonospaceRunMetrics { em_ratio }` stub (`width = em_ratio * size_pt * text.chars().count()`)
  is provided for deterministic tests and as a no-shaper fallback — it reproduces the pre-shaping
  per-char sum, so a run with no kerning pairs measures identically under stub and shaper.
- `export-pdf` implements `RunMetrics` for its shaping context by shaping `text` with a
  `rustybuzz::Face` (LTR, default script/language for increment 1) and summing
  `glyph_position.x_advance`, scaled from font units to points by `size_pt / units_per_em`.

### `break_by_width` (text-layout)

`break_by_width(text, max_width_pt, size_pt, metrics: &impl RunMetrics)` keeps its greedy word-based
shape from spec 0015, but the width test becomes `measure_run(candidate) <= max_width_pt`, where
`candidate` is the whole prospective line (so kerning/ligatures across the line are captured, not just
within a word). Same edge behavior as 0015: single over-wide word emitted alone; whitespace
normalized to single spaces; empty text → no lines.

### Wiring

- `layout-engine::lay_out(doc, metrics: &impl RunMetrics)` measures with `break_by_width` as above.
- `export-pdf::export` builds the shaping context (the `rustybuzz::Face` over the embedded font bytes)
  **once** and passes it to both `lay_out` and `write_pdf`, mirroring spec 0015's build-once pattern.
  `EmbeddedFont` construction and the drawn content stream are unchanged.

## Inputs / outputs

- **Input:** a `Document` plus a `RunMetrics` implementation (the rustybuzz-backed shaper in the
  export path; the monospace stub in tests).
- **Output:** `Vec<LaidOutPage>` whose lines are broken against **shaped** widths. Export output
  bytes are structurally unchanged from spec 0015 except where kerning/ligatures move a word to a
  different line (and thus shift pagination). No new PDF operators are emitted this increment.

## Acceptance criteria

- With a font that has a real negative kern (e.g. the bundled test font's `AV`/`VA` pair), a run
  containing that pair measures **strictly narrower** under the rustybuzz `RunMetrics` than the
  spec-0015 per-char sum of the same characters. (Fixture per `CLAUDE.md`'s out-of-tree convention if
  a kern-carrying font isn't already in the tree.)
- Under `MonospaceRunMetrics`, `break_by_width` reproduces spec 0015's greedy breaks exactly
  (behavior parity for the stub — no shaping means no change).
- `break_by_width` measures the whole candidate line (kerning across word boundaries counts), breaks
  before the first word that would exceed the measured max, emits a single over-wide word alone, and
  yields no lines for empty text.
- `lay_out` and `export` compile against `RunMetrics`; the full workspace is green (`fmt`,
  `clippy -D warnings`, `build`, `test`). `rustybuzz` is added to `text-layout`/`export-pdf` as a
  permissive (MIT/Apache-2.0-compatible) dependency.
- Export output remains valid under the existing Ghostscript golden-file gate (drawn content
  unchanged; only line grouping/pagination may differ).

## Non-goals (named follow-up increments)

- **Shaped glyphs into output** — emitting shaped glyph ids/positions in the content stream, and the
  **shaping-GID ↔ subset-GID reconciliation** the subsetter's remap requires. This increment measures
  with shaping but still draws as before.
- **Knuth-Plass** optimal/justified breaking + **hyphenation** (the next quality upgrade after
  shaping-aware measurement lands; still greedy here).
- **Complex scripts / bidi** — RTL, Arabic/Indic shaping, script/language itemization. Increment 1 is
  LTR, single default script.
- **Leading from font metrics** — line height stays the spec-0015 `BODY_LINE_HEIGHT_PT` constant.
- **Font in `Document`** for interactive/on-screen layout — still passed in by the export caller.
