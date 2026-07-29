//! Spec 0081: **every colour that reaches the page is checked.**
//!
//! Two colour-bearing sites reached the press file without ever being looked at, and both are the
//! same defect wearing different clothes — a producer of ink that the checker did not know about:
//!
//! - **(A)** a master page's static text. `preflight` walks `doc.content` and never visits a master;
//!   `preflight_pages` walks placed geometry but its colour loop `continue`d on anything that was
//!   not a `PlacedBlock::Rect`, and a `MasterStatic::Text` becomes a `PlacedBlock::Text`. A running
//!   head at CMYK 90/90/90/90 — **360% ink** — passed with zero findings and printed on every page
//!   its master governed.
//! - **(B)** a run's *named character style*. The run half of the model check read `run.style.color`
//!   — the direct override — and skipped a run that had none. The colour that reaches the page is
//!   the **resolved** one, folding the named style.
//!
//! Every test here fails against the pre-0081 checker; the last one is the one that fails against a
//! checker that simply reports everything, which is spec 0050's expensive failure mode.

use quill_core_model::{
    Block, BlockId, CharacterStyle, Color, Document, InlineStyle, MasterPage, MasterStatic, Rect,
    Run,
};
use quill_export_pdf::{
    export, preflight, preflight_pages, CheckId, ExportOptions, PodPreset, Severity,
};
use quill_layout_engine::lay_out;
use quill_text_layout::{MonospaceRunMetrics, NoHyphenator};

const METRICS: MonospaceRunMetrics = MonospaceRunMetrics { em_ratio: 0.6 };

/// 90/90/90/90 = 360% total ink, half again past the generic preset's 240.
const OVER_INKED: Color = Color::Cmyk {
    c: 0.9,
    m: 0.9,
    y: 0.9,
    k: 0.9,
};

const RGB: Color = Color::Rgb {
    r: 1.0,
    g: 0.0,
    b: 0.0,
};

/// A document whose every page carries one running head, in `color`.
fn doc_with_running_head(color: Color) -> Document {
    let mut doc = Document::sample();
    doc.master_pages = vec![MasterPage {
        statics: vec![MasterStatic::text(
            Rect {
                x_pt: 72.0,
                y_pt: 36.0,
                w_pt: 200.0,
                h_pt: 12.0,
            },
            "The Ruined Keep",
            color,
        )],
        ..MasterPage::plain("body")
    }];
    doc.default_master = Some("body".into());
    doc.assign_missing_block_ids().expect("ids");
    doc
}

/// A document with one paragraph whose middle run is set in a named character style carrying
/// `color`, and which overrides nothing itself.
fn doc_with_character_style(color: Color) -> Document {
    let mut doc = Document::sample();
    doc.styles.character.insert(
        "shout".into(),
        CharacterStyle {
            color: Some(color),
            ..CharacterStyle::EMPTY
        },
    );
    doc.content = vec![Block::Body {
        id: BlockId(1),
        runs: vec![
            Run::plain("A plain opening, "),
            Run {
                character: Some("shout".into()),
                // Overrides nothing: the colour comes from the *named* style, which is the whole
                // point. A direct override was already checked.
                style: InlineStyle::EMPTY,
                ..Run::plain("shouted")
            },
            Run::plain(", and a plain close."),
        ],
        style: None,
        color: Color::Gray { v: 0.0 },
    }];
    doc.assign_missing_block_ids().expect("ids");
    doc
}

fn placed_findings(doc: &Document) -> Vec<quill_export_pdf::Finding> {
    let pages = lay_out(doc, &METRICS, &NoHyphenator);
    preflight_pages(&pages, &PodPreset::generic(), &doc.page_setup, &doc.assets)
}

fn has(findings: &[quill_export_pdf::Finding], check: CheckId) -> bool {
    findings
        .iter()
        .any(|f| f.check == check && f.severity == Severity::Error)
}

