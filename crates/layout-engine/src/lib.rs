//! Layout engine: turns a document into positioned pages.
//!
//! The real engine is incremental and dependency-tracked (see the plan) so that editing one
//! text thread reflows only affected pages. This scaffold lays content out naively — stacking
//! text and image blocks, paginating when a block would exceed the page height — so downstream
//! crates compile and the export pipeline has something to consume. Uses `quill-text-layout`
//! for line breaking.

use std::collections::{BTreeMap, BTreeSet};

mod session;

pub use session::{LayoutResult, LayoutSession, LayoutStats};

use quill_core_model::{
    Asset, Block, BlockId, Color, Document, Margins, MasterPage, MasterStatic, PageSetup,
    ParagraphStyle, Rect, StatBlock, StyleSheet, TextAlign, PAGE_TOKEN, STATBLOCK_ATTR_STYLE,
    STATBLOCK_BODY_STYLE, STATBLOCK_TITLE_STYLE,
};
use quill_text_layout::{justify_paragraph_hyphenated, Alignment, Hyphenator, Line, RunMetrics};

/// A positioned rectangular region that content flows into. The layout engine fills a frame
/// top-to-bottom; a block that would pass the frame's bottom edge overflows — to the next page in
/// this increment, to the next frame in a thread once threading lands (spec 0019 incr. 2).
///
/// Introduced as a seam **at parity**: the frame [`lay_out`] uses is [`Frame::full_page`] (the whole
/// trim area at the origin), so the produced pages — and every export golden test — are byte-identical
/// to the pre-frame implicit column. A frame with a non-zero origin, a narrower width, or a shorter
/// height is the new capability, exercised via [`lay_out_in_frame`].
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Frame {
    pub rect: Rect,
}

impl Frame {
    /// The whole-page content frame: the entire trim area at the origin. This is the frame
    /// [`lay_out`] uses, so its output is identical to the pre-frame implicit column. Margins/insets
    /// and multiple frames per page are follow-ups (spec 0019 non-goals).
    pub fn full_page(page_setup: &PageSetup) -> Frame {
        Frame {
            rect: Rect {
                x_pt: 0.0,
                y_pt: 0.0,
                w_pt: page_setup.trim.w_pt,
                h_pt: page_setup.trim.h_pt,
            },
        }
    }
}

/// An ordered chain of [`Frame`]s that content flows through — a *thread* (spec 0019 incr. 2).
///
/// Content fills `frames[0]` top-to-bottom; a block that overflows the current frame continues into
/// the **next** frame in the thread (two columns on a page, a story that runs box-to-box), and onto
/// a new page — restarting at `frames[0]` — once the thread's frames are exhausted. A single-frame
/// thread reproduces the incr. 1 [`lay_out_in_frame`] behavior exactly (parity), so the same set of
/// frames is repeated per page on overflow. The frames live in `layout-engine` and are supplied by
/// the caller; persisting author-defined threads into the `.tpub` model is a later increment.
#[derive(Debug, Clone, PartialEq)]
pub struct Thread {
    pub frames: Vec<Frame>,
}

impl Thread {
    /// A left-to-right chain of `count` equal-width columns spanning the trim area, separated by
    /// `gutter_pt` of horizontal space, each the full trim height at `y = 0` (spec 0020). Content
    /// laid into the returned thread via [`lay_out_in_thread`] fills the leftmost column
    /// top-to-bottom, then the next column, and onto a new page once the last column fills.
    ///
    /// A single column (`count == 1`) is the whole trim area — identical to [`Frame::full_page`]
    /// (the gutter is then irrelevant, there being no interior gutter). Derived from `PageSetup`
    /// like [`Frame::full_page`]: no authored field, no serialized-model change. Panics if
    /// `count == 0` (a thread must have at least one frame — loud failure over a silent empty
    /// thread).
    pub fn columns(page_setup: &PageSetup, count: usize, gutter_pt: f32) -> Thread {
        assert!(
            count >= 1,
            "a multi-column thread needs at least one column"
        );
        let trim_w = page_setup.trim.w_pt;
        let trim_h = page_setup.trim.h_pt;
        // Total gutter is between columns only: (count - 1) gutters. What's left divides evenly.
        let col_w = (trim_w - (count - 1) as f32 * gutter_pt) / count as f32;
        // A gutter wide enough to consume the trim yields a non-positive column width. Fail loudly
        // rather than emit negative-width, overlapping frames that would silently corrupt layout
        // downstream (break_paragraph against a negative width) — see CLAUDE.md's press-safety rule.
        assert!(
            col_w > 0.0,
            "gutter {gutter_pt} pt too large for {count} columns in {trim_w} pt trim (col_w = {col_w})"
        );
        let frames = (0..count)
            .map(|i| Frame {
                rect: Rect {
                    x_pt: i as f32 * (col_w + gutter_pt),
                    y_pt: 0.0,
                    w_pt: col_w,
                    h_pt: trim_h,
                },
            })
            .collect();
        Thread { frames }
    }
}

/// Compute an image's placed size in points from its pixel dimensions and DPI, preserving aspect
/// ratio and scaling down to fit `content_width` when the natural width is wider. See spec 0009.
///
/// Falls back to a square at `content_width` when pixel dimensions or DPI are unknown (`0`), so
/// documents authored before pixel info was captured still lay out.
fn image_size(asset: &Asset, content_width: f32) -> (f32, f32) {
    if asset.px_w == 0 || asset.px_h == 0 || asset.dpi <= 0.0 {
        return (content_width, content_width); // legacy square placeholder
    }
    let natural_w = asset.px_w as f32 / asset.dpi * 72.0;
    let natural_h = asset.px_h as f32 / asset.dpi * 72.0;
    if natural_w > content_width {
        let scale = content_width / natural_w;
        (content_width, natural_h * scale)
    } else {
        (natural_w, natural_h)
    }
}

/// A block positioned on a page.
#[derive(Debug, Clone, PartialEq)]
pub enum PlacedBlock {
    Text {
        frame: Rect,
        /// The block this came from, or [`BlockId::UNASSIGNED`] for master furniture, which is not
        /// content and has no identity (spec 0040).
        ///
        /// Placed geometry had no way back to the block that produced it, so "which page is this
        /// heading on" — what a table of contents and a PDF bookmark both need — was unanswerable
        /// from a `Vec<LaidOutPage>`.
        source: BlockId,
        /// Broken lines, each carrying its inter-word justification adjustment (spec 0017 incr. 2).
        lines: Vec<Line>,
        color: Color,
        /// The size the text was measured at. Carried here because `PlacedBlock` is all the writer
        /// and the screen renderer see: without it they would have to re-derive the size from the
        /// document, and any disagreement would put glyphs in the wrong place (spec 0028).
        font_size_pt: f32,
        /// Baseline-to-baseline distance, likewise carried rather than re-derived.
        leading_pt: f32,
    },
    Image {
        frame: Rect,
        /// See [`PlacedBlock::Text::source`].
        source: BlockId,
        asset_id: String,
    },
    /// A filled and/or stroked rectangle — a rule, a border, a tinted panel (spec 0037).
    ///
    /// Introduced at parity: nothing in the model produces one yet. It exists because a stat block
    /// (spec 0038) is a tinted, ruled, padded box, and until now the engine could emit only text
    /// and images — there was no way to draw a line.
    ///
    /// Both `fill` and `stroke` are optional and a rectangle with neither draws nothing, so a
    /// caller can express a rule (stroke only), a tint (fill only) or a panel (both) without three
    /// variants.
    Rect {
        frame: Rect,
        fill: Option<Color>,
        stroke: Option<Stroke>,
    },
}

/// A rectangle's outline.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Stroke {
    pub color: Color,
    /// Line width in **points**, not pixels. A hairline that silently becomes device-dependent is
    /// the classic press bug here: 0.25 pt is a real hairline at 2400 dpi and invisible at 300.
    pub width_pt: f32,
}

/// A laid-out page.
#[derive(Debug, Clone, PartialEq, Default)]
pub struct LaidOutPage {
    /// Zero-based position in the document.
    ///
    /// A page had no identity before spec 0029, which is why nothing could vary by page: a running
    /// head cannot say "42" and a verso cannot mirror a recto if the page does not know which one it
    /// is. Incremental layout (spec 0031) also needs it to report *which* pages changed.
    pub index: usize,
    /// Content that flowed onto this page.
    pub blocks: Vec<PlacedBlock>,
    /// Content the page's template contributed — running heads, folios, background art.
    ///
    /// Kept separate from `blocks` rather than merged, for two reasons: it is drawn first (so master
    /// art sits behind flowed content), and incremental relayout can leave it alone, since it does
    /// not depend on where the text happened to break.
    pub statics: Vec<PlacedBlock>,
}

/// One heading, and the page it landed on (spec 0040).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct HeadingEntry {
    pub id: BlockId,
    pub level: u8,
    pub text: String,
    /// Zero-based page index. A table of contents prints `page_index + 1`, the same one-based
    /// number [`MasterStatic`]'s `{page}` token resolves to.
    pub page_index: usize,
}

/// Which page each heading landed on, in document order.
///
/// Derived from the **laid-out pages** rather than accumulated during pagination, and that is the
/// load-bearing decision. An incremental pass reuses whole pages from the previous layout
/// (spec 0031), so an index built up as blocks were placed would be missing every heading on a
/// reused page — and would be missing them precisely when the document had just been edited, which
/// is always. Deriving it from the final page vector makes it correct by construction for the
/// incremental path and the cold path alike, at the cost of one walk over the pages.
///
/// A heading appearing more than once in the page vector reports its **first** page. That cannot
/// happen today, because a block is placed whole into one frame and never split (see the roadmap's
/// known issues) — but a TOC entry and a bookmark both mean "where does this start", so the rule is
/// stated here rather than left to depend on an invariant that is expected to change.
///
/// Master furniture is skipped: it carries [`BlockId::UNASSIGNED`] and is not content.
pub fn heading_index(doc: &Document, pages: &[LaidOutPage]) -> Vec<HeadingEntry> {
    let mut seen: BTreeSet<BlockId> = BTreeSet::new();
    let mut out = Vec::new();
    for page in pages {
        for placed in &page.blocks {
            let PlacedBlock::Text { source, .. } = placed else {
                continue;
            };
            if !source.is_assigned() || !seen.insert(*source) {
                continue;
            }
            let Some(Block::Heading { level, text, .. }) = doc.block(*source) else {
                continue;
            };
            out.push(HeadingEntry {
                id: *source,
                level: *level,
                text: text.clone(),
                page_index: page.index,
            });
        }
    }
    out
}

/// Supplies the geometry and static content of each page.
///
/// Two things in the pagination loop previously made every page identical: the page-advance branch
/// reset the frame cursor into the *same* frame list, and a page had no index to vary anything by.
/// This trait is the seam where both stop being true — a master page is an implementation of it.
///
/// Introduced **at parity**: [`UniformTemplate`] returns the same frames for every page and no
/// statics, which is exactly the previous behavior. Making the geometry vary per page is the new
/// capability, exercised in tests and used by spec 0030's authored master pages.
pub trait PageTemplate {
    /// The frames content flows through on `page_index`, in order.
    ///
    /// Must be non-empty; a page with nowhere to put content would silently drop it.
    fn frames(&self, page_index: usize) -> Vec<Frame>;

    /// Content the template draws on `page_index`, independent of what flows there.
    fn statics(&self, _page_index: usize) -> Vec<PlacedBlock> {
        Vec::new()
    }
}

/// A template that gives every page the same frames and no static content.
///
/// The parity implementation: layout through this is identical to the pre-template engine.
#[derive(Debug, Clone, PartialEq)]
pub struct UniformTemplate {
    pub thread: Thread,
}

impl UniformTemplate {
    pub fn new(thread: Thread) -> Self {
        UniformTemplate { thread }
    }
}

impl PageTemplate for UniformTemplate {
    fn frames(&self, _page_index: usize) -> Vec<Frame> {
        self.thread.frames.clone()
    }
}

/// A [`PageTemplate`] built from a document's page setup and its master pages (specs 0030, 0035).
///
/// This is where authored layout finally reaches the engine: margins, column count and gutter come
/// from the master governing each page, and that master's statics are stamped onto the page with
/// `{page}` resolved.
///
/// With no master and zero margins it produces exactly [`Frame::full_page`], so a document that
/// declares neither lays out as it always did.
///
/// The master is resolved **per page** rather than once, because spec 0035 lets a document assign a
/// different master to page 0 (a chapter opener, a title page) than to the body. Resolution itself
/// lives on [`Document::master_for`] so the engine and the model cannot disagree about which master
/// a page has.
pub struct DocumentTemplate<'a> {
    doc: &'a Document,
    page_setup: &'a PageSetup,
    styles: &'a StyleSheet,
}

impl<'a> DocumentTemplate<'a> {
    pub fn new(doc: &'a Document) -> DocumentTemplate<'a> {
        DocumentTemplate {
            doc,
            page_setup: &doc.page_setup,
            styles: &doc.styles,
        }
    }

    /// The master governing `page_index`, or none.
    fn master(&self, page_index: usize) -> Option<&'a MasterPage> {
        self.doc.master_for(page_index)
    }

    /// The margins in effect on `page_index`: the master's if it sets them, else the document's.
    fn margins(&self, page_index: usize) -> Margins {
        self.master(page_index)
            .and_then(|m| m.margins)
            .unwrap_or(self.page_setup.margins)
    }

    /// The text area of `page_index`, after margins are taken off the trim.
    fn content_rect(&self, page_index: usize) -> Rect {
        let m = self.margins(page_index);
        let (left, right) = m.left_right(page_index, self.page_setup.facing_pages);
        Rect {
            x_pt: left,
            y_pt: m.top_pt,
            w_pt: (self.page_setup.trim.w_pt - left - right).max(0.0),
            h_pt: (self.page_setup.trim.h_pt - m.top_pt - m.bottom_pt).max(0.0),
        }
    }
}

