//! Assembles the PDF object graph (writer §1-4) for the profile and level selected in
//! [`ExportOptions`] — PDF/X-1a:2001 or PDF/X-3:2002 under [`ExportProfile::Press`], or a
//! non-conformant linked file under [`ExportProfile::Screen`] (spec 0052).
//!
//! Consumes laid-out pages plus the document and writes bytes: catalog + pages tree, one page per
//! `LaidOutPage` (`MediaBox == BleedBox`, centered `TrimBox`), an embedded subset font, image
//! XObjects (grayscale `/DeviceGray` or color `/DeviceCMYK`), the ICC `OutputIntent`, and the XMP
//! identification packet — plus, under the screen profile, one `/Link` annotation per contents
//! entry. Both X-1a:2001 and
//! X-3:2002 pin the header to **PDF 1.3**; the only per-level difference the trivial layout
//! reaches is the `GTS_PDFX*` identification strings.

use std::collections::{BTreeMap, BTreeSet};
use std::io::Write;
use std::path::Path;

use pdf_writer::types::{
    ActionType, AnnotationType, CidFontType, FontFlags, SystemInfo, TrappingStatus, UnicodeCmap,
};
use pdf_writer::writers::OutputIntent;
use pdf_writer::{Content, Filter, Finish, Name, Pdf, Rect, Ref, Str, TextStr};

use quill_color::RgbToCmyk;
use quill_core_model::{Color, Document};
use quill_layout_engine::{LaidOutPage, PlacedBlock};

use crate::images::Pixels;
use crate::{fonts, geom, icc, images, ExportError, ExportOptions, ExportProfile};

// Body/heading font size and line advance, in points. Shared with the layout engine (spec 0015)
// so glyphs are measured and drawn at the same size and land on the rows layout reserved.
use quill_text_layout::Line;

/// Monotonic indirect-reference allocator.
struct Alloc(i32);
impl Alloc {
    fn bump(&mut self) -> Ref {
        let r = Ref::new(self.0);
        self.0 += 1;
        r
    }
}

/// Write `doc` (already laid out into `pages`) as a PDF/X-1a:2001 file to `out`.
pub fn write_pdf(
    doc: &Document,
    opts: &ExportOptions,
    pages: &[LaidOutPage],
    family: &fonts::EmbeddedFamily,
    out: &mut impl Write,
) -> Result<(), ExportError> {
    // --- Inputs: ICC bytes, images --------------------------------------------------------
    // The font is built once in `export` (it is also the layout engine's metrics source, spec 0015)
    // and passed in.
    //
    // The screen profile writes no OutputIntent and may be given no profile at all (spec 0052). A
    // profile that *is* supplied is still read, validated and used for RGB→CMYK conversion under
    // both profiles — the screen file carries the same ink as the press file; only the
    // OutputIntent dictionary and the PDF/X identification differ, because writing an OutputIntent
    // into a file that is not PDF/X would be claiming a conformance it does not have.
    let want_icc =
        opts.profile == ExportProfile::Press || !opts.output_intent_icc.trim().is_empty();
    let icc_bytes = if want_icc {
        let bytes = std::fs::read(&opts.output_intent_icc)
            .map_err(|e| ExportError::Icc(format!("reading '{}': {e}", opts.output_intent_icc)))?;
        icc::check_icc(&bytes).map_err(ExportError::Icc)?;
        Some(bytes)
    } else {
        None
    };
    // Written only for PDF/X. A screen export with a supplied profile uses it for colour and emits
    // no OutputIntent object.
    let write_output_intent_obj = opts.profile == ExportProfile::Press;

    // Color images are converted to CMYK against the OutputIntent profile (spec 0005). With no
    // profile, `RgbToCmyk` falls back to its naive path — which preflight records as the cost of
    // omitting `--icc`, rather than leaving it to be discovered in the file.
    let cmyk = RgbToCmyk::from_output_profile(icc_bytes.as_deref().unwrap_or_default());

    // Decode each distinct placed image once (missing/unsupported → skipped). The base directory
    // comes from the caller (spec 0025) — resolving against the process working directory silently
    // dropped images whenever the CWD was not the document's own directory.
    let base_dir: &Path = &opts.asset_root;
    let mut images_by_id: BTreeMap<String, ImageObj> = BTreeMap::new();
    for page in pages {
        for block in page.statics.iter().chain(page.blocks.iter()) {
            if let PlacedBlock::Image { asset_id, .. } = block {
                if images_by_id.contains_key(asset_id) {
                    continue;
                }
                if let Some(asset) = doc.assets.iter().find(|a| &a.id == asset_id) {
                    if let Some(img) = images::resolve(asset, base_dir, &cmyk) {
                        let idx = images_by_id.len();
                        images_by_id.insert(
                            asset_id.clone(),
                            ImageObj {
                                name: format!("Im{idx}"),
                                pixels: img.pixels,
                                width: img.width,
                                height: img.height,
                                id: Ref::new(1), // placeholder, assigned below
                            },
                        );
                    }
                }
            }
        }
    }

    // --- Allocate references --------------------------------------------------------------
    let mut alloc = Alloc(1);
    let catalog_id = alloc.bump();
    let page_tree_id = alloc.bump();
    let info_id = alloc.bump();
    let xmp_id = alloc.bump();
    // Allocated only when an OutputIntent is actually written. A reference with no object behind it
    // is legal (the xref gets a free entry) but it is a lie in the file, and the screen profile is
    // the profile that must not look like it half-claims PDF/X.
    let icc_id = write_output_intent_obj.then(|| alloc.bump());
    // One Type0 chain per embedded face (spec 0064), each with its own `/ToUnicode` CMap
    // (spec 0068): Type0, CIDFont, FontDescriptor, FontFile, CMap.
    let font_ids: Vec<[Ref; 5]> = (0..family.len())
        .map(|_| {
            [
                alloc.bump(),
                alloc.bump(),
                alloc.bump(),
                alloc.bump(),
                alloc.bump(),
            ]
        })
        .collect();
    for img in images_by_id.values_mut() {
        img.id = alloc.bump();
    }
    let page_refs: Vec<(Ref, Ref)> = pages.iter().map(|_| (alloc.bump(), alloc.bump())).collect();

    // Link annotations (spec 0052). Allocated *after* every press object, and only for annotations
    // that will actually be written, so a press export — which produces none — allocates exactly
    // the references it always did and its bytes cannot move.
    let mut annots: Vec<Vec<PendingLink>> = Vec::with_capacity(pages.len());
    for (i, page) in pages.iter().enumerate() {
        let mut on_page = Vec::new();
        for block in page.statics.iter().chain(page.blocks.iter()) {
            let PlacedBlock::Link {
                frame, target_page, ..
            } = block
            else {
                continue;
            };
            if !annotation_is_legal(opts.profile, frame, &doc.page_setup, i) {
                continue;
            }
            on_page.push(PendingLink {
                id: alloc.bump(),
                rect: *frame,
                target_page: *target_page,
            });
        }
        annots.push(on_page);
    }

    // Bookmarks (spec 0042). A 500-page PDF with no outline is unusable, and until now the writer
    // emitted a catalog and a page tree and nothing navigational at all.
    let outline = OutlineTree::build(&quill_layout_engine::heading_index(doc, pages), &mut alloc);

    // --- Build the document ---------------------------------------------------------------
    // `Some(level)` for a press export, `None` for a screen one (spec 0052). Read once here so the
    // info dictionary and the XMP packet cannot disagree about what the file claims.
    let pdfx = opts.profile.pdfx(opts.version);

    let mut pdf = Pdf::new();
    pdf.set_version(1, 3); // PDF/X-1a:2001 == PDF 1.3

    // Catalog + OutputIntent + XMP reference.
    {
        let mut cat = pdf.catalog(catalog_id);
        cat.pages(page_tree_id);
        cat.metadata(xmp_id);
        // Only when there is something to show. An empty outline tree renders as an empty,
        // confusing bookmark pane, which is worse than no pane at all.
        if let Some(root) = outline.root {
            cat.outlines(root);
        }
        // PDF/X only. A screen file states no output condition at all rather than one a viewer
        // might read as a press guarantee.
        if let Some(icc_id) = icc_id {
            let mut intents = cat.output_intents();
            write_output_intent(intents.push(), icc_id);
            intents.finish();
        }
        cat.finish();
    }

    // Pages tree.
    {
        let mut tree = pdf.pages(page_tree_id);
        tree.kids(page_refs.iter().map(|(p, _)| *p));
        tree.count(page_refs.len() as i32);
        tree.finish();
    }

    outline.write(&mut pdf, &page_refs);

    // Document info (title/creator/producer + PDF/X identification keys, mirrored in XMP).
    {
        let mut info = pdf.document_info(info_id);
        info.title(TextStr(&doc.metadata.title));
        info.creator(TextStr("Quill"));
        info.producer(TextStr("Quill export-pdf"));
        info.trapped(TrappingStatus::NotTrapped);
        // The identification keys are the *claim* of conformance, so the screen profile omits them
        // entirely rather than writing a weaker string (spec 0052). One source for the decision,
        // shared with the XMP packet below, so the two halves of the identification cannot disagree.
        if let Some(version) = pdfx {
            info.pair(
                Name(b"GTS_PDFXVersion"),
                Str(version.identifier().as_bytes()),
            );
            if let Some(conf) = version.conformance() {
                info.pair(Name(b"GTS_PDFXConformance"), Str(conf.as_bytes()));
            }
        }
        info.finish();
    }

    // XMP metadata packet (uncompressed).
    let id_hex = doc_id_hex(doc);
    {
        let xmp = crate::xmp::build_xmp(pdfx, &doc.metadata.title, &id_hex, &id_hex);
        let mut s = pdf.stream(xmp_id, &xmp);
        s.pair(Name(b"Type"), Name(b"Metadata"));
        s.pair(Name(b"Subtype"), Name(b"XML"));
        s.finish();
    }

    // ICC profile stream (DestOutputProfile, /N 4). Written only alongside the OutputIntent that
    // references it — an orphan profile stream in a screen file would be dead weight in every copy
    // a customer downloads.
    if let (Some(icc_id), Some(bytes)) = (icc_id, icc_bytes.as_ref()) {
        let mut icc_stream = pdf.icc_profile(icc_id, bytes);
        icc_stream.n(4);
        icc_stream.finish();
    }

    for ((_, font), ids) in family.faces().zip(&font_ids) {
        write_font(&mut pdf, font, ids[0], ids[1], ids[2], ids[3], ids[4]);
    }

    // Image XObjects: grayscale as /DeviceGray, color as /DeviceCMYK (spec 0005).
    for img in images_by_id.values() {
        let bytes = match &img.pixels {
            Pixels::Gray(g) => g,
            Pixels::Cmyk(c) => c,
        };
        let compressed = deflate(bytes);
        let mut xobj = pdf.image_xobject(img.id, &compressed);
        xobj.width(img.width as i32);
        xobj.height(img.height as i32);
        match img.pixels {
            Pixels::Gray(_) => xobj.color_space().device_gray(),
            Pixels::Cmyk(_) => xobj.color_space().device_cmyk(),
        };
        xobj.bits_per_component(8);
        xobj.filter(Filter::FlateDecode);
        xobj.finish();
    }

    // Pages.
    for (i, (page, (page_id, content_id))) in pages.iter().zip(&page_refs).enumerate() {
        let g = geom::page_geom(&doc.page_setup, i);
        let content = render_page(page, &g, family, &images_by_id);

        {
            let mut p = pdf.page(*page_id);
            p.parent(page_tree_id);
            let media = Rect::new(0.0, 0.0, g.media_w, g.media_h);
            p.media_box(media);
            p.bleed_box(media);
            let (tx, ty) = g.trim_origin_pdf();
            p.trim_box(Rect::new(tx, ty, tx + g.trim_w, ty + g.trim_h));
            p.contents(*content_id);

            let mut res = p.resources();
            {
                // Every embedded face is available on every page. Naming only the faces a given
                // page happens to use would make the resource dictionary depend on pagination, and
                // a page that reflowed into a bold word would need its dictionary rewritten.
                let mut fonts_dict = res.fonts();
                for (slot, ids) in font_ids.iter().enumerate() {
                    let name = fonts::EmbeddedFamily::resource_name(slot);
                    fonts_dict.pair(Name(name.as_bytes()), ids[0]);
                }
            }
            // Only the images actually placed on this page.
            let mut xo = res.x_objects();
            let mut seen = BTreeSet::new();
            for block in page.statics.iter().chain(page.blocks.iter()) {
                if let PlacedBlock::Image { asset_id, .. } = block {
                    if let Some(img) = images_by_id.get(asset_id) {
                        if seen.insert(asset_id.clone()) {
                            xo.pair(Name(img.name.as_bytes()), img.id);
                        }
                    }
                }
            }
            xo.finish();
            res.finish();
            // `/Annots` is written only when the page actually has one. An empty array is legal and
            // would still be an `/Annots` key in a press file, which is precisely the thing spec
            // 0052's press test looks for — so "no annotations" has to mean *no key*.
            if !annots[i].is_empty() {
                p.annotations(annots[i].iter().map(|a| a.id));
            }
            p.finish();
        }

        // `/FlateDecode`, like the image XObjects and the font programs above (spec 0071). PDF 1.2,
        // so it costs the PDF/X-1a:2001 conformance nothing — the header stays at 1.3 and the same
        // filter is already on two other stream classes in this very file.
        //
        // It was the last stream that carries *bulk* and was not compressed — the XMP packet and
        // the `/ToUnicode` CMap stay uncompressed on purpose, and are fixed small costs. That was
        // affordable while the page held only operators and glyph bytes. Spec 0068 filled it with
        // per-pair `TJ` adjustments:
        // short repetitive integers drawn from a handful of values, which is close to a best case
        // for deflate. The cost is that nothing can grep the finished file for operator text any
        // more; `stream_read::content_streams` inflates, and so does the CI preflight job.
        let compressed = deflate(&content);
        let mut s = pdf.stream(*content_id, &compressed);
        s.filter(Filter::FlateDecode);
        s.finish();
    }

    // Link annotation objects (spec 0052). None under the press profile, by the PDF/X rule above.
    for (i, page_links) in annots.iter().enumerate() {
        let g = geom::page_geom(&doc.page_setup, i);
        for link in page_links {
            let mut a = pdf.annotation(link.id);
            a.subtype(AnnotationType::Link);
            a.rect(annot_rect(&g, &link.rect));
            // `/Border [0 0 0]`: no visible box. A viewer's default border is a 1-unit rectangle
            // drawn around every link, which would put a frame around every contents entry.
            a.border(0.0, 0.0, 0.0, None);
            if let Some((target, _)) = page_refs.get(link.target_page) {
                // `Fit`, matching the outline's destinations (spec 0042): the whole page is what a
                // contents entry means, and it survives the reader's own zoom setting.
                a.action()
                    .action_type(ActionType::GoTo)
                    .destination()
                    .page(*target)
                    .fit();
            }
            a.finish();
        }
    }

    // Trailer file identifier (deterministic → reproducible golden output).
    let id_bytes = doc_id_bytes(doc).to_vec();
    pdf.set_file_id((id_bytes.clone(), id_bytes));

    out.write_all(&pdf.finish())?;
    Ok(())
}

