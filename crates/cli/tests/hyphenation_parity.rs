//! Spec 0059: the screen and the press must break lines the same way.
//!
//! This is the whole increment, asserted directly. Until spec 0059, `quill render` laid out with
//! `NoHyphenator` while `quill export` used the real en-US hyphenator, so a document could have a
//! different line count — and therefore a different **page count** — on screen than in the file
//! that went to the printer. `CLAUDE.md` states the rule that breaks in as many words: *one shaper
//! for screen and press, so they cannot drift.*
//!
//! It lives in `cli` because `cli` is the only crate that depends on both paths, which is also why
//! it is the only place the two can be compared without inventing a dependency edge.

use quill_core_model::{Block, Color, Document, Margins, PageSetup, Size};
use quill_export_pdf::ExportOptions;
use quill_layout_engine::{LaidOutPage, PlacedBlock};
use quill_text_layout::NoHyphenator;

const INK: Color = Color::Gray { v: 0.0 };

/// Prose chosen to be hyphenation-sensitive: long words, in a narrow measure, so the two
/// hyphenators genuinely disagree. A corpus that hyphenates nowhere would pass this test with the
/// bug still in place.
const PASSAGES: &[&str] = &[
    "The extraordinary illumination of the subterranean chambers demonstrated an architectural \
     sophistication that contemporary scholarship had persistently underestimated, notwithstanding \
     considerable archaeological documentation.",
    "Incomprehensible inscriptions covered the antechamber walls, their interpretation complicated \
     by centuries of accumulated mineral deposition and the unfortunate enthusiasm of earlier \
     expeditions.",
    "Reconnaissance established that the passageway continued northwards for approximately three \
     hundred yards before terminating at an apparently impenetrable obstruction of considerable \
     antiquity.",
];

fn corpus() -> Vec<(&'static str, Document)> {
    let build = |trim: Size, margin_pt: f32| {
        let mut doc = Document::sample();
        doc.page_setup = PageSetup {
            trim,
            margins: Margins {
                top_pt: margin_pt,
                bottom_pt: margin_pt,
                inside_pt: margin_pt,
                outside_pt: margin_pt,
            },
            ..PageSetup::default()
        };
        doc.content = PASSAGES
            .iter()
            .flat_map(|p| {
                [
                    Block::heading(2, "A Chamber", INK),
                    Block::body(*p, INK),
                    Block::body(*p, INK),
                ]
            })
            .collect();
        doc.assign_missing_block_ids().expect("ids");
        doc
    };
    vec![
        (
            "single column",
            build(
                Size {
                    w_pt: 432.0,
                    h_pt: 648.0,
                },
                0.0,
            ),
        ),
        (
            "narrow measure",
            build(
                Size {
                    w_pt: 234.0,
                    h_pt: 432.0,
                },
                18.0,
            ),
        ),
        (
            "wide margins",
            build(
                Size {
                    w_pt: 432.0,
                    h_pt: 648.0,
                },
                90.0,
            ),
        ),
    ]
}

/// Every laid-out line, in order, as `page:text`. The strongest comparison available short of
/// comparing `PlacedBlock` wholesale, and unlike a page count it localizes a disagreement.
fn line_texts(pages: &[LaidOutPage]) -> Vec<String> {
    let mut out = Vec::new();
    for page in pages {
        for block in &page.blocks {
            if let PlacedBlock::Text { lines, .. } = block {
                for line in lines {
                    out.push(format!("{}:{}", page.index, line.text));
                }
            }
        }
    }
    out
}

#[test]
fn the_screen_and_the_press_break_lines_identically() {
    let font = quill_fonts::Font::bundled();
    let opts = ExportOptions::default();
    for (name, doc) in corpus() {
        let screen = quill_render::lay_out_for_screen(&doc, &font);
        let press = quill_export_pdf::lay_out_for_press(&doc, &opts).expect("press layout");

        assert_eq!(
            screen.len(),
            press.len(),
            "`{name}`: {} pages on screen, {} in the press file",
            screen.len(),
            press.len()
        );
        let (a, b) = (line_texts(&screen), line_texts(&press));
        assert_eq!(
            a.len(),
            b.len(),
            "`{name}`: {} lines on screen, {} in the press file",
            a.len(),
            b.len()
        );
        for (i, (s, p)) in a.iter().zip(&b).enumerate() {
            assert_eq!(s, p, "`{name}`: line {i} differs between screen and press");
        }
    }
}

/// The corpus has to actually exercise hyphenation, or the test above passes for the wrong reason.
///
/// Asserted against `NoHyphenator` — the parity default the seam was built around, and what the
/// screen path used before spec 0059. If this stops failing to match, the corpus has gone soft and
/// the parity assertion has stopped meaning anything.
#[test]
fn the_corpus_is_hyphenation_sensitive() {
    let font = quill_fonts::Font::bundled();
    let mut differed = 0;
    for (name, doc) in corpus() {
        let with = quill_render::lay_out_for_screen(&doc, &font);
        let without = quill_layout_engine::lay_out(&doc, &font, &NoHyphenator);
        if line_texts(&with) != line_texts(&without) {
            differed += 1;
        } else {
            eprintln!("note: `{name}` breaks identically with and without hyphenation");
        }
    }
    assert!(
        differed > 0,
        "no corpus document breaks differently with hyphenation on; this corpus cannot detect \
         the defect spec 0059 fixed"
    );
}
