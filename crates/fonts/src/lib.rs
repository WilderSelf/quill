//! Shared font measurement, shaping and outlines — see `specs/0032-shared-fonts-and-page-geometry.md`.
//!
//! ## Why this crate exists
//!
//! Everything that decides where a glyph goes has to agree, or the screen shows one thing and the
//! printed page another. Until now the only real [`RunMetrics`] implementation lived inside
//! `export-pdf`'s **private** `mod fonts`, so a screen renderer had two options: depend on the PDF
//! exporter (dragging `pdf-writer` and the whole press pipeline into the paint path), or lay out
//! with the monospace approximation used by tests — and disagree with the exported PDF about every
//! line break.
//!
//! This crate is the third option. It holds font *facts* — advances, shaped run widths, ascent,
//! glyph outlines. It deliberately holds no PDF types: subsetting, Identity-H encoding and
//! FontDescriptor flags stay in `export-pdf`, because those are about the file format rather than
//! about the font.
//!
//! `export-pdf` measures through this crate too, so there is exactly one shaper in the workspace
//! and no way for the two paths to drift.

mod hyphenate;

pub use hyphenate::HypherHyphenator;

use std::collections::BTreeSet;
use std::sync::Mutex;

use quill_text_layout::{CharMetrics, RunFormat, RunMetrics};
use ttf_parser::{Face as TtfFace, GlyphId};

/// The bundled regular font program. SIL OFL-1.1 — see `assets/SourceSerif4-LICENSE.txt`, which is
/// the licence for all four faces.
pub const BUNDLED_TTF: &[u8] = include_bytes!("../assets/SourceSerif4-Regular.ttf");

/// The bundled bold face.
///
/// A separate program, and necessarily so: every bundled face is a *static instance* of Source
/// Serif 4 with no `fvar` table, so a weight cannot be instanced from another weight at run time.
/// Bold is a font, not a flag. All four are built by `tools/fonts/build-faces.py`, which records
/// where they came from and how to rebuild them.
pub const BUNDLED_BOLD_TTF: &[u8] = include_bytes!("../assets/SourceSerif4-Bold.ttf");

/// The bundled italic face — see [`BUNDLED_BOLD_TTF`] for why it is its own program.
pub const BUNDLED_ITALIC_TTF: &[u8] = include_bytes!("../assets/SourceSerif4-Italic.ttf");

/// The bundled bold-italic face — see [`BUNDLED_BOLD_TTF`] for why it is its own program.
pub const BUNDLED_BOLD_ITALIC_TTF: &[u8] = include_bytes!("../assets/SourceSerif4-BoldItalic.ttf");

/// Every face this build ships, in no particular order. The family that selects between them by
/// weight and style is spec 0064's; this is the inventory the assets themselves guarantee.
pub const BUNDLED_FACES: [&[u8]; 4] = [
    BUNDLED_TTF,
    BUNDLED_BOLD_TTF,
    BUNDLED_ITALIC_TTF,
    BUNDLED_BOLD_ITALIC_TTF,
];

/// Which face of a family: how heavy, and upright or slanted (spec 0064).
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Default)]
pub struct FaceKey {
    /// `OS/2` `usWeightClass` — 400 regular, 700 bold.
    pub weight: u16,
    pub italic: bool,
}

impl FaceKey {
    pub const REGULAR: FaceKey = FaceKey {
        weight: 400,
        italic: false,
    };

    pub fn new(weight: u16, italic: bool) -> FaceKey {
        FaceKey { weight, italic }
    }
}

/// What a family answered a request with.
#[derive(Debug, Clone, Copy)]
pub struct Selection {
    /// The face that will be used — equal to the request when the family had it.
    pub key: FaceKey,
    /// Its index in the family, which is also the slot a PDF resource name is derived from.
    pub index: usize,
}

