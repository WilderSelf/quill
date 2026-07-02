//! Linked-image resolution and decoding for export (spec 0002 reqs 2, 7; spec 0005; spec 0008).
//!
//! Grayscale inputs decode to 8-bit `/DeviceGray` (unambiguously legal, no ICC transform). Color
//! inputs are converted to 8-bit CMYK via [`RgbToCmyk`] and emitted as `/DeviceCMYK`,
//! the only image color space PDF/X-1a permits — so an author's color art survives export instead
//! of being desaturated. A missing or undecodable asset returns `None` and is skipped by the
//! writer rather than failing the whole export. Alpha is dropped (no `/SMask`), preserving the
//! "no transparency" invariant.
//!
//! Both **PNG** and **JPEG** inputs are supported; the format is picked from the leading magic
//! bytes. JPEG is *decoded to pixels and re-embedded as CMYK/gray*, **not** passed through as a
//! `/DCTDecode` stream: a typical author JPEG is YCbCr→RGB, and embedding it verbatim would yield
//! a `/DeviceRGB` image that violates PDF/X-1a's CMYK-only rule (req #2). Decoding routes RGB JPEG
//! through the same [`RgbToCmyk`] converter (and its ≤240% ink clamp) as PNG, so the writer,
//! color, and preflight layers are format-agnostic. A **CMYK JPEG** (already CMYK) is embedded
//! directly as `/DeviceCMYK` after the same ink clamp, but only in the unambiguous Adobe-APP14
//! transform-0 case — see specs/0008-jpeg-image-input.md and specs/0012-cmyk-jpeg-input.md.

use std::path::Path;

use quill_color::{clamp_cmyk_u8, RgbToCmyk};
use quill_core_model::Asset;

/// A decoded image, ready to embed. Grayscale is one byte per pixel (`/DeviceGray`); CMYK is four
/// (`/DeviceCMYK`).
pub struct DecodedImage {
    pub width: u32,
    pub height: u32,
    pub pixels: Pixels,
}

/// Decoded pixel data, tagged by the PDF color space it will be written as.
pub enum Pixels {
    /// One byte per pixel, row-major, `width * height` bytes.
    Gray(Vec<u8>),
    /// Four bytes per pixel (C,M,Y,K), row-major, `width * height * 4` bytes.
    Cmyk(Vec<u8>),
}

/// Resolve `asset.path` against `base_dir` and decode it, converting color via `cmyk`.
///
/// Returns `None` (skip, don't fail) if the file is missing, unreadable, or in a format we don't
/// handle for M0. PNG of any bit depth or color type (grayscale, RGB, palette, 16-bit) is
/// normalized and decoded; JPEG handles 8-bit gray/RGB and Adobe transform-0 CMYK (YCCK/16-bit
/// JPEG remain deferred).
pub fn resolve(asset: &Asset, base_dir: &Path, cmyk: &RgbToCmyk) -> Option<DecodedImage> {
    let path = base_dir.join(&asset.path);
    let bytes = std::fs::read(&path).ok()?;
    decode(&bytes, cmyk)
}

/// Decode PNG or JPEG bytes, dispatched on the leading magic bytes. Grayscale stays gray; color is
/// converted to CMYK via `cmyk`. Unknown/unsupported formats return `None` (skip, don't fail).
pub fn decode(bytes: &[u8], cmyk: &RgbToCmyk) -> Option<DecodedImage> {
    if bytes.starts_with(b"\x89PNG\r\n\x1a\n") {
        decode_png(bytes, cmyk)
    } else if bytes.starts_with(&[0xFF, 0xD8, 0xFF]) {
        decode_jpeg(bytes, cmyk)
    } else {
        None
    }
}

