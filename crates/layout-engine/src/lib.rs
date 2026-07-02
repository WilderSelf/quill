//! Layout engine: turns a document into positioned pages.
//!
//! The real engine is incremental and dependency-tracked (see the plan) so that editing one
//! text thread reflows only affected pages. This scaffold lays content out naively — stacking
//! text and image blocks, paginating when a block would exceed the page height — so downstream
//! crates compile and the export pipeline has something to consume. Uses `quill-text-layout`
//! for line breaking.

use quill_core_model::{Asset, Block, Color, Document, Rect};
use quill_text_layout::{
    justify_paragraph, Alignment, Line, RunMetrics, BODY_FONT_SIZE_PT, BODY_LINE_HEIGHT_PT,
};

/// Compute an image's placed size in points from its pixel dimensions and DPI, preserving aspect
/// ratio and scaling down to fit `content_width` when the natural width is wider. See spec 0009.
///
/// Falls back to a square at `content_width` when pixel dimensions or DPI are unknown (`0`), so
/// documents authored before pixel info was captured still lay out.
fn image_size(asset: &Asset, content_width: f32) -> (f32, f32) {
    if asset.px_w == 0 || asset.px_h == 0 || asset.dpi <= 0.0 {
        return (content_width, content_width); // legacy square placeholder
    }
    let natural_w = asset.px_w as f32 / asset.dpi * 72.0;
    let natural_h = asset.px_h as f32 / asset.dpi * 72.0;
    if natural_w > content_width {
        let scale = content_width / natural_w;
        (content_width, natural_h * scale)
    } else {
        (natural_w, natural_h)
    }
}

/// A block positioned on a page.
#[derive(Debug, Clone, PartialEq)]
pub enum PlacedBlock {
    Text {
        frame: Rect,
        /// Broken lines, each carrying its inter-word justification adjustment (spec 0017 incr. 2).
        lines: Vec<Line>,
        color: Color,
    },
    Image {
        frame: Rect,
        asset_id: String,
    },
}

/// A laid-out page.
#[derive(Debug, Clone, PartialEq, Default)]
pub struct LaidOutPage {
    pub blocks: Vec<PlacedBlock>,
}

/// Lay a document out into pages. Paginates: starts a new page when a block would push `y`
/// past `doc.page_setup.trim.h_pt`. Returns at least one page (even if the document is empty).
///
/// Text is broken to fit the frame width using the caller-supplied `metrics` (the embedded font in
/// the export path) at [`BODY_FONT_SIZE_PT`] — see `specs/0015-text-metrics-line-breaking.md` and
/// spec 0016 for the shift to run-based measurement.
pub fn lay_out(doc: &Document, metrics: &impl RunMetrics) -> Vec<LaidOutPage> {
    let width = doc.page_setup.trim.w_pt;
    let page_h = doc.page_setup.trim.h_pt;

    let mut pages: Vec<LaidOutPage> = Vec::new();
    let mut page = LaidOutPage::default();
    let mut y: f32 = 0.0;

    for block in &doc.content {
        match block {
            Block::Heading { text, color, .. } | Block::Body { text, color, .. } => {
                // Body text is justified for press-quality even spacing; headings stay ragged-left
                // (a heading is typically one short line, where justification would do nothing
                // anyway — its single line is the paragraph's last, which is never justified).
                let align = match block {
                    Block::Heading { .. } => Alignment::Left,
                    _ => Alignment::Justified,
                };
                let lines = justify_paragraph(text, width, BODY_FONT_SIZE_PT, align, metrics);
                let height = lines.len() as f32 * BODY_LINE_HEIGHT_PT;

                // If this block doesn't fit (and the page already has content), start a new page.
                if y + height > page_h && !page.blocks.is_empty() {
                    pages.push(page);
                    page = LaidOutPage::default();
                    y = 0.0;
                }

                page.blocks.push(PlacedBlock::Text {
                    frame: Rect {
                        x_pt: 0.0,
                        y_pt: y,
                        w_pt: width,
                        h_pt: height,
                    },
                    lines,
                    color: *color,
                });
                y += height;
            }
            Block::Image { asset } => {
                // Resolve the asset id. If not found, skip this block (no panic).
                let Some(asset_rec) = doc.assets.iter().find(|a| &a.id == asset) else {
                    continue;
                };

                // Size the image from its pixel dimensions and DPI at its true aspect ratio,
                // scaling down to fit the content width when wider. See spec 0009.
                let (w, h) = image_size(asset_rec, width);

                if y + h > page_h && !page.blocks.is_empty() {
                    pages.push(page);
                    page = LaidOutPage::default();
                    y = 0.0;
                }

                page.blocks.push(PlacedBlock::Image {
                    frame: Rect {
                        x_pt: 0.0,
                        y_pt: y,
                        w_pt: w,
                        h_pt: h,
                    },
                    asset_id: asset.clone(),
                });
                y += h;
            }
        }
    }

    // Always emit the last (possibly empty) page so callers receive >= 1 page.
    pages.push(page);
    pages
}

