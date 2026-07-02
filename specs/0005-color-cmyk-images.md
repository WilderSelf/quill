# 0005 — Color CMYK image embedding

- **Milestone:** M0
- **Status:** implemented
- **Crates:** `quill-color` (owner of conversion), `quill-export-pdf` (owner of embedding)

## Goal

Let `quill export` embed **color raster art as real CMYK image data** in the PDF/X output,
converted through the OutputIntent profile when possible. Grayscale images keep their exact
current `/DeviceGray` path (byte-stable). This finally wires `lcms2` into the `color` crate as the
architecture intends.

## Background / why

The product exists to print **art-heavy color** TTRPG books. Yet today every color image is
silently destroyed: `images::decode_gray` desaturates every RGB/RGBA PNG to Rec.601 luma and
emits it as a single-channel `/DeviceGray` XObject. An author's color map or full-page
illustration exports as flat grayscale with no warning — the worst possible failure for the target
user. This spec makes color art survive as CMYK, the only color space PDF/X-1a permits for image
content.

## Scope — conversion + colorspace selection

The subsetting/geometry/OutputIntent machinery does not change. Only image decoding, the RGB→CMYK
conversion, and the image XObject's declared color space change. JPEG, ink clamping, alpha, and
indexed/16-bit PNG are explicit non-goals.

## Hard requirements

1. **CMYK conversion lives in `quill-color`.** A `RgbToCmyk` converter is the single place image
   color conversion happens. `RgbToCmyk::from_output_profile(icc_bytes: &[u8]) -> Self` builds an
   `lcms2` transform `sRGB (RGB_8) → OutputIntent profile (CMYK_8)`, Perceptual intent.
   `convert(&self, rgb: &[u8]) -> Vec<u8>` maps `3·n` input bytes to `4·n` output bytes (C,M,Y,K).
2. **Graceful fallback for non-convertible profiles.** A CMYK output profile that lacks the `BToA`
   tables needed as a transform *destination* (including the synthesized test profile from
   `icc::synth_cmyk_profile`) cannot drive an lcms2 transform. In that case `from_output_profile`
   falls back to the deterministic vectorized naive conversion (`naive_rgb_to_cmyk`, scaled to
   `u8`) and reports `mode() == Naive`; a convertible profile reports `mode() == Icc`. Conversion
   never fails and never panics.
3. **Correct ink polarity, no `/Decode` array.** lcms2 `CMYK_8` (non-`_REV`) and the naive path
   both encode 0 = 0% ink … 255 = 100% ink, matching PDF `/DeviceCMYK` (sample 0 → 0.0). White RGB
   → ~(0,0,0,0) (blank paper); black RGB → high K. No `/Decode` inversion is emitted.
4. **Grayscale path unchanged & byte-stable.** `Grayscale`/`GrayscaleAlpha` PNGs (alpha dropped)
   still decode to one byte/pixel and emit `/DeviceGray`, identical bytes to before this spec. Only
   `Rgb`/`Rgba` (alpha dropped) route through `RgbToCmyk` and emit `/DeviceCMYK`, 4 components.
5. **Unsupported inputs still skip, not fail.** Indexed, non-8-bit, missing, or undecodable assets
   return `None` and are skipped by the writer (unchanged), so the default sample still exports.
6. **No preflight/CLI signature change.** Color *images* are already permitted (only RGB *content
   colors* are rejected); `--icc` already supplies the profile used for the transform.

## Public surface

```text
// quill-color
pub struct RgbToCmyk { /* ... */ }
pub enum ConvMode { Icc, Naive }
impl RgbToCmyk {
    pub fn from_output_profile(icc_bytes: &[u8]) -> Self; // never fails; falls back to Naive
    pub fn mode(&self) -> ConvMode;
    pub fn convert(&self, rgb: &[u8]) -> Vec<u8>;         // 3·n bytes -> 4·n bytes
}

// quill-export-pdf::images
pub struct DecodedImage { pub width: u32, pub height: u32, pub pixels: Pixels }
pub enum Pixels { Gray(Vec<u8>), Cmyk(Vec<u8>) }          // Cmyk is width*height*4 bytes
pub fn resolve(asset: &Asset, base_dir: &Path, cmyk: &RgbToCmyk) -> Option<DecodedImage>;
pub fn decode(bytes: &[u8], cmyk: &RgbToCmyk) -> Option<DecodedImage>;
```

## Acceptance criteria

- **`quill-color`:** white RGB → `(0,0,0,0)`, black RGB → `(_,_,_,255)` in fallback mode; output
  length `== px·4`; a non-CMYK/garbage profile yields `mode() == Naive`.
- **`images` regression:** existing grayscale tests (`decodes_bundled_grayscale`, missing-file,
  garbage-bytes) pass unchanged, now returning `Pixels::Gray`.
- **`images` color path:** a small RGB PNG built in-memory with the `png` encoder decodes to
  `Pixels::Cmyk` of `width·height·4` bytes.
- **Export-level:** a document placing an RGB image produces a PDF whose image XObject declares
  `/DeviceCMYK`; the bundled-grayscale export still declares `/DeviceGray`; Ghostscript interprets
  both without error (CI well-formedness gate).

## Non-goals (fast-follows, each its own later spec)

- **JPEG / `DCTDecode`** decode (the `png` crate can't; skipped as today).
- **Per-pixel ink-coverage enforcement/clamping** (≤240%) on images — a well-behaved OutputIntent
  profile with GCR/UCR bounds totals; genuine clamping is its own spec.
- **ICC-accurate numeric conversion validated in CI** — no free tool certifies PDF/X and CI has no
  B2A-equipped profile, so numeric fidelity is proven via the project's periodic real-profile/POD
  uploads; CI proves structural correctness (`/DeviceCMYK`, 4 components) and fallback numerics.
- **Alpha / `/SMask`** (dropped, preserving the no-transparency invariant), **indexed** and
  **16-bit** PNG, **spot color**.
