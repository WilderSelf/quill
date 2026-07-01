//! Font subsetting and composite-font embedding (spec 0002 req 3).
//!
//! One bundled OFL TrueType font (Liberation Serif, SIL OFL-1.1) is subset to only the glyphs a
//! document uses and embedded as a Type0/CIDFontType2 composite font with Identity-H encoding.
//!
//! The `subsetter` crate **remaps** glyph IDs to a compact range (0 = `.notdef`, then contiguous)
//! — it does not preserve original GIDs. So the content stream is encoded with the *remapped*
//! GIDs and `CIDToGIDMap` is `/Identity` (CID == new GID == subset glyph index). This is the
//! single sharpest correctness risk in export (a mismatch renders wrong glyphs while still
//! passing veraPDF's "font embedded" check); [`tests::gids_are_consistent`] pins it down.

use std::collections::{BTreeSet, HashMap};

use subsetter::GlyphRemapper;
use ttf_parser::{Face, GlyphId};

use crate::ExportError;

/// The bundled font program (full file; subset at export time). SIL OFL-1.1 — see
/// `assets/LiberationSerif-LICENSE.txt`.
const FONT_TTF: &[u8] = include_bytes!("../assets/LiberationSerif-Regular.ttf");

/// A subset font ready to embed, plus everything needed to encode text against it.
pub struct EmbeddedFont {
    /// PostScript name with a subset tag, e.g. `ABCDEF+LiberationSerif`.
    pub base_font: String,
    /// The subset TrueType program (uncompressed; the writer flate-compresses it).
    pub subset: Vec<u8>,
    /// Glyph advance widths in new-GID order, scaled to the PDF 1000-unit em.
    pub widths: Vec<f32>,
    /// Font bounding box `[x_min, y_min, x_max, y_max]` in 1000-unit em.
    pub bbox: [f32; 4],
    pub ascent: f32,
    pub descent: f32,
    pub cap_height: f32,
    pub stem_v: f32,
    /// Character → remapped (subset) glyph id, for content-stream encoding.
    char_to_gid: HashMap<char, u16>,
}

impl EmbeddedFont {
    /// Encode a line as Identity-H text: two big-endian bytes per glyph (subset GIDs).
    pub fn encode_line(&self, text: &str) -> Vec<u8> {
        let mut out = Vec::with_capacity(text.chars().count() * 2);
        for ch in text.chars() {
            let gid = self.char_to_gid.get(&ch).copied().unwrap_or(0);
            out.extend_from_slice(&gid.to_be_bytes());
        }
        out
    }

    /// Ascent in points at a given font size (baseline sits this far below the frame top).
    pub fn ascent_pt(&self, size_pt: f32) -> f32 {
        self.ascent * size_pt / 1000.0
    }
}

/// Subset and measure the bundled font for exactly the `chars` a document uses.
pub fn build(chars: &BTreeSet<char>) -> Result<EmbeddedFont, ExportError> {
    let face = Face::parse(FONT_TTF, 0).map_err(|e| ExportError::Font(format!("parse: {e}")))?;
    let scale = 1000.0 / face.units_per_em() as f32;

    // Original GIDs used, and the char→original-GID pairs for later remapping.
    let mut used_orig: Vec<u16> = Vec::with_capacity(chars.len() + 1);
    let mut char_orig: Vec<(char, u16)> = Vec::with_capacity(chars.len());
    for &ch in chars {
        let gid = face.glyph_index(ch).map(|g| g.0).unwrap_or(0);
        used_orig.push(gid);
        char_orig.push((ch, gid));
    }

    // Remap to a compact GID range (always includes .notdef = 0), then subset.
    let remapper = GlyphRemapper::new_from_glyphs_sorted(&used_orig);
    let subset = subsetter::subset(FONT_TTF, 0, &remapper)
        .map_err(|e| ExportError::Font(format!("subset: {e}")))?;

    // Widths in new-GID order: remapped_gids() yields old GIDs ordered by their new GID.
    let widths: Vec<f32> = remapper
        .remapped_gids()
        .map(|old| face.glyph_hor_advance(GlyphId(old)).unwrap_or(0) as f32 * scale)
        .collect();

    // Char → new (subset) GID.
    let mut char_to_gid = HashMap::with_capacity(char_orig.len());
    for (ch, old) in char_orig {
        char_to_gid.insert(ch, remapper.get(old).unwrap_or(0));
    }

    let bb = face.global_bounding_box();
    let bbox = [
        bb.x_min as f32 * scale,
        bb.y_min as f32 * scale,
        bb.x_max as f32 * scale,
        bb.y_max as f32 * scale,
    ];
    let ascent = face.ascender() as f32 * scale;
    let descent = face.descender() as f32 * scale;
    let cap_height = face
        .capital_height()
        .map(|c| c as f32 * scale)
        .unwrap_or(ascent * 0.7);

    Ok(EmbeddedFont {
        base_font: format!("{}+LiberationSerif", subset_tag(&used_orig)),
        subset,
        widths,
        bbox,
        ascent,
        descent,
        cap_height,
        stem_v: 80.0,
        char_to_gid,
    })
}

