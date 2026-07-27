//! Layout throughput on the 500-page synthetic document — see `specs/0027-perf-harness.md`.
//!
//! Run with `cargo bench -p quill-testdoc --bench layout`. Exits non-zero if a measurement blows
//! past its ceiling in `benches/budgets.toml`.
//!
//! ## Why this asserts a ratio and not a millisecond count
//!
//! Shared CI runners vary by 10–30% between runs, and `rust-toolchain.toml` pins the floating
//! `stable` channel, so any absolute ceiling drifts on every Rust release and eventually gets
//! "fixed" by raising the number until it stops failing — at which point it measures nothing. What
//! this gate is actually for is catching an *algorithmic* regression: someone reintroducing a
//! quadratic scan, or making layout re-do work it used to cache. That shows up as a change in
//! **shape**, which a same-run ratio detects reliably and cheaply.

use quill_testdoc::{page_count, synthetic_document, SynthSpec, HARNESS_METRICS};
use quill_text_layout::NoHyphenator;

mod budget;

fn main() {
    let budgets = budget::Budgets::load();
    let mut failures: Vec<String> = Vec::new();

    // --- Full-document layout -----------------------------------------------------------------
    let spec = SynthSpec::default();
    let doc = synthetic_document(&spec);
    let pages = page_count(&doc);

    let elapsed = budget::min_of(3, || {
        let out = quill_layout_engine::lay_out(&doc, &HARNESS_METRICS, &NoHyphenator);
        std::hint::black_box(out);
    });
    let ms_per_page = elapsed.as_secs_f64() * 1000.0 / pages as f64;
    println!(
        "lay_out: {pages} pages in {:.1} ms  ({ms_per_page:.3} ms/page)",
        elapsed.as_secs_f64() * 1000.0
    );
    budgets.check("layout.ms_per_page", ms_per_page, &mut failures);

    // --- Scaling: 250 vs 500 pages ------------------------------------------------------------
    // The real assertion. Layout is a single forward pass, so doubling the document should roughly
    // double the time; anything superlinear means work is being repeated per block.
    let half = synthetic_document(&SynthSpec {
        target_pages: spec.target_pages / 2,
        ..spec
    });
    let t_half = budget::min_of(3, || {
        std::hint::black_box(quill_layout_engine::lay_out(
            &half,
            &HARNESS_METRICS,
            &NoHyphenator,
        ));
    });
    let ratio = elapsed.as_secs_f64() / t_half.as_secs_f64().max(f64::EPSILON);
    println!(
        "scaling: {:.1} ms at {} pages vs {:.1} ms at {} pages  (ratio {ratio:.2})",
        elapsed.as_secs_f64() * 1000.0,
        pages,
        t_half.as_secs_f64() * 1000.0,
        page_count(&half),
    );
    budgets.check("layout.scaling_ratio", ratio, &mut failures);

    // --- Incremental relayout after a one-paragraph edit (spec 0031) --------------------------
    // The M1 claim itself. Reported as counters and a ratio against the full pass, never as an
    // absolute time.
    let mut session = quill_layout_engine::LayoutSession::new();
    let first = session.relayout(&doc, &HARNESS_METRICS, &NoHyphenator);
    let total_pages = first.pages.len();

    let mut edited = doc.clone();
    let target = edited.content.len() / 2;
    let id = edited.content[target].id();
    edited.content[target] = quill_core_model::Block::body(
        "an edited paragraph, roughly the same length as the one it replaced in the document",
        quill_core_model::Color::Cmyk {
            c: 0.0,
            m: 0.0,
            y: 0.0,
            k: 1.0,
        },
    );
    edited.content[target].set_id(id);

    let t_incremental = budget::min_of(3, || {
        // A fresh session per timed run: re-running `relayout` on the same edited document would
        // measure the no-op path, not the edit path.
        let mut s = quill_layout_engine::LayoutSession::new();
        s.relayout(&doc, &HARNESS_METRICS, &NoHyphenator);
        std::hint::black_box(s.relayout(&edited, &HARNESS_METRICS, &NoHyphenator));
    });
    // Subtract the priming pass to isolate the edit's cost.
    let edit_only = t_incremental.as_secs_f64() - elapsed.as_secs_f64();
    let fraction = (edit_only / elapsed.as_secs_f64()).max(0.0);

    let stats = session
        .relayout(&edited, &HARNESS_METRICS, &NoHyphenator)
        .stats;
    println!(
        "incremental edit: {} of {total_pages} pages reflowed, {} blocks measured, \
         {} from cache  (~{:.1}% of a full pass)",
        stats.pages_reflowed,
        stats.blocks_measured,
        stats.blocks_from_cache,
        fraction * 100.0
    );
    budgets.check(
        "layout.incremental_pages_reflowed",
        stats.pages_reflowed as f64,
        &mut failures,
    );
    budgets.check("layout.incremental_work_fraction", fraction, &mut failures);

    budget::report(failures);
}
