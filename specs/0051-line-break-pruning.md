# 0051 — Knuth-Plass active-node pruning

**Milestone:** M3 · **Status:** implemented

## Why

Spec 0027's harness measured it on its first run and `benches/budgets.toml` has pinned it ever
since: an 8× longer paragraph cost **~36×** the time, where linear is 8× and fully quadratic 64×.
Knuth-Plass is a linear-time algorithm in practice precisely because it *prunes* — a start that can
no longer reach the current position inside the measure is retired, never rescanned — and
`break_paragraph_hyphenated` did no pruning at all. It scanned `for s in 0..=e` for every
breakpoint `e`, and cloned a growing `Vec<usize>` of line starts into every one of those candidates.

Two costs, one shape: `O(items²)` candidates × `O(lines)` words copied each.

### Today's behaviour, measured before the fix

Release build, reference machine for this increment, `break_paragraph_hyphenated` at the default
6×9in body measure (432 pt) over generated corpus prose:

| words | time | per doubling |
|---|---|---|
| 250 | 0.50 ms | — |
| 500 | 1.80 ms | ×3.59 |
| 1,000 | 8.96 ms | ×4.97 |
| 2,000 | 50.8 ms | ×5.68 |
| 4,000 | 345 ms | ×6.06 |
| 8,000 | 2,091 ms | ×6.06 |
| **20,000** | **49,750 ms** | — |

A doubling costing ×6 is an empirical exponent of ~2.6 — worse than quadratic, because of the
per-candidate `Vec` clone. The number that matters is the last row: **one paragraph of 20,000 words
took 49.7 seconds**. That is not a slow benchmark, it is a hung application, and spec 0043's
importer makes such a paragraph easy to produce by accident — a stat block or a table flattened into
one run of prose is exactly the shape this product's users paste in.

Low severity for the 30–90-word paragraphs a real book is made of, which is why 0027 recorded it
rather than fixing it. It is a genuine cliff everywhere else.

## What

Both changes are local to `break_paragraph_hyphenated` (crates/text-layout/src/lib.rs:231). The item
stream, the badness/demerit model, the tie-break rule, the fallback and the reconstructed strings are
all untouched. **No output moves.**

### 1. Retire starts that can never reach the current end (crates/text-layout/src/lib.rs:436-465)

`base_demerits` (crates/text-layout/src/lib.rs:303) rejects a line whose adjustment ratio is below
−1 — over-wide even at full shrink. Written out, that rejection is exactly

```
(natural − shrink) > l        where natural = Σw + extra_w,  shrink = Σz
```

and both sums are **monotone non-decreasing in the line's end**, because every item contributes
`width − shrink ≥ 0`: a box `w ≥ 0`, a glue `g − g/3 > 0`, a penalty `0`. The hyphen width
`extra_w ≥ 0` only ever adds. So if start `s` is already too far back for end `e`, it is too far back
for every end after `e`, for either kind of break. It can be retired for good.

The active set is therefore a contiguous window `[s_lo, e]` with a monotone lower bound:

```rust
let tightest = |s, e| (wsum[e] - zsum[e]) - (wsum[s] - zsum[s]);
while s_lo < e && tightest(s_lo, e) > l { s_lo += 1; }
```

`s_lo` advances at most `n_items` times over the whole paragraph, so the candidate scan drops from
`O(items²)` to `O(items × window)`, the window being how many items fit on one line. The terminal
line's scan starts at `s_lo` for the same reason.

This removes only transitions `base_demerits` would have rejected. Nothing that could have won is
lost.

### 2. A back link instead of a cloned start sequence (crates/text-layout/src/lib.rs:341-433)

A DP node carried `starts: Vec<usize>`, cloned per candidate. It now carries `back: usize` — the
item index its last line starts at, which is also the index of the node that line extends (a node is
named by where its *next* line starts). The winning sequence is materialized once, at the end, by
walking the chain (crates/text-layout/src/lib.rs:518). `Node` becomes `Copy`; the DP allocates
nothing per candidate.

The subtle part is the **tie-break, which is preserved exactly**. Spec 0017 settles equal demerits
and equal line counts by the lexicographically earliest start sequence, so that identical input
yields identical lines. With the sequence no longer materialized, comparing two of them means
comparing two back-chains — which decomposes, since `path(t) = path(back(t)) ++ [back(t)]`:

```
cmp(t1, t2) = cmp(back(t1), back(t2))          if that is not Equal
            = back(t1) vs back(t2)             otherwise
```

Only equal-length paths are ever compared (the `lines` test runs first), and every node the walk
touches was finalized at an earlier `e`, so its winner can no longer move. The comparison is
iterative rather than recursive — a 20,000-word paragraph is thousands of lines deep — and
**memoized on the node pair**, because a corpus of equal-width words ties constantly and an
unmemoized `O(lines)` walk per tie would reintroduce the quadratic this increment removes.

### Not done

No cap on the active set and no threshold retry. Classic Knuth-Plass adds those for the case where
pruning leaves no feasible breakpoint at all; here that case already has an answer — the greedy
`break_by_width` fallback (spec 0018) — and a threshold retry would *change output*, which this
increment's central acceptance criterion forbids. Pruning that provably cannot change a break is the
whole scope.

