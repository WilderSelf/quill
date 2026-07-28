//! Core document model and the open `.tpub` file format for Quill.
//!
//! Holds the serializable document tree shared across the layout, render, and export crates.
//! See `docs/format-spec.md` and `specs/0001-pdf-x-export.md`.

use std::collections::{BTreeMap, BTreeSet};
use std::fmt;

/// Re-exported so consumers get the component types from the model they appear in, rather than
/// having to add a dependency on `quill-components-ttrpg` to name a field of `Block` (spec 0038).
pub use quill_components_ttrpg::{RandomTable, StatBlock, Table, TableEntry};
use serde::{Deserialize, Serialize};

mod container;
mod geom;
mod import;
mod template;
mod version;

pub use container::{OpenedTpub, Tpub, MANIFEST_NAME};
pub use geom::{page_geom, PageGeom};
pub use import::{import, Diagnostic, ImportError, Imported};
pub use template::{Template, BODY_MASTER, FOLIO_STYLE, OPENER_MASTER};
pub use version::LoadError;

/// Typographic points (1/72 inch) — the internal unit throughout Quill.
pub type Pt = f32;

/// The current `.tpub` manifest format version.
///
/// **2** since spec 0030 added master pages and margins. The bump is deliberate even though the new
/// fields are all `serde(default)` and a v1 manifest therefore loads unchanged: the point of a
/// version is to stop an *older* build from opening a document it would silently mis-lay-out. A
/// build that predates master pages would ignore `master_pages` entirely and produce a document
/// without its running heads, folios or column geometry — and could then save that back. Refusing
/// to open is the correct outcome; quietly dropping the layout is exactly the silent corruption
/// `CLAUDE.md` forbids.
pub const FORMAT_VERSION: u32 = 2;

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
    /// Page margins. `inside`/`outside` rather than left/right because a bound book's margins are
    /// relative to the *spine*: the inside margin is at the spine on both sides of a spread, so it
    /// falls on the left of a recto and the right of a verso. Left/right would force every layout
    /// rule to special-case parity.
    #[serde(default)]
    pub margins: Margins,
}

/// Page margins in points, expressed relative to the binding.
///
/// Defaults to zero on every edge — the pre-spec-0030 behavior, where the text frame was the whole
/// trim area. Zero margins let text run to the trim edge, which is not a sane *design* default, but
/// changing it would silently reflow every existing document; it is a template concern (M2), and
/// the roadmap records it as an explicit open question rather than an oversight.
#[derive(Debug, Clone, Copy, PartialEq, Default, Serialize, Deserialize)]
pub struct Margins {
    #[serde(default)]
    pub top_pt: Pt,
    #[serde(default)]
    pub bottom_pt: Pt,
    /// The margin at the spine.
    #[serde(default)]
    pub inside_pt: Pt,
    /// The margin at the fore-edge.
    #[serde(default)]
    pub outside_pt: Pt,
}

impl Margins {
    /// A uniform margin on all four edges.
    pub fn uniform(pt: Pt) -> Margins {
        Margins {
            top_pt: pt,
            bottom_pt: pt,
            inside_pt: pt,
            outside_pt: pt,
        }
    }

