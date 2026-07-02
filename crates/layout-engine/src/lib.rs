//! Layout engine: turns a document into positioned pages.
//!
//! The real engine is incremental and dependency-tracked (see the plan) so that editing one
//! text thread reflows only affected pages. This scaffold lays content out naively — stacking
//! text and image blocks, paginating when a block would exceed the page height — so downstream
//! crates compile and the export pipeline has something to consume. Uses `quill-text-layout`
//! for line breaking.

use quill_core_model::{Asset, Block, Color, Document, PageSetup, Rect};
use quill_text_layout::{
    justify_paragraph_hyphenated, Alignment, Hyphenator, Line, RunMetrics, BODY_FONT_SIZE_PT,
    BODY_LINE_HEIGHT_PT,
};

/// A positioned rectangular region that content flows into. The layout engine fills a frame
/// top-to-bottom; a block that would pass the frame's bottom edge overflows — to the next page in
/// this increment, to the next frame in a thread once threading lands (spec 0019 incr. 2).
///
/// Introduced as a seam **at parity**: the frame [`lay_out`] uses is [`Frame::full_page`] (the whole
/// trim area at the origin), so the produced pages — and every export golden test — are byte-identical
/// to the pre-frame implicit column. A frame with a non-zero origin, a narrower width, or a shorter
/// height is the new capability, exercised via [`lay_out_in_frame`].
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Frame {
    pub rect: Rect,
}

impl Frame {
    /// The whole-page content frame: the entire trim area at the origin. This is the frame
    /// [`lay_out`] uses, so its output is identical to the pre-frame implicit column. Margins/insets
    /// and multiple frames per page are follow-ups (spec 0019 non-goals).
    pub fn full_page(page_setup: &PageSetup) -> Frame {
        Frame {
            rect: Rect {
                x_pt: 0.0,
                y_pt: 0.0,
                w_pt: page_setup.trim.w_pt,
                h_pt: page_setup.trim.h_pt,
            },
        }
    }
}

/// An ordered chain of [`Frame`]s that content flows through — a *thread* (spec 0019 incr. 2).
///
/// Content fills `frames[0]` top-to-bottom; a block that overflows the current frame continues into
/// the **next** frame in the thread (two columns on a page, a story that runs box-to-box), and onto
/// a new page — restarting at `frames[0]` — once the thread's frames are exhausted. A single-frame
/// thread reproduces the incr. 1 [`lay_out_in_frame`] behavior exactly (parity), so the same set of
/// frames is repeated per page on overflow. The frames live in `layout-engine` and are supplied by
/// the caller; persisting author-defined threads into the `.tpub` model is a later increment.
#[derive(Debug, Clone, PartialEq)]
pub struct Thread {
    pub frames: Vec<Frame>,
}

impl Thread {
    /// A left-to-right chain of `count` equal-width columns spanning the trim area, separated by
    /// `gutter_pt` of horizontal space, each the full trim height at `y = 0` (spec 0020). Content
    /// laid into the returned thread via [`lay_out_in_thread`] fills the leftmost column
    /// top-to-bottom, then the next column, and onto a new page once the last column fills.
    ///
    /// A single column (`count == 1`) is the whole trim area — identical to [`Frame::full_page`]
    /// (the gutter is then irrelevant, there being no interior gutter). Derived from `PageSetup`
    /// like [`Frame::full_page`]: no authored field, no serialized-model change. Panics if
    /// `count == 0` (a thread must have at least one frame — loud failure over a silent empty
    /// thread).
    pub fn columns(page_setup: &PageSetup, count: usize, gutter_pt: f32) -> Thread {
        assert!(
            count >= 1,
            "a multi-column thread needs at least one column"
        );
        let trim_w = page_setup.trim.w_pt;
        let trim_h = page_setup.trim.h_pt;
        // Total gutter is between columns only: (count - 1) gutters. What's left divides evenly.
        let col_w = (trim_w - (count - 1) as f32 * gutter_pt) / count as f32;
        // A gutter wide enough to consume the trim yields a non-positive column width. Fail loudly
        // rather than emit negative-width, overlapping frames that would silently corrupt layout
        // downstream (break_paragraph against a negative width) — see CLAUDE.md's press-safety rule.
        assert!(
            col_w > 0.0,
            "gutter {gutter_pt} pt too large for {count} columns in {trim_w} pt trim (col_w = {col_w})"
        );
        let frames = (0..count)
            .map(|i| Frame {
                rect: Rect {
                    x_pt: i as f32 * (col_w + gutter_pt),
                    y_pt: 0.0,
                    w_pt: col_w,
                    h_pt: trim_h,
                },
            })
            .collect();
        Thread { frames }
    }
}

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

