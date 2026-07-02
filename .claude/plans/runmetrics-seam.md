# Plan — RunMetrics measurement seam (spec 0016 increment 1, part 1 of 2)

## Task

Introduce the **run-based** measurement seam that spec 0016 requires, at **parity** — no
`rustybuzz`, no new dependency, byte-identical export output. This is the smaller, lower-risk half
of spec 0016 increment 1; the `rustybuzz`-backed `RunMetrics` impl + kern fixture test lands in the
follow-up PR.

Rationale (CLAUDE.md "one atomic change per plan/turn"): swapping the measurement seam from per-char
(`CharMetrics`) to per-run (`RunMetrics`) is a self-contained, reviewable refactor that changes no
output. Wiring the actual shaper is a separable, riskier step (new dep, GID space, fixture).

## Acceptance criteria

- New `RunMetrics` trait in `text-layout`: `fn measure_run(&self, text: &str, size_pt: f32) -> f32`.
- New `MonospaceRunMetrics { em_ratio }` stub: `width = em_ratio * size_pt * text.chars().count()`,
  reproducing the pre-existing per-char monospace sum (behavior parity).
- `break_by_width` takes `&impl RunMetrics` and measures the **whole candidate line**
  (`measure_run(current + " " + word) <= max_width_pt`), so cross-word kerning will be captured once
  a real shaper is wired. Same edge behavior as spec 0015: single over-wide word emitted alone;
  whitespace normalized to single spaces; empty text → no lines.
- `CharMetrics` + `MonospaceMetrics` **stay** (spec: still the fallback/per-char seam; still what the
  export font implements and what `advance_pt` tests exercise).
- `layout-engine::lay_out(doc, &impl RunMetrics)`; its tests use `MonospaceRunMetrics`.
- `export-pdf`'s `EmbeddedFont` implements `RunMetrics` by summing its own per-char advances
  (delegating to the existing `CharMetrics`/`advance_pt`), so measured widths — and therefore line
  breaks, pagination, and PDF bytes — are **identical to today**.
- Full workspace green: `fmt`, `clippy -D warnings`, `build`, `test`. No new dependency added.
- Existing Ghostscript golden gate unaffected (output unchanged).

## Files to touch

- `crates/text-layout/src/lib.rs` — add `RunMetrics` + `MonospaceRunMetrics`; rewrite
  `break_by_width` on `measure_run`; drop the now-unused private `measure` helper; update/extend
  tests to the new stub; keep `CharMetrics`/`MonospaceMetrics` and their test.
- `crates/layout-engine/src/lib.rs` — `lay_out` bound `RunMetrics`; import + test stub swap.
- `crates/export-pdf/src/fonts.rs` — add `impl RunMetrics for EmbeddedFont` (sum of `advance_pt`);
  keep the `CharMetrics` impl; add a test that `measure_run` equals the per-char sum.
- `crates/export-pdf/src/lib.rs` — no call change (font now also implements `RunMetrics`); no edit
  expected beyond confirming it compiles.
- `specs/0016-rustybuzz-shaping.md` / `specs/README.md` — mark 0016 `in-progress`, note the 2-PR
  split of increment 1.

## Test strategy

- Reuse spec 0015's `break_by_width` tests verbatim (wrap boundary, oversized word, empty, all-fit,
  parity) under `MonospaceRunMetrics` — they must pass unchanged, proving parity.
- Add `MonospaceRunMetrics::measure_run` unit test (scales with char count and size).
- Add `EmbeddedFont::measure_run` == sum-of-`advance_pt` test in `fonts.rs`.
- Whole-workspace `cargo test` confirms export byte output is undisturbed (existing export golden
  assertions still pass).
