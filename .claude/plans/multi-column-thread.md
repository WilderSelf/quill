# Plan — spec 0020: multi-column thread from page setup

Implements spec `specs/0020-multi-column-thread.md`: a `Thread::columns(page_setup, count, gutter_pt)`
constructor that divides the trim area into `count` equal-width, full-height columns, so content laid
via the existing `lay_out_in_thread` flows column → column → next page (real multi-column layout).

## Approach (derived constructor, at parity — no model change)

Parallel to `Frame::full_page`: geometry derived from `PageSetup`, purely in `layout-engine`. No
`core-model`/format change, no `FORMAT_VERSION` bump, `lay_out` untouched → export byte-identical.
The new capability is exercised via direct tests laying content into a `Thread::columns` thread.

- `col_w = (trim_w - (count-1)*gutter) / count`; column `i` at `x = i*(col_w+gutter)`, `y=0`,
  `w=col_w`, `h=trim_h`.
- `count == 1` → single full-page frame (== `Frame::full_page`, gutter ignored).
- `count == 0` → `assert!(count >= 1)` (loud failure, matches `lay_out_in_thread`'s frame assert).

## Files to touch

- `crates/layout-engine/src/lib.rs` — add `Thread::columns`; tests.
- `specs/0020-multi-column-thread.md`, `specs/README.md` — the spec (done).

## Test strategy

- Single column equals `Frame::full_page(page).rect` for any gutter.
- N columns: equal widths; `N*col_w + (N-1)*gutter == trim_w` (epsilon); left-to-right,
  non-overlapping (`x_{i+1} == x_i + col_w + gutter`); every column `y==0`, `h==trim_h`.
- Integration: content overflowing column 0 of a 2-column thread continues into column 1 on the
  *same page* (composes with `lay_out_in_thread`).
- Parity: `lay_out` output unchanged (existing tests + goldens stay green).
- Full workspace green (fmt, clippy -D warnings, build, test).