/// Lay a document out into pages, flowing its content into the whole-page frame
/// ([`Frame::full_page`]). Paginates: starts a new page when a block would pass the frame's bottom
/// edge (the full trim height here). Returns at least one page (even if the document is empty).
///
/// Text is broken to fit the frame width using the caller-supplied `metrics` (the embedded font in
/// the export path) at [`BODY_FONT_SIZE_PT`] — see `specs/0015-text-metrics-line-breaking.md` and
/// spec 0016 for the shift to run-based measurement.
///
/// `hyphenator` supplies the legal in-word break points (spec 0018): the export path passes an
/// en-US `hypher`-backed hyphenator so long words break at syllable boundaries; tests pass
/// [`quill_text_layout::NoHyphenator`] for the spec-0017 parity path.
pub fn lay_out(
    doc: &Document,
    metrics: &impl RunMetrics,
    hyphenator: &impl Hyphenator,
) -> Vec<LaidOutPage> {
    // At parity: the whole trim area at the origin, so the produced pages are byte-identical to the
    // pre-frame implicit column (spec 0019 incr. 1).
    lay_out_in_frame(
        &doc.content,
        &doc.assets,
        &Frame::full_page(&doc.page_setup),
        metrics,
        hyphenator,
    )
}

/// Flow `content` into a single [`Frame`], paginating vertically. Equivalent to
/// [`lay_out_in_thread`] over a one-frame thread: text wraps to the frame width, blocks are
/// positioned at the frame origin, and a block overflows to a new page (repeating the same frame
/// geometry) when it would pass the frame's bottom edge — see spec 0019.
///
/// `assets` resolves [`Block::Image`] ids; unknown ids are skipped.
pub fn lay_out_in_frame(
    content: &[Block],
    assets: &[Asset],
    frame: &Frame,
    metrics: &impl RunMetrics,
    hyphenator: &impl Hyphenator,
) -> Vec<LaidOutPage> {
    lay_out_in_thread(
        content,
        assets,
        &Thread {
            frames: vec![*frame],
        },
        metrics,
        hyphenator,
    )
}

/// The intrinsic size of a block once broken/measured for a given frame width, plus the payload
/// needed to place it. Re-computed against each candidate frame the block is tried in, since both
/// text wrapping and image sizing depend on the frame width (spec 0019 incr. 2).
enum Measured {
    Text {
        lines: Vec<Line>,
        color: Color,
    },
    Image {
        asset_id: String,
        /// The sized placement width (spec 0009), which may be narrower than the frame.
        width: f32,
    },
}

/// Break/size `block` against a frame of `width` points, returning the placement payload and its
/// height. `None` means "skip this block" — currently only an unresolved [`Block::Image`] id.
///
/// Called once per candidate frame in [`lay_out_in_thread`]'s placement loop so a block that
/// advances into a different-width frame re-wraps (text) / re-fits (image) to that frame's width.
fn measure_block(
    block: &Block,
    width: f32,
    assets: &[Asset],
    metrics: &impl RunMetrics,
    hyphenator: &impl Hyphenator,
) -> Option<(Measured, f32)> {
    match block {
        Block::Heading { text, color, .. } | Block::Body { text, color, .. } => {
            // Body text is justified for press-quality even spacing; headings stay ragged-left
            // (a heading is typically one short line, where justification would do nothing anyway —
            // its single line is the paragraph's last, which is never justified).
            let align = match block {
                Block::Heading { .. } => Alignment::Left,
                _ => Alignment::Justified,
            };
            let lines = justify_paragraph_hyphenated(
                text,
                width,
                BODY_FONT_SIZE_PT,
                align,
                metrics,
                hyphenator,
            );
            let height = lines.len() as f32 * BODY_LINE_HEIGHT_PT;
            Some((
                Measured::Text {
                    lines,
                    color: *color,
                },
                height,
            ))
        }
        Block::Image { asset } => {
            // Resolve the asset id. If not found, skip this block (no panic).
            let asset_rec = assets.iter().find(|a| &a.id == asset)?;
            // Size the image at its true aspect ratio, scaling down to fit the frame width when
            // wider. See spec 0009.
            let (w, h) = image_size(asset_rec, width);
            Some((
                Measured::Image {
                    asset_id: asset.clone(),
                    width: w,
                },
                h,
            ))
        }
    }
}

