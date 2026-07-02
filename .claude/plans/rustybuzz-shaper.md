# Plan — rustybuzz shaper (spec 0016 increment 1, part 2 of 2 / "1b")

## Task

Replace the export font's parity-stub `measure_run` (part 1a, per-char sum) with **real rustybuzz
shaping** — kerning + ligatures — behind the `RunMetrics` seam that part 1a built, plus the
kern-fixture acceptance test. Measurement only: no new PDF operators, drawn content unchanged.

## Decision — build the shaping context once (spec 0016 "Wiring")

`rustybuzz::Face<'a>` borrows the font bytes, so it can't be cached inside `EmbeddedFont`
(self-referential). Instead `EmbeddedFont` now **owns its original program bytes**, and a new
`ShapingContext<'a>` borrows the font, holds the `rustybuzz::Face` built **once**, and implements
`RunMetrics`. `export()` builds it (`font.shaper()`) and passes it to `lay_out` — mirroring spec
0015's build-once metrics pattern. The old `impl RunMetrics for EmbeddedFont` (per-char sum) is
removed; `CharMetrics`/`advance_pt` stays (still the fallback + the per-char test seam).

Probe (out-of-tree, scratchpad): the bundled Source Serif 4 kerns `AV` by −119/1000 em and forms
the `fi` ligature — so the acceptance criterion is met by the **bundled** font; no fixture needed.

## Acceptance criteria (from spec 0016)

- `ShapingContext::measure_run` shapes `text` LTR with `rustybuzz` (default script/lang), sums
  `x_advance`, scales `size_pt / units_per_em` → points.
- A run with a real negative kern (`AV`) measures **strictly narrower** under shaping than the
  spec-0015 per-char sum of the same chars. New test in `fonts.rs` using the bundled font.
- A single-glyph run measures **equal** to `advance_pt` of that char (no kern/ligature possible) —
  parity anchor replacing part-1a's whole-string equality test (now false for kern pairs).
- `MonospaceRunMetrics` parity (text-layout) unchanged; `break_by_width` still greedy.
- `rustybuzz` added as a permissive (MIT/Apache-2.0) dep, resolving ttf-parser to the **single**
  0.25 version subsetter pins (verified via `cargo tree -d`).
- Full workspace green (`fmt`, `clippy -D warnings`, `build`, `test`); Ghostscript golden gate
  still passes (drawn bytes unchanged; only line grouping/pagination may shift).

## Files to touch

- `Cargo.toml` — add `rustybuzz = "0.20"` to `[workspace.dependencies]` (0.20 → ttf-parser 0.25.1).
- `crates/export-pdf/Cargo.toml` — depend on `rustybuzz`.
- `crates/export-pdf/src/fonts.rs` — store `program: Vec<u8>` on `EmbeddedFont`; add
  `ShapingContext` + `EmbeddedFont::shaper`; replace the `RunMetrics` impl; swap the part-1a
  equality test for the single-glyph parity + `AV` kern tests.
- `crates/export-pdf/src/lib.rs` — `export()` builds `font.shaper()` and passes it to `lay_out`.
- `specs/0016-rustybuzz-shaping.md` / `specs/README.md` — mark increment 1 (both PRs) complete →
  spec `implemented`; note the shaper lives in export-pdf (rustybuzz dep is there, not text-layout).

## Test strategy

- `shaped_kern_pair_is_narrower_than_per_char_sum` — the acceptance test (bundled font, `AV`).
- `single_glyph_run_equals_advance_pt` — parity anchor.
- Whole-workspace `cargo test` confirms export golden bytes are undisturbed (drawn content path
  untouched; only measurement changed).
- `cargo tree -d` to confirm no duplicate ttf-parser.
