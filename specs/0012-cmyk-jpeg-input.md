# 0012 — CMYK JPEG input

- **Milestone:** M0
- **Status:** implemented
- **Crates:** `quill-export-pdf` (owner of decode/embedding), `quill-color` (reused ink clamp)

## Goal

Let `quill export` accept a **CMYK JPEG** linked asset instead of silently dropping it. A CMYK JPEG
carrying an Adobe **APP14 color-transform 0** marker decodes to true-ink CMYK and embeds as
press-legal `/DeviceCMYK`, reusing the spec-0006 ≤240% ink clamp. This closes the deferred CMYK case
from spec 0008 for the product's art-heavy target user.

## Background / why

The product exists to print **art-heavy color** TTRPG books for print-on-demand, and the common
delivery format for CMYK-separated art out of Photoshop is a **CMYK JPEG**. Before this spec,
`images::decode_jpeg` returned `None` for `PixelFormat::CMYK32` (spec 0008 non-goal), and the writer
**silently skips** a `None` asset (spec 0005 req 5). So an author who placed a CMYK `.jpg` got a book
with the art missing and *no warning* — the worst kind of failure, and precisely in the workflow the
product targets.

## Approach — accept only the unambiguous Adobe transform-0 case

Two facts (verified against `jpeg-decoder` 0.3.2 source + an out-of-tree round-trip) shape the scope:

1. `jpeg-decoder` reports `PixelFormat::CMYK32` for **both** true-CMYK (Adobe APP14 transform 0) and
   **YCCK** (transform 2) JPEGs, and `ImageInfo` does not expose which transform was used.
2. For transform-0 CMYK, the decoder emits `255 - stored`; Adobe stores CMYK **inverted**, so the
   decoder output is **true ink directly** — no inversion needed (confirmed empirically:
   white→`(0,0,0,0)`, solid K→`(0,0,0,255)`, rich black→`(255,255,255,255)`). For **YCCK**, the
   decoder emits `[R,G,B,255-K]` in the same CMYK32 buffer — unusable as CMYK without an RGB→CMYK
   reinterpretation.

Because the pixel format alone can't disambiguate the two, and because emitting **wrong color** to a
press file is worse than a visibly-missing image the author will notice, Quill accepts a CMYK JPEG
**only** when it carries an Adobe APP14 marker with transform byte `0`. That case is decoded as
true-ink CMYK, clamped to ≤240% ink (`clamp_cmyk_u8`), and embedded through the existing
`/FlateDecode` `/DeviceCMYK` XObject writer. YCCK, markerless, and otherwise-ambiguous CMYK JPEGs
continue to return `None` (skipped) — no regression, never mis-colored.

## Scope

Only image *decoding* changes, inside `images::decode_jpeg`. The writer's image loop, the
`RgbToCmyk` converter (unused on this path — the data is already CMYK), preflight, and `layout-engine`
are untouched. The change reuses `quill_color::clamp_cmyk_u8` directly rather than routing through
`RgbToCmyk::convert` (which is an RGB→CMYK transform and would be wrong for already-CMYK data).

## Hard requirements

1. **Transform gate.** `decode_jpeg` accepts `PixelFormat::CMYK32` **iff** the JPEG carries an Adobe
   APP14 (`FF EE`) segment whose payload begins `Adobe\0` and whose transform byte (payload index 11)
   is `0`. Otherwise `CMYK32` returns `None` (unchanged skip behavior). `L16` still returns `None`.
2. **True-ink, no inversion.** For an accepted transform-0 CMYK JPEG the decoder output is already
   true ink; each pixel is clamped with `clamp_cmyk_u8(c,m,y,k)` and emitted as `Pixels::Cmyk`
   (four bytes/pixel, `/DeviceCMYK`). No RGB→CMYK conversion is applied.
3. **Skip, never fail.** A decode error, a non-transform-0 CMYK JPEG (YCCK/markerless), `L16`, or a
   truncated file returns `None` (the writer skips it). Export of the rest of the document still
   succeeds; nothing panics.
4. **Other JPEG/PNG paths byte-stable.** `L8`→`Pixels::Gray` and `RGB24`→`RgbToCmyk::convert`
   (spec 0008) and every PNG path (specs 0005/0006/0010) are unchanged; their tests pass unchanged.
5. **No new dependency.** Uses the existing `jpeg-decoder` dep and a hand-rolled APP14 scan; no
   workspace dep is added. The committed fixture is generated out-of-tree with `jpeg-encoder` per the
   CLAUDE.md test-fixture convention.
6. **No public-surface / CLI / preflight change.** `DecodedImage`/`Pixels`, `resolve`/`decode`
   signatures, `ExportOptions`, and every `CheckId` are unchanged; `quill export` picks CMYK JPEG up
   with no new flag.

## Acceptance criteria

- **`images` unit:** a committed transform-0 CMYK JPEG fixture (`test_cmyk.jpg`) decodes to
  `Pixels::Cmyk` of `w*h*4` bytes with every 4-byte group summing ≤ 612; its rich-black quadrant
  (encoded `255,255,255,255`, pre-clamp sum 1020) is clamped ≤ 612, proving the clamp is applied.
- **Transform gate:** an in-test YCCK-encoded CMYK JPEG (APP14 transform 2) → `None`; a CMYK JPEG
  with its APP14 segment absent → `None`. (JPEG is lossy, so value tests assert structure/bounds.)
- **Export-level:** a `Document` placing the CMYK fixture exports a PDF whose image XObject declares
  `/DeviceCMYK`; Ghostscript interprets it without error (CI well-formedness gate).
- **Regression:** the existing suite stays green; default (PNG) and gray/RGB-JPEG export bytes
  unchanged.

## Non-goals (each its own later spec)

- **YCCK (APP14 transform 2) CMYK JPEGs** — need an RGB→CMYK reinterpretation of the decoder's
  `[R,G,B,255-K]` output.
- **Markerless / non-transform-0 CMYK JPEGs** — the storage convention can't be known, so skipping
  is the only press-safe choice.
- **16-bit (`L16`) JPEG input.**
- **`/DCTDecode` passthrough / CMYK re-encoding** to preserve DCT compression (needs a CMYK-JPEG
  encoder in the workspace).
