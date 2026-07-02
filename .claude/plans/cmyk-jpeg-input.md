# Plan — CMYK JPEG input (spec 0012)

## Task (restated)

`quill export` currently **drops** a CMYK JPEG: `images::decode_jpeg` returns `None` for
`PixelFormat::CMYK32` (deferred by spec 0008), and the writer silently skips a `None` asset. For
the product's art-heavy POD target, a CMYK JPEG is the most likely delivery format from Photoshop,
so dropping it — with no warning — is a real data-loss defect. This increment decodes the common,
unambiguous CMYK-JPEG case (Adobe APP14 color-transform 0) to press-legal `/DeviceCMYK`.

## Key facts established against installed source (jpeg-decoder 0.3.2, jpeg-encoder 0.7.0)

- `jpeg-decoder` returns `PixelFormat::CMYK32` for **both** true-CMYK (APP14 transform 0) and YCCK
  (transform 2) JPEGs, and `ImageInfo` does **not** expose which — so the pixel format alone can't
  disambiguate.
- For APP14 transform-0 CMYK, `color_convert_line_cmyk` emits `255 - stored`; Adobe stores CMYK
  inverted, so the decoder output is **true ink** directly (empirically confirmed: white→(0,0,0,0),
  solid K→(0,0,0,255), rich black (255,255,255,255)→(255,255,255,255)). **No inversion needed.**
- For YCCK (transform 2), the decoder emits `[R,G,B,255-K]` in a CMYK32 buffer — **garbage as
  CMYK**. Supporting it needs an RGB→CMYK reinterpretation → out of scope; stays skipped.
- Therefore: accept **only** JPEGs carrying an Adobe APP14 marker with transform byte `0`. Detect
  via a tiny APP14 scan (jpeg-decoder doesn't expose it). Everything else (YCCK, markerless,
  ambiguous) returns `None` — no regression, never mis-colored.

## Acceptance criteria

- `images::decode` accepts a committed transform-0 CMYK JPEG fixture, returning `Pixels::Cmyk` of
  `w*h*4` bytes with every 4-byte group summing ≤ 612 (the spec-0006 clamp applied).
- The rich-black quadrant (encoded 255,255,255,255, sum 1020) is clamped ≤ 612 — proving the path
  applies `clamp_cmyk_u8` (a naive pass-through would exceed).
- A CMYK JPEG **without** an APP14 transform-0 marker (e.g. the YCCK-encoded fixture, in-test) →
  `None` (skipped, not mis-decoded).
- Export-level: a `Document` placing the CMYK fixture exports a PDF whose image XObject declares
  `/DeviceCMYK`; Ghostscript CI gate interprets it without error.
- Regression: existing tests stay green; grayscale/RGB JPEG and PNG paths byte-unchanged.

## Files to touch

- `specs/0012-cmyk-jpeg-input.md` (new), `specs/README.md` (index row).
- `crates/export-pdf/src/images.rs` — `decode_jpeg` CMYK32 arm + `adobe_transform` APP14 scanner;
  module doc comment. Reuse `quill_color::clamp_cmyk_u8`.
- `crates/export-pdf/assets/test_cmyk.jpg` — committed fixture (8×8, generated out-of-tree with
  jpeg-encoder per the CLAUDE.md fixture convention; not added as a workspace dep).
- Update spec 0008's non-goal note to point at 0012 for the CMYK case (leave 16-bit/YCCK deferred).

## Test strategy

- Unit (`images` tests): decode committed `test_cmyk.jpg` → `Pixels::Cmyk`, size + ≤612 clamp
  assertions; an in-test YCCK JPEG (or a stripped-APP14 buffer) → `None`; PNG/gray/RGB regressions
  untouched.
- Export-level test in `lib.rs`: place the fixture, assert `/DeviceCMYK` present; Ghostscript CI
  gate covers well-formedness.

## Non-goals (unchanged from 0008, still deferred)

- YCCK (APP14 transform 2) and 16-bit (`L16`) JPEG input.
- `/DCTDecode` passthrough / CMYK re-encoding to keep DCT compression.
- Markerless/ambiguous CMYK JPEGs (skipped, since the storage convention can't be known).
