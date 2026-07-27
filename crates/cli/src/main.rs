//! Headless CLI — exercises the layout/preflight/export pipeline without a GUI. This is the
//! primary way milestone M0 is built and tested. See `specs/0001-pdf-x-export.md`.

use std::path::{Path, PathBuf};
use std::process::ExitCode;

use clap::{Args, Parser, Subcommand, ValueEnum};
use quill_core_model::{Document, Tpub};
use quill_export_pdf::{
    export, preflight, synth_cmyk_profile, ExportOptions, PdfxVersion, PreflightReport, Severity,
};

#[derive(Parser)]
#[command(
    name = "quill",
    version,
    about = "Quill TTRPG desktop publishing (CLI)"
)]
struct Cli {
    #[command(subcommand)]
    command: Command,
}

#[derive(Subcommand)]
enum Command {
    /// Print the built-in sample document's manifest JSON.
    Sample,
    /// Run print preflight against a document (uses the built-in sample if no path given).
    Preflight(DocArgs),
    /// Export a document to press-ready PDF/X (preflight + write).
    Export(ExportArgs),
    /// Write a synthesized CMYK OutputIntent ICC profile (for testing/CI; not a real press profile).
    SynthIcc {
        /// Output path for the `.icc` file.
        output: String,
    },
    /// Pack a `document.json` and its linked assets into a portable `.tpub` container.
    Pack(PackArgs),
    /// Render one page to a PNG, as the on-screen canvas would draw it (spec 0033).
    Render(RenderArgs),
}

#[derive(Args)]
struct RenderArgs {
    /// Path to a `.tpub` or `document.json` (optional; falls back to the built-in sample).
    input: Option<String>,
    /// Zero-based page to render.
    #[arg(long, default_value_t = 0)]
    page: usize,
    /// Pixels per point. 1.0 is 72 dpi; 2.0 is a HiDPI or zoomed view.
    #[arg(long, default_value_t = 2.0)]
    scale: f32,
    /// Output PNG path.
    #[arg(short, long)]
    output: String,
}

#[derive(Args)]
struct PackArgs {
    /// Path to the `document.json` to pack. Its linked assets are resolved relative to it.
    input: String,
    /// Output `.tpub` path.
    #[arg(short, long)]
    output: String,
}

#[derive(Clone, Copy, ValueEnum)]
enum PdfxArg {
    X1a,
    X3,
}

impl From<PdfxArg> for PdfxVersion {
    fn from(v: PdfxArg) -> Self {
        match v {
            PdfxArg::X1a => PdfxVersion::X1a2001,
            PdfxArg::X3 => PdfxVersion::X3_2002,
        }
    }
}

#[derive(Args)]
struct DocArgs {
    /// Path to a `.tpub` `document.json` (optional; falls back to the built-in sample).
    input: Option<String>,
}

#[derive(Args)]
struct ExportArgs {
    /// Path to a `.tpub` `document.json` (optional; falls back to the built-in sample).
    input: Option<String>,
    /// Output PDF path.
    #[arg(short, long)]
    output: String,
    /// PDF/X conformance level.
    #[arg(long, value_enum, default_value_t = PdfxArg::X1a)]
    pdfx: PdfxArg,
    /// ICC profile for the PDF/X OutputIntent.
    #[arg(long)]
    icc: String,
    /// TrueType (.ttf) or CFF OpenType (.otf) font to embed; defaults to the bundled Source Serif 4.
    #[arg(long)]
    font: Option<String>,
    /// Export even if preflight fails.
    #[arg(long)]
    force: bool,
}

/// A loaded document plus the directory its relative asset paths resolve against (spec 0025).
struct Loaded {
    doc: Document,
    asset_root: PathBuf,
}

/// Load a `.tpub` container or a bare `document.json`.
///
/// A `.tpub` is extracted next to itself (`book.tpub` → `book.tpub.d/`) so that repeated opens are
/// idempotent and the extracted assets are findable rather than hidden in a temp directory that
/// nothing owns. A bare `document.json` resolves its assets against its own directory — not the
/// process working directory, which is what the writer used to assume.
fn load_doc(input: &Option<String>) -> Result<Loaded, String> {
    let Some(path) = input else {
        return Ok(Loaded {
            doc: Document::sample(),
            asset_root: PathBuf::from("."),
        });
    };
    let path = Path::new(path);

    if path
        .extension()
        .is_some_and(|e| e.eq_ignore_ascii_case("tpub"))
    {
        let extract_to = path.with_extension("tpub.d");
        let opened = Tpub::open_into(path, &extract_to).map_err(|e| e.to_string())?;
        return Ok(Loaded {
            doc: opened.document,
            asset_root: opened.asset_root,
        });
    }

    let text =
        std::fs::read_to_string(path).map_err(|e| format!("reading {}: {e}", path.display()))?;
    let doc = Document::from_json(&text).map_err(|e| format!("{}: {e}", path.display()))?;
    Ok(Loaded {
        doc,
        asset_root: path
            .parent()
            .filter(|p| !p.as_os_str().is_empty())
            .unwrap_or(Path::new("."))
            .to_path_buf(),
    })
}

fn print_report(report: &PreflightReport) {
    if report.findings.is_empty() {
        println!("preflight: no findings.");
    }
    for f in &report.findings {
        let tag = match f.severity {
            Severity::Error => "error",
            Severity::Warning => "warn",
        };
        println!("  [{tag}] {:?}: {}", f.check, f.message);
    }
}

