//! Font subsetting and composite-font embedding (spec 0002 req 3, spec 0004 user fonts, spec 0011
//! CFF fonts).
//!
//! A font program — one of `quill-fonts`' four bundled Source Serif 4 faces (SIL OFL-1.1, `glyf`
//! outlines; licence at `crates/fonts/assets/SourceSerif4-LICENSE.txt`) or a
//! user-supplied file — is subset to only the glyphs a document uses **in that face** and embedded
//! as a Type0
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

use std::collections::{BTreeMap, BTreeSet, HashMap};

use pdf_writer::types::FontFlags;
use quill_fonts::FaceKey;
use quill_text_layout::Hyphenator;
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

/// The bundled regular program (full file; subset at export time).
///
/// Taken from `quill-fonts` rather than from a second copy in this crate's own assets, as it was
/// until spec 0064: the family's other three faces could only come from there, and one face read
/// from here while three came from there is two answers to "which program is Source Serif 4".
/// The bytes are the ones every export byte-hash in the workspace was measured against.
pub(crate) const FONT_TTF: &[u8] = quill_fonts::BUNDLED_TTF;

/// PostScript-style family name for the bundled font, embedded after the subset tag
/// (`ABCDEF+<NAME>`). User fonts derive their own name via [`derive_font_name`].
pub(crate) const FONT_NAME: &str = "SourceSerif4";

/// What a face has to be able to draw: the **runs of text** set in it, and the loose characters
/// that reach it without belonging to a run (spec 0068).
///
/// Two collections rather than one, because a glyph id is not a function of a character. The
/// subset is cut by glyph, and a ligature's glyph exists only in the output of *shaping a run* —
/// no set of characters can name it. The loose characters are the other half: the space and the
/// hyphen the line breaker can synthesize, the digits a `{page}` token becomes, and everything a
/// caller can only offer one character at a time.
#[derive(Debug, Default, Clone)]
pub struct FaceText {
    /// Deduplicated, so a word repeated ten thousand times is shaped once and the subset tag does
    /// not depend on how often a document happens to say it.
    runs: BTreeSet<String>,
    chars: BTreeSet<char>,
}

impl FaceText {
    /// A face that draws nothing but the given characters — the shape every caller that predates
    /// spec 0068 had.
    pub fn from_chars(chars: impl IntoIterator<Item = char>) -> FaceText {
        FaceText {
            runs: BTreeSet::new(),
            chars: chars.into_iter().collect(),
        }
    }

    /// A face that draws exactly this text, as a run. A test convenience: production callers build
    /// a face up run by run, because that is how a document arrives.
    #[cfg(test)]
    pub fn from_text(text: &str) -> FaceText {
        let mut out = FaceText::default();
        out.add_run(text);
        out
    }

    pub fn add_run(&mut self, text: &str) {
        if !text.is_empty() {
            self.runs.insert(text.to_string());
        }
    }

    pub fn add_chars(&mut self, chars: impl IntoIterator<Item = char>) {
        self.chars.extend(chars);
    }

    pub fn merge(&mut self, other: &FaceText) {
        self.runs.extend(other.runs.iter().cloned());
        self.chars.extend(other.chars.iter().copied());
    }

    pub fn runs(&self) -> impl Iterator<Item = &str> {
        self.runs.iter().map(String::as_str)
    }

    /// Every character this face must be able to draw on its own — the loose ones plus every
    /// character of every run.
    pub fn chars(&self) -> BTreeSet<char> {
        let mut out = self.chars.clone();
        for run in &self.runs {
            out.extend(run.chars());
        }
        out
    }
}

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
    /// Character → remapped (subset) glyph id. **Not** the encoding path since spec 0068 — the
    /// writer draws the shaped glyph run — but still what a single character's advance is looked
    /// up through ([`quill_text_layout::CharMetrics`]) and what pins the subset invariant in
    /// [`tests::gids_are_consistent`].
    char_to_gid: HashMap<char, u16>,
    /// Original (font program) glyph id → remapped (subset) glyph id. This is the encoding path:
    /// shaping answers in the program's own ids, and `subsetter` renumbers.
    gid_map: HashMap<u16, u16>,
    /// Remapped glyph id → the characters it was shaped from, for the `/ToUnicode` CMap. Three
    /// characters for an `ffi`, one for an ordinary glyph.
    to_unicode: BTreeMap<u16, String>,
}