    /// Resolve `inside`/`outside` into left/right for a given page.
    ///
    /// On a facing-pages document, odd (zero-based even) pages are rectos with the spine on the
    /// left. With facing pages off, every page is treated as a recto — a single-sided document has
    /// no spread to mirror across.
    pub fn left_right(&self, page_index: usize, facing_pages: bool) -> (Pt, Pt) {
        let recto = !facing_pages || page_index.is_multiple_of(2);
        if recto {
            (self.inside_pt, self.outside_pt)
        } else {
            (self.outside_pt, self.inside_pt)
        }
    }
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
            margins: Margins::default(),
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
        /// Overrides the structural default (`h{level}`). `None` is the common case.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        style: Option<String>,
    },
    Body {
        #[serde(default)]
        id: BlockId,
        text: String,
        color: Color,
        /// Overrides the structural default (`body`). `None` is the common case.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        style: Option<String>,
    },
    /// A creature or NPC stat block — the TTRPG-native content object this product exists for
    /// (spec 0038).
    ///
    /// Carries the portable [`StatBlock`] verbatim rather than flattening it into fields, so the
    /// same value can be authored, exchanged and rolled on without a document in sight. The
    /// document adds only what placing it on a page needs: identity and ink.
    StatBlock {
        #[serde(default)]
        id: BlockId,
        stat: StatBlock,
        /// The ink every line in the block is set in. One colour rather than one per section: a
        /// stat block is a single typographic object, and per-line colour would multiply the
        /// preflight surface for no authoring gain.
        color: Color,
    },
    /// A generated table of contents (spec 0041).
    ///
    /// Carries no entries: they are *derived* from where the headings actually landed, which is not
    /// known until the document is laid out — and changes when it is. Storing them would mean
    /// storing something that is stale the moment anything is edited.
    Toc {
        #[serde(default)]
        id: BlockId,
        /// Heading shown above the entries. Empty for none.
        #[serde(default)]
        title: String,
        /// Deepest heading level listed. `2` lists h1 and h2 and omits h3.
        #[serde(default = "two")]
        max_level: u8,
        color: Color,
    },
    /// A table — an equipment list, an encounter table, a random table (spec 0039).
    Table {
        #[serde(default)]
        id: BlockId,
        table: Table,
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
            Block::Heading { id, .. }
            | Block::Body { id, .. }
            | Block::Image { id, .. }
            | Block::StatBlock { id, .. }
            | Block::Table { id, .. }
            | Block::Toc { id, .. } => *id,
        }
    }

    pub fn set_id(&mut self, new: BlockId) {
        match self {
            Block::Heading { id, .. }
            | Block::Body { id, .. }
            | Block::Image { id, .. }
            | Block::StatBlock { id, .. }
            | Block::Table { id, .. }
            | Block::Toc { id, .. } => *id = new,
        }
    }

    /// A body paragraph with no id yet, taking the default `body` style.
    pub fn body(text: impl Into<String>, color: Color) -> Block {
        Block::Body {
            id: BlockId::UNASSIGNED,
            text: text.into(),
            color,
            style: None,
        }
    }

    /// A heading with no id yet, taking the default `h{level}` style.
    pub fn heading(level: u8, text: impl Into<String>, color: Color) -> Block {
        Block::Heading {
            id: BlockId::UNASSIGNED,
            level,
            text: text.into(),
            color,
            style: None,
        }
    }

    /// Name an explicit paragraph style for this block. No-op on an image.
    pub fn with_style(mut self, name: impl Into<String>) -> Block {
        match &mut self {
            Block::Heading { style, .. } | Block::Body { style, .. } => *style = Some(name.into()),
            // Neither has one paragraph to style: an image has none, and a stat block is a
            // composite whose parts resolve `statblock-*` individually.
            Block::Image { .. }
            | Block::StatBlock { .. }
            | Block::Table { .. }
            | Block::Toc { .. } => {}
        }
        self
    }

    /// An image placement referencing an [`Asset::id`], with no id yet.
    pub fn image(asset: impl Into<String>) -> Block {
        Block::Image {
            id: BlockId::UNASSIGNED,
            asset: asset.into(),
        }
    }
}

/// How a paragraph's lines are set within their frame.
///
/// Mirrors `quill_text_layout::Alignment`, which is not serializable and lives downstream of this
/// crate. Keeping an authored spelling here means the *document* owns the intent and the layout
/// crate owns the algorithm.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "snake_case")]
pub enum TextAlign {
    /// Stretch inter-word space so every line but the last fills the measure.
    #[default]
    Justified,
    /// Ragged right: words sit at their natural advances.
    Left,
}

/// The typographic treatment of a paragraph.
///
/// Before this existed, size and leading were crate constants in `quill-text-layout`
/// (`BODY_FONT_SIZE_PT`, `BODY_LINE_HEIGHT_PT`) and every block in every document was set at body
/// size — headings included, which meant a heading was distinguishable from body text only by
/// being ragged-left.
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
pub struct ParagraphStyle {
    pub font_size_pt: Pt,
    /// Baseline-to-baseline distance. Kept separate from `font_size_pt` rather than derived from a
    /// multiplier, because press typography routinely wants them set independently.
    pub leading_pt: Pt,
    #[serde(default)]
    pub align: TextAlign,
    /// Vertical space reserved above the paragraph. This is what stops a heading from sitting flush
    /// against the paragraph before it.
    #[serde(default)]
    pub space_before_pt: Pt,
    #[serde(default)]
    pub space_after_pt: Pt,
}

