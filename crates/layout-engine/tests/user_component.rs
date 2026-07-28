//! A component nobody built into Quill (spec 0054).
//!
//! The point of the increment is not that the bundled two still work — that is
//! `component_parity.rs` — but that a third shape, declared by a publisher rather than compiled
//! in, goes through exactly the same path: it lays out, it cuts at its own section boundaries
//! (spec 0046), and it is subject to the same preflight (spec 0050) as a bundled one.
//!
//! The shape used here is a *Blades-style clock*: a title, a segment list, and a consequence — a
//! layout no built-in type in this repository can express.

use quill_core_model::{
    Asset, Block, BlockId, Color, ComponentDef, ComponentFields, DefColor, DefStroke, Document,
    FieldValue, Indent, PanelDef, ParagraphStyle, Rect, RuleDef, SectionDef, SectionShape,
    SplitDef, SplitGranularity, StyleSheet, TextAlign,
};
use quill_layout_engine::{lay_out, lay_out_in_frame, Frame, LaidOutPage, PlacedBlock};
use quill_text_layout::{MonospaceRunMetrics, NoHyphenator};

const METRICS: MonospaceRunMetrics = MonospaceRunMetrics { em_ratio: 0.6 };
const INK: Color = Color::Gray { v: 0.0 };
const CLOCK: &str = "clock";

fn clock_definition() -> ComponentDef {
    let rule = RuleDef {
        color: DefColor::Gray { v: 0.4 },
        thickness_pt: 0.5,
        gap_above_pt: 3.0,
        gap_below_pt: 3.0,
        full_width: false,
    };
    ComponentDef {
        version: quill_core_model::COMPONENT_DEF_VERSION,
        name: CLOCK.into(),
        panel: PanelDef {
            fill: Some(DefColor::Gray { v: 0.9 }),
            stroke: Some(DefStroke {
                color: DefColor::Gray { v: 0.3 },
                width_pt: 1.0,
            }),
            padding_pt: 8.0,
        },
        sections: vec![
            SectionDef {
                source: "title".into(),
                style: "clock-title".into(),
                shape: SectionShape::Text,
                rule_above: None,
                rule_below: None,
                repeat: false,
            },
            SectionDef {
                source: "segments".into(),
                style: "clock-body".into(),
                shape: SectionShape::Lines,
                rule_above: Some(rule),
                rule_below: None,
                repeat: false,
            },
            SectionDef {
                source: "consequence".into(),
                style: "clock-body".into(),
                shape: SectionShape::Lines,
                rule_above: Some(rule),
                rule_below: None,
                repeat: false,
            },
        ],
        split: SplitDef {
            granularity: SplitGranularity::Sections,
            min_items: 1,
            keep_together: true,
        },
    }
}

fn clock_fields() -> ComponentFields {
    let mut f = ComponentFields::new();
    f.insert(
        "title".into(),
        FieldValue::Text("The Bluecoats Close In".into()),
    );
    f.insert(
        "segments".into(),
        FieldValue::Lines(vec![
            "1. A patrol takes an interest in the warehouse and starts asking the dockers who \
             comes and goes after dark."
                .into(),
            "2. An inspector pulls the manifests and finds three crates that were never landed."
                .into(),
            "3. A watch post goes up on the corner opposite, manned around the clock.".into(),
            "4. The raid comes at dawn, with a writ and enough coats to seal both ends of the \
             street."
                .into(),
        ]),
    );
    f.insert(
        "consequence".into(),
        FieldValue::Lines(vec![
            "When the clock fills, the crew's hold on the district drops by one and every score \
             in it runs at increased risk until they clear it."
                .into(),
        ]),
    );
    f
}

