//! Spec 0049 — POD presets. The tests that make the preset trustworthy rather than merely present:
//! `generic` is provably today's constants, no preflight path can still read a bare constant, the
//! thresholds provably move with the preset, and every bundled preset can be audited.

use quill_color::MAX_INK_COVERAGE_PCT;
use quill_core_model::{Color, Document, Size, DEFAULT_BLEED_PT};
use quill_export_pdf::{preflight, CheckId, ExportOptions, PdfxVersion, PodPreset, Severity};

/// A `PageSetup`-legal ICC path so the OutputIntent finding does not crowd the assertions.
fn opts(preset: PodPreset) -> ExportOptions {
    ExportOptions {
        output_intent_icc: "profiles/cmyk.icc".into(),
        preset,
        ..Default::default()
    }
}

#[test]
fn generic_is_numerically_identical_to_the_constants_it_replaced() {
    // The claim the whole increment rests on: adding presets changes no existing behaviour. Stated
    // against the constants themselves, so a future edit to either side has to face this test.
    let g = PodPreset::generic();
    assert_eq!(g.max_ink_pct, MAX_INK_COVERAGE_PCT);
    assert_eq!(g.bleed_pt, DEFAULT_BLEED_PT);
    assert_eq!(g.min_dpi(false), 300.0);
    assert_eq!(g.min_dpi(true), 600.0);
    assert_eq!(g.pdfx, PdfxVersion::X1a2001);
    assert_eq!(ExportOptions::default().preset, g, "generic is the default");
}

#[test]
fn no_preflight_code_path_still_reads_the_bare_constants() {
    // Structural, because the failure it guards is invisible at runtime: a check that kept reading
    // `MAX_INK_COVERAGE_PCT` would pass every test above (generic *equals* it) and silently ignore
    // the user's chosen printer. The test-module portion is excluded — a test fixture is entitled
    // to a literal dpi.
    let src = include_str!("../src/lib.rs");
    let non_test = src.split("#[cfg(test)]").next().expect("source");
    for needle in [
        "MAX_INK_COVERAGE_PCT",
        "DEFAULT_BLEED_PT",
        "within_ink_limit(",
        "300.0",
        "600.0",
        "240.0",
    ] {
        assert!(
            !non_test.contains(needle),
            "preflight must read '{needle}' from a preset, not from a constant"
        );
    }
}

/// A CMYK colour at `pct` total ink.
fn ink(pct: f32) -> Color {
    let each = pct / 400.0;
    Color::Cmyk {
        c: each,
        m: each,
        y: each,
        k: each,
    }
}

#[test]
fn a_stricter_preset_fails_ink_that_generic_passes_and_vice_versa() {
    // The only test that proves presets do anything. Asserted in *both* directions: a checker that
    // simply failed everything would pass the first half.
    let mut doc = Document::sample();
    doc.content
        .push(quill_core_model::Block::body("dark", ink(220.0)));
    doc.assign_missing_block_ids().expect("ids");

    let lenient = preflight(&doc, &opts(PodPreset::generic()));
    assert!(
        !lenient
            .findings
            .iter()
            .any(|f| f.check == CheckId::InkCoverage),
        "220% is inside generic's 240% limit: {:?}",
        lenient.findings
    );

    // A press that will not take 220%. Hand-built rather than bundled: no vendor's real limit was
    // verified when this shipped, and inventing one to make a test pass is exactly the failure the
    // spec forbids.
    let strict = PodPreset {
        name: "strict-test-press".into(),
        max_ink_pct: 200.0,
        ..PodPreset::generic()
    };
    let report = preflight(&doc, &opts(strict));
    let finding = report
        .findings
        .iter()
        .find(|f| f.check == CheckId::InkCoverage)
        .expect("220% must fail a 200% press");
    assert_eq!(finding.severity, Severity::Error);
    assert!(finding.message.contains("200"), "{}", finding.message);
}

#[test]
fn the_bleed_floor_and_the_dpi_thresholds_move_with_the_preset() {
    // The other two thresholds, in the direction that cannot pass by accident: the sample is clean
    // under generic and must fail under a preset that asks for more.
    let doc = Document::sample();
    assert!(preflight(&doc, &opts(PodPreset::generic())).passed());

    let demanding = PodPreset {
        bleed_pt: 18.0,
        min_dpi_color: 400.0,
        ..PodPreset::generic()
    };
    let report = preflight(&doc, &opts(demanding));
    assert!(report.findings.iter().any(|f| f.check == CheckId::Bleed));
    assert!(report
        .findings
        .iter()
        .any(|f| f.check == CheckId::ImageResolution));
}

#[test]
fn an_unlisted_trim_is_a_warning_never_an_error() {
    // An unusual trim is a conversation with the printer, not a corrupt file. A preflight that
    // blocked it would teach its user to reach for `--force`, which costs every other check.
    let mut doc = Document::sample();
    doc.page_setup.trim = Size {
        w_pt: 123.0,
        h_pt: 456.0,
    };
    let report = preflight(&doc, &opts(PodPreset::generic()));
    let finding = report
        .findings
        .iter()
        .find(|f| f.check == CheckId::TrimSize)
        .expect("an unlisted trim must be reported");
    assert_eq!(finding.severity, Severity::Warning);
    assert!(report.passed(), "a warning must not fail preflight");

    // The reuse direction, twice: a listed trim says nothing, and a preset that states no
    // catalogue says nothing about any trim at all.
    let listed = preflight(&Document::sample(), &opts(PodPreset::generic()));
    assert!(!listed.findings.iter().any(|f| f.check == CheckId::TrimSize));

    let silent = PodPreset::by_name("lulu").expect("bundled");
    assert!(silent.trim_sizes.is_empty());
    let report = preflight(&doc, &opts(silent));
    assert!(!report.findings.iter().any(|f| f.check == CheckId::TrimSize));
}

#[test]
fn every_bundled_preset_can_be_audited_against_its_source() {
    // Vendor requirements change. A preset that cannot be checked against where its numbers came
    // from becomes wrong silently — the same failure class as a CI job that is not a required
    // context.
    for p in PodPreset::bundled() {
        assert!(!p.name.is_empty(), "a preset needs a name");
        assert!(!p.source.trim().is_empty(), "{}: empty source", p.name);
        assert!(
            !p.retrieved.trim().is_empty(),
            "{}: empty retrieved",
            p.name
        );
        // `YYYY-MM-DD`, so "when" is answerable rather than merely present.
        assert_eq!(p.retrieved.len(), 10, "{}: {}", p.name, p.retrieved);
        assert_eq!(p.retrieved.matches('-').count(), 2, "{}", p.name);
        assert!(p.max_ink_pct > 0.0 && p.bleed_pt >= 0.0 && p.min_dpi_color > 0.0);

        // Honesty, asserted: an unconfirmed preset must say so in the field a user reads, and must
        // not be *looser* than the conservative default it stands in for.
        if !p.confirmed {
            assert!(
                p.source.contains("NOT"),
                "{}: an unconfirmed preset must say so in its source",
                p.name
            );
            let g = PodPreset::generic();
            assert!(p.max_ink_pct <= g.max_ink_pct, "{} loosens ink", p.name);
            assert!(p.bleed_pt >= g.bleed_pt, "{} loosens bleed", p.name);
            assert!(p.min_dpi_color >= g.min_dpi_color, "{} loosens dpi", p.name);
        }
    }
}
