//! The versioned load contract for `.tpub` manifests — see `docs/format-spec.md` ("Versioning")
//! and `specs/0025-tpub-container-and-load-contract.md`.
//!
//! The format spec has always promised that readers "reject formats newer than they understand and
//! migrate older ones forward". This module is that promise in code. Before this existed,
//! [`crate::Document::from_json`] deserialized any `format_version` silently, so a document written
//! by a newer Quill would load with its new fields dropped on the floor — the failure mode
//! `CLAUDE.md` forbids, since a silently half-loaded document can be saved back over the original
//! and lose the parts this build did not understand.
//!
//! Migration runs on the untyped [`serde_json::Value`] rather than on `Document`, because a v(N-1)
//! manifest by definition does not fit the v(N) `serde` types — that is what makes it old.

use std::fmt;

use crate::{FORMAT_VERSION, PACK_VERSION, TEMPLATE_VERSION};

/// Everything that can go wrong loading a document or a `.tpub` container.
///
/// Deliberately does not expose `serde_json::Error` in its public API: the manifest being JSON is
/// an implementation detail of the format, and callers that match on a `serde` error type would
/// make it impossible to change encoding later without a breaking change.
#[derive(Debug)]
pub enum LoadError {
    /// The manifest is not well-formed, or does not match the schema this build understands.
    Parse(String),
    /// The manifest declares a `format_version` newer than this build supports. This is a refusal,
    /// not a fallback: see the module docs.
    UnsupportedVersion { found: u32, supported: u32 },
    /// The container could not be read or written (I/O, or a malformed zip).
    Container(String),
    /// The container is structurally valid but not a `.tpub` (e.g. no `document.json`).
    NotATpub(String),
    /// Two blocks claim the same [`BlockId`](crate::BlockId). Refused rather than repaired: see
    /// [`Document::assign_missing_block_ids`](crate::Document::assign_missing_block_ids).
    DuplicateBlockId(u64),
    /// A user-authored template file (spec 0053) is not well-formed, or does not match the schema
    /// this build understands.
    ///
    /// Distinct from [`LoadError::Parse`] rather than reused, because an error has to name the
    /// thing that is wrong: a malformed *template* reported as a malformed document manifest sends
    /// the reader to the wrong file.
    TemplateParse(String),
    /// A template file declares a `template_version` newer than this build supports. A refusal, on
    /// the same reasoning as [`LoadError::UnsupportedVersion`].
    UnsupportedTemplateVersion { found: u32, supported: u32 },
    /// A component definition (spec 0054) is malformed, or declares a version this build does not
    /// understand. Its own variant, and it names the definition, because a definition is the unit
    /// a pack ships and a reader has to know which one to fix.
    ComponentDef(quill_components_ttrpg::ComponentDefError),
    /// A `.qpack` manifest (spec 0055) is not well-formed, or does not match the schema.
    PackParse(String),
    /// A pack declares a `pack_version` newer than this build supports. A refusal, on the same
    /// reasoning as [`LoadError::UnsupportedVersion`].
    UnsupportedPackVersion { found: u32, supported: u32 },
    /// The container is a readable zip but not a `.qpack` (no `pack.json`).
    NotAQpack(String),
    /// A pack omits its `source` or `license`. Content arriving from a stranger with no provenance
    /// is content nobody should install, so this is a refusal rather than a warning.
    PackMissingProvenance { pack: String, field: String },
    /// A pack carries a path that is absolute or escapes its root, in its manifest or as a zip
    /// entry. A pack is the one artifact here that routinely comes from someone else.
    PackUnsafePath { pack: String, path: String },
    /// A document requires a pack that is not installed (spec 0056). Names what is installed too:
    /// a reader who has just been refused needs to know what they *do* have.
    PackNotInstalled {
        pack: String,
        version: String,
        installed: Vec<String>,
        root: String,
    },
    /// Two resolved packs both define the same component or template. Refused rather than
    /// resolved last-one-wins, which would make the winner depend on iteration order.
    PackConflict {
        what: String,
        item: String,
        first: String,
        second: String,
    },
    /// A different pack is already installed at this name and version.
    PackAlreadyInstalled {
        pack: String,
        version: String,
        path: String,
    },
}