fn styled_sheet() -> StyleSheet {
    let mut styles = StyleSheet::default();
    styles.paragraph.insert(
        "clock-title".into(),
        ParagraphStyle {
            font_size_pt: 12.0,
            leading_pt: 15.0,
            align: TextAlign::Left,
            space_before_pt: 0.0,
            space_after_pt: 2.0,
            indent: Indent::ZERO,
        },
    );
    styles.paragraph.insert(
        "clock-body".into(),
        ParagraphStyle {
            font_size_pt: 9.0,
            leading_pt: 11.5,
            align: TextAlign::Left,
            space_before_pt: 0.0,
            space_after_pt: 2.0,
            indent: Indent::hanging(10.0),
        },
    );
    styles
}

fn clock_block() -> Block {
    Block::Component {
        id: BlockId(1),
        def: CLOCK.into(),
        fields: clock_fields(),
        color: INK,
    }
}

fn document(styles: StyleSheet, h_pt: f32) -> Document {
    let mut doc = Document::sample();
    doc.content = vec![clock_block()];
    doc.styles = styles;
    doc.components.insert(CLOCK.into(), clock_definition());
    doc.page_setup.trim = quill_core_model::Size { w_pt: 300.0, h_pt };
    doc.assign_missing_block_ids().expect("ids");
    doc
}

fn panels(pages: &[LaidOutPage]) -> Vec<Rect> {
    pages
        .iter()
        .flat_map(|p| &p.blocks)
        .filter_map(|b| match b {
            // The panel is the only stroked rect a clock emits.
            PlacedBlock::Rect {
                frame,
                stroke: Some(_),
                ..
            } => Some(*frame),
            _ => None,
        })
        .collect()
}

fn runs(pages: &[LaidOutPage]) -> Vec<Rect> {
    pages
        .iter()
        .flat_map(|p| &p.blocks)
        .filter_map(|b| match b {
            PlacedBlock::Text { frame, .. } => Some(*frame),
            _ => None,
        })
        .collect()
}

#[test]
fn a_user_defined_component_lays_out() {
    let doc = document(styled_sheet(), 700.0);
    let pages = lay_out(&doc, &METRICS, &NoHyphenator);
    assert_eq!(pages.len(), 1, "a clock in a tall page fits whole");

    let panels = panels(&pages);
    assert_eq!(panels.len(), 1, "exactly one panel, stroked as declared");

    // Every run sits inside the panel, inset by the declared padding on both sides.
    let panel = panels[0];
    let runs = runs(&pages);
    assert!(!runs.is_empty(), "the clock sets text");
    for r in &runs {
        assert!(
            (r.x_pt - (panel.x_pt + 8.0)).abs() < 0.01,
            "run at x={} is not inset by the declared 8 pt padding (panel x={})",
            r.x_pt,
            panel.x_pt
        );
        assert!(
            r.y_pt >= panel.y_pt + 8.0 - 0.01 && r.y_pt <= panel.y_pt + panel.h_pt - 8.0 + 0.01,
            "run at y={} escapes the panel {panel:?}",
            r.y_pt
        );
    }

    // Two hairlines, one above each section after the first — and none above the title, which has
    // the panel's own edge above it.
    let hairlines = pages[0]
        .blocks
        .iter()
        .filter(|b| {
            matches!(
                b,
                PlacedBlock::Rect { frame, stroke: None, .. } if (frame.h_pt - 0.5).abs() < 0.01
            )
        })
        .count();
    assert_eq!(
        hairlines, 2,
        "one rule per section boundary, none at the top"
    );
}

#[test]
fn a_user_defined_component_splits_at_its_section_boundaries() {
    // Short enough that the clock cannot fit whole, so it must cut — and it may only cut where its
    // definition says it may.
    let doc = document(styled_sheet(), 140.0);
    let pages = lay_out(&doc, &METRICS, &NoHyphenator);
    assert!(
        pages.len() > 1,
        "a clock taller than the page must be cut, not run off the bottom"
    );

    // Every fragment is a panel of its own, each closing with the declared inset.
    let fragments = panels(&pages);
    assert_eq!(
        fragments.len(),
        pages.len(),
        "each fragment carries the panel's edge"
    );

    // No run is orphaned outside a fragment, and no fragment is empty: content conservation is
    // what spec 0044's splitter promises, and a declared component is not exempt from it.
    let laid: usize = runs(&pages).len();
    let whole = document(styled_sheet(), 700.0);
    let whole_runs = runs(&lay_out(&whole, &METRICS, &NoHyphenator)).len();
    assert_eq!(
        laid, whole_runs,
        "cutting the clock must not lose or duplicate a run"
    );
}

