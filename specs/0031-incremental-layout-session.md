# 0031 — Incremental, dependency-tracked layout

**Milestone:** M1 · **Status:** implemented

## Why

`CLAUDE.md` states it as a non-negotiable: *editing one text thread must re-flow only affected
pages, never the whole document.* Nothing implemented it. Every call re-ran Knuth-Plass over every
block from index 0, so changing one paragraph on page 3 re-broke all 500 pages.

That is precisely the behavior the primary competitor is documented to collapse under, and avoiding
it is the reason this engine exists at all. This increment is the claim made real, and measured.

## What

`LayoutSession::relayout(doc, metrics, hyphenator) -> LayoutResult { pages, stats, changed_pages }`.

Three mechanisms bound the work, each covering a different part of the cost:

**1. A measurement cache**, keyed by what a measurement actually depends on: block identity, a
fingerprint of its content, the frame width it was broken to, and its resolved style. If that key
were missing a dimension the measurement depends on, the cache would return a stale layout and the
document would be silently wrong — so the key *is* the contract.

**2. Resume from a checkpoint.** The flow records its state at every page boundary, so an edit on
page 300 restarts from page 300 rather than block 0. This required extracting the pagination loop
into a resumable `flow` function shared by both the one-shot and incremental paths. Two
implementations would drift, and a divergence between full and incremental layout is the worst bug
available here: the document would look different depending on how you arrived at it. Three tests
assert the two agree exactly, after an edit, an insert and a delete.

**3. Stop when the flow re-converges.** After a local edit the flow usually returns to exactly the
state it had before — same block, same page number, same column, same y — within a page or two. When
it does, and nothing later changed, the remaining pages are reused verbatim and the pass *stops
there*.

Mechanism 3 is what turns "reflow from the edit" into "reflow around the edit", and stopping early
matters more than it looks: see the measurements below.

### Detecting change

Two fingerprints, because two different things move layout:

- **Per-block content** — text, level, style name, and colour. Colour does not affect *measurement*,
  but it does affect the placed block, so a colour-only edit must still invalidate the cached result.
- **Document context** — page setup, stylesheet, master pages, default master. This one is
  load-bearing and was missing in the first implementation: without it the diff sees only blocks, so
  restyling a document or changing its margins looks like "nothing changed" and the session returns
  the previous pages unaltered. **A stale document presented as a current one is worse than being
  slow.** Caught by a test that restyles the body face and compares against a full pass.

The context fingerprint uses derived `Debug` output rather than a hand-written field walk: these
types grow as the model does, and a hand-written fingerprint silently stops covering a field the
moment one is added — surfacing as a document that refuses to re-flow after an edit nobody can see.

### Reporting

`LayoutStats` is deterministic **work counters**, not timings — the M1 claim is a statement about
work, and counters state it far more precisely than a wall-clock number that swings 10–30% on a
shared runner. `changed_pages` reports exactly the pages that differ, which is what a viewport
repaints and nothing more; an under-report is a stale screen.

## Measured

500-page synthetic document, one paragraph edited in the middle:

| | value |
|---|---|
| Pages re-flowed | **1 of 500** |
| Blocks measured | **1** |
| Blocks served from cache | 6 |
| Cost vs. a full layout pass | **~0.2%** |

Both are now bench-gated in `benches/budgets.toml`.

### What early stopping was worth

The first working version applied re-convergence *after* the flow, by truncating pages. It produced
the same output and the same "1 of 500 pages re-flowed" — but still **walked 1849 blocks**,
fingerprinting and cache-looking-up each one, at 12.2% of a full pass.

Stopping *inside* the loop at the re-convergence boundary took it to 6 blocks and 0.2% — roughly
sixty times cheaper. The counters looked correct either way. Only the timing ratio in the bench
showed the difference, which is a concrete argument for landing spec 0027's harness before this
work rather than after.

## Acceptance criteria

- [x] A first pass equals a full `lay_out` exactly.
- [x] Incremental output equals a full relayout after an edit, an insert, and a delete.
- [x] An unchanged document re-flows nothing and measures nothing.
- [x] Editing one paragraph in a 500-page document re-flows ≤ 3 pages and reuses the rest.
- [x] An edit on the last page reuses everything before it.
- [x] A style change invalidates the cache and matches a full pass.
- [x] A colour-only edit is picked up.
- [x] `changed_pages` equals exactly the set of pages that differ, including pages removed when a document shrinks.
- [x] `invalidate` forces a full pass.
- [x] Bench-gated: pages re-flowed and work fraction both have budgets.

## Known constraint

The cache key covers content, width and style but **not** the font metrics or hyphenator, which are
passed per call and are not comparable values. A session is therefore bound by contract to the
metrics it was first used with; `invalidate()` must be called if they change. This is stated in the
module docs rather than hidden, and it is why the type is a session rather than a free function over
a global cache.

## Non-goals

- Sub-page incrementality — re-flowing part of a page. The unit of reuse is a page.
- Parallel layout across page ranges.
- Persisting the cache across sessions.
- Incremental *export*. Export re-walks the pages it is given; only layout is incremental.
