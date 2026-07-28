//! Document templates — the beginner on-ramp (spec 0036).
//!
//! A [`Template`] is everything a document has *except content*: trim, margins, a type scale, the
//! master pages and the per-page assignments that make page 1 a chapter opener. Starting from one
//! is the difference between "a blank page with zero margins" and "a book".
//!
//! The *bundled* templates are Rust data rather than files on disk: no template directory, no
//! search path, no decision about what happens when a template goes missing at open time, and no
//! new dependency. Spec 0053 then made the type loadable *from* a file — `quill new --from
//! house-style.json` — without adding any of that: `--from` takes a path the shell resolved, so
//! there is still nothing to search and nothing to go missing behind the user's back.
//!
//! A template file is a **published format** the moment it ships, so it carries its own
//! [`TEMPLATE_VERSION`] and its own migration chain (`version::migrate_template`), separate from
//! the document's [`FORMAT_VERSION`]. See spec 0053 for when each is owed a bump.

use std::sync::OnceLock;

use serde::{Deserialize, Serialize};

use crate::Indent;
use crate::{
    heading_style_name, version, Color, Document, LoadError, Margins, MasterPage, MasterStatic,
    PageOverride, PageSetup, ParagraphStyle, Pt, Rect, Size, StaticAlign, StyleSheet, TextAlign,
    BODY_STYLE, DEFAULT_BLEED_PT, FORMAT_VERSION, PAGE_TOKEN,
};

/// The current template-file format version (spec 0053).
///
/// A **separate integer** from [`FORMAT_VERSION`] because the two version different artifacts: a
/// template file is not a document, and coupling them would force every template ever written to be
/// re-versioned whenever the document model changed in a way templates never see — a new `Block`
/// variant, say, which a template cannot contain.
///
/// It is owed a bump on *either* of two triggers, and the second is the one that is easy to miss:
///
/// 1. the template envelope changes — a field added to or removed from [`Template`] itself; or
/// 2. a [`FORMAT_VERSION`] bump changes the serialized shape of [`PageSetup`], [`StyleSheet`],
///    [`MasterPage`] or [`PageOverride`], the four document types a template file embeds. Spec
///    0047's v2 → v3 reached `MasterStatic`, which lives inside a `MasterPage`, and would have owed
///    a template migration had template files existed then.
pub const TEMPLATE_VERSION: u32 = 1;

/// The style name bundled templates give their folios.
pub const FOLIO_STYLE: &str = "folio";

/// The master every bundled template applies by default.
pub const BODY_MASTER: &str = "body";

/// The master a bundled template puts on page 0, where it has one.
pub const OPENER_MASTER: &str = "chapter-opener";

/// A starting point for a new document: everything a [`Document`] has except its content.
///
/// Serializable since spec 0053, which is what makes `quill new --from house-style.json` possible.
/// The serialized form is the published template-file format documented in `docs/format-spec.md`.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Template {
    /// The template-file format version this template was written as. See [`TEMPLATE_VERSION`].
    ///
    /// Not `serde(default)`: `version::migrate_template` inserts it before deserialization, exactly
    /// as the document loader does for `format_version`, so an absent field is *decided* by the
    /// version gate rather than defaulted past it.
    pub template_version: u32,
    /// Slug used by `quill new --template <name>`.
    pub name: String,
    /// Human-readable label.
    pub title: String,
    /// One line, shown by `quill new --list`.
    pub description: String,
    pub page_setup: PageSetup,
    #[serde(default)]
    pub styles: StyleSheet,
    #[serde(default)]
    pub master_pages: Vec<MasterPage>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub default_master: Option<String>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub pages: Vec<PageOverride>,
}

/// Page geometry offered to a template from outside it — a POD preset (spec 0049) is the intended
/// source, and `quill new --preset` the intended caller.
///
/// Carries only the two numbers a preset and a template can both claim. Everything else a preset
/// states (ink limits, dpi floors, the PDF/X level) is an export-time concern that never touches a
/// template.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct PageGeometrySeed {
    pub trim: Size,
    pub bleed_pt: Pt,
}

/// Trims closer than this are the same trim. Sizes are `f32` points; a printer's 6×9 is 432×648
/// however it was arrived at, and an exact float compare would call two of them different.
const TRIM_EPSILON_PT: Pt = 0.01;

