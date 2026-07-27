//! The editor shell — see `specs/0034-egui-app-shell.md`.
//!
//! ## Why the shell is a library
//!
//! Everything interesting about an editor — opening a document, mapping the viewport, deciding
//! which pages to paint, routing an edit into incremental relayout — is logic, not windowing. Kept
//! in a library it is ordinary unit-testable code that runs on every leg of the CI matrix with no
//! display server, no GPU and no system libraries. The binary is a thin launcher behind a
//! non-default `gui` feature.
//!
//! That split is what lets the shell's *behavior* be gated by CI at all. A shell that only existed
//! inside `eframe::run_native` could be verified by compiling and nothing else.
//!
//! ## What the shell must not do
//!
//! Paint 500 pages. `CLAUDE.md`'s central constraint is that a 500-page art-heavy book stays
//! smooth, and a viewport that walks the whole document each frame violates it in the shell rather
//! than in the engine. [`AppState::visible_pages`] bounds painting to what is on screen, and
//! [`AppState::apply_edit`] repaints only the pages spec 0031 reports as changed.

use std::path::{Path, PathBuf};

use quill_core_model::{page_geom, Block, BlockId, Document, LoadError, Tpub};
use quill_fonts::Font;
use quill_layout_engine::{LaidOutPage, LayoutSession, LayoutStats};
use quill_render::{paint_page, PaintOp, PopulateReport, ProxyCache};
use quill_text_layout::NoHyphenator;

/// Gap between pages in the scrolling viewport, in points.
pub const PAGE_GAP_PT: f32 = 24.0;

/// How the document is currently being viewed.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Viewport {
    /// Scroll offset from the top of the document, in points.
    pub scroll_pt: f32,
    /// Height of the visible area, in points.
    pub height_pt: f32,
    /// Pixels per point.
    pub zoom: f32,
}

impl Default for Viewport {
    fn default() -> Self {
        Viewport {
            scroll_pt: 0.0,
            height_pt: 800.0,
            zoom: 1.0,
        }
    }
}

/// What happened when the document changed.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct EditOutcome {
    pub stats: LayoutStats,
    /// Pages that need repainting — exactly spec 0031's `changed_pages`, no more.
    pub repaint: Vec<usize>,
}

/// The editor's state: one open document and how it is being looked at.
pub struct AppState {
    doc: Document,
    asset_root: PathBuf,
    font: Font,
    session: LayoutSession,
    pages: Vec<LaidOutPage>,
    proxies: ProxyCache,
    populate: PopulateReport,
    pub viewport: Viewport,
    /// Counts every page painted, so a test can prove the viewport is virtualized.
    painted_pages: usize,
}

impl AppState {
    /// Open a `.tpub` container, extracting alongside it.
    ///
    /// A document with a broken linked asset still opens: `populate_from_assets` reports it as
    /// skipped and the page renders without that image. Refusing to open a 500-page book because
    /// one asset moved would be the wrong trade on screen — see spec 0033.
    pub fn open(path: &Path) -> Result<AppState, LoadError> {
        let opened = Tpub::open_into(path, &path.with_extension("tpub.d"))?;
        Ok(AppState::from_document(opened.document, opened.asset_root))
    }

    /// Open the built-in sample, for a first run with no file.
    pub fn sample() -> AppState {
        AppState::from_document(Document::sample(), PathBuf::from("."))
    }

    pub fn from_document(doc: Document, asset_root: PathBuf) -> AppState {
        let mut state = AppState {
            doc,
            asset_root,
            font: Font::bundled(),
            session: LayoutSession::new(),
            pages: Vec::new(),
            proxies: ProxyCache::new(),
            populate: PopulateReport::default(),
            viewport: Viewport::default(),
            painted_pages: 0,
        };
        state.populate = state
            .proxies
            .populate_from_assets(&state.doc.assets, &state.asset_root);
        let result = state
            .session
            .relayout(&state.doc, &state.font, &NoHyphenator);
        state.pages = result.pages;
        state
    }

    pub fn document(&self) -> &Document {
        &self.doc
    }

    pub fn pages(&self) -> &[LaidOutPage] {
        &self.pages
    }

    pub fn page_count(&self) -> usize {
        self.pages.len()
    }

    /// What loading the linked assets found — how many proxies were generated, and how many links
    /// could not be resolved.
    pub fn asset_report(&self) -> &PopulateReport {
        &self.populate
    }

    pub fn painted_pages(&self) -> usize {
        self.painted_pages
    }

    /// Height of one page including the gap below it, in points.
    fn page_stride(&self) -> f32 {
        let geom = page_geom(&self.doc.page_setup, 0);
        geom.media_h + PAGE_GAP_PT
    }

    /// Total scrollable height, in points.
    pub fn document_height_pt(&self) -> f32 {
        (self.page_count() as f32 * self.page_stride() - PAGE_GAP_PT).max(0.0)
    }

    /// Top of `page_index` in document space, in points.
    pub fn page_top_pt(&self, page_index: usize) -> f32 {
        page_index as f32 * self.page_stride()
    }

