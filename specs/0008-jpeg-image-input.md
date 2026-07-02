# 0008 — JPEG image input

- **Milestone:** M0
- **Status:** implemented
- **Crates:** `quill-export-pdf` (owner of decode/embedding), `quill-color` (reused converter)

## Goal

Let `quill export` accept **JPEG** linked art, not just PNG. A JPEG asset decodes and embeds as
press-legal `/DeviceCMYK` (color) or `/DeviceGray` (grayscale) image data, reusing the spec-0005
`RgbToCmyk` converter and the spec-0006 ≤240% ink clamp. This closes the biggest remaining
image-input gap for the product's art-heavy target user.

## Background / why

The product exists to print **art-heavy color** TTRPG books, and JPEG is the overwhelmingly common
delivery format for that art. Yet before this spec, `images::decode` ran every asset through the
`png` crate alone and returned `None` for anything else — and the writer **silently skips** a
`None` asset (spec 0005 req 5). So an author who placed a `.jpg` illustration got a book with the
art missing and *no warning*: the worst kind of failure. This was the explicit deferred non-goal of
spec 0005 ("JPEG / `DCTDecode` — the `png` crate can't; skipped as today").

## Approach — decode to pixels, do **not** `/DCTDecode`-passthrough

The obvious "embed the JPEG bytes verbatim under `/DCTDecode`" is **not PDF/X-1a-legal** for the
common case. A typical author JPEG is YCbCr and decodes to RGB; embedded verbatim it is a
`/DeviceRGB` image, which violates the CMYK-only rule (spec 0001 req #2). So Quill **decodes** JPEG
to pixels and routes them through the exact same path as PNG color art: RGB → CMYK via
`RgbToCmyk::convert` (which already clamps to ≤240% ink), grayscale stays `/DeviceGray`, and the
result is embedded through the existing `/FlateDecode` XObject writer. This trades away DCT
compression (the re-embedded stream is larger) for guaranteed conformance and **zero changes** to
the writer, color, preflight, and layout layers — the whole change is one decode seam.

## Scope

Only image *decoding* changes. `decode` gains a magic-byte sniffer that dispatches PNG (unchanged)
vs JPEG (new). The writer's image loop, the `RgbToCmyk` converter, preflight
(`ImageResolution`/`InkCoverage`, which use author-declared `Asset.dpi`), and `layout-engine`
(square placeholder sizing) are untouched.

## Hard requirements

1. **Format dispatch.** `images::decode(bytes, cmyk)` selects by leading magic bytes: PNG
   (`89 50 4E 47 0D 0A 1A 0A`) → the existing PNG path; JPEG (`FF D8 FF`) → the new JPEG path;
   anything else → `None`. `resolve` is unchanged (it delegates to `decode`).
2. **JPEG pixel matrix.** Decoding uses `jpeg-decoder`. `PixelFormat::L8` → `Pixels::Gray` (one
   byte/pixel, `/DeviceGray`); `PixelFormat::RGB24` → `Pixels::Cmyk(cmyk.convert(&data))`
   (`/DeviceCMYK`, ≤240% ink by the reused clamp). `PixelFormat::CMYK32` and `L16` → `None`.
3. **Skip, never fail.** A decode error, an unsupported pixel format, or a truncated file returns
   `None` (the writer skips it), exactly like an undecodable PNG. Export of the rest of the
   document still succeeds; nothing panics.
4. **PNG path byte-stable.** The sniffer must route real PNG bytes to the unchanged PNG decoder;
   existing PNG grayscale/color/clamp/missing-file/garbage tests pass unchanged.
5. **Permissive dependency.** `jpeg-decoder` (MIT OR Apache-2.0, pure Rust, image-rs family) with
   `default-features = false` to drop its `rayon` parallel-decode feature — no FreeType/GPL and a
   lean dep graph.
6. **No public-surface / CLI / preflight change.** `DecodedImage`/`Pixels` types, `resolve`/`decode`
   signatures, `ExportOptions`, and every `CheckId` are unchanged; `quill export` picks JPEG up with
   no new flag.

## Acceptance criteria

- **`images` unit:** a committed grayscale JPEG fixture decodes to `Pixels::Gray` of `w*h` bytes; a
  committed RGB JPEG fixture decodes to `Pixels::Cmyk` of `w*h*4` bytes with every 4-byte group
  summing ≤ 612; the sniffer still routes the PNG fixture to `Pixels::Gray`; a truncated JPEG →
  `None`. (JPEG is lossy, so tests assert structure, not exact pixel bytes.)
- **Export-level:** a `Document` placing the RGB JPEG fixture exports a PDF whose image XObject
  declares `/DeviceCMYK`; Ghostscript interprets it without error (CI well-formedness gate).
- **Regression:** the existing 66 tests stay green; default (PNG) export bytes unchanged.

## Non-goals (each its own later spec)

- **`/DCTDecode` passthrough / CMYK-JPEG re-encoding** to preserve compression (needs a CMYK-JPEG
  encoder).
- **CMYK (`CMYK32`) JPEG input** — the common Adobe-APP14 transform-0 case is now handled by
  [spec 0012](0012-cmyk-jpeg-input.md); YCCK (transform 2) and **16-bit (`L16`)** JPEG input remain
  deferred.
- **Auto-deriving `Asset` pixel dimensions / true aspect-ratio sizing** in `layout-engine` (images
  are still placed as a full-width square) — shared with the PNG path, separate layout work.
- **JPEG alpha** (JPEG has none) and **indexed/other** exotic inputs.
