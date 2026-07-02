//! Font subsetting and composite-font embedding (spec 0002 req 3, spec 0004 user fonts, spec 0011
//! CFF fonts).
//!
//! A font program — the bundled OFL font (Source Serif 4, SIL OFL-1.1, `glyf` outlines) or a
//! user-supplied file — is subset to only the glyphs a document uses and embedded as a Type0
//! composite font with Identity-H encoding. The descendant-font flavour follows the outlines
//! ([`OutlineKind`]): TrueType embeds `glyf` as `FontFile2` under `CIDFontType2`, while CFF (`.otf`)
//! embeds the bare `CFF ` table as `FontFile3` under `CIDFontType0`. See
//! `specs/0004-user-font-embedding.md` and `specs/0011-cff-font-embedding.md`.
//!
//! The `subsetter` crate **remaps** glyph IDs to a compact range (0 = `.notdef`, then contiguous)
//! — it does not preserve original GIDs. So the content stream is encoded with the *remapped*
//! GIDs and `CIDToGIDMap` is `/Identity` (CID == new GID == subset glyph index). This is the
//! single sharpest correctness risk in export (a mismatch renders wrong glyphs while still
//! passing veraPDF's "font embedded" check); [`tests::gids_are_consistent`] pins it down.

use std::collections::{BTreeSet, HashMap};

use pdf_writer::types::FontFlags;
use subsetter::GlyphRemapper;
use ttf_parser::{Face, GlyphId, Permissions, RawFace, Tag};

use crate::ExportError;

/// Which outline flavour a font program carries — this decides the PDF embedding path.
///
/// TrueType (`glyf`) embeds the whole subset sfnt as `FontFile2` under a `CIDFontType2` descendant.
/// CFF embeds the bare `CFF ` table as `FontFile3` (`/Subtype /CIDFontType0C`) under a
/// `CIDFontType0` descendant — the only PDF 1.3-legal form (the `FontFile3 /Subtype /OpenType`
/// wrapper is PDF 1.6+). See `specs/0011-cff-font-embedding.md`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum OutlineKind {
    TrueType,
    Cff,
}

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
    /// The font program the writer embeds (uncompressed; the writer flate-compresses it). For
    /// [`OutlineKind::TrueType`] this is the subset sfnt (embedded as `FontFile2`); for
    /// [`OutlineKind::Cff`] this is the bare `CFF ` table (embedded as `FontFile3`).
    pub subset: Vec<u8>,
    /// Outline flavour of `subset`, selecting the writer's embedding path.
    pub outlines: OutlineKind,
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

/// Real per-glyph advances drive line breaking (spec 0015): the layout engine measures text with
/// this font's own `hmtx` widths, so a line that "fits" renders within the frame. An unknown char
/// maps to `.notdef` (GID 0) and uses its advance — the same fallback as [`EmbeddedFont::encode_line`].
impl quill_text_layout::CharMetrics for EmbeddedFont {
    fn advance_pt(&self, ch: char, size_pt: f32) -> f32 {
        let gid = self.char_to_gid.get(&ch).copied().unwrap_or(0) as usize;
        let em = self.widths.get(gid).copied().unwrap_or(0.0); // 1000-unit em
        em * size_pt / 1000.0
    }
}