/// Decode PNG bytes: grayscale stays gray, color is converted to CMYK via `cmyk`.
///
/// Inputs are normalized to 8-bit color via `normalize_to_color8` (= `EXPAND | STRIP_16`): palette
/// images expand to RGB(A), sub-8-bit grayscale expands to 8-bit, `tRNS` expands to an alpha
/// channel, and 16-bit samples are stripped to 8-bit. Every PNG therefore reaches the Gray/RGB
/// arms below and flows through the shared CMYK(+240% clamp) path (spec 0010).
fn decode_png(bytes: &[u8], cmyk: &RgbToCmyk) -> Option<DecodedImage> {
    use png::{BitDepth, ColorType};

    let mut decoder = png::Decoder::new(std::io::Cursor::new(bytes));
    decoder.set_transformations(png::Transformations::normalize_to_color8());
    let mut reader = decoder.read_info().ok()?;
    let mut buf = vec![0u8; reader.output_buffer_size()?];
    let info = reader.next_frame(&mut buf).ok()?;

    if info.bit_depth != BitDepth::Eight {
        return None; // defensive: normalization already forces 8-bit output.
    }
    let (w, h) = (info.width, info.height);
    let px = (w as usize) * (h as usize);
    let data = &buf[..info.buffer_size()];

    let pixels = match info.color_type {
        ColorType::Grayscale => Pixels::Gray(data[..px].to_vec()),
        ColorType::GrayscaleAlpha => Pixels::Gray(data.chunks_exact(2).map(|p| p[0]).collect()),
        ColorType::Rgb => {
            debug_assert_eq!(data.len(), px * 3);
            Pixels::Cmyk(cmyk.convert(data))
        }
        ColorType::Rgba => {
            let rgb: Vec<u8> = data
                .chunks_exact(4)
                .flat_map(|p| [p[0], p[1], p[2]])
                .collect();
            Pixels::Cmyk(cmyk.convert(&rgb))
        }
        ColorType::Indexed => return None, // defensive: EXPAND already turns palette into RGB(A).
    };
    Some(DecodedImage {
        width: w,
        height: h,
        pixels,
    })
}

/// Decode baseline/progressive JPEG bytes: 8-bit grayscale (`L8`) stays gray, 8-bit RGB (`RGB24`)
/// is converted to CMYK via `cmyk` (reusing the ≤240% ink clamp). A **CMYK JPEG** (`CMYK32`) is
/// accepted only when it carries an Adobe APP14 marker with color-transform `0` (spec 0012): such a
/// file stores CMYK inverted, so `jpeg-decoder` returns true ink directly and we embed it as
/// `/DeviceCMYK` after clamping to ≤240% ink. YCCK / markerless / ambiguous CMYK JPEGs, and `L16`,
/// are skipped (`None`) — `jpeg-decoder`'s YCCK output is `[R,G,B,255-K]`, unusable as CMYK, and
/// emitting wrong color to a press file is worse than a visibly-missing image. A decode error also
/// returns `None` (skip, don't fail the export).
fn decode_jpeg(bytes: &[u8], cmyk: &RgbToCmyk) -> Option<DecodedImage> {
    use jpeg_decoder::PixelFormat;

    let mut decoder = jpeg_decoder::Decoder::new(std::io::Cursor::new(bytes));
    let data = decoder.decode().ok()?;
    let info = decoder.info()?; // populated once decode() succeeds

    let (w, h) = (info.width as u32, info.height as u32);
    let pixels = match info.pixel_format {
        PixelFormat::L8 => Pixels::Gray(data),
        PixelFormat::RGB24 => Pixels::Cmyk(cmyk.convert(&data)),
        // Only Adobe transform-0 CMYK is unambiguous true-ink CMYK; anything else is skipped.
        PixelFormat::CMYK32 if adobe_transform(bytes) == Some(0) => {
            let clamped = data
                .chunks_exact(4)
                .flat_map(|p| clamp_cmyk_u8(p[0], p[1], p[2], p[3]))
                .collect();
            Pixels::Cmyk(clamped)
        }
        PixelFormat::CMYK32 | PixelFormat::L16 => return None, // deferred (specs 0008, 0012)
    };
    Some(DecodedImage {
        width: w,
        height: h,
        pixels,
    })
}

