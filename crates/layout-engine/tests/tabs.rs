//! Spec 0067: a paragraph positioned by its tab stops, end to end.
//!
//! The mechanism's arithmetic is asserted in `text-layout` (`tab_tests`), to a hundredth of a point
//! and against hand-computed numbers. What this file asserts is the part only the engine can: that a
//! document naming stops actually places its text at them, that a document naming none is unmoved,
//! and that a tabbed line spends no justification — because a tab is a hard position and stretching
//! the gap in front of one would move the stop.

use quill_core_model::{
    Block, Color, Document, Leader, ParagraphStyle, StyleSheet, TabAlign, TabStop, TextAlign,
    BODY_STYLE,
};
use quill_layout_engine::{lay_out, PlacedBlock};
use quill_text_layout::{MonospaceRunMetrics, NoHyphenator};

const METRICS: MonospaceRunMetrics = MonospaceRunMetrics { em_ratio: 0.6 };
const INK: Color = Color::Gray { v: 0.0 };

/// A one-block document whose body style names `stops`.
fn doc_with(stops: Vec<TabStop>, text: &str) -> Document {
    let mut doc = Document::sample();
    doc.content = vec![Block::body(text, INK)];
    doc.assets.clear();
    let mut styles = StyleSheet::default();
    styles.paragraph.insert(
        BODY_STYLE.to_string(),
        ParagraphStyle {
            align: TextAlign::Justified,
            ..ParagraphStyle::default()
        },
    );
    if !stops.is_empty() {
        styles.tabs.insert(BODY_STYLE.to_string(), stops);
    }
    doc.styles = styles;
    doc.assign_missing_block_ids().expect("ids");
    doc
}

/// Every placed text piece of the first page, as `(x, text)` relative to its frame.
fn placed(doc: &Document) -> Vec<(f32, String)> {
    let pages = lay_out(doc, &METRICS, &NoHyphenator);
    let mut out = Vec::new();
    for block in &pages[0].blocks {
        if let PlacedBlock::Text { frame, lines, .. } = block {
            for line in lines {
                out.push((frame.x_pt, line.text.clone()));
            }
        }
    }
    // Relative to the leftmost piece, so the frame's own inset does not have to be spelled out here.
    let origin = out.iter().map(|(x, _)| *x).fold(f32::INFINITY, f32::min);
    out.into_iter().map(|(x, t)| (x - origin, t)).collect()
}

#[test]
fn a_right_stop_ends_its_segment_at_the_stop() {
    // "Chapter One" is 66 pt at 6 pt per character; the page number is right-aligned to 300.
    let doc = doc_with(
        vec![TabStop {
            position_pt: 300.0,
            align: TabAlign::Right,
            leader: None,
        }],
        "Chapter One\t42",
    );
    let pieces = placed(&doc);
    let number = pieces
        .iter()
        .find(|(_, t)| t == "42")
        .expect("the number is placed");
    // Two characters at 6 pt: its left edge is 12 pt before the stop.
    assert!(
        (number.0 - 288.0).abs() < 0.01,
        "expected 288, got {}",
        number.0
    );
}

#[test]
fn a_dot_leader_is_drawn_between_the_two_and_touches_neither() {
    let doc = doc_with(
        vec![TabStop {
            position_pt: 300.0,
            align: TabAlign::Right,
            leader: Some(Leader {
                glyph: '.',
                gap_pt: 3.0,
            }),
        }],
        "Chapter One\t42",
    );
    let pieces = placed(&doc);
    let leader = pieces
        .iter()
        .find(|(_, t)| t.starts_with(".."))
        .expect("a leader is drawn");
    let title_end = 66.0;
    let number_start = 288.0;
    assert!(
        leader.0 >= title_end + 3.0 - 0.01,
        "leader starts at {} , title ends at {title_end}",
        leader.0
    );
    let leader_end = leader.0 + 6.0 * leader.1.chars().count() as f32;
    assert!(
        leader_end <= number_start - 3.0 + 0.01,
        "leader ends at {leader_end}, number starts at {number_start}"
    );
    // Whole repetitions only: no partial glyph at either end.
    assert!(leader.1.chars().all(|c| c == '.'));
}

#[test]
fn a_tabbed_line_spends_no_justification() {
    // The paragraph style is justified. A tabbed line is positioned by its stops, so stretching the
    // spaces in it would move text away from a stop it was placed at.
    let doc = doc_with(
        vec![TabStop {
            position_pt: 300.0,
            align: TabAlign::Right,
            leader: None,
        }],
        "a tabbed row of text\t42",
    );
    let pages = lay_out(&doc, &METRICS, &NoHyphenator);
    for block in &pages[0].blocks {
        if let PlacedBlock::Text { lines, .. } = block {
            for line in lines {
                assert_eq!(
                    line.space_adjust_pt, 0.0,
                    "a tabbed segment must not be stretched: {:?}",
                    line.text
                );
            }
        }
    }
}

#[test]
fn a_document_that_names_no_stops_is_unmoved() {
    // The mechanism fires on two conditions — stops named *and* a tab present — so neither a
    // document without stops nor one that names stops it never uses can be moved by this increment.
    let text = "an ordinary paragraph with enough words in it to run to a second line of prose";
    let bare = placed(&doc_with(Vec::new(), text));
    let named_unused = placed(&doc_with(
        vec![TabStop {
            position_pt: 100.0,
            align: TabAlign::Left,
            leader: None,
        }],
        text,
    ));
    assert_eq!(bare, named_unused);
    assert!(bare.len() > 1, "sanity: the prose should wrap");
}

#[test]
fn a_tab_in_a_document_with_no_stops_is_not_a_position() {
    // Without stops there is no mechanism to invoke, and the tab is whitespace like any other — the
    // behaviour that predates this increment, kept so a document cannot change by acquiring a tab
    // character it never asked to be positioned by.
    let pieces = placed(&doc_with(Vec::new(), "left\tright"));
    assert_eq!(pieces.len(), 1);
    assert_eq!(pieces[0].1, "left right");
}
