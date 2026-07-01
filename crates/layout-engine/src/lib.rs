//! Layout engine: turns a document into positioned pages.
//!
//! The real engine is incremental and dependency-tracked (see the plan) so that editing one
//! text thread reflows only affected pages. This scaffold lays content out naively — one page,
//! stacking text blocks — so downstream crates compile and the export pipeline has something
//! to consume. Uses `quill-text-layout` for line breaking.

use quill_core_model::{Block, Document, Rect};
use quill_text_layout::greedy_break;

/// A block positioned on a page.
#[derive(Debug, Clone, PartialEq)]
pub struct PlacedBlock {
    pub frame: Rect,
    pub lines: Vec<String>,
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

/// Lay a document out into pages. Placeholder: a single page stacking text blocks vertically.
pub fn lay_out(doc: &Document) -> Vec<LaidOutPage> {
    let width = doc.page_setup.trim.w_pt;
    let max_chars = (width / APPROX_CHAR_WIDTH_PT).max(1.0) as usize;

    let mut page = LaidOutPage::default();
    let mut y = 0.0;
    for block in &doc.content {
        let text = match block {
            Block::Heading { text, .. } | Block::Body { text, .. } => text.as_str(),
            Block::Image { .. } => continue,
        };
        let lines = greedy_break(text, max_chars);
        let height = lines.len() as f32 * APPROX_LINE_HEIGHT_PT;
        page.blocks.push(PlacedBlock {
            frame: Rect {
                x_pt: 0.0,
                y_pt: y,
                w_pt: width,
                h_pt: height,
            },
            lines,
        });
        y += height;
    }
    vec![page]
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn lays_out_sample_into_one_page() {
        let pages = lay_out(&Document::sample());
        assert_eq!(pages.len(), 1);
        assert!(!pages[0].blocks.is_empty());
    }
}