impl Template {
    /// The built-in templates.
    pub fn bundled() -> &'static [Template] {
        static BUNDLED: OnceLock<Vec<Template>> = OnceLock::new();
        BUNDLED.get_or_init(|| vec![adventure(), rulebook(), playtest()])
    }

    /// Look a bundled template up by slug.
    pub fn by_name(name: &str) -> Option<&'static Template> {
        Template::bundled().iter().find(|t| t.name == name)
    }

    /// The slugs of every bundled template, for error messages and `--list`.
    pub fn names() -> Vec<&'static str> {
        Template::bundled()
            .iter()
            .map(|t| t.name.as_str())
            .collect()
    }

    /// Serialize to the template-file format (spec 0053).
    pub fn to_json(&self) -> Result<String, serde_json::Error> {
        serde_json::to_string_pretty(self)
    }

    /// Parse a template file, migrating it forward if it is older than [`TEMPLATE_VERSION`] and
    /// refusing it if it is newer.
    ///
    /// Mirrors [`Document::from_json`] deliberately, including that it never leaks
    /// `serde_json::Error`: the file being JSON is an implementation detail of the format, and a
    /// caller matching on a `serde` error type would make the encoding impossible to change later
    /// (spec 0025's reasoning, applied to the second published format).
    pub fn from_json(s: &str) -> Result<Template, LoadError> {
        let mut value: serde_json::Value =
            serde_json::from_str(s).map_err(|e| LoadError::TemplateParse(e.to_string()))?;
        version::migrate_template(&mut value)?;
        serde_json::from_value(value).map_err(|e| LoadError::TemplateParse(e.to_string()))
    }

    /// Compose an outside page geometry with this template's own (spec 0053).
    ///
    /// The precedence, in two halves, each with its own reason:
    ///
    /// - **Trim: the template wins.** A template's masters, margins and furniture are authored
    ///   *against* a specific trim — the bundled ones compute a folio's `y_pt` from the page height
    ///   and its rect width from the trim width. Re-trimming from underneath moves every one of
    ///   those numbers without moving the geometry authored for them, so the folio lands on the last
    ///   line or past the trim, and nothing at layout time catches it because furniture does not
    ///   participate in the flow. That is the silent press failure `CLAUDE.md` forbids.
    /// - **Bleed: the larger wins.** Bleed is a floor, not a design choice — the distance art must
    ///   extend past the trim so the guillotine cannot cut white. A seed asking for more is stating
    ///   a press requirement and costs the design nothing, because bleed lives entirely outside the
    ///   trim box; a seed asking for less would cost something, so it is not honored either.
    ///
    /// A disagreement about the trim is *reported*, not swallowed — see [`Template::disagrees_on_trim`].
    pub fn seeded_with(&self, seed: PageGeometrySeed) -> Template {
        Template {
            page_setup: PageSetup {
                bleed_pt: self.page_setup.bleed_pt.max(seed.bleed_pt),
                ..self.page_setup
            },
            ..self.clone()
        }
    }

    /// Whether a seed's trim differs from this template's, so a caller can say so.
    ///
    /// A warning rather than a refusal, matching spec 0049's own severity choice for a trim outside
    /// a preset's list: an unusual trim is a conversation with the printer, not a corrupt file.
    pub fn disagrees_on_trim(&self, seed: PageGeometrySeed) -> bool {
        let t = self.page_setup.trim;
        (t.w_pt - seed.trim.w_pt).abs() > TRIM_EPSILON_PT
            || (t.h_pt - seed.trim.h_pt).abs() > TRIM_EPSILON_PT
    }
}

impl Document {
    /// An empty document laid out to this template.
    ///
    /// Empty rather than seeded with placeholder text on purpose: a starter document whose first
    /// act is to make the author delete someone else's words is worse than one that is simply
    /// ready. The template is what carries the design.
    pub fn from_template(template: &Template) -> Document {
        let mut doc = Document {
            format_version: FORMAT_VERSION,
            metadata: crate::Metadata::default(),
            page_setup: template.page_setup,
            content: Vec::new(),
            assets: Vec::new(),
            // Templates reference only the bundled face, which is embeddable. A document that
            // claimed otherwise would fail preflight the moment it was created, which is precisely
            // the experience these exist to prevent.
            fonts_embeddable: true,
            revision: 0,
            next_block_id: 0,
            styles: template.styles.clone(),
            master_pages: template.master_pages.clone(),
            default_master: template.default_master.clone(),
            pages: template.pages.clone(),
            components: Default::default(),
            requires: Vec::new(),
        };
        // Normalize exactly as `Document::sample()` does, and for the same reason: everything
        // downstream treats what it is handed as a *loaded* document, so one built in memory must
        // already look like one or every consumer special-cases it.
        //
        // It matters even with no content. `assign_missing_block_ids` raises `next_block_id` to 1
        // (ids start at 1 so 0 can mean UNASSIGNED), so skipping it here would give a document that
        // is not equal to itself after a save and load — which is how this was found.
        doc.assign_missing_block_ids()
            .expect("a template has no blocks, so no ids can collide");
        doc
    }
}

impl Template {
    /// The exact inverse of [`Document::from_template`]: everything a document has *except its
    /// content*, lifted back out (spec 0057).
    ///
    /// The round-trip is asserted rather than assumed. Without it, `from_template` and
    /// `from_document` are two functions that happen to share field names, and the day one grows a
    /// field the other does not is the day an extracted pack quietly stops carrying part of the
    /// look.
    pub fn from_document(
        doc: &Document,
        name: impl Into<String>,
        title: impl Into<String>,
        description: impl Into<String>,
    ) -> Template {
        Template {
            template_version: TEMPLATE_VERSION,
            name: name.into(),
            title: title.into(),
            description: description.into(),
            page_setup: doc.page_setup,
            styles: doc.styles.clone(),
            master_pages: doc.master_pages.clone(),
            default_master: doc.default_master.clone(),
            pages: doc.pages.clone(),
        }
    }
}

