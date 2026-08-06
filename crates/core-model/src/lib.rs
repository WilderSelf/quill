//! Core document model and the open `.tpub` file format for Quill.
//!
//! Holds the serializable document tree shared across the layout, render, and export crates.
//! See `docs/format-spec.md` and `specs/0001-pdf-x-export.md`.

use std::collections::{BTreeMap, BTreeSet};
use std::fmt;

/// Re-exported so consumers get the component types from the model they appear in, rather than
/// having to add a dependency on `quill-components` to name a field of `Block` (spec 0038).
pub use quill_components::{
    normalized_widths, ComponentDef, ComponentDefError, ComponentFields, ComponentLibrary,
    DefColor, DefStroke, FieldValue, Panel, PanelDef, RangeEntry, RangeTable, RuleDef, SectionDef,
    SectionShape, SplitDef, SplitGranularity, Table, ZebraDef, COMPONENT_DEF_VERSION,
};
use serde::{Deserialize, Serialize};

mod book;
mod collate;
mod components;
mod container;
mod geom;
mod import;
mod pack;
mod resolve;
mod template;
mod version;

pub use book::{
    chapter_prefix, Book, BookChapter, BookNote, ComposedBook, OpenedBook, BOOK_VERSION,
};
pub use collate::{collation_key, CollationKey};
pub use components::{
    builtin_components, statblock_definition, table_definition, PANEL_PADDING_PT,
    STATBLOCK_COMPONENT, TABLE_CELL_PADDING_PT, TABLE_COMPONENT,
};
pub use container::{OpenedTpub, Tpub, MANIFEST_NAME};
pub use geom::{page_geom, PageGeom};
pub use import::{import, Diagnostic, ImportError, Imported};
pub use pack::{OpenedPack, PackManifest, Qpack, PACK_MANIFEST_NAME, PACK_VERSION};
pub use resolve::{
    install, install_dir, installed, pack_root, resolve, version_matches, InstalledPack,
    PackRequirement, ResolvedPacks,
};
pub use template::{Template, BODY_MASTER, FOLIO_STYLE, OPENER_MASTER, TEMPLATE_VERSION};
pub use version::LoadError;

/// Typographic points (1/72 inch) — the internal unit throughout Quill.
pub type Pt = f32;

/// The current `.tpub` manifest format version.
///
/// **10** since spec 0080 gave a document [`PageBreak`]s — where the flow starts a new page, and
/// which side of the spread it must start on;
/// **9** since spec 0078 gave a [`Run`] an [`IndexMark`] and the model a [`Block::Index`] — the
/// terms a back-of-book index is built from;
/// **8** since spec 0077 gave a document [`Footnote`]s — an anchor in the text flow and note text
/// set in a band at the foot of the frame the anchor lands in;
/// **7** since spec 0076 gave a [`Run`] a [`RunSource`] — a cross-reference that prints the folio of
/// the page its target landed on; **6** since spec 0073 gave a [`Section`] a [`Folio`]; **5** since spec 0072 gave a document
/// [`Section`]s; **4** since spec 0063 replaced a paragraph's
/// `text` with styled `runs`; **3** since spec 0047 gave [`MasterStatic::Text`] an alignment and
/// page-parity mirroring; **2** since spec 0030 added master pages and margins. Each bump is
/// deliberate even though the new fields are all `serde(default)` and an older manifest therefore
/// loads unchanged: the point of a version is to stop an *older* build from opening a document it
/// would silently mis-lay-out. A build that predates master pages would ignore `master_pages`
/// entirely and produce a document without its running heads, folios or column geometry; a build
/// that predates spec 0047 would draw every static left-aligned in an unmirrored rect, putting the
/// folio in the gutter on every verso; a build that predates spec 0072 would drop `sections` and
/// set every chapter opener in the body master; a build that predates spec 0073 would drop `folio`
/// and print arabic page numbers through front matter the author set in roman; a build that predates
/// spec 0076 would drop every `source` and **destroy which block each cross-reference points at**,
/// unrecoverably, on the first save; and a build that predates spec 0077 would drop `footnotes` and
/// **destroy every note's prose** — not intent this time but the author's own words; and a build that
/// predates spec 0078 would drop every `index` mark and **destroy the whole index**, which is
/// authored intent in exactly [`RunSource`]'s sense: which terms an author chose to index, and what
/// each files under, cannot be recovered from anything left in the file; and a build that predates
/// spec 0080 would drop every `breaks` entry and **run every chapter of the book on**, setting a
/// chapter opener halfway down the page the previous chapter ended on — visually wrong everywhere
/// and announced nowhere. Any of them could
/// then save that back over the original.
/// Refusing to open is the correct outcome; quietly dropping the layout is exactly the silent
/// corruption `CLAUDE.md` forbids.
pub const FORMAT_VERSION: u32 = 10;

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
    /// The baseline grid this document's pages are set to (spec 0058), or `None` for none.
    ///
    /// Opt-in, and deliberately so. A grid is a design choice, not a correctness fix: a document
    /// that does not ask for one has no reason to move, and forcing every existing book, template
    /// and fixture through a re-derivation buys nothing a publisher wanted. The same posture
    /// [`Margins`] already takes, one field down.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub baseline_grid: Option<BaselineGrid>,
}

/// A ladder of baselines every block's first line snaps down to (spec 0058).
///
/// Measured from the **top of the trim box**, not from a frame. That is the whole point: two
/// columns of one page, and the two pages of a spread, share one ladder, so their baselines align.
/// A frame-relative grid would align each column to itself and to nothing else — which is a
/// document, not a book.
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
pub struct BaselineGrid {
    /// Distance between grid lines. Set it to the body leading.
    pub step_pt: Pt,
    /// The first grid line, measured down from the top of the trim box.
    #[serde(default)]
    pub origin_pt: Pt,
}

impl BaselineGrid {
    pub fn new(step_pt: Pt) -> Self {
        Self {
            step_pt,
            origin_pt: 0.0,
        }
    }

    /// Whether this grid is usable at all. A non-positive or non-finite step would divide by zero
    /// or loop forever, so it is treated as "no grid" rather than as an error — the authoring
    /// posture the rest of the model takes.
    pub fn is_usable(&self) -> bool {
        self.step_pt.is_finite() && self.step_pt > 0.0 && self.origin_pt.is_finite()
    }

    /// The first grid position at or **after** `y_pt`, in the same page-top-relative space.
    ///
    /// Snaps *down the page* (to a larger y), never up: moving a baseline up could pull it above
    /// the frame it was placed in, or on top of the block before it.
    pub fn snap_down(&self, y_pt: Pt) -> Pt {
        if !self.is_usable() {
            return y_pt;
        }
        let k = ((y_pt - self.origin_pt) / self.step_pt).ceil();
        // `ceil` of a value that is already on a line returns it unchanged, so a baseline already
        // on the grid does not jump a whole step. The epsilon guards the f32 case where it lands a
        // hair below a line and would otherwise be pushed a full step down.
        let snapped = self.origin_pt + k * self.step_pt;
        if snapped - y_pt < -0.001 {
            snapped + self.step_pt
        } else if (snapped - y_pt) > self.step_pt - 0.001 {
            snapped - self.step_pt
        } else {
            snapped
        }
    }

    /// Styles whose leading is **not** a whole multiple of the step, with their leading.
    ///
    /// A block's first baseline is snapped; its later lines follow at the style's leading, so they
    /// land on the grid exactly when that leading is a grid multiple. That is the real typographic
    /// contract of a baseline grid, and a house style is designed around it — so the honest answer is to
    /// name the styles that break it rather than to silently misalign them or to lay every line out
    /// against the grid, which is a different engine.
    pub fn off_grid_styles(&self, styles: &StyleSheet) -> Vec<(String, Pt)> {
        if !self.is_usable() {
            return Vec::new();
        }
        let mut out: Vec<(String, Pt)> = Vec::new();
        for (name, style) in &styles.paragraph {
            let k = style.leading_pt / self.step_pt;
            if (k - k.round()).abs() > 0.001 {
                out.push((name.clone(), style.leading_pt));
            }
        }
        out
    }
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
        if is_recto(page_index, facing_pages) {
            (self.inside_pt, self.outside_pt)
        } else {
            (self.outside_pt, self.inside_pt)
        }
    }
}

/// Whether `page_index` is a right-hand page — the spine on its left, the fore-edge on its right.
///
/// The one place page parity is decided. [`Margins::left_right`] and [`StaticAlign`] (spec 0047)
/// both call it rather than each re-deriving the rule, so the parity of a margin and the parity of a
/// folio cannot disagree.
///
/// With `facing_pages` off every page is a recto: a single-sided document has no spread to mirror
/// across.
pub fn is_recto(page_index: usize, facing_pages: bool) -> bool {
    !facing_pages || page_index.is_multiple_of(2)
}

impl Default for PageSetup {
    fn default() -> Self {
        // A common 6x9in "digest" trim.
        Self {
            baseline_grid: None,
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

/// A stretch of text set differently from its neighbours (spec 0063).
///
/// A paragraph is an ordered list of these. Before they existed a block carried one `String` and one
/// `Color`, so quill could not set a single word differently from the words beside it — and every
/// absent typographic feature that is a property of a *run* (character styles, drop caps, small
/// caps, tracking, baseline shift) was downstream of that.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct Run {
    pub text: String,
    /// What this run sets differently. Empty is the common case and the one that must cost nothing.
    #[serde(default, skip_serializing_if = "InlineStyle::is_empty")]
    pub style: InlineStyle,
    /// The named treatment this run is set in (spec 0065), resolved against the stylesheet's
    /// `character` map. Beside `style` rather than inside it, for the same reason `Block::style` is
    /// beside a block's fields: a name is not an override, it is what the overrides apply *to*.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub character: Option<String>,
    /// Where this run's *characters* come from (spec 0076). [`RunSource::Authored`] — the default,
    /// and every run written before this existed — means `text`.
    #[serde(default, skip_serializing_if = "RunSource::is_authored")]
    pub source: RunSource,
    /// The term this run puts in the index (spec 0078), or `None` — which is every run written
    /// before this existed.
    ///
    /// **Beside [`Run::source`] rather than a variant of it**, and that is a decision rather than a
    /// convenience. `RunSource`'s variants are mutually exclusive by construction: a run draws its
    /// own characters, or a reference's folio, or a footnote's number, and it cannot draw two of
    /// those at once. A mark is not that kind of thing. "…see the *bestiary* on page 42" is one run
    /// that is *both* a cross-reference and an index term, and a `RunSource::Index` variant would
    /// make the model unable to say so. It sits where [`Run::character`] sits, for
    /// [`Run::character`]'s stated reason: an annotation on a run is not a replacement for what the
    /// run is.
    ///
    /// It is also why a mark contributes to the font subset through its own path rather than
    /// through [`RunSource::contributes`] — see [`IndexMark::contributes`].
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub index: Option<IndexMark>,
}

impl Run {
    /// A run that sets nothing differently — the paragraph's own treatment.
    pub fn plain(text: impl Into<String>) -> Run {
        Run {
            text: text.into(),
            style: InlineStyle::EMPTY,
            character: None,
            source: RunSource::Authored,
            index: None,
        }
    }

    /// A cross-reference: a run that prints the **folio** of the page `target` was placed on
    /// (spec 0076) — "see page 42", surviving repagination.
    ///
    /// `text` is set to [`UNRESOLVED_REFERENCE`] and is never read by layout. That is deliberate on
    /// two counts. It is not a *cache* of the last resolved number, for spec 0041's reason: a stored
    /// page number is stale the moment anything is edited, and one that was right an edit ago is
    /// worse than none. And it is what a build that predates this spec — which drops `source` as an
    /// unknown key — prints, so such a build shows the same visible failure this one shows for a
    /// reference it cannot resolve, rather than a plausible wrong number.
    pub fn reference(target: BlockId) -> Run {
        Run {
            text: UNRESOLVED_REFERENCE.into(),
            style: InlineStyle::EMPTY,
            character: None,
            source: RunSource::Reference { target },
            index: None,
        }
    }

    /// A footnote anchor: a run that prints the **number** of the note `note` (spec 0077).
    ///
    /// `text` is [`UNRESOLVED_REFERENCE`] for [`Run::reference`]'s reasons, and one more that is
    /// specific to this variant: an anchor whose note is not in the document has no number, and a
    /// number is what the reader uses to find the note. Printing nothing would leave a sentence
    /// ending in a raised space.
    ///
    /// The anchor is **not** superscripted here. That is a character style
    /// ([`Run::character`]), and picking a raise and a size at this level would be picking them
    /// without knowing the paragraph's — see the non-goals in `specs/0077-footnotes.md`.
    pub fn footnote(note: BlockId) -> Run {
        Run {
            text: UNRESOLVED_REFERENCE.into(),
            style: InlineStyle::EMPTY,
            character: None,
            source: RunSource::Footnote { note },
            index: None,
        }
    }
}

/// A term this run puts in the index (spec 0078).
///
/// The mark is **run-level** because that is the granularity at which a page can be attributed: the
/// index says which page a term was discussed on, and a page is what a run lands on. It is the slot
/// spec 0065 established when it put a named character style beside a run's overrides — an
/// annotation on a stretch of text, orthogonal to how that stretch is set.
///
/// The **term is authored rather than taken from the run's text**, and both consequences matter.
/// An author can index a concept the sentence never names ("morale" on a paragraph about routing),
/// and can file an inverted heading ("Keep, the Ruined") against text that reads normally. It is
/// also what makes a mark's characters a *new* font-subset path: a term's letters need not appear
/// in the run's own text at all, and even when they do, the index draws them in the index's face
/// rather than the run's — see [`IndexMark::contributes`].
#[derive(Debug, Clone, Default, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct IndexMark {
    /// The term as it is printed in the index.
    pub term: String,
    /// What the term files under, when that is not the term itself.
    ///
    /// The escape hatch every index in publishing has, and the reason the collation rule does not
    /// have to be clairvoyant: `de Gaulle, Charles` files under `Gaulle`, `1984` under
    /// `Nineteen Eighty-Four`, `St Albans` under `Saint Albans`. No collator — not this one, not a
    /// full UCA implementation — can derive any of those, because they are facts about the language
    /// and about the author's intent rather than about the characters.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub sort_as: Option<String>,
}

impl IndexMark {
    /// A mark that files under its own printed term.
    pub fn new(term: impl Into<String>) -> IndexMark {
        IndexMark {
            term: term.into(),
            sort_as: None,
        }
    }

    /// Everything an index entry built from this mark can draw — what the font-subset collector
    /// must carry (spec 0078).
    ///
    /// A **run**, not loose characters: the term is authored text, knowable exactly before layout,
    /// so its ligatures are cut into the subset like any other run's (spec 0068). `sort_as`
    /// contributes nothing, because it is never printed.
    ///
    /// This is a *third* instance of the class spec 0074 closed and spec 0076 extended, and it is
    /// the one where the characters are drawn somewhere other than where they were authored. A
    /// cross-reference draws inside its own run, so it takes the run's face; an index term is drawn
    /// by the [`Block::Index`] block, in the index's own style, in the *regular* face — which is why
    /// the collector puts this contribution where a contents entry's goes rather than in the run's
    /// bucket.
    pub fn contributes(&self) -> TokenText {
        TokenText {
            runs: vec![self.term.clone()],
            chars: BTreeSet::new(),
        }
    }
}

/// What separates the two ends of a coalesced page range in an index entry (spec 0078).
///
/// An en dash, which is what a range is set with. A const rather than a literal at the two sites
/// that need it, so the string the coalescer *writes* and the string the font-subset collector
/// *embeds* cannot drift — the same "one answer" property spec 0076 required of a folio's alphabet.
pub const INDEX_RANGE_SEPARATOR: &str = "\u{2013}";

/// What separates one page reference from the next in an index entry (spec 0078). One const, both
/// sites, for [`INDEX_RANGE_SEPARATOR`]'s reason.
pub const INDEX_PAGE_SEPARATOR: &str = ", ";

/// What a term's pages print as: `iv, 42–45, 47` (spec 0078).
///
/// **This coalesces folios, not page indices**, and the distinction is the whole of the function.
/// Consecutive page *indices* need not be consecutive *folios*: a section boundary can restart the
/// count (`restart_at`) or change the format, so pages 9 and 10 of a file can print `ix` and `1`.
/// Joining those into `ix–1` would be a range that does not exist. Each element is therefore the
/// `(format, number)` behind the printed folio — spec 0073's numbering before it becomes text — and
/// two entries join a run only when they share a format **and** their numbers are consecutive.
///
/// A run of two or more consecutive folios prints as `first–last`. Two is deliberate rather than
/// three: `42–43` is what an index sets for two adjacent pages, and a rule with a threshold of
/// three would print `42, 43` beside `42–45` for no reason a reader could see.
///
/// Pure over numbers, with no engine coupling at all, which is what the M6 audit predicted for it —
/// and it is why this is unit-tested against a folio format change directly rather than only
/// through a laid-out document.
pub fn coalesce_folios(folios: &[(NumberFormat, u32)]) -> String {
    let mut out = String::new();
    let mut i = 0usize;
    while i < folios.len() {
        let (format, start) = folios[i];
        let mut end = i;
        while end + 1 < folios.len()
            && folios[end + 1].0 == format
            && folios[end + 1].1 == folios[end].1.saturating_add(1)
        {
            end += 1;
        }
        if !out.is_empty() {
            out.push_str(INDEX_PAGE_SEPARATOR);
        }
        out.push_str(&format.format(start));
        if end > i {
            out.push_str(INDEX_RANGE_SEPARATOR);
            out.push_str(&format.format(folios[end].1));
        }
        i = end + 1;
    }
    out
}

/// What a cross-reference prints when its target is not in the document (spec 0076).
///
/// **A visible failure, not an empty string.** A cross-reference is *content*, in the text flow, so
/// the two silent options are both worse than this one: rendering nothing leaves "see page ." in a
/// finished-looking sentence, and refusing to open the document loses a whole book to one dangling
/// id — the outcome spec 0072 rejected for a section anchor, and for the same reason. `CLAUDE.md`'s
/// rule is to prefer a visible failure over silent press-corruption, and this is what visible looks
/// like in running text: it survives to the proof, where it reads as unfinished rather than as
/// finished.
///
/// It is also why [`RunSource::contributes`] carries these characters into the font subset. A marker
/// that renders as three `.notdef` boxes is a *less* legible failure than the one it replaces.
pub const UNRESOLVED_REFERENCE: &str = "[?]";

/// Every block a cross-reference in `content` points at (spec 0076).
///
/// A `&[Block]` rather than a `&Document` because the layout fixpoint holds content and a template,
/// not a document — the same reason [`StaticToken`]'s siblings in `quill_layout_engine` come in
/// both shapes. Empty for every document that has no cross-reference, which is what lets the
/// derivation, the extra fixpoint comparison and the whole cost of this feature be skipped outright.
pub fn reference_targets(content: &[Block]) -> BTreeSet<BlockId> {
    content
        .iter()
        .flat_map(|b| b.runs())
        .filter_map(|r| r.source.target())
        .collect()
}

/// Where a [`Run`]'s characters come from (spec 0076).
///
/// **This enum is to a body run what [`StaticToken`] is to a master static, and for the same
/// reason.** A run whose text is generated at layout time is a second instance of the class spec
/// 0074 closed: the font-subset collector runs *before* layout, so every character such a run can
/// draw has to be predicted, and a character it misses is a `.notdef` box in a press file with no
/// error anywhere. Spec 0074 closed that for furniture with one enum and two exhaustive matches; a
/// cross-reference is a **new path** — it is content, in the flow, resolved per instance rather than
/// per page — so it owes the same structural treatment rather than a special case:
///
/// 1. [`RunSource::contributes`] is an exhaustive `match`, and it lives here in `core-model` beside
///    the enum, so a new variant cannot exist anywhere in the workspace without saying what
///    characters it can draw.
/// 2. The resolver — `quill_layout_engine::resolve_run_texts` — matches exhaustively too, so a new
///    variant does not compile there either.
/// 3. There is one answer to "what can a folio draw": [`Document::folio_formats`], which
///    [`StaticToken::Page`] already asks. A cross-reference does not get a second one.
///
/// The currency is [`TokenText`] for the same reason: the collector has one way to absorb a
/// contribution, and two producers that each have to be exhaustive in order to compile.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum RunSource {
    /// The run draws its own [`Run::text`]. Every run written before spec 0076.
    #[default]
    Authored,
    /// The run draws the folio of the page `target` was placed on — "see page 42".
    ///
    /// The **folio** (spec 0073), not the page index: this is a number a *reader* is told, and a
    /// reference that says `4` for a page printed `iv` sends them to the wrong page. That is the
    /// same split 0073 drew between a contents entry and a `/Link` destination.
    ///
    /// The target is any [`BlockId`], not just a heading's. Nothing about the mechanism is
    /// heading-specific — `section_starts` already anchors to any placed variant that carries an
    /// identity — and "see the table on page 42" is the same sentence as "see the chapter on page
    /// 42". Restricting it to headings would be a structure-shaped mechanism where a general one
    /// costs nothing, which is the test `CLAUDE.md` applies to every new type and field.
    Reference { target: BlockId },
    /// The run draws the **number** of footnote `note` (spec 0077).
    ///
    /// Two runs in the workspace carry this variant, and deliberately the same one: the *anchor*
    /// in the text flow, and the number that opens the *note* itself at the foot of the frame —
    /// see [`Document::footnote_blocks`]. One variant means one resolver, one contribution to the
    /// font subset, and no way for the two to print different numbers.
    ///
    /// The number is **document-sequential**, derived from the order the anchors appear in
    /// `content`, which makes it exactly the shape of a list marker (spec 0066): known before any
    /// page exists, and context rather than content. Per-page restart is a named non-goal; see the
    /// spec.
    Footnote { note: BlockId },
}