impl EmbeddedFont {
    /// Ascent in points at a given font size (baseline sits this far below the frame top).
    pub fn ascent_pt(&self, size_pt: f32) -> f32 {
        self.ascent * size_pt / 1000.0
    }

    /// Subset glyph id → the characters it came from (spec 0068), for the `/ToUnicode` CMap.
    pub fn to_unicode(&self) -> impl Iterator<Item = (u16, &str)> {
        self.to_unicode.iter().map(|(g, s)| (*g, s.as_str()))
    }

    /// Segments of encoded glyph bytes, each followed by a `TJ` adjustment (0 = none).
    ///
    /// This is what the writer draws: `text` shaped through the *same* entry point measurement
    /// used, encoded as Identity-H subset glyph ids, with the difference between what the viewer
    /// will advance by and what shaping asked for taken back as a `TJ` number.
    pub fn shape_piece(&self, shaper: &quill_fonts::Font, text: &str) -> Vec<(Vec<u8>, i32)> {
        self.shape_segments(shaper, text, 0.0)
            .into_iter()
            .map(|(bytes, adjust)| (bytes, adjust as i32))
            .collect()
    }

    /// [`shape_piece`](Self::shape_piece) with a justification amount spent after every inter-word
    /// space, in the same thousandths-of-a-text-space-unit the kern corrections are in.
    ///
    /// One shaping of the whole piece, not one per word: splitting at spaces first would throw
    /// away any kern pair that straddles a space — which is the defect this spec's named non-goal
    /// records on the *screen* side, and it would be perverse to introduce it here while fixing it
    /// there.
    pub fn shape_piece_justified(
        &self,
        shaper: &quill_fonts::Font,
        text: &str,
        per_space: f32,
    ) -> Vec<(Vec<u8>, f32)> {
        self.shape_segments(shaper, text, per_space)
    }

    /// The shared body of the two above.
    ///
    /// Shaping at a 1000 pt size makes each advance come out directly in the thousandths of an em
    /// that both `/W` and `TJ` are stated in, so no second scaling convention exists to disagree
    /// with the first.
    ///
    /// The sign: `TJ` amounts are **subtracted** from the position, and the viewer advances by
    /// `/W[gid]` — the raw `hmtx` advance. A tightening kern shapes narrower than `hmtx`, so
    /// `hmtx - shaped` is positive and subtracting it moves the pen left. That is the wanted
    /// direction, and it is the same convention the justification amount at [`crate::writer`]
    /// already uses.
    ///
    /// The amount is rounded to an integer **explicitly** rather than left to the bundled faces'
    /// `upem == 1000` to make it integral: a user font at 2048 would otherwise emit `ryu`
    /// shortest-round-trip floats, and both the file size and the digest's cross-platform
    /// reproducibility would start depending on `f32` ordering.
    ///
    /// A zero adjustment is not emitted — most pairs do not kern, and a `0` after each of them
    /// would grow every content stream for no effect. Consecutive uncorrected glyphs coalesce into
    /// one string, which is what keeps a kern-free line a single `Tj`.
    fn shape_segments(
        &self,
        shaper: &quill_fonts::Font,
        text: &str,
        per_space: f32,
    ) -> Vec<(Vec<u8>, f32)> {
        let glyphs = shaper.shape(text, 1000.0);
        let mut out: Vec<(Vec<u8>, f32)> = Vec::new();
        let mut current: Vec<u8> = Vec::new();
        for glyph in &glyphs {
            let gid = self.gid_map.get(&glyph.gid).copied().unwrap_or(0);
            current.extend_from_slice(&gid.to_be_bytes());
            let hmtx = self.widths.get(gid as usize).copied().unwrap_or(0.0);
            let kern = (hmtx - glyph.advance_pt).round() as i32 as f32;
            // Justification is spent after the space glyph itself, exactly where the character-wise
            // writer put it: one adjustment per inter-word space.
            let justify = if per_space != 0.0 && is_space_at(text, glyph.cluster) {
                per_space
            } else {
                0.0
            };
            let adjust = kern + justify;
            if adjust != 0.0 {
                out.push((std::mem::take(&mut current), adjust));
            }
        }
        if !current.is_empty() {
            out.push((current, 0.0));
        }
        out
    }
}

/// Whether the character at byte offset `at` is an inter-word space. A space never merges into a
/// ligature, so a cluster that starts at one *is* the space glyph.
fn is_space_at(text: &str, at: u32) -> bool {
    text.get(at as usize..).is_some_and(|s| s.starts_with(' '))
}

