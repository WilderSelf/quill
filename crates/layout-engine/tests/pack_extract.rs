//! Spec 0057: a finished book becomes a reusable pack, and the pack reproduces the look on a
//! *different* book.
//!
//! In `layout-engine` for the same reason `pack_resolution.rs` is: the claim is about geometry.
//! A pack whose fields merged but whose pages did not change has not carried a look anywhere.

use std::fs;
use std::path::PathBuf;

use quill_core_model::{
    install, Asset, Block, BlockId, Color, Document, Indent, MasterPage, MasterStatic,
    PackManifest, PackRequirement, ParagraphStyle, Qpack, Rect, TextAlign, BODY_STYLE,
    STATBLOCK_COMPONENT,
};
use quill_layout_engine::{lay_out, LaidOutPage, PlacedBlock};
use quill_text_layout::{MonospaceRunMetrics, NoHyphenator};

const METRICS: MonospaceRunMetrics = MonospaceRunMetrics { em_ratio: 0.6 };
const INK: Color = Color::Gray { v: 0.0 };

const PROSE: &str = "The vault door had not opened in three hundred years, and the dust on its \
                     hinges had gone to stone.";

/// Deliberately *not* the pack's own title. The first version of this test used the same words for
/// both, so the assertion below passed on the pack's `title` field and proved nothing.
const HEADING: &str = "Chapter One: The Long Dark";

fn tmp_dir(tag: &str) -> PathBuf {
    let dir = std::env::temp_dir().join(format!("quill-extract-{tag}-{}", std::process::id()));
    let _ = fs::remove_dir_all(&dir);
    fs::create_dir_all(&dir).expect("temp dir");
    dir
}

/// A book with a house style: a distinctive body face, a restyled stat block, a master carrying an
/// ornament, and some prose that must *not* travel.
fn house_style_book() -> Document {
    let mut doc = Document::sample();
    doc.styles.paragraph.insert(
        BODY_STYLE.into(),
        ParagraphStyle {
            font_size_pt: 10.5,
            leading_pt: 15.5,
            align: TextAlign::Left,
            space_before_pt: 0.0,
            space_after_pt: 4.0,
            indent: Indent::ZERO,
        },
    );
    let mut sb = quill_core_model::statblock_definition();
    sb.panel.padding_pt = 14.0;
    doc.components.insert(STATBLOCK_COMPONENT.into(), sb);

    doc.assets = vec![
        Asset {
            id: "ornament".into(),
            path: "assets/ornament.png".into(),
            px_w: 300,
            px_h: 60,
            dpi: 300.0,
            line_art: true,
            has_alpha: false,
        },
        Asset {
            id: "cover-art".into(),
            path: "assets/cover.png".into(),
            px_w: 1800,
            px_h: 2400,
            dpi: 300.0,
            line_art: false,
            has_alpha: false,
        },
    ];
    doc.master_pages = vec![MasterPage {
        name: "body".into(),
        margins: Default::default(),
        columns: 1,
        gutter_pt: 0.0,
        statics: vec![MasterStatic::Image {
            asset: "ornament".into(),
            rect: Rect {
                x_pt: 36.0,
                y_pt: 600.0,
                w_pt: 100.0,
                h_pt: 20.0,
            },
        }],
    }];
    doc.default_master = Some("body".into());
    doc.content = vec![
        Block::heading(1, HEADING, INK),
        Block::body(PROSE, INK),
        // Flow art: content, and must not be extracted.
        Block::image("cover-art"),
    ];
    doc.assign_missing_block_ids().expect("ids");
    doc
}

fn extracted() -> PackManifest {
    PackManifest::extract_from(
        &house_style_book(),
        "ashen-vault",
        "The Ashen Vault",
        "1.0.0",
        "https://example.invalid/ashen-vault",
        "CC-BY-4.0",
    )
}

/// The styling a page exposes: the y-step between consecutive body baselines, and the stat-block
/// panel inset. Both are direct reads of the *look* rather than of the content.
fn leading_and_inset(pages: &[LaidOutPage]) -> (f32, f32) {
    let mut ys: Vec<f32> = Vec::new();
    let mut panel_x = None;
    let mut first_run_x = None;
    for b in pages.iter().flat_map(|p| &p.blocks) {
        match b {
            PlacedBlock::Text { frame, lines, .. } if lines.len() > 1 => ys.push(frame.h_pt),
            PlacedBlock::Rect {
                frame,
                stroke: Some(_),
                ..
            } => panel_x = Some(frame.x_pt),
            _ => {}
        }
        if let PlacedBlock::Text { frame, .. } = b {
            if panel_x.is_some() && first_run_x.is_none() {
                first_run_x = Some(frame.x_pt);
            }
        }
    }
    let inset = match (panel_x, first_run_x) {
        (Some(p), Some(r)) => r - p,
        _ => f32::NAN,
    };
    (ys.first().copied().unwrap_or(f32::NAN), inset)
}

