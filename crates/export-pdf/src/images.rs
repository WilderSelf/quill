//! Linked-image resolution and decoding for export (spec 0002 reqs 2, 7).
//!
//! PDF/X-1a forbids RGB image data, so every placed image is decoded to 8-bit **grayscale** and
//! emitted as a `/DeviceGray` XObject (unambiguously legal, no ICC transform needed). A missing
//! or undecodable asset returns `None` and is skipped by the writer rather than failing the whole
//! export — so the default sample (whose `assets/map1.png` does not exist) still produces a valid
//! PDF. Alpha is dropped (no `/SMask`), preserving the "no transparency" invariant.

use std::path::Path;

use quill_core_model::Asset;

/// A decoded image reduced to 8-bit grayscale, ready to embed as `/DeviceGray`.
pub struct GrayImage {
    pub width: u32,
    pub height: u32,
    /// One byte per pixel, row-major, `width * height` bytes.
    pub gray: Vec<u8>,
}

/// Resolve `asset.path` against `base_dir` and decode it to grayscale.
///
/// Returns `None` (skip, don't fail) if the file is missing, unreadable, or in a format we don't
/// handle for M0 (anything but 8-bit Gray/GrayAlpha/RGB/RGBA).
pub fn resolve(asset: &Asset, base_dir: &Path) -> Option<GrayImage> {
    let path = base_dir.join(&asset.path);
    let bytes = std::fs::read(&path).ok()?;
    decode_gray(&bytes)
}

/// Decode PNG bytes to 8-bit grayscale.
pub fn decode_gray(bytes: &[u8]) -> Option<GrayImage> {
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

    let gray: Vec<u8> = match info.color_type {
        ColorType::Grayscale => data[..px].to_vec(),
        ColorType::GrayscaleAlpha => data.chunks_exact(2).map(|p| p[0]).collect(),
        ColorType::Rgb => data
            .chunks_exact(3)
            .map(|p| luma(p[0], p[1], p[2]))
            .collect(),
        ColorType::Rgba => data
            .chunks_exact(4)
            .map(|p| luma(p[0], p[1], p[2]))
            .collect(),
        ColorType::Indexed => return None, // not expanded; skip for M0
    };
    debug_assert_eq!(gray.len(), px);
    Some(GrayImage {
        width: w,
        height: h,
        gray,
    })
}

/// Rec. 601 luma, integer-approximated.
fn luma(r: u8, g: u8, b: u8) -> u8 {
    ((77 * r as u32 + 150 * g as u32 + 29 * b as u32) >> 8) as u8
}

#[cfg(test)]
mod tests {
    use super::*;

    const TEST_PNG: &[u8] = include_bytes!("../assets/test_gray.png");

    #[test]
    fn decodes_bundled_grayscale() {
        let img = decode_gray(TEST_PNG).expect("decode test_gray.png");
        assert_eq!(img.width, 8);
        assert_eq!(img.height, 8);
        assert_eq!(img.gray.len(), 64);
    }

    #[test]
    fn missing_file_is_skipped_not_fatal() {
        let asset = Asset {
            id: "x".into(),
            path: "does-not-exist.png".into(),
            dpi: 300.0,
            line_art: false,
        };
        assert!(resolve(&asset, Path::new("/nonexistent")).is_none());
    }

    #[test]
    fn garbage_bytes_decode_to_none() {
        assert!(decode_gray(b"not a png").is_none());
    }
}