// ---------------------------------------------------------------- (A) master statics

#[test]
fn an_over_inked_master_static_is_reported_over_placed_geometry() {
    let doc = doc_with_running_head(OVER_INKED);
    let findings = placed_findings(&doc);
    assert!(
        has(&findings, CheckId::InkCoverage),
        "a 360% running head prints on every page its master governs: {findings:?}"
    );
}

#[test]
fn an_rgb_master_static_is_reported_over_placed_geometry() {
    let doc = doc_with_running_head(RGB);
    let findings = placed_findings(&doc);
    assert!(
        has(&findings, CheckId::ColorSpace),
        "the writer turns this silently black; preflight must refuse it first: {findings:?}"
    );
}

#[test]
fn an_over_inked_master_static_is_reported_by_the_model_check_too() {
    // `quill preflight` lays nothing out. A document check that stayed blind here would tell an
    // author "no findings" about a file the exporter is about to refuse — a false pass, which is
    // the shape spec 0052 built `Skipped` to avoid.
    let doc = doc_with_running_head(OVER_INKED);
    let report = preflight(&doc, &ExportOptions::default());
    assert!(
        has(&report.findings, CheckId::InkCoverage),
        "{:?}",
        report.findings
    );
}

#[test]
fn an_rgb_master_static_is_reported_by_the_model_check_too() {
    let doc = doc_with_running_head(RGB);
    let report = preflight(&doc, &ExportOptions::default());
    assert!(
        has(&report.findings, CheckId::ColorSpace),
        "{:?}",
        report.findings
    );
}

#[test]
fn an_over_inked_master_static_is_reported_not_emitted() {
    // The whole claim, end to end and through the real export path: the bytes are never written.
    let icc = std::env::temp_dir().join(format!("quill_0081_{}.icc", std::process::id()));
    std::fs::write(&icc, quill_export_pdf::synth_cmyk_profile()).expect("write icc");
    let opts = ExportOptions {
        output_intent_icc: icc.to_string_lossy().into_owned(),
        ..Default::default()
    };

    let doc = doc_with_running_head(OVER_INKED);
    let mut bytes = Vec::new();
    let err = export(&doc, &opts, &mut bytes).expect_err("a 360% folio must not export");
    assert!(
        matches!(err, quill_export_pdf::ExportError::PreflightFailed(_)),
        "{err:?}"
    );
    assert!(bytes.is_empty(), "nothing may be written for a refused doc");

    // And the same document with a legal running head still exports, so the refusal cannot have
    // been export breaking generally.
    let legal = doc_with_running_head(Color::Gray { v: 0.0 });
    let mut ok = Vec::new();
    export(&legal, &opts, &mut ok).expect("a legal running head exports");
    assert!(ok.starts_with(b"%PDF-1.3"));

    let _ = std::fs::remove_file(&icc);
}

// ---------------------------------------------------------------- (B) character styles

#[test]
fn an_rgb_character_style_is_reported() {
    let doc = doc_with_character_style(RGB);
    assert!(
        has(&placed_findings(&doc), CheckId::ColorSpace),
        "a named style's colour reaches the page exactly as a direct override does"
    );
    let report = preflight(&doc, &ExportOptions::default());
    assert!(
        has(&report.findings, CheckId::ColorSpace),
        "{:?}",
        report.findings
    );
}

#[test]
fn an_over_inked_character_style_is_reported() {
    let doc = doc_with_character_style(OVER_INKED);
    assert!(
        has(&placed_findings(&doc), CheckId::InkCoverage),
        "360% of ink is 360% of ink whether the run names it or a style does"
    );
    let report = preflight(&doc, &ExportOptions::default());
    assert!(
        has(&report.findings, CheckId::InkCoverage),
        "{:?}",
        report.findings
    );
}

// ---------------------------------------------------------------- the enumeration itself

