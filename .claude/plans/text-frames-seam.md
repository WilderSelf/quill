# Plan: text frames + threading — increment 1 (Frame seam, at parity)

**Spec:** `specs/0019-text-frames-threading.md` (authored as step 0 of this increment).
**Plan authorization:** the approved plan enumerates M1 scope as *"text frames + threading;
paragraph/character styles; master pages; …"* — this is the first increment of that arc.

## Goal

Introduce an explicit content **`Frame`** (a positioned rectangular region content flows into)
in `layout-engine`, and flow blocks into a frame's geometry rather than the implicit
"full-page column at x=0". Ship it **at parity**: the default frame is the whole trim area at
origin (0,0), so `lay_out` — and therefore every export/golden test — is byte-identical. This
mirrors the team's seam-first increments (spec 0016 incr.1, spec 0018 incr.1).

Threading (overflow chaining across multiple frames per page) is increment 2 and is explicitly
out of scope here.

## Acceptance criteria

1. `layout-engine` exposes `Frame { rect: Rect }` and `Frame::full_page(&PageSetup) -> Frame`
   returning `{ x:0, y:0, w:trim.w, h:trim.h }`.
2. A new `lay_out_in_frame(content, assets, frame, metrics, hyphenator) -> Vec<LaidOutPage>`
   flows blocks into `frame`: text wraps to `frame.rect.w_pt`, blocks are positioned at
   `frame.rect.x_pt` / `frame.rect.y_pt + local_y`, a block overflows when it would pass
   `frame.rect.y_pt + frame.rect.h_pt`, and a new page resets the cursor to the frame origin.
3. `lay_out` becomes a thin wrapper: `lay_out_in_frame(&doc.content, &doc.assets,
   &Frame::full_page(&doc.page_setup), …)` — output unchanged (parity).
4. New tests prove the seam has real behavior: a non-zero frame origin offsets placed blocks;
   a narrower frame width changes wrapping; a shorter frame height bounds pagination earlier.
5. All existing tests stay green; export golden/CI byte output is unchanged.

## Files to touch

- `specs/0019-text-frames-threading.md` (new) + `specs/README.md` row.
- `crates/layout-engine/src/lib.rs` — add `Frame`, `lay_out_in_frame`, rewire `lay_out`, tests.

## Test strategy

`cargo test -p quill-layout-engine` for the new seam behavior + existing pagination/parity;
full `cargo test` to confirm export golden tests are byte-identical.