/// A link annotation that is going to be written: its object reference, its rectangle in page
/// (top-left) space, and the page it navigates to.
struct PendingLink {
    id: Ref,
    rect: quill_core_model::Rect,
    target_page: usize,
}

/// Whether an annotation at `rect` may be written under `profile` (spec 0052).
///
/// The press profile does **not** ask "is this the screen profile?" — it asks spec 0042's question,
/// [`crate::annotation_finding`], which is the PDF/X rule stated as code. Because quill writes
/// `MediaBox == BleedBox` (spec 0013), every annotation on the page fails it and a press file
/// therefore contains none. Writing it as the rule rather than as `if screen { … }` is what makes
/// the press file's emptiness a *consequence of PDF/X* rather than of a branch someone can flip,
/// and it stays correct if the two boxes ever diverge.
fn annotation_is_legal(
    profile: ExportProfile,
    rect: &quill_core_model::Rect,
    page_setup: &quill_core_model::PageSetup,
    page_index: usize,
) -> bool {
    match profile {
        ExportProfile::Press => crate::annotation_finding(rect, page_setup, page_index).is_none(),
        // A screen file claims no PDF/X conformance, so the BleedBox rule does not apply to it.
        ExportProfile::Screen => true,
    }
}

/// A placed rectangle (top-left origin, the space `PlacedBlock` uses) as a PDF `/Rect`
/// (bottom-left origin).
///
/// Flipped through the same [`geom`] offsets the content stream uses, not a second convention —
/// spec 0013's one-source-of-truth rule. A link whose hot area is flipped by a different formula
/// from the text under it is a defect no parser would ever report.
fn annot_rect(g: &geom::PageGeom, rect: &quill_core_model::Rect) -> Rect {
    let x0 = g.off_x + rect.x_pt;
    let y1 = g.media_h - (g.off_y + rect.y_pt);
    let y0 = y1 - rect.h_pt;
    Rect::new(x0, y0, x0 + rect.w_pt, y1)
}

/// A decoded image plus its assigned reference and content-stream name.
struct ImageObj {
    name: String,
    pixels: Pixels,
    width: u32,
    height: u32,
    id: Ref,
}

