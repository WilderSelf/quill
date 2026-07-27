# 0028 — Persisted paragraph styles

**Milestone:** M1 · **Status:** implemented

## Why

Font size and leading were **crate constants** — `BODY_FONT_SIZE_PT = 10.0` and
`BODY_LINE_HEIGHT_PT = 12.0` in `quill-text-layout`. Every block in every document was measured and
drawn at body size, headings included. A level-1 heading was distinguishable from body text only by
being ragged-left, and the exported PDF's content stream contained exactly one `/F0 10 Tf` for the
whole document.

For a product whose entire purpose is press-quality book layout, that is not a missing nicety; it is
the absence of typography.

Styles also have to land *before* the measurement cache in spec 0031, because the style a block was
measured with becomes part of that cache's key. Retrofitting a key dimension after a cache hardens
is strictly more expensive than including it from the start.

## What

### The model

`ParagraphStyle { font_size_pt, leading_pt, align, space_before_pt, space_after_pt }`, held in a
named `StyleSheet` on the `Document`. A **named sheet** rather than per-block formatting: changing
"every heading in the book" has to be one edit, not a sweep across 500 pages.

Leading is stored, not derived from a multiplier on size, because press typography routinely sets
the two independently.

`TextAlign` is a serializable spelling of alignment in `core-model`, mapped to
`quill_text_layout::Alignment` by the engine — the *document* owns the intent, the layout crate owns
the algorithm.

Resolution is: the block's explicit `style` name, else its structural default (`body`, or
`h{level}`). An unknown name falls back to `body`, then to `ParagraphStyle::default()`. **A missing
style must not lose the text** — a renamed style setting a paragraph in the body face is
recoverable; a paragraph vanishing is not.

Defaults preserve history exactly: `body` is 10 pt on 12 pt justified, so a document that never
mentions styles lays out precisely as it did before styles existed. Headings get a conventional
descending scale, ragged-left, with space above so they separate from the text they follow.

### Reaching the page

The load-bearing change is that **`PlacedBlock::Text` now carries `font_size_pt` and `leading_pt`**.
`PlacedBlock` is all the PDF writer and (later) the screen renderer ever see; before this it
recorded lines and colour but not how the text had been measured, so the writer had no choice but to
use the global constant. Styles would have changed line breaking while export kept drawing at 10 pt.

Three places consume it, and all three had to change together or the output would be internally
inconsistent:

1. **Measurement** — `justify_paragraph_hyphenated` breaks at the style's size, and block height is
   `lines × leading + space_before + space_after`. Space is part of the occupied height so
   *pagination reserves it*: a heading that only fits on the next page because of its space-above
   must break there.
2. **Placement** — the text frame starts *below* the space above. The space belongs to the block's
   height but no text is drawn in it.
3. **Drawing** — `set_font` uses the block's size, the baseline advance uses its leading, and
   `show_line`'s `TJ` adjustment divides by that size. That last one matters: the adjustment is in
   thousandths of an **em**, so a global constant would misplace every word on a justified line set
   at anything but body size.

## Acceptance criteria

- [x] The default `body` style is 10 pt / 12 pt / justified — the historical treatment, unchanged.
- [x] `h1`..`h6` descend in size, are all at least body size, are ragged-left, and carry space above.
- [x] A block can name an explicit style, which overrides its structural default.
- [x] An unknown style name falls back rather than losing the text or panicking.
- [x] A heading level beyond 6 still resolves (`level` is a `u8`; nothing stops a document declaring 99).
- [x] An image block resolves to a default style — `resolve` is total.
- [x] Styles round-trip through JSON; a manifest without them gets the defaults, with no `FORMAT_VERSION` bump.
- [x] A heading lays out at its style's size and leading, not body's.
- [x] The same text at h1 size breaks into **more lines** than at body size — proving style reaches *measurement*, not just drawing. If it only reached the writer, text would be drawn larger than the space reserved for it.
- [x] Space-above appears as a gap between the previous block's bottom and the heading's frame.
- [x] A body-only document is positioned exactly as before styles existed (blocks at y = 0, 12, 24, 36, 48).
- [x] Performance budgets still met.

## The exported PDF changes, as intended

`SAMPLE_EXPORT_DIGEST` is updated. Verified by reading the emitted text operators rather than by
accepting the new number:

| | before | after |
|---|---|---|
| Font sizes in the content stream | `/F0 10 Tf` | `/F0 24 Tf` *and* `/F0 10 Tf` |
| First baselines | 646.64, 634.64, … | 611.14 (heading), 590.64, 578.64, … |

The sample's h1 now sets at 24 pt with space above, and the body text below it moved down
accordingly. This is the increment's entire point made visible.

## Non-goals

- **Character styles** — inline runs (bold, italic, small caps) inside a paragraph. The `Block` text
  is still a flat `String` with no run structure, so there is nothing to attach a character style
  to; adding runs is a model change of its own size and does not belong bundled with this one.
  (The spec index entry is named "paragraph styles" for that reason.)
- Per-block font *selection*. There is still one font per document, an `ExportOptions` field rather
  than a model field.
- Style inheritance or cascading. Styles are flat.
- Baseline-grid snapping, which interacts with leading and is a separate concern.
