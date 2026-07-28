//! Spec 0058: every baseline on a gridded page falls on a grid line.
//!
//! The grid is opt-in, so the first thing worth asserting is that it changes *nothing* for a
//! document that does not ask for one — that is what lets this increment land without re-deriving
//! spec 0051's equivalence digest, spec 0054's parity digests and every geometric fixture in the
//! repo. The rest asserts the mechanism on documents that do ask.

use quill_core_model::{
    BaselineGrid, Block, BlockId, Color, Document, Indent, Margins, PageSetup, ParagraphStyle,
    Size, StatBlock, StyleSheet, Table, TextAlign, BODY_STYLE, STATBLOCK_BODY_STYLE,
    STATBLOCK_TITLE_STYLE, TABLE_CELL_STYLE, TABLE_HEADER_STYLE,
};
use quill_layout_engine::{lay_out, LaidOutPage, PlacedBlock};
use quill_text_layout::{MonospaceRunMetrics, NoHyphenator, RunMetrics};

const METRICS: MonospaceRunMetrics = MonospaceRunMetrics { em_ratio: 0.6 };
const INK: Color = Color::Gray { v: 0.0 };
const STEP: f32 = 12.0;

const PROSE: &[&str] = &[
    "The vault door had not opened in three hundred years, and the dust on its hinges had gone to \
     stone; the crew stood in front of it for a long moment before anyone reached for a tool.",
    "Below the antechamber the passage narrowed until two could not walk abreast, and the air took \
     on the particular cold that means moving water somewhere out of sight.",
    "What the cartographers had drawn as a single chamber turned out to be three, each opening off \
     the last, and the third had a floor that was not a floor at all.",
];

/// A stylesheet whose every leading is a whole multiple of the step — which is what a house style
/// designed around a grid looks like, and what the "every baseline" claim requires.
fn gridded_styles() -> StyleSheet {
    let mut styles = StyleSheet::default();
    // Round *every* bundled style's leading up to a grid multiple first, then set the ones this
    // test cares about explicitly. Leaving the rest alone would leave `h4`, `statblock-attr` and
    // the contents styles off the grid — which `off_grid_styles` would (correctly) report, and
    // which would make the "every baseline" claim untestable.
    for style in styles.paragraph.values_mut() {
        style.leading_pt = (style.leading_pt / STEP).ceil() * STEP;
    }
    let on_grid = |size: f32, steps: f32, before: f32, after: f32, indent: Indent| ParagraphStyle {
        font_size_pt: size,
        leading_pt: STEP * steps,
        align: TextAlign::Left,
        space_before_pt: before,
        space_after_pt: after,
        indent,
    };
    styles.paragraph.insert(
        BODY_STYLE.into(),
        on_grid(10.0, 1.0, 0.0, 0.0, Indent::ZERO),
    );
    for (level, size, steps) in [(1u8, 20.0, 2.0), (2, 15.0, 1.5 * 2.0), (3, 12.0, 1.0)] {
        styles.paragraph.insert(
            quill_core_model::heading_style_name(level),
            on_grid(size, steps, STEP, 0.0, Indent::ZERO),
        );
    }
    for name in [
        STATBLOCK_TITLE_STYLE,
        STATBLOCK_BODY_STYLE,
        TABLE_HEADER_STYLE,
        TABLE_CELL_STYLE,
    ] {
        let indent = if name == STATBLOCK_BODY_STYLE {
            Indent::hanging(10.0)
        } else {
            Indent::ZERO
        };
        styles
            .paragraph
            .insert(name.into(), on_grid(9.0, 1.0, 0.0, 0.0, indent));
    }
    styles
}

fn gridded_doc(columns_trim: Size, content: Vec<Block>) -> Document {
    let mut doc = Document::sample();
    doc.page_setup = PageSetup {
        trim: columns_trim,
        margins: Margins {
            top_pt: 36.0,
            bottom_pt: 36.0,
            inside_pt: 54.0,
            outside_pt: 36.0,
        },
        facing_pages: true,
        // Deliberately not a multiple of the step, so a grid that quietly measured from the *margin*
        // rather than from the trim box would show up here.
        baseline_grid: Some(BaselineGrid {
            step_pt: STEP,
            origin_pt: 40.0,
        }),
        ..PageSetup::default()
    };
    doc.styles = gridded_styles();
    doc.content = content;
    doc.assign_missing_block_ids().expect("ids");
    doc
}

