# 0044 — Block fragmentation

**Milestone:** M3 · **Status:** implemented

## Why

The pagination loop moves a block that does not fit **whole** to the next frame
(`crates/layout-engine/src/lib.rs:1320`). On a single-column page nothing shows. On spec 0036's
two-column `rulebook` template it leaves a hole at the foot of a column every time the next
paragraph is taller than the space left — which, for continuous prose in a ~150 pt measure, is most
paragraphs. That is the milestone's flagship template setting text badly.

Three increments have now wanted this. Spec 0038 promised stat-block splitting and descoped it;
spec 0039 promised table row breaking and header repetition and descoped both; and spec 0036's
ragged column feet were the first sighting. Building it inside any one of them would be the second
of three implementations.

The recorded objection is real and this spec's central job is to answer it. Splitting was described
as "measure this block for at most H points", which puts available height into `MeasureKey`
(`crates/layout-engine/src/session.rs:93-99`) — the same block against a half-full frame and an
empty one becomes two cache entries, thrashing the hot path spec 0031 exists to keep cold.

## What

### Splitting is a derivation over the cached measurement, not a second measurement

A paragraph broken by Knuth-Plass at width *W* yields a line list that **does not depend on how much
vertical space is available**. The optimal break is a function of the measure. Choosing where to cut
that list is therefore a pure function of the already-measured value plus a height, and needs no
cache entry of its own.

This is TeX's separation between breaking a paragraph into a vertical list (once, for a measure) and
`\vsplit` (cutting an already-built vertical list to a height), and quill adopts it deliberately.
`MeasureKey` is unchanged. A block that splits across four frames is measured **once** per distinct
frame width, exactly as one that does not.

The contract this rests on, stated so a later increment cannot violate it silently: **a block whose
measurement genuinely depends on the available height must return no break opportunities.** It may
not participate in fragmentation at all rather than quietly making the cache key wrong.

### The API

```rust
impl Measured {
    /// Heights of the items this measurement may be cut between, in order.
    /// `None` — this measurement is indivisible.
    fn break_items(&self) -> Option<Vec<f32>>;

    /// The largest legal cut whose fragment fits in `avail_pt`, or `None` if none is.
    fn cut_fitting(&self, avail_pt: f32) -> Option<usize>;

    /// Cut into a fragment of the first `at` items and a remainder of the rest,
    /// both fully measured at the same width, with their heights.
    fn split_at(&self, at: usize) -> Option<(Measured, f32, Measured, f32)>;
}
```

`Measured::Text` is the only splittable variant in this spec. Its items are lines; `Image` and
`Panel` return `None`. 0045 teaches `Panel` its rows and 0046 its sections; neither changes anything
here.

The heights are not a division of the block's height, because vertical space belongs to the ends of
a paragraph and not to its middle:

- fragment height = `space_before_pt + k * leading_pt` — the paragraph starts here, so its
  space-above applies; it does not end here, so its space-below does not.
- remainder height = `(n - k) * leading_pt + space_after_pt` — the converse.

A continuation is therefore *not* `total − fragment`, and `split_at` returns both heights rather
than letting a caller subtract.

### Widows and orphans

A paragraph may not leave one line behind or carry one line forward. `MIN_ITEMS_PER_FRAGMENT = 2`,
so legal cuts are `k ∈ [2, n-2]` and a paragraph of 3 lines or fewer never splits. This is a
typographic rule, not a nicety: a lone line stranded at the top of a column is the defect a reader
notices first, and a splitter without it makes the page worse in a new way while fixing the old one.

### The flow loop

The doesn't-fit branch tries the largest legal cut that fits before falling back to today's
move-whole behavior. `FlowState` (`crates/layout-engine/src/lib.rs:1175`) gains `split_at: usize` —
an **absolute** item offset into the current block, 0 at its start — so a page-boundary checkpoint
can sit mid-block and incremental relayout can resume from it.

A pending offset is applied by re-measuring the block at the new frame's width and taking the
remainder of `split_at(offset)`. Re-measuring rather than carrying the remainder value is what keeps
the existing "re-measure against each candidate frame" invariant intact, and it is what makes a
checkpoint restorable from six `Copy` fields rather than from a measured payload.

### Splitting requires the continuation frame to be the same width

The offset is an index into the item list *at the width the cut was decided at*. A frame of a
different width re-wraps to a different line list, against which that index means something else —
so the loop **only splits when the frame the remainder will land in has the same width** (within
0.01 pt), and moves the block whole otherwise.

Uniform-width columns are the case that exists: `Thread::columns` builds equal widths, and unequal
columns are a named non-goal of spec 0020. The guard costs one peek at the next frame and makes the
mechanism correct rather than approximately correct. Splitting across differing widths needs the
block re-broken for the second width, which needs the *block*, not its measurement — a genuinely
different design, recorded as a follow-up rather than approximated here.

### What does not change

- `MeasureKey`, and therefore the measurement cache's behavior.
- Image blocks, which move whole.
- A block that fits, which is placed exactly as today.
- A block that does not fit an **empty** frame, which is still placed and overflows rather than
  looping — the `frame_empty` guard is untouched.
- `Document::sample()`'s export bytes.

## Acceptance criteria

- Regression: `Document::sample()`'s export byte-hash unchanged; every existing layout, render,
  session and export test passes; the Ghostscript CI job stays green.
