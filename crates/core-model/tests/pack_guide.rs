//! Every JSON example in `docs/pack-authoring.md`, parsed (spec 0061).
//!
//! The anti-drift precedent specs 0030, 0043 and 0053 all use: a guide whose examples have quietly
//! stopped compiling is worse than no guide, because a reader who copies one and gets an error
//! concludes the *tool* is broken. So the examples are tested rather than proof-read.
//!
//! Each fenced block carries its type in the info string — ```json component-def — and this file
//! parses it as that type. A block whose tag names no known type is a **failure**, not a skip: an
//! untagged example is one nobody is checking.

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

use quill_core_model::{
    Block, ComponentDef, DefColor, Document, PackManifest, PackRequirement, StyleSheet,
};

fn repo_root() -> PathBuf {
    // `CARGO_MANIFEST_DIR` is `crates/core-model`.
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .and_then(Path::parent)
        .expect("workspace root")
        .to_path_buf()
}

/// Every fenced block in `markdown` whose info string starts with `json`, as `(tag, body)`.
fn json_blocks(markdown: &str) -> Vec<(String, String)> {
    let mut out = Vec::new();
    let mut lines = markdown.lines();
    while let Some(line) = lines.next() {
        let Some(info) = line.strip_prefix("```") else {
            continue;
        };
        let mut words = info.split_whitespace();
        if words.next() != Some("json") {
            // Skip to the end of this block so a `text` fence containing ``` cannot confuse us.
            for l in lines.by_ref() {
                if l.starts_with("```") {
                    break;
                }
            }
            continue;
        }
        let tag = words.next().unwrap_or("").to_string();
        let mut body = String::new();
        for l in lines.by_ref() {
            if l.starts_with("```") {
                break;
            }
            body.push_str(l);
            body.push('\n');
        }
        out.push((tag, body));
    }
    out
}

#[test]
fn every_example_in_the_guide_parses_as_what_it_claims_to_be() {
    let path = repo_root().join("docs/pack-authoring.md");
    let text = std::fs::read_to_string(&path)
        .unwrap_or_else(|e| panic!("reading {}: {e}", path.display()));
    let blocks = json_blocks(&text);
    assert!(
        blocks.len() >= 6,
        "the guide should carry several examples, found {}",
        blocks.len()
    );

    let mut seen: BTreeMap<&str, usize> = BTreeMap::new();
    for (tag, body) in &blocks {
        let outcome: Result<(), String> = match tag.as_str() {
            "pack-manifest" => PackManifest::from_json(body)
                .map(|_| ())
                .map_err(|e| e.to_string()),
            "component-def" => serde_json::from_str::<ComponentDef>(body)
                .map_err(|e| e.to_string())
                .and_then(|d| d.validate().map_err(|e| e.to_string())),
            "def-color" => serde_json::from_str::<DefColor>(body)
                .map(|_| ())
                .map_err(|e| e.to_string()),
            "block" => serde_json::from_str::<Block>(body)
                .map(|_| ())
                .map_err(|e| e.to_string()),
            "requires" => serde_json::from_str::<Vec<PackRequirement>>(body)
                .map(|_| ())
                .map_err(|e| e.to_string()),
            "styles" => serde_json::from_str::<StyleSheet>(body)
                .map(|_| ())
                .map_err(|e| e.to_string()),
            "" => Err(
                "this example has no type tag, so nothing checks it; add one \
                       (e.g. ```json component-def)"
                    .into(),
            ),
            other => Err(format!(
                "unknown example type `{other}`; add it to `pack_guide.rs` or fix the tag"
            )),
        };
        *seen
            .entry(match tag.as_str() {
                "" => "untagged",
                t => Box::leak(t.to_string().into_boxed_str()),
            })
            .or_default() += 1;
        outcome
            .unwrap_or_else(|e| panic!("`{tag}` example in the guide does not parse: {e}\n{body}"));
    }

    // The guide has to actually show the two things it exists to explain.
    for required in ["pack-manifest", "component-def", "block", "requires"] {
        assert!(
            seen.contains_key(required),
            "the guide no longer shows a `{required}` example; it is the thing being documented"
        );
    }
}