/// The faces a document can set text in, indexed by weight and slant (spec 0064).
///
/// A family rather than a `Font` because bold is not a flag on a face: every bundled face is a
/// static instance with no `fvar`, so the four weights and slants quill ships are four font
/// programs. Selection is by nearest match and is *announced*, because a document that asked for
/// bold, got regular and was told nothing is a press file that quietly is not what its author wrote.
pub struct FontFamily {
    faces: Vec<(FaceKey, Font)>,
    /// Substitutions already reported, so a warning is printed once per distinct request rather than
    /// once per run. A warning printed per run is a warning nobody reads.
    reported: Mutex<BTreeSet<(FaceKey, FaceKey)>>,
}

impl FontFamily {
    /// The four faces this build ships.
    pub fn bundled() -> FontFamily {
        FontFamily::from_faces(
            [
                (FaceKey::new(400, false), BUNDLED_TTF),
                (FaceKey::new(700, false), BUNDLED_BOLD_TTF),
                (FaceKey::new(400, true), BUNDLED_ITALIC_TTF),
                (FaceKey::new(700, true), BUNDLED_BOLD_ITALIC_TTF),
            ]
            .into_iter()
            .map(|(k, p)| (k, Font::from_bytes(p).expect("a bundled face must parse"))),
        )
    }

    /// A family of exactly one face — a user-supplied program (spec 0004), which answers every
    /// request with the one face it has and says so.
    pub fn single(font: Font) -> FontFamily {
        let key = FaceKey::new(font.weight(), font.is_italic());
        FontFamily::from_faces([(key, font)])
    }

    pub fn from_faces(faces: impl IntoIterator<Item = (FaceKey, Font)>) -> FontFamily {
        let mut faces: Vec<(FaceKey, Font)> = faces.into_iter().collect();
        // A stable order, so a PDF resource slot is a function of the family and not of insertion.
        faces.sort_by_key(|(k, _)| *k);
        assert!(!faces.is_empty(), "a family needs at least one face");
        FontFamily {
            faces,
            reported: Mutex::new(BTreeSet::new()),
        }
    }

    pub fn len(&self) -> usize {
        self.faces.len()
    }

    pub fn is_empty(&self) -> bool {
        false // `from_faces` asserts otherwise
    }

    /// Every face, in slot order.
    pub fn faces(&self) -> impl Iterator<Item = (FaceKey, &Font)> {
        self.faces.iter().map(|(k, f)| (*k, f))
    }

    pub fn font(&self, index: usize) -> &Font {
        &self.faces[index].1
    }

    /// The regular face — what a caller that knows nothing about runs measures with, and what
    /// `measure_run` answers for.
    pub fn regular(&self) -> &Font {
        self.font(self.select(FaceKey::REGULAR).index)
    }

    /// Resolve a request to a face this family actually has.
    ///
    /// Nearest is defined, not incidental: the requested slant wins first, because an italic set
    /// upright is wrong in a way a slightly-off weight is not; then the smallest absolute weight
    /// difference; then the lighter of a tie, because a text face is more often wanted than a
    /// display one.
    pub fn select(&self, want: FaceKey) -> Selection {
        let index = self
            .faces
            .iter()
            .enumerate()
            .min_by_key(|(_, (k, _))| {
                (
                    k.italic != want.italic,
                    k.weight.abs_diff(want.weight),
                    k.weight,
                )
            })
            .map(|(i, _)| i)
            .expect("a family has at least one face");
        let key = self.faces[index].0;
        if key != want {
            let first = self
                .reported
                .lock()
                .map(|mut seen| seen.insert((want, key)))
                .unwrap_or(false);
            if first {
                eprintln!(
                    "warning: this family has no {}; setting it in {} instead",
                    describe(want),
                    describe(key)
                );
            }
        }
        Selection { key, index }
    }
}

fn describe(k: FaceKey) -> String {
    let slant = if k.italic { " italic" } else { "" };
    match k.weight {
        400 => format!("regular{slant}"),
        700 => format!("bold{slant}"),
        w => format!("weight {w}{slant}"),
    }
}

