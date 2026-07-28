//! Installing, listing and resolving content packs (spec 0056).
//!
//! Spec 0055 can read and write a `.qpack`. This is what makes one *usable*: a place packs live, a
//! way a document names the ones it needs, and a resolution step that either produces a merged
//! library or refuses.
//!
//! ## Why a refusal rather than a fallback
//!
//! A document naming a pack that is not installed does not lay out. It does not fall back to the
//! bundled stat block and the default stylesheet, which would produce a book that looks *subtly*
//! wrong — right page count, right blocks, wrong face — and would be discovered at the print shop.
//! `CLAUDE.md`'s first rule, applied to somebody else's content: prefer a visible failure.

use std::collections::{BTreeMap, BTreeSet};
use std::fs;
use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

use crate::{ComponentLibrary, LoadError, PackManifest, StyleSheet, Template, PACK_MANIFEST_NAME};

/// A pack a document needs, by name and version.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct PackRequirement {
    pub name: String,
    /// An exact version, or a dotted prefix of one. Empty accepts any installed version. See
    /// [`version_matches`].
    #[serde(default)]
    pub version: String,
}

impl PackRequirement {
    pub fn new(name: impl Into<String>, version: impl Into<String>) -> Self {
        Self {
            name: name.into(),
            version: version.into(),
        }
    }
}

/// A pack found on disk.
#[derive(Debug, Clone, PartialEq)]
pub struct InstalledPack {
    pub manifest: PackManifest,
    /// The directory holding this pack's `pack.json` and `assets/`.
    pub root: PathBuf,
}

/// Whether an installed `version` satisfies a `requirement`.
///
/// The rule, in full, because a version-matching rule that is not written down is a rule nobody can
/// rely on:
///
/// - An empty requirement accepts anything.
/// - Otherwise both are split on `.` and the requirement's components must equal the installed
///   version's, component for component, as a **prefix**.
///
/// Comparing whole dotted components rather than characters is what stops `"1"` from matching
/// `10.0.0` — the substring reading is the classic version-matching bug, and it is closed by
/// construction rather than by a special case.
pub fn version_matches(requirement: &str, version: &str) -> bool {
    let requirement = requirement.trim();
    if requirement.is_empty() {
        return true;
    }
    let mut want = requirement.split('.');
    let mut have = version.split('.');
    loop {
        match (want.next(), have.next()) {
            (None, _) => return true,
            (Some(_), None) => return false,
            (Some(a), Some(b)) if a == b => {}
            (Some(_), Some(_)) => return false,
        }
    }
}

/// Order two version strings, component by component: numerically when both components parse as
/// integers, lexically otherwise.
///
/// Numeric where it can be, because `1.10.0` is newer than `1.9.0` and a purely lexical order says
/// the opposite — which would quietly resolve a requirement to a stale pack.
fn version_cmp(a: &str, b: &str) -> std::cmp::Ordering {
    use std::cmp::Ordering;
    let mut ap = a.split('.');
    let mut bp = b.split('.');
    loop {
        match (ap.next(), bp.next()) {
            (None, None) => return Ordering::Equal,
            (None, Some(_)) => return Ordering::Less,
            (Some(_), None) => return Ordering::Greater,
            (Some(x), Some(y)) => {
                let ord = match (x.parse::<u64>(), y.parse::<u64>()) {
                    (Ok(m), Ok(n)) => m.cmp(&n),
                    _ => x.cmp(y),
                };
                if ord != Ordering::Equal {
                    return ord;
                }
            }
        }
    }
}

/// The directory installed packs live in.
///
/// Resolved from an explicit path, then `$QUILL_PACKS`, then the platform data directory. Hand-
/// rolled against three environment variables rather than adding a platform-directories crate: it
/// is a dozen lines, and the dependency graph stays as small and as permissive as `CLAUDE.md` asks.
pub fn pack_root(explicit: Option<&Path>) -> PathBuf {
    if let Some(p) = explicit {
        return p.to_path_buf();
    }
    if let Some(p) = std::env::var_os("QUILL_PACKS") {
        return PathBuf::from(p);
    }
    #[cfg(windows)]
    {
        if let Some(appdata) = std::env::var_os("APPDATA") {
            return PathBuf::from(appdata).join("quill").join("packs");
        }
    }
    #[cfg(not(windows))]
    {
        if let Some(xdg) = std::env::var_os("XDG_DATA_HOME") {
            return PathBuf::from(xdg).join("quill").join("packs");
        }
        if let Some(home) = std::env::var_os("HOME") {
            return PathBuf::from(home)
                .join(".local")
                .join("share")
                .join("quill")
                .join("packs");
        }
    }
    PathBuf::from("quill-packs")
}