/// Deterministic six-uppercase-letter subset tag (PDF requires `AAAAAA+` form).
fn subset_tag(gids: &[u16]) -> String {
    // FNV-1a over the gid set → six letters A–Z. Deterministic ⇒ reproducible golden output.
    let mut h: u32 = 0x811c_9dc5;
    for g in gids {
        h ^= *g as u32;
        h = h.wrapping_mul(0x0100_0193);
    }
    let mut tag = String::with_capacity(6);
    for i in 0..6 {
        let v = (h >> (i * 5)) & 0x1f; // 0..31
        tag.push((b'A' + (v % 26) as u8) as char);
    }
    tag
}

#[cfg(test)]
mod tests {
    use super::*;

    fn charset(s: &str) -> BTreeSet<char> {
        s.chars().collect()
    }

    #[test]
    fn subsets_and_measures() {
        let font = build(&charset("The Dungeon")).unwrap();
        assert!(!font.subset.is_empty());
        assert!(
            font.subset.len() < FONT_TTF.len(),
            "subset should be smaller"
        );
        assert!(font.widths.len() >= 2); // notdef + at least one real glyph
        assert!(font.widths.iter().all(|w| w.is_finite() && *w >= 0.0));
        assert!(font.ascent > 0.0 && font.descent < 0.0);
        assert!(font.base_font.ends_with("+LiberationSerif"));
        assert_eq!(font.base_font.len(), 6 + 1 + "LiberationSerif".len());
    }

    #[test]
    fn encodes_two_bytes_per_glyph_nonzero() {
        let font = build(&charset("A")).unwrap();
        let bytes = font.encode_line("A");
        assert_eq!(bytes.len(), 2);
        let gid = u16::from_be_bytes([bytes[0], bytes[1]]);
        assert_ne!(gid, 0, "'A' must map to a real subset glyph, not .notdef");
        assert!(gid < font.widths.len() as u16);
    }

    #[test]
    fn unknown_char_maps_to_notdef() {
        let font = build(&charset("A")).unwrap();
        // A char not in the subset encodes to GID 0.
        let bytes = font.encode_line("\u{2603}"); // snowman, not built
        assert_eq!(bytes, vec![0, 0]);
    }

    /// The correctness invariant: the width recorded for a char's subset GID equals the original
    /// face's advance for that char. If subsetting silently changed the GID↔glyph relationship,
    /// this catches it — the failure veraPDF cannot see.
    #[test]
    fn gids_are_consistent() {
        let face = Face::parse(FONT_TTF, 0).unwrap();
        let scale = 1000.0 / face.units_per_em() as f32;
        let font = build(&charset("The Dungeon")).unwrap();
        for ch in "The Dungeon".chars() {
            let new_gid = font.char_to_gid[&ch] as usize;
            let orig_gid = face.glyph_index(ch).unwrap().0;
            let orig_w = face.glyph_hor_advance(GlyphId(orig_gid)).unwrap() as f32 * scale;
            assert!(
                (font.widths[new_gid] - orig_w).abs() < 0.01,
                "width mismatch for '{ch}': subset gid {new_gid}"
            );
        }
    }
}
