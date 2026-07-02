# 0006 — Per-pixel image ink-coverage clamping (≤240%)

- **Milestone:** M0
- **Status:** implemented
- **Crates:** `quill-color` (owner of conversion + the clamp), `quill-export-pdf` (image pipeline)

## Goal

Guarantee that **every CMYK image pixel** Quill embeds is within the DriveThruRPG total-ink limit
of **240%** (`MAX_INK_COVERAGE_PCT`). Enforcement happens at the single RGB→CMYK conversion
chokepoint, so no linked color image can carry an ink-limit violation into a PDF/X export.

## Background / why

Spec 0005 wired color image embedding: RGB/RGBA art is converted to CMYK via
`RgbToCmyk::convert` — an `lcms2` transform when the OutputIntent profile has usable `BToA`
tables, otherwise a deterministic **naive** conversion (the CI/default path, since the bundled
synth profile has no B2A). Nothing bounded the ink on the result:

- The naive conversion readily exceeds 240%. RGB `(26,0,0)` → CMYK `(0,255,255,229)` =
  `739/255 ≈ 290%`.
- Preflight's `InkCoverage` check only iterates authored text/heading colors; `Block::Image`
  is skipped, so image pixels are never checked.

An over-ink illustration is exactly the press-rejection failure the product must prevent, and it
is not something an author can hand-fix pixel by pixel. Spec 0005 explicitly deferred this
("Per-pixel ink-coverage enforcement/clamping (≤240%) on images … is its own spec").

## Approach — enforce by clamping, not by failing preflight

Image ink is enforced by **clamping at conversion time**, not by a failing preflight check. A
detect-and-reject preflight check is the wrong tool for image content (you cannot repaint
individual pixels to satisfy it), and preflight has no decoded-pixel access — it would have to
re-decode every asset only to confirm what the clamp already guarantees. Clamping inside
`RgbToCmyk::convert`, which both the ICC and naive paths flow through, makes an over-ink image
pixel structurally impossible. No image-pixel preflight pass is added.

## Hard requirements

1. **Clamp function in `quill-color`.** `clamp_cmyk_u8(c, m, y, k: u8) -> [u8; 4]` maps any 8-bit
   CMYK pixel to one whose four samples sum to at most the ink budget. The budget derives from
   the existing `MAX_INK_COVERAGE_PCT` (single source of truth): `240% × 255 = 612` sample units.
2. **K-preserving CMY scale.** A pixel already within budget is returned **unchanged** (byte-
   identical). Over-budget, **K is preserved** and C, M, Y are scaled by `(612 − k) / (c+m+y)`,
   each channel **floored** so the post-scale sum never rounds back over the limit.
3. **Both conversion paths clamp.** `RgbToCmyk::convert` applies `clamp_cmyk_u8` to every output
   pixel in both the `lcms2` and naive branches, so the guarantee is path-independent. A
   well-behaved profile that already respects the limit is unaffected (clamp is a no-op there).
4. **Public surface unchanged.** The spec-0005 API — `RgbToCmyk`, `ConvMode`,
   `from_output_profile`, `mode`, `convert` — keeps its signatures; clamping is an internal
   post-step. `naive_rgb_to_cmyk` (the raw `f32`→`Color` helper) stays unclamped.
5. **Grayscale untouched, no preflight/CLI change.** `/DeviceGray` images are byte-stable; there
   is no new preflight `CheckId`, no CLI flag, and under-limit color pixels are byte-identical to
   spec 0005 output.

## Acceptance criteria

- **`quill-color`:** `clamp_cmyk_u8` leaves an under-limit pixel unchanged, preserves K and scales
  CMY for the `(0,255,255,229)` case to a sum ≤ 612, and is a no-op on a pixel exactly at 612.
  `convert` on RGB `(26,0,0)` (naive) yields a pixel summing ≤ 612.
- **`images`:** an in-memory over-ink RGB PNG decodes to `Pixels::Cmyk` whose every 4-byte group
  sums ≤ 612; the white/black `decodes_rgb_to_cmyk` case and byte-stable grayscale tests remain
  green (values unaffected by the clamp).
- **Export-level:** the CI Ghostscript well-formedness gate still passes; `/DeviceCMYK` and
  `/DeviceGray` image structure is unchanged.

## Non-goals (each its own later spec)

- **ICC-accurate numeric conversion / gamut mapping** — still proven via periodic real-profile /
  POD uploads (no free tool certifies PDF/X, and CI has no B2A-equipped profile).
- **Soft-proofing**, **spot color**, and changing **authored text-color** ink handling (already
  rejected at preflight when over-limit).
- **JPEG / `DCTDecode`**, alpha / `/SMask`, indexed, and 16-bit PNG (deferred by spec 0005).