/// Read the Adobe APP14 color-transform byte from a JPEG, if present.
///
/// Scans marker segments for `FF EE` whose payload begins `Adobe\0`; returns the transform byte
/// (payload index 11: `0` = none/CMYK-or-RGB, `1` = YCbCr, `2` = YCCK). Returns `None` when there is
/// no such marker. `jpeg-decoder` consumes this internally but does not expose it, so the CMYK JPEG
/// path (spec 0012) re-reads it to accept only the unambiguous transform-0 case.
fn adobe_transform(bytes: &[u8]) -> Option<u8> {
    // JPEG is a sequence of `FF <marker> [<len_hi> <len_lo> <payload...>]` segments after the SOI.
    let mut i = 2; // skip SOI (FF D8)
    while i + 4 <= bytes.len() {
        if bytes[i] != 0xFF {
            return None; // not at a marker boundary; give up rather than misparse
        }
        let marker = bytes[i + 1];
        // Standalone markers (RSTn, EOI, TEM) carry no length; SOS begins entropy data. (SOI was
        // already consumed by the `i = 2` skip above.)
        if marker == 0xD9 || marker == 0x01 || (0xD0..=0xD7).contains(&marker) {
            i += 2;
            continue;
        }
        if marker == 0xDA {
            return None; // reached scan data without finding APP14
        }
        let len = u16::from_be_bytes([bytes[i + 2], bytes[i + 3]]) as usize;
        if len < 2 {
            return None;
        }
        let payload = bytes.get(i + 4..i + 2 + len)?;
        if marker == 0xEE && payload.len() >= 12 && payload.starts_with(b"Adobe\0") {
            return Some(payload[11]);
        }
        i += 2 + len;
    }
    None
}

#[cfg(test)]
mod tests {
    use super::*;

    const TEST_PNG: &[u8] = include_bytes!("../assets/test_gray.png");
    // Tiny 8x8 JPEG fixtures (JPEG is lossy + decode-only in `jpeg-decoder`, so unlike the PNG
    // tests these are committed rather than synthesized in-memory). Grayscale is single-component
    // (decodes L8); the color one is a solid-red YCbCr JPEG (decodes RGB24).
    const TEST_JPEG_GRAY: &[u8] = include_bytes!("../assets/test_gray.jpg");
    const TEST_JPEG_RGB: &[u8] = include_bytes!("../assets/test_rgb.jpg");
    // 8x8 CMYK JPEG, Adobe APP14 transform 0 (true-ink CMYK). Four quadrants: white, solid K,
    // solid cyan, and a full rich-black (255,255,255,255) that pre-clamp sums to 1020 (>612).
    // Generated out-of-tree with `jpeg-encoder` per the CLAUDE.md fixture convention (spec 0012).
    const TEST_JPEG_CMYK: &[u8] = include_bytes!("../assets/test_cmyk.jpg");

    /// A converter with no real profile → deterministic naive fallback (fine for tests).
    fn naive_converter() -> RgbToCmyk {
        RgbToCmyk::from_output_profile(b"")
    }

    /// Encode a tiny RGB PNG in-memory (keeps the test deterministic; no committed binary).
    fn rgb_png(width: u32, height: u32, rgb: &[u8]) -> Vec<u8> {
        let mut out = Vec::new();
        {
            let mut enc = png::Encoder::new(&mut out, width, height);
            enc.set_color(png::ColorType::Rgb);
            enc.set_depth(png::BitDepth::Eight);
            let mut writer = enc.write_header().expect("png header");
            writer.write_image_data(rgb).expect("png data");
        }
        out
    }

    /// Encode a tiny indexed (palette) PNG in-memory.
    fn indexed_png(width: u32, height: u32, palette_rgb: &[u8], indices: &[u8]) -> Vec<u8> {
        let mut out = Vec::new();
        {
            let mut enc = png::Encoder::new(&mut out, width, height);
            enc.set_color(png::ColorType::Indexed);
            enc.set_depth(png::BitDepth::Eight);
            enc.set_palette(palette_rgb.to_vec());
            let mut writer = enc.write_header().expect("png header");
            writer.write_image_data(indices).expect("png data");
        }
        out
    }

    /// Encode a tiny 16-bit grayscale PNG in-memory. `samples` are big-endian u16 bytes (PNG order).
    fn gray16_png(width: u32, height: u32, samples_be: &[u8]) -> Vec<u8> {
        let mut out = Vec::new();
        {
            let mut enc = png::Encoder::new(&mut out, width, height);
            enc.set_color(png::ColorType::Grayscale);
            enc.set_depth(png::BitDepth::Sixteen);
            let mut writer = enc.write_header().expect("png header");
            writer.write_image_data(samples_be).expect("png data");
        }
        out
    }

