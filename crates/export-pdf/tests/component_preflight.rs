//! Spec 0054's cross-cutting rule: **a pack may not make output less press-correct.**
//!
//! Spec 0050's placed-geometry preflight runs over a *declared* component's decoration exactly as
//! it runs over a bundled one. That is the whole basis of `docs/roadmap.md`'s decision not to build
//! executable extensions — a declarative component supplies inputs, quill produces the geometry,
//! and the press checks stay authoritative. Asserted rather than assumed, because "the existing
//! check covers it" is exactly the kind of claim that stops being true without anyone noticing.

use quill_core_model::{
    Block, BlockId, Color, ComponentDef, ComponentFields, DefColor, DefStroke, Document,
    FieldValue, PanelDef, SectionDef, SectionShape, SplitDef,
};
use quill_export_pdf::{preflight_pages, CheckId, PodPreset, Severity};
use quill_layout_engine::lay_out;
use quill_text_layout::{MonospaceRunMetrics, NoHyphenator};

const METRICS: MonospaceRunMetrics = MonospaceRunMetrics { em_ratio: 0.6 };

/// A component whose panel is tinted however the caller says.
fn doc_with_panel(fill: DefColor, stroke: Option<DefStroke>) -> Document {
    let mut doc = Document::sample();
    doc.content = vec![Block::Component {
        id: BlockId(1),
        def: "notice".into(),
        fields: {
            let mut f = ComponentFields::new();
            f.insert("title".into(), FieldValue::Text("A Warning".into()));
            f
        },
        color: Color::Gray { v: 0.0 },
    }];
    doc.components.insert(
        "notice".into(),
        ComponentDef {
            version: quill_core_model::COMPONENT_DEF_VERSION,
            name: "notice".into(),
            panel: PanelDef {
                fill: Some(fill),
                stroke,
                padding_pt: 6.0,
            },
            sections: vec![SectionDef {
                source: "title".into(),
                style: "body".into(),
                shape: SectionShape::Text,
                rule_above: None,
                rule_below: None,
                repeat: false,
            }],
            split: SplitDef::default(),
        },
    );
    doc.assign_missing_block_ids().expect("ids");
    doc
}

fn findings(doc: &Document) -> Vec<quill_export_pdf::Finding> {
    let preset = PodPreset::generic();
    let pages = lay_out(doc, &METRICS, &NoHyphenator);
    preflight_pages(&pages, &preset, &doc.page_setup, &doc.assets)
}

#[test]
fn a_declared_panel_over_the_ink_limit_is_an_error() {
    // 100 + 100 + 100 + 60 = 360% total ink, well past the generic preset's 240.
    let doc = doc_with_panel(
        DefColor::Cmyk {
            c: 1.0,
            m: 1.0,
            y: 1.0,
            k: 0.6,
        },
        None,
    );
    let over = findings(&doc);
    assert!(
        over.iter()
            .any(|f| f.check == CheckId::InkCoverage && f.severity == Severity::Error),
        "a packed component's panel must be ink-checked exactly as a bundled one is: {over:?}"
    );
}

#[test]
fn a_declared_stroke_over_the_ink_limit_is_an_error() {
    let doc = doc_with_panel(
        DefColor::Gray { v: 0.9 },
        Some(DefStroke {
            color: DefColor::Cmyk {
                c: 0.9,
                m: 0.9,
                y: 0.9,
                k: 0.9,
            },
            width_pt: 1.0,
        }),
    );
    let over = findings(&doc);
    assert!(
        over.iter()
            .any(|f| f.check == CheckId::InkCoverage && f.severity == Severity::Error),
        "the stroke is decoration too: {over:?}"
    );
}

#[test]
fn a_declared_panel_within_the_limit_is_clean() {
    let doc = doc_with_panel(DefColor::Gray { v: 0.92 }, None);
    let clean = findings(&doc);
    assert!(
        !clean
            .iter()
            .any(|f| f.check == CheckId::InkCoverage || f.check == CheckId::ColorSpace),
        "a conservatively tinted panel must not be flagged: {clean:?}"
    );
}