impl Default for ParagraphStyle {
    fn default() -> Self {
        // The historical body treatment, preserved exactly: 10 pt on 12 pt, justified. Any document
        // that does not mention styles must lay out precisely as it did before they existed.
        Self {
            font_size_pt: 10.0,
            leading_pt: 12.0,
            align: TextAlign::Justified,
            space_before_pt: 0.0,
            space_after_pt: 0.0,
        }
    }
}

/// The style name applied to body paragraphs when a block names none.
pub const BODY_STYLE: &str = "body";

/// The stat block's name line (spec 0038).
pub const STATBLOCK_TITLE_STYLE: &str = "statblock-title";
/// A stat block's `Key  Value` attribute lines.
pub const STATBLOCK_ATTR_STYLE: &str = "statblock-attr";
/// A stat block's prose sections — overview, details, actions, reactions.
pub const STATBLOCK_BODY_STYLE: &str = "statblock-body";

/// A table's header row (spec 0039).
pub const TABLE_HEADER_STYLE: &str = "table-header";
/// A table's body cells.
pub const TABLE_CELL_STYLE: &str = "table-cell";

/// A generated table of contents' own heading (spec 0041).
pub const TOC_TITLE_STYLE: &str = "toc-title";

/// The style for a contents entry at heading level `level` (`toc-1`..`toc-6`).
pub fn toc_entry_style_name(level: u8) -> String {
    format!("toc-{}", level.clamp(1, 6))
}

/// The style name for a heading of the given level (`h1`..`h6`).
pub fn heading_style_name(level: u8) -> String {
    format!("h{}", level.clamp(1, 6))
}

/// Named paragraph styles for a document.
///
/// A named sheet rather than per-block formatting: changing "every heading in the book" has to be
/// one edit, not a sweep over 500 pages. Blocks name a style; the sheet holds the treatment.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct StyleSheet {
    #[serde(default)]
    pub paragraph: BTreeMap<String, ParagraphStyle>,
}

impl Default for StyleSheet {
    fn default() -> Self {
        let mut paragraph = BTreeMap::new();
        paragraph.insert(BODY_STYLE.to_string(), ParagraphStyle::default());
        // A conventional descending scale. Headings are ragged-left because justifying a one-line
        // heading would stretch it across the measure; and they carry space above so they separate
        // from the text they follow.
        for (level, size, leading) in [
            (1u8, 24.0, 28.0),
            (2, 18.0, 22.0),
            (3, 14.0, 18.0),
            (4, 12.0, 15.0),
            (5, 11.0, 14.0),
            (6, 10.0, 13.0),
        ] {
            paragraph.insert(
                heading_style_name(level),
                ParagraphStyle {
                    font_size_pt: size,
                    leading_pt: leading,
                    align: TextAlign::Left,
                    space_before_pt: leading * 0.75,
                    space_after_pt: leading * 0.25,
                },
            );
        }
        // Stat-block treatment (spec 0038). Built in rather than left to the author, because the
        // point of a first-class component is that dropping one in produces something that already
        // looks like a stat block. Restyling the whole book is still one edit — these three names.
        paragraph.insert(
            STATBLOCK_TITLE_STYLE.to_string(),
            ParagraphStyle {
                font_size_pt: 13.0,
                leading_pt: 16.0,
                align: TextAlign::Left,
                space_before_pt: 0.0,
                space_after_pt: 3.0,
            },
        );
        paragraph.insert(
            STATBLOCK_ATTR_STYLE.to_string(),
            ParagraphStyle {
                font_size_pt: 9.0,
                leading_pt: 11.0,
                align: TextAlign::Left,
                space_before_pt: 0.0,
                space_after_pt: 0.0,
            },
        );
        paragraph.insert(
            STATBLOCK_BODY_STYLE.to_string(),
            ParagraphStyle {
                font_size_pt: 9.0,
                leading_pt: 11.5,
                // Ragged, not justified: a stat block sits in a narrow panel where justification
                // opens rivers the surrounding body text would not show.
                align: TextAlign::Left,
                space_before_pt: 0.0,
                space_after_pt: 3.0,
            },
        );
        // Table treatment (spec 0039), on the same principle as the stat block's: a table dropped
        // into a document should already read as one.
        paragraph.insert(
            TABLE_HEADER_STYLE.to_string(),
            ParagraphStyle {
                font_size_pt: 9.0,
                leading_pt: 11.5,
                align: TextAlign::Left,
                space_before_pt: 0.0,
                space_after_pt: 0.0,
            },
        );
        paragraph.insert(
            TABLE_CELL_STYLE.to_string(),
            ParagraphStyle {
                font_size_pt: 9.0,
                leading_pt: 11.5,
                // Ragged: a table cell is a narrow measure, and justifying one opens rivers a
                // paragraph of the same text would not show.
                align: TextAlign::Left,
                space_before_pt: 0.0,
                space_after_pt: 0.0,
            },
        );
        // Contents treatment (spec 0041). Deeper levels are set smaller and indented, which is what
        // makes a contents list scannable without the author styling six levels by hand.
        paragraph.insert(
            TOC_TITLE_STYLE.to_string(),
            ParagraphStyle {
                font_size_pt: 18.0,
                leading_pt: 22.0,
                align: TextAlign::Left,
                space_before_pt: 0.0,
                space_after_pt: 11.0,
            },
        );
        for level in 1u8..=6 {
            let size = (12.0 - (level as f32 - 1.0) * 0.5).max(8.5);
            paragraph.insert(
                toc_entry_style_name(level),
                ParagraphStyle {
                    font_size_pt: size,
                    leading_pt: size + 4.0,
                    align: TextAlign::Left,
                    // Level 1 entries get air above them; deeper ones sit tight under their parent.
                    space_before_pt: if level == 1 { 5.0 } else { 0.0 },
                    space_after_pt: 0.0,
                },
            );
        }
        StyleSheet { paragraph }
    }
}

