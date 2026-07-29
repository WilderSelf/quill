//! Linked-image resolution and decoding for export (spec 0002 reqs 2, 7; spec 0005; spec 0008).
//!
//! Grayscale inputs decode to 8-bit `/DeviceGray` (unambiguously legal, no ICC transform). Color
//! inputs are converted to 8-bit CMYK via [`RgbToCmyk`] and emitted as `/DeviceCMYK`,
//! the only image color space PDF/X-1a permits — so an author's color art survives export instead
//! of being desaturated. A missing or undecodable asset returns `None` and is skipped by the
//! writer rather than failing the whole export. Alpha is **flattened onto paper white** before
//! conversion (no `/SMask`), preserving the "no transparency" invariant — see spec 0082, and note
//! that until it shipped the alpha channel was *discarded* and the colour stored underneath
//! converted, which turned an ordinary transparent surround into a solid black rectangle.
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

use quill_color::{clamp_cmyk_u8, flatten_over_paper, RgbToCmyk};
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
///
/// The two alpha-bearing arms flatten onto paper white first (spec 0082). `tRNS` needs no arm of
/// its own precisely because `EXPAND` has already turned it into one of those two.
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
        // Composited onto paper and emitted as `/DeviceGray`: there is no conversion on this path,
        // so the flatten *is* the whole of it.
        ColorType::GrayscaleAlpha => Pixels::Gray(flatten_over_paper(data, 1)),
        ColorType::Rgb => {
            debug_assert_eq!(data.len(), px * 3);
            Pixels::Cmyk(cmyk.convert(data))
        }
        // Composite, then convert, then clamp — in that order, and the order is argued in
        // `flatten_over_paper`'s doc comment. It replaces the channel-dropping widen this arm used
        // to do, so it costs no extra pass over the pixels: the same traversal, doing arithmetic
        // instead of discarding a byte.
        ColorType::Rgba => Pixels::Cmyk(cmyk.convert(&flatten_over_paper(data, 3))),
        ColorType::Indexed => return None, // defensive: EXPAND already turns palette into RGB(A).
    };
    Some(DecodedImage {
        width: w,
        height: h,
        pixels,
    })
}

/// Resolve `asset.path` against `base_dir` and report whether the linked file **carries** an alpha
/// channel — reading only its header (spec 0082).
///
/// Returns `None` for a link that does not resolve, is unreadable, or is in a format this module
/// does not decode: "don't know", which is a different answer from "no" and is treated as one by
/// the caller.
pub fn probe_alpha_at(asset: &Asset, base_dir: &Path) -> Option<bool> {
    let mut file = std::fs::File::open(base_dir.join(&asset.path)).ok()?;
    // Enough to sniff the magic bytes and, for a PNG, to reach `IHDR`/`PLTE`/`tRNS` — every chunk
    // that decides the answer must precede `IDAT`, so no image data is ever read.
    let mut head = vec![0u8; 8];
    {
        use std::io::Read;
        file.read_exact(&mut head).ok()?;
    }
    if head.starts_with(b"\x89PNG\r\n\x1a\n") {
        use std::io::Seek;
        file.rewind().ok()?;
        return probe_png_alpha(std::io::BufReader::new(file));
    }
    if head.starts_with(&[0xFF, 0xD8, 0xFF]) {
        return Some(false); // JPEG has no alpha channel in any pixel format this module decodes
    }
    None
}

/// [`probe_alpha_at`] over bytes already in hand.
///
/// `#[cfg(test)]` because every production caller has a path rather than a buffer, and reading a
/// whole file to answer a header question would undo the point of the streaming version. It exists
/// so the dispatch can be asserted over in-memory fixtures, which is how every other decode in this
/// module is tested.
#[cfg(test)]
pub fn probe_alpha(bytes: &[u8]) -> Option<bool> {
    if bytes.starts_with(b"\x89PNG\r\n\x1a\n") {
        probe_png_alpha(std::io::Cursor::new(bytes))
    } else if bytes.starts_with(&[0xFF, 0xD8, 0xFF]) {
        Some(false)
    } else {
        None
    }
}