#[cfg(test)]
mod tests {
    use super::*;
    use quill_core_model::{Asset, Block, Color, Document, Metadata, PageSetup, Size};
    use quill_text_layout::MonospaceRunMetrics;

    /// 0.6 em × 10 pt = 6 pt/char, matching the old `APPROX_CHAR_WIDTH_PT` stand-in so these
    /// pagination tests keep their familiar per-character arithmetic.
    const MONO: MonospaceRunMetrics = MonospaceRunMetrics { em_ratio: 0.6 };

    #[test]
    fn lays_out_sample_into_one_page() {
        // Document::sample() has 2 short text blocks + asset "map1" (referenced by no Block::Image
        // in the sample, so no image block is placed). Content fits well within one page.
        let pages = lay_out(&Document::sample(), &MONO);
        assert!(!pages.is_empty());
        assert!(!pages[0].blocks.is_empty());
    }

    /// The lines of the first `PlacedBlock::Text` found across `pages`.
    fn first_text_lines(pages: &[LaidOutPage]) -> Vec<Line> {
        pages
            .iter()
            .flat_map(|p| &p.blocks)
            .find_map(|b| match b {
                PlacedBlock::Text { lines, .. } => Some(lines.clone()),
                _ => None,
            })
            .expect("a text block")
    }

    #[test]
    fn body_is_justified_headings_are_ragged() {
        // A paragraph long enough to wrap under the 432 pt frame (72 chars/line at 6 pt/char under
        // MONO) exercises the alignment wiring: as a Body it is justified (its underfull interior
        // line stretches — non-zero adjust — while the last line stays ragged); the identical text
        // as a Heading stays fully ragged (Alignment::Left). Spec 0017 increment 2.
        let words =
            "goblins raid the village at dusk stealing grain and copper coins from every trembling home nearby";
        let body = doc_with_blocks(vec![Block::Body {
            text: words.into(),
            color: Color::Gray { v: 0.0 },
        }]);
        let heading = doc_with_blocks(vec![Block::Heading {
            level: 1,
            text: words.into(),
            color: Color::Gray { v: 0.0 },
        }]);

        let body_lines = first_text_lines(&lay_out(&body, &MONO));
        let heading_lines = first_text_lines(&lay_out(&heading, &MONO));

        assert!(body_lines.len() >= 2, "body should wrap to >= 2 lines");
        assert!(
            body_lines.iter().any(|l| l.space_adjust_pt != 0.0),
            "a justified body line should carry a non-zero adjustment"
        );
        assert_eq!(
            body_lines.last().unwrap().space_adjust_pt,
            0.0,
            "the paragraph's last line stays ragged"
        );
        assert!(
            heading_lines.iter().all(|l| l.space_adjust_pt == 0.0),
            "headings are ragged-left (never justified)"
        );
    }

    /// Build a minimal document from scratch with the given content blocks and default page setup.
    fn doc_with_blocks(content: Vec<Block>) -> Document {
        Document {
            format_version: quill_core_model::FORMAT_VERSION,
            metadata: Metadata::default(),
            page_setup: PageSetup::default(), // 432 × 648 pt (6×9 in)
            content,
            assets: vec![],
            fonts_embeddable: false,
        }
    }

    #[test]
    fn paginates_when_content_overflows() {
        // Each Body block produces 1 line = BODY_LINE_HEIGHT_PT (12 pt).
        // Page height is 648 pt → 54 lines fit. Push 100 blocks to guarantee overflow.
        let blocks: Vec<Block> = (0..100)
            .map(|i| Block::Body {
                text: format!("Line {i}"),
                color: Color::Gray { v: 0.0 },
            })
            .collect();
        let doc = doc_with_blocks(blocks);
        let pages = lay_out(&doc, &MONO);
        assert!(
            pages.len() >= 2,
            "expected at least 2 pages, got {}",
            pages.len()
        );
    }

