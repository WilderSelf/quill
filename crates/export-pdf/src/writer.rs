//! Assembles the PDF/X-1a:2001 object graph (writer §1-4).
//!
//! Consumes laid-out pages plus the document and writes bytes: catalog + pages tree, one page per
//! `LaidOutPage` (`MediaBox == BleedBox`, centered `TrimBox`), an embedded subset font, grayscale
//! image XObjects, the ICC `OutputIntent`, and the XMP identification packet. PDF/X-1a pins the
//! header to **PDF 1.3**.

use std::collections::{BTreeMap, BTreeSet};
use std::io::Write;
use std::path::Path;

use pdf_writer::types::{CidFontType, FontFlags, SystemInfo, TrappingStatus};
use pdf_writer::writers::OutputIntent;
use pdf_writer::{Content, Filter, Finish, Name, Pdf, Rect, Ref, Str, TextStr};

use quill_core_model::{Color, Document};
use quill_layout_engine::{LaidOutPage, PlacedBlock};

use crate::{fonts, geom, icc, images, ExportError, ExportOptions};

/// Body/heading font size and line advance, in points. Mirrors the layout engine's line-height
/// stand-in so glyphs land on the rows the layout reserved for them.
const FONT_SIZE_PT: f32 = 10.0;
const LINE_HEIGHT_PT: f32 = 12.0;

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
    out: &mut impl Write,
) -> Result<(), ExportError> {
    // --- Inputs: font subset, ICC bytes, images -------------------------------------------
    let used_chars = collect_chars(pages);
    let font = fonts::build(&used_chars)?;

    let icc_bytes = std::fs::read(&opts.output_intent_icc)
        .map_err(|e| ExportError::Icc(format!("reading '{}': {e}", opts.output_intent_icc)))?;
    icc::check_icc(&icc_bytes).map_err(ExportError::Icc)?;

    // Decode each distinct placed image once (missing/unsupported → skipped).
    let base_dir = Path::new(".");
    let mut images_by_id: BTreeMap<String, ImageObj> = BTreeMap::new();
    for page in pages {
        for block in &page.blocks {
            if let PlacedBlock::Image { asset_id, .. } = block {
                if images_by_id.contains_key(asset_id) {
                    continue;
                }
                if let Some(asset) = doc.assets.iter().find(|a| &a.id == asset_id) {
                    if let Some(img) = images::resolve(asset, base_dir) {
                        let idx = images_by_id.len();
                        images_by_id.insert(
                            asset_id.clone(),
                            ImageObj {
                                name: format!("Im{idx}"),
                                gray: img.gray,
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
    let icc_id = alloc.bump();
    let type0_id = alloc.bump();
    let cid_id = alloc.bump();
    let descriptor_id = alloc.bump();
    let fontfile_id = alloc.bump();
    for img in images_by_id.values_mut() {
        img.id = alloc.bump();
    }
    let page_refs: Vec<(Ref, Ref)> = pages.iter().map(|_| (alloc.bump(), alloc.bump())).collect();

    // --- Build the document ---------------------------------------------------------------
    let mut pdf = Pdf::new();
    pdf.set_version(1, 3); // PDF/X-1a:2001 == PDF 1.3

    // Catalog + OutputIntent + XMP reference.
    {
        let mut cat = pdf.catalog(catalog_id);
        cat.pages(page_tree_id);
        cat.metadata(xmp_id);
        let mut intents = cat.output_intents();
        write_output_intent(intents.push(), icc_id);
        intents.finish();
        cat.finish();
    }

    // Pages tree.
    {
        let mut tree = pdf.pages(page_tree_id);
        tree.kids(page_refs.iter().map(|(p, _)| *p));
        tree.count(page_refs.len() as i32);
        tree.finish();
    }

    // Document info (title/creator/producer + PDF/X identification keys, mirrored in XMP).
    {
        let mut info = pdf.document_info(info_id);
        info.title(TextStr(&doc.metadata.title));
        info.creator(TextStr("Quill"));
        info.producer(TextStr("Quill export-pdf"));
        info.trapped(TrappingStatus::NotTrapped);
        info.pair(Name(b"GTS_PDFXVersion"), Str(b"PDF/X-1a:2001"));
        info.pair(Name(b"GTS_PDFXConformance"), Str(b"PDF/X-1a:2001"));
        info.finish();
    }

    // XMP metadata packet (uncompressed).
    let id_hex = doc_id_hex(doc);
    {
        let xmp = crate::xmp::build_xmp(&doc.metadata.title, &id_hex, &id_hex);
        let mut s = pdf.stream(xmp_id, &xmp);
        s.pair(Name(b"Type"), Name(b"Metadata"));
        s.pair(Name(b"Subtype"), Name(b"XML"));
        s.finish();
    }

    // ICC profile stream (DestOutputProfile, /N 4).
    {
        let mut icc_stream = pdf.icc_profile(icc_id, &icc_bytes);
        icc_stream.n(4);
        icc_stream.finish();
    }

    write_font(
        &mut pdf,
        &font,
        type0_id,
        cid_id,
        descriptor_id,
        fontfile_id,
    );

    // Image XObjects.
    for img in images_by_id.values() {
        let compressed = deflate(&img.gray);
        let mut xobj = pdf.image_xobject(img.id, &compressed);
        xobj.width(img.width as i32);
        xobj.height(img.height as i32);
        xobj.color_space().device_gray();
        xobj.bits_per_component(8);
        xobj.filter(Filter::FlateDecode);
        xobj.finish();
    }

    // Pages.
    for (i, (page, (page_id, content_id))) in pages.iter().zip(&page_refs).enumerate() {
        let g = geom::page_geom(&doc.page_setup, i);
        let content = render_page(page, &g, &font, &images_by_id);

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
            res.fonts().pair(Name(b"F0"), type0_id);
            // Only the images actually placed on this page.
            let mut xo = res.x_objects();
            let mut seen = BTreeSet::new();
            for block in &page.blocks {
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
            p.finish();
        }

        pdf.stream(*content_id, &content).finish();
    }

    // Trailer file identifier (deterministic → reproducible golden output).
    let id_bytes = doc_id_bytes(doc).to_vec();
    pdf.set_file_id((id_bytes.clone(), id_bytes));

    out.write_all(&pdf.finish())?;
    Ok(())
}

/// A decoded image plus its assigned reference and content-stream name.
struct ImageObj {
    name: String,
    gray: Vec<u8>,
    width: u32,
    height: u32,
    id: Ref,
}

/// Every character used by text blocks across all pages.
fn collect_chars(pages: &[LaidOutPage]) -> BTreeSet<char> {
    let mut set = BTreeSet::new();
    for page in pages {
        for block in &page.blocks {
            if let PlacedBlock::Text { lines, .. } = block {
                for line in lines {
                    set.extend(line.chars());
                }
            }
        }
    }
    set
}

/// Write the `/OutputIntent` dict. The identifier is `Custom`, so `/Info` is required.
fn write_output_intent(mut oi: OutputIntent<'_>, icc_id: Ref) {
    oi.subtype(pdf_writer::types::OutputIntentSubtype::PDFX);
    oi.output_condition_identifier(TextStr("Custom"));
    oi.info(TextStr("Quill synthetic CMYK output intent"));
    oi.dest_output_profile(icc_id);
    oi.finish();
}

/// Emit the Type0 → CIDFontType2 → FontDescriptor → FontFile2 chain.
fn write_font(
    pdf: &mut Pdf,
    font: &fonts::EmbeddedFont,
    type0_id: Ref,
    cid_id: Ref,
    descriptor_id: Ref,
    fontfile_id: Ref,
) {
    let base = font.base_font.as_bytes();
    {
        let mut t0 = pdf.type0_font(type0_id);
        t0.base_font(Name(base));
        t0.encoding_predefined(Name(b"Identity-H"));
        t0.descendant_font(cid_id);
        t0.finish();
    }
    {
        let mut cid = pdf.cid_font(cid_id);
        cid.subtype(CidFontType::Type2);
        cid.base_font(Name(base));
        cid.system_info(SystemInfo {
            registry: Str(b"Adobe"),
            ordering: Str(b"Identity"),
            supplement: 0,
        });
        cid.font_descriptor(descriptor_id);
        cid.cid_to_gid_map_predefined(Name(b"Identity"));
        cid.widths().consecutive(0, font.widths.iter().copied());
        cid.finish();
    }
    {
        let mut fd = pdf.font_descriptor(descriptor_id);
        fd.name(Name(base));
        fd.flags(FontFlags::SERIF | FontFlags::NON_SYMBOLIC);
        fd.bbox(Rect::new(
            font.bbox[0],
            font.bbox[1],
            font.bbox[2],
            font.bbox[3],
        ));
        fd.italic_angle(0.0);
        fd.ascent(font.ascent);
        fd.descent(font.descent);
        fd.cap_height(font.cap_height);
        fd.stem_v(font.stem_v);
        fd.font_file2(fontfile_id);
        fd.finish();
    }
    {
        let compressed = deflate(&font.subset);
        let mut s = pdf.stream(fontfile_id, &compressed);
        s.filter(Filter::FlateDecode);
        s.pair(Name(b"Length1"), font.subset.len() as i32);
        s.finish();
    }
}

/// Build one page's content stream: text (black) and image XObjects, y-flipped into PDF space.
fn render_page(
    page: &LaidOutPage,
    g: &geom::PageGeom,
    font: &fonts::EmbeddedFont,
    images_by_id: &BTreeMap<String, ImageObj>,
) -> Vec<u8> {
    let mut content = Content::new();
    for block in &page.blocks {
        match block {
            PlacedBlock::Text {
                frame,
                lines,
                color,
            } => {
                content.begin_text();
                content.set_font(Name(b"F0"), FONT_SIZE_PT);
                // Emit the authored press-legal fill color. Preflight rejects RGB before export,
                // so `Rgb` is unreachable here; fall back to black (rather than panicking) so a
                // `--force` export can never abort mid-stream.
                match color {
                    Color::Gray { v } => content.set_fill_gray(*v),
                    Color::Cmyk { c, m, y, k } => content.set_fill_cmyk(*c, *m, *y, *k),
                    Color::Rgb { .. } => content.set_fill_gray(0.0),
                };
                let ascent = font.ascent_pt(FONT_SIZE_PT);
                for (li, line) in lines.iter().enumerate() {
                    let top_y = frame.y_pt + ascent + li as f32 * LINE_HEIGHT_PT;
                    let (x, y) = g.flip(frame.x_pt, top_y);
                    // Absolute text matrix per line (avoids relative-Td bookkeeping).
                    content.set_text_matrix([1.0, 0.0, 0.0, 1.0, x, y]);
                    let encoded = font.encode_line(line);
                    content.show(Str(&encoded));
                }
                content.end_text();
            }
            PlacedBlock::Image { frame, asset_id } => {
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
        }
    }
    content.finish().as_slice().to_vec()
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
    use quill_core_model::PageSetup;

    /// Build a one-page layout holding a single text block with the given fill color, render it,
    /// and return the (uncompressed) content-stream bytes as a lossy string. Unlike the finished
    /// PDF, `render_page` output is not FlateDecode'd, so fill operators are directly greppable.
    fn render_text_color(color: Color) -> String {
        let setup = PageSetup::default();
        let g = geom::page_geom(&setup, 0);
        let mut chars = BTreeSet::new();
        chars.insert('H');
        chars.insert('i');
        let font = fonts::build(&chars).expect("build bundled font");
        let page = LaidOutPage {
            blocks: vec![PlacedBlock::Text {
                frame: quill_core_model::Rect {
                    x_pt: 0.0,
                    y_pt: 0.0,
                    w_pt: setup.trim.w_pt,
                    h_pt: LINE_HEIGHT_PT,
                },
                lines: vec!["Hi".to_string()],
                color,
            }],
        };
        let content = render_page(&page, &g, &font, &BTreeMap::new());
        String::from_utf8_lossy(&content).into_owned()
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
}