impl PageTemplate for DocumentTemplate<'_> {
    fn frames(&self, page_index: usize) -> Vec<Frame> {
        let area = self.content_rect(page_index);
        let master = self.master(page_index);
        let columns = master.map(|m| m.columns).unwrap_or(1).max(1);
        let gutter = master.map(|m| m.gutter_pt).unwrap_or(0.0);

        let col_w = (area.w_pt - (columns - 1) as f32 * gutter) / columns as f32;
        // A gutter wide enough to consume the text area would give negative-width, overlapping
        // frames. Rather than panic on an authored document — a user can type any gutter — fall
        // back to a single column, which is wrong-looking but recoverable and obvious on screen.
        if col_w <= 0.0 {
            return vec![Frame { rect: area }];
        }
        (0..columns)
            .map(|i| Frame {
                rect: Rect {
                    x_pt: area.x_pt + i as f32 * (col_w + gutter),
                    y_pt: area.y_pt,
                    w_pt: col_w,
                    h_pt: area.h_pt,
                },
            })
            .collect()
    }

    fn statics(&self, page_index: usize) -> Vec<PlacedBlock> {
        let Some(master) = self.master(page_index) else {
            return Vec::new();
        };
        master
            .statics
            .iter()
            .map(|s| match s {
                MasterStatic::Text {
                    rect,
                    text,
                    color,
                    style,
                } => {
                    // One-based, because that is what a reader sees printed on a page.
                    let resolved = text.replace(PAGE_TOKEN, &(page_index + 1).to_string());
                    let ps = style
                        .as_deref()
                        .and_then(|n| self.styles.paragraph.get(n).copied())
                        .unwrap_or_default();
                    PlacedBlock::Text {
                        frame: *rect,
                        // Furniture is not content: it has no block, so no identity.
                        source: BlockId::UNASSIGNED,
                        // Master furniture is a single line at a fixed position; it is not flowed,
                        // so it is not broken. A running head that overflows its rect is an
                        // authoring problem, and one that is visible on screen.
                        lines: vec![Line {
                            text: resolved,
                            space_adjust_pt: 0.0,
                        }],
                        color: *color,
                        font_size_pt: ps.font_size_pt,
                        leading_pt: ps.leading_pt,
                    }
                }
                MasterStatic::Image { rect, asset } => PlacedBlock::Image {
                    frame: *rect,
                    source: BlockId::UNASSIGNED,
                    asset_id: asset.clone(),
                },
            })
            .collect()
    }
}

/// Lay a document out into pages, flowing its content into the whole-page frame
/// ([`Frame::full_page`]). Paginates: starts a new page when a block would pass the frame's bottom
/// edge (the full trim height here). Returns at least one page (even if the document is empty).
///
/// Text is broken to fit the frame width using the caller-supplied `metrics` (the embedded font in
/// the export path) at [`BODY_FONT_SIZE_PT`] — see `specs/0015-text-metrics-line-breaking.md` and
/// spec 0016 for the shift to run-based measurement.
///
/// `hyphenator` supplies the legal in-word break points (spec 0018): the export path passes an
/// en-US `hypher`-backed hyphenator so long words break at syllable boundaries; tests pass
/// [`quill_text_layout::NoHyphenator`] for the spec-0017 parity path.
pub fn lay_out(
    doc: &Document,
    metrics: &impl RunMetrics,
    hyphenator: &impl Hyphenator,
) -> Vec<LaidOutPage> {
    // Authored layout reaches the engine here (spec 0030): margins, columns and master furniture
    // come from the document. With no master and zero margins this is exactly `Frame::full_page`,
    // so a document declaring neither lays out as it always did.
    lay_out_with_template(
        &doc.content,
        &doc.assets,
        &doc.styles,
        &DocumentTemplate::new(doc),
        metrics,
        hyphenator,
    )
}

/// Flow `content` into a single [`Frame`], paginating vertically. Equivalent to
/// [`lay_out_in_thread`] over a one-frame thread: text wraps to the frame width, blocks are
/// positioned at the frame origin, and a block overflows to a new page (repeating the same frame
/// geometry) when it would pass the frame's bottom edge — see spec 0019.
///
/// `assets` resolves [`Block::Image`] ids; unknown ids are skipped.
pub fn lay_out_in_frame(
    content: &[Block],
    assets: &[Asset],
    styles: &StyleSheet,
    frame: &Frame,
    metrics: &impl RunMetrics,
    hyphenator: &impl Hyphenator,
) -> Vec<LaidOutPage> {
    lay_out_in_thread(
        content,
        assets,
        styles,
        &Thread {
            frames: vec![*frame],
        },
        metrics,
        hyphenator,
    )
}

/// An id → [`Asset`] lookup, built once per layout pass. Borrows the document's assets rather than
/// copying them; layout never outlives the document it is laying out.
type AssetIndex<'a> = BTreeMap<&'a str, &'a Asset>;

/// The intrinsic size of a block once broken/measured for a given frame width, plus the payload
/// needed to place it. Re-computed against each candidate frame the block is tried in, since both
/// text wrapping and image sizing depend on the frame width (spec 0019 incr. 2).
#[derive(Clone)]
pub(crate) enum Measured {
    Text {
        lines: Vec<Line>,
        color: Color,
        style: ParagraphStyle,
    },
    Image {
        asset_id: String,
        /// The sized placement width (spec 0009), which may be narrower than the frame.
        width: f32,
    },
    /// A composite: a decorated panel containing several independently-styled text runs
    /// (spec 0038).
    ///
    /// Every offset is **relative to the block's own origin**, so the whole thing can be placed by
    /// adding the flow cursor once. Measuring it as a unit is what makes it a single block to
    /// pagination — it moves whole to the next frame when it does not fit, exactly as any other
    /// block does.
    Panel {
        fill: Option<Color>,
        stroke: Option<Stroke>,
        parts: Vec<PanelPart>,
        /// Horizontal rules inside the panel, as offsets from its top. What gives the panel
        /// internal structure — without them the sections run together and it reads as a tinted
        /// paragraph rather than as a stat block.
        rules: Vec<f32>,
    },
}

/// One styled run inside a [`Measured::Panel`], positioned relative to the panel's top-left.
#[derive(Debug, Clone, PartialEq)]
pub(crate) struct PanelPart {
    pub dx_pt: f32,
    pub dy_pt: f32,
    pub w_pt: f32,
    pub lines: Vec<Line>,
    pub color: Color,
    pub font_size_pt: f32,
    pub leading_pt: f32,
}

/// Break/size `block` against a frame of `width` points, returning the placement payload and its
/// height. `None` means "skip this block" — currently only an unresolved [`Block::Image`] id.
///
/// Called once per candidate frame in [`lay_out_in_thread`]'s placement loop so a block that
/// advances into a different-width frame re-wraps (text) / re-fits (image) to that frame's width.
pub(crate) fn measure_block(
    block: &Block,
    width: f32,
    assets: &AssetIndex<'_>,
    styles: &StyleSheet,
    metrics: &impl RunMetrics,
    hyphenator: &impl Hyphenator,
) -> Option<(Measured, f32)> {
    match block {
        Block::Heading { text, color, .. } | Block::Body { text, color, .. } => {
            // Size, leading and alignment now come from the document's stylesheet rather than from
            // crate constants (spec 0028). Before this, every block in every document was set at
            // body size — a heading differed from body text only by being ragged-left.
            let style = styles.resolve(block);
            let align = match style.align {
                TextAlign::Justified => Alignment::Justified,
                TextAlign::Left => Alignment::Left,
            };
            let lines = justify_paragraph_hyphenated(
                text,
                width,
                style.font_size_pt,
                align,
                metrics,
                hyphenator,
            );
            // Space before/after is part of the block's occupied height, so pagination accounts for
            // it: a heading that only fits on the next page because of its space-above must break
            // there, or it would sit at the very top with its space silently swallowed.
            let height = lines.len() as f32 * style.leading_pt
                + style.space_before_pt
                + style.space_after_pt;
            Some((
                Measured::Text {
                    lines,
                    color: *color,
                    style,
                },
                height,
            ))
        }
        Block::StatBlock { stat, color, .. } => Some(measure_stat_block(
            stat, *color, width, styles, metrics, hyphenator,
        )),
        Block::Image { asset, .. } => {
            // Resolve the asset id. If not found, skip this block (no panic).
            let asset_rec = *assets.get(asset.as_str())?;
            // Size the image at its true aspect ratio, scaling down to fit the frame width when
            // wider. See spec 0009.
            let (w, h) = image_size(asset_rec, width);
            Some((
                Measured::Image {
                    asset_id: asset.clone(),
                    width: w,
                },
                h,
            ))
        }
    }
}

/// Padding between a stat block's panel edge and its text, on all four sides.
///
/// A constant rather than an authored field: the panel is a built-in component whose whole value is
/// that it looks right with no authoring, and a padding a user could set to zero would let text sit
/// on the rule.
pub const STATBLOCK_PADDING_PT: f32 = 6.0;

/// The panel's background tint. 8% black — enough to read as a panel on paper, far enough inside
/// the ink limit that it can never be the thing that fails preflight.
const STATBLOCK_FILL: Color = Color::Gray { v: 0.92 };

/// The panel's outer rule.
const STATBLOCK_STROKE: Stroke = Stroke {
    color: Color::Gray { v: 0.35 },
    width_pt: 0.75,
};

/// Thickness of the hairlines that separate a stat block's sections.
const SECTION_RULE_PT: f32 = 0.5;

/// Space above and below each section rule.
const SECTION_RULE_GAP_PT: f32 = 2.5;

/// Break a stat block into a padded, tinted, ruled panel of styled runs (spec 0038).
///
/// The sections are laid in the order a table reads them — name, attributes, then the prose
/// sections — each through the same Knuth-Plass path as body text, so a stat block's justification,
/// hyphenation and metrics are the document's and not a second implementation.
fn measure_stat_block(
    stat: &StatBlock,
    color: Color,
    width: f32,
    styles: &StyleSheet,
    metrics: &impl RunMetrics,
    hyphenator: &impl Hyphenator,
) -> (Measured, f32) {
    let inner_w = (width - STATBLOCK_PADDING_PT * 2.0).max(1.0);
    let mut parts: Vec<PanelPart> = Vec::new();
    let mut y = STATBLOCK_PADDING_PT;

    // Name, overview, attributes, details, actions, reactions — the order `StatBlock`'s own doc
    // comment states, which is the order the compact layout this mirrors reads in. Getting it wrong
    // puts the creature's type *after* its armour class, which is visibly not a stat block; the
    // first draft did exactly that and only the render showed it.
    //
    // `Section` marks where a rule goes: after the name, and between each group that follows.
    let mut runs: Vec<(String, &str, bool)> =
        vec![(stat.name.clone(), STATBLOCK_TITLE_STYLE, true)];
    let push_group = |runs: &mut Vec<(String, &str, bool)>, lines: Vec<String>| {
        for (i, line) in lines.into_iter().enumerate() {
            runs.push((line, STATBLOCK_BODY_STYLE, i == 0));
        }
    };
    push_group(&mut runs, stat.overview.clone());
    for (i, (k, v)) in stat.attributes.iter().enumerate() {
        // A colon, not the two spaces this first used: `break_by_width` normalizes every run of
        // inter-word whitespace to a single U+0020, so the intended visual gap collapsed to an
        // ordinary word space and "Armour Class 15 (leather, shield)" read as one sentence. With no
        // bold weight available, punctuation is what distinguishes the key.
        runs.push((format!("{k}: {v}"), STATBLOCK_ATTR_STYLE, i == 0));
    }
    for section in [&stat.details, &stat.actions, &stat.reactions] {
        push_group(&mut runs, section.clone());
    }

    let mut rules: Vec<f32> = Vec::new();
    for (idx, (text, style_name, starts_section)) in runs.into_iter().enumerate() {
        let style = styles
            .paragraph
            .get(style_name)
            .copied()
            .unwrap_or_default();
        let lines = justify_paragraph_hyphenated(
            &text,
            inner_w,
            style.font_size_pt,
            Alignment::Left,
            metrics,
            hyphenator,
        );
        y += style.space_before_pt;
        // A rule separates each section from the one above. Not before the first run, which has
        // the panel's own edge above it.
        if starts_section && idx > 0 {
            y += SECTION_RULE_GAP_PT;
            rules.push(y);
            y += SECTION_RULE_GAP_PT;
        }
        let n = lines.len();
        parts.push(PanelPart {
            dx_pt: STATBLOCK_PADDING_PT,
            dy_pt: y,
            w_pt: inner_w,
            lines,
            color,
            font_size_pt: style.font_size_pt,
            leading_pt: style.leading_pt,
        });
        y += n as f32 * style.leading_pt + style.space_after_pt;
    }

    let height = y + STATBLOCK_PADDING_PT;
    (
        Measured::Panel {
            fill: Some(STATBLOCK_FILL),
            stroke: Some(STATBLOCK_STROKE),
            parts,
            rules,
        },
        height,
    )
}

/// Flow `content` through a [`Thread`]'s frames, paginating across frames and then pages
/// (spec 0019 incr. 2). Content fills the first frame top-to-bottom; a block that overflows the
/// current frame continues into the next frame in the thread, and onto a fresh page — restarting at
/// the first frame — once the thread's frames are exhausted.
///
/// An oversized block (taller than a frame) is placed in an otherwise-empty frame rather than
/// skipping forever — the same "already has content" guard incr. 1 used, now measured per frame. A
/// single-frame thread is exactly [`lay_out_in_frame`] (parity). `assets` resolves [`Block::Image`]
/// ids; unknown ids are skipped. A thread must have at least one frame.
pub fn lay_out_in_thread(
    content: &[Block],
    assets: &[Asset],
    styles: &StyleSheet,
    thread: &Thread,
    metrics: &impl RunMetrics,
    hyphenator: &impl Hyphenator,
) -> Vec<LaidOutPage> {
    lay_out_with_template(
        content,
        assets,
        styles,
        &UniformTemplate::new(thread.clone()),
        metrics,
        hyphenator,
    )
}

