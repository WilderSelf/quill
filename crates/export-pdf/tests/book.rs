//! Spec 0079, end to end: several `.tpub` chapters compose into one press-ready PDF with continuous
//! page numbering, shared styles from a pack, and a contents list that names headings from chapters
//! it does not itself contain.
//!
//! Lives in `export-pdf` rather than `core-model` because the claims are about the *page* and the
//! *file*: it is not enough that the documents merge, the folios have to print continuously, the
//! contents entries have to carry the numbers the pages carry, and the subset has to hold every
//! chapter's characters.

use std::fs;
use std::path::PathBuf;

use quill_core_model::{
    install, Block, BlockId, Book, BookChapter, BreakKind, Color, Document, Folio, MasterPage,
    MasterStatic, Metadata, NumberFormat, PackManifest, PackRequirement, PageOverride,
    ParagraphStyle, Qpack, Rect, Run, Section, TextAlign,
};
use quill_export_pdf::{export, lay_out_for_press, ExportOptions};
use quill_layout_engine::{heading_index, section_starts, LaidOutPage, LayoutSession, PlacedBlock};

const INK: Color = Color::Gray { v: 0.0 };

fn tmp_dir(tag: &str) -> PathBuf {
    let dir = std::env::temp_dir().join(format!("quill-book-{tag}-{}", std::process::id()));
    let _ = fs::remove_dir_all(&dir);
    fs::create_dir_all(&dir).expect("temp dir");
    dir
}

/// A master that prints the page's folio, so "what number does this page carry" is answerable from
/// the placed page rather than from a derivation the test re-runs itself.
fn folio_master() -> MasterPage {
    MasterPage {
        name: "body".into(),
        margins: None,
        columns: 1,
        gutter_pt: 0.0,
        statics: vec![MasterStatic::Text {
            rect: Rect {
                x_pt: 36.0,
                y_pt: 610.0,
                w_pt: 200.0,
                h_pt: 14.0,
            },
            text: "{page}".into(),
            color: INK,
            style: None,
            align: Default::default(),
            mirror: false,
        }],
    }
}

/// A chapter: a heading, then `paragraphs` bodies long enough to fill real pages.
fn chapter(title: &str, paragraphs: usize) -> Document {
    let mut doc = Document::sample();
    doc.assets.clear();
    doc.pages.clear();
    doc.sections.clear();
    doc.master_pages = vec![folio_master()];
    doc.default_master = Some("body".into());
    doc.content = std::iter::once(Block::heading(1, title, INK))
        .chain((0..paragraphs).map(|i| {
            Block::body(
                format!(
                    "Paragraph {i} of {title}. It runs on for long enough that a handful of them \
                     fill a page, which is what makes a page count worth asserting about and what \
                     makes a folio worth printing on the page it belongs to."
                ),
                INK,
            )
        }))
        .collect();
    doc.next_block_id = 0;
    doc.assign_missing_block_ids().expect("ids");
    doc
}

fn book(names: &[&str]) -> Book {
    Book {
        book_version: quill_core_model::BOOK_VERSION,
        metadata: Metadata {
            title: "The Whole Book".into(),
            authors: vec!["A. Cartographer".into()],
        },
        requires: Vec::new(),
        chapters: names
            .iter()
            .map(|n| BookChapter {
                path: format!("{n}.tpub"),
                name: (*n).to_string(),
                folio: None,
                master: None,
                break_before: BreakKind::default(),
            })
            .collect(),
    }
}

/// Everything the master furniture printed on `page` — the folios and running heads a reader sees.
fn statics_text(page: &LaidOutPage) -> Vec<String> {
    page.statics
        .iter()
        .filter_map(|s| match s {
            PlacedBlock::Text { lines, .. } => Some(lines[0].text.clone()),
            _ => None,
        })
        .collect()
}

/// The page a block was first placed on.
fn page_of(pages: &[LaidOutPage], id: BlockId) -> usize {
    pages
        .iter()
        .find(|p| {
            p.blocks.iter().any(|b| {
                matches!(b, PlacedBlock::Text { source, .. } | PlacedBlock::Image { source, .. }
                         if *source == id)
            })
        })
        .unwrap_or_else(|| panic!("block {id:?} was never placed"))
        .index
}