/// Write the `/OutputIntent` dict. The identifier is `Custom`, so `/Info` is required.
fn write_output_intent(mut oi: OutputIntent<'_>, icc_id: Ref) {
    oi.subtype(pdf_writer::types::OutputIntentSubtype::PDFX);
    oi.output_condition_identifier(TextStr("Custom"));
    oi.info(TextStr("Quill synthetic CMYK output intent"));
    oi.dest_output_profile(icc_id);
    oi.finish();
}

/// Emit the Type0 → CIDFont → FontDescriptor → FontFile chain. The descendant-font subtype and the
/// embedded font stream depend on the outline flavour (spec 0011):
/// - **TrueType** (`glyf`): `CIDFontType2` + `CIDToGIDMap /Identity`, program in `FontFile2` with
///   a `Length1` (the sfnt byte length).
/// - **CFF**: `CIDFontType0` (no `CIDToGIDMap` — it is meaningful only for `CIDFontType2`), bare
///   `CFF ` table in `FontFile3` tagged `/Subtype /CIDFontType0C`, no `Length1`.
///
/// The Type0 font also names a `/ToUnicode` CMap (spec 0068). With `Identity-H` and no CMap, a copy
/// or a search over the file yields subset glyph ids rather than text — survivable while the
/// encoding was one glyph per character, because the mapping was recoverable in principle, and
/// *not* survivable once a ligature draws as one glyph: nothing downstream can learn that one glyph
/// was three characters, because the cluster map that says so exists only at the moment of shaping.
fn write_font(
    pdf: &mut Pdf,
    font: &fonts::EmbeddedFont,
    type0_id: Ref,
    cid_id: Ref,
    descriptor_id: Ref,
    fontfile_id: Ref,
    to_unicode_id: Ref,
) {
    let base = font.base_font.as_bytes();
    let is_cff = font.outlines == fonts::OutlineKind::Cff;
    {
        let mut t0 = pdf.type0_font(type0_id);
        t0.base_font(Name(base));
        t0.encoding_predefined(Name(b"Identity-H"));
        t0.descendant_font(cid_id);
        t0.to_unicode(to_unicode_id);
        t0.finish();
    }
    {
        let mut cid = pdf.cid_font(cid_id);
        cid.subtype(if is_cff {
            CidFontType::Type0
        } else {
            CidFontType::Type2
        });
        cid.base_font(Name(base));
        cid.system_info(SystemInfo {
            registry: Str(b"Adobe"),
            ordering: Str(b"Identity"),
            supplement: 0,
        });
        cid.font_descriptor(descriptor_id);
        // CIDToGIDMap applies only to CIDFontType2; CIDFontType0 maps CID→glyph via the CFF charset.
        if !is_cff {
            cid.cid_to_gid_map_predefined(Name(b"Identity"));
        }
        cid.widths().consecutive(0, font.widths.iter().copied());
        cid.finish();
    }
    {
        let mut fd = pdf.font_descriptor(descriptor_id);
        fd.name(Name(base));
        fd.flags(FontFlags::from_bits_retain(font.flags.bits()));
        fd.bbox(Rect::new(
            font.bbox[0],
            font.bbox[1],
            font.bbox[2],
            font.bbox[3],
        ));
        fd.italic_angle(font.italic_angle);
        fd.ascent(font.ascent);
        fd.descent(font.descent);
        fd.cap_height(font.cap_height);
        fd.stem_v(font.stem_v);
        if is_cff {
            fd.font_file3(fontfile_id);
        } else {
            fd.font_file2(fontfile_id);
        }
        fd.finish();
    }
    {
        let compressed = deflate(&font.subset);
        let mut s = pdf.stream(fontfile_id, &compressed);
        s.filter(Filter::FlateDecode);
        if is_cff {
            // FontFile3 identifies the embedded program by its /Subtype; bare CFF is CIDFontType0C.
            s.pair(Name(b"Subtype"), Name(b"CIDFontType0C"));
        } else {
            s.pair(Name(b"Length1"), font.subset.len() as i32);
        }
        s.finish();
    }
    {
        // Left uncompressed, like the XMP packet and unlike the font program: it is the one object
        // in the file a reader may have to open by hand to answer "what does this glyph say", and
        // the workspace's tests read it out of the finished PDF rather than out of a fixture.
        let mut cmap = UnicodeCmap::<u16>::new(Name(b"Adobe-Identity-UCS"), UCS_SYSTEM_INFO);
        for (gid, text) in font.to_unicode() {
            cmap.pair_with_multiple(gid, text.chars());
        }
        let bytes = cmap.finish();
        let mut stream = pdf.cmap(to_unicode_id, bytes.as_slice());
        stream.name(Name(b"Adobe-Identity-UCS"));
        stream.system_info(UCS_SYSTEM_INFO);
        stream.finish();
    }
}

/// The `CIDSystemInfo` a `/ToUnicode` CMap declares: `Adobe-UCS-0`, which is what the destination
/// strings are (UTF-16BE), not the `Adobe-Identity-0` the *encoding* uses.
const UCS_SYSTEM_INFO: SystemInfo<'static> = SystemInfo {
    registry: Str(b"Adobe"),
    ordering: Str(b"UCS"),
    supplement: 0,
};

