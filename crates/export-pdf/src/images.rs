//! Linked-image resolution and decoding for export (spec 0002 reqs 2, 7; spec 0005).
//!
//! Grayscale inputs decode to 8-bit `/DeviceGray` (unambiguously legal, no ICC transform). Color
//! (RGB/RGBA) inputs are converted to 8-bit CMYK via [`RgbToCmyk`] and emitted as `/DeviceCMYK`,
//! the only image color space PDF/X-1a permits — so an author's color art survives export instead
//! of being desaturated. A missing or undecodable asset returns `None` and is skipped by the
//! writer rather than failing the whole export. Alpha is dropped (no `/SMask`), preserving the
//! "no transparency" invariant.

use std::path::Path;

use quill_color::RgbToCmyk;
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
/// handle for M0 (anything but 8-bit Gray/GrayAlpha/RGB/RGBA).
pub fn resolve(asset: &Asset, base_dir: &Path, cmyk: &RgbToCmyk) -> Option<DecodedImage> {
    let path = base_dir.join(&asset.path);
    let bytes = std::fs::read(&path).ok()?;
    decode(&bytes, cmyk)
}

/// Decode PNG bytes: grayscale stays gray, color is converted to CMYK via `cmyk`.
pub fn decode(bytes: &[u8], cmyk: &RgbToCmyk) -> Option<DecodedImage> {
    use png::{BitDepth, ColorType};

    let decoder = png::Decoder::new(std::io::Cursor::new(bytes));
    let mut reader = decoder.read_info().ok()?;
    let mut buf = vec![0u8; reader.output_buffer_size()?];
    let info = reader.next_frame(&mut buf).ok()?;

    if info.bit_depth != BitDepth::Eight {
        return None; // M0 handles 8-bit inputs only.
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
        ColorType::Indexed => return None, // not expanded; skip for M0
    };
    Some(DecodedImage {
        width: w,
        height: h,
        pixels,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    const TEST_PNG: &[u8] = include_bytes!("../assets/test_gray.png");

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
}