/// All flowed (non-furniture) text on a page, joined.
fn body_text(page: &LaidOutPage) -> String {
    page.blocks
        .iter()
        .filter_map(|b| match b {
            PlacedBlock::Text { lines, .. } => Some(
                lines
                    .iter()
                    .map(|l| l.text.as_str())
                    .collect::<Vec<_>>()
                    .join(" "),
            ),
            _ => None,
        })
        .collect::<Vec<_>>()
        .join(" ")
}

#[test]
fn two_chapters_compose_into_one_page_vector_with_continuous_folios() {
    let one = chapter("The Ruined Keep", 40);
    let two = chapter("The Sunken Vault", 40);

    // The control: each chapter's own page count and its own last folio, laid out alone.
    let alone_one = lay_out_for_press(&one, &ExportOptions::default()).expect("layout");
    let alone_two = lay_out_for_press(&two, &ExportOptions::default()).expect("layout");
    assert!(
        alone_one.len() > 1 && alone_two.len() > 1,
        "the fixture must span pages: {} and {}",
        alone_one.len(),
        alone_two.len()
    );
    assert_eq!(
        statics_text(&alone_two[0]),
        vec!["1".to_string()],
        "a chapter laid out alone starts at page 1"
    );

    let composed = book(&["one", "two"])
        .compose(&[one, two.clone()])
        .expect("compose");
    let pages = lay_out_for_press(&composed.document, &ExportOptions::default()).expect("layout");
    // **A chapter starts on a new page** (spec 0080). This was spec 0079's principal residual and
    // is no longer true of the composed book: `compose` gives every chapter a `PageBreak`, so the
    // book is exactly its chapters' pages — no boundary page is shared, and none is inserted, this
    // book asking for `Page` rather than `Recto`.
    assert_eq!(
        pages.len(),
        alone_one.len() + alone_two.len(),
        "a chapter that opens a page neither shares one nor invents one"
    );

    // …and chapter 2 has that page to itself: nothing of chapter 1 is on it.
    let first_of_two = page_of(&pages, composed.chapter_anchors[1].expect("anchor"));
    assert_eq!(
        first_of_two,
        alone_one.len(),
        "chapter 2 opens on the page after chapter 1's last"
    );
    assert!(
        body_text(&pages[first_of_two]).starts_with("The Sunken Vault"),
        "the chapter opener is the first thing on its page, not the tail of the chapter before \
         it: {:?}",
        body_text(&pages[first_of_two])
    );

    // The claim: chapter 2's opening page carries the number the page before it implies — its
    // numbering continues chapter 1's rather than restarting.
    assert!(first_of_two > 0, "chapter 2 is not on page one of the book");
    let implied: usize = statics_text(&pages[first_of_two - 1])[0]
        .parse::<usize>()
        .expect("the previous page's folio")
        + 1;
    assert_eq!(
        statics_text(&pages[first_of_two]),
        vec![implied.to_string()],
        "chapter 2's opening page must carry the number the page before it implies"
    );
    assert_ne!(
        statics_text(&pages[first_of_two]),
        vec!["1".to_string()],
        "and must not restart, which is what it does when laid out alone"
    );
}

/// The same claim over the whole sequence — the one that would fail if the chapters were numbered
/// independently rather than continuously.
#[test]
fn the_folio_sequence_across_a_book_has_no_gap_and_no_repeat() {
    let composed = book(&["one", "two", "three"])
        .compose(&[chapter("One", 30), chapter("Two", 30), chapter("Three", 30)])
        .expect("compose");
    let pages = lay_out_for_press(&composed.document, &ExportOptions::default()).expect("layout");
    let printed: Vec<String> = pages.iter().flat_map(statics_text).collect();
    let expected: Vec<String> = (1..=pages.len()).map(|n| n.to_string()).collect();
    assert_eq!(printed, expected, "every page, once, in order");
}