/// Per-glyph `hmtx` advances (spec 0015). Line breaking measures through the shared shaper in
/// `quill-fonts` (specs 0016, 0032, 0064); this per-char advance stays as the anchor a single-glyph
/// run is checked against. An unknown char maps to `.notdef` (GID 0) and uses its advance — the same
/// fallback [`EmbeddedFont::shape_piece`] takes for a glyph outside the subset.
impl quill_text_layout::CharMetrics for EmbeddedFont {
    fn advance_pt(&self, ch: char, size_pt: f32) -> f32 {
        let gid = self.char_to_gid.get(&ch).copied().unwrap_or(0) as usize;
        let em = self.widths.get(gid).copied().unwrap_or(0.0); // 1000-unit em
        em * size_pt / 1000.0
    }
}

/// Subset and measure the **bundled** font for exactly the text a document draws in it.
///
/// A thin wrapper over [`build_from_bytes`] that pins the name (`SourceSerif4`) and adds the
/// `SERIF` descriptor flag, preserving byte-for-byte the output every prior spec relied on.
pub fn build(text: &FaceText) -> Result<EmbeddedFont, ExportError> {
    let mut font = build_from_bytes(FONT_TTF, Some(FONT_NAME), text)?;
    font.flags |= FontFlags::SERIF;
    Ok(font)
}

/// The faces a document actually sets text in, each subset to the characters *it* carries
/// (spec 0064).
///
/// One `EmbeddedFont` per used face, and only the used ones: a document with no bold embeds no bold
/// program. The slot a face occupies is its PDF resource number, so `/F0` stays `/F0` for a document
/// that uses one face — which is every document that predates this spec.
pub struct EmbeddedFamily {
    faces: Vec<(FaceKey, EmbeddedFont)>,
    /// The same programs, parsed for measurement. Layout measures through this, the writer draws
    /// through the subsets above, and both come from one set of bytes (spec 0032).
    metrics: quill_fonts::FontFamily,
}

/// The face a run format names (spec 0064).
pub fn face_of(fmt: quill_text_layout::RunFormat) -> FaceKey {
    FaceKey::new(fmt.weight, fmt.italic)
}

impl EmbeddedFamily {
    pub fn new(faces: Vec<(FaceKey, EmbeddedFont)>, metrics: quill_fonts::FontFamily) -> Self {
        assert!(
            !faces.is_empty(),
            "a document sets text in at least one face"
        );
        EmbeddedFamily { faces, metrics }
    }

    pub fn len(&self) -> usize {
        self.faces.len()
    }

    pub fn faces(&self) -> impl Iterator<Item = (FaceKey, &EmbeddedFont)> {
        self.faces.iter().map(|(k, f)| (*k, f))
    }

    /// The face a run set in `want` is drawn with, as a slot index. A request the document never
    /// declared — which cannot happen for text that was laid out — falls back to slot 0 rather than
    /// panicking in the writer.
    pub fn slot(&self, want: FaceKey) -> usize {
        let resolved = self.metrics.select(want).key;
        self.faces
            .iter()
            .position(|(k, _)| *k == resolved)
            .unwrap_or(0)
    }

    pub fn font(&self, slot: usize) -> &EmbeddedFont {
        &self.faces[slot].1
    }

    /// The parsed program a slot's text is **shaped** through (spec 0068).
    ///
    /// The same object the layout engine measured with, reached the same way — a slot names a face
    /// key, and the family resolves it. If the writer shaped through anything else, the press file
    /// would once again draw a different glyph sequence than the one that was measured, which is
    /// the whole defect this spec closes.
    pub fn shaper(&self, slot: usize) -> &quill_fonts::Font {
        self.metrics
            .font(self.metrics.select(self.faces[slot].0).index)
    }

    /// The PDF resource name for a slot: `F0`, `F1`, …
    pub fn resource_name(slot: usize) -> String {
        format!("F{slot}")
    }

    /// The metrics layout measures with.
    pub fn metrics(&self) -> &quill_fonts::FontFamily {
        &self.metrics
    }
}