/// Build one page's content stream: text (black) and image XObjects, y-flipped into PDF space.
fn render_page(
    page: &LaidOutPage,
    g: &geom::PageGeom,
    family: &fonts::EmbeddedFamily,
    images_by_id: &BTreeMap<String, ImageObj>,
) -> Vec<u8> {
    let mut content = Content::new();
    // Statics first, then flowed content: a master page's background art has to sit *behind* the
    // text that flows over it, and PDF content streams paint in order (spec 0029).
    for block in page.statics.iter().chain(page.blocks.iter()) {
        match block {
            PlacedBlock::Text {
                frame,
                lines,
                color,
                run_colors,
                run_formats,
                run_shifts,
                weight,
                italic,
                font_size_pt,
                leading_pt,
                ..
            } => {
                // The block's own face and size: what a span with no override of its own is set in
                // (spec 0064).
                let base = quill_text_layout::RunFormat {
                    size_pt: *font_size_pt,
                    weight: *weight,
                    italic: *italic,
                    tracking_pt: 0.0,
                };
                let base_slot = family.slot(fonts::face_of(base));
                content.begin_text();
                // Per-block size, from the style the layout engine measured with (spec 0028).
                // Taking it from the placed block rather than re-deriving it is what guarantees the
                // glyphs are drawn at the size they were broken at — any disagreement between the
                // two would put text in the wrong place, or overset a frame that measured as fitting.
                let base_name = fonts::EmbeddedFamily::resource_name(base_slot);
                content.set_font(Name(base_name.as_bytes()), *font_size_pt);
                // Emit the authored press-legal fill color. Preflight rejects RGB before export,
                // so `Rgb` is unreachable here; fall back to black (rather than panicking) so a
                // `--force` export can never abort mid-stream.
                match color {
                    Color::Gray { v } => content.set_fill_gray(*v),
                    Color::Cmyk { c, m, y, k } => content.set_fill_cmyk(*c, *m, *y, *k),
                    Color::Rgb { .. } => content.set_fill_gray(0.0),
                };
                // The text state actually in force, carried across the whole block. `Tf`, `Tc`,
                // `Ts` and the fill colour all outlive the line that set them, so a line that
                // assumes what the block set — rather than what the *previous line* left — draws
                // its glyphs out of another face's subset. That is `.notdef` boxes in a press file,
                // so the state is tracked rather than assumed.
                let mut state = TextState {
                    slot: base_slot,
                    size_pt: *font_size_pt,
                    tracking_pt: 0.0,
                    shift_pt: 0.0,
                    ink: fmt_ink(color),
                };
                // Ascent is the family's, and the four bundled faces share one set of vertical
                // metrics, so it is the same wherever a run sits (spec 0064).
                let ascent = family.font(base_slot).ascent_pt(*font_size_pt);
                // The frame's left edge is the leftmost line's, not the measure's (spec 0069).
                let indent_base = quill_text_layout::indent_base(lines);
                for (li, line) in lines.iter().enumerate() {
                    let top_y = frame.y_pt + ascent + li as f32 * leading_pt;
                    // The line's own left inset (spec 0048), relative to the block's ink box — the
                    // same field and the same shared base the screen painter uses, so press and
                    // screen cannot disagree about an indent.
                    let (x, y) = geom::flip(g, frame.x_pt + line.indent_pt - indent_base, top_y);
                    // Absolute text matrix per line (avoids relative-Td bookkeeping).
                    content.set_text_matrix([1.0, 0.0, 0.0, 1.0, x, y]);
                    // One colour *and* one format for the whole line is the overwhelmingly common
                    // case and the one that must stay byte-identical: it takes the path it took
                    // before runs existed. Note that "one format" is compared against the format
                    // the line is actually set in, which need not be the block's — a bold run long
                    // enough to fill a line makes every interior line uniformly bold.
                    match (
                        single_ink(line, run_colors, *color),
                        single_format(line, run_formats, base),
                        single_shift(line, run_shifts),
                    ) {
                        (Some(ink), Some(fmt), Some(shift)) => {
                            set_state(&mut content, family, &mut state, fmt, shift, ink);
                            show_line(&mut content, family, state.slot, line, fmt.size_pt);
                        }
                        _ => show_line_by_span(
                            &mut content,
                            family,
                            line,
                            base,
                            run_colors,
                            run_formats,
                            run_shifts,
                            *color,
                            &mut state,
                        ),
                    }
                }
                // `Tc` and `Ts` are graphics state and outlive the text object, so a block that set
                // either has to put it back — the next block re-emits its own `Tf` and fill, but
                // nothing re-emits a tracking nobody asked for.
                if state.tracking_pt != 0.0 {
                    content.set_char_spacing(0.0);
                }
                if state.shift_pt != 0.0 {
                    content.set_rise(0.0);
                }
                content.end_text();
            }
            PlacedBlock::Rect {
                frame,
                fill,
                stroke,
            } => {
                // Degenerate geometry emits nothing at all, rather than an empty path with a `n`
                // no-op: a content stream should not carry operators that draw nothing.
                if (fill.is_none() && stroke.is_none()) || frame.w_pt <= 0.0 || frame.h_pt <= 0.0 {
                    continue;
                }
                content.save_state();
                if let Some(color) = fill {
                    set_fill(&mut content, color);
                }
                if let Some(s) = stroke {
                    set_stroke(&mut content, &s.color);
                    // Points, unscaled: no CTM is in effect here, so the operand is the width the
                    // author asked for. A hairline that silently became device-dependent is the
                    // classic press bug in this code.
                    content.set_line_width(s.width_pt);
                }
                // PDF's origin is bottom-left; `frame` is top-left, so the rect's PDF y is its
                // *bottom* edge. Derived through the same offsets `geom::flip` uses, not a second
                // convention (spec 0013's one-source-of-truth rule).
                let y_bottom = g.media_h - (g.off_y + frame.y_pt + frame.h_pt);
                content.rect(g.off_x + frame.x_pt, y_bottom, frame.w_pt, frame.h_pt);
                // Nonzero winding, not even-odd: identical for a single rectangle, but `f` is the
                // conventional operator and `f*` in a stream reads as though a subpath was expected.
                match (fill.is_some(), stroke.is_some()) {
                    (true, true) => content.fill_nonzero_and_stroke(),
                    (true, false) => content.fill_nonzero(),
                    (false, true) => content.stroke(),
                    (false, false) => unreachable!("guarded above"),
                };
                content.restore_state();
            }
            PlacedBlock::Image {
                frame, asset_id, ..
            } => {
                if let Some(img) = images_by_id.get(asset_id) {
                    // Bottom-left of the image in PDF space, then scale the unit square to size.
                    let y_bottom = g.media_h - (g.off_y + frame.y_pt + frame.h_pt);
                    let x_left = g.off_x + frame.x_pt;
                    content.save_state();
                    content.transform([frame.w_pt, 0.0, 0.0, frame.h_pt, x_left, y_bottom]);
                    content.x_object(Name(img.name.as_bytes()));
                    content.restore_state();
                }
            }
            // A link candidate is navigation, not ink: it contributes no content-stream operator in
            // either profile. Under `Screen` it becomes an `/Annots` entry outside the stream;
            // under `Press` it becomes nothing at all (spec 0052).
            PlacedBlock::Link { .. } => {}
        }
    }
    content.finish().as_slice().to_vec()
}

/// One node of the PDF outline tree.
struct OutlineNode {
    id: Ref,
    title: String,
    page_index: usize,
    children: Vec<usize>,
    parent: Option<usize>,
}

/// The `/Outlines` tree, built from spec 0040's heading index.
///
/// Nesting follows heading level: an `h2` after an `h1` is its child, an `h1` after an `h2` closes
/// back to the root. Levels may skip (an `h3` directly under an `h1`) and may start deep (a
/// document whose first heading is an `h2`) — both are authoring realities, and the stack handles
/// them by attaching to the nearest shallower ancestor rather than by assuming a well-formed
/// hierarchy.
struct OutlineTree {
    root: Option<Ref>,
    nodes: Vec<OutlineNode>,
    top: Vec<usize>,
}

impl OutlineTree {
    fn build(headings: &[quill_layout_engine::HeadingEntry], alloc: &mut Alloc) -> OutlineTree {
        if headings.is_empty() {
            return OutlineTree {
                root: None,
                nodes: Vec::new(),
                top: Vec::new(),
            };
        }
        let root = alloc.bump();
        let mut nodes: Vec<OutlineNode> = Vec::with_capacity(headings.len());
        let mut top: Vec<usize> = Vec::new();
        // (level, node index) of the open ancestors, shallowest first.
        let mut stack: Vec<(u8, usize)> = Vec::new();

        for h in headings {
            while stack.last().is_some_and(|(level, _)| *level >= h.level) {
                stack.pop();
            }
            let parent = stack.last().map(|(_, i)| *i);
            let index = nodes.len();
            nodes.push(OutlineNode {
                id: alloc.bump(),
                title: h.text.clone(),
                page_index: h.page_index,
                children: Vec::new(),
                parent,
            });
            match parent {
                Some(p) => nodes[p].children.push(index),
                None => top.push(index),
            }
            stack.push((h.level, index));
        }

        OutlineTree {
            root: Some(root),
            nodes,
            top,
        }
    }

    /// Visible descendants of `index`, counting the whole open subtree.
    ///
    /// The `/Count` of an open item is its *visible descendants*, not its direct children — an item
    /// with two children that each have three is 8, not 2. Getting this wrong is the kind of defect
    /// a parser accepts and a viewer renders wrongly, which is why it is computed rather than
    /// approximated.
    fn visible(&self, index: usize) -> i32 {
        self.nodes[index]
            .children
            .iter()
            .map(|c| 1 + self.visible(*c))
            .sum()
    }

    fn write(&self, pdf: &mut Pdf, page_refs: &[(Ref, Ref)]) {
        let Some(root) = self.root else { return };
        {
            let mut o = pdf.outline(root);
            if let (Some(first), Some(last)) = (self.top.first(), self.top.last()) {
                o.first(self.nodes[*first].id);
                o.last(self.nodes[*last].id);
            }
            // Every item is open, so the root's count is simply all of them.
            o.count(self.nodes.len() as i32);
            o.finish();
        }

        for (index, node) in self.nodes.iter().enumerate() {
            let siblings = match node.parent {
                Some(p) => &self.nodes[p].children,
                None => &self.top,
            };
            let at = siblings.iter().position(|i| *i == index).expect("sibling");

            let mut item = pdf.outline_item(node.id);
            item.title(TextStr(&node.title));
            item.parent(node.parent.map_or(root, |p| self.nodes[p].id));
            if at > 0 {
                item.prev(self.nodes[siblings[at - 1]].id);
            }
            if at + 1 < siblings.len() {
                item.next(self.nodes[siblings[at + 1]].id);
            }
            if let (Some(first), Some(last)) = (node.children.first(), node.children.last()) {
                item.first(self.nodes[*first].id);
                item.last(self.nodes[*last].id);
                item.count(self.visible(index));
            }
            if let Some((page, _)) = page_refs.get(node.page_index) {
                // `fit` rather than an explicit position: the whole page is the destination, which
                // is what a chapter bookmark means and what survives a viewer's own zoom setting.
                item.dest().page(*page).fit();
            }
            item.finish();
        }
    }
}

/// Set the non-stroking colour in the authored space.
///
/// `Rgb` is unreachable — preflight rejects it before export — but falls back to black rather than
/// panicking, so a `--force` export can never abort mid-stream. Same posture as the text path.
fn set_fill(content: &mut Content, color: &Color) {
    match color {
        Color::Gray { v } => content.set_fill_gray(*v),
        Color::Cmyk { c, m, y, k } => content.set_fill_cmyk(*c, *m, *y, *k),
        Color::Rgb { .. } => content.set_fill_gray(0.0),
    };
}

/// Set the stroking colour. Separate from [`set_fill`] because PDF's stroking operators are
/// distinct (`K`/`G` rather than `k`/`g`) — using the fill operator for a stroke sets the wrong
/// colour silently, with no error anywhere in the pipeline.
fn set_stroke(content: &mut Content, color: &Color) {
    match color {
        Color::Gray { v } => content.set_stroke_gray(*v),
        Color::Cmyk { c, m, y, k } => content.set_stroke_cmyk(*c, *m, *y, *k),
        Color::Rgb { .. } => content.set_stroke_gray(0.0),
    };
}

