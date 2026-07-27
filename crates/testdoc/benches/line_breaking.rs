//! Knuth-Plass line breaking throughput — see `specs/0027-perf-harness.md`.
//!
//! Justification is the inner loop of layout: `lay_out` calls it once per paragraph per candidate
//! frame. Measuring it separately means a regression can be attributed — a slower full-document
//! layout with unchanged per-paragraph cost points at the engine, not the breaker.

use quill_testdoc::{document_with_blocks, SynthSpec, HARNESS_METRICS};
use quill_text_layout::{justify_paragraph_hyphenated, Alignment, NoHyphenator};

mod budget;

/// The default 6x9in trim width, which is what paragraphs are actually broken against.
const FRAME_WIDTH_PT: f32 = 432.0;

fn main() {
    let budgets = budget::Budgets::load();
    let mut failures = Vec::new();

    // Pull real generated prose rather than one repeated string: Knuth-Plass cost depends on the
    // number of feasible breakpoints, so a single sentence repeated would measure the wrong shape.
    let doc = document_with_blocks(&SynthSpec::default(), 2_000);
    let paragraphs: Vec<&str> = doc
        .content
        .iter()
        .filter_map(|b| match b {
            quill_core_model::Block::Body { text, .. } => Some(text.as_str()),
            _ => None,
        })
        .collect();
    assert!(!paragraphs.is_empty(), "generator produced no body text");

    let elapsed = budget::min_of(3, || {
        for p in &paragraphs {
            std::hint::black_box(justify_paragraph_hyphenated(
                p,
                FRAME_WIDTH_PT,
                quill_text_layout::BODY_FONT_SIZE_PT,
                Alignment::Justified,
                &HARNESS_METRICS,
                &NoHyphenator,
            ));
        }
    });
    let us_per_paragraph = elapsed.as_secs_f64() * 1_000_000.0 / paragraphs.len() as f64;
    println!(
        "justify_paragraph_hyphenated: {} paragraphs in {:.1} ms  ({us_per_paragraph:.1} us/para)",
        paragraphs.len(),
        elapsed.as_secs_f64() * 1000.0
    );
    budgets.check(
        "line_breaking.us_per_paragraph",
        us_per_paragraph,
        &mut failures,
    );

    // Scaling in paragraph *length*: Knuth-Plass is quadratic in breakpoints in the worst case, so
    // a long paragraph costing wildly more than proportionally is the regression worth catching.
    let short: String = paragraphs[0]
        .split_whitespace()
        .take(40)
        .collect::<Vec<_>>()
        .join(" ");
    let long = std::iter::repeat_n(short.as_str(), 8)
        .collect::<Vec<_>>()
        .join(" ");
    let t_short = budget::min_of(5, || {
        std::hint::black_box(justify_paragraph_hyphenated(
            &short,
            FRAME_WIDTH_PT,
            quill_text_layout::BODY_FONT_SIZE_PT,
            Alignment::Justified,
            &HARNESS_METRICS,
            &NoHyphenator,
        ));
    });
    let t_long = budget::min_of(5, || {
        std::hint::black_box(justify_paragraph_hyphenated(
            &long,
            FRAME_WIDTH_PT,
            quill_text_layout::BODY_FONT_SIZE_PT,
            Alignment::Justified,
            &HARNESS_METRICS,
            &NoHyphenator,
        ));
    });
    let ratio = t_long.as_secs_f64() / t_short.as_secs_f64().max(f64::EPSILON);
    println!("length scaling: 8x the words costs {ratio:.2}x the time");
    budgets.check("line_breaking.length_scaling_ratio", ratio, &mut failures);

    budget::report(failures);
}
