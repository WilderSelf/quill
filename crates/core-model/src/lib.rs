//! Core document model and the open `.tpub` file format for Quill.
//!
//! Holds the serializable document tree shared across the layout, render, and export crates.
//! See `docs/format-spec.md` and `specs/0001-pdf-x-export.md`.

use std::collections::{BTreeMap, BTreeSet};
use std::fmt;

use serde::{Deserialize, Serialize};

mod container;
mod version;

pub use container::{OpenedTpub, Tpub, MANIFEST_NAME};
pub use version::LoadError;

/// Typographic points (1/72 inch) — the internal unit throughout Quill.
pub type Pt = f32;

/// The current `.tpub` manifest format version.
pub const FORMAT_VERSION: u32 = 1;

/// 0.125 inch expressed in points — the DriveThruRPG-required bleed on outside edges.
pub const DEFAULT_BLEED_PT: Pt = 9.0;

/// A width/height in points.
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
pub struct Size {
    pub w_pt: Pt,
    pub h_pt: Pt,
}

/// An axis-aligned rectangle in points, origin top-left.
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
pub struct Rect {
    pub x_pt: Pt,
    pub y_pt: Pt,
    pub w_pt: Pt,
    pub h_pt: Pt,
}

/// A color value.
///
/// Press output must be `Cmyk` or `Gray`; `Rgb` is authoring-only and must be converted (see
/// `quill-color`) before it can appear in a PDF/X export.
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
#[serde(tag = "space", rename_all = "lowercase")]
pub enum Color {
    /// Each channel in `0.0..=1.0`.
    Cmyk { c: f32, m: f32, y: f32, k: f32 },
    /// Single channel in `0.0..=1.0` (0 = black, 1 = white).
    Gray { v: f32 },
    /// Authoring-only; not permitted in press output.
    Rgb { r: f32, g: f32, b: f32 },
}

/// Document-level metadata, written into both the manifest and the exported PDF.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct Metadata {
    pub title: String,
    #[serde(default)]
    pub authors: Vec<String>,
}

/// Trim size, bleed, and facing-page setup for the whole document.
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
pub struct PageSetup {
    pub trim: Size,
    pub bleed_pt: Pt,
    pub facing_pages: bool,
}

impl Default for PageSetup {
    fn default() -> Self {
        // A common 6x9in "digest" trim.
        Self {
            trim: Size {
                w_pt: 432.0,
                h_pt: 648.0,
            },
            bleed_pt: DEFAULT_BLEED_PT,
            facing_pages: true,
        }
    }
}

/// A linked asset (image, etc.). Assets are referenced, not inlined — see the format spec.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Asset {
    pub id: String,
    pub path: String,
    /// Pixel dimensions of the source image. Used by the layout engine to place the image at its
    /// true aspect ratio and physical size (`pt = px / dpi * 72`). See `specs/0009-image-sizing.md`.
    /// `0` means "unknown" — the layout engine falls back to a square, full-width placeholder.
    #[serde(default)]
    pub px_w: u32,
    /// See [`Asset::px_w`].
    #[serde(default)]
    pub px_h: u32,
    /// Native (source) resolution of the image, in dots per inch. Combined with `px_w`/`px_h`
    /// it determines the placed size (`pt = px / dpi * 72`; see spec 0009). Preflight's
    /// `ImageResolution` check gates on this value.
    pub dpi: f32,
    /// True for bilevel line art (600 dpi threshold instead of 300).
    #[serde(default)]
    pub line_art: bool,
    /// True if the linked image carries an alpha channel. PDF/X forbids live transparency, so
    /// export flattens it (alpha is dropped); preflight warns when this will happen.
    #[serde(default)]
    pub has_alpha: bool,
}

