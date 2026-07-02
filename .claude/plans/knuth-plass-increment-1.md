# Plan — spec 0017 incr.1: Knuth-Plass optimal line breaking (ragged output)

## Goal
Add `break_paragraph` (Knuth-Plass total-fit) to `text-layout`, over a box/glue model on the
spec-0016 `RunMetrics` seam, returning the same `Vec<String>` shape as `break_by_width`. Wire
`layout-engine::lay_out` to use it. Rendering stays ragged/left-aligned — the only output change is
which words land on which line. `break_by_width` is retained as fallback + parity oracle.

## Acceptance criteria (from spec 0017)
- Optimal beats greedy on a crafted paragraph: exact lines == hand-computed optimum, total demerits
  strictly lower than greedy's.
- Degenerate parity with `break_by_width`: empty/whitespace → no lines; single over-wide word →
  alone; all-fits → one line.
- No-feasible-breaking fallback: paragraph with an unbreakable over-wide word lays out (== greedy),
  never panics/empty.
- Deterministic tie-break: fewest lines, then lexicographically earliest break-index sequence.
- `lay_out` uses `break_paragraph`; workspace green (fmt/clippy -D warnings/build/test).
- Ghostscript golden gate still valid (regenerate golden if breakpoints move).

## Model
- Box = word, width `measure_run(word, size)`. Glue = one space, natural `g = measure_run(" ", size)`,
  stretch `g/2`, shrink `g/3`. Line natural width additive: `Σ box + Σ glue`.
- Adjustment ratio `r`: `Y=Z=0 → 0`; `W≤L → (L−W)/Y` (or 0 if Y=0); `W>L → (L−W)/Z` (infeasible if Z=0).
- Feasible iff `r ≥ −1`. Badness `100·|r|³` clamped to 10000. Demerits `(LINE_PENALTY + badness)²`,
  `LINE_PENALTY = 10`. Last line: badness 0 when it fits (W≤L).
- Total-fit DP over word prefixes; tie-break (demerits, fewest lines, lexicographically earliest
  line-start sequence). No feasible full breaking → fall back to `break_by_width`.

## Files
- `crates/text-layout/src/lib.rs` — add `LINE_PENALTY`, `break_paragraph`, DP + tests.
- `crates/layout-engine/src/lib.rs` — swap `break_by_width` → `break_paragraph` in `lay_out`.

## Test strategy
- Crafted optimal-beats-greedy paragraph under `MonospaceRunMetrics{em_ratio:0.6}` (additive, so the
  test can recompute demerits exactly to assert KP < greedy).
- Parity/fallback/determinism unit tests mirroring the `break_by_width` degenerate suite.
- Existing export golden test re-run; regenerate golden only if breakpoints shift.
