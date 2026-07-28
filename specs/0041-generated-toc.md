# 0041 — `Block::Toc`: generated contents with a bounded fixpoint

**Milestone:** M2 · **Status:** implemented

## Why

The cyclic increment, and the last of the three the milestone is named for. A table of contents
lists page numbers; its own length changes where every later page break falls; and that changes the
numbers it lists.

Spec 0040 answered "which page is this heading on". This is what consumes it.

## What

### The block stores nothing

```rust
Block::Toc { id, title, max_level, color }
```

No entries. They are **derived** from where the headings actually landed, which is not known until
the document is laid out and changes when it is. A stored entry is stale the moment anything is
edited, and a contents list whose numbers were right one edit ago is worse than none at all.

### The fixpoint

`lay_out_with_toc_status` lays out, reads the heading index, regenerates the entries, and lays out
again — stopping when the index stops changing. `TocStatus { iterations, converged }` reports what
happened.

**The bound is not decoration.** A contents entry can push a heading onto the next page, whose
longer number lengthens the entry, which pushes the heading further — or shortens it and pulls the
heading back, forever. Spec 0031 recorded unbounded "reflow until state matches" as the way a
pathological document hangs; a contents list is the case that actually oscillates.
`TOC_MAX_ITERATIONS` is 8. On hitting it the loop returns the **last iterate**: a complete document
with nothing missing, whose contents may disagree with a page number by one, and
`converged: false` so the caller can see that rather than being handed a guess presented as settled.

**Documents without a contents block do not enter the loop at all.** Not "skip it in spirit" — the
branch is taken before anything else happens, in both the one-shot and session paths, so nothing
about the cost or the incremental behaviour of every other document changes.

### Derived content in a cache keyed on authored content

The resolved index is **context**, not block content: it joins the stylesheet, page setup, masters
and page list in `context_fingerprint`. So a fixpoint iteration that fed a different index cannot
reuse the previous iterate's pages. The `Toc` block's own content fingerprint covers only its title
and `max_level` — fingerprinting the index there as well would make every contents block re-measure
whenever any heading moved, even when the entries it lists did not change.

Because intermediate iterations overwrite the session's stored pages, `changed_pages` is measured
against where the document was *before the call*, not against the previous iterate.

### Entries

Each is two runs plus a leader: the title (indented by level, truncated with an ellipsis rather
than wrapped — a contents list is scanned, and a two-line entry whose number sits beside its first
line reads as two entries), a dot leader, and the page number **right-aligned to the measure**, so a
1-digit and a 3-digit page end in the same column. Two runs rather than one padded string because
the number has to land at an exact x; padding with dots would put it wherever the last dot fell.

Built-in `toc-title` and `toc-1`..`toc-6` styles, sized and indented by level.

## Acceptance criteria

- Regression: the export byte-hash changes (`toc-*` styles join `StyleSheet::default()`), verified
  identifier-only — both files 8786 bytes, 124 differing, all inside the XMP identifier clusters and
  the trailer `/ID`. Ghostscript CI green.
- **The printed numbers match the final layout.** Every entry's number equals the page its heading
  is on in the *same* pass's index — cross-checked against `heading_index_of`, so a first-pass
  contents list cannot agree with first-pass numbers while being wrong about the document.
- Convergence is measured, not asserted: a document with two chapters settles in ≤ 3 passes.
- A document with no contents block takes exactly **one** pass.
- The loop terminates within the cap on a document with 57 headings and a long contents list, and
  returns a document with no content dropped — asserted structurally rather than by hunting an
  oscillating fixture, because what must hold is that *whatever* the loop does it terminates and
  loses nothing.
- `max_level: 2` lists h1 and h2 and omits h3.
- Page numbers are right-aligned to the measure's right edge, to 0.01 pt.
- A contents list with no headings is just its title, set in `toc-title` and larger than a `toc-1`
  entry.
- Session: renaming a chapter reaches the index; the fixpoint runs (`iterations > 1`) and converges.
- `benches/budgets.toml` unchanged — `quill-testdoc` emits no contents block, so the 500-page
  workload is the one every prior budget measured.

## Test strategy

The final-state cross-check is the primary test and is written so it cannot pass by comparing a
first-pass contents list to first-pass numbers.

The non-convergence test asserts the *invariants* — bounded iterations, nothing dropped — rather
than trying to construct an oscillating document. Hunting a fixture that provably oscillates would
be a test of the fixture, and the property that matters is that the loop is safe whatever the input.

Reading entries back off the page is filtered by source `BlockId`: a contents block shares its page
with whatever follows it, so reading every run on page 0 sweeps up the body text. That was found by
a failing test.

## Risks

- **Derived content in a cache keyed on authored content** fails silently in the stale direction.
  Splitting the index into *context* and the title/level into *content* is what keeps both
  directions right, and it is the part to re-check if a contents list ever goes stale.
- **The cap's value is a judgement.** Eight passes is generous for the documents seen here (two to
  three), and a document that needs more is one that is oscillating rather than converging slowly.
- `measure_block` and the `Measurer` trait grew a parameter with each of specs 0026, 0028 and 0041,
  so the ambient inputs are now a `BlockContext` struct — the next addition changes one struct
  rather than five signatures. Clippy's argument-count lint is what forced the issue, correctly.
- Interaction with spec 0035's index-based page assignment is real and was called out when 0035
  shipped: a contents list that grows by a page slides every subsequent `PageOverride`. That remains
  the accepted semantics, and it is the strongest argument for the roadmap's open question about
  anchoring masters to sections instead.
