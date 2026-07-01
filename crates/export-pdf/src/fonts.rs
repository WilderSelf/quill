//! Font subsetting and composite-font embedding (spec 0002 req 3, spec 0004 user fonts).
//!
//! A font program — either the bundled OFL font (Source Serif 4, SIL OFL-1.1, `glyf` outlines) or
//! a user-supplied TrueType file — is subset to only the glyphs a document uses and embedded as a
//! Type0/CIDFontType2 composite font with Identity-H encoding. See `specs/0004-user-font-embedding.md`.
//!
//! The `subsetter` crate **remaps** glyph IDs to a compact range (0 = `.notdef`, then contiguous)
//! — it does not preserve original GIDs. So the content stream is encoded with the *remapped*
//! GIDs and `CIDToGIDMap` is `/Identity` (CID == new GID == subset glyph index). This is the
//! single sharpest correctness risk in export (a mismatch renders wrong glyphs while still
//! passing veraPDF's "font embedded" check); [`tests::gids_are_consistent`] pins it down.

use std::collections::{BTreeSet, HashMap};

use pdf_writer::types::FontFlags;
use subsetter::GlyphRemapper;
use ttf_parser::{Face, GlyphId, Permissions};

use crate::ExportError;

/// The bundled font program (full file; subset at export time). SIL OFL-1.1 — see
/// `assets/SourceSerif4-LICENSE.txt`.
pub(crate) const FONT_TTF: &[u8] = include_bytes!("../assets/SourceSerif4-Regular.ttf");

/// PostScript-style family name for the bundled font, embedded after the subset tag
/// (`ABCDEF+<NAME>`). User fonts derive their own name via [`derive_font_name`].
const FONT_NAME: &str = "SourceSerif4";

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
    /// Italic angle in degrees (0.0 for upright faces), for the FontDescriptor.
    pub italic_angle: f32,
    /// FontDescriptor flags derived from the face (serif/italic/fixed-pitch/non-symbolic).
    pub flags: FontFlags,
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

/// Subset and measure the **bundled** font for exactly the `chars` a document uses.
///
/// A thin wrapper over [`build_from_bytes`] that pins the name (`SourceSerif4`) and adds the
/// `SERIF` descriptor flag, preserving byte-for-byte the output every prior spec relied on.
pub fn build(chars: &BTreeSet<char>) -> Result<EmbeddedFont, ExportError> {
    let mut font = build_from_bytes(FONT_TTF, Some(FONT_NAME), chars)?;
    font.flags |= FontFlags::SERIF;
    Ok(font)
}

/// Subset and measure an arbitrary TrueType (`glyf`) font `program` for the given `chars`.
///
/// `name_override` pins the embedded family name (used for the bundled font); when `None`, the
/// name is derived from the font's own `name` table and sanitised to a valid PDF name. Rejects
/// CFF/OpenType-CFF programs and fonts whose `OS/2` `fsType` forbids embedding.
pub fn build_from_bytes(
    program: &[u8],
    name_override: Option<&str>,
    chars: &BTreeSet<char>,
) -> Result<EmbeddedFont, ExportError> {
    let face = Face::parse(program, 0).map_err(|e| ExportError::Font(format!("parse: {e}")))?;
    let tables = face.tables();
    check_outline_format(tables.glyf.is_some(), tables.cff.is_some())?;
    check_embeddable(face.permissions())?;

    let name = match name_override {
        Some(n) => n.to_string(),
        None => derive_font_name(&face),
    };
    let flags = descriptor_flags(&face);
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
    let subset = subsetter::subset(program, 0, &remapper)
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
        base_font: format!("{}+{name}", subset_tag(&used_orig)),
        subset,
        widths,
        bbox,
        ascent,
        descent,
        cap_height,
        stem_v: 80.0,
        italic_angle: face.italic_angle(),
        flags,
        char_to_gid,
    })
}

/// Require TrueType (`glyf`) outlines. CFF/OpenType-CFF is a named fast-follow (needs a different
/// `FontFile3`/`CIDFontType0` writer path), so it is rejected with a clear message.
fn check_outline_format(has_glyf: bool, has_cff: bool) -> Result<(), ExportError> {
    if has_glyf {
        return Ok(());
    }
    let msg = if has_cff {
        "OpenType/CFF fonts (.otf) are not yet supported; supply a TrueType (.ttf) font"
    } else {
        "font has no TrueType (glyf) outlines"
    };
    Err(ExportError::Font(msg.into()))
}

/// Reject fonts whose `OS/2` `fsType` marks them *Restricted License* embedding. A missing/
/// malformed `OS/2` table (`None`) or any other class (installable, preview-and-print, editable)
/// is treated as embeddable — only the explicit no-embedding bit is a hard stop.
fn check_embeddable(permissions: Option<Permissions>) -> Result<(), ExportError> {
    if permissions == Some(Permissions::Restricted) {
        return Err(ExportError::Font(
            "font license forbids embedding (OS/2 fsType: Restricted License)".into(),
        ));
    }
    Ok(())
}

