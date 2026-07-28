//! The `.tpub` zip container — see `docs/format-spec.md` ("Container") and
//! `specs/0025-tpub-container-and-load-contract.md`.
//!
//! A `.tpub` is a zip holding `document.json` plus the linked originals it references:
//!
//! ```text
//! document.json      # the manifest (the `serde` types in this crate)
//! assets/            # linked originals (images), referenced by relative path
//! fonts/             # fonts used by the document
//! thumbnails/        # optional cached previews
//! ```
//!
//! ## Why opening extracts to a directory
//!
//! [`Asset::path`](crate::Asset::path) is a relative path, and until now nothing in the workspace
//! said what it was relative *to* — `export-pdf` hardcoded `Path::new(".")` and `quill-render`
//! made it a caller argument, so the two could disagree about where a document's images live. The
//! container is the natural owner of that answer, and it answers with a real filesystem directory:
//! both consumers already take a `&Path` base directory, and image decoding wants ordinary file
//! reads rather than a zip-streaming shim threaded through every call site.
//!
//! Extraction is therefore explicit — [`Tpub::open_into`] takes the destination — rather than
//! conjuring a temp directory internally. A hidden temp directory would be invisible state with no
//! owner and no defined lifetime; making the caller name it keeps "where are this document's assets
//! right now" answerable.

use std::fs::{self, File};
use std::io::{Read, Write};
use std::path::{Component, Path, PathBuf};

use crate::{Document, LoadError};

/// The manifest's fixed name inside the container.
pub const MANIFEST_NAME: &str = "document.json";

/// A `.tpub` opened onto the filesystem: the parsed manifest plus the directory its relative asset
/// paths resolve against.
#[derive(Debug, Clone, PartialEq)]
pub struct OpenedTpub {
    pub document: Document,
    /// The directory [`Asset::path`](crate::Asset::path) is relative to.
    pub asset_root: PathBuf,
}

/// Reader/writer for the `.tpub` container.
pub struct Tpub;

impl Tpub {
    /// Read just the manifest, without extracting anything.
    ///
    /// This is the cheap probe — it is what makes "reject a too-new document *before* writing its
    /// assets all over the filesystem" possible, and it is all a document browser or a file-picker
    /// preview needs.
    pub fn read_manifest(path: &Path) -> Result<Document, LoadError> {
        let file = File::open(path)
            .map_err(|e| LoadError::Container(format!("opening '{}': {e}", path.display())))?;
        let mut archive = zip::ZipArchive::new(file)
            .map_err(|e| LoadError::Container(format!("'{}': {e}", path.display())))?;
        let mut entry = archive.by_name(MANIFEST_NAME).map_err(|_| {
            LoadError::NotATpub(format!("'{}' has no {MANIFEST_NAME}", path.display()))
        })?;
        let mut text = String::new();
        entry
            .read_to_string(&mut text)
            .map_err(|e| LoadError::Container(format!("reading {MANIFEST_NAME}: {e}")))?;
        Document::from_json(&text)
    }

    /// Open a `.tpub`, extracting its payload into `extract_to`, and return the document together
    /// with the asset root its relative paths resolve against.
    ///
    /// The manifest is parsed and version-gated *first*: a document this build cannot understand
    /// must not leave a directory of half-extracted files behind.
    pub fn open_into(path: &Path, extract_to: &Path) -> Result<OpenedTpub, LoadError> {
        let document = Self::read_manifest(path)?;

        let file = File::open(path)
            .map_err(|e| LoadError::Container(format!("opening '{}': {e}", path.display())))?;
        let mut archive = zip::ZipArchive::new(file)
            .map_err(|e| LoadError::Container(format!("'{}': {e}", path.display())))?;

        fs::create_dir_all(extract_to).map_err(|e| {
            LoadError::Container(format!("creating '{}': {e}", extract_to.display()))
        })?;

        for i in 0..archive.len() {
            let mut entry = archive
                .by_index(i)
                .map_err(|e| LoadError::Container(format!("entry {i}: {e}")))?;
            let name = entry.name().to_string();
            if name == MANIFEST_NAME {
                continue;
            }
            // `entry.name()` is attacker-controlled for any document a user did not author. A
            // relative path containing `..` (or an absolute one) would let a crafted `.tpub` write
            // outside the extraction directory when opened — the classic zip-slip. Refuse loudly.
            let Some(rel) = safe_relative_path(&name) else {
                return Err(LoadError::Container(format!(
                    "entry '{name}' escapes the extraction directory"
                )));
            };
            let dest = extract_to.join(&rel);
            if entry.is_dir() {
                fs::create_dir_all(&dest).map_err(|e| {
                    LoadError::Container(format!("creating '{}': {e}", dest.display()))
                })?;
                continue;
            }
            if let Some(parent) = dest.parent() {
                fs::create_dir_all(parent).map_err(|e| {
                    LoadError::Container(format!("creating '{}': {e}", parent.display()))
                })?;
            }
            let mut bytes = Vec::new();
            entry
                .read_to_end(&mut bytes)
                .map_err(|e| LoadError::Container(format!("reading '{name}': {e}")))?;
            fs::write(&dest, &bytes)
                .map_err(|e| LoadError::Container(format!("writing '{}': {e}", dest.display())))?;
        }

        Ok(OpenedTpub {
            document,
            asset_root: extract_to.to_path_buf(),
        })
    }

