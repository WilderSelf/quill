//! Color handling: ink coverage, limits, and RGB→CMYK conversion.
//!
//! Ink-coverage math is exact and self-contained. RGB→CMYK image conversion ([`RgbToCmyk`])
//! is backed by `lcms2` when the OutputIntent profile is a usable transform destination, and
//! falls back to the naive conversion otherwise (see spec 0005). Soft-proofing remains a later
//! spec. See `specs/0001-pdf-x-export.md` and `specs/0005-color-cmyk-images.md`.

use lcms2::{Intent, PixelFormat, Profile, Transform};
use quill_core_model::Color;

/// DriveThruRPG's maximum total ink coverage, in percent (sum of C+M+Y+K).
pub const MAX_INK_COVERAGE_PCT: f32 = 240.0;

/// Small tolerance so a color intended to sit exactly on the limit isn't rejected by
/// floating-point round-off.
const INK_EPS_PCT: f32 = 0.05;

/// Total ink coverage of a CMYK color, in percent (`0..=400`).
pub fn cmyk_ink_coverage_pct(c: f32, m: f32, y: f32, k: f32) -> f32 {
    (c + m + y + k) * 100.0
}

/// Ink coverage for any color that is valid in press output.
///
/// Returns `None` for `Rgb`, which is not permitted in a PDF/X export and must be converted
/// first — callers should treat `None` as "not press-ready".
pub fn ink_coverage_pct(color: &Color) -> Option<f32> {
    match *color {
        Color::Cmyk { c, m, y, k } => Some(cmyk_ink_coverage_pct(c, m, y, k)),
        // Grayscale prints on the black plate only (0 = white paper, 1 = solid black).
        Color::Gray { v } => Some((1.0 - v) * 100.0),
        Color::Rgb { .. } => None,
    }
}

/// Whether a color is within the ink limit. `Rgb` is never within limit (must convert first).
pub fn within_ink_limit(color: &Color) -> bool {
    matches!(ink_coverage_pct(color), Some(pct) if pct <= MAX_INK_COVERAGE_PCT + INK_EPS_PCT)
}

/// Which conversion path a [`RgbToCmyk`] is using.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ConvMode {
    /// A profile-aware `lcms2` transform (sRGB → the OutputIntent CMYK profile).
    Icc,
    /// The naive fallback, used when the profile can't drive an lcms2 transform.
    Naive,
}

/// Converts 8-bit RGB pixel data to 8-bit CMYK for image embedding (spec 0005).
///
/// Built from the export's OutputIntent ICC bytes: if that profile is a usable transform
/// *destination* (has `BToA` tables — real vendor CMYK profiles do), conversion goes through an
/// `lcms2` transform from sRGB; otherwise it falls back to [`naive_rgb_to_cmyk`]. Either way the
/// output uses the standard ink polarity (0 = no ink, 255 = full ink), matching PDF `/DeviceCMYK`
/// so no `/Decode` array is needed. Construction never fails and conversion never panics on valid
/// (multiple-of-3) input.
pub struct RgbToCmyk {
    // `None` selects the naive fallback. `Transform<u8, u8>` because we feed and receive raw
    // byte slices (lcms2 treats `[u8]` as a per-pixel-format special case).
    transform: Option<Transform<u8, u8>>,
}

impl RgbToCmyk {
    /// Build a converter from the OutputIntent ICC profile bytes. Falls back to the naive path
    /// (never errors) if the profile is invalid or lacks the tables needed as a transform target.
    pub fn from_output_profile(icc_bytes: &[u8]) -> Self {
        let transform = Profile::new_icc(icc_bytes).ok().and_then(|dst| {
            let src = Profile::new_srgb();
            Transform::new(
                &src,
                PixelFormat::RGB_8,
                &dst,
                PixelFormat::CMYK_8,
                Intent::Perceptual,
            )
            .ok()
        });
        Self { transform }
    }

    /// Which path this converter uses.
    pub fn mode(&self) -> ConvMode {
        if self.transform.is_some() {
            ConvMode::Icc
        } else {
            ConvMode::Naive
        }
    }