    /// Map a point in a page's own space to document space, then to screen pixels.
    pub fn doc_to_screen(&self, page_index: usize, x_pt: f32, y_pt: f32) -> (f32, f32) {
        let doc_y = self.page_top_pt(page_index) + y_pt;
        (
            x_pt * self.viewport.zoom,
            (doc_y - self.viewport.scroll_pt) * self.viewport.zoom,
        )
    }

    /// The inverse of [`doc_to_screen`](Self::doc_to_screen).
    pub fn screen_to_doc(&self, page_index: usize, x_px: f32, y_px: f32) -> (f32, f32) {
        let doc_y = y_px / self.viewport.zoom + self.viewport.scroll_pt;
        (
            x_px / self.viewport.zoom,
            doc_y - self.page_top_pt(page_index),
        )
    }

    /// The range of pages intersecting the viewport.
    ///
    /// This is the whole reason a 500-page document can be scrolled at all: without it the shell
    /// would paint every page every frame, and the performance constraint would be violated by the
    /// UI rather than by the engine.
    pub fn visible_pages(&self) -> std::ops::Range<usize> {
        if self.pages.is_empty() {
            return 0..0;
        }
        let stride = self.page_stride().max(f32::EPSILON);
        let first = (self.viewport.scroll_pt / stride).floor().max(0.0) as usize;
        let last = ((self.viewport.scroll_pt + self.viewport.height_pt) / stride).floor() as usize;
        let first = first.min(self.pages.len() - 1);
        let last = (last + 1).min(self.pages.len());
        first..last.max(first + 1)
    }

    /// Build the paint list for everything currently on screen.
    pub fn paint_visible(&mut self) -> Vec<PaintOp> {
        let range = self.visible_pages();
        let mut ops = Vec::new();
        for index in range {
            let geom = page_geom(&self.doc.page_setup, index);
            ops.extend(paint_page(
                &self.pages[index],
                &geom,
                &self.font,
                &self.proxies,
            ));
            self.painted_pages += 1;
        }
        ops
    }

    /// Scroll so `page_index` is at the top of the viewport.
    pub fn scroll_to_page(&mut self, page_index: usize) {
        self.viewport.scroll_pt = self.page_top_pt(page_index);
    }

    /// Replace a block's text and re-flow incrementally.
    ///
    /// Routes through [`LayoutSession`] rather than a full layout, and reports exactly which pages
    /// changed so the canvas repaints those and nothing else.
    pub fn edit_text(&mut self, id: BlockId, text: impl Into<String>) -> EditOutcome {
        let text = text.into();
        for block in &mut self.doc.content {
            if block.id() == id {
                let replacement = match block {
                    Block::Heading {
                        level,
                        color,
                        style,
                        ..
                    } => {
                        let mut b = Block::heading(*level, text.clone(), *color);
                        if let Some(name) = style.clone() {
                            b = b.with_style(name);
                        }
                        b
                    }
                    Block::Body { color, style, .. } => {
                        let mut b = Block::body(text.clone(), *color);
                        if let Some(name) = style.clone() {
                            b = b.with_style(name);
                        }
                        b
                    }
                    // An image block has no text to edit; leave it alone rather than replacing it
                    // with something the caller did not ask for.
                    Block::Image { .. } => return EditOutcome::default(),
                };
                *block = replacement;
                block.set_id(id);
                break;
            }
        }
        self.doc.bump_revision();
        self.relayout()
    }

    /// Re-flow after an external mutation of the document.
    pub fn relayout(&mut self) -> EditOutcome {
        let result = self.session.relayout(&self.doc, &self.font, &NoHyphenator);
        self.pages = result.pages;
        EditOutcome {
            stats: result.stats,
            repaint: result.changed_pages,
        }
    }

