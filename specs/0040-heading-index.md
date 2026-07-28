# 0040 — Heading index

**Milestone:** M2 · **Status:** implemented

## Why

"Which page is chapter 3 on" was unanswerable. `LayoutResult` reported `pages`, `stats` and
`changed_pages`; `LaidOutPage` carried `index`, `blocks` and `statics`; and a `PlacedBlock` had
geometry but **no back-reference to the `BlockId` that produced it**. So nothing downstream could
map content to a page.

A table of contents (spec 0041) and a PDF outline (0042) both need exactly that mapping and nothing
else. It ships alone, as a small additive change, for the same reason spec 0027 landed the perf
harness before the work it gated: if the index and the fixpoint loop land together and the TOC comes
out wrong, neither is trustworthy.

## What

### A placed block knows where it came from

`PlacedBlock::Text` and `PlacedBlock::Image` gain `source: BlockId`. Master furniture carries
`BlockId::UNASSIGNED` — a running head is text on a page and is *not* content, so it has no
identity, and the existing `UNASSIGNED` sentinel already means exactly that.

`PlacedBlock::Rect` (spec 0037) deliberately has no source: decoration is synthesized by the engine
and never corresponds to a block.

### The index

```rust
pub struct HeadingEntry { pub id: BlockId, pub level: u8, pub text: String, pub page_index: usize }

pub fn heading_index(doc: &Document, pages: &[LaidOutPage]) -> Vec<HeadingEntry>
```

`LayoutResult.headings` carries it, from both the incremental path and the no-op early-return path.

### Derived from the pages, not accumulated during pagination

This is the load-bearing decision, and it is what makes the increment correct rather than merely
present. An incremental pass **reuses whole pages** from the previous layout (spec 0031), so an
index built up as blocks were placed would be missing every heading on a reused page — and would be
missing them precisely when the document had just been edited, which is always.

Deriving it from the final page vector makes it correct by construction on the incremental path and
the cold path alike, at the cost of one walk over the pages. That walk is also why this needed the
`source` back-reference: without it the pages simply do not contain the information.

### First occurrence wins

A heading appearing more than once in the page vector reports its **first** page. That cannot happen
today, because a block is placed whole into one frame and never split (the roadmap's known issues
record this). But a TOC entry and a bookmark both mean "where does this start", so the rule is
stated in code rather than left to depend on an invariant that is expected to change.

## Acceptance criteria

- Regression: `Document::sample()`'s export byte-hash unchanged; every existing layout, render and
  export test passes.
- Document order, correct levels, correct ids: a document with h1/h2/h2 spread over several pages
  produces three entries in document order with non-decreasing page numbers, each naming a real
  heading block.
- Master furniture never enters the index — a running head is not a table-of-contents entry.
  Asserted against a document that has one.
- An empty document gives an empty index; a document of only body text gives an empty index. The
  second is the reuse direction: an index that reported every text block would pass every test
  above.
- **Incremental**: on a document with four chapter headings, an edit late in the document reuses
  pages (asserted, so the fixture cannot prove nothing) and the index is still complete and still
  equal to a cold pass's.
- **Current, not merely present**: inserting a page's worth of content ahead of a heading moves it to
  a later page and the index says so.
- The no-op early-return path still reports headings, or a caller that repaints without editing would
  watch its TOC empty itself.
- Parity: the session's index equals `heading_index` over a one-shot `lay_out` of the same document,
  asserted on every case above.
- `benches/budgets.toml` unchanged — one walk over already-produced pages, on a path that already
  walks every block.

## Test strategy

Straight assertions on the returned vector. The two that justify the increment existing separately
are the incremental ones, and both are written so they cannot pass vacuously: the reuse assertion
checks `pages_reused > 0` before concluding anything, and every case cross-checks against a full
cold pass rather than against a hand-written expectation.

## Risks

- **The index must be rebuilt on a pass that reuses pages**, or a TOC built on it goes stale exactly
  when the document is edited. Deriving from the final pages is the structural answer rather than a
  thing to remember; the incremental test is the guard.
- Adding a field to two `PlacedBlock` variants is a breaking change to a public enum. Eight
  construction sites, all in tests, plus two pattern sites in `render` and `export-pdf` — the
  compiler finds every one, and none of them is a silent-failure surface.