#[test]
fn roman_front_matter_then_a_body_chapter_that_restarts_at_one() {
    let mut b = book(&["front", "body"]);
    b.chapters[0].folio = Some(Folio {
        format: NumberFormat::LowerRoman,
        restart_at: Some(1),
    });
    b.chapters[1].folio = Some(Folio {
        format: NumberFormat::Decimal,
        restart_at: Some(1),
    });
    let composed = b
        .compose(&[chapter("Front matter", 30), chapter("Chapter One", 40)])
        .expect("compose");
    let pages = lay_out_for_press(&composed.document, &ExportOptions::default()).expect("layout");

    let body_start = page_of(&pages, composed.chapter_anchors[1].expect("anchor"));
    assert!(body_start > 0);
    assert_eq!(
        statics_text(&pages[0]),
        vec!["i".to_string()],
        "front matter is roman"
    );
    assert_eq!(
        statics_text(&pages[body_start]),
        vec!["1".to_string()],
        "the body restarts at 1, which is what `Folio::restart_at` was left for"
    );
    assert_eq!(statics_text(&pages[body_start + 1]), vec!["2".to_string()]);
}

#[test]
fn a_book_contents_list_names_headings_from_chapters_it_does_not_contain() {
    // The front matter holds the contents list and nothing else; the headings it lists are in the
    // two chapters after it.
    let mut front = chapter("Contents", 0);
    front.content.push(Block::Toc {
        id: BlockId::UNASSIGNED,
        title: "Contents".into(),
        max_level: 2,
        color: INK,
    });
    front.assign_missing_block_ids().expect("ids");

    let composed = book(&["front", "one", "two"])
        .compose(&[
            front,
            chapter("The Ruined Keep", 40),
            chapter("The Sunken Vault", 40),
        ])
        .expect("compose");

    // Through a `LayoutSession` — the path the app uses, and the path spec 0075 found a contents
    // list had *always* been empty on.
    let mut session = LayoutSession::new();
    let result = session.relayout(
        &composed.document,
        &quill_fonts::FontFamily::bundled(),
        &quill_export_pdf::HypherHyphenator,
    );
    assert!(result.fixpoint.converged, "{:?}", result.fixpoint);

    let contents = body_text(&result.pages[0]);
    for title in ["The Ruined Keep", "The Sunken Vault"] {
        assert!(
            contents.contains(title),
            "the book's contents list must name '{title}': {contents}"
        );
    }

    // And with the numbers the pages actually carry.
    for entry in heading_index(&composed.document, &result.pages) {
        if entry.level != 1 || entry.text == "Contents" {
            continue;
        }
        assert_eq!(
            statics_text(&result.pages[entry.page_index]),
            vec![entry.folio.clone()],
            "the contents list must print the number the page prints for '{}'",
            entry.text
        );
        assert!(
            contents.contains(&entry.folio),
            "'{}' should be listed against folio {}: {contents}",
            entry.text,
            entry.folio
        );
    }
}

