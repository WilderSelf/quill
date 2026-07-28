//! Spec 0054's anti-drift gate: the workspace's vocabulary stays domain-neutral.
//!
//! Quill is a general-purpose desktop publishing application first. Illustrated game books are the
//! flagship use case, but every *mechanism* must be one a cookbook, a field guide or a manual could
//! use on the same terms — so the vocabulary the code is written in has to be general too. A rename
//! holds for exactly as long as nothing reintroduces the old word, which is what this asserts.
//!
//! Deliberately narrow: it bans one token rather than policing prose. `ttrpg` is the word that
//! appeared in a crate name, a dependency comment and a CLI banner, and its return is the specific
//! drift worth catching. Genre-flavoured *fixture content* (`Document::sample()`'s text, the
//! `testdoc` word bank, a test creature called Goblin) is content and not mechanism, and is exempt
//! by design — spec 0062 records why re-wording those would cost more than it bought.
//!
//! The precedent is specs 0030, 0043 and 0053, each of which parses its own documentation in a test
//! so the docs cannot drift from the code.

use std::fs;
use std::path::{Path, PathBuf};

/// The `crates/` directory, found from this test's own manifest rather than the working directory —
/// `cargo test` runs from the workspace root but `cargo test -p` need not.
fn crates_dir() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .expect("crates/core-model has a parent")
        .to_path_buf()
}

fn visit(dir: &Path, found: &mut Vec<String>) {
    for entry in fs::read_dir(dir).expect("readable directory") {
        let entry = entry.expect("readable entry");
        let path = entry.path();
        let name = entry.file_name();
        let name = name.to_string_lossy();
        if path.is_dir() {
            // `target/` is build output and `assets/` is binary fixtures; neither is source.
            if name != "target" && name != "assets" {
                visit(&path, found);
            }
            continue;
        }
        let is_source = matches!(
            path.extension().and_then(|e| e.to_str()),
            Some("rs") | Some("toml") | Some("md")
        );
        if !is_source {
            continue;
        }
        // This file has to name the word in order to ban it.
        if path.ends_with(Path::new("tests/vocabulary.rs")) {
            continue;
        }
        let Ok(text) = fs::read_to_string(&path) else {
            continue; // not UTF-8: a fixture, not source
        };
        if text.to_lowercase().contains("ttrpg") {
            found.push(path.display().to_string());
        }
    }
}

#[test]
fn no_crate_source_names_a_single_genre() {
    let mut found = Vec::new();
    visit(&crates_dir(), &mut found);
    assert!(
        found.is_empty(),
        "spec 0062: `ttrpg` is back in the source. A mechanism only one genre can use is a defect \
         here; name what the code does instead (a panel, a range table, a reference template). \
         Offending files: {found:#?}"
    );
}
