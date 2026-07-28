# Spec 0058 — The baseline grid

**Milestone:** M4 · **Size:** large · **Status:** implemented

## Problem

`CLAUDE.md` has named the baseline grid a `layout-engine` responsibility since the beginning and
nothing implements it. Specs 0019, 0020 and 0028 each deferred it explicitly. Facing pages whose
baselines do not align is the defect that separates a book from a document, and it is the last of
the big typographic gaps.

## What this builds

An **opt-in** baseline grid: a page-relative ladder of lines that every block's first baseline snaps
down to.

```rust
pub struct BaselineGrid {
    /// Distance between grid lines. Set it to the body leading.
    pub step_pt: f32,
    /// The first grid line, measured down from the **top of the trim box**.
    pub origin_pt: f32,
}

// on PageSetup, #[serde(default, skip_serializing_if = "Option::is_none")]
pub baseline_grid: Option<BaselineGrid>,
```

### Opt-in, deliberately — overruling the roadmap's risk note

The roadmap anticipated that "grid snapping changes every baseline in the document, so it changes
the export hash and every geometric fixture in the repo", and budgeted spec 0051's re-derivation
discipline for it. **That is avoidable, and avoiding it is the better decision.**

A grid is a design choice, not a correctness fix. A document that does not ask for one has no reason
to move, and forcing every existing book, template and fixture through a re-derivation buys nothing
a publisher wanted. `None` is the default, `PageSetup::default()` is unchanged, no bundled template
opts in, and `Document::sample()`'s export is byte-identical. What the increment must prove is that
the mechanism *works*, and that is proved on documents that ask for it.

This is the same posture `PageSetup::default()` already takes on margins (spec 0036): the default
stays where it is and templates supply the design.

### Where snapping happens, and why there

In the flow loop, at the point a block's height is known and before it is placed: the cursor `y` is
advanced to the smallest grid position `g` with `g >= y`. Nothing else moves.

**Measured from the page top, not the frame top.** That is the whole point — two columns of one
page, and the two pages of a spread, share one ladder, so their baselines align. A frame-relative
grid would align each column to itself and to nothing else.

**The snap target is the baseline, not the frame top.** A `PlacedBlock::Text` is drawn with its
first baseline at `frame.y_pt + ascent(size)`, by both the writer and the screen renderer
(spec 0032). So the engine snaps `y + ascent(size)` onto the ladder and back-solves `y`. Snapping
the frame top instead would put the *box* on the grid and the type slightly off it, which is the
error that looks almost right.

`RunMetrics` therefore grows `ascent_pt`, with a default of `0.8 × size` so every existing
implementation keeps compiling and the monospace stub stays font-free. `quill-fonts::Font`
overrides it with the real value — the same one the writer and the renderer already use, so all
three agree by construction.

### Per frame and local

Snapping reads one number — the cursor — and writes one. It is `O(1)` per block, needs no second
pass, and no block's position depends on any block after it. The incremental budget is what proves
it: `incremental_blocks_measured` and `incremental_pages_reflowed` are unchanged, because snapping
is not a measurement and does not enter a cache key.

A global grid recompute is a **named non-goal**.

### What a grid can and cannot align

A block's *first* baseline is snapped. Its subsequent lines follow at the style's leading, so they
land on the grid exactly when that leading is a whole multiple of `step_pt`. That is the real
typographic contract of a baseline grid and not a limitation to hide: a house style is *designed*
around its grid.

So `BaselineGrid::off_grid_styles` names every style whose leading is not a multiple of the step,
and `quill preflight` prints them. A silent misalignment is the failure mode; a list of the four
styles that need their leading rounded is a fix that takes a minute.

## Acceptance criteria

- Every text baseline on a gridded page falls on a grid line, asserted to **0.01 pt** across a
  multi-page, two-column document whose styles are grid-multiples.
- Grid snapping composes with **fragmentation** (0044): a continuation's first baseline is on the
  grid.
- It composes with **indents** (0048) and with **declared components** (0054): a component's
  sections snap as body text does.
- Facing pages align: the set of baselines on a left page and on the facing right page is the same
  ladder.
- Snapping is per frame and local; the incremental budget is unchanged.
- Ungridded documents are **byte-identical**: `Document::sample()`'s export hash, spec 0054's
  parity digests and spec 0051's equivalence digest all unmoved.
- `off_grid_styles` names a style whose leading is not a grid multiple, and is surfaced by the CLI.
- A rendered spread showing two facing pages with aligned baselines is attached.

## Non-goals

- A global grid recompute. See above.
- Snapping *every* line of a block whose leading is not a grid multiple. That would mean laying out
  a paragraph line-by-line against the grid, which is a different engine; the honest answer is to
  name the style and let the designer fix its leading.
- Turning the grid on for bundled templates. A separate, deliberate decision, and one worth making
  with a designer's eye rather than as a side effect of building the mechanism.