/// The guide states the executable-extension decision. Asserted, because that paragraph is the
/// whole reason the format is declarative, and a guide that loses it leaves the next person to
/// re-derive the argument — or, worse, to assume there wasn't one.
#[test]
fn the_guide_records_why_a_pack_contains_no_code() {
    let text = std::fs::read_to_string(repo_root().join("docs/pack-authoring.md")).expect("guide");
    for phrase in [
        "a pack contains no code",
        "prefer a visible failure over silent press-corruption",
        "supplies *inputs*",
    ] {
        assert!(
            text.to_lowercase().contains(&phrase.to_lowercase()),
            "the guide no longer says: {phrase}"
        );
    }
}

/// Both worked example packs load, validate, and carry their provenance.
#[test]
fn the_shipped_example_packs_are_valid() {
    let dir = repo_root().join("examples/packs");
    let mut found = 0;
    for entry in std::fs::read_dir(&dir).expect("examples/packs").flatten() {
        let path = entry.path();
        if path.extension().is_none_or(|e| e != "json") {
            continue;
        }
        found += 1;
        let text = std::fs::read_to_string(&path).expect("read");
        let manifest =
            PackManifest::from_json(&text).unwrap_or_else(|e| panic!("{}: {e}", path.display()));
        assert!(!manifest.source.trim().is_empty());
        assert!(!manifest.license.trim().is_empty());
        assert_eq!(
            path.file_stem().unwrap().to_string_lossy(),
            manifest.name,
            "an example pack's filename should match its slug, so `pack install` reads naturally"
        );
        // Its definitions must resolve every style they name, from the pack's own stylesheet or
        // the bundled one. A definition whose styles were left behind sets in the default face on
        // every machine but its author's.
        let mut styles = StyleSheet::default();
        styles.paragraph.extend(
            manifest
                .styles
                .paragraph
                .iter()
                .map(|(k, v)| (k.clone(), *v)),
        );
        for def in manifest.components.values() {
            for name in def.style_names() {
                assert!(
                    styles.paragraph.contains_key(name),
                    "`{}` in {} names style `{name}`, which neither the pack nor the bundled \
                     stylesheet supplies",
                    def.name,
                    path.display()
                );
            }
        }
    }
    assert!(
        found >= 2,
        "spec 0061 asks for at least two worked examples, found {found}"
    );
}

/// The guide's `move` example and the shipped `pbta-moves` pack must not drift apart.
#[test]
fn the_guides_component_example_matches_the_shipped_pack() {
    let guide = std::fs::read_to_string(repo_root().join("docs/pack-authoring.md")).expect("guide");
    let example: ComponentDef = json_blocks(&guide)
        .into_iter()
        .find(|(tag, _)| tag == "component-def")
        .map(|(_, body)| serde_json::from_str(&body).expect("parses"))
        .expect("the guide shows a component definition");

    let text = std::fs::read_to_string(repo_root().join("examples/packs/pbta-moves.json"))
        .expect("the pbta-moves example pack");
    let shipped = PackManifest::from_json(&text).expect("parses");
    let packed = shipped
        .components
        .get(&example.name)
        .unwrap_or_else(|| panic!("`{}` is not in the shipped pack", example.name));

    assert_eq!(
        &example, packed,
        "the guide's `{}` example and the shipped pack have drifted; they are the same definition \
         and a reader will copy the guide's",
        example.name
    );
}

/// A document that requires both example packs lays out through them (spec 0056, end to end).
#[test]
fn a_document_requiring_the_example_packs_resolves_and_lays_out() {
    let root = std::env::temp_dir().join(format!("quill-guide-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&root);

    for name in ["grimdark", "pbta-moves"] {
        let text = std::fs::read_to_string(repo_root().join(format!("examples/packs/{name}.json")))
            .expect("example pack");
        let manifest = PackManifest::from_json(&text).expect("parses");
        quill_core_model::install(&manifest, Path::new("."), &root, false).expect("install");
    }

    let mut doc = Document::sample();
    doc.requires = vec![
        PackRequirement::new("grimdark", "1"),
        PackRequirement::new("pbta-moves", "1.0"),
    ];
    let packs = doc.resolve_packs(&root).expect("both resolve");
    doc.apply_packs(&packs).expect("apply");

    assert!(
        doc.requires.is_empty(),
        "a satisfied requirement is cleared, which is what lets the document lay out"
    );
    assert!(
        doc.components.contains_key("move"),
        "the `move` definition must have arrived from the pack"
    );
    assert!(
        doc.styles.paragraph.contains_key("move-outcome"),
        "and the styles it names with it"
    );
    let _ = std::fs::remove_dir_all(&root);
}