/// The one ink this line is set in, or `None` if its spans disagree (spec 0063).
///
/// Compared against `fallback` — the block's own colour — and not merely against the line's first
/// span, because a line lying wholly inside one recoloured run is uniform in *that* colour, which
/// is not the block's.
fn single_ink(line: &Line, run_colors: &[Color], fallback: Color) -> Option<Color> {
    if run_colors.is_empty() || line.spans.is_empty() {
        return Some(fallback);
    }
    let first = run_colors
        .get(line.spans[0].run)
        .copied()
        .unwrap_or(fallback);
    line.spans
        .iter()
        .all(|sp| fmt_ink(&run_colors.get(sp.run).copied().unwrap_or(fallback)) == fmt_ink(&first))
        .then_some(first)
}

/// Colours are `f32` and so not `Eq`; compare them by their debug form, which is what the rest of
/// the workspace already does when it needs colour identity rather than colour arithmetic.
fn fmt_ink(c: &Color) -> String {
    format!("{c:?}")
}

/// The one format this line is set in, or `None` if its spans disagree (spec 0064).
///
/// Returns the *line's* format, not the block's. A run long enough to fill a line makes every
/// interior line of it uniformly bold — uniform, but not uniform in the block's face, and answering
/// this question against the block would draw those lines from the wrong subset.
fn single_format(
    line: &Line,
    run_formats: &[quill_text_layout::RunFormat],
    base: quill_text_layout::RunFormat,
) -> Option<quill_text_layout::RunFormat> {
    if run_formats.is_empty() || line.spans.is_empty() {
        return Some(base);
    }
    let first = run_formats.get(line.spans[0].run).copied().unwrap_or(base);
    line.spans
        .iter()
        .all(|sp| run_formats.get(sp.run).copied().unwrap_or(base) == first)
        .then_some(first)
}

/// The PDF text state a block is currently drawing in.
///
/// `Tf`, `Tc`, `Ts` and the fill colour all persist until something changes them, so "what is in
/// force" is a property of the content stream and not of the line being written. Tracking it means
/// each operator is emitted exactly when it changes — which is what keeps a single-face block's
/// stream byte-identical to what it was before the family existed, and what stops a bold line from
/// leaving the font set to bold for the plain line after it.
struct TextState {
    slot: usize,
    size_pt: f32,
    tracking_pt: f32,
    shift_pt: f32,
    ink: String,
}

/// The one baseline shift this line is set at, or `None` if its spans disagree (spec 0064).
fn single_shift(line: &Line, run_shifts: &[f32]) -> Option<f32> {
    if run_shifts.is_empty() || line.spans.is_empty() {
        return Some(0.0);
    }
    let first = run_shifts.get(line.spans[0].run).copied().unwrap_or(0.0);
    line.spans
        .iter()
        .all(|sp| run_shifts.get(sp.run).copied().unwrap_or(0.0) == first)
        .then_some(first)
}

/// Bring the stream's text state to `fmt`/`shift`/`ink`, emitting only the operators that change.
fn set_state(
    content: &mut Content,
    family: &fonts::EmbeddedFamily,
    state: &mut TextState,
    fmt: quill_text_layout::RunFormat,
    shift_pt: f32,
    ink: Color,
) {
    let slot = family.slot(fonts::face_of(fmt));
    if slot != state.slot || fmt.size_pt != state.size_pt {
        let name = fonts::EmbeddedFamily::resource_name(slot);
        content.set_font(Name(name.as_bytes()), fmt.size_pt);
        state.slot = slot;
        state.size_pt = fmt.size_pt;
    }
    if fmt.tracking_pt != state.tracking_pt {
        content.set_char_spacing(fmt.tracking_pt);
        state.tracking_pt = fmt.tracking_pt;
    }
    if shift_pt != state.shift_pt {
        content.set_rise(shift_pt);
        state.shift_pt = shift_pt;
    }
    let want = fmt_ink(&ink);
    if want != state.ink {
        set_fill(content, &ink);
        state.ink = want;
    }
}

/// Show one line, changing fill colour, face, size, tracking and baseline shift at each span
/// boundary (specs 0063, 0064).
///
/// Colour and text-state operators are ordinary graphics state and are legal inside a text object,
/// so this needs no `ET`/`BT` pair — the text matrix set for the line stays current across the whole
/// line, and PDF advances the text position by the glyphs shown, which are the same glyphs, at the
/// same widths, that the family measured with.
///
/// Justification is unchanged in meaning: the adjustment goes after every inter-word space, exactly
/// as [`show_line`] places it. It is emitted in whichever span's array the space falls, so a span
/// boundary inside a word cannot lose or double an adjustment — and it is computed from *that
/// span's* size, because the unit is a thousandth of the current font size and a span set at another
/// size would otherwise spend the wrong amount of space.
#[allow(clippy::too_many_arguments)]
fn show_line_by_span(
    content: &mut Content,
    family: &fonts::EmbeddedFamily,
    line: &Line,
    base: quill_text_layout::RunFormat,
    run_colors: &[Color],
    run_formats: &[quill_text_layout::RunFormat],
    run_shifts: &[f32],
    fallback: Color,
    state: &mut TextState,
) {
    let mut at = 0usize;
    for sp in &line.spans {
        let end = (at + sp.len_bytes).min(line.text.len());
        let piece = &line.text[at..end];
        at = end;

        let fmt = run_formats.get(sp.run).copied().unwrap_or(base);
        let ink = run_colors.get(sp.run).copied().unwrap_or(fallback);
        // A rise is per span rather than per format: it is the one override that is not part of
        // `RunFormat`, because it moves a glyph without changing an advance (spec 0064).
        let shift = run_shifts.get(sp.run).copied().unwrap_or(0.0);
        set_state(content, family, state, fmt, shift, ink);

        if piece.is_empty() {
            continue;
        }
        // The span's own size, not the line's: the `TJ` unit is a thousandth of the *current* font
        // size, so a span set at another size would otherwise spend the wrong amount of space. Each
        // span is shaped on its own, exactly as it was measured — which is why a kern pair
        // straddling a span boundary is neither counted twice nor drawn.
        let per_space = justification_amount(line, fmt.size_pt);
        show_segments(content, &shaped(family, state.slot, piece, per_space));
    }
}

/// Draw one line's shaped glyph run (spec 0068).
///
/// `slot` is the face already in force — the text state was brought to it by [`set_state`], and
/// shaping through any other face would draw a sequence the line was not measured at.
fn show_line(
    content: &mut Content,
    family: &fonts::EmbeddedFamily,
    slot: usize,
    line: &Line,
    font_size_pt: f32,
) {
    let per_space = justification_amount(line, font_size_pt);
    show_segments(content, &shaped(family, slot, &line.text, per_space));
}

/// Shape one piece of text in the face already in force, and return the encoded glyph segments with
/// the `TJ` amount that follows each.
///
/// A ragged line takes the plain path, where every adjustment is an integral number of thousandths
/// of an em; a justified one additionally spends `per_space` after each inter-word space.
fn shaped(
    family: &fonts::EmbeddedFamily,
    slot: usize,
    text: &str,
    per_space: f32,
) -> Vec<(Vec<u8>, f32)> {
    let font = family.font(slot);
    let shaper = family.shaper(slot);
    if per_space == 0.0 {
        font.shape_piece(shaper, text)
            .into_iter()
            .map(|(bytes, adjust)| (bytes, adjust as f32))
            .collect()
    } else {
        font.shape_piece_justified(shaper, text, per_space)
    }
}

/// The `TJ` amount spent after each inter-word space to justify `line`, in thousandths of a text
/// space unit at `font_size_pt` (spec 0017, increment 2).
///
/// `TJ` amounts are **subtracted** from the position, so a rightward (space-adding) shift of
/// `space_adjust_pt` points is a *negative* number. `Tw` word spacing is unusable here — the font
/// is a Type0/Identity-H composite (2-byte codes) and PDF word spacing applies only to single-byte
/// code 32.
fn justification_amount(line: &Line, font_size_pt: f32) -> f32 {
    if line.space_adjust_pt == 0.0 {
        0.0
    } else {
        -1000.0 * line.space_adjust_pt / font_size_pt
    }
}

