//! Layout engine: turns a document into positioned pages.
//!
//! The real engine is incremental and dependency-tracked (see the plan) so that editing one
//! text thread reflows only affected pages. This scaffold lays content out naively — stacking
//! text and image blocks, paginating when a block would exceed the page height — so downstream
//! crates compile and the export pipeline has something to consume. Uses `quill-text-layout`
//! for line breaking.

use quill_core_model::{Block, Color, Document, Rect};
use quill_text_layout::greedy_break;

/// A block positioned on a page.
#[derive(Debug, Clone, PartialEq)]
pub enum PlacedBlock {
    Text {
        frame: Rect,
        lines: Vec<String>,
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

/// Rough stand-in for glyph advance until real shaping lands, in points per character.
const APPROX_CHAR_WIDTH_PT: f32 = 6.0;
/// Rough stand-in for line height, in points.
const APPROX_LINE_HEIGHT_PT: f32 = 12.0;

/// Lay a document out into pages. Paginates: starts a new page when a block would push `y`
/// past `doc.page_setup.trim.h_pt`. Returns at least one page (even if the document is empty).
pub fn lay_out(doc: &Document) -> Vec<LaidOutPage> {
    let width = doc.page_setup.trim.w_pt;
    let page_h = doc.page_setup.trim.h_pt;
    let max_chars = (width / APPROX_CHAR_WIDTH_PT).max(1.0) as usize;

    let mut pages: Vec<LaidOutPage> = Vec::new();
    let mut page = LaidOutPage::default();
    let mut y: f32 = 0.0;

    for block in &doc.content {
        match block {
            Block::Heading { text, color, .. } | Block::Body { text, color, .. } => {
                let lines = greedy_break(text, max_chars);
                let height = lines.len() as f32 * APPROX_LINE_HEIGHT_PT;

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
                let Some(_asset_rec) = doc.assets.iter().find(|a| &a.id == asset) else {
                    continue;
                };

                // Asset lacks pixel-dimension fields (px_w / px_h), so we cannot compute the
                // true placed size via `w_pt = px_w / dpi * 72.0`. Real decode / sizing lands in
                // the export-pdf PR. For now we use the full content width and treat the image as
                // square (unknown aspect ratio).
                let w = width;
                let h = w; // square placeholder — no aspect ratio available yet

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

    #[test]
    fn lays_out_sample_into_one_page() {
        // Document::sample() has 2 short text blocks + asset "map1" (referenced by no Block::Image
        // in the sample, so no image block is placed). Content fits well within one page.
        let pages = lay_out(&Document::sample());
        assert!(!pages.is_empty());
        assert!(!pages[0].blocks.is_empty());
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
        // Each Body block produces 1 line = APPROX_LINE_HEIGHT_PT (12 pt).
        // Page height is 648 pt → 54 lines fit. Push 100 blocks to guarantee overflow.
        let blocks: Vec<Block> = (0..100)
            .map(|i| Block::Body {
                text: format!("Line {i}"),
                color: Color::Gray { v: 0.0 },
            })
            .collect();
        let doc = doc_with_blocks(blocks);
        let pages = lay_out(&doc);
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
        let lines_per_page = (page_h / APPROX_LINE_HEIGHT_PT).floor() as usize; // 54

        let make_block = |i: usize| Block::Body {
            text: format!("L{i}"),
            color: Color::Gray { v: 0.0 },
        };

        // Exactly lines_per_page blocks → fits on one page.
        let exact_blocks: Vec<Block> = (0..lines_per_page).map(make_block).collect();
        let doc_exact = doc_with_blocks(exact_blocks);
        let pages_exact = lay_out(&doc_exact);
        assert_eq!(
            pages_exact.len(),
            1,
            "expected 1 page for exact fit, got {}",
            pages_exact.len()
        );

        // One extra block → must spill to a second page.
        let overflow_blocks: Vec<Block> = (0..=lines_per_page).map(make_block).collect();
        let doc_overflow = doc_with_blocks(overflow_blocks);
        let pages_overflow = lay_out(&doc_overflow);
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
                dpi: 300.0,
                line_art: false,
            }],
            fonts_embeddable: false,
        };

        let pages = lay_out(&doc);

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
            PlacedBlock::Image { asset_id: id, .. } => {
                assert_eq!(id, &asset_id);
            }
            other => panic!("expected Image block, got {other:?}"),
        }
    }
}