fn main() -> ExitCode {
    match Cli::parse().command {
        Command::Sample => match Document::sample().to_json() {
            Ok(json) => {
                println!("{json}");
                ExitCode::SUCCESS
            }
            Err(e) => {
                eprintln!("error: {e}");
                ExitCode::FAILURE
            }
        },

        Command::Preflight(args) => {
            let loaded = match load_doc(&args.input) {
                Ok(d) => d,
                Err(e) => {
                    eprintln!("error: {e}");
                    return ExitCode::FAILURE;
                }
            };
            // No ICC supplied here, so the OutputIntent check will report — that is expected
            // for a bare preflight and signals what an export would still need.
            let report = preflight(
                &loaded.doc,
                &ExportOptions {
                    asset_root: loaded.asset_root,
                    ..Default::default()
                },
            );
            print_report(&report);
            if report.passed() {
                ExitCode::SUCCESS
            } else {
                ExitCode::FAILURE
            }
        }

        Command::Export(args) => {
            let loaded = match load_doc(&args.input) {
                Ok(d) => d,
                Err(e) => {
                    eprintln!("error: {e}");
                    return ExitCode::FAILURE;
                }
            };
            let doc = loaded.doc;
            let opts = ExportOptions {
                version: args.pdfx.into(),
                output_intent_icc: args.icc,
                font_path: args.font,
                force: args.force,
                asset_root: loaded.asset_root,
            };
            print_report(&preflight(&doc, &opts));

            let mut bytes = Vec::new();
            match export(&doc, &opts, &mut bytes) {
                Ok(()) => match std::fs::write(&args.output, &bytes) {
                    Ok(()) => {
                        println!("wrote {} ({} bytes)", args.output, bytes.len());
                        ExitCode::SUCCESS
                    }
                    Err(e) => {
                        eprintln!("error writing {}: {e}", args.output);
                        ExitCode::FAILURE
                    }
                },
                Err(e) => {
                    eprintln!("export failed: {e}");
                    ExitCode::FAILURE
                }
            }
        }

        Command::Pack(args) => {
            let loaded = match load_doc(&Some(args.input)) {
                Ok(d) => d,
                Err(e) => {
                    eprintln!("error: {e}");
                    return ExitCode::FAILURE;
                }
            };
            // Read every linked asset up front. An asset that cannot be read is a hard failure
            // here, not a warning: a `.tpub` is meant to be portable, and quietly packing a
            // container with a missing image produces a document that is broken everywhere else it
            // is opened.
            let mut payload: Vec<(String, Vec<u8>)> = Vec::new();
            for asset in &loaded.doc.assets {
                let path = loaded.asset_root.join(&asset.path);
                match std::fs::read(&path) {
                    Ok(bytes) => payload.push((asset.path.clone(), bytes)),
                    Err(e) => {
                        eprintln!("error: asset '{}' ({}): {e}", asset.id, path.display());
                        return ExitCode::FAILURE;
                    }
                }
            }
            let entries: Vec<(&str, &[u8])> = payload
                .iter()
                .map(|(n, b)| (n.as_str(), b.as_slice()))
                .collect();
            match Tpub::write(&loaded.doc, Path::new(&args.output), &entries) {
                Ok(()) => {
                    println!("wrote {} ({} assets)", args.output, entries.len());
                    ExitCode::SUCCESS
                }
                Err(e) => {
                    eprintln!("error: {e}");
                    ExitCode::FAILURE
                }
            }
        }

        Command::Render(args) => {
            let loaded = match load_doc(&args.input) {
                Ok(d) => d,
                Err(e) => {
                    eprintln!("error: {e}");
                    return ExitCode::FAILURE;
                }
            };
            let font = quill_fonts::Font::bundled();
            // Screen layout goes through the same shaper the exporter uses (spec 0032), so what is
            // drawn here is what the PDF would contain — that is the whole point of the shared crate.
            let pages =
                quill_layout_engine::lay_out(&loaded.doc, &font, &quill_text_layout::NoHyphenator);
            let Some(page) = pages.get(args.page) else {
                eprintln!(
                    "error: page {} out of range (document has {})",
                    args.page,
                    pages.len()
                );
                return ExitCode::FAILURE;
            };

            // Populate screen proxies from the document's linked assets. A missing link is skipped,
            // not fatal: on screen it is recoverable and visible.
            let mut proxies = quill_render::ProxyCache::new();
            let report = proxies.populate_from_assets(&loaded.doc.assets, &loaded.asset_root);

            let geom = quill_core_model::page_geom(&loaded.doc.page_setup, args.page);
            let ops = quill_render::paint_page(page, &geom, &font, &proxies);
            let Some(raster) = quill_render::rasterize(&ops, &font, &proxies, args.scale) else {
                eprintln!("error: page has no drawable area");
                return ExitCode::FAILURE;
            };
            let Some(png) = quill_render::to_png(&raster) else {
                eprintln!("error: encoding PNG");
                return ExitCode::FAILURE;
            };
            match std::fs::write(&args.output, &png) {
                Ok(()) => {
                    println!(
                        "wrote {} ({}x{} px, {} ops, {} proxies generated, {} skipped)",
                        args.output,
                        raster.width,
                        raster.height,
                        ops.len(),
                        report.generated,
                        report.skipped
                    );
                    ExitCode::SUCCESS
                }
                Err(e) => {
                    eprintln!("error writing {}: {e}", args.output);
                    ExitCode::FAILURE
                }
            }
        }

        Command::SynthIcc { output } => match std::fs::write(&output, synth_cmyk_profile()) {
            Ok(()) => {
                println!("wrote {output}");
                ExitCode::SUCCESS
            }
            Err(e) => {
                eprintln!("error writing {output}: {e}");
                ExitCode::FAILURE
            }
        },
    }
}
