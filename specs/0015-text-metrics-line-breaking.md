# 0015 — Real width-based line breaking (font metrics)

- **Milestone:** M0
- **Status:** in-progress (increment 1 of a multi-part spec)
- **Crates:** `quill-text-layout` (owner), `quill-layout-engine`, `quill-export-pdf`

## Problem

Line breaking used a fixed stand-in — `APPROX_CHAR_WIDTH_PT = 6.0` points per character — to
decide how many characters fit on a line (`max_chars = frame_width / 6.0`). That number has no
relationship to the font the export path actually embeds: a line the layout "fit" at 72
characters may render far wider (or narrower) than the frame, so **text overflows the frame** (or
wastes space). This is the last substantive M0 export-correctness gap: every other stage measures
real geometry, but text does not.

The real font *is* known — `export-pdf` builds an `EmbeddedFont` with per-glyph advance
`widths` (in 1000-unit em) — but only at export time, and the layout engine has no access to it.

## Architectural decision (why metrics are passed in, not stored)

The font that drives measurement is supplied to the layout engine **by the export caller**, via a
`CharMetrics` trait defined in the lowest crate (`quill-text-layout`). The font is **not** added
to `Document` in M0.

- **No dependency cycle.** `export-pdf → layout-engine → text-layout` already holds. Putting the
  trait in `text-layout` lets `layout-engine` measure without depending on `export-pdf` (where the
  font code lives), and lets `export-pdf` implement it for `EmbeddedFont`.
- **No premature schema change.** Storing the font in `Document` only matters for interactive /
  on-screen layout (M1), where layout runs without an export in flight. M0 is headless export:
  the caller always has the font, so it can build it once and hand it to layout. Font-in-`Document`
  is deferred to M1 rather than guessed at now.

## Behavior (increment 1)

### `CharMetrics` (text-layout)

```rust
pub trait CharMetrics {
    /// Advance width of `ch` at `size_pt`, in points.
    fn advance_pt(&self, ch: char, size_pt: f32) -> f32;
}
```

`export-pdf` implements it for `EmbeddedFont`: `advance = widths[char_to_gid[ch]] * size / 1000`
(unknown char → `.notdef` advance, i.e. the GID-0 width). A `MonospaceMetrics { em_ratio }` stub
(`advance = em_ratio * size_pt`) is provided for tests and as a deterministic fallback.

### `break_by_width` (text-layout)

```rust
pub fn break_by_width(text: &str, max_width_pt: f32, size_pt: f32, metrics: &impl CharMetrics)
    -> Vec<String>
```

Greedy, word-based (same shape as the breaker it replaces, now measured in real points):

- Split on whitespace; append a word to the current line while the measured advance of
  `current + " " + word` stays `<= max_width_pt`; otherwise start a new line.
- A word wider than `max_width_pt` on its own is placed alone (it will overflow — breaking
  oversized words / hyphenation is deferred).
- Whitespace is normalized to single spaces between words; empty text yields no lines.

Width of a string = the sum of `advance_pt` over its chars (nominal per-char advances; no
kerning/shaping in increment 1).

### Shared body metrics

`BODY_FONT_SIZE_PT = 10.0` and `BODY_LINE_HEIGHT_PT = 12.0` live in `text-layout` and are used by
**both** the layout engine (to measure and to reserve row height) and the writer (to set the font
size and per-line advance), replacing the previously divergent `6.0 / 10.0 / 12.0` constants.

### Wiring

- `layout-engine::lay_out(doc, metrics: &impl CharMetrics)` breaks text with `break_by_width` at
  `BODY_FONT_SIZE_PT` against the frame width; block height is `lines * BODY_LINE_HEIGHT_PT`.
- `export-pdf::export` collects the document's text chars, builds the `EmbeddedFont` **once**, and
  passes `&font` to both `lay_out` and `write_pdf`. `write_pdf` no longer builds the font or
  collects chars from the laid-out pages.

## Inputs / outputs

- **Input:** a `Document` plus a `CharMetrics` implementation (the embedded font in the export
  path; a stub in tests).
- **Output:** `Vec<LaidOutPage>` whose text lines fit their frame width under the given font's
  real advances. Export output is unchanged in structure; only line grouping (and thus pagination)
  now reflects real widths.

## Acceptance criteria

- `break_by_width` packs as many words as fit the measured width and breaks before the first word
  that would exceed it; a single over-wide word is emitted on its own line; empty text → no lines.
- Under `MonospaceMetrics { em_ratio: 0.6 }` at `BODY_FONT_SIZE_PT` (0.6 × 10 = 6 pt/char), a line
  breaks at the same word boundary the old `max_chars` breaker did — behavior parity for the stub.
- `lay_out` and `export` compile against the new `CharMetrics` parameter; the full workspace
  builds and tests green (`fmt`, `clippy -D warnings`, `build`, `test`).
- Layout and the writer measure text at the *same* font size (`BODY_FONT_SIZE_PT`), so the row a
  line is measured for is the row it is drawn on.

## Non-goals (named follow-up increments)

- **Knuth-Plass** optimal / justified line breaking (this is greedy; the quality upgrade is next).
- **Hyphenation** and breaking oversized single words.
- **Shaping** — kerning, ligatures, complex-script/bidi (`rustybuzz`); advances here are nominal
  per-char `hmtx` widths.
- **Leading derived from font metrics** (increment 1 keeps the fixed 12 pt line height).
- **Font in `Document`** for interactive / on-screen layout (M1).
- Per-frame margins / multi-column frames.