- **Conservation, document-wide.** Concatenating the lines of every `PlacedBlock::Text` whose
  `source` is block *B*, in page-then-y order, reproduces *B*'s unsplit line list exactly — asserted
  over a multi-page two-column document in which at least four blocks split. No line duplicated,
  lost or reordered. This is the assertion that catches the only failure that matters.
- A 20-line paragraph entering a 12-line column that already holds one line leaves 11 lines behind
  and carries 9 forward, and the continuation's first line is the 12th — asserted by line *text*,
  not by count, because a count passes just as happily if the continuation repeats the fragment.
- Widows/orphans: a paragraph with room for exactly 1 line at the foot of a column moves whole; a
  cut that would strand 1 line in the remainder is taken one line earlier; a 3-line paragraph never
  splits. Three separate assertions.
- Space-before is charged to the fragment and space-after to the remainder — asserted as exact
  arithmetic, because getting it wrong makes every continuation one space-after too tall and shows
  up only as slow page drift.
- The ragged-foot defect is gone: no full column in the two-column `rulebook` template is left with
  more than two lines unset. Measured on a 120-paragraph fixture of *varying* lengths, with
  splitting suppressed and then enabled:

  |        | pages | total unset | worst column |
  |---|---|---|---|
  | before |  30   |  6333.5 pt  |   327.5 pt (26 lines) |
  | after  |  24   |   166.0 pt  |    15.0 pt (1.2 lines) |

  The residual is the widow rule's floor, not a miss: a legal fragment needs two lines (25 pt), so a
  smaller gap is one the engine is right to leave. Paragraph lengths must vary for this to measure
  anything — a fixture of equal-length paragraphs tiles a column identically either way and reports
  no change. Asserted numerically **and** shown as a rendered page image in the PR.
- **A split costs no extra measurement.** A document whose blocks split across many frames reports
  `blocks_measured` equal to the number of distinct (block, width) pairs, not the number of
  placements. Asserted via `LayoutStats`.
- Incremental parity with a full relayout for a document containing splits, after an edit **before**,
  **inside** and **after** a split block — three assertions, matching session.rs's existing parity
  tests.
- Resuming from a checkpoint whose `split_at > 0` reproduces the same pages as a full pass.
- Unequal frame widths: a block facing a narrower continuation frame moves whole rather than
  splitting. Asserted, so the guard cannot be removed without a test failing.
- `Measured::Image` yields no break opportunities; an oversized image still moves whole.
- The doc comment at `crates/layout-engine/src/lib.rs:217-220` — which says a heading cannot appear
  twice "because a block is placed whole into one frame and never split" — is corrected in this PR,
  and a heading that splits reports the page its **first** line landed on.
- `benches/budgets.toml`: `ms_per_page` and `scaling_ratio` stay within budget.
  `incremental_pages_reflowed` does **not**, and re-baselining it is part of this increment rather
  than a concession — see "The incremental budget" below.


### The incremental budget moves, and what replaces it

Editing one paragraph in the middle of the 500-page synthetic document reflowed **1** page before
this increment and reflows **248** after it. That is not a regression in the engine; it is the
direct consequence of the defect being fixed.

Before fragmentation every page foot carried a few lines of slack, because a block that did not fit
moved whole. A one-line change to a paragraph was absorbed by that slack: the next page began with
the same block at the same y, the flow re-converged, and the counter read 1. Fragmentation fills
each column to within two lines of its foot — that is the whole point — so there is no slack left to
absorb anything. A paragraph that gains a line shifts every subsequent page boundary by a line,
permanently, and the 248 pages after the edit genuinely change. Repainting them is correct.

The old number was therefore measuring *looseness*, not efficiency. What the M1 constraint actually
cares about — not redoing the expensive work — is unchanged and is now stated directly:

|                              | before | after |
|---|---|---|
| pages reflowed               |    1   |  248  |
| **blocks measured**          |    1   |  **1** |
| wall-clock, as % of a full pass |  6.3% |  8.8% |

So a new gate, `incremental_blocks_measured`, carries the claim, and `incremental_pages_reflowed` is
re-baselined to 260 — "everything after a mid-document edit, and not one page more" — where it still
catches checkpoint resume breaking, which would send it to ~500.

Both are deterministic counters, and the budget file's 2× tolerance exists for runner timing noise.
Applying it to a counter re-baselined to 260 would put the failure threshold at 520, above every
value the counter can physically take on a 499-page document: a budget that cannot fail. Work
counters are therefore checked exactly, via a new `Budgets::check_exact`. This is the same lesson the
repo already paid for when four CI jobs emitted check-runs that were never required contexts.

## Test strategy

The conservation test is written first and may never be weakened. Everything else is exact
arithmetic in the repo's style, plus the two-painter-agnostic geometry assertions layout-engine
already uses. The rendered page follows spec 0036's precedent — the increment's value proposition is
that the page looks right, so the image is produced and looked at, not inferred from numbers.

## Risks

**The incremental path.** `FlowState` is the resume contract, and a mid-block checkpoint that is not
correctly restored produces a document subtly different after an edit than after a full relayout —
silent, and only in the direction users actually experience. The three parity tests are the guard,
and the edit-*inside*-a-split-block case is the one that would not be written by accident.

**Progress.** A cut that yields an empty fragment, or one that does not advance the absolute offset,
loops forever. The offset is required to strictly increase and the fragment to be non-empty; both
are invariants worth asserting rather than reasoning about.

**The height split.** Fragment and remainder heights are not a partition of the original, and the
natural implementation — subtract — is wrong in a way that accumulates rather than announcing
itself.