#[test]
fn every_placed_variant_states_the_ink_it_draws() {
    use quill_layout_engine::{InkSite, PlacedBlock, Stroke};

    let r = Rect {
        x_pt: 0.0,
        y_pt: 0.0,
        w_pt: 10.0,
        h_pt: 10.0,
    };
    let a = Color::Gray { v: 0.1 };
    let b = Color::Gray { v: 0.2 };
    let c = Color::Gray { v: 0.3 };

    // A text block reports its own ink *and* every run's: a span whose run has no entry falls back
    // to the block's, so both are drawn and both must be checked.
    let text = PlacedBlock::Text {
        frame: r,
        source: BlockId(1),
        lines: Vec::new(),
        color: a,
        run_colors: vec![b, c],
        run_formats: Vec::new(),
        run_shifts: Vec::new(),
        weight: 400,
        italic: false,
        font_size_pt: 10.0,
        leading_pt: 12.0,
    };
    let sites: Vec<_> = text.inks().iter().map(|i| (i.site, i.color)).collect();
    assert_eq!(
        sites,
        vec![
            (InkSite::Text, a),
            (InkSite::Run(0), b),
            (InkSite::Run(1), c)
        ]
    );

    let rect = PlacedBlock::Rect {
        frame: r,
        fill: Some(a),
        stroke: Some(Stroke {
            color: b,
            width_pt: 1.0,
        }),
    };
    let sites: Vec<_> = rect.inks().iter().map(|i| (i.site, i.color)).collect();
    assert_eq!(sites, vec![(InkSite::Fill, a), (InkSite::Stroke, b)]);

    // The two that draw no authored colour. Asserted rather than assumed: "it has no colour" is the
    // claim the pre-0081 loop made about `Text` as well, by `continue`-ing on it.
    assert!(PlacedBlock::Image {
        frame: r,
        source: BlockId(1),
        asset_id: "art".into(),
    }
    .inks()
    .is_empty());
    assert!(PlacedBlock::Link {
        frame: r,
        source: BlockId(1),
        target_page: 0,
    }
    .inks()
    .is_empty());

    // A rectangle with neither draws nothing.
    assert!(PlacedBlock::Rect {
        frame: r,
        fill: None,
        stroke: None,
    }
    .inks()
    .is_empty());
}

// ---------------------------------------------------------------- the false-positive case

#[test]
fn a_document_whose_colours_are_all_legal_produces_no_colour_findings() {
    // Spec 0050's expensive failure mode. Without this the tests above would pass against a checker
    // that reports every colour it sees, and an author who is shown a finding for legal ink learns
    // to ignore the panel — which costs more than the check is worth.
    let legal = [
        Color::Gray { v: 0.0 },
        Color::Gray { v: 0.85 },
        Color::Cmyk {
            c: 0.6,
            m: 0.4,
            y: 0.3,
            k: 0.2,
        }, // 150%
    ];
    for color in legal {
        for doc in [
            doc_with_running_head(color),
            doc_with_character_style(color),
        ] {
            let placed = placed_findings(&doc);
            assert!(
                !placed
                    .iter()
                    .any(|f| f.check == CheckId::InkCoverage || f.check == CheckId::ColorSpace),
                "legal ink must not be flagged over geometry ({color:?}): {placed:?}"
            );
            let report = preflight(&doc, &ExportOptions::default());
            assert!(
                !report
                    .findings
                    .iter()
                    .any(|f| f.check == CheckId::InkCoverage || f.check == CheckId::ColorSpace),
                "legal ink must not be flagged over the model ({color:?}): {:?}",
                report.findings
            );
        }
    }

    // And the shipped sample, which is what every other golden in the workspace rests on.
    let sample = Document::sample();
    let placed = placed_findings(&sample);
    assert!(
        !placed
            .iter()
            .any(|f| f.check == CheckId::InkCoverage || f.check == CheckId::ColorSpace),
        "the sample's colours are legal: {placed:?}"
    );
}
