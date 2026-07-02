# 0019 — Text frames + threading

- **Milestone:** M1
- **Status:** in-progress (increment 1 — explicit content `Frame` seam, at parity)
- **Crates:** `quill-layout-engine` (owner), `quill-core-model`, `quill-export-pdf`

## Problem

The layout engine has no concept of a **frame**. `lay_out` flows every block into an implicit
single column pinned to the page: text is placed at `x = 0`, wrapped to the full trim *width*,
stacked down from `y = 0`, and paginated when a block would pass the full trim *height*. There is
no positioned region content flows into, and therefore no way to place a text column somewhere
other than the whole page, put two columns on a page, or **thread** overflow from one region into
another.

Frames + threading are named M1 scope in the approved plan (*"text frames + threading; …"*) and
are the structural prerequisite for master pages, multi-column layout, and the "InDesign-like
ceiling" the product promises. Everything downstream (a text column that isn't the whole page, a
sidebar, a caption box, threaded story flow) needs an explicit frame object first.

## Architectural decision (introduce the frame seam at parity, then thread across frames)

The valuable end state — text flowing through a chain of positioned frames across pages — has two
separable parts: (1) an explicit **`Frame`** that content flows into (geometry: origin + size), and
(2) **threading**, where content that overflows one frame continues into the next. Part 2 is where
multi-frame story flow and its pagination changes land; part 1 is a pure structural seam.

Following the team's established risk-ordering (spec 0016 incr.1 "measure through the seam at
parity", spec 0018 incr.1 "penalty stream at parity"), this spec lands the **frame seam first, at
parity**, then adds threading as a second increment:

- **Increment 1 — the `Frame` seam, at parity (this increment).** Introduce `Frame { rect }` in
  `layout-engine` and flow blocks into a frame's *geometry* — text wraps to the frame width, blocks
  are positioned at the frame origin, overflow is measured against the frame's bottom edge — rather
  than the implicit full-page column. The default frame `lay_out` uses is the **whole trim area at
  origin (0,0)**, so the produced `Vec<LaidOutPage>` is byte-identical and every export/golden test
  is unchanged. The only *new* capability is that a frame with a non-zero origin, a narrower width,
  or a shorter height now lays out correctly — proved by tests calling the frame path directly.
  Lowest-risk: a pure structural refactor with no export-output change and no data-model change to
  the serialized document.
- **Increment 2 — threading (named, deferred below).** A page carries one or more frames; content
  that overflows a frame continues into the next frame in its thread (and onto the next page when
  the thread's frames are exhausted). This is where multi-frame pagination and any golden
  regeneration land.

The acyclic crate seam is unchanged: `layout-engine` owns `Frame` and the flow logic, `export-pdf`
consumes the resulting `PlacedBlock`s exactly as today (their `frame: Rect` coordinates are already
page-relative, so a full-page frame reproduces the current positions).

## Behavior (increment 1)

### `Frame`

```rust
/// A positioned rectangular region that content flows into. The layout engine fills a frame
/// top-to-bottom; a block that would pass the frame's bottom edge overflows (to the next page here;
/// to the next frame in a thread — increment 2).
pub struct Frame {
    pub rect: Rect,
}

impl Frame {
    /// The whole-page content frame: the entire trim area at the origin. This is the frame `lay_out`
    /// uses, so its output is identical to the pre-frame implicit column (parity). Margins/insets and
    /// multiple frames per page are follow-ups.
    pub fn full_page(page_setup: &PageSetup) -> Frame;
}
```

### `lay_out_in_frame` (layout-engine)

```rust
pub fn lay_out_in_frame(
    content: &[Block],
    assets: &[Asset],
    frame: &Frame,
    metrics: &impl RunMetrics,
    hyphenator: &impl Hyphenator,
) -> Vec<LaidOutPage>
```

Flows `content` into `frame`, identically to today's `lay_out` except that the page column is the
frame's geometry rather than the full trim:

- **Width:** text wraps to `frame.rect.w_pt` (was the full trim width).
- **Origin:** each placed block's `frame.x_pt` is `frame.rect.x_pt` (was `0`); its `y_pt` is
  `frame.rect.y_pt + local_y`, where `local_y` accumulates from the frame top.
- **Overflow:** a block starts a new page when `local_y + block_height` would pass
  `frame.rect.h_pt` **and** the current page already has content (unchanged rule, now measured
  against the frame height rather than the trim height). A new page resets the cursor to the frame
  origin.
- Image sizing (spec 0009) uses `frame.rect.w_pt` as the content width to fit against.

`lay_out` becomes a thin wrapper preserving its current signature and output:

```rust
pub fn lay_out(doc, metrics, hyphenator) -> Vec<LaidOutPage> {
    lay_out_in_frame(&doc.content, &doc.assets, &Frame::full_page(&doc.page_setup), metrics, hyphenator)
}
```

## Inputs / outputs

- **Input:** a slice of `Block`s + `Asset`s, a `Frame`, a `RunMetrics`, and a `Hyphenator`.
- **Output:** `Vec<LaidOutPage>` whose `PlacedBlock` frames are positioned within the given frame.
  For `Frame::full_page` this is byte-identical to the current `lay_out` — **no export-output change
  this increment.**

## Acceptance criteria

- **Parity:** `lay_out(doc, …)` returns exactly what it returned before for `Document::sample()` and
  the existing pagination/justification/hyphenation tests (all current `layout-engine` tests stay
  green unchanged), and the export Ghostscript golden gate is unaffected (byte-identical output).
- **Frame origin offsets placed blocks:** laying the same content into a frame at a non-zero origin
  `(fx, fy)` shifts every placed block's `x_pt`/`y_pt` by `(fx, fy)` relative to the full-page
  layout (for content that fits on one page).
- **Frame width changes wrapping:** a paragraph laid into a frame narrower than the trim wraps to
  more lines than the same paragraph in the full-page frame (text respects the frame width, not the
  page width).
- **Frame height bounds pagination:** content that fits on one full-page frame paginates into
  multiple pages when laid into a frame whose height is a fraction of the trim height (overflow is
  measured against the frame's bottom edge).
- Full workspace green (`fmt`, `clippy --all-targets --all-features -D warnings`, `build`, `test`).

## Non-goals (named follow-up increments)

- **Threading — increment 2.** Multiple frames per page and overflow chaining from one frame to the
  next (and onto the next page when a thread's frames are exhausted). This increment lays out into
  exactly one frame geometry, repeated per page on overflow — it does not chain distinct frames.
- **Frames in the document model.** `Frame` lives in `layout-engine` and is derived from
  `PageSetup`; it is **not** yet a serialized `.tpub` field. Persisting author-defined frames
  (position, size, thread membership) into `core-model` is a later increment — it is a real
  data-model + format-version change and stays out of this parity seam.
- **Margins / insets.** `Frame::full_page` is the whole trim; deriving a smaller content frame from
  page margins (so text no longer bleeds to the trim edge) rides with the model change above, since
  margins are an authored `PageSetup` field.
- **Multi-column, baseline-grid alignment across frames, non-rectangular / text-wrap frames** — all
  later M1/M3 work on top of this seam.

## Performance note

`Frame` is a plain geometry value; the flow logic is the same single pass over blocks as today, so
there is no added cost at parity. Threading and the incremental/dependency-tracked reflow that keeps
500-page docs smooth build on this seam in later increments (a reflowed frame's chain is
recomputed, not the whole document); the perf gate applies once the harness lands.