    /// Mutable access for edits the shell does not model yet.
    pub fn document_mut(&mut self) -> &mut Document {
        &mut self.doc
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use quill_core_model::Color;

    fn doc_of(n: usize) -> Document {
        let mut doc = Document::sample();
        doc.assets.clear();
        doc.content = (0..n)
            .map(|i| {
                Block::body(
                    format!("paragraph {i} with enough words to occupy roughly a line of text"),
                    Color::Gray { v: 0.0 },
                )
            })
            .collect();
        doc.assign_missing_block_ids().unwrap();
        doc
    }

    fn state_of(n: usize) -> AppState {
        AppState::from_document(doc_of(n), PathBuf::from("."))
    }

    #[test]
    fn opening_a_document_lays_it_out() {
        let state = state_of(200);
        assert!(state.page_count() > 1);
        assert!(state.document_height_pt() > 0.0);
    }

    #[test]
    fn the_sample_opens_without_a_file() {
        let state = AppState::sample();
        assert!(state.page_count() >= 1);
    }

    #[test]
    fn viewport_math_round_trips_at_every_zoom() {
        let mut state = state_of(50);
        for zoom in [0.25, 1.0, 4.0] {
            state.viewport.zoom = zoom;
            state.viewport.scroll_pt = 137.0;
            for (page, x, y) in [(0usize, 10.0, 20.0), (3, 400.0, 600.0)] {
                let (sx, sy) = state.doc_to_screen(page, x, y);
                let (bx, by) = state.screen_to_doc(page, sx, sy);
                assert!((bx - x).abs() < 0.01, "x at zoom {zoom}: {bx} vs {x}");
                assert!((by - y).abs() < 0.01, "y at zoom {zoom}: {by} vs {y}");
            }
        }
    }

    #[test]
    fn only_visible_pages_are_painted_in_a_long_document() {
        // The constraint the shell itself could violate: a viewport that walks all 500 pages every
        // frame makes the document unusable no matter how fast layout is.
        let mut state = state_of(700);
        let total = state.page_count();
        assert!(total > 10, "need a long document, got {total}");

        state.scroll_to_page(total - 1);
        let visible = state.visible_pages();
        assert!(
            visible.len() <= 4,
            "expected a handful of visible pages, got {}",
            visible.len()
        );

        state.paint_visible();
        assert_eq!(state.painted_pages(), visible.len());
        assert!(state.painted_pages() < total);
    }

    #[test]
    fn scrolling_to_a_page_puts_it_at_the_top() {
        let mut state = state_of(300);
        state.scroll_to_page(5);
        assert_eq!(state.visible_pages().start, 5);
    }

    #[test]
    fn the_visible_range_is_never_empty_for_a_non_empty_document() {
        let mut state = state_of(10);
        // Scrolled past the end — a viewport bug should not produce a blank window.
        state.viewport.scroll_pt = 1_000_000.0;
        let range = state.visible_pages();
        assert!(!range.is_empty());
        assert!(range.end <= state.page_count());
    }

    #[test]
    fn painting_produces_ops_for_each_visible_page() {
        let mut state = state_of(100);
        let visible = state.visible_pages().len();
        let ops = state.paint_visible();
        let pages = ops
            .iter()
            .filter(|op| matches!(op, PaintOp::Page { .. }))
            .count();
        assert_eq!(pages, visible);
    }

    #[test]
    fn an_edit_repaints_only_the_pages_that_changed() {
        // The payoff of spec 0031 reaching the UI: a keystroke must not repaint the book.
        let mut state = state_of(500);
        let total = state.page_count();
        assert!(total > 5);
        let id = state.document().content[4].id();

        let outcome = state.edit_text(id, "a much shorter paragraph");
        assert!(
            outcome.repaint.len() <= 3,
            "expected a bounded repaint, got {} of {total} pages",
            outcome.repaint.len()
        );
        assert!(outcome.stats.pages_reflowed <= 3);
        assert!(outcome.stats.pages_reused > 0);
    }

    #[test]
    fn an_edit_actually_changes_the_text() {
        let mut state = state_of(20);
        let id = state.document().content[2].id();
        state.edit_text(id, "replaced");
        let block = state
            .document()
            .content
            .iter()
            .find(|b| b.id() == id)
            .unwrap();
        match block {
            Block::Body { text, .. } => assert_eq!(text, "replaced"),
            _ => panic!("expected a body block"),
        }
        assert_eq!(block.id(), id, "editing must preserve block identity");
    }

    #[test]
    fn editing_an_image_block_is_a_no_op() {
        let mut doc = doc_of(5);
        let id = doc.new_block_id();
        let mut image = Block::image("nope");
        image.set_id(id);
        doc.content.push(image);
        let mut state = AppState::from_document(doc, PathBuf::from("."));

        let outcome = state.edit_text(id, "text on an image?");
        assert_eq!(outcome, EditOutcome::default());
        assert!(matches!(
            state.document().content.last(),
            Some(Block::Image { .. })
        ));
    }

    #[test]
    fn a_document_with_a_missing_asset_still_opens() {
        // A broken link must not stop a 500-page book from opening.
        let mut doc = doc_of(5);
        doc.assets = vec![quill_core_model::Asset {
            id: "gone".into(),
            path: "definitely/not/here.png".into(),
            px_w: 100,
            px_h: 100,
            dpi: 300.0,
            line_art: false,
            has_alpha: false,
        }];
        doc.content.push(Block::image("gone"));
        doc.assign_missing_block_ids().unwrap();

        let state = AppState::from_document(doc, PathBuf::from("."));
        assert!(state.asset_report().skipped >= 1);
        assert!(state.page_count() >= 1, "the document must still lay out");
    }

    #[test]
    fn a_restyle_reflows_the_document() {
        // Goes through the same session, so this also guards spec 0031's context fingerprint from
        // the UI side.
        let mut state = state_of(200);
        let before = state.page_count();
        let mut body = state.document().styles.paragraph["body"];
        body.font_size_pt = 20.0;
        body.leading_pt = 24.0;
        state
            .document_mut()
            .styles
            .paragraph
            .insert("body".into(), body);
        let outcome = state.relayout();
        assert!(!outcome.repaint.is_empty(), "a restyle must repaint");
        assert!(
            state.page_count() > before,
            "larger text should need more pages: {before} -> {}",
            state.page_count()
        );
    }
}
