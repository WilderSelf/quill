//! Export size on the sample and the 500-page synthetic document — see
//! `specs/0071-compress-content-streams.md`.
//!
//! Run with `cargo bench -p quill-testdoc --bench export_size`. Exits non-zero if a measurement
//! blows past its ceiling in `benches/budgets.toml`.
//!
//! ## Why this exists at all
//!
//! Spec 0068 grew the 500-page export by 14.7% and nobody noticed until someone measured it by
//! hand. Every other cost in this harness is timed; the *artefact* the product exists to produce
//! was unguarded. A 500-page art-heavy book is the stated workload, and the file that comes out of
//! it is what a publisher uploads.
//!
//! ## Why the check is exact rather than 2x-tolerant
//!
//! `tolerance_factor` exists for one reason, stated at the top of `benches/budgets.toml`: shared
//! runners vary by 10–30% between runs. An exported byte count does not vary at all — the same
//! document, the same committed fonts and the same committed ICC produce the same file every time,
//! which is already what `SAMPLE_EXPORT_DIGEST` rests on. Doubling the ceiling would put it above
//! the pre-0068 size this increment exists to get back under, i.e. the budget could not fail for
//! the exact regression it guards. That is spec 0051's lesson, and `check_exact` is the mechanism.
//!
//! ## Why the parity ICC
//!
//! A PDF/X export embeds the OutputIntent profile verbatim, and `synth-icc` stamps the current
//! time into the ICC header — so a synthesized profile makes the size a clock. The committed
//! `crates/export-pdf/assets/parity-outputintent.icc` is the same fixture the byte-parity digest
//! uses, for the same reason.

use quill_core_model::Document;
use quill_export_pdf::{export, ExportOptions};
use quill_testdoc::{page_count, synthetic_document, SynthSpec};

mod budget;

/// The committed parity profile, so the measured size is the writer's and not the clock's.
const PARITY_ICC: &[u8] = include_bytes!("../../export-pdf/assets/parity-outputintent.icc");

fn opts(force: bool) -> (ExportOptions, std::path::PathBuf) {
    let path = std::env::temp_dir().join("quill_export_size_parity.icc");
    std::fs::write(&path, PARITY_ICC).expect("write parity ICC");
    (
        ExportOptions {
            output_intent_icc: path.to_string_lossy().into_owned(),
            // The synthetic document is generated at `PageSetup::default()`, which has no bleed, so
            // it fails preflight by construction. The workload under measurement is the *writer*.
            force,
            ..Default::default()
        },
        path,
    )
}

fn main() {
    let budgets = budget::Budgets::load();
    let mut failures: Vec<String> = Vec::new();

    let (options, icc) = opts(false);
    let mut sample = Vec::new();
    export(&Document::sample(), &options, &mut sample).expect("sample export");
    println!("sample: {} bytes", sample.len());
    budgets.check_exact("export.sample_bytes", sample.len() as f64, &mut failures);

    let (options, _) = opts(true);
    let spec = SynthSpec::default();
    let doc = synthetic_document(&spec);
    let pages = page_count(&doc);
    let mut synthetic = Vec::new();
    let started = std::time::Instant::now();
    export(&doc, &options, &mut synthetic).expect("synthetic export");
    println!(
        "synthetic: {} blocks, {pages} pages, {} bytes in {:.1} ms",
        doc.content.len(),
        synthetic.len(),
        started.elapsed().as_secs_f64() * 1000.0
    );
    budgets.check_exact(
        "export.synthetic_500_page_bytes",
        synthetic.len() as f64,
        &mut failures,
    );
    let _ = std::fs::remove_file(&icc);

    budget::report(failures);
}
