//! CPU rasterizer for the screen paint list — see `specs/0033-screen-render.md`.
//!
//! Backed by `tiny-skia`: pure Rust, permissive (BSD-3-Clause), the same geometry semantics as
//! Skia, and — the reason it was chosen over `skia-safe` — no native C++ build on any leg of the
//! three-OS CI matrix. The paint list in [`crate::paint`] keeps this module swappable; a GPU
//! backend would replace this file and nothing else.

use quill_fonts::{FaceKey, Font, FontFamily, PathCmd};
use tiny_skia::{FillRule, Paint, PathBuilder, Pixmap, PixmapPaint, Rect, Transform};

use crate::paint::{PaintOp, PAPER};
use crate::ProxyCache;

/// A rasterized page: non-premultiplied RGBA8, row-major.
#[derive(Debug, Clone, PartialEq)]
pub struct Raster {
    pub width: u32,
    pub height: u32,
    pub rgba: Vec<u8>,
}

/// Rasterize a paint list at `scale` pixels per point.
///
/// `scale` is the viewport zoom: 1.0 renders a point as a pixel (72 dpi), 2.0 is a HiDPI or
/// zoomed-in view. Returns `None` only for a degenerate (zero-area) page.
pub fn rasterize(
    ops: &[PaintOp],
    family: &FontFamily,
    proxies: &ProxyCache,
    scale: f32,
) -> Option<Raster> {
    let (page_w, page_h) = ops.iter().find_map(|op| match op {
        PaintOp::Page { w_pt, h_pt } => Some((*w_pt, *h_pt)),
        _ => None,
    })?;
    let width = (page_w * scale).round().max(1.0) as u32;
    let height = (page_h * scale).round().max(1.0) as u32;
    let mut pixmap = Pixmap::new(width, height)?;

    for op in ops {
        match op {
            PaintOp::Page { w_pt, h_pt } => {
                fill_rect(&mut pixmap, 0.0, 0.0, *w_pt, *h_pt, PAPER, scale);
            }
            PaintOp::TrimGuide { .. } => {
                // A viewport affordance, not part of the document. Deliberately not drawn into the
                // raster: `quill render` output is compared against expected page content in CI,
                // and a guide line would be indistinguishable from real content in that check.
            }
            PaintOp::Rect {
                x_pt,
                y_pt,
                w_pt,
                h_pt,
                fill_rgb,
                stroke,
            } => {
                if let Some(rgb) = fill_rgb {
                    fill_rect(&mut pixmap, *x_pt, *y_pt, *w_pt, *h_pt, *rgb, scale);
                }
                if let Some((rgb, width_pt)) = stroke {
                    stroke_rect(
                        &mut pixmap,
                        (*x_pt, *y_pt, *w_pt, *h_pt),
                        *rgb,
                        *width_pt,
                        scale,
                    );
                }
            }
            PaintOp::Text {
                x_pt,
                baseline_pt,
                text,
                size_pt,
                space_adjust_pt,
                rgb,
                weight,
                italic,
                tracking_pt,
                baseline_shift_pt,
            } => {
                // The face the op names, resolved through the same family the layout measured
                // with — so the screen draws the face the page will (spec 0064).
                let face = family.select(FaceKey::new(*weight, *italic));
                draw_text(
                    &mut pixmap,
                    family.font(face.index),
                    &TextRun {
                        x_pt: *x_pt,
                        // A baseline shift raises the glyphs without moving the line: positive is
                        // up, and the raster's y grows downward.
                        baseline_pt: *baseline_pt - *baseline_shift_pt,
                        text,
                        size_pt: *size_pt,
                        space_adjust_pt: *space_adjust_pt,
                        rgb: *rgb,
                        tracking_pt: *tracking_pt,
                    },
                    scale,
                );
            }
            PaintOp::Image {
                x_pt,
                y_pt,
                w_pt,
                h_pt,
                asset_id,
                ..
            } => {
                draw_proxy(
                    &mut pixmap,
                    proxies,
                    asset_id,
                    (*x_pt, *y_pt, *w_pt, *h_pt),
                    scale,
                );
            }
        }
    }

    // tiny-skia works premultiplied; hand back straight RGBA, which is what `Proxy` uses and what
    // an image encoder expects.
    let rgba = pixmap
        .pixels()
        .iter()
        .flat_map(|p| {
            let a = p.alpha();
            let demul = |c: u8| {
                if a == 0 {
                    0
                } else {
                    ((c as u32 * 255) / a as u32).min(255) as u8
                }
            };
            [demul(p.red()), demul(p.green()), demul(p.blue()), a]
        })
        .collect();

    Some(Raster {
        width,
        height,
        rgba,
    })
}