    #[test]
    fn boundary_exactly_one_page_then_spills() {
        // Compute how many single-line blocks fill one page exactly.
        let page_h = PageSetup::default().trim.h_pt; // 648.0
        let lines_per_page = (page_h / BODY_LINE_HEIGHT_PT).floor() as usize; // 54

        let make_block = |i: usize| Block::Body {
            text: format!("L{i}"),
            color: Color::Gray { v: 0.0 },
        };

        // Exactly lines_per_page blocks → fits on one page.
        let exact_blocks: Vec<Block> = (0..lines_per_page).map(make_block).collect();
        let doc_exact = doc_with_blocks(exact_blocks);
        let pages_exact = lay_out(&doc_exact, &MONO);
        assert_eq!(
            pages_exact.len(),
            1,
            "expected 1 page for exact fit, got {}",
            pages_exact.len()
        );

        // One extra block → must spill to a second page.
        let overflow_blocks: Vec<Block> = (0..=lines_per_page).map(make_block).collect();
        let doc_overflow = doc_with_blocks(overflow_blocks);
        let pages_overflow = lay_out(&doc_overflow, &MONO);
        assert!(
            pages_overflow.len() >= 2,
            "expected >= 2 pages after overflow, got {}",
            pages_overflow.len()
        );
    }

    #[test]
    fn places_image_block() {
        let asset_id = "img1".to_string();
        let doc = Document {
            format_version: quill_core_model::FORMAT_VERSION,
            metadata: Metadata::default(),
            page_setup: PageSetup {
                trim: Size {
                    w_pt: 432.0,
                    h_pt: 648.0,
                },
                ..PageSetup::default()
            },
            content: vec![
                Block::Image {
                    asset: asset_id.clone(),
                },
                // Unknown asset — should be silently skipped.
                Block::Image {
                    asset: "unknown-asset-xyz".to_string(),
                },
            ],
            assets: vec![Asset {
                id: asset_id.clone(),
                path: "assets/img1.png".into(),
                // 900×600 px at 300 dpi → natural 216×144 pt, both within the 432 pt content
                // width, so placed at natural size with a 1.5 aspect ratio (not a square).
                px_w: 900,
                px_h: 600,
                dpi: 300.0,
                line_art: false,
                has_alpha: false,
            }],
            fonts_embeddable: false,
        };

        let pages = lay_out(&doc, &MONO);

        // Collect all image blocks across all pages.
        let image_blocks: Vec<&PlacedBlock> = pages
            .iter()
            .flat_map(|p| p.blocks.iter())
            .filter(|b| matches!(b, PlacedBlock::Image { .. }))
            .collect();

        assert_eq!(
            image_blocks.len(),
            1,
            "expected exactly 1 image block (unknown asset skipped)"
        );

        match &image_blocks[0] {
            PlacedBlock::Image {
                asset_id: id,
                frame,
            } => {
                assert_eq!(id, &asset_id);
                assert!((frame.w_pt - 216.0).abs() < 0.01, "w = {}", frame.w_pt);
                assert!((frame.h_pt - 144.0).abs() < 0.01, "h = {}", frame.h_pt);
            }
            other => panic!("expected Image block, got {other:?}"),
        }
    }

    fn sized_asset(px_w: u32, px_h: u32, dpi: f32) -> Asset {
        Asset {
            id: "a".into(),
            path: "a.png".into(),
            px_w,
            px_h,
            dpi,
            line_art: false,
            has_alpha: false,
        }
    }

    #[test]
    fn wide_image_scales_down_to_content_width_preserving_aspect() {
        let content_width = 432.0;
        // 4000×2000 px at 300 dpi → natural 960×480 pt (wider than 432) → scaled to width 432,
        // height 216, keeping the 2:1 aspect ratio.
        let (w, h) = image_size(&sized_asset(4000, 2000, 300.0), content_width);
        assert!((w - content_width).abs() < 0.01, "w = {w}");
        assert!((h - 216.0).abs() < 0.01, "h = {h}");
        assert!((w / h - 2.0).abs() < 0.001, "aspect = {}", w / h);
    }

    #[test]
    fn small_image_placed_at_natural_size() {
        // 300×450 px at 300 dpi → 72×108 pt, both within the content width → natural size.
        let (w, h) = image_size(&sized_asset(300, 450, 300.0), 432.0);
        assert!((w - 72.0).abs() < 0.01, "w = {w}");
        assert!((h - 108.0).abs() < 0.01, "h = {h}");
    }

    #[test]
    fn missing_pixel_dims_fall_back_to_square() {
        let content_width = 432.0;
        let (w, h) = image_size(&sized_asset(0, 0, 300.0), content_width);
        assert_eq!(w, content_width);
        assert_eq!(h, content_width);
    }
}
