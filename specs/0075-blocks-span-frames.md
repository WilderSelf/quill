# 0075 — A derived or composite block may span frames

**Milestone:** M6 · **Status:** implemented

## Why

Two entries in the roadmap's known issues are the same defect wearing different clothes: **an
indivisible `Measured::Panel` placed in a frame it does not fit**, reaching the flow branch that
places a block whole and lets it overflow the page.

- **A generated contents list taller than its frame overflows the page.** `measure_toc` returned
  `Measured::Panel { split: None, .. }` with the comment "a contents block is deliberately
  indivisible", so `break_items` yielded `None`, `cut_fitting` yielded `None`, and the flow placed
  it whole. **This shipped, and nothing exercised it** — every contents fixture in the workspace is
  a handful of chapters, and a real 500-page book's contents list is three pages.
- **A stat-block section taller than its column cannot be cut.** Spec 0046 cuts a composite
  *between sections and nowhere else*, so a single long section has no legal cut at all. Recorded,
  and pinned by a test whose own doc comment said it would invert when the fix shipped.

0075 is sequenced before the index and before footnotes because both inherit this exactly: an index
is `Block::Toc`'s derivation with different entries, and a footnote that splits across pages needs
the same mechanism.

## What

Spec 0044's `\vsplit` wired through the blocks that refused to cut. No second mechanism: the same
`break_items` / `cut_fitting` / `split_at` triple, the same `FlowState::split_at` absolute offset,
the same `MeasureKey`.

### The contents list cuts between entries

An entry is the atom, for the reason a table's is a row and a stat block's is a section: it is the
unit a reader recognises, and half of one is not navigation at all. `measure_toc` records the
panel-local y of every entry and hands `PanelSplit` the differences.

Three details are decided rather than inherited:

- **The list's own title is folded into item 0**, exactly as a panel's top inset is, so a fragment
  can never be the title alone.
- **Nothing is re-stated on a continuation.** `repeat_h` is zero. A repeated "Contents" would read
  as a second contents list rather than as the same one carrying on — the opposite of a table
  header, where the header is what makes the rows legible.
- **`keep_together` is true.** A contents list that fits the next frame whole belongs there whole,
  which is also precisely the behaviour every existing document already had. The change is confined
  to the case that was broken.

The old comment's objection — that the fixpoint would re-derive a half-placed list under its own
fragments — is answered by the fixpoint itself. Each iteration re-measures the **whole** block from
the heading index and re-cuts it; a fragment is never derived from a fragment. And nothing in the
measurement depends on the available height, which is spec 0044's precondition for offering break
opportunities at all.

### The fragment minimum is per-variant, and this one is two

`MIN_ENTRIES_PER_FRAGMENT = 2`. Spec 0046 had to make the minimum per-variant when it found that
demanding two *sections* made the smallest legal cut larger than a column, so nothing cut and the
panel ran off the page. The question that settles the number is therefore "what is an item here?",
and a contents entry is one line — a tabbed `{title}\t{number}` that never wraps, by the clipping
rule. A lone entry at the top of a page is the same widow a lone line is, so it takes
`MIN_LINES_PER_FRAGMENT`'s answer.

It is safe here in a way it was not for sections, and the check is arithmetic rather than faith: the
smallest legal fragment is the title plus two entries, about 60 pt in the bundled styles, against
the 378 pt of the narrowest column the `reference` template builds.

### A composite may cut *inside* a section when no section boundary fits

This is the roadmap's open question, and it is answered rather than deferred — at **element**
granularity.

`PanelSplit` gains `preferred: Vec<usize>`. A component's items are now its elements for every
granularity; `SplitGranularity` chooses which of those cuts the engine would *rather* take:

- `Sections` — prefer a section start, fall back to any element.
- `Elements` — no preference.

`cut_fitting` takes the largest legal *preferred* cut that fits, and the largest cut of any kind
only when no preferred one does. So a stat block still breaks between sections whenever it can —
spec 0046's promise, and `a_cut_stat_block_never_separates_an_attributes_list` still passes — and
has somewhere to go when it cannot. The distinction the old code could not express is "would rather
break between sections" versus "may only break between sections"; the second is what made an
over-tall section uncuttable.

`min_items` is counted in elements for every granularity as a result. Both bundled definitions and
both example packs declare `1` and `2`, whose meanings are unchanged in practice; the reinterpretation
is stated in `docs/pack-authoring.md` and on `SplitDef::min_items` rather than left to be discovered.

### Keep-together must actually move the block

Found by the fixture the known issue named, and it is **not** what the known issue diagnosed. Spec
0046's keep-together says the loop "prefers moving a block whole when it would fit the continuation
frame entire". It declined the cut — and then the `frame_empty` guard placed the block where it
stood and let it overflow, in a frame it does not fit, while a frame that would have held it whole
sat one column away.

On the `reference` template that is not a corner case: page 0 gives 378 pt columns and page 1 gives
540 pt ones, so a 440 pt panel prefers to move. The guard now excludes `keep_whole`. Progress is not
at risk — `keep_whole` is only true when the block fits the continuation frame entire, so the next
iteration places it: one move, never a loop.

**No amount of finer cutting would have reached this half of the defect.** Verifying the inherited
diagnosis before implementing its named fix is what surfaced it.

### The progress invariants are asserted

