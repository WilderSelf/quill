# 0045 — Table continuation

**Milestone:** M3 · **Status:** implemented

## Why

Spec 0039's roadmap entry promised tables that break between rows with the header repeating on the
continuation. Both were descoped onto a mechanism that did not exist: a table is one block, a block
was placed whole, so it could not break between rows, and with no continuation there was nothing for
a header to repeat onto. A 500-row random table was placed into one frame and overflowed the page —
the existing test asserts only that no cell is *lost*
(`crates/layout-engine/src/lib.rs`, `a_five_hundred_row_table_places_every_cell`), which was the
most that could honestly be claimed.

Spec 0044 built the mechanism. This increment is the thin part: teach a table's measurement where
its break opportunities are, and what has to be re-emitted at the top of each continuation.

## What

### A panel learns how it may be cut

`Measured::Panel` gains one field:

```rust
Panel {
    fill, stroke, parts, decorations,
    /// How this panel may be cut, or `None` for a panel that must be placed whole.
    split: Option<PanelSplit>,
}

struct PanelSplit {
    /// Height of each item, in order. Item 0 includes `repeat_h`, because every fragment
    /// begins with the repeated prefix.
    items: Vec<f32>,
    /// Content re-emitted at the top of every continuation — a table's header row and the rule
    /// under it — held at panel-local dy in `[0, repeat_h)`, and also present in `parts` /
    /// `decorations` so the *first* fragment gets it by ordinary slicing.
    repeat_parts: Vec<PanelPart>,
    repeat_decorations: Vec<PanelRect>,
    repeat_h: f32,
}
```

`break_items` returns `items`; `split_at(k)` slices `parts` and `decorations` at the cumulative
offset, and builds the remainder by prepending the repeat content and shifting everything after the
cut up by `cut_y - repeat_h`.

A stat block gets `split: None` here and its own value in spec 0046. A contents block
(`measure_toc`) and an empty table get `None` permanently.

### The remainder is not a suffix

This is what makes the increment more than a mechanical application of 0044. A continuation is the
rows after the cut **with the header prepended**, so its height is not `total − fragment` and its
item list is not a suffix of the original. That is exactly why `split_at` returns two fully measured
values with their own heights rather than an index for the caller to subtract from.

The corollary is an off-by-one with a visible symptom: a fit check that forgets the repeated header
overfills every continuation by exactly one header row. `items[0]` including `repeat_h` is what
prevents it, and it is asserted directly.

### Zebra striping survives the seam for free

The alternating bands are already `decorations` at panel-local dy, computed from each row's index in
the **whole** table. Slicing and shifting them preserves that, so the row starting a continuation
keeps the stripe it would have had. Getting this wrong produces two adjacent same-coloured rows at a
page boundary, which reads as a rendering bug — and is the kind of defect a distribution of numbers
never shows. It is asserted, and looked at.

### The panel rectangle closes and reopens

`place_measured` already draws the panel rect from the measurement's own height, so a fragment gets
a rect of the fragment's height and a continuation gets its own. A table has neither fill nor stroke
so it emits no outer rect at all; the behaviour matters for 0046 and is asserted here because this
is where the mechanism lands.

### Widows, orphans, and tables too small to split

The two-item minimum from 0044 applies unchanged: a legal cut leaves at least two rows on each side,
so a table of three rows or fewer is placed whole.

### A cuttable block no longer needs an empty frame to be an excuse

Spec 0044 left the `frame_empty` guard alone: a block too tall for an *empty* frame was placed there
and allowed to overflow, because moving it on would loop past every frame forever. That is still the
right answer for a block that **cannot** be cut — an image, a single enormous row — but it is the
wrong one for a table, because a table normally starts its own page. The frame is empty, so the
guard fires, and a 500-row random table runs off the bottom of the book.

So the guard is narrowed to what it is actually for: a block that does not fit is cut if it can be,
whether or not the frame is empty, and only an **uncuttable** one is placed whole and allowed to
overflow. Progress is still guaranteed, by the same invariant as 0044 — a cut is non-empty and
strictly advances the absolute offset — so emptiness was never what bounded the loop.

This is a behaviour change to what 0044 shipped, and it is stated here rather than folded in
quietly. It applies to long paragraphs too: one starting an empty column now flows on instead of
overrunning it.

## Acceptance criteria

- Regression: `Document::sample()`'s export byte-hash unchanged; every existing test passes.
- **Conservation, strengthened.** `a_five_hundred_row_table_places_every_cell` is upgraded from
  "every cell placed somewhere" to "every cell placed **exactly once, in row order**", and its
  comment about blocks not splitting is removed. The roadmap's known-issue entry loses its table
  clause.
- A table with a header spanning three frames repeats the header at the top of frames 2 and 3 and
  nowhere else — asserted by counting header-text placements (exactly 3) and by each sitting at the
  top of its fragment.
- A table with `header: None` splits and loses nothing, with no repeat.
- The header's height is charged to every fragment: the number of rows in a continuation is one
  fewer than a naive `total − fragment` would allow. Asserted as exact arithmetic.
- Zebra parity at the seam: the row that starts a continuation is striped by its index in the whole
  table, not in the fragment. Asserted, and shown in the attached render.
- A 500-row table paginates rather than overflowing one frame, and every cell lands exactly once in
  row order — the strengthened test above is what states this.
- A row taller than a whole empty frame is placed whole and overflows rather than looping: it has no
  legal cut, which is the case the narrowed guard still covers.
- Nothing overruns its frame. Asserted over every placed text, rect and image of a split table,
  because a repeated header charged to the wrong side shows up exactly as an overrun.
- Three rows or fewer never split.
- The panel decoration closes at the bottom of each fragment and reopens at the top of the
  continuation, rather than one rect spanning a page break — asserted by rect count and bounds.
- A split table costs no extra measurement, as 0044: `blocks_measured` counts distinct
  (block, width) pairs.
- Incremental parity with a full relayout for a document whose table splits, after an edit before,
  inside and after it.
- A rendered image of a table breaking across a page is attached to the PR.

## Test strategy

Row conservation first — it is the assertion that catches content loss, which is the only failure
that silently produces a wrong book. Then header-repeat counting, then zebra parity at the seam. The
zebra test exists because someone asked what the page would *look* like; it is written deliberately
rather than by extension of the others.

## Risks

**The repeated header's height.** Charging it to the fragment but not to the continuation, or the
reverse, overfills or underfills by exactly one header row. It is a one-line error with a visible
symptom and no natural test unless one is written for it.

**No legal cut.** A table whose header plus one row does not fit an empty frame must fall back to
placing whole. Returning an empty fragment would advance nothing and loop forever, so the invariant
"a fragment is non-empty and the absolute offset strictly increases" is as load-bearing here as in
0044.

**Float drift in cumulative offsets.** Item heights are summed to find a cut; slicing `parts` by
comparing their `dy_pt` against that sum must use the *same* summation, or a cell exactly on the
boundary lands in both halves or in neither. Both are conservation failures, and the conservation
test is what catches them.