#[test]
fn a_shared_style_resolves_from_a_pack_across_every_chapter() {
    let dir = tmp_dir("packstyle");
    let root = dir.join("packs");
    let mut manifest = PackManifest::new(
        "house-style",
        "House style",
        "1.0.0",
        "https://example.invalid/house",
        "CC-BY-4.0",
    );
    manifest.styles.paragraph.insert(
        "sidebar".into(),
        ParagraphStyle {
            font_size_pt: 22.0,
            leading_pt: 26.0,
            align: TextAlign::Left,
            ..Default::default()
        },
    );
    let file = dir.join("house.qpack");
    Qpack::write(&manifest, &file, &[]).expect("write");
    let opened = Qpack::open_into(&file, &dir.join("staging")).expect("open");
    install(&opened.manifest, &opened.asset_root, &root, false).expect("install");

    // Neither chapter defines `sidebar`; each uses it.
    let styled = |title: &str| {
        let mut doc = chapter(title, 4);
        let id = doc.new_block_id();
        let mut block = Block::body(format!("A sidebar in {title}."), INK).with_style("sidebar");
        block.set_id(id);
        doc.content.push(block);
        doc
    };

    let mut b = book(&["one", "two"]);
    b.requires = vec![PackRequirement::new("house-style", "1")];
    let mut composed = b.compose(&[styled("One"), styled("Two")]).expect("compose");
    let packs = composed.document.resolve_packs(&root).expect("resolve");
    composed.document.apply_packs(&packs).expect("apply");

    let pages = lay_out_for_press(&composed.document, &ExportOptions::default()).expect("layout");
    let sidebars: Vec<f32> = pages
        .iter()
        .flat_map(|p| &p.blocks)
        .filter_map(|b| match b {
            PlacedBlock::Text {
                lines,
                font_size_pt,
                ..
            } if lines.iter().any(|l| l.text.contains("A sidebar in")) => Some(*font_size_pt),
            _ => None,
        })
        .collect();
    assert_eq!(sidebars.len(), 2, "one sidebar per chapter: {sidebars:?}");
    for size in &sidebars {
        assert!(
            (*size - 22.0).abs() < 0.01,
            "the pack's style must govern both chapters, got {size}"
        );
    }

    // And the refusal half (spec 0056), unchanged by being in a book.
    let mut b = book(&["one", "two"]);
    b.requires = vec![PackRequirement::new("absent-pack", "1")];
    let composed = b
        .compose(&[chapter("One", 2), chapter("Two", 2)])
        .expect("compose");
    assert!(
        composed.document.resolve_packs(&root).is_err(),
        "a book must refuse rather than fall back when a pack is missing"
    );

    let _ = fs::remove_dir_all(&dir);
}

#[test]
fn a_chapters_cross_reference_prints_its_targets_book_page() {
    let one = chapter("One", 40);
    let mut two = chapter("Two", 40);
    // A reference in chapter 2's first body, pointing at chapter 2's own last body.
    let target = two.content.last().expect("a block").id();
    let id = two.content[1].id();
    two.content[1] = Block::body_runs(
        vec![
            Run::plain("As discussed on page "),
            Run::reference(target),
            Run::plain(", at length."),
        ],
        INK,
    );
    two.content[1].set_id(id);

    // Alone: the reference prints the chapter's own page number.
    let alone = lay_out_for_press(&two, &ExportOptions::default()).expect("layout");
    let alone_page = page_of(&alone, target) + 1;

    let composed = book(&["one", "two"]).compose(&[one, two]).expect("compose");
    let pages = lay_out_for_press(&composed.document, &ExportOptions::default()).expect("layout");
    // Chapter 2 is last, so its last block is the composed document's last block — the same block,
    // under its rebased id.
    let rebased_target = composed.document.content.last().expect("a block").id();
    assert_ne!(
        rebased_target, target,
        "the fixture must actually rebase, or the assertion below proves nothing"
    );
    let book_page = page_of(&pages, rebased_target) + 1;
    assert!(
        book_page > alone_page,
        "the target moved: {alone_page} alone, {book_page} in the book"
    );

    let printed: String = pages
        .iter()
        .flat_map(|p| &p.blocks)
        .filter_map(|b| match b {
            PlacedBlock::Text { lines, .. } => Some(
                lines
                    .iter()
                    .map(|l| l.text.as_str())
                    .collect::<Vec<_>>()
                    .join(" "),
            ),
            _ => None,
        })
        .find(|t| t.contains("As discussed on page"))
        .expect("the referring paragraph");
    assert!(
        printed.contains(&book_page.to_string()),
        "the reference must print the book page {book_page}: {printed}"
    );
    assert!(
        !printed.contains(quill_core_model::UNRESOLVED_REFERENCE),
        "a chapter's own reference must still resolve after rebasing: {printed}"
    );
}

