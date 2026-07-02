# 0009 — Image placement sizing (true aspect ratio)

**Milestone:** M0 **Status:** implemented

## Problem

The layout engine placed every linked image as a **square at the full content width**, because
`Asset` carried no pixel dimensions. Any non-square image was therefore distorted in the exported
PDF — unacceptable for press output. Spec 0002 already assumes the correct formula
(`w_pt = px_w / dpi * 72.0`), and spec 0008 lists "auto-deriving `Asset` pixel dimensions / true
aspect-ratio sizing in `layout-engine`" as a deferred fast-follow. This spec is that follow-up.

## Behavior

- `Asset` gains two fields: `px_w: u32`, `px_h: u32` — the source image's pixel dimensions. Both
  are `#[serde(default)]` (default `0`) so pre-existing manifests still deserialize.
- When laying out a `Block::Image`, the engine computes the placed frame from the asset's pixels
  and DPI:
  - **Natural size** in points: `w = px_w / dpi * 72`, `h = px_h / dpi * 72`.
  - **Fit to width:** if the natural width exceeds the content width, scale both dimensions down
    proportionally so the width equals the content width; the aspect ratio is preserved.
  - **Otherwise** place at natural size.
- **Fallback:** if `px_w == 0`, `px_h == 0`, or `dpi <= 0` (dimensions unknown), fall back to the
  legacy square, full-content-width placeholder so such documents still lay out without error.
- Pagination is unchanged: a block that would push past the page height starts a new page (unless
  the page is empty).

## Inputs / outputs

- **Input:** a `Document` whose `Block::Image` blocks reference `Asset`s with `px_w`/`px_h`/`dpi`.
- **Output:** `LaidOutPage`s whose `PlacedBlock::Image` frames carry the correct placed size. The
  PDF writer already scales the image XObject to `frame.w_pt × frame.h_pt`, so correct frames
  yield correct on-page geometry with no writer change.

## Acceptance criteria

- A landscape image wider than the content area is scaled to the content width with its aspect
  ratio preserved (`w/h` unchanged).
- An image smaller than the content width is placed at its natural physical size.
- An asset with unknown (`0`) pixel dimensions falls back to the square placeholder.
- Existing preflight checks (`ImageResolution` via `Asset.dpi`) are unaffected.

## Non-goals

- Decoding the image to *measure* pixel dimensions automatically (they are author-declared here,
  mirroring `dpi`/`line_art`/`has_alpha`). Auto-measurement can come later.
- Explicit per-frame image sizing/cropping/fit modes (an M1 layout-engine concern).
- **Height fitting.** Sizing fits to *width* only. An image narrower than the content width but
  taller than the page is placed at its natural height and overflows the page bottom (the legacy
  square placeholder could never exceed the page height, so this state is newly reachable).
  Height-aware fitting or splitting is deferred to M1's real layout engine.