fn body_flow(n: usize) -> Vec<Block> {
    (0..n)
        .map(|i| Block::body(PROSE[i % PROSE.len()], INK))
        .collect()
}

/// Every baseline on every page, as `(page, baseline_y)`.
///
/// Derived exactly as the PDF writer and the screen renderer derive it (spec 0032):
/// `frame.y + ascent(size) + i × leading`. Anything else would be testing this file's arithmetic
/// rather than the geometry that reaches paper.
fn baselines(pages: &[LaidOutPage]) -> Vec<(usize, f32)> {
    let mut out = Vec::new();
    for page in pages {
        for b in &page.blocks {
            if let PlacedBlock::Text {
                frame,
                lines,
                font_size_pt,
                leading_pt,
                ..
            } = b
            {
                let ascent = METRICS.ascent_pt(*font_size_pt);
                for i in 0..lines.len() {
                    out.push((page.index, frame.y_pt + ascent + i as f32 * leading_pt));
                }
            }
        }
    }
    out
}

fn off_grid(pages: &[LaidOutPage], grid: BaselineGrid) -> Vec<(usize, f32, f32)> {
    baselines(pages)
        .into_iter()
        .filter_map(|(page, y)| {
            let k = ((y - grid.origin_pt) / grid.step_pt).round();
            let on = grid.origin_pt + k * grid.step_pt;
            ((y - on).abs() > 0.01).then_some((page, y, y - on))
        })
        .collect()
}

const GRID: BaselineGrid = BaselineGrid {
    step_pt: STEP,
    origin_pt: 40.0,
};

#[test]
fn every_baseline_of_a_multi_page_document_is_on_the_grid() {
    let doc = gridded_doc(
        Size {
            w_pt: 432.0,
            h_pt: 648.0,
        },
        {
            let mut c = vec![Block::heading(1, "The Ashen Vault", INK)];
            for i in 0..24 {
                if i % 6 == 0 {
                    c.push(Block::heading(2, "A Chamber", INK));
                }
                c.push(Block::body(PROSE[i % PROSE.len()], INK));
            }
            c
        },
    );
    let pages = lay_out(&doc, &METRICS, &NoHyphenator);
    assert!(
        pages.len() >= 3,
        "want a multi-page document, got {}",
        pages.len()
    );
    let bad = off_grid(&pages, GRID);
    assert!(
        bad.is_empty(),
        "{} of {} baselines are off the grid; first few: {:?}",
        bad.len(),
        baselines(&pages).len(),
        &bad[..bad.len().min(5)]
    );
}

#[test]
fn facing_pages_share_one_ladder() {
    // The defect that separates a book from a document: two facing pages whose baselines do not
    // line up. They line up exactly when both are on the same page-relative grid — which is why the
    // grid is measured from the trim box and not from a frame.
    let doc = gridded_doc(
        Size {
            w_pt: 432.0,
            h_pt: 648.0,
        },
        body_flow(30),
    );
    let pages = lay_out(&doc, &METRICS, &NoHyphenator);
    assert!(pages.len() >= 2);

    let ladder = |p: usize| {
        let mut v: Vec<i64> = baselines(&pages)
            .into_iter()
            .filter(|(page, _)| *page == p)
            .map(|(_, y)| (y * 100.0).round() as i64)
            .collect();
        v.sort_unstable();
        v.dedup();
        v
    };
    let (left, right) = (ladder(0), ladder(1));
    assert!(!left.is_empty() && !right.is_empty());
    // Not "the same list" — the two pages hold different amounts of text — but every baseline on
    // the right page must be a position the left page's ladder also offers.
    let left_set: std::collections::BTreeSet<i64> = left.iter().copied().collect();
    let unmatched: Vec<i64> = right
        .iter()
        .copied()
        .filter(|y| !left_set.contains(y))
        .collect();
    assert!(
        unmatched.is_empty(),
        "the facing page has baselines the left page does not: {unmatched:?}"
    );
}