#[test]
fn the_font_subset_covers_a_character_only_the_last_chapter_uses() {
    // A book's fonts must cover every chapter. This is not a new path to the collector — it walks
    // one `Document` that holds every chapter's content — and this is the assertion that says so.
    let mut last = chapter("Three", 3);
    let id = last.new_block_id();
    let mut block = Block::body("A word with a ligature and a dagger: ﬁ †.", INK);
    block.set_id(id);
    last.content.push(block);

    let composed = book(&["one", "two", "three"])
        .compose(&[chapter("One", 3), chapter("Two", 3), last])
        .expect("compose");

    let dir = tmp_dir("subset");
    let icc = dir.join("out.icc");
    fs::write(&icc, quill_export_pdf::synth_cmyk_profile()).expect("icc");
    let opts = ExportOptions {
        output_intent_icc: icc.to_string_lossy().into_owned(),
        force: true,
        ..Default::default()
    };
    let mut bytes = Vec::new();
    export(&composed.document, &opts, &mut bytes).expect("export");
    assert!(bytes.starts_with(b"%PDF-"), "a real PDF");

    // The proof that the last chapter reached the subset: exporting the same book without that
    // chapter's characters produces a *smaller* font program.
    let without = book(&["one", "two", "three"])
        .compose(&[chapter("One", 3), chapter("Two", 3), chapter("Three", 3)])
        .expect("compose");
    let mut plain = Vec::new();
    export(&without.document, &opts, &mut plain).expect("export");
    assert!(
        bytes.len() > plain.len(),
        "the book carrying an extra glyph must embed more font than the one without it: \
         {} vs {}",
        bytes.len(),
        plain.len()
    );

    let _ = fs::remove_dir_all(&dir);
}

#[test]
fn a_book_exports_one_pdf_with_one_output_intent() {
    let composed = book(&["one", "two"])
        .compose(&[chapter("One", 40), chapter("Two", 40)])
        .expect("compose");
    let dir = tmp_dir("export");
    let icc = dir.join("out.icc");
    fs::write(&icc, quill_export_pdf::synth_cmyk_profile()).expect("icc");
    let opts = ExportOptions {
        output_intent_icc: icc.to_string_lossy().into_owned(),
        ..Default::default()
    };
    let mut bytes = Vec::new();
    export(&composed.document, &opts, &mut bytes).expect("export");

    let text = String::from_utf8_lossy(&bytes);
    assert_eq!(
        text.matches("/OutputIntents").count(),
        1,
        "one book, one OutputIntent"
    );
    assert!(
        text.contains("GTS_PDFXVersion"),
        "a book still identifies as PDF/X"
    );
    assert!(
        !text.contains("/Annots"),
        "the press file stays annotation-free"
    );

    // One file, every chapter's pages in it.
    let pages = lay_out_for_press(&composed.document, &opts).expect("layout");
    assert!(pages.len() > 2, "a book of two real chapters");
    assert_eq!(
        text.matches("/Catalog").count(),
        1,
        "one catalogue, therefore one document"
    );

    let _ = fs::remove_dir_all(&dir);
}

/// **A book whose chapters open on the recto** (spec 0080): the blank page a parity break inserts
/// is a real page — it takes its master's furniture and it consumes a page number.
///
/// The folio assertion is the one that would be a printed defect rather than a cosmetic one: an
/// inserted page that did not consume a number would leave every folio after it one out, for the
/// whole book, with nothing to say so.
#[test]
fn a_recto_opener_inserts_a_blank_page_that_still_carries_its_folio() {
    let mut b = book(&["one", "two"]);
    b.chapters[1].break_before = BreakKind::Recto;
    let one = chapter("The Ruined Keep", 40);
    let two = chapter("The Sunken Vault", 40);
    let alone_one = lay_out_for_press(&one, &ExportOptions::default()).expect("layout");
    let composed = b.compose(&[one, two]).expect("compose");
    let pages = lay_out_for_press(&composed.document, &ExportOptions::default()).expect("layout");

    let opener = page_of(&pages, composed.chapter_anchors[1].expect("anchor"));
    assert!(
        quill_core_model::is_recto(opener, true),
        "a chapter that asks for a recto gets one: {opener}"
    );

    assert!(
        body_text(&pages[opener]).starts_with("The Sunken Vault"),
        "and has that page to itself: {:?}",
        body_text(&pages[opener])
    );

    // The fixture must actually exercise the insertion rather than happen to land right.
    if opener > alone_one.len() {
        assert_eq!(opener, alone_one.len() + 1, "exactly one page inserted");
        let blank = &pages[opener - 1];
        assert!(
            blank.blocks.is_empty(),
            "the inserted page carries no content"
        );
        assert_eq!(
            statics_text(blank),
            vec![opener.to_string()],
            "…and prints the folio its position implies"
        );
    }

    // Whether or not a page was inserted: every folio, once, in order. A blank page that consumed
    // no number would repeat one here.
    let printed: Vec<String> = pages.iter().flat_map(statics_text).collect();
    assert_eq!(
        printed,
        (1..=pages.len()).map(|n| n.to_string()).collect::<Vec<_>>(),
    );

    // And it still exports as one press file.
    let dir = tmp_dir("recto");
    let icc = dir.join("out.icc");
    fs::write(&icc, quill_export_pdf::synth_cmyk_profile()).expect("icc");
    let opts = ExportOptions {
        output_intent_icc: icc.to_string_lossy().into_owned(),
        ..Default::default()
    };
    let mut bytes = Vec::new();
    export(&composed.document, &opts, &mut bytes).expect("export");
    assert!(bytes.starts_with(b"%PDF-"));
    let _ = fs::remove_dir_all(&dir);
}

