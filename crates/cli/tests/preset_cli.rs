//! The CLI surface of spec 0049 — POD presets — tested through the real binary.
//!
//! "The flag is accepted", "an unknown name does not panic" and "the report did not change" are
//! claims about `quill`, not about a library function, so they are asserted against
//! `CARGO_BIN_EXE_quill` rather than by calling into the crates.

use std::path::PathBuf;
use std::process::{Command, Output};

/// The exact stdout `quill preflight` produced *before* presets existed, captured from the
/// pre-0049 build. This increment is a refactor plus a flag: any difference in this file is a bug,
/// not a new feature.
const GOLDEN_REPORT: &str = include_str!("golden/preflight-sample.txt");

/// Compare ignoring line-ending style.
///
/// `.gitattributes` marks the golden fixture `-text` so git does not rewrite it on checkout, and
/// that is the real fix. This is the belt to that pair of braces: a fixture regenerated on a Windows
/// machine would otherwise reintroduce the same failure, and a golden test that is fragile about
/// `\r` is testing the checkout rather than the program.
fn same_report(actual: &str, expected: &str) -> bool {
    actual.replace("\r\n", "\n") == expected.replace("\r\n", "\n")
}

fn quill(args: &[&str]) -> Output {
    Command::new(env!("CARGO_BIN_EXE_quill"))
        .args(args)
        .output()
        .expect("run quill")
}

fn stdout(out: &Output) -> String {
    String::from_utf8(out.stdout.clone()).expect("utf-8 stdout")
}

/// A scratch path unique to one test, so the suite can run in parallel.
fn scratch(tag: &str) -> PathBuf {
    let mut p = std::env::temp_dir();
    p.push(format!("quill-preset-{tag}-{}.tpub", std::process::id()));
    p
}

#[test]
fn preflight_with_no_preset_is_byte_identical_to_the_pre_preset_report() {
    // The regression the whole increment stands on. `Document::sample()` under no `--preset` must
    // produce exactly what it produced before `PodPreset` existed — same findings, same order,
    // same wording, same exit status.
    let out = quill(&["preflight"]);
    assert!(
        same_report(&stdout(&out), GOLDEN_REPORT),
        "the report moved"
    );
    assert!(
        !out.status.success(),
        "a bare preflight still fails on the missing OutputIntent"
    );
}

#[test]
fn an_explicit_generic_preset_reports_exactly_what_the_default_does() {
    // Proves the default *is* `generic`, rather than merely resembling it: if the two paths ever
    // diverged, the flagless golden above would keep passing while `--preset generic` drifted.
    let out = quill(&["preflight", "--preset", "generic"]);
    assert!(same_report(&stdout(&out), GOLDEN_REPORT));
}

#[test]
fn a_vendor_preset_is_accepted_and_announces_that_it_is_unconfirmed() {
    // The roadmap's `--preset lulu` criterion, plus the honesty requirement: a preset whose numbers
    // were not read from the vendor must not be able to look authoritative just because it carries
    // the vendor's name.
    let out = quill(&["preflight", "--preset", "lulu"]);
    assert!(
        same_report(&stdout(&out), GOLDEN_REPORT),
        "lulu's numbers are generic's"
    );
    let note = String::from_utf8(out.stderr).expect("utf-8 stderr");
    assert!(
        note.contains("UNCONFIRMED") && note.contains("Lulu"),
        "an unconfirmed preset must say so: {note}"
    );
}

#[test]
fn an_unknown_preset_fails_listing_the_available_names() {
    // Never a panic, and never a silent fall back to a default — preflighting against a printer the
    // user did not choose is the failure this increment exists to prevent.
    let out = quill(&["preflight", "--preset", "no-such-printer"]);
    assert!(!out.status.success());
    let err = String::from_utf8(out.stderr).expect("utf-8 stderr");
    assert!(err.contains("unknown preset 'no-such-printer'"), "{err}");
    for name in ["generic", "drivethrurpg", "lulu", "ingramspark"] {
        assert!(err.contains(name), "the alternatives must be listed: {err}");
    }
    assert!(!err.contains("panicked"), "{err}");
}

#[test]
fn export_accepts_the_preset_flag_and_rejects_an_unknown_one() {
    let icc = {
        let mut p = std::env::temp_dir();
        p.push(format!("quill-preset-icc-{}.icc", std::process::id()));
        p
    };
    let icc = icc.to_string_lossy().to_string();
    assert!(quill(&["synth-icc", &icc]).status.success());

    let pdf = scratch("export").with_extension("pdf");
    let pdf = pdf.to_string_lossy().to_string();
    let ok = quill(&[
        "export",
        "-o",
        &pdf,
        "--icc",
        &icc,
        "--preset",
        "ingramspark",
    ]);
    assert!(
        ok.status.success(),
        "{}",
        String::from_utf8_lossy(&ok.stderr)
    );

    let bad = quill(&["export", "-o", &pdf, "--icc", &icc, "--preset", "nope"]);
    assert!(!bad.status.success());
    assert!(String::from_utf8_lossy(&bad.stderr).contains("available: generic"));

    let _ = std::fs::remove_file(&pdf);
    let _ = std::fs::remove_file(&icc);
}

#[test]
fn a_preset_travels_on_the_command_line_and_never_into_the_document() {
    // Spec 0049's format decision, asserted rather than merely documented: a document is not bound
    // to one printer, so the manifest must carry no preset — only the *effect* the preset had on
    // the page setup.
    let path = scratch("new");
    let out = quill(&[
        "new",
        "--template",
        "adventure",
        "--preset",
        "drivethrurpg",
        "-o",
        &path.to_string_lossy(),
    ]);
    assert!(
        out.status.success(),
        "{}",
        String::from_utf8_lossy(&out.stderr)
    );

    let doc = quill_core_model::Tpub::read_manifest(&path).expect("read manifest");
    assert_eq!(doc.page_setup.bleed_pt, 9.0, "the bleed was seeded");
    let json = doc.to_json().expect("serialize");
    assert!(
        !json.contains("preset"),
        "the manifest must carry no preset: {json}"
    );

    let _ = std::fs::remove_file(&path);
}

#[test]
fn quill_presets_prints_provenance_for_every_bundled_preset() {
    // Provenance a user cannot read is provenance that is not doing its job.
    let out = quill(&["presets"]);
    assert!(out.status.success());
    let text = stdout(&out);
    for name in ["generic", "drivethrurpg", "lulu", "ingramspark"] {
        assert!(text.contains(name), "{name} missing from the listing");
    }
    assert!(text.contains("source:"), "{text}");
    assert!(text.contains("retrieved"), "{text}");
    assert!(text.contains("UNCONFIRMED"), "{text}");
}