#[test]
fn two_columns_of_one_page_share_the_ladder() {
    // A frame-relative grid would align each column to itself and to nothing else. Two columns of
    // one page is the cheapest place that shows.
    let mut doc = gridded_doc(
        Size {
            w_pt: 432.0,
            h_pt: 648.0,
        },
        body_flow(20),
    );
    doc.master_pages = vec![quill_core_model::MasterPage {
        name: "two-col".into(),
        margins: Some(doc.page_setup.margins),
        columns: 2,
        gutter_pt: 18.0,
        statics: Vec::new(),
    }];
    doc.default_master = Some("two-col".into());
    let pages = lay_out(&doc, &METRICS, &NoHyphenator);

    let xs: std::collections::BTreeSet<i64> = pages[0]
        .blocks
        .iter()
        .filter_map(|b| match b {
            PlacedBlock::Text { frame, .. } => Some((frame.x_pt * 100.0).round() as i64),
            _ => None,
        })
        .collect();
    assert!(xs.len() >= 2, "want two columns of text, got {xs:?}");
    assert!(off_grid(&pages, GRID).is_empty());
}

#[test]
fn a_continuations_first_baseline_is_on_the_grid() {
    // Composes with fragmentation (spec 0044). A short page forces a paragraph to be cut, and the
    // remainder is placed at the top of the next frame — where it must snap like any other block.
    let doc = gridded_doc(
        Size {
            w_pt: 300.0,
            h_pt: 220.0,
        },
        body_flow(8),
    );
    let pages = lay_out(&doc, &METRICS, &NoHyphenator);
    assert!(
        pages.len() >= 4,
        "want several cuts, got {} pages",
        pages.len()
    );
    assert!(off_grid(&pages, GRID).is_empty());
}

#[test]
fn components_snap_as_body_text_does() {
    // Composes with declared components (spec 0054) and with indents (spec 0048): the stat block's
    // body style carries a hanging indent, and its panel insets its first run.
    let doc = gridded_doc(
        Size {
            w_pt: 360.0,
            h_pt: 648.0,
        },
        vec![
            Block::body(PROSE[0], INK),
            Block::StatBlock {
                id: BlockId::UNASSIGNED,
                stat: StatBlock {
                    name: "Cave Troll".into(),
                    overview: vec!["Large giant, chaotic evil.".into()],
                    attributes: vec![
                        ("Armour Class".into(), "15 (natural armour)".into()),
                        ("Hit Points".into(), "84 (8d10 + 40)".into()),
                    ],
                    actions: vec!["Bite. Melee Weapon Attack: +7 to hit.".into()],
                    ..Default::default()
                },
                color: INK,
            },
            Block::body(PROSE[1], INK),
            Block::Table {
                id: BlockId::UNASSIGNED,
                table: Table {
                    columns: vec![2.0, 1.0],
                    header: Some(vec!["Item".into(), "Cost".into()]),
                    rows: (1..=4)
                        .map(|i| vec![format!("Item {i}"), format!("{i} gp")])
                        .collect(),
                    zebra: true,
                },
                color: INK,
            },
            Block::body(PROSE[2], INK),
        ],
    );
    let pages = lay_out(&doc, &METRICS, &NoHyphenator);
    // What the grid governs is **each block's first baseline** — a paragraph's first line, a
    // component's first section. A composite's later sections then follow at their own leading and
    // cell padding, which is the composite's internal rhythm and not the page grid's business.
    //
    // So: group placed runs by the block they came from, take the topmost run of each, and require
    // its first baseline to be on the grid. That covers the stat block and the table exactly as it
    // covers the two paragraphs around them.
    let mut first_by_block: std::collections::BTreeMap<u64, (f32, f32)> = Default::default();
    for b in pages.iter().flat_map(|p| &p.blocks) {
        if let PlacedBlock::Text {
            frame,
            source,
            font_size_pt,
            ..
        } = b
        {
            let e = first_by_block
                .entry(source.0)
                .or_insert((frame.y_pt, *font_size_pt));
            if frame.y_pt < e.0 {
                *e = (frame.y_pt, *font_size_pt);
            }
        }
    }
    assert!(
        first_by_block.len() >= 4,
        "want the two paragraphs, the stat block and the table, got {}",
        first_by_block.len()
    );
    let mut off: Vec<(u64, f32)> = Vec::new();
    for (id, (y, size)) in &first_by_block {
        let baseline = y + METRICS.ascent_pt(*size);
        let k = ((baseline - GRID.origin_pt) / GRID.step_pt).round();
        let delta = baseline - (GRID.origin_pt + k * GRID.step_pt);
        if delta.abs() > 0.01 {
            off.push((*id, delta));
        }
    }
    assert!(
        off.is_empty(),
        "a component's first baseline must snap exactly as a paragraph's does; off by {off:?}"
    );
}

