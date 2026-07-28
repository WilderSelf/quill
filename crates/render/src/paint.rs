//! The screen paint list — see `specs/0033-screen-render.md`.
//!
//! ## Why a list of ops rather than direct drawing
//!
//! `docs/roadmap.md` records the canvas backend as `tiny-skia`, chosen for being pure Rust and
//! adding no native build to the three-OS CI matrix — but explicitly kept swappable for a GPU
//! backend later. Emitting a backend-neutral op list first and rasterizing second is what makes
//! that swap a change to one module instead of a change to layout.
//!
//! It also makes screen rendering *testable*. Pixel golden files are flaky across platforms because
//! anti-aliasing differs; an op list is exact, so the assertions here are about geometry and
//! content rather than about pixels.
//!
//! ## Coordinates
//!
//! Everything is top-left origin, in points, the same space `layout-engine` produces. The PDF
//! writer's bottom-left flip is a PDF convention and does not appear here.

use quill_core_model::{Color, PageGeom};
use quill_fonts::Font;
use quill_layout_engine::{LaidOutPage, PlacedBlock};

use crate::ProxyCache;

/// One drawing instruction. Deliberately small and concrete.
#[derive(Debug, Clone, PartialEq)]
pub enum PaintOp {
    /// Fill the page (media box) with paper.
    Page { w_pt: f32, h_pt: f32 },
    /// The trim rectangle, for a page-edge guide.
    TrimGuide {
        x_pt: f32,
        y_pt: f32,
        w_pt: f32,
        h_pt: f32,
    },
    /// A run of text on one baseline.
    Text {
        /// Left edge of the run.
        x_pt: f32,
        /// The **baseline**, not the top of the line.
        baseline_pt: f32,
        text: String,
        size_pt: f32,
        /// Extra space inserted between words to justify the line (spec 0017).
        space_adjust_pt: f32,
        rgb: [u8; 3],
    },
    /// A filled and/or stroked rectangle — a rule, a border, a tinted panel (spec 0037).
    ///
    /// Distinct from [`PaintOp::TrimGuide`], which is a screen-only guide that is never press
    /// content. This one is ink: the same op the PDF writer emits.
    Rect {
        x_pt: f32,
        y_pt: f32,
        w_pt: f32,
        h_pt: f32,
        fill_rgb: Option<[u8; 3]>,
        /// Outline colour and width in points.
        stroke: Option<([u8; 3], f32)>,
    },
    /// A linked image, drawn from its cached screen proxy.
    Image {
        x_pt: f32,
        y_pt: f32,
        w_pt: f32,
        h_pt: f32,
        asset_id: String,
        /// Proxy pixel dimensions, so a test can prove full-res was never touched.
        src_w: u32,
        src_h: u32,
    },
}