/// Where a given pack version is installed under `root`.
pub fn install_dir(root: &Path, name: &str, version: &str) -> PathBuf {
    root.join(name).join(version)
}

/// Every pack installed under `root`, in name-then-version order.
///
/// A directory walk rather than an index file: an index is a second source of truth that can
/// disagree with what is on disk, and the disagreement shows up as a pack that lists but does not
/// resolve. A directory that does not parse as a pack is **skipped**, not fatal — listing is a
/// read-only report, and one broken pack must not hide the other nine.
pub fn installed(root: &Path) -> Vec<InstalledPack> {
    let mut out: Vec<InstalledPack> = Vec::new();
    let Ok(names) = fs::read_dir(root) else {
        return out;
    };
    let mut name_dirs: Vec<PathBuf> = names.flatten().map(|e| e.path()).collect();
    name_dirs.sort();
    for name_dir in name_dirs {
        let Ok(versions) = fs::read_dir(&name_dir) else {
            continue;
        };
        let mut version_dirs: Vec<PathBuf> = versions.flatten().map(|e| e.path()).collect();
        version_dirs.sort();
        for dir in version_dirs {
            let manifest_path = dir.join(PACK_MANIFEST_NAME);
            let Ok(text) = fs::read_to_string(&manifest_path) else {
                continue;
            };
            let Ok(manifest) = PackManifest::from_json(&text) else {
                continue;
            };
            out.push(InstalledPack {
                manifest,
                root: dir,
            });
        }
    }
    out
}

/// Install an opened pack under `root`, returning where it landed.
///
/// Refuses to overwrite a *different* pack already at that path unless `force`: two packs claiming
/// the same name and version is a mistake somebody needs to see, and silently replacing one changes
/// how every document that requires it lays out.
pub fn install(
    manifest: &PackManifest,
    payload_root: &Path,
    root: &Path,
    force: bool,
) -> Result<PathBuf, LoadError> {
    manifest.validate()?;
    let dest = install_dir(root, &manifest.name, &manifest.version);
    let existing = dest.join(PACK_MANIFEST_NAME);
    if existing.exists() && !force {
        let same = fs::read_to_string(&existing)
            .ok()
            .and_then(|t| PackManifest::from_json(&t).ok())
            .is_some_and(|m| &m == manifest);
        if !same {
            return Err(LoadError::PackAlreadyInstalled {
                pack: manifest.name.clone(),
                version: manifest.version.clone(),
                path: dest.display().to_string(),
            });
        }
    }
    fs::create_dir_all(&dest)
        .map_err(|e| LoadError::Container(format!("creating '{}': {e}", dest.display())))?;
    let text = manifest
        .to_json()
        .map_err(|e| LoadError::PackParse(format!("serializing manifest: {e}")))?;
    fs::write(dest.join(PACK_MANIFEST_NAME), text)
        .map_err(|e| LoadError::Container(format!("writing '{}': {e}", dest.display())))?;

    // The payload the container extracted, copied in beside the manifest. Only the paths the
    // manifest actually declares: a pack's assets are what it says they are, not whatever happened
    // to be in the zip.
    for asset in &manifest.assets {
        let from = payload_root.join(&asset.path);
        if !from.exists() {
            continue;
        }
        let to = dest.join(&asset.path);
        if let Some(parent) = to.parent() {
            fs::create_dir_all(parent).map_err(|e| {
                LoadError::Container(format!("creating '{}': {e}", parent.display()))
            })?;
        }
        fs::copy(&from, &to)
            .map_err(|e| LoadError::Container(format!("copying '{}': {e}", from.display())))?;
    }
    Ok(dest)
}