/// Flow `content` through pages whose geometry and static content come from `template`
/// (spec 0029).
///
/// This is [`lay_out_in_thread`] generalized: instead of one frame list repeated on every page, each
/// page asks the template what its frames are. With a [`UniformTemplate`] the two are identical.
///
/// Panics if the template hands back a page with no frames — a page with nowhere to put content
/// would silently drop it, and losing content is exactly the class of failure `CLAUDE.md` forbids.
pub fn lay_out_with_template(
    content: &[Block],
    assets: &[Asset],
    styles: &StyleSheet,
    template: &impl PageTemplate,
    metrics: &impl RunMetrics,
    hyphenator: &impl Hyphenator,
) -> Vec<LaidOutPage> {
    flow(
        content,
        assets,
        styles,
        template,
        metrics,
        hyphenator,
        FlowState::start(template),
        &mut NoCache,
        None,
    )
    .pages
}

/// Where the flow had reached at a page boundary — everything needed to resume from there.
///
/// Incremental relayout (spec 0031) resumes from the last checkpoint before an edit instead of
/// restarting at block 0. That is only sound because this is genuinely *all* the state the loop
/// carries: capture the wrong subset and resumed layout silently diverges from a full pass.
#[derive(Debug, Clone, Copy, PartialEq)]
pub(crate) struct FlowState {
    /// Index into `content` of the next block to place.
    pub block_idx: usize,
    pub page_index: usize,
    pub frame_idx: usize,
    pub y: f32,
    pub frame_empty: bool,
}

impl FlowState {
    fn start(template: &impl PageTemplate) -> FlowState {
        FlowState {
            block_idx: 0,
            page_index: 0,
            frame_idx: 0,
            y: frames_for(template, 0)[0].rect.y_pt,
            frame_empty: true,
        }
    }
}

/// Fetch a page's frames, refusing an empty list rather than silently dropping content.
pub(crate) fn frames_for(template: &impl PageTemplate, index: usize) -> Vec<Frame> {
    let frames = template.frames(index);
    assert!(
        !frames.is_empty(),
        "page template produced no frames for page {index}; content would be dropped"
    );
    frames
}

/// Intercepts measurement so a session can serve repeats from cache. See `session.rs`.
pub(crate) trait Measurer {
    fn measure<M: RunMetrics, H: Hyphenator>(
        &mut self,
        block: &Block,
        width: f32,
        assets: &AssetIndex<'_>,
        styles: &StyleSheet,
        metrics: &M,
        hyphenator: &H,
    ) -> Option<(Measured, f32)>;
}

/// The non-caching measurer used by the one-shot path.
pub(crate) struct NoCache;

impl Measurer for NoCache {
    fn measure<M: RunMetrics, H: Hyphenator>(
        &mut self,
        block: &Block,
        width: f32,
        assets: &AssetIndex<'_>,
        styles: &StyleSheet,
        metrics: &M,
        hyphenator: &H,
    ) -> Option<(Measured, f32)> {
        measure_block(block, width, assets, styles, metrics, hyphenator)
    }
}

/// The result of a flow pass: the pages produced plus the state at each page's start.
pub(crate) struct FlowResult {
    pub pages: Vec<LaidOutPage>,
    /// `checkpoints[i]` is the flow state at the moment page `i` began.
    pub checkpoints: Vec<FlowState>,
    /// Set when the flow stopped early because it re-converged with a previous pass. The value is
    /// the page index in *that* pass whose remaining pages are still valid.
    pub resynced_at: Option<usize>,
}

/// Lets an incremental pass stop as soon as it rejoins the previous layout.
///
/// Without this, an edit on page 250 of 500 still *walks* all 250 remaining blocks — cheaply, from
/// cache, but 250 blocks of hashing and lookup is not nothing. Stopping at the boundary is what
/// makes the cost proportional to the edit rather than to the tail of the document.
pub(crate) struct Resync<'a> {
    /// Page-start states from the previous pass.
    pub checkpoints: &'a [FlowState],
    /// Index past the last changed block; before it, nothing can be assumed to match.
    pub last_dirty: usize,
}