impl fmt::Display for LoadError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            LoadError::Parse(m) => write!(f, "malformed document manifest: {m}"),
            LoadError::UnsupportedVersion { found, supported } => write!(
                f,
                "document format version {found} is newer than this build supports \
                 (up to {supported}); upgrade Quill to open it"
            ),
            LoadError::Container(m) => write!(f, "reading .tpub container: {m}"),
            LoadError::NotATpub(m) => write!(f, "not a .tpub container: {m}"),
            LoadError::DuplicateBlockId(id) => write!(
                f,
                "two content blocks share id {id}; block ids must be unique within a document"
            ),
            LoadError::TemplateParse(m) => write!(f, "malformed template file: {m}"),
            LoadError::UnsupportedTemplateVersion { found, supported } => write!(
                f,
                "template file version {found} is newer than this build supports \
                 (up to {supported}); upgrade Quill to use it"
            ),
            LoadError::ComponentDef(e) => write!(f, "{e}"),
            LoadError::PackParse(m) => write!(f, "malformed pack manifest: {m}"),
            LoadError::UnsupportedPackVersion { found, supported } => write!(
                f,
                "pack format version {found} is newer than this build supports \
                 (up to {supported}); upgrade Quill to install it"
            ),
            LoadError::NotAQpack(m) => write!(f, "not a .qpack container: {m}"),
            LoadError::PackMissingProvenance { pack, field } => write!(
                f,
                "pack `{pack}` has no `{field}`; a pack must say where it came from and \
                 under what licence before it can be installed"
            ),
            LoadError::PackUnsafePath { pack, path } => write!(
                f,
                "pack `{pack}` carries the path '{path}', which is absolute or escapes the \
                 pack root"
            ),
            LoadError::PackNotInstalled {
                pack,
                version,
                installed,
                root,
            } => {
                let want = if version.trim().is_empty() {
                    "any version".to_string()
                } else {
                    format!("version {version}")
                };
                let have = if installed.is_empty() {
                    "nothing".to_string()
                } else {
                    installed.join(", ")
                };
                write!(
                    f,
                    "this document requires pack `{pack}` ({want}), which is not installed in \
                     '{root}' — installed there: {have}. Install it with `quill pack install`."
                )
            }
            LoadError::PackConflict {
                what,
                item,
                first,
                second,
            } => write!(
                f,
                "packs `{first}` and `{second}` both define the {what} `{item}`; \
                 uninstall one or use a document-level definition"
            ),
            LoadError::PackAlreadyInstalled {
                pack,
                version,
                path,
            } => write!(
                f,
                "a different pack `{pack}` {version} is already installed at '{path}'; \
                 pass --force to replace it"
            ),
        }
    }
}

impl std::error::Error for LoadError {}

/// Bring a manifest forward to [`FORMAT_VERSION`], or refuse it.
///
/// The chain is exhaustive by construction: each arm migrates exactly one version forward and falls
/// through to the next, so adding a v2 only means adding one arm. A manifest with no
/// `format_version` at all is treated as v1 — the field was always written, but tolerating its
/// absence costs nothing and gives hand-authored test manifests a shorter form.
pub fn migrate(value: &mut serde_json::Value) -> Result<(), LoadError> {
    let obj = value
        .as_object_mut()
        .ok_or_else(|| LoadError::Parse("manifest root is not a JSON object".into()))?;

    let found = match obj.get("format_version") {
        None => FORMAT_VERSION,
        Some(v) => v.as_u64().ok_or_else(|| {
            LoadError::Parse("`format_version` is not a non-negative integer".into())
        })? as u32,
    };

    if found > FORMAT_VERSION {
        return Err(LoadError::UnsupportedVersion {
            found,
            supported: FORMAT_VERSION,
        });
    }

    // Migration chain. Each step mutates `obj` from version N to N+1 and falls through to the next,
    // so a future bump is one added arm rather than a redesign.
    if found < 2 {
        migrate_1_to_2(obj);
    }
    if found < 3 {
        migrate_2_to_3(obj);
    }

    obj.insert("format_version".into(), FORMAT_VERSION.into());
    Ok(())
}