/// Emit encoded glyph segments, as a plain `Tj` when nothing needs adjusting and a positioned `TJ`
/// when something does.
///
/// The single-`Tj` case is not an optimisation: it is what keeps a kern-free, unjustified line
/// byte-identical to what the writer emitted before it drew shaped runs, which is what makes the
/// control case of spec 0068's width table meaningful rather than self-referential.
fn show_segments(content: &mut Content, segments: &[(Vec<u8>, f32)]) {
    // Empty text still shows an empty string, as the character encoder did — a text object that
    // silently drew nothing would be a second thing to explain.
    if segments.is_empty() {
        content.show(Str(&[]));
        return;
    }
    if let [(bytes, adjust)] = segments {
        if *adjust == 0.0 {
            content.show(Str(bytes));
            return;
        }
    }
    let mut tj = content.show_positioned();
    let mut items = tj.items();
    for (bytes, adjust) in segments {
        if !bytes.is_empty() {
            items.show(Str(bytes));
        }
        if *adjust != 0.0 {
            items.adjust(*adjust);
        }
    }
}

/// zlib-deflate for `/FlateDecode` streams.
fn deflate(data: &[u8]) -> Vec<u8> {
    let mut e = flate2::write::ZlibEncoder::new(Vec::new(), flate2::Compression::default());
    e.write_all(data).expect("deflate");
    e.finish().expect("deflate finish")
}

/// 16 deterministic bytes derived from the document (FNV-1a expanded), for the trailer `/ID`.
fn doc_id_bytes(doc: &Document) -> [u8; 16] {
    let json = doc.to_json().unwrap_or_default();
    let mut out = [0u8; 16];
    let mut h: u64 = 0xcbf2_9ce4_8422_2325;
    for (i, b) in json.bytes().enumerate() {
        h ^= b as u64;
        h = h.wrapping_mul(0x0000_0100_0000_01b3);
        out[i % 16] ^= (h >> ((i % 8) * 8)) as u8;
    }
    out
}