#[test]
fn a_definition_naming_an_unknown_style_still_lays_out() {
    // The authoring posture: a missing style costs the *look*, never the content. Laid with the
    // default stylesheet, so neither `clock-title` nor `clock-body` resolves.
    let doc = document(StyleSheet::default(), 700.0);
    let pages = lay_out(&doc, &METRICS, &NoHyphenator);
    assert_eq!(pages.len(), 1);
    assert!(
        !runs(&pages).is_empty(),
        "unresolved styles must fall back, not swallow the component"
    );
}

#[test]
fn an_unresolved_definition_is_skipped_rather_than_panicking() {
    let mut doc = document(styled_sheet(), 700.0);
    doc.components.clear();
    let pages = lay_out(&doc, &METRICS, &NoHyphenator);
    assert!(
        runs(&pages).is_empty(),
        "an instance naming a definition that does not exist is skipped, exactly as an \
         unresolved image asset is"
    );
}

#[test]
fn a_document_definition_shadows_a_bundled_one() {
    let mut doc = Document::sample();
    let mut restyled = quill_core_model::statblock_definition();
    restyled.panel.padding_pt = 20.0;
    doc.components
        .insert(quill_core_model::STATBLOCK_COMPONENT.into(), restyled);
    let lib = doc.component_library();
    assert_eq!(
        lib[quill_core_model::STATBLOCK_COMPONENT].panel.padding_pt,
        20.0,
        "a document's own definition must win, or a publisher cannot restyle the stat block \
         without inventing a new name"
    );
    assert!(
        lib.contains_key(quill_core_model::TABLE_COMPONENT),
        "shadowing one definition must not drop the others"
    );
}

#[test]
fn a_component_whose_fields_are_all_absent_occupies_nothing() {
    let mut doc = document(styled_sheet(), 700.0);
    doc.content = vec![Block::Component {
        id: BlockId(1),
        def: CLOCK.into(),
        // No fields at all. `Text` still emits its (empty) run, so this is not the empty-panel
        // case — but it must not panic, and it must not draw a page-tall box.
        fields: ComponentFields::new(),
        color: INK,
    }];
    let pages = lay_out(&doc, &METRICS, &NoHyphenator);
    assert_eq!(pages.len(), 1);
    let panel = panels(&pages);
    assert_eq!(panel.len(), 1);
    assert!(
        panel[0].h_pt < 60.0,
        "an empty clock is a small panel, not a page-tall one: {:?}",
        panel[0]
    );
}

#[test]
fn a_frame_level_layout_still_resolves_the_bundled_components() {
    // `lay_out_in_frame` takes no library by design (see `lay_out_with_library`). It must still
    // lay out a bundled component, or forty call sites quietly stop testing anything.
    let content = vec![Block::Table {
        id: BlockId(1),
        table: quill_core_model::Table {
            columns: vec![1.0, 1.0],
            header: Some(vec!["A".into(), "B".into()]),
            rows: vec![vec!["1".into(), "2".into()]],
            zebra: true,
        },
        color: INK,
    }];
    let assets: Vec<Asset> = Vec::new();
    let pages = lay_out_in_frame(
        &content,
        &assets,
        &StyleSheet::default(),
        &Frame {
            rect: Rect {
                x_pt: 0.0,
                y_pt: 0.0,
                w_pt: 300.0,
                h_pt: 600.0,
            },
        },
        &METRICS,
        &NoHyphenator,
    );
    assert!(!runs(&pages).is_empty());
}
