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

use crate::FORMAT_VERSION;

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
