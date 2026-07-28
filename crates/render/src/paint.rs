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
use quill_fonts::FontFamily;
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
        /// Which face of the family this run is set in (spec 0064). The rasterizer selects it from
        /// the same family the layout measured with, so the screen draws the face the page does.
        weight: u16,
        italic: bool,
        /// Extra advance per glyph, and a vertical offset from the baseline — the screen halves of
        /// the PDF's `Tc` and `Ts`.
        tracking_pt: f32,
        baseline_shift_pt: f32,
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
/// Whether every span of this line resolves to the same ink (spec 0063).
///
/// Colours are `f32` and so not `Eq`; compared by debug form, as the writer does, so the two
/// painters cannot disagree about whether a line needs splitting.
/// The one format this line is set in, or `None` if its spans disagree (spec 0064).
///
/// Against the line's own spans and defaulting to `base`, not against `base` itself: a run long
/// enough to fill a line makes every interior line of it uniformly bold, and painting those lines in
/// the block's face would paint a bold paragraph regular from its second line on.
fn line_format(
    line: &quill_text_layout::Line,
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

/// The one baseline shift this line is set at, or `None` if its spans disagree.
fn line_shift(line: &quill_text_layout::Line, run_shifts: &[f32]) -> Option<f32> {
    if run_shifts.is_empty() || line.spans.is_empty() {
        return Some(0.0);
    }
    let first = run_shifts.get(line.spans[0].run).copied().unwrap_or(0.0);
    line.spans
        .iter()
        .all(|sp| run_shifts.get(sp.run).copied().unwrap_or(0.0) == first)
        .then_some(first)
}

fn line_is_one_ink(line: &quill_text_layout::Line, run_colors: &[quill_core_model::Color]) -> bool {
    let mut inks = line
        .spans
        .iter()
        .map(|sp| run_colors.get(sp.run).map(|c| format!("{c:?}")));
    let first = inks.next().flatten();
    inks.all(|c| c == first)
}

pub fn paint_page(
    page: &LaidOutPage,
    geom: &PageGeom,
    family: &FontFamily,
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
                run_colors,
                run_formats,
                run_shifts,
                weight,
                italic,
                font_size_pt,
                leading_pt,
                ..
            } => {
                // The block's own face and size: what a span with no override resolves to
                // (spec 0064).
                let base = quill_text_layout::RunFormat {
                    size_pt: *font_size_pt,
                    weight: *weight,
                    italic: *italic,
                    tracking_pt: 0.0,
                };
                let rgb = quill_color::to_srgb(color);
                // The baseline comes from the shared font crate, which is also where the PDF writer
                // gets it (spec 0032). One source, so screen and page agree about where a line sits.
                let ascent = quill_text_layout::RunMetrics::ascent_pt(family, *font_size_pt);
                for (i, line) in lines.iter().enumerate() {
                    let x0 = geom.off_x + frame.x_pt + line.indent_pt;
                    let baseline_pt = geom.off_y + frame.y_pt + ascent + i as f32 * leading_pt;
                    // One op per line unless the line's runs really are set in different inks
                    // (spec 0063). The single-ink path is the one every existing document takes,
                    // and it must stay exactly what it was.
                    let uniform = (run_colors.is_empty() || line_is_one_ink(line, run_colors))
                        .then_some(())
                        .and(line_format(line, run_formats, base))
                        .zip(line_shift(line, run_shifts));
                    if let Some((fmt, shift)) = uniform {
                        ops.push(PaintOp::Text {
                            // A line's own left inset (spec 0048), added here and in the PDF writer
                            // from the same field, so the two derivation sites cannot disagree about
                            // which lines are indented.
                            x_pt: x0,
                            baseline_pt,
                            text: line.text.clone(),
                            size_pt: fmt.size_pt,
                            space_adjust_pt: line.space_adjust_pt,
                            rgb,
                            weight: fmt.weight,
                            italic: fmt.italic,
                            tracking_pt: fmt.tracking_pt,
                            baseline_shift_pt: shift,
                        });
                        continue;
                    }
                    // Where each span starts, from the shared helper the PDF writer's own
                    // advance is measured against (spec 0064) — one derivation, so the screen and
                    // the page cannot disagree about where a run begins.
                    let offsets = quill_text_layout::span_offsets(line, run_formats, base, family);
                    let mut at = 0usize;
                    for (i, sp) in line.spans.iter().enumerate() {
                        let end = (at + sp.len_bytes).min(line.text.len());
                        let piece = &line.text[at..end];
                        let fmt = run_formats.get(sp.run).copied().unwrap_or(base);
                        ops.push(PaintOp::Text {
                            x_pt: x0 + offsets.get(i).copied().unwrap_or(0.0),
                            baseline_pt,
                            text: piece.to_string(),
                            size_pt: fmt.size_pt,
                            space_adjust_pt: line.space_adjust_pt,
                            rgb: run_colors.get(sp.run).map_or(rgb, quill_color::to_srgb),
                            weight: fmt.weight,
                            italic: fmt.italic,
                            tracking_pt: fmt.tracking_pt,
                            baseline_shift_pt: run_shifts.get(sp.run).copied().unwrap_or(0.0),
                        });
                        at = end;
                    }
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
            // A link candidate (spec 0052) is a destination attached to a rectangle, not ink. It
            // paints nothing here for the same reason it paints nothing in the PDF writer: the
            // *text* under it is already drawn by its own `Text` block, and a canvas that tinted or
            // underlined links would be showing the reader something the printed page will not have.
            PlacedBlock::Link { .. } => {}
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
    // The shared en-US hyphenator, not `NoHyphenator` (spec 0059). These fixtures used to assert a
    // layout the exporter would never produce, because the screen path hyphenated differently.
    use quill_fonts::HypherHyphenator;

    fn sample_page() -> (LaidOutPage, PageGeom, FontFamily) {
        let doc = Document::sample();
        let font = FontFamily::bundled();
        let pages = lay_out(&doc, &font, &HypherHyphenator);
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

    #[test]
    fn a_link_candidate_paints_nothing() {
        // Spec 0052: a link is a destination attached to a rectangle, not ink. The screen canvas
        // must show exactly what the printed page will — a highlighted or boxed contents entry on
        // screen would be a preview of something the press file does not contain.
        let (mut page, geom, font) = sample_page();
        page.blocks = vec![PlacedBlock::Link {
            frame: quill_core_model::Rect {
                x_pt: 10.0,
                y_pt: 20.0,
                w_pt: 80.0,
                h_pt: 30.0,
            },
            source: quill_core_model::BlockId::UNASSIGNED,
            target_page: 3,
        }];
        let ops = paint_page(&page, &geom, &font, &ProxyCache::new());
        // The page fill is the only op a blank page carries; nothing may be added on top of it.
        let bare = paint_page(
            &LaidOutPage {
                blocks: Vec::new(),
                ..page.clone()
            },
            &geom,
            &font,
            &ProxyCache::new(),
        );
        assert_eq!(ops, bare, "a link candidate must contribute no paint op");
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
        let expected =
            geom.off_y + frame_y + quill_text_layout::RunMetrics::ascent_pt(&font, first_text.1);
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
        let font = FontFamily::bundled();
        let pages = lay_out(&doc, &font, &HypherHyphenator);
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
        let font = FontFamily::bundled();
        let pages = lay_out(&doc, &font, &HypherHyphenator);
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

        let font = FontFamily::bundled();
        let pages = lay_out(&doc, &font, &HypherHyphenator);
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
        let font = FontFamily::bundled();
        let mut page = lay_out(&doc, &font, &HypherHyphenator)[0].clone();
        page.statics = vec![PlacedBlock::Text {
            run_formats: Vec::new(),
            run_shifts: Vec::new(),
            weight: 400,
            italic: false,
            run_colors: Vec::new(),
            source: quill_core_model::BlockId::UNASSIGNED,
            frame: quill_core_model::Rect {
                x_pt: 0.0,
                y_pt: 600.0,
                w_pt: 400.0,
                h_pt: 12.0,
            },
            lines: vec![quill_text_layout::Line::single_run(
                "running head",
                0.0,
                0.0,
            )],
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

    /// The screen half of the same defect the PDF writer had: a line lying wholly inside a bold run
    /// is uniform, and painting it in the *block's* face would paint every interior line of a long
    /// bold run regular — with the page drawing it bold.
    #[test]
    fn a_line_inside_a_bold_run_is_painted_bold() {
        use quill_text_layout::{Line, RunFormat, Span};

        let span = |run: usize, len: usize| Span {
            run,
            len_bytes: len,
        };
        let line = |text: &str, run: usize| Line {
            text: text.into(),
            spans: vec![span(run, text.len())],
            space_adjust_pt: 0.0,
            indent_pt: 0.0,
        };
        let (mut page, geom, family) = sample_page();
        page.blocks = vec![PlacedBlock::Text {
            source: quill_core_model::BlockId::UNASSIGNED,
            frame: quill_core_model::Rect {
                x_pt: 0.0,
                y_pt: 0.0,
                w_pt: 300.0,
                h_pt: 36.0,
            },
            lines: vec![line("abc", 0), line("def", 1), line("gh", 2)],
            color: quill_core_model::Color::Gray { v: 0.0 },
            run_colors: Vec::new(),
            run_formats: vec![
                RunFormat::plain(10.0),
                RunFormat {
                    size_pt: 10.0,
                    weight: 700,
                    italic: false,
                    tracking_pt: 0.0,
                },
                RunFormat::plain(10.0),
            ],
            run_shifts: Vec::new(),
            weight: 400,
            italic: false,
            font_size_pt: 10.0,
            leading_pt: 12.0,
        }];
        page.statics.clear();

        let ops = paint_page(&page, &geom, &family, &ProxyCache::new());
        let weights: Vec<u16> = ops
            .iter()
            .filter_map(|op| match op {
                PaintOp::Text { weight, .. } => Some(*weight),
                _ => None,
            })
            .collect();
        assert_eq!(weights, vec![400, 700, 400], "ops: {ops:?}");
    }
}