/// A parsed font, ready to measure and draw with.
///
/// Owns its program bytes so the borrow checker does not force every caller to keep the source
/// buffer alive alongside it — a font outlives the `Vec` it was read from in every real use.
pub struct Font {
    program: Vec<u8>,
    units_per_em: f32,
    ascent: f32,
    descent: f32,
}

/// A single drawable contour of a glyph, in font units with the baseline at y = 0.
///
/// Deliberately a flat command list rather than a backend path type: the screen renderer's
/// rasterizer is swappable (see the decisions log in `docs/roadmap.md`), and a font crate that
/// named a specific canvas type would pin that choice here.
#[derive(Debug, Clone, PartialEq)]
pub enum PathCmd {
    MoveTo {
        x: f32,
        y: f32,
    },
    LineTo {
        x: f32,
        y: f32,
    },
    QuadTo {
        cx: f32,
        cy: f32,
        x: f32,
        y: f32,
    },
    CurveTo {
        c1x: f32,
        c1y: f32,
        c2x: f32,
        c2y: f32,
        x: f32,
        y: f32,
    },
    Close,
}

/// A glyph's outline plus the advance to the next glyph.
#[derive(Debug, Clone, PartialEq, Default)]
pub struct Outline {
    pub commands: Vec<PathCmd>,
    /// Advance width in font units.
    pub advance: f32,
}

/// A shaped glyph: which glyph, where it sits along the run, and which characters it came from.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct PositionedGlyph {
    pub gid: u16,
    /// Offset from the run's origin, in points.
    pub x_pt: f32,
    /// This glyph's advance, in points.
    pub advance_pt: f32,
    /// Byte offset into the shaped text of the first character this glyph came from — HarfBuzz's
    /// cluster value (spec 0068).
    ///
    /// It is the only record of *which characters became which glyph*, and it exists only at the
    /// moment of shaping: once `office` has become `o ffi c e` there is nothing in the glyph ids to
    /// say the second one was three letters. The PDF writer needs it twice — to build the
    /// `/ToUnicode` map that keeps a press file searchable, and to know which glyph is the
    /// inter-word space justification is spent at. Clusters are ascending for the LTR direction
    /// this shaper forces, so the characters a glyph covers run from its own cluster to the next
    /// glyph's.
    pub cluster: u32,
}

impl Font {
    /// Parse a font program. `None` if `ttf-parser` cannot read it.
    pub fn from_bytes(program: impl Into<Vec<u8>>) -> Option<Font> {
        let program = program.into();
        let face = TtfFace::parse(&program, 0).ok()?;
        let units_per_em = face.units_per_em() as f32;
        // Scaled to the 1000-unit em the rest of the pipeline (and PDF) works in.
        let ascent = face.ascender() as f32 * 1000.0 / units_per_em;
        let descent = face.descender() as f32 * 1000.0 / units_per_em;
        Some(Font {
            program,
            units_per_em,
            ascent,
            descent,
        })
    }

    /// The font this build ships with.
    pub fn bundled() -> Font {
        Font::from_bytes(BUNDLED_TTF).expect("the bundled font must parse")
    }

