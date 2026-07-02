//! Headless CLI — exercises the layout/preflight/export pipeline without a GUI. This is the
//! primary way milestone M0 is built and tested. See `specs/0001-pdf-x-export.md`.

use std::process::ExitCode;

use clap::{Args, Parser, Subcommand, ValueEnum};
use quill_core_model::Document;
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

fn load_doc(input: &Option<String>) -> Result<Document, String> {
    match input {
        Some(path) => {
            let text = std::fs::read_to_string(path).map_err(|e| format!("reading {path}: {e}"))?;
            Document::from_json(&text).map_err(|e| format!("parsing {path}: {e}"))
        }
        None => Ok(Document::sample()),
    }
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
            let doc = match load_doc(&args.input) {
                Ok(d) => d,
                Err(e) => {
                    eprintln!("error: {e}");
                    return ExitCode::FAILURE;
                }
            };
            // No ICC supplied here, so the OutputIntent check will report — that is expected
            // for a bare preflight and signals what an export would still need.
            let report = preflight(&doc, &ExportOptions::default());
            print_report(&report);
            if report.passed() {
                ExitCode::SUCCESS
            } else {
                ExitCode::FAILURE
            }
        }

        Command::Export(args) => {
            let doc = match load_doc(&args.input) {
                Ok(d) => d,
                Err(e) => {
                    eprintln!("error: {e}");
                    return ExitCode::FAILURE;
                }
            };
            let opts = ExportOptions {
                version: args.pdfx.into(),
                output_intent_icc: args.icc,
                font_path: args.font,
                force: args.force,
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
