//! Spec 0056, end to end: a pack is written, installed, resolved by a document that names it, and
//! the document then lays out *through the pack's definitions* rather than the bundled ones.
//!
//! Lives in `layout-engine` rather than `core-model` because the claim being tested is about
//! geometry: it is not enough that the definitions merge, the placed page has to change.

use std::fs;
use std::path::{Path, PathBuf};

use quill_core_model::{
    install, resolve, statblock_definition, Block, BlockId, Color, Document, PackManifest,
    PackRequirement, Qpack, StatBlock, STATBLOCK_COMPONENT,
};
use quill_layout_engine::{lay_out, LaidOutPage, PlacedBlock};
use quill_text_layout::{MonospaceRunMetrics, NoHyphenator};

const METRICS: MonospaceRunMetrics = MonospaceRunMetrics { em_ratio: 0.6 };
const INK: Color = Color::Gray { v: 0.0 };

fn tmp_dir(tag: &str) -> PathBuf {
    let dir = std::env::temp_dir().join(format!("quill-packres-{tag}-{}", std::process::id()));
    let _ = fs::remove_dir_all(&dir);
    fs::create_dir_all(&dir).expect("temp dir");
    dir
}

/// A pack that restyles the bundled stat block: the same name, a much larger panel inset.
///
/// Restyling `statblock` rather than adding a new name is deliberate — it is the case a publisher
/// actually wants (make *my* books' stat blocks look like this), and it is the case where a silent
/// precedence bug is invisible.
fn roomy_pack(padding_pt: f32) -> PackManifest {
    let mut m = PackManifest::new(
        "roomy",
        "Roomy stat blocks",
        "1.4.0",
        "https://example.invalid/roomy",
        "CC-BY-4.0",
    );
    let mut def = statblock_definition();
    def.panel.padding_pt = padding_pt;
    m.components.insert(STATBLOCK_COMPONENT.into(), def);
    m
}

fn doc_with_statblock() -> Document {
    let mut doc = Document::sample();
    doc.content = vec![Block::StatBlock {
        id: BlockId(1),
        stat: StatBlock {
            name: "Cave Troll".into(),
            overview: vec!["Large giant, chaotic evil.".into()],
            attributes: vec![("Armour Class".into(), "15".into())],
            ..Default::default()
        },
        color: INK,
    }];
    doc.assign_missing_block_ids().expect("ids");
    doc
}

/// The horizontal inset between the panel's left edge and the first run inside it — which is
/// exactly the declared padding, and so a direct read of which definition won.
fn panel_inset(pages: &[LaidOutPage]) -> f32 {
    let mut panel_x = None;
    let mut run_x = None;
    for b in pages.iter().flat_map(|p| &p.blocks) {
        match b {
            PlacedBlock::Rect {
                frame,
                stroke: Some(_),
                ..
            } => panel_x = Some(frame.x_pt),
            PlacedBlock::Text { frame, .. } if run_x.is_none() => run_x = Some(frame.x_pt),
            _ => {}
        }
    }
    run_x.expect("a run") - panel_x.expect("a panel")
}

#[test]
fn a_document_lays_out_through_the_pack_it_requires() {
    let dir = tmp_dir("endtoend");
    let file = dir.join("roomy.qpack");
    let root = dir.join("packs");
    Qpack::write(&roomy_pack(20.0), &file, &[]).expect("write");
    let opened = Qpack::open_into(&file, &dir.join("staging")).expect("open");
    install(&opened.manifest, &opened.asset_root, &root, false).expect("install");

    // Bundled behaviour first, as the control.
    let bundled = panel_inset(&lay_out(&doc_with_statblock(), &METRICS, &NoHyphenator));
    assert!(
        (bundled - 6.0).abs() < 0.01,
        "the bundled stat block insets by 6 pt, got {bundled}"
    );

    let mut doc = doc_with_statblock();
    doc.requires = vec![PackRequirement::new("roomy", "1.4")];
    let packs = doc.resolve_packs(&root).expect("resolve");
    doc.apply_packs(&packs).expect("apply");
    assert!(
        doc.requires.is_empty(),
        "a satisfied requirement is cleared, which is what makes `lay_out`'s guard meaningful"
    );

    let packed = panel_inset(&lay_out(&doc, &METRICS, &NoHyphenator));
    assert!(
        (packed - 20.0).abs() < 0.01,
        "the pack's definition must govern the geometry, got {packed}"
    );
    let _ = fs::remove_dir_all(&dir);
}