    #[test]
    fn decodes_indexed_png_to_cmyk() {
        // 2x1 palette: index 0 = white, index 1 = black. EXPAND turns it into RGB, then CMYK.
        let png = indexed_png(2, 1, &[255, 255, 255, 0, 0, 0], &[0, 1]);
        let img = decode(&png, &naive_converter()).expect("decode indexed png");
        assert_eq!((img.width, img.height), (2, 1));
        match img.pixels {
            Pixels::Cmyk(c) => {
                assert_eq!(c.len(), 2 * 4, "4 bytes per pixel");
                assert_eq!(&c[0..4], &[0, 0, 0, 0], "white → no ink");
                assert_eq!(&c[4..8], &[0, 0, 0, 255], "black → solid K");
            }
            Pixels::Gray(_) => panic!("indexed PNG must decode to Cmyk"),
        }
    }

    #[test]
    fn decodes_16bit_grayscale_png() {
        // 2x1 16-bit grayscale: 0xFFFF (white), 0x0000 (black). STRIP_16 keeps the high byte.
        let png = gray16_png(2, 1, &[0xFF, 0xFF, 0x00, 0x00]);
        let img = decode(&png, &naive_converter()).expect("decode 16-bit png");
        assert_eq!((img.width, img.height), (2, 1));
        match img.pixels {
            Pixels::Gray(g) => assert_eq!(g, vec![255, 0], "16-bit stripped to 8-bit high byte"),
            Pixels::Cmyk(_) => panic!("grayscale PNG must decode to Gray"),
        }
    }

    #[test]
    fn decodes_bundled_grayscale() {
        let img = decode(TEST_PNG, &naive_converter()).expect("decode test_gray.png");
        assert_eq!(img.width, 8);
        assert_eq!(img.height, 8);
        match img.pixels {
            Pixels::Gray(g) => assert_eq!(g.len(), 64),
            Pixels::Cmyk(_) => panic!("grayscale PNG must decode to Gray"),
        }
    }

    #[test]
    fn decodes_rgb_to_cmyk() {
        // 2x1 RGB: white then black.
        let png = rgb_png(2, 1, &[255, 255, 255, 0, 0, 0]);
        let img = decode(&png, &naive_converter()).expect("decode rgb png");
        assert_eq!((img.width, img.height), (2, 1));
        match img.pixels {
            Pixels::Cmyk(c) => {
                assert_eq!(c.len(), 2 * 4, "4 bytes per pixel");
                assert_eq!(&c[0..4], &[0, 0, 0, 0], "white → no ink");
                assert_eq!(&c[4..8], &[0, 0, 0, 255], "black → solid K");
            }
            Pixels::Gray(_) => panic!("RGB PNG must decode to Cmyk"),
        }
    }

    #[test]
    fn color_pixels_are_clamped_to_ink_limit() {
        // A dark saturated red maps well over 240% ink under the naive path; every emitted
        // CMYK pixel must be clamped to the limit (spec 0006).
        let png = rgb_png(1, 1, &[26, 0, 0]);
        let img = decode(&png, &naive_converter()).expect("decode rgb png");
        match img.pixels {
            Pixels::Cmyk(c) => {
                for px in c.chunks_exact(4) {
                    let sum: u16 = px.iter().map(|&v| v as u16).sum();
                    assert!(sum <= 612, "image pixel exceeds 240% ink: {px:?} = {sum}");
                }
            }
            Pixels::Gray(_) => panic!("RGB PNG must decode to Cmyk"),
        }
    }

    #[test]
    fn missing_file_is_skipped_not_fatal() {
        let asset = Asset {
            id: "x".into(),
            path: "does-not-exist.png".into(),
            px_w: 600,
            px_h: 600,
            dpi: 300.0,
            line_art: false,
            has_alpha: false,
        };
        assert!(resolve(&asset, Path::new("/nonexistent"), &naive_converter()).is_none());
    }

    #[test]
    fn garbage_bytes_decode_to_none() {
        assert!(decode(b"not a png", &naive_converter()).is_none());
    }

    // --- JPEG input (spec 0008). JPEG is lossy, so assert structure, not exact pixel bytes. ---

    #[test]
    fn decodes_grayscale_jpeg_to_gray() {
        let img = decode(TEST_JPEG_GRAY, &naive_converter()).expect("decode gray jpeg");
        assert_eq!((img.width, img.height), (8, 8));
        match img.pixels {
            Pixels::Gray(g) => assert_eq!(g.len(), 8 * 8, "one byte per pixel"),
            Pixels::Cmyk(_) => panic!("grayscale JPEG must decode to Gray"),
        }
    }