/// A stable identity for a content block, unique within a document and preserved across saves.
///
/// Blocks were previously addressable only by their index in [`Document::content`], which makes an
/// insert at index 0 renumber every block after it. Incremental layout (spec 0031) keys a
/// measurement cache on the block it measured, so it needs a name that survives editing — an index
/// is not one.
///
/// `u64` rather than a string: this is a cache key on the hot path, so `Copy` + `Eq` + `Hash` with
/// no allocation is what it wants. It serializes as a plain number, so the manifest stays readable
/// and diffable. (Note that nothing else in this crate derives `Eq`/`Hash` — every geometry field
/// is `f32`.)
#[derive(
    Debug, Clone, Copy, Default, PartialEq, Eq, Hash, PartialOrd, Ord, Serialize, Deserialize,
)]
#[serde(transparent)]
pub struct BlockId(pub u64);

impl BlockId {
    /// The id of a block that has not been given one yet — a block built in memory, or loaded from
    /// a manifest written before ids existed. [`Document::assign_missing_block_ids`] replaces these
    /// with real ids; no assigned block ever has this value.
    pub const UNASSIGNED: BlockId = BlockId(0);

    pub fn is_assigned(self) -> bool {
        self != BlockId::UNASSIGNED
    }
}

impl fmt::Display for BlockId {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}", self.0)
    }
}

/// A semantic content block — the "easy" authoring layer.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum Block {
    Heading {
        #[serde(default)]
        id: BlockId,
        level: u8,
        text: String,
        color: Color,
    },
    Body {
        #[serde(default)]
        id: BlockId,
        text: String,
        color: Color,
    },
    Image {
        #[serde(default)]
        id: BlockId,
        asset: String,
    },
}

impl Block {
    /// This block's stable identity. [`BlockId::UNASSIGNED`] until the document assigns one.
    pub fn id(&self) -> BlockId {
        match self {
            Block::Heading { id, .. } | Block::Body { id, .. } | Block::Image { id, .. } => *id,
        }
    }

    pub fn set_id(&mut self, new: BlockId) {
        match self {
            Block::Heading { id, .. } | Block::Body { id, .. } | Block::Image { id, .. } => {
                *id = new
            }
        }
    }

    /// A body paragraph with no id yet.
    pub fn body(text: impl Into<String>, color: Color) -> Block {
        Block::Body {
            id: BlockId::UNASSIGNED,
            text: text.into(),
            color,
        }
    }

    /// A heading with no id yet.
    pub fn heading(level: u8, text: impl Into<String>, color: Color) -> Block {
        Block::Heading {
            id: BlockId::UNASSIGNED,
            level,
            text: text.into(),
            color,
        }
    }

    /// An image placement referencing an [`Asset::id`], with no id yet.
    pub fn image(asset: impl Into<String>) -> Block {
        Block::Image {
            id: BlockId::UNASSIGNED,
            asset: asset.into(),
        }
    }
}

/// The whole document.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Document {
    pub format_version: u32,
    #[serde(default)]
    pub metadata: Metadata,
    pub page_setup: PageSetup,
    #[serde(default)]
    pub content: Vec<Block>,
    #[serde(default)]
    pub assets: Vec<Asset>,
    /// Whether all fonts referenced by the document can be embedded/subset for export.
    #[serde(default)]
    pub fonts_embeddable: bool,
    /// Monotonic edit counter, bumped by every mutation. Incremental layout (spec 0031) uses it to
    /// answer "is what I cached still current?" without diffing the whole tree.
    #[serde(default)]
    pub revision: u64,
    /// The next [`BlockId`] to hand out. Persisted so that ids are never *reused* after a reload —
    /// reusing the id of a deleted block would silently hand a stale cache entry to a new block.
    #[serde(default)]
    pub next_block_id: u64,
}