## Acceptance criteria

- **Output is unchanged for every paragraph in the test corpus.** The 500-page synthetic document's
  3,078 text blocks, broken at four measures (432 / 198 / 96 / 54 pt) with and without a dense stub
  hyphenator — 690,915 lines — digest to `0xa013fe79a5d8e97e`, recorded from the pre-0051 breaker
  and asserted, not recomputed (crates/testdoc/tests/line_break_equivalence.rs). ✅ holds exactly.
- `Document::sample()` export byte-hash unchanged: 8,786 bytes,
  `081cb93720d272cb54ad0a8ebf8b7912c1db777e6933bfda484a0ec906ae25b0`, before and after. ✅
- The scaling ratio improves measurably and the new value is pinned, with both numbers legible in
  `benches/budgets.toml`: **35.8× recorded → 34.2× re-measured on this machine → 8.3× after**, i.e.
  linear to within noise. Pinned at 12.0 (was 40.0). ✅
- The pathological input completes within a stated wall-clock bound rather than being untestable:
  one paragraph of 20,000 words is a bench case with a budget of 25 ms.
  **49,750 ms → 4.4 ms, a factor of ~11,000.** ✅
- No behaviour change when a paragraph has no feasible breaking: the greedy fallback still fires,
  asserted by an unbreakable-long-word case in both crates.
- Every existing `quill-text-layout` test passes unchanged — including the determinism, ceiling-
  badness and double-hyphen tests, which are the ones that exercise the tie-break.

## Test strategy

**Corpus equivalence first, and it gates everything else.** The test
(crates/testdoc/tests/line_break_equivalence.rs) was written and pinned against the *unmodified*
breaker before a line of the fix existed, which is the only ordering under which it can prove
anything. Four measures, because pruning's effect depends entirely on how many items fit in a line:
432 pt is what `lay_out` really uses, 198 pt is spec 0036's two-column `rulebook`, 96 pt makes
hyphenation load-bearing, and 54 pt is narrower than most corpus words and so drives the
no-feasible-breaking fallback. The hyphenator is a stub that breaks every third character rather
than `hypher` — denser penalties than en-US, and no dependency edge from `quill-testdoc` to
`quill-export-pdf`.

Beyond the committed test, the change was checked against an **adversarial tie-dense corpus** —
paragraphs of one repeated word (2 to 1,500 words, six word widths) and of two alternating widths,
broken at eight measures with and without hyphenation, 426,655 lines — dumped from the pre-change
and post-change builds and compared with `cmp`. Identical. Repeated identical words make every
breaking of the same line count cost the same demerits, so the lexicographic tie-break decides the
answer alone: this is the corpus that would catch a tie-break rewrite that is merely *plausible*.
It is not committed because the committed digest covers the same code path at less than a sixth of
the runtime.

The bench (crates/testdoc/benches/line_breaking.rs) then measures the ratio and the 20,000-word
case. The pathological case is timed once rather than through `min_of`: the budget is a hang
detector with three orders of magnitude of headroom, so noise is irrelevant and repeats would only
make the bench slower.

### Re-deriving the digest when the corpus moves

The digest is not a magic number and may not be pasted from a failing run. It is re-derived by
checking out `text-layout`'s **pre-pruning** source, running the test, taking the value it reports,
then restoring pruning and confirming the value is unchanged. Only that sequence proves anything.

It has been re-derived once. Specs 0044-0046 pack columns tighter, and `quill-testdoc` sizes its
document by *measuring* to ~500 pages, so reaching that target now takes more blocks: 3,078
paragraphs / 690,915 lines became 3,251 / 730,328. The corpus changed; line breaking did not. The
guard assertions on paragraph and line count are what made that distinction visible instead of
letting a digest mismatch read as "pruning broke something" — or worse, letting someone re-record
the digest from the post-pruning build and prove nothing at all.

## Risks

- **Pruning that is a hair too aggressive changes line breaks in rare paragraphs**, which changes
  page breaks, which changes the export hash — a silent typographic regression that the perf bench
  would report as a *success*. Corpus equivalence is the only thing standing in front of it. The
  structural mitigation is that the prune condition is not a heuristic: it is `base_demerits`'s own
  rejection, restated in a form whose monotonicity is provable from the sign of each item's
  `width − shrink`.
- **The tie-break is the part most likely to be subtly wrong**, because it was a whole-sequence
  comparison and is now a chain walk. Equal-demerit ties are common with monospace metrics and near
  universal in the adversarial corpus, which is why that corpus was run rather than reasoned about.
- **The memo could in principle grow large** on a pathological tie-dense paragraph. It is bounded by
  the number of distinct node pairs actually compared, which the active window bounds per `e`; the
  20,000-word case is the measurement that says it stays cheap in practice (4.4 ms total).
- `us_per_paragraph` improved ~4× as a side effect but its ceiling is deliberately *not* re-pinned:
  64.2 was recorded on the reference machine and this increment was measured on a faster one, so
  re-pinning would import a different machine's speed into the gate. The ratio carries the
  improvement instead, because a ratio is machine-independent.