    /// Write a `.tpub`: the manifest plus the given payload entries (`("assets/map.png", bytes)`).
    ///
    /// Entry names are container-relative and use `/` separators regardless of host platform, as
    /// the zip format requires — a Windows-authored `.tpub` must open on Linux.
    pub fn write(doc: &Document, path: &Path, entries: &[(&str, &[u8])]) -> Result<(), LoadError> {
        if let Some(parent) = path.parent() {
            if !parent.as_os_str().is_empty() {
                fs::create_dir_all(parent).map_err(|e| {
                    LoadError::Container(format!("creating '{}': {e}", parent.display()))
                })?;
            }
        }
        let file = File::create(path)
            .map_err(|e| LoadError::Container(format!("creating '{}': {e}", path.display())))?;
        let mut zw = zip::ZipWriter::new(file);
        let opts: zip::write::FileOptions<'_, ()> =
            zip::write::FileOptions::default().compression_method(zip::CompressionMethod::Deflated);

        let manifest = doc
            .to_json()
            .map_err(|e| LoadError::Parse(format!("serializing manifest: {e}")))?;
        zw.start_file(MANIFEST_NAME, opts)
            .map_err(|e| LoadError::Container(format!("{MANIFEST_NAME}: {e}")))?;
        zw.write_all(manifest.as_bytes())
            .map_err(|e| LoadError::Container(format!("{MANIFEST_NAME}: {e}")))?;

        for (name, bytes) in entries {
            if safe_relative_path(name).is_none() {
                return Err(LoadError::Container(format!(
                    "entry '{name}' is not a safe container-relative path"
                )));
            }
            zw.start_file(*name, opts)
                .map_err(|e| LoadError::Container(format!("'{name}': {e}")))?;
            zw.write_all(bytes)
                .map_err(|e| LoadError::Container(format!("'{name}': {e}")))?;
        }

        zw.finish()
            .map_err(|e| LoadError::Container(format!("finishing '{}': {e}", path.display())))?;
        Ok(())
    }
}

