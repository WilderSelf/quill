//! Core document model and the open `.tpub` file format for Quill.
//!
//! Holds the serializable document tree shared across the layout, render, and export crates.
//! See `docs/format-spec.md` and `specs/0001-pdf-x-export.md`.

use serde::{Deserialize, Serialize};

/// Typographic points (1/72 inch) — the internal unit throughout Quill.
pub type Pt = f32;

/// The current `.tpub` manifest format version.
pub const FORMAT_VERSION: u32 = 1;

/// 0.125 inch expressed in points — the DriveThruRPG-required bleed on outside edges.
pub const DEFAULT_BLEED_PT: Pt = 9.0;

/// A width/height in points.
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
pub struct Size {
    pub w_pt: Pt,
    pub h_pt: Pt,
}

/// An axis-aligned rectangle in points, origin top-left.
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
pub struct Rect {
    pub x_pt: Pt,
    pub y_pt: Pt,
    pub w_pt: Pt,
    pub h_pt: Pt,
}

/// A color value.
///
/// Press output must be `Cmyk` or `Gray`; `Rgb` is authoring-only and must be converted (see
/// `quill-color`) before it can appear in a PDF/X export.
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
#[serde(tag = "space", rename_all = "lowercase")]
pub enum Color {
    /// Each channel in `0.0..=1.0`.
    Cmyk { c: f32, m: f32, y: f32, k: f32 },
    /// Single channel in `0.0..=1.0` (0 = black, 1 = white).
    Gray { v: f32 },
    /// Authoring-only; not permitted in press output.
    Rgb { r: f32, g: f32, b: f32 },
}

/// Document-level metadata, written into both the manifest and the exported PDF.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct Metadata {
    pub title: String,
    #[serde(default)]
    pub authors: Vec<String>,
}

/// Trim size, bleed, and facing-page setup for the whole document.
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
pub struct PageSetup {
    pub trim: Size,
    pub bleed_pt: Pt,
    pub facing_pages: bool,
}

impl Default for PageSetup {
    fn default() -> Self {
        // A common 6x9in "digest" trim.
        Self {
            trim: Size {
                w_pt: 432.0,
                h_pt: 648.0,
            },
            bleed_pt: DEFAULT_BLEED_PT,
            facing_pages: true,
        }
    }
}

/// A linked asset (image, etc.). Assets are referenced, not inlined — see the format spec.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Asset {
    pub id: String,
    pub path: String,
    /// Pixel dimensions of the source image. Used by the layout engine to place the image at its
    /// true aspect ratio and physical size (`pt = px / dpi * 72`). See `specs/0009-image-sizing.md`.
    /// `0` means "unknown" — the layout engine falls back to a square, full-width placeholder.
    #[serde(default)]
    pub px_w: u32,
    /// See [`Asset::px_w`].
    #[serde(default)]
    pub px_h: u32,
    /// Native (source) resolution of the image, in dots per inch. Combined with `px_w`/`px_h`
    /// it determines the placed size (`pt = px / dpi * 72`; see spec 0009). Preflight's
    /// `ImageResolution` check gates on this value.
    pub dpi: f32,
    /// True for bilevel line art (600 dpi threshold instead of 300).
    #[serde(default)]
    pub line_art: bool,
    /// True if the linked image carries an alpha channel. PDF/X forbids live transparency, so
    /// export flattens it (alpha is dropped); preflight warns when this will happen.
    #[serde(default)]
    pub has_alpha: bool,
}

/// A semantic content block — the "easy" authoring layer.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum Block {
    Heading {
        level: u8,
        text: String,
        color: Color,
    },
    Body {
        text: String,
        color: Color,
    },
    Image {
        asset: String,
    },
}

/// The whole document.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Document {
    pub format_version: u32,
    #[serde(default)]
    pub metadata: Metadata,
    pub page_setup: PageSetup,
    #[serde(default)]
    pub content: Vec<Block>,
    #[serde(default)]
    pub assets: Vec<Asset>,
    /// Whether all fonts referenced by the document can be embedded/subset for export.
    #[serde(default)]
    pub fonts_embeddable: bool,
}

impl Document {
    /// A minimal, valid document used by tests, the CLI sample, and the M0 export spike.
    pub fn sample() -> Self {
        Self {
            format_version: FORMAT_VERSION,
            metadata: Metadata {
                title: "Sample Adventure".into(),
                authors: vec!["Anon".into()],
            },
            page_setup: PageSetup::default(),
            content: vec![
                Block::Heading {
                    level: 1,
                    text: "The Dungeon".into(),
                    color: Color::Gray { v: 0.0 },
                },
                Block::Body {
                    text: "A dank corridor stretches into darkness.".into(),
                    color: Color::Cmyk {
                        c: 0.0,
                        m: 0.0,
                        y: 0.0,
                        k: 1.0,
                    },
                },
            ],
            assets: vec![Asset {
                id: "map1".into(),
                path: "assets/map1.png".into(),
                px_w: 1500,
                px_h: 1200,
                dpi: 300.0,
                line_art: false,
                has_alpha: false,
            }],
            fonts_embeddable: true,
        }
    }

    /// Serialize the manifest to pretty JSON (the `document.json` inside a `.tpub`).
    pub fn to_json(&self) -> Result<String, serde_json::Error> {
        serde_json::to_string_pretty(self)
    }

    /// Parse a manifest from JSON.
    pub fn from_json(s: &str) -> Result<Self, serde_json::Error> {
        serde_json::from_str(s)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn sample_round_trips_through_json() {
        let doc = Document::sample();
        let json = doc.to_json().expect("serialize");
        let back = Document::from_json(&json).expect("deserialize");
        assert_eq!(doc, back);
    }

    #[test]
    fn default_bleed_is_one_eighth_inch() {
        assert_eq!(DEFAULT_BLEED_PT, 9.0);
        assert_eq!(PageSetup::default().bleed_pt, DEFAULT_BLEED_PT);
    }
}