impl RunSource {
    /// Whether this run draws its own text — the default, and what `skip_serializing_if` asks.
    pub fn is_authored(&self) -> bool {
        matches!(self, RunSource::Authored)
    }

    /// The block this run refers to, if any.
    pub fn target(&self) -> Option<BlockId> {
        match self {
            RunSource::Authored | RunSource::Footnote { .. } => None,
            RunSource::Reference { target } => Some(*target),
        }
    }

    /// The footnote this run numbers, if any.
    pub fn note(&self) -> Option<BlockId> {
        match self {
            RunSource::Authored | RunSource::Reference { .. } => None,
            RunSource::Footnote { note } => Some(*note),
        }
    }

    /// The thing this run's drawn characters are derived from — a reference's target, a footnote's
    /// note — or `None` for an authored run.
    ///
    /// One accessor rather than two call sites asking `target().or(note())`, because the resolved
    /// map is keyed by *whatever the run points at*: a cross-reference's target block and a
    /// footnote both have a [`BlockId`], and merging them into one map is what lets one fingerprint
    /// and one resolver serve both.
    pub fn referent(&self) -> Option<BlockId> {
        match self {
            RunSource::Authored => None,
            RunSource::Reference { target } => Some(*target),
            RunSource::Footnote { note } => Some(*note),
        }
    }

    /// Everything this run can draw in `doc` — what the font-subset collector must carry.
    ///
    /// Exhaustive by construction; see the type's documentation for why that is the point of it.
    /// Deliberately not conditioned on where anything landed, for [`StaticToken::contributes`]'s
    /// reason: embedding a few characters nobody draws costs a few hundred bytes, and failing to
    /// embed one that is drawn is a `.notdef` box in a press file.
    pub fn contributes(&self, doc: &Document) -> TokenText {
        match self {
            RunSource::Authored => TokenText::default(),
            // A folio is arithmetic, so what is knowable before layout is the alphabet each
            // configured format can write — the same question, asked of the same function, that
            // `StaticToken::Page` asks. The unresolved marker *is* knowable exactly, so it travels
            // as a run and its characters are cut into the subset like any other run's.
            RunSource::Reference { .. } => TokenText {
                runs: vec![UNRESOLVED_REFERENCE.to_string()],
                chars: doc
                    .folio_formats()
                    .into_iter()
                    .flat_map(|f| f.alphabet())
                    .collect(),
            },
            // A footnote number is arithmetic too, so again only the alphabet is knowable — and
            // there is exactly one answer to what a footnote number can draw, because there is
            // exactly one numbering: document-sequential decimal (see [`footnote_number`]). It is
            // deliberately *not* asked of `folio_formats`: a folio and a note number are two
            // different quantities that happen to be numbers, and tying the note's alphabet to the
            // book's page numbering would make a roman front matter change what a footnote can
            // print. The unresolved marker travels as a run for the reason above.
            RunSource::Footnote { .. } => TokenText {
                runs: vec![UNRESOLVED_REFERENCE.to_string()],
                chars: NumberFormat::Decimal.alphabet(),
            },
        }
    }
}

/// What footnote `n` prints — the **one** answer, asked by the resolver and by the font-subset
/// collector alike (spec 0077).
///
/// Decimal and document-sequential. A function rather than an inline `to_string` so that
/// [`RunSource::contributes`]' alphabet and the resolver cannot drift: whatever this can produce is
/// what [`NumberFormat::Decimal`] can write.
pub fn footnote_number(n: u32) -> String {
    NumberFormat::Decimal.format(n)
}

/// What separates a note's number from its text at the foot of the frame (spec 0077).
///
/// Authored characters rather than generated ones — knowable exactly — so it travels as a *run* of
/// the synthesised note block and its ligatures are cut into the subset like any other run's. It is
/// **not** part of what [`RunSource::Footnote`] resolves to, because the anchor in the flow prints
/// the number alone.
pub const FOOTNOTE_NUMBER_SUFFIX: &str = ". ";

/// What each note prints, keyed by note id (spec 0077).
///
/// Takes the *synthesised note blocks* — [`Document::footnote_blocks`]' output — rather than the
/// content, so that there is exactly **one** statement of the numbering rule in the workspace and
/// one statement of which notes exist. `footnote_blocks` decides both (anchor order, skipping a note
/// the document does not hold); this only counts.
///
/// Derived from content order and known before anything is laid out, which is the numbering
/// decision — see [`Document::footnote_numbers`].
pub fn footnote_numbers(notes: &[Block]) -> BTreeMap<BlockId, String> {
    notes
        .iter()
        .enumerate()
        .map(|(i, b)| (b.id(), footnote_number(i as u32 + 1)))
        .collect()
}

/// A note set at the foot of the frame its anchor lands in (spec 0077).
///
/// **It is not in `content`**, and that is the whole of the model change: a footnote is a *second
/// flow*, entered from a [`RunSource::Footnote`] run in the first. Putting the note text in
/// `content` would set it inline where the anchor is, which is the thing a footnote exists not to
/// do.
///
/// It carries runs rather than a `String` for [`Block::Body`]'s reason — a note may emphasise a
/// word, and may itself carry a cross-reference. It carries an id because the anchor has to be able
/// to name it and because the placed note reports that id as its `source`, which is what makes the
/// "anchor and note on the same page" property assertable from a page vector.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Footnote {
    #[serde(default)]
    pub id: BlockId,
    /// The note's text, as one or more styled runs.
    pub runs: Vec<Run>,
    /// The note's ink. A run may override it.
    pub color: Color,
    /// Overrides the structural default ([`FOOTNOTE_STYLE`]). `None` is the common case.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub style: Option<String>,
}

impl Footnote {
    /// A note of plain text in the document's ink, with no id yet.
    pub fn plain(text: impl Into<String>, color: Color) -> Footnote {
        Footnote {
            id: BlockId::UNASSIGNED,
            runs: vec![Run::plain(text)],
            color,
            style: None,
        }
    }
}

/// How heavy a face is, on the `OS/2` `usWeightClass` scale the faces themselves are labelled on
/// (spec 0064).
///
/// A number rather than a `Bold` flag, and deliberately: the general mechanism is the one a family
/// with a Light and a Semibold can also use, and a boolean would have to be replaced rather than
/// extended the first time one appeared. The bundled family answers 400 and 700; asking for 300 gets
/// the nearest it has, and is told so.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[serde(transparent)]
pub struct Weight(pub u16);

impl Weight {
    pub const REGULAR: Weight = Weight(400);
    pub const BOLD: Weight = Weight(700);

    pub fn is_regular(&self) -> bool {
        *self == Weight::REGULAR
    }
}

impl Default for Weight {
    fn default() -> Weight {
        Weight::REGULAR
    }
}

/// A run's overrides of the paragraph treatment it sits in.
///
/// Every field is an `Option` *override* rather than a value, so "absent" and "the same as the
/// paragraph's" stay distinguishable: editing a paragraph style must move a run that did not opt
/// out, and must not move one that did.
///
#[derive(Debug, Clone, Copy, Default, PartialEq, Serialize, Deserialize)]
pub struct InlineStyle {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub size_pt: Option<Pt>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub color: Option<Color>,
    /// Extra letter-spacing, in points per character. Negative tightens.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub tracking_pt: Option<Pt>,
    /// Vertical offset from the baseline. Positive raises — a superscript.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub baseline_shift_pt: Option<Pt>,
    /// Which weight of the family to set this run in (spec 0064).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub weight: Option<Weight>,
    /// Whether to set this run in the family's italic (spec 0064).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub italic: Option<bool>,
}

impl InlineStyle {
    pub const EMPTY: InlineStyle = InlineStyle {
        size_pt: None,
        color: None,
        tracking_pt: None,
        baseline_shift_pt: None,
        weight: None,
        italic: None,
    };

    pub fn is_empty(&self) -> bool {
        *self == InlineStyle::EMPTY
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
        /// The heading's text, as one or more styled runs (spec 0063). A `FORMAT_VERSION` 3
        /// document's single `text` string migrates to one plain run.
        runs: Vec<Run>,
        /// The paragraph's ink. A run may override it.
        color: Color,
        /// Overrides the structural default (`h{level}`). `None` is the common case.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        style: Option<String>,
    },
    Body {
        #[serde(default)]
        id: BlockId,
        /// The paragraph's text, as one or more styled runs (spec 0063).
        runs: Vec<Run>,
        /// The paragraph's ink. A run may override it.
        color: Color,
        /// Overrides the structural default (`body`). `None` is the common case.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        style: Option<String>,
    },
    /// A titled, bordered record of named sections — a creature, a recipe, a specimen, a part
    /// number (specs 0038, 0062).
    ///
    /// Carries the portable [`Panel`] verbatim rather than flattening it into fields, so the same
    /// value can be authored and exchanged without a document in sight. The document adds only what
    /// placing it on a page needs: identity and ink.
    ///
    /// The wire form is deliberately still `"kind": "stat_block"` with a `"stat"` field: renaming
    /// it would be a `FORMAT_VERSION` migration to a name with a known expiry date, since this
    /// variant is a bundled specialization of [`Block::Component`] and retires when it does. Spec
    /// 0062 records the reasoning; a test pins the wire form so it stays a decision rather than a
    /// memory.
    #[serde(rename = "stat_block")]
    Panel {
        #[serde(default)]
        id: BlockId,
        #[serde(rename = "stat")]
        panel: Panel,
        /// The ink every line in the block is set in. One colour rather than one per section: a
        /// panel is a single typographic object, and per-line colour would multiply the preflight
        /// surface for no authoring gain.
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
    /// A generated back-of-book index (spec 0078).
    ///
    /// Carries no entries, for [`Block::Toc`]'s reason exactly and with the same force: they are
    /// *derived* from the pages the marked runs actually landed on, which is not known until the
    /// document is laid out and changes when it is. A stored entry is a page number that was right
    /// one edit ago, which is worse than none.
    ///
    /// What it does carry is the one thing that cannot be derived: the articles this book's language
    /// ignores at the front of a term.
    Index {
        #[serde(default)]
        id: BlockId,
        /// Heading shown above the entries. Empty for none.
        #[serde(default)]
        title: String,
        /// Words dropped from the front of a term before it is filed (spec 0078).
        ///
        /// **Empty by default, and that is the decision.** An English book states
        /// `["A", "An", "The"]` and a French one states `["Le", "La", "Les", "L'"]`; quill states
        /// nothing, because a built-in English list would make the collation a mechanism only an
        /// English book can use — the shape `CLAUDE.md` calls a defect — and because an article list
        /// applied to the wrong language files entries where nobody will look for them.
        ///
        /// A word here is only an article when the term continues with a space or an apostrophe, so
        /// `Theatre` is not `The` + `atre`, and the longest match wins so `["A", "An"]` cannot file
        /// `An Atlas` under `n`.
        #[serde(default, skip_serializing_if = "Vec::is_empty")]
        ignore_leading: Vec<String>,
        color: Color,
    },
    /// A table — an equipment list, an encounter table, a random table (spec 0039).
    Table {
        #[serde(default)]
        id: BlockId,
        table: Table,
        color: Color,
    },
    /// An instance of a declared component (spec 0054) — the general case
    /// [`Block::Panel`] and [`Block::Table`] are the two bundled specializations of.
    ///
    /// Additive: a manifest that carries none loads and lays out exactly as before, which is why
    /// `FORMAT_VERSION` stays 3.
    Component {
        #[serde(default)]
        id: BlockId,
        /// The definition this instance is set by, resolved against the document's component
        /// library. A name that does not resolve is skipped, exactly as an unresolved image asset
        /// is — the refusal for a *malformed* definition happens at load, where there is an error
        /// channel.
        def: String,
        /// The authored content, field by field.
        #[serde(default)]
        fields: ComponentFields,
        /// The ink every run in the component is set in. One colour rather than one per section,
        /// on [`Block::Panel`]'s reasoning.
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
            | Block::Panel { id, .. }
            | Block::Table { id, .. }
            | Block::Component { id, .. }
            | Block::Toc { id, .. }
            | Block::Index { id, .. } => *id,
        }
    }

    pub fn set_id(&mut self, new: BlockId) {
        match self {
            Block::Heading { id, .. }
            | Block::Body { id, .. }
            | Block::Image { id, .. }
            | Block::Panel { id, .. }
            | Block::Table { id, .. }
            | Block::Component { id, .. }
            | Block::Toc { id, .. }
            | Block::Index { id, .. } => *id = new,
        }
    }

    /// A body paragraph with no id yet, taking the default `body` style.
    pub fn body(text: impl Into<String>, color: Color) -> Block {
        Block::Body {
            id: BlockId::UNASSIGNED,
            runs: vec![Run::plain(text)],
            color,
            style: None,
        }
    }

    /// A body paragraph of already-built runs.
    pub fn body_runs(runs: Vec<Run>, color: Color) -> Block {
        Block::Body {
            id: BlockId::UNASSIGNED,
            runs,
            color,
            style: None,
        }
    }

    /// A heading of already-built runs.
    pub fn heading_runs(level: u8, runs: Vec<Run>, color: Color) -> Block {
        Block::Heading {
            id: BlockId::UNASSIGNED,
            level,
            runs,
            color,
            style: None,
        }
    }

    /// A heading with no id yet, taking the default `h{level}` style.
    pub fn heading(level: u8, text: impl Into<String>, color: Color) -> Block {
        Block::Heading {
            id: BlockId::UNASSIGNED,
            level,
            runs: vec![Run::plain(text)],
            color,
            style: None,
        }
    }

    /// The block's text with its run structure flattened away.
    ///
    /// For consumers that want the characters and not the treatment — subsetting, the heading index,
    /// a diagnostic. Anything that *sets* the text must read the runs.
    pub fn plain_text(&self) -> Option<String> {
        match self {
            Block::Heading { runs, .. } | Block::Body { runs, .. } => {
                Some(runs.iter().map(|r| r.text.as_str()).collect())
            }
            _ => None,
        }
    }

    /// The block's styled runs, or an empty slice for a block that has none.
    ///
    /// The two flowed-text variants are the only ones that carry runs; a panel's sections, a table's
    /// cells and a component's fields are strings. Added by spec 0076 so that "every run in the
    /// document" — which is what the cross-reference derivation and the font-subset collector both
    /// ask — is one function rather than a `match` copied into each of them.
    pub fn runs(&self) -> &[Run] {
        match self {
            Block::Heading { runs, .. } | Block::Body { runs, .. } => runs,
            Block::Image { .. }
            | Block::Panel { .. }
            | Block::Table { .. }
            | Block::Component { .. }
            | Block::Toc { .. }
            | Block::Index { .. } => &[],
        }
    }