/// The pagination loop, resumable from `start`.
///
/// Extracted from `lay_out_with_template` so the one-shot path and the incremental session run
/// *the same code*. Two implementations would drift, and a divergence between full and incremental
/// layout is the worst possible bug here: the document would look different depending on how you
/// arrived at it.
#[allow(clippy::too_many_arguments)]
pub(crate) fn flow(
    content: &[Block],
    assets: &[Asset],
    styles: &StyleSheet,
    template: &impl PageTemplate,
    metrics: &impl RunMetrics,
    hyphenator: &impl Hyphenator,
    start: FlowState,
    measurer: &mut impl Measurer,
    resync: Option<Resync<'_>>,
) -> FlowResult {
    // Build the id → asset index once for the whole pass. `measure_block` runs once per candidate
    // frame per block, and it used to resolve image ids with a linear scan of `assets` — quadratic
    // in an art-heavy document, which is precisely the workload this engine exists for (spec 0026).
    let assets: AssetIndex<'_> = assets.iter().map(|a| (a.id.as_str(), a)).collect();
    let assets = &assets;

    let mut pages: Vec<LaidOutPage> = Vec::new();
    let mut checkpoints: Vec<FlowState> = Vec::new();
    let mut page_index: usize = start.page_index;
    let mut frames = frames_for(template, page_index);
    let mut page = LaidOutPage {
        index: page_index,
        blocks: Vec::new(),
        statics: template.statics(page_index),
    };
    // Which frame on the current page the cursor is filling.
    let mut frame_idx: usize = start.frame_idx;
    // Absolute y cursor, starting at the current frame's top and reset there on each frame advance.
    let mut y: f32 = start.y;
    // Whether the *current* frame has received a block yet — mirrors incr. 1's page-empty guard,
    // now per frame so an oversized block is placed rather than skipped through every frame/page.
    let mut frame_empty = start.frame_empty;
    checkpoints.push(start);

    for (offset, block) in content[start.block_idx..].iter().enumerate() {
        let block_idx = start.block_idx + offset;
        // Advance frames / pages until the block fits, then place it. The block is re-measured
        // against each candidate frame's width (wrapping/sizing depend on it), so a block that
        // advances into a narrower frame re-wraps to that width rather than keeping a stale
        // measurement. Bounded to <= 2 iterations: after one advance the new frame is empty, so the
        // next iteration places (the `frame_empty` guard also places an oversized block rather than
        // looping past every frame).
        loop {
            let frame = frames[frame_idx];
            let Some((measured, height)) =
                measurer.measure(block, frame.rect.w_pt, assets, styles, metrics, hyphenator)
            else {
                break; // unresolved image asset → skip this block (no panic)
            };
            let bottom = frame.rect.y_pt + frame.rect.h_pt;

            if y + height > bottom && !frame_empty {
                // Doesn't fit and the current frame has content → move on before placing.
                if frame_idx + 1 < frames.len() {
                    frame_idx += 1; // next frame on this page
                } else {
                    // Page exhausted. The next page asks the template for its own geometry, rather
                    // than reusing this page's — that is the whole point of the seam.
                    pages.push(page);
                    page_index += 1;
                    frames = frames_for(template, page_index);
                    page = LaidOutPage {
                        index: page_index,
                        blocks: Vec::new(),
                        statics: template.statics(page_index),
                    };
                    frame_idx = 0;
                    // Record where the new page begins, so a later edit can resume from here.
                    let at_boundary = FlowState {
                        block_idx,
                        page_index,
                        frame_idx: 0,
                        y: frames[0].rect.y_pt,
                        frame_empty: true,
                    };
                    checkpoints.push(at_boundary);

                    // If this page begins in exactly the state the previous pass's page of the same
                    // number began in, and nothing past here changed, the rest of the old layout is
                    // still correct — stop, rather than re-deriving pages we already have.
                    if let Some(r) = &resync {
                        if block_idx >= r.last_dirty
                            && r.checkpoints.get(page_index) == Some(&at_boundary)
                        {
                            checkpoints.pop();
                            return FlowResult {
                                pages,
                                checkpoints,
                                resynced_at: Some(page_index),
                            };
                        }
                    }
                }
                y = frames[frame_idx].rect.y_pt;
                frame_empty = true;
                continue; // re-measure against the frame it moved into
            }

            // A block usually yields one placed item; a composite (spec 0038) yields its panel
            // plus one text run per section. Pagination still sees a single block — it moved whole
            // to this frame or not at all.
            let placed: Vec<PlacedBlock> = match measured {
                Measured::Text {
                    lines,
                    color,
                    style,
                } => vec![PlacedBlock::Text {
                    source: block.id(),
                    // The frame starts *below* the style's space-above: that space belongs to this
                    // block's height (so pagination reserves it) but no text is drawn in it.
                    frame: Rect {
                        x_pt: frame.rect.x_pt,
                        y_pt: y + style.space_before_pt,
                        w_pt: frame.rect.w_pt,
                        h_pt: height - style.space_before_pt,
                    },
                    lines,
                    color,
                    // Carried through to the writer. Without this the exported PDF would set every
                    // paragraph at body size regardless of its style, because `PlacedBlock` was the
                    // only thing the writer sees and it did not record how the text was measured.
                    font_size_pt: style.font_size_pt,
                    leading_pt: style.leading_pt,
                }],
                Measured::Image { asset_id, width } => vec![PlacedBlock::Image {
                    source: block.id(),
                    frame: Rect {
                        x_pt: frame.rect.x_pt,
                        y_pt: y,
                        w_pt: width,
                        h_pt: height,
                    },
                    asset_id,
                }],
                Measured::Panel {
                    fill,
                    stroke,
                    parts,
                    rules,
                } => {
                    // The panel first, so it sits behind its own text — the same
                    // decoration-before-content order the writer and the paint list rely on.
                    let mut out = vec![PlacedBlock::Rect {
                        frame: Rect {
                            x_pt: frame.rect.x_pt,
                            y_pt: y,
                            w_pt: frame.rect.w_pt,
                            h_pt: height,
                        },
                        fill,
                        stroke,
                    }];
                    out.extend(rules.into_iter().map(|dy| PlacedBlock::Rect {
                        frame: Rect {
                            x_pt: frame.rect.x_pt + STATBLOCK_PADDING_PT,
                            y_pt: y + dy,
                            w_pt: (frame.rect.w_pt - STATBLOCK_PADDING_PT * 2.0).max(0.0),
                            h_pt: SECTION_RULE_PT,
                        },
                        fill: Some(STATBLOCK_STROKE.color),
                        stroke: None,
                    }));
                    out.extend(parts.into_iter().map(|p| PlacedBlock::Text {
                        source: block.id(),
                        frame: Rect {
                            x_pt: frame.rect.x_pt + p.dx_pt,
                            y_pt: y + p.dy_pt,
                            w_pt: p.w_pt,
                            h_pt: p.lines.len() as f32 * p.leading_pt,
                        },
                        lines: p.lines,
                        color: p.color,
                        font_size_pt: p.font_size_pt,
                        leading_pt: p.leading_pt,
                    }));
                    out
                }
            };
            page.blocks.extend(placed);
            y += height;
            frame_empty = false;
            break;
        }
    }

    // Always emit the last (possibly empty) page so callers receive >= 1 page.
    pages.push(page);
    FlowResult {
        pages,
        checkpoints,
        resynced_at: None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use quill_core_model::{
        Asset, Block, Color, Document, Metadata, PageOverride, PageSetup, Size,
    };
    use quill_text_layout::{Hyphenator, MonospaceRunMetrics, NoHyphenator};
    use quill_text_layout::{BODY_FONT_SIZE_PT, BODY_LINE_HEIGHT_PT};

    /// 0.6 em × 10 pt = 6 pt/char, matching the old `APPROX_CHAR_WIDTH_PT` stand-in so these
    /// pagination tests keep their familiar per-character arithmetic.
    const MONO: MonospaceRunMetrics = MonospaceRunMetrics { em_ratio: 0.6 };

    #[test]
    fn lays_out_sample_into_one_page() {
        // Document::sample() has 2 text blocks (a short heading + a body paragraph that wraps to
        // a few lines) + asset "map1" (referenced by no Block::Image in the sample, so no image
        // block is placed). Content still fits well within one page.
        let pages = lay_out(&Document::sample(), &MONO, &NoHyphenator);
        assert!(!pages.is_empty());
        assert!(!pages[0].blocks.is_empty());
    }

    #[test]
    fn sample_body_wraps_and_justifies() {
        // The CI Ghostscript preflight exports Document::sample() and parses its content stream to
        // exercise the justified-`TJ` path (spec 0017 incr. 2). That only happens if the sample's
        // body paragraph wraps to >= 2 lines, giving an interior line a non-zero adjustment. Guard
        // that invariant here so shortening the sample text can't silently drop the CI coverage.
        let pages = lay_out(&Document::sample(), &MONO, &NoHyphenator);
        // The sample leads with a short heading, then the body paragraph; look for any text block
        // that both wraps (>= 2 lines) and carries a justified (non-zero-adjustment) interior line.
        let wrapped_justified = pages
            .iter()
            .flat_map(|p| &p.blocks)
            .filter_map(|b| match b {
                PlacedBlock::Text { lines, .. } => Some(lines),
                _ => None,
            })
            .any(|lines| lines.len() >= 2 && lines.iter().any(|l| l.space_adjust_pt != 0.0));
        assert!(
            wrapped_justified,
            "sample must contain a wrapped, justified paragraph so CI parses a justified TJ"
        );
    }

    /// The lines of the first `PlacedBlock::Text` found across `pages`.
    fn first_text_lines(pages: &[LaidOutPage]) -> Vec<Line> {
        pages
            .iter()
            .flat_map(|p| &p.blocks)
            .find_map(|b| match b {
                PlacedBlock::Text { lines, .. } => Some(lines.clone()),
                _ => None,
            })
            .expect("a text block")
    }

    /// Breaks the crafted long word in `lay_out_threads_the_hyphenator` in half; nothing else.
    struct HalfStub;
    impl Hyphenator for HalfStub {
        fn hyphenate(&self, word: &str) -> Vec<usize> {
            if word.len() == 100 {
                vec![50]
            } else {
                Vec::new()
            }
        }
    }

    #[test]
    fn lay_out_threads_the_hyphenator() {
        // Proves `lay_out` actually passes its hyphenator down to the breaker (spec 0018 incr. 2).
        // A single 100-char word (600 pt under MONO) overflows the 432 pt frame with NoHyphenator
        // (one long line, no hyphen). With HalfStub it splits at offset 50 — the first line ends in
        // a rendered hyphen — so the two paths must differ.
        let doc = doc_with_blocks(vec![Block::body("z".repeat(100), Color::Gray { v: 0.0 })]);

        let plain = first_text_lines(&lay_out(&doc, &MONO, &NoHyphenator));
        let hyphenated = first_text_lines(&lay_out(&doc, &MONO, &HalfStub));

        assert_eq!(
            plain.len(),
            1,
            "no hyphenation → the word overflows on one line"
        );
        assert!(!plain[0].text.ends_with('-'));
        assert_eq!(
            hyphenated.len(),
            2,
            "HalfStub splits the word across two lines"
        );
        assert!(
            hyphenated[0].text.ends_with('-'),
            "the broken line renders a trailing hyphen"
        );
    }

    #[test]
    fn body_is_justified_headings_are_ragged() {
        // A paragraph long enough to wrap under the 432 pt frame (72 chars/line at 6 pt/char under
        // MONO) exercises the alignment wiring: as a Body it is justified (its underfull interior
        // line stretches — non-zero adjust — while the last line stays ragged); the identical text
        // as a Heading stays fully ragged (Alignment::Left). Spec 0017 increment 2.
        let words =
            "goblins raid the village at dusk stealing grain and copper coins from every trembling home nearby";
        let body = doc_with_blocks(vec![Block::body(words, Color::Gray { v: 0.0 })]);
        let heading = doc_with_blocks(vec![Block::heading(1, words, Color::Gray { v: 0.0 })]);

        let body_lines = first_text_lines(&lay_out(&body, &MONO, &NoHyphenator));
        let heading_lines = first_text_lines(&lay_out(&heading, &MONO, &NoHyphenator));

        assert!(body_lines.len() >= 2, "body should wrap to >= 2 lines");
        assert!(
            body_lines.iter().any(|l| l.space_adjust_pt != 0.0),
            "a justified body line should carry a non-zero adjustment"
        );
        assert_eq!(
            body_lines.last().unwrap().space_adjust_pt,
            0.0,
            "the paragraph's last line stays ragged"
        );
        assert!(
            heading_lines.iter().all(|l| l.space_adjust_pt == 0.0),
            "headings are ragged-left (never justified)"
        );
    }

    /// The `Rect` of the first `PlacedBlock::Text` found across `pages`.
    fn first_text_frame(pages: &[LaidOutPage]) -> Rect {
        pages
            .iter()
            .flat_map(|p| &p.blocks)
            .find_map(|b| match b {
                PlacedBlock::Text { frame, .. } => Some(*frame),
                _ => None,
            })
            .expect("a text block")
    }

    #[test]
    fn full_page_frame_is_the_whole_trim_at_origin() {
        // The seam's parity anchor: Frame::full_page is exactly the trim area at (0,0), which is why
        // lay_out (which uses it) stays byte-identical to the pre-frame column (spec 0019 incr. 1).
        let page = PageSetup::default();
        let frame = Frame::full_page(&page);
        assert_eq!(frame.rect.x_pt, 0.0);
        assert_eq!(frame.rect.y_pt, 0.0);
        assert_eq!(frame.rect.w_pt, page.trim.w_pt);
        assert_eq!(frame.rect.h_pt, page.trim.h_pt);
    }

    #[test]
    fn lay_out_matches_full_page_frame_path() {
        // lay_out is exactly lay_out_in_frame over the full-page frame — same pages, proving the
        // wrapper introduces no divergence (parity).
        let doc = Document::sample();
        let via_lay_out = lay_out(&doc, &MONO, &NoHyphenator);
        let via_frame = lay_out_in_frame(
            &doc.content,
            &doc.assets,
            &StyleSheet::default(),
            &Frame::full_page(&doc.page_setup),
            &MONO,
            &NoHyphenator,
        );
        assert_eq!(via_lay_out, via_frame);
    }

    #[test]
    fn frame_origin_offsets_placed_blocks() {
        // The same single short paragraph, laid full-page vs. into a frame at origin (36, 48). For
        // content that fits on one page, every placed block shifts by exactly (36, 48).
        let content = vec![Block::body("short line", Color::Gray { v: 0.0 })];
        let assets: Vec<Asset> = vec![];
        let page = PageSetup::default();

        let full = first_text_frame(&lay_out_in_frame(
            &content,
            &assets,
            &StyleSheet::default(),
            &Frame::full_page(&page),
            &MONO,
            &NoHyphenator,
        ));
        let offset = Frame {
            rect: Rect {
                x_pt: 36.0,
                y_pt: 48.0,
                w_pt: page.trim.w_pt,
                h_pt: page.trim.h_pt,
            },
        };
        let shifted = first_text_frame(&lay_out_in_frame(
            &content,
            &assets,
            &StyleSheet::default(),
            &offset,
            &MONO,
            &NoHyphenator,
        ));

        assert!(
            (shifted.x_pt - full.x_pt - 36.0).abs() < 0.01,
            "x: {} vs {}",
            shifted.x_pt,
            full.x_pt
        );
        assert!(
            (shifted.y_pt - full.y_pt - 48.0).abs() < 0.01,
            "y: {} vs {}",
            shifted.y_pt,
            full.y_pt
        );
    }

    #[test]
    fn narrower_frame_wraps_to_more_lines() {
        // A paragraph that wraps to N lines in the full-page frame wraps to strictly more lines in a
        // frame half as wide — text respects the frame width, not the page width.
        let content = vec![Block::body("goblins raid the village at dusk stealing grain and copper coins from every trembling home nearby", Color::Gray { v: 0.0 })];
        let assets: Vec<Asset> = vec![];
        let page = PageSetup::default();

        let wide = first_text_lines(&lay_out_in_frame(
            &content,
            &assets,
            &StyleSheet::default(),
            &Frame::full_page(&page),
            &MONO,
            &NoHyphenator,
        ));
        let narrow_frame = Frame {
            rect: Rect {
                x_pt: 0.0,
                y_pt: 0.0,
                w_pt: page.trim.w_pt / 2.0,
                h_pt: page.trim.h_pt,
            },
        };
        let narrow = first_text_lines(&lay_out_in_frame(
            &content,
            &assets,
            &StyleSheet::default(),
            &narrow_frame,
            &MONO,
            &NoHyphenator,
        ));
        assert!(
            narrow.len() > wide.len(),
            "narrow frame {} lines should exceed wide frame {} lines",
            narrow.len(),
            wide.len()
        );
    }

    #[test]
    fn shorter_frame_paginates_earlier() {
        // 20 single-line blocks fit on one full-page frame (648 pt / 12 pt = 54 lines). A 60 pt-tall
        // frame holds only ~5 lines, so the same content spills to multiple pages — overflow is
        // measured against the frame's bottom edge, not the trim height.
        let content: Vec<Block> = (0..20)
            .map(|i| Block::body(format!("L{i}"), Color::Gray { v: 0.0 }))
            .collect();
        let assets: Vec<Asset> = vec![];
        let page = PageSetup::default();

        let full = lay_out_in_frame(
            &content,
            &assets,
            &StyleSheet::default(),
            &Frame::full_page(&page),
            &MONO,
            &NoHyphenator,
        );
        assert_eq!(
            full.len(),
            1,
            "20 lines fit one full page, got {}",
            full.len()
        );

        let short_frame = Frame {
            rect: Rect {
                x_pt: 0.0,
                y_pt: 0.0,
                w_pt: page.trim.w_pt,
                h_pt: 60.0,
            },
        };
        let short = lay_out_in_frame(
            &content,
            &assets,
            &StyleSheet::default(),
            &short_frame,
            &MONO,
            &NoHyphenator,
        );
        assert!(
            short.len() >= 2,
            "a 60 pt frame must paginate 20 lines, got {}",
            short.len()
        );
    }

    /// Two side-by-side columns on a 432×648 page: a left frame and a right frame, each `w` wide and
    /// `h` tall at the top of the page. Used by the threading tests.
    fn two_column_thread(w: f32, h: f32) -> Thread {
        Thread {
            frames: vec![
                Frame {
                    rect: Rect {
                        x_pt: 0.0,
                        y_pt: 0.0,
                        w_pt: w,
                        h_pt: h,
                    },
                },
                Frame {
                    rect: Rect {
                        x_pt: 216.0,
                        y_pt: 0.0,
                        w_pt: w,
                        h_pt: h,
                    },
                },
            ],
        }
    }

    #[test]
    fn single_frame_thread_matches_lay_out_in_frame() {
        // Parity: a one-frame thread is exactly the incr. 1 single-frame path, so lay_out (and thus
        // export output) is unchanged by threading.
        let doc = Document::sample();
        let frame = Frame::full_page(&doc.page_setup);
        let via_frame = lay_out_in_frame(
            &doc.content,
            &doc.assets,
            &StyleSheet::default(),
            &frame,
            &MONO,
            &NoHyphenator,
        );
        let via_thread = lay_out_in_thread(
            &doc.content,
            &doc.assets,
            &StyleSheet::default(),
            &Thread {
                frames: vec![frame],
            },
            &MONO,
            &NoHyphenator,
        );
        assert_eq!(via_frame, via_thread);
    }

    #[test]
    fn overflow_chains_into_next_frame_on_same_page() {
        // Two 96 pt-tall columns (8 lines each) side by side. 12 single-line blocks overflow the
        // left column (8 lines) and must continue into the RIGHT column on the SAME page — not spill
        // to a second page (12 <= 16 lines of capacity).
        let content: Vec<Block> = (0..12)
            .map(|i| Block::body(format!("L{i}"), Color::Gray { v: 0.0 }))
            .collect();
        let thread = two_column_thread(216.0, 96.0);
        let pages = lay_out_in_thread(
            &content,
            &[],
            &StyleSheet::default(),
            &thread,
            &MONO,
            &NoHyphenator,
        );

        assert_eq!(
            pages.len(),
            1,
            "12 lines fit two 8-line columns on one page"
        );
        let xs: Vec<f32> = pages[0]
            .blocks
            .iter()
            .map(|b| match b {
                PlacedBlock::Text { frame, .. }
                | PlacedBlock::Image { frame, .. }
                | PlacedBlock::Rect { frame, .. } => frame.x_pt,
            })
            .collect();
        assert!(
            xs.contains(&0.0),
            "some blocks land in the left column (x=0)"
        );
        assert!(
            xs.contains(&216.0),
            "overflow continues into the right column (x=216)"
        );
    }

    #[test]
    fn new_page_only_after_last_frame_fills() {
        // Two 96 pt-tall columns = 16 lines of capacity per page. 20 single-line blocks overflow
        // BOTH columns and must spill to a second page, restarting at the first (left) frame.
        let content: Vec<Block> = (0..20)
            .map(|i| Block::body(format!("L{i}"), Color::Gray { v: 0.0 }))
            .collect();
        let thread = two_column_thread(216.0, 96.0);
        let pages = lay_out_in_thread(
            &content,
            &[],
            &StyleSheet::default(),
            &thread,
            &MONO,
            &NoHyphenator,
        );

        assert!(
            pages.len() >= 2,
            "20 lines exceed two 8-line columns, got {} pages",
            pages.len()
        );
        // Page 2's first block restarts at the first frame's origin (left column, y=0).
        match &pages[1].blocks[0] {
            PlacedBlock::Text { frame, .. } => {
                assert_eq!(frame.x_pt, 0.0, "page 2 restarts in the left column");
                assert_eq!(frame.y_pt, 0.0, "page 2 restarts at the frame top");
            }
            other => panic!("expected a text block, got {other:?}"),
        }
    }

    #[test]
    fn right_column_blocks_carry_right_frame_x() {
        // Every block that overflowed into the right column must carry that frame's x (216), never
        // the left frame's — proving placement uses the frame the block actually landed in.
        let content: Vec<Block> = (0..12)
            .map(|i| Block::body(format!("L{i}"), Color::Gray { v: 0.0 }))
            .collect();
        let thread = two_column_thread(216.0, 96.0);
        let pages = lay_out_in_thread(
            &content,
            &[],
            &StyleSheet::default(),
            &thread,
            &MONO,
            &NoHyphenator,
        );
        let right: Vec<&Rect> = pages[0]
            .blocks
            .iter()
            .filter_map(|b| match b {
                PlacedBlock::Text { frame, .. } if frame.x_pt == 216.0 => Some(frame),
                _ => None,
            })
            .collect();
        assert!(!right.is_empty(), "some blocks landed in the right column");
        assert!(
            right.iter().all(|f| f.w_pt == 216.0),
            "right-column blocks wrap to the right frame's width"
        );
    }

    #[test]
    fn advanced_block_rewraps_to_landed_frame_width() {
        // A block that overflows a WIDE frame into a NARROWER next frame must re-wrap to the narrow
        // width — not keep the wide measurement (spec 0019 incr. 2, "width per frame"). Frame A is a
        // full-width, 1-line-tall box; frame B is narrow. The first block fills A exactly; the
        // second overflows into B and must break to multiple lines at B's width (a stale wide
        // measurement would place it as a single line spilling past B's right edge).
        let thread = Thread {
            frames: vec![
                Frame {
                    rect: Rect {
                        x_pt: 0.0,
                        y_pt: 0.0,
                        w_pt: 432.0,
                        h_pt: BODY_LINE_HEIGHT_PT, // holds exactly one line
                    },
                },
                Frame {
                    rect: Rect {
                        x_pt: 216.0,
                        y_pt: 0.0,
                        w_pt: 60.0, // 10 chars/line under MONO (6 pt/char)
                        h_pt: 400.0,
                    },
                },
            ],
        };
        // "alpha beta gamma delta" = 22 chars: one line at 432 pt, but cannot fit in fewer than 3
        // lines of 10 chars, so it must wrap to >= 2 lines in frame B.
        let content = vec![
            Block::body("first", Color::Gray { v: 0.0 }),
            Block::body("alpha beta gamma delta", Color::Gray { v: 0.0 }),
        ];
        let pages = lay_out_in_thread(
            &content,
            &[],
            &StyleSheet::default(),
            &thread,
            &MONO,
            &NoHyphenator,
        );

        // The second block lands in the narrow frame B (x = 216).
        let (frame, lines) = pages[0]
            .blocks
            .iter()
            .find_map(|b| match b {
                PlacedBlock::Text { frame, lines, .. } if frame.x_pt == 216.0 => {
                    Some((frame, lines))
                }
                _ => None,
            })
            .expect("second block landed in the narrow frame");
        assert_eq!(frame.w_pt, 60.0, "carries the landed (narrow) frame width");
        assert!(
            lines.len() >= 2,
            "re-wrapped to the narrow frame width, got {} line(s)",
            lines.len()
        );
        assert!(
            (frame.h_pt - lines.len() as f32 * BODY_LINE_HEIGHT_PT).abs() < 0.01,
            "height matches the re-wrapped line count (not the stale 1-line height)"
        );
    }

    #[test]
    fn single_column_is_the_full_page() {
        // count == 1 is the whole trim area (== Frame::full_page), regardless of gutter.
        let page = PageSetup::default();
        for gutter in [0.0, 12.0, 36.0] {
            let thread = Thread::columns(&page, 1, gutter);
            assert_eq!(thread.frames.len(), 1);
            assert_eq!(
                thread.frames[0].rect,
                Frame::full_page(&page).rect,
                "gutter {gutter}"
            );
        }
    }

    #[test]
    fn columns_tile_the_trim_width() {
        // N columns of equal width, separated by (N-1) gutters, exactly span the trim width and are
        // laid left-to-right without overlap.
        let page = PageSetup::default();
        let trim_w = page.trim.w_pt;
        let gutter = 18.0;
        for count in [2usize, 3, 4] {
            let thread = Thread::columns(&page, count, gutter);
            assert_eq!(thread.frames.len(), count);

            let col_w = thread.frames[0].rect.w_pt;
            // All columns share the same width.
            assert!(
                thread
                    .frames
                    .iter()
                    .all(|f| (f.rect.w_pt - col_w).abs() < 0.01),
                "count {count}: columns should be equal width"
            );
            // N columns + (N-1) gutters span the trim width exactly.
            let spanned = count as f32 * col_w + (count - 1) as f32 * gutter;
            assert!(
                (spanned - trim_w).abs() < 0.01,
                "count {count}: {spanned} should span trim width {trim_w}"
            );
            // Left-to-right, non-overlapping: each column starts one (col_w + gutter) past the last.
            for i in 1..count {
                let prev = thread.frames[i - 1].rect;
                let cur = thread.frames[i].rect;
                assert!(
                    (cur.x_pt - (prev.x_pt + col_w + gutter)).abs() < 0.01,
                    "count {count}: column {i} should follow the gutter after column {}",
                    i - 1
                );
            }
        }
    }

    #[test]
    #[should_panic(expected = "too large")]
    fn oversized_gutter_panics_rather_than_corrupting() {
        // A gutter wide enough to make the column width non-positive must fail loudly, not emit
        // negative-width overlapping frames (CLAUDE.md: visible failure over silent corruption).
        let page = PageSetup::default(); // 432 pt trim: two 500 pt gutters can't fit.
        Thread::columns(&page, 2, 500.0);
    }

    #[test]
    fn columns_are_full_height_at_the_top() {
        let page = PageSetup::default();
        let thread = Thread::columns(&page, 3, 12.0);
        for (i, f) in thread.frames.iter().enumerate() {
            assert_eq!(f.rect.y_pt, 0.0, "column {i} y");
            assert_eq!(f.rect.h_pt, page.trim.h_pt, "column {i} height");
        }
    }

    #[test]
    fn columns_compose_with_threading() {
        // A short two-column thread (columns only ~8 lines tall) fed enough blocks to overflow the
        // first column must continue into the SECOND column on the same page — proving the
        // constructor composes with lay_out_in_thread to produce real multi-column flow.
        let page = PageSetup::default();
        // Full-height columns hold ~54 lines each, too many to overflow cheaply; shrink the height
        // by rebuilding the thread's frames to 96 pt (8 lines) so 12 blocks overflow column 0.
        let base = Thread::columns(&page, 2, 18.0);
        // Keep the constructor's derived x/width; only shrink the height to force overflow.
        let thread = Thread {
            frames: base
                .frames
                .iter()
                .map(|f| Frame {
                    rect: Rect {
                        h_pt: 96.0,
                        ..f.rect
                    },
                })
                .collect(),
        };
        // The substituted frames must still carry the derived column width (432 − 18)/2 = 207, so a
        // regression in col_w can't slip past this test.
        assert!((thread.frames[0].rect.w_pt - 207.0).abs() < 0.01);
        let content: Vec<Block> = (0..12)
            .map(|i| Block::body(format!("L{i}"), Color::Gray { v: 0.0 }))
            .collect();
        let pages = lay_out_in_thread(
            &content,
            &[],
            &StyleSheet::default(),
            &thread,
            &MONO,
            &NoHyphenator,
        );

        assert_eq!(
            pages.len(),
            1,
            "12 lines fit two 8-line columns on one page"
        );
        let left_x = thread.frames[0].rect.x_pt;
        let right_x = thread.frames[1].rect.x_pt;
        let xs: Vec<f32> = pages[0]
            .blocks
            .iter()
            .map(|b| match b {
                PlacedBlock::Text { frame, .. }
                | PlacedBlock::Image { frame, .. }
                | PlacedBlock::Rect { frame, .. } => frame.x_pt,
            })
            .collect();
        // Partition, not just presence: the 8-line-tall first column holds exactly the first 8
        // blocks, the remaining 4 overflow into the second column on the same page.
        let left = xs.iter().filter(|&&x| x == left_x).count();
        let right = xs.iter().filter(|&&x| x == right_x).count();
        assert_eq!(left, 8, "left column holds its 8 lines");
        assert_eq!(right, 4, "the remaining 4 overflow into the right column");
    }

    /// Build a minimal document from scratch with the given content blocks and default page setup.
    fn doc_with_blocks(content: Vec<Block>) -> Document {
        let mut doc = Document {
            format_version: quill_core_model::FORMAT_VERSION,
            metadata: Metadata::default(),
            page_setup: PageSetup::default(), // 432 × 648 pt (6×9 in)
            content,
            assets: vec![],
            fonts_embeddable: false,
            revision: 0,
            next_block_id: 0,
            styles: StyleSheet::default(),
            master_pages: Vec::new(),
            default_master: None,
            pages: Vec::new(),
        };
        // Give the blocks ids, as a loaded document would have (spec 0026).
        doc.assign_missing_block_ids().expect("fresh blocks");
        doc
    }

    #[test]
    fn paginates_when_content_overflows() {
        // Each Body block produces 1 line = BODY_LINE_HEIGHT_PT (12 pt).
        // Page height is 648 pt → 54 lines fit. Push 100 blocks to guarantee overflow.
        let blocks: Vec<Block> = (0..100)
            .map(|i| Block::body(format!("Line {i}"), Color::Gray { v: 0.0 }))
            .collect();
        let doc = doc_with_blocks(blocks);
        let pages = lay_out(&doc, &MONO, &NoHyphenator);
        assert!(
            pages.len() >= 2,
            "expected at least 2 pages, got {}",
            pages.len()
        );
    }

    #[test]
    fn boundary_exactly_one_page_then_spills() {
        // Compute how many single-line blocks fill one page exactly.
        let page_h = PageSetup::default().trim.h_pt; // 648.0
        let lines_per_page = (page_h / BODY_LINE_HEIGHT_PT).floor() as usize; // 54

        let make_block = |i: usize| Block::body(format!("L{i}"), Color::Gray { v: 0.0 });

        // Exactly lines_per_page blocks → fits on one page.
        let exact_blocks: Vec<Block> = (0..lines_per_page).map(make_block).collect();
        let doc_exact = doc_with_blocks(exact_blocks);
        let pages_exact = lay_out(&doc_exact, &MONO, &NoHyphenator);
        assert_eq!(
            pages_exact.len(),
            1,
            "expected 1 page for exact fit, got {}",
            pages_exact.len()
        );

        // One extra block → must spill to a second page.
        let overflow_blocks: Vec<Block> = (0..=lines_per_page).map(make_block).collect();
        let doc_overflow = doc_with_blocks(overflow_blocks);
        let pages_overflow = lay_out(&doc_overflow, &MONO, &NoHyphenator);
        assert!(
            pages_overflow.len() >= 2,
            "expected >= 2 pages after overflow, got {}",
            pages_overflow.len()
        );
    }

    #[test]
    fn places_image_block() {
        let asset_id = "img1".to_string();
        let doc = Document {
            format_version: quill_core_model::FORMAT_VERSION,
            metadata: Metadata::default(),
            page_setup: PageSetup {
                trim: Size {
                    w_pt: 432.0,
                    h_pt: 648.0,
                },
                ..PageSetup::default()
            },
            content: vec![
                Block::image(asset_id.clone()),
                // Unknown asset — should be silently skipped.
                Block::image("unknown-asset-xyz".to_string()),
            ],
            assets: vec![Asset {
                id: asset_id.clone(),
                path: "assets/img1.png".into(),
                // 900×600 px at 300 dpi → natural 216×144 pt, both within the 432 pt content
                // width, so placed at natural size with a 1.5 aspect ratio (not a square).
                px_w: 900,
                px_h: 600,
                dpi: 300.0,
                line_art: false,
                has_alpha: false,
            }],
            fonts_embeddable: false,
            revision: 0,
            next_block_id: 0,
            styles: StyleSheet::default(),
            master_pages: Vec::new(),
            default_master: None,
            pages: Vec::new(),
        };

        let pages = lay_out(&doc, &MONO, &NoHyphenator);

        // Collect all image blocks across all pages.
        let image_blocks: Vec<&PlacedBlock> = pages
            .iter()
            .flat_map(|p| p.blocks.iter())
            .filter(|b| matches!(b, PlacedBlock::Image { .. }))
            .collect();

        assert_eq!(
            image_blocks.len(),
            1,
            "expected exactly 1 image block (unknown asset skipped)"
        );

        match &image_blocks[0] {
            PlacedBlock::Image {
                asset_id: id,
                frame,
                ..
            } => {
                assert_eq!(id, &asset_id);
                assert!((frame.w_pt - 216.0).abs() < 0.01, "w = {}", frame.w_pt);
                assert!((frame.h_pt - 144.0).abs() < 0.01, "h = {}", frame.h_pt);
            }
            other => panic!("expected Image block, got {other:?}"),
        }
    }

    fn sized_asset(px_w: u32, px_h: u32, dpi: f32) -> Asset {
        Asset {
            id: "a".into(),
            path: "a.png".into(),
            px_w,
            px_h,
            dpi,
            line_art: false,
            has_alpha: false,
        }
    }

    #[test]
    fn wide_image_scales_down_to_content_width_preserving_aspect() {
        let content_width = 432.0;
        // 4000×2000 px at 300 dpi → natural 960×480 pt (wider than 432) → scaled to width 432,
        // height 216, keeping the 2:1 aspect ratio.
        let (w, h) = image_size(&sized_asset(4000, 2000, 300.0), content_width);
        assert!((w - content_width).abs() < 0.01, "w = {w}");
        assert!((h - 216.0).abs() < 0.01, "h = {h}");
        assert!((w / h - 2.0).abs() < 0.001, "aspect = {}", w / h);
    }

    #[test]
    fn small_image_placed_at_natural_size() {
        // 300×450 px at 300 dpi → 72×108 pt, both within the content width → natural size.
        let (w, h) = image_size(&sized_asset(300, 450, 300.0), 432.0);
        assert!((w - 72.0).abs() < 0.01, "w = {w}");
        assert!((h - 108.0).abs() < 0.01, "h = {h}");
    }

    #[test]
    fn missing_pixel_dims_fall_back_to_square() {
        let content_width = 432.0;
        let (w, h) = image_size(&sized_asset(0, 0, 300.0), content_width);
        assert_eq!(w, content_width);
        assert_eq!(h, content_width);
    }

    /// The size/leading a placed text block reports.
    fn first_text_metrics(pages: &[LaidOutPage]) -> (f32, f32) {
        pages
            .iter()
            .flat_map(|p| p.blocks.iter())
            .find_map(|b| match b {
                PlacedBlock::Text {
                    font_size_pt,
                    leading_pt,
                    ..
                } => Some((*font_size_pt, *leading_pt)),
                _ => None,
            })
            .expect("expected a text block")
    }

    #[test]
    fn a_heading_is_laid_out_at_its_style_size_not_body_size() {
        // Before spec 0028 every block was measured at BODY_FONT_SIZE_PT, so a heading differed
        // from body text only by being ragged-left.
        let doc = doc_with_blocks(vec![Block::heading(1, "Title", Color::Gray { v: 0.0 })]);
        let (size, leading) = first_text_metrics(&lay_out(&doc, &MONO, &NoHyphenator));
        let expected = doc.styles.paragraph["h1"];
        assert_eq!(size, expected.font_size_pt);
        assert_eq!(leading, expected.leading_pt);
        assert!(size > BODY_FONT_SIZE_PT, "h1 must be larger than body");
    }

    #[test]
    fn body_text_keeps_the_historical_size_and_leading() {
        let doc = doc_with_blocks(vec![Block::body("some prose", Color::Gray { v: 0.0 })]);
        let (size, leading) = first_text_metrics(&lay_out(&doc, &MONO, &NoHyphenator));
        assert_eq!(size, BODY_FONT_SIZE_PT);
        assert_eq!(leading, BODY_LINE_HEIGHT_PT);
    }

    #[test]
    fn a_larger_style_wraps_to_more_lines_in_the_same_frame() {
        // The load-bearing consequence: style affects *measurement*, not just what is drawn. If it
        // only reached the writer, text would be drawn larger than the space reserved for it.
        let text = "a moderately long line of prose that will wrap differently at two sizes";
        let small = doc_with_blocks(vec![Block::body(text, Color::Gray { v: 0.0 })]);
        let large = doc_with_blocks(vec![Block::heading(1, text, Color::Gray { v: 0.0 })]);
        let count = |pages: Vec<LaidOutPage>| {
            pages
                .iter()
                .flat_map(|p| p.blocks.iter())
                .find_map(|b| match b {
                    PlacedBlock::Text { lines, .. } => Some(lines.len()),
                    _ => None,
                })
                .unwrap()
        };
        assert!(
            count(lay_out(&large, &MONO, &NoHyphenator))
                > count(lay_out(&small, &MONO, &NoHyphenator)),
            "the same text at h1 size must break into more lines than at body size"
        );
    }

    #[test]
    fn space_before_offsets_the_frame_without_being_drawn_in() {
        // Space above is part of the block's occupied height (so pagination reserves it) but no
        // text sits in it — the text frame starts below it.
        let doc = doc_with_blocks(vec![
            Block::body("first", Color::Gray { v: 0.0 }),
            Block::heading(2, "Heading", Color::Gray { v: 0.0 }),
        ]);
        let pages = lay_out(&doc, &MONO, &NoHyphenator);
        let frames: Vec<Rect> = pages
            .iter()
            .flat_map(|p| p.blocks.iter())
            .filter_map(|b| match b {
                PlacedBlock::Text { frame, .. } => Some(*frame),
                _ => None,
            })
            .collect();
        let h2 = doc.styles.paragraph["h2"];
        let gap = frames[1].y_pt - (frames[0].y_pt + frames[0].h_pt);
        assert!(
            (gap - h2.space_before_pt).abs() < 0.01,
            "expected {} pt of space above the heading, got {gap}",
            h2.space_before_pt
        );
    }

    #[test]
    fn a_document_of_only_body_text_lays_out_exactly_as_before_styles() {
        // Parity: with the default sheet, body-only content must be positioned identically to the
        // pre-styles engine — one line per BODY_LINE_HEIGHT_PT starting at the frame top.
        let content: Vec<Block> = (0..5)
            .map(|i| Block::body(format!("L{i}"), Color::Gray { v: 0.0 }))
            .collect();
        let doc = doc_with_blocks(content);
        let pages = lay_out(&doc, &MONO, &NoHyphenator);
        let ys: Vec<f32> = pages[0]
            .blocks
            .iter()
            .filter_map(|b| match b {
                PlacedBlock::Text { frame, .. } => Some(frame.y_pt),
                _ => None,
            })
            .collect();
        assert_eq!(ys, vec![0.0, 12.0, 24.0, 36.0, 48.0]);
    }

    // --- Per-page template seam (spec 0029) ---------------------------------------------------

    /// A template whose frames narrow on every page — the simplest thing a uniform thread cannot
    /// express, and enough to prove geometry really is asked for per page.
    struct NarrowingTemplate;

    impl PageTemplate for NarrowingTemplate {
        fn frames(&self, page_index: usize) -> Vec<Frame> {
            vec![Frame {
                rect: Rect {
                    x_pt: 0.0,
                    y_pt: 0.0,
                    w_pt: 400.0 - 50.0 * page_index as f32,
                    h_pt: 100.0,
                },
            }]
        }
    }

    /// A template that stamps a folio on every page.
    struct FolioTemplate;

    impl PageTemplate for FolioTemplate {
        fn frames(&self, _page_index: usize) -> Vec<Frame> {
            vec![Frame {
                rect: Rect {
                    x_pt: 0.0,
                    y_pt: 0.0,
                    w_pt: 432.0,
                    h_pt: 100.0,
                },
            }]
        }
        fn statics(&self, page_index: usize) -> Vec<PlacedBlock> {
            vec![PlacedBlock::Text {
                source: BlockId::UNASSIGNED,
                frame: Rect {
                    x_pt: 0.0,
                    y_pt: 620.0,
                    w_pt: 432.0,
                    h_pt: 12.0,
                },
                lines: vec![Line {
                    text: format!("{}", page_index + 1),
                    space_adjust_pt: 0.0,
                }],
                color: Color::Gray { v: 0.0 },
                font_size_pt: 9.0,
                leading_pt: 11.0,
            }]
        }
    }

    fn many_lines(n: usize) -> Vec<Block> {
        (0..n)
            .map(|i| Block::body(format!("L{i}"), Color::Gray { v: 0.0 }))
            .collect()
    }

    #[test]
    fn a_uniform_template_is_identical_to_a_plain_thread() {
        // Parity, the whole basis for landing this seam separately from master pages.
        let content = many_lines(40);
        let thread = Thread::columns(&PageSetup::default(), 2, 12.0);
        let styles = StyleSheet::default();

        let via_thread = lay_out_in_thread(&content, &[], &styles, &thread, &MONO, &NoHyphenator);
        let via_template = lay_out_with_template(
            &content,
            &[],
            &styles,
            &UniformTemplate::new(thread.clone()),
            &MONO,
            &NoHyphenator,
        );
        assert_eq!(via_thread, via_template);
    }

    #[test]
    fn pages_are_numbered_in_order_from_zero() {
        // A page had no identity at all before this: a running head could not say "42".
        let pages = lay_out_in_thread(
            &many_lines(200),
            &[],
            &StyleSheet::default(),
            &Thread::columns(&PageSetup::default(), 1, 0.0),
            &MONO,
            &NoHyphenator,
        );
        assert!(pages.len() > 2, "expected several pages");
        let indices: Vec<usize> = pages.iter().map(|p| p.index).collect();
        assert_eq!(indices, (0..pages.len()).collect::<Vec<_>>());
    }

    #[test]
    fn each_page_takes_its_own_geometry_from_the_template() {
        // The capability the seam exists for: before this, the page-advance branch reset into the
        // same frame list, so every page was geometrically identical by construction.
        let pages = lay_out_with_template(
            &many_lines(60),
            &[],
            &StyleSheet::default(),
            &NarrowingTemplate,
            &MONO,
            &NoHyphenator,
        );
        assert!(pages.len() >= 3, "expected at least 3 pages");
        let widths: Vec<f32> = pages
            .iter()
            .take(3)
            .map(|p| match &p.blocks[0] {
                PlacedBlock::Text { frame, .. } => frame.w_pt,
                _ => panic!("expected text"),
            })
            .collect();
        assert_eq!(widths, vec![400.0, 350.0, 300.0]);
    }

    #[test]
    fn template_statics_land_on_every_page_and_vary_by_page() {
        let pages = lay_out_with_template(
            &many_lines(30),
            &[],
            &StyleSheet::default(),
            &FolioTemplate,
            &MONO,
            &NoHyphenator,
        );
        assert!(pages.len() >= 2);
        for (i, page) in pages.iter().enumerate() {
            assert_eq!(page.statics.len(), 1, "page {i} should carry a folio");
            match &page.statics[0] {
                PlacedBlock::Text { lines, .. } => {
                    assert_eq!(lines[0].text, format!("{}", i + 1))
                }
                _ => panic!("expected a text folio"),
            }
        }
    }

    #[test]
    fn statics_are_kept_separate_from_flowed_content() {
        // They are drawn first (so master art sits behind text) and incremental relayout can leave
        // them alone; merging them into `blocks` would lose both properties.
        let pages = lay_out_with_template(
            &many_lines(5),
            &[],
            &StyleSheet::default(),
            &FolioTemplate,
            &MONO,
            &NoHyphenator,
        );
        assert_eq!(pages[0].statics.len(), 1);
        assert_eq!(pages[0].blocks.len(), 5);
    }

    #[test]
    #[should_panic(expected = "produced no frames")]
    fn a_template_with_no_frames_fails_loudly() {
        // Content silently disappearing is the failure class CLAUDE.md forbids.
        struct Empty;
        impl PageTemplate for Empty {
            fn frames(&self, _: usize) -> Vec<Frame> {
                Vec::new()
            }
        }
        lay_out_with_template(
            &many_lines(1),
            &[],
            &StyleSheet::default(),
            &Empty,
            &MONO,
            &NoHyphenator,
        );
    }

    // --- Authored master pages (spec 0030) ----------------------------------------------------

    fn doc_with_master(master: MasterPage, blocks: Vec<Block>) -> Document {
        let mut doc = doc_with_blocks(blocks);
        doc.default_master = Some(master.name.clone());
        doc.master_pages = vec![master];
        doc
    }

    #[test]
    fn a_document_with_no_master_lays_out_in_the_full_page_frame() {
        // Parity: the pre-0030 behavior must survive a document that declares nothing.
        let doc = doc_with_blocks(vec![Block::body("x", Color::Gray { v: 0.0 })]);
        let pages = lay_out(&doc, &MONO, &NoHyphenator);
        match &pages[0].blocks[0] {
            PlacedBlock::Text { frame, .. } => {
                assert_eq!(frame.x_pt, 0.0);
                assert_eq!(frame.y_pt, 0.0);
                assert_eq!(frame.w_pt, doc.page_setup.trim.w_pt);
            }
            _ => panic!("expected text"),
        }
        assert!(pages[0].statics.is_empty());
    }

    #[test]
    fn margins_inset_the_text_frame() {
        let mut doc = doc_with_blocks(vec![Block::body("x", Color::Gray { v: 0.0 })]);
        doc.page_setup.margins = Margins::uniform(36.0);
        let pages = lay_out(&doc, &MONO, &NoHyphenator);
        match &pages[0].blocks[0] {
            PlacedBlock::Text { frame, .. } => {
                assert_eq!(frame.x_pt, 36.0);
                assert_eq!(frame.y_pt, 36.0);
                assert_eq!(frame.w_pt, 432.0 - 72.0);
            }
            _ => panic!("expected text"),
        }
    }

    #[test]
    fn inside_and_outside_margins_mirror_across_a_spread() {
        // The reason margins are inside/outside rather than left/right: the spine margin has to be
        // on the left of a recto and the right of a verso, or a bound book's text drifts toward the
        // gutter on every other page.
        let mut doc = doc_with_blocks(many_lines(200));
        doc.page_setup.facing_pages = true;
        doc.page_setup.margins = Margins {
            top_pt: 0.0,
            bottom_pt: 0.0,
            inside_pt: 60.0,
            outside_pt: 20.0,
        };
        let pages = lay_out(&doc, &MONO, &NoHyphenator);
        assert!(pages.len() >= 2);
        let x = |p: &LaidOutPage| match &p.blocks[0] {
            PlacedBlock::Text { frame, .. } => frame.x_pt,
            _ => panic!("expected text"),
        };
        assert_eq!(x(&pages[0]), 60.0, "recto: spine on the left");
        assert_eq!(x(&pages[1]), 20.0, "verso: spine on the right");
    }

    #[test]
    fn a_single_sided_document_does_not_mirror() {
        let mut doc = doc_with_blocks(many_lines(200));
        doc.page_setup.facing_pages = false;
        doc.page_setup.margins = Margins {
            top_pt: 0.0,
            bottom_pt: 0.0,
            inside_pt: 60.0,
            outside_pt: 20.0,
        };
        let pages = lay_out(&doc, &MONO, &NoHyphenator);
        for page in pages.iter().take(3) {
            match &page.blocks[0] {
                PlacedBlock::Text { frame, .. } => assert_eq!(frame.x_pt, 60.0),
                _ => panic!("expected text"),
            }
        }
    }

    #[test]
    fn a_masters_column_count_divides_the_text_area() {
        let master = MasterPage {
            columns: 2,
            gutter_pt: 12.0,
            ..MasterPage::plain("body-master")
        };
        let doc = doc_with_master(master, many_lines(200));
        let pages = lay_out(&doc, &MONO, &NoHyphenator);
        let xs: Vec<f32> = pages[0]
            .blocks
            .iter()
            .filter_map(|b| match b {
                PlacedBlock::Text { frame, .. } => Some(frame.x_pt),
                _ => None,
            })
            .collect();
        let col_w = (432.0 - 12.0) / 2.0;
        assert!(xs.contains(&0.0), "expected a left column");
        assert!(
            xs.iter().any(|x| (*x - (col_w + 12.0)).abs() < 0.01),
            "expected a right column at {}, got {xs:?}",
            col_w + 12.0
        );
    }

    #[test]
    fn a_master_stamps_its_statics_with_the_page_number_resolved() {
        let master = MasterPage {
            statics: vec![MasterStatic::Text {
                rect: Rect {
                    x_pt: 0.0,
                    y_pt: 620.0,
                    w_pt: 432.0,
                    h_pt: 12.0,
                },
                text: "The Dungeon — {page}".into(),
                color: Color::Gray { v: 0.0 },
                style: Some("body".into()),
            }],
            ..MasterPage::plain("running-head")
        };
        let doc = doc_with_master(master, many_lines(200));
        let pages = lay_out(&doc, &MONO, &NoHyphenator);
        assert!(pages.len() >= 2);
        for (i, page) in pages.iter().enumerate().take(3) {
            match &page.statics[0] {
                PlacedBlock::Text { lines, .. } => {
                    assert_eq!(lines[0].text, format!("The Dungeon — {}", i + 1))
                }
                _ => panic!("expected a running head"),
            }
        }
    }

    #[test]
    fn an_unknown_default_master_degrades_to_the_page_setup() {
        // A renamed master must not refuse to lay the book out.
        let mut doc = doc_with_blocks(vec![Block::body("x", Color::Gray { v: 0.0 })]);
        doc.default_master = Some("was-renamed".into());
        let pages = lay_out(&doc, &MONO, &NoHyphenator);
        match &pages[0].blocks[0] {
            PlacedBlock::Text { frame, .. } => assert_eq!(frame.w_pt, 432.0),
            _ => panic!("expected text"),
        }
    }

    #[test]
    fn an_over_wide_gutter_falls_back_to_one_column_instead_of_panicking() {
        // `Thread::columns` panics on this, which is right for a programmatic caller. Here the
        // gutter is *authored*, and a user can type any number — a document that cannot be opened
        // is worse than one that looks wrong and can be fixed.
        let master = MasterPage {
            columns: 3,
            gutter_pt: 1000.0,
            ..MasterPage::plain("bad")
        };
        let doc = doc_with_master(master, vec![Block::body("x", Color::Gray { v: 0.0 })]);
        let pages = lay_out(&doc, &MONO, &NoHyphenator);
        match &pages[0].blocks[0] {
            PlacedBlock::Text { frame, .. } => assert_eq!(frame.w_pt, 432.0),
            _ => panic!("expected text"),
        }
    }

    #[test]
    fn master_statics_and_margins_round_trip_through_the_document() {
        let master = MasterPage {
            margins: Some(Margins::uniform(24.0)),
            columns: 2,
            gutter_pt: 10.0,
            statics: vec![MasterStatic::Image {
                rect: Rect {
                    x_pt: 0.0,
                    y_pt: 0.0,
                    w_pt: 432.0,
                    h_pt: 648.0,
                },
                asset: "bg".into(),
            }],
            ..MasterPage::plain("full-bleed")
        };
        let doc = doc_with_master(master, vec![Block::body("x", Color::Gray { v: 0.0 })]);
        let back = Document::from_json(&doc.to_json().expect("save")).expect("load");
        assert_eq!(back.master_pages, doc.master_pages);
        assert_eq!(back.default_master, doc.default_master);
    }

    #[test]
    fn a_master_can_override_the_documents_margins() {
        let master = MasterPage {
            margins: Some(Margins::uniform(50.0)),
            ..MasterPage::plain("wide")
        };
        let mut doc = doc_with_master(master, vec![Block::body("x", Color::Gray { v: 0.0 })]);
        doc.page_setup.margins = Margins::uniform(10.0);
        let pages = lay_out(&doc, &MONO, &NoHyphenator);
        match &pages[0].blocks[0] {
            PlacedBlock::Text { frame, .. } => assert_eq!(frame.x_pt, 50.0),
            _ => panic!("expected text"),
        }
    }

    // --- Per-page master assignment (spec 0035) -----------------------------------------------

    /// A document with an `opener` master (deep top margin, no furniture) and a `body` master
    /// (shallow top margin, a folio), defaulting to `body`.
    fn doc_with_opener_and_body(blocks: Vec<Block>) -> Document {
        let opener = MasterPage {
            margins: Some(Margins {
                top_pt: 108.0,
                bottom_pt: 36.0,
                inside_pt: 36.0,
                outside_pt: 36.0,
            }),
            ..MasterPage::plain("opener")
        };
        let body = MasterPage {
            margins: Some(Margins::uniform(36.0)),
            statics: vec![MasterStatic::Text {
                rect: Rect {
                    x_pt: 0.0,
                    y_pt: 620.0,
                    w_pt: 432.0,
                    h_pt: 12.0,
                },
                text: "{page}".into(),
                color: Color::Gray { v: 0.0 },
                style: Some("body".into()),
            }],
            ..MasterPage::plain("body")
        };
        let mut doc = doc_with_blocks(blocks);
        doc.master_pages = vec![opener, body];
        doc.default_master = Some("body".into());
        doc
    }

    fn frame_y(page: &LaidOutPage) -> f32 {
        match &page.blocks[0] {
            PlacedBlock::Text { frame, .. } => frame.y_pt,
            other => panic!("expected text, got {other:?}"),
        }
    }

    #[test]
    fn a_page_override_gives_that_page_its_own_geometry() {
        // The chapter-opener case, which is the whole reason this exists: page 0 starts 108 pt down
        // the page, every page after it at the body master's 36 pt.
        let mut doc = doc_with_opener_and_body(many_lines(200));
        doc.pages = vec![PageOverride {
            master: Some("opener".into()),
        }];
        let pages = lay_out(&doc, &MONO, &NoHyphenator);
        assert!(pages.len() >= 3, "need a body page to compare against");
        assert_eq!(frame_y(&pages[0]), 108.0);
        assert_eq!(frame_y(&pages[1]), 36.0);
        assert_eq!(frame_y(&pages[2]), 36.0);
    }

    #[test]
    fn an_unknown_page_master_falls_back_to_the_default_then_to_the_page_setup() {
        // Both fallback steps, because a renamed master must cost the page its furniture and not
        // cost the author the page — the same posture as an unknown style name.
        let mut doc = doc_with_opener_and_body(vec![Block::body("x", Color::Gray { v: 0.0 })]);
        doc.pages = vec![PageOverride {
            master: Some("was-renamed".into()),
        }];
        let pages = lay_out(&doc, &MONO, &NoHyphenator);
        assert_eq!(
            frame_y(&pages[0]),
            36.0,
            "unknown page master should fall back to `body`"
        );

        doc.default_master = Some("also-renamed".into());
        let pages = lay_out(&doc, &MONO, &NoHyphenator);
        assert_eq!(
            frame_y(&pages[0]),
            0.0,
            "with the default gone too, the document's own page setup governs"
        );
    }

    #[test]
    fn an_override_that_names_no_master_falls_through_to_the_default() {
        // `Some(PageOverride { master: None })` is not the same as "no master": an entry that
        // declines to override still gets the document's default.
        let mut doc = doc_with_opener_and_body(vec![Block::body("x", Color::Gray { v: 0.0 })]);
        doc.pages = vec![PageOverride { master: None }];
        let pages = lay_out(&doc, &MONO, &NoHyphenator);
        assert_eq!(frame_y(&pages[0]), 36.0);
    }

    #[test]
    fn a_page_list_need_not_match_the_document_length() {
        // Short: governs what it covers, the rest fall back. Long: the surplus is ignored rather
        // than being an error, because the content that justified those entries may just have been
        // deleted.
        let mut doc = doc_with_opener_and_body(many_lines(200));
        doc.pages = vec![PageOverride {
            master: Some("opener".into()),
        }];
        let short = lay_out(&doc, &MONO, &NoHyphenator);
        assert!(short.len() >= 2);
        assert_eq!(frame_y(&short[1]), 36.0);

        doc.pages = (0..short.len() + 50)
            .map(|i| PageOverride {
                master: (i == 0).then(|| "opener".to_string()),
            })
            .collect();
        let long = lay_out(&doc, &MONO, &NoHyphenator);
        assert_eq!(long.len(), short.len(), "surplus entries add no pages");
        assert_eq!(frame_y(&long[0]), 108.0);
    }

    #[test]
    fn statics_resolve_against_the_pages_own_master() {
        // The first thing in the engine to vary statics *between* pages: the opener carries no
        // folio, the body master does, and the token is still one-based.
        let mut doc = doc_with_opener_and_body(many_lines(200));
        doc.pages = vec![PageOverride {
            master: Some("opener".into()),
        }];
        let pages = lay_out(&doc, &MONO, &NoHyphenator);
        assert!(pages.len() >= 2);
        assert!(pages[0].statics.is_empty(), "the opener has no furniture");
        match &pages[1].statics[0] {
            PlacedBlock::Text { lines, .. } => assert_eq!(lines[0].text, "2"),
            other => panic!("expected a folio, got {other:?}"),
        }
    }

    #[test]
    fn per_page_statics_still_do_not_consume_flow_space() {
        // Spec 0029's invariant, re-asserted because statics now differ from page to page: adding
        // furniture must not move a single line of the text it labels.
        let mut with = doc_with_opener_and_body(many_lines(200));
        with.pages = vec![PageOverride {
            master: Some("opener".into()),
        }];
        let mut without = with.clone();
        for m in &mut without.master_pages {
            m.statics.clear();
        }
        let a = lay_out(&with, &MONO, &NoHyphenator);
        let b = lay_out(&without, &MONO, &NoHyphenator);
        assert_eq!(a.len(), b.len());
        for (pa, pb) in a.iter().zip(b.iter()) {
            assert_eq!(pa.blocks, pb.blocks, "flowed content must be identical");
        }
    }

    #[test]
    fn the_page_list_round_trips_through_the_document() {
        let mut doc = doc_with_opener_and_body(vec![Block::body("x", Color::Gray { v: 0.0 })]);
        doc.pages = vec![
            PageOverride {
                master: Some("opener".into()),
            },
            PageOverride { master: None },
        ];
        let back = Document::from_json(&doc.to_json().expect("save")).expect("load");
        assert_eq!(back.pages, doc.pages);
    }

    // --- Document templates (spec 0036) --------------------------------------------------------

    #[test]
    fn a_template_document_lays_out_with_its_opener_on_page_zero() {
        // The end-to-end claim of the on-ramp: from a template, without authoring any layout, page
        // 0 is a chapter opener and pages 1+ are body.
        for name in ["adventure", "rulebook"] {
            let t = quill_core_model::Template::by_name(name).expect("bundled");
            let mut doc = Document::from_template(t);
            doc.content = many_lines(400);
            doc.assign_missing_block_ids().expect("ids");

            let pages = lay_out(&doc, &MONO, &NoHyphenator);
            assert!(pages.len() >= 3, "{name}: need body pages to compare");

            let opener_top = t
                .master_pages
                .iter()
                .find(|m| m.name == quill_core_model::OPENER_MASTER)
                .and_then(|m| m.margins)
                .expect("opener margins")
                .top_pt;
            let body_top = t
                .master_pages
                .iter()
                .find(|m| m.name == quill_core_model::BODY_MASTER)
                .and_then(|m| m.margins)
                .expect("body margins")
                .top_pt;

            assert!(
                (frame_y(&pages[0]) - opener_top).abs() < 0.01,
                "{name}: page 0"
            );
            assert!(
                (frame_y(&pages[1]) - body_top).abs() < 0.01,
                "{name}: page 1"
            );
            assert!(
                pages[0].statics.is_empty(),
                "{name}: a chapter opener carries no folio"
            );
            match &pages[1].statics[0] {
                PlacedBlock::Text { lines, .. } => assert_eq!(lines[0].text, "2"),
                other => panic!("{name}: expected a folio, got {other:?}"),
            }
        }
    }

    #[test]
    fn an_empty_template_document_still_produces_a_page() {
        // Nothing in the workspace had ever laid out a document with no blocks before templates
        // existed. A starter must open, not produce zero pages or panic.
        for t in quill_core_model::Template::bundled() {
            let doc = Document::from_template(t);
            let pages = lay_out(&doc, &MONO, &NoHyphenator);
            assert_eq!(pages.len(), 1, "template `{}`", t.name);
            assert!(pages[0].blocks.is_empty());
        }
    }

    #[test]
    fn a_two_column_template_really_gives_two_frames() {
        // Guards the arithmetic in the bundled rulebook: 432 - 54 - 40 = 338 pt of text area, two
        // columns with a 14 pt gutter ⇒ 162 pt each at x = 54 and x = 230.
        // The template is facing-pages, so the spine margin swaps sides: a recto starts at the
        // 54 pt inside margin, a verso at the 40 pt fore-edge. Both are asserted, because a
        // template that got this backwards would drift every other spread toward the gutter and
        // still look right on page 0.
        let t = quill_core_model::Template::by_name("rulebook").expect("bundled");
        let doc = Document::from_template(t);
        let template = DocumentTemplate::new(&doc);

        let recto = template.frames(0);
        assert_eq!(recto.len(), 2);
        assert!((recto[0].rect.w_pt - 162.0).abs() < 0.01);
        assert!((recto[1].rect.w_pt - 162.0).abs() < 0.01);
        assert!((recto[0].rect.x_pt - 54.0).abs() < 0.01);
        assert!((recto[1].rect.x_pt - 230.0).abs() < 0.01);

        let verso = template.frames(1);
        assert_eq!(verso.len(), 2);
        assert!((verso[0].rect.x_pt - 40.0).abs() < 0.01);
        assert!((verso[0].rect.w_pt - 162.0).abs() < 0.01);
    }

    // --- Stat blocks (spec 0038) ----------------------------------------------------------------

    fn goblin() -> quill_core_model::StatBlock {
        quill_core_model::StatBlock {
            name: "Goblin".into(),
            overview: vec!["Small humanoid, chaotic".into()],
            attributes: vec![("AC".into(), "15".into()), ("HP".into(), "7".into())],
            details: vec![],
            actions: vec!["Scimitar. +4 to hit, 5 damage.".into()],
            reactions: vec![],
        }
    }

    fn stat_doc() -> Document {
        let mut doc = doc_with_blocks(vec![Block::StatBlock {
            id: BlockId::UNASSIGNED,
            stat: goblin(),
            color: Color::Gray { v: 0.0 },
        }]);
        doc.assign_missing_block_ids().expect("ids");
        doc
    }

    #[test]
    fn a_stat_block_places_a_panel_behind_its_own_text() {
        let doc = stat_doc();
        let pages = lay_out(&doc, &MONO, &NoHyphenator);
        let blocks = &pages[0].blocks;

        assert!(
            matches!(blocks[0], PlacedBlock::Rect { .. }),
            "the panel must be placed first, so it sits behind its text"
        );
        // Name + 2 attributes + 1 overview + 1 action = 5 runs.
        let texts = blocks
            .iter()
            .filter(|b| matches!(b, PlacedBlock::Text { .. }))
            .count();
        assert_eq!(texts, 5, "one run per section line");
        let rects = blocks
            .iter()
            .filter(|b| matches!(b, PlacedBlock::Rect { .. }))
            .count();
        assert_eq!(rects, 4, "the panel plus one rule per section boundary");
        assert_eq!(blocks.len(), 9, "and nothing else");
    }

    #[test]
    fn a_stat_blocks_text_is_inset_by_the_padding_on_every_side() {
        // The panel spans the frame; every run sits `STATBLOCK_PADDING_PT` inside it, and the last
        // run's baseline block ends at least a padding above the panel's bottom edge.
        let doc = stat_doc();
        let pages = lay_out(&doc, &MONO, &NoHyphenator);
        let PlacedBlock::Rect { frame: panel, .. } = pages[0].blocks[0] else {
            panic!("expected a panel");
        };

        let runs: Vec<&PlacedBlock> = pages[0]
            .blocks
            .iter()
            .filter(|b| matches!(b, PlacedBlock::Text { .. }))
            .collect();
        for r in &runs {
            let PlacedBlock::Text { frame, .. } = r else {
                panic!("expected text")
            };
            assert!((frame.x_pt - (panel.x_pt + STATBLOCK_PADDING_PT)).abs() < 0.01);
            assert!((frame.w_pt - (panel.w_pt - STATBLOCK_PADDING_PT * 2.0)).abs() < 0.01);
            assert!(frame.y_pt >= panel.y_pt + STATBLOCK_PADDING_PT - 0.01);
        }
        let last = match runs.last().unwrap() {
            PlacedBlock::Text { frame, .. } => frame.y_pt + frame.h_pt,
            _ => unreachable!(),
        };
        assert!(
            last <= panel.y_pt + panel.h_pt - STATBLOCK_PADDING_PT + 0.01,
            "the bottom padding must be inside the panel"
        );
    }

    #[test]
    fn a_stat_block_reads_in_the_order_the_component_documents() {
        // `StatBlock`'s own doc comment states the compact layout as
        // Overview / Attributes / Details / Actions / Reactions, after the name. The first draft
        // put the attributes before the overview, which puts a creature's armour class above its
        // type — visibly not a stat block, and invisible to every assertion until it was rendered.
        let mut doc = stat_doc();
        doc.content[0] = Block::StatBlock {
            id: doc.content[0].id(),
            stat: quill_core_model::StatBlock {
                name: "Name".into(),
                overview: vec!["Overview".into()],
                attributes: vec![("Attr".into(), "1".into())],
                details: vec!["Details".into()],
                actions: vec!["Actions".into()],
                reactions: vec!["Reactions".into()],
            },
            color: Color::Gray { v: 0.0 },
        };
        let pages = lay_out(&doc, &MONO, &NoHyphenator);
        let texts: Vec<String> = pages[0]
            .blocks
            .iter()
            .filter_map(|b| match b {
                PlacedBlock::Text { lines, .. } => Some(lines[0].text.clone()),
                _ => None,
            })
            .collect();
        assert_eq!(
            texts,
            [
                "Name",
                "Overview",
                "Attr: 1",
                "Details",
                "Actions",
                "Reactions"
            ]
        );
    }

    #[test]
    fn a_stat_block_rules_between_its_sections() {
        // Without them the sections run together and the panel reads as a tinted paragraph. Each
        // rule spans the padded inner width and sits between two runs, never at the very top.
        let doc = stat_doc();
        let pages = lay_out(&doc, &MONO, &NoHyphenator);
        let rects: Vec<&PlacedBlock> = pages[0]
            .blocks
            .iter()
            .filter(|b| matches!(b, PlacedBlock::Rect { .. }))
            .collect();

        // The panel, plus one rule per section boundary. The fixture has name / overview /
        // attributes / actions ⇒ three boundaries.
        assert_eq!(rects.len(), 4, "one panel and three section rules");
        let PlacedBlock::Rect { frame: panel, .. } = rects[0] else {
            unreachable!()
        };
        for rule in &rects[1..] {
            let PlacedBlock::Rect {
                frame,
                fill,
                stroke,
            } = rule
            else {
                unreachable!()
            };
            assert!(fill.is_some() && stroke.is_none(), "a rule is a thin fill");
            assert!((frame.x_pt - (panel.x_pt + STATBLOCK_PADDING_PT)).abs() < 0.01);
            assert!((frame.w_pt - (panel.w_pt - STATBLOCK_PADDING_PT * 2.0)).abs() < 0.01);
            assert!(
                frame.y_pt > panel.y_pt && frame.y_pt < panel.y_pt + panel.h_pt,
                "a rule must sit inside its panel"
            );
        }
    }

    #[test]
    fn a_stat_blocks_title_is_set_larger_than_its_body() {
        // What "looks like a stat block with zero authoring" means, as a number: the built-in
        // styles are actually applied, rather than everything coming out at body size.
        let doc = stat_doc();
        let pages = lay_out(&doc, &MONO, &NoHyphenator);
        let sizes: Vec<f32> = pages[0]
            .blocks
            .iter()
            .filter_map(|b| match b {
                PlacedBlock::Text { font_size_pt, .. } => Some(*font_size_pt),
                _ => None,
            })
            .collect();
        assert!(
            sizes[0] > sizes[1],
            "the name must be larger than an attribute line: {sizes:?}"
        );
    }

    #[test]
    fn a_stat_block_moves_whole_to_the_next_frame_rather_than_splitting() {
        // Keep-together. It is the existing pagination rule — a block moves whole when it does not
        // fit — and this asserts a stat block really is one block to that rule, rather than a group
        // of runs that could be torn apart across a page boundary.
        let mut doc = stat_doc();
        let stat = doc.content.remove(0);
        doc.content = many_lines(52);
        doc.content.push(stat);
        doc.assign_missing_block_ids().expect("ids");

        let pages = lay_out(&doc, &MONO, &NoHyphenator);
        assert!(pages.len() >= 2, "the fixture must actually paginate");

        // Every piece of the stat block landed on the same page as its panel.
        let panel_page = pages
            .iter()
            .position(|p| {
                p.blocks
                    .iter()
                    .any(|b| matches!(b, PlacedBlock::Rect { .. }))
            })
            .expect("the panel must be placed somewhere");
        let stat_id = doc.content.last().unwrap().id();
        for (i, page) in pages.iter().enumerate() {
            let from_stat = page
                .blocks
                .iter()
                .filter(|b| matches!(b, PlacedBlock::Text { source, .. } if *source == stat_id))
                .count();
            if i == panel_page {
                assert_eq!(from_stat, 5, "all runs on the panel's page");
            } else {
                assert_eq!(from_stat, 0, "no run may be orphaned onto page {i}");
            }
        }
    }

    #[test]
    fn a_stat_block_wider_than_its_frame_still_wraps_inside_the_padding() {
        // The padding must come off the measure, not just off the position — otherwise long prose
        // is broken to the full frame width and then drawn inset, so it overruns the panel.
        let mut doc = stat_doc();
        doc.content[0] = Block::StatBlock {
            id: doc.content[0].id(),
            stat: quill_core_model::StatBlock {
                actions: vec![
                    "A very long action description that will certainly need to wrap \
                               across more than one line in any reasonable measure at all."
                        .into(),
                ],
                ..goblin()
            },
            color: Color::Gray { v: 0.0 },
        };
        let pages = lay_out(&doc, &MONO, &NoHyphenator);
        let PlacedBlock::Rect { frame: panel, .. } = pages[0].blocks[0] else {
            panic!("expected a panel");
        };
        for b in &pages[0].blocks {
            let PlacedBlock::Text { frame, .. } = b else {
                continue;
            };
            assert!(
                frame.x_pt + frame.w_pt <= panel.x_pt + panel.w_pt - STATBLOCK_PADDING_PT + 0.01,
                "a run overran the panel's right padding"
            );
        }
    }

    // --- Heading index (spec 0040) --------------------------------------------------------------

    #[test]
    fn the_heading_index_reports_document_order_and_the_right_pages() {
        let mut doc = doc_with_blocks(vec![]);
        doc.content
            .push(Block::heading(1, "One", Color::Gray { v: 0.0 }));
        doc.content.extend(many_lines(60));
        doc.content
            .push(Block::heading(2, "Two", Color::Gray { v: 0.0 }));
        doc.content.extend(many_lines(60));
        doc.content
            .push(Block::heading(2, "Three", Color::Gray { v: 0.0 }));
        doc.assign_missing_block_ids().expect("ids");

        let pages = lay_out(&doc, &MONO, &NoHyphenator);
        let index = heading_index(&doc, &pages);

        assert_eq!(
            index.iter().map(|h| h.text.as_str()).collect::<Vec<_>>(),
            ["One", "Two", "Three"],
            "document order, not page order"
        );
        assert_eq!(index.iter().map(|h| h.level).collect::<Vec<_>>(), [1, 2, 2]);
        assert_eq!(index[0].page_index, 0);
        assert!(
            index[2].page_index >= index[1].page_index,
            "page numbers must be non-decreasing"
        );
        // Every entry names a real heading block.
        for h in &index {
            assert!(matches!(doc.block(h.id), Some(Block::Heading { .. })));
        }
    }

    #[test]
    fn master_furniture_never_enters_the_heading_index() {
        // A running head is text on a page and is not content. It carries `BlockId::UNASSIGNED`,
        // and an index that included it would put the folio in the table of contents.
        let master = MasterPage {
            statics: vec![MasterStatic::Text {
                rect: Rect {
                    x_pt: 0.0,
                    y_pt: 620.0,
                    w_pt: 432.0,
                    h_pt: 12.0,
                },
                text: "The Dungeon — {page}".into(),
                color: Color::Gray { v: 0.0 },
                style: Some("body".into()),
            }],
            ..MasterPage::plain("running-head")
        };
        let mut doc = doc_with_master(
            master,
            vec![Block::heading(1, "Only", Color::Gray { v: 0.0 })],
        );
        doc.assign_missing_block_ids().expect("ids");
        let pages = lay_out(&doc, &MONO, &NoHyphenator);
        assert!(
            !pages[0].statics.is_empty(),
            "the fixture must have furniture"
        );
        assert_eq!(heading_index(&doc, &pages).len(), 1);
    }

    #[test]
    fn an_empty_document_has_an_empty_heading_index() {
        let doc = doc_with_blocks(vec![]);
        let pages = lay_out(&doc, &MONO, &NoHyphenator);
        assert!(heading_index(&doc, &pages).is_empty());
    }

    #[test]
    fn a_document_of_only_body_text_has_an_empty_heading_index() {
        // The reuse direction: an index that reported every text block would pass the tests above.
        let doc = doc_with_blocks(many_lines(20));
        let pages = lay_out(&doc, &MONO, &NoHyphenator);
        assert!(heading_index(&doc, &pages).is_empty());
    }

    #[test]
    fn many_images_all_place_through_the_asset_index() {
        // Resolution used to be a linear scan of `assets` run once per candidate frame per block —
        // quadratic, on exactly the workload this engine exists for. This asserts correctness at
        // scale (every block placed, none silently skipped); the timing claim belongs to the bench
        // harness in spec 0027, not to a unit test on a shared runner.
        const N: usize = 2_000;
        let assets: Vec<Asset> = (0..N)
            .map(|i| Asset {
                id: format!("img{i}"),
                path: format!("assets/img{i}.png"),
                px_w: 300,
                px_h: 300,
                dpi: 300.0,
                line_art: false,
                has_alpha: false,
            })
            .collect();
        let content: Vec<Block> = (0..N).map(|i| Block::image(format!("img{i}"))).collect();

        let mut doc = doc_with_blocks(content);
        doc.assets = assets;
        let pages = lay_out(&doc, &MONO, &NoHyphenator);

        let placed = pages
            .iter()
            .flat_map(|p| p.blocks.iter())
            .filter(|b| matches!(b, PlacedBlock::Image { .. }))
            .count();
        assert_eq!(placed, N, "every image block must be placed");
    }

    #[test]
    fn an_unknown_asset_id_is_still_skipped_without_panicking() {
        // Behavior preserved across the index change: an unresolvable image is skipped, not fatal.
        let mut doc = doc_with_blocks(vec![
            Block::image("nope"),
            Block::body("after", Color::Gray { v: 0.0 }),
        ]);
        doc.assets = vec![];
        let pages = lay_out(&doc, &MONO, &NoHyphenator);
        let images = pages
            .iter()
            .flat_map(|p| p.blocks.iter())
            .filter(|b| matches!(b, PlacedBlock::Image { .. }))
            .count();
        assert_eq!(images, 0);
        let texts = pages
            .iter()
            .flat_map(|p| p.blocks.iter())
            .filter(|b| matches!(b, PlacedBlock::Text { .. }))
            .count();
        assert_eq!(texts, 1, "the block after a skipped image must still place");
    }
}