/// Build the paint list for one laid-out page.
///
/// Deterministic: the same inputs always produce the same ops, which is what lets the op list be
/// the golden artifact instead of a bitmap.
///
/// An image with no cached proxy emits **nothing** and does not panic. On screen a missing link is
/// recoverable and obvious; refusing to draw the page around it would make a 500-page book
/// unopenable because one asset moved. (Export takes the opposite view for the same reason it
/// always does: a silently dropped image reaching a print shop is not recoverable.)
pub fn paint_page(
    page: &LaidOutPage,
    geom: &PageGeom,
    font: &Font,
    proxies: &ProxyCache,
) -> Vec<PaintOp> {
    let mut ops = vec![
        PaintOp::Page {
            w_pt: geom.media_w,
            h_pt: geom.media_h,
        },
        PaintOp::TrimGuide {
            x_pt: geom.off_x,
            y_pt: geom.off_y,
            w_pt: geom.trim_w,
            h_pt: geom.trim_h,
        },
    ];

    // Statics first, then flowed content — master art sits behind the text that flows over it, the
    // same order the PDF writer paints in (spec 0029).
    for block in page.statics.iter().chain(page.blocks.iter()) {
        match block {
            PlacedBlock::Text {
                frame,
                lines,
                color,
                font_size_pt,
                leading_pt,
                ..
            } => {
                let rgb = quill_color::to_srgb(color);
                // The baseline comes from the shared font crate, which is also where the PDF writer
                // gets it (spec 0032). One source, so screen and page agree about where a line sits.
                let ascent = font.ascent_pt(*font_size_pt);
                for (i, line) in lines.iter().enumerate() {
                    ops.push(PaintOp::Text {
                        x_pt: geom.off_x + frame.x_pt,
                        baseline_pt: geom.off_y + frame.y_pt + ascent + i as f32 * leading_pt,
                        text: line.text.clone(),
                        size_pt: *font_size_pt,
                        space_adjust_pt: line.space_adjust_pt,
                        rgb,
                    });
                }
            }
            PlacedBlock::Rect {
                frame,
                fill,
                stroke,
            } => {
                // Nothing to draw is drawn as nothing, rather than as an empty path: a degenerate
                // rect must not put an operator in the list that the PDF writer would then have to
                // emit as a malformed `re n`.
                if (fill.is_none() && stroke.is_none()) || frame.w_pt <= 0.0 || frame.h_pt <= 0.0 {
                    continue;
                }
                ops.push(PaintOp::Rect {
                    x_pt: geom.off_x + frame.x_pt,
                    y_pt: geom.off_y + frame.y_pt,
                    w_pt: frame.w_pt,
                    h_pt: frame.h_pt,
                    fill_rgb: fill.as_ref().map(quill_color::to_srgb),
                    stroke: stroke
                        .as_ref()
                        .map(|s| (quill_color::to_srgb(&s.color), s.width_pt)),
                });
            }
            PlacedBlock::Image {
                frame, asset_id, ..
            } => {
                let Some(proxy) = proxies.get(asset_id) else {
                    continue; // no proxy yet, or an unresolvable link — draw nothing
                };
                ops.push(PaintOp::Image {
                    x_pt: geom.off_x + frame.x_pt,
                    y_pt: geom.off_y + frame.y_pt,
                    w_pt: frame.w_pt,
                    h_pt: frame.h_pt,
                    asset_id: asset_id.clone(),
                    src_w: proxy.width,
                    src_h: proxy.height,
                });
            }
        }
    }
    ops
}

/// The paper colour a page is filled with.
pub const PAPER: [u8; 3] = [255, 255, 255];

/// Convert an authored colour to screen sRGB. Re-exported so callers need not reach into
/// `quill-color` for the one function they want.
pub fn screen_rgb(color: &Color) -> [u8; 3] {
    quill_color::to_srgb(color)
}

#[cfg(test)]
mod tests {
    use super::*;
    use quill_core_model::{page_geom, Document};
    use quill_layout_engine::{lay_out, Stroke};
    use quill_text_layout::NoHyphenator;

    fn sample_page() -> (LaidOutPage, PageGeom, Font) {
        let doc = Document::sample();
        let font = Font::bundled();
        let pages = lay_out(&doc, &font, &NoHyphenator);
        let geom = page_geom(&doc.page_setup, 0);
        (pages[0].clone(), geom, font)
    }

    #[test]
    fn painting_is_deterministic() {
        // The op list — not a bitmap — is the golden artifact: anti-aliasing differs across
        // platforms, so a pixel golden would be flaky on the three-OS matrix while proving less.
        let (page, geom, font) = sample_page();
        let cache = ProxyCache::new();
        let a = paint_page(&page, &geom, &font, &cache);
        let b = paint_page(&page, &geom, &font, &cache);
        assert_eq!(a, b);
    }

    // --- Decoration (spec 0037) ---------------------------------------------------------------

    fn rect_block(fill: Option<Color>, stroke: Option<Stroke>) -> PlacedBlock {
        PlacedBlock::Rect {
            frame: quill_core_model::Rect {
                x_pt: 10.0,
                y_pt: 20.0,
                w_pt: 80.0,
                h_pt: 30.0,
            },
            fill,
            stroke,
        }
    }

    const TINT: Color = Color::Gray { v: 0.9 };

