//! Document templates — the beginner on-ramp (spec 0036).
//!
//! A [`Template`] is everything a document has *except content*: trim, margins, a type scale, the
//! master pages and the per-page assignments that make page 1 a chapter opener. Starting from one
//! is the difference between "a blank page with zero margins" and "a book".
//!
//! Templates are Rust data rather than files on disk: no template directory, no search path, no
//! decision about what happens when a template goes missing at open time, and no new dependency.
//! User-authored templates need all three of those problems solved and are an M3 follow-up.

use std::sync::OnceLock;

use crate::{
    heading_style_name, Color, Document, Margins, MasterPage, MasterStatic, PageOverride,
    PageSetup, ParagraphStyle, Rect, Size, StyleSheet, TextAlign, BODY_STYLE, DEFAULT_BLEED_PT,
    FORMAT_VERSION, PAGE_TOKEN,
};

/// The style name bundled templates give their folios.
pub const FOLIO_STYLE: &str = "folio";

/// The master every bundled template applies by default.
pub const BODY_MASTER: &str = "body";

/// The master a bundled template puts on page 0, where it has one.
pub const OPENER_MASTER: &str = "chapter-opener";

/// A starting point for a new document: everything a [`Document`] has except its content.
#[derive(Debug, Clone, PartialEq)]
pub struct Template {
    /// Slug used by `quill new --template <name>`.
    pub name: String,
    /// Human-readable label.
    pub title: String,
    /// One line, shown by `quill new --list`.
    pub description: String,
    pub page_setup: PageSetup,
    pub styles: StyleSheet,
    pub master_pages: Vec<MasterPage>,
    pub default_master: Option<String>,
    pub pages: Vec<PageOverride>,
}

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
        },
    );
    styles
}

/// A page-number folio in the bottom margin band, inset from the trim edge.
///
/// Two constraints decide the geometry, and both were found by rendering rather than by arithmetic.
///
/// `y_pt` must land between the text area's bottom edge and the trim, or the page number prints on
/// top of the last line — furniture is positioned absolutely and does not participate in the flow,
/// so nothing else would catch the collision.
///
/// `x_pt` must be inset. A [`MasterStatic::Text`] is drawn as one line starting at its rect's left
/// edge: there is no alignment on a static and no mirroring by page parity, so a full-width rect
/// does not centre the number, it prints it hard against the trim — where a trimmer will eventually
/// cut it off. Inset by the fore-edge margin, which is the smaller of the two sides, so the folio
/// clears the trim on a recto and a verso alike without ever landing inside the text column.
fn folio(trim: Size, y_pt: f32, inset_pt: f32) -> MasterStatic {
    MasterStatic::Text {
        rect: Rect {
            x_pt: inset_pt,
            y_pt,
            w_pt: (trim.w_pt - inset_pt * 2.0).max(0.0),
            h_pt: 12.0,
        },
        text: PAGE_TOKEN.to_string(),
        color: INK,
        style: Some(FOLIO_STYLE.to_string()),
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
                statics: vec![folio(DIGEST, 606.0, body_margins.outside_pt)],
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
                statics: vec![folio(DIGEST, 606.0, body_margins.outside_pt)],
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
            statics: vec![folio(LETTER, 738.0, body_margins.outside_pt)],
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

    #[test]
    fn no_bundled_static_sits_against_the_trim_edge() {
        // Found by rendering, not by arithmetic: a folio with a full-width rect at x = 0 does not
        // come out centred, because a `MasterStatic::Text` is one line drawn from its rect's left
        // edge — there is no alignment on a static. It printed hard against the trim, which is
        // where the guillotine goes.
        //
        // Asserted as a margin-relative clearance rather than "> 0" so the assertion states the
        // press rule (furniture stays inside the margins) rather than the symptom.
        for t in Template::bundled() {
            for master in &t.master_pages {
                let m = master.margins.unwrap_or(t.page_setup.margins);
                let clearance = m.inside_pt.min(m.outside_pt);
                for s in &master.statics {
                    let rect = match s {
                        MasterStatic::Text { rect, .. } | MasterStatic::Image { rect, .. } => rect,
                    };
                    assert!(
                        rect.x_pt >= clearance,
                        "template `{}` master `{}`: static starts at x={} inside the {clearance} pt \
                         side margin",
                        t.name,
                        master.name,
                        rect.x_pt
                    );
                    assert!(
                        rect.x_pt + rect.w_pt <= t.page_setup.trim.w_pt - clearance,
                        "template `{}` master `{}`: static runs past the fore-edge margin",
                        t.name,
                        master.name
                    );
                }
            }
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