Spec 0044 called them "invariants worth asserting rather than reasoning about" and there was no
`assert!` on either. This is the increment that should add them, because it adds new ways for a cut
to be degenerate: a contents list's entries and a composite's elements are both item lists derived
somewhere else. The flow loop now asserts, once per cut, that

- the fragment is non-empty,
- the remainder is non-empty, and
- the absolute item offset strictly increases.

`Measured::item_count` exists so the remainder check costs no allocation.

### The contents list was empty in the incremental path all along

Found by this increment, fixed by it, and adjacent rather than central — recorded here because it is
the reason the increment is observable at all where a user would see it.

A contents block's entries come from the heading index, which is *context*, so `content_fingerprint`
deliberately keeps it out of `MeasureKey`: hashing it in would re-measure every contents block
whenever any heading moved. But nothing evicted the cached measurement when the index changed, and
the fixpoint's **first** pass runs with an empty index by construction. So the second pass served the
first pass's list from cache, and every document laid out through a `LayoutSession` — the path the
app uses — placed a contents list consisting of nothing but its own title, on every pass, for ever.

The session now drops cached measurements for contents blocks when the context fingerprint moves:
as narrow as the derivation, one re-measure per contents block per pass, every other cache entry
untouched. The test that should have caught it asserted only that the heading *index* saw a rename
and never looked at what was drawn; it now asserts the placed entries.

### What does not change

- `MeasureKey`, and therefore the measurement cache's behaviour. A cut is a derivation over an
  already-cached measurement; `incremental_blocks_measured` stays at 1.
- `Document::sample()`'s export bytes — it has no `Block::Toc` and no panel.
- The geometry of every component fixture in `component_parity`. A uniform-height template cannot
  reach the keep-together change (an empty frame's available height *is* the continuation frame's
  height, so `keep_whole` is false), and the preference keeps every cut that used to be taken.
- `FORMAT_VERSION`, `TEMPLATE_VERSION`, `COMPONENT_DEF_VERSION`. No persisted shape moves; a
  `.qpack` written before this increment lays out the same way unless it was one of the documents
  that overflowed.

### What still overflows — the named residual

A **single authored run** that wraps taller than a frame. The floor is now one element, so a stat
block whose one section is one enormous paragraph still has no legal cut and is placed whole. That
needs a cut inside a `PanelPart`'s line list — splitting the part, re-deriving its ink box (spec
0069), and threading run metrics into `Measured::split_at`, which takes none today. It is a
different change and is recorded as a follow-up rather than half-built here.
`a_stat_block_of_one_oversized_run_is_placed_whole` pins it, as its predecessor pinned the case this
spec fixed.

## Acceptance criteria

- Regression: `Document::sample()`'s export byte-hash unchanged; `component_parity`'s three digest
  sets unchanged on all ten fixtures; every existing layout, session, render and export test passes.
- **A contents list longer than a frame is split across pages and no entry is lost.** A 150-chapter
  fixture — none existed — asserted for: entries on at least three pages, one entry per chapter in
  document order, the list's own title placed exactly once, no fragment below the entry minimum, and
  nothing overrunning a frame.
- The fixpoint still converges over a contents list that spans pages, and what it settled on is true
  of the document it produced: every printed number equals the page its heading actually landed on.
- `a_section_taller_than_its_column_is_placed_whole` is **rewritten, not deleted**, per its own doc
  comment: the overflow assertion becomes an assertion that there is none. Conservation is asserted
  on both sides of the rewrite and was already there.
- A cut falls **between two elements of one section** when no section boundary can carry it —
  asserted by consecutive runs of one section landing on different pages, which no arrangement of
  section boundaries can produce.
- Incremental parity with a full relayout across a contents list that straddles a page boundary — a
  checkpoint with `split_at > 0` inside a block that is re-derived every pass.
- A session places the contents *entries*, not just the title.
- `benches/budgets.toml`: every entry within budget, `incremental_blocks_measured` still 1.

## Test strategy

Conservation first, as in 0044 and 0045: content loss is the only failure that silently produces a
wrong book. Each of the four behaviour changes was then proved against its own defect by
reintroducing it and watching the test fail — the contents-list `split`, the element boundaries, the
keep-together move, and the session eviction — and restored byte-for-byte afterwards.

The digests are the other half. `SAMPLE_EXPORT_DIGEST` and all three `component_parity` sets are
claims that nothing moved, and they are unchanged rather than re-derived: this increment is one
whose blast radius should be exactly the documents that were broken, so an unmoved digest is the
result, not an obstacle.

## Risks

**Cutting something that should have stayed together.** The preference is what stands between "may
cut inside a section" and "does". Asserted by the attributes-list test, which is written from the
other direction — a cut that never falls inside a section when a section boundary fits.

**The fixpoint.** A contents list that now changes the page count by *pages* rather than by lines
moves every heading further, which changes the numbers it prints. It still converges, and the reason
is structural rather than lucky: an entry's height does not depend on its number — it is one clipped,
never-wrapped line — so the list's height is fixed once the heading set is, and only the printed
digits move. That is also why **no oscillating contents fixture could be constructed for this
increment**: with the entry set fixed, there is no free variable left to oscillate. The bound and its
`converged: false` report stay as they are, for the second derived quantity (spec 0076) that will
have one.

**Float drift in cumulative offsets**, unchanged from 0045: `split_at` slices `parts` by comparing
`dy_pt` against a sum of `items`, so both must accumulate the same way. The conservation assertions
are what catch it.