impl Document {
    /// A minimal, valid document used by tests, the CLI sample, and the M0 export spike.
    pub fn sample() -> Self {
        let mut doc = Self {
            format_version: FORMAT_VERSION,
            metadata: Metadata {
                title: "Sample Adventure".into(),
                authors: vec!["Anon".into()],
            },
            page_setup: PageSetup::default(),
            content: vec![
                Block::heading(1, "The Dungeon", Color::Gray { v: 0.0 }),
                Block::body(
                    // Long enough to wrap to >= 2 lines under the default 432 pt frame at
                    // BODY_FONT_SIZE_PT, so the first (interior) line is justified — the CI
                    // Ghostscript preflight then parses a positioned `TJ` operator (spec 0017 incr. 2),
                    // not just a ragged single-line `Tj`.
                    "A dank corridor stretches into darkness, its cold stone walls slick with \
                     creeping moss, while the slow steady drip of water echoes from the unseen \
                     depths somewhere far ahead in the gloom.",
                    Color::Cmyk {
                        c: 0.0,
                        m: 0.0,
                        y: 0.0,
                        k: 1.0,
                    },
                ),
            ],
            assets: vec![Asset {
                id: "map1".into(),
                path: "assets/map1.png".into(),
                px_w: 1500,
                px_h: 1200,
                dpi: 300.0,
                line_art: false,
                has_alpha: false,
            }],
            fonts_embeddable: true,
            revision: 0,
            next_block_id: 0,
        };
        // The sample is a *loaded* document as far as everything downstream is concerned, so it
        // carries real ids like one — otherwise every consumer would have to special-case it.
        doc.assign_missing_block_ids()
            .expect("sample blocks are constructed unassigned, so cannot collide");
        doc
    }

    /// Hand out a fresh [`BlockId`], never one already used by this document.
    pub fn new_block_id(&mut self) -> BlockId {
        // Ids start at 1 so that 0 can mean UNASSIGNED.
        self.next_block_id = self.next_block_id.max(1);
        let id = BlockId(self.next_block_id);
        self.next_block_id += 1;
        id
    }

    /// Record that the document changed.
    pub fn bump_revision(&mut self) {
        self.revision += 1;
    }

    /// Give every unidentified block an id, and verify that the identified ones are unique.
    ///
    /// Called on every load. Manifests written before ids existed have none, and blocks built in
    /// memory start [`BlockId::UNASSIGNED`]; both get ids here, in document order.
    ///
    /// A duplicate id is an error rather than something to repair. Two blocks claiming one identity
    /// means a cache lookup can return the wrong block's layout, and silently renumbering one of
    /// them would break whichever external reference was pointing at the block that got moved.
    pub fn assign_missing_block_ids(&mut self) -> Result<(), LoadError> {
        let mut seen: BTreeSet<u64> = BTreeSet::new();
        for block in &self.content {
            let id = block.id();
            if id.is_assigned() && !seen.insert(id.0) {
                return Err(LoadError::DuplicateBlockId(id.0));
            }
        }
        // Never hand out an id already in the document, even if `next_block_id` was absent from the
        // manifest or has fallen behind.
        let highest = seen.iter().next_back().copied().unwrap_or(0);
        self.next_block_id = self.next_block_id.max(highest + 1);

        let mut next = self.next_block_id;
        for block in &mut self.content {
            if !block.id().is_assigned() {
                block.set_id(BlockId(next));
                next += 1;
            }
        }
        self.next_block_id = next;
        Ok(())
    }

    /// Look up a block by identity.
    pub fn block(&self, id: BlockId) -> Option<&Block> {
        self.content.iter().find(|b| b.id() == id)
    }

    /// An id → [`Asset`] index.
    ///
    /// Layout resolves an image block's asset once per candidate frame, and did so with a linear
    /// scan of `assets` — quadratic in a document with many images, which is exactly what an
    /// art-heavy 500-page book is. Built once per layout pass and shared.
    pub fn asset_index(&self) -> BTreeMap<&str, &Asset> {
        self.assets.iter().map(|a| (a.id.as_str(), a)).collect()
    }

    /// Serialize the manifest to pretty JSON (the `document.json` inside a `.tpub`).
    pub fn to_json(&self) -> Result<String, serde_json::Error> {
        serde_json::to_string_pretty(self)
    }