/// Flow `content` through a [`Thread`]'s frames, paginating across frames and then pages
/// (spec 0019 incr. 2). Content fills the first frame top-to-bottom; a block that overflows the
/// current frame continues into the next frame in the thread, and onto a fresh page — restarting at
/// the first frame — once the thread's frames are exhausted.
///
/// An oversized block (taller than a frame) is placed in an otherwise-empty frame rather than
/// skipping forever — the same "already has content" guard incr. 1 used, now measured per frame. A
/// single-frame thread is exactly [`lay_out_in_frame`] (parity). `assets` resolves [`Block::Image`]
/// ids; unknown ids are skipped. A thread must have at least one frame.
pub fn lay_out_in_thread(
    content: &[Block],
    assets: &[Asset],
    thread: &Thread,
    metrics: &impl RunMetrics,
    hyphenator: &impl Hyphenator,
) -> Vec<LaidOutPage> {
    assert!(
        !thread.frames.is_empty(),
        "a thread must have at least one frame"
    );

    let mut pages: Vec<LaidOutPage> = Vec::new();
    let mut page = LaidOutPage::default();
    // Which frame in the thread the cursor is currently filling.
    let mut frame_idx: usize = 0;
    // Absolute y cursor, starting at the current frame's top and reset there on each frame advance.
    let mut y: f32 = thread.frames[0].rect.y_pt;
    // Whether the *current* frame has received a block yet — mirrors incr. 1's page-empty guard,
    // now per frame so an oversized block is placed rather than skipped through every frame/page.
    let mut frame_empty = true;

    for block in content {
        // Advance frames / pages until the block fits, then place it. The block is re-measured
        // against each candidate frame's width (wrapping/sizing depend on it), so a block that
        // advances into a narrower frame re-wraps to that width rather than keeping a stale
        // measurement. Bounded to <= 2 iterations: after one advance the new frame is empty, so the
        // next iteration places (the `frame_empty` guard also places an oversized block rather than
        // looping past every frame).
        loop {
            let frame = thread.frames[frame_idx];
            let Some((measured, height)) =
                measure_block(block, frame.rect.w_pt, assets, metrics, hyphenator)
            else {
                break; // unresolved image asset → skip this block (no panic)
            };
            let bottom = frame.rect.y_pt + frame.rect.h_pt;

            if y + height > bottom && !frame_empty {
                // Doesn't fit and the current frame has content → move on before placing.
                if frame_idx + 1 < thread.frames.len() {
                    frame_idx += 1; // next frame in the thread, same page
                } else {
                    pages.push(page); // thread exhausted → new page, back to the first frame
                    page = LaidOutPage::default();
                    frame_idx = 0;
                }
                y = thread.frames[frame_idx].rect.y_pt;
                frame_empty = true;
                continue; // re-measure against the frame it moved into
            }

            let placed = match measured {
                Measured::Text { lines, color } => PlacedBlock::Text {
                    frame: Rect {
                        x_pt: frame.rect.x_pt,
                        y_pt: y,
                        w_pt: frame.rect.w_pt,
                        h_pt: height,
                    },
                    lines,
                    color,
                },
                Measured::Image { asset_id, width } => PlacedBlock::Image {
                    frame: Rect {
                        x_pt: frame.rect.x_pt,
                        y_pt: y,
                        w_pt: width,
                        h_pt: height,
                    },
                    asset_id,
                },
            };
            page.blocks.push(placed);
            y += height;
            frame_empty = false;
            break;
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
    use quill_text_layout::{Hyphenator, MonospaceRunMetrics, NoHyphenator};

    /// 0.6 em × 10 pt = 6 pt/char, matching the old `APPROX_CHAR_WIDTH_PT` stand-in so these
    /// pagination tests keep their familiar per-character arithmetic.
    const MONO: MonospaceRunMetrics = MonospaceRunMetrics { em_ratio: 0.6 };

    #[test]
    fn lays_out_sample_into_one_page() {
        // Document::sample() has 2 text blocks (a short heading + a body paragraph that wraps to
        // a few lines) + asset "map1" (referenced by no Block::Image in the sample, so no image
        // block is placed). Content still fits well within one page.
        let pages = lay_out(&Document::sample(), &MONO, &NoHyphenator);
        assert!(!pages.is_empty());
        assert!(!pages[0].blocks.is_empty());
    }

    #[test]
    fn sample_body_wraps_and_justifies() {
        // The CI Ghostscript preflight exports Document::sample() and parses its content stream to
        // exercise the justified-`TJ` path (spec 0017 incr. 2). That only happens if the sample's
        // body paragraph wraps to >= 2 lines, giving an interior line a non-zero adjustment. Guard
        // that invariant here so shortening the sample text can't silently drop the CI coverage.
        let pages = lay_out(&Document::sample(), &MONO, &NoHyphenator);
        // The sample leads with a short heading, then the body paragraph; look for any text block
        // that both wraps (>= 2 lines) and carries a justified (non-zero-adjustment) interior line.
        let wrapped_justified = pages
            .iter()
            .flat_map(|p| &p.blocks)
            .filter_map(|b| match b {
                PlacedBlock::Text { lines, .. } => Some(lines),
                _ => None,
            })
            .any(|lines| lines.len() >= 2 && lines.iter().any(|l| l.space_adjust_pt != 0.0));
        assert!(
            wrapped_justified,
            "sample must contain a wrapped, justified paragraph so CI parses a justified TJ"
        );
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

    /// Breaks the crafted long word in `lay_out_threads_the_hyphenator` in half; nothing else.
    struct HalfStub;
    impl Hyphenator for HalfStub {
        fn hyphenate(&self, word: &str) -> Vec<usize> {
            if word.len() == 100 {
                vec![50]
            } else {
                Vec::new()
            }
        }
    }

    #[test]
    fn lay_out_threads_the_hyphenator() {
        // Proves `lay_out` actually passes its hyphenator down to the breaker (spec 0018 incr. 2).
        // A single 100-char word (600 pt under MONO) overflows the 432 pt frame with NoHyphenator
        // (one long line, no hyphen). With HalfStub it splits at offset 50 — the first line ends in
        // a rendered hyphen — so the two paths must differ.
        let doc = doc_with_blocks(vec![Block::Body {
            text: "z".repeat(100),
            color: Color::Gray { v: 0.0 },
        }]);

        let plain = first_text_lines(&lay_out(&doc, &MONO, &NoHyphenator));
        let hyphenated = first_text_lines(&lay_out(&doc, &MONO, &HalfStub));

        assert_eq!(
            plain.len(),
            1,
            "no hyphenation → the word overflows on one line"
        );
        assert!(!plain[0].text.ends_with('-'));
        assert_eq!(
            hyphenated.len(),
            2,
            "HalfStub splits the word across two lines"
        );
        assert!(
            hyphenated[0].text.ends_with('-'),
            "the broken line renders a trailing hyphen"
        );
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

        let body_lines = first_text_lines(&lay_out(&body, &MONO, &NoHyphenator));
        let heading_lines = first_text_lines(&lay_out(&heading, &MONO, &NoHyphenator));

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

    /// The `Rect` of the first `PlacedBlock::Text` found across `pages`.
    fn first_text_frame(pages: &[LaidOutPage]) -> Rect {
        pages
            .iter()
            .flat_map(|p| &p.blocks)
            .find_map(|b| match b {
                PlacedBlock::Text { frame, .. } => Some(*frame),
                _ => None,
            })
            .expect("a text block")
    }

    #[test]
    fn full_page_frame_is_the_whole_trim_at_origin() {
        // The seam's parity anchor: Frame::full_page is exactly the trim area at (0,0), which is why
        // lay_out (which uses it) stays byte-identical to the pre-frame column (spec 0019 incr. 1).
        let page = PageSetup::default();
        let frame = Frame::full_page(&page);
        assert_eq!(frame.rect.x_pt, 0.0);
        assert_eq!(frame.rect.y_pt, 0.0);
        assert_eq!(frame.rect.w_pt, page.trim.w_pt);
        assert_eq!(frame.rect.h_pt, page.trim.h_pt);
    }

    #[test]
    fn lay_out_matches_full_page_frame_path() {
        // lay_out is exactly lay_out_in_frame over the full-page frame — same pages, proving the
        // wrapper introduces no divergence (parity).
        let doc = Document::sample();
        let via_lay_out = lay_out(&doc, &MONO, &NoHyphenator);
        let via_frame = lay_out_in_frame(
            &doc.content,
            &doc.assets,
            &Frame::full_page(&doc.page_setup),
            &MONO,
            &NoHyphenator,
        );
        assert_eq!(via_lay_out, via_frame);
    }

    #[test]
    fn frame_origin_offsets_placed_blocks() {
        // The same single short paragraph, laid full-page vs. into a frame at origin (36, 48). For
        // content that fits on one page, every placed block shifts by exactly (36, 48).
        let content = vec![Block::Body {
            text: "short line".into(),
            color: Color::Gray { v: 0.0 },
        }];
        let assets: Vec<Asset> = vec![];
        let page = PageSetup::default();

        let full = first_text_frame(&lay_out_in_frame(
            &content,
            &assets,
            &Frame::full_page(&page),
            &MONO,
            &NoHyphenator,
        ));
        let offset = Frame {
            rect: Rect {
                x_pt: 36.0,
                y_pt: 48.0,
                w_pt: page.trim.w_pt,
                h_pt: page.trim.h_pt,
            },
        };
        let shifted = first_text_frame(&lay_out_in_frame(
            &content,
            &assets,
            &offset,
            &MONO,
            &NoHyphenator,
        ));

        assert!(
            (shifted.x_pt - full.x_pt - 36.0).abs() < 0.01,
            "x: {} vs {}",
            shifted.x_pt,
            full.x_pt
        );
        assert!(
            (shifted.y_pt - full.y_pt - 48.0).abs() < 0.01,
            "y: {} vs {}",
            shifted.y_pt,
            full.y_pt
        );
    }

    #[test]
    fn narrower_frame_wraps_to_more_lines() {
        // A paragraph that wraps to N lines in the full-page frame wraps to strictly more lines in a
        // frame half as wide — text respects the frame width, not the page width.
        let content = vec![Block::Body {
            text: "goblins raid the village at dusk stealing grain and copper coins from every trembling home nearby".into(),
            color: Color::Gray { v: 0.0 },
        }];
        let assets: Vec<Asset> = vec![];
        let page = PageSetup::default();

        let wide = first_text_lines(&lay_out_in_frame(
            &content,
            &assets,
            &Frame::full_page(&page),
            &MONO,
            &NoHyphenator,
        ));
        let narrow_frame = Frame {
            rect: Rect {
                x_pt: 0.0,
                y_pt: 0.0,
                w_pt: page.trim.w_pt / 2.0,
                h_pt: page.trim.h_pt,
            },
        };
        let narrow = first_text_lines(&lay_out_in_frame(
            &content,
            &assets,
            &narrow_frame,
            &MONO,
            &NoHyphenator,
        ));
        assert!(
            narrow.len() > wide.len(),
            "narrow frame {} lines should exceed wide frame {} lines",
            narrow.len(),
            wide.len()
        );
    }

    #[test]
    fn shorter_frame_paginates_earlier() {
        // 20 single-line blocks fit on one full-page frame (648 pt / 12 pt = 54 lines). A 60 pt-tall
        // frame holds only ~5 lines, so the same content spills to multiple pages — overflow is
        // measured against the frame's bottom edge, not the trim height.
        let content: Vec<Block> = (0..20)
            .map(|i| Block::Body {
                text: format!("L{i}"),
                color: Color::Gray { v: 0.0 },
            })
            .collect();
        let assets: Vec<Asset> = vec![];
        let page = PageSetup::default();

        let full = lay_out_in_frame(
            &content,
            &assets,
            &Frame::full_page(&page),
            &MONO,
            &NoHyphenator,
        );
        assert_eq!(
            full.len(),
            1,
            "20 lines fit one full page, got {}",
            full.len()
        );

        let short_frame = Frame {
            rect: Rect {
                x_pt: 0.0,
                y_pt: 0.0,
                w_pt: page.trim.w_pt,
                h_pt: 60.0,
            },
        };
        let short = lay_out_in_frame(&content, &assets, &short_frame, &MONO, &NoHyphenator);
        assert!(
            short.len() >= 2,
            "a 60 pt frame must paginate 20 lines, got {}",
            short.len()
        );
    }

    /// Two side-by-side columns on a 432×648 page: a left frame and a right frame, each `w` wide and
    /// `h` tall at the top of the page. Used by the threading tests.
    fn two_column_thread(w: f32, h: f32) -> Thread {
        Thread {
            frames: vec![
                Frame {
                    rect: Rect {
                        x_pt: 0.0,
                        y_pt: 0.0,
                        w_pt: w,
                        h_pt: h,
                    },
                },
                Frame {
                    rect: Rect {
                        x_pt: 216.0,
                        y_pt: 0.0,
                        w_pt: w,
                        h_pt: h,
                    },
                },
            ],
        }
    }

    #[test]
    fn single_frame_thread_matches_lay_out_in_frame() {
        // Parity: a one-frame thread is exactly the incr. 1 single-frame path, so lay_out (and thus
        // export output) is unchanged by threading.
        let doc = Document::sample();
        let frame = Frame::full_page(&doc.page_setup);
        let via_frame = lay_out_in_frame(&doc.content, &doc.assets, &frame, &MONO, &NoHyphenator);
        let via_thread = lay_out_in_thread(
            &doc.content,
            &doc.assets,
            &Thread {
                frames: vec![frame],
            },
            &MONO,
            &NoHyphenator,
        );
        assert_eq!(via_frame, via_thread);
    }

    #[test]
    fn overflow_chains_into_next_frame_on_same_page() {
        // Two 96 pt-tall columns (8 lines each) side by side. 12 single-line blocks overflow the
        // left column (8 lines) and must continue into the RIGHT column on the SAME page — not spill
        // to a second page (12 <= 16 lines of capacity).
        let content: Vec<Block> = (0..12)
            .map(|i| Block::Body {
                text: format!("L{i}"),
                color: Color::Gray { v: 0.0 },
            })
            .collect();
        let thread = two_column_thread(216.0, 96.0);
        let pages = lay_out_in_thread(&content, &[], &thread, &MONO, &NoHyphenator);

        assert_eq!(
            pages.len(),
            1,
            "12 lines fit two 8-line columns on one page"
        );
        let xs: Vec<f32> = pages[0]
            .blocks
            .iter()
            .map(|b| match b {
                PlacedBlock::Text { frame, .. } => frame.x_pt,
                PlacedBlock::Image { frame, .. } => frame.x_pt,
            })
            .collect();
        assert!(
            xs.contains(&0.0),
            "some blocks land in the left column (x=0)"
        );
        assert!(
            xs.contains(&216.0),
            "overflow continues into the right column (x=216)"
        );
    }

    #[test]
    fn new_page_only_after_last_frame_fills() {
        // Two 96 pt-tall columns = 16 lines of capacity per page. 20 single-line blocks overflow
        // BOTH columns and must spill to a second page, restarting at the first (left) frame.
        let content: Vec<Block> = (0..20)
            .map(|i| Block::Body {
                text: format!("L{i}"),
                color: Color::Gray { v: 0.0 },
            })
            .collect();
        let thread = two_column_thread(216.0, 96.0);
        let pages = lay_out_in_thread(&content, &[], &thread, &MONO, &NoHyphenator);

        assert!(
            pages.len() >= 2,
            "20 lines exceed two 8-line columns, got {} pages",
            pages.len()
        );
        // Page 2's first block restarts at the first frame's origin (left column, y=0).
        match &pages[1].blocks[0] {
            PlacedBlock::Text { frame, .. } => {
                assert_eq!(frame.x_pt, 0.0, "page 2 restarts in the left column");
                assert_eq!(frame.y_pt, 0.0, "page 2 restarts at the frame top");
            }
            other => panic!("expected a text block, got {other:?}"),
        }
    }

    #[test]
    fn right_column_blocks_carry_right_frame_x() {
        // Every block that overflowed into the right column must carry that frame's x (216), never
        // the left frame's — proving placement uses the frame the block actually landed in.
        let content: Vec<Block> = (0..12)
            .map(|i| Block::Body {
                text: format!("L{i}"),
                color: Color::Gray { v: 0.0 },
            })
            .collect();
        let thread = two_column_thread(216.0, 96.0);
        let pages = lay_out_in_thread(&content, &[], &thread, &MONO, &NoHyphenator);
        let right: Vec<&Rect> = pages[0]
            .blocks
            .iter()
            .filter_map(|b| match b {
                PlacedBlock::Text { frame, .. } if frame.x_pt == 216.0 => Some(frame),
                _ => None,
            })
            .collect();
        assert!(!right.is_empty(), "some blocks landed in the right column");
        assert!(
            right.iter().all(|f| f.w_pt == 216.0),
            "right-column blocks wrap to the right frame's width"
        );
    }

    #[test]
    fn advanced_block_rewraps_to_landed_frame_width() {
        // A block that overflows a WIDE frame into a NARROWER next frame must re-wrap to the narrow
        // width — not keep the wide measurement (spec 0019 incr. 2, "width per frame"). Frame A is a
        // full-width, 1-line-tall box; frame B is narrow. The first block fills A exactly; the
        // second overflows into B and must break to multiple lines at B's width (a stale wide
        // measurement would place it as a single line spilling past B's right edge).
        let thread = Thread {
            frames: vec![
                Frame {
                    rect: Rect {
                        x_pt: 0.0,
                        y_pt: 0.0,
                        w_pt: 432.0,
                        h_pt: BODY_LINE_HEIGHT_PT, // holds exactly one line
                    },
                },
                Frame {
                    rect: Rect {
                        x_pt: 216.0,
                        y_pt: 0.0,
                        w_pt: 60.0, // 10 chars/line under MONO (6 pt/char)
                        h_pt: 400.0,
                    },
                },
            ],
        };
        // "alpha beta gamma delta" = 22 chars: one line at 432 pt, but cannot fit in fewer than 3
        // lines of 10 chars, so it must wrap to >= 2 lines in frame B.
        let content = vec![
            Block::Body {
                text: "first".into(),
                color: Color::Gray { v: 0.0 },
            },
            Block::Body {
                text: "alpha beta gamma delta".into(),
                color: Color::Gray { v: 0.0 },
            },
        ];
        let pages = lay_out_in_thread(&content, &[], &thread, &MONO, &NoHyphenator);

        // The second block lands in the narrow frame B (x = 216).
        let (frame, lines) = pages[0]
            .blocks
            .iter()
            .find_map(|b| match b {
                PlacedBlock::Text { frame, lines, .. } if frame.x_pt == 216.0 => {
                    Some((frame, lines))
                }
                _ => None,
            })
            .expect("second block landed in the narrow frame");
        assert_eq!(frame.w_pt, 60.0, "carries the landed (narrow) frame width");
        assert!(
            lines.len() >= 2,
            "re-wrapped to the narrow frame width, got {} line(s)",
            lines.len()
        );
        assert!(
            (frame.h_pt - lines.len() as f32 * BODY_LINE_HEIGHT_PT).abs() < 0.01,
            "height matches the re-wrapped line count (not the stale 1-line height)"
        );
    }

    #[test]
    fn single_column_is_the_full_page() {
        // count == 1 is the whole trim area (== Frame::full_page), regardless of gutter.
        let page = PageSetup::default();
        for gutter in [0.0, 12.0, 36.0] {
            let thread = Thread::columns(&page, 1, gutter);
            assert_eq!(thread.frames.len(), 1);
            assert_eq!(
                thread.frames[0].rect,
                Frame::full_page(&page).rect,
                "gutter {gutter}"
            );
        }
    }

    #[test]
    fn columns_tile_the_trim_width() {
        // N columns of equal width, separated by (N-1) gutters, exactly span the trim width and are
        // laid left-to-right without overlap.
        let page = PageSetup::default();
        let trim_w = page.trim.w_pt;
        let gutter = 18.0;
        for count in [2usize, 3, 4] {
            let thread = Thread::columns(&page, count, gutter);
            assert_eq!(thread.frames.len(), count);

            let col_w = thread.frames[0].rect.w_pt;
            // All columns share the same width.
            assert!(
                thread
                    .frames
                    .iter()
                    .all(|f| (f.rect.w_pt - col_w).abs() < 0.01),
                "count {count}: columns should be equal width"
            );
            // N columns + (N-1) gutters span the trim width exactly.
            let spanned = count as f32 * col_w + (count - 1) as f32 * gutter;
            assert!(
                (spanned - trim_w).abs() < 0.01,
                "count {count}: {spanned} should span trim width {trim_w}"
            );
            // Left-to-right, non-overlapping: each column starts one (col_w + gutter) past the last.
            for i in 1..count {
                let prev = thread.frames[i - 1].rect;
                let cur = thread.frames[i].rect;
                assert!(
                    (cur.x_pt - (prev.x_pt + col_w + gutter)).abs() < 0.01,
                    "count {count}: column {i} should follow the gutter after column {}",
                    i - 1
                );
            }
        }
    }

    #[test]
    #[should_panic(expected = "too large")]
    fn oversized_gutter_panics_rather_than_corrupting() {
        // A gutter wide enough to make the column width non-positive must fail loudly, not emit
        // negative-width overlapping frames (CLAUDE.md: visible failure over silent corruption).
        let page = PageSetup::default(); // 432 pt trim: two 500 pt gutters can't fit.
        Thread::columns(&page, 2, 500.0);
    }

    #[test]
    fn columns_are_full_height_at_the_top() {
        let page = PageSetup::default();
        let thread = Thread::columns(&page, 3, 12.0);
        for (i, f) in thread.frames.iter().enumerate() {
            assert_eq!(f.rect.y_pt, 0.0, "column {i} y");
            assert_eq!(f.rect.h_pt, page.trim.h_pt, "column {i} height");
        }
    }

    #[test]
    fn columns_compose_with_threading() {
        // A short two-column thread (columns only ~8 lines tall) fed enough blocks to overflow the
        // first column must continue into the SECOND column on the same page — proving the
        // constructor composes with lay_out_in_thread to produce real multi-column flow.
        let page = PageSetup::default();
        // Full-height columns hold ~54 lines each, too many to overflow cheaply; shrink the height
        // by rebuilding the thread's frames to 96 pt (8 lines) so 12 blocks overflow column 0.
        let base = Thread::columns(&page, 2, 18.0);
        // Keep the constructor's derived x/width; only shrink the height to force overflow.
        let thread = Thread {
            frames: base
                .frames
                .iter()
                .map(|f| Frame {
                    rect: Rect {
                        h_pt: 96.0,
                        ..f.rect
                    },
                })
                .collect(),
        };
        // The substituted frames must still carry the derived column width (432 − 18)/2 = 207, so a
        // regression in col_w can't slip past this test.
        assert!((thread.frames[0].rect.w_pt - 207.0).abs() < 0.01);
        let content: Vec<Block> = (0..12)
            .map(|i| Block::Body {
                text: format!("L{i}"),
                color: Color::Gray { v: 0.0 },
            })
            .collect();
        let pages = lay_out_in_thread(&content, &[], &thread, &MONO, &NoHyphenator);

        assert_eq!(
            pages.len(),
            1,
            "12 lines fit two 8-line columns on one page"
        );
        let left_x = thread.frames[0].rect.x_pt;
        let right_x = thread.frames[1].rect.x_pt;
        let xs: Vec<f32> = pages[0]
            .blocks
            .iter()
            .map(|b| match b {
                PlacedBlock::Text { frame, .. } => frame.x_pt,
                PlacedBlock::Image { frame, .. } => frame.x_pt,
            })
            .collect();
        // Partition, not just presence: the 8-line-tall first column holds exactly the first 8
        // blocks, the remaining 4 overflow into the second column on the same page.
        let left = xs.iter().filter(|&&x| x == left_x).count();
        let right = xs.iter().filter(|&&x| x == right_x).count();
        assert_eq!(left, 8, "left column holds its 8 lines");
        assert_eq!(right, 4, "the remaining 4 overflow into the right column");
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
        let pages = lay_out(&doc, &MONO, &NoHyphenator);
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
        let pages_exact = lay_out(&doc_exact, &MONO, &NoHyphenator);
        assert_eq!(
            pages_exact.len(),
            1,
            "expected 1 page for exact fit, got {}",
            pages_exact.len()
        );

        // One extra block → must spill to a second page.
        let overflow_blocks: Vec<Block> = (0..=lines_per_page).map(make_block).collect();
        let doc_overflow = doc_with_blocks(overflow_blocks);
        let pages_overflow = lay_out(&doc_overflow, &MONO, &NoHyphenator);
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

        let pages = lay_out(&doc, &MONO, &NoHyphenator);

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