    #[test]
    fn a_decoration_rect_paints_with_its_page_offset_applied() {
        let (mut page, geom, font) = sample_page();
        page.blocks = vec![rect_block(Some(TINT), None)];
        let ops = paint_page(&page, &geom, &font, &ProxyCache::new());
        let rect = ops
            .iter()
            .find(|o| matches!(o, PaintOp::Rect { .. }))
            .expect("a rect op");
        match rect {
            PaintOp::Rect {
                x_pt,
                y_pt,
                w_pt,
                h_pt,
                fill_rgb,
                stroke,
            } => {
                assert_eq!(*x_pt, geom.off_x + 10.0);
                assert_eq!(*y_pt, geom.off_y + 20.0);
                assert_eq!((*w_pt, *h_pt), (80.0, 30.0));
                assert!(fill_rgb.is_some());
                assert!(stroke.is_none());
            }
            _ => unreachable!(),
        }
    }

    #[test]
    fn decoration_paints_before_the_text_it_sits_behind() {
        // A panel must not paint over the text it frames. Ops are emitted in block order and the
        // list is the golden artifact, so ordering is asserted on the list rather than on pixels.
        let (mut page, geom, font) = sample_page();
        let text = page.blocks[0].clone();
        page.blocks = vec![rect_block(Some(TINT), None), text];
        let ops = paint_page(&page, &geom, &font, &ProxyCache::new());
        let rect_at = ops
            .iter()
            .position(|o| matches!(o, PaintOp::Rect { .. }))
            .expect("rect");
        let text_at = ops
            .iter()
            .position(|o| matches!(o, PaintOp::Text { .. }))
            .expect("text");
        assert!(rect_at < text_at, "the panel must be painted first");
    }

    #[test]
    fn a_decoration_that_draws_nothing_emits_no_op() {
        // No colours, and degenerate geometry. Neither may put an op in the list: the PDF writer
        // would have to turn it into a path with no paint operator.
        let (page, geom, font) = sample_page();
        for block in [
            rect_block(None, None),
            PlacedBlock::Rect {
                frame: quill_core_model::Rect {
                    x_pt: 0.0,
                    y_pt: 0.0,
                    w_pt: 0.0,
                    h_pt: 10.0,
                },
                fill: Some(TINT),
                stroke: None,
            },
        ] {
            let mut p = page.clone();
            p.blocks = vec![block];
            let ops = paint_page(&p, &geom, &font, &ProxyCache::new());
            assert!(!ops.iter().any(|o| matches!(o, PaintOp::Rect { .. })));
        }
    }

    #[test]
    fn a_page_starts_with_paper_and_a_trim_guide() {
        let (page, geom, font) = sample_page();
        let ops = paint_page(&page, &geom, &font, &ProxyCache::new());
        assert!(matches!(ops[0], PaintOp::Page { .. }));
        assert!(matches!(ops[1], PaintOp::TrimGuide { .. }));
        match ops[0] {
            PaintOp::Page { w_pt, h_pt } => {
                assert_eq!(w_pt, geom.media_w);
                assert_eq!(h_pt, geom.media_h);
            }
            _ => unreachable!(),
        }
    }

    #[test]
    fn the_screen_baseline_matches_the_exporters() {
        // The WYSIWYG guard. Both take the baseline from `Font::ascent_pt` (spec 0032); if they
        // ever diverged, a line would sit in one place on screen and another on the page.
        let (page, geom, font) = sample_page();
        let ops = paint_page(&page, &geom, &font, &ProxyCache::new());
        let first_text = ops
            .iter()
            .find_map(|op| match op {
                PaintOp::Text {
                    baseline_pt,
                    size_pt,
                    ..
                } => Some((*baseline_pt, *size_pt)),
                _ => None,
            })
            .expect("the sample has text");
        let frame_y = match &page.blocks[0] {
            PlacedBlock::Text { frame, .. } => frame.y_pt,
            _ => panic!("expected text first"),
        };
        let expected = geom.off_y + frame_y + font.ascent_pt(first_text.1);
        assert!(
            (first_text.0 - expected).abs() < 0.001,
            "baseline {} vs expected {expected}",
            first_text.0
        );
    }

