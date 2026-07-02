# Plan — spec 0019 incr. 2: text-frame threading

Implements increment 2 of `specs/0019-text-frames-threading.md`: content that overflows one
frame continues into the next frame in its **thread**, and onto a new page (restarting at the
thread's first frame) once the thread's frames are exhausted.

## Approach (generalize the single-frame seam, at parity)

Incr. 1 gave us `Frame` + `lay_out_in_frame` (fill one frame top-to-bottom, paginate on overflow).
Incr. 2 generalizes that to an ordered chain of frames:

- Add `Thread { frames: Vec<Frame> }` — the ordered regions content flows through.
- Add `lay_out_in_thread(content, assets, thread, metrics, hyphenator)`: fill `frames[0]`; when a
  block overflows the current frame *and the frame is non-empty*, advance to the next frame in the
  thread; when the last frame overflows, push the page and restart at `frames[0]` on a fresh page.
- `lay_out_in_frame` becomes a thin wrapper: `lay_out_in_thread` over a single-frame thread. This
  keeps `lay_out` (→ export) byte-identical — **no export-output change, no golden regen** this
  increment.

The "non-empty frame" guard mirrors incr. 1's `!page.blocks.is_empty()` overflow rule: an oversized
block is placed (overflowing) in an empty frame rather than looping forever. Placement loop is
bounded to ≤2 iterations per block (advance once → next iteration the frame is empty → place).

## Files to touch

- `crates/layout-engine/src/lib.rs` — add `Thread`, `lay_out_in_thread`; refactor
  `lay_out_in_frame` to delegate; tests.
- `specs/0019-text-frames-threading.md` — mark incr. 2 in progress; add incr-2 behavior +
  acceptance section.
- `specs/README.md` — status line (stays `in-progress`).

## Test strategy

- **Parity:** `lay_out_in_frame(f)` == `lay_out_in_thread(Thread{[f]})`; all existing tests green
  unchanged; `lay_out` output unchanged (export golden gate unaffected).
- **Chain within a page:** two side-by-side frames; content that overflows frame A continues into
  frame B on the *same page* (blocks appear at both frames' x-coordinates, one page).
- **New page after last frame:** content overflowing both frames spills to a 2nd page whose first
  block sits at `frames[0]`'s origin.
- **Column x-coordinates:** blocks in frame B carry frame B's `x_pt`, not frame A's.
- Full workspace green (fmt, clippy -D warnings, build, test).
