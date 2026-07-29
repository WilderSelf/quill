//! Spec 0082: the screen and the press must agree about what a transparent pixel looks like.
//!
//! PDF/X-1a and PDF/X-3 both forbid live transparency, so the press file has to decide what an
//! alpha-bearing pixel becomes. Until spec 0082 it decided by *dropping* the alpha channel and
//! converting the RGB stored underneath — which PNG leaves entirely unconstrained, and which most
//! encoders write as `(0,0,0,0)` — so a transparent surround left the screen as paper and reached
//! the printer as solid black. `CLAUDE.md` states the rule this breaks: one shaper for screen and
//! press, so they cannot drift.
//!
//! It lives in `cli` for `hyphenation_parity.rs`'s reason: `cli` is the only crate that depends on
//! both paths, so it is the only place the two can be compared without inventing a dependency edge.

use quill_core_model::{Asset, Block, Document};
use quill_export_pdf::{synth_cmyk_profile, ExportOptions};
use quill_render::{PaintOp, ProxyCache};

/// A 64x64 PNG whose left half is transparent with black stored underneath, and whose right half is
/// an opaque red. Large enough that a sample well inside either half is unambiguous.
fn half_transparent_png() -> Vec<u8> {
    const N: u32 = 64;
    let mut rgba = vec![0u8; (N * N * 4) as usize];
    for y in 0..N {
        for x in 0..N {
            let i = ((y * N + x) * 4) as usize;
            if x >= N / 2 {
                rgba[i] = 255; // opaque red
                rgba[i + 3] = 255;
            }
            // the left half stays (0,0,0,0)
        }
    }
    let mut out = Vec::new();
    {
        let mut enc = png::Encoder::new(&mut out, N, N);
        enc.set_color(png::ColorType::Rgba);
        enc.set_depth(png::BitDepth::Eight);
        let mut w = enc.write_header().expect("png header");
        w.write_image_data(&rgba).expect("png data");
    }
    out
}

fn doc(dir: &std::path::Path) -> Document {
    let mut doc = Document::sample();
    doc.assets = vec![Asset {
        id: "logo".into(),
        path: "logo.png".into(),
        px_w: 64,
        px_h: 64,
        dpi: 300.0,
        line_art: false,
        has_alpha: true,
    }];
    doc.content = vec![Block::image("logo")];
    doc.assign_missing_block_ids().expect("ids");
    let _ = dir;
    doc
}

/// Every stream payload in a PDF, zlib-inflated when it is zlib. The image XObject is
/// `/FlateDecode`d like everything else the writer emits.
fn inflated_streams(pdf: &[u8]) -> Vec<Vec<u8>> {
    use std::io::Read;
    let mut out = Vec::new();
    let mut i = 0;
    while let Some(rel) = pdf[i..]
        .windows(6)
        .position(|w| w == b"stream")
        .map(|p| p + i)
    {
        let mut start = rel + 6;
        if pdf[..rel].ends_with(b"end") {
            i = start;
            continue;
        }
        while start < pdf.len() && (pdf[start] == b'\r' || pdf[start] == b'\n') {
            start += 1;
        }
        let Some(end) = pdf[start..]
            .windows(9)
            .position(|w| w == b"endstream")
            .map(|p| p + start)
        else {
            break;
        };
        let raw = &pdf[start..end];
        let mut inflated = Vec::new();
        match flate2::read::ZlibDecoder::new(raw).read_to_end(&mut inflated) {
            Ok(_) => out.push(inflated),
            Err(_) => out.push(raw.to_vec()),
        }
        i = end;
    }
    out
}

/// What the press puts on paper for the source pixel at `(x, y)`, as sRGB.
fn press_pixel(pdf: &[u8], x: usize, y: usize) -> [u8; 3] {
    let pixels = inflated_streams(pdf)
        .into_iter()
        .find(|s| s.len() == 64 * 64 * 4)
        .expect("an image XObject of 64x64 CMYK pixels");
    let o = (y * 64 + x) * 4;
    quill_color::to_srgb(&quill_core_model::Color::Cmyk {
        c: pixels[o] as f32 / 255.0,
        m: pixels[o + 1] as f32 / 255.0,
        y: pixels[o + 2] as f32 / 255.0,
        k: pixels[o + 3] as f32 / 255.0,
    })
}

#[test]
fn screen_and_press_agree_about_a_transparent_pixel() {
    let dir = std::env::temp_dir().join(format!("quill_0082_parity_{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    std::fs::write(dir.join("logo.png"), half_transparent_png()).unwrap();
    let icc = dir.join("out.icc");
    std::fs::write(&icc, synth_cmyk_profile()).unwrap();

    let doc = doc(&dir);

    // --- the press file ------------------------------------------------------------------------
    let opts = ExportOptions {
        output_intent_icc: icc.to_string_lossy().into_owned(),
        asset_root: dir.clone(),
        ..Default::default()
    };
    let mut pdf = Vec::new();
    quill_export_pdf::export(&doc, &opts, &mut pdf).expect("export");

    // --- the screen ----------------------------------------------------------------------------
    // The real screen path, end to end: the same layout, the proxy cache the app fills, and the
    // rasterizer that paints it — so what is compared is what a viewer sees, not an intermediate.
    let font = quill_fonts::FontFamily::bundled();
    let pages = quill_render::lay_out_for_screen(&doc, &font);
    let page = pages.first().expect("one page");
    let mut proxies = ProxyCache::new();
    proxies.populate_from_assets(&doc.assets, &dir);
    let geom = quill_core_model::page_geom(&doc.page_setup, 0);
    let ops = quill_render::paint_page(page, &geom, &font, &proxies);
    let raster = quill_render::rasterize(&ops, &font, &proxies, 1.0).expect("raster");

    let (ix, iy, iw, ih) = ops
        .iter()
        .find_map(|op| match op {
            PaintOp::Image {
                x_pt,
                y_pt,
                w_pt,
                h_pt,
                ..
            } => Some((*x_pt, *y_pt, *w_pt, *h_pt)),
            _ => None,
        })
        .expect("the page draws the image");

    let screen_at = |fx: f32, fy: f32| -> [u8; 3] {
        let px = (ix + iw * fx).round() as usize;
        let py = (iy + ih * fy).round() as usize;
        let o = (py * raster.width as usize + px) * 4;
        [raster.rgba[o], raster.rgba[o + 1], raster.rgba[o + 2]]
    };

    // --- the comparison ------------------------------------------------------------------------
    // A quarter of the way across is deep inside the transparent half; three quarters is deep
    // inside the opaque one. The second sample is what stops this passing against a screen path
    // that simply drew nothing.
    let close = |a: [u8; 3], b: [u8; 3], what: &str| {
        for i in 0..3 {
            assert!(
                (a[i] as i32 - b[i] as i32).abs() <= 3,
                "{what}: screen {a:?} vs press {b:?}"
            );
        }
    };
    close(
        screen_at(0.25, 0.5),
        press_pixel(&pdf, 16, 32),
        "the transparent half",
    );
    close(
        screen_at(0.75, 0.5),
        press_pixel(&pdf, 48, 32),
        "the opaque half",
    );
    // And say plainly what they must agree *on*, so a future change that made both wrong in the
    // same way would still fail here.
    assert_eq!(
        press_pixel(&pdf, 16, 32),
        [255, 255, 255],
        "a transparent pixel is paper, on both paths"
    );

    let _ = std::fs::remove_dir_all(&dir);
}
