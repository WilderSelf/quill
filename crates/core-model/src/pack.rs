//! The `.qpack` content pack — a shareable bundle of templates, styles, component definitions and
//! assets (spec 0055).
//!
//! Spec 0054 made a component declarable. A declaration that cannot leave the document it was
//! written in is not an ecosystem, and this is how it leaves: a zip with a manifest, versioned and
//! version-gated, following [`crate::container`]'s `.tpub` precedent to the letter because a pack
//! is the same *kind* of object and a second, differently-shaped container would be a second thing
//! to get wrong.
//!
//! ```text
//! pack.json          # identity, provenance, and all declarative content
//! assets/            # linked originals the pack's templates and components reference
//! ```
//!
//! ## Why provenance is mandatory
//!
//! [`PackManifest::source`] and [`PackManifest::license`] are required and non-empty, checked on
//! read *and* on write. Content arriving from a stranger with no provenance is content nobody
//! should install, and a field that may be empty is a field that will be. Making the writer refuse
//! too means a pack that cannot be installed cannot be produced in the first place.
//!
//! ## Why the path check is stricter than a document's
//!
//! A `.tpub` refuses a zip *entry* that escapes on extraction. A `.qpack` refuses that and also an
//! [`Asset::path`] inside the manifest that is absolute or traverses — before a single byte is
//! written anywhere — because a pack is the one artifact in this system that routinely comes from
//! someone else.

use std::collections::BTreeMap;
use std::fs::{self, File};
use std::io::{Read, Write};
use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

use crate::container::safe_relative_path;
use crate::{version, Asset, ComponentDef, LoadError, StyleSheet, Template};

/// The current `.qpack` container format version.
///
/// Its own integer, distinct from `FORMAT_VERSION`, `TEMPLATE_VERSION` and
/// `COMPONENT_DEF_VERSION`, on spec 0053's reasoning: these version different artifacts, and
/// coupling them re-versions every pack ever written whenever any one of them moves.
pub const PACK_VERSION: u32 = 1;

/// The manifest's fixed name inside the container.
pub const PACK_MANIFEST_NAME: &str = "pack.json";

/// Everything a pack carries.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct PackManifest {
    /// The **container format** version — quill's. Not `serde(default)`: the version gate inserts
    /// it before deserialization, so an absent field is *decided* rather than defaulted past.
    pub pack_version: u32,
    /// Slug a document names the pack by (spec 0056).
    pub name: String,
    /// Human-readable label.
    pub title: String,
    /// One line, shown by `quill pack info` and `quill pack list`.
    #[serde(default)]
    pub description: String,
    /// The **pack's own** content version — the author's, not quill's. A publisher shipping
    /// "Ashen Vault 1.2.0" is making no claim about the container format.
    pub version: String,
    /// Where this came from: a URL, a repository, a person. Required and non-empty.
    pub source: String,
    /// The licence the content is offered under. Required and non-empty.
    pub license: String,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub templates: Vec<Template>,
    #[serde(default)]
    pub styles: StyleSheet,
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub components: BTreeMap<String, ComponentDef>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub assets: Vec<Asset>,
}

impl PackManifest {
    /// A manifest with the current format version, the given identity and provenance, and nothing
    /// in it yet.
    pub fn new(
        name: impl Into<String>,
        title: impl Into<String>,
        version: impl Into<String>,
        source: impl Into<String>,
        license: impl Into<String>,
    ) -> Self {
        Self {
            pack_version: PACK_VERSION,
            name: name.into(),
            title: title.into(),
            description: String::new(),
            version: version.into(),
            source: source.into(),
            license: license.into(),
            templates: Vec::new(),
            styles: StyleSheet::default(),
            components: BTreeMap::new(),
            assets: Vec::new(),
        }
    }

    /// Refuse a pack this build should not install.
    ///
    /// Runs on read and on write. Covers provenance, asset paths, and — through spec 0054's own
    /// validator — every component definition the pack carries, so a malformed definition is caught
    /// at the container boundary rather than at layout, where there is no error channel.
    pub fn validate(&self) -> Result<(), LoadError> {
        if self.name.trim().is_empty() {
            return Err(LoadError::PackParse("pack `name` is empty".into()));
        }
        if self.version.trim().is_empty() {
            return Err(LoadError::PackParse(format!(
                "pack `{}` has an empty `version`",
                self.name
            )));
        }
        for (field, value) in [("source", &self.source), ("license", &self.license)] {
            if value.trim().is_empty() {
                return Err(LoadError::PackMissingProvenance {
                    pack: self.name.clone(),
                    field: field.into(),
                });
            }
        }
        for asset in &self.assets {
            if safe_relative_path(&asset.path).is_none() {
                return Err(LoadError::PackUnsafePath {
                    pack: self.name.clone(),
                    path: asset.path.clone(),
                });
            }
        }
        for (key, def) in &self.components {
            def.validate().map_err(LoadError::ComponentDef)?;
            if &def.name != key {
                return Err(LoadError::ComponentDef(
                    quill_components_ttrpg::ComponentDefError::NameMismatch {
                        key: key.clone(),
                        def: def.name.clone(),
                    },
                ));
            }
        }
        Ok(())
    }