/// Whether a PNG reaches [`decode_png`]'s alpha-bearing arms, from its header alone.
///
/// Asked of the decoder under **the same transformations the decode uses**, so the answer is
/// "does this file take a path that composites?" rather than "does the `IHDR` colour type happen to
/// end in Alpha?". That is what makes a `tRNS`-keyed palette answer `true`: `EXPAND` turns it into
/// `Rgba`, and it is `EXPAND`'s output the flattening arms match on.
fn probe_png_alpha(read: impl std::io::BufRead + std::io::Seek) -> Option<bool> {
    use png::ColorType;
    let mut decoder = png::Decoder::new(read);
    decoder.set_transformations(png::Transformations::normalize_to_color8());
    let reader = decoder.read_info().ok()?;
    Some(matches!(
        reader.output_color_type().0,
        ColorType::GrayscaleAlpha | ColorType::Rgba
    ))
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

    /// Encode a tiny RGBA PNG in-memory.
    fn rgba_png(width: u32, height: u32, rgba: &[u8]) -> Vec<u8> {
        let mut out = Vec::new();
        {
            let mut enc = png::Encoder::new(&mut out, width, height);
            enc.set_color(png::ColorType::Rgba);
            enc.set_depth(png::BitDepth::Eight);
            let mut writer = enc.write_header().expect("png header");
            writer.write_image_data(rgba).expect("png data");
        }
        out
    }

    /// Encode a tiny grayscale+alpha PNG in-memory (`[gray, alpha]` per pixel).
    fn gray_alpha_png(width: u32, height: u32, ga: &[u8]) -> Vec<u8> {
        let mut out = Vec::new();
        {
            let mut enc = png::Encoder::new(&mut out, width, height);
            enc.set_color(png::ColorType::GrayscaleAlpha);
            enc.set_depth(png::BitDepth::Eight);
            let mut writer = enc.write_header().expect("png header");
            writer.write_image_data(ga).expect("png data");
        }
        out
    }

    /// Encode a tiny indexed PNG carrying a `tRNS` chunk, so `EXPAND` has a colour key to turn into
    /// an alpha channel (spec 0010's third normalization).
    fn indexed_trns_png(
        width: u32,
        height: u32,
        palette_rgb: &[u8],
        trns: &[u8],
        indices: &[u8],
    ) -> Vec<u8> {
        let mut out = Vec::new();
        {
            let mut enc = png::Encoder::new(&mut out, width, height);
            enc.set_color(png::ColorType::Indexed);
            enc.set_depth(png::BitDepth::Eight);
            enc.set_palette(palette_rgb.to_vec());
            enc.set_trns(trns.to_vec());
            let mut writer = enc.write_header().expect("png header");
            writer.write_image_data(indices).expect("png data");
        }
        out
    }

    /// Encode a tiny 8-bit grayscale PNG whose `tRNS` chunk keys one grey level transparent.
    /// `EXPAND` turns that into a `GrayscaleAlpha` buffer.
    fn gray_trns_png(width: u32, height: u32, transparent_level: u16, samples: &[u8]) -> Vec<u8> {
        let mut out = Vec::new();
        {
            let mut enc = png::Encoder::new(&mut out, width, height);
            enc.set_color(png::ColorType::Grayscale);
            enc.set_depth(png::BitDepth::Eight);
            enc.set_trns(transparent_level.to_be_bytes().to_vec());
            let mut writer = enc.write_header().expect("png header");
            writer.write_image_data(samples).expect("png data");
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

    // --- Alpha is flattened onto paper, not discarded (spec 0082) --------------------------------
    //
    // Every test here asserts the *emitted* samples. The defect these were written against dropped
    // the alpha channel and converted the RGB stored underneath it, and PNG places no constraint on
    // that colour: `(0,0,0,0)` is what most encoders write, and it converted to solid K.

    fn cmyk_of(img: &DecodedImage) -> &[u8] {
        match &img.pixels {
            Pixels::Cmyk(c) => c,
            Pixels::Gray(_) => panic!("expected a CMYK image"),
        }
    }

    fn gray_of(img: &DecodedImage) -> &[u8] {
        match &img.pixels {
            Pixels::Gray(g) => g,
            Pixels::Cmyk(_) => panic!("expected a grayscale image"),
        }
    }

    #[test]
    fn a_fully_transparent_rgba_pixel_is_paper_not_black() {
        // The logo case: a transparent surround stored as (0,0,0,0), beside an opaque red.
        let png = rgba_png(2, 1, &[0, 0, 0, 0, 255, 0, 0, 255]);
        let img = decode(&png, &naive_converter()).expect("decode rgba png");
        let c = cmyk_of(&img);
        assert_eq!(
            &c[0..4],
            &[0, 0, 0, 0],
            "a transparent pixel must composite onto paper (no ink), not print as solid K"
        );
        assert_eq!(&c[4..8], &[0, 255, 255, 0], "the opaque pixel is unchanged");
    }

    #[test]
    fn a_fully_transparent_grayscale_alpha_pixel_is_paper_white() {
        // Same defect on the /DeviceGray path: alpha was dropped and the black underneath kept.
        let png = gray_alpha_png(2, 1, &[0, 0, 0, 255]);
        let img = decode(&png, &naive_converter()).expect("decode gray+alpha png");
        assert_eq!(
            gray_of(&img),
            &[255, 0],
            "transparent → paper white; opaque black is unchanged"
        );
    }

    #[test]
    fn a_trns_keyed_indexed_png_flattens_onto_paper() {
        // `EXPAND` turns a palette + tRNS into RGBA before either arm sees it (spec 0010), so the
        // keyed index arrives as alpha = 0 over palette entry 0 — black, here, deliberately.
        let png = indexed_trns_png(2, 1, &[0, 0, 0, 255, 0, 0], &[0], &[0, 1]);
        let img = decode(&png, &naive_converter()).expect("decode indexed+tRNS png");
        let c = cmyk_of(&img);
        assert_eq!(&c[0..4], &[0, 0, 0, 0], "the keyed index is paper, not ink");
        assert_eq!(&c[4..8], &[0, 255, 255, 0], "the opaque index is unchanged");
    }

    #[test]
    fn a_trns_keyed_grayscale_png_flattens_onto_paper() {
        // The other `EXPAND` route: grayscale + tRNS becomes GrayscaleAlpha.
        let png = gray_trns_png(2, 1, 0, &[0, 128]);
        let img = decode(&png, &naive_converter()).expect("decode gray+tRNS png");
        assert_eq!(
            gray_of(&img),
            &[255, 128],
            "the keyed level is paper; every other level is unchanged"
        );
    }

    #[test]
    fn partial_alpha_composites_toward_paper() {
        // Not just the alpha = 0 case: a half-covered pixel lands halfway to the paper.
        let png = rgba_png(1, 1, &[0, 0, 0, 128]);
        let img = decode(&png, &naive_converter()).expect("decode rgba png");
        let c = cmyk_of(&img);
        // 0·(128/255) + 255·(127/255) = 127 grey → naive CMYK (0,0,0,128).
        assert_eq!(&c[0..4], &[0, 0, 0, 128]);
    }

    #[test]
    fn flattened_pixels_are_still_ink_clamped() {
        // Composite first, clamp last: the clamp is a guarantee about the bytes that get embedded,
        // so it must run *after* the composite has decided what those bytes are (spec 0006).
        let png = rgba_png(1, 1, &[26, 0, 0, 255]);
        let img = decode(&png, &naive_converter()).expect("decode rgba png");
        let sum: u16 = cmyk_of(&img).iter().map(|&v| v as u16).sum();
        assert!(sum <= 612, "flattened pixel exceeds 240% ink: {sum}");
    }

    #[test]
    fn an_image_without_alpha_is_untouched() {
        // The claim that the whole increment rests on: nothing that has no alpha channel moves.
        let rgb = decode(
            &rgb_png(2, 1, &[255, 255, 255, 26, 0, 0]),
            &naive_converter(),
        )
        .expect("decode rgb png");
        assert_eq!(cmyk_of(&rgb), &[0, 0, 0, 0, 0, 191, 191, 229]);
        let gray = decode(TEST_PNG, &naive_converter()).expect("decode gray png");
        assert_eq!(gray_of(&gray).len(), 64);
    }

    #[test]
    fn probe_alpha_reports_what_the_file_carries() {
        // The decoder is what knows, so it is what is asked (spec 0082). Every shape that reaches
        // an alpha-bearing arm answers `true`, and every shape that cannot answers `false`.
        assert_eq!(probe_alpha(&rgba_png(1, 1, &[0, 0, 0, 0])), Some(true));
        assert_eq!(probe_alpha(&gray_alpha_png(1, 1, &[0, 0])), Some(true));
        assert_eq!(
            probe_alpha(&indexed_trns_png(1, 1, &[0, 0, 0], &[0], &[0])),
            Some(true)
        );
        assert_eq!(probe_alpha(&gray_trns_png(1, 1, 0, &[0])), Some(true));
        assert_eq!(probe_alpha(&rgb_png(1, 1, &[1, 2, 3])), Some(false));
        assert_eq!(
            probe_alpha(&indexed_png(1, 1, &[0, 0, 0], &[0])),
            Some(false)
        );
        assert_eq!(probe_alpha(TEST_PNG), Some(false));
        // JPEG cannot carry alpha at all, and an unreadable file answers "don't know".
        assert_eq!(probe_alpha(TEST_JPEG_RGB), Some(false));
        assert_eq!(probe_alpha(b"not an image"), None);
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