    /// Parse a manifest from JSON, migrating it forward if it is older than [`FORMAT_VERSION`] and
    /// refusing it if it is newer.
    ///
    /// The version gate runs on the untyped JSON *before* deserialization, because an older
    /// manifest by definition does not fit the current `serde` types. See [`version::migrate`].
    pub fn from_json(s: &str) -> Result<Self, LoadError> {
        let mut value: serde_json::Value =
            serde_json::from_str(s).map_err(|e| LoadError::Parse(e.to_string()))?;
        version::migrate(&mut value)?;
        let mut doc: Document =
            serde_json::from_value(value).map_err(|e| LoadError::Parse(e.to_string()))?;
        doc.assign_missing_block_ids()?;
        Ok(doc)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn sample_round_trips_through_json() {
        let doc = Document::sample();
        let json = doc.to_json().expect("serialize");
        let back = Document::from_json(&json).expect("deserialize");
        assert_eq!(doc, back);
    }

    #[test]
    fn default_bleed_is_one_eighth_inch() {
        assert_eq!(DEFAULT_BLEED_PT, 9.0);
        assert_eq!(PageSetup::default().bleed_pt, DEFAULT_BLEED_PT);
    }

    // --- Block identity (spec 0026) -----------------------------------------------------------

    /// A manifest with no ids at all — the shape every document written before spec 0026 has.
    const UNIDENTIFIED: &str = r#"{
        "format_version": 1,
        "page_setup": {"trim": {"w_pt": 432.0, "h_pt": 648.0}, "bleed_pt": 9.0,
                       "facing_pages": true},
        "content": [
            {"kind": "heading", "level": 1, "text": "A", "color": {"space": "gray", "v": 0.0}},
            {"kind": "body", "text": "B", "color": {"space": "gray", "v": 0.0}},
            {"kind": "image", "asset": "x"}
        ]
    }"#;

    #[test]
    fn every_block_variant_reports_its_id() {
        let doc = Document::from_json(UNIDENTIFIED).expect("load");
        assert!(matches!(doc.content[0], Block::Heading { .. }));
        assert!(matches!(doc.content[1], Block::Body { .. }));
        assert!(matches!(doc.content[2], Block::Image { .. }));
        for block in &doc.content {
            assert!(block.id().is_assigned(), "{block:?} has no id");
        }
    }

    #[test]
    fn blocks_without_ids_get_them_in_document_order() {
        let doc = Document::from_json(UNIDENTIFIED).expect("load");
        let ids: Vec<u64> = doc.content.iter().map(|b| b.id().0).collect();
        assert_eq!(ids, vec![1, 2, 3]);
    }

    #[test]
    fn ids_are_stable_across_a_save_and_reload() {
        // The property incremental layout depends on: an id names the same block tomorrow.
        let first = Document::from_json(UNIDENTIFIED).expect("load");
        let reloaded = Document::from_json(&first.to_json().expect("save")).expect("reload");
        let before: Vec<u64> = first.content.iter().map(|b| b.id().0).collect();
        let after: Vec<u64> = reloaded.content.iter().map(|b| b.id().0).collect();
        assert_eq!(before, after);
    }

    #[test]
    fn ids_survive_an_insert_at_the_front() {
        // The reason ids exist at all: with index addressing, inserting at 0 renumbers everything.
        let mut doc = Document::from_json(UNIDENTIFIED).expect("load");
        let original: Vec<u64> = doc.content.iter().map(|b| b.id().0).collect();
        let new_id = doc.new_block_id();
        let mut inserted = Block::body("new first", Color::Gray { v: 0.0 });
        inserted.set_id(new_id);
        doc.content.insert(0, inserted);

        let after: Vec<u64> = doc.content.iter().skip(1).map(|b| b.id().0).collect();
        assert_eq!(after, original, "existing blocks must keep their ids");
        assert!(!original.contains(&new_id.0), "the new id must be fresh");
    }