    #[test]
    fn decodes_rgb_jpeg_to_clamped_cmyk() {
        let img = decode(TEST_JPEG_RGB, &naive_converter()).expect("decode rgb jpeg");
        assert_eq!((img.width, img.height), (8, 8));
        match img.pixels {
            Pixels::Cmyk(c) => {
                assert_eq!(c.len(), 8 * 8 * 4, "four bytes per pixel");
                for px in c.chunks_exact(4) {
                    let sum: u16 = px.iter().map(|&v| v as u16).sum();
                    assert!(sum <= 612, "jpeg pixel exceeds 240% ink: {px:?} = {sum}");
                }
            }
            Pixels::Gray(_) => panic!("RGB JPEG must decode to Cmyk"),
        }
    }

    #[test]
    fn png_dispatch_is_unchanged_by_sniffer() {
        // The magic-byte sniffer must still route a real PNG through the PNG path.
        let img = decode(TEST_PNG, &naive_converter()).expect("decode png via sniffer");
        assert!(matches!(img.pixels, Pixels::Gray(_)));
    }

    #[test]
    fn truncated_jpeg_decodes_to_none() {
        // Valid JPEG magic but a truncated body → decode error → skip, not panic/fail.
        let truncated = &TEST_JPEG_RGB[..TEST_JPEG_RGB.len() / 2];
        assert!(decode(truncated, &naive_converter()).is_none());
    }

    // --- CMYK JPEG input (spec 0012). Accepted only for Adobe APP14 transform 0. ---

    #[test]
    fn decodes_transform0_cmyk_jpeg_to_clamped_cmyk() {
        let img = decode(TEST_JPEG_CMYK, &naive_converter()).expect("decode cmyk jpeg");
        assert_eq!((img.width, img.height), (8, 8));
        match img.pixels {
            Pixels::Cmyk(c) => {
                assert_eq!(c.len(), 8 * 8 * 4, "four bytes per pixel");
                let sums = || {
                    c.chunks_exact(4)
                        .map(|px| px.iter().map(|&v| v as u16).sum::<u16>())
                };
                for (s, px) in sums().zip(c.chunks_exact(4)) {
                    assert!(s <= 612, "cmyk jpeg pixel exceeds 240% ink: {px:?} = {s}");
                }
                // The rich-black quadrant (encoded 255,255,255,255 = 1020 pre-clamp) proves the
                // ≤240% clamp actually fired: its clamped pixels sit at the 612 ceiling, whereas a
                // naive pass-through would leave the max near 1020 (and trip the loop above).
                assert_eq!(
                    sums().max().unwrap(),
                    612,
                    "heavy-ink pixel should clamp to 612"
                );
            }
            Pixels::Gray(_) => panic!("CMYK JPEG must decode to Cmyk"),
        }
    }

    #[test]
    fn non_transform0_cmyk_jpeg_is_skipped() {
        // Flip the Adobe transform byte 0 → 2 (YCCK). jpeg-decoder still yields CMYK32, but the
        // data is no longer true-ink CMYK, so the transform gate must skip it rather than mis-color.
        let mut bytes = TEST_JPEG_CMYK.to_vec();
        let sig = bytes
            .windows(6)
            .position(|w| w == b"Adobe\0")
            .expect("Adobe marker");
        bytes[sig + 11] = 2;
        assert!(
            decode(&bytes, &naive_converter()).is_none(),
            "YCCK CMYK must be skipped"
        );
    }

    #[test]
    fn cmyk_jpeg_without_adobe_marker_is_skipped() {
        // Corrupt the Adobe signature so no APP14 transform can be read → ambiguous → skip.
        let mut bytes = TEST_JPEG_CMYK.to_vec();
        let sig = bytes
            .windows(6)
            .position(|w| w == b"Adobe\0")
            .expect("Adobe marker");
        bytes[sig] = b'X';
        assert!(
            decode(&bytes, &naive_converter()).is_none(),
            "markerless CMYK must be skipped"
        );
    }

    #[test]
    fn adobe_transform_reads_committed_fixture() {
        assert_eq!(adobe_transform(TEST_JPEG_CMYK), Some(0));
        // A file with no Adobe APP14 marker (the RGB JPEG has none) → None.
        assert_eq!(adobe_transform(TEST_JPEG_RGB), None);
    }
}