fn fill_rect(pixmap: &mut Pixmap, x: f32, y: f32, w: f32, h: f32, rgb: [u8; 3], scale: f32) {
    let Some(rect) = Rect::from_xywh(x * scale, y * scale, w * scale, h * scale) else {
        return;
    };
    let mut paint = Paint::default();
    paint.set_color_rgba8(rgb[0], rgb[1], rgb[2], 255);
    paint.anti_alias = false;
    pixmap.fill_rect(rect, &paint, Transform::identity(), None);
}

/// Stroke a rectangle's outline, centred on the path as PDF's `S` operator does.
///
/// Drawn as four filled bars rather than through a stroked path so the screen and the page agree
/// about which pixels a 0.5 pt line covers: `tiny-skia`'s stroker and PDF's would not have to round
/// a sub-pixel width the same way, and this is geometry both can state exactly.
fn stroke_rect(
    pixmap: &mut Pixmap,
    r: (f32, f32, f32, f32),
    rgb: [u8; 3],
    width_pt: f32,
    scale: f32,
) {
    if width_pt <= 0.0 {
        return;
    }
    let (x, y, w, h) = r;
    let half = width_pt / 2.0;
    // Top, bottom, left, right — each extended by `half` at the ends so the corners are square
    // rather than notched.
    for (bx, by, bw, bh) in [
        (x - half, y - half, w + width_pt, width_pt),
        (x - half, y + h - half, w + width_pt, width_pt),
        (x - half, y - half, width_pt, h + width_pt),
        (x + w - half, y - half, width_pt, h + width_pt),
    ] {
        fill_rect(pixmap, bx, by, bw, bh, rgb, scale);
    }
}

/// One line's worth of drawing parameters, grouped so the call site reads as a unit.
struct TextRun<'a> {
    x_pt: f32,
    baseline_pt: f32,
    text: &'a str,
    size_pt: f32,
    space_adjust_pt: f32,
    rgb: [u8; 3],
    /// Extra advance per glyph (spec 0064) — the screen half of the PDF's `Tc`.
    tracking_pt: f32,
}

/// Draw one line of text by shaping it and filling each glyph's outline.
///
/// Re-shaping here rather than receiving glyphs from layout is a known wart, recorded in spec 0032:
/// `text-layout::Line` carries text plus a justification adjustment, not positioned glyphs. It goes
/// through the *same* shaper the PDF writer uses, and — since spec 0068 — on the *same* whole line
/// rather than word by word, so the two agree glyph for glyph as well as on totals.
///
/// The word-at-a-time version this replaced shaped each word separately, which silently threw away
/// any kern pair straddling a space while `measure_run`, which shapes the whole line, counted it —
/// the same class of defect as the press one spec 0068 closes, in the opposite direction. Position
/// is now accumulated glyph by glyph, and the justification is spent at the space glyph itself,
/// which is exactly where the writer's `TJ` adjustment goes.
fn draw_text(pixmap: &mut Pixmap, font: &Font, run: &TextRun<'_>, scale: f32) {
    let TextRun {
        x_pt,
        baseline_pt,
        text,
        size_pt,
        space_adjust_pt,
        rgb,
        tracking_pt,
    } = *run;
    let mut paint = Paint::default();
    paint.set_color_rgba8(rgb[0], rgb[1], rgb[2], 255);
    paint.anti_alias = true;

    for (gid, x) in glyph_run(font, text, size_pt, space_adjust_pt, tracking_pt).0 {
        if let Some(outline) = font.outline(gid) {
            fill_glyph(
                pixmap,
                &outline.commands,
                (x_pt + x) * scale,
                baseline_pt * scale,
                size_pt * scale / font.units_per_em(),
                &paint,
            );
        }
    }
}

