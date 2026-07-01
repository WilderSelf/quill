//! Color handling: ink coverage, limits, and (placeholder) conversions.
//!
//! ICC-based conversion and soft-proofing will be backed by `lcms2` in a later spec-driven
//! commit; the naive conversion here is clearly marked and exists so ink-coverage and limit
//! logic can be built and tested now. See `specs/0001-pdf-x-export.md`.

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

/// Naive, **placeholder** RGB→CMYK conversion (no ICC/gamut handling). Will be replaced by a
/// profile-aware `lcms2` conversion. Do not rely on its color accuracy.
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
}