/// Hex form of [`doc_id_bytes`] for the XMP `DocumentID`/`InstanceID`.
fn doc_id_hex(doc: &Document) -> String {
    doc_id_bytes(doc)
        .iter()
        .map(|b| format!("{b:02x}"))
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::stream_read::{shown_items, Shown};
    use quill_core_model::PageSetup;
    use quill_layout_engine::Stroke;
    use quill_text_layout::{
        BODY_FONT_SIZE_PT as FONT_SIZE_PT, BODY_LINE_HEIGHT_PT as LINE_HEIGHT_PT,
    };

    /// The bundled regular face as a one-face family — what every one of these tests draws with.
    fn test_family(text: &fonts::FaceText) -> fonts::EmbeddedFamily {
        let font = fonts::build(text).expect("build bundled font");
        fonts::EmbeddedFamily::new(
            vec![(quill_fonts::FaceKey::REGULAR, font)],
            quill_fonts::FontFamily::single(
                quill_fonts::Font::from_bytes(quill_fonts::BUNDLED_TTF).expect("parse"),
            ),
        )
    }

    /// Build a one-page layout holding a single text block with the given fill color, render it,
    /// and return the content-stream bytes as a lossy string. Content streams are not FlateDecode'd
    /// anywhere — not here and not in the finished file — so operators are directly greppable.
    fn render_text_color(color: Color) -> String {
        let setup = PageSetup::default();
        let g = geom::page_geom(&setup, 0);
        let font = test_family(&fonts::FaceText::from_text("Hi"));
        let page = LaidOutPage {
            index: 0,
            statics: Vec::new(),
            blocks: vec![PlacedBlock::Text {
                run_formats: Vec::new(),
                run_shifts: Vec::new(),
                weight: 400,
                italic: false,
                run_colors: Vec::new(),
                source: quill_core_model::BlockId::UNASSIGNED,
                frame: quill_core_model::Rect {
                    x_pt: 0.0,
                    y_pt: 0.0,
                    w_pt: setup.trim.w_pt,
                    h_pt: LINE_HEIGHT_PT,
                },
                font_size_pt: FONT_SIZE_PT,
                leading_pt: LINE_HEIGHT_PT,
                lines: vec![Line::single_run("Hi".to_string(), 0.0, 0.0)],
                color,
            }],
        };
        let content = render_page(&page, &g, &font, &BTreeMap::new());
        String::from_utf8_lossy(&content).into_owned()
    }

    /// Spec 0064: a line whose spans differ in format switches face, size, tracking and rise at the
    /// span boundary — and resets the two that are text state, so they cannot leak into the next
    /// line.
    #[test]
    fn a_mixed_format_line_switches_face_tracking_and_rise() {
        let setup = PageSetup::default();
        let g = geom::page_geom(&setup, 0);
        let chars = fonts::FaceText::from_chars("Hiabc".chars());
        let family = fonts::EmbeddedFamily::new(
            vec![
                (
                    quill_fonts::FaceKey::REGULAR,
                    fonts::build(&chars).expect("regular"),
                ),
                (
                    quill_fonts::FaceKey::new(700, false),
                    fonts::build_from_bytes(quill_fonts::BUNDLED_BOLD_TTF, None, &chars)
                        .expect("bold"),
                ),
            ],
            quill_fonts::FontFamily::bundled(),
        );
        let line = Line {
            text: "Hi abc".into(),
            spans: vec![
                quill_text_layout::Span {
                    run: 0,
                    len_bytes: 3,
                },
                quill_text_layout::Span {
                    run: 1,
                    len_bytes: 3,
                },
            ],
            space_adjust_pt: 0.0,
            indent_pt: 0.0,
        };
        let page = LaidOutPage {
            index: 0,
            statics: Vec::new(),
            blocks: vec![PlacedBlock::Text {
                run_colors: Vec::new(),
                run_formats: vec![
                    quill_text_layout::RunFormat::plain(FONT_SIZE_PT),
                    quill_text_layout::RunFormat {
                        size_pt: FONT_SIZE_PT,
                        weight: 700,
                        italic: false,
                        tracking_pt: 0.5,
                    },
                ],
                run_shifts: vec![0.0, 2.0],
                weight: 400,
                italic: false,
                source: quill_core_model::BlockId::UNASSIGNED,
                frame: quill_core_model::Rect {
                    x_pt: 0.0,
                    y_pt: 0.0,
                    w_pt: setup.trim.w_pt,
                    h_pt: LINE_HEIGHT_PT,
                },
                font_size_pt: FONT_SIZE_PT,
                leading_pt: LINE_HEIGHT_PT,
                lines: vec![line],
                color: Color::Gray { v: 0.0 },
            }],
        };
        let content = String::from_utf8_lossy(&render_page(&page, &g, &family, &BTreeMap::new()))
            .into_owned();
        assert!(content.contains("/F0"), "the regular face is named");
        assert!(content.contains("/F1"), "the bold face is named: {content}");
        assert!(content.contains("0.5 Tc"), "tracking as Tc: {content}");
        assert!(content.contains("2 Ts"), "baseline shift as Ts: {content}");
        // Both are text state, so both are put back — a tracking left applied would silently widen
        // every line after this one.
        assert!(content.contains("0 Tc"), "tracking reset: {content}");
        assert!(content.contains("0 Ts"), "rise reset: {content}");
    }

    /// The defect an adversarial review of spec 0064 found: a line lying **wholly inside** a bold
    /// run is uniform — uniformly bold — and the first uniformity test compared a line's spans to
    /// each other rather than to the block's own face. Such a line took the fast path and was shown
    /// while the *block's* font was current, so its glyph ids were read out of the regular subset
    /// and drew as `.notdef` boxes.
    ///
    /// Asserted on the operator sequence, per line, because that is where it went wrong: what
    /// matters is which font is in force at each `Tj`, not which fonts appear in the stream.
    #[test]
    fn every_line_is_shown_in_the_face_it_was_set_in() {
        let setup = PageSetup::default();
        let g = geom::page_geom(&setup, 0);
        let chars = fonts::FaceText::from_chars("abcdefgh ".chars());
        let family = fonts::EmbeddedFamily::new(
            vec![
                (
                    quill_fonts::FaceKey::REGULAR,
                    fonts::build(&chars).expect("regular"),
                ),
                (
                    quill_fonts::FaceKey::new(700, false),
                    fonts::build_from_bytes(quill_fonts::BUNDLED_BOLD_TTF, None, &chars)
                        .expect("bold"),
                ),
            ],
            quill_fonts::FontFamily::bundled(),
        );
        let span = |run: usize, len: usize| quill_text_layout::Span {
            run,
            len_bytes: len,
        };
        let line = |text: &str, run: usize| Line {
            text: text.into(),
            spans: vec![span(run, text.len())],
            space_adjust_pt: 0.0,
            indent_pt: 0.0,
        };
        let page = LaidOutPage {
            index: 0,
            statics: Vec::new(),
            blocks: vec![PlacedBlock::Text {
                run_colors: Vec::new(),
                // Run 0 regular, run 1 bold, run 2 regular — and one line entirely inside each.
                run_formats: vec![
                    quill_text_layout::RunFormat::plain(FONT_SIZE_PT),
                    quill_text_layout::RunFormat {
                        size_pt: FONT_SIZE_PT,
                        weight: 700,
                        italic: false,
                        tracking_pt: 0.0,
                    },
                    quill_text_layout::RunFormat::plain(FONT_SIZE_PT),
                ],
                run_shifts: Vec::new(),
                weight: 400,
                italic: false,
                source: quill_core_model::BlockId::UNASSIGNED,
                frame: quill_core_model::Rect {
                    x_pt: 0.0,
                    y_pt: 0.0,
                    w_pt: setup.trim.w_pt,
                    h_pt: 3.0 * LINE_HEIGHT_PT,
                },
                font_size_pt: FONT_SIZE_PT,
                leading_pt: LINE_HEIGHT_PT,
                lines: vec![line("abc", 0), line("def", 1), line("gh", 2)],
                color: Color::Gray { v: 0.0 },
            }],
        };
        let content = String::from_utf8_lossy(&render_page(&page, &g, &family, &BTreeMap::new()))
            .into_owned();

        // Walk the stream tracking the font in force, and record which one each show was made in.
        // Both show operators count: spec 0068 made `TJ` the form a kerning line takes, so a walk
        // that matched only `" Tj"` would stop seeing lines rather than start failing — it would go
        // structurally blind, which is the failure mode this test exists to have caught.
        let mut current = String::new();
        let mut shown = Vec::new();
        for op in content.lines() {
            if let Some(name) = op.split_whitespace().next() {
                if op.contains(" Tf") && name.starts_with("/F") {
                    current = name.to_string();
                }
            }
            if op.ends_with(" Tj") || op.ends_with(" TJ") {
                shown.push(current.clone());
            }
        }
        assert_eq!(
            shown,
            vec!["/F0", "/F1", "/F0"],
            "each line must be shown in its own face:\n{content}"
        );
    }

    /// A block set in one face and one size emits exactly what it emitted before the family existed:
    /// one `Tf`, no `Tc`, no `Ts`.
    #[test]
    fn a_single_format_block_emits_no_text_state_it_does_not_need() {
        let content = render_text_color(Color::Gray { v: 0.0 });
        assert!(content.contains("/F0"));
        assert!(!content.contains("Tc"), "no tracking was set: {content}");
        assert!(!content.contains("Ts"), "no rise was set: {content}");
    }

    /// Render a page holding a single decoration rect and return the greppable content bytes.
    fn render_rect(
        frame: quill_core_model::Rect,
        fill: Option<Color>,
        stroke: Option<Stroke>,
    ) -> String {
        let setup = PageSetup::default();
        let g = geom::page_geom(&setup, 0);
        let font = test_family(&fonts::FaceText::default());
        let page = LaidOutPage {
            index: 0,
            statics: Vec::new(),
            blocks: vec![PlacedBlock::Rect {
                frame,
                fill,
                stroke,
            }],
        };
        let content = render_page(&page, &g, &font, &BTreeMap::new());
        String::from_utf8_lossy(&content).into_owned()
    }

    const PANEL: quill_core_model::Rect = quill_core_model::Rect {
        x_pt: 20.0,
        y_pt: 30.0,
        w_pt: 100.0,
        h_pt: 40.0,
    };

    #[test]
    fn a_filled_and_stroked_rect_emits_both_colours_and_a_paint_operator() {
        let s = render_rect(
            PANEL,
            Some(Color::Cmyk {
                c: 0.0,
                m: 0.0,
                y: 0.0,
                k: 0.1,
            }),
            Some(Stroke {
                color: Color::Gray { v: 0.0 },
                width_pt: 0.5,
            }),
        );
        assert!(
            s.lines().any(|l| l.ends_with(" k")),
            "CMYK fill colour: {s}"
        );
        assert!(
            s.lines().any(|l| l.ends_with(" G")),
            "gray STROKE colour: {s}"
        );
        assert!(s.contains(" re"), "rect path: {s}");
        assert!(s.contains("0.5 w"), "stroke width in points: {s}");
        assert!(s.lines().any(|l| l == "B"), "fill-and-stroke operator: {s}");
    }

    #[test]
    fn rect_geometry_is_flipped_into_pdf_space_exactly() {
        // Top-left `y = 30`, height 40, on a 648 pt trim. Bleed is on the three *non-binding*
        // edges only (spec 0013), so on this recto the media box is 666 pt tall with the content
        // origin 9 pt up, and there is no offset on the bound left edge:
        //   y = 666 - (9 + 30 + 40) = 587, x = 0 + 20 = 20.
        let s = render_rect(PANEL, Some(Color::Gray { v: 0.5 }), None);
        assert!(
            s.contains("20 587 100 40 re"),
            "expected exact flipped geometry, got: {s}"
        );
        assert!(s.lines().any(|l| l == "f"), "fill-only uses `f`: {s}");
        assert!(
            !s.lines().any(|l| l == "B"),
            "fill-only must not stroke: {s}"
        );
    }

    #[test]
    fn a_stroke_only_rect_uses_the_stroking_operators() {
        // `S`, not `f` — and the *stroking* colour operator `G`, not the fill `g`. Using the fill
        // operator for a stroke sets the wrong colour with no error anywhere in the pipeline.
        let s = render_rect(
            PANEL,
            None,
            Some(Stroke {
                color: Color::Gray { v: 0.0 },
                width_pt: 1.0,
            }),
        );
        assert!(
            s.lines().any(|l| l.ends_with(" G")),
            "stroking gray operator: {s}"
        );
        assert!(s.lines().any(|l| l == "S"), "stroke operator: {s}");
        assert!(!s.lines().any(|l| l == "f"), "must not fill: {s}");
    }

    #[test]
    fn a_rect_that_draws_nothing_emits_nothing() {
        // No colours, zero width, zero height — none of which may leave a dangling path in the
        // content stream. An `re` with no paint operator is a malformed stream.
        assert!(!render_rect(PANEL, None, None).contains("re"));
        let flat = quill_core_model::Rect { w_pt: 0.0, ..PANEL };
        assert!(!render_rect(flat, Some(Color::Gray { v: 0.0 }), None).contains("re"));
        let thin = quill_core_model::Rect { h_pt: 0.0, ..PANEL };
        assert!(!render_rect(thin, Some(Color::Gray { v: 0.0 }), None).contains("re"));
    }

    /// Render a single justified line and return the (uncompressed, greppable) content bytes.
    fn render_justified_line(text: &str, space_adjust_pt: f32) -> Vec<u8> {
        render_line_in(
            &test_family(&fonts::FaceText::from_text(text)),
            text,
            space_adjust_pt,
        )
    }

    /// The same, in a family the caller already built — so a test can ask the *same* subset both
    /// what was drawn and how wide its glyphs are declared to be.
    fn render_line_in(font: &fonts::EmbeddedFamily, text: &str, space_adjust_pt: f32) -> Vec<u8> {
        let setup = PageSetup::default();
        let g = geom::page_geom(&setup, 0);
        let page = LaidOutPage {
            index: 0,
            statics: Vec::new(),
            blocks: vec![PlacedBlock::Text {
                run_formats: Vec::new(),
                run_shifts: Vec::new(),
                weight: 400,
                italic: false,
                run_colors: Vec::new(),
                source: quill_core_model::BlockId::UNASSIGNED,
                frame: quill_core_model::Rect {
                    x_pt: 0.0,
                    y_pt: 0.0,
                    w_pt: setup.trim.w_pt,
                    h_pt: LINE_HEIGHT_PT,
                },
                font_size_pt: FONT_SIZE_PT,
                leading_pt: LINE_HEIGHT_PT,
                lines: vec![Line::single_run(text, space_adjust_pt, 0.0)],
                color: Color::Gray { v: 0.0 },
            }],
        };
        render_page(&page, &g, font, &BTreeMap::new())
    }

    /// The width the page will actually draw, in points: the `/W` entry of every glyph the stream
    /// showed, less every `TJ` amount it wrote (they are *subtracted* from the position).
    fn drawn_width_pt(content: &[u8], font: &fonts::EmbeddedFont, size_pt: f32) -> f32 {
        let mut units = 0.0f32;
        for item in shown_items(content) {
            match item {
                Shown::Glyphs(gids) => {
                    for gid in gids {
                        units += font.widths[gid as usize];
                    }
                }
                Shown::Adjust(amount) => units -= amount,
            }
        }
        units * size_pt / 1000.0
    }

    /// Spec 0068's headline acceptance criterion: **the width the PDF draws equals the width layout
    /// measured, to 0.01 pt**, over the five cases the spec tabulates.
    ///
    /// Measured by summing the `/W` entries of the glyphs the content stream actually shows and the
    /// `TJ` adjustments it actually writes, parsed back out of the emitted bytes. Deliberately not
    /// re-derived from the shaper: that would assert the shaper against itself and would pass even
    /// if the writer emitted nothing at all.
    #[test]
    fn the_drawn_width_equals_the_measured_width() {
        use quill_text_layout::RunMetrics;
        let cases: [(&str, &str); 5] = [
            (
                "ordinary prose",
                "The quick brown fox jumps over the lazy dog and then runs off",
            ),
            ("kern-heavy", "AV Wa To Yo, Pa."),
            ("office", "office"),
            (
                "ligature-rich",
                "The officer shuffled a fistful of ruffled affidavits off the shelf",
            ),
            ("control", "mmmmm nnnnn ooooo"),
        ];
        for (name, text) in cases {
            let family = test_family(&fonts::FaceText::from_text(text));
            let bytes = render_line_in(&family, text, 0.0);
            let drawn = drawn_width_pt(&bytes, family.font(0), FONT_SIZE_PT);
            let measured = family.metrics().measure_run(text, FONT_SIZE_PT);
            assert!(
                (drawn - measured).abs() < 0.01,
                "{name}: drawn {drawn} vs measured {measured} ({:+})",
                drawn - measured
            );
            // Nothing drew as `.notdef`, or the widths above would agree about a box.
            for item in shown_items(&bytes) {
                if let Shown::Glyphs(gids) = item {
                    assert!(gids.iter().all(|g| *g != 0), "{name}: .notdef drawn");
                }
            }
        }

        // The control case must come out at *exactly* zero adjustment, or the instrument is
        // measuring its own machinery rather than the writer's.
        let family = test_family(&fonts::FaceText::from_text("mmmmm nnnnn ooooo"));
        let control = render_line_in(&family, "mmmmm nnnnn ooooo", 0.0);
        assert_eq!(
            shown_items(&control)
                .into_iter()
                .filter(|i| matches!(i, Shown::Adjust(_)))
                .count(),
            0
        );
    }

    /// A justified line's width is its measured width plus the space the justifier bought — and the
    /// kern corrections must survive alongside it, not be replaced by it.
    #[test]
    fn a_justified_line_draws_its_measure_plus_the_space_it_was_given() {
        use quill_text_layout::RunMetrics;
        let text = "AV Wa To Yo";
        let family = test_family(&fonts::FaceText::from_text(text));
        let bytes = render_line_in(&family, text, 3.0);
        let drawn = drawn_width_pt(&bytes, family.font(0), FONT_SIZE_PT);
        // Three inter-word spaces, each widened by 3 pt.
        let measured = family.metrics().measure_run(text, FONT_SIZE_PT) + 3.0 * 3.0;
        assert!(
            (drawn - measured).abs() < 0.01,
            "drawn {drawn} vs measured {measured}"
        );
    }

    /// Spec 0068: a kern pair straddling a **span** boundary is neither lost nor doubled. There was
    /// no test for that boundary before this spec, and the trap is the one already documented for
    /// justification — each span is shaped and measured on its own, so the drawn width has to be
    /// the sum of the spans' measured widths, not the width of the line shaped as a whole.
    #[test]
    fn a_kern_pair_across_a_span_boundary_is_neither_lost_nor_doubled() {
        use quill_text_layout::RunMetrics;
        let setup = PageSetup::default();
        let g = geom::page_geom(&setup, 0);
        // `AV` kerns hard. Split across two runs that are set in the *same* face, so the only thing
        // that differs is where the span boundary falls.
        let family = test_family(&fonts::FaceText::from_text("AV"));
        let line = Line {
            text: "AV".into(),
            spans: vec![
                quill_text_layout::Span {
                    run: 0,
                    len_bytes: 1,
                },
                quill_text_layout::Span {
                    run: 1,
                    len_bytes: 1,
                },
            ],
            space_adjust_pt: 0.0,
            indent_pt: 0.0,
        };
        let page = LaidOutPage {
            index: 0,
            statics: Vec::new(),
            blocks: vec![PlacedBlock::Text {
                // Two runs differing in colour, so the by-span path is the one taken.
                run_colors: vec![Color::Gray { v: 0.0 }, Color::Gray { v: 0.5 }],
                run_formats: vec![
                    quill_text_layout::RunFormat::plain(FONT_SIZE_PT),
                    quill_text_layout::RunFormat::plain(FONT_SIZE_PT),
                ],
                run_shifts: Vec::new(),
                weight: 400,
                italic: false,
                source: quill_core_model::BlockId::UNASSIGNED,
                frame: quill_core_model::Rect {
                    x_pt: 0.0,
                    y_pt: 0.0,
                    w_pt: setup.trim.w_pt,
                    h_pt: LINE_HEIGHT_PT,
                },
                font_size_pt: FONT_SIZE_PT,
                leading_pt: LINE_HEIGHT_PT,
                lines: vec![line],
                color: Color::Gray { v: 0.0 },
            }],
        };
        let bytes = render_page(&page, &g, &family, &BTreeMap::new());
        let drawn = drawn_width_pt(&bytes, family.font(0), FONT_SIZE_PT);
        // What layout measured: each span on its own, which is how `measure_format` is called.
        let per_span = family.metrics().measure_run("A", FONT_SIZE_PT)
            + family.metrics().measure_run("V", FONT_SIZE_PT);
        assert!(
            (drawn - per_span).abs() < 0.01,
            "drawn {drawn} vs the sum of the spans' measures {per_span}"
        );
        // And the kern was not smuggled in twice, nor once: the whole-line shaping is strictly
        // narrower, so a drawn width equal to *that* would mean the writer shaped across the
        // boundary and the line would end short of where layout put the next one.
        let whole = family.metrics().measure_run("AV", FONT_SIZE_PT);
        assert!(whole < per_span - 0.01, "sanity: AV should kern");
        assert!(
            (drawn - whole).abs() > 0.01,
            "the kern was applied across a span boundary"
        );
    }

    #[test]
    fn text_emits_cmyk_fill_operator() {
        let s = render_text_color(Color::Cmyk {
            c: 0.1,
            m: 0.2,
            y: 0.3,
            k: 0.4,
        });
        assert!(
            s.contains("0.1 0.2 0.3 0.4 k"),
            "expected CMYK fill operator, got:\n{s}"
        );
    }

    #[test]
    fn text_emits_gray_fill_operator() {
        let s = render_text_color(Color::Gray { v: 0.5 });
        assert!(
            s.contains("0.5 g"),
            "expected grayscale fill operator, got:\n{s}"
        );
    }

    /// Rewritten by spec 0068 rather than deleted. It used to assert that a ragged line contains no
    /// `TJ` **at all**, which this spec invalidates by design — a ragged line whose glyphs kern now
    /// carries that kern as a `TJ` adjustment, which is the whole point.
    ///
    /// What it was actually protecting survives and is what is asserted here: a line that needs no
    /// correction is still one plain `Tj` and nothing more, so the overwhelmingly common case did
    /// not grow a positioned array it has no use for. The second half is the new half — a line that
    /// *does* kern must take the positioned form, or the correction has nowhere to go.
    #[test]
    fn a_line_that_needs_no_correction_is_drawn_with_one_show() {
        // Neither `kern` nor `liga` fires on these pairs in the bundled face.
        let bytes = render_justified_line("mmmmm nnnnn ooooo", 0.0);
        let s = String::from_utf8_lossy(&bytes);
        assert!(s.contains(" Tj"), "expected a Tj show, got:\n{s}");
        assert!(
            !s.contains(" TJ"),
            "a line needing no correction must not use TJ, got:\n{s}"
        );
        assert_eq!(
            shown_items(&bytes)
                .iter()
                .filter(|i| matches!(i, Shown::Adjust(_)))
                .count(),
            0,
            "the control case must emit exactly zero adjustments:\n{s}"
        );

        let kerned = render_justified_line("AV Wa To", 0.0);
        let k = String::from_utf8_lossy(&kerned);
        assert!(
            k.contains(" TJ"),
            "a ragged line that kerns must carry the kern as TJ, got:\n{k}"
        );
    }

    #[test]
    fn justified_line_emits_positioned_tj_with_adjustment() {
        // Two gaps, +3 pt each. amount = -1000 · 3 / FONT_SIZE_PT(10) = -300, an integer → "-300".
        // The positive space-add is a *negative* TJ number (the amount is subtracted from position).
        let bytes = render_justified_line("ab cd ef", 3.0);
        let s = String::from_utf8_lossy(&bytes);
        assert!(s.contains("TJ"), "expected a positioned TJ, got:\n{s}");
        assert!(
            s.contains("-300"),
            "expected the -300 TJ adjustment, got:\n{s}"
        );
    }
}
