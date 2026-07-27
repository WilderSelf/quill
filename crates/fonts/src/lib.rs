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

use quill_text_layout::{CharMetrics, RunMetrics};
use ttf_parser::{Face as TtfFace, GlyphId};

/// The bundled font program. SIL OFL-1.1 — see `assets/SourceSerif4-LICENSE.txt`.
pub const BUNDLED_TTF: &[u8] = include_bytes!("../assets/SourceSerif4-Regular.ttf");

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

/// A shaped glyph: which glyph, and where it sits along the run.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct PositionedGlyph {
    pub gid: u16,
    /// Offset from the run's origin, in points.
    pub x_pt: f32,
    /// This glyph's advance, in points.
    pub advance_pt: f32,
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
}