#[test]
fn a_document_definition_still_beats_the_packs() {
    let dir = tmp_dir("precedence");
    let root = dir.join("packs");
    install(&roomy_pack(20.0), Path::new("."), &root, false).expect("install");

    let mut doc = doc_with_statblock();
    doc.requires = vec![PackRequirement::new("roomy", "")];
    let mut mine = statblock_definition();
    mine.panel.padding_pt = 30.0;
    doc.components.insert(STATBLOCK_COMPONENT.into(), mine);

    let packs = doc.resolve_packs(&root).expect("resolve");
    doc.apply_packs(&packs).expect("apply");
    let inset = panel_inset(&lay_out(&doc, &METRICS, &NoHyphenator));
    assert!(
        (inset - 30.0).abs() < 0.01,
        "bundled < packs < the document's own: got {inset}"
    );
    let _ = fs::remove_dir_all(&dir);
}

#[test]
fn a_missing_pack_refuses_rather_than_falling_back() {
    let dir = tmp_dir("refusal");
    let root = dir.join("packs");
    fs::create_dir_all(&root).unwrap();

    let mut doc = doc_with_statblock();
    doc.requires = vec![PackRequirement::new("roomy", "1.4")];
    let err = doc
        .resolve_packs(&root)
        .expect_err("a document naming an uninstalled pack must not resolve");
    let text = err.to_string();
    assert!(
        text.contains("roomy"),
        "the error must name the pack: {text}"
    );
    assert!(text.contains("1.4"), "and the version it asked for: {text}");
    let _ = fs::remove_dir_all(&dir);
}

#[test]
#[should_panic(expected = "requires content packs that have not been resolved")]
fn laying_out_an_unresolved_document_is_refused_rather_than_defaulted() {
    // The library-level half of the same rule. A caller who skips resolution would otherwise get a
    // book set in the default face, which is exactly the failure mode spec 0056 exists to prevent —
    // and it would look plausible right up to the print shop.
    let mut doc = doc_with_statblock();
    doc.requires = vec![PackRequirement::new("roomy", "1.4")];
    let _ = lay_out(&doc, &METRICS, &NoHyphenator);
}

#[test]
fn a_documents_requirements_survive_a_manifest_round_trip() {
    let mut doc = doc_with_statblock();
    doc.requires = vec![
        PackRequirement::new("roomy", "1.4"),
        PackRequirement::new("ashen-vault", ""),
    ];
    let json = doc.to_json().expect("serialize");
    let back = Document::from_json(&json).expect("load");
    assert_eq!(back.requires, doc.requires);

    // And a document with none writes none, which is what keeps spec 0056 additive.
    let plain = doc_with_statblock().to_json().expect("serialize");
    assert!(!plain.contains("\"requires\""));
}

#[test]
fn resolution_picks_the_highest_matching_version_installed() {
    let dir = tmp_dir("versions");
    let root = dir.join("packs");
    for (v, pad) in [("1.4.0", 20.0), ("1.10.0", 24.0), ("2.0.0", 40.0)] {
        let mut m = roomy_pack(pad);
        m.version = v.into();
        install(&m, Path::new("."), &root, false).expect("install");
    }
    let got = resolve(&[PackRequirement::new("roomy", "1")], &root).expect("resolve");
    assert_eq!(
        got.packs[0].manifest.version, "1.10.0",
        "1.10.0 is newer than 1.4.0 — a lexical comparison would disagree"
    );
    let _ = fs::remove_dir_all(&dir);
}