    /// Turn a finished book into a reusable pack (spec 0057).
    ///
    /// **A pack is a look, not a book.** No blocks, no paragraph text, no stat block, no table, no
    /// image placed in the flow — a pack that carried its author's prose would make every book
    /// built from it a derivative of that prose.
    ///
    /// Three inclusion rules, each of which is a decision rather than a convenience:
    ///
    /// - Only [`Document::components`] travels, never the bundled definitions. Every quill already
    ///   has those, and shipping a copy would pin a definition the pack's author never edited to
    ///   whatever this build's version of it happened to be.
    /// - Only assets referenced by a [`MasterStatic::Image`] travel. Furniture art is design; a
    ///   rule ornament in a running head belongs to the look. Art placed in the *flow* is content
    ///   and stays behind. "Extract the assets" would take the art, which is why the rule is
    ///   stated this precisely.
    /// - Provenance is a parameter, not a default. Spec 0055 would refuse an unprovenanced pack at
    ///   write time anyway, and that is a worse place to learn it.
    pub fn extract_from(
        doc: &crate::Document,
        name: impl Into<String>,
        title: impl Into<String>,
        version: impl Into<String>,
        source: impl Into<String>,
        license: impl Into<String>,
    ) -> PackManifest {
        let name = name.into();
        let title = title.into();
        let template = Template::from_document(
            doc,
            name.clone(),
            title.clone(),
            format!(
                "Extracted from a document with {} block(s).",
                doc.content.len()
            ),
        );

        // Asset ids the master furniture references, and nothing else.
        let mut furniture: std::collections::BTreeSet<&str> = Default::default();
        for master in &doc.master_pages {
            for stat in &master.statics {
                if let crate::MasterStatic::Image { asset, .. } = stat {
                    furniture.insert(asset.as_str());
                }
            }
        }

        let mut m = PackManifest::new(name, title, version, source, license);
        m.description = template.description.clone();
        m.styles = doc.styles.clone();
        m.components = doc.components.clone();
        m.templates = vec![template];
        m.assets = doc
            .assets
            .iter()
            .filter(|a| furniture.contains(a.id.as_str()))
            .cloned()
            .collect();
        m
    }

    pub fn to_json(&self) -> Result<String, serde_json::Error> {
        serde_json::to_string_pretty(self)
    }

    /// Parse a manifest, migrating it forward if older than [`PACK_VERSION`] and refusing it if
    /// newer, then validating it.
    pub fn from_json(s: &str) -> Result<Self, LoadError> {
        let mut value: serde_json::Value =
            serde_json::from_str(s).map_err(|e| LoadError::PackParse(e.to_string()))?;
        version::migrate_pack(&mut value)?;
        let manifest: PackManifest =
            serde_json::from_value(value).map_err(|e| LoadError::PackParse(e.to_string()))?;
        manifest.validate()?;
        Ok(manifest)
    }
}

/// A `.qpack` opened onto the filesystem: the parsed manifest plus the directory its relative asset
/// paths resolve against. The `.tpub` shape, for the same reason — see [`crate::OpenedTpub`].
#[derive(Debug, Clone, PartialEq)]
pub struct OpenedPack {
    pub manifest: PackManifest,
    pub asset_root: PathBuf,
}

/// Reader/writer for the `.qpack` container.
pub struct Qpack;

impl Qpack {
    /// Read just the manifest, without extracting anything.
    ///
    /// The cheap probe, and what makes "refuse a pack before writing its assets across the
    /// filesystem" possible — which matters more here than for a document, because a pack is
    /// content from a stranger.
    pub fn read_manifest(path: &Path) -> Result<PackManifest, LoadError> {
        let file = File::open(path)
            .map_err(|e| LoadError::Container(format!("opening '{}': {e}", path.display())))?;
        let mut archive = zip::ZipArchive::new(file)
            .map_err(|e| LoadError::Container(format!("'{}': {e}", path.display())))?;
        let mut entry = archive.by_name(PACK_MANIFEST_NAME).map_err(|_| {
            LoadError::NotAQpack(format!("'{}' has no {PACK_MANIFEST_NAME}", path.display()))
        })?;
        let mut text = String::new();
        entry
            .read_to_string(&mut text)
            .map_err(|e| LoadError::Container(format!("reading {PACK_MANIFEST_NAME}: {e}")))?;
        PackManifest::from_json(&text)
    }