/// Ink. Grayscale rather than CMYK for furniture: a folio set in rich black would be a registration
/// problem on press for no typographic gain.
const INK: Color = Color::Gray { v: 0.0 };

/// A stylesheet with a type scale sized for `body_pt`, plus a folio style.
///
/// Built on [`StyleSheet::default`] rather than from scratch, so a template can never be missing a
/// style the resolver expects — an unknown name falls back to `body`, and losing a paragraph's
/// treatment silently is exactly what that fallback exists to avoid.
fn scaled_styles(body_pt: f32, leading_pt: f32, scale: &[(u8, f32, f32)]) -> StyleSheet {
    let mut styles = StyleSheet::default();
    styles.paragraph.insert(
        BODY_STYLE.to_string(),
        ParagraphStyle {
            font_size_pt: body_pt,
            leading_pt,
            align: TextAlign::Justified,
            space_before_pt: 0.0,
            space_after_pt: 0.0,
            indent: Indent::ZERO,
        },
    );
    for &(level, size, leading) in scale {
        styles.paragraph.insert(
            heading_style_name(level),
            ParagraphStyle {
                font_size_pt: size,
                leading_pt: leading,
                // Headings are ragged-left: justifying a one-line heading stretches it across the
                // measure. Space above separates it from the text it follows.
                align: TextAlign::Left,
                space_before_pt: leading * 0.75,
                space_after_pt: leading * 0.25,
                indent: Indent::ZERO,
            },
        );
    }
    styles.paragraph.insert(
        FOLIO_STYLE.to_string(),
        ParagraphStyle {
            font_size_pt: (body_pt - 1.0).max(7.0),
            leading_pt,
            align: TextAlign::Left,
            space_before_pt: 0.0,
            space_after_pt: 0.0,
            indent: Indent::ZERO,
        },
    );
    styles
}

/// A page-number folio at the fore-edge corner of the bottom margin band.
///
/// Two constraints decide the geometry, and both were found by rendering rather than by arithmetic.
///
/// `y_pt` must land between the text area's bottom edge and the trim, or the page number prints on
/// top of the last line — furniture is positioned absolutely and does not participate in the flow,
/// so nothing else would catch the collision.
///
/// Horizontally the rect is the **text measure** — from the spine margin to the fore-edge margin, as
/// the page looks on a recto — set [`StaticAlign::Outside`] and mirrored. The number therefore sits
/// at the outside corner of whichever half of the spread it lands on, one margin clear of the trim,
/// which is where a bound book puts it.
///
/// Before spec 0047 a static had neither alignment nor mirroring, and this function insetted the
/// rect by the fore-edge margin instead: with the line drawn from the rect's left edge, a full-width
/// rect printed the number hard against the trim, where the trimmer goes. That workaround is gone —
/// it encoded a missing model concept in each template, and the third template encoded it again.
fn folio(trim: Size, y_pt: f32, margins: Margins) -> MasterStatic {
    MasterStatic::Text {
        rect: Rect {
            x_pt: margins.inside_pt,
            y_pt,
            w_pt: (trim.w_pt - margins.inside_pt - margins.outside_pt).max(0.0),
            h_pt: 12.0,
        },
        text: PAGE_TOKEN.to_string(),
        color: INK,
        style: Some(FOLIO_STYLE.to_string()),
        align: StaticAlign::Outside,
        mirror: true,
    }
}

const DIGEST: Size = Size {
    w_pt: 432.0,
    h_pt: 648.0,
};

const LETTER: Size = Size {
    w_pt: 612.0,
    h_pt: 792.0,
};

/// 6×9 single-column adventure module.
fn adventure() -> Template {
    // Inside margin exceeds the fore-edge because a bound book's text drifts toward the gutter
    // otherwise — the detail a beginner is least likely to know to add.
    let body_margins = Margins {
        top_pt: 54.0,
        bottom_pt: 54.0,
        inside_pt: 63.0,
        outside_pt: 45.0,
    };
    Template {
        template_version: TEMPLATE_VERSION,
        name: "adventure".into(),
        title: "Adventure module (6×9)".into(),
        description: "Single-column 6×9 digest with a chapter opener and page numbers.".into(),
        page_setup: PageSetup {
            trim: DIGEST,
            bleed_pt: DEFAULT_BLEED_PT,
            facing_pages: true,
            margins: body_margins,
        },
        styles: scaled_styles(
            10.5,
            14.0,
            &[(1, 24.0, 28.0), (2, 17.0, 21.0), (3, 13.0, 17.0)],
        ),
        master_pages: vec![
            MasterPage {
                // The opener drops the first line a third of the way down the page and carries no
                // folio, so a chapter start reads as one.
                margins: Some(Margins {
                    top_pt: 198.0,
                    ..body_margins
                }),
                ..MasterPage::plain(OPENER_MASTER)
            },
            MasterPage {
                margins: Some(body_margins),
                // Text area ends at 648 - 54 = 594; the folio sits below it, inside the margin.
                statics: vec![folio(DIGEST, 606.0, body_margins)],
                ..MasterPage::plain(BODY_MASTER)
            },
        ],
        default_master: Some(BODY_MASTER.into()),
        pages: vec![PageOverride {
            master: Some(OPENER_MASTER.into()),
        }],
    }
}