    /// [`Block::runs`], mutably (spec 0079).
    ///
    /// The one caller is book composition, which has to shift every identity a run names by the
    /// chapter's id offset. Written as the mirror of [`Block::runs`] — the same exhaustive match over
    /// the same variants — so the two cannot come to disagree about which blocks carry runs.
    pub fn runs_mut(&mut self) -> &mut [Run] {
        match self {
            Block::Heading { runs, .. } | Block::Body { runs, .. } => runs,
            Block::Image { .. }
            | Block::Panel { .. }
            | Block::Table { .. }
            | Block::Component { .. }
            | Block::Toc { .. }
            | Block::Index { .. } => &mut [],
        }
    }

    /// Name an explicit paragraph style for this block. No-op on an image.
    pub fn with_style(mut self, name: impl Into<String>) -> Block {
        match &mut self {
            Block::Heading { style, .. } | Block::Body { style, .. } => *style = Some(name.into()),
            // Neither has one paragraph to style: an image has none, and a stat block is a
            // composite whose parts resolve `statblock-*` individually.
            Block::Image { .. }
            | Block::Panel { .. }
            | Block::Table { .. }
            | Block::Component { .. }
            | Block::Toc { .. }
            | Block::Index { .. } => {}
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
    /// Left insets: a first-line indent, a hanging indent, or neither (spec 0048).
    ///
    /// Additive and defaulted to zero, so every document that predates it lays out exactly as it
    /// did — `FORMAT_VERSION` does not move.
    #[serde(default, skip_serializing_if = "Indent::is_zero")]
    pub indent: Indent,
    /// Set as a list item (spec 0066). `None` is the common case and costs nothing: a document that
    /// names no list serializes and lays out exactly as it did before lists existed.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub list: Option<ListSpec>,
    /// Which weight of the family this paragraph is set in (spec 0064).
    ///
    /// Here as well as on [`InlineStyle`] so that a run has a face to override rather than a
    /// constant to contradict — the same reason `font_size_pt` lives here and `size_pt` lives there.
    #[serde(default, skip_serializing_if = "Weight::is_regular")]
    pub weight: Weight,
    #[serde(default, skip_serializing_if = "is_false")]
    pub italic: bool,
}

/// Where a tab stops, how the text there is aligned, and what fills the gap before it (spec 0067).
///
/// In [`StyleSheet`] keyed by style name rather than on [`ParagraphStyle`], and that is the design
/// fork this spec resolves: `ParagraphStyle` is `Copy` and is looked up and copied per block per
/// measurement, so a `Vec<TabStop>` on it would either cost a heap allocation on that path or force
/// the type to stop being `Copy`. A fixed-size array was considered and rejected — eight stops is
/// both an arbitrary ceiling and ~96 bytes copied per block.
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
pub struct TabStop {
    /// Distance from the frame's left edge, in points.
    pub position_pt: Pt,
    #[serde(default)]
    pub align: TabAlign,
    /// What fills the gap in front of this stop. `None` leaves it blank.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub leader: Option<Leader>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum TabAlign {
    #[default]
    Left,
    Centre,
    Right,
    /// The decimal separator sits at the stop. What counts as the separator is stated rather than
    /// taken from a locale: layout must not depend on the machine that runs it.
    Decimal,
}

/// A repeated character filling the gap in front of a stop — a dot leader in a contents list, a
/// rule in a price list.
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
pub struct Leader {
    pub glyph: char,
    /// Blank space left at each end, so the leader never touches the text on either side.
    #[serde(default)]
    pub gap_pt: Pt,
}

/// A **named** run treatment (spec 0065).
///
/// Exactly the fields [`InlineStyle`] carries, and deliberately the same shape: a named treatment
/// and an unnamed one are the same thing said in two places, and a field a style could express but
/// an override could not would be a field an author could not opt out of.
#[derive(Debug, Clone, Copy, Default, PartialEq, Serialize, Deserialize)]
pub struct CharacterStyle {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub size_pt: Option<Pt>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub color: Option<Color>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub tracking_pt: Option<Pt>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub baseline_shift_pt: Option<Pt>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub weight: Option<Weight>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub italic: Option<bool>,
}

impl CharacterStyle {
    pub const EMPTY: CharacterStyle = CharacterStyle {
        size_pt: None,
        color: None,
        tracking_pt: None,
        baseline_shift_pt: None,
        weight: None,
        italic: None,
    };

    /// This style with `over` applied on top, **field by field** (spec 0065).
    ///
    /// Field by field and not wholesale: a run naming `strong` and overriding only its colour is
    /// bold *and* recoloured, because a weight is not displaced by an override of something else.
    pub fn with(self, over: InlineStyle) -> CharacterStyle {
        CharacterStyle {
            size_pt: over.size_pt.or(self.size_pt),
            color: over.color.or(self.color),
            tracking_pt: over.tracking_pt.or(self.tracking_pt),
            baseline_shift_pt: over.baseline_shift_pt.or(self.baseline_shift_pt),
            weight: over.weight.or(self.weight),
            italic: over.italic.or(self.italic),
        }
    }
}

impl From<InlineStyle> for CharacterStyle {
    fn from(s: InlineStyle) -> CharacterStyle {
        CharacterStyle::EMPTY.with(s)
    }
}

/// A paragraph set as a list item: what marks it, and at what depth (spec 0066).
///
/// A property of the *paragraph*, not a block type of its own. A list is a run of consecutive
/// paragraphs that happen to be marked; making it a container would mean a second flow model, and
/// nothing about breaking, fragmentation or the grid works differently inside one.
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
pub struct ListSpec {
    pub marker: ListMarker,
    /// The first ordinal at this level. Ignored by a bullet.
    #[serde(default = "one_u32")]
    pub start: u32,
    /// Nesting depth, `0` outermost. Each level counts independently.
    #[serde(default)]
    pub level: u8,
}

fn one_u32() -> u32 {
    1
}

/// What stands in a list item's gutter.
///
/// A single glyph and a single suffix rather than strings, so a [`ParagraphStyle`] stays `Copy` —
/// it is passed by value through the whole measurement path, and a heap allocation there would be
/// paid per block per re-layout. It is also what markers actually are: a bullet is one character,
/// and `1.` is a counter plus one.
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum ListMarker {
    /// A fixed glyph, repeated for every item.
    Bullet { glyph: char },
    /// A counter, formatted and suffixed — `1.`, `iv)`, `C.`.
    Number {
        format: NumberFormat,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        suffix: Option<char>,
    },
}

/// How an ordinal is written.
///
/// Built by spec 0066 for a list item's marker and reused unchanged by spec 0073 for a section's
/// folio: "the third page of the front matter is written `iii`" and "the third item of this list is
/// written `iii`" are the same question, and a second numeral implementation would be a second
/// answer to it.
#[derive(
    Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize, Default,
)]
#[serde(rename_all = "snake_case")]
pub enum NumberFormat {
    #[default]
    Decimal,
    LowerAlpha,
    UpperAlpha,
    LowerRoman,
    UpperRoman,
}

impl NumberFormat {
    /// Format `n` (1-based). `0` and anything a format cannot express fall back to decimal, because
    /// a marker that silently vanished would leave an item looking like body text.
    pub fn format(self, n: u32) -> String {
        match self {
            NumberFormat::Decimal => n.to_string(),
            NumberFormat::LowerAlpha => alpha(n, b'a'),
            NumberFormat::UpperAlpha => alpha(n, b'A'),
            NumberFormat::LowerRoman => roman(n).to_lowercase(),
            NumberFormat::UpperRoman => roman(n),
        }
    }

    /// Every character this format can ever produce, for any ordinal (spec 0073).
    ///
    /// This exists because of the font subset. A folio is resolved at *layout* time, so the
    /// characters it draws are not in the document's text and the collector cannot see them — and a
    /// character missing from a subset is a `.notdef` box in a press file with no error anywhere,
    /// which is `CLAUDE.md`'s silent-press-corruption class. The collector carried a hardcoded
    /// `'0'..='9'` because arabic was the only folio there was; a roman one draws seven letters it
    /// has never seen.
    ///
    /// Derived by **running the formatter** over ordinals that exercise every symbol it has, rather
    /// than by a second hand-written table beside [`NumberFormat::format`]. A table would be a
    /// second statement of the same fact, free to disagree with the first — spec 0013's rule that
    /// the validator must read the field the writer emits, applied to glyphs. The fallbacks are
    /// covered by the same trick: `0` and an out-of-range ordinal are written in decimal by
    /// `format` itself, so asking it for those two is what pulls the digits in for roman and alpha
    /// without this function having to know that they do it.
    pub fn alphabet(self) -> BTreeSet<char> {
        let mut out: BTreeSet<char> = BTreeSet::new();
        // `0`, and a value past every format's expressible range. Whatever each format does with
        // them — a literal `"0"`, a decimal fallback, or a very long alpha run — is by definition
        // something it can produce.
        for n in [0, 1_234_567_890] {
            out.extend(self.format(n).chars());
        }
        match self {
            // 3888 = MMMDCCCLXXXVIII: all seven roman symbols in one number.
            NumberFormat::LowerRoman | NumberFormat::UpperRoman => {
                out.extend(self.format(3888).chars())
            }
            // One full turn of the bijective base-26 wheel — `a`..`z` / `A`..`Z`. Every longer
            // ordinal is written with those same 26 letters.
            NumberFormat::LowerAlpha | NumberFormat::UpperAlpha => {
                for n in 1..=26 {
                    out.extend(self.format(n).chars());
                }
            }
            NumberFormat::Decimal => {}
        }
        out
    }
}

/// Bijective base-26: 1 → `a`, 26 → `z`, 27 → `aa`. Not ordinary base-26, which would need a zero
/// digit and would write the 27th item as `ba`.
fn alpha(n: u32, base: u8) -> String {
    if n == 0 {
        return "0".into();
    }
    let mut out = Vec::new();
    let mut n = n;
    while n > 0 {
        let rem = ((n - 1) % 26) as u8;
        out.push(base + rem);
        n = (n - 1) / 26;
    }
    out.reverse();
    String::from_utf8(out).expect("ascii")
}

/// Additive-subtractive roman numerals. Above 3999 there is no standard form, so it falls back to
/// decimal rather than emitting something a reader would have to decode.
fn roman(n: u32) -> String {
    if n == 0 || n > 3999 {
        return n.to_string();
    }
    const TABLE: &[(u32, &str)] = &[
        (1000, "M"),
        (900, "CM"),
        (500, "D"),
        (400, "CD"),
        (100, "C"),
        (90, "XC"),
        (50, "L"),
        (40, "XL"),
        (10, "X"),
        (9, "IX"),
        (5, "V"),
        (4, "IV"),
        (1, "I"),
    ];
    let mut out = String::new();
    let mut n = n;
    for (v, sym) in TABLE {
        while n >= *v {
            out.push_str(sym);
            n -= v;
        }
    }
    out
}

/// A paragraph's left insets, in points (spec 0048).
///
/// Mirrors `quill_text_layout::Indent`; kept as its own serialized type because `core-model` does
/// not depend on `text-layout` and the file format must not be defined by a layout crate.
#[derive(Debug, Clone, Copy, PartialEq, Default, Serialize, Deserialize)]
pub struct Indent {
    /// Inset applied to the paragraph's first line — the opener a book sets by default.
    #[serde(default)]
    pub first_pt: Pt,
    /// Inset applied to every line after the first — what makes a wrapped `Armour Class: 15
    /// (leather, shield)` line up under its value rather than under its key.
    #[serde(default)]
    pub rest_pt: Pt,
}

impl Indent {
    pub const ZERO: Indent = Indent {
        first_pt: 0.0,
        rest_pt: 0.0,
    };

    /// A hanging indent of `pt`: first line flush, every line after it inset.
    pub const fn hanging(pt: Pt) -> Indent {
        Indent {
            first_pt: 0.0,
            rest_pt: pt,
        }
    }

    pub fn is_zero(&self) -> bool {
        self.first_pt == 0.0 && self.rest_pt == 0.0
    }
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
            indent: Indent::ZERO,
            list: None,
            weight: Weight::REGULAR,
            italic: false,
        }
    }
}

/// The style name applied to body paragraphs when a block names none.
pub const BODY_STYLE: &str = "body";

/// The stat block's name line (spec 0038).
pub const PANEL_TITLE_STYLE: &str = "statblock-title";
/// A stat block's `Key  Value` attribute lines.
pub const PANEL_ATTR_STYLE: &str = "statblock-attr";
/// A stat block's prose sections — overview, details, actions, reactions.
pub const PANEL_BODY_STYLE: &str = "statblock-body";

/// A table's header row (spec 0039).
pub const TABLE_HEADER_STYLE: &str = "table-header";
/// A table's body cells.
pub const TABLE_CELL_STYLE: &str = "table-cell";

/// Emphasis — the *name* a run is set in, not the treatment it resolves to (spec 0065).
///
/// Named for what it means rather than for what it looks like, which is the whole point of naming a
/// treatment: a house style that sets emphasis as letterspaced small caps edits `emphasis`, while a
/// document that had said `italic` would have to be re-authored.
pub const EMPHASIS_STYLE: &str = "emphasis";
/// Strong emphasis.
pub const STRONG_STYLE: &str = "strong";
/// Both at once — what `***this***` imports to.
pub const STRONG_EMPHASIS_STYLE: &str = "strong-emphasis";
/// A paragraph's opening phrase, set apart from the prose that follows it.
pub const LEAD_IN_STYLE: &str = "lead-in";

/// The named run treatments every stylesheet starts with (spec 0065).
///
/// Four, not the five the roadmap named: `code` is deliberately absent, because a code treatment is
/// a *monospace face* and the bundled family has one design. A `code` that set nothing but a
/// slightly smaller size would look like a mistake rather than like code, and shipping a named
/// treatment that does not produce the treatment is the failure spec 0064's announced-substitution
/// rule exists to avoid. It arrives when a document can name a second family.
pub fn builtin_character_styles() -> BTreeMap<String, CharacterStyle> {
    let mut out = BTreeMap::new();
    out.insert(
        EMPHASIS_STYLE.to_string(),
        CharacterStyle {
            italic: Some(true),
            ..CharacterStyle::EMPTY
        },
    );
    out.insert(
        STRONG_STYLE.to_string(),
        CharacterStyle {
            weight: Some(Weight::BOLD),
            ..CharacterStyle::EMPTY
        },
    );
    out.insert(
        STRONG_EMPHASIS_STYLE.to_string(),
        CharacterStyle {
            weight: Some(Weight::BOLD),
            italic: Some(true),
            ..CharacterStyle::EMPTY
        },
    );
    out.insert(
        LEAD_IN_STYLE.to_string(),
        CharacterStyle {
            weight: Some(Weight::BOLD),
            ..CharacterStyle::EMPTY
        },
    );
    out
}

/// A generated table of contents' own heading (spec 0041).
pub const TOC_TITLE_STYLE: &str = "toc-title";

/// A generated index's own heading (spec 0078).
pub const INDEX_TITLE_STYLE: &str = "index-title";
/// One index entry — a term and the pages it was marked on.
///
/// One style, where a contents list has six. A contents list's levels are the *heading* levels it
/// mirrors and are therefore given; an index has no such structure to mirror — sub-entries are a
/// named non-goal in spec 0078 — so a second level here would be a style nothing can select.
pub const INDEX_ENTRY_STYLE: &str = "index-entry";

/// The style a footnote's text is set in (spec 0077).
///
/// Named and in the default sheet, on spec 0066's precedent for `list-bullet`: a footnote has a
/// conventional treatment (smaller, tighter, ragged) that a document should get without authoring
/// it, and a house style that wants a different one edits the entry rather than every note.
///
/// A [`Footnote`] that names a style the sheet does not hold falls through to `body` like any other
/// block — a renamed style costs the note its treatment, not the author the note.
pub const FOOTNOTE_STYLE: &str = "footnote";

/// A bulleted list item (spec 0066).
pub const LIST_BULLET_STYLE: &str = "list-bullet";
/// A numbered list item.
pub const LIST_NUMBER_STYLE: &str = "list-number";
/// The gutter a list marker sits in: every line of an item is inset by this, and the marker is
/// drawn outside it at the frame's edge.
pub const LIST_GUTTER_PT: Pt = 14.0;

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
    /// Tab stops, keyed by the *paragraph* style they belong to (spec 0067). Resolved beside the
    /// paragraph style and passed by reference, which is what keeps `ParagraphStyle` `Copy`.
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub tabs: BTreeMap<String, Vec<TabStop>>,
    /// Named run treatments (spec 0065) — the ones this document *authored*.
    ///
    /// The four built-ins are deliberately **not** in here. They are resolved by
    /// [`StyleSheet::character`] when the map does not answer, so a document that uses `emphasis`
    /// without redefining it writes nothing about it to disk: the built-in can then be improved
    /// without migrating every document that ever named it, and no document's bytes move for a
    /// treatment it never authored.
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub character: BTreeMap<String, CharacterStyle>,
}

