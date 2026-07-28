//! `quill new --from <file>` — user-authored templates end to end (spec 0053).
//!
//! Runs the real binary and opens the container it produced, because the claim under test is about
//! what `quill new` *writes*. A library test could assert that a `Template` loads; only this can
//! assert that the document in the `.tpub` on disk carries the file's page setup, styles and
//! masters.

use std::path::{Path, PathBuf};
use std::process::{Command, Output};

use quill_core_model::{Document, Template, Tpub, TEMPLATE_VERSION};

/// The binary cargo just built for this integration test.
const QUILL: &str = env!("CARGO_BIN_EXE_quill");

/// A per-test scratch directory. Named after the test so a failure leaves an inspectable
/// directory, and re-created each run so a stale file cannot make a test pass.
fn temp_dir(tag: &str) -> PathBuf {
    let dir = std::env::temp_dir().join(format!("quill-0053-{}-{tag}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).expect("create temp dir");
    dir
}

fn run(args: &[&str]) -> Output {
    Command::new(QUILL).args(args).output().expect("run quill")
}

fn stdout(out: &Output) -> String {
    String::from_utf8_lossy(&out.stdout).into_owned()
}

fn stderr(out: &Output) -> String {
    String::from_utf8_lossy(&out.stderr).into_owned()
}

/// Open a written `.tpub` back into a document.
fn reopen(tpub: &Path, into: &Path) -> Document {
    Tpub::open_into(tpub, into)
        .expect("open the written container")
        .document
}

/// A template that is deliberately *not* one of the bundled three: a different trim, a different
/// scale, a differently named master. A test that authored a copy of `reference` could pass while
/// `--from` silently fell back to a bundled template.
fn authored_template() -> Template {
    Template::from_json(
        r#"{
            "template_version": 1,
            "name": "house-style",
            "title": "House style (5x8)",
            "description": "Narrow trim, one column, a running head.",
            "page_setup": {"trim": {"w_pt": 360.0, "h_pt": 576.0}, "bleed_pt": 12.0,
                           "facing_pages": true,
                           "margins": {"top_pt": 48.0, "bottom_pt": 48.0,
                                       "inside_pt": 50.0, "outside_pt": 36.0}},
            "styles": {"paragraph": {
                "body": {"font_size_pt": 9.0, "leading_pt": 11.5, "align": "justified"},
                "h1": {"font_size_pt": 19.0, "leading_pt": 23.0, "align": "left"},
                "running-head": {"font_size_pt": 7.5, "leading_pt": 11.5, "align": "left"}}},
            "master_pages": [
              {"name": "house-body", "columns": 1,
               "margins": {"top_pt": 48.0, "bottom_pt": 48.0,
                           "inside_pt": 50.0, "outside_pt": 36.0},
               "statics": [{"kind": "text",
                            "rect": {"x_pt": 50.0, "y_pt": 534.0, "w_pt": 274.0, "h_pt": 12.0},
                            "text": "{page}", "color": {"space": "gray", "v": 0.0},
                            "style": "running-head", "align": "outside", "mirror": true}]}],
            "default_master": "house-body",
            "pages": [{"master": "house-body"}]
        }"#,
    )
    .expect("the fixture template must load")
}

#[test]
fn new_from_a_template_file_writes_a_document_carrying_that_file_field_by_field() {
    let dir = temp_dir("from");
    let template_path = dir.join("house-style.json");
    let tpub = dir.join("book.tpub");
    let template = authored_template();
    std::fs::write(&template_path, template.to_json().expect("serialize")).expect("write template");

    let out = run(&[
        "new",
        "--from",
        template_path.to_str().unwrap(),
        "--output",
        tpub.to_str().unwrap(),
    ]);
    assert!(out.status.success(), "stderr: {}", stderr(&out));
    // The success line names the template by its own `name`, whichever way it was found.
    assert!(
        stdout(&out).contains("from template 'house-style'"),
        "stdout: {}",
        stdout(&out)
    );

    let doc = reopen(&tpub, &dir.join("book.tpub.d"));

    // Field by field, not "it produced a file". Each of these is a separate way the loader could
    // be wrong, and a whole-struct compare would report only the first.
    assert_eq!(doc.page_setup, template.page_setup);
    assert_eq!(doc.page_setup.trim.w_pt, 360.0);
    assert_eq!(doc.page_setup.trim.h_pt, 576.0);
    assert_eq!(doc.page_setup.bleed_pt, 12.0);
    assert_eq!(doc.page_setup.margins, template.page_setup.margins);
    assert_eq!(doc.styles, template.styles);
    assert_eq!(
        doc.styles.paragraph["running-head"],
        template.styles.paragraph["running-head"]
    );
    assert_eq!(doc.master_pages, template.master_pages);
    assert_eq!(
        doc.master_pages[0].statics,
        template.master_pages[0].statics
    );
    assert_eq!(doc.default_master, template.default_master);
    assert_eq!(doc.pages, template.pages);

    // And it is a document, not just a bag of fields: the master it names resolves.
    assert_eq!(
        doc.master_for(0).map(|m| m.name.as_str()),
        Some("house-body")
    );
    assert!(doc.content.is_empty(), "a new document starts empty");
}

#[test]
fn new_from_a_missing_or_malformed_file_fails_naming_the_problem() {
    let dir = temp_dir("errors");

    let missing = dir.join("not-here.json");
    let out = run(&[
        "new",
        "--from",
        missing.to_str().unwrap(),
        "--output",
        dir.join("a.tpub").to_str().unwrap(),
    ]);
    assert!(!out.status.success());
    assert!(stderr(&out).contains("not-here.json"), "{}", stderr(&out));

    // Malformed: a syntax error.
    let bad = dir.join("bad.json");
    std::fs::write(&bad, "{ this is not json").expect("write");
    let out = run(&[
        "new",
        "--from",
        bad.to_str().unwrap(),
        "--output",
        dir.join("b.tpub").to_str().unwrap(),
    ]);
    assert!(!out.status.success());
    assert!(
        stderr(&out).contains("malformed template file"),
        "{}",
        stderr(&out)
    );

    // Newer than this build: refused, and the message says so rather than half-loading it.
    let newer = dir.join("newer.json");
    let json = authored_template().to_json().expect("serialize").replace(
        &format!("\"template_version\": {TEMPLATE_VERSION}"),
        &format!("\"template_version\": {}", TEMPLATE_VERSION + 1),
    );
    std::fs::write(&newer, json).expect("write");
    let out = run(&[
        "new",
        "--from",
        newer.to_str().unwrap(),
        "--output",
        dir.join("c.tpub").to_str().unwrap(),
    ]);
    assert!(!out.status.success());
    assert!(
        stderr(&out).contains("newer than this build supports"),
        "{}",
        stderr(&out)
    );

    // No output file was written in any of the three cases: a refused template must not leave a
    // half-formed document behind.
    for name in ["a.tpub", "b.tpub", "c.tpub"] {
        assert!(
            !dir.join(name).exists(),
            "{name} must not have been written"
        );
    }
}

#[test]
fn from_and_template_are_mutually_exclusive() {
    // One document has one starting point. Silently preferring one would be the "typo produces the
    // wrong template" failure spec 0036 already refused.
    let dir = temp_dir("exclusive");
    let out = run(&[
        "new",
        "--template",
        "reference",
        "--from",
        "anything.json",
        "--output",
        dir.join("x.tpub").to_str().unwrap(),
    ]);
    assert!(!out.status.success());
    assert!(
        stderr(&out).contains("cannot be used with"),
        "{}",
        stderr(&out)
    );
}

#[test]
fn list_and_template_are_unchanged_by_the_addition_of_from() {
    // The regression half of spec 0053: `--from` is purely additive.
    let dir = temp_dir("bundled");

    let out = run(&["new", "--list"]);
    assert!(out.status.success(), "stderr: {}", stderr(&out));
    let listed = stdout(&out);
    for t in Template::bundled() {
        assert!(listed.contains(&t.name), "--list must name `{}`", t.name);
        assert!(listed.contains(&t.title), "--list must name `{}`", t.title);
    }
    assert_eq!(
        listed.lines().count(),
        Template::bundled().len(),
        "--list must print exactly the bundled set: {listed}"
    );

    for name in Template::names() {
        let tpub = dir.join(format!("{name}.tpub"));
        let out = run(&[
            "new",
            "--template",
            name,
            "--output",
            tpub.to_str().unwrap(),
        ]);
        assert!(out.status.success(), "stderr: {}", stderr(&out));
        let doc = reopen(&tpub, &dir.join(format!("{name}.d")));
        assert_eq!(
            doc,
            Document::from_template(Template::by_name(name).expect("bundled")),
            "`--template {name}` must build exactly what it always did"
        );
    }

    // And a typo still fails, listing the valid names.
    let out = run(&[
        "new",
        "--template",
        "nope",
        "--output",
        dir.join("nope.tpub").to_str().unwrap(),
    ]);
    assert!(!out.status.success());
    assert!(stderr(&out).contains("reference"), "{}", stderr(&out));
}

#[test]
fn new_with_neither_a_slug_nor_a_file_says_so() {
    let dir = temp_dir("neither");
    let out = run(&["new", "--output", dir.join("x.tpub").to_str().unwrap()]);
    assert!(!out.status.success());
    let err = stderr(&out);
    assert!(
        err.contains("--template") && err.contains("--from"),
        "{err}"
    );
}