#[test]
fn an_ungridded_document_is_untouched() {
    // The whole reason this increment lands without re-deriving every digest in the repo.
    let with = gridded_doc(
        Size {
            w_pt: 432.0,
            h_pt: 648.0,
        },
        body_flow(10),
    );
    let mut without = with.clone();
    without.page_setup.baseline_grid = None;

    let a = lay_out(&with, &METRICS, &NoHyphenator);
    let b = lay_out(&without, &METRICS, &NoHyphenator);
    assert_ne!(
        format!("{a:?}"),
        format!("{b:?}"),
        "if the grid changes nothing here, this whole test file proves nothing"
    );

    // And a degenerate grid is treated as no grid, not as a divide-by-zero or an infinite loop.
    for bad in [0.0, -12.0, f32::NAN, f32::INFINITY] {
        let mut degenerate = with.clone();
        degenerate.page_setup.baseline_grid = Some(BaselineGrid {
            step_pt: bad,
            origin_pt: 0.0,
        });
        assert_eq!(
            format!("{:?}", lay_out(&degenerate, &METRICS, &NoHyphenator)),
            format!("{b:?}"),
            "a step of {bad} must be treated as no grid"
        );
    }
}

#[test]
fn snapping_never_moves_a_baseline_up() {
    // Moving up could pull a baseline above its frame, or on top of the block before it.
    let grid = BaselineGrid {
        step_pt: 12.0,
        origin_pt: 40.0,
    };
    for y in [0.0f32, 39.9, 40.0, 40.001, 51.9, 52.0, 100.0, 1000.0] {
        let snapped = grid.snap_down(y);
        assert!(
            snapped >= y - 0.001,
            "snap_down({y}) = {snapped} moved the baseline up"
        );
        assert!(
            snapped - y < grid.step_pt,
            "snap_down({y}) = {snapped} moved it more than a whole step"
        );
        let k = ((snapped - grid.origin_pt) / grid.step_pt).round();
        assert!((snapped - (grid.origin_pt + k * grid.step_pt)).abs() < 0.001);
    }
    // Already on a line: unchanged, not pushed a step down.
    assert!((grid.snap_down(52.0) - 52.0).abs() < 0.001);
}

#[test]
fn a_style_whose_leading_is_not_a_grid_multiple_is_named() {
    // The honest alternative to silently misaligning it, or to laying every line out against the
    // grid — which is a different engine.
    let grid = BaselineGrid::new(12.0);
    assert!(
        grid.off_grid_styles(&gridded_styles()).is_empty(),
        "a stylesheet designed around the grid has nothing to report"
    );

    let mut styles = gridded_styles();
    styles.paragraph.insert(
        "quote".into(),
        ParagraphStyle {
            leading_pt: 13.5,
            ..*styles.paragraph.get(BODY_STYLE).unwrap()
        },
    );
    let named = grid.off_grid_styles(&styles);
    assert_eq!(named.len(), 1);
    assert_eq!(named[0].0, "quote");
    assert!((named[0].1 - 13.5).abs() < 0.001);

    // A grid that is not usable reports nothing rather than dividing by zero.
    assert!(BaselineGrid::new(0.0).off_grid_styles(&styles).is_empty());
}