/// A *different* book — different words, different creature — set from nothing but the pack.
fn second_book(pack: &PackManifest) -> Document {
    let mut doc = Document::from_template(&pack.templates[0]);
    doc.components = pack.components.clone();
    doc.content = vec![
        Block::body(
            "A wholly different passage, about a wholly different place, long enough that it \
             wraps to more than one line under any reasonable measure.",
            INK,
        ),
        Block::StatBlock {
            id: BlockId::UNASSIGNED,
            stat: quill_core_model::StatBlock {
                name: "Rust Wight".into(),
                overview: vec!["Medium undead, lawful evil.".into()],
                attributes: vec![("Armour Class".into(), "13".into())],
                ..Default::default()
            },
            color: INK,
        },
    ];
    doc.assign_missing_block_ids().expect("ids");
    doc
}

/// The same second book, set with no pack at all — the control.
fn second_book_bare() -> Document {
    let mut doc = Document::sample();
    let pack = extracted();
    doc.content = second_book(&pack).content;
    doc.assign_missing_block_ids().expect("ids");
    doc
}

#[test]
fn an_extracted_pack_reproduces_the_look_on_a_different_document() {
    let pack = extracted();
    let (leading, inset) =
        leading_and_inset(&lay_out(&second_book(&pack), &METRICS, &NoHyphenator));
    let (bare_leading, bare_inset) =
        leading_and_inset(&lay_out(&second_book_bare(), &METRICS, &NoHyphenator));

    assert!(
        leading != bare_leading,
        "the pack's body leading must differ from the default, or this test proves nothing"
    );
    assert!(
        (inset - 14.0).abs() < 0.01,
        "the pack's stat-block inset must govern the second book, got {inset} (bare: {bare_inset})"
    );

    // And it matches what the *first* book gets, which is the actual claim: the look travelled.
    let (first_leading, first_inset) =
        leading_and_inset(&lay_out(&house_style_book(), &METRICS, &NoHyphenator));
    assert!(
        (leading - first_leading).abs() < 0.01,
        "second book's body leading {leading} does not match the first's {first_leading}"
    );
    assert!(first_inset.is_nan() || (first_inset - inset).abs() < 0.01);
}

#[test]
fn content_is_not_extracted() {
    let json = extracted().to_json().expect("serialize");
    // Asserted over the whole serialized pack rather than over a field list, so a future field
    // that happens to carry text is caught too.
    assert!(
        !json.contains("The vault door"),
        "a pack is a look, not a book: prose must not survive extraction"
    );
    assert!(!json.contains(HEADING), "…and neither must a heading");
    let pack = extracted();
    assert!(
        !json.contains("cover.png"),
        "flow art is content and stays behind: {json}"
    );
    assert!(
        pack.assets.iter().any(|a| a.id == "ornament"),
        "furniture art is design and travels"
    );
    assert_eq!(pack.assets.len(), 1, "and nothing else does");
}

#[test]
fn a_template_extracted_from_a_document_is_the_inverse_of_applying_one() {
    // Without this, `from_template` and `from_document` are two functions that happen to share
    // field names, and the day one grows a field the other does not is the day an extracted pack
    // quietly stops carrying part of the look.
    for original in quill_core_model::Template::bundled() {
        let doc = Document::from_template(original);
        let back = quill_core_model::Template::from_document(
            &doc,
            &original.name,
            &original.title,
            &original.description,
        );
        assert_eq!(
            &back, original,
            "`{}` did not survive the round trip",
            original.name
        );
        assert_eq!(
            Document::from_template(&back),
            doc,
            "`{}` round-trips as a struct but builds a different document",
            original.name
        );
    }
}

#[test]
fn an_extracted_pack_round_trips_and_installs() {
    let dir = tmp_dir("install");
    let file = dir.join("ashen.qpack");
    let pack = extracted();
    // The ornament is the only payload, matching the manifest exactly.
    Qpack::write(&pack, &file, &[("assets/ornament.png", b"stand-in")]).expect("write");
    assert_eq!(Qpack::read_manifest(&file).expect("read"), pack);

    let opened = Qpack::open_into(&file, &dir.join("staging")).expect("open");
    let root = dir.join("packs");
    install(&opened.manifest, &opened.asset_root, &root, false).expect("install");

    let mut doc = second_book(&pack);
    doc.components.clear();
    doc.styles = Default::default();
    doc.requires = vec![PackRequirement::new("ashen-vault", "1.0")];
    let resolved = doc.resolve_packs(&root).expect("resolve");
    doc.apply_packs(&resolved).expect("apply");

    let (_, inset) = leading_and_inset(&lay_out(&doc, &METRICS, &NoHyphenator));
    assert!(
        (inset - 14.0).abs() < 0.01,
        "the extracted pack must govern through the full install→resolve path, got {inset}"
    );
    // The furniture asset landed beside the manifest.
    assert!(root
        .join("ashen-vault")
        .join("1.0.0")
        .join("assets/ornament.png")
        .exists());
    let _ = fs::remove_dir_all(&dir);
}

#[test]
fn extraction_carries_only_the_documents_own_definitions() {
    let pack = extracted();
    assert_eq!(
        pack.components.len(),
        1,
        "the bundled table is not the document's to ship: {:?}",
        pack.components.keys().collect::<Vec<_>>()
    );
    assert!(pack.components.contains_key(STATBLOCK_COMPONENT));
}