/// FontDescriptor flags derived from the face: always non-symbolic; italic and fixed-pitch from
/// the face's own metadata. `SERIF` is *not* derived here (unreliable across arbitrary fonts) —
/// the bundled font adds it explicitly in [`build`]; omitting it for user fonts is PDF/X-legal.
fn descriptor_flags(face: &Face) -> FontFlags {
    let mut flags = FontFlags::NON_SYMBOLIC;
    if face.is_italic() {
        flags |= FontFlags::ITALIC;
    }
    if face.is_monospaced() {
        flags |= FontFlags::FIXED_PITCH;
    }
    flags
}

/// Derive an embedded family name from the face's `name` table: prefer the PostScript name
/// (ID 6), fall back to the family name (ID 1), then sanitise to a valid PDF name.
fn derive_font_name(face: &Face) -> String {
    let raw = name_record(face, 6)
        .or_else(|| name_record(face, 1))
        .unwrap_or_default();
    sanitize_pdf_name(&raw)
}

/// First Unicode-decodable, non-empty `name` record with the given ID, if any.
fn name_record(face: &Face, name_id: u16) -> Option<String> {
    face.names()
        .into_iter()
        .filter(|n| n.name_id == name_id)
        .filter_map(|n| n.to_string())
        .find(|s| !s.is_empty())
}

/// Strip a raw font name down to characters valid in a PDF name (`[A-Za-z0-9-]`) — PDF BaseFont
/// names cannot contain spaces. Falls back to `"Font"` when nothing survives.
fn sanitize_pdf_name(raw: &str) -> String {
    let cleaned: String = raw
        .chars()
        .filter(|c| c.is_ascii_alphanumeric() || *c == '-')
        .collect();
    if cleaned.is_empty() {
        "Font".to_string()
    } else {
        cleaned
    }
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
        assert!(font.base_font.ends_with("+SourceSerif4"));
        assert_eq!(font.base_font.len(), 6 + 1 + FONT_NAME.len());
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

    /// Spec 0004: the bundled default keeps its historical descriptor flags exactly
    /// (`SERIF | NON_SYMBOLIC`) — no regression from the flag-derivation refactor.
    #[test]
    fn bundled_flags_are_serif_nonsymbolic() {
        let font = build(&charset("The Dungeon")).unwrap();
        assert_eq!(
            font.flags.bits(),
            (FontFlags::SERIF | FontFlags::NON_SYMBOLIC).bits()
        );
        assert_eq!(font.italic_angle, 0.0);
    }

    /// The user-font path (no name override) derives the embedded name from the face's own `name`
    /// table — exercised here by feeding the bundled bytes through `build_from_bytes` directly, so
    /// no second font asset is needed. Source Serif's PostScript name starts with "SourceSerif".
    #[test]
    fn user_path_derives_name_from_face() {
        let font = build_from_bytes(FONT_TTF, None, &charset("The Dungeon")).unwrap();
        let (tag, name) = font.base_font.split_once('+').expect("tag+name");
        assert_eq!(tag.len(), 6);
        assert!(
            name.starts_with("SourceSerif"),
            "derived name should come from the face, got {name:?}"
        );
        assert!(
            !name.contains(' '),
            "derived name must be a valid PDF name (no spaces)"
        );
        // Non-italic, non-monospaced upright face ⇒ only NON_SYMBOLIC (no SERIF: user path).
        assert_eq!(font.flags.bits(), FontFlags::NON_SYMBOLIC.bits());
    }

    #[test]
    fn outline_format_guard() {
        assert!(check_outline_format(true, false).is_ok()); // TrueType
        assert!(check_outline_format(true, true).is_ok()); // glyf wins if both present
        let cff = check_outline_format(false, true).unwrap_err();
        assert!(matches!(cff, ExportError::Font(m) if m.contains("CFF")));
        assert!(check_outline_format(false, false).is_err()); // no outlines at all
    }

    #[test]
    fn embeddable_guard_rejects_only_restricted() {
        assert!(check_embeddable(None).is_ok());
        assert!(check_embeddable(Some(Permissions::Installable)).is_ok());
        assert!(check_embeddable(Some(Permissions::PreviewAndPrint)).is_ok());
        assert!(check_embeddable(Some(Permissions::Editable)).is_ok());
        assert!(check_embeddable(Some(Permissions::Restricted)).is_err());
    }

    #[test]
    fn sanitizes_pdf_names() {
        assert_eq!(sanitize_pdf_name("Source Serif 4"), "SourceSerif4");
        assert_eq!(sanitize_pdf_name("My Font-Bold"), "MyFont-Bold");
        assert_eq!(sanitize_pdf_name("Ünïcödé!"), "ncd"); // non-ASCII/punct dropped
        assert_eq!(sanitize_pdf_name("   "), "Font"); // nothing survives → fallback
        assert_eq!(sanitize_pdf_name(""), "Font");
    }
}