/// Run measurement for line breaking (spec 0016 increment 1). This part-1 implementation is the
/// per-char sum of [`CharMetrics::advance_pt`] — kerning/ligature-free, so measured widths (and thus
/// every line break, page break, and exported byte) are identical to spec 0015. The `rustybuzz`-
/// backed shaper that replaces this body with real shaping lands in the follow-up increment; the
/// seam here keeps the export path building against `RunMetrics` in the meantime.
impl quill_text_layout::RunMetrics for EmbeddedFont {
    fn measure_run(&self, text: &str, size_pt: f32) -> f32 {
        use quill_text_layout::CharMetrics;
        text.chars().map(|ch| self.advance_pt(ch, size_pt)).sum()
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

/// Subset and measure an arbitrary TrueType (`glyf`) or CFF (`.otf`) font `program` for the given
/// `chars`. The outline flavour ([`OutlineKind`]) is detected and recorded so the writer picks the
/// matching PDF embedding path (spec 0011).
///
/// `name_override` pins the embedded family name (used for the bundled font); when `None`, the
/// name is derived from the font's own `name` table and sanitised to a valid PDF name. Rejects
/// fonts with neither outline table and fonts whose `OS/2` `fsType` forbids embedding.
pub fn build_from_bytes(
    program: &[u8],
    name_override: Option<&str>,
    chars: &BTreeSet<char>,
) -> Result<EmbeddedFont, ExportError> {
    let face = Face::parse(program, 0).map_err(|e| ExportError::Font(format!("parse: {e}")))?;
    let tables = face.tables();
    let outlines = outline_kind(tables.glyf.is_some(), tables.cff.is_some())?;
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

    // Remap to a compact GID range (always includes .notdef = 0), then subset. `subsetter` always
    // returns an sfnt wrapper (subset `glyf` or `CFF ` table inside), converting SID-keyed CFF to
    // CID-keyed with an identity GID=CID map — so the remapped GID is usable directly as the CID.
    let remapper = GlyphRemapper::new_from_glyphs_sorted(&used_orig);
    let subset_sfnt = subsetter::subset(program, 0, &remapper)
        .map_err(|e| ExportError::Font(format!("subset: {e}")))?;

    // TrueType embeds the whole subset sfnt as FontFile2; CFF embeds the bare `CFF ` table as
    // FontFile3/CIDFontType0C (PDF 1.3 forbids the FontFile3 OpenType wrapper). See spec 0011.
    let subset = match outlines {
        OutlineKind::TrueType => subset_sfnt,
        OutlineKind::Cff => extract_cff_table(&subset_sfnt)?,
    };

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
        outlines,
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

/// Classify a font's outline flavour. `glyf` wins when both tables are present (a rare but legal
/// combination); a CFF-only font selects the `FontFile3`/`CIDFontType0` path (spec 0011). A font
/// with neither outline table cannot be embedded.
fn outline_kind(has_glyf: bool, has_cff: bool) -> Result<OutlineKind, ExportError> {
    if has_glyf {
        Ok(OutlineKind::TrueType)
    } else if has_cff {
        Ok(OutlineKind::Cff)
    } else {
        Err(ExportError::Font(
            "font has no TrueType (glyf) or CFF outlines".into(),
        ))
    }
}

/// Pull the raw `CFF ` table bytes out of the subset sfnt that `subsetter` produced. PDF/X output
/// is PDF 1.3, where `FontFile3` must carry a bare CFF (`/Subtype /CIDFontType0C`) — the sfnt-
/// wrapped `/Subtype /OpenType` form is PDF 1.6+. `subsetter` always keeps a `CFF ` table for a
/// CFF input, so a missing table here is an internal invariant break, not a user-input error.
fn extract_cff_table(subset_sfnt: &[u8]) -> Result<Vec<u8>, ExportError> {
    let raw = RawFace::parse(subset_sfnt, 0)
        .map_err(|e| ExportError::Font(format!("subset parse: {e}")))?;
    raw.table(Tag::from_bytes(b"CFF "))
        .map(<[u8]>::to_vec)
        .ok_or_else(|| ExportError::Font("subset sfnt is missing its CFF table".into()))
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

    /// A synthetic CFF-outline OTF fixture (built from scratch with fontTools — no third-party
    /// outlines, so no license encumbrance; see the plan). Glyphs: .notdef + A–E + space.
    const TEST_CFF_OTF: &[u8] = include_bytes!("../assets/test-cff.otf");

    #[test]
    fn outline_kind_classifies() {
        assert_eq!(outline_kind(true, false).unwrap(), OutlineKind::TrueType);
        assert_eq!(outline_kind(true, true).unwrap(), OutlineKind::TrueType); // glyf wins
        assert_eq!(outline_kind(false, true).unwrap(), OutlineKind::Cff);
        assert!(outline_kind(false, false).is_err()); // no outlines at all
    }

    #[test]
    fn bundled_font_is_truetype() {
        let font = build(&charset("The Dungeon")).unwrap();
        assert_eq!(font.outlines, OutlineKind::TrueType);
    }

    /// The CFF path: an `.otf` embeds as a bare `CFF ` table, not a subset sfnt. A CFF program
    /// begins with its header (major version `0x01`); a subset sfnt would begin with the sfnt tag
    /// (`00 01 00 00` for TrueType or `OTTO` for OpenType). Guards the FontFile3/CIDFontType0C path.
    #[test]
    fn cff_embeds_bare_table() {
        let font = build_from_bytes(TEST_CFF_OTF, None, &charset("ABC")).unwrap();
        assert_eq!(font.outlines, OutlineKind::Cff);
        assert!(font.subset.len() >= 4, "CFF program too short to inspect");
        assert_eq!(
            font.subset[0], 0x01,
            "CFF program must start with major version 1"
        );
        assert_ne!(
            &font.subset[..4],
            b"OTTO",
            "must be bare CFF, not an sfnt wrapper"
        );
        assert_ne!(&font.subset[..4], &[0x00, 0x01, 0x00, 0x00]);
        assert!(font.widths.len() >= 2); // notdef + at least one real glyph
        assert!(font.base_font.contains("QuillTestCFF"));
    }

    /// The GID↔glyph invariant (see [`gids_are_consistent`]) must also hold on the CFF path: the
    /// width recorded for a char's subset GID equals the original face's advance for that char.
    #[test]
    fn cff_gids_are_consistent() {
        let face = Face::parse(TEST_CFF_OTF, 0).unwrap();
        let scale = 1000.0 / face.units_per_em() as f32;
        let font = build_from_bytes(TEST_CFF_OTF, None, &charset("ABCDE")).unwrap();
        for ch in "ABCDE".chars() {
            let new_gid = font.char_to_gid[&ch] as usize;
            let orig_gid = face.glyph_index(ch).unwrap().0;
            let orig_w = face.glyph_hor_advance(GlyphId(orig_gid)).unwrap() as f32 * scale;
            assert!(
                (font.widths[new_gid] - orig_w).abs() < 0.01,
                "width mismatch for '{ch}': subset gid {new_gid}"
            );
        }
    }

    /// Spec 0015: `CharMetrics::advance_pt` returns a char's real advance, and falls back to the
    /// `.notdef` (GID 0) advance for a char outside the subset — mirroring `encode_line`'s fallback.
    #[test]
    fn advance_pt_measures_and_falls_back_to_notdef() {
        use quill_text_layout::CharMetrics;
        let font = build(&charset("Ao")).unwrap();
        // A built char measures to its own glyph's advance at the given size.
        let a_gid = font.char_to_gid[&'A'] as usize;
        let expect_a = font.widths[a_gid] * 12.0 / 1000.0;
        assert!((font.advance_pt('A', 12.0) - expect_a).abs() < 0.001);
        // An un-built char falls back to widths[0] (.notdef).
        let expect_notdef = font.widths[0] * 12.0 / 1000.0;
        assert!((font.advance_pt('\u{2603}', 12.0) - expect_notdef).abs() < 0.001);
    }

    /// Spec 0016 (increment 1, part 1): the run measurement seam is at parity — `measure_run(s)`
    /// equals the sum of `advance_pt` over `s`'s chars, so line breaking is unchanged until the
    /// rustybuzz shaper replaces this body.
    #[test]
    fn measure_run_equals_per_char_sum() {
        use quill_text_layout::{CharMetrics, RunMetrics};
        let font = build(&charset("The Dungeon")).unwrap();
        for s in ["The", "Dungeon", "The Dungeon", ""] {
            let per_char: f32 = s.chars().map(|ch| font.advance_pt(ch, 11.0)).sum();
            assert!(
                (font.measure_run(s, 11.0) - per_char).abs() < 1e-4,
                "measure_run should equal the per-char sum for {s:?}"
            );
        }
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
