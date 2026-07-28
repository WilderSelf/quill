//! The Quill desktop window.
//!
//! Deliberately thin. Every decision the shell makes — what to paint, how the viewport maps, how an
//! edit re-flows — lives in `quill-app`'s library and is unit-tested without a window. This file
//! only opens one and forwards input, so the untested surface is as small as it can be.
//!
//! Built with `cargo run -p quill-app --features gui`. The feature is non-default because `eframe`
//! pulls winit and glow, which need system libraries on Linux.

use std::path::PathBuf;

use eframe::egui;
use quill_app::AppState;
use quill_render::PaintOp;

fn main() -> eframe::Result<()> {
    let path = std::env::args().nth(1).map(PathBuf::from);
    let state = match &path {
        Some(p) => match AppState::open(p) {
            Ok(s) => s,
            Err(e) => {
                eprintln!("error opening {}: {e}", p.display());
                std::process::exit(1);
            }
        },
        None => AppState::sample(),
    };

    eframe::run_native(
        "Quill",
        eframe::NativeOptions::default(),
        Box::new(|_cc| Ok(Box::new(Editor { state }))),
    )
}

struct Editor {
    state: AppState,
}

impl eframe::App for Editor {
    fn update(&mut self, ctx: &egui::Context, _frame: &mut eframe::Frame) {
        egui::TopBottomPanel::top("status").show(ctx, |ui| {
            let skipped = self.state.asset_report().skipped;
            ui.horizontal(|ui| {
                ui.label(format!(
                    "{}  —  {} pages",
                    self.state.document().metadata.title,
                    self.state.page_count()
                ));
                if skipped > 0 {
                    // Surfaced rather than swallowed: a document opens with a broken link, but the
                    // user has to be told which art is missing before sending it to print.
                    ui.colored_label(
                        egui::Color32::from_rgb(180, 120, 0),
                        format!("{skipped} linked image(s) unresolved"),
                    );
                }
            });
        });

        egui::CentralPanel::default().show(ctx, |ui| {
            let zoom = self.state.viewport.zoom;
            self.state.viewport.height_pt = ui.available_height() / zoom;
            let total_px = self.state.document_height_pt() * zoom;

            egui::ScrollArea::vertical().show(ui, |ui| {
                let (rect, _) = ui.allocate_exact_size(
                    egui::vec2(ui.available_width(), total_px),
                    egui::Sense::hover(),
                );
                self.state.viewport.scroll_pt =
                    ((ui.clip_rect().top() - rect.top()).max(0.0)) / zoom;

                // Only what is on screen — the shell must not walk 500 pages per frame.
                let ops = self.state.paint_visible();
                let first = self.state.visible_pages().start;
                let origin = egui::pos2(
                    rect.left(),
                    rect.top() + self.state.page_top_pt(first) * zoom,
                );
                draw(ui, origin, &ops, zoom);
            });
        });
    }
}

/// Paint the op list into egui's shape buffer.
///
/// Text uses egui's own font here rather than the tiny-skia outline rasterizer: wiring the raster
/// into a texture is a separate step, and what matters for correctness — *where* each line sits —
/// comes from the op list, which is the same one `quill render` rasterizes.
fn draw(ui: &egui::Ui, origin: egui::Pos2, ops: &[PaintOp], zoom: f32) {
    let painter = ui.painter();
    let at = |x: f32, y: f32| egui::pos2(origin.x + x * zoom, origin.y + y * zoom);
    for op in ops {
        // Each arm returns a different handle type; the shell does not use them.
        match op {
            PaintOp::Page { w_pt, h_pt } => {
                painter.rect_filled(
                    egui::Rect::from_min_size(at(0.0, 0.0), egui::vec2(w_pt * zoom, h_pt * zoom)),
                    0.0_f32,
                    egui::Color32::WHITE,
                );
            }
            PaintOp::TrimGuide {
                x_pt,
                y_pt,
                w_pt,
                h_pt,
            } => {
                painter.rect_stroke(
                    egui::Rect::from_min_size(
                        at(*x_pt, *y_pt),
                        egui::vec2(w_pt * zoom, h_pt * zoom),
                    ),
                    0.0_f32,
                    egui::Stroke::new(1.0_f32, egui::Color32::from_gray(200)),
                    egui::StrokeKind::Outside,
                );
            }
            PaintOp::Text {
                x_pt,
                baseline_pt,
                text,
                size_pt,
                rgb,
                ..
            } => {
                let _ = painter.text(
                    at(*x_pt, *baseline_pt),
                    egui::Align2::LEFT_BOTTOM,
                    text,
                    egui::FontId::proportional(size_pt * zoom),
                    egui::Color32::from_rgb(rgb[0], rgb[1], rgb[2]),
                );
            }
            PaintOp::Rect {
                x_pt,
                y_pt,
                w_pt,
                h_pt,
                fill_rgb,
                stroke,
            } => {
                let rect = egui::Rect::from_min_size(
                    at(*x_pt, *y_pt),
                    egui::vec2(w_pt * zoom, h_pt * zoom),
                );
                if let Some(rgb) = fill_rgb {
                    painter.rect_filled(
                        rect,
                        0.0_f32,
                        egui::Color32::from_rgb(rgb[0], rgb[1], rgb[2]),
                    );
                }
                if let Some((rgb, width_pt)) = stroke {
                    painter.rect_stroke(
                        rect,
                        0.0_f32,
                        egui::Stroke::new(
                            width_pt * zoom,
                            egui::Color32::from_rgb(rgb[0], rgb[1], rgb[2]),
                        ),
                        egui::StrokeKind::Middle,
                    );
                }
            }
            PaintOp::Image {
                x_pt,
                y_pt,
                w_pt,
                h_pt,
                ..
            } => {
                painter.rect_filled(
                    egui::Rect::from_min_size(
                        at(*x_pt, *y_pt),
                        egui::vec2(w_pt * zoom, h_pt * zoom),
                    ),
                    0.0_f32,
                    egui::Color32::from_gray(220),
                );
            }
        }
    }
}