impl Default for StyleSheet {
    fn default() -> Self {
        let mut paragraph = BTreeMap::new();
        paragraph.insert(BODY_STYLE.to_string(), ParagraphStyle::default());
        // The two conventional list treatments, ready to name (spec 0066). Ragged rather than
        // justified: a list item is usually short, and justifying two words across a measure is
        // the defect that makes a list look broken.
        //
        // The indent is *uniform*, not spec 0048's hanging shape: every line of an item is inset by
        // the gutter and the marker is drawn outside it, so a wrapped line lines up under the item's
        // text. A hanging indent would leave the first line flush with the marker and inset only
        // the wraps, which is the key/value shape and reads as a broken list.
        for (name, marker) in [
            (LIST_BULLET_STYLE, ListMarker::Bullet { glyph: '\u{2022}' }),
            (
                LIST_NUMBER_STYLE,
                ListMarker::Number {
                    format: NumberFormat::Decimal,
                    suffix: Some('.'),
                },
            ),
        ] {
            paragraph.insert(
                name.to_string(),
                ParagraphStyle {
                    weight: Weight::REGULAR,
                    italic: false,
                    align: TextAlign::Left,
                    indent: Indent {
                        first_pt: LIST_GUTTER_PT,
                        rest_pt: LIST_GUTTER_PT,
                    },
                    list: Some(ListSpec {
                        marker,
                        start: 1,
                        level: 0,
                    }),
                    ..ParagraphStyle::default()
                },
            );
        }
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
                    weight: Weight::REGULAR,
                    italic: false,
                    list: None,
                    font_size_pt: size,
                    leading_pt: leading,
                    align: TextAlign::Left,
                    space_before_pt: leading * 0.75,
                    space_after_pt: leading * 0.25,
                    indent: Indent::ZERO,
                },
            );
        }
        // Footnote treatment (spec 0077). Smaller and tighter than the body, ragged rather than
        // justified — a note is short and its measure is the column's, so justifying two lines of
        // it is the defect that makes a footnote look broken. Space above is zero: the band already
        // separates it, and space above the first note would be charged inside the band.
        paragraph.insert(
            FOOTNOTE_STYLE.to_string(),
            ParagraphStyle {
                weight: Weight::REGULAR,
                italic: false,
                list: None,
                font_size_pt: 8.0,
                leading_pt: 10.0,
                align: TextAlign::Left,
                space_before_pt: 0.0,
                space_after_pt: 2.0,
                indent: Indent::ZERO,
            },
        );
        // Stat-block treatment (spec 0038). Built in rather than left to the author, because the
        // point of a first-class component is that dropping one in produces something that already
        // looks like a stat block. Restyling the whole book is still one edit — these three names.
        paragraph.insert(
            PANEL_TITLE_STYLE.to_string(),
            ParagraphStyle {
                weight: Weight::REGULAR,
                italic: false,
                list: None,
                font_size_pt: 13.0,
                leading_pt: 16.0,
                align: TextAlign::Left,
                space_before_pt: 0.0,
                space_after_pt: 3.0,
                indent: Indent::ZERO,
            },
        );
        paragraph.insert(
            PANEL_ATTR_STYLE.to_string(),
            ParagraphStyle {
                weight: Weight::REGULAR,
                italic: false,
                list: None,
                font_size_pt: 9.0,
                leading_pt: 11.0,
                align: TextAlign::Left,
                space_before_pt: 0.0,
                space_after_pt: 0.0,
                indent: Indent::hanging(10.0),
            },
        );
        paragraph.insert(
            PANEL_BODY_STYLE.to_string(),
            ParagraphStyle {
                weight: Weight::REGULAR,
                italic: false,
                list: None,
                font_size_pt: 9.0,
                leading_pt: 11.5,
                // Ragged, not justified: a stat block sits in a narrow panel where justification
                // opens rivers the surrounding body text would not show.
                align: TextAlign::Left,
                space_before_pt: 0.0,
                space_after_pt: 3.0,
                indent: Indent::ZERO,
            },
        );
        // Table treatment (spec 0039), on the same principle as the stat block's: a table dropped
        // into a document should already read as one.
        paragraph.insert(
            TABLE_HEADER_STYLE.to_string(),
            ParagraphStyle {
                weight: Weight::REGULAR,
                italic: false,
                list: None,
                font_size_pt: 9.0,
                leading_pt: 11.5,
                align: TextAlign::Left,
                space_before_pt: 0.0,
                space_after_pt: 0.0,
                indent: Indent::ZERO,
            },
        );
        paragraph.insert(
            TABLE_CELL_STYLE.to_string(),
            ParagraphStyle {
                weight: Weight::REGULAR,
                italic: false,
                list: None,
                font_size_pt: 9.0,
                leading_pt: 11.5,
                // Ragged: a table cell is a narrow measure, and justifying one opens rivers a
                // paragraph of the same text would not show.
                align: TextAlign::Left,
                space_before_pt: 0.0,
                space_after_pt: 0.0,
                indent: Indent::ZERO,
            },
        );
        // Contents treatment (spec 0041). Deeper levels are set smaller and indented, which is what
        // makes a contents list scannable without the author styling six levels by hand.
        paragraph.insert(
            TOC_TITLE_STYLE.to_string(),
            ParagraphStyle {
                weight: Weight::REGULAR,
                italic: false,
                list: None,
                font_size_pt: 18.0,
                leading_pt: 22.0,
                align: TextAlign::Left,
                space_before_pt: 0.0,
                space_after_pt: 11.0,
                indent: Indent::ZERO,
            },
        );
        for level in 1u8..=6 {
            let size = (12.0 - (level as f32 - 1.0) * 0.5).max(8.5);
            paragraph.insert(
                toc_entry_style_name(level),
                ParagraphStyle {
                    weight: Weight::REGULAR,
                    italic: false,
                    list: None,
                    font_size_pt: size,
                    leading_pt: size + 4.0,
                    align: TextAlign::Left,
                    // Level 1 entries get air above them; deeper ones sit tight under their parent.
                    space_before_pt: if level == 1 { 5.0 } else { 0.0 },
                    space_after_pt: 0.0,
                    indent: Indent::ZERO,
                },
            );
        }
        // Index treatment (spec 0078), on the same principle as the contents list's: an index
        // dropped into a book should already read as one. Smaller and tighter than a contents
        // entry, because an index is a long list of short lines and a contents list is a short list
        // of long ones, and ragged rather than justified for the reason a footnote is — an entry is
        // one clipped line that never wraps, so there is nothing to justify.
        paragraph.insert(
            INDEX_TITLE_STYLE.to_string(),
            ParagraphStyle {
                weight: Weight::REGULAR,
                italic: false,
                list: None,
                font_size_pt: 18.0,
                leading_pt: 22.0,
                align: TextAlign::Left,
                space_before_pt: 0.0,
                space_after_pt: 11.0,
                indent: Indent::ZERO,
            },
        );
        paragraph.insert(
            INDEX_ENTRY_STYLE.to_string(),
            ParagraphStyle {
                weight: Weight::REGULAR,
                italic: false,
                list: None,
                font_size_pt: 9.0,
                leading_pt: 11.0,
                align: TextAlign::Left,
                space_before_pt: 0.0,
                space_after_pt: 0.0,
                indent: Indent::ZERO,
            },
        );
        StyleSheet {
            paragraph,
            tabs: BTreeMap::new(),
            character: BTreeMap::new(),
        }
    }
}

impl StyleSheet {
    /// The style that applies to `block`.
    ///
    /// Resolution is: the block's explicit style name, else its structural default (`body`, or
    /// `h{level}` for a heading). An unknown name falls back to `body` and finally to
    /// [`ParagraphStyle::default`] — a missing style must not lose the text. Losing a paragraph
    /// because its style was renamed would be far worse than setting it in the body face.
    /// The named run treatment `name` resolves to, or `None` if nothing defines it (spec 0065).
    ///
    /// A document's own `character` map wins, then the built-ins. `None` is not an error: the run
    /// lays out with the paragraph's treatment and its own overrides still apply, which is the
    /// authoring-posture fallback specs 0028 and 0054 both take — losing text because a style was
    /// renamed, or because the pack defining it is missing, would be far worse than setting it
    /// plainly.
    /// The tab stops a block's paragraph style names, in ascending position order (spec 0067).
    ///
    /// Empty when the style names none, which is every document that predates them — and is what
    /// lets the measurement path skip the mechanism entirely rather than lay out a tabless line
    /// through it.
    pub fn tabs_for(&self, block: &Block) -> &[TabStop] {
        let name = match block {
            Block::Heading { style, level, .. } => {
                style.clone().unwrap_or_else(|| heading_style_name(*level))
            }
            Block::Body { style, .. } => style.clone().unwrap_or_else(|| BODY_STYLE.to_string()),
            _ => return &[],
        };
        self.tabs.get(&name).map_or(&[], |v| v.as_slice())
    }

    pub fn character(&self, name: &str) -> Option<CharacterStyle> {
        self.character
            .get(name)
            .copied()
            .or_else(|| builtin_character_styles().get(name).copied())
    }

    /// The treatment a run is actually set in: its named character style with its own overrides
    /// folded on top, field by field (spec 0065).
    ///
    /// **One function, because its consumers must not be able to disagree about precedence.** The
    /// layout engine derives a run's ink, format and baseline shift from this; preflight derives the
    /// colour it has to check from it (spec 0081). A preflight that resolved a run's colour its own
    /// way could read `run.style.color` and miss the named style's — which is exactly the defect
    /// 0081 fixed, and exactly the reason a second implementation of this may not exist.
    ///
    /// A name that resolves to nothing is not an error: the run lays out with the paragraph's
    /// treatment and its own overrides still apply, which is the posture specs 0028 and 0054 both
    /// take.
    pub fn resolve_run(&self, run: &Run) -> CharacterStyle {
        run.character
            .as_deref()
            .and_then(|name| self.character(name))
            .unwrap_or(CharacterStyle::EMPTY)
            .with(run.style)
    }

    pub fn resolve(&self, block: &Block) -> ParagraphStyle {
        let named = match block {
            Block::Heading { style, level, .. } => {
                style.clone().unwrap_or_else(|| heading_style_name(*level))
            }
            Block::Body { style, .. } => style.clone().unwrap_or_else(|| BODY_STYLE.to_string()),
            // A composite has no single paragraph treatment; its parts resolve `statblock-*`
            // themselves. `resolve` must still be total, so both fall back to the default.
            Block::Image { .. }
            | Block::Panel { .. }
            | Block::Table { .. }
            | Block::Component { .. }
            | Block::Toc { .. }
            | Block::Index { .. } => return ParagraphStyle::default(),
        };
        self.paragraph
            .get(&named)
            .or_else(|| self.paragraph.get(BODY_STYLE))
            .copied()
            .unwrap_or_default()
    }
}

/// How a [`MasterStatic::Text`] is set within its rect (spec 0047).
///
/// Distinct from [`TextAlign`], which is the treatment of a *flowed* paragraph: a static is one
/// unbroken line, so there is nothing to justify and no last line to except. `Left` is the default
/// and is exactly the pre-spec-0047 behaviour — a static that says nothing is drawn where it always
/// was.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum StaticAlign {
    /// Flush to the rect's left edge.
    #[default]
    Left,
    /// Centred in the rect.
    Center,
    /// Flush to the rect's right edge.
    Right,
    /// Flush to the spine side: left on a recto, right on a verso.
    Inside,
    /// Flush to the fore-edge side: right on a recto, left on a verso.
    Outside,
}

impl StaticAlign {
    /// The x at which a line of width `line_w_pt` sits inside `rect` on `page_index`.
    ///
    /// `Inside`/`Outside` resolve by page parity through [`is_recto`] — the same rule
    /// [`Margins::left_right`] uses, which is what makes "outside" mean the same thing to a margin
    /// and to a folio.
    ///
    /// A line wider than its rect keeps its aligned edge and overflows *inward* rather than being
    /// clamped: a too-wide `Outside` running head then runs into the page instead of off the trim.
    /// Visible and fixable beats silently guillotined (`CLAUDE.md`).
    pub fn x_for(self, rect: Rect, line_w_pt: Pt, page_index: usize, facing_pages: bool) -> Pt {
        let recto = is_recto(page_index, facing_pages);
        let resolved = match self {
            StaticAlign::Inside if recto => StaticAlign::Left,
            StaticAlign::Inside => StaticAlign::Right,
            StaticAlign::Outside if recto => StaticAlign::Right,
            StaticAlign::Outside => StaticAlign::Left,
            other => other,
        };
        match resolved {
            StaticAlign::Center => rect.x_pt + (rect.w_pt - line_w_pt) / 2.0,
            StaticAlign::Right => rect.x_pt + rect.w_pt - line_w_pt,
            // `Inside`/`Outside` were resolved above; `Left` is the rect's own origin.
            _ => rect.x_pt,
        }
    }
}

/// A repeating element a master page stamps onto each page it governs.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum MasterStatic {
    /// A line of text at a fixed position — a running head, a folio, a footer.
    ///
    /// `text` may contain any [`StaticToken`] — `{page}`, `{section}`, `{heading:N}` — replaced when
    /// the page is laid out. Tokens rather than distinct "folio" and "running head" variants, so one
    /// static can read "The Dungeon — 42" without needing a second element type, and so a new
    /// derived string costs a token rather than a variant every consumer must learn.
    Text {
        /// Where the line sits, relative to the trim box — **as it looks on a recto** when
        /// `mirror` is set.
        rect: Rect,
        text: String,
        color: Color,
        /// Paragraph style name, resolved against the document's stylesheet.
        #[serde(default)]
        style: Option<String>,
        /// Where the line sits within `rect` (spec 0047).
        #[serde(default, skip_serializing_if = "is_default_align")]
        align: StaticAlign,
        /// Whether `rect` is mirrored across the spine on a verso (spec 0047).
        ///
        /// Alignment alone cannot put a folio at the outside corner of both halves of a spread,
        /// because the band it may sit in is *itself* asymmetric whenever the inside and outside
        /// margins differ. Set this and the rect flips about the page's vertical centre on a verso,
        /// exactly as [`Margins`] swap sides. Ignored when `facing_pages` is off.
        #[serde(default, skip_serializing_if = "is_false")]
        mirror: bool,
    },
    /// A linked image at a fixed position — background art, a border, a decorative rule.
    Image { rect: Rect, asset: String },
}

/// Both spec-0047 fields are omitted from the manifest when they hold their defaults, on the
/// precedent of `pages` (spec 0035). Not cosmetic: the exported PDF's `/ID` is a hash of the
/// manifest text, so a migration that added two keys to every static would change the identifier —
/// and therefore every exported byte — of every document that has furniture.
fn is_default_align(a: &StaticAlign) -> bool {
    *a == StaticAlign::default()
}

fn is_false(b: &bool) -> bool {
    !*b
}

impl MasterStatic {
    /// A left-aligned, unmirrored text static — the shape every static had before spec 0047.
    pub fn text(rect: Rect, text: impl Into<String>, color: Color) -> MasterStatic {
        MasterStatic::Text {
            rect,
            text: text.into(),
            color,
            style: None,
            align: StaticAlign::default(),
            mirror: false,
        }
    }

    /// Where this static's box sits on `page_index`, after parity mirroring.
    ///
    /// The authored rect is what the page looks like on a **recto**; on a verso a mirrored rect is
    /// reflected about the page's vertical centre. An unmirrored static — and every static on a
    /// single-sided document — gets its authored rect back unchanged, which is the pre-0047
    /// behaviour.
    pub fn rect_on(&self, page_index: usize, setup: &PageSetup) -> Rect {
        let (rect, mirror) = match self {
            MasterStatic::Text { rect, mirror, .. } => (*rect, *mirror),
            MasterStatic::Image { rect, .. } => (*rect, false),
        };
        if !mirror || is_recto(page_index, setup.facing_pages) {
            return rect;
        }
        Rect {
            x_pt: setup.trim.w_pt - (rect.x_pt + rect.w_pt),
            ..rect
        }
    }
}

/// The page-number token replaced in [`MasterStatic::Text`] — see [`StaticToken::Page`].
pub const PAGE_TOKEN: &str = "{page}";

/// The section-name token (spec 0074) — see [`StaticToken::Section`].
pub const SECTION_TOKEN: &str = "{section}";

/// The opening of the heading token (spec 0074): `{heading:1}`, `{heading:2}`, …
///
/// A prefix rather than a whole spelling because the token carries a level. See
/// [`StaticToken::Heading`].
pub const HEADING_TOKEN_PREFIX: &str = "{heading:";

/// Everything a [`StaticToken`] can draw, for the font-subset collector (spec 0074).
///
/// Two collections for the reason `FaceText` has two: since spec 0068 the writer draws *shaped*
/// glyphs, so a token whose resolved value is a knowable string contributes that string as a **run**
/// — its ligatures are cut into the subset — while a token whose value is arithmetic rather than
/// authored text can only offer the alphabet it will be written in, one character at a time.
#[derive(Debug, Default, Clone, PartialEq, Eq)]
pub struct TokenText {
    /// Whole strings this token can resolve to.
    pub runs: Vec<String>,
    /// Loose characters, where the exact string is not knowable before layout.
    pub chars: BTreeSet<char>,
}

/// A token in a [`MasterStatic::Text`] that is resolved when the page is laid out (spec 0074).
///
/// **This enum is the one list of layout-time tokens, and that is the point of it.** The font-subset
/// collector runs *before* layout, so every character a token will become has to be predicted; a
/// character it misses is a `.notdef` box in a press file with no error anywhere, which is
/// `CLAUDE.md`'s silent-press-corruption class and which this exact site has been caught by twice
/// (PR #107, and spec 0073's roman folio). Three properties close it structurally rather than by
/// remembering:
///
/// 1. [`StaticToken::scan`] is the **only** way to find a token in a string, and both the resolver
///    (`quill_layout_engine::resolve_static_text`) and the collector (`collect_doc_faces`) call it.
///    A spelling the parser does not know is resolved by nobody, so the two cannot disagree about
///    what a token *is*.
/// 2. [`StaticToken::contributes`] is an exhaustive `match`. A new variant does not compile until it
///    says what characters it can draw.
/// 3. The resolver's `match` is exhaustive too, so a new variant does not compile there either.
///
/// Adding a token therefore fails to build until the collector has been taught about it. There is no
/// comment to remember and no runtime check to skip.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum StaticToken {
    /// `{page}` — the page's folio (spec 0073): one-based arabic unless a section says otherwise.
    Page,
    /// `{section}` — the [`Section::name`] of the section the page belongs to (spec 0074).
    Section,
    /// `{heading:N}` — the text of the last heading of level ≤ `N` at or before this page.
    ///
    /// `{heading:1}` is "the current chapter", which is what a running head almost always wants;
    /// `{heading:2}` follows the sub-section as well, which is what a reference book's verso wants.
    Heading { level: u8 },
}

impl StaticToken {
    /// Every token occurrence in `text`, in order, as (byte range, token).
    ///
    /// The single parser. A `{…}` group whose body is not a token this build knows is left alone and
    /// prints literally — the same posture a dangling master name and a dangling style name already
    /// have, and the reason an older build's handling of a newer token is *loud* rather than silent.
    pub fn scan(text: &str) -> Vec<(std::ops::Range<usize>, StaticToken)> {
        let mut out = Vec::new();
        let mut cursor = 0;
        while let Some(open) = text[cursor..].find('{') {
            let start = cursor + open;
            let Some(close) = text[start..].find('}') else {
                break;
            };
            let end = start + close + 1;
            if let Some(token) = StaticToken::parse(&text[start..end]) {
                out.push((start..end, token));
            }
            cursor = end;
        }
        out
    }

    /// One whole `{…}` group, or `None` if it spells no token.
    fn parse(whole: &str) -> Option<StaticToken> {
        if whole == PAGE_TOKEN {
            return Some(StaticToken::Page);
        }
        if whole == SECTION_TOKEN {
            return Some(StaticToken::Section);
        }
        let level = whole
            .strip_prefix(HEADING_TOKEN_PREFIX)?
            .strip_suffix('}')?
            .parse()
            .ok()?;
        Some(StaticToken::Heading { level })
    }