/// Where each glyph of `text` sits along the line, and where the pen ends up.
///
/// Split out of [`draw_text`] so the positions can be asserted without rasterizing: what matters is
/// that the line ends where layout said it would, and a pixel count cannot say that.
///
/// Tracking is spent per glyph, after each one, exactly as a PDF `Tc` spends it (spec 0064); the
/// justification allowance is spent after each inter-word space, exactly where the writer's `TJ`
/// adjustment goes (spec 0068).
fn glyph_run(
    font: &Font,
    text: &str,
    size_pt: f32,
    space_adjust_pt: f32,
    tracking_pt: f32,
) -> (Vec<(u16, f32)>, f32) {
    let mut pen = 0.0f32;
    let mut out = Vec::new();
    for glyph in font.shape(text, size_pt) {
        out.push((glyph.gid, pen));
        pen += glyph.advance_pt + tracking_pt;
        if text
            .get(glyph.cluster as usize..)
            .is_some_and(|rest| rest.starts_with(' '))
        {
            pen += space_adjust_pt;
        }
    }
    (out, pen)
}

/// Fill a glyph outline. Font units are y-up from the baseline; the raster is y-down.
fn fill_glyph(
    pixmap: &mut Pixmap,
    commands: &[PathCmd],
    origin_x: f32,
    baseline_y: f32,
    unit_scale: f32,
    paint: &Paint,
) {
    if commands.is_empty() {
        return; // a space: advances, draws nothing
    }
    let mut pb = PathBuilder::new();
    let px = |x: f32| origin_x + x * unit_scale;
    let py = |y: f32| baseline_y - y * unit_scale;
    for cmd in commands {
        match cmd {
            PathCmd::MoveTo { x, y } => pb.move_to(px(*x), py(*y)),
            PathCmd::LineTo { x, y } => pb.line_to(px(*x), py(*y)),
            PathCmd::QuadTo { cx, cy, x, y } => pb.quad_to(px(*cx), py(*cy), px(*x), py(*y)),
            PathCmd::CurveTo {
                c1x,
                c1y,
                c2x,
                c2y,
                x,
                y,
            } => pb.cubic_to(px(*c1x), py(*c1y), px(*c2x), py(*c2y), px(*x), py(*y)),
            PathCmd::Close => pb.close(),
        }
    }
    if let Some(path) = pb.finish() {
        pixmap.fill_path(&path, paint, FillRule::Winding, Transform::identity(), None);
    }
}

/// Blit a cached screen proxy into the page.
///
/// The full-resolution original is never opened here — that is the core performance strategy
/// (`CLAUDE.md`: never composite full-res on screen). Full-res is touched only at export.
fn draw_proxy(
    pixmap: &mut Pixmap,
    proxies: &ProxyCache,
    asset_id: &str,
    rect: (f32, f32, f32, f32),
    scale: f32,
) {
    let (x_pt, y_pt, w_pt, h_pt) = rect;
    let Some(proxy) = proxies.get(asset_id) else {
        return;
    };
    let Some(mut src) = Pixmap::new(proxy.width, proxy.height) else {
        return;
    };
    for (i, px) in src.pixels_mut().iter_mut().enumerate() {
        let o = i * 4;
        let (r, g, b, a) = (
            proxy.rgba[o],
            proxy.rgba[o + 1],
            proxy.rgba[o + 2],
            proxy.rgba[o + 3],
        );
        *px = tiny_skia::ColorU8::from_rgba(r, g, b, a).premultiply();
    }
    let sx = w_pt * scale / proxy.width as f32;
    let sy = h_pt * scale / proxy.height as f32;
    pixmap.draw_pixmap(
        0,
        0,
        src.as_ref(),
        &PixmapPaint::default(),
        Transform::from_row(sx, 0.0, 0.0, sy, x_pt * scale, y_pt * scale),
        None,
    );
}

