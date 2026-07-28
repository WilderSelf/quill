//! Headless CLI — exercises the layout/preflight/export pipeline without a GUI. This is the
//! primary way milestone M0 is built and tested. See `specs/0001-pdf-x-export.md`.

use std::path::{Path, PathBuf};
use std::process::ExitCode;

use clap::{Args, Parser, Subcommand, ValueEnum};
use quill_core_model::{import, Document, Template, Tpub};
use quill_export_pdf::{
    export, preflight, synth_cmyk_profile, ExportOptions, ExportProfile, PdfxVersion, PodPreset,
    PreflightReport, Severity,
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
    /// Start a new document from a built-in template (spec 0036).
    New(NewArgs),
    /// Import the authoring syntax into a `.tpub` (spec 0043).
    Import(ImportArgs),
    /// List the bundled POD presets with their numbers and provenance (spec 0049).
    Presets,
}

#[derive(Args)]
struct ImportArgs {
    /// Path to the source file.
    input: String,
    /// Output `.tpub` path.
    #[arg(short, long)]
    output: String,
    /// Template slug supplying page setup, styles and masters. See `quill new --list`.
    #[arg(long, default_value = "adventure")]
    template: String,
}

#[derive(Args)]
struct NewArgs {
    /// Template slug. See `--list`.
    #[arg(long)]
    template: Option<String>,
    /// Path to a user-authored template file (spec 0053). Mutually exclusive with `--template`.
    ///
    /// A path, not a name: there is deliberately no template directory and no search path, so
    /// nothing can go missing behind the user's back.
    #[arg(long, conflicts_with = "template")]
    from: Option<String>,
    /// Output `.tpub` path.
    #[arg(short, long)]
    output: Option<String>,
    /// List the built-in templates and exit.
    #[arg(long)]
    list: bool,
    /// POD preset seeding the page setup's bleed (and trim, if the template's is not one the
    /// preset lists). See `quill presets`.
    #[arg(long)]
    preset: Option<String>,
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

/// Which of the two files to write (spec 0052).
#[derive(Clone, Copy, PartialEq, Eq, ValueEnum)]
enum ProfileArg {
    /// Press-ready PDF/X. Requires `--icc`. The default.
    Press,
    /// The customer's file: clickable contents links, **not** press-ready.
    Screen,
}

impl From<ProfileArg> for ExportProfile {
    fn from(v: ProfileArg) -> Self {
        match v {
            ProfileArg::Press => ExportProfile::Press,
            ProfileArg::Screen => ExportProfile::Screen,
        }
    }
}

#[derive(Args)]
struct DocArgs {
    /// Path to a `.tpub` `document.json` (optional; falls back to the built-in sample).
    input: Option<String>,
    /// POD preset supplying the preflight thresholds. Defaults to `generic`, which is exactly the
    /// values quill has always checked. See `quill presets`.
    #[arg(long)]
    preset: Option<String>,
}

#[derive(Args)]
struct ExportArgs {
    /// Path to a `.tpub` `document.json` (optional; falls back to the built-in sample).
    input: Option<String>,
    /// Output PDF path.
    #[arg(short, long)]
    output: String,
    /// PDF/X conformance level. Defaults to the level the selected preset states (X-1a for every
    /// bundled preset), rather than to a constant that could disagree with it. Ignored by
    /// `--profile screen`, which claims no conformance.
    #[arg(long, value_enum)]
    pdfx: Option<PdfxArg>,
    /// POD preset supplying the preflight thresholds and the default conformance level. See
    /// `quill presets`.
    #[arg(long)]
    preset: Option<String>,
    /// Export profile: `press` (PDF/X, the default) or `screen` (clickable links, not press-ready).
    #[arg(long, value_enum, default_value_t = ProfileArg::Press)]
    profile: ProfileArg,
    /// ICC profile for the PDF/X OutputIntent. Required by `--profile press`; optional for
    /// `--profile screen`, which writes no OutputIntent but will use a supplied profile for
    /// RGB image conversion.
    #[arg(long)]
    icc: Option<String>,
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

/// Resolve `--preset`, or the conservative default when the flag is absent (spec 0049).
///
/// An unknown name is an error listing the available ones — never a silent fall back to a default,
/// which would preflight against a printer the user did not choose. Selecting an *unconfirmed*
/// preset prints a note to stderr carrying its provenance: a preset whose numbers were not read
/// from the vendor must not be able to look authoritative just because it has the vendor's name on
/// it.
fn resolve_preset(name: &Option<String>) -> Result<PodPreset, String> {
    let Some(name) = name else {
        return Ok(PodPreset::generic());
    };
    let Some(preset) = PodPreset::by_name(name) else {
        return Err(format!(
            "unknown preset '{name}'; available: {}",
            PodPreset::names().join(", ")
        ));
    };
    if !preset.confirmed {
        eprintln!(
            "note: preset '{}' is UNCONFIRMED — {}",
            preset.name, preset.source
        );
    }
    Ok(preset)
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
    // A shorter report under a profile that checks fewer things must say so (spec 0052). "No
    // findings" over an unstated subset of the checks is the shape of a false pass.
    if !report.skipped.is_empty() {
        println!(
            "  applied: {}",
            report
                .applied()
                .iter()
                .map(|c| format!("{c:?}"))
                .collect::<Vec<_>>()
                .join(", ")
        );
        for s in &report.skipped {
            println!("  skipped: {:?} — {}", s.check, s.reason);
        }
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
            let preset = match resolve_preset(&args.preset) {
                Ok(p) => p,
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
                    preset,
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
            let preset = match resolve_preset(&args.preset) {
                Ok(p) => p,
                Err(e) => {
                    eprintln!("error: {e}");
                    return ExitCode::FAILURE;
                }
            };
            let doc = loaded.doc;
            // Press requires an OutputIntent, and this is a hard error naming the flag rather than
            // a preflight finding: a press export with no profile is a mistake to correct, not a
            // document to fix. Screen may omit it entirely.
            if args.profile == ProfileArg::Press && args.icc.is_none() {
                eprintln!(
                    "error: --icc is required by --profile press (PDF/X needs an ICC \
                     OutputIntent); pass --profile screen for a non-press, linked PDF"
                );
                return ExitCode::FAILURE;
            }
            if args.profile == ProfileArg::Screen {
                // Printed before anything else, and unconditionally. A user who ships this file to
                // a printer has to have read past a line that says not to.
                println!(
                    "profile: screen — NOT press-ready. This file carries clickable contents \
                     links and claims no PDF/X conformance (no GTS_PDFXVersion, no OutputIntent). \
                     Send the --profile press export to the printer."
                );
            }
            let opts = ExportOptions {
                // An explicit `--pdfx` wins; otherwise the preset states the conformance level, so
                // the flag and the preset cannot silently disagree.
                version: args.pdfx.map_or(preset.pdfx, Into::into),
                profile: args.profile.into(),
                output_intent_icc: args.icc.unwrap_or_default(),
                font_path: args.font,
                force: args.force,
                asset_root: loaded.asset_root,
                preset,
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

        Command::New(args) => new_document(args),

        Command::Import(args) => import_document(args),

        Command::Presets => {
            // Provenance a user cannot read is provenance that is not doing its job, so `source`
            // and `retrieved` are printed, not just the numbers.
            for p in PodPreset::bundled() {
                println!("{} — {}", p.name, p.title);
                println!(
                    "  bleed {}pt · safety {}pt · ink {}% · {}/{} dpi · {}",
                    p.bleed_pt,
                    p.safety_pt,
                    p.max_ink_pct,
                    p.min_dpi_color,
                    p.min_dpi_line_art,
                    p.pdfx.identifier()
                );
                let trims = if p.trim_sizes.is_empty() {
                    "no trim catalogue stated".to_string()
                } else {
                    p.trim_sizes
                        .iter()
                        .map(|s| format!("{}x{}pt", s.w_pt, s.h_pt))
                        .collect::<Vec<_>>()
                        .join(", ")
                };
                println!("  trims: {trims}");
                println!(
                    "  {} · retrieved {}",
                    if p.confirmed {
                        "CONFIRMED"
                    } else {
                        "UNCONFIRMED"
                    },
                    p.retrieved
                );
                println!("  source: {}", p.source);
            }
            ExitCode::SUCCESS
        }
    }
}

/// `quill import` — a thin caller over `quill_core_model::import` (spec 0043).
///
/// Deliberately thin: the importer is a library function so the app can reuse it without going
/// through a process.
fn import_document(args: ImportArgs) -> ExitCode {
    let Some(template) = Template::by_name(&args.template) else {
        eprintln!(
            "error: unknown template '{}'; available: {}",
            args.template,
            Template::names().join(", ")
        );
        return ExitCode::FAILURE;
    };
    let source = match std::fs::read_to_string(&args.input) {
        Ok(s) => s,
        Err(e) => {
            eprintln!("error reading {}: {e}", args.input);
            return ExitCode::FAILURE;
        }
    };
    let imported = match import(&source, template) {
        Ok(i) => i,
        Err(e) => {
            eprintln!("error: {e}");
            return ExitCode::FAILURE;
        }
    };
    // Warnings are printed, never swallowed: every one means something was kept as plain text
    // rather than laid out the way it was written.
    for w in &imported.warnings {
        eprintln!("warning: line {}: {}", w.line, w.message);
    }
    match Tpub::write(&imported.document, Path::new(&args.output), &[]) {
        Ok(()) => {
            println!(
                "wrote {} ({} blocks from template '{}', {} warning(s))",
                args.output,
                imported.document.content.len(),
                args.template,
                imported.warnings.len()
            );
            ExitCode::SUCCESS
        }
        Err(e) => {
            eprintln!("error: {e}");
            ExitCode::FAILURE
        }
    }
}

/// `quill new` — start a document from a built-in template (spec 0036) or a user-authored template
/// file (spec 0053).
fn new_document(args: NewArgs) -> ExitCode {
    if args.list {
        // `--list` names the *bundled* set, which is what it has always meant: a user-authored
        // template is a path the user already has, not something to enumerate.
        for t in Template::bundled() {
            println!("{:<10} {} — {}", t.name, t.title, t.description);
        }
        return ExitCode::SUCCESS;
    }

    let template = match resolve_template(&args) {
        Ok(t) => t,
        Err(e) => {
            eprintln!("error: {e}");
            return ExitCode::FAILURE;
        }
    };
    let Some(output) = args.output.as_deref() else {
        eprintln!("error: --output is required");
        return ExitCode::FAILURE;
    };

    let doc_template = &template;
    let mut doc = Document::from_template(doc_template);

    // Seed from the preset (spec 0049), with the precedence spec 0053 argues for and this resolves
    // to: **the preset supplies the bleed, the template keeps its trim, and a disagreement is
    // reported rather than acted on.**
    //
    // Bleed is a press floor living entirely outside the trim box, so taking the preset's costs the
    // design nothing. The trim is the opposite: a template's furniture is authored *against* it —
    // the bundled folios derive their y from the page height and their width from the trim — so
    // retrimming moves the page without moving the geometry authored for it, and nothing at layout
    // time catches it, because furniture does not participate in the flow. Silently relocating a
    // folio onto the last line of text, or past the trim, is exactly the press corruption
    // `CLAUDE.md` says to prefer a visible failure over.
    if let Some(name) = args.preset.as_deref() {
        let preset = match resolve_preset(&Some(name.to_string())) {
            Ok(p) => p,
            Err(e) => {
                eprintln!("error: {e}");
                return ExitCode::FAILURE;
            }
        };
        doc.page_setup.bleed_pt = doc.page_setup.bleed_pt.max(preset.bleed_pt);
        if !preset.lists_trim(doc.page_setup.trim) {
            eprintln!(
                "warning: this template trims to {}x{}pt, which preset '{}' does not list; \
                 keeping the template's trim — confirm it with the printer",
                doc.page_setup.trim.w_pt, doc.page_setup.trim.h_pt, preset.name
            );
        }
    }

    // A template links no assets, so the container carries the manifest alone.
    match Tpub::write(&doc, Path::new(output), &[]) {
        Ok(()) => {
            println!("wrote {output} from template '{}'", template.name);
            ExitCode::SUCCESS
        }
        Err(e) => {
            eprintln!("error: {e}");
            ExitCode::FAILURE
        }
    }
}

/// Find the template `quill new` was asked for: a bundled slug, or a file.
///
/// Owned rather than `&'static`, because a loaded template is not static and a document does not
/// care which of the two it came from. `clap` has already refused `--template` and `--from`
/// together, so exactly one of the three arms below applies.
fn resolve_template(args: &NewArgs) -> Result<Template, String> {
    if let Some(path) = args.from.as_deref() {
        let text = std::fs::read_to_string(path).map_err(|e| format!("reading {path}: {e}"))?;
        // The typed load error names what is wrong with the *template* — a malformed file and one
        // written by a newer Quill are different problems and say so (spec 0053).
        return Template::from_json(&text).map_err(|e| format!("{path}: {e}"));
    }
    let Some(name) = args.template.as_deref() else {
        return Err("--template or --from is required (or pass --list)".into());
    };
    // A typo must not silently produce a document from the wrong template — or from a default one,
    // which is the same mistake wearing a friendlier face.
    Template::by_name(name).cloned().ok_or_else(|| {
        format!(
            "unknown template '{name}'; available: {}",
            Template::names().join(", ")
        )
    })
}
