# 0019 — Text frames + threading

- **Milestone:** M1
- **Status:** in-progress (increment 2 — threading across a thread's frames)
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

## Behavior (increment 2 — threading)

Increment 1 flows content into one frame, paginating vertically. Increment 2 generalizes that to an
ordered **thread** of frames: content that overflows the current frame continues into the next frame
in the thread, and onto a new page — restarting at the thread's first frame — once the thread's
frames are exhausted.

### `Thread`

```rust
/// An ordered chain of frames that content flows through. Content fills frames[0]; a block that
/// overflows continues into the next frame in the thread, and onto a new page (restarting at
/// frames[0]) once the thread's frames are exhausted.
pub struct Thread {
    pub frames: Vec<Frame>,
}
```

### `lay_out_in_thread` (layout-engine)

```rust
pub fn lay_out_in_thread(
    content: &[Block],
    assets: &[Asset],
    thread: &Thread,
    metrics: &impl RunMetrics,
    hyphenator: &impl Hyphenator,
) -> Vec<LaidOutPage>
```

- **Fill order:** blocks fill `frames[0]` top-to-bottom, then `frames[1]`, … on the *same page*.
- **Frame advance:** a block that would pass the current frame's bottom edge (and the frame already
  has content) continues into the next frame in the thread; the cursor resets to that frame's top.
- **Page advance:** overflowing the **last** frame pushes the page and restarts at `frames[0]` on a
  fresh page (the same thread geometry is repeated per page).
- **Oversized block:** a block taller than a frame is placed in an otherwise-empty frame rather than
  skipping through every frame/page — the incr. 1 "already has content" guard, now measured per
  frame. The placement loop is therefore bounded to ≤ 2 iterations per block.
- **Width per frame:** text wrapping and image sizing use the frame the block lands in, so a
  block that advances into a narrower frame re-wraps to that width.

`lay_out_in_frame` becomes a thin wrapper over a single-frame thread, so `lay_out` (→ export) is
unchanged — **no export-output change and no golden regeneration this increment.** Multi-frame page
layouts only reach export once `lay_out` itself is given a multi-frame thread, which rides with the
`PageSetup`/model change named in the non-goals below.

## Inputs / outputs

- **Input (incr. 1):** a slice of `Block`s + `Asset`s, a `Frame`, a `RunMetrics`, and a
  `Hyphenator`. **(incr. 2)** the `Frame` becomes a `Thread` (a one-frame thread == incr. 1).
- **Output:** `Vec<LaidOutPage>` whose `PlacedBlock` frames are positioned within the given
  frame/thread. For `Frame::full_page` / a single full-page thread this is byte-identical to the
  current `lay_out` — **no export-output change this increment.**

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

### Acceptance criteria (increment 2 — threading)

- **Parity:** a single-frame `Thread` returns exactly what `lay_out_in_frame` returns for that
  frame, so `lay_out` (and the export golden gate) is unchanged.
- **Overflow chains into the next frame on the same page:** content that overflows the first frame
  of a two-frame thread continues into the second frame on the *same page* (blocks appear at both
  frames' x-coordinates, one page) rather than spilling to a new page.
- **New page only after the last frame fills:** content that overflows *both* frames spills to a
  second page whose first block restarts at the first frame's origin.
- **Landed-frame geometry:** a block that overflowed into a later frame carries that frame's x and
  width, not the earlier frame's.

## Non-goals (named follow-up increments)

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