impl StyleSheet {
    /// The style that applies to `block`.
    ///
    /// Resolution is: the block's explicit style name, else its structural default (`body`, or
    /// `h{level}` for a heading). An unknown name falls back to `body` and finally to
    /// [`ParagraphStyle::default`] — a missing style must not lose the text. Losing a paragraph
    /// because its style was renamed would be far worse than setting it in the body face.
    pub fn resolve(&self, block: &Block) -> ParagraphStyle {
        let named = match block {
            Block::Heading { style, level, .. } => {
                style.clone().unwrap_or_else(|| heading_style_name(*level))
            }
            Block::Body { style, .. } => style.clone().unwrap_or_else(|| BODY_STYLE.to_string()),
            // A composite has no single paragraph treatment; its parts resolve `statblock-*`
            // themselves. `resolve` must still be total, so both fall back to the default.
            Block::Image { .. }
            | Block::StatBlock { .. }
            | Block::Table { .. }
            | Block::Toc { .. } => return ParagraphStyle::default(),
        };
        self.paragraph
            .get(&named)
            .or_else(|| self.paragraph.get(BODY_STYLE))
            .copied()
            .unwrap_or_default()
    }
}

/// A repeating element a master page stamps onto each page it governs.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum MasterStatic {
    /// A line of text at a fixed position — a running head, a folio, a footer.
    ///
    /// `text` may contain `{page}`, replaced with the one-based page number when the page is laid
    /// out. A token rather than a distinct "folio" variant, so a running head can read
    /// "The Dungeon — 42" without needing a second element type.
    Text {
        /// Where the line sits, relative to the trim box.
        rect: Rect,
        text: String,
        color: Color,
        /// Paragraph style name, resolved against the document's stylesheet.
        #[serde(default)]
        style: Option<String>,
    },
    /// A linked image at a fixed position — background art, a border, a decorative rule.
    Image { rect: Rect, asset: String },
}

/// The page-number token replaced in [`MasterStatic::Text`].
pub const PAGE_TOKEN: &str = "{page}";

/// A named page template: the geometry and repeating furniture shared by many pages.
///
/// This is the "pro layer" half of the hybrid paradigm — the thing that makes a 500-page book
/// consistent without touching 500 pages.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct MasterPage {
    pub name: String,
    /// Margins for pages using this master; falls back to the document's when absent.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub margins: Option<Margins>,
    /// Number of columns the text frame is divided into.
    #[serde(default = "one")]
    pub columns: usize,
    /// Horizontal space between columns.
    #[serde(default)]
    pub gutter_pt: Pt,
    /// Repeating elements stamped on every page using this master.
    #[serde(default)]
    pub statics: Vec<MasterStatic>,
}

fn one() -> usize {
    1
}