/// Subset and measure an arbitrary TrueType (`glyf`) or CFF (`.otf`) font `program` for the given
/// text. The outline flavour ([`OutlineKind`]) is detected and recorded so the writer picks the
/// matching PDF embedding path (spec 0011).
///
/// `name_override` pins the embedded family name (used for the bundled font); when `None`, the
/// name is derived from the font's own `name` table and sanitised to a valid PDF name. Rejects
/// fonts with neither outline table and fonts whose `OS/2` `fsType` forbids embedding.
pub fn build_from_bytes(
    program: &[u8],
    name_override: Option<&str>,
    text: &FaceText,
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

    let UsedGlyphs {
        gids: used_orig,
        char_gids: char_orig,
        to_unicode: to_unicode_orig,
    } = used_glyphs(program, &face, text);

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
    // Original → new GID: the encoding path, since shaping answers in the program's own ids.
    let gid_map: HashMap<u16, u16> = used_orig
        .iter()
        .map(|old| (*old, remapper.get(*old).unwrap_or(0)))
        .collect();
    // …and the same remap applied to the characters-behind-a-glyph map, which is written out as
    // the `/ToUnicode` CMap.
    let to_unicode: BTreeMap<u16, String> = to_unicode_orig
        .into_iter()
        .filter_map(|(old, s)| {
            remapper
                .get(old)
                .filter(|new| *new != 0)
                .map(|new| (new, s))
        })
        .collect();

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
        gid_map,
        to_unicode,
    })
}

/// The original glyph ids a face has to carry, the char→gid pairs, and what each glyph was shaped
/// from (spec 0068).
///
/// The gid set is the **union** of two passes, and the union is required rather than
/// belt-and-braces:
///
/// - **Shaping each collected run** is the only thing that can put a ligature's glyph in the
///   subset. No set of characters names the `ffi` glyph; only shaping `office` produces it.
/// - **The plain cmap lookup for every individual character** is what covers the pieces the writer
///   actually draws. The line breaker splits a run, and shaping a *substring* yields glyphs the
///   whole-run shaping never does: `office` shapes to `o ffi c e` with no standalone `f` or `i`
///   glyph at all, so a line broken between `of` and `fice` would draw `.notdef` if only the
///   full-run gids were subset.
///
/// A third pass covers the rest of that same hole from the other side. The breaker's other split
/// point is a hyphenation break, and the *halves* of a hyphenated word shape to glyphs neither of
/// the two passes above yields: `of-` and `fice` need the `fi` ligature, which appears in neither
/// the shaping of `office` (that gives `ffi`) nor in any single character. So each distinct word
/// is split at the offsets the shared hyphenator reports — the same one the layout pass breaks
/// with — and both halves are shaped, the first with the `-` the breaker materialises.
///
/// This is the file's existing principle applied one level up: a slightly larger subset is a cost;
/// a `.notdef` in a press file is a defect.
fn used_glyphs(program: &[u8], face: &Face, text: &FaceText) -> UsedGlyphs {
    let mut used: BTreeSet<u16> = BTreeSet::new();
    let mut to_unicode: BTreeMap<u16, String> = BTreeMap::new();

    // Shaping first, so a ligature's glyph is described by the characters it actually replaced
    // rather than by whatever single codepoint may also map to it.
    if let Some(shaper) = quill_fonts::Font::from_bytes(program) {
        let mut words: BTreeSet<&str> = BTreeSet::new();
        for run in text.runs() {
            shape_into(&shaper, run, &mut used, &mut to_unicode);
            words.extend(run.split_whitespace());
        }
        let hyphenator = quill_fonts::HypherHyphenator;
        for word in words {
            for off in hyphenator.hyphenate(word) {
                if off == 0 || off >= word.len() || !word.is_char_boundary(off) {
                    continue;
                }
                shape_into(
                    &shaper,
                    &format!("{}-", &word[..off]),
                    &mut used,
                    &mut to_unicode,
                );
                shape_into(&shaper, &word[off..], &mut used, &mut to_unicode);
            }
        }
    }

    let chars = text.chars();
    let mut char_orig: Vec<(char, u16)> = Vec::with_capacity(chars.len());
    for ch in chars {
        let gid = face.glyph_index(ch).map(|g| g.0).unwrap_or(0);
        used.insert(gid);
        char_orig.push((ch, gid));
        if gid != 0 {
            to_unicode.entry(gid).or_insert_with(|| ch.to_string());
        }
    }

    // Sorted and deduplicated, so the subset tag — an FNV hash over this vector — is a function of
    // *which* glyphs a document uses and not of the order they were discovered in.
    UsedGlyphs {
        gids: used.into_iter().collect(),
        char_gids: char_orig,
        to_unicode,
    }
}