    fn face(&self) -> TtfFace<'_> {
        TtfFace::parse(&self.program, 0).expect("program parsed once at construction")
    }

    pub fn program(&self) -> &[u8] {
        &self.program
    }

    pub fn units_per_em(&self) -> f32 {
        self.units_per_em
    }

    /// The face's own `OS/2` weight class, so a family built from arbitrary programs indexes them by
    /// what they say they are rather than by what a file was named.
    pub fn weight(&self) -> u16 {
        self.face().weight().to_number()
    }

    pub fn is_italic(&self) -> bool {
        self.face().is_italic()
    }

    /// How many glyphs `text` shapes to — the unit tracking is spent in.
    pub fn glyph_count(&self, text: &str) -> usize {
        match rustybuzz::Face::from_slice(&self.program, 0) {
            Some(face) => {
                let mut buffer = rustybuzz::UnicodeBuffer::new();
                buffer.push_str(text);
                buffer.set_direction(rustybuzz::Direction::LeftToRight);
                rustybuzz::shape(&face, &[], buffer).len()
            }
            None => text.chars().count(),
        }
    }

    /// Ascent in points at a given size — how far below a frame's top the first baseline sits.
    ///
    /// The PDF writer and the screen renderer both take their baseline from here, which is what
    /// keeps a line of text in the same place on screen as on the page.
    pub fn ascent_pt(&self, size_pt: f32) -> f32 {
        self.ascent * size_pt / 1000.0
    }

    pub fn descent_pt(&self, size_pt: f32) -> f32 {
        self.descent * size_pt / 1000.0
    }

    /// The glyph a character maps to, or `None` if the font has no coverage for it.
    pub fn glyph_for(&self, ch: char) -> Option<u16> {
        self.face().glyph_index(ch).map(|g| g.0)
    }

    /// A glyph's outline and advance, in font units. `None` for a glyph with no outline at all.
    ///
    /// Note that a *blank* glyph — a space — legitimately has an empty command list with a non-zero
    /// advance, so an empty outline is not the same as a missing one.
    pub fn outline(&self, gid: u16) -> Option<Outline> {
        let face = self.face();
        let id = GlyphId(gid);
        let advance = face.glyph_hor_advance(id)? as f32;
        let mut builder = OutlineCollector::default();
        face.outline_glyph(id, &mut builder);
        Some(Outline {
            commands: builder.commands,
            advance,
        })
    }

    /// Shape `text` and return each glyph with its position along the run, in points.
    ///
    /// The summed advances equal [`measure_run`](RunMetrics::measure_run) for the same input — the
    /// invariant screen rendering depends on. If drawing and measuring could disagree, text would
    /// be positioned by one and wrapped by the other.
    pub fn shape(&self, text: &str, size_pt: f32) -> Vec<PositionedGlyph> {
        let Some(face) = rustybuzz::Face::from_slice(&self.program, 0) else {
            return Vec::new();
        };
        let mut buffer = rustybuzz::UnicodeBuffer::new();
        buffer.push_str(text);
        buffer.set_direction(rustybuzz::Direction::LeftToRight);
        let shaped = rustybuzz::shape(&face, &[], buffer);
        let scale = size_pt / face.units_per_em() as f32;
        let mut x = 0.0;
        let mut out = Vec::with_capacity(shaped.len());
        for (info, pos) in shaped
            .glyph_infos()
            .iter()
            .zip(shaped.glyph_positions().iter())
        {
            let advance = pos.x_advance as f32 * scale;
            out.push(PositionedGlyph {
                gid: info.glyph_id as u16,
                x_pt: x,
                advance_pt: advance,
                cluster: info.cluster,
            });
            x += advance;
        }
        out
    }

    /// A value that differs when the font program differs — a cache key for anything keyed on
    /// "which font was this measured with".
    pub fn identity(&self) -> u64 {
        let mut h: u64 = 0xcbf2_9ce4_8422_2325;
        // Hash length plus a sample rather than every byte: a font is megabytes and this is called
        // per cache lookup. Length and spread samples separate any two real fonts.
        h ^= self.program.len() as u64;
        h = h.wrapping_mul(0x0000_0100_0000_01b3);
        let step = (self.program.len() / 64).max(1);
        for b in self.program.iter().step_by(step) {
            h ^= *b as u64;
            h = h.wrapping_mul(0x0000_0100_0000_01b3);
        }
        h
    }
}