/// 6×9 two-column rulebook.
fn rulebook() -> Template {
    let body_margins = Margins {
        top_pt: 54.0,
        bottom_pt: 54.0,
        inside_pt: 54.0,
        outside_pt: 40.0,
    };
    // Text area 432 - 54 - 40 = 338 pt; two columns with a 14 pt gutter are 162 pt each.
    Template {
        template_version: TEMPLATE_VERSION,
        name: "rulebook".into(),
        title: "Rulebook (6×9, two columns)".into(),
        description: "Two-column 6×9 reference book with a chapter opener and page numbers.".into(),
        page_setup: PageSetup {
            trim: DIGEST,
            bleed_pt: DEFAULT_BLEED_PT,
            facing_pages: true,
            margins: body_margins,
        },
        styles: scaled_styles(
            9.5,
            12.5,
            &[(1, 22.0, 26.0), (2, 15.0, 19.0), (3, 12.0, 15.0)],
        ),
        master_pages: vec![
            MasterPage {
                margins: Some(Margins {
                    top_pt: 216.0,
                    ..body_margins
                }),
                columns: 2,
                gutter_pt: 14.0,
                ..MasterPage::plain(OPENER_MASTER)
            },
            MasterPage {
                margins: Some(body_margins),
                columns: 2,
                gutter_pt: 14.0,
                statics: vec![folio(DIGEST, 606.0, body_margins)],
                ..MasterPage::plain(BODY_MASTER)
            },
        ],
        default_master: Some(BODY_MASTER.into()),
        pages: vec![PageOverride {
            master: Some(OPENER_MASTER.into()),
        }],
    }
}