/// What [`used_glyphs`] found, in the font program's own glyph ids (before `subsetter` renumbers).
struct UsedGlyphs {
    /// Sorted and deduplicated.
    gids: Vec<u16>,
    char_gids: Vec<(char, u16)>,
    /// Glyph id → the characters it was shaped from.
    to_unicode: BTreeMap<u16, String>,
}

/// Shape `text` and record every glyph it produces, plus the characters behind each one.
///
/// Clusters are ascending under the LTR direction the shaper forces, so the characters a glyph
/// covers run from its own cluster to the next glyph's.
fn shape_into(
    shaper: &quill_fonts::Font,
    text: &str,
    used: &mut BTreeSet<u16>,
    to_unicode: &mut BTreeMap<u16, String>,
) {
    let glyphs = shaper.shape(text, 1000.0);
    for (i, glyph) in glyphs.iter().enumerate() {
        used.insert(glyph.gid);
        if glyph.gid == 0 {
            continue;
        }
        let start = glyph.cluster as usize;
        let end = glyphs
            .get(i + 1)
            .map_or(text.len(), |next| next.cluster as usize);
        if start >= end || end > text.len() || !text.is_char_boundary(start) {
            continue;
        }
        if let Some(s) = text.get(start..end) {
            to_unicode.entry(glyph.gid).or_insert_with(|| s.to_string());
        }
    }
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

    fn charset(s: &str) -> FaceText {
        FaceText::from_chars(s.chars())
    }

    /// The bundled regular face as a shaper — what the writer draws through.
    fn bundled_shaper() -> quill_fonts::Font {
        quill_fonts::Font::from_bytes(FONT_TTF).expect("the bundled face parses")
    }

    /// Flatten the encoder's segments into the subset gids it actually emits.
    fn emitted_gids(font: &EmbeddedFont, shaper: &quill_fonts::Font, text: &str) -> Vec<u16> {
        font.shape_piece(shaper, text)
            .into_iter()
            .flat_map(|(bytes, _)| {
                bytes
                    .chunks(2)
                    .map(|c| u16::from_be_bytes([c[0], c[1]]))
                    .collect::<Vec<_>>()
            })
            .collect()
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
        let segments = font.shape_piece(&bundled_shaper(), "A");
        assert_eq!(segments.len(), 1, "one uncorrected glyph is one segment");
        assert_eq!(segments[0].1, 0, "an isolated glyph needs no correction");
        assert_eq!(segments[0].0.len(), 2);
        let gid = u16::from_be_bytes([segments[0].0[0], segments[0].0[1]]);
        assert_ne!(gid, 0, "'A' must map to a real subset glyph, not .notdef");
        assert!(gid < font.widths.len() as u16);
    }

    #[test]
    fn unknown_char_maps_to_notdef() {
        let font = build(&charset("A")).unwrap();
        // A char not in the subset encodes to GID 0.
        assert_eq!(
            emitted_gids(&font, &bundled_shaper(), "\u{2603}"), // snowman, not built
            vec![0]
        );
    }

    /// Spec 0068's first acceptance criterion: `ffi` in the source draws as **one** glyph, and that
    /// glyph's id is in the subset. Before this spec the subset was cut from a set of characters,
    /// so the ligature's id could not be in it at all.
    #[test]
    fn a_ligature_draws_as_one_glyph_and_is_in_the_subset() {
        let font = build(&FaceText::from_text("office")).unwrap();
        let shaper = bundled_shaper();
        let gids = emitted_gids(&font, &shaper, "office");
        assert_eq!(gids.len(), 4, "o + ffi + c + e, not six glyphs");
        assert!(gids.iter().all(|g| *g != 0), "no .notdef: {gids:?}");
        // The second glyph is the ligature, and its width is the ligature's own advance — proof it
        // reached the subset rather than being remapped onto something else.
        let ligature = gids[1] as usize;
        let orig = shaper.shape("ffi", 1000.0);
        assert_eq!(orig.len(), 1);
        assert!((font.widths[ligature] - orig[0].advance_pt).abs() < 0.01);
        assert_eq!(
            font.to_unicode.get(&(ligature as u16)).map(String::as_str),
            Some("ffi"),
            "the ligature must map back to the three characters it came from"
        );
    }

    /// The line breaker splits runs, and shaping a *substring* produces glyphs the whole-run
    /// shaping never yields — which is why the subset is a union rather than just the run's own
    /// glyphs. `office` gives no standalone `f`, and its hyphenated halves need the `fi` ligature
    /// that neither `office` nor any single character produces.
    #[test]
    fn a_broken_run_still_draws_from_the_subset() {
        let font = build(&FaceText::from_text("office")).unwrap();
        let shaper = bundled_shaper();
        for piece in ["of", "fice", "of-", "o", "f", "ffi", "fi", "ice"] {
            let gids = emitted_gids(&font, &shaper, piece);
            assert!(
                gids.iter().all(|g| *g != 0),
                "{piece:?} drew a .notdef: {gids:?}"
            );
        }
    }

    /// The correction's sign and magnitude, against the raw `hmtx` advances the `/W` array states.
    /// A tightening kern must come out **positive**, because `TJ` subtracts.
    #[test]
    fn a_kern_pair_emits_a_positive_correction() {
        let font = build(&FaceText::from_text("AV")).unwrap();
        let shaper = bundled_shaper();
        let segments = font.shape_piece(&shaper, "AV");
        let total: i32 = segments.iter().map(|(_, a)| a).sum();
        assert!(
            total > 0,
            "a tightening kern moves the pen left: {segments:?}"
        );
        // And it is exactly the difference the /W array will over-advance by.
        let gids = emitted_gids(&font, &shaper, "AV");
        let declared: f32 = gids.iter().map(|g| font.widths[*g as usize]).sum();
        let shaped: f32 = shaper
            .shape("AV", 1000.0)
            .iter()
            .map(|g| g.advance_pt)
            .sum();
        assert!((declared - shaped - total as f32).abs() < 0.51);
    }

    /// The control case the whole instrument rests on: text in which neither `kern` nor `liga`
    /// fires emits **no** adjustment at all, so it is still a single `Tj`.
    #[test]
    fn text_that_does_not_kern_emits_no_adjustment() {
        let font = build(&FaceText::from_text("mmmmm nnnnn ooooo")).unwrap();
        let segments = font.shape_piece(&bundled_shaper(), "mmmmm nnnnn ooooo");
        assert_eq!(segments.len(), 1, "one uncorrected run: {segments:?}");
        assert_eq!(segments[0].1, 0);
        assert_eq!(segments[0].0.len(), 17 * 2);
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
    /// `.notdef` (GID 0) advance for a char outside the subset — mirroring the fallback
    /// [`EmbeddedFont::shape_piece`] takes for a glyph the subset does not carry.
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

    /// Spec 0016 (increment 1, part 2): a single-glyph run has no kern pair or ligature, so shaped
    /// measurement equals the per-char advance — the parity anchor that the whole-string equality of
    /// part 1 becomes once real shaping is wired (kern pairs now differ, tested below).
    #[test]
    fn single_glyph_run_equals_advance_pt() {
        use quill_text_layout::{CharMetrics, RunMetrics};
        let font = build(&charset("AVo ")).unwrap();
        // Measured through the shared shaper the export actually lays out with (specs 0032, 0064),
        // over the same program this subset was cut from.
        let shaper = quill_fonts::Font::from_bytes(FONT_TTF).expect("the bundled face parses");
        for ch in ['A', 'V', 'o', ' '] {
            let shaped = shaper.measure_run(&ch.to_string(), 11.0);
            let per_char = font.advance_pt(ch, 11.0);
            assert!(
                (shaped - per_char).abs() < 1e-4,
                "single-glyph shaped width should equal advance_pt for {ch:?}: {shaped} vs {per_char}"
            );
        }
        // Empty run measures to zero.
        assert_eq!(shaper.measure_run("", 11.0), 0.0);
    }

    /// Spec 0016 acceptance: a run containing a real negative kern pair (`AV` in the bundled Source
    /// Serif 4) measures **strictly narrower** under rustybuzz shaping than the spec-0015 per-char
    /// sum of the same characters — proving kerning is now reflected in line-break measurement.
    #[test]
    fn shaped_kern_pair_is_narrower_than_per_char_sum() {
        use quill_text_layout::{CharMetrics, RunMetrics};
        let font = build(&charset("AV")).unwrap();
        let shaper = quill_fonts::Font::from_bytes(FONT_TTF).expect("the bundled face parses");
        let shaped = shaper.measure_run("AV", 11.0);
        let per_char = font.advance_pt('A', 11.0) + font.advance_pt('V', 11.0);
        assert!(
            shaped + 1e-4 < per_char,
            "AV should shape tighter than the per-char sum: shaped={shaped} sum={per_char}"
        );
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