    /// Convert packed 8-bit RGB (`3·n` bytes) to packed 8-bit CMYK (`4·n` bytes).
    ///
    /// # Panics
    /// If `rgb.len()` is not a multiple of 3.
    pub fn convert(&self, rgb: &[u8]) -> Vec<u8> {
        assert_eq!(rgb.len() % 3, 0, "RGB input length must be a multiple of 3");
        let px = rgb.len() / 3;
        let mut out = vec![0u8; px * 4];
        match &self.transform {
            Some(t) => t.transform_pixels(rgb, &mut out),
            None => {
                for (src, dst) in rgb.chunks_exact(3).zip(out.chunks_exact_mut(4)) {
                    let (r, g, b) = (
                        src[0] as f32 / 255.0,
                        src[1] as f32 / 255.0,
                        src[2] as f32 / 255.0,
                    );
                    if let Color::Cmyk { c, m, y, k } = naive_rgb_to_cmyk(r, g, b) {
                        dst[0] = unit_to_u8(c);
                        dst[1] = unit_to_u8(m);
                        dst[2] = unit_to_u8(y);
                        dst[3] = unit_to_u8(k);
                    }
                }
            }
        }
        out
    }
}

/// Map a `0.0..=1.0` ink fraction to an 8-bit sample (0 = no ink, 255 = full ink).
fn unit_to_u8(v: f32) -> u8 {
    (v * 255.0).round().clamp(0.0, 255.0) as u8
}

/// Naive RGB→CMYK conversion (no ICC/gamut handling). Used as the [`RgbToCmyk`] fallback and for
/// callers that only need an approximate mapping. Do not rely on its color accuracy.
pub fn naive_rgb_to_cmyk(r: f32, g: f32, b: f32) -> Color {
    let k = 1.0 - r.max(g).max(b);
    if (1.0 - k).abs() < f32::EPSILON {
        return Color::Cmyk {
            c: 0.0,
            m: 0.0,
            y: 0.0,
            k: 1.0,
        };
    }
    Color::Cmyk {
        c: (1.0 - r - k) / (1.0 - k),
        m: (1.0 - g - k) / (1.0 - k),
        y: (1.0 - b - k) / (1.0 - k),
        k,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn ink_limit_boundary() {
        // 0.6 * 4 = 2.4 -> ~240%: within limit (modulo float round-off).
        let ok = Color::Cmyk {
            c: 0.6,
            m: 0.6,
            y: 0.6,
            k: 0.6,
        };
        assert!((ink_coverage_pct(&ok).unwrap() - 240.0).abs() < 0.1);
        assert!(within_ink_limit(&ok));

        // 0.6025 * 4 = 2.41 -> 241%: over limit.
        let over = Color::Cmyk {
            c: 0.6025,
            m: 0.6025,
            y: 0.6025,
            k: 0.6025,
        };
        assert!(ink_coverage_pct(&over).unwrap() > MAX_INK_COVERAGE_PCT + INK_EPS_PCT);
        assert!(!within_ink_limit(&over));
    }

    #[test]
    fn rgb_is_not_press_ready() {
        let rgb = Color::Rgb {
            r: 0.2,
            g: 0.4,
            b: 0.6,
        };
        assert_eq!(ink_coverage_pct(&rgb), None);
        assert!(!within_ink_limit(&rgb));
    }

    #[test]
    fn gray_uses_black_plate_only() {
        assert_eq!(ink_coverage_pct(&Color::Gray { v: 1.0 }), Some(0.0)); // white paper
        assert_eq!(ink_coverage_pct(&Color::Gray { v: 0.0 }), Some(100.0)); // solid black
    }

    #[test]
    fn naive_conversion_pure_black() {
        assert_eq!(
            naive_rgb_to_cmyk(0.0, 0.0, 0.0),
            Color::Cmyk {
                c: 0.0,
                m: 0.0,
                y: 0.0,
                k: 1.0
            }
        );
    }

    #[test]
    fn garbage_profile_falls_back_to_naive() {
        let conv = RgbToCmyk::from_output_profile(b"not an icc profile");
        assert_eq!(conv.mode(), ConvMode::Naive);
    }

    #[test]
    fn fallback_conversion_polarity_and_length() {
        let conv = RgbToCmyk::from_output_profile(b"");
        assert_eq!(conv.mode(), ConvMode::Naive);

        // Two pixels: pure white then pure black.
        let out = conv.convert(&[255, 255, 255, 0, 0, 0]);
        assert_eq!(out.len(), 8, "4 bytes per pixel");
        // White paper: no ink on any plate.
        assert_eq!(&out[0..4], &[0, 0, 0, 0]);
        // Black: solid black plate, no CMY.
        assert_eq!(&out[4..8], &[0, 0, 0, 255]);
    }

    #[test]
    #[should_panic(expected = "multiple of 3")]
    fn convert_rejects_non_triple_length() {
        let conv = RgbToCmyk::from_output_profile(b"");
        conv.convert(&[0, 0, 0, 0]);
    }
}
