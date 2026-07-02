# Plan — Spec 0017 increment 2: justified rendering

## Goal
Land increment 2 of spec 0017: carry each line's justification adjustment out of layout and
have the writer stretch/shrink inter-word space so justified lines fill the frame. Last line of a
paragraph stays ragged. This is the increment that emits a new PDF operator (positioned `TJ`) —
`Tw` is unusable here because the font is a Type0/Identity-H composite (2-byte codes), and PDF word
spacing applies only to single-byte code 32.

## Acceptance (from spec 0017)
- Justified interior lines fill the frame exactly (natural width + Σ per-space adjust == L).
- Last line of a paragraph and single-word lines stay ragged (adjust 0).
- Alignment mode selection: Body → Justified, Heading → Left (ragged).
- Greedy fallback (an over-wide word) renders fully ragged — never over-shrinks.
- Deterministic; workspace green (fmt/clippy/build/test); GS well-formedness gate still passes.

## Design / files
1. `crates/text-layout/src/lib.rs`
   - `pub enum Alignment { Justified, Left }`.
   - `pub struct Line { pub text: String, pub space_adjust_pt: f32 }` — points to ADD to each
     inter-word gap. Positive = stretch, negative = shrink, 0 = ragged.
   - `pub fn justify_paragraph(text, max_width_pt, size_pt, align, metrics) -> Vec<Line>`:
     breakpoints from `break_paragraph` (unchanged), then resolve per-line adjustment.
     Because all inter-word glues are identical, the resolved per-gap add is simply
     `(L - W) / spaces` for every justified non-last line (the adjustment ratio in point form).
     Fallback (any word wider than L) → all ragged. `break_paragraph` stays the parity oracle.
2. `crates/layout-engine/src/lib.rs`
   - `PlacedBlock::Text.lines: Vec<String>` → `Vec<Line>`.
   - `lay_out` calls `justify_paragraph`, Body=Justified, Heading=Left.
3. `crates/export-pdf/src/writer.rs`
   - `render_page`: adjust==0 → current single `Tj` (unchanged bytes). Else positioned `TJ`:
     words re-joined with trailing space glyphs, `adjust = -1000 * space_adjust_pt / FONT_SIZE`
     between them (amount is subtracted, thousandths of text space — verified in pdf-writer 0.15).

## Tests
- text-layout: justified fills frame; last/single-word ragged; Left all-0; fallback all-0; determinism.
- writer: justified line emits a `TJ` with the expected adjustment sign/magnitude; ragged line unchanged.
