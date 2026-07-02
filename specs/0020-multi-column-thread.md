# 0020 — Multi-column thread from page setup

- **Milestone:** M1
- **Status:** in-progress (increment 1 — the `Thread::columns` constructor, at parity)
- **Crates:** `quill-layout-engine` (owner), `quill-core-model`

## Problem

Spec 0019 built the threading seam: `Thread { frames }` + `lay_out_in_thread` flow content across an
ordered chain of frames (frame → frame → next page). But nothing constructs a *multi-frame* thread
yet — `lay_out` still uses a single full-page frame ([`Frame::full_page`]), so the threading engine
has no way to produce the **multi-column** layouts that TTRPG books lean on (two-column stat pages,
sidebars). Spec 0019 named multi-column as "later M1 work on top of this seam"; this spec delivers
the first, smallest piece of it.

## Architectural decision (a derived constructor, at parity — no model change)

Following the same risk-ordering the frame/threading seam used (introduce the capability as a pure
`layout-engine` constructor exercised via direct tests, leaving `lay_out`/export byte-identical), add
a **`Thread::columns`** constructor that divides the trim area into N equal-width columns. This is the
exact parallel of [`Frame::full_page`]: geometry derived from `PageSetup`, no authored field, no
serialized-model change, no `FORMAT_VERSION` bump.

- Multi-column layouts reach export only once `lay_out` is given a multi-column thread — which rides
  with the authored `PageSetup` column-count / frame-persistence increment (a real data-model +
  format-version change, named in spec 0019's non-goals and **out of scope here**).
- This increment is therefore parity-preserving: `lay_out` is untouched, every export golden is
  unchanged, and the new capability is proved by laying content into a `Thread::columns` thread in
  tests.

## Behavior

### `Thread::columns`

```rust
impl Thread {
    /// A left-to-right chain of `count` equal-width columns spanning the trim area, separated by
    /// `gutter_pt` of horizontal space, each the full trim height at `y = 0`. Content laid into the
    /// returned thread (via `lay_out_in_thread`) fills the leftmost column top-to-bottom, then the
    /// next column, and onto a new page once the last column fills. A single column (`count == 1`)
    /// is the whole trim area — identical to `Frame::full_page` (the gutter is then irrelevant).
    pub fn columns(page_setup: &PageSetup, count: usize, gutter_pt: f32) -> Thread;
}
```

- **Column width:** `col_w = (trim_w - (count - 1) * gutter_pt) / count`. All columns share this
  width.
- **Column origin:** column `i` (0-based) has `x = i * (col_w + gutter_pt)`, `y = 0`.
- **Column height:** the full trim height (`trim_h`); vertical margins are a later (authored) concern.
- **`count == 1`:** one frame equal to `Frame::full_page(page_setup)` (gutter ignored — there is no
  interior gutter with a single column).
- **`count == 0`** is a caller error: the constructor asserts `count >= 1` (loud failure over a
  silent empty thread, matching `lay_out_in_thread`'s "a thread must have at least one frame").
- **Oversized gutter** (`(count - 1) * gutter_pt >= trim_w`, so the derived `col_w <= 0`) is also a
  caller error: the constructor asserts `col_w > 0` rather than emit negative-width, overlapping
  frames that would silently corrupt downstream layout (`break_paragraph` against a negative width) —
  a visible failure over silent press-corruption, per the project's non-negotiable rule.

## Inputs / outputs

- **Input:** a `&PageSetup`, a column `count` (≥ 1), and a `gutter_pt`.
- **Output:** a `Thread` of `count` full-height frames, left to right. Laid out via the existing
  `lay_out_in_thread`, so the pagination/threading behavior is spec 0019's, unchanged.

## Acceptance criteria

- **Single column is the full page:** `Thread::columns(page, 1, g)` has exactly one frame equal to
  `Frame::full_page(page).rect` for any gutter `g`.
- **Columns tile the trim width:** for `count = N`, the frames' widths are all equal and
  `N * col_w + (N - 1) * gutter == trim_w` (within a float epsilon); columns are left-to-right and
  non-overlapping (column `i+1`'s `x` == column `i`'s `x + col_w + gutter`).
- **Full height at the top:** every column has `y == 0` and `h == trim_h`.
- **Threading integration:** content that overflows the first column of a two-column thread continues
  into the second column on the *same page* (blocks appear at both columns' x-coordinates, one page)
  — i.e. the constructor composes with `lay_out_in_thread` to produce real multi-column flow.
- **Loud failure on a nonsensical gutter:** `Thread::columns(page, count, gutter)` panics when the
  gutter is wide enough that `col_w <= 0`, rather than returning negative-width overlapping frames.
- **Parity:** `lay_out` is untouched; the export Ghostscript golden gate is unaffected (no
  export-output change this increment).
- Full workspace green (`fmt`, `clippy --all-targets --all-features -D warnings`, `build`, `test`).

## Non-goals (named follow-up increments)

- **Authored column count / frames in the document model.** Persisting a column count (or explicit
  author-placed frames + thread membership) into `.tpub` so `lay_out`/export produce columns is the
  data-model + `FORMAT_VERSION` change named in spec 0019's non-goals — it stays out of this derived
  constructor.
- **Vertical margins / insets.** Columns here span the full trim height; deriving a shorter content
  height from page margins rides with the authored-`PageSetup` change above.
- **Unequal / variable-width columns, per-column baseline grids, balanced columns** (equal-height
  last row), **cross-column optimal (Knuth-Plass) breaking** — all later work on top of this and the
  spec 0019 seam.