/// Encode a raster as a PNG.
pub fn to_png(raster: &Raster) -> Option<Vec<u8>> {
    let mut out = Vec::new();
    {
        let mut enc = png::Encoder::new(&mut out, raster.width, raster.height);
        enc.set_color(png::ColorType::Rgba);
        enc.set_depth(png::BitDepth::Eight);
        let mut writer = enc.write_header().ok()?;
        writer.write_image_data(&raster.rgba).ok()?;
    }
    Some(out)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::paint::paint_page;
    use quill_core_model::{page_geom, Document};
    use quill_layout_engine::lay_out;
    // The shared en-US hyphenator, not `NoHyphenator` (spec 0059). These fixtures used to assert a
    // layout the exporter would never produce, because the screen path hyphenated differently.
    use quill_fonts::HypherHyphenator;

    /// Rasterize the sample document's first page.
    fn render(scale: f32) -> (Raster, FontFamily) {
        let doc = Document::sample();
        let font = FontFamily::bundled();
        let pages = lay_out(&doc, &font, &HypherHyphenator);
        let geom = page_geom(&doc.page_setup, 0);
        let cache = ProxyCache::new();
        let ops = paint_page(&pages[0], &geom, &font, &cache);
        let raster = rasterize(&ops, &font, &cache, scale).expect("raster");
        (raster, font)
    }

    /// Fraction of pixels that are not paper-white.
    fn ink_fraction(r: &Raster) -> f32 {
        let dark = r.rgba.chunks(4).filter(|p| p[0] < 200).count();
        dark as f32 / (r.width * r.height) as f32
    }

    /// Spec 0068's named non-goal, shipped: the screen used to shape each word separately, so a
    /// kern pair straddling a space was lost on screen while `measure_run` — which shapes the whole
    /// line — counted it. The line then drew wider than it measured, the same class of defect as
    /// the press one, in the opposite direction.
    ///
    /// Asserted on where the pen ends up, against the measurement layout broke the line with.
    #[test]
    fn a_line_draws_to_the_width_it_was_measured_at() {
        use quill_text_layout::RunMetrics;
        let font = quill_fonts::Font::bundled();
        for text in [
            "AV Wa To Yo",
            "The officer shuffled a fistful of ruffled affidavits",
        ] {
            let (_, end) = glyph_run(&font, text, 10.0, 0.0, 0.0);
            let measured = font.measure_run(text, 10.0);
            assert!(
                (end - measured).abs() < 0.001,
                "{text:?}: drew to {end}, measured {measured}"
            );
        }
        // A justified line draws its measure plus the space it was given, once per inter-word gap.
        let text = "AV Wa To Yo";
        let (_, end) = glyph_run(&font, text, 10.0, 2.0, 0.0);
        assert!((end - (font.measure_run(text, 10.0) + 3.0 * 2.0)).abs() < 0.001);
        // And tracking is still spent once per shaped glyph, not per character.
        let ligature = "office";
        let (glyphs, end) = glyph_run(&font, ligature, 10.0, 0.0, 1.0);
        assert_eq!(glyphs.len(), 4, "the ffi ligature should form");
        assert!((end - (font.measure_run(ligature, 10.0) + 4.0)).abs() < 0.001);
    }

    #[test]
    fn a_filled_decoration_lands_where_it_was_placed() {
        // Coarse invariants only — a known-colour fill inside the rect and paper outside it — per
        // spec 0033's rule that pixel goldens are flaky across the three-OS matrix. Anti-aliasing
        // is off for rect fills, so the inside pixel is exact.
        let doc = Document::sample();
        let font = FontFamily::bundled();
        let geom = page_geom(&doc.page_setup, 0);
        let cache = ProxyCache::new();
        let ops = vec![
            PaintOp::Page {
                w_pt: geom.media_w,
                h_pt: geom.media_h,
            },
            PaintOp::Rect {
                x_pt: 100.0,
                y_pt: 100.0,
                w_pt: 60.0,
                h_pt: 40.0,
                fill_rgb: Some([0, 0, 0]),
                stroke: None,
            },
        ];
        let r = rasterize(&ops, &font, &cache, 1.0).expect("raster");
        let px = |x: u32, y: u32| {
            let i = ((y * r.width + x) * 4) as usize;
            [r.rgba[i], r.rgba[i + 1], r.rgba[i + 2]]
        };
        assert_eq!(px(130, 120), [0, 0, 0], "inside the rect must be filled");
        assert_ne!(px(50, 50), [0, 0, 0], "outside the rect must stay paper");
        assert_ne!(px(200, 200), [0, 0, 0], "and past it too");
    }

    #[test]
    fn a_stroked_decoration_draws_its_edges_and_not_its_middle() {
        let doc = Document::sample();
        let font = FontFamily::bundled();
        let geom = page_geom(&doc.page_setup, 0);
        let cache = ProxyCache::new();
        let ops = vec![
            PaintOp::Page {
                w_pt: geom.media_w,
                h_pt: geom.media_h,
            },
            PaintOp::Rect {
                x_pt: 100.0,
                y_pt: 100.0,
                w_pt: 60.0,
                h_pt: 40.0,
                fill_rgb: None,
                stroke: Some(([0, 0, 0], 4.0)),
            },
        ];
        let r = rasterize(&ops, &font, &cache, 1.0).expect("raster");
        let px = |x: u32, y: u32| {
            let i = ((y * r.width + x) * 4) as usize;
            [r.rgba[i], r.rgba[i + 1], r.rgba[i + 2]]
        };
        assert_eq!(px(130, 100), [0, 0, 0], "the top edge is stroked");
        assert_eq!(px(100, 120), [0, 0, 0], "the left edge is stroked");
        assert_ne!(px(130, 120), [0, 0, 0], "the middle is not filled");
    }

    #[test]
    fn dimensions_follow_the_page_and_the_scale() {
        let (one, _) = render(1.0);
        let (two, _) = render(2.0);
        assert_eq!(two.width, one.width * 2);
        assert_eq!(two.height, one.height * 2);
        assert_eq!(one.rgba.len(), (one.width * one.height * 4) as usize);
    }

    #[test]
    fn the_page_is_paper_coloured_and_not_blank() {
        // Two failures this catches at once: a page that renders as an empty white sheet (nothing
        // drawn), and one that renders dark (the paper fill missing or the demultiply inverted).
        let (raster, _) = render(2.0);
        let ink = ink_fraction(&raster);
        assert!(ink > 0.001, "page looks blank ({ink:.4} ink)");
        assert!(
            ink < 0.5,
            "page looks like it rendered dark ({ink:.4} ink) — is the paper fill missing?"
        );
    }

    #[test]
    fn a_corner_pixel_is_paper() {
        // The margin area outside any text must be paper, which pins down that the fill happened
        // and that the demultiply round-trips an opaque white.
        let (raster, _) = render(1.0);
        let px = &raster.rgba[0..4];
        assert_eq!(px, &[255, 255, 255, 255], "top-left corner should be paper");
    }

    #[test]
    fn rasterizing_is_deterministic() {
        let (a, _) = render(2.0);
        let (b, _) = render(2.0);
        assert_eq!(a, b);
    }

    #[test]
    fn a_proxy_image_lands_in_the_raster() {
        let mut doc = Document::sample();
        doc.content = vec![quill_core_model::Block::image("map1")];
        doc.assign_missing_block_ids().unwrap();
        let mut cache = ProxyCache::new();
        assert!(cache.insert_png("map1", &crate::tests_support::tiny_png(16, 16)));

        let font = FontFamily::bundled();
        let pages = lay_out(&doc, &font, &HypherHyphenator);
        let geom = page_geom(&doc.page_setup, 0);
        let ops = paint_page(&pages[0], &geom, &font, &cache);
        let raster = rasterize(&ops, &font, &cache, 1.0).expect("raster");
        assert!(
            ink_fraction(&raster) > 0.001,
            "the placed image should put colour on the page"
        );
    }

    #[test]
    fn a_png_round_trips() {
        let (raster, _) = render(1.0);
        let png = to_png(&raster).expect("encode");
        let decoded = crate::decode_png_proxy(&png).expect("the encoder must emit a decodable PNG");
        assert_eq!(decoded.width, raster.width);
        assert_eq!(decoded.height, raster.height);
    }
}
