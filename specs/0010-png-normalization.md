# 0010 — PNG input normalization (indexed + 16-bit)

**Milestone:** M0 **Status:** implemented

## Problem

`decode_png` handled only 8-bit grayscale/RGB(A) PNG. **Indexed** (palette) and **16-bit** PNGs
were silently dropped (`return None`), so the image simply vanished from the export with no
warning — the failure mode spec 0008 named "the worst kind." Specs 0005 and 0006 explicitly
deferred indexed/16-bit PNG to a later spec; this is that spec.

## Behavior

`decode_png` asks the `png` decoder to normalize every input to 8-bit color before reading:

```rust
decoder.set_transformations(png::Transformations::normalize_to_color8()); // EXPAND | STRIP_16
```

- `EXPAND`: palette → RGB (or RGBA when a `tRNS` chunk is present); sub-8-bit grayscale → 8-bit;
  `tRNS` → alpha channel.
- `STRIP_16`: 16-bit samples → 8-bit (high byte).

After normalization the decoder only ever yields 8-bit Grayscale / GrayscaleAlpha / RGB / RGBA,
which the existing arms already handle: grayscale stays `/DeviceGray`; color is converted through
`RgbToCmyk` (including the ≤240% ink clamp, spec 0006) to `/DeviceCMYK`. Alpha is dropped, keeping
the "no live transparency" invariant.

The pre-existing `bit_depth != Eight` guard and `ColorType::Indexed` match arm remain as defensive
fallbacks (unreachable for real inputs once normalization is applied).

## Inputs / outputs

- **Input:** any PNG — grayscale/RGB/RGBA/palette, 1/2/4/8/16-bit.
- **Output:** a `DecodedImage` (`Gray` or `Cmyk`), identical in shape to the existing 8-bit path.
- Non-PNG dispatch and JPEG handling are unchanged.

## Acceptance criteria

- An indexed PNG decodes (palette expanded → CMYK) instead of being dropped.
- A 16-bit PNG decodes (stripped to 8-bit) instead of being dropped.
- Existing 8-bit grayscale/RGB/RGBA PNG behavior is unchanged (transforms are no-ops there).
- The ink-coverage clamp still applies to converted color pixels.

## Non-goals

- **CMYK / 16-bit (`L16`) JPEG** — still deferred (spec 0008 non-goal; CMYK JPEGs carry the
  Adobe-APP14 inversion wrinkle).
- Preserving a wider gamut than 8-bit in the CMYK pipeline (M0 embeds 8-bit CMYK/gray).
- Emitting a preflight warning for still-undecodable images — a possible later preflight spec.