/// The packs a document's requirements resolve to, and the library they merge into.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct ResolvedPacks {
    pub packs: Vec<InstalledPack>,
}

impl ResolvedPacks {
    /// Component definitions from every resolved pack, refusing a name two packs both define.
    ///
    /// Last-one-wins would make which pack won depend on `BTreeMap` iteration order — that is, on
    /// the packs' names — which is a coin flip a publisher cannot see, debug or predict.
    pub fn components(&self) -> Result<ComponentLibrary, LoadError> {
        let mut out = ComponentLibrary::new();
        let mut owner: BTreeMap<String, String> = BTreeMap::new();
        for pack in &self.packs {
            for (name, def) in &pack.manifest.components {
                if let Some(first) = owner.get(name) {
                    return Err(LoadError::PackConflict {
                        what: "component".into(),
                        item: name.clone(),
                        first: first.clone(),
                        second: pack.manifest.name.clone(),
                    });
                }
                owner.insert(name.clone(), pack.manifest.name.clone());
                out.insert(name.clone(), def.clone());
            }
        }
        Ok(out)
    }

    /// Templates from every resolved pack, refusing a name two packs both define — same rule, same
    /// reason.
    pub fn templates(&self) -> Result<Vec<Template>, LoadError> {
        let mut out: Vec<Template> = Vec::new();
        let mut owner: BTreeMap<String, String> = BTreeMap::new();
        for pack in &self.packs {
            for t in &pack.manifest.templates {
                if let Some(first) = owner.get(&t.name) {
                    return Err(LoadError::PackConflict {
                        what: "template".into(),
                        item: t.name.clone(),
                        first: first.clone(),
                        second: pack.manifest.name.clone(),
                    });
                }
                owner.insert(t.name.clone(), pack.manifest.name.clone());
                out.push(t.clone());
            }
        }
        Ok(out)
    }

    /// Paragraph styles from every resolved pack, merged in pack order.
    ///
    /// Deliberately **not** a conflict: a style name is a shared vocabulary (`body`, `h1`,
    /// `table-cell`) that packs are expected to co-define, and refusing there would make any two
    /// packs uninstallable together. A component name is the opposite — it is the pack's own
    /// contribution, and two packs claiming one is a real collision.
    pub fn styles(&self) -> StyleSheet {
        let mut out = StyleSheet::default();
        for pack in &self.packs {
            for (name, style) in &pack.manifest.styles.paragraph {
                out.paragraph.insert(name.clone(), *style);
            }
        }
        out
    }
}