/// Convert a container entry name to a path that is guaranteed to stay under the extraction root,
/// or `None` if it does not.
///
/// Rejects absolute paths, drive prefixes and any `..` component. Note it rejects rather than
/// sanitizes: silently rewriting `../../etc/foo` to `etc/foo` would extract a file the document
/// never described, and a `.tpub` that tried to escape is not a document to be repaired.
pub(crate) fn safe_relative_path(name: &str) -> Option<PathBuf> {
    if name.is_empty() {
        return None;
    }
    let path = Path::new(name);
    let mut out = PathBuf::new();
    for component in path.components() {
        match component {
            Component::Normal(part) => out.push(part),
            Component::CurDir => {}
            Component::ParentDir | Component::RootDir | Component::Prefix(_) => return None,
        }
    }
    if out.as_os_str().is_empty() {
        None
    } else {
        Some(out)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::FORMAT_VERSION;

    /// Per repo convention: a process-unique temp dir, cleaned up best-effort by the caller.
    fn temp_dir(tag: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!("quill-tpub-{}-{tag}", std::process::id()));
        let _ = fs::remove_dir_all(&dir);
        fs::create_dir_all(&dir).expect("temp dir");
        dir
    }

    #[test]
    fn round_trips_a_document_and_its_assets() {
        let dir = temp_dir("roundtrip");
        let tpub = dir.join("book.tpub");
        let png = b"\x89PNG\r\n\x1a\n-not-really-a-png-but-bytes-are-bytes";

        let doc = Document::sample();
        Tpub::write(&doc, &tpub, &[("assets/map1.png", png)]).expect("write");

        let opened = Tpub::open_into(&tpub, &dir.join("extracted")).expect("open");
        assert_eq!(opened.document, doc, "manifest must survive the round trip");

        let asset = opened.asset_root.join("assets/map1.png");
        assert!(asset.exists(), "asset should be extracted under asset_root");
        assert_eq!(fs::read(&asset).expect("read asset"), png);

        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn round_trips_authored_layout_including_per_page_masters() {
        // `round_trips_a_document_and_its_assets` uses `Document::sample()`, whose masters and page
        // list are both empty — so it proves nothing about authored layout surviving the container.
        // Spec 0035's round-trip criterion is this test.
        let dir = temp_dir("masters");
        let tpub = dir.join("book.tpub");

        let mut doc = Document::sample();
        doc.master_pages = vec![
            crate::MasterPage::plain("opener"),
            crate::MasterPage::plain("body"),
        ];
        doc.default_master = Some("body".into());
        doc.pages = vec![
            crate::PageOverride {
                master: Some("opener".into()),
            },
            crate::PageOverride { master: None },
        ];

        Tpub::write(&doc, &tpub, &[("assets/map1.png", b"x")]).expect("write");
        let opened = Tpub::open_into(&tpub, &dir.join("extracted")).expect("open");
        assert_eq!(opened.document.pages, doc.pages);
        assert_eq!(opened.document.master_pages, doc.master_pages);
        assert_eq!(opened.document, doc);

        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn asset_root_resolves_the_documents_relative_paths() {
        // The whole point of the container: `Asset::path` is meaningful relative to `asset_root`.
        let dir = temp_dir("assetroot");
        let tpub = dir.join("book.tpub");
        let doc = Document::sample();
        let path = doc.assets[0].path.clone();
        Tpub::write(&doc, &tpub, &[(path.as_str(), b"bytes")]).expect("write");

        let opened = Tpub::open_into(&tpub, &dir.join("x")).expect("open");
        assert!(opened
            .asset_root
            .join(&opened.document.assets[0].path)
            .exists());

        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn a_too_new_document_is_refused_and_extracts_nothing() {
        let dir = temp_dir("toonew");
        let tpub = dir.join("future.tpub");
        // Hand-build a container whose manifest claims a future format version.
        {
            let file = File::create(&tpub).expect("create");
            let mut zw = zip::ZipWriter::new(file);
            let opts: zip::write::FileOptions<'_, ()> = zip::write::FileOptions::default();
            zw.start_file(MANIFEST_NAME, opts).expect("start");
            let manifest = format!(
                r#"{{"format_version": {}, "page_setup": {{"trim": {{"w_pt": 1.0, "h_pt": 1.0}},
                   "bleed_pt": 9.0, "facing_pages": true}}}}"#,
                FORMAT_VERSION + 98
            );
            zw.write_all(manifest.as_bytes()).expect("write");
            zw.start_file("assets/should-not-appear.bin", opts)
                .expect("start");
            zw.write_all(b"payload").expect("write");
            zw.finish().expect("finish");
        }

        let extract_to = dir.join("extracted");
        let err = Tpub::open_into(&tpub, &extract_to).unwrap_err();
        assert!(matches!(err, LoadError::UnsupportedVersion { .. }), "{err}");
        assert!(
            !extract_to.join("assets/should-not-appear.bin").exists(),
            "a refused document must not leave extracted files behind"
        );

        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn a_zip_without_a_manifest_is_not_a_tpub() {
        let dir = temp_dir("nomanifest");
        let path = dir.join("plain.zip");
        {
            let file = File::create(&path).expect("create");
            let mut zw = zip::ZipWriter::new(file);
            let opts: zip::write::FileOptions<'_, ()> = zip::write::FileOptions::default();
            zw.start_file("readme.txt", opts).expect("start");
            zw.write_all(b"hello").expect("write");
            zw.finish().expect("finish");
        }
        assert!(matches!(
            Tpub::read_manifest(&path),
            Err(LoadError::NotATpub(_))
        ));

        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn a_missing_file_is_a_container_error_not_a_panic() {
        let missing = std::env::temp_dir().join("quill-definitely-absent.tpub");
        assert!(matches!(
            Tpub::read_manifest(&missing),
            Err(LoadError::Container(_))
        ));
    }

    #[test]
    fn entries_that_escape_the_extraction_root_are_refused() {
        // Zip-slip: a crafted container must not be able to write outside `extract_to`.
        for escaping in [
            "../escape.bin",
            "assets/../../escape.bin",
            "/abs/escape.bin",
        ] {
            assert!(
                safe_relative_path(escaping).is_none(),
                "'{escaping}' should be refused"
            );
        }
        for ok in ["assets/map.png", "./fonts/x.ttf", "thumbnails/a/b.png"] {
            assert!(safe_relative_path(ok).is_some(), "'{ok}' should be allowed");
        }
    }

    #[test]
    fn writing_an_escaping_entry_name_is_refused() {
        let dir = temp_dir("writeescape");
        let err = Tpub::write(
            &Document::sample(),
            &dir.join("x.tpub"),
            &[("../evil.bin", b"x")],
        )
        .unwrap_err();
        assert!(matches!(err, LoadError::Container(_)));
        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn read_manifest_does_not_extract() {
        let dir = temp_dir("probe");
        let tpub = dir.join("book.tpub");
        Tpub::write(&Document::sample(), &tpub, &[("assets/map1.png", b"x")]).expect("write");

        let doc = Tpub::read_manifest(&tpub).expect("probe");
        assert_eq!(doc, Document::sample());
        // Nothing but the container itself should exist in the directory.
        let entries: Vec<_> = fs::read_dir(&dir)
            .expect("read dir")
            .filter_map(|e| e.ok())
            .map(|e| e.file_name())
            .collect();
        assert_eq!(entries.len(), 1, "probe must not extract: {entries:?}");

        let _ = fs::remove_dir_all(&dir);
    }
}