    /// Everything this token can draw in `doc` — what the font-subset collector must carry.
    ///
    /// Deliberately **not** conditioned on where anything landed, for [`Document::folio_formats`]'s
    /// reason: a section name is in the set whether or not that section is placed today. Embedding a
    /// few characters nobody draws costs a few hundred bytes; failing to embed one that is drawn is a
    /// `.notdef` box in a press file.
    pub fn contributes(&self, doc: &Document) -> TokenText {
        match self {
            // A folio is arithmetic, not authored text: what it will say is not known until the
            // sections have been placed, so what is carried is the alphabet each configured format
            // can write (spec 0073).
            StaticToken::Page => TokenText {
                runs: Vec::new(),
                chars: doc
                    .folio_formats()
                    .into_iter()
                    .flat_map(|f| f.alphabet())
                    .collect(),
            },
            // Authored text, and knowable exactly — so it travels as a run and its ligatures are cut
            // into the subset like any other run's.
            StaticToken::Section => TokenText {
                runs: doc.sections.iter().map(|s| s.name.clone()).collect(),
                chars: BTreeSet::new(),
            },
            // Every heading this token could reach. A deeper level is a superset of a shallower one,
            // so the level really does narrow what is carried.
            StaticToken::Heading { level } => TokenText {
                runs: doc
                    .content
                    .iter()
                    .filter(|b| matches!(b, Block::Heading { level: l, .. } if l <= level))
                    .filter_map(|b| b.plain_text())
                    .collect(),
                chars: BTreeSet::new(),
            },
        }
    }
}

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
/// that pushes the book by a page slides every subsequent assignment.
///
/// That was recorded here as the gap a notion of "section" would have to fill, and spec 0072 fills
/// it — **without replacing this type**. A [`Section`] is anchored to a [`BlockId`]; the page its
/// anchor lands on is read off the laid-out page vector each pass, and a `Vec<PageOverride>` is
/// synthesised from it by [`Document::page_assignment`]. So index assignment turned out to be the
/// right *representation* and the wrong authoring surface: this is still what resolution reads, and
/// [`Document::master_for`] is unchanged. A positionally authored list is still honoured, and is
/// still positional — it is a section that survives repagination, not an entry in this list.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct PageOverride {
    /// The master this page uses, overriding [`Document::default_master`].
    ///
    /// `None` declines to override — which is deliberately *not* the same as "no master": an
    /// explicit entry that sets nothing still falls through to the document's default.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub master: Option<String>,
}

/// A named division of the document, anchored to the block that opens it (spec 0072).
///
/// The anchor is a [`BlockId`], not a page index, and that is the whole point: a block id is stable
/// across every edit that does not delete the block (spec 0026), so "the master that opens this
/// chapter" survives repagination in a way `pages[7]` cannot. Where the section *starts* is derived
/// — the first page its anchor block was placed on — and from that derivation the engine synthesises
/// the [`PageOverride`] list that [`Document::master_for`] has always read.
///
/// A section is deliberately **not** a container: it does not own its blocks and there is no tree.
/// It marks a point in the flat content list, and a section runs until the next section's anchor.
/// Nesting a part inside a chapter is expressible as two sections whose anchors are adjacent, and
/// making it a container would change every consumer of `Document::content` for a feature nothing
/// needs yet.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Section {
    /// What this section is called — a chapter title, "Front matter", "Appendix A".
    ///
    /// Authored rather than derived from the anchor's text, because the two are routinely different:
    /// a running head says "The Ruined Keep" where the chapter opener says "Chapter One: The Ruined
    /// Keep". Spec 0074 is what prints it.
    pub name: String,
    /// The block that opens the section.
    ///
    /// An anchor naming a block the document does not contain is **not** an error: the section is
    /// simply not placed, and contributes no assignment. Losing a chapter opener's furniture because
    /// the heading it pointed at was deleted is recoverable and visible; refusing to open the
    /// document is neither — the same posture a dangling master name gets.
    pub start: BlockId,
    /// The master applied to the section's **opening page**, overriding
    /// [`Document::default_master`] there and nowhere else.
    ///
    /// One page, not a range, because that is what the feature this answers actually is: a chapter
    /// opener has a deeper top margin and no folio, and page two of the chapter is ordinary body.
    /// A master for the rest of the section is a different thing (it would have to lose to the
    /// *next* section's opener) and is left for whoever needs it.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub master: Option<String>,
    /// How this section's pages are numbered (spec 0073), or `None` to carry on exactly as the
    /// pages before it were numbered.
    ///
    /// Unlike [`Section::master`] this governs the section's **whole run** — every page from its
    /// opener until the next section that states one — because that is what page numbering is. A
    /// format that applied to one page would number the first page of the front matter `i` and the
    /// second `2`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub folio: Option<Folio>,
}

/// How a [`Section`]'s pages are numbered (spec 0073).
///
/// The two halves of what a book actually needs: roman front matter and arabic body is a *format*
/// change, and a part opener that starts the count again is a *restart*. They are independent —
/// appendix pages that keep counting from the body but are written `A-1`-style would be a format
/// change with no restart — so they are two fields rather than one enum.
///
/// The numerals themselves are [`NumberFormat`]'s, unchanged from spec 0066.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
pub struct Folio {
    /// The numerals this section's folios are written in. Defaults to [`NumberFormat::Decimal`],
    /// which is what every page was written in before this existed.
    #[serde(default)]
    pub format: NumberFormat,
    /// The number this section's **first page** carries.
    ///
    /// `None` continues the count from the page before, which is what a section that only changes
    /// the *format* wants. `Some(1)` is the restart a part opener asks for; `Some(n)` is the offset
    /// a chapter extracted from a larger book needs, and is the shape spec 0079's book-level
    /// pagination will read.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub restart_at: Option<u32>,
}

/// One stretch of pages numbered by a single rule (spec 0073).
///
/// The derived form of the folio settings scattered across [`Document::sections`]: a document is a
/// sequence of runs, each beginning at a page and carrying a format and the number that page shows.
/// Every folio in the run is `start + (page - first_page)`, which is why this is a *range*
/// description rather than a per-page table — a 500-page book has two or three of these.
///
/// It is also exactly the shape PDF's `/PageLabels` number tree wants, which is not a coincidence:
/// both answer "from this page onward, count like this".
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct FolioRun {
    /// Zero-based index of the first page this run governs.
    pub first_page: usize,
    /// How the numbers in this run are written.
    pub format: NumberFormat,
    /// The number shown on `first_page`.
    pub start: u32,
}

/// A forced page break: the content at [`PageBreak::before`] resumes on a **new page** rather than
/// in the frame the flow had reached (spec 0080).
///
/// ### Where a break lives, and why it is not on the block
///
/// A break is a property *of a block* and not a `Block` variant of its own — spec 0066's finding
/// about a list, at the second site that needed it. A `Block::PageBreak` would be a block with no
/// content that every consumer of a block has to skip: it would need an id, a measurement, a
/// placement, a fragment rule and an arm in the font-subset collector, and deleting the heading it
/// was written for would leave it behind, breaking the page in front of whatever moved up. It is
/// also the shape the model has already rejected once: a section is a marker anchored to a block,
/// not a container block sitting between two others.
///
/// It is **anchored** rather than stored inside the block for a reason with teeth: a break changes
/// *where a block goes*, never *what it measures*, so it must not be inside the value the
/// measurement cache keys on. `MeasureKey` hashes the block's content (`content_fingerprint`), so a
/// `break_before` field on `Block::Body` would re-break a paragraph — at the same width, to the
/// identical line list — every time an author moved a chapter opener. That is exactly the mistake
/// spec 0077 declined to make with a note's text, and anchoring makes it unrepresentable rather
/// than merely avoided.
///
/// An entry naming a block the document does not contain is **not** an error and contributes
/// nothing — the posture a dangling section anchor (spec 0072), a dangling master name and a
/// dangling style name already have.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct PageBreak {
    /// The block the flow must start a page for. The break belongs to the seam *in front of* it.
    pub before: BlockId,
    /// What kind of page it must start on. Defaulted, because an entry that exists at all is a
    /// break: the common case says nothing and gets [`BreakKind::Page`].
    #[serde(default)]
    pub kind: BreakKind,
}

/// What a [`PageBreak`] demands of the page the content resumes on (spec 0080).
///
/// Three kinds and one rule: the flow advances pages until it is at the top of a page the kind
/// accepts. `Recto` and `Verso` are the same parity test with the two answers — a mechanism about
/// *which side of the spread*, not about right-hand pages, because "the plate faces the opener" is
/// as ordinary a request as "the chapter opens right".
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum BreakKind {
    /// No break: the content continues in the frame the flow had reached. Representable so that a
    /// book file can *decline* the break its chapters get by default (see `BookChapter`), and
    /// filtered out before the flow ever sees it.
    None,
    /// The next page, whichever side it falls on.
    #[default]
    Page,
    /// The next right-hand page, inserting one blank page when the next page is a verso. This is
    /// "start this section on the next recto", named as a non-goal by specs 0072 and 0073.
    Recto,
    /// The next left-hand page, by the same rule with the other answer.
    Verso,
}

impl BreakKind {
    /// Whether a break of this kind is satisfied by a page starting at `page_index`.
    ///
    /// Parity goes through [`is_recto`] rather than through a parity test written here, so the side
    /// a break lands on and the side a margin mirrors to cannot disagree — the one place page parity
    /// is decided (spec 0047).
    ///
    /// **With facing pages off, every kind accepts every page.** A single-sided document has no
    /// spread, which is the same answer [`Margins::left_right`] already gives, and it is also what
    /// bounds the flow: a `Verso` break on a document with no versos would otherwise search for a
    /// page that does not exist.
    pub fn accepts(self, page_index: usize, facing_pages: bool) -> bool {
        match self {
            BreakKind::None | BreakKind::Page => true,
            BreakKind::Recto => is_recto(page_index, facing_pages),
            BreakKind::Verso => !facing_pages || !is_recto(page_index, facing_pages),
        }
    }