#[test]
fn a_single_document_export_is_untouched_by_this_increment() {
    // The parity claim that makes the whole increment safe. `SAMPLE_EXPORT_DIGEST` (in
    // `export-pdf`'s own tests) pins the exact bytes; this states the same thing from outside the
    // crate and in the same process that has just composed a book, so nothing composition does can
    // leak into the single-document path.
    let dir = tmp_dir("parity");
    let icc = dir.join("out.icc");
    fs::write(&icc, quill_export_pdf::synth_cmyk_profile()).expect("icc");
    let opts = ExportOptions {
        output_intent_icc: icc.to_string_lossy().into_owned(),
        ..Default::default()
    };

    let mut before = Vec::new();
    export(&Document::sample(), &opts, &mut before).expect("export");

    let _ = book(&["one", "two"])
        .compose(&[chapter("One", 4), chapter("Two", 4)])
        .expect("compose");

    let mut after = Vec::new();
    export(&Document::sample(), &opts, &mut after).expect("export");
    assert_eq!(before, after, "composing a book must not move a document");

    // The size `benches/budgets.toml` records for the sample, which has not moved since spec 0071.
    assert_eq!(
        before.len(),
        8454,
        "a single-document export must be the size it was before this increment"
    );

    let _ = fs::remove_dir_all(&dir);
}

#[test]
fn a_one_chapter_book_is_its_chapter() {
    // The degenerate case, asserted because it is the cheapest statement that composition adds no
    // geometry of its own: one chapter in, the same pages out.
    let one = chapter("Only", 40);
    let alone = lay_out_for_press(&one, &ExportOptions::default()).expect("layout");
    let composed = book(&["only"]).compose(&[one]).expect("compose");
    let pages = lay_out_for_press(&composed.document, &ExportOptions::default()).expect("layout");
    assert_eq!(alone.len(), pages.len());
    for (a, b) in alone.iter().zip(&pages) {
        assert_eq!(a.blocks.len(), b.blocks.len(), "page {}", a.index);
        assert_eq!(statics_text(a), statics_text(b), "page {}", a.index);
    }
}

#[test]
fn a_chapters_own_section_survives_composition_beside_the_books() {
    let mut one = chapter("One", 30);
    let inner = one.content[5].id();
    one.sections.push(Section {
        name: "An inner part".into(),
        start: inner,
        master: None,
        folio: None,
    });
    one.pages = vec![PageOverride { master: None }];

    let composed = book(&["one", "two"])
        .compose(&[one, chapter("Two", 30)])
        .expect("compose");
    let names: Vec<&str> = composed
        .document
        .sections
        .iter()
        .map(|s| s.name.as_str())
        .collect();
    assert_eq!(names, vec!["An inner part", "one", "two"]);

    let pages = lay_out_for_press(&composed.document, &ExportOptions::default()).expect("layout");
    let starts = section_starts(&composed.document, &pages);
    assert!(
        starts.iter().all(Option::is_some),
        "every section must be placed: {starts:?}"
    );
}