/// US Letter single-column playtest document.
fn playtest() -> Template {
    let body_margins = Margins::uniform(72.0);
    Template {
        template_version: TEMPLATE_VERSION,
        name: "playtest".into(),
        title: "Playtest document (US Letter)".into(),
        description: "Single-column US Letter draft for hand-outs and playtest packets.".into(),
        page_setup: PageSetup {
            trim: LETTER,
            bleed_pt: DEFAULT_BLEED_PT,
            // A playtest packet is read single-sided and printed at home, so there is no spine to
            // mirror margins about.
            facing_pages: false,
            margins: body_margins,
        },
        styles: scaled_styles(
            11.0,
            14.5,
            &[(1, 20.0, 24.0), (2, 15.0, 19.0), (3, 12.5, 16.0)],
        ),
        // No opener: a playtest draft is a working document, not a book.
        master_pages: vec![MasterPage {
            margins: Some(body_margins),
            // Text area ends at 792 - 72 = 720; the folio sits below it, inside the margin.
            statics: vec![folio(LETTER, 738.0, body_margins)],
            ..MasterPage::plain(BODY_MASTER)
        }],
        default_master: Some(BODY_MASTER.into()),
        pages: Vec::new(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::Block;

    /// The text area a page's master produces, mirroring `DocumentTemplate::content_rect` in the
    /// layout engine. Duplicated here rather than depended on, because `core-model` sits *below*
    /// `layout-engine` — the point of the assertion is that furniture and text cannot collide, and
    /// that is checkable from the geometry alone.
    fn text_area(setup: &PageSetup, master: &MasterPage, page_index: usize) -> Rect {
        let m = master.margins.unwrap_or(setup.margins);
        let (left, right) = m.left_right(page_index, setup.facing_pages);
        Rect {
            x_pt: left,
            y_pt: m.top_pt,
            w_pt: setup.trim.w_pt - left - right,
            h_pt: setup.trim.h_pt - m.top_pt - m.bottom_pt,
        }
    }

    #[test]
    fn every_bundled_template_round_trips_through_the_manifest() {
        for t in Template::bundled() {
            let doc = Document::from_template(t);
            let back = Document::from_json(&doc.to_json().expect("save")).expect("load");
            assert_eq!(back, doc, "template `{}` must survive a save/load", t.name);
        }
    }

    #[test]
    fn every_bundled_template_has_real_margins() {
        // The entire point. A template with zero margins is the default the on-ramp exists to
        // avoid, and this is written as a loop so a fourth template cannot skip it.
        for t in Template::bundled() {
            for master in &t.master_pages {
                let m = master.margins.unwrap_or(t.page_setup.margins);
                for (edge, v) in [
                    ("top", m.top_pt),
                    ("bottom", m.bottom_pt),
                    ("inside", m.inside_pt),
                    ("outside", m.outside_pt),
                ] {
                    assert!(
                        v > 0.0,
                        "template `{}` master `{}` has a zero {edge} margin",
                        t.name,
                        master.name
                    );
                }
            }
        }
    }

    #[test]
    fn every_bundled_template_carries_the_styles_authoring_expects() {
        for t in Template::bundled() {
            for name in [BODY_STYLE, "h1", "h2", "h3", FOLIO_STYLE] {
                assert!(
                    t.styles.paragraph.contains_key(name),
                    "template `{}` is missing the `{name}` style",
                    t.name
                );
            }
            let body = t.styles.paragraph[BODY_STYLE];
            assert!(
                body.leading_pt > body.font_size_pt,
                "template `{}` sets body solid or tighter",
                t.name
            );
        }
    }

    #[test]
    fn every_master_name_a_bundled_template_uses_actually_exists() {
        // A dangling name degrades silently by design (spec 0035), which is right for a document a
        // user edited and wrong for one we ship — so the shipped ones are checked.
        for t in Template::bundled() {
            let doc = Document::from_template(t);
            assert!(
                doc.master_for(0).is_some(),
                "template `{}` resolves no master for page 0",
                t.name
            );
            for (i, over) in t.pages.iter().enumerate() {
                if let Some(name) = &over.master {
                    assert!(
                        t.master_pages.iter().any(|m| &m.name == name),
                        "template `{}` page {i} names missing master `{name}`",
                        t.name
                    );
                }
            }
            let default = t.default_master.as_deref().expect("a default master");
            assert!(
                t.master_pages.iter().any(|m| m.name == default),
                "template `{}` default master `{default}` does not exist",
                t.name
            );
        }
    }

    #[test]
    fn no_bundled_folio_can_land_on_the_text_it_labels() {
        // Furniture is positioned absolutely and never participates in the flow, so nothing at
        // layout time would catch a folio sitting on top of the last line. The margin band is what
        // makes it safe, and the numbers are authored by hand — so they are asserted.
        for t in Template::bundled() {
            for master in &t.master_pages {
                let area = text_area(&t.page_setup, master, 0);
                for s in &master.statics {
                    let rect = match s {
                        MasterStatic::Text { rect, .. } | MasterStatic::Image { rect, .. } => rect,
                    };
                    let clear = rect.y_pt >= area.y_pt + area.h_pt
                        || rect.y_pt + rect.h_pt <= area.y_pt
                        || rect.x_pt >= area.x_pt + area.w_pt
                        || rect.x_pt + rect.w_pt <= area.x_pt;
                    assert!(
                        clear,
                        "template `{}` master `{}`: static at y={} overlaps the text area {:?}",
                        t.name, master.name, rect.y_pt, area
                    );
                }
            }
        }
    }

    /// A generous stand-in for a folio's set width — wider than four digits at any bundled body
    /// size, so a clearance that holds for it holds for the real line. `core-model` sits below the
    /// font stack and cannot measure text, which is exactly why [`StaticAlign::x_for`] takes a
    /// width instead of finding one.
    const FOLIO_W_PT: f32 = 24.0;

    #[test]
    fn no_bundled_static_sits_against_the_trim_edge_on_either_half_of_a_spread() {
        // Found by rendering, not by arithmetic: before spec 0047 a folio with a full-width rect at
        // x = 0 did not come out centred, because a `MasterStatic::Text` was one line drawn from
        // its rect's left edge. It printed hard against the trim, which is where the guillotine
        // goes.
        //
        // Asserted as a margin-relative clearance rather than "> 0" so the assertion states the
        // press rule (furniture stays inside the margins) rather than the symptom — and now on
        // **both** pages of a spread, since parity is the half the numeric tests used to miss.
        for t in Template::bundled() {
            for master in &t.master_pages {
                let m = master.margins.unwrap_or(t.page_setup.margins);
                let clearance = m.inside_pt.min(m.outside_pt);
                let fore_edge = t.page_setup.trim.w_pt - clearance;
                for page in 0..2 {
                    for s in &master.statics {
                        let rect = s.rect_on(page, &t.page_setup);
                        let (x, w) = match s {
                            MasterStatic::Text { align, .. } => (
                                align.x_for(rect, FOLIO_W_PT, page, t.page_setup.facing_pages),
                                FOLIO_W_PT,
                            ),
                            MasterStatic::Image { .. } => (rect.x_pt, rect.w_pt),
                        };
                        assert!(
                            x >= clearance,
                            "template `{}` master `{}` page {page}: static starts at x={x} inside \
                             the {clearance} pt side margin",
                            t.name,
                            master.name,
                        );
                        assert!(
                            x + w <= fore_edge,
                            "template `{}` master `{}` page {page}: static ends at {} past the \
                             {fore_edge} pt fore-edge margin",
                            t.name,
                            master.name,
                            x + w,
                        );
                    }
                }
            }
        }
    }

    #[test]
    fn a_bundled_folio_sits_at_the_outside_corner_of_both_halves_of_a_spread() {
        // The design spec 0036 could not express and worked around with a fore-edge inset. A folio
        // at the same x on a recto and a verso is the defect; two different x values, each one
        // margin in from its own trim edge, is the fix.
        for t in Template::bundled()
            .iter()
            .filter(|t| t.page_setup.facing_pages)
        {
            let master = t
                .master_pages
                .iter()
                .find(|m| m.name == BODY_MASTER)
                .expect("a body master");
            let s = master.statics.first().expect("a folio");
            let m = master.margins.unwrap_or(t.page_setup.margins);
            let MasterStatic::Text { align, .. } = s else {
                panic!("the folio must be text")
            };
            let recto = align.x_for(s.rect_on(0, &t.page_setup), FOLIO_W_PT, 0, true);
            let verso = align.x_for(s.rect_on(1, &t.page_setup), FOLIO_W_PT, 1, true);
            assert_ne!(
                recto, verso,
                "template `{}`: a spread must not repeat one corner",
                t.name
            );
            assert!(
                (recto + FOLIO_W_PT - (t.page_setup.trim.w_pt - m.outside_pt)).abs() < 0.01,
                "template `{}`: the recto folio must end at the fore-edge margin, got {recto}",
                t.name
            );
            assert!(
                (verso - m.outside_pt).abs() < 0.01,
                "template `{}`: the verso folio must start at the fore-edge margin, got {verso}",
                t.name
            );
        }
    }

    #[test]
    fn a_template_with_an_opener_puts_it_on_page_zero_only() {
        for name in ["adventure", "rulebook"] {
            let t = Template::by_name(name).expect("bundled");
            let doc = Document::from_template(t);
            assert_eq!(
                doc.master_for(0).map(|m| m.name.as_str()),
                Some(OPENER_MASTER),
                "{name}: page 0 must open the chapter"
            );
            for page in 1..4 {
                assert_eq!(
                    doc.master_for(page).map(|m| m.name.as_str()),
                    Some(BODY_MASTER),
                    "{name}: page {page} must be body"
                );
            }
        }
    }

    #[test]
    fn an_openers_first_line_starts_lower_than_the_bodys() {
        // What "chapter opener" means, as a number rather than as a claim.
        for name in ["adventure", "rulebook"] {
            let t = Template::by_name(name).expect("bundled");
            let opener = t
                .master_pages
                .iter()
                .find(|m| m.name == OPENER_MASTER)
                .expect("opener");
            let body = t
                .master_pages
                .iter()
                .find(|m| m.name == BODY_MASTER)
                .expect("body");
            assert!(
                text_area(&t.page_setup, opener, 0).y_pt > text_area(&t.page_setup, body, 0).y_pt,
                "{name}: the opener must start lower than the body"
            );
        }
    }

    #[test]
    fn a_template_document_starts_empty_and_editable() {
        let doc = Document::from_template(Template::by_name("rulebook").expect("bundled"));
        assert!(doc.content.is_empty());
        assert_eq!(doc.revision, 0);
        assert_eq!(doc.format_version, FORMAT_VERSION);
        assert!(doc.fonts_embeddable, "a starter must not fail preflight");

        // And it takes content the same way any other document does.
        let mut doc = doc;
        doc.content.push(Block::body("first words", INK));
        doc.assign_missing_block_ids().expect("ids");
        assert!(doc.content[0].id().is_assigned());
    }

    #[test]
    fn the_default_page_setup_keeps_zero_margins() {
        // Spec 0036 answers the roadmap's standing question by making it moot, not by changing the
        // default: `Document::sample()` is the CI Ghostscript golden fixture and the export
        // byte-hash is derived from it. This assertion is what stops a later increment from
        // "fixing" the default and moving the golden path with it.
        assert_eq!(PageSetup::default().margins, Margins::default());
        assert_eq!(Margins::default().top_pt, 0.0);
    }

    // ---------------------------------------------------------------------------------------
    // Spec 0053 — user-authored templates.
    // ---------------------------------------------------------------------------------------

    #[test]
    fn every_bundled_template_round_trips_through_a_template_file() {
        // The test that proves the template file format is real rather than write-only. Written as
        // a loop over `bundled()` for spec 0036's reason: a fourth template must be caught by the
        // assertions that already exist rather than needing a new one.
        for t in Template::bundled() {
            let json = t.to_json().expect("serialize");
            let back = Template::from_json(&json).expect("load");
            assert_eq!(&back, t, "template `{}` must survive a file", t.name);
            // And every field the document actually consumes came back — asserted separately from
            // the whole-struct compare, because `assert_eq!` on a struct that gained a field by
            // mistake would still pass while the field was wrong in both halves.
            assert_eq!(back.page_setup, t.page_setup);
            assert_eq!(back.styles, t.styles);
            assert_eq!(back.master_pages, t.master_pages);
            assert_eq!(back.default_master, t.default_master);
            assert_eq!(back.pages, t.pages);
            assert_eq!(back.template_version, TEMPLATE_VERSION);
        }
    }

    #[test]
    fn a_template_loaded_from_a_file_builds_the_same_document_as_the_bundled_one() {
        // The round-trip above is about the `Template`; this is about the thing a user gets. A
        // format that round-tripped the struct but produced a different document would pass the
        // first test and fail the only claim that matters.
        for t in Template::bundled() {
            let loaded = Template::from_json(&t.to_json().expect("serialize")).expect("load");
            assert_eq!(
                Document::from_template(&loaded),
                Document::from_template(t),
                "template `{}` must build the same document from a file",
                t.name
            );
        }
    }

    #[test]
    fn a_template_file_newer_than_this_build_is_refused_not_silently_downgraded() {
        // Expressed relative to TEMPLATE_VERSION, so it keeps testing "one newer than we
        // understand" across every future bump rather than silently stopping at a literal.
        let next = TEMPLATE_VERSION + 1;
        let json = adventure().to_json().expect("serialize").replace(
            &format!("\"template_version\": {TEMPLATE_VERSION}"),
            &format!("\"template_version\": {next}"),
        );
        match Template::from_json(&json) {
            Err(LoadError::UnsupportedTemplateVersion { found, supported }) => {
                assert_eq!(found, next);
                assert_eq!(supported, TEMPLATE_VERSION);
                // The message has to be actionable, i.e. name both versions.
                let msg = LoadError::UnsupportedTemplateVersion {
                    found,
                    supported: TEMPLATE_VERSION,
                }
                .to_string();
                assert!(msg.contains("template file"), "got: {msg}");
            }
            other => panic!("expected UnsupportedTemplateVersion, got {other:?}"),
        }
    }

    #[test]
    fn a_far_future_template_version_is_refused_too() {
        let json = r#"{"template_version": 99, "name": "x", "title": "x", "description": "x",
                       "page_setup": {"trim": {"w_pt": 1.0, "h_pt": 1.0}, "bleed_pt": 9.0,
                                      "facing_pages": true}}"#;
        assert!(matches!(
            Template::from_json(json),
            Err(LoadError::UnsupportedTemplateVersion { found: 99, .. })
        ));
    }

    #[test]
    fn a_malformed_template_file_is_a_typed_parse_error_not_a_panic() {
        // Bound to `LoadError` explicitly, on spec 0025's posture: this is the assertion that the
        // public signature does not leak `serde_json::Error`.
        let err: LoadError = Template::from_json("{ this is not json").unwrap_err();
        assert!(matches!(err, LoadError::TemplateParse(_)), "got {err:?}");
        // And it names the *template*, not the document manifest — the two are different files and
        // an error that sends the reader to the wrong one is worse than no error text at all.
        assert!(err.to_string().contains("template file"), "{err}");
    }

    #[test]
    fn a_template_file_that_is_valid_json_but_the_wrong_shape_is_also_a_parse_error() {
        // The half a syntax test misses: well-formed JSON that does not match the schema. Loading
        // this as a defaulted template would hand the user a document silently missing its trim.
        let err =
            Template::from_json(r#"{"name": "x", "title": "x", "description": "x"}"#).unwrap_err();
        assert!(matches!(err, LoadError::TemplateParse(_)), "got {err:?}");

        let err = Template::from_json("[1, 2, 3]").unwrap_err();
        assert!(matches!(err, LoadError::TemplateParse(_)), "got {err:?}");

        let err = Template::from_json(
            r#"{"template_version": "one", "name": "x", "title": "x", "description": "x",
                "page_setup": {"trim": {"w_pt": 1.0, "h_pt": 1.0}, "bleed_pt": 9.0,
                               "facing_pages": true}}"#,
        )
        .unwrap_err();
        assert!(matches!(err, LoadError::TemplateParse(_)), "got {err:?}");
    }

    #[test]
    fn an_absent_template_version_is_treated_as_current() {
        // Tolerated so a hand-written file can be short, exactly as the document loader tolerates a
        // missing `format_version`.
        let json = r#"{"name": "hand", "title": "Hand-written", "description": "short",
                       "page_setup": {"trim": {"w_pt": 432.0, "h_pt": 648.0}, "bleed_pt": 9.0,
                                      "facing_pages": true}}"#;
        let t = Template::from_json(json).expect("a short template file must load");
        assert_eq!(t.template_version, TEMPLATE_VERSION);
        // Everything optional defaults, and the default stylesheet is a real one — so a minimal
        // template can never be missing a style the resolver expects.
        assert!(t.styles.paragraph.contains_key(BODY_STYLE));
        assert!(t.master_pages.is_empty());
        assert!(t.default_master.is_none());
        assert!(t.pages.is_empty());
    }

    #[test]
    fn the_documented_template_example_parses() {
        // The anti-drift guard this repo already uses for the manifest and the authoring syntax:
        // the example in `docs/format-spec.md` is *the* one someone copies, so it is parsed here
        // rather than trusted. Extracted from the doc itself, never duplicated.
        const DOC: &str = include_str!("../../../docs/format-spec.md");
        let example = DOC
            .split("```json")
            .skip(1)
            .filter_map(|rest| rest.split("```").next())
            .find(|block| block.contains("\"template_version\""))
            .expect("docs/format-spec.md must carry a ```json template-file example");

        let t = Template::from_json(example).expect("the documented template must load");
        assert_eq!(t.template_version, TEMPLATE_VERSION);
        // And it must demonstrate what it claims to: a real two-column book with a mirrored folio
        // and a chapter opener on page 0, not a stub that parses.
        assert_eq!(t.name, "house-style");
        assert_eq!(t.page_setup.trim, DIGEST);
        assert_eq!(t.default_master.as_deref(), Some(BODY_MASTER));
        assert_eq!(t.pages[0].master.as_deref(), Some(OPENER_MASTER));
        let body = t
            .master_pages
            .iter()
            .find(|m| m.name == BODY_MASTER)
            .expect("a body master");
        assert_eq!(body.columns, 2);
        let MasterStatic::Text { align, mirror, .. } = &body.statics[0] else {
            panic!("the folio must be text")
        };
        assert_eq!(*align, StaticAlign::Outside);
        assert!(*mirror);
        // The example is a document someone can actually start from.
        let doc = Document::from_template(&t);
        assert_eq!(
            doc.master_for(0).map(|m| m.name.as_str()),
            Some(OPENER_MASTER)
        );
        assert_eq!(
            doc.master_for(1).map(|m| m.name.as_str()),
            Some(BODY_MASTER)
        );
    }

    /// A stand-in for what spec 0049's `PodPreset` will hand `quill new --preset`.
    fn seed(w_pt: f32, h_pt: f32, bleed_pt: f32) -> PageGeometrySeed {
        PageGeometrySeed {
            trim: Size { w_pt, h_pt },
            bleed_pt,
        }
    }

    #[test]
    fn a_geometry_seed_never_retrims_a_template() {
        // The precedence spec 0053 fixes, and the half with teeth: a template's folio `y_pt` is
        // derived from the page height and its rect from the trim width, so re-trimming from
        // underneath would move the page without moving the furniture authored for it — furniture
        // does not participate in the flow, so nothing at layout time would catch the collision.
        let t = Template::by_name("rulebook").expect("bundled");
        let seeded = t.seeded_with(seed(LETTER.w_pt, LETTER.h_pt, DEFAULT_BLEED_PT));
        assert_eq!(
            seeded.page_setup.trim, DIGEST,
            "the template's trim must win"
        );
        // And everything else the template carries is untouched by a seed.
        assert_eq!(seeded.master_pages, t.master_pages);
        assert_eq!(seeded.styles, t.styles);
        assert_eq!(seeded.pages, t.pages);
    }

    #[test]
    fn a_geometry_seed_raises_a_templates_bleed_but_never_lowers_it() {
        // The other half: bleed is a press floor, not a design choice, and it lives entirely
        // outside the trim box — so honoring a stricter requirement costs the design nothing while
        // lowering it would cost something.
        let t = Template::by_name("adventure").expect("bundled");
        assert_eq!(t.page_setup.bleed_pt, DEFAULT_BLEED_PT);

        let stricter = t.seeded_with(seed(DIGEST.w_pt, DIGEST.h_pt, DEFAULT_BLEED_PT + 3.0));
        assert_eq!(stricter.page_setup.bleed_pt, DEFAULT_BLEED_PT + 3.0);

        let looser = t.seeded_with(seed(DIGEST.w_pt, DIGEST.h_pt, 0.0));
        assert_eq!(
            looser.page_setup.bleed_pt, DEFAULT_BLEED_PT,
            "a seed must not lower a template's bleed"
        );
    }

    #[test]
    fn a_trim_disagreement_is_reportable_rather_than_silent() {
        // A warning rather than a refusal, matching spec 0049's own severity choice: an unusual
        // trim is a conversation with the printer, not a corrupt file. But it must be *sayable*,
        // or the precedence above becomes the confusing-by-default behavior the spec exists to
        // prevent.
        let t = Template::by_name("rulebook").expect("bundled");
        assert!(t.disagrees_on_trim(seed(LETTER.w_pt, LETTER.h_pt, DEFAULT_BLEED_PT)));
        assert!(!t.disagrees_on_trim(seed(DIGEST.w_pt, DIGEST.h_pt, DEFAULT_BLEED_PT)));
        // Two trims that differ only below the epsilon are the same trim: 432x648 is 6x9 however
        // it was arrived at, and an exact float compare would report a disagreement that is not one.
        assert!(!t.disagrees_on_trim(seed(DIGEST.w_pt + 0.005, DIGEST.h_pt, DEFAULT_BLEED_PT)));
    }

    #[test]
    fn lookup_is_by_slug_and_unknown_names_are_none() {
        assert_eq!(
            Template::by_name("rulebook").map(|t| t.name.as_str()),
            Some("rulebook")
        );
        assert!(Template::by_name("Rulebook").is_none(), "slugs are exact");
        assert!(Template::by_name("nope").is_none());
        assert_eq!(Template::names().len(), Template::bundled().len());
    }
}