    /// Whether this kind asks the flow for anything at all.
    pub fn is_break(self) -> bool {
        self != BreakKind::None
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
    /// The document's sections (spec 0072), in document order.
    ///
    /// Order is the author's, not derived from where the anchors sit in `content`: nothing here
    /// re-sorts them, because a list that silently reorders itself is a list nobody can edit. What
    /// *is* derived is where each one starts — see [`Document::page_assignment`].
    ///
    /// An empty list is exactly the pre-0072 behaviour, which is why every other document is
    /// unmoved. It is **not** why `FORMAT_VERSION` moved: an older build would drop this list and
    /// set every chapter opener in the body master, silently, which is the bump rule in
    /// `docs/format-spec.md`.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub sections: Vec<Section>,
    /// The document's footnotes (spec 0077), in no particular order.
    ///
    /// Order here is *not* the numbering: a note is numbered by where its **anchor** appears in
    /// `content`, so re-ordering this list changes nothing a reader sees. That is what makes the
    /// list an unordered store rather than a second content sequence to keep in step.
    ///
    /// A note nothing anchors is not an error and is not placed — the same posture a dangling
    /// section anchor gets. An empty list is exactly the pre-0077 behaviour; it is **not** why
    /// `FORMAT_VERSION` moved, which is that an older build drops this list and destroys the notes.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub footnotes: Vec<Footnote>,
    /// The document's forced page breaks (spec 0080), anchored to the blocks they open a page for.
    ///
    /// Order here is not meaningful: a break is looked up by its anchor, and two entries naming the
    /// same block are resolved by the last one, on [`Document::sections`]' "later authored wins".
    ///
    /// An empty list is exactly the pre-0080 behaviour — every document without one lays out and
    /// exports byte for byte as it did. It is **not** why `FORMAT_VERSION` moved: an older build
    /// drops this list, sets the whole book as one run of text with every chapter beginning halfway
    /// down a page, and saves that back over the author's intent.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub breaks: Vec<PageBreak>,
    /// Component definitions this document carries beyond the bundled two (spec 0054).
    ///
    /// Keyed by definition name. Additive and defaulted, so a v3 manifest written before component
    /// definitions existed loads and lays out exactly as it did — which is why `FORMAT_VERSION`
    /// stays 3. Spec 0056 adds the other source: definitions resolved from installed packs.
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub components: BTreeMap<String, ComponentDef>,
    /// The content packs this document needs (spec 0056).
    ///
    /// A requirement that does not resolve is a refusal, not a fallback: a book set in the default
    /// face because its pack was missing looks *subtly* wrong, and is discovered at the print shop.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub requires: Vec<PackRequirement>,
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
            sections: Vec::new(),
            footnotes: Vec::new(),
            breaks: Vec::new(),
            components: Default::default(),
            requires: Vec::new(),
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
        // Footnotes share the one id space (spec 0077): an anchor names a note by `BlockId`, the
        // placed note reports that id as its `source`, and the resolved-text map that serves both a
        // cross-reference's target and a note's number is keyed by it. Two of them colliding would
        // be the same silent wrong-layout this function already refuses for two blocks.
        for id in self
            .content
            .iter()
            .map(Block::id)
            .chain(self.footnotes.iter().map(|f| f.id))
        {
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
        for note in &mut self.footnotes {
            if !note.id.is_assigned() {
                note.id = BlockId(next);
                next += 1;
            }
        }
        self.next_block_id = next;
        Ok(())
    }

    /// What each footnote's anchor prints, keyed by note id (spec 0077).
    ///
    /// **Derived from `content` order, before anything is laid out** — which is the whole of the
    /// numbering decision. A number that restarted per page would not be knowable here (the page is
    /// not known until the flow runs) and would feed back into the flow through the anchor run's
    /// width, so it would need a fixpoint *inside* `flow`. Document-sequential is exactly
    /// [`crate::ListSpec`]'s shape instead: one walk over the blocks, in order, assigning 1, 2, 3…
    /// to each distinct note the first time an anchor names it. See `specs/0077-footnotes.md` for
    /// the argument and for what per-page restart would cost.
    ///
    /// A note in [`Document::footnotes`] that no anchor names gets no number and is not placed. An
    /// anchor naming a note the document does not hold is numbered anyway — it is a real anchor in
    /// real prose — and its note is simply missing; the anchor is what tells the reader so.
    ///
    /// Empty for every document without a footnote, which is what lets the whole feature be skipped.
    pub fn footnote_numbers(&self) -> BTreeMap<BlockId, String> {
        footnote_numbers(&self.footnote_blocks())
    }

    /// Each anchored footnote as the [`Block`] that is measured and placed for it (spec 0077).
    ///
    /// **One synthesis site, read by both the layout engine and the font-subset collector**, which
    /// is the structural half of this increment's answer to spec 0074's class. The note's number is
    /// a [`RunSource::Footnote`] run — the same variant the anchor carries, so the two cannot print
    /// different numbers — followed by an authored [`FOOTNOTE_NUMBER_SUFFIX`] run and the note's own
    /// runs. A collector that walks these blocks therefore carries every character a note draws
    /// without knowing anything about footnotes.
    ///
    /// The block takes the note's id, so a placed note reports it as its `source` and "the anchor
    /// and its note landed on the same page" is a question a page vector can answer.
    ///
    /// Only notes an anchor names **and** that the document actually holds are returned, in
    /// document order — the order they are numbered in, which is also the order a frame's band
    /// stacks them in. An anchor whose note is missing is therefore absent from
    /// [`footnote_numbers`] and prints [`UNRESOLVED_REFERENCE`]: a visible failure, and the notes
    /// that *are* there stay numbered 1, 2, 3 without a hole.
    pub fn footnote_blocks(&self) -> Vec<Block> {
        let mut out = Vec::new();
        for block in &self.content {
            for run in block.runs() {
                let Some(id) = run.source.note() else {
                    continue;
                };
                if out.iter().any(|b: &Block| b.id() == id) {
                    continue;
                }
                let Some(note) = self.footnotes.iter().find(|f| f.id == id) else {
                    continue;
                };
                let mut runs = Vec::with_capacity(note.runs.len() + 2);
                runs.push(Run::footnote(id));
                runs.push(Run::plain(FOOTNOTE_NUMBER_SUFFIX));
                runs.extend(note.runs.iter().cloned());
                out.push(Block::Body {
                    id,
                    runs,
                    color: note.color,
                    style: Some(
                        note.style
                            .clone()
                            .unwrap_or_else(|| FOOTNOTE_STYLE.to_string()),
                    ),
                });
            }
        }
        out
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
        self.master_in(&self.pages, page_index)
    }

    /// [`Document::master_for`] against an explicit assignment.
    ///
    /// The same resolution, reading a list handed in rather than `self.pages`, so that the
    /// section-derived assignment (spec 0072) resolves through *this* function and not a second one
    /// that could disagree with it. `master_for` is this with `self.pages`, so nothing about a
    /// document without sections moves.
    pub fn master_in<'d>(
        &'d self,
        pages: &[PageOverride],
        page_index: usize,
    ) -> Option<&'d MasterPage> {
        let named = pages
            .get(page_index)
            .and_then(|p| p.master.as_deref())
            .and_then(|name| self.master_pages.iter().find(|m| m.name == name));
        named.or_else(|| {
            self.default_master
                .as_deref()
                .and_then(|name| self.master_pages.iter().find(|m| m.name == name))
        })
    }

    /// Synthesise the per-page assignment from where each section's anchor landed (spec 0072).
    ///
    /// `starts` is parallel to [`Document::sections`]: `starts[i]` is the zero-based page
    /// `sections[i]`'s anchor block was first placed on, or `None` if the anchor is not in the
    /// document (or was not placed). The result is the authored [`Document::pages`] list with one
    /// entry written per placed section that names a master.
    ///
    /// **A section wins over a positional entry at the same index**, and that is the one precedence
    /// decision here. The two disagree only when a document carries both; the section is the one
    /// that tracked the content through repagination, and the positional entry is by construction
    /// the stale statement of the same intent — which is the defect spec 0035 recorded and could not
    /// fix. Sections themselves are applied in list order, so two sections whose anchors land on the
    /// same page give that page the later one's master.
    ///
    /// Pure, and deliberately not a method on the engine: it takes numbers and returns the list, so
    /// the derivation can be tested without laying anything out.
    pub fn page_assignment(&self, starts: &[Option<usize>]) -> Vec<PageOverride> {
        let mut pages = self.pages.clone();
        for (section, start) in self.sections.iter().zip(starts) {
            let (Some(start), Some(master)) = (start, section.master.as_ref()) else {
                continue;
            };
            if pages.len() <= *start {
                pages.resize(*start + 1, PageOverride::default());
            }
            pages[*start].master = Some(master.clone());
        }
        pages
    }

    /// The page-numbering runs this document's sections produce (spec 0073).
    ///
    /// `starts` is the same list [`Document::page_assignment`] takes: `starts[i]` is the page
    /// `sections[i]`'s anchor was placed on, or `None` if it was not placed. Pure, and takes numbers
    /// rather than pages, for the same reason that one does — the derivation is testable without
    /// laying anything out.
    ///
    /// The result always begins with a run at page 0, so **every page has a rule** and a document
    /// with no sections gets exactly the arabic 1-based numbering it had before this existed. A
    /// section that states no [`Folio`] starts no run: it is a division of the document, not of the
    /// pagination, and a chapter opener does not restart the page numbers.
    ///
    /// Ordered by **page**, not by the author's section order, because a run is a statement about a
    /// stretch of the book. Two sections placed on the same page collapse to one run and the later
    /// one wins — the same precedence [`Document::page_assignment`] gives a master, and for the same
    /// reason: nothing re-sorts the authored list, so "later in the list" is the only tie-break that
    /// does not depend on where the anchors happened to land.
    pub fn folio_runs(&self, starts: &[Option<usize>]) -> Vec<FolioRun> {
        // Page 0 onwards, arabic from 1: the pre-0073 rule, and the rule for every page ahead of the
        // first section that states a folio.
        let mut runs = vec![FolioRun {
            first_page: 0,
            format: NumberFormat::Decimal,
            start: 1,
        }];

        let mut stated: Vec<(usize, Folio)> = self
            .sections
            .iter()
            .zip(starts)
            .filter_map(|(s, start)| Some(((*start)?, s.folio?)))
            .collect();
        // Stable, so two sections on one page stay in the author's order and the later one is
        // applied last.
        stated.sort_by_key(|(page, _)| *page);

        for (page, folio) in stated {
            // What this page would have shown had the section said nothing — which is what
            // `restart_at: None` means, and it has to be read off the run in force *here* rather
            // than from a running counter, because a run is arithmetic rather than a walk.
            let previous = runs.last().expect("a run at page 0 always exists");
            let continued = previous
                .start
                .saturating_add((page.saturating_sub(previous.first_page)) as u32);
            let run = FolioRun {
                first_page: page,
                format: folio.format,
                start: folio.restart_at.unwrap_or(continued),
            };
            // A second section on the same page replaces the first's run rather than adding an
            // empty one, so the list stays strictly increasing and `folio_in` can take the last
            // run that starts at or before a page.
            if previous.first_page == page {
                let last = runs.len() - 1;
                runs[last] = run;
            } else {
                runs.push(run);
            }
        }
        runs
    }

    /// The folio printed on `page_index` — the string a reader sees on the page (spec 0073).
    ///
    /// The one place a page number becomes text. Everything that prints one goes through here: the
    /// `{page}` token in a master static, and the contents list, which must agree with it or the
    /// book sends a reader to a page that does not answer to the number beside its title.
    pub fn folio_in(runs: &[FolioRun], page_index: usize) -> String {
        let (format, number) = Document::folio_number_in(runs, page_index);
        format.format(number)
    }

    /// The numbering behind [`Document::folio_in`] — the format and the number, before either
    /// becomes text (spec 0078).
    ///
    /// Page-range coalescing needs both. `iv` and `v` join into a range and `ix` and `1` do not,
    /// and neither fact is recoverable from the two *strings*: telling them apart means comparing
    /// the format and the arithmetic, which is exactly what this returns. `folio_in` is one call to
    /// [`NumberFormat::format`] over it, so there is still one implementation of what a page prints
    /// and not two.
    pub fn folio_number_in(runs: &[FolioRun], page_index: usize) -> (NumberFormat, u32) {
        let Some(run) = runs.iter().rev().find(|r| r.first_page <= page_index) else {
            // Unreachable through `folio_runs`, which always emits a run at page 0. Stated rather
            // than `expect`ed because an empty list is a caller's mistake, not a corrupt document,
            // and a folio that vanished is better than a panic in an export.
            return (NumberFormat::Decimal, page_index as u32 + 1);
        };
        (
            run.format,
            run.start
                .saturating_add((page_index - run.first_page) as u32),
        )
    }

    /// Which section `page_index` belongs to, by name (spec 0074).
    ///
    /// `starts` is the same list [`Document::page_assignment`] and [`Document::folio_runs`] take, and
    /// this is pure over numbers for the same reason they are.
    ///
    /// A section runs from its anchor's page until the next section's anchor — the rule spec 0072
    /// stated when it made a section a *marker* rather than a container — so this is the placed
    /// section with the greatest start page at or before `page_index`. `None` before the first one:
    /// a half-title ahead of every section belongs to no section, and a running head there prints
    /// nothing rather than borrowing chapter one's name.
    ///
    /// Two sections on one page collapse to the **later authored** one, which is the precedence
    /// [`Document::page_assignment`] and [`Document::folio_runs`] already give and for the same
    /// reason: nothing re-sorts the authored list, so "later in the list" is the only tie-break that
    /// does not depend on where the anchors happened to land.
    pub fn section_at(&self, starts: &[Option<usize>], page_index: usize) -> Option<&str> {
        self.sections
            .iter()
            .zip(starts)
            .filter_map(|(section, start)| Some(((*start)?, section)))
            .filter(|(start, _)| *start <= page_index)
            // `max_by_key` yields the *last* of several equal maxima, which is the tie-break above.
            .max_by_key(|(start, _)| *start)
            .map(|(_, section)| section.name.as_str())
    }

    /// [`Document::folio_in`] against a fresh derivation — see [`Document::folio_runs`].
    pub fn folio(&self, starts: &[Option<usize>], page_index: usize) -> String {
        Document::folio_in(&self.folio_runs(starts), page_index)
    }

    /// Every numeral format a folio in this document can be written in (spec 0073).
    ///
    /// For the font subset, and it is deliberately **not** conditioned on where the sections landed:
    /// a format is in the set if any section states it, whether or not that section is placed today.
    /// Embedding seven roman letters nobody draws costs a few hundred bytes; failing to embed one
    /// that is drawn is a `.notdef` box in a press file.
    ///
    /// [`NumberFormat::Decimal`] is always present, because the pages ahead of the first section
    /// that states a folio are numbered in it — and because that is what keeps every existing
    /// document's subset, and therefore its exported bytes, exactly where they were.
    pub fn folio_formats(&self) -> BTreeSet<NumberFormat> {
        let mut out = BTreeSet::from([NumberFormat::Decimal]);
        out.extend(
            self.sections
                .iter()
                .filter_map(|s| s.folio)
                .map(|f| f.format),
        );
        out
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
        doc.validate_components()?;
        Ok(doc)
    }

    /// Refuse a document carrying a component definition this build cannot lay out (spec 0054).
    ///
    /// Runs on load rather than at measurement, because layout has no error channel by design and
    /// a definition this build half-understands would otherwise produce geometry that goes to a
    /// printer. The key a definition is filed under is also checked against the name inside it: a
    /// mismatch means `Block::Component { def }` would resolve one and validate the other.
    pub fn validate_components(&self) -> Result<(), LoadError> {
        for (key, def) in &self.components {
            def.validate().map_err(LoadError::ComponentDef)?;
            if &def.name != key {
                return Err(LoadError::ComponentDef(ComponentDefError::NameMismatch {
                    key: key.clone(),
                    def: def.name.clone(),
                }));
            }
        }
        Ok(())
    }

    /// Every definition this document can resolve: the bundled two, plus its own.
    ///
    /// A document definition of the same name shadows a bundled one — which is what lets a
    /// publisher restyle the stat block without a new name, and is why the bundled library is
    /// built first and then extended.
    pub fn component_library(&self) -> ComponentLibrary {
        let mut lib = builtin_components();
        lib.extend(self.components.iter().map(|(k, v)| (k.clone(), v.clone())));
        lib
    }

    /// Resolve this document's [`Document::requires`] against the packs installed under `root`
    /// (spec 0056).
    pub fn resolve_packs(&self, root: &std::path::Path) -> Result<ResolvedPacks, LoadError> {
        resolve(&self.requires, root)
    }

    /// Fold resolved packs into this document, and clear the requirements they satisfied.
    ///
    /// Resolution is a *document transformation* rather than an argument threaded into layout, and
    /// that is the load-bearing choice here. If layout took an optional pack set, the default would
    /// be "no packs", and a caller who forgot would lay a book out in the default face — the silent
    /// fallback this spec exists to prevent. Instead a document either has been resolved (its
    /// `requires` is empty) or has not, and [`quill_layout_engine::lay_out`] refuses the latter.
    ///
    /// Precedence, least to most specific: **bundled < packs < the document's own.** A pack
    /// defining `statblock` is a legitimate restyle of the bundled one; the document's own beats
    /// both, because the document is the thing being edited.
    pub fn apply_packs(&mut self, packs: &ResolvedPacks) -> Result<(), LoadError> {
        let pack_components = packs.components()?;
        // Merged under the document's own, not over: `extend` would let a pack silently replace a
        // definition the author wrote in this very file.
        for (name, def) in pack_components {
            self.components.entry(name).or_insert(def);
        }
        for (name, style) in packs.styles().paragraph {
            self.styles.paragraph.entry(name).or_insert(style);
        }
        self.requires.clear();
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Spec 0054: a malformed or newer-versioned definition is refused at *load*, with an error
    /// that names it. Layout has no error channel by design, so this is the only place the refusal
    /// can happen — and a definition this build half-understands would otherwise produce geometry
    /// that goes to a printer.
    #[test]
    fn a_malformed_component_definition_is_refused_by_name() {
        let manifest = |def: &str| {
            let mut value = serde_json::to_value(Document::sample()).expect("serialize");
            let components: serde_json::Value =
                serde_json::from_str(&format!(r#"{{"clock":{def}}}"#)).expect("definition json");
            value
                .as_object_mut()
                .unwrap()
                .insert("components".into(), components);
            value.to_string()
        };

        // A version this build does not understand.
        let err = Document::from_json(&manifest(
            r#"{"version":99,"name":"clock","sections":[{"source":"title","style":"body","shape":"text"}]}"#,
        ))
        .expect_err("a newer definition version must be refused");
        assert!(matches!(err, LoadError::ComponentDef(_)), "{err:?}");
        let text = err.to_string();
        assert!(text.contains("clock"), "the error must name it: {text}");
        assert!(text.contains("99"), "and say what it found: {text}");

        // No sections at all.
        let err = Document::from_json(&manifest(r#"{"name":"clock","sections":[]}"#))
            .expect_err("a definition with no sections must be refused");
        assert!(err.to_string().contains("clock"), "{err}");

        // Filed under one name, calling itself another: `Block::Component { def }` resolves the
        // key, so this would validate one definition and lay out a different one.
        let err = Document::from_json(&manifest(
            r#"{"name":"cloque","sections":[{"source":"title","style":"body","shape":"text"}]}"#,
        ))
        .expect_err("a key/name mismatch must be refused");
        assert!(err.to_string().contains("cloque"), "{err}");

        // And the well-formed one loads.
        let doc = Document::from_json(&manifest(
            r#"{"name":"clock","sections":[{"source":"title","style":"body","shape":"text"}]}"#,
        ))
        .expect("a well-formed definition loads");
        assert!(doc.components.contains_key("clock"));
        assert!(
            doc.component_library().contains_key(STATBLOCK_COMPONENT),
            "a document's own definitions extend the bundled library rather than replacing it"
        );
    }

    /// A manifest that carries no definitions is byte-identical to one written before they
    /// existed — which is what makes spec 0054 additive and keeps `FORMAT_VERSION` at 3.
    #[test]
    fn a_document_with_no_component_definitions_serializes_none() {
        let json = Document::sample().to_json().expect("serialize");
        assert!(
            !json.contains("\"components\""),
            "an empty component library must not appear in the manifest"
        );
    }

    #[test]
    fn sample_round_trips_through_json() {
        let doc = Document::sample();
        let json = doc.to_json().expect("serialize");
        let back = Document::from_json(&json).expect("deserialize");
        assert_eq!(doc, back);
    }

    #[test]
    fn the_panel_wire_form_is_pinned_to_its_pre_0062_names() {
        // Spec 0062 renamed the Rust surface and deliberately did NOT rename the on-disk one: the
        // tag and the field retire with `Block::Panel` itself, which is a bundled specialization of
        // `Block::Component`. A migration to a name that is already scheduled for removal is one
        // nobody should write. Enforced rather than remembered.
        let block = Block::Panel {
            id: BlockId(1),
            panel: Panel {
                name: "Astrolabe".into(),
                ..Panel::default()
            },
            color: Color::Gray { v: 0.0 },
        };
        let json = serde_json::to_string(&block).expect("serialize");
        assert!(
            json.contains(r#""kind":"stat_block""#),
            "the wire tag must stay `stat_block`: {json}"
        );
        assert!(
            json.contains(r#""stat":"#),
            "the wire field must stay `stat`: {json}"
        );

        // And a block written before the rename still loads, which is what the pin is for. The
        // literal is the pre-0062 wire form typed out, not a re-serialization of the value above:
        // a round-trip through today's code would agree with itself whatever the names became.
        let v3 = r#"{"kind":"stat_block","id":7,"stat":{"name":"Astrolabe"},"color":{"space":"gray","v":0.0}}"#;
        let back: Block = serde_json::from_str(v3).expect("a pre-0062 block must load");
        let Block::Panel { id, panel, .. } = &back else {
            panic!("expected a panel, got {back:?}")
        };
        assert_eq!(*id, BlockId(7));
        assert_eq!(panel.name, "Astrolabe");
        // `FORMAT_VERSION` has since moved to 4, for runs (spec 0063) and not for the rename —
        // which is exactly why the tag and field above are asserted directly rather than through
        // the version number. A pre-0062 block deserializes without a migration because its
        // *block* wire form never changed.
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
                weight: Weight::REGULAR,
                italic: false,
                list: None,
                font_size_pt: 8.0,
                leading_pt: 9.5,
                align: TextAlign::Left,
                space_before_pt: 0.0,
                space_after_pt: 0.0,
                indent: Indent::ZERO,
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
                weight: Weight::REGULAR,
                italic: false,
                list: None,
                font_size_pt: 13.5,
                leading_pt: 16.0,
                align: TextAlign::Left,
                space_before_pt: 6.0,
                space_after_pt: 3.0,
                indent: Indent::ZERO,
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

    // --- Sections (spec 0072) -----------------------------------------------------------------

    /// Two masters, two blocks, and a section anchored to the second.
    fn doc_with_a_section() -> Document {
        let mut doc = doc_with_two_masters();
        let anchor = doc.content[1].id();
        doc.sections = vec![Section {
            name: "Chapter One".into(),
            start: anchor,
            master: Some("opener".into()),
            folio: None,
        }];
        doc
    }

    #[test]
    fn a_section_writes_its_master_onto_the_page_its_anchor_landed_on() {
        // The derivation, as arithmetic: the whole engine-side mechanism is this function plus a
        // page number read off the laid-out pages.
        let doc = doc_with_a_section();
        assert_eq!(
            doc.page_assignment(&[Some(0)])[0].master.as_deref(),
            Some("opener")
        );

        // And it reaches wherever the anchor went, growing the list as needed. Pages before it are
        // untouched entries, which resolve to the document default.
        let pages = doc.page_assignment(&[Some(3)]);
        assert_eq!(pages.len(), 4);
        assert_eq!(pages[3].master.as_deref(), Some("opener"));
        assert!(pages[..3].iter().all(|p| p.master.is_none()));
        assert_eq!(
            doc.master_in(&pages, 0).map(|m| m.name.as_str()),
            Some("body")
        );
        assert_eq!(
            doc.master_in(&pages, 3).map(|m| m.name.as_str()),
            Some("opener")
        );
    }

    #[test]
    fn a_section_that_is_not_placed_or_names_no_master_assigns_nothing() {
        // Two ways a section contributes nothing, and neither is an error: its anchor was deleted
        // (so nothing derived a start), or it exists only to be named — which is what spec 0074's
        // running head will use it for.
        let mut doc = doc_with_a_section();
        assert_eq!(doc.page_assignment(&[None]), doc.pages);

        doc.sections[0].master = None;
        assert_eq!(doc.page_assignment(&[Some(2)]), doc.pages);
    }

    #[test]
    fn a_section_wins_over_a_positional_entry_for_the_same_page() {
        // The one precedence decision in the increment. A document carrying both is a document
        // whose positional entry is the stale statement of the same intent — it was written when
        // the chapter opened on page 0 and did not move when the chapter did.
        let mut doc = doc_with_a_section();
        doc.pages = vec![
            PageOverride {
                master: Some("opener".into()),
            },
            PageOverride { master: None },
        ];
        let pages = doc.page_assignment(&[Some(1)]);
        assert_eq!(
            pages[1].master.as_deref(),
            Some("opener"),
            "the section's page"
        );
        assert_eq!(
            pages[0].master.as_deref(),
            Some("opener"),
            "and an authored entry the section does not reach is left exactly as authored"
        );

        // Where they collide, the section wins.
        let pages = doc.page_assignment(&[Some(0)]);
        doc.sections[0].master = Some("body".into());
        let collided = doc.page_assignment(&[Some(0)]);
        assert_eq!(pages[0].master.as_deref(), Some("opener"));
        assert_eq!(collided[0].master.as_deref(), Some("body"));
    }

    #[test]
    fn sections_round_trip_through_the_manifest() {
        let doc = doc_with_a_section();
        let json = doc.to_json().expect("save");
        let back = Document::from_json(&json).expect("load");
        assert_eq!(back, doc, "a document with sections must equal itself");
        assert_eq!(back.sections[0].start, doc.content[1].id());
        assert_eq!(back.sections[0].name, "Chapter One");
    }

    #[test]
    fn the_empty_cases_round_trip_too() {
        // Spec 0053's lesson: a new model surface owes its empty case a test, because that is the
        // case every existing document is in. Two of them here — no sections, and no blocks at all
        // to anchor one to.
        let doc = Document::sample();
        let json = doc.to_json().expect("save");
        assert!(
            !json.contains("\"sections\""),
            "an empty list must not reach the manifest: it would move the `/ID` of every \
             document in existence. {json}"
        );
        assert_eq!(Document::from_json(&json).expect("load"), doc);

        let mut empty = Document::sample();
        empty.content.clear();
        empty.sections = vec![Section {
            name: "Front matter".into(),
            // A block id that is not in the document — the anchor was deleted, or never existed.
            start: BlockId(999),
            master: None,
            folio: None,
        }];
        let back = Document::from_json(&empty.to_json().expect("save")).expect("load");
        assert_eq!(back, empty, "a document with no blocks still round-trips");
        assert_eq!(back.page_assignment(&[None]), Vec::new());
    }

    // --- Forced page breaks (spec 0080) -------------------------------------------------------

    #[test]
    fn a_break_round_trips_and_stays_out_of_the_manifest_when_absent() {
        // Spec 0053's lesson, and the empty case is every document in existence: a document that
        // breaks nowhere must not write a `breaks` key, because doing so would move the `/ID` of
        // every file quill has ever written.
        let plain = Document::sample();
        let json = plain.to_json().expect("save");
        assert!(
            !json.contains("\"breaks\""),
            "a document with no break must not write `breaks`: {json}"
        );

        let mut doc = Document::sample();
        doc.assign_missing_block_ids().expect("ids");
        doc.breaks = vec![
            PageBreak {
                before: doc.content[1].id(),
                kind: BreakKind::Recto,
            },
            // The common case says nothing about its kind, which is what makes `Page` the default
            // that matters: it is the one an author writes by writing the entry.
            PageBreak {
                before: doc.content[0].id(),
                kind: BreakKind::Page,
            },
        ];
        let json = doc.to_json().expect("save");
        let back = Document::from_json(&json).expect("load");
        assert_eq!(back, doc, "a document with breaks must equal itself");
        assert_eq!(back.breaks[0].kind, BreakKind::Recto);

        // A break naming a block the document does not hold round-trips as readily, and so does a
        // document with no blocks at all to anchor one to: a dangling anchor is a state the model
        // holds, not an error it refuses (spec 0072's posture).
        let mut dangling = Document::sample();
        dangling.content.clear();
        dangling.breaks = vec![PageBreak {
            before: BlockId(999),
            kind: BreakKind::Verso,
        }];
        assert_eq!(
            Document::from_json(&dangling.to_json().expect("save")).expect("load"),
            dangling
        );
    }

    /// Parity, at the one place it is decided, and in all three directions — including the one that
    /// bounds the flow: with facing pages off there are no versos, so every kind accepts every page.
    #[test]
    fn a_break_kind_accepts_pages_by_parity() {
        assert!(BreakKind::Page.accepts(0, true) && BreakKind::Page.accepts(1, true));
        assert!(BreakKind::Recto.accepts(0, true), "page 0 is a recto");
        assert!(!BreakKind::Recto.accepts(1, true));
        assert!(BreakKind::Recto.accepts(2, true));
        assert!(!BreakKind::Verso.accepts(0, true));
        assert!(BreakKind::Verso.accepts(1, true));
        // …and it is `is_recto` deciding, not a second parity rule written here.
        for page in 0..6 {
            assert_eq!(BreakKind::Recto.accepts(page, true), is_recto(page, true));
            assert_eq!(BreakKind::Verso.accepts(page, true), !is_recto(page, true));
        }
        // Facing pages off: a single-sided document has no spread to mirror across, which is the
        // answer `Margins::left_right` already gives — and it is what stops a `Verso` break
        // searching for a page that cannot exist.
        for page in 0..6 {
            assert!(BreakKind::Recto.accepts(page, false));
            assert!(BreakKind::Verso.accepts(page, false));
        }
        // `None` is not a break at all, which is how a book file declines the one it would get.
        assert!(!BreakKind::None.is_break());
        assert!(BreakKind::Page.is_break() && BreakKind::Recto.is_break());
        assert_eq!(BreakKind::default(), BreakKind::Page);
    }

    // --- Folio formats and restart (spec 0073) ------------------------------------------------

    /// Roman front matter, arabic body — the derivation, as arithmetic, with nothing laid out.
    #[test]
    fn a_section_numbers_its_own_run_of_pages() {
        let mut doc = doc_with_two_masters();
        let front = doc.content[0].id();
        let body = doc.content[1].id();
        doc.sections = vec![
            Section {
                name: "Front matter".into(),
                start: front,
                master: None,
                folio: Some(Folio {
                    format: NumberFormat::LowerRoman,
                    restart_at: Some(1),
                }),
            },
            Section {
                name: "Body".into(),
                start: body,
                master: None,
                folio: Some(Folio {
                    format: NumberFormat::Decimal,
                    restart_at: Some(1),
                }),
            },
        ];

        // Front matter on pages 0..4, body from page 4.
        let starts = [Some(0), Some(4)];
        let folios: Vec<String> = (0..8).map(|p| doc.folio(&starts, p)).collect();
        assert_eq!(
            folios,
            ["i", "ii", "iii", "iv", "1", "2", "3", "4"],
            "the body restarts at 1 and the front matter counts in roman"
        );
    }

    #[test]
    fn a_section_that_states_no_restart_keeps_counting() {
        // A format change without a restart: the appendix keeps the book's count and only changes
        // how it is written. This is why `format` and `restart_at` are two fields.
        let mut doc = doc_with_two_masters();
        let appendix = doc.content[1].id();
        doc.sections = vec![Section {
            name: "Appendix".into(),
            start: appendix,
            master: None,
            folio: Some(Folio {
                format: NumberFormat::UpperRoman,
                restart_at: None,
            }),
        }];
        let starts = [Some(3)];
        assert_eq!(doc.folio(&starts, 2), "3");
        assert_eq!(
            doc.folio(&starts, 3),
            "IV",
            "page 3 was going to be `4`, and it is still the fourth page"
        );
        assert_eq!(doc.folio(&starts, 4), "V");
    }

    #[test]
    fn a_document_with_no_sections_is_numbered_exactly_as_it_was() {
        // The case every existing document is in, asserted rather than assumed: arabic, one-based,
        // from page 0 — the single expression `{page}` resolved to before this increment.
        let doc = Document::sample();
        assert_eq!(
            doc.folio_runs(&[]),
            vec![FolioRun {
                first_page: 0,
                format: NumberFormat::Decimal,
                start: 1
            }]
        );
        for page in 0..200 {
            assert_eq!(doc.folio(&[], page), (page + 1).to_string());
        }

        // And a section that states no folio is not a numbering statement at all: it divides the
        // document, not the pagination.
        let sectioned = doc_with_a_section();
        assert_eq!(sectioned.folio_runs(&[Some(2)]), doc.folio_runs(&[]));
    }

    #[test]
    fn an_unplaced_section_numbers_nothing_and_a_collision_takes_the_later_one() {
        let mut doc = doc_with_two_masters();
        let a = doc.content[0].id();
        let b = doc.content[1].id();
        doc.sections = vec![
            Section {
                name: "Deleted anchor".into(),
                start: a,
                master: None,
                folio: Some(Folio {
                    format: NumberFormat::LowerRoman,
                    restart_at: Some(1),
                }),
            },
            Section {
                name: "Part two".into(),
                start: b,
                master: None,
                folio: Some(Folio {
                    format: NumberFormat::UpperAlpha,
                    restart_at: Some(1),
                }),
            },
        ];

        // An anchor that was not placed contributes no run — the posture a dangling master name
        // already has.
        assert_eq!(doc.folio(&[None, Some(1)], 0), "1");
        assert_eq!(doc.folio(&[None, Some(1)], 1), "A");

        // Both on the same page: one run, the later section's, so the list stays one rule per page.
        let runs = doc.folio_runs(&[Some(1), Some(1)]);
        assert_eq!(runs.len(), 2, "page 0's default, then the one at page 1");
        assert_eq!(runs[1].format, NumberFormat::UpperAlpha);
    }

    #[test]
    fn page_ranges_coalesce_into_runs() {
        use NumberFormat::Decimal;
        let d = |n: u32| (Decimal, n);
        assert_eq!(coalesce_folios(&[]), "");
        assert_eq!(coalesce_folios(&[d(42)]), "42");
        // Two consecutive pages are a range, not a list: `42–43` is what an index sets.
        assert_eq!(coalesce_folios(&[d(42), d(43)]), "42\u{2013}43");
        assert_eq!(
            coalesce_folios(&[d(42), d(43), d(44), d(45), d(47)]),
            "42\u{2013}45, 47"
        );
        assert_eq!(coalesce_folios(&[d(1), d(3), d(5)]), "1, 3, 5");
    }

    #[test]
    fn a_range_never_spans_a_folio_format_change() {
        // **The case that looks correct on an arabic-only document.** Pages 8 and 9 of the file are
        // consecutive *indices*; if the second is the first page of a section set in arabic they
        // print `viii` and `1`, and joining them would print a range that does not exist.
        use NumberFormat::{Decimal, LowerRoman};
        assert_eq!(
            coalesce_folios(&[(LowerRoman, 7), (LowerRoman, 8), (Decimal, 1), (Decimal, 2)]),
            "vii\u{2013}viii, 1\u{2013}2"
        );
        // And a format change alone breaks it, even where the numbers *would* have been
        // consecutive — `iv` and `5` are not a range whatever the arithmetic says.
        assert_eq!(coalesce_folios(&[(LowerRoman, 4), (Decimal, 5)]), "iv, 5");
    }

    #[test]
    fn a_restart_breaks_a_range_between_consecutive_pages() {
        // The other half of spec 0073's numbering, and the one a format comparison alone would
        // miss: same format, consecutive page indices, but the section restarted the count. Pages
        // printing `9` then `1` are not `9–1`.
        use NumberFormat::Decimal;
        assert_eq!(
            coalesce_folios(&[(Decimal, 8), (Decimal, 9), (Decimal, 1), (Decimal, 2)]),
            "8\u{2013}9, 1\u{2013}2"
        );
    }

    #[test]
    fn a_folio_is_its_own_numbering_formatted() {
        // `folio_in` and `folio_number_in` must be one answer, or an index would coalesce a run of
        // pages whose printed folios said something else. Asserted over a two-run document rather
        // than a default one, so the roman arm is actually exercised.
        let runs = vec![
            FolioRun {
                first_page: 0,
                format: NumberFormat::LowerRoman,
                start: 1,
            },
            FolioRun {
                first_page: 4,
                format: NumberFormat::Decimal,
                start: 1,
            },
        ];
        for page in 0..12 {
            let (format, n) = Document::folio_number_in(&runs, page);
            assert_eq!(Document::folio_in(&runs, page), format.format(n));
        }
        // Including the degenerate empty-list path both share.
        let (format, n) = Document::folio_number_in(&[], 6);
        assert_eq!(Document::folio_in(&[], 6), format.format(n));
        assert_eq!(format.format(n), "7");
    }

    #[test]
    fn the_subset_alphabet_covers_everything_the_formatter_can_produce() {
        // The font-subset guarantee, proved against the formatter rather than against a table:
        // sweep every ordinal a book could plausibly reach (and the two fallback cases) and assert
        // no character comes out that `alphabet()` did not promise. A missing character here is a
        // `.notdef` box in a press file.
        for format in [
            NumberFormat::Decimal,
            NumberFormat::LowerAlpha,
            NumberFormat::UpperAlpha,
            NumberFormat::LowerRoman,
            NumberFormat::UpperRoman,
        ] {
            let alphabet = format.alphabet();
            for n in (0..5000).chain([9999, 100_000, u32::MAX]) {
                for ch in format.format(n).chars() {
                    assert!(
                        alphabet.contains(&ch),
                        "{format:?} writes {n} as {:?}, and {ch:?} is not in its alphabet",
                        format.format(n)
                    );
                }
            }
        }

        // And the alphabets are what a reader would expect, so a mistake in the sweep above cannot
        // pass by being empty.
        assert!(NumberFormat::LowerRoman
            .alphabet()
            .is_superset(&"ivxlcdm".chars().collect()));
        assert!(NumberFormat::UpperRoman
            .alphabet()
            .is_superset(&"IVXLCDM".chars().collect()));
        assert!(NumberFormat::LowerAlpha
            .alphabet()
            .is_superset(&('a'..='z').collect()));
        assert_eq!(
            NumberFormat::Decimal.alphabet(),
            ('0'..='9').collect(),
            "decimal is the ten digits and nothing else"
        );
    }

    #[test]
    fn the_formats_in_use_always_include_decimal() {
        // Decimal is unconditional because the pages ahead of the first section are numbered in it
        // — and because that is what keeps every existing document's subset where it was.
        let doc = Document::sample();
        assert_eq!(doc.folio_formats(), BTreeSet::from([NumberFormat::Decimal]));

        let mut doc = doc_with_a_section();
        doc.sections[0].folio = Some(Folio {
            format: NumberFormat::LowerRoman,
            restart_at: Some(1),
        });
        assert_eq!(
            doc.folio_formats(),
            BTreeSet::from([NumberFormat::Decimal, NumberFormat::LowerRoman])
        );
    }

    // --- The layout-time token set (spec 0074) --------------------------------------------------

    /// One parser, and it is the reason the resolver and the font-subset collector cannot disagree
    /// about what a token is. Both halves are asserted: what it finds, and what it leaves alone.
    #[test]
    fn the_token_parser_finds_every_token_and_walks_past_everything_else() {
        assert_eq!(
            StaticToken::scan("{section} — {heading:2} — {page}")
                .into_iter()
                .map(|(_, t)| t)
                .collect::<Vec<_>>(),
            vec![
                StaticToken::Section,
                StaticToken::Heading { level: 2 },
                StaticToken::Page
            ]
        );

        // The ranges are what the resolver splices on, so they are part of the contract.
        let scanned = StaticToken::scan("a{page}b");
        assert_eq!(scanned, vec![(1..7, StaticToken::Page)]);

        // A group that spells no token is not a token. That is what makes an *older* build meeting a
        // *newer* token loud rather than silent: it prints `{section}` on the page, visibly wrong,
        // rather than something plausible and wrong.
        for text in [
            "{}",
            "{pages}",
            "{Page}",
            "{heading}",
            "{heading:}",
            "{heading:x}",
            "{heading:1",
            "no tokens at all",
            "{heading:999}", // past a u8: not a level this build can mean
        ] {
            assert!(
                StaticToken::scan(text).is_empty(),
                "{text:?} spells no token"
            );
        }
    }

    /// What each token can draw, which is the collector's whole question. Asked of the document, so
    /// a book that uses none of this carries nothing extra.
    #[test]
    fn a_token_contributes_the_text_it_can_actually_print() {
        let mut doc = doc_with_a_section();
        let id = doc.content[0].id();
        doc.content[0] = Block::heading(1, "The Ruined Keep", Color::Gray { v: 0.0 });
        doc.content[0].set_id(id);

        // A folio is arithmetic: its alphabet, not a string.
        let page = StaticToken::Page.contributes(&doc);
        assert!(page.runs.is_empty());
        assert!(page.chars.is_superset(&('0'..='9').collect()));

        // A section name and a heading are authored text, and travel as runs so their ligatures are
        // cut into the subset (spec 0068).
        assert_eq!(
            StaticToken::Section.contributes(&doc).runs,
            vec!["Chapter One".to_string()]
        );
        assert_eq!(
            StaticToken::Heading { level: 1 }.contributes(&doc).runs,
            vec!["The Ruined Keep".to_string()]
        );
        assert!(StaticToken::Section.contributes(&doc).chars.is_empty());
    }

    /// Which section a page belongs to — pure arithmetic over the start pages, as
    /// [`Document::folio_runs`] is, and with the same tie-break.
    #[test]
    fn a_page_belongs_to_the_last_section_that_opened_at_or_before_it() {
        let mut doc = doc_with_a_section();
        doc.sections = vec![
            Section {
                name: "Front matter".into(),
                start: doc.content[0].id(),
                master: None,
                folio: None,
            },
            Section {
                name: "Body".into(),
                start: doc.content[1].id(),
                master: None,
                folio: None,
            },
        ];
        let starts = [Some(2), Some(5)];

        // Ahead of every section: no section, and no borrowing of the first one's name.
        assert_eq!(doc.section_at(&starts, 0), None);
        assert_eq!(doc.section_at(&starts, 1), None);
        // A section runs until the next one's anchor.
        assert_eq!(doc.section_at(&starts, 2), Some("Front matter"));
        assert_eq!(doc.section_at(&starts, 4), Some("Front matter"));
        assert_eq!(doc.section_at(&starts, 5), Some("Body"));
        assert_eq!(doc.section_at(&starts, 500), Some("Body"));
        // An unplaced section contributes nothing, exactly as it contributes no master assignment.
        assert_eq!(doc.section_at(&[Some(2), None], 500), Some("Front matter"));
        // Two on one page: the later authored one wins, as it does for a master and for a folio.
        assert_eq!(doc.section_at(&[Some(3), Some(3)], 3), Some("Body"));
    }

    #[test]
    fn a_folio_round_trips_and_stays_out_of_the_manifest_when_absent() {
        let mut doc = doc_with_a_section();
        assert!(
            !doc.to_json().expect("save").contains("\"folio\""),
            "a section that states no folio must not write the key"
        );

        doc.sections[0].folio = Some(Folio {
            format: NumberFormat::LowerRoman,
            restart_at: Some(1),
        });
        let back = Document::from_json(&doc.to_json().expect("save")).expect("load");
        assert_eq!(back, doc);
        assert_eq!(
            back.sections[0].folio.expect("folio").format,
            NumberFormat::LowerRoman
        );

        // And the default is expressible without either key, because a `Folio` is what a section
        // that changes only the restart wants too.
        let bare: Folio = serde_json::from_str("{}").expect("an empty folio object");
        assert_eq!(bare, Folio::default());
        assert_eq!(bare.format, NumberFormat::Decimal);
        assert_eq!(bare.restart_at, None);
    }

    // --- Footnotes (spec 0077) -----------------------------------------------------------------

    #[test]
    fn a_footnote_round_trips_and_stays_out_of_the_manifest_when_absent() {
        // Spec 0053's lesson again, and the empty case is every document in existence: a document
        // with no note must not write a `footnotes` key, because doing so would move the `/ID` of
        // every file quill has ever written.
        let plain = Document::sample();
        let json = plain.to_json().expect("save");
        assert!(
            !json.contains("\"footnotes\""),
            "a document with no note must not write `footnotes`: {json}"
        );

        let mut doc = Document::sample();
        doc.footnotes = vec![Footnote::plain("a note", Color::Gray { v: 0.0 })];
        doc.content.push(Block::body_runs(
            vec![Run::plain("a claim"), Run::footnote(BlockId(0))],
            Color::Gray { v: 0.0 },
        ));
        doc.assign_missing_block_ids().expect("ids");
        let note = doc.footnotes[0].id;
        if let Some(Block::Body { runs, .. }) = doc.content.last_mut() {
            runs[1] = Run::footnote(note);
        }
        let json = doc.to_json().expect("save");
        let back = Document::from_json(&json).expect("load");
        assert_eq!(back, doc, "a document with a footnote must equal itself");
        assert_eq!(
            back.content.last().expect("anchoring block").runs()[1]
                .source
                .note(),
            Some(note)
        );

        // An anchor naming a note the document does not hold round-trips as readily: a dangling
        // anchor is a state the model can hold, not an error it refuses.
        let mut dangling = Document::sample();
        dangling.content = vec![Block::body_runs(
            vec![Run::footnote(BlockId(999))],
            Color::Gray { v: 0.0 },
        )];
        dangling.assign_missing_block_ids().expect("ids");
        assert_eq!(
            Document::from_json(&dangling.to_json().expect("save")).expect("load"),
            dangling
        );
    }

    #[test]
    fn a_footnote_shares_the_one_id_space_and_a_collision_is_refused() {
        let mut doc = Document::sample();
        doc.footnotes = vec![
            Footnote::plain("one", Color::Gray { v: 0.0 }),
            Footnote::plain("two", Color::Gray { v: 0.0 }),
        ];
        doc.assign_missing_block_ids().expect("ids");
        let ids: BTreeSet<u64> = doc
            .content
            .iter()
            .map(|b| b.id().0)
            .chain(doc.footnotes.iter().map(|f| f.id.0))
            .collect();
        assert_eq!(
            ids.len(),
            doc.content.len() + doc.footnotes.len(),
            "every block and every note has its own id"
        );

        // And a collision is refused rather than repaired, exactly as two blocks are: a shared
        // identity means an anchor can resolve to the wrong thing.
        let clash = doc.content[0].id();
        doc.footnotes[0].id = clash;
        assert!(matches!(
            doc.assign_missing_block_ids(),
            Err(LoadError::DuplicateBlockId(n)) if n == clash.0
        ));
    }

    #[test]
    fn a_note_block_is_synthesised_once_and_carries_its_own_number() {
        // One synthesis site, read by the engine and by the font-subset collector alike. What it
        // produces is the assertion, because everything downstream is derived from it.
        let mut doc = Document::sample();
        doc.content = vec![Block::body_runs(
            vec![Run::plain("a claim"), Run::footnote(BlockId(0))],
            Color::Gray { v: 0.0 },
        )];
        doc.footnotes = vec![Footnote::plain("the note", Color::Gray { v: 0.0 })];
        doc.next_block_id = 0;
        doc.assign_missing_block_ids().expect("ids");
        let note = doc.footnotes[0].id;
        if let Some(Block::Body { runs, .. }) = doc.content.last_mut() {
            runs[1] = Run::footnote(note);
        }

        let blocks = doc.footnote_blocks();
        assert_eq!(blocks.len(), 1);
        assert_eq!(blocks[0].id(), note, "the note block carries the note's id");
        let runs = blocks[0].runs();
        assert_eq!(
            runs[0].source,
            RunSource::Footnote { note },
            "the note opens with the same generated run its anchor carries"
        );
        assert_eq!(runs[1].text, FOOTNOTE_NUMBER_SUFFIX);
        assert_eq!(runs[2].text, "the note");
        assert_eq!(
            doc.footnote_numbers().get(&note).map(String::as_str),
            Some("1")
        );

        // A note nothing anchors is not synthesised and is not numbered: it is not an error, it is
        // simply not placed.
        doc.footnotes
            .push(Footnote::plain("orphan", Color::Gray { v: 0.0 }));
        doc.assign_missing_block_ids().expect("ids");
        assert_eq!(doc.footnote_blocks().len(), 1);
        assert_eq!(doc.footnote_numbers().len(), 1);
    }

    #[test]
    fn a_footnote_anchor_stores_the_marker_rather_than_a_number() {
        // Spec 0041's rule, applied to the second generated run. A stored number is stale the moment
        // a note is inserted above it, and it is what an older build would print — plausibly and
        // wrongly — where `[?]` reads as unfinished.
        let run = Run::footnote(BlockId(7));
        assert_eq!(run.text, UNRESOLVED_REFERENCE);
        assert_eq!(run.source.note(), Some(BlockId(7)));
        assert_eq!(run.source.target(), None, "a note is not a link target");
        assert_eq!(run.source.referent(), Some(BlockId(7)));
    }

    #[test]
    fn a_footnote_number_contributes_its_own_alphabet_and_not_the_folios() {
        // Property 4 of spec 0076's class, applied honestly rather than by copying: there is one
        // answer to what a *folio* can draw and one answer to what a *note number* can draw, and
        // they are different questions. Tying the note's alphabet to the book's page numbering would
        // let roman front matter change what a footnote can print.
        let mut doc = Document::sample();
        doc.sections = vec![Section {
            name: "Front".into(),
            start: doc.content[0].id(),
            master: None,
            folio: Some(Folio {
                format: NumberFormat::LowerRoman,
                restart_at: Some(1),
            }),
        }];
        let note = RunSource::Footnote { note: BlockId(1) }.contributes(&doc);
        assert_eq!(note.chars, NumberFormat::Decimal.alphabet());
        assert!(
            !note.chars.contains(&'v'),
            "a roman folio must not widen what a footnote number can draw"
        );
        assert_eq!(note.runs, vec![UNRESOLVED_REFERENCE.to_string()]);

        // …while a cross-reference in the same document does carry the roman alphabet, which is what
        // makes the two genuinely different answers rather than an oversight.
        let reference = RunSource::Reference { target: BlockId(1) }.contributes(&doc);
        assert!(reference.chars.contains(&'v'));
    }

    // --- Cross-references (spec 0076) ---------------------------------------------------------

    #[test]
    fn a_cross_reference_round_trips_and_stays_out_of_the_manifest_when_absent() {
        // Spec 0053's lesson, and the empty case is every document in existence: an authored run
        // must not write a `source` key, because doing so would move the `/ID` of every file quill
        // has ever written.
        let plain = Document::sample();
        let json = plain.to_json().expect("save");
        assert!(
            !json.contains("\"source\""),
            "an authored run must not write `source`: {json}"
        );
        assert_eq!(Document::from_json(&json).expect("load"), plain);

        let mut doc = Document::sample();
        let target = doc.content[0].id();
        doc.content.push(Block::body_runs(
            vec![
                Run::plain("See page "),
                Run::reference(target),
                Run::plain("."),
            ],
            Color::Gray { v: 0.0 },
        ));
        doc.assign_missing_block_ids().expect("ids");
        let json = doc.to_json().expect("save");
        let back = Document::from_json(&json).expect("load");
        assert_eq!(
            back, doc,
            "a document with a cross-reference must equal itself"
        );
        assert_eq!(
            back.content.last().expect("the referring block").runs()[1]
                .source
                .target(),
            Some(target)
        );

        // And a reference to a block that is not in the document round-trips as readily: a dangling
        // target is a state the model can hold, not an error it refuses.
        let mut dangling = Document::sample();
        dangling.content = vec![Block::body_runs(
            vec![Run::reference(BlockId(999))],
            Color::Gray { v: 0.0 },
        )];
        dangling.assign_missing_block_ids().expect("ids");
        assert_eq!(
            Document::from_json(&dangling.to_json().expect("save")).expect("load"),
            dangling
        );
    }

    #[test]
    fn a_reference_run_stores_the_marker_rather_than_a_page_number() {
        // Spec 0041's rule, applied to a run: what a reference resolves to is derived, never stored.
        // A stored number is stale the moment anything is edited, and one that was right an edit ago
        // is worse than none — and it is what an older build would print, plausibly and wrongly.
        let run = Run::reference(BlockId(7));
        assert_eq!(run.text, UNRESOLVED_REFERENCE);
        assert!(!run.source.is_authored());
        assert_eq!(run.source.target(), Some(BlockId(7)));
        assert_eq!(Run::plain("x").source.target(), None);
    }

    #[test]
    fn what_a_cross_reference_can_draw_is_the_folio_alphabet_and_the_marker() {
        // The structural half of spec 0074's mechanism, at its second site: `contributes` is what
        // the font-subset collector reads, and it is an exhaustive match, so a run source that
        // reaches the resolver without reaching the collector does not compile.
        let mut doc = Document::sample();
        doc.sections = vec![Section {
            name: "Front".into(),
            start: doc.content[0].id(),
            master: None,
            folio: Some(Folio {
                format: NumberFormat::LowerRoman,
                restart_at: Some(1),
            }),
        }];
        let carried = RunSource::Reference {
            target: doc.content[0].id(),
        }
        .contributes(&doc);
        assert_eq!(carried.runs, vec![UNRESOLVED_REFERENCE.to_string()]);
        for ch in "ivxlcdm0123456789".chars() {
            assert!(carried.chars.contains(&ch), "{ch:?} must be carried");
        }
        // There is one answer to what a folio can draw, and both askers get it.
        assert_eq!(carried.chars, StaticToken::Page.contributes(&doc).chars);

        // An authored run contributes nothing, which is what keeps every existing document's subset
        // — and therefore its exported bytes — exactly where it was.
        assert_eq!(RunSource::Authored.contributes(&doc), TokenText::default());
    }

    #[test]
    fn reference_targets_are_collected_once_per_block_referred_to() {
        let mut doc = Document::sample();
        let a = doc.content[0].id();
        doc.content.push(Block::body_runs(
            vec![Run::reference(a), Run::plain(" and "), Run::reference(a)],
            Color::Gray { v: 0.0 },
        ));
        doc.content.push(Block::body_runs(
            vec![Run::reference(a)],
            Color::Gray { v: 0.0 },
        ));
        doc.assign_missing_block_ids().expect("ids");
        assert_eq!(reference_targets(&doc.content), BTreeSet::from([a]));
        assert!(
            reference_targets(&Document::sample().content).is_empty(),
            "and a document with none skips the derivation outright"
        );
    }

    #[test]
    fn a_stat_block_round_trips_through_the_manifest() {
        let mut doc = Document::sample();
        doc.content.push(Block::Panel {
            id: BlockId::UNASSIGNED,
            panel: Panel {
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
        let Block::Panel { panel, .. } = &back.content[2] else {
            panic!("expected a stat block")
        };
        assert_eq!(panel.attributes.len(), 2);
        assert_eq!(panel.name, "Goblin");
    }

    #[test]
    fn the_built_in_stat_block_styles_exist_and_are_ordered() {
        // A stat block resolves these three by name. If one were missing it would fall back to
        // `body` and the block would come out looking like a paragraph.
        let sheet = StyleSheet::default();
        for name in [PANEL_TITLE_STYLE, PANEL_ATTR_STYLE, PANEL_BODY_STYLE] {
            assert!(sheet.paragraph.contains_key(name), "missing `{name}`");
        }
        assert!(
            sheet.paragraph[PANEL_TITLE_STYLE].font_size_pt
                > sheet.paragraph[PANEL_BODY_STYLE].font_size_pt,
            "the name must be set larger than the prose"
        );
    }

    // --- Master static alignment and parity (spec 0047) ---------------------------------------

    /// A 100 pt-wide band starting 50 pt in, on a 432 pt page.
    const BAND: Rect = Rect {
        x_pt: 50.0,
        y_pt: 600.0,
        w_pt: 100.0,
        h_pt: 12.0,
    };

    #[test]
    fn left_alignment_is_exactly_the_pre_0047_placement() {
        // The default must be the old behaviour to the point: a static that says nothing about
        // alignment is drawn from its rect's left edge, on every page, as it always was.
        for page in 0..4 {
            assert_eq!(StaticAlign::Left.x_for(BAND, 20.0, page, true), 50.0);
            assert_eq!(StaticAlign::default(), StaticAlign::Left);
        }
    }

    #[test]
    fn a_line_is_centred_and_right_aligned_within_its_rect() {
        // 100 pt band, 20 pt line: centred at 50 + 40, flush right at 50 + 80.
        assert!((StaticAlign::Center.x_for(BAND, 20.0, 0, true) - 90.0).abs() < 0.01);
        assert!((StaticAlign::Right.x_for(BAND, 20.0, 0, true) - 130.0).abs() < 0.01);
    }

    #[test]
    fn inside_and_outside_swap_with_page_parity() {
        // The defect this spec exists to fix, at the arithmetic level: two adjacent pages must not
        // resolve to the same x.
        let recto = StaticAlign::Outside.x_for(BAND, 20.0, 0, true);
        let verso = StaticAlign::Outside.x_for(BAND, 20.0, 1, true);
        assert!((recto - 130.0).abs() < 0.01, "recto outside is flush right");
        assert!((verso - 50.0).abs() < 0.01, "verso outside is flush left");
        assert_ne!(recto, verso);
        // And `inside` is its mirror image, not a second name for the same edge.
        assert_eq!(StaticAlign::Inside.x_for(BAND, 20.0, 0, true), 50.0);
        assert_eq!(StaticAlign::Inside.x_for(BAND, 20.0, 1, true), 130.0);
    }

    #[test]
    fn parity_resolution_is_off_when_facing_pages_is() {
        // Same rule as `Margins::left_right`: a single-sided document has no spread to mirror
        // across, so every page is a recto.
        for page in 0..4 {
            assert_eq!(
                StaticAlign::Outside.x_for(BAND, 20.0, page, false),
                StaticAlign::Right.x_for(BAND, 20.0, page, false)
            );
        }
    }

    #[test]
    fn an_over_long_line_keeps_its_aligned_edge_and_overflows_inward() {
        // A right-aligned static wider than its rect must run into the page, not off the trim.
        // Clamping to the rect's left edge would push the overflow the other way — into the
        // guillotine, silently.
        let x = StaticAlign::Right.x_for(BAND, 160.0, 0, true);
        assert!(x < BAND.x_pt, "expected leftward overflow, got {x}");
        assert!((x + 160.0 - (BAND.x_pt + BAND.w_pt)).abs() < 0.01);
    }

    #[test]
    fn a_mirrored_rect_flips_about_the_page_centre_on_a_verso() {
        let setup = PageSetup::default(); // 432 pt wide, facing pages on
        let s = MasterStatic::Text {
            rect: BAND,
            text: "{page}".into(),
            color: Color::Gray { v: 0.0 },
            style: None,
            align: StaticAlign::Outside,
            mirror: true,
        };
        assert_eq!(
            s.rect_on(0, &setup),
            BAND,
            "a recto keeps the authored rect"
        );
        // 432 - (50 + 100) = 282.
        assert_eq!(s.rect_on(1, &setup).x_pt, 282.0);
        assert_eq!(s.rect_on(1, &setup).w_pt, BAND.w_pt);
    }

    #[test]
    fn an_unmirrored_static_and_a_single_sided_document_keep_the_authored_rect() {
        let mut setup = PageSetup::default();
        let plain = MasterStatic::text(BAND, "x", Color::Gray { v: 0.0 });
        for page in 0..4 {
            assert_eq!(plain.rect_on(page, &setup), BAND, "unmirrored never moves");
        }
        let mirrored = MasterStatic::Text {
            rect: BAND,
            text: "x".into(),
            color: Color::Gray { v: 0.0 },
            style: None,
            align: StaticAlign::Outside,
            mirror: true,
        };
        setup.facing_pages = false;
        for page in 0..4 {
            assert_eq!(mirrored.rect_on(page, &setup), BAND);
        }
        // An image static has no `mirror` field at all and is never moved.
        let image = MasterStatic::Image {
            rect: BAND,
            asset: "bg".into(),
        };
        assert_eq!(image.rect_on(1, &PageSetup::default()), BAND);
    }

    #[test]
    fn margins_and_statics_agree_about_which_side_is_outside() {
        // Both call `is_recto`, so this cannot drift — asserted anyway, because the whole point of
        // the shared helper is that a folio and a margin never disagree about a page.
        let m = Margins {
            top_pt: 0.0,
            bottom_pt: 0.0,
            inside_pt: 54.0,
            outside_pt: 40.0,
        };
        for page in 0..4 {
            let (left, _right) = m.left_right(page, true);
            let inside_is_left = left == m.inside_pt;
            assert_eq!(inside_is_left, is_recto(page, true));
            let x = StaticAlign::Inside.x_for(BAND, 20.0, page, true);
            assert_eq!(
                x == BAND.x_pt,
                inside_is_left,
                "page {page}: the inside edge must be the same side for both"
            );
        }
    }

    #[test]
    fn the_new_static_fields_round_trip_and_default_out_of_the_manifest() {
        let mut doc = Document::sample();
        doc.master_pages = vec![MasterPage {
            statics: vec![
                MasterStatic::Text {
                    rect: BAND,
                    text: PAGE_TOKEN.into(),
                    color: Color::Gray { v: 0.0 },
                    style: None,
                    align: StaticAlign::Outside,
                    mirror: true,
                },
                MasterStatic::text(BAND, "plain", Color::Gray { v: 0.0 }),
            ],
            ..MasterPage::plain("body")
        }];
        doc.default_master = Some("body".into());
        let json = doc.to_json().expect("save");
        assert_eq!(Document::from_json(&json).expect("load"), doc);
        // A default-aligned, unmirrored static writes neither key. This is what keeps a migrated
        // v2 document's manifest text — and therefore the `/ID` its export is hashed from —
        // exactly where it was (spec 0047).
        let masters = serde_json::to_string(&doc.master_pages).expect("masters");
        assert_eq!(masters.matches("\"align\"").count(), 1, "{masters}");
        assert_eq!(masters.matches("\"mirror\"").count(), 1, "{masters}");
    }

    #[test]
    fn an_empty_page_list_is_omitted_from_the_manifest() {
        // `skip_serializing_if` keeps the text manifest readable and git-diffable — the property
        // BlockId's plain-number encoding exists to protect.
        let json = Document::sample().to_json().expect("save");
        assert!(!json.contains("\"pages\""));
    }
}
