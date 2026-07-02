# Plan: image aspect-ratio sizing (spec 0009)

## Task (restated)
Linked images are currently placed as a **square at full content width** because `Asset`
carries no pixel dimensions. This distorts every non-square image in press output. Give `Asset`
pixel-dimension fields and size image frames from pixels + DPI, preserving aspect ratio and
scaling down to fit the content width. M0 fast-follow, derivable from specs 0001/0002 and the
plan's M0 "an image over N pages" requirement — not net-new product scope.

## Acceptance criteria
- `core-model::Asset` gains `px_w: u32` and `px_h: u32` (serde `default` for back-compat).
- `layout-engine::lay_out` sizes an image block from the asset:
  - natural size in points: `w = px_w / dpi * 72`, `h = px_h / dpi * 72`.
  - if natural width exceeds the content width, scale both dims down proportionally so width
    equals the content width (aspect ratio preserved).
  - if natural width fits, place at natural size.
  - **fallback**: if `px_w == 0 || px_h == 0 || dpi <= 0`, keep the legacy square-full-width
    placeholder (documents without pixel info still lay out).
  - pagination behaviour unchanged (new page when the block would overflow a non-empty page).
- The documented square-placeholder stopgap comment in `layout-engine` is removed/updated.
- New spec `specs/0009-image-sizing.md` (status implemented) + row in `specs/README.md`.
- `Asset::sample`/`Document::sample` and all `Asset { .. }` test literals updated with pixel dims.
- `ImageResolution` preflight behaviour unchanged (still reads `asset.dpi`).

## Files to touch
- `crates/core-model/src/lib.rs` — add fields; update `sample()`.
- `crates/layout-engine/src/lib.rs` — image sizing math + fallback; tests.
- `crates/export-pdf/src/lib.rs` — update Asset literals in tests.
- `crates/export-pdf/src/images.rs` — update Asset literal in test.
- `specs/0009-image-sizing.md` — new.
- `specs/README.md` — index row.

## Test strategy
- layout-engine: unit tests for (a) landscape image scaled to content width with correct
  aspect ratio, (b) small image placed at natural size, (c) zero-pixel fallback → square.
- core-model: existing JSON round-trip covers the new fields (serde).
- export-pdf: existing image-embedding tests keep passing with pixel dims added.
- Full `cargo fmt/clippy/build/test --workspace`.