/// Resolve `requirements` against the packs installed under `root`.
///
/// Every requirement must resolve, or the whole thing fails naming the one that did not, the
/// version asked for, and what is actually installed — a reader who has just been refused needs to
/// know what they *do* have.
pub fn resolve(requirements: &[PackRequirement], root: &Path) -> Result<ResolvedPacks, LoadError> {
    let available = installed(root);
    let mut packs: Vec<InstalledPack> = Vec::new();
    let mut seen: BTreeSet<String> = BTreeSet::new();

    for req in requirements {
        if !seen.insert(req.name.clone()) {
            return Err(LoadError::PackConflict {
                what: "requirement".into(),
                item: req.name.clone(),
                first: req.name.clone(),
                second: req.name.clone(),
            });
        }
        let mut candidates: Vec<&InstalledPack> = available
            .iter()
            .filter(|p| {
                p.manifest.name == req.name && version_matches(&req.version, &p.manifest.version)
            })
            .collect();
        candidates.sort_by(|a, b| version_cmp(&a.manifest.version, &b.manifest.version));
        match candidates.pop() {
            Some(p) => packs.push(p.clone()),
            None => {
                let have: Vec<String> = available
                    .iter()
                    .filter(|p| p.manifest.name == req.name)
                    .map(|p| p.manifest.version.clone())
                    .collect();
                return Err(LoadError::PackNotInstalled {
                    pack: req.name.clone(),
                    version: req.version.clone(),
                    installed: have,
                    root: root.display().to_string(),
                });
            }
        }
    }
    Ok(ResolvedPacks { packs })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{statblock_definition, Qpack, STATBLOCK_COMPONENT};

    fn tmp_dir(tag: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!("quill-resolve-{tag}-{}", std::process::id()));
        let _ = fs::remove_dir_all(&dir);
        fs::create_dir_all(&dir).expect("temp dir");
        dir
    }

    fn pack(name: &str, version: &str) -> PackManifest {
        PackManifest::new(
            name,
            format!("{name} pack"),
            version,
            "https://example.invalid",
            "CC-BY-4.0",
        )
    }

    fn install_into(root: &Path, manifest: &PackManifest) {
        install(manifest, Path::new("."), root, false).expect("install");
    }

    #[test]
    fn the_version_rule_is_a_dotted_prefix_and_never_a_substring() {
        assert!(version_matches("1.2.0", "1.2.0"));
        assert!(!version_matches("1.2.0", "1.2.1"));

        assert!(version_matches("1.2", "1.2.0"));
        assert!(version_matches("1.2", "1.2.9"));
        assert!(!version_matches("1.2", "1.3.0"));

        assert!(version_matches("1", "1.9.3"));
        // The classic bug: `1` must not match `10.0.0`.
        assert!(!version_matches("1", "10.0.0"));
        assert!(!version_matches("1", "2.0.0"));

        assert!(version_matches("", "anything"));
        assert!(version_matches("  ", "0.1"));

        // A requirement longer than the version is not a prefix of it.
        assert!(!version_matches("1.2.0", "1.2"));
    }

    #[test]
    fn versions_order_numerically_where_they_can() {
        use std::cmp::Ordering;
        assert_eq!(version_cmp("1.10.0", "1.9.0"), Ordering::Greater);
        assert_eq!(version_cmp("1.2.0", "1.2.0"), Ordering::Equal);
        assert_eq!(version_cmp("1.2", "1.2.0"), Ordering::Less);
        assert_eq!(version_cmp("2.0", "10.0"), Ordering::Less);
        // Non-numeric components fall back to lexical rather than panicking.
        assert_eq!(version_cmp("1.0-beta", "1.0-alpha"), Ordering::Greater);
    }

    #[test]
    fn a_requirement_resolves_to_the_highest_matching_version() {
        let root = tmp_dir("highest");
        for v in ["1.2.0", "1.9.0", "1.10.0", "2.0.0"] {
            install_into(&root, &pack("vault", v));
        }
        let got = resolve(&[PackRequirement::new("vault", "1")], &root).expect("resolve");
        assert_eq!(got.packs.len(), 1);
        assert_eq!(
            got.packs[0].manifest.version, "1.10.0",
            "1.10.0 is newer than 1.9.0; a lexical order would say otherwise"
        );

        let exact = resolve(&[PackRequirement::new("vault", "1.2.0")], &root).expect("resolve");
        assert_eq!(exact.packs[0].manifest.version, "1.2.0");

        let any = resolve(&[PackRequirement::new("vault", "")], &root).expect("resolve");
        assert_eq!(any.packs[0].manifest.version, "2.0.0");
        let _ = fs::remove_dir_all(&root);
    }

    #[test]
    fn a_missing_pack_is_a_refusal_that_says_what_is_installed() {
        let root = tmp_dir("missing");
        install_into(&root, &pack("vault", "1.2.0"));

        let err = resolve(&[PackRequirement::new("vault", "3")], &root)
            .expect_err("an unsatisfiable requirement must refuse");
        match &err {
            LoadError::PackNotInstalled {
                pack,
                version,
                installed,
                ..
            } => {
                assert_eq!(pack, "vault");
                assert_eq!(version, "3");
                assert_eq!(installed, &["1.2.0".to_string()]);
            }
            other => panic!("expected PackNotInstalled, got {other:?}"),
        }
        let text = err.to_string();
        assert!(
            text.contains("vault") && text.contains('3') && text.contains("1.2.0"),
            "{text}"
        );

        // And a pack that is not installed at all.
        let err = resolve(&[PackRequirement::new("nowhere", "1")], &root).expect_err("refuse");
        assert!(err.to_string().contains("nowhere"), "{err}");
        let _ = fs::remove_dir_all(&root);
    }

    #[test]
    fn two_packs_defining_the_same_component_is_an_error_naming_both() {
        let root = tmp_dir("conflict");
        for name in ["alpha", "beta"] {
            let mut m = pack(name, "1.0.0");
            m.components
                .insert(STATBLOCK_COMPONENT.into(), statblock_definition());
            install_into(&root, &m);
        }
        let resolved = resolve(
            &[
                PackRequirement::new("alpha", "1"),
                PackRequirement::new("beta", "1"),
            ],
            &root,
        )
        .expect("both resolve");
        let err = resolved
            .components()
            .expect_err("a component defined twice must refuse, not pick one");
        let text = err.to_string();
        assert!(text.contains("alpha"), "must name the first pack: {text}");
        assert!(text.contains("beta"), "must name the second pack: {text}");
        assert!(text.contains(STATBLOCK_COMPONENT), "must name it: {text}");
        let _ = fs::remove_dir_all(&root);
    }

    #[test]
    fn styles_merge_rather_than_conflict() {
        let root = tmp_dir("styles");
        for name in ["alpha", "beta"] {
            let mut m = pack(name, "1.0.0");
            m.styles = StyleSheet::default();
            install_into(&root, &m);
        }
        let resolved = resolve(
            &[
                PackRequirement::new("alpha", ""),
                PackRequirement::new("beta", ""),
            ],
            &root,
        )
        .expect("resolve");
        // A shared vocabulary: two packs both naming `body` must not make them uninstallable
        // together.
        assert!(resolved.styles().paragraph.contains_key(crate::BODY_STYLE));
        let _ = fs::remove_dir_all(&root);
    }

    #[test]
    fn installing_a_different_pack_at_the_same_name_and_version_is_refused() {
        let root = tmp_dir("collide");
        let a = pack("vault", "1.0.0");
        install_into(&root, &a);
        // Re-installing the identical pack is fine: it is idempotent, not a conflict.
        install_into(&root, &a);

        let mut b = pack("vault", "1.0.0");
        b.title = "A different pack entirely".into();
        let err = install(&b, Path::new("."), &root, false)
            .expect_err("two different packs at one name/version must be refused");
        assert!(
            matches!(err, LoadError::PackAlreadyInstalled { .. }),
            "{err:?}"
        );
        // …unless the caller says so.
        install(&b, Path::new("."), &root, true).expect("--force overwrites");
        let _ = fs::remove_dir_all(&root);
    }

    #[test]
    fn listing_skips_a_directory_that_is_not_a_pack() {
        let root = tmp_dir("listing");
        install_into(&root, &pack("good", "1.0.0"));
        fs::create_dir_all(root.join("junk").join("0.0.0")).unwrap();
        fs::write(
            root.join("junk").join("0.0.0").join(PACK_MANIFEST_NAME),
            "{",
        )
        .unwrap();
        fs::write(root.join("loose-file"), "not a directory").unwrap();

        let found = installed(&root);
        assert_eq!(found.len(), 1, "one broken pack must not hide the others");
        assert_eq!(found[0].manifest.name, "good");
        let _ = fs::remove_dir_all(&root);
    }

    #[test]
    fn a_pack_installs_from_a_container_and_round_trips_to_the_installed_form() {
        let dir = tmp_dir("container");
        let file = dir.join("vault.qpack");
        let mut m = pack("vault", "1.0.0");
        m.components
            .insert(STATBLOCK_COMPONENT.into(), statblock_definition());
        Qpack::write(&m, &file, &[]).expect("write");

        let opened = Qpack::open_into(&file, &dir.join("staging")).expect("open");
        let root = dir.join("root");
        let where_ = install(&opened.manifest, &opened.asset_root, &root, false).expect("install");
        assert_eq!(where_, install_dir(&root, "vault", "1.0.0"));

        let listed = installed(&root);
        assert_eq!(listed.len(), 1);
        assert_eq!(listed[0].manifest, m);
        let _ = fs::remove_dir_all(&dir);
    }
}