    /// Open a `.qpack`, extracting its payload into `extract_to`.
    ///
    /// The manifest is parsed, version-gated and validated *first*: a pack this build refuses must
    /// not leave a directory of half-extracted files behind.
    pub fn open_into(path: &Path, extract_to: &Path) -> Result<OpenedPack, LoadError> {
        let manifest = Self::read_manifest(path)?;

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
            if name == PACK_MANIFEST_NAME {
                continue;
            }
            // Zip-slip, refused rather than sanitized: rewriting `../../etc/foo` to `etc/foo`
            // would extract a file the pack never described, and a pack that tried to escape is
            // not a pack to be repaired.
            let Some(rel) = safe_relative_path(&name) else {
                return Err(LoadError::PackUnsafePath {
                    pack: manifest.name.clone(),
                    path: name,
                });
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

        Ok(OpenedPack {
            manifest,
            asset_root: extract_to.to_path_buf(),
        })
    }

    /// Write a `.qpack`: the manifest plus the given payload entries.
    ///
    /// Validates before writing anything, so a pack missing its provenance is never produced.
    pub fn write(
        manifest: &PackManifest,
        path: &Path,
        entries: &[(&str, &[u8])],
    ) -> Result<(), LoadError> {
        manifest.validate()?;
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

        let text = manifest
            .to_json()
            .map_err(|e| LoadError::PackParse(format!("serializing manifest: {e}")))?;
        zw.start_file(PACK_MANIFEST_NAME, opts)
            .map_err(|e| LoadError::Container(format!("{PACK_MANIFEST_NAME}: {e}")))?;
        zw.write_all(text.as_bytes())
            .map_err(|e| LoadError::Container(format!("{PACK_MANIFEST_NAME}: {e}")))?;

        for (name, bytes) in entries {
            if safe_relative_path(name).is_none() {
                return Err(LoadError::PackUnsafePath {
                    pack: manifest.name.clone(),
                    path: (*name).to_string(),
                });
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

#[cfg(test)]
mod tests {
    use super::*;
    use crate::Document;

    fn tmp_dir(tag: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!("quill-qpack-{tag}-{}", std::process::id()));
        let _ = fs::remove_dir_all(&dir);
        fs::create_dir_all(&dir).expect("temp dir");
        dir
    }

    fn sample_pack() -> PackManifest {
        let mut m = PackManifest::new(
            "ashen-vault",
            "The Ashen Vault",
            "1.2.0",
            "https://example.invalid/ashen-vault",
            "CC-BY-4.0",
        );
        m.description = "A house style for grim dungeon-crawl books.".into();
        m.templates = vec![Template::bundled()[0].clone()];
        m.components.insert(
            crate::STATBLOCK_COMPONENT.into(),
            crate::statblock_definition(),
        );
        m.styles = StyleSheet::default();
        m
    }

    #[test]
    fn a_pack_round_trips_through_the_container() {
        let dir = tmp_dir("roundtrip");
        let path = dir.join("ashen.qpack");
        let pack = sample_pack();
        Qpack::write(&pack, &path, &[("assets/cover.png", b"not-really-a-png")]).expect("write");

        let read = Qpack::read_manifest(&path).expect("read manifest");
        assert_eq!(read, pack, "the manifest must survive the container");

        let opened = Qpack::open_into(&path, &dir.join("out")).expect("open");
        assert_eq!(opened.manifest, pack);
        assert_eq!(
            fs::read(opened.asset_root.join("assets/cover.png")).expect("payload"),
            b"not-really-a-png"
        );
        let _ = fs::remove_dir_all(&dir);
    }

    /// Spec 0053's precedent, applied: a format that round-trips the *struct* but builds a
    /// different *document* proves nothing. So the comparison is over what the templates make.
    #[test]
    fn every_bundled_template_survives_a_pack_round_trip() {
        let dir = tmp_dir("templates");
        let path = dir.join("bundled.qpack");
        let mut pack = PackManifest::new(
            "bundled",
            "Bundled templates",
            "1.0.0",
            "quill",
            "MIT OR Apache-2.0",
        );
        pack.templates = Template::bundled().to_vec();
        Qpack::write(&pack, &path, &[]).expect("write");

        let read = Qpack::read_manifest(&path).expect("read");
        assert_eq!(read.templates.len(), Template::bundled().len());
        for (before, after) in Template::bundled().iter().zip(&read.templates) {
            assert_eq!(before, after, "`{}` changed shape", before.name);
            assert_eq!(
                Document::from_template(before),
                Document::from_template(after),
                "`{}` round-trips as a struct but builds a different document",
                before.name
            );
        }
        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn a_pack_with_no_provenance_is_refused_on_read_and_on_write() {
        let dir = tmp_dir("provenance");
        for field in ["source", "license"] {
            let mut pack = sample_pack();
            match field {
                "source" => pack.source = "  ".into(),
                _ => pack.license = String::new(),
            }
            // Refused on write, so a pack that cannot be installed cannot be produced.
            let err = Qpack::write(&pack, &dir.join("bad.qpack"), &[])
                .expect_err("an unprovenanced pack must not be writable");
            assert!(
                matches!(&err, LoadError::PackMissingProvenance { pack, field: f }
                    if pack == "ashen-vault" && f == field),
                "{err:?}"
            );
            // And on read, for a pack this build did not write.
            let mut value = serde_json::to_value(sample_pack()).unwrap();
            value
                .as_object_mut()
                .unwrap()
                .insert(field.into(), serde_json::Value::String(String::new()));
            let err = PackManifest::from_json(&value.to_string()).expect_err("refused on read");
            assert!(err.to_string().contains(field), "{err}");
            assert!(err.to_string().contains("ashen-vault"), "{err}");
        }
        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn each_container_failure_is_its_own_typed_error() {
        let dir = tmp_dir("errors");

        // Not a zip at all.
        let not_zip = dir.join("plain.qpack");
        fs::write(&not_zip, b"this is not a zip").unwrap();
        assert!(matches!(
            Qpack::read_manifest(&not_zip),
            Err(LoadError::Container(_))
        ));

        // A readable zip with no manifest.
        let no_manifest = dir.join("empty.qpack");
        {
            let f = File::create(&no_manifest).unwrap();
            let mut zw = zip::ZipWriter::new(f);
            let opts: zip::write::FileOptions<'_, ()> = zip::write::FileOptions::default();
            zw.start_file("readme.txt", opts).unwrap();
            zw.write_all(b"hello").unwrap();
            zw.finish().unwrap();
        }
        assert!(matches!(
            Qpack::read_manifest(&no_manifest),
            Err(LoadError::NotAQpack(_))
        ));

        // Malformed manifest.
        assert!(matches!(
            PackManifest::from_json("{ not json"),
            Err(LoadError::PackParse(_))
        ));

        // Newer container format.
        let mut value = serde_json::to_value(sample_pack()).unwrap();
        value.as_object_mut().unwrap().insert(
            "pack_version".into(),
            serde_json::Value::from(PACK_VERSION + 1),
        );
        match PackManifest::from_json(&value.to_string()) {
            Err(LoadError::UnsupportedPackVersion { found, supported }) => {
                assert_eq!(found, PACK_VERSION + 1);
                assert_eq!(supported, PACK_VERSION);
            }
            other => panic!("expected a version refusal, got {other:?}"),
        }
        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn a_traversing_asset_path_is_refused_in_the_manifest() {
        for bad in ["../../etc/passwd", "/etc/passwd", "assets/../../out.png"] {
            let mut pack = sample_pack();
            pack.assets = vec![Asset {
                id: "x".into(),
                path: bad.into(),
                px_w: 1,
                px_h: 1,
                dpi: 300.0,
                line_art: false,
                has_alpha: false,
            }];
            let err = pack
                .validate()
                .expect_err("`{bad}` must not be an installable asset path");
            assert!(
                matches!(&err, LoadError::PackUnsafePath { path, .. } if path == bad),
                "{bad}: {err:?}"
            );
        }
    }

    #[test]
    fn a_traversing_zip_entry_is_refused_on_extraction() {
        let dir = tmp_dir("slip");
        let path = dir.join("evil.qpack");
        // Written by hand: `Qpack::write` refuses to produce one, which is the point.
        {
            let f = File::create(&path).unwrap();
            let mut zw = zip::ZipWriter::new(f);
            let opts: zip::write::FileOptions<'_, ()> = zip::write::FileOptions::default();
            zw.start_file(PACK_MANIFEST_NAME, opts).unwrap();
            zw.write_all(sample_pack().to_json().unwrap().as_bytes())
                .unwrap();
            zw.start_file("../escaped.txt", opts).unwrap();
            zw.write_all(b"pwned").unwrap();
            zw.finish().unwrap();
        }
        let out = dir.join("out");
        let err = Qpack::open_into(&path, &out).expect_err("zip-slip must be refused");
        assert!(matches!(err, LoadError::PackUnsafePath { .. }), "{err:?}");
        assert!(
            !dir.join("escaped.txt").exists(),
            "nothing may be written outside the extraction root"
        );
        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn a_malformed_component_definition_is_refused_at_the_container_boundary() {
        let mut pack = sample_pack();
        let mut broken = crate::statblock_definition();
        broken.sections.clear();
        pack.components.insert("broken".into(), broken);
        let err = pack
            .validate()
            .expect_err("a malformed definition is refused");
        assert!(matches!(err, LoadError::ComponentDef(_)), "{err:?}");
    }
}