/// Per-character advances — the kerning-free fallback, and the anchor a single-glyph run is checked
/// against.
impl CharMetrics for Font {
    fn advance_pt(&self, ch: char, size_pt: f32) -> f32 {
        let face = self.face();
        let gid = face.glyph_index(ch).unwrap_or(GlyphId(0));
        let units = face.glyph_hor_advance(gid).unwrap_or(0) as f32;
        units * size_pt / self.units_per_em
    }
}

/// Run measurement backed by real shaping: kerning and ligatures are accounted for across the whole
/// run, unlike a per-character sum.
impl RunMetrics for Font {
    /// The font's own ascent, so a baseline snapped by the layout engine (spec 0058) lands where
    /// the writer and the renderer actually draw it — all three read this one number.
    fn ascent_pt(&self, size_pt: f32) -> f32 {
        Font::ascent_pt(self, size_pt)
    }

    fn measure_run(&self, text: &str, size_pt: f32) -> f32 {
        let Some(face) = rustybuzz::Face::from_slice(&self.program, 0) else {
            // Degrade to the kerning-free per-char sum rather than panic.
            return text.chars().map(|ch| self.advance_pt(ch, size_pt)).sum();
        };
        let mut buffer = rustybuzz::UnicodeBuffer::new();
        buffer.push_str(text);
        buffer.set_direction(rustybuzz::Direction::LeftToRight);
        let shaped = rustybuzz::shape(&face, &[], buffer);
        let units: i32 = shaped
            .glyph_positions()
            .iter()
            .map(|pos| pos.x_advance)
            .sum();
        units as f32 * size_pt / face.units_per_em() as f32
    }
}

/// Measurement through a family: a run measures in the face it names (spec 0064).
impl CharMetrics for FontFamily {
    fn advance_pt(&self, ch: char, size_pt: f32) -> f32 {
        self.regular().advance_pt(ch, size_pt)
    }
}

impl RunMetrics for FontFamily {
    /// The regular face, exactly as a lone [`Font`] answers. Every caller that predates the family
    /// measures what it always measured.
    fn measure_run(&self, text: &str, size_pt: f32) -> f32 {
        self.regular().measure_run(text, size_pt)
    }

    /// Ascent is the family's, not the face's: the four bundled faces share one set of vertical
    /// metrics precisely so that emphasising a word cannot move the line it sits on.
    fn ascent_pt(&self, size_pt: f32) -> f32 {
        RunMetrics::ascent_pt(self.regular(), size_pt)
    }

    fn measure_format(&self, text: &str, fmt: RunFormat) -> f32 {
        let font = self.font(self.select(FaceKey::new(fmt.weight, fmt.italic)).index);
        let base = font.measure_run(text, fmt.size_pt);
        if fmt.tracking_pt == 0.0 {
            return base;
        }
        // Per shaped glyph, because that is what a PDF `Tc` adds. Counting characters would measure
        // an `fi` ligature twice and draw it once.
        base + fmt.tracking_pt * font.glyph_count(text) as f32
    }
}

/// Collects `ttf-parser`'s outline callbacks into a [`PathCmd`] list.
#[derive(Default)]
struct OutlineCollector {
    commands: Vec<PathCmd>,
}

