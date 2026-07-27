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

    // Migration chain goes here. Each step mutates `obj` from version N to N+1 and falls through to
    // the next, so the first real bump (spec 0030's FORMAT_VERSION 2) is one added arm:
    //
    //     if found < 2 { migrate_1_to_2(obj); }
    //
    // Today FORMAT_VERSION is 1 and nothing is older, so there is nothing to run. What matters is
    // that `migrate` is already on every load path for when that bump lands.
    let _ = found;

    obj.insert("format_version".into(), FORMAT_VERSION.into());
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::Document;

    #[test]
    fn a_newer_format_version_is_refused_not_silently_downgraded() {
        let json = r#"{"format_version": 2, "page_setup": {"trim": {"w_pt": 1.0, "h_pt": 1.0},
                       "bleed_pt": 9.0, "facing_pages": true}}"#;
        match Document::from_json(json) {
            Err(LoadError::UnsupportedVersion { found, supported }) => {
                assert_eq!(found, 2);
                assert_eq!(supported, FORMAT_VERSION);
            }
            other => panic!("expected UnsupportedVersion, got {other:?}"),
        }
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
