# 0027 — Performance harness: synthetic 500-page document, benches, CI budget gate

**Milestone:** M1 · **Status:** implemented

## Why

`CLAUDE.md` states the constraint the entire architecture is built around: *500 pages, art-heavy,
must stay smooth. The primary competitor is documented to collapse on long docs. Performance is a
feature, benchmark-gated in CI.*

None of that was measured. `cargo bench` was documented as the M1 gate but was a **silent no-op** —
no `benches/` directory, no `[[bench]]` target, no benchmark of any kind in the workspace. The
project's central claim was unfalsifiable.

This lands the measuring stick **before** the layout work it exists to gate. Master pages, persisted
frames and incremental layout each change the engine's hot path; without a baseline first, each one
would ship on the assumption it was fast, and the milestone would end with an unverifiable
performance claim.

## What

### `quill-testdoc`

A workspace-internal crate generating deterministic synthetic documents, plus the bench targets that
measure against them. Nothing depends on it, which is what keeps the dependency edge one-way: the
benches live *here* rather than in `layout-engine`, so `layout-engine` never needs a dev-dependency
on its own consumer.

**The page count is measured, not assumed.** A spec asks for *500 pages*, not for a block count.
Those differ: leading, margins, hyphenation and frame geometry all change how many pages a fixed
block count fills. A hardcoded count would quietly stop being a 500-page workload the first time any
of those changed — and the benchmark would keep printing numbers as if nothing had happened.
`synthetic_document` lays its candidate out and converges on the requested page count, bounded to 24
iterations so a non-monotonic relationship fails loudly instead of hanging CI.

Determinism comes from a small xorshift PRNG rather than `rand`: a dependency for eight lines of
arithmetic, and one whose stream is not guaranteed stable across versions — which would silently
change the workload under the benchmark.

The harness measures with `MonospaceRunMetrics`, not the real shaped font, so the layout benchmark
measures layout rather than `rustybuzz`.

### Bench targets

Hand-rolled `std::time::Instant`, `harness = false, test = false`. No criterion: ~30 transitive
crates against the workspace's minimal-graph rule, and `test = false` keeps `cargo test` from
running benches on every leg of the three-OS matrix for no signal.

| Target | Measures |
|---|---|
| `layout` | `lay_out` ms/page at 500 pages; the 500-vs-250 scaling ratio |
| `line_breaking` | Knuth-Plass µs/paragraph; cost of an 8× longer paragraph |
| `proxy_cache` | Cold proxy generation ms/image; warm re-populate speedup |

Timings take the **minimum** of N runs, not the mean: every source of noise on a shared runner only
ever adds time, so the minimum is both the better estimate of true cost and far more stable.

### The gate

`benches/budgets.toml`, checked with a `tolerance_factor` of 2.0 — a **blowup detector, not a
micro-regression detector**. Shared runners vary 10–30% and `rust-toolchain.toml` pins the floating
`stable` channel, so a tight absolute ceiling would fail on unrelated noise, get raised until it
stopped failing, and then measure nothing. The load-bearing entries are the *ratios*: a quadratic
scan reintroduced into layout changes a ratio by far more than any noise threshold.

**The CI step folds into the existing Linux job.** A new job emits a new check-run, and a new
check-run is not automatically a required branch-protection context — it could fail silently while
PRs kept merging. Running inside an existing job makes the gate real without an out-of-band admin
change, and leaves the macOS/Windows legs untouched.

## Measured baseline

Release build, reference machine:

| Measurement | Value |
|---|---|
| `lay_out`, 500 pages | 238 ms — **0.46 ms/page** |
| Layout scaling, 500 vs 250 pages | **1.94×** (linear would be 2.0) |
| Knuth-Plass per paragraph | **64 µs** |
| Proxy generation, 1200×900 | **4.9 ms/image** |
| Proxy re-populate, unchanged | **~8300× faster than cold** |

Two of these are worth stating plainly. Layout is genuinely linear in document length — the 500-page
target is not in danger from the engine's shape. And spec 0024's incremental proxy invalidation is
not merely working, it is enormous: re-populating unchanged art costs essentially nothing.

## Finding: Knuth-Plass is superlinear in paragraph length

The harness caught something on its first run. An 8× longer paragraph costs **35.8×** the time.
Linear would be 8×, fully quadratic 64× — so the line breaker is behaving closer to O(n²) than to
O(n). The textbook algorithm keeps this near-linear by pruning active nodes that can no longer lead
to a feasible break; that pruning appears to be missing or ineffective.

**Not fixed here, deliberately.** This increment lands the measuring stick; changing the line breaker
in the same commit would mean changing the thing measured and the measurement together, and neither
result would be trustworthy.

Severity in practice is low — real paragraphs run 30–90 words, where absolute cost is ~64 µs — but
it is a genuine cliff for pathological input, such as a stat block or table flattened into a single
very long paragraph, which is a plausible thing for this product's users to produce. The budget
records today's reality so a *further* regression is still caught; it is not an endorsement. Tracked
in `docs/roadmap.md`.

That this was found by writing the benchmark, in an area everyone assumed was fine, is the argument
for landing the harness before the work it gates rather than after.

## Acceptance criteria

- [x] `synthetic_document` produces 495–505 pages for a 500-page target, and hits a 250-page target too (the scaling check compares both, so the generator must be right at both sizes).
- [x] Same seed ⇒ byte-identical document across calls; different seeds ⇒ different documents (guards a seed that is accepted and then ignored).
- [x] The generated document is art-heavy (>100 images) with an asset per image, matching the workload the perf target is stated against.
- [x] Generated blocks carry ids and round-trip through JSON, so benches measure the code path real documents take.
- [x] `cargo bench -p quill-testdoc` builds, prints ms/page, and exits 0 when budgets are met — non-zero when not (verified by an actual violation during development).
- [x] `cargo test --workspace --all-features` does **not** execute the bench targets.
- [x] `cargo clippy --all-targets --all-features -- -D warnings` is clean, benches included.
- [x] The CI gate runs in the existing Linux-only job; the OS matrix cost is unchanged.
- [x] No new runtime dependency and no `[dev-dependencies]`; the only new crate is workspace-internal `quill-testdoc`.
- [x] `CLAUDE.md`'s `cargo run -p cli` corrected to `-p quill-cli` (no package named `cli` has ever existed) and its `cargo bench` line now describes what exists.

## Non-goals

- Fixing the Knuth-Plass scaling finding (above).
- Comparing against a stored historical baseline. `Swatinem/rust-cache` does not persist bench
  results between runs, which is why budgets are committed to the repo instead.
- Benchmarking export. It is already gated externally by the Ghostscript job, and its cost is
  dominated by compression rather than by anything M1 changes.
- Any absolute wall-clock assertion in CI beyond the 2× blowup ceiling. A stricter gate needs
  dedicated runners.
