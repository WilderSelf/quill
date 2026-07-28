# 0037 — Decoration primitive: fills, rules and borders

**Milestone:** M2 · **Status:** implemented

## Why

Nothing in the workspace can draw a line or a box. `PlacedBlock` had exactly two variants, `Text`
and `Image`; `PaintOp` had four, none of which was a rectangle fill or a stroke, and `TrimGuide` is
a screen-only viewport affordance that is deliberately never rasterized into press output.

A stat block (spec 0038) is a tinted, ruled, padded box containing several differently-styled
lines. A random table (0039) has zebra rows and rules. Neither can be built on text and images
alone, so the primitive has to exist first — and it lands **at parity**, with nothing in the model
producing one yet, so no existing document changes and the export byte-hash holds.

The increment's other half is the part that matters more.

## What

### The primitive

```rust
pub enum PlacedBlock {
    // ...
    Rect { frame: Rect, fill: Option<Color>, stroke: Option<Stroke> },
}

pub struct Stroke { pub color: Color, pub width_pt: Pt }
```

Both halves optional, so one variant expresses a rule (stroke only), a tint (fill only) and a panel
(both). A rect with neither, or with a zero dimension, draws **nothing** — it emits no paint op and
no PDF operator at all, rather than a path with no paint operator, which is a malformed content
stream.

`PaintOp::Rect` mirrors it for the screen, `tiny-skia` rasterizes it, and the PDF writer emits
`re` plus `f` / `S` / `B`. Stroke width is in **points**, unscaled: no CTM is in effect at that
point in the stream, so the `w` operand is the width the author asked for. A hairline that silently
becomes device-dependent is the classic press bug in this code.

Fill and stroke colours are set through *separate* operators (`k`/`g` versus `K`/`G`). Using the
fill operator for a stroke sets the wrong colour with no error anywhere in the pipeline, which is
why the writer tests assert on the exact operator rather than on "a colour was set".

### Preflight learns about geometry

This is the load-bearing half. Both press colour checks walk `doc.content`:
`preflight` matches `Block::Heading | Block::Body` for a colour and returns `None` for images. A
`PlacedBlock::Rect` is produced by the *layout engine*, carries its own colours, and reaches the page
without ever having been a `Block` — so on the day 0038 emits a panel tinted at 280% total ink, it
would sail past preflight and into a print shop. That is exactly the silent-press-corruption class
`CLAUDE.md` forbids, and it is invisible to every test that only looks at text and images.

Add `preflight_pages(&[LaidOutPage]) -> Vec<Finding>`, checking every decoration's fill and stroke
for RGB and for the 240% ink limit, and run it inside `export` **on the pages it already laid out**
rather than laying the document out a second time inside `preflight`. The model-level checks stay
where they are; the geometry-level ones run against what the writer is about to draw.

Public, so it can be tested directly rather than only through a full export — the tests construct a
`PlacedBlock::Rect` by hand, because nothing produces one yet. That is the point: the check exists
before the thing it guards, which is spec 0013's lesson (the validator and the writer must agree on
the rule before anything relies on it).

## Acceptance criteria

- Regression: `Document::sample()`'s export byte-hash matches the committed constant; every existing
  export-pdf and render test passes untouched; Ghostscript CI green.
- A filled and stroked rect emits its fill colour, its **stroking** colour operator, `re`, the width
  in points, and `B` — asserted on the decompressed content stream, as the existing writer tests do.
- Geometry is flipped exactly: a rect at top-left `y = 30`, height 40, on a 648 pt trim emits
  `20 587 100 40 re`, derived through the same offsets `geom::flip` uses rather than a second
  convention. Asserted as an exact substring.
- A fill-only rect uses `f` and does not stroke; a stroke-only rect uses `S` and `G` and does not
  fill. Both asserted, both directions.
- A rect with no colours, zero width, or zero height emits no `re` at all — asserted for all three.
- On screen: a decoration paints with the page offset applied, paints *before* the text it sits
  behind (asserted on op order, the golden artifact), and emits no op when it would draw nothing.
- Rasterization: a filled rect puts its colour inside its bounds and leaves paper outside; a stroked
  rect draws its edges and not its middle. Coarse invariants only, per spec 0033 — no pixel golden.
- **Preflight sees it.** A decoration at 280% ink is a `Severity::Error` `InkCoverage` finding; an
  RGB fill *and* an RGB stroke are each a `ColorSpace` error; a press-legal decoration and an empty
  page produce no findings. All four asserted — the last is what stops the first three passing
  against a checker that simply reports everything.
- `benches/budgets.toml` unchanged: this increment adds no work to any measured path.

## Test strategy

Parity first: the export byte-hash plus the untouched existing suites prove the seam is inert.
Capability tests construct `PlacedBlock::Rect` directly rather than through a document, which is how
this repo proves a parameter is genuinely threaded through.

The preflight tests are the load-bearing ones and were written to fail against the pre-0037 checker.
Both directions are asserted for every check, because a one-directional colour test passes against
an implementation that flags everything.

## Risks

- **Preflight's shape changed.** It is now two functions with two inputs — a model check and a
  geometry check — rather than one. The alternative, laying the document out inside `preflight`,
  would have doubled the work on a 500-page document and made a cheap call expensive. The cost of
  the split is that a caller who runs only `preflight` no longer sees everything `export` checks;
  that is stated on both functions.
- **Stroke rendering differs between the two backends by construction.** The rasterizer draws four
  filled bars rather than a stroked path, so screen and page agree about which pixels a sub-pixel
  width covers instead of relying on two strokers rounding identically. Corners are square; a
  future join style would need real path stroking.
- PDF colour-space operators differ per space, and getting the fill/stroke pair wrong is a silent
  colour bug. That is why the operand assertions are exact rather than "a colour was set".