fn two() -> u8 {
    2
}

impl MasterPage {
    /// A single-column master with no furniture — the geometry a document has with no master at all.
    pub fn plain(name: impl Into<String>) -> MasterPage {
        MasterPage {
            name: name.into(),
            margins: None,
            columns: 1,
            gutter_pt: 0.0,
            statics: Vec::new(),
        }
    }
}

/// Per-page overrides of what the document otherwise decides (spec 0035).
///
/// Indexed positionally: `pages[i]` governs page `i`. Assignment by index means inserting content
/// that pushes the book by a page slides every subsequent assignment — accepted semantics for now;
/// anchoring a master to the chapter it opens needs a notion of "section" the model does not have,
/// and is recorded as an open question in `docs/roadmap.md`.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct PageOverride {
    /// The master this page uses, overriding [`Document::default_master`].
    ///
    /// `None` declines to override — which is deliberately *not* the same as "no master": an
    /// explicit entry that sets nothing still falls through to the document's default.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub master: Option<String>,
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
    /// Named paragraph styles (spec 0028). Defaulted, so a manifest that predates styles loads and
    /// lays out exactly as it did before they existed.
    #[serde(default)]
    pub styles: StyleSheet,
    /// Named master pages (spec 0030).
    #[serde(default)]
    pub master_pages: Vec<MasterPage>,
    /// The master applied to every page that does not override it. `None` — or a name not in
    /// `master_pages` — means the document's own page setup governs, which is the pre-0030
    /// behavior.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub default_master: Option<String>,
    /// Per-page overrides (spec 0035). An absent or empty list is exactly the pre-0035 behavior —
    /// every page on `default_master` — which is why this is additive and `FORMAT_VERSION` stays 2.
    ///
    /// The list need not cover the document: pages past its end fall back to `default_master`, and
    /// entries past the end of the document are ignored rather than being an error, because the
    /// content that justified them may simply have been deleted.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub pages: Vec<PageOverride>,
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
            styles: StyleSheet::default(),
            master_pages: Vec::new(),
            default_master: None,
            pages: Vec::new(),
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

    /// The master page governing `page_index` (spec 0035).
    ///
    /// Resolution is: the page's own override, else [`Document::default_master`], else none — and
    /// a name that matches no master falls through to the next step rather than failing. That
    /// fallback is deliberate and matches [`StyleSheet::resolve`]: a renamed master should cost the
    /// page its furniture, not cost the author the page. A missing running head is obvious the
    /// moment the page is looked at; a document that refuses to lay out is not recoverable by the
    /// person who typed the name.
    pub fn master_for(&self, page_index: usize) -> Option<&MasterPage> {
        let named = self
            .pages
            .get(page_index)
            .and_then(|p| p.master.as_deref())
            .and_then(|name| self.master_pages.iter().find(|m| m.name == name));
        named.or_else(|| {
            self.default_master
                .as_deref()
                .and_then(|name| self.master_pages.iter().find(|m| m.name == name))
        })
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

    // --- Paragraph styles (spec 0028) ---------------------------------------------------------

    #[test]
    fn the_default_body_style_preserves_the_historical_treatment() {
        // 10 pt on 12 pt justified were crate constants in quill-text-layout. A document that never
        // mentions styles must lay out exactly as it did before styles existed.
        let s = ParagraphStyle::default();
        assert_eq!(s.font_size_pt, 10.0);
        assert_eq!(s.leading_pt, 12.0);
        assert_eq!(s.align, TextAlign::Justified);
        assert_eq!(s.space_before_pt, 0.0);
        assert_eq!(s.space_after_pt, 0.0);
    }

    #[test]
    fn headings_resolve_to_their_level_style_and_are_larger_than_body() {
        let sheet = StyleSheet::default();
        let body = sheet.resolve(&Block::body("x", Color::Gray { v: 0.0 }));
        let mut previous = f32::MAX;
        for level in 1..=6u8 {
            let style = sheet.resolve(&Block::heading(level, "x", Color::Gray { v: 0.0 }));
            assert!(
                style.font_size_pt >= body.font_size_pt,
                "h{level} should not be smaller than body"
            );
            assert!(
                style.font_size_pt <= previous,
                "h{level} should not be larger than h{}",
                level - 1
            );
            assert_eq!(style.align, TextAlign::Left, "headings are ragged-left");
            assert!(
                style.space_before_pt > 0.0,
                "h{level} needs space above so it separates from the text it follows"
            );
            previous = style.font_size_pt;
        }
        assert!(
            sheet
                .resolve(&Block::heading(1, "x", Color::Gray { v: 0.0 }))
                .font_size_pt
                > body.font_size_pt,
            "h1 must be visibly larger than body — the whole point of this increment"
        );
    }

    #[test]
    fn a_block_can_name_an_explicit_style() {
        let mut sheet = StyleSheet::default();
        sheet.paragraph.insert(
            "sidebar".into(),
            ParagraphStyle {
                font_size_pt: 8.0,
                leading_pt: 9.5,
                align: TextAlign::Left,
                space_before_pt: 0.0,
                space_after_pt: 0.0,
            },
        );
        let block = Block::body("aside", Color::Gray { v: 0.0 }).with_style("sidebar");
        assert_eq!(sheet.resolve(&block).font_size_pt, 8.0);
    }

    #[test]
    fn an_unknown_style_name_falls_back_rather_than_losing_the_text() {
        // A renamed or deleted style must not make a paragraph vanish or panic — setting it in the
        // body face is recoverable, losing it is not.
        let sheet = StyleSheet::default();
        let block = Block::body("x", Color::Gray { v: 0.0 }).with_style("does-not-exist");
        assert_eq!(sheet.resolve(&block), ParagraphStyle::default());
    }

    #[test]
    fn a_heading_beyond_h6_still_resolves() {
        // `level` is a u8, so nothing stops a document declaring level 99.
        let sheet = StyleSheet::default();
        let style = sheet.resolve(&Block::heading(99, "x", Color::Gray { v: 0.0 }));
        assert_eq!(style, sheet.paragraph["h6"]);
    }

    #[test]
    fn styles_round_trip_through_json() {
        let mut doc = Document::sample();
        doc.styles.paragraph.insert(
            "callout".into(),
            ParagraphStyle {
                font_size_pt: 13.5,
                leading_pt: 16.0,
                align: TextAlign::Left,
                space_before_pt: 6.0,
                space_after_pt: 3.0,
            },
        );
        let back = Document::from_json(&doc.to_json().expect("save")).expect("load");
        assert_eq!(back.styles, doc.styles);
    }

    #[test]
    fn a_manifest_without_styles_gets_the_defaults() {
        // Backwards compatibility: `styles` is serde(default), so no FORMAT_VERSION bump.
        let doc = Document::from_json(UNIDENTIFIED).expect("load");
        assert_eq!(doc.styles, StyleSheet::default());
        assert_eq!(doc.styles.resolve(&doc.content[1]).font_size_pt, 10.0);
    }

    #[test]
    fn an_image_block_resolves_to_the_default_style() {
        // Images have no paragraph treatment, but `resolve` must be total.
        assert_eq!(
            StyleSheet::default().resolve(&Block::image("x")),
            ParagraphStyle::default()
        );
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

    // --- Per-page master assignment (spec 0035) -----------------------------------------------

    fn doc_with_two_masters() -> Document {
        let mut doc = Document::sample();
        doc.master_pages = vec![MasterPage::plain("opener"), MasterPage::plain("body")];
        doc.default_master = Some("body".into());
        doc
    }

    #[test]
    fn a_manifest_without_a_page_list_still_loads() {
        // The whole reason FORMAT_VERSION stays 2: `pages` is serde(default), so a manifest written
        // before spec 0035 loads unchanged and lays out exactly as it did.
        //
        // Asserted on both a v1 fixture (which reaches the current types through `migrate`) and a
        // literal v2 one (which does not), because the two take different paths into the struct and
        // only the second is the case spec 0035 actually claims.
        let migrated = Document::from_json(UNIDENTIFIED).expect("v1 load");
        assert!(migrated.pages.is_empty());
        assert!(migrated.master_for(0).is_none());

        let v2 = format!(
            r#"{{"format_version":{FORMAT_VERSION},
                "page_setup":{{"trim":{{"w_pt":432.0,"h_pt":648.0}},"bleed_pt":9.0,
                               "facing_pages":true}},
                "content":[],
                "master_pages":[{{"name":"body"}}],
                "default_master":"body"}}"#
        );
        let doc = Document::from_json(&v2).expect("a v2 manifest with no `pages` key must load");
        assert!(doc.pages.is_empty());
        assert_eq!(doc.master_for(0).map(|m| m.name.as_str()), Some("body"));
    }

    #[test]
    fn a_page_override_wins_over_the_default_master() {
        let mut doc = doc_with_two_masters();
        doc.pages = vec![PageOverride {
            master: Some("opener".into()),
        }];
        assert_eq!(doc.master_for(0).map(|m| m.name.as_str()), Some("opener"));
        assert_eq!(doc.master_for(1).map(|m| m.name.as_str()), Some("body"));
    }

    #[test]
    fn master_resolution_falls_through_every_step() {
        // An unknown name at either level degrades to the next rather than failing — the same
        // posture as `StyleSheet::resolve`. Losing the furniture beats losing the page.
        let mut doc = doc_with_two_masters();
        doc.pages = vec![PageOverride {
            master: Some("was-renamed".into()),
        }];
        assert_eq!(
            doc.master_for(0).map(|m| m.name.as_str()),
            Some("body"),
            "unknown page master ⇒ the document default"
        );

        doc.default_master = Some("also-renamed".into());
        assert!(
            doc.master_for(0).is_none(),
            "unknown default too ⇒ the document's own page setup"
        );
    }

    #[test]
    fn an_override_naming_no_master_is_not_the_same_as_no_master() {
        let mut doc = doc_with_two_masters();
        doc.pages = vec![PageOverride { master: None }];
        assert_eq!(
            doc.master_for(0).map(|m| m.name.as_str()),
            Some("body"),
            "an entry that declines to override still gets the default"
        );
    }

    #[test]
    fn a_page_list_shorter_or_longer_than_the_document_is_fine() {
        let mut doc = doc_with_two_masters();
        doc.pages = vec![PageOverride {
            master: Some("opener".into()),
        }];
        // Past the end of the list: fall back.
        assert_eq!(doc.master_for(99).map(|m| m.name.as_str()), Some("body"));
        // Past the end of the document: never consulted, and not an error to hold.
        doc.pages.extend((0..50).map(|_| PageOverride {
            master: Some("opener".into()),
        }));
        assert_eq!(doc.master_for(0).map(|m| m.name.as_str()), Some("opener"));
    }

    #[test]
    fn a_stat_block_round_trips_through_the_manifest() {
        let mut doc = Document::sample();
        doc.content.push(Block::StatBlock {
            id: BlockId::UNASSIGNED,
            stat: StatBlock {
                name: "Goblin".into(),
                overview: vec!["Small humanoid, chaotic".into()],
                attributes: vec![("AC".into(), "15".into()), ("HP".into(), "7".into())],
                details: vec!["Nimble Escape.".into()],
                actions: vec!["Scimitar. +4 to hit.".into()],
                reactions: vec![],
            },
            color: Color::Gray { v: 0.0 },
        });
        doc.assign_missing_block_ids().expect("ids");

        let back = Document::from_json(&doc.to_json().expect("save")).expect("load");
        assert_eq!(back, doc);
        // The component survives as a component, not as flattened text.
        let Block::StatBlock { stat, .. } = &back.content[2] else {
            panic!("expected a stat block")
        };
        assert_eq!(stat.attributes.len(), 2);
        assert_eq!(stat.name, "Goblin");
    }

    #[test]
    fn the_built_in_stat_block_styles_exist_and_are_ordered() {
        // A stat block resolves these three by name. If one were missing it would fall back to
        // `body` and the block would come out looking like a paragraph.
        let sheet = StyleSheet::default();
        for name in [
            STATBLOCK_TITLE_STYLE,
            STATBLOCK_ATTR_STYLE,
            STATBLOCK_BODY_STYLE,
        ] {
            assert!(sheet.paragraph.contains_key(name), "missing `{name}`");
        }
        assert!(
            sheet.paragraph[STATBLOCK_TITLE_STYLE].font_size_pt
                > sheet.paragraph[STATBLOCK_BODY_STYLE].font_size_pt,
            "the name must be set larger than the prose"
        );
    }

    #[test]
    fn an_empty_page_list_is_omitted_from_the_manifest() {
        // `skip_serializing_if` keeps the text manifest readable and git-diffable — the property
        // BlockId's plain-number encoding exists to protect.
        let json = Document::sample().to_json().expect("save");
        assert!(!json.contains("\"pages\""));
    }
}