impl ttf_parser::OutlineBuilder for OutlineCollector {
    fn move_to(&mut self, x: f32, y: f32) {
        self.commands.push(PathCmd::MoveTo { x, y });
    }
    fn line_to(&mut self, x: f32, y: f32) {
        self.commands.push(PathCmd::LineTo { x, y });
    }
    fn quad_to(&mut self, cx: f32, cy: f32, x: f32, y: f32) {
        self.commands.push(PathCmd::QuadTo { cx, cy, x, y });
    }
    fn curve_to(&mut self, c1x: f32, c1y: f32, c2x: f32, c2y: f32, x: f32, y: f32) {
        self.commands.push(PathCmd::CurveTo {
            c1x,
            c1y,
            c2x,
            c2y,
            x,
            y,
        });
    }
    fn close(&mut self) {
        self.commands.push(PathCmd::Close);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_bundled_font_parses() {
        let font = Font::bundled();
        assert!(font.units_per_em() > 0.0);
        assert!(font.ascent_pt(10.0) > 0.0);
        assert!(font.descent_pt(10.0) < 0.0, "descent is below the baseline");
    }

    #[test]
    fn measuring_and_shaping_agree() {
        // The invariant screen rendering rests on: if the drawing path and the measuring path could
        // disagree, text would be wrapped by one and positioned by the other.
        let font = Font::bundled();
        for text in ["Hello", "AVA Waffle", "fi ligature test", "a"] {
            let measured = font.measure_run(text, 10.0);
            let summed: f32 = font.shape(text, 10.0).iter().map(|g| g.advance_pt).sum();
            assert!(
                (measured - summed).abs() < 0.001,
                "'{text}': measure_run {measured} vs summed advances {summed}"
            );
        }
    }

    #[test]
    fn shaping_accounts_for_kerning() {
        // The reason shaping exists rather than a per-character sum.
        let font = Font::bundled();
        let shaped = font.measure_run("AV", 100.0);
        let per_char: f32 = "AV".chars().map(|c| font.advance_pt(c, 100.0)).sum();
        assert!(
            shaped < per_char,
            "expected the kerned pair to be tighter: {shaped} vs {per_char}"
        );
    }

    #[test]
    fn glyph_positions_advance_left_to_right() {
        let font = Font::bundled();
        let glyphs = font.shape("abc", 12.0);
        assert_eq!(glyphs.len(), 3);
        assert_eq!(glyphs[0].x_pt, 0.0);
        for pair in glyphs.windows(2) {
            assert!(pair[1].x_pt > pair[0].x_pt);
        }
    }

    /// Spec 0068: the cluster of each shaped glyph is the byte offset of the first character it
    /// came from, so the characters behind a glyph run from its cluster to the next glyph's. This
    /// is what lets the PDF writer state that one glyph was three letters — the fact that stops
    /// existing the moment shaping is over.
    #[test]
    fn clusters_name_the_characters_a_glyph_came_from() {
        let font = Font::bundled();
        let text = "office";
        let glyphs = font.shape(text, 10.0);
        assert_eq!(
            glyphs.len(),
            4,
            "the `ffi` ligature should form: {glyphs:?}"
        );
        let mut spans = Vec::new();
        for (i, g) in glyphs.iter().enumerate() {
            let start = g.cluster as usize;
            let end = glyphs
                .get(i + 1)
                .map_or(text.len(), |next| next.cluster as usize);
            spans.push(&text[start..end]);
        }
        assert_eq!(spans, vec!["o", "ffi", "c", "e"]);
    }

    #[test]
    fn a_letter_has_an_outline_and_a_space_does_not() {
        // A blank glyph legitimately has an empty command list with a non-zero advance, so "empty"
        // must not be confused with "missing".
        let font = Font::bundled();
        let a = font.glyph_for('A').expect("font should cover 'A'");
        let outline = font.outline(a).expect("'A' should have an outline");
        assert!(!outline.commands.is_empty());
        assert!(outline.advance > 0.0);

        let space = font.glyph_for(' ').expect("font should cover space");
        let blank = font.outline(space).expect("space should still report");
        assert!(blank.commands.is_empty(), "a space draws nothing");
        assert!(blank.advance > 0.0, "but it still advances");
    }

    #[test]
    fn an_uncovered_character_has_no_glyph() {
        let font = Font::bundled();
        // A CJK ideograph is not in a Latin text face.
        assert!(font.glyph_for('漢').is_none());
    }

    #[test]
    fn identity_distinguishes_fonts_and_is_stable() {
        let a = Font::bundled();
        let b = Font::bundled();
        assert_eq!(a.identity(), b.identity(), "same bytes, same identity");
        let truncated = Font::from_bytes(&BUNDLED_TTF[..BUNDLED_TTF.len()]).unwrap();
        assert_eq!(a.identity(), truncated.identity());
    }

    #[test]
    fn garbage_is_rejected_rather_than_panicking() {
        assert!(Font::from_bytes(vec![0u8; 32]).is_none());
    }

    /// Every codepoint a face maps to a glyph.
    fn coverage(program: &[u8]) -> std::collections::BTreeSet<u32> {
        let face = TtfFace::parse(program, 0).expect("a bundled face must parse");
        let mut out = std::collections::BTreeSet::new();
        for table in face.tables().cmap.iter().flat_map(|c| c.subtables) {
            table.codepoints(|cp| {
                if table.glyph_index(cp).is_some() {
                    out.insert(cp);
                }
            });
        }
        out
    }

    #[test]
    fn all_four_bundled_faces_parse() {
        for program in BUNDLED_FACES {
            let font = Font::from_bytes(program).expect("every bundled face must parse");
            assert!(font.units_per_em() > 0.0);
            assert!(font.ascent_pt(10.0) > 0.0);
            assert!(font.descent_pt(10.0) < 0.0);
        }
    }

    #[test]
    fn the_faces_cover_exactly_the_same_characters() {
        // A family whose bold refuses a character its regular accepts would drop a glyph the
        // moment a word was emphasised — a silent hole rather than a visible one.
        let regular = coverage(BUNDLED_TTF);
        assert!(regular.len() > 200, "sanity: {} codepoints", regular.len());
        for program in BUNDLED_FACES {
            assert_eq!(
                coverage(program),
                regular,
                "a bundled face covers a different character set than the regular"
            );
        }
    }

    #[test]
    fn the_faces_share_one_set_of_vertical_metrics() {
        // Leading is a property of the paragraph, not of the run that happens to be in it. If bold
        // ascended further than regular, bolding a word would move the line — and would move it
        // differently on screen and on the page, since both read these numbers.
        let regular = Font::bundled();
        for program in BUNDLED_FACES {
            let font = Font::from_bytes(program).unwrap();
            assert_eq!(font.units_per_em(), regular.units_per_em());
            assert_eq!(font.ascent_pt(12.0), regular.ascent_pt(12.0));
            assert_eq!(font.descent_pt(12.0), regular.descent_pt(12.0));
        }
    }

    #[test]
    fn the_faces_are_actually_different_programs() {
        // The failure this guards against is a build that copied the regular four times: the
        // identities would match, and every "bold" would silently render regular.
        let ids: Vec<u64> = BUNDLED_FACES
            .iter()
            .map(|p| Font::from_bytes(*p).unwrap().identity())
            .collect();
        for (i, a) in ids.iter().enumerate() {
            for b in &ids[i + 1..] {
                assert_ne!(a, b, "two bundled faces are the same program");
            }
        }

        // And they are different in the direction their names claim.
        let text = "Handgloves";
        let regular = Font::bundled().measure_run(text, 12.0);
        let bold = Font::from_bytes(BUNDLED_BOLD_TTF)
            .unwrap()
            .measure_run(text, 12.0);
        let italic = Font::from_bytes(BUNDLED_ITALIC_TTF)
            .unwrap()
            .measure_run(text, 12.0);
        assert!(
            bold > regular,
            "bold {bold} should set wider than {regular}"
        );
        assert!(
            (italic - regular).abs() > 0.01,
            "italic {italic} should not measure as regular {regular}"
        );
    }

    #[test]
    fn the_family_answers_each_request_with_the_face_asked_for() {
        let family = FontFamily::bundled();
        assert_eq!(family.len(), 4);
        for want in [
            FaceKey::new(400, false),
            FaceKey::new(700, false),
            FaceKey::new(400, true),
            FaceKey::new(700, true),
        ] {
            assert_eq!(family.select(want).key, want);
        }
    }

    #[test]
    fn a_missing_face_resolves_to_the_nearest_one_by_a_stated_rule() {
        let family = FontFamily::bundled();
        // Slant wins first: a semibold italic is set in the italic, not in the bold upright, even
        // though 700 is the nearer weight to 600 than 400 is.
        assert_eq!(
            family.select(FaceKey::new(600, true)).key,
            FaceKey::new(700, true)
        );
        assert_eq!(
            family.select(FaceKey::new(300, false)).key,
            FaceKey::new(400, false)
        );
        assert_eq!(
            family.select(FaceKey::new(900, false)).key,
            FaceKey::new(700, false)
        );
        // A tie goes to the lighter face.
        assert_eq!(
            family.select(FaceKey::new(550, false)).key,
            FaceKey::new(400, false)
        );
    }

    #[test]
    fn a_one_face_family_answers_everything_with_the_face_it_has() {
        // The user-supplied-font case (spec 0004): asking it for bold italic is not an error, and
        // is not silent either — `select` reports the substitution on stderr.
        let family = FontFamily::single(Font::bundled());
        assert_eq!(family.len(), 1);
        for want in [FaceKey::new(700, false), FaceKey::new(400, true)] {
            assert_eq!(family.select(want).key, FaceKey::REGULAR);
        }
    }

    #[test]
    fn a_run_measures_in_the_face_it_names() {
        let family = FontFamily::bundled();
        let text = "Handgloves";
        let regular = family.measure_format(text, RunFormat::plain(12.0));
        let bold = family.measure_format(
            text,
            RunFormat {
                weight: 700,
                ..RunFormat::plain(12.0)
            },
        );
        assert!(bold > regular, "bold {bold} vs regular {regular}");
        // And `measure_run` still answers for the regular face, so every caller that predates the
        // family measures exactly what it did.
        assert_eq!(regular, family.measure_run(text, 12.0));
        assert_eq!(regular, Font::bundled().measure_run(text, 12.0));
    }

    #[test]
    fn tracking_is_spent_per_shaped_glyph_not_per_character() {
        // "office" shapes its `ffi` to one glyph, so tracking must be added six times minus the
        // ligature's saving — five, not seven. Counting characters would spend what the page does
        // not draw.
        let family = FontFamily::bundled();
        let regular = Font::bundled();
        let text = "office";
        let glyphs = regular.glyph_count(text);
        assert!(glyphs < text.chars().count(), "the ligature should form");
        let base = family.measure_format(text, RunFormat::plain(12.0));
        let tracked = family.measure_format(
            text,
            RunFormat {
                tracking_pt: 1.0,
                ..RunFormat::plain(12.0)
            },
        );
        assert!((tracked - base - glyphs as f32).abs() < 0.001);
    }

    #[test]
    fn the_family_ascent_does_not_change_with_the_face() {
        let family = FontFamily::bundled();
        assert_eq!(
            RunMetrics::ascent_pt(&family, 12.0),
            RunMetrics::ascent_pt(&Font::bundled(), 12.0)
        );
    }

    #[test]
    fn the_italic_faces_are_slanted_and_the_bold_faces_are_bold() {
        // Read from the faces' own metadata rather than from the file name, so a mislabelled asset
        // is caught here rather than by a reader wondering why nothing looks bold.
        let cases = [
            (BUNDLED_TTF, 400, false),
            (BUNDLED_BOLD_TTF, 700, false),
            (BUNDLED_ITALIC_TTF, 400, true),
            (BUNDLED_BOLD_ITALIC_TTF, 700, true),
        ];
        for (program, weight, italic) in cases {
            let face = TtfFace::parse(program, 0).unwrap();
            assert_eq!(face.weight().to_number(), weight);
            assert_eq!(face.is_italic(), italic);
            assert_eq!(face.is_bold(), weight == 700);
        }
    }
}
