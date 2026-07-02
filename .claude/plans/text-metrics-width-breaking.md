# Plan — spec 0015 increment 1: real width-based line breaking

## Goal
Replace the fixed `APPROX_CHAR_WIDTH_PT = 6.0` character-count line breaker with real
width-based greedy breaking driven by the embedded font's actual glyph advances, wired
end-to-end through the export pipeline. Closes the "text overflows the frame" gap.

## Architectural decision
Font metrics are **supplied to the layout engine by the export caller** via a `CharMetrics`
trait (defined in the lowest crate, `text-layout`). The font stays **out of `Document`** for
M0 — no schema change. `export-pdf` builds the embedded font once (from the document's chars),
implements `CharMetrics` for it, and passes it to both `lay_out` and `write_pdf`. This
decouples layout from any font crate (no `export-pdf → layout-engine → export-pdf` cycle) and
defers font-in-`Document` (needed for interactive/screen layout) to M1.

## Changes
1. **text-layout**: add `BODY_FONT_SIZE_PT`/`BODY_LINE_HEIGHT_PT` shared consts, a `CharMetrics`
   trait (`advance_pt(ch, size_pt) -> f32`), a `MonospaceMetrics` stub (tests/fallback), and
   `break_by_width(text, max_width_pt, size_pt, &metrics)`. Remove `greedy_break`.
2. **layout-engine**: `lay_out(doc, &impl CharMetrics)` uses `break_by_width` at
   `BODY_FONT_SIZE_PT`, height from `BODY_LINE_HEIGHT_PT`. Drop the two `APPROX_*` consts.
   Update tests to pass `MonospaceMetrics`.
3. **export-pdf**: add `quill-text-layout` dep; `impl CharMetrics for EmbeddedFont` (advance =
   `widths[char_to_gid[ch]] * size / 1000`); `export()` collects doc chars, builds the font
   once, passes `&font` to `lay_out` and `write_pdf`; `write_pdf` takes `&font` (drop internal
   build + `collect_chars`); render_page uses the shared `BODY_*` consts.

## Deferred (named follow-up increments of spec 0015)
Knuth-Plass optimal/justified breaking; hyphenation; shaping/kerning/ligatures (rustybuzz);
breaking oversized single words; leading derived from font metrics; font-in-`Document` for
interactive layout.

## Validation
`cargo fmt --check`, `cargo clippy --all-targets --all-features -- -D warnings`,
`cargo build`, `cargo test` (all crates). New unit tests: width breaking picks the fit count,
an over-wide word is placed alone, empty text yields no lines, `MonospaceMetrics` matches the
old 6pt-per-char behavior at 10pt with `em_ratio = 0.6`.