/// v1 → v2 (spec 0030): master pages, per-page margins.
///
/// Structurally a no-op, and deliberately written as one rather than skipped. Every added field is
/// `serde(default)`, so a v1 manifest already deserializes into the v2 types with the right values —
/// no margins, no masters, which is precisely what a v1 document meant. Explicitly defaulting them
/// here rather than relying on that keeps the chain readable as a record of what each version
/// changed, and means a later step that *does* need to rewrite a field has an obvious place to go.
fn migrate_1_to_2(obj: &mut serde_json::Map<String, serde_json::Value>) {
    if let Some(setup) = obj.get_mut("page_setup").and_then(|v| v.as_object_mut()) {
        setup
            .entry("margins")
            .or_insert_with(|| serde_json::json!({}));
    }
    obj.entry("master_pages")
        .or_insert_with(|| serde_json::json!([]));
}

/// v2 → v3 (spec 0047): a text master static gains `align` and `mirror`.
///
/// Structurally a no-op, on the same principle as [`migrate_1_to_2`] and written out for the same
/// reason. A v2 static *meant* "left-aligned, in a rect that does not mirror across the spine", and
/// that is exactly what the defaults produce — so a v2 document lays out unchanged. Defaulting them
/// here rather than relying on `serde(default)` keeps the chain readable as a record of what each
/// version changed.
///
/// The written-out defaults do not survive back into the manifest: both fields are
/// `skip_serializing_if`-defaulted, so a migrated document re-serializes without them and its
/// exported `/ID` does not move. That is deliberate — see spec 0047's risks.
fn migrate_2_to_3(obj: &mut serde_json::Map<String, serde_json::Value>) {
    let Some(masters) = obj.get_mut("master_pages").and_then(|v| v.as_array_mut()) else {
        return;
    };
    for statics in masters
        .iter_mut()
        .filter_map(|m| m.get_mut("statics"))
        .filter_map(|s| s.as_array_mut())
    {
        for s in statics.iter_mut() {
            let Some(s) = s.as_object_mut() else { continue };
            // An image static gains neither field; only text is aligned or mirrored.
            if s.get("kind").and_then(|k| k.as_str()) != Some("text") {
                continue;
            }
            s.entry("align").or_insert_with(|| "left".into());
            s.entry("mirror").or_insert_with(|| false.into());
        }
    }
}

/// Bring a **template file** (spec 0053) forward to [`TEMPLATE_VERSION`], or refuse it.
///
/// Deliberately written beside [`migrate`], arm for arm, so the second version chain in this
/// workspace cannot be built to a different shape than the first. Same rules: the gate runs on the
/// untyped value before deserialization, an absent version is treated as current, an older one
/// migrates forward one step at a time, and a newer one is refused rather than half-loaded.
///
/// The chain is empty because nothing is older than v1. The shape is here so the first real bump is
/// an added arm rather than a redesign — and spec 0053 states *when* that bump is owed: whenever the
/// template envelope changes, **or** whenever a [`FORMAT_VERSION`] bump changes the serialized shape
/// of `PageSetup`, `StyleSheet`, `MasterPage` or `PageOverride`, all four of which a template file
/// embeds. Spec 0047's v2 → v3 was exactly such a bump; had template files existed then, they would
/// have owed a migration too.
pub fn migrate_template(value: &mut serde_json::Value) -> Result<(), LoadError> {
    let obj = value.as_object_mut().ok_or_else(|| {
        LoadError::TemplateParse("template file root is not a JSON object".into())
    })?;

    let found = match obj.get("template_version") {
        None => TEMPLATE_VERSION,
        Some(v) => v.as_u64().ok_or_else(|| {
            LoadError::TemplateParse("`template_version` is not a non-negative integer".into())
        })? as u32,
    };

    if found > TEMPLATE_VERSION {
        return Err(LoadError::UnsupportedTemplateVersion {
            found,
            supported: TEMPLATE_VERSION,
        });
    }

    obj.insert("template_version".into(), TEMPLATE_VERSION.into());
    Ok(())
}

