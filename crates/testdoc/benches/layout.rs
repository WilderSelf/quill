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

    // Isolating the edit needs two timings of the *same shape*, differing only by the edit. A
    // fresh session per run is required because re-running `relayout` on an already-edited document
    // measures the no-op path; and the baseline must be a session priming pass, not a plain
    // `lay_out` — the session also fingerprints, caches and checkpoints, so subtracting `lay_out`
    // would charge the edit for the session's own setup. That mistake made this number swing
    // between 0.2% and 23% run to run.
    let t_prime = budget::min_of(5, || {
        let mut s = quill_layout_engine::LayoutSession::new();
        std::hint::black_box(s.relayout(&doc, &HARNESS_METRICS, &NoHyphenator));
    });
    let t_prime_then_edit = budget::min_of(5, || {
        let mut s = quill_layout_engine::LayoutSession::new();
        s.relayout(&doc, &HARNESS_METRICS, &NoHyphenator);
        std::hint::black_box(s.relayout(&edited, &HARNESS_METRICS, &NoHyphenator));
    });
    let edit_only = (t_prime_then_edit.as_secs_f64() - t_prime.as_secs_f64()).max(0.0);
    let fraction = edit_only / elapsed.as_secs_f64();

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
    budgets.check_exact(
        "layout.incremental_pages_reflowed",
        stats.pages_reflowed as f64,
        &mut failures,
    );
    // The gate that actually states "do not redo the expensive work". Since spec 0044 packed the
    // columns tight, `pages_reflowed` no longer implies re-measurement — see the note in
    // benches/budgets.toml — and this is the counter that carries the claim.
    budgets.check_exact(
        "layout.incremental_blocks_measured",
        stats.blocks_measured as f64,
        &mut failures,
    );
    // Reported, not gated. Extracting a sub-millisecond edit by subtracting two ~200 ms timings
    // amplifies runner noise enormously — measured 4.9%, 5.6%, 9.7% and 23% across runs of
    // unchanged code. That is exactly what spec 0027 says to avoid, and the counters above state
    // the claim ("re-flow only affected pages") both precisely and deterministically.
    println!("  (timing ratio is informational only — too noisy at this magnitude to gate)");

    // --- Fixpoint iterations over a document with every derived quantity (spec 0076) ----------
    //
    // The gap the M6 audit found: `FIXPOINT_MAX_ITERATIONS` *bounds* the loop and nothing
    // *measures* it. Every iteration is a full-document pass, so the cost multiplies with each
    // derived quantity a document carries, and `layout.scaling_ratio` above cannot see it — that
    // ratio measures the shape of one pass, and this growth is in the number of passes.
    //
    // The workload is deliberately the worst one the engine can currently be handed: the 500-page
    // synthetic document with all three derived quantities at once — a generated contents list, two
    // sections with folio formats, and forty cross-references scattered through it.
    let mut derived = doc.clone();
    derived.content.insert(
        0,
        quill_core_model::Block::Toc {
            id: quill_core_model::BlockId::UNASSIGNED,
            title: "Contents".into(),
            max_level: 2,
            color: quill_core_model::Color::Gray { v: 0.0 },
        },
    );
    derived.assign_missing_block_ids().expect("ids");
    let headings: Vec<quill_core_model::BlockId> = derived
        .content
        .iter()
        .filter(|b| matches!(b, quill_core_model::Block::Heading { .. }))
        .map(|b| b.id())
        .collect();
    // Two sections: front matter in roman, body in arabic restarting at 1.
    derived.sections = vec![
        quill_core_model::Section {
            name: "Front matter".into(),
            start: derived.content[1].id(),
            master: None,
            folio: Some(quill_core_model::Folio {
                format: quill_core_model::NumberFormat::LowerRoman,
                restart_at: Some(1),
            }),
        },
        quill_core_model::Section {
            name: "Body".into(),
            start: headings[1],
            master: None,
            folio: Some(quill_core_model::Folio {
                format: quill_core_model::NumberFormat::Decimal,
                restart_at: Some(1),
            }),
        },
    ];
    // Forty cross-references, each naming a heading a long way further on.
    let mut placed = 0usize;
    for i in (20..derived.content.len()).step_by(80) {
        if placed >= 40 {
            break;
        }
        let quill_core_model::Block::Body { .. } = &derived.content[i] else {
            continue;
        };
        let id = derived.content[i].id();
        let target = headings[(placed * 3 + 5) % headings.len()];
        derived.content[i] = quill_core_model::Block::body_runs(
            vec![
                quill_core_model::Run::plain("A paragraph that cites something else: see page "),
                quill_core_model::Run::reference(target),
                quill_core_model::Run::plain(" for the rest of that discussion, at length."),
            ],
            quill_core_model::Color::Gray { v: 0.0 },
        );
        derived.content[i].set_id(id);
        placed += 1;
    }
    assert_eq!(placed, 40, "the fixture must actually carry its references");

    // A back-of-book index, the fourth derived quantity (spec 0078), with two hundred marked terms
    // scattered through the document — enough that the index itself spans pages, which is the case
    // that can move what it lists.
    let mut marked = 0usize;
    for i in (30..derived.content.len()).step_by(15) {
        if marked >= 200 {
            break;
        }
        let quill_core_model::Block::Body { .. } = &derived.content[i] else {
            continue;
        };
        let id = derived.content[i].id();
        let mut run = quill_core_model::Run::plain(
            "A paragraph that discusses a subject at some length, and is indexed for it.",
        );
        run.index = Some(quill_core_model::IndexMark::new(format!(
            "term{:03}",
            marked % 120
        )));
        derived.content[i] =
            quill_core_model::Block::body_runs(vec![run], quill_core_model::Color::Gray { v: 0.0 });
        derived.content[i].set_id(id);
        marked += 1;
    }
    assert_eq!(
        marked, 200,
        "the fixture must actually carry its index marks"
    );
    derived.content.push(quill_core_model::Block::Index {
        id: quill_core_model::BlockId::UNASSIGNED,
        title: "Index".into(),
        ignore_leading: vec!["A".into(), "An".into(), "The".into()],
        color: quill_core_model::Color::Gray { v: 0.0 },
    });
    derived.assign_missing_block_ids().expect("ids");

    let mut fixpoint_session = quill_layout_engine::LayoutSession::new();
    let derived_result = fixpoint_session.relayout(&derived, &HARNESS_METRICS, &NoHyphenator);
    println!(
        "fixpoint: {} pages, {} iterations, converged {}  (contents list + 2 sections + 40 \
         cross-references + a 120-term index)",
        derived_result.pages.len(),
        derived_result.fixpoint.iterations,
        derived_result.fixpoint.converged
    );
    assert!(
        derived_result.fixpoint.converged,
        "the fixture must settle; a non-converging one would pin the cap rather than the cost"
    );
    // `check_exact`, not `check`. An iteration count is a deterministic work counter — same
    // document, same metrics, same passes on every machine — so the tolerance the timing entries
    // need does not apply, and applying it would be actively harmful here: doubling a ceiling of 4
    // puts the limit at 8, which *is* `FIXPOINT_MAX_ITERATIONS`, so the gate could only fire on a
    // document that already reports `converged: false` and reports it loudly. That is spec 0051's
    // lesson — a budget whose limit is unreachable is not a budget.
    budgets.check_exact(
        "layout.fixpoint_iterations",
        derived_result.fixpoint.iterations as f64,
        &mut failures,
    );

    // --- The book fixpoint (spec 0079) --------------------------------------------------------
    //
    // The M6 audit's named risk, measured rather than discovered: a book's contents list names
    // headings from chapters it does not contain, so the fixpoint's derived quantity spans every
    // chapter and *each iteration re-lays every chapter*. With `FIXPOINT_MAX_ITERATIONS = 8`, a
    // 10-chapter book could in principle cost 80 chapter layouts per relayout.
    //
    // The question that decides whether it is linear or quadratic is precisely **whether the
    // iteration count grows with the chapter count**, because a pass is a whole-book pass either
    // way. So the workload holds the total content fixed and varies only how many chapters it is
    // cut into: if the cost multiplied by the chapter count, the ratio below would be 2.
    let book_spec = SynthSpec {
        target_pages: 200,
        ..SynthSpec::default()
    };
    let five = quill_testdoc::synthetic_book(&book_spec, 5);
    let ten = quill_testdoc::synthetic_book(&book_spec, 10);

    let mut five_session = quill_layout_engine::LayoutSession::new();
    let five_result = five_session.relayout(&five.document, &HARNESS_METRICS, &NoHyphenator);
    let mut ten_session = quill_layout_engine::LayoutSession::new();
    let ten_result = ten_session.relayout(&ten.document, &HARNESS_METRICS, &NoHyphenator);

    println!(
        "book fixpoint: 5 chapters → {} pages in {} iterations (converged {}); \
         10 chapters → {} pages in {} iterations (converged {})",
        five_result.pages.len(),
        five_result.fixpoint.iterations,
        five_result.fixpoint.converged,
        ten_result.pages.len(),
        ten_result.fixpoint.iterations,
        ten_result.fixpoint.converged
    );
    assert!(
        five_result.fixpoint.converged && ten_result.fixpoint.converged,
        "the book fixture must settle; a non-converging one would pin the cap rather than the cost"
    );

    budgets.check_exact(
        "layout.book_fixpoint_iterations",
        ten_result.fixpoint.iterations as f64,
        &mut failures,
    );
    // `check_exact` on a *ratio*, which is unusual here and is right for the same reason the
    // iteration count is: both sides are deterministic pass counts, so the runner variance
    // `tolerance_factor` exists for does not apply. Applying it would put the limit at 2.0 — exactly
    // the value a cost that multiplied by the chapter count would produce — which is the
    // unreachable-limit trap spec 0051 recorded.
    let chapter_ratio =
        ten_result.fixpoint.iterations as f64 / five_result.fixpoint.iterations.max(1) as f64;
    budgets.check_exact("layout.book_chapter_ratio", chapter_ratio, &mut failures);

    // And the per-pass shape, so a book that settles in the same number of passes but does more work
    // per pass is still caught. Timed, therefore `check`.
    let book_pages = ten_result.pages.len();
    let book_elapsed = budget::min_of(3, || {
        let mut session = quill_layout_engine::LayoutSession::new();
        std::hint::black_box(session.relayout(&ten.document, &HARNESS_METRICS, &NoHyphenator));
    });
    let book_ms_per_page = book_elapsed.as_secs_f64() * 1000.0 / book_pages as f64;
    println!(
        "book layout: {book_pages} pages in {:.1} ms  ({book_ms_per_page:.3} ms/page, \
         {} fixpoint passes)",
        book_elapsed.as_secs_f64() * 1000.0,
        ten_result.fixpoint.iterations
    );
    budgets.check("layout.book_ms_per_page", book_ms_per_page, &mut failures);

    budget::report(failures);
}