    #[test]
    fn duplicate_ids_are_refused_rather_than_renumbered() {
        let json = r#"{
            "format_version": 1,
            "page_setup": {"trim": {"w_pt": 432.0, "h_pt": 648.0}, "bleed_pt": 9.0,
                           "facing_pages": true},
            "content": [
                {"kind": "body", "id": 7, "text": "A", "color": {"space": "gray", "v": 0.0}},
                {"kind": "body", "id": 7, "text": "B", "color": {"space": "gray", "v": 0.0}}
            ]
        }"#;
        assert!(matches!(
            Document::from_json(json),
            Err(LoadError::DuplicateBlockId(7))
        ));
    }

    #[test]
    fn assigned_ids_are_preserved_and_gaps_filled_around_them() {
        let json = r#"{
            "format_version": 1,
            "page_setup": {"trim": {"w_pt": 432.0, "h_pt": 648.0}, "bleed_pt": 9.0,
                           "facing_pages": true},
            "content": [
                {"kind": "body", "id": 42, "text": "A", "color": {"space": "gray", "v": 0.0}},
                {"kind": "body", "text": "B", "color": {"space": "gray", "v": 0.0}}
            ]
        }"#;
        let doc = Document::from_json(json).expect("load");
        assert_eq!(doc.content[0].id().0, 42, "an existing id must be kept");
        assert!(
            doc.content[1].id().0 > 42,
            "a fresh id must not collide with an existing one, got {}",
            doc.content[1].id().0
        );
    }

    #[test]
    fn a_thousand_new_ids_are_all_distinct_and_never_collide() {
        let mut doc = Document::sample();
        let existing: BTreeSet<u64> = doc.content.iter().map(|b| b.id().0).collect();
        let minted: BTreeSet<u64> = (0..1000).map(|_| doc.new_block_id().0).collect();
        assert_eq!(minted.len(), 1000, "ids must be distinct");
        assert!(minted.is_disjoint(&existing), "must not reuse a live id");
        assert!(!minted.contains(&0), "0 is reserved for UNASSIGNED");
    }

    #[test]
    fn ids_are_not_reused_after_a_reload() {
        // `next_block_id` is persisted precisely so a deleted block's id is never handed to a new
        // one — that would silently give the new block the old one's cached layout.
        let mut doc = Document::sample();
        let minted = doc.new_block_id();
        let mut reloaded = Document::from_json(&doc.to_json().expect("save")).expect("reload");
        assert!(
            reloaded.new_block_id().0 > minted.0,
            "reload must not rewind the allocator"
        );
    }

    #[test]
    fn revision_increases_and_survives_a_round_trip() {
        let mut doc = Document::sample();
        assert_eq!(doc.revision, 0);
        doc.bump_revision();
        doc.bump_revision();
        assert_eq!(doc.revision, 2);
        let back = Document::from_json(&doc.to_json().expect("save")).expect("reload");
        assert_eq!(back.revision, 2, "a load must preserve the stored revision");
    }

    #[test]
    fn asset_index_resolves_by_id() {
        let doc = Document::sample();
        let index = doc.asset_index();
        assert_eq!(index.len(), doc.assets.len());
        assert_eq!(
            index.get("map1").map(|a| a.path.as_str()),
            Some("assets/map1.png")
        );
        assert!(!index.contains_key("nope"));
    }

    #[test]
    fn a_manifest_predating_ids_still_loads() {
        // Backwards compatibility: `id`, `revision` and `next_block_id` are all `serde(default)`,
        // so no FORMAT_VERSION bump is needed for this increment.
        let doc = Document::from_json(UNIDENTIFIED).expect("a v1 manifest must still load");
        assert_eq!(doc.format_version, FORMAT_VERSION);
        assert_eq!(doc.revision, 0);
        assert_eq!(doc.content.len(), 3);
    }
}