/// Bring a `.qpack` manifest forward to [`PACK_VERSION`], or refuse it (spec 0055).
///
/// The same three-step shape as the two above — read the declared version, refuse a newer one,
/// stamp the current one — because a third differently-shaped version gate would be a third thing
/// to get wrong.
pub fn migrate_pack(value: &mut serde_json::Value) -> Result<(), LoadError> {
    let obj = value
        .as_object_mut()
        .ok_or_else(|| LoadError::PackParse("pack manifest root is not a JSON object".into()))?;

    let found = match obj.get("pack_version") {
        None => PACK_VERSION,
        Some(v) => v.as_u64().ok_or_else(|| {
            LoadError::PackParse("`pack_version` is not a non-negative integer".into())
        })? as u32,
    };

    if found > PACK_VERSION {
        return Err(LoadError::UnsupportedPackVersion {
            found,
            supported: PACK_VERSION,
        });
    }

    obj.insert("pack_version".into(), PACK_VERSION.into());
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::Document;

    #[test]
    fn a_newer_format_version_is_refused_not_silently_downgraded() {
        // Expressed relative to FORMAT_VERSION rather than as a literal, so this keeps testing
        // "one version newer than we understand" across every future bump. Written with a literal
        // 2, it silently stopped testing anything the moment spec 0030 bumped to 2.
        let next = FORMAT_VERSION + 1;
        let json = format!(
            r#"{{"format_version": {next}, "page_setup": {{"trim": {{"w_pt": 1.0, "h_pt": 1.0}},
               "bleed_pt": 9.0, "facing_pages": true}}}}"#
        );
        match Document::from_json(&json) {
            Err(LoadError::UnsupportedVersion { found, supported }) => {
                assert_eq!(found, next);
                assert_eq!(supported, FORMAT_VERSION);
            }
            other => panic!("expected UnsupportedVersion, got {other:?}"),
        }
    }

    #[test]
    fn a_v1_manifest_migrates_forward_to_v2() {
        // The first real exercise of the chain spec 0025 built: a document written before master
        // pages existed must still open, and must mean what it meant then — no margins, no masters.
        let json = r#"{
            "format_version": 1,
            "page_setup": {"trim": {"w_pt": 432.0, "h_pt": 648.0}, "bleed_pt": 9.0,
                           "facing_pages": true},
            "content": [{"kind": "body", "text": "old", "color": {"space": "gray", "v": 0.0}}]
        }"#;
        let doc = Document::from_json(json).expect("a v1 manifest must still load");
        assert_eq!(doc.format_version, FORMAT_VERSION);
        assert_eq!(doc.page_setup.margins, crate::Margins::default());
        assert!(doc.master_pages.is_empty());
        assert!(doc.default_master.is_none());
        assert_eq!(doc.content.len(), 1);
    }

    #[test]
    fn a_migrated_v1_document_lays_out_as_it_did_before() {
        // Migration must be behavior-preserving, not merely parse-succeeding: zero margins and no
        // master reproduce the full-page frame a v1 document was laid out in.
        let json = r#"{
            "format_version": 1,
            "page_setup": {"trim": {"w_pt": 432.0, "h_pt": 648.0}, "bleed_pt": 9.0,
                           "facing_pages": true}
        }"#;
        let doc = Document::from_json(json).expect("load");
        assert_eq!(doc.page_setup.margins.top_pt, 0.0);
        assert_eq!(doc.page_setup.margins.inside_pt, 0.0);
    }

    /// A document written by a v2 build, committed **as bytes** (spec 0047).
    ///
    /// Generated by no code in this workspace, and that is the point: a fixture the current
    /// serializer wrote would migrate correctly by construction and prove nothing. This one is what
    /// a build that no longer exists actually put on disk — a folio inset to the fore-edge margin,
    /// spec 0036's workaround for the defect 0047 removes.
    const V2_MASTERS: &str = include_str!("../assets/v2-masters.json");

    #[test]
    fn a_v2_manifest_migrates_forward_to_v3() {
        let doc = Document::from_json(V2_MASTERS).expect("a v2 manifest must still load");
        assert_eq!(doc.format_version, FORMAT_VERSION);
        // It means what it meant: left-aligned, unmirrored, in the rect it was authored with.
        let master = doc.master_for(0).expect("the fixture's master");
        assert_eq!(master.statics.len(), 2);
        let crate::MasterStatic::Text {
            rect,
            align,
            mirror,
            ..
        } = &master.statics[0]
        else {
            panic!("expected the folio")
        };
        assert_eq!(*align, crate::StaticAlign::Left);
        assert!(!*mirror);
        assert_eq!(rect.x_pt, 40.0);
        assert_eq!(rect.w_pt, 352.0);
    }

    #[test]
    fn migrating_a_v2_document_does_not_move_its_exported_identity() {
        // The hazard a format bump carries: the exported PDF's `/ID` is a hash of the manifest text
        // (`export-pdf`'s `doc_id_bytes`), so a migration that wrote two defaulted keys into every
        // static would change the identifier of every document that has furniture — and every
        // `.tpub` in existence would re-export as a different file. `skip_serializing_if` is what
        // stops it, and this is the assertion that keeps it stopped.
        let migrated = Document::from_json(V2_MASTERS).expect("load");
        let json = migrated.to_json().expect("save");
        // Scoped to the masters: `align` is also a paragraph-style field, so a whole-manifest
        // search would pass for the wrong reason.
        let masters = serde_json::to_string(&migrated.master_pages).expect("masters");
        assert!(
            !masters.contains("\"align\""),
            "migration must not add `align`: {masters}"
        );
        assert!(
            !masters.contains("\"mirror\""),
            "migration must not add `mirror`: {masters}"
        );

        // And the manifest is byte-identical to the one the same document reaches as a native v3
        // read — i.e. the migration is lossless in the identity direction, not merely in meaning.
        let native = V2_MASTERS.replace("\"format_version\": 2", "\"format_version\": 3");
        let native = Document::from_json(&native).expect("load as v3");
        assert_eq!(json, native.to_json().expect("save"));
    }

    #[test]
    fn the_whole_version_chain_migrates_forward() {
        // v1 -> v2 -> v3 in one hop, and every step's meaning preserved: a v1 document has no
        // margins and no masters; a v2 one has both, with unaligned statics.
        let v1 = Document::from_json(
            r#"{"format_version": 1,
                "page_setup": {"trim": {"w_pt": 432.0, "h_pt": 648.0}, "bleed_pt": 9.0,
                               "facing_pages": true}}"#,
        )
        .expect("v1");
        assert_eq!(v1.format_version, FORMAT_VERSION);
        assert_eq!(v1.page_setup.margins, crate::Margins::default());
        assert!(v1.master_pages.is_empty());

        let v2 = Document::from_json(V2_MASTERS).expect("v2");
        assert_eq!(v2.format_version, FORMAT_VERSION);
        assert_eq!(v2.master_pages.len(), 1);

        // And the far end of the chain still refuses: v(current + 1) is not a document this build
        // may open, whatever it contains.
        let next = FORMAT_VERSION + 1;
        let v4 = V2_MASTERS.replace(
            "\"format_version\": 2",
            &format!("\"format_version\": {next}"),
        );
        assert!(matches!(
            Document::from_json(&v4),
            Err(LoadError::UnsupportedVersion { found, .. }) if found == next
        ));
    }

    #[test]
    fn the_documented_manifest_example_parses() {
        // The anti-drift guard the repo already uses for the authoring syntax (`import.rs`): the
        // manifest example in `docs/format-spec.md` is *the* example someone copies, so it is
        // parsed here rather than trusted. Extracted from the doc itself, never duplicated.
        const DOC: &str = include_str!("../../../docs/format-spec.md");
        let example = DOC
            .split("```json")
            .nth(1)
            .and_then(|rest| rest.split("```").next())
            .expect("docs/format-spec.md must carry a ```json manifest example");
        assert!(
            example.contains("\"format_version\""),
            "the first ```json block must be the manifest example"
        );

        let doc = Document::from_json(example).expect("the documented manifest must load");
        assert_eq!(doc.format_version, FORMAT_VERSION);
        // And it must show what it claims to: a master with both spec-0047 fields exercised. Page 1
        // rather than 0, because the example also demonstrates a page override — page 0 is the
        // chapter opener, which carries no furniture.
        assert_eq!(
            doc.master_for(0).map(|m| m.name.as_str()),
            Some("chapter-opener")
        );
        let master = doc.master_for(1).expect("the documented body master");
        let aligns: Vec<(crate::StaticAlign, bool)> = master
            .statics
            .iter()
            .filter_map(|s| match s {
                crate::MasterStatic::Text { align, mirror, .. } => Some((*align, *mirror)),
                crate::MasterStatic::Image { .. } => None,
            })
            .collect();
        assert_eq!(
            aligns,
            vec![
                (crate::StaticAlign::Center, false),
                (crate::StaticAlign::Outside, true)
            ]
        );
    }

    #[test]
    fn a_far_future_version_is_refused_too() {
        let json = r#"{"format_version": 99, "page_setup": {"trim": {"w_pt": 1.0, "h_pt": 1.0},
                       "bleed_pt": 9.0, "facing_pages": true}}"#;
        assert!(matches!(
            Document::from_json(json),
            Err(LoadError::UnsupportedVersion { found: 99, .. })
        ));
    }

    #[test]
    fn an_absent_format_version_migrates_forward_to_current() {
        let json = r#"{"page_setup": {"trim": {"w_pt": 1.0, "h_pt": 1.0},
                       "bleed_pt": 9.0, "facing_pages": true}}"#;
        let doc = Document::from_json(json).expect("absent version should migrate, not fail");
        assert_eq!(doc.format_version, FORMAT_VERSION);
    }

    #[test]
    fn malformed_json_is_a_parse_error_not_a_panic() {
        // Bind the error to `LoadError` explicitly: this is the assertion that the public signature
        // no longer leaks `serde_json::Error`.
        let err: LoadError = Document::from_json("{ this is not json").unwrap_err();
        assert!(matches!(err, LoadError::Parse(_)));
    }

    #[test]
    fn a_non_object_manifest_is_a_parse_error() {
        let err = Document::from_json("[1, 2, 3]").unwrap_err();
        assert!(matches!(err, LoadError::Parse(_)));
    }

    #[test]
    fn a_non_integer_format_version_is_a_parse_error() {
        let err = Document::from_json(r#"{"format_version": "one"}"#).unwrap_err();
        assert!(matches!(err, LoadError::Parse(_)));
    }

    #[test]
    fn display_names_the_versions_so_the_message_is_actionable() {
        let e = LoadError::UnsupportedVersion {
            found: 7,
            supported: 1,
        };
        let msg = e.to_string();
        assert!(msg.contains('7') && msg.contains('1'), "got: {msg}");
    }
}