    #[test]
    fn text_ops_carry_their_own_size() {
        // Styles (spec 0028) must reach the screen, not just the PDF.
        let mut doc = Document::sample();
        doc.content = vec![
            quill_core_model::Block::heading(1, "Title", Color::Gray { v: 0.0 }),
            quill_core_model::Block::body("body text", Color::Gray { v: 0.0 }),
        ];
        doc.assign_missing_block_ids().unwrap();
        let font = Font::bundled();
        let pages = lay_out(&doc, &font, &NoHyphenator);
        let ops = paint_page(
            &pages[0],
            &page_geom(&doc.page_setup, 0),
            &font,
            &ProxyCache::new(),
        );
        let sizes: Vec<f32> = ops
            .iter()
            .filter_map(|op| match op {
                PaintOp::Text { size_pt, .. } => Some(*size_pt),
                _ => None,
            })
            .collect();
        assert!(sizes.len() >= 2);
        assert!(
            sizes[0] > sizes[1],
            "the heading should paint larger than the body: {sizes:?}"
        );
    }

    #[test]
    fn an_image_with_no_proxy_draws_nothing_and_does_not_panic() {
        // On screen a missing link is recoverable and visible; refusing to draw the page around it
        // would make a 500-page book unopenable because one asset moved.
        let mut doc = Document::sample();
        doc.content.push(quill_core_model::Block::image("map1"));
        doc.assign_missing_block_ids().unwrap();
        let font = Font::bundled();
        let pages = lay_out(&doc, &font, &NoHyphenator);
        let ops = paint_page(
            &pages[0],
            &page_geom(&doc.page_setup, 0),
            &font,
            &ProxyCache::new(),
        );
        assert!(!ops.iter().any(|op| matches!(op, PaintOp::Image { .. })));
    }

    #[test]
    fn an_image_with_a_proxy_emits_an_op_at_proxy_resolution() {
        // Proves full-res is never composited on screen — the core performance strategy.
        let mut doc = Document::sample();
        doc.content.push(quill_core_model::Block::image("map1"));
        doc.assign_missing_block_ids().unwrap();

        let mut cache = ProxyCache::new();
        let png = crate::tests_support::tiny_png(8, 6);
        assert!(cache.insert_png("map1", &png));
        let proxy_dims = {
            let p = cache.get("map1").unwrap();
            (p.width, p.height)
        };

        let font = Font::bundled();
        let pages = lay_out(&doc, &font, &NoHyphenator);
        let ops: Vec<PaintOp> = pages
            .iter()
            .flat_map(|p| paint_page(p, &page_geom(&doc.page_setup, p.index), &font, &cache))
            .collect();
        let image = ops
            .iter()
            .find_map(|op| match op {
                PaintOp::Image { src_w, src_h, .. } => Some((*src_w, *src_h)),
                _ => None,
            })
            .expect("expected an image op");
        assert_eq!(image, proxy_dims);
    }

    #[test]
    fn statics_paint_before_flowed_content() {
        // Master art has to sit behind the text flowing over it, and paint order is what decides.
        let doc = Document::sample();
        let font = Font::bundled();
        let mut page = lay_out(&doc, &font, &NoHyphenator)[0].clone();
        page.statics = vec![PlacedBlock::Text {
            source: quill_core_model::BlockId::UNASSIGNED,
            frame: quill_core_model::Rect {
                x_pt: 0.0,
                y_pt: 600.0,
                w_pt: 400.0,
                h_pt: 12.0,
            },
            lines: vec![quill_text_layout::Line {
                text: "running head".into(),
                space_adjust_pt: 0.0,
            }],
            color: Color::Gray { v: 0.5 },
            font_size_pt: 9.0,
            leading_pt: 11.0,
        }];
        let ops = paint_page(
            &page,
            &page_geom(&doc.page_setup, 0),
            &font,
            &ProxyCache::new(),
        );
        let texts: Vec<&str> = ops
            .iter()
            .filter_map(|op| match op {
                PaintOp::Text { text, .. } => Some(text.as_str()),
                _ => None,
            })
            .collect();
        assert_eq!(texts[0], "running head");
    }

    #[test]
    fn press_colour_is_converted_for_the_screen() {
        let (page, geom, font) = sample_page();
        let ops = paint_page(&page, &geom, &font, &ProxyCache::new());
        // The sample's body is 100% K, which must preview as black rather than as raw CMYK bytes.
        assert!(ops
            .iter()
            .any(|op| matches!(op, PaintOp::Text { rgb, .. } if *rgb == [0, 0, 0])));
    }
}
