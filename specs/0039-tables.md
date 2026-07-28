# 0039 — `Block::Table`

**Milestone:** M2 · **Status:** implemented

## Why

The other half of `quill-components-ttrpg`: `RandomTable` had `lookup` and `is_complete`
implemented and tested, and no way to put one on a page. And a rulebook is full of *ordinary*
tables — equipment, prices, encounter difficulty — which the model could not express at all.

## What

### A general table, not a random-table renderer

```rust
pub struct Table {
    pub columns: Vec<f32>,          // fractions of the measure, normalized on use
    pub header: Option<Vec<String>>,
    pub rows: Vec<Vec<String>>,
    pub zebra: bool,                // default true
}
```

`Table::from_random(&RandomTable)` builds the two-column die-range form, so a random table is the
special case rather than the only case. `Block::Table { id, table, color }` places one.

Column widths are **normalized**, so `[1, 3]` and `[0.25, 0.75]` mean the same thing — an author
should not have to make them sum to one, and taking them literally would run the table off the
frame. A zero, negative, non-finite or miscounted width falls back to an equal split rather than
failing: the same authoring posture as spec 0030's over-wide gutter, where bad input costs the look
and never the content. A degenerate column would silently swallow its cells, which is the outcome
worth avoiding.

Zebra banding defaults **on**. A wide table without banding is hard to read across, and it is
exactly the kind of thing a beginner would not think to switch on.

### Built on spec 0038's composite seam

A cell is a `PanelPart` and a zebra band is a `PanelRect`, so tables and stat blocks share one
placement path. That required generalizing the panel's `rules: Vec<f32>` into
`decorations: Vec<PanelRect>` — a section rule and a zebra band are the same thing, a filled
rectangle at an offset, and one list means paint order is decided in one place.

Row height is the **tallest cell in the row**, so a wrapped cell pushes its row down rather than
overlapping the one beneath. Cell padding comes off the *measure*, not only the position, so a
wrapped cell stays inside its column instead of being broken to the full column width and then
drawn inset.

## Scope: row-breaking and header repetition are not in this increment

The roadmap's 0039 entry promised breaking between rows with the header repeating on the
continuation. Both are blocked on the same thing spec 0038 hit: **a block never splits across
frames.** A table is one block, so it cannot break between rows, and with no continuation there is
nothing for a header to repeat onto.

This is not a table-specific gap and it should not get a table-specific fix. The roadmap's known
issue now records that paragraphs, stat blocks and tables all want one splitting mechanism, and why
it needs its own spec (it puts available height into the measurement-cache key). A 500-row table is
placed and overflows its frame, the same as any oversized block — asserted, so that at least no cell
is lost.

`RandomTable::is_complete`'s gap warning is also deferred. It belongs to the `RandomTable`, which
the model does not retain after conversion; retaining it would store the same content twice. The
check exists and is tested on the component, so a CLI or app affordance can surface it without the
document carrying a derived copy.

## Acceptance criteria

- Regression: the export byte-hash **changes** (two new `table-*` styles in
  `StyleSheet::default()`), verified identifier-only the same way spec 0038's move was: both files
  8559 bytes, 120 differing bytes, every one inside the XMP `DocumentID`/`InstanceID` or the trailer
  `/ID`. No content stream moved. Ghostscript CI green.
- Exact column geometry: a 432 pt frame with widths `0.25/0.75` puts cell text at `x = 3` and
  `x = 111`, measuring 102 and 318 pt, after the 3 pt cell padding. Asserted to 0.01 pt.
- `[1, 3]` normalizes to the same geometry as `[0.25, 0.75]`.
- A zero, negative or miscounted width falls back to an equal split and **loses no cell** — asserted
  for all three degenerate forms.
- A wrapped cell pushes its whole row down: the next row's `y` clears the wrapped cell's full
  height. Asserted against the measured line count, not a hard-coded offset.
- Zebra shades alternate rows and, switched off with no header, produces **no decoration at all**.
  Both directions, so it cannot pass against an implementation that always or never bands.
- Decoration paints before the cells it sits behind — asserted on placement order.
- An empty table occupies nothing and does not panic.
- `Table::from_random` renders `1-3` for a span and `4` for a single value — the singleton case is
  the one that ships and then embarrasses.
- A 500-row table places every one of its 1,000 cells.
- The non-ASCII font-subset guard covers headers and cells (the spec-0026 `.notdef` case).
- Cache correctness: every cell, the header, the widths and the zebra flag are in the content
  fingerprint.
- `benches/budgets.toml` unchanged; `quill-testdoc` emits no tables.

## Test strategy

Geometry as exact arithmetic through `MonospaceRunMetrics`. The load-bearing ones are the
degenerate-width cases (which assert no cell is lost, not merely that nothing panicked), the
wrapped-row test (asserted against the actual wrapped line count), and the both-directions zebra
test.

## Risks

- **The panel seam now serves two callers.** Generalizing `rules` to `decorations` was the right
  move but it means a change for one component silently affects the other; the stat block's rule
  tests are what catch that.
- Placement previously emitted a panel rect unconditionally. A table has no outer panel, so an
  invisible `Rect` was travelling through the whole pipeline until the tests caught it — spec 0037's
  "a rectangle drawing nothing emits nothing" rule now applies at placement as well as in the
  writer.
- Row-breaking's absence is a real limitation for a long table and is recorded rather than hidden.
