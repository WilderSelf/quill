//! Layout engine: turns a document into positioned pages.
//!
//! The real engine is incremental and dependency-tracked (see the plan) so that editing one
//! text thread reflows only affected pages. This scaffold lays content out naively — stacking
//! text and image blocks, paginating when a block would exceed the page height — so downstream
//! crates compile and the export pipeline has something to consume. Uses `quill-text-layout`
//! for line breaking.

use std::cell::RefCell;
use std::collections::{BTreeMap, BTreeSet};

mod component;
mod session;

use component::measure_component;
pub use session::{LayoutResult, LayoutSession, LayoutStats};

use quill_core_model::{
    builtin_components, toc_entry_style_name, Asset, BaselineGrid, Block, BlockId, BreakKind,
    CharacterStyle, Color, ComponentLibrary, Document, FolioRun, IndexMark, ListMarker, Margins,
    MasterPage, MasterStatic, NumberFormat, PageBreak, PageOverride, PageSetup, ParagraphStyle,
    Rect, RunSource, StaticToken, StyleSheet, TextAlign, INDEX_ENTRY_STYLE, INDEX_TITLE_STYLE,
    STATBLOCK_COMPONENT, TABLE_COMPONENT, TOC_TITLE_STYLE, UNRESOLVED_REFERENCE,
};
use quill_text_layout::{
    justify_runs_indented, Alignment, Hyphenator, Line, RunFormat, RunMetrics,
};

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
///
/// # `frame` is the ink (spec 0069)
///
/// **A placed part's `frame` is the bounding box of the ink that part actually draws**: `x_pt` is
/// the left edge of its leftmost glyph, `x_pt + w_pt` the right edge of its rightmost, and `h_pt`
/// follows the same rule vertically — the line boxes drawn, never the space reserved around them.
/// It is *not* the slot the content was laid into. Every producer in this crate is checked against
/// that one sentence, and the three exceptions it used to have are gone.
///
/// The slot is not lost by this: a frame's geometry and the stylesheet still say what measure a
/// paragraph was broken to, and both are available without going through placed output. What is
/// *not* recoverable downstream is the ink, because `PlacedBlock` carries no alignment field — a
/// right-aligned segment in a wide slot is indistinguishable from a left-aligned one, and under
/// slot semantics its reported box is nowhere near its glyphs. Both consumers of this geometry want
/// ink: spec 0050's safe-area check asks whether ink lands in the margin, and `under_dpi` divides
/// pixels by the width the image was *drawn* at.
///
/// **A multi-line paragraph is a bounding box, not an advance.** [`Line::indent_pt`] is added at
/// draw time and differs per line under a hanging indent (spec 0048), so no single line's box is
/// the paragraph's:
///
/// ```text
/// x_pt = measure_left + min over lines of indent_pt
/// w_pt = max over lines of (indent_pt + advance) - min over lines of indent_pt
/// ```
///
/// [`quill_text_layout::ink_box`] is the one place that computes it, and both painters subtract
/// [`quill_text_layout::indent_base`] so the change moves no glyph. A single line at zero indent —
/// a tab segment, a master static, a list marker — is the degenerate case.
///
/// **[`PlacedBlock::Image`] is the one variant where the old rule and the new agree**, and it keeps
/// drawn-extent semantics exactly: `under_dpi` divides a pixel count by `frame.w_pt`, so anything
/// else would silently mis-report resolution. That is a press-safety regression rather than a
/// cosmetic one.
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
        /// The resolved ink of each authored run, indexed by [`quill_text_layout::Span::run`]
        /// (spec 0063). Empty means the block predates runs or has exactly one, and every span
        /// takes `color` — the path the writer and the painter took before runs existed.
        #[allow(clippy::doc_markdown)]
        run_colors: Vec<Color>,
        /// Each authored run's resolved face, size and tracking (spec 0064), indexed the same way as
        /// `run_colors`. Empty means one format for the whole block — the path the writer and the
        /// painter took before the family existed, and the one every unstyled document still takes.
        run_formats: Vec<RunFormat>,
        /// Each authored run's baseline shift in points, positive up. Empty when none shifts.
        run_shifts: Vec<f32>,
        /// The face the block itself is set in — what a span with no override of its own resolves
        /// to (spec 0064). Carried for the same reason `font_size_pt` is: `PlacedBlock` is all the
        /// writer and the screen renderer see.
        weight: u16,
        italic: bool,
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
    /// A **candidate** internal link: this rectangle, if the export target allows annotations,
    /// navigates to `target_page` (spec 0052).
    ///
    /// It draws nothing — not on screen, not on press. It is geometry plus a destination, and what
    /// becomes of it is the *exporter's* decision: the `Screen` profile turns it into a `/Link`
    /// annotation, and the `Press` profile puts it through spec 0042's `annotation_finding` and
    /// therefore emits nothing at all, since PDF/X requires annotations outside the BleedBox and
    /// quill writes `MediaBox == BleedBox`.
    ///
    /// Emitted in *both* profiles on purpose. Layout must not depend on the export target: a
    /// document that paginated differently per profile would make the press and screen files
    /// disagree about what is on which page, which is the one difference a reader comparing them
    /// would actually notice.
    Link {
        frame: Rect,
        /// See [`PlacedBlock::Text::source`].
        source: BlockId,
        /// Zero-based index of the page this link navigates to.
        target_page: usize,
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
    /// Kept separate from `blocks` because it is drawn first, so master art sits behind flowed
    /// content.
    ///
    /// **Until spec 0074 this also said incremental relayout could leave it alone, "since it does
    /// not depend on where the text happened to break". That is now false and the sentence is
    /// removed rather than softened.** A `{section}` or `{heading:N}` running head is a function of
    /// which content landed on the page, so a reused page's furniture can name the *previous*
    /// chapter — a wrong running head on a printed page, produced silently. Statics are therefore
    /// resolved by `place_statics` in one post-pass over the finished page vector, for every page,
    /// reused or not; nothing else may fill this field.
    pub statics: Vec<PlacedBlock>,
}

/// One heading, and the page it landed on (spec 0040).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct HeadingEntry {
    pub id: BlockId,
    pub level: u8,
    pub text: String,
    /// Zero-based page index — the heading's *physical* position in the document.
    ///
    /// This is what a link or a bookmark destination targets, and it is deliberately **not** what a
    /// contents list prints. Since spec 0073 those are two different numbers: page 3 of a book whose
    /// front matter is roman is the fourth page of the file and prints `iv`.
    pub page_index: usize,
    /// What the page the heading landed on actually prints (spec 0073) — its folio.
    ///
    /// The string a contents list shows, so that the number beside a chapter title is the number a
    /// reader will find printed on the page it sends them to. `(page_index + 1).to_string()` for
    /// every document that states no folio format, which is the pre-0073 behaviour exactly.
    ///
    /// Filled by whoever has the numbering to hand: [`heading_index`] resolves it from the document,
    /// and the layout fixpoint — which holds a template rather than a document — resolves it through
    /// [`PageTemplate::folio`]. Both call [`Document::folio_in`], so there is one implementation of
    /// what a folio says and two ways of reaching it, not two answers.
    pub folio: String,
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
/// A heading appearing more than once in the page vector reports its **first** page. Since spec
/// 0044 a long heading really can split across a frame boundary and appear twice, so this is a live
/// rule rather than a precaution: a TOC entry and a bookmark both mean "where does this start".
///
/// Master furniture is skipped: it carries [`BlockId::UNASSIGNED`] and is not content.
/// Each entry's `folio` is resolved from the document's own sections (spec 0073), so a caller
/// holding a `Document` — the incremental session, the PDF outline — gets the printed number
/// without re-deriving the numbering itself.
pub fn heading_index(doc: &Document, pages: &[LaidOutPage]) -> Vec<HeadingEntry> {
    let mut out = heading_index_of(&doc.content, pages);
    // The same `starts` the template derived, from the same page vector, through the same public
    // function — see `HeadingEntry::folio`. Computed once for the whole index rather than per entry.
    let runs = doc.folio_runs(&section_starts(doc, pages));
    for entry in &mut out {
        entry.folio = Document::folio_in(&runs, entry.page_index);
    }
    out
}

/// [`heading_index`] over a bare block list, for callers that have no `Document` — the TOC fixpoint
/// (spec 0041) runs inside `lay_out_with_template`, which takes content rather than a document.
///
/// Every entry's `folio` is the **default** numbering, arabic and one-based, because a block list
/// carries no sections to say otherwise. A caller whose document has folio formats must resolve
/// them: the fixpoint does it through [`PageTemplate::folio`], and [`heading_index`] does it from
/// the document.
pub fn heading_index_of(content: &[Block], pages: &[LaidOutPage]) -> Vec<HeadingEntry> {
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
            let Some(block @ Block::Heading { level, .. }) =
                content.iter().find(|b| b.id() == *source)
            else {
                continue;
            };
            let text = &block.plain_text().unwrap_or_default();
            out.push(HeadingEntry {
                id: *source,
                level: *level,
                text: text.clone(),
                page_index: page.index,
                folio: (page.index + 1).to_string(),
            });
        }
    }
    out
}

/// The page each of `doc`'s sections opens on (spec 0072), parallel to [`Document::sections`].
///
/// `None` where the anchor block is not in the page vector at all — it was deleted, or the id names
/// nothing. That is not an error: the section contributes no assignment and the pages it would have
/// governed keep the document's default, which is the same posture a dangling master name gets.
///
/// Derived from the **laid-out pages** for exactly the reason [`heading_index`] is, and the reason
/// is load-bearing rather than stylistic: an incremental pass reuses whole pages, so a start
/// accumulated during pagination would be missing for every section on a reused page — precisely
/// when the document has just been edited. A block placed on more than one page (spec 0044 can split
/// one) reports its **first**, because a section starts where its opener starts.
pub fn section_starts(doc: &Document, pages: &[LaidOutPage]) -> Vec<Option<usize>> {
    let mut first: BTreeMap<BlockId, usize> = BTreeMap::new();
    for page in pages {
        for placed in &page.blocks {
            // Every variant that carries an identity, not just `Text`: a section may be anchored to
            // an image, a table or a stat block as readily as to a heading, and a composite's parts
            // all carry the composite's id. `Rect` is furniture within a block and has none.
            let source = match placed {
                PlacedBlock::Text { source, .. }
                | PlacedBlock::Image { source, .. }
                | PlacedBlock::Link { source, .. } => *source,
                PlacedBlock::Rect { .. } => BlockId::UNASSIGNED,
            };
            if source.is_assigned() {
                first.entry(source).or_insert(page.index);
            }
        }
    }
    doc.sections
        .iter()
        .map(|s| first.get(&s.start).copied())
        .collect()
}

/// [`heading_index_of`] with each entry's folio resolved through the template (spec 0073).
///
/// What the layout fixpoint uses. It holds a `&impl PageTemplate` and no `Document`, so the
/// numbering has to come from the template — and it must, because a contents list that printed
/// `4` beside a chapter whose page prints `iv` sends the reader to a page that does not answer to
/// the number they were given.
pub fn heading_index_with_folios(
    content: &[Block],
    pages: &[LaidOutPage],
    template: &impl PageTemplate,
) -> Vec<HeadingEntry> {
    let mut out = heading_index_of(content, pages);
    for entry in &mut out {
        entry.folio = template.folio(entry.page_index);
    }
    out
}

/// One index term, and the pages it was marked on (spec 0078).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct IndexEntry {
    /// The term as it prints.
    pub term: String,
    /// Zero-based page indices the term was marked on, ascending and deduplicated.
    ///
    /// Physical positions, like [`HeadingEntry::page_index`] and for the same reason: this is what a
    /// destination would target, and it is deliberately **not** what the entry prints.
    pub pages: Vec<usize>,
    /// What the entry prints after the term: `iv, 42–45, 47`.
    ///
    /// Folios (spec 0073), coalesced (spec 0078). Spec 0073's split falls on the reader's side here
    /// exactly as it does for a contents entry: an index that says `4` for a page printed `iv`
    /// sends the reader to a page that does not answer to the number they were given.
    pub folios: String,
}

/// The index a document's marked runs produce, collated and page-ranged (spec 0078).
///
/// Derived from the **laid-out pages** rather than accumulated during pagination — [`heading_index`]'s
/// decision, and load-bearing for the same reason: an incremental pass reuses whole pages, so
/// anything counted while placing goes missing on a reused page, and goes missing precisely when the
/// document has just been edited.
///
/// **A mark's page is the page its own run landed on**, not the first page of its block, and that is
/// the part worth being exact about. A placed paragraph carries the span map spec 0063 built — which
/// authored run every stretch of every line came from — so a paragraph cut across a page boundary
/// (spec 0044) reports a term marked in its last sentence on the page that sentence is actually on.
/// Falling back to the block's first page would be right for every paragraph that fits a frame and
/// wrong for exactly the long ones an index is for. The fallback is still there for the case that
/// has no span at all: a mark on a run whose text is empty, or on a paragraph positioned by tab
/// stops, which is laid as panel parts and carries no run map.
///
/// Returns empty when nothing is marked, and every caller skips the walk in that case, so a document
/// with no index pays nothing.
pub fn index_entries(
    content: &[Block],
    pages: &[LaidOutPage],
    template: &impl PageTemplate,
    ignore_leading: &[String],
) -> Vec<IndexEntry> {
    // (block, run index) → mark, in document order. Empty for every document with no marks.
    let marks: Vec<(BlockId, usize, &IndexMark)> = content
        .iter()
        .flat_map(|b| {
            let id = b.id();
            b.runs()
                .iter()
                .enumerate()
                .filter_map(move |(i, r)| r.index.as_ref().map(|m| (id, i, m)))
        })
        .collect();
    if marks.is_empty() {
        return Vec::new();
    }

    // First page each (block, run) was drawn on, and first page each block was placed on.
    let mut run_page: BTreeMap<(BlockId, usize), usize> = BTreeMap::new();
    let mut block_page: BTreeMap<BlockId, usize> = BTreeMap::new();
    for page in pages {
        for placed in &page.blocks {
            let PlacedBlock::Text { source, lines, .. } = placed else {
                continue;
            };
            if !source.is_assigned() {
                continue;
            }
            block_page.entry(*source).or_insert(page.index);
            for line in lines {
                for span in &line.spans {
                    run_page.entry((*source, span.run)).or_insert(page.index);
                }
            }
        }
    }

    // Term → the mark that decides its filing, plus its pages. Keyed by the *printed* term, so two
    // marks that print the same thing are one entry however they are spelled in the manifest —
    // which is what makes "a term marked twice on one page appears once" true by construction
    // rather than by a later pass. The mark kept is the first in document order that states a
    // `sort_as`, else the first: an author who files a term two ways has contradicted themselves,
    // and taking the earlier statement is the only tie-break that does not depend on where in the
    // book each mention happened to land.
    let mut grouped: BTreeMap<&str, (&IndexMark, BTreeSet<usize>)> = BTreeMap::new();
    for (block, run, mark) in &marks {
        let Some(page) = run_page
            .get(&(*block, *run))
            .or_else(|| block_page.get(block))
            .copied()
        else {
            // Marked in a block the pass did not place — deleted, or an image whose asset is
            // missing. It contributes no page, and a term with no page contributes no entry: the
            // reader cannot turn to a page that has nothing on it (spec 0076's answer for an
            // unplaced reference target, at the one other site that asks the question).
            continue;
        };
        let slot = grouped
            .entry(mark.term.as_str())
            .or_insert_with(|| (*mark, BTreeSet::new()));
        if slot.0.sort_as.is_none() && mark.sort_as.is_some() {
            slot.0 = *mark;
        }
        slot.1.insert(page);
    }

    let mut out: Vec<(quill_core_model::CollationKey, IndexEntry)> = grouped
        .into_values()
        .map(|(mark, pages_set)| {
            let pages: Vec<usize> = pages_set.into_iter().collect();
            // The **folio**, not the page index, and its numbering rather than its text: coalescing
            // `ix` with `1` would print a range that does not exist, and the two strings alone
            // cannot say so (spec 0078).
            let numbering: Vec<(NumberFormat, u32)> =
                pages.iter().map(|p| template.folio_number(*p)).collect();
            (
                quill_core_model::collation_key(mark, ignore_leading),
                IndexEntry {
                    term: mark.term.clone(),
                    folios: quill_core_model::coalesce_folios(&numbering),
                    pages,
                },
            )
        })
        .collect();
    // The collation is a *total* order (see `quill_core_model::collate`), so this is deterministic
    // whatever order the marks were found in — which matters, because the marks are found in
    // document order and an index whose ties broke by first mention would reorder itself when a
    // paragraph moved.
    out.sort_by(|a, b| a.0.cmp(&b.0));
    out.into_iter().map(|(_, e)| e).collect()
}

/// The article list the document's index block declares, or empty (spec 0078).
///
/// One function rather than the same `find`/`match` at the three sites that ask — the two fixpoints
/// and the session's context fingerprint — because the collation an index is *rendered* with and the
/// collation it is *derived* with must be the same one.
pub fn index_ignore_leading(content: &[Block]) -> &[String] {
    content
        .iter()
        .find_map(|b| match b {
            Block::Index { ignore_leading, .. } => Some(ignore_leading.as_slice()),
            _ => None,
        })
        .unwrap_or(&[])
}

/// What a page's furniture may read off the document's content (spec 0074).
///
/// One field today, and a struct rather than a bare slice for `BlockContext`'s stated reason: the
/// list of ambient inputs grew by one with each of specs 0026, 0028 and 0041, and passing them as a
/// unit means the next addition changes one struct rather than every signature between here and the
/// template. A cross-reference's resolved page (spec 0076) and a footnote's number (0077) are the
/// next two things that will want to be here.
#[derive(Clone, Copy)]
pub struct StaticContext<'a> {
    /// Where each heading landed (spec 0040), in document order — the same index a contents block
    /// renders from, derived from the **final** page vector rather than accumulated during flow.
    pub headings: &'a [HeadingEntry],
}

impl StaticContext<'_> {
    /// The context a template with no content-derived furniture needs.
    pub const EMPTY: StaticContext<'static> = StaticContext { headings: &[] };

    /// The text of the last heading of level ≤ `level` at or before `page_index` — what
    /// `{heading:N}` prints.
    ///
    /// "At or before" rather than "on", because a chapter's second page is still in that chapter and
    /// carries no heading of its own. `None` before the first qualifying heading: a running head
    /// ahead of chapter one prints nothing rather than borrowing a later chapter's name.
    ///
    /// A heading that opens a page names that page. That is a real decision rather than a fallout of
    /// `<=`: the alternative — a page's running head naming the chapter it *came from* until the
    /// next page — is what some house styles want for a verso, and it is expressible by authoring the
    /// token on one master rather than by changing this rule.
    pub fn heading_at(&self, page_index: usize, level: u8) -> Option<&str> {
        self.headings
            .iter()
            .rfind(|h| h.level <= level && h.page_index <= page_index)
            .map(|h| h.text.as_str())
    }
}

/// Replace every layout-time token in a master static's text (spec 0074).
///
/// Public because it is the resolver half of the contract [`quill_core_model::StaticToken`]
/// describes: a [`PageTemplate`] implementation that draws its own furniture should reach for this
/// rather than write a second parser, because a second parser is a second answer to what a token is.
///
/// The `match` below is exhaustive, which is half of what makes adding a token without teaching the
/// font-subset collector fail to compile — the other half is `StaticToken::contributes`.
///
/// A `{…}` group that spells no token is copied through untouched. That is what makes an older build
/// meeting a newer token **loud**: it prints the literal `{section}` on the page, which is visibly
/// wrong, rather than printing something plausible and wrong.
pub fn resolve_static_text(
    text: &str,
    page_index: usize,
    folio: &str,
    section: Option<&str>,
    ctx: &StaticContext<'_>,
) -> String {
    let tokens = StaticToken::scan(text);
    if tokens.is_empty() {
        return text.to_string();
    }
    let mut out = String::with_capacity(text.len());
    let mut cursor = 0;
    for (range, token) in tokens {
        out.push_str(&text[cursor..range.start]);
        match token {
            StaticToken::Page => out.push_str(folio),
            StaticToken::Section => out.push_str(section.unwrap_or_default()),
            StaticToken::Heading { level } => {
                out.push_str(ctx.heading_at(page_index, level).unwrap_or_default())
            }
        }
        cursor = range.end;
    }
    out.push_str(&text[cursor..]);
    out
}

/// Resolve every page's furniture, once, over the finished page vector (spec 0074).
///
/// **The one place [`LaidOutPage::statics`] is filled**, and that is the whole structural answer to
/// the tail-page-reuse defect. Furniture used to be stamped on a page as the flow created it, which
/// was sound only while a static was a pure function of `page_index`; the incremental session reuses
/// whole pages verbatim and re-asserts nothing but `page.index`, so a `{heading:1}` running head on
/// a reused page would have kept naming the chapter that was there before the edit. Running over
/// every page — reused, kept and reflowed alike — makes that unrepresentable rather than guarded
/// against.
///
/// Costs no extra layout pass: statics are furniture, consume no flow space, and so cannot move a
/// line break (the M6 audit's finding, and the reason a running head is not a fixpoint).
pub(crate) fn place_statics(
    template: &impl PageTemplate,
    pages: &mut [LaidOutPage],
    headings: &[HeadingEntry],
    metrics: &impl RunMetrics,
) {
    let ctx = StaticContext { headings };
    for page in pages.iter_mut() {
        page.statics = template.statics(page.index, metrics, &ctx);
    }
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

    /// Content the template draws on `page_index`.
    ///
    /// `metrics` is the same run-metrics the page's text is broken with. Furniture needs it because
    /// a static can be aligned within its rect (spec 0047), and "centred" is a statement about the
    /// line's measured width — resolving it anywhere else would mean measuring text twice, in two
    /// crates, and hoping the two agree.
    ///
    /// `ctx` is the **content channel** spec 0074 added: `{heading:N}` names the chapter a page is
    /// in, which is not a function of the page number. It is a parameter rather than a field on the
    /// template because the heading index is derived from the laid-out pages — the template is built
    /// before there are any — and because `BlockContext::headings` is the same channel for the same
    /// quantity one layer down.
    ///
    /// Called by `place_statics` and nowhere else, once per page, *after* the flow. Resolving
    /// furniture during the flow would have to use the previous pass's heading index, which is stale
    /// by exactly the amount the pass just changed; resolving it afterwards costs zero extra passes,
    /// which is sound only because furniture consumes no flow space and so cannot move a line break.
    fn statics(
        &self,
        _page_index: usize,
        _metrics: &dyn RunMetrics,
        _ctx: &StaticContext<'_>,
    ) -> Vec<PlacedBlock> {
        Vec::new()
    }

    /// The name of the section `page_index` belongs to (spec 0074), or `None`.
    ///
    /// On the template for [`PageTemplate::folio`]'s reason, and it is the same shape: the sections'
    /// start pages are derived by [`PageTemplate::reassign`] and live on the template, so the
    /// template is the only thing that can answer. The default is `None` — a template that knows
    /// nothing about sections has no section to name, and `{section}` prints nothing there.
    fn section_name(&self, _page_index: usize) -> Option<String> {
        None
    }

    /// Whether this template's furniture reads the heading index — i.e. uses `{heading:N}`.
    ///
    /// Purely a cost question, and it exists because deriving the heading index is a walk over every
    /// placed block. A document whose masters use no heading token must not pay for one, which is
    /// what makes "a document using no new token lays out exactly as it did before" a measurable
    /// claim rather than a hope. Derived from [`quill_core_model::StaticToken::scan`] rather than
    /// from a hand-kept flag, so it cannot disagree with the resolver about what a token is.
    fn statics_read_headings(&self) -> bool {
        false
    }

    /// The baseline grid this template's pages are set to (spec 0058), or `None` for none.
    ///
    /// On the template rather than passed alongside it, because a grid is page geometry and the
    /// template is what owns page geometry. Defaulted to `None`, so every existing implementation —
    /// and every ungridded document — is unchanged.
    fn baseline_grid(&self) -> Option<BaselineGrid> {
        None
    }

    /// Re-derive whatever this template computes from where content actually landed (spec 0072),
    /// returning **whether the derivation changed**.
    ///
    /// A section is anchored to a block, so the page it opens on is not known until the document has
    /// been laid out — and the master it puts on that page changes column count and margins, which
    /// changes where every later block falls, which can move the section. So this joins the fixpoint
    /// the contents list already runs: lay out, re-derive, and stop when nothing moves. Returning
    /// `false` is what ends the loop, so a template that derives nothing costs exactly zero extra
    /// passes — which is why the default is `false` and why every document without sections or a
    /// contents list still takes one pass.
    ///
    /// Takes `&self` because the flow loop only ever holds a shared reference to its template;
    /// [`DocumentTemplate`] keeps its derived state behind a `RefCell` rather than every caller
    /// threading `&mut` through forty call sites for a capability one implementation has.
    fn reassign(&self, _pages: &[LaidOutPage]) -> bool {
        false
    }

    /// A fingerprint of everything [`PageTemplate::reassign`] derived, or `0` if nothing.
    ///
    /// The incremental session's `context_fingerprint` is what stops a stale page being reused, and
    /// it hashes the *document*. A derived assignment is not on the document — it is on the template
    /// — so without this a fixpoint iteration that changed the assignment but nothing else would
    /// look to the session like "nothing changed", and it would hand back the previous iterate's
    /// pages. That is exactly the shape of defect spec 0075 found in the contents list, in the one
    /// place it could recur.
    fn derived_fingerprint(&self) -> u64 {
        0
    }

    /// What `page_index` prints as its page number — its **folio** (spec 0073).
    ///
    /// The default is `page_index + 1`: arabic, one-based, which is the single expression `{page}`
    /// resolved to before this existed and is what every template that knows nothing about sections
    /// still answers.
    ///
    /// On the template rather than passed alongside it, for the reason [`PageTemplate::frames`] is:
    /// numbering is page geometry in the same sense margins are — a property of the page, decided by
    /// whatever governs pages. It is also what lets the layout fixpoint, which holds a template and
    /// not a document, print the same number in a contents list that the folio prints on the page.
    ///
    /// A folio consumes no flow space (it is furniture, `LaidOutPage::statics`), so this can never
    /// move a line break and adds **zero** fixpoint iterations.
    fn folio(&self, page_index: usize) -> String {
        (page_index + 1).to_string()
    }

    /// The numbering behind [`PageTemplate::folio`] — the format and the number, before either
    /// becomes text (spec 0078).
    ///
    /// An index coalesces page ranges, and that cannot be done on the printed strings: `iv` and `v`
    /// join and `ix` and `1` do not, and telling those apart means comparing the format and the
    /// arithmetic. The default matches [`PageTemplate::folio`]'s default exactly, and
    /// `a_template_folio_is_its_own_numbering_formatted` asserts the two agree for every
    /// implementation in the workspace — one number, printed one way, whichever question is asked.
    fn folio_number(&self, page_index: usize) -> (NumberFormat, u32) {
        (NumberFormat::Decimal, page_index as u32 + 1)
    }

    /// Whether this template's pages are a bound spread — the input to page parity (spec 0080).
    ///
    /// Asked of the template rather than of the document for [`PageTemplate::folio`]'s reason: the
    /// flow holds a template and not a document, and which side of the spread a page falls on is
    /// page geometry in exactly the sense margins are. It resolves through
    /// [`quill_core_model::is_recto`] at the one site that reads it, so a recto break and a mirrored
    /// margin cannot disagree about what a recto is.
    ///
    /// The default is `false` — a bare thread of frames is not a bound book — and with facing pages
    /// off every break kind accepts every page, which is the same answer `Margins::left_right`
    /// already gives a single-sided document.
    fn facing_pages(&self) -> bool {
        false
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
///
/// Since spec 0072 the assignment it resolves against is not necessarily the document's authored
/// one: a [`Section`] is anchored to a block, and the page that block lands on is only known once
/// the document has been laid out. [`PageTemplate::reassign`] is where that derivation happens, and
/// `assignment` below is what it produces. Before the first pass — and for every document without
/// sections — it is `doc.pages`, unchanged.
pub struct DocumentTemplate<'a> {
    doc: &'a Document,
    page_setup: &'a PageSetup,
    styles: &'a StyleSheet,
    /// What the sections derived, and the assignment synthesised from it.
    ///
    /// Behind a `RefCell` because the flow loop holds `&dyn PageTemplate`: the alternative is `&mut`
    /// at every call site in the crate for a capability one implementation has. Borrows are taken
    /// for the length of one field read and never held across a call back into the template, so
    /// there is no re-entrancy to get wrong.
    derived: RefCell<Derived>,
}

/// What a [`DocumentTemplate`] derives from a laid-out document (spec 0072).
#[derive(Debug, Clone, PartialEq)]
struct Derived {
    /// Page each section starts on, parallel to `doc.sections`; `None` when the anchor block is not
    /// in the document or was not placed.
    starts: Vec<Option<usize>>,
    /// The per-page assignment synthesised from `starts` — see [`Document::page_assignment`].
    assignment: Vec<PageOverride>,
    /// The page numbering synthesised from `starts` (spec 0073) — see [`Document::folio_runs`].
    ///
    /// Held beside the assignment rather than recomputed per page because both are derived from the
    /// same `starts` by the same `reassign`, and a second derivation site is a second thing that can
    /// disagree. It is re-derived wholesale every pass, so it caches nothing across a change and
    /// owes spec 0075's eviction nothing.
    folio_runs: Vec<FolioRun>,
}

impl<'a> DocumentTemplate<'a> {
    pub fn new(doc: &'a Document) -> DocumentTemplate<'a> {
        DocumentTemplate {
            doc,
            page_setup: &doc.page_setup,
            styles: &doc.styles,
            // Nothing has been laid out yet, so no section has a start and the assignment is the
            // authored one. A document with no sections never leaves this state.
            derived: RefCell::new(Derived {
                starts: vec![None; doc.sections.len()],
                assignment: doc.pages.clone(),
                folio_runs: doc.folio_runs(&vec![None; doc.sections.len()]),
            }),
        }
    }

    /// The master governing `page_index`, or none.
    fn master(&self, page_index: usize) -> Option<&'a MasterPage> {
        self.doc
            .master_in(&self.derived.borrow().assignment, page_index)
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

    fn baseline_grid(&self) -> Option<BaselineGrid> {
        // Measured from the top of the trim box, which is the space the flow cursor already lives
        // in — so two columns of one page and the two pages of a spread share one ladder.
        self.page_setup
            .baseline_grid
            .filter(BaselineGrid::is_usable)
    }

    fn statics(
        &self,
        page_index: usize,
        metrics: &dyn RunMetrics,
        ctx: &StaticContext<'_>,
    ) -> Vec<PlacedBlock> {
        let Some(master) = self.master(page_index) else {
            return Vec::new();
        };
        // Resolved once for the page rather than once per static: both read a `RefCell` the flow
        // loop holds only shared access to, and a master may carry several statics.
        let folio = self.folio(page_index);
        let section = self.section_name(page_index);
        master
            .statics
            .iter()
            .map(|s| {
                // Parity mirroring happens first and for both variants' geometry: the rect a static
                // is aligned *within* is the one it actually occupies on this page (spec 0047).
                let rect = s.rect_on(page_index, self.page_setup);
                match s {
                    MasterStatic::Text {
                        text,
                        color,
                        style,
                        align,
                        ..
                    } => {
                        // Every layout-time token at once (spec 0074), through the one parser the
                        // font-subset collector also reads: `{page}` is the page's folio (spec
                        // 0073), `{section}` its section's name, `{heading:N}` the chapter it is
                        // in. A static with no token is returned unchanged, so a document that uses
                        // none of them resolves to exactly the string it always did.
                        let resolved =
                            resolve_static_text(text, page_index, &folio, section.as_deref(), ctx);
                        let ps = style
                            .as_deref()
                            .and_then(|n| self.styles.paragraph.get(n).copied())
                            .unwrap_or_default();
                        // The placed frame is narrowed to the line it holds, so it reports where the
                        // text *is* rather than the box it was aligned in — which is what a geometry
                        // preflight over placed content has to ask.
                        // Measured in the face the static is drawn in (spec 0064): a bold running
                        // head measured regular would be aligned to a width it does not have, and
                        // would report that width to a geometry preflight.
                        let line_w = metrics.measure_format(
                            &resolved,
                            RunFormat {
                                size_pt: ps.font_size_pt,
                                weight: ps.weight.0,
                                italic: ps.italic,
                                tracking_pt: 0.0,
                            },
                        );
                        let x_pt =
                            align.x_for(rect, line_w, page_index, self.page_setup.facing_pages);
                        PlacedBlock::Text {
                            frame: Rect {
                                x_pt,
                                w_pt: line_w,
                                ..rect
                            },
                            // Furniture is not content: it has no block, so no identity.
                            source: BlockId::UNASSIGNED,
                            run_formats: Vec::new(),
                            run_shifts: Vec::new(),
                            weight: ps.weight.0,
                            italic: ps.italic,
                            // Master furniture is a single line at a fixed position; it is not flowed,
                            // so it is not broken. A running head that overflows its rect is an
                            // authoring problem, and one that is visible on screen.
                            lines: vec![Line {
                                spans: vec![quill_text_layout::Span {
                                    run: 0,
                                    len_bytes: resolved.len(),
                                }],
                                text: resolved,
                                space_adjust_pt: 0.0,
                                indent_pt: 0.0,
                            }],
                            color: *color,
                            // Furniture is one run by construction.
                            run_colors: Vec::new(),
                            font_size_pt: ps.font_size_pt,
                            leading_pt: ps.leading_pt,
                        }
                    }
                    MasterStatic::Image { asset, .. } => PlacedBlock::Image {
                        frame: rect,
                        source: BlockId::UNASSIGNED,
                        asset_id: asset.clone(),
                    },
                }
            })
            .collect()
    }

    /// Re-derive the per-page assignment from where the section anchors actually landed (spec 0072).
    ///
    /// Two lines of work and one comparison: read the starts off the page vector, synthesise the
    /// assignment from them, and report whether the *starts* moved. The comparison is on the starts
    /// rather than on the assignment because that is the quantity the fixpoint is over — two
    /// different sets of starts can synthesise the same assignment (a section that names no master
    /// contributes nothing), and reporting "changed" for one of those would iterate for no reason.
    fn reassign(&self, pages: &[LaidOutPage]) -> bool {
        let starts = section_starts(self.doc, pages);
        let mut derived = self.derived.borrow_mut();
        if starts == derived.starts {
            return false;
        }
        derived.assignment = self.doc.page_assignment(&starts);
        // Both derivations move together, off the one `starts` (spec 0073). The folio does *not*
        // get its own comparison: it cannot move a line break, so a pass in which only the numbering
        // changed has nothing left to settle.
        derived.folio_runs = self.doc.folio_runs(&starts);
        derived.starts = starts;
        true
    }

    fn folio(&self, page_index: usize) -> String {
        Document::folio_in(&self.derived.borrow().folio_runs, page_index)
    }

    fn folio_number(&self, page_index: usize) -> (NumberFormat, u32) {
        // The same `folio_runs` `folio` reads, through the function `folio_in` itself calls, so the
        // number an index coalesces and the number a page prints cannot be two numbers.
        Document::folio_number_in(&self.derived.borrow().folio_runs, page_index)
    }

    fn section_name(&self, page_index: usize) -> Option<String> {
        // Off the same `starts` the assignment and the folio runs come from (spec 0072), so a
        // running head, a chapter opener's master and a roman folio cannot disagree about which page
        // a section begins on.
        self.doc
            .section_at(&self.derived.borrow().starts, page_index)
            .map(str::to_string)
    }

    fn facing_pages(&self) -> bool {
        // The document's own answer, not a second one: `Margins::left_right` and `StaticAlign`
        // already resolve parity from this field, and a recto break that read a different one would
        // put a chapter opener on a page whose margins mirror the other way.
        self.page_setup.facing_pages
    }

    fn statics_read_headings(&self) -> bool {
        self.doc
            .master_pages
            .iter()
            .flat_map(|m| &m.statics)
            .any(|s| match s {
                MasterStatic::Text { text, .. } => StaticToken::scan(text)
                    .iter()
                    .any(|(_, t)| matches!(t, StaticToken::Heading { .. })),
                MasterStatic::Image { .. } => false,
            })
    }

    fn derived_fingerprint(&self) -> u64 {
        // `Debug`-derived, on the same reasoning as `session::context_fingerprint`: a hand-written
        // walk stops covering a field the moment one is added, and the symptom is a stale page
        // presented as a current one.
        let starts = self.derived.borrow();
        if starts.starts.is_empty() {
            // Nothing derived, so the trait's `0` — a document with no sections is byte-for-byte
            // the document it was before this increment, incremental behaviour included.
            return 0;
        }
        let mut h: u64 = 0xcbf2_9ce4_8422_2325;
        for b in format!("{:?}", starts.starts).as_bytes() {
            h ^= *b as u64;
            h = h.wrapping_mul(0x0000_0100_0000_01b3);
        }
        h
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
    assert!(
        doc.requires.is_empty(),
        "this document requires content packs that have not been resolved: {}. Call \
         `Document::apply_packs` with the result of `Document::resolve_packs` before laying it \
         out — laying it out anyway would set the book in the default face, which is the silent \
         fallback spec 0056 exists to prevent.",
        doc.requires
            .iter()
            .map(|r| format!("`{}` {}", r.name, r.version))
            .collect::<Vec<_>>()
            .join(", ")
    );
    lay_out_resolved(
        &doc.content,
        // Synthesised once per layout rather than once per fixpoint pass, for the component
        // library's reason (spec 0077). Empty for every document without a footnote.
        &doc.footnote_blocks(),
        // The document's forced page breaks (spec 0080). Empty for every document that states
        // none, which is what makes such a document take the path it took before they existed.
        &doc.breaks,
        &doc.assets,
        &doc.styles,
        // The document's own definitions shadow the bundled ones (spec 0054). Built here rather
        // than inside `flow`, so it is built once per layout rather than once per pass of the
        // contents fixpoint.
        &doc.component_library(),
        &DocumentTemplate::new(doc),
        metrics,
        hyphenator,
    )
    .0
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
        /// The paragraph's ink — the colour a line's spans fall back to.
        color: Color,
        /// The resolved ink of each authored run, indexed by [`quill_text_layout::Span::run`]
        /// (spec 0063). One entry per run, already folded with the paragraph's colour, so no
        /// consumer has to know how an override resolves.
        run_colors: Vec<Color>,
        /// Each authored run's resolved face, size and tracking (spec 0064), indexed the same way.
        /// Empty when every run is set in the paragraph's own format.
        run_formats: Vec<RunFormat>,
        /// Each authored run's baseline shift in points, positive up. Empty when none shifts.
        run_shifts: Vec<f32>,
        /// This paragraph's list marker, if it is a list item (spec 0066). Drawn in the gutter at
        /// the first line's baseline, so it never enters the text flow and cannot move a break.
        marker: Option<String>,
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
        /// Decoration inside the panel, positioned relative to its top-left: the hairlines that
        /// separate a stat block's sections, the shaded bands behind a table's alternating rows.
        ///
        /// Generalized from a list of rule offsets when tables arrived (spec 0039). A rule and a
        /// zebra band are the same thing — a filled rectangle at an offset — and one list means the
        /// paint order is decided in one place rather than two.
        decorations: Vec<PanelRect>,
        /// How this panel may be cut across a frame boundary (spec 0045), or `None` for one that
        /// must be placed whole.
        split: Option<PanelSplit>,
    },
}

/// Where a [`Measured::Panel`] may be cut, and what every continuation has to re-state.
///
/// A table's rows and (spec 0046) a stat block's sections are both "a list of pieces that may be
/// separated, with a prefix that has to be repeated". Holding that as data on the measurement keeps
/// spec 0044's rule intact: cutting is a derivation over an already-cached value, so nothing here
/// depends on the available height.
#[derive(Debug, Clone, PartialEq)]
pub(crate) struct PanelSplit {
    /// Height of each item, in order.
    ///
    /// **Item 0 includes `repeat_h`**, because every fragment begins with the repeated prefix. A
    /// fit check that forgets this overfills each continuation by exactly one header row — a
    /// one-line error with a visible symptom and no natural test unless one is written for it.
    pub items: Vec<f32>,
    /// Content re-emitted at the top of every continuation: a table's header row and the rule under
    /// it. Held at panel-local dy in `[0, repeat_h)` and *also* present in `parts`/`decorations`, so
    /// the first fragment picks it up by ordinary slicing and only continuations pay for the copy.
    pub repeat_parts: Vec<PanelPart>,
    pub repeat_decorations: Vec<PanelRect>,
    pub repeat_h: f32,
    /// Space a *fragment* must reserve below its last item — a stat block's bottom padding, so a
    /// cut panel closes with the same inset it opens with. The mirror of `repeat_h`, which is space
    /// a *continuation* reserves above its first. Zero for a table, which has no panel edge.
    pub trailing_pt: f32,
    /// The fewest items this panel's fragments may contain — a table's rows and a stat block's
    /// sections want different answers, and each definition states its own (spec 0054's
    /// `SplitDef::min_items`).
    pub min_items: usize,
    /// Item indices a cut would **rather** fall at. Empty means every item is equally good.
    ///
    /// Spec 0075. A composite's items are its elements — a row, an attribute pair, one action —
    /// but a stat block still wants to be cut *between sections*, which is what spec 0046
    /// promised. Holding the preference separately from the item list is what lets the engine
    /// keep that promise and still have somewhere to cut when no section boundary fits: the
    /// largest legal preferred cut wins, and an element boundary is the fallback rather than the
    /// rule. Before this the two were the same list, so "prefers a section boundary" and "may only
    /// cut at a section boundary" could not be told apart — and a single section taller than a
    /// column had no legal cut at all and ran off the page.
    ///
    /// Indices are into `items`, and a cut of `k` items is preferred when `k` appears here.
    pub preferred: Vec<usize>,
    /// Prefer moving whole to the next frame over cutting, when the block would fit there entire
    /// (spec 0046).
    ///
    /// This is the difference between a composite and a paragraph. Splitting a paragraph is the
    /// *point* — moving it whole is what left the hole spec 0044 fixed. Splitting a stat block that
    /// would have fitted the next column intact is a worse page than moving it, which is what spec
    /// 0038's keep-together promised. Cutting stays the fallback for one that fits nowhere.
    pub keep_together: bool,
}

impl PanelSplit {
    /// Panel-local y at which item `k` ends — the cut offset for a fragment of `k` items.
    ///
    /// Summed the same way every caller sums it. Slicing `parts` by comparing `dy_pt` against a
    /// differently-accumulated total would put a cell exactly on the boundary into both halves or
    /// neither, and both are content-conservation failures.
    fn cut_at(&self, k: usize) -> f32 {
        self.items[..k].iter().sum()
    }
}

/// The fewest *lines* a paragraph fragment or remainder may contain (spec 0044).
///
/// Two: a paragraph may not leave a widow behind at the foot of a column or carry an orphan forward
/// to the top of the next. This is a typographic rule and not a nicety — a lone stranded line is the
/// defect a reader notices first, and a splitter without it would fix the ragged-foot defect by
/// introducing a worse one.
pub(crate) const MIN_LINES_PER_FRAGMENT: usize = 2;

/// The fewest *contents entries* a fragment of a generated contents list may contain (spec 0075).
///
/// **Two, and it is the line rule rather than the section rule.** Spec 0046 had to make the minimum
/// per-variant when it found that demanding two *sections* made the smallest legal cut larger than
/// a column, so nothing cut and the panel ran off the page. The question that decides the number is
/// therefore "what is an item here?", and a contents entry is one line: a `{title}\t{number}` tabbed
/// line that never wraps, by the clipping rule in [`measure_toc`]. A lone entry stranded at the top
/// of a page is the same widow a lone line is, so it gets the same answer as
/// [`MIN_LINES_PER_FRAGMENT`].
///
/// It is safe here in a way it was not for sections, and the check is arithmetic rather than
/// faith: the smallest legal fragment is the list's own title plus two entries — about 60 pt in the
/// bundled styles — against the 378 pt of the narrowest column the `reference` template builds. Two
/// entries could only price a fragment out of a frame that fits fewer than three lines of type.
pub(crate) const MIN_ENTRIES_PER_FRAGMENT: usize = 2;

impl Measured {
    /// Distance from this block's flow position `y` down to its **first baseline** (spec 0058), or
    /// `None` for a block that has no baseline.
    ///
    /// Mirrors `place_measured` exactly, and has to: a text block is drawn below its style's
    /// space-above, and a panel's first run sits at its own `dy_pt` inside the panel. Snapping the
    /// block's *top* instead would put the box on the grid and the type a few points off it, which
    /// is the error that looks almost right.
    pub(crate) fn first_baseline_offset(&self, metrics: &impl RunMetrics) -> Option<f32> {
        match self {
            Measured::Text { style, .. } => {
                Some(style.space_before_pt + metrics.ascent_pt(style.font_size_pt))
            }
            Measured::Panel { parts, .. } => parts
                .first()
                .map(|p| p.dy_pt + metrics.ascent_pt(p.font_size_pt)),
            // An image has no baseline. Its top snaps instead, so the block after it starts from a
            // grid position rather than from wherever the image happened to end.
            Measured::Image { .. } => None,
        }
    }

    /// The heights of the items this measurement may be cut between, in order. `None` means
    /// indivisible.
    ///
    /// **A variant whose measurement depends on the available height must return `None`.** Spec
    /// 0044's whole design rests on splitting being a *derivation over an already-cached
    /// measurement* rather than a second measurement: a paragraph's optimal break at a given width
    /// does not depend on the space left in the column, so cutting its line list needs no cache
    /// entry of its own and `MeasureKey` gains no height dimension. A height-dependent measurement
    /// that offered break opportunities anyway would not merely split badly — it would make the
    /// measurement cache wrong.
    fn break_items(&self) -> Option<Vec<f32>> {
        match self {
            Measured::Text { lines, style, .. } => Some(vec![style.leading_pt; lines.len()]),
            // A table's rows (spec 0045) and a stat block's sections (0046) both arrive as an
            // authored item list on the measurement; a panel without one is indivisible.
            Measured::Panel { split, .. } => split.as_ref().map(|s| s.items.clone()),
            Measured::Image { .. } => None,
        }
    }

    /// How many items this measurement may be cut between, without building the list.
    ///
    /// [`Self::break_items`] clones a `Vec` and the progress assertions in the flow loop only need
    /// its length; a count that allocated would make an invariant check cost more than the thing it
    /// guards.
    fn item_count(&self) -> Option<usize> {
        match self {
            Measured::Text { lines, .. } => Some(lines.len()),
            Measured::Panel { split, .. } => split.as_ref().map(|s| s.items.len()),
            Measured::Image { .. } => None,
        }
    }

    /// The height charged to a fragment before its first item — the space *above* a paragraph,
    /// which belongs to the piece that starts it and to no other.
    fn fragment_lead_pt(&self) -> f32 {
        match self {
            Measured::Text { style, .. } => style.space_before_pt,
            // A panel's repeated prefix is charged inside item 0 rather than as a lead, so that a
            // *continuation* is charged it too. A lead would be counted once, at the top.
            Measured::Image { .. } | Measured::Panel { .. } => 0.0,
        }
    }

    /// The largest legal cut whose fragment fits within `avail_pt`, or `None` when no cut is legal.
    ///
    /// Legal means both sides keep [`MIN_LINES_PER_FRAGMENT`] items (or the panel's own
    /// [`PanelSplit::min_items`]), so a paragraph of three lines or fewer never splits at all.
    ///
    /// **A preferred cut beats a larger one** (spec 0075). A stat block's items are its elements but
    /// its preference is a section boundary, so the largest *preferred* cut that fits wins, and the
    /// largest cut of any kind is taken only when no preferred one fits. That is the difference
    /// between "would rather break between sections" and "may only break between sections" — and the
    /// second is what made a single over-tall section uncuttable, and the panel overflow the page.
    fn cut_fitting(&self, avail_pt: f32) -> Option<usize> {
        let items = self.break_items()?;
        let n = items.len();
        let min = self.min_items_per_fragment();
        if n < 2 * min {
            return None;
        }
        let last_legal = n - min;
        let mut used = self.fragment_lead_pt();
        // Whatever the fragment must reserve *below* its last item is part of what has to fit, or a
        // cut stat block's panel edge is drawn past the bottom of the frame.
        let trail = self.fragment_trail_pt();
        let preferred = self.preferred_cuts();
        let mut best = None;
        let mut best_preferred = None;
        for (i, h) in items.iter().enumerate() {
            used += h;
            if used + trail > avail_pt {
                break;
            }
            let k = i + 1;
            if k >= min && k <= last_legal {
                best = Some(k);
                if preferred.is_none_or(|p| p.contains(&k)) {
                    best_preferred = Some(k);
                }
            }
        }
        best_preferred.or(best)
    }

    /// The cuts this value would rather take, or `None` when it has no preference among its items.
    ///
    /// See [`PanelSplit::preferred`]. A paragraph has no preference — one line break is as good as
    /// another — and neither does a table, whose rows are already the unit it wants.
    fn preferred_cuts(&self) -> Option<&[usize]> {
        match self {
            Measured::Panel {
                split: Some(split), ..
            } if !split.preferred.is_empty() => Some(&split.preferred),
            _ => None,
        }
    }

    /// The fewest items either side of a legal cut, which differs by what an "item" is.
    fn min_items_per_fragment(&self) -> usize {
        match self {
            Measured::Text { .. } => MIN_LINES_PER_FRAGMENT,
            Measured::Panel {
                split: Some(split), ..
            } => split.min_items,
            Measured::Image { .. } | Measured::Panel { split: None, .. } => usize::MAX,
        }
    }

    /// Space a fragment reserves after its last item. See [`PanelSplit::trailing_pt`].
    fn fragment_trail_pt(&self) -> f32 {
        match self {
            Measured::Panel {
                split: Some(split), ..
            } => split.trailing_pt,
            // A paragraph fragment does not end the paragraph, so it is charged no space below.
            Measured::Text { .. }
            | Measured::Image { .. }
            | Measured::Panel { split: None, .. } => 0.0,
        }
    }

    /// Whether this block would rather move whole to the next frame than be cut, given that it
    /// would fit there entire. See [`PanelSplit::keep_together`].
    fn prefers_keep_together(&self) -> bool {
        matches!(self, Measured::Panel { split: Some(s), .. } if s.keep_together)
    }

    /// Cut into a fragment of the first `at` items and a remainder of the rest, both fully measured
    /// at the same width, with their heights. `None` when `at` is not a cut this value admits.
    ///
    /// The two heights are deliberately **not** a partition of the whole: vertical space belongs to
    /// the ends of a paragraph and not to its middle. The fragment starts the paragraph so it is
    /// charged the space above; the remainder ends it so it is charged the space below. Returning
    /// both rather than letting a caller subtract is what stops the natural, wrong implementation.
    fn split_at(&self, at: usize) -> Option<(Measured, f32, Measured, f32)> {
        match self {
            Measured::Text {
                lines,
                color,
                run_colors,
                run_formats,
                run_shifts,
                marker,
                style,
            } => {
                if at == 0 || at >= lines.len() {
                    return None;
                }
                let head_h = style.space_before_pt + at as f32 * style.leading_pt;
                let head = Measured::Text {
                    lines: lines[..at].to_vec(),
                    color: *color,
                    // The marker belongs to the item, and the head is where the item starts.
                    marker: marker.clone(),
                    // Both halves keep the whole run table: a span's `run` indexes the paragraph's
                    // runs, and re-basing it per fragment would make a continuation's spans mean
                    // something different from its head's.
                    run_colors: run_colors.clone(),
                    run_formats: run_formats.clone(),
                    run_shifts: run_shifts.clone(),
                    style: *style,
                };
                // The continuation carries no space-above: it does not start the paragraph. The
                // style is edited rather than the height alone, because placement reads
                // `space_before_pt` to inset the text within the block's box and the two must agree.
                let tail_lines = lines[at..].to_vec();
                let tail_h = tail_lines.len() as f32 * style.leading_pt + style.space_after_pt;
                let tail = Measured::Text {
                    run_colors: run_colors.clone(),
                    run_formats: run_formats.clone(),
                    run_shifts: run_shifts.clone(),
                    // A continuation is not a new item, so it is not marked again — the same rule
                    // spec 0045 applies to a table's header, from the other direction.
                    marker: None,
                    lines: tail_lines,
                    color: *color,
                    style: ParagraphStyle {
                        list: None,
                        space_before_pt: 0.0,
                        ..*style
                    },
                };
                Some((head, head_h, tail, tail_h))
            }
            Measured::Panel {
                fill,
                stroke,
                parts,
                decorations,
                split: Some(split),
            } => {
                if at == 0 || at >= split.items.len() {
                    return None;
                }
                let cut = split.cut_at(at);
                let total: f32 = split.items.iter().sum();

                // The fragment is everything above the cut, unshifted — including the prefix, which
                // is in `parts`/`decorations` already.
                let head = Measured::Panel {
                    fill: *fill,
                    stroke: *stroke,
                    parts: parts.iter().filter(|p| p.dy_pt < cut).cloned().collect(),
                    decorations: decorations
                        .iter()
                        .filter(|d| d.dy_pt < cut)
                        .cloned()
                        .collect(),
                    split: None,
                };

                // The continuation re-states the prefix and then carries everything below the cut,
                // moved up so the first surviving item sits directly under it. The shift preserves
                // each decoration's identity, which is why a zebra band keeps the stripe it had:
                // the bands were computed from each row's index in the *whole* table, and moving
                // them cannot change that.
                let shift = cut - split.repeat_h;
                let mut tail_parts = split.repeat_parts.clone();
                tail_parts.extend(parts.iter().filter(|p| p.dy_pt >= cut).map(|p| PanelPart {
                    dy_pt: p.dy_pt - shift,
                    ..p.clone()
                }));
                let mut tail_decorations = split.repeat_decorations.clone();
                tail_decorations.extend(decorations.iter().filter(|d| d.dy_pt >= cut).map(|d| {
                    PanelRect {
                        dy_pt: d.dy_pt - shift,
                        ..d.clone()
                    }
                }));

                // The continuation can be cut again, so it carries its own item list — with the
                // prefix folded into its first item exactly as the original's was.
                let mut tail_items: Vec<f32> = split.items[at..].to_vec();
                tail_items[0] += split.repeat_h;
                // The remainder keeps the same preferences, re-based onto its own item list. A
                // boundary at or before the cut is gone with the items it separated; index 0 of the
                // remainder is where the continuation starts, and a cut of nothing is not a cut, so
                // only strictly-later boundaries survive.
                let tail_preferred: Vec<usize> = split
                    .preferred
                    .iter()
                    .filter(|i| **i > at)
                    .map(|i| i - at)
                    .collect();
                let tail = Measured::Panel {
                    fill: *fill,
                    stroke: *stroke,
                    parts: tail_parts,
                    decorations: tail_decorations,
                    split: Some(PanelSplit {
                        items: tail_items,
                        repeat_parts: split.repeat_parts.clone(),
                        repeat_decorations: split.repeat_decorations.clone(),
                        repeat_h: split.repeat_h,
                        trailing_pt: split.trailing_pt,
                        min_items: split.min_items,
                        preferred: tail_preferred,
                        keep_together: split.keep_together,
                    }),
                };
                Some((head, cut + split.trailing_pt, tail, total - shift))
            }
            Measured::Image { .. } | Measured::Panel { split: None, .. } => None,
        }
    }
}

/// One decorative rectangle inside a [`Measured::Panel`], relative to the panel's top-left.
#[derive(Debug, Clone, PartialEq)]
pub(crate) struct PanelRect {
    pub dx_pt: f32,
    pub dy_pt: f32,
    pub w_pt: f32,
    pub h_pt: f32,
    pub fill: Option<Color>,
    pub stroke: Option<Stroke>,
}

/// One styled run inside a [`Measured::Panel`], positioned relative to the panel's top-left.
///
/// A part carries **both** the measure it was broken to and the ink that came out of it, and the
/// distinction is the point (spec 0069). `dx_pt`/`w_pt` are the slot — a table column, a panel's
/// inner width — and stay on the *measurement*, where the producer knows them and where the
/// containment invariants are asserted. `ink_dx_pt`/`ink_w_pt` are what reaches placed output,
/// because [`PlacedBlock`]'s frame is the ink.
#[derive(Debug, Clone, PartialEq)]
pub(crate) struct PanelPart {
    /// Left edge of the **measure** this run was broken to, relative to the panel's top-left.
    pub dx_pt: f32,
    pub dy_pt: f32,
    /// Width of that measure. Never placed; see `ink_w_pt`.
    pub w_pt: f32,
    /// The run's ink box, relative to `dx_pt`. `(0.0, w_pt)` for a part whose ink fills its measure
    /// by construction, which is what a tab segment's own extent is.
    pub ink_dx_pt: f32,
    pub ink_w_pt: f32,
    pub lines: Vec<Line>,
    pub color: Color,
    pub font_size_pt: f32,
    pub leading_pt: f32,
    /// Zero-based page this run links to, for the runs that are navigation rather than prose — a
    /// contents entry's title (spec 0052). `None` for every other run.
    ///
    /// Carried on the part rather than re-derived by the writer because the part is what gets
    /// sliced when a panel is cut across a frame boundary (spec 0045): a link travels with the run
    /// it belongs to, into whichever fragment that run ends up in, for free.
    pub link_page: Option<usize>,
}

/// Break/size `block` against a frame of `width` points, returning the placement payload and its
/// height. `None` means "skip this block" — currently only an unresolved [`Block::Image`] id.
///
/// Called once per candidate frame in [`lay_out_in_thread`]'s placement loop so a block that
/// advances into a different-width frame re-wraps (text) / re-fits (image) to that frame's width.
/// The ambient inputs every block measurement needs, other than the block and its width.
///
/// Grouped because they travel together through `flow`, the `Measurer` trait and `measure_block`,
/// and the list grew by one with each of specs 0026, 0028 and 0041. Passing them as a unit means
/// the next addition changes one struct rather than five signatures.
#[derive(Clone, Copy)]
pub(crate) struct BlockContext<'a> {
    pub assets: &'a AssetIndex<'a>,
    pub styles: &'a StyleSheet,
    /// Where the headings landed, for a contents block to render from (spec 0041). Empty on the
    /// first pass of the fixpoint, and for every document that has no contents block.
    pub headings: &'a [HeadingEntry],
    /// The collated index, for an index block to render from (spec 0078). Empty on the first pass
    /// of the fixpoint, and for every document that marks nothing.
    pub index: &'a [IndexEntry],
    /// The component definitions this document can resolve (spec 0054): the bundled two, plus
    /// whatever it or its packs supply.
    pub components: &'a ComponentLibrary,
    /// Each list item's marker, keyed by block (spec 0066).
    ///
    /// Derived in one pass over the document's blocks *before* anything is placed, never
    /// accumulated while placing. Spec 0041 learned this the hard way: an incremental pass reuses
    /// whole pages, so a counter advanced during placement goes missing on a reused page — and goes
    /// missing exactly when the document was just edited, which is always.
    pub markers: &'a BTreeMap<BlockId, String>,
    /// What every run whose characters are generated at layout time currently draws, keyed by the
    /// **thing the run points at** (specs 0076, 0077).
    ///
    /// Two quantities live in one map, and they are disjoint by construction because a document has
    /// one id space: a cross-referenced block's folio, keyed by the target block; and a footnote's
    /// number, keyed by the note. Merging them is what lets one resolver, one per-block fingerprint
    /// and one fixpoint comparison serve both instead of two of each — and the two are not
    /// independent, since a footnote band moves the page a cross-reference target lands on.
    ///
    /// Keyed by referent rather than by referring block so the derivation is one walk regardless of
    /// how many runs point at the same chapter — a book cites its important pages many times — and
    /// so that two references to one block cannot disagree.
    ///
    /// A referent absent from this map is **unresolved** and draws
    /// [`quill_core_model::UNRESOLVED_REFERENCE`]. That is a state, not an error: it is what the
    /// first pass of the cold fixpoint sees for a cross-reference, and it is what an anchor naming a
    /// note the document does not hold sees for ever.
    pub resolved: &'a BTreeMap<BlockId, String>,
}

/// Every list item's marker, derived from the blocks in document order (spec 0066).
///
/// A list is a run of consecutive paragraphs whose resolved style carries a [`ListSpec`], so this
/// walks the content once and keeps a counter per nesting level. A level's counter restarts when a
/// shallower item or a non-list paragraph interrupts it, which is what makes two lists separated by
/// a paragraph two lists rather than one long one.
pub(crate) fn list_markers(content: &[Block], styles: &StyleSheet) -> BTreeMap<BlockId, String> {
    let mut out = BTreeMap::new();
    // Counter per level; `None` at a level means "not currently inside a list at that depth".
    let mut counters: Vec<Option<u32>> = Vec::new();
    for block in content {
        if !matches!(block, Block::Body { .. } | Block::Heading { .. }) {
            counters.clear();
            continue;
        }
        let Some(spec) = styles.resolve(block).list else {
            counters.clear();
            continue;
        };
        let level = spec.level as usize;
        if counters.len() <= level {
            counters.resize(level + 1, None);
        }
        // A shallower item closes every deeper level: `1, a, b, 2` restarts the inner list.
        for deeper in counters.iter_mut().skip(level + 1) {
            *deeper = None;
        }
        let n = match counters[level] {
            Some(prev) => prev + 1,
            None => spec.start,
        };
        counters[level] = Some(n);
        let text = match spec.marker {
            ListMarker::Bullet { glyph } => glyph.to_string(),
            ListMarker::Number { format, suffix } => {
                let mut t = format.format(n);
                if let Some(c) = suffix {
                    t.push(c);
                }
                t
            }
        };
        out.insert(block.id(), text);
    }
    out
}

/// What each cross-reference target's folio says, derived from where it actually landed (spec 0076).
///
/// Keyed by the **target** block, one entry per distinct block any run refers to. A target that is
/// nowhere in the page vector — deleted, or an id that names nothing — is simply absent, which is
/// what makes it draw [`quill_core_model::UNRESOLVED_REFERENCE`].
///
/// Derived from the **laid-out pages** rather than accumulated while placing, for exactly the reason
/// [`heading_index_of`] and [`section_starts`] are: an incremental pass reuses whole pages, so
/// anything counted during placement goes missing on a reused page — and goes missing precisely when
/// the document has just been edited. A block placed on more than one page (spec 0044 can split one)
/// reports its **first**: a reference means "where does this start".
///
/// The number is the page's **folio**, through [`PageTemplate::folio`] — the same function the
/// `{page}` token and a contents entry go through, so a cross-reference, a running folio and a
/// contents list cannot print three different numbers for one page.
///
/// Returns empty when `targets` is empty, and every caller skips the walk entirely in that case, so
/// a document with no cross-reference pays nothing.
pub fn reference_folios(
    targets: &BTreeSet<BlockId>,
    pages: &[LaidOutPage],
    template: &impl PageTemplate,
) -> BTreeMap<BlockId, String> {
    let mut out: BTreeMap<BlockId, String> = BTreeMap::new();
    if targets.is_empty() {
        return out;
    }
    for page in pages {
        for placed in &page.blocks {
            // Every variant that carries an identity, for `section_starts`' reason: a reference may
            // name an image, a table or a stat block as readily as a heading, and a composite's
            // parts all carry the composite's id. `Rect` is furniture within a block and has none.
            let source = match placed {
                PlacedBlock::Text { source, .. }
                | PlacedBlock::Image { source, .. }
                | PlacedBlock::Link { source, .. } => *source,
                PlacedBlock::Rect { .. } => BlockId::UNASSIGNED,
            };
            if source.is_assigned() && targets.contains(&source) {
                out.entry(source)
                    .or_insert_with(|| template.folio(page.index));
            }
        }
    }
    out
}

/// Each run's drawn text: its own characters, or what its [`RunSource`] resolves to (specs 0076,
/// 0077).
///
/// **The resolver half of the contract [`RunSource`] describes**, and the `match` below is
/// exhaustive, which is half of what makes adding a run source without teaching the font-subset
/// collector fail to compile — the other half is `RunSource::contributes`.
///
/// Returns `None` when every run is authored, which is every paragraph in every document that has
/// neither a cross-reference nor a footnote anchor. That is not a tidiness: the caller then borrows
/// the runs' own `&str`s exactly as it did before this existed, so the hot measurement path
/// allocates nothing new and a document without the feature costs precisely what it cost.
fn resolve_run_texts(
    runs: &[quill_core_model::Run],
    resolved: &BTreeMap<BlockId, String>,
) -> Option<Vec<String>> {
    if runs.iter().all(|r| r.source.is_authored()) {
        return None;
    }
    Some(
        runs.iter()
            .map(|r| match r.source {
                RunSource::Authored => r.text.clone(),
                // Both generated variants read the one map, keyed by whatever the run points at.
                // Written as two arms rather than through `RunSource::referent` so that the `match`
                // is still what fails to compile when a third variant arrives.
                RunSource::Reference { target } => resolved
                    .get(&target)
                    .cloned()
                    .unwrap_or_else(|| UNRESOLVED_REFERENCE.to_string()),
                RunSource::Footnote { note } => resolved
                    .get(&note)
                    .cloned()
                    .unwrap_or_else(|| UNRESOLVED_REFERENCE.to_string()),
            })
            .collect(),
    )
}

/// A run's treatment, resolved through all three layers: the paragraph, the character style it
/// names, and its own overrides (spec 0065).
///
/// One function, because the three consumers — ink, format and baseline shift — must not be able to
/// disagree about precedence. Field by field, so a run naming `strong` and overriding only its
/// colour stays bold.
fn resolved_style(run: &quill_core_model::Run, styles: &StyleSheet) -> CharacterStyle {
    // A name that resolves to nothing is not an error: the run lays out with the paragraph's
    // treatment and its own overrides still apply. Losing text because a style was renamed — or
    // because the pack that defined it is missing — would be far worse than setting it plainly,
    // which is the posture specs 0028 and 0054 both take.
    let named = run
        .character
        .as_deref()
        .and_then(|name| styles.character(name))
        .unwrap_or(CharacterStyle::EMPTY);
    named.with(run.style)
}

/// Each run's resolved ink: its named style, its own override, or the paragraph's (specs 0063,
/// 0065).
///
/// Resolved once, here, so that nothing downstream has to know how an override resolves — the
/// writer and the screen painter must not be able to disagree about a run's colour, which is the
/// same rule `Line::indent_pt` follows.
fn run_inks(runs: &[quill_core_model::Run], paragraph: Color, styles: &StyleSheet) -> Vec<Color> {
    runs.iter()
        .map(|r| resolved_style(r, styles).color.unwrap_or(paragraph))
        .collect()
}

/// Each run's resolved format: its named style and its own overrides folded onto the paragraph's
/// treatment (specs 0064, 0065).
///
/// Empty when every run resolves to the breaker's own default — regular, upright, untracked, at the
/// paragraph's size — which is every document that names no style and no override anywhere. That
/// emptiness is load-bearing: the breaker, the writer and the painter all take their pre-family path
/// when there is one format, so those documents lay out and export byte for byte as they did. It is
/// emptiness against *the default*, not against the paragraph: a bold paragraph style is a format
/// the breaker has to be told about.
fn run_formats(
    runs: &[quill_core_model::Run],
    style: &ParagraphStyle,
    styles: &StyleSheet,
) -> Vec<RunFormat> {
    let paragraph = RunFormat {
        size_pt: style.font_size_pt,
        weight: style.weight.0,
        italic: style.italic,
        tracking_pt: 0.0,
    };
    let formats: Vec<RunFormat> = runs
        .iter()
        .map(|r| {
            let s = resolved_style(r, styles);
            RunFormat {
                size_pt: s.size_pt.unwrap_or(paragraph.size_pt),
                weight: s.weight.map_or(paragraph.weight, |w| w.0),
                italic: s.italic.unwrap_or(paragraph.italic),
                tracking_pt: s.tracking_pt.unwrap_or(0.0),
            }
        })
        .collect();
    if paragraph == RunFormat::plain(style.font_size_pt) && formats.iter().all(|f| *f == paragraph)
    {
        return Vec::new();
    }
    formats
}

/// Each run's baseline shift, in points, positive up (specs 0064, 0065).
///
/// Not part of [`RunFormat`]: it moves a glyph without changing an advance, so it is a drawing
/// property and must not invalidate a line break. Empty when no run shifts.
fn run_shifts(runs: &[quill_core_model::Run], styles: &StyleSheet) -> Vec<f32> {
    let shifts: Vec<f32> = runs
        .iter()
        .map(|r| resolved_style(r, styles).baseline_shift_pt.unwrap_or(0.0))
        .collect();
    if shifts.iter().all(|s| *s == 0.0) {
        return Vec::new();
    }
    shifts
}

pub(crate) fn measure_block(
    block: &Block,
    width: f32,
    ctx: &BlockContext<'_>,
    metrics: &impl RunMetrics,
    hyphenator: &impl Hyphenator,
) -> Option<(Measured, f32)> {
    let BlockContext {
        assets,
        styles,
        headings,
        index,
        components,
        markers,
        resolved,
    } = *ctx;
    match block {
        Block::Heading { runs, color, .. } | Block::Body { runs, color, .. } => {
            // Size, leading and alignment now come from the document's stylesheet rather than from
            // crate constants (spec 0028). Before this, every block in every document was set at
            // body size — a heading differed from body text only by being ragged-left.
            let style = styles.resolve(block);
            let align = match style.align {
                TextAlign::Justified => Alignment::Justified,
                TextAlign::Left => Alignment::Left,
            };
            // The runs are one paragraph to the breaker (spec 0063): a change of treatment must
            // not be able to move a break, or the run model would be a layout change rather than a
            // generalization. What comes back is per-line spans naming the run each stretch is from.
            //
            // A cross-reference run draws its resolved folio rather than its own text (spec 0076),
            // and it is substituted **here**, before the item stream is built: its width moves line
            // breaks — "see page 142" is three digits where "see page 42" is two — so it has to be
            // in the paragraph the breaker sees rather than painted over it afterwards. `None` is
            // every paragraph of every document without the feature, and takes the borrowing path
            // this line always took.
            let texts_owned = resolve_run_texts(runs, resolved);
            let texts: Vec<&str> = match &texts_owned {
                Some(v) => v.iter().map(String::as_str).collect(),
                None => runs.iter().map(|r| r.text.as_str()).collect(),
            };
            // A paragraph whose style names tab stops, and whose text actually contains a tab, is
            // positioned by those stops rather than broken to the measure (spec 0067). Both
            // conditions, so a document that names stops it does not use lays out exactly as it did.
            let stops = styles.tabs_for(block);
            if !stops.is_empty() && texts.iter().any(|t| t.contains('\t')) {
                return Some(measure_tabbed(
                    &texts, stops, &style, *color, width, metrics,
                ));
            }
            // Each run's own face, size and tracking (spec 0064). Empty when they all resolve to
            // the paragraph's, which is the path that must not move a glyph.
            let formats = run_formats(runs, &style, styles);
            let lines = justify_runs_indented(
                &texts,
                &formats,
                width,
                quill_text_layout::Indent {
                    first_pt: style.indent.first_pt,
                    rest_pt: style.indent.rest_pt,
                },
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
                    run_colors: run_inks(runs, *color, styles),
                    run_formats: formats,
                    run_shifts: run_shifts(runs, styles),
                    marker: markers.get(&block.id()).cloned(),
                    style,
                },
                height,
            ))
        }
        Block::Toc {
            title,
            max_level,
            color,
            ..
        } => Some(measure_toc(
            title, *max_level, *color, width, styles, headings, metrics,
        )),
        Block::Index { title, color, .. } => {
            Some(measure_index(title, *color, width, styles, index, metrics))
        }
        // The two bundled components are *sugar*: they keep the authored shape a person would
        // rather write by hand, and convert to definition + fields here (spec 0054), so nothing
        // measures a stat block or a table twice.
        Block::Table { table, color, .. } => Some(measure_component(
            components.get(TABLE_COMPONENT)?,
            &table.to_fields(),
            *color,
            width,
            styles,
            metrics,
            hyphenator,
        )),
        Block::Panel { panel, color, .. } => Some(measure_component(
            components.get(STATBLOCK_COMPONENT)?,
            &panel.to_fields(),
            *color,
            width,
            styles,
            metrics,
            hyphenator,
        )),
        Block::Component {
            def, fields, color, ..
        } => Some(measure_component(
            // A definition that does not resolve is skipped, exactly as an unresolved image asset
            // is. Layout has no error channel by design; a *malformed* definition is refused at
            // load, where there is one.
            components.get(def.as_str())?,
            fields,
            *color,
            width,
            styles,
            metrics,
            hyphenator,
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

/// Width reserved at the right edge of a contents line for its page number.
const TOC_NUMBER_COLUMN_PT: f32 = 26.0;

/// Indent applied per heading level below the first.
const TOC_INDENT_PT: f32 = 12.0;

/// Gap between the end of an entry's leader and its page number.
const TOC_LEADER_GAP_PT: f32 = 4.0;

/// A paragraph positioned by its tab stops (spec 0067).
///
/// One line, and deliberately: a tabbed paragraph is a *row* — a contents entry, a price line, a
/// bibliography entry, a specification sheet's key and value. Wrapping one is a different mechanism
/// (which stop does a continuation line start at?) and is named as a non-goal rather than guessed
/// at. Text that outruns its stop goes to the next stop, and a segment with no stop left butts
/// against what precedes it, so nothing is ever dropped.
///
/// Built as panel parts, which is what the generated contents list has been since spec 0041 — the
/// same shape, now produced by a general mechanism instead of by hand.
fn measure_tabbed(
    texts: &[&str],
    stops: &[quill_core_model::TabStop],
    style: &ParagraphStyle,
    color: Color,
    width: f32,
    metrics: &impl RunMetrics,
) -> (Measured, f32) {
    let joined: String = texts.concat();
    let stops: Vec<quill_text_layout::TabStop> = stops
        .iter()
        .map(|t| quill_text_layout::TabStop {
            // A stop past the measure would place text off the frame; clamped, because a stop is a
            // house style's number and a measure is the page's.
            position_pt: t.position_pt.min(width),
            align: match t.align {
                quill_core_model::TabAlign::Left => quill_text_layout::TabAlign::Left,
                quill_core_model::TabAlign::Centre => quill_text_layout::TabAlign::Centre,
                quill_core_model::TabAlign::Right => quill_text_layout::TabAlign::Right,
                quill_core_model::TabAlign::Decimal => quill_text_layout::TabAlign::Decimal,
            },
            leader: t.leader.map(|l| quill_text_layout::Leader {
                glyph: l.glyph,
                gap_pt: l.gap_pt,
            }),
        })
        .collect();

    let parts = tab_parts(
        &joined,
        &stops,
        0.0,
        style.space_before_pt,
        style.font_size_pt,
        style.leading_pt,
        color,
        metrics,
    );

    let height = style.space_before_pt + style.leading_pt + style.space_after_pt;
    (
        Measured::Panel {
            fill: None,
            stroke: None,
            parts,
            decorations: Vec::new(),
            // One line: there is nothing to split between.
            split: None,
        },
        height,
    )
}

/// One tabbed line, laid against `stops` and returned as panel parts (spec 0067).
///
/// The one place a tabbed line becomes placed geometry. Both callers go through it: an authored
/// paragraph whose style names stops ([`measure_tabbed`]), and the generated contents list, whose
/// entries are a tabbed line that spec 0041 used to write out longhand (spec 0070).
///
/// `origin_dx_pt` shifts the whole line right — a contents entry's per-level indent — and is added
/// to each segment's resolved `x_pt` rather than being folded into the stops, so a stop stays the
/// column it names.
#[allow(clippy::too_many_arguments)]
fn tab_parts(
    text: &str,
    stops: &[quill_text_layout::TabStop],
    origin_dx_pt: f32,
    dy_pt: f32,
    font_size_pt: f32,
    leading_pt: f32,
    color: Color,
    metrics: &impl RunMetrics,
) -> Vec<PanelPart> {
    quill_text_layout::lay_tabs(text, stops, &|s| metrics.measure_run(s, font_size_pt))
        .into_iter()
        .map(|seg| PanelPart {
            dx_pt: origin_dx_pt + seg.x_pt,
            dy_pt,
            // `lay_tabs` already resolved the stop into the segment's own extent for all four
            // alignments, so a segment's measure and its ink are the same rectangle — the
            // degenerate case of spec 0069's rule, one line at zero indent.
            w_pt: seg.w_pt,
            ink_dx_pt: 0.0,
            ink_w_pt: seg.w_pt,
            // Each segment is drawn where the stops put it, so it carries no justification: a tab is
            // a hard position, and stretching the gap in front of one would move the stop.
            lines: vec![Line::single_run(seg.text, 0.0, 0.0)],
            color,
            font_size_pt,
            leading_pt,
            link_page: None,
        })
        .collect()
}

/// Build a table of contents from where the headings actually landed (spec 0041).
///
/// The entries are *derived*, never stored: a stored entry is stale the moment anything is edited,
/// and a contents list whose numbers were right one edit ago is worse than none.
///
/// **Each entry is a tabbed line** (spec 0070): `"{clipped title}\t{number}"` against one right stop
/// at the measure's right edge, with a `.` leader. Spec 0041 computed that by hand — an indent, a
/// clip column, a leader origin, a dot count and a right-aligned number — and spec 0067 then shipped
/// the general mechanism beside it. This is the same arithmetic, so every x position and the dot
/// count are unchanged; what moves is the three *widths*, which now report ink rather than columns
/// (spec 0069).
///
/// The **clipping** stays here and did not generalize. Truncating with an ellipsis is not a tab
/// rule; it is the contents list's own "an entry does not wrap", and a price list or a bibliography
/// wants no part of it.
fn measure_toc(
    title: &str,
    max_level: u8,
    color: Color,
    width: f32,
    styles: &StyleSheet,
    headings: &[HeadingEntry],
    metrics: &impl RunMetrics,
) -> (Measured, f32) {
    let mut parts: Vec<PanelPart> = Vec::new();
    // Panel-local y of each place this list may be cut: the top of every entry. Item 0's boundary is
    // rewritten to 0.0 once the walk is done, so the list's own title is absorbed into the first
    // item and can never be left behind as a fragment of its own — the same trick
    // `measure_component` uses for a panel's top inset.
    let mut boundaries: Vec<f32> = Vec::new();
    let mut y = 0.0;

    if !title.is_empty() {
        let style = styles
            .paragraph
            .get(TOC_TITLE_STYLE)
            .copied()
            .unwrap_or_default();
        y += style.space_before_pt;
        // The list's own heading is one line, set flush left: its ink is its measured advance, not
        // the whole frame measure it was given (spec 0069).
        let title_w = metrics.measure_run(title, style.font_size_pt);
        parts.push(PanelPart {
            dx_pt: 0.0,
            dy_pt: y,
            w_pt: width,
            ink_dx_pt: 0.0,
            ink_w_pt: title_w,
            lines: vec![Line::single_run(title.to_string(), 0.0, 0.0)],
            color,
            font_size_pt: style.font_size_pt,
            leading_pt: style.leading_pt,
            // The contents list's own heading points nowhere; only its entries do.
            link_page: None,
        });
        y += style.leading_pt + style.space_after_pt;
    }

    for h in headings.iter().filter(|h| h.level <= max_level) {
        let style = styles
            .paragraph
            .get(&toc_entry_style_name(h.level))
            .copied()
            .unwrap_or_default();
        // Recorded before the entry's space-above, so a fragment that begins here begins with that
        // space rather than swallowing it — the rule `measure_component` states for a section.
        boundaries.push(y);
        y += style.space_before_pt;

        let indent = (h.level.saturating_sub(1)) as f32 * TOC_INDENT_PT;
        // The **folio**, not the page index (spec 0073): what the page the heading landed on
        // actually prints. `link_page` below still carries `h.page_index`, because a destination is
        // a physical page reference and a printed number is not.
        let number = h.folio.clone();

        // Title, clipped to the space before the number column. A long chapter name is truncated
        // with an ellipsis rather than wrapped: a contents list is scanned, and a two-line entry
        // whose page number sits beside the first line reads as two entries.
        let title_max = (width - indent - TOC_NUMBER_COLUMN_PT - TOC_LEADER_GAP_PT).max(1.0);
        let mut text = h.text.clone();
        if metrics.measure_run(&text, style.font_size_pt) > title_max {
            while !text.is_empty()
                && metrics.measure_run(&format!("{text}…"), style.font_size_pt) > title_max
            {
                text.pop();
            }
            text.push('…');
        }

        // One right stop at the measure's right edge, with a dot leader — which is what a contents
        // entry *is*, and what spec 0041 wrote out by hand. The stop is stated relative to the
        // entry's own origin, so the whole line moves with the level indent.
        //
        // Right-aligned, so the number's right edge sits at the measure's right edge and a 3-digit
        // page ends in the same column as a 1-digit one. The leader fills the gap between the two
        // and is inset by `TOC_LEADER_GAP_PT` at each end, so it touches neither.
        let stops = [quill_text_layout::TabStop {
            position_pt: width - indent,
            align: quill_text_layout::TabAlign::Right,
            leader: Some(quill_text_layout::Leader {
                glyph: '.',
                gap_pt: TOC_LEADER_GAP_PT,
            }),
        }];
        let mut entry = tab_parts(
            &format!("{text}\t{number}"),
            &stops,
            indent,
            y,
            style.font_size_pt,
            style.leading_pt,
            color,
            metrics,
        );
        // The entry's destination (spec 0052). Recorded on the *title* run — `lay_tabs` emits the
        // line's first stretch first, and a leader can only follow a tab — which is the entry's own
        // placed text, and taken from the same `h.page_index` the printed number is derived from,
        // so a link and the number beside it can never disagree about where the heading is.
        //
        // Under spec 0069 that run's rectangle is now its measured advance rather than the clip
        // column, so the hot area ends where the title does instead of running across the leader.
        if let Some(title) = entry.first_mut() {
            title.link_page = Some(h.page_index);
        }
        parts.append(&mut entry);

        y += style.leading_pt + style.space_after_pt;
    }

    // A contents list is cut **between entries** (spec 0075), which is the natural atom for the same
    // reason a table's is a row and a stat block's a section: an entry is one line of navigation,
    // and half of one is not navigation at all.
    //
    // Spec 0045 made this `None` — "a contents block is deliberately indivisible" — and that was
    // wrong in a way nothing measured, because every contents fixture in the workspace is a handful
    // of chapters. A real 500-page book's contents list is three pages, and an indivisible panel
    // taller than its frame reaches the branch that places a block whole and lets it run off the
    // page. The fixpoint objection the old comment raised is answered by the fixpoint itself: each
    // iteration re-measures the *whole* block from the heading index and re-cuts it, so a fragment
    // is never re-derived from a fragment.
    //
    // Nothing here depends on the available height — the entries, their clipping and their leaders
    // are functions of the measure alone — which is spec 0044's precondition for offering break
    // opportunities at all, and why `MeasureKey` is untouched.
    let split = (boundaries.len() >= 2).then(|| {
        // Item 0 absorbs the list's own title, exactly as a panel's item 0 absorbs its top inset.
        let mut items: Vec<f32> = Vec::with_capacity(boundaries.len());
        for i in 0..boundaries.len() {
            let start = if i == 0 { 0.0 } else { boundaries[i] };
            let end = boundaries.get(i + 1).copied().unwrap_or(y);
            items.push(end - start);
        }
        PanelSplit {
            items,
            // Nothing is re-stated at the top of a continuation. A repeated "Contents" heading would
            // read as a second contents list rather than as the same one carrying on, which is the
            // opposite of a table header — there the header is what makes the rows legible.
            repeat_parts: Vec::new(),
            repeat_decorations: Vec::new(),
            repeat_h: 0.0,
            // No panel edge to close: a contents list draws neither fill nor stroke.
            trailing_pt: 0.0,
            min_items: MIN_ENTRIES_PER_FRAGMENT,
            // Every entry is as good a place to break as any other.
            preferred: Vec::new(),
            // A contents list short enough to fit the next frame whole belongs there whole, which is
            // also exactly the behaviour every existing document gets today.
            keep_together: true,
        }
    });

    (
        Measured::Panel {
            fill: None,
            stroke: None,
            parts,
            decorations: Vec::new(),
            split,
        },
        y,
    )
}

/// Gap between an index entry's term and its page numbers (spec 0078).
const INDEX_GAP_PT: f32 = 6.0;

/// Build a back-of-book index from where the marked runs actually landed (spec 0078).
///
/// [`measure_toc`]'s shape, deliberately and almost line for line: the entries are *derived* and
/// never stored, each entry is one tabbed line against one right stop, a term too long for its
/// column is clipped with an ellipsis rather than wrapped, and the block cuts between entries.
/// Reusing the shape is the point — an index is a contents list with different entries, and the two
/// producing different geometry would be two answers to one question.
///
/// **Three things differ, and each is a decision.**
///
/// - **No dot leader.** A contents list has a dozen long entries and the leader is what carries the
///   eye across the gap; an index has hundreds of short ones, and a page of dot leaders reads as a
///   contents list rather than as an index. The right stop stays, because keeping the numbers in one
///   scannable column is what a right stop is *for*.
/// - **The number column is measured, not reserved.** A contents entry ends in a page number and
///   `TOC_NUMBER_COLUMN_PT` is wide enough for any of them; an index entry ends in
///   `iv, 42–45, 47`, whose width is a property of the entry. So the term is clipped against what
///   this entry's own folios measure.
/// - **No `link_page`.** A contents entry points at one page; an index entry points at several, and
///   a hot area over the whole run of numbers would navigate to whichever one the implementation
///   picked. Spec 0052's plumbing is per-rectangle, so the honest version is one link per number,
///   which is a placement change and a named non-goal in spec 0078 rather than a guess made here.
fn measure_index(
    title: &str,
    color: Color,
    width: f32,
    styles: &StyleSheet,
    entries: &[IndexEntry],
    metrics: &impl RunMetrics,
) -> (Measured, f32) {
    let mut parts: Vec<PanelPart> = Vec::new();
    // Panel-local y of every place this block may be cut: the top of every entry. Item 0's boundary
    // is rewritten to 0.0 below so the index's own title is absorbed into the first item and can
    // never be left behind alone — `measure_toc`'s trick, and a panel's top inset before that.
    let mut boundaries: Vec<f32> = Vec::new();
    let mut y = 0.0;

    if !title.is_empty() {
        let style = styles
            .paragraph
            .get(INDEX_TITLE_STYLE)
            .copied()
            .unwrap_or_default();
        y += style.space_before_pt;
        let title_w = metrics.measure_run(title, style.font_size_pt);
        parts.push(PanelPart {
            dx_pt: 0.0,
            dy_pt: y,
            w_pt: width,
            ink_dx_pt: 0.0,
            // Its measured advance, not the frame measure it was given (spec 0069).
            ink_w_pt: title_w,
            lines: vec![Line::single_run(title.to_string(), 0.0, 0.0)],
            color,
            font_size_pt: style.font_size_pt,
            leading_pt: style.leading_pt,
            link_page: None,
        });
        y += style.leading_pt + style.space_after_pt;
    }

    let style = styles
        .paragraph
        .get(INDEX_ENTRY_STYLE)
        .copied()
        .unwrap_or_default();
    for entry in entries {
        // Recorded before the entry's space-above, so a fragment that begins here begins with that
        // space rather than swallowing it.
        boundaries.push(y);
        y += style.space_before_pt;

        let numbers_w = metrics.measure_run(&entry.folios, style.font_size_pt);
        let term_max = (width - numbers_w - INDEX_GAP_PT).max(1.0);
        let mut text = entry.term.clone();
        if metrics.measure_run(&text, style.font_size_pt) > term_max {
            while !text.is_empty()
                && metrics.measure_run(&format!("{text}…"), style.font_size_pt) > term_max
            {
                text.pop();
            }
            text.push('…');
        }

        let stops = [quill_text_layout::TabStop {
            position_pt: width,
            align: quill_text_layout::TabAlign::Right,
            leader: None,
        }];
        let mut line = tab_parts(
            &format!("{text}\t{}", entry.folios),
            &stops,
            0.0,
            y,
            style.font_size_pt,
            style.leading_pt,
            color,
            metrics,
        );
        parts.append(&mut line);
        y += style.leading_pt + style.space_after_pt;
    }

    // Cut between entries (spec 0075), for `measure_toc`'s reason: an entry is one line of
    // navigation and half of one is not navigation at all. An index is the block that makes this
    // mechanism unavoidable rather than merely correct — a 500-page book's contents list is three
    // pages, and its index is longer.
    let split = (boundaries.len() >= 2).then(|| {
        let mut items: Vec<f32> = Vec::with_capacity(boundaries.len());
        for i in 0..boundaries.len() {
            let start = if i == 0 { 0.0 } else { boundaries[i] };
            let end = boundaries.get(i + 1).copied().unwrap_or(y);
            items.push(end - start);
        }
        PanelSplit {
            items,
            // Nothing is re-stated on a continuation, for `measure_toc`'s reason: a repeated "Index"
            // reads as a second index rather than as the same one carrying on.
            repeat_parts: Vec::new(),
            repeat_decorations: Vec::new(),
            repeat_h: 0.0,
            trailing_pt: 0.0,
            min_items: MIN_ENTRIES_PER_FRAGMENT,
            preferred: Vec::new(),
            keep_together: true,
        }
    });

    (
        Measured::Panel {
            fill: None,
            stroke: None,
            parts,
            decorations: Vec::new(),
            split,
        },
        y,
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
    lay_out_with_fixpoint_status(content, assets, styles, template, metrics, hyphenator).0
}

/// [`lay_out_with_fixpoint_status`] over an explicit component library (spec 0054).
///
/// The frame-level entry points above deliberately do not take one: a frame primitive lays out the
/// *bundled* components, which is what every one of its callers wants, and threading a library
/// through them would add a parameter to forty call sites to say "the default" forty times. A
/// document that carries its own definitions reaches layout through [`lay_out`], which builds the
/// library from the document.
///
/// **Lays out no footnotes**, for the reason [`heading_index_of`] resolves no folios: a bare block
/// list carries none — they live on the document — so an anchor in one prints
/// [`quill_core_model::UNRESOLVED_REFERENCE`] rather than a number for a note nobody handed over.
/// A document reaches layout through [`lay_out`], which passes its own.
pub fn lay_out_with_library(
    content: &[Block],
    assets: &[Asset],
    styles: &StyleSheet,
    components: &ComponentLibrary,
    template: &impl PageTemplate,
    metrics: &impl RunMetrics,
    hyphenator: &impl Hyphenator,
) -> (Vec<LaidOutPage>, FixpointStatus) {
    lay_out_resolved(
        content,
        &[],
        // See above: a bare block list carries no forced breaks either — they live on the document.
        &[],
        assets,
        styles,
        components,
        template,
        metrics,
        hyphenator,
    )
}

/// How the layout fixpoint resolved (spec 0041, generalised by spec 0072).
///
/// Named for the fixpoint rather than for the contents list since spec 0072, because there are now
/// two derived quantities in it — the contents list's page numbers and the sections' start pages —
/// and a status called `toc` at every call site would be wrong at half of them.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct FixpointStatus {
    /// Layout passes run. `1` when the document derives nothing from its own layout.
    pub iterations: usize,
    /// Whether the derived quantities settled. `false` means the cap was reached and the **last
    /// iterate** was returned: a laid-out document with nothing missing, whose contents may disagree
    /// with a page number by one, or whose chapter opener may be on the wrong page. The caller can
    /// see it rather than being told a guess converged.
    pub converged: bool,
}

/// The most layout passes the fixpoint may take before giving up.
///
/// A bound is not optional. A contents entry can push a heading onto the next page, whose longer
/// number lengthens the entry, which pushes the heading further — or shortens it and pulls the
/// heading back, forever. Spec 0031 recorded unbounded "reflow until state matches" as the way a
/// pathological document hangs; a contents list is the case that actually oscillates.
///
/// Spec 0072 puts a second quantity under the same bound, and it oscillates far more readily than
/// the contents list does: a section's opener master changes the margins of the page the section
/// starts on, which can push the anchor onto the next page, where the same master then applies and
/// gives the previous page its capacity back. The bound is shared rather than per-quantity because
/// the cost that matters is the number of **full-document passes**, and one budget over the loop is
/// the only thing that bounds it. See spec 0076 for the counter the bench file is owed.
pub const FIXPOINT_MAX_ITERATIONS: usize = 8;

/// [`lay_out_with_template`], reporting how the layout fixpoint resolved.
///
/// A table of contents lists page numbers, its own length changes where every later page break
/// falls, and that changes the numbers it lists. So layout runs to a fixpoint: lay out, read where
/// the headings landed, regenerate the entries, lay out again, and stop when the index stops
/// changing.
///
/// Since spec 0072 a second quantity rides the same loop: a section is anchored to a block, so its
/// start page is read off the laid-out pages, and the master it puts there changes that page's
/// margins and column count — which can move the anchor. See [`PageTemplate::reassign`].
///
/// Documents that derive neither take exactly one pass. The loop is entered but exits on its first
/// comparison, so nothing about the cost or the behaviour of every other document changes.
pub fn lay_out_with_fixpoint_status(
    content: &[Block],
    assets: &[Asset],
    styles: &StyleSheet,
    template: &impl PageTemplate,
    metrics: &impl RunMetrics,
    hyphenator: &impl Hyphenator,
) -> (Vec<LaidOutPage>, FixpointStatus) {
    lay_out_resolved(
        content,
        // See [`lay_out_with_library`]: a bare block list carries no footnotes and no breaks.
        &[],
        &[],
        assets,
        styles,
        &builtin_components(),
        template,
        metrics,
        hyphenator,
    )
}

#[allow(clippy::too_many_arguments)]
fn lay_out_resolved(
    content: &[Block],
    notes: &[Block],
    breaks: &[PageBreak],
    assets: &[Asset],
    styles: &StyleSheet,
    components: &ComponentLibrary,
    template: &impl PageTemplate,
    metrics: &impl RunMetrics,
    hyphenator: &impl Hyphenator,
) -> (Vec<LaidOutPage>, FixpointStatus) {
    let once =
        |headings: &[HeadingEntry], index: &[IndexEntry], resolved: &BTreeMap<BlockId, String>| {
            flow(
                content,
                notes,
                breaks,
                assets,
                styles,
                components,
                headings,
                index,
                resolved,
                template,
                metrics,
                hyphenator,
                FlowState::start(template),
                &mut NoCache,
                None,
            )
            .pages
        };

    // Three derived quantities, one loop (specs 0072, 0076). Nesting them — settle the sections,
    // then settle the contents inside that — would multiply the pass count rather than add to it,
    // and a pass is a whole-document flow. They are also not independent: a section's opener master
    // changes the page count, which changes the numbers the contents list prints and the numbers the
    // cross-references print.
    let has_toc = content.iter().any(|b| matches!(b, Block::Toc { .. }));
    // A fourth derived quantity, riding the same loop for the same arithmetic (spec 0078): an index
    // is derived from the pages its marked runs landed on, its own length moves the pages it lists,
    // and it is not independent of the other three — a longer index moves nothing above it, but a
    // section's opener master moves what page every term is on.
    let has_index = content.iter().any(|b| matches!(b, Block::Index { .. }));
    let ignore_leading = index_ignore_leading(content);
    // Empty for every document without a cross-reference, which is what skips the derivation, the
    // comparison and the extra pass outright.
    let targets = quill_core_model::reference_targets(content);

    // **A footnote number is not a fourth derived quantity, and that is the numbering decision**
    // (spec 0077). It is derived from where the anchors sit in `content`, so it is known here,
    // before the first pass, and never changes across passes — exactly `list_markers`' shape. It
    // seeds the resolved map rather than joining the loop, so a document whose *only* new feature is
    // footnotes still converges in one pass.
    let numbers = quill_core_model::footnote_numbers(notes);

    let mut resolved: BTreeMap<BlockId, String> = numbers.clone();
    let mut pages = once(&[], &[], &resolved);
    // Asked once before the loop, so a template that derives nothing answers `false` and the loop
    // exits on its first comparison — one pass, exactly as before, for every document that has
    // neither a contents list, nor a section, nor a cross-reference.
    let mut reassigned = template.reassign(&pages);

    let mut headings: Vec<HeadingEntry> = Vec::new();
    let mut index: Vec<IndexEntry> = Vec::new();
    let mut iterations = 1;
    let converged = loop {
        let next = if has_toc {
            heading_index_with_folios(content, &pages, template)
        } else {
            Vec::new()
        };
        let next_index = if has_index {
            index_entries(content, &pages, template, ignore_leading)
        } else {
            Vec::new()
        };
        // The footnote numbers go back in every pass because the map is one map; they are constant,
        // so they can never be the reason the comparison fails.
        let mut next_refs = reference_folios(&targets, &pages, template);
        next_refs.extend(numbers.iter().map(|(k, v)| (*k, v.clone())));
        if next == headings && next_index == index && next_refs == resolved && !reassigned {
            break true;
        }
        if iterations >= FIXPOINT_MAX_ITERATIONS {
            break false;
        }
        headings = next;
        index = next_index;
        resolved = next_refs;
        pages = once(&headings, &index, &resolved);
        reassigned = template.reassign(&pages);
        iterations += 1;
    };

    // Furniture last, once, over the settled page vector (spec 0074) — **not** inside `once`, and
    // that is the whole reason a content-derived running head costs zero extra passes. A static
    // consumes no flow space, so resolving it after the flow cannot change the flow; resolving it
    // *during* the flow would have to read the previous iterate's heading index, and correcting that
    // would be another pass.
    //
    // The index is re-derived from the final pages rather than reusing the loop's `headings`, which
    // is the index the last pass was laid out *with* and is one iterate stale on a document that
    // failed to converge. Skipped entirely unless some master actually uses `{heading:N}`, so a
    // document that does not costs exactly what it cost before this increment.
    let heads = if template.statics_read_headings() {
        heading_index_of(content, &pages)
    } else {
        Vec::new()
    };
    place_statics(template, &mut pages, &heads, metrics);

    (
        pages,
        FixpointStatus {
            iterations,
            converged,
        },
    )
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
    /// How many of that block's items earlier frames already took (spec 0044): 0 at a block's
    /// start, non-zero when the flow was cut mid-block and this page continues it.
    ///
    /// This is what makes a checkpoint able to sit *inside* a block. It is an absolute item offset,
    /// interpreted against a fresh measurement at the resumed frame's width — sound because the
    /// loop only cuts when the continuation frame has that same width.
    pub split_at: usize,
    pub page_index: usize,
    pub frame_idx: usize,
    pub y: f32,
    pub frame_empty: bool,
    /// The footnote this page's first band opens with, part-set (spec 0077), or `None`.
    ///
    /// **The only new loop state a footnote adds, and working out that it is the only one is the
    /// increment's obligation to this type's doc comment.** The band a frame reserves is a function
    /// of the notes whose anchors were placed *in that frame*, and a checkpoint is only ever taken
    /// at a page boundary, where the first frame is empty and no anchor has been placed yet — so the
    /// accrued band height at a checkpoint is always zero and is not state.
    ///
    /// What is not zero is a note too tall for the band it was called into: its remainder continues
    /// at the foot of the next frame (spec 0075's mechanism, over the same item list), and a page
    /// boundary can fall in the middle of one. Resuming without it would drop the tail of a note and
    /// give the page back the height the tail was occupying — a different document after an edit
    /// than after a full pass, which is the exact failure this type's doc comment names.
    pub note_carry: Option<NoteCarry>,
}

/// A part-set footnote continuing into the next frame's band (spec 0077).
///
/// Absolute item offset into the note's own measurement, exactly as [`FlowState::split_at`] is into
/// the block's, and interpreted the same way: re-measured at the resumed frame's width and cut
/// again. Two `Copy` fields, so a checkpoint stays six numbers rather than a measured payload.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct NoteCarry {
    /// The note's id — its [`Block`] in the synthesised note list.
    pub note: BlockId,
    /// How many of its items earlier frames' bands already took. Strictly increases per frame,
    /// which is what bounds the band (see [`Band::commit`]).
    pub split_at: usize,
}

impl FlowState {
    fn start(template: &impl PageTemplate) -> FlowState {
        FlowState {
            block_idx: 0,
            split_at: 0,
            page_index: 0,
            frame_idx: 0,
            y: frames_for(template, 0)[0].rect.y_pt,
            frame_empty: true,
            note_carry: None,
        }
    }
}

/// Space between the body's last line and the rule that opens a footnote band (spec 0077).
pub(crate) const NOTE_RULE_SPACE_ABOVE_PT: f32 = 6.0;
/// Space between that rule and the first note under it.
pub(crate) const NOTE_RULE_SPACE_BELOW_PT: f32 = 3.0;
/// Thickness of the rule. A hairline in points, never in pixels — see [`Stroke::width_pt`].
pub(crate) const NOTE_RULE_THICKNESS_PT: f32 = 0.5;
/// How wide the rule is, as a fraction of the frame. A footnote rule is conventionally short.
pub(crate) const NOTE_RULE_WIDTH_FRACTION: f32 = 0.3;
/// The most of a frame a footnote band may take before it starts carrying the rest forward.
///
/// **Three quarters, and it is a correctness rule rather than a taste one.** The band is at the foot
/// of the frame the *anchor* lands in, so a band allowed to fill the frame would leave the anchor's
/// own paragraph nowhere to sit — and the flow's only answer to a block that fits nowhere is to
/// place it anyway and let it overflow, printing the paragraph over its own note. Capping the band
/// guarantees the anchor has somewhere to go.
///
/// It is a *preference*, not a hard limit: a note with no legal cut inside the cap is cut against
/// the whole frame instead, and one with no legal cut at all is taken whole and overflows. Progress
/// never depends on the cap — see [`Band::commit`].
pub(crate) const NOTE_BAND_MAX_FRACTION: f32 = 0.75;

/// The height a non-empty band spends on its separator, before any note.
fn note_band_lead_pt() -> f32 {
    NOTE_RULE_SPACE_ABOVE_PT + NOTE_RULE_THICKNESS_PT + NOTE_RULE_SPACE_BELOW_PT
}

/// The notes reserved at the foot of the frame currently being filled (spec 0077).
///
/// **This is the whole of the mechanism, and what it is *not* is the point.** It reduces the
/// `bottom`/available-height term the flow loop compares against and nothing else. No block is ever
/// measured against a height, `MeasureKey` gains no height dimension, and a note is measured exactly
/// as any other block is — at a frame width, through the same `Measurer`, so it is cached like one.
/// `Measured::break_items`' rule is that a measurement which depends on the available height must
/// offer no break opportunities; nothing here makes any measurement depend on one.
#[derive(Default, Clone)]
pub(crate) struct Band {
    /// The note fragments committed to this frame, in the order they will be stacked.
    items: Vec<BandItem>,
    /// Total height reserved, including the separator lead. `0.0` when empty, which is what makes a
    /// document with no footnote see exactly the `bottom` it always saw.
    height: f32,
}

#[derive(Clone)]
struct BandItem {
    source: BlockId,
    measured: Measured,
    height: f32,
    /// The note's ink, kept beside the measurement so the separator rule above the band is drawn in
    /// the colour the notes under it are set in — a black rule over CMYK notes would be a second
    /// answer to what colour this band is.
    ink: Color,
}

impl Band {
    /// How much of the frame's bottom this band has taken.
    fn height(&self) -> f32 {
        self.height
    }

    /// Add note `note` from item `from`, taking as much of it as `frame_h` leaves room for.
    ///
    /// Returns the carry: `Some` when the note was cut and its remainder must open the next frame's
    /// band, `None` when it was taken whole.
    ///
    /// **Progress is what the three branches are for.** A cut advances the absolute item offset
    /// strictly, exactly as spec 0044's does, so the note is consumed within its own item count. The
    /// third branch — no legal cut fits — takes the note whole and lets it overflow rather than
    /// carrying nothing forward, which is the only way this could fail to terminate and is the same
    /// answer the `frame_empty` guard already gives a block too tall for an empty frame. A band may
    /// therefore exceed its frame, but only by ending the carry, and that is asserted rather than
    /// reasoned about.
    ///
    /// `allow_cut` is `false` for every note but the last one a placement calls for, and for every
    /// note once a carry already exists. A frame's band may therefore carry **at most one** note
    /// forward, and it is the last one in the band — without that, two cuts in one frame would
    /// leave the second overwriting the first and a note would silently disappear.
    #[allow(clippy::too_many_arguments)]
    fn commit<M: RunMetrics, H: Hyphenator>(
        &mut self,
        note: &Block,
        from: usize,
        frame_w: f32,
        frame_h: f32,
        allow_cut: bool,
        ctx: &BlockContext<'_>,
        measurer: &mut impl Measurer,
        metrics: &M,
        hyphenator: &H,
    ) -> Option<NoteCarry> {
        let Some((whole, whole_h)) = measurer.measure(note, frame_w, ctx, metrics, hyphenator)
        else {
            return None; // a note that cannot be measured is simply not set
        };
        let (measured, height) = match whole.split_at(from) {
            Some((_, _, remainder, remainder_h)) => (remainder, remainder_h),
            None => (whole, whole_h),
        };
        let lead = if self.items.is_empty() {
            note_band_lead_pt()
        } else {
            0.0
        };
        // Two rooms, tried in order: the cap, then the whole frame. A note that fits the cap is
        // taken whole; one that does not is cut at the cap if it legally can; and only if *neither*
        // works does the frame's full height come into it. That ordering is what keeps a 90%-tall
        // note from squeezing its own anchor out while still never depending on the cap to
        // terminate.
        let cap_room = frame_h * NOTE_BAND_MAX_FRACTION - self.height - lead;
        let full_room = frame_h - self.height - lead;
        let cut = |band: &mut Self, room: f32| {
            let k = measured.cut_fitting(room).filter(|_| allow_cut)?;
            let (fragment, fragment_h, _, _) = measured
                .split_at(k)
                .expect("`cut_fitting` returned an index `split_at` refuses");
            assert!(
                k > 0,
                "a footnote cut must advance the note's item offset, or the band cannot terminate"
            );
            band.push(lead, note.id(), fragment, fragment_h);
            Some(NoteCarry {
                note: note.id(),
                split_at: from + k,
            })
        };
        let carry = if height <= cap_room {
            self.push(lead, note.id(), measured.clone(), height);
            None
        } else if let Some(c) = cut(self, cap_room) {
            Some(c)
        } else if height <= full_room {
            self.push(lead, note.id(), measured.clone(), height);
            None
        } else if let Some(c) = cut(self, full_room) {
            Some(c)
        } else {
            // No legal cut fits the frame at all. Take it whole and overflow, which ends the carry
            // — the only branch that can leave the band taller than its frame, and the only reason
            // this cannot fail to make progress.
            self.push(lead, note.id(), measured.clone(), height);
            None
        };
        assert!(
            self.height <= frame_h || carry.is_none(),
            "a footnote band that overruns its frame must not also carry forward, or the flow \
             cannot make progress"
        );
        carry
    }

    fn push(&mut self, lead: f32, source: BlockId, measured: Measured, height: f32) {
        self.height += lead + height;
        let ink = match &measured {
            Measured::Text { color, .. } => *color,
            Measured::Panel { parts, .. } => {
                parts.first().map_or(Color::Gray { v: 0.0 }, |p| p.color)
            }
            Measured::Image { .. } => Color::Gray { v: 0.0 },
        };
        self.items.push(BandItem {
            source,
            measured,
            height,
            ink,
        });
    }

    /// Emit the band's placed geometry at the foot of `frame`, and empty it for the next frame.
    ///
    /// Into `LaidOutPage::blocks` rather than `statics`, because a note is *content*: it consumes
    /// flow space, it carries the note's own [`BlockId`], and `preflight_pages` walks blocks, so a
    /// note gets the safe-area and ink checks every other placed thing gets for free.
    fn flush(&mut self, page: &mut LaidOutPage, frame: &Frame, metrics: &impl RunMetrics) {
        if self.items.is_empty() {
            self.height = 0.0;
            return;
        }
        let mut y = frame.rect.y_pt + frame.rect.h_pt - self.height + NOTE_RULE_SPACE_ABOVE_PT;
        page.blocks.push(PlacedBlock::Rect {
            frame: Rect {
                x_pt: frame.rect.x_pt,
                y_pt: y,
                w_pt: frame.rect.w_pt * NOTE_RULE_WIDTH_FRACTION,
                h_pt: NOTE_RULE_THICKNESS_PT,
            },
            fill: Some(self.items[0].ink),
            stroke: None,
        });
        y += NOTE_RULE_THICKNESS_PT + NOTE_RULE_SPACE_BELOW_PT;
        for item in std::mem::take(&mut self.items) {
            page.blocks.extend(place_measured(
                item.measured,
                item.height,
                frame,
                y,
                item.source,
                metrics,
            ));
            y += item.height;
        }
        self.height = 0.0;
    }
}

/// Two frame widths are the same measure when they agree to within a hundredth of a point — the
/// tolerance the repo's geometry assertions already use.
fn same_width(a: f32, b: f32) -> bool {
    (a - b).abs() <= 0.01
}

/// The width of the frame a block's continuation would land in: the next frame on this page, or the
/// first frame of the next page when this one is exhausted.
fn continuation_frame(
    frames: &[Frame],
    frame_idx: usize,
    template: &impl PageTemplate,
    page_index: usize,
) -> (f32, f32) {
    match frames.get(frame_idx + 1) {
        Some(next) => (next.rect.w_pt, next.rect.h_pt),
        None => {
            let r = frames_for(template, page_index + 1)[0].rect;
            (r.w_pt, r.h_pt)
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
        ctx: &BlockContext<'_>,
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
        ctx: &BlockContext<'_>,
        metrics: &M,
        hyphenator: &H,
    ) -> Option<(Measured, f32)> {
        measure_block(block, width, ctx, metrics, hyphenator)
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

/// Which footnotes a placement calls for, in the order they are anchored (spec 0077).
///
/// Read off the **measurement**, not the block, so a block that was cut across a frame boundary
/// attributes each anchor to the fragment its line actually landed in — which is what makes "an
/// anchor and its note are on the same page" true rather than usually true. A line's spans name the
/// runs it is made of, and a run names its note.
///
/// A measurement with no lines to attribute by — a tabbed paragraph measures as a panel, and is
/// indivisible — falls back to the block's runs: it is placed whole, so every note it calls belongs
/// to the frame it lands in.
fn notes_in(measured: &Measured, block: &Block) -> Vec<BlockId> {
    let runs = block.runs();
    if runs.iter().all(|r| r.source.note().is_none()) {
        return Vec::new();
    }
    let mut out: Vec<BlockId> = Vec::new();
    let add = |id: BlockId, out: &mut Vec<BlockId>| {
        if !out.contains(&id) {
            out.push(id);
        }
    };
    match measured {
        Measured::Text { lines, .. } => {
            for line in lines {
                for span in &line.spans {
                    if let Some(id) = runs.get(span.run).and_then(|r| r.source.note()) {
                        add(id, &mut out);
                    }
                }
            }
        }
        Measured::Image { .. } | Measured::Panel { .. } => {
            for id in runs.iter().filter_map(|r| r.source.note()) {
                add(id, &mut out);
            }
        }
    }
    out
}

/// The band this frame would have if `ids` were committed to it — **without committing them**.
///
/// The fit check has to know the reserved height before it knows whether the block fits, and the
/// block only fits if the height is reserved: the circularity is broken by reserving on a *copy* and
/// keeping it only when the placement is taken. Reserving against the block's whole set of notes and
/// then committing only the fragment's is conservative in the safe direction — a fragment can end up
/// with more room than was reserved, never less.
#[allow(clippy::too_many_arguments)]
fn reserve<M: RunMetrics, H: Hyphenator>(
    band: &Band,
    carry: Option<NoteCarry>,
    ids: &[BlockId],
    notes: &BTreeMap<BlockId, &Block>,
    frame: &Frame,
    ctx: &BlockContext<'_>,
    measurer: &mut impl Measurer,
    metrics: &M,
    hyphenator: &H,
) -> (Band, Option<NoteCarry>) {
    let mut trial = band.clone();
    let mut out = carry;
    for (i, id) in ids.iter().enumerate() {
        let Some(note) = notes.get(id) else {
            // An anchor naming a note the document does not hold. The anchor still prints — it
            // prints `[?]` — and there is nothing to reserve.
            continue;
        };
        let allow_cut = out.is_none() && i + 1 == ids.len();
        if let Some(c) = trial.commit(
            note,
            0,
            frame.rect.w_pt,
            frame.rect.h_pt,
            allow_cut,
            ctx,
            measurer,
            metrics,
            hyphenator,
        ) {
            out = Some(c);
        }
    }
    (trial, out)
}

/// Open a frame's band with whatever note the previous frame carried forward (spec 0077).
#[allow(clippy::too_many_arguments)]
fn open_band<M: RunMetrics, H: Hyphenator>(
    carry: Option<NoteCarry>,
    notes: &BTreeMap<BlockId, &Block>,
    frame: &Frame,
    ctx: &BlockContext<'_>,
    measurer: &mut impl Measurer,
    metrics: &M,
    hyphenator: &H,
) -> (Band, Option<NoteCarry>) {
    let mut band = Band::default();
    let Some(c) = carry else {
        return (band, None);
    };
    let Some(note) = notes.get(&c.note) else {
        return (band, None);
    };
    let next = band.commit(
        note,
        c.split_at,
        frame.rect.w_pt,
        frame.rect.h_pt,
        true,
        ctx,
        measurer,
        metrics,
        hyphenator,
    );
    if let Some(n) = next {
        assert!(
            n.split_at > c.split_at,
            "a carried footnote must take at least one item per frame, or the flow cannot terminate"
        );
    }
    (band, next)
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
    notes: &[Block],
    breaks: &[PageBreak],
    assets: &[Asset],
    styles: &StyleSheet,
    components: &ComponentLibrary,
    headings: &[HeadingEntry],
    index: &[IndexEntry],
    resolved: &BTreeMap<BlockId, String>,
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
    let markers = list_markers(content, styles);
    // id → note block, for the same reason `assets` is an index: an anchor resolves its note once
    // per placement attempt. Empty for every document without a footnote.
    let note_index: BTreeMap<BlockId, &Block> = notes.iter().map(|b| (b.id(), b)).collect();
    // id → the break in front of that block (spec 0080). Built here, beside the note index, for the
    // same two reasons: the loop asks per block, and it must not ask a list.
    //
    // `BreakKind::None` is filtered out rather than carried, so "there is an entry" and "the flow
    // does something" are the same statement inside the loop. **Empty for every document that
    // states no break**, which is what makes such a document take exactly the path it took before
    // this increment — the map is consulted once per block and answers nothing.
    let break_index: BTreeMap<BlockId, BreakKind> = breaks
        .iter()
        .filter(|b| b.kind.is_break())
        .map(|b| (b.before, b.kind))
        .collect();
    let facing_pages = template.facing_pages();
    let ctx = BlockContext {
        assets: &assets,
        styles,
        headings,
        index,
        components,
        markers: &markers,
        resolved,
    };
    let grid = template.baseline_grid().filter(BaselineGrid::is_usable);

    let mut pages: Vec<LaidOutPage> = Vec::new();
    let mut checkpoints: Vec<FlowState> = Vec::new();
    let mut page_index: usize = start.page_index;
    let mut frames = frames_for(template, page_index);
    let mut page = LaidOutPage {
        index: page_index,
        blocks: Vec::new(),
        // Left empty here and filled by `place_statics` once the pass is over (spec 0074). A
        // `{heading:N}` running head is a function of what landed on the page, which the flow does
        // not know until it has finished laying it out.
        statics: Vec::new(),
    };
    // Which frame on the current page the cursor is filling.
    let mut frame_idx: usize = start.frame_idx;
    // Absolute y cursor, starting at the current frame's top and reset there on each frame advance.
    let mut y: f32 = start.y;
    // Whether the *current* frame has received a block yet — mirrors incr. 1's page-empty guard,
    // now per frame so an oversized block is placed rather than skipped through every frame/page.
    let mut frame_empty = start.frame_empty;
    checkpoints.push(start);
    // The notes reserved at the foot of the frame being filled, and the one note (if any) whose
    // remainder the *next* frame's band opens with (spec 0077). Both are empty for every document
    // without a footnote, and an empty band reduces nothing.
    let (mut band, mut carry) = open_band(
        start.note_carry,
        &note_index,
        &frames[frame_idx],
        &ctx,
        measurer,
        metrics,
        hyphenator,
    );

    for (offset, block) in content[start.block_idx..].iter().enumerate() {
        let block_idx = start.block_idx + offset;
        // How much of this block earlier frames already took (spec 0044). Non-zero only for the
        // block a resumed flow was in the middle of.
        let mut split_at = if offset == 0 { start.split_at } else { 0 };

        // **The forced page break** (spec 0080), and the whole of it: advance pages until the flow
        // is at the top of a page the break's kind accepts. Skipped when `split_at > 0`, because a
        // break belongs to the seam in front of a block and a *continuation* of that block has
        // already crossed it — re-firing on the remainder would page for ever.
        //
        // **Three properties, and each is what stops something going wrong:**
        //
        // 1. *A break at the top of an untouched page is a no-op.* `frame_empty && frame_idx == 0`
        //    is exactly "this page has received nothing" — `frame_empty` is set true on every frame
        //    and page advance, and frames are filled in order, so an empty first frame means an
        //    empty page. Without this, the first block of a document, and every block whose break
        //    the flow has just honoured, would insert a spurious blank page. It is also what makes
        //    a *chapter opened on its own* cost nothing: page 0 has received nothing.
        // 2. *A break advances a **page**, never a frame.* On a two-column page a break in the
        //    **first** column does not settle for the second — the page carries the previous
        //    chapter's text either way — so a non-empty page fails the test however many frames it
        //    has left, and the loop advances the page rather than the frame. (A break in the *last*
        //    column is the case that cannot tell the two apart, which is why the test is written
        //    against the first.)
        // 3. *It terminates, and the bound is two.* Each turn either advances to a page that has
        //    received nothing (making 1 true) or, having done that, flips the page's parity — and
        //    parity alternates, so at most one blank page is ever inserted. The assertion states
        //    that rather than trusting it: this is a **new way to advance without consuming
        //    content**, which is the one thing spec 0044's progress invariants do not cover, since
        //    they bound a cut of a block and a blank page cuts nothing at all.
        if split_at == 0 {
            if let Some(kind) = break_index.get(&block.id()).copied() {
                let mut advances = 0usize;
                while !(frame_idx == 0 && frame_empty && kind.accepts(page_index, facing_pages)) {
                    debug_assert!(
                        frame_idx > 0 || !frame_empty || page.blocks.is_empty(),
                        "a page whose first frame is empty must have received nothing"
                    );
                    // The band belongs to the frame being left, exactly as in the doesn't-fit
                    // branch: a page inserted for parity still sets whatever note the previous page
                    // carried into it, rather than dropping the tail of a note to stay blank.
                    band.flush(&mut page, &frames[frame_idx], metrics);
                    pages.push(page);
                    page_index += 1;
                    frames = frames_for(template, page_index);
                    page = LaidOutPage {
                        index: page_index,
                        blocks: Vec::new(),
                        statics: Vec::new(),
                    };
                    frame_idx = 0;
                    // A page emitted by a break is a page like any other and records where the flow
                    // had reached, so a later edit can resume from it. **The state is unchanged by
                    // this increment**: resuming here re-evaluates the break against this page's
                    // own parity and re-derives the same decision, which is why a blank page's
                    // checkpoint reproduces a blank page rather than filling it in.
                    checkpoints.push(FlowState {
                        block_idx,
                        split_at,
                        page_index,
                        frame_idx: 0,
                        y: frames[0].rect.y_pt,
                        frame_empty: true,
                        note_carry: carry,
                    });
                    let (b, c) = open_band(
                        carry,
                        &note_index,
                        &frames[0],
                        &ctx,
                        measurer,
                        metrics,
                        hyphenator,
                    );
                    band = b;
                    carry = c;
                    y = frames[0].rect.y_pt;
                    frame_empty = true;
                    advances += 1;
                    assert!(
                        advances <= 2,
                        "a forced page break must be satisfied within one page advance and at \
                         most one blank page, or the flow cannot terminate"
                    );
                }
            }
        }
        // Advance frames / pages until the block fits, then place it. The block is re-measured
        // against each candidate frame's width (wrapping/sizing depend on it), so a block that
        // advances into a narrower frame re-wraps to that width rather than keeping a stale
        // measurement. Since spec 0044 a block that does not fit may leave a fragment behind before
        // advancing, so the loop can now run once per frame the block spans; `split_at` strictly
        // increases each time, which is what bounds it.
        loop {
            let frame = frames[frame_idx];
            let Some((whole, whole_height)) =
                measurer.measure(block, frame.rect.w_pt, &ctx, metrics, hyphenator)
            else {
                break; // unresolved image asset → skip this block (no panic)
            };
            // Take the part earlier frames have not had. Re-measuring and re-cutting, rather than
            // carrying the remainder along, is what keeps the re-measure-per-frame invariant and
            // what lets a checkpoint be five `Copy` fields instead of a measured payload.
            let (measured, height) = match whole.split_at(split_at) {
                Some((_, _, remainder, remainder_h)) => (remainder, remainder_h),
                None => (whole, whole_height),
            };
            // Snap to the baseline grid (spec 0058), before the fit check rather than after: a
            // block pushed down by the snap may no longer fit, and must then move on like any
            // other. `O(1)`, reading one number and writing one — the grid is per frame and local,
            // and a global recompute is a named non-goal.
            //
            // A block that occupies nothing is skipped: an empty table draws no ink, and snapping
            // it would push everything after it down by up to a whole step for a block the reader
            // cannot see.
            if let Some(grid) = grid {
                if height > 0.0 {
                    let off = measured.first_baseline_offset(metrics).unwrap_or(0.0);
                    y = grid.snap_down(y + off) - off;
                }
            }
            // **The one place a footnote touches the flow** (spec 0077): the frame's bottom, and
            // nothing else. The notes this placement would call for are reserved on a *copy* of the
            // band, because the block only fits if the space is reserved and the space is only
            // reserved if the block fits — the copy is kept when the placement is taken and thrown
            // away when it is not. No block is measured against a height anywhere in here, and
            // `MeasureKey` gains no height dimension: a note is measured at a frame *width*, through
            // the same `Measurer`, exactly as any other block is.
            //
            // `wanted` is empty for every block in every document without a footnote, and an empty
            // reservation leaves `bottom` the number it has always been.
            let wanted = notes_in(&measured, block);
            let (trial, trial_carry) = if wanted.is_empty() {
                (band.clone(), carry)
            } else {
                reserve(
                    &band,
                    carry,
                    &wanted,
                    &note_index,
                    &frame,
                    &ctx,
                    measurer,
                    metrics,
                    hyphenator,
                )
            };
            let bottom = frame.rect.y_pt + frame.rect.h_pt - trial.height();

            if y + height > bottom {
                // Fill this frame with as much of the block as legally fits before moving on
                // (spec 0044) — but only when the continuation lands in a frame of the same width.
                // The cut is an index into the line list *at this width*, and a frame of another
                // width re-wraps to a different list against which that index means something else.
                let mut cut_taken = false;
                let (next_w, next_h) = continuation_frame(&frames, frame_idx, template, page_index);
                // A composite would rather move whole than be cut, when moving whole actually
                // works (spec 0046). Splitting a stat block that would have fitted the next column
                // intact is a worse page than moving it — which is the keep-together spec 0038
                // promised — while a paragraph is the opposite case and must still be cut.
                //
                // `next_h` is the next frame's *unreduced* height, and a carried note will have
                // taken part of it before this block ever reaches it (spec 0077). Comparing against
                // a height that will no longer exist is how keep-together would move a panel into a
                // frame it does not fit. Skipped entirely when nothing is carried, so no document
                // without a footnote sees a different number.
                let next_h = match carry {
                    None => next_h,
                    Some(_) => {
                        let next = Frame {
                            rect: Rect {
                                w_pt: next_w,
                                h_pt: next_h,
                                ..frame.rect
                            },
                        };
                        let (opened, _) = open_band(
                            carry,
                            &note_index,
                            &next,
                            &ctx,
                            measurer,
                            metrics,
                            hyphenator,
                        );
                        next_h - opened.height()
                    }
                };
                let keep_whole = measured.prefers_keep_together() && height <= next_h;
                if !keep_whole && same_width(frame.rect.w_pt, next_w) {
                    if let Some(k) = measured.cut_fitting(bottom - y) {
                        if let Some((fragment, fragment_h, remainder, _)) = measured.split_at(k) {
                            // **The progress invariant** (spec 0044, and 0045's narrowed
                            // `frame_empty` guard rests on it): this loop runs once per frame the
                            // block spans and is bounded by nothing else. A cut that left an empty
                            // fragment, or one that did not advance the absolute offset, would spin
                            // here forever — and a cut whose remainder was empty would place the
                            // block's tail twice.
                            //
                            // Structural until spec 0075, which is the increment that adds new ways
                            // for a cut to be degenerate: a contents list's entries and a
                            // composite's elements are both item lists derived somewhere else. 0044
                            // called these "invariants worth asserting rather than reasoning
                            // about", so they are asserted — once per cut, which is nothing beside
                            // the measurement it follows.
                            let advanced = split_at + k;
                            assert!(k > 0, "a cut must leave a non-empty fragment behind");
                            assert!(
                                remainder.item_count().is_some_and(|n| n > 0),
                                "a cut must carry a non-empty remainder forward"
                            );
                            assert!(
                                advanced > split_at,
                                "the absolute item offset must strictly increase across a cut, \
                                 or the flow loop cannot terminate"
                            );
                            // Reserve for real, against the notes the **fragment** actually holds
                            // rather than the whole block's — the anchors in the remainder travel
                            // with it into the next frame, and their notes with them. That is what
                            // makes "an anchor and its note are on the same page" hold across a cut
                            // and not merely usually.
                            if !wanted.is_empty() {
                                let taken = notes_in(&fragment, block);
                                let (b, c) = reserve(
                                    &band,
                                    carry,
                                    &taken,
                                    &note_index,
                                    &frame,
                                    &ctx,
                                    measurer,
                                    metrics,
                                    hyphenator,
                                );
                                band = b;
                                carry = c;
                            }
                            page.blocks.extend(place_measured(
                                fragment,
                                fragment_h,
                                &frame,
                                y,
                                block.id(),
                                metrics,
                            ));
                            split_at = advanced;
                            cut_taken = true;
                        }
                    }
                }
                // A block too tall for an *empty* frame used to be placed there and allowed to
                // overflow, because moving it on would loop past every frame forever. That guard is
                // still needed for a block that cannot be cut — an image, a single enormous row —
                // but it is the wrong answer for one that can: spec 0045's 500-row table starts its
                // own page, so the frame is empty, and placing it whole would run it off the bottom
                // of the book. A cut advances the absolute offset, so progress is guaranteed and the
                // guard is not required to reach it (spec 0045, extending 0044).
                // `keep_whole` is excluded deliberately (spec 0075). Keep-together declined the cut
                // *because the block fits the next frame entire*, so the answer is to move it
                // there — and the empty-frame guard used to override exactly that, placing the
                // block in a frame it does not fit while a frame that would have held it whole sat
                // one column away. On the `reference` template that is not a corner case: page 0
                // gives 378 pt columns and page 1 gives 540 pt ones, so a 440 pt panel prefers to
                // move, and then overflowed where it stood. It is half of the recorded stat-block
                // overflow, and the half no amount of finer cutting could have reached.
                //
                // Progress is not at risk: `keep_whole` is only true when the block fits the
                // continuation frame entire, so the next iteration places it. One move, never a
                // loop.
                //
                // The guard asks whether the **committed** band leaves any room, not the
                // prospective one (spec 0077). A frame whose band is already full — a carried note
                // that took the whole of it — has nowhere to put the block, and forcing it in there
                // would print the block over the note. Moving on is safe because it is bounded:
                // a carry takes at least one item of its note per frame, so it is consumed within
                // that note's own item count and the band cannot stay full for ever. What the guard
                // still does, unchanged, is place a block whose *own* notes are what it cannot fit
                // beside — it overflows, loudly, rather than looping.
                let band_leaves_room = band.height() < frame.rect.h_pt;
                if !cut_taken && frame_empty && !keep_whole && band_leaves_room {
                    // Cannot be cut and the frame is empty: place it and let it overflow.
                } else {
                    // Doesn't fit → move on before placing. The band belongs to the frame being
                    // left, so it is set at its foot now; the frame moved into opens a fresh one
                    // with whatever this one carried forward.
                    band.flush(&mut page, &frame, metrics);
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
                            statics: Vec::new(), // see above: `place_statics` fills these
                        };
                        frame_idx = 0;
                        // Record where the new page begins, so a later edit can resume from here.
                        let at_boundary = FlowState {
                            block_idx,
                            split_at,
                            page_index,
                            frame_idx: 0,
                            y: frames[0].rect.y_pt,
                            frame_empty: true,
                            note_carry: carry,
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
                    let (b, c) = open_band(
                        carry,
                        &note_index,
                        &frames[frame_idx],
                        &ctx,
                        measurer,
                        metrics,
                        hyphenator,
                    );
                    band = b;
                    carry = c;
                    y = frames[frame_idx].rect.y_pt;
                    frame_empty = true;
                    continue; // re-measure against the frame it moved into
                }
            }

            // The placement is taken, so the reservation it was checked against becomes the frame's
            // real band. Nothing else in the loop writes `band`, which is what keeps a reservation
            // that was thrown away from leaving a hole at the foot of a frame.
            band = trial;
            carry = trial_carry;
            page.blocks.extend(place_measured(
                measured,
                height,
                &frame,
                y,
                block.id(),
                metrics,
            ));
            y += height;
            frame_empty = false;
            break;
        }
    }

    // Drain whatever the last frame carried forward (spec 0077). A note called near the end of a
    // document can be taller than the space left for it, and there is no block after it to advance
    // the frame — so the flow adds the frames the note still needs itself, rather than dropping its
    // tail. Bounded by the note's own item count: `open_band` asserts that each frame takes at least
    // one item, so this runs at most once per remaining item.
    while carry.is_some() {
        band.flush(&mut page, &frames[frame_idx], metrics);
        if frame_idx + 1 < frames.len() {
            frame_idx += 1;
        } else {
            pages.push(page);
            page_index += 1;
            frames = frames_for(template, page_index);
            page = LaidOutPage {
                index: page_index,
                blocks: Vec::new(),
                statics: Vec::new(),
            };
            frame_idx = 0;
            checkpoints.push(FlowState {
                block_idx: content.len(),
                split_at: 0,
                page_index,
                frame_idx: 0,
                y: frames[0].rect.y_pt,
                frame_empty: true,
                note_carry: carry,
            });
        }
        let (b, c) = open_band(
            carry,
            &note_index,
            &frames[frame_idx],
            &ctx,
            measurer,
            metrics,
            hyphenator,
        );
        band = b;
        carry = c;
    }
    // The last frame's band, before the last page is emitted.
    band.flush(&mut page, &frames[frame_idx], metrics);
    // Always emit the last (possibly empty) page so callers receive >= 1 page.
    pages.push(page);
    FlowResult {
        pages,
        checkpoints,
        resynced_at: None,
    }
}

/// Turn a measurement into placed geometry at the flow cursor.
///
/// A block usually yields one placed item; a composite (spec 0038) yields its panel plus one text
/// run per section. Since spec 0044 a `measured` may be a *fragment* of a block rather than the
/// whole of it — placement does not care, and deliberately: every fragment carries the same
/// `source`, which is what lets the heading index and the conservation invariant treat the pieces
/// as one block.
///
/// `metrics` is here because [`PlacedBlock`]'s frame is the ink (spec 0069) and ink has to be
/// measured. It is the same run-metrics the block was broken with — a second one could report a
/// width the text was never set to.
fn place_measured(
    measured: Measured,
    height: f32,
    frame: &Frame,
    y: f32,
    source: BlockId,
    metrics: &impl RunMetrics,
) -> Vec<PlacedBlock> {
    match measured {
        Measured::Text {
            lines,
            color,
            run_colors,
            run_formats,
            run_shifts,
            marker,
            style,
        } => {
            // The paragraph's own format: what a span with no override of its own is set in, and
            // the same `base` both painters measure against (spec 0064).
            let base = RunFormat {
                size_pt: style.font_size_pt,
                weight: style.weight.0,
                italic: style.italic,
                tracking_pt: 0.0,
            };
            let (ink_dx, ink_w) = quill_text_layout::ink_box(&lines, &run_formats, base, metrics);
            let text_block = PlacedBlock::Text {
                source,
                run_colors,
                run_formats,
                run_shifts,
                weight: style.weight.0,
                italic: style.italic,
                // The frame starts *below* the style's space-above and ends at the last line's
                // bottom: both spaces belong to this block's height (so pagination reserves them)
                // but no text is drawn in either, and the frame is the ink (spec 0069).
                frame: Rect {
                    x_pt: frame.rect.x_pt + ink_dx,
                    y_pt: y + style.space_before_pt,
                    w_pt: ink_w,
                    h_pt: lines.len() as f32 * style.leading_pt,
                },
                lines,
                color,
                // Carried through to the writer. Without this the exported PDF would set every
                // paragraph at body size regardless of its style, because `PlacedBlock` was the
                // only thing the writer sees and it did not record how the text was measured.
                font_size_pt: style.font_size_pt,
                leading_pt: style.leading_pt,
            };
            let Some(marker) = marker else {
                return vec![text_block];
            };
            // The marker sits in the gutter the item's indent opens, on its first baseline —
            // never in the text flow, so it cannot move a break, and every line of the item is
            // inset past it so a wrap lines up under the text (spec 0048's `Indent` opens the
            // gutter; this only fills it).
            let PlacedBlock::Text {
                frame: text_frame, ..
            } = &text_block
            else {
                unreachable!("just built a text block")
            };
            // The gutter is the slot; the marker's own glyphs are what it reports (spec 0069). A
            // bullet in a 24 pt gutter is a few points of ink, and saying "24 pt" of it would put
            // the item's box under the safe-area check for text that is not there. Its x is the
            // frame's left edge rather than the item text's, which the item's indent has already
            // moved right.
            let marker_w = metrics.measure_format(
                &marker,
                RunFormat {
                    size_pt: style.font_size_pt,
                    weight: style.weight.0,
                    italic: style.italic,
                    tracking_pt: 0.0,
                },
            );
            let marker_block = PlacedBlock::Text {
                source,
                frame: Rect {
                    x_pt: frame.rect.x_pt,
                    y_pt: text_frame.y_pt,
                    w_pt: marker_w,
                    h_pt: style.leading_pt,
                },
                lines: vec![Line::single_run(marker, 0.0, 0.0)],
                color,
                // A marker is set in the paragraph's own format, so it needs no per-run table.
                run_formats: Vec::new(),
                run_shifts: Vec::new(),
                weight: style.weight.0,
                italic: style.italic,
                // One run by construction: a marker is generated, not authored.
                run_colors: Vec::new(),
                font_size_pt: style.font_size_pt,
                leading_pt: style.leading_pt,
            };
            vec![marker_block, text_block]
        }
        Measured::Image { asset_id, width } => vec![PlacedBlock::Image {
            source,
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
            decorations,
            split: _,
        } => {
            // The panel first, so it sits behind its own text — the same
            // decoration-before-content order the writer and the paint list rely on.
            //
            // Omitted entirely when it has neither fill nor stroke. A table has no outer
            // panel, only bands and a rule, and spec 0037's rule is that a rectangle
            // drawing nothing emits nothing — it belongs here as much as in the writer, or
            // every table carries an invisible rect through the whole pipeline.
            let mut out = Vec::new();
            if fill.is_some() || stroke.is_some() {
                out.push(PlacedBlock::Rect {
                    frame: Rect {
                        x_pt: frame.rect.x_pt,
                        y_pt: y,
                        w_pt: frame.rect.w_pt,
                        h_pt: height,
                    },
                    fill,
                    stroke,
                });
            }
            out.extend(decorations.into_iter().map(|d| PlacedBlock::Rect {
                frame: Rect {
                    x_pt: frame.rect.x_pt + d.dx_pt,
                    y_pt: y + d.dy_pt,
                    w_pt: d.w_pt,
                    h_pt: d.h_pt,
                },
                fill: d.fill,
                stroke: d.stroke,
            }));
            for p in parts {
                // The part's ink, not the column or the panel's inner width it was broken to
                // (spec 0069). The measure stays on the `PanelPart`, which is where the producer
                // knows it and where the containment invariants are asserted.
                let rect = Rect {
                    x_pt: frame.rect.x_pt + p.dx_pt + p.ink_dx_pt,
                    y_pt: y + p.dy_pt,
                    w_pt: p.ink_w_pt,
                    h_pt: p.lines.len() as f32 * p.leading_pt,
                };
                // The link candidate is emitted from the *same* rectangle as the run it belongs to
                // (spec 0052), rather than from a separately computed one: a link whose hot area
                // has drifted off its own text is the defect this makes structurally impossible.
                //
                // Sharing the rectangle only makes the hot area right if the rectangle is the ink,
                // which is why this claim was true vertically and false horizontally until spec
                // 0070: the only part carrying `link_page` is a contents entry's title, and it
                // reported the column it was clipped to, so the hot area ran across the dot leader.
                if let Some(target_page) = p.link_page {
                    out.push(PlacedBlock::Link {
                        source,
                        frame: rect,
                        target_page,
                    });
                }
                out.push(PlacedBlock::Text {
                    source,
                    frame: rect,
                    lines: p.lines,
                    color: p.color,
                    // A panel part is one run: its text comes from a component field, and rich
                    // text inside a declared component's sections is a later increment.
                    run_colors: Vec::new(),
                    run_formats: Vec::new(),
                    run_shifts: Vec::new(),
                    // A panel's sections are set in the paragraph face. Weight and slant on the
                    // styles a *declared component* resolves are where the component interpreter
                    // reads them, and are not this increment's (spec 0064).
                    weight: 400,
                    italic: false,
                    font_size_pt: p.font_size_pt,
                    leading_pt: p.leading_pt,
                });
            }
            out
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use quill_core_model::{DefColor, SectionShape, PANEL_PADDING_PT};
    use quill_text_layout::justify_paragraph_hyphenated;

    /// A definition's declared colour as the engine's own, so a test asserts against the
    /// *definition* rather than against a number copied out of it. A duplicated literal stops
    /// testing anything the moment the definition changes (spec 0054).
    fn ink(c: DefColor) -> Color {
        match c {
            DefColor::Gray { v } => Color::Gray { v },
            DefColor::Cmyk { c, m, y, k } => Color::Cmyk { c, m, y, k },
        }
    }

    /// The bundled stat block's panel tint.
    #[allow(non_snake_case)]
    fn PANEL_FILL() -> Color {
        ink(quill_core_model::statblock_definition()
            .panel
            .fill
            .expect("the bundled stat block declares a panel fill"))
    }

    /// The bundled table's zebra shade.
    #[allow(non_snake_case)]
    fn TABLE_ZEBRA_FILL() -> Color {
        let def = quill_core_model::table_definition();
        for section in &def.sections {
            if let SectionShape::Rows { zebra: Some(z), .. } = &section.shape {
                return ink(z.fill);
            }
        }
        panic!("the bundled table declares zebra banding");
    }
    use quill_core_model::{
        Asset, Block, Color, Document, Metadata, PageOverride, PageSetup, Size, StaticAlign,
        BODY_STYLE, PAGE_TOKEN,
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
                | PlacedBlock::Rect { frame, .. }
                | PlacedBlock::Link { frame, .. } => frame.x_pt,
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
        // The frame they landed in is proved by their x, which is the frame's; their width is their
        // own ink (spec 0069), and `L0` is two characters wide in either column. What must still
        // hold is that the ink stays inside the column it was broken to.
        assert!(
            right
                .iter()
                .all(|f| f.x_pt + f.w_pt <= 216.0 + 216.0 + 0.01),
            "right-column ink stays inside the right frame: {right:?}"
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
                | PlacedBlock::Rect { frame, .. }
                | PlacedBlock::Link { frame, .. } => frame.x_pt,
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
            components: Default::default(),
            requires: Vec::new(),
            next_block_id: 0,
            styles: StyleSheet::default(),
            master_pages: Vec::new(),
            default_master: None,
            pages: Vec::new(),
            sections: Vec::new(),
            footnotes: Vec::new(),
            breaks: Vec::new(),
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
            components: Default::default(),
            requires: Vec::new(),
            fonts_embeddable: false,
            revision: 0,
            next_block_id: 0,
            styles: StyleSheet::default(),
            master_pages: Vec::new(),
            default_master: None,
            pages: Vec::new(),
            sections: Vec::new(),
            footnotes: Vec::new(),
            breaks: Vec::new(),
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
        fn statics(
            &self,
            page_index: usize,
            _metrics: &dyn RunMetrics,
            _ctx: &StaticContext<'_>,
        ) -> Vec<PlacedBlock> {
            vec![PlacedBlock::Text {
                run_formats: Vec::new(),
                run_shifts: Vec::new(),
                weight: 400,
                italic: false,
                run_colors: Vec::new(),
                source: BlockId::UNASSIGNED,
                frame: Rect {
                    x_pt: 0.0,
                    y_pt: 620.0,
                    w_pt: 432.0,
                    h_pt: 12.0,
                },
                lines: vec![Line::single_run(format!("{}", page_index + 1), 0.0, 0.0)],
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
        // The measures come from the template, which is the thing that varies them; what placed
        // content proves is that it was laid into *that* page's frame — its ink starts at the
        // frame's left edge and ends inside its right (spec 0069). Reading the measure back off a
        // placed width would only work while every line happened to fill its column.
        let measures: Vec<f32> = (0..3)
            .map(|i| NarrowingTemplate.frames(i)[0].rect.w_pt)
            .collect();
        assert_eq!(measures, vec![400.0, 350.0, 300.0]);
        for (i, page) in pages.iter().take(3).enumerate() {
            let PlacedBlock::Text { frame, .. } = &page.blocks[0] else {
                panic!("expected text");
            };
            let rect = NarrowingTemplate.frames(i)[0].rect;
            assert_eq!(frame.x_pt, rect.x_pt, "page {i} starts at its frame");
            assert!(
                frame.x_pt + frame.w_pt <= rect.x_pt + rect.w_pt + 0.01,
                "page {i} ink {frame:?} left its {}pt measure",
                rect.w_pt
            );
        }
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
        // The *frame* is the full page — asked of the template, which is where a measure is known.
        // A placed block's width is its ink (spec 0069), and a one-character paragraph has 6 pt of
        // it however wide the page is, so reading the measure back off placed output would be
        // asking the wrong object.
        let frames = DocumentTemplate::new(&doc).frames(0);
        assert_eq!(frames[0].rect.w_pt, doc.page_setup.trim.w_pt);
        match &pages[0].blocks[0] {
            PlacedBlock::Text { frame, .. } => {
                assert_eq!(frame.x_pt, 0.0);
                assert_eq!(frame.y_pt, 0.0);
                assert_eq!(frame.w_pt, MONO.measure_run("x", BODY_FONT_SIZE_PT));
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
        // The margin is a property of the *frame*, so it is asserted on the frame; the placed block
        // then only has to start at the frame's left edge, which is what "inset" means for ink
        // (spec 0069).
        let frames = DocumentTemplate::new(&doc).frames(0);
        assert_eq!(frames[0].rect.x_pt, 36.0);
        assert_eq!(frames[0].rect.w_pt, 432.0 - 72.0);
        match &pages[0].blocks[0] {
            PlacedBlock::Text { frame, .. } => {
                assert_eq!(frame.x_pt, 36.0);
                assert_eq!(frame.y_pt, 36.0);
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
                align: StaticAlign::Left,
                mirror: false,
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
        // The claim is about the *frame* the fallback produced, so it is asked of the template. A
        // placed block reports its ink (spec 0069), and one character is 6 pt wide on a 432 pt page
        // as on any other.
        assert_eq!(DocumentTemplate::new(&doc).frames(0)[0].rect.w_pt, 432.0);
        assert!(matches!(&pages[0].blocks[0], PlacedBlock::Text { .. }));
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
        // One column across the whole page, asked of the template — see the note in
        // `an_unknown_default_master_degrades_to_the_page_setup`.
        let frames = DocumentTemplate::new(&doc).frames(0);
        assert_eq!(frames.len(), 1);
        assert_eq!(frames[0].rect.w_pt, 432.0);
        assert!(matches!(&pages[0].blocks[0], PlacedBlock::Text { .. }));
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
                align: StaticAlign::Left,
                mirror: false,
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
        for name in ["digest", "reference"] {
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
        let t = quill_core_model::Template::by_name("reference").expect("bundled");
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

    #[test]
    fn the_rulebook_template_no_longer_ends_its_columns_ragged() {
        // The defect spec 0044 exists to fix, in the template it was found in, measured rather than
        // asserted by construction. Before fragmentation a column ended wherever the next paragraph
        // happened not to fit; on the rulebook's 162 pt measure that is most paragraphs.
        //
        // Measured on this fixture with splitting suppressed and then enabled:
        //
        //             pages   total unset   worst column
        //   before      30       6333.5 pt     327.5 pt   (26 lines of white at a column foot)
        //   after       24        166.0 pt      15.0 pt   (1.2 lines)
        //
        // Paragraph lengths must *vary* for this to measure anything: a fixture of equal-length
        // paragraphs tiles a column identically with and without splitting, reports no change, and
        // would pass while proving nothing. The first attempt at this test did exactly that.
        //
        // The residual 15 pt is the widow rule's floor and not a miss: a legal fragment needs two
        // lines (25 pt), so a gap smaller than that is one the engine is right to leave.
        let t = quill_core_model::Template::by_name("reference").expect("bundled");
        let mut doc = Document::from_template(t);
        doc.content = (0..120)
            .map(|i| {
                let words = 20 + (i * 37) % 90;
                let body: String = (0..words)
                    .map(|w| format!("word{w}"))
                    .collect::<Vec<_>>()
                    .join(" ");
                Block::body(format!("Paragraph {i}. {body}."), Color::Gray { v: 0.0 })
            })
            .collect();
        doc.assign_missing_block_ids().expect("ids");
        let pages = lay_out(&doc, &MONO, &NoHyphenator);
        let template = DocumentTemplate::new(&doc);
        let leading = doc.styles.resolve(&doc.content[0]).leading_pt;

        let mut slacks = Vec::new();
        for page in &pages {
            // The last page is where the text ran out; its short columns are not a defect.
            if page.index + 1 == pages.len() {
                continue;
            }
            for frame in template.frames(page.index) {
                if let Some(bottom) = column_bottom(page, &frame) {
                    slacks.push(frame.rect.y_pt + frame.rect.h_pt - bottom);
                }
            }
        }
        assert!(
            slacks.len() >= 20,
            "need a document of full columns, got {}",
            slacks.len()
        );

        let worst = slacks.iter().cloned().fold(0.0f32, f32::max);
        assert!(
            worst < 2.0 * leading,
            "a full column left {worst:.1} pt unset, more than the two-line minimum fragment"
        );
        assert!(
            pages.len() <= 25,
            "the same copy took 30 pages before fragmentation; got {}",
            pages.len()
        );
    }

    /// The y at which a column's content ends, or `None` if nothing was placed in it.
    fn column_bottom(page: &LaidOutPage, frame: &Frame) -> Option<f32> {
        page.blocks
            .iter()
            .filter_map(|b| match b {
                PlacedBlock::Text { frame: f, .. } | PlacedBlock::Image { frame: f, .. } => {
                    same_width(f.x_pt, frame.rect.x_pt).then_some(f.y_pt + f.h_pt)
                }
                // Neither draws content: a decoration is sized by what it decorates, and a link
                // candidate is not ink at all (spec 0052).
                PlacedBlock::Rect { .. } | PlacedBlock::Link { .. } => None,
            })
            .fold(None, |acc: Option<f32>, y| {
                Some(acc.map_or(y, |a: f32| a.max(y)))
            })
    }

    // ----- spec 0048: indents reach placed geometry ---------------------------------------------

    #[test]
    fn a_hanging_indent_reaches_the_placed_lines() {
        // The style's indent must survive measurement into `PlacedBlock`, because that is all the
        // two painters ever see. Asserted on placed geometry rather than on the breaker's output,
        // which is where a correctly-computed indent would otherwise be dropped.
        let mut styles = StyleSheet::default();
        styles.paragraph.insert(
            BODY_STYLE.to_string(),
            ParagraphStyle {
                weight: Default::default(),
                italic: false,
                list: None,
                font_size_pt: BODY_FONT_SIZE_PT,
                leading_pt: BODY_LINE_HEIGHT_PT,
                align: TextAlign::Left,
                space_before_pt: 0.0,
                space_after_pt: 0.0,
                indent: quill_core_model::Indent::hanging(12.0),
            },
        );
        let mut content = vec![Block::body(n_line_paragraph(6), Color::Gray { v: 0.0 })];
        content[0].set_id(BlockId(1));
        let thread = split_columns(1, 400.0);
        let pages = lay_out_in_thread(&content, &[], &styles, &thread, &MONO, &NoHyphenator);

        let lines = pages[0]
            .blocks
            .iter()
            .find_map(|b| match b {
                PlacedBlock::Text { lines, .. } => Some(lines),
                _ => None,
            })
            .expect("the paragraph must be placed");
        assert!(lines.len() >= 3, "need several lines, got {}", lines.len());
        assert_eq!(lines[0].indent_pt, 0.0, "the first line hangs flush");
        assert!(
            lines[1..].iter().all(|l| l.indent_pt == 12.0),
            "every line after the first carries the hanging indent"
        );
    }

    // ----- spec 0069: a placed frame is the ink the part draws -----------------------------------

    /// A single frame of an explicit measure at the page origin. `split_columns` fixes its width at
    /// 120 pt, and these tests are about the relationship between a measure and the ink inside it,
    /// so the measure has to be theirs to state.
    fn one_column(w_pt: f32, h_pt: f32) -> Thread {
        Thread {
            frames: vec![Frame {
                rect: Rect {
                    x_pt: 0.0,
                    y_pt: 0.0,
                    w_pt,
                    h_pt,
                },
            }],
        }
    }

    #[test]
    fn a_multi_line_paragraph_reports_an_ink_bounding_box_not_an_advance() {
        // The case a naive reading of the rule gets wrong, and the reason the rule has to be
        // written down. `indent_pt` is added at *draw* time and differs per line under a hanging
        // indent (spec 0048), so no single line's box is the paragraph's: the frame is
        // `min(indent)` wide of the measure's left edge, and reaches `max(indent + advance)`.
        // A tab segment — one line, zero indent — is the degenerate case this generalizes.
        let indent_pt = 12.0;
        let mut styles = StyleSheet::default();
        styles.paragraph.insert(
            BODY_STYLE.to_string(),
            ParagraphStyle {
                align: TextAlign::Left,
                indent: quill_core_model::Indent::hanging(indent_pt),
                space_before_pt: 0.0,
                space_after_pt: 0.0,
                ..Default::default()
            },
        );
        let mut content = vec![Block::body(n_line_paragraph(6), Color::Gray { v: 0.0 })];
        content[0].set_id(BlockId(1));
        // 120 pt, which is what `n_line_paragraph` is calibrated against.
        let measure = 120.0;
        let thread = one_column(measure, 400.0);
        let pages = lay_out_in_thread(&content, &[], &styles, &thread, &MONO, &NoHyphenator);

        let PlacedBlock::Text { frame, lines, .. } = &pages[0].blocks[0] else {
            panic!("expected text");
        };
        assert!(lines.len() >= 3, "need several lines");
        // The first line hangs flush and the rest are indented, so the paragraph's ink starts at
        // the measure's left edge — at line 0's origin, not at any later line's.
        assert_eq!(lines[0].indent_pt, 0.0);
        assert!(lines[1..].iter().all(|l| l.indent_pt == indent_pt));
        assert_eq!(frame.x_pt, 0.0, "x is min(indent), which line 0 supplies");

        // And the width is the widest *reach*, which under a hanging indent may well be a line
        // whose own advance is not the longest.
        let advance = |l: &Line| MONO.measure_run(&l.text, BODY_FONT_SIZE_PT);
        let want: f32 = lines
            .iter()
            .map(|l| l.indent_pt + advance(l))
            .fold(0.0, f32::max);
        assert!(
            (frame.w_pt - want).abs() < 0.01,
            "w {} vs {want}",
            frame.w_pt
        );
        assert!(frame.w_pt <= measure + 0.01, "and never past the measure");

        // The box really is a bound over the lines rather than any one of them: at least one line
        // is strictly narrower than the block reports, or this fixture proves nothing.
        assert!(
            lines.iter().any(|l| l.indent_pt + advance(l) < want - 0.01),
            "the fixture must have a short line, or the bounding box is untested"
        );
    }

    #[test]
    fn a_right_aligned_part_reports_a_box_over_its_ink_not_its_slot() {
        // `PlacedBlock` carries no alignment field, so a right-aligned segment in a wide slot is
        // indistinguishable from a left-aligned one downstream. Under slot semantics its reported
        // box is nowhere near its glyphs — which is the first of the five reasons the decision log
        // gives. Asserted through the tab mechanism (spec 0067), where all four alignments are
        // resolved into an x.
        let measure = 400.0;
        let stop_pt = 300.0;
        let mut styles = StyleSheet::default();
        styles.paragraph.insert(
            BODY_STYLE.to_string(),
            ParagraphStyle {
                align: TextAlign::Left,
                space_before_pt: 0.0,
                space_after_pt: 0.0,
                ..Default::default()
            },
        );
        styles.tabs.insert(
            BODY_STYLE.to_string(),
            vec![quill_core_model::TabStop {
                position_pt: stop_pt,
                align: quill_core_model::TabAlign::Right,
                leader: None,
            }],
        );
        let mut content = vec![Block::body("Key\tValue", Color::Gray { v: 0.0 })];
        content[0].set_id(BlockId(1));
        let thread = one_column(measure, 400.0);
        let pages = lay_out_in_thread(&content, &[], &styles, &thread, &MONO, &NoHyphenator);

        let boxes: Vec<(f32, f32, String)> = pages[0]
            .blocks
            .iter()
            .filter_map(|b| match b {
                PlacedBlock::Text { frame, lines, .. } => {
                    Some((frame.x_pt, frame.w_pt, lines[0].text.clone()))
                }
                _ => None,
            })
            .collect();
        assert_eq!(boxes.len(), 2, "a key and a right-aligned value: {boxes:?}");
        let (x, w, ref text) = boxes[1];
        let advance = MONO.measure_run(text, BODY_FONT_SIZE_PT);
        // Its right edge is the stop and its left edge is one advance back — the ink, not the slot
        // from the previous segment to the stop, and not the whole measure.
        assert!((x + w - stop_pt).abs() < 0.01, "right edge {}", x + w);
        assert!((w - advance).abs() < 0.01, "width {w} vs advance {advance}");
        assert!(x > boxes[0].0 + boxes[0].1, "and it sits past the key");
    }

    #[test]
    fn a_list_marker_reports_its_glyphs_not_the_gutter_they_sit_in() {
        // The gutter the indent opens is the slot; a bullet is a few points of ink inside it
        // (spec 0069). Reporting the gutter would put a marker's box under the safe-area check for
        // text that is not there.
        let gutter_pt = 24.0;
        let mut styles = StyleSheet::default();
        styles.paragraph.insert(
            BODY_STYLE.to_string(),
            ParagraphStyle {
                align: TextAlign::Left,
                list: Some(quill_core_model::ListSpec {
                    marker: ListMarker::Bullet { glyph: '\u{2022}' },
                    start: 1,
                    level: 0,
                }),
                indent: quill_core_model::Indent {
                    first_pt: gutter_pt,
                    rest_pt: gutter_pt,
                },
                space_before_pt: 0.0,
                space_after_pt: 0.0,
                ..Default::default()
            },
        );
        let mut content = vec![Block::body("An item", Color::Gray { v: 0.0 })];
        content[0].set_id(BlockId(1));
        let thread = one_column(400.0, 400.0);
        let pages = lay_out_in_thread(&content, &[], &styles, &thread, &MONO, &NoHyphenator);

        let PlacedBlock::Text {
            frame: marker,
            lines: marker_lines,
            ..
        } = &pages[0].blocks[0]
        else {
            panic!("expected the marker first");
        };
        assert_eq!(marker.x_pt, 0.0, "the marker sits at the frame's left edge");
        let advance = MONO.measure_run(&marker_lines[0].text, BODY_FONT_SIZE_PT);
        assert!((marker.w_pt - advance).abs() < 0.01, "w {}", marker.w_pt);
        assert!(marker.w_pt < gutter_pt, "and is narrower than its gutter");

        // The item's own text starts past the gutter, which is where it is drawn.
        let PlacedBlock::Text { frame: item, .. } = &pages[0].blocks[1] else {
            panic!("expected the item text");
        };
        assert_eq!(item.x_pt, gutter_pt);
    }

    #[test]
    fn a_stat_blocks_attribute_value_hangs_under_itself() {
        // The built-in treatment, which is where the defect lived: a wrapped attribute lines up
        // under its value rather than under its key. `statblock-attr` carries the hanging indent in
        // `StyleSheet::default()`, so dropping a stat block in gets it with no authoring.
        let panel = quill_core_model::Panel {
            name: "Barrow Wight".into(),
            overview: vec![],
            attributes: vec![(
                "Armour Class".into(),
                "15 (leather armour, shield, and a helm)".into(),
            )],
            details: vec![],
            actions: vec![],
            reactions: vec![],
        };
        let mut content = vec![Block::Panel {
            id: BlockId::UNASSIGNED,
            panel,
            color: Color::Gray { v: 0.0 },
        }];
        content[0].set_id(BlockId(1));
        let thread = split_columns(1, 400.0);
        let pages = lay_out_in_thread(
            &content,
            &[],
            &StyleSheet::default(),
            &thread,
            &MONO,
            &NoHyphenator,
        );

        let attr = pages[0]
            .blocks
            .iter()
            .filter_map(|b| match b {
                PlacedBlock::Text { lines, .. } if lines[0].text.starts_with("Armour") => {
                    Some(lines)
                }
                _ => None,
            })
            .next()
            .expect("the attribute run must be placed");
        assert!(
            attr.len() >= 2,
            "the fixture must wrap, got {} line(s)",
            attr.len()
        );
        assert_eq!(attr[0].indent_pt, 0.0);
        assert!(
            attr[1..].iter().all(|l| l.indent_pt > 0.0),
            "wrapped attribute lines must hang"
        );
        // And the key is one unbreakable unit, so it can never be the whole of a line.
        assert!(
            attr.iter().all(|l| l.text.trim() != "Armour"),
            "the key must not be split: {:?}",
            attr.iter().map(|l| &l.text).collect::<Vec<_>>()
        );
    }

    // --- Stat blocks (spec 0038) ----------------------------------------------------------------

    fn goblin() -> quill_core_model::Panel {
        quill_core_model::Panel {
            name: "Goblin".into(),
            overview: vec!["Small humanoid, chaotic".into()],
            attributes: vec![("AC".into(), "15".into()), ("HP".into(), "7".into())],
            details: vec![],
            actions: vec!["Scimitar. +4 to hit, 5 damage.".into()],
            reactions: vec![],
        }
    }

    /// Measure one block against `width` exactly as the flow does, so a test can assert against the
    /// **measure a producer broke to** rather than trying to recover it from placed output.
    ///
    /// Spec 0069 is why this exists. A placed frame is the ink a part draws, so a containment
    /// assertion read off placed geometry weakens to "the text that happened to be there fits" —
    /// which cannot catch a cell broken to too wide a measure, the defect those assertions were
    /// written for. The measure is known here, at the producer, and is asserted here.
    fn measure_at(block: &Block, width: f32) -> Measured {
        let assets: AssetIndex<'_> = AssetIndex::new();
        let styles = StyleSheet::default();
        let components = quill_core_model::builtin_components();
        let markers = BTreeMap::new();
        let ctx = BlockContext {
            assets: &assets,
            styles: &styles,
            headings: &[],
            index: &[],
            components: &components,
            markers: &markers,
            resolved: &BTreeMap::new(),
        };
        measure_block(block, width, &ctx, &MONO, &NoHyphenator)
            .expect("the fixtures all measure")
            .0
    }

    /// Every `(measure_left, measure_width, ink_left, ink_width)` a panel measurement produced,
    /// relative to the panel's own origin.
    fn panel_parts(measured: &Measured) -> Vec<(f32, f32, f32, f32)> {
        let Measured::Panel { parts, .. } = measured else {
            panic!("expected a panel measurement");
        };
        parts
            .iter()
            .map(|p| (p.dx_pt, p.w_pt, p.dx_pt + p.ink_dx_pt, p.ink_w_pt))
            .collect()
    }

    fn stat_doc() -> Document {
        let mut doc = doc_with_blocks(vec![Block::Panel {
            id: BlockId::UNASSIGNED,
            panel: goblin(),
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
        // The panel spans the frame; every run sits `PANEL_PADDING_PT` inside it, and the last
        // run's baseline block ends at least a padding above the panel's bottom edge.
        let doc = stat_doc();
        let pages = lay_out(&doc, &MONO, &NoHyphenator);
        let PlacedBlock::Rect { frame: panel, .. } = pages[0].blocks[0] else {
            panic!("expected a panel");
        };

        // The inset is a property of the **measure**, so it is asserted on the measurement, where
        // the producer knows it: every section is broken to the panel's inner width, at the
        // padding. Placed output can no longer answer this — a run's frame is its ink (spec 0069),
        // and a two-word section's ink is nowhere near the inner width even when the measure is
        // right.
        let measured = measure_at(&doc.content[0], panel.w_pt);
        for (mx, mw, ix, iw) in panel_parts(&measured) {
            assert!((mx - PANEL_PADDING_PT).abs() < 0.01, "measure left {mx}");
            assert!(
                (mw - (panel.w_pt - PANEL_PADDING_PT * 2.0)).abs() < 0.01,
                "measure width {mw}"
            );
            // And the ink each section drew stays inside the measure it was given.
            assert!(
                ix >= mx - 0.01 && ix + iw <= mx + mw + 0.01,
                "ink {ix}+{iw}"
            );
        }

        let runs: Vec<&PlacedBlock> = pages[0]
            .blocks
            .iter()
            .filter(|b| matches!(b, PlacedBlock::Text { .. }))
            .collect();
        for r in &runs {
            let PlacedBlock::Text { frame, .. } = r else {
                panic!("expected text")
            };
            assert!((frame.x_pt - (panel.x_pt + PANEL_PADDING_PT)).abs() < 0.01);
            assert!(frame.x_pt + frame.w_pt <= panel.x_pt + panel.w_pt - PANEL_PADDING_PT + 0.01);
            assert!(frame.y_pt >= panel.y_pt + PANEL_PADDING_PT - 0.01);
        }
        let last = match runs.last().unwrap() {
            PlacedBlock::Text { frame, .. } => frame.y_pt + frame.h_pt,
            _ => unreachable!(),
        };
        assert!(
            last <= panel.y_pt + panel.h_pt - PANEL_PADDING_PT + 0.01,
            "the bottom padding must be inside the panel"
        );
    }

    #[test]
    fn a_stat_block_reads_in_the_order_the_component_documents() {
        // `Panel`'s own doc comment states the compact layout as
        // Overview / Attributes / Details / Actions / Reactions, after the name. The first draft
        // put the attributes before the overview, which puts a creature's armour class above its
        // type — visibly not a stat block, and invisible to every assertion until it was rendered.
        let mut doc = stat_doc();
        doc.content[0] = Block::Panel {
            id: doc.content[0].id(),
            panel: quill_core_model::Panel {
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
            assert!((frame.x_pt - (panel.x_pt + PANEL_PADDING_PT)).abs() < 0.01);
            assert!((frame.w_pt - (panel.w_pt - PANEL_PADDING_PT * 2.0)).abs() < 0.01);
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
    fn a_stat_block_that_fits_the_next_frame_still_moves_whole_to_it() {
        // Keep-together, preserved. Spec 0046 gave a stat block section boundaries it *could* be cut
        // at; this asserts it is still not cut when moving it whole works, which is what spec 0038
        // promised. The preference order is the behaviour, and it is asserted before the split is.
        let mut doc = stat_doc();
        let stat = doc.content.remove(0);
        doc.content = many_lines(52);
        doc.content.push(stat);
        doc.assign_missing_block_ids().expect("ids");

        let pages = lay_out(&doc, &MONO, &NoHyphenator);
        assert!(pages.len() >= 2, "the fixture must actually paginate");

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

    /// A stat block with `n` action lines — long enough to fit no frame at all.
    fn big_goblin(n: usize) -> quill_core_model::Panel {
        quill_core_model::Panel {
            actions: (0..n)
                .map(|i| format!("Action {i}. Does something."))
                .collect(),
            details: (0..n)
                .map(|i| format!("Detail {i} about the creature."))
                .collect(),
            reactions: vec!["Parry. Adds 2 to AC.".into()],
            ..goblin()
        }
    }

    /// Every line placed for `source`, in page then y order.
    fn stat_runs(pages: &[LaidOutPage], source: BlockId) -> Vec<String> {
        let mut out = Vec::new();
        for page in pages {
            let mut placed: Vec<(f32, Vec<String>)> = page
                .blocks
                .iter()
                .filter_map(|b| match b {
                    PlacedBlock::Text {
                        source: s,
                        frame,
                        lines,
                        ..
                    } if *s == source => Some((
                        frame.y_pt,
                        lines.iter().map(|l| l.text.clone()).collect::<Vec<_>>(),
                    )),
                    _ => None,
                })
                .collect();
            placed.sort_by(|a, b| a.0.partial_cmp(&b.0).unwrap());
            out.extend(placed.into_iter().flat_map(|(_, l)| l));
        }
        out
    }

    #[test]
    fn a_stat_block_too_tall_for_any_frame_splits_at_a_section() {
        // Spec 0038's original promise, now buildable. The block fits nowhere, so keep-together has
        // nothing to prefer and the fallback applies.
        let mut doc = stat_doc();
        doc.content = vec![Block::Panel {
            id: BlockId::UNASSIGNED,
            panel: big_goblin(30),
            color: Color::Gray { v: 0.0 },
        }];
        doc.assign_missing_block_ids().expect("ids");
        let id_pages = lay_out(&doc, &MONO, &NoHyphenator);
        assert!(
            id_pages.len() >= 2,
            "a stat block taller than the page must paginate, got {}",
            id_pages.len()
        );

        let id = doc.content[0].id();
        let placed = stat_runs(&id_pages, id);

        // Conservation: every run exactly once, in order.
        let mut expected = vec!["Goblin".to_string(), "Small humanoid, chaotic".to_string()];
        expected.push("AC: 15".into());
        expected.push("HP: 7".into());
        let big = big_goblin(30);
        expected.extend(big.details.clone());
        expected.extend(big.actions.clone());
        expected.extend(big.reactions.clone());
        assert_eq!(
            placed, expected,
            "every run exactly once, in document order"
        );
    }

    #[test]
    fn a_cut_stat_block_never_separates_an_attributes_list() {
        // The rule that makes a section the unit: a cut falls *between* sections and never inside
        // one, so `AC` and `HP` cannot end up on different pages. Attributes are one section by
        // construction, which is what makes this true rather than lucky.
        let mut doc = stat_doc();
        doc.content = vec![Block::Panel {
            id: BlockId::UNASSIGNED,
            panel: quill_core_model::Panel {
                attributes: (0..6)
                    .map(|i| (format!("Attr{i}"), format!("{i}")))
                    .collect(),
                ..big_goblin(30)
            },
            color: Color::Gray { v: 0.0 },
        }];
        doc.assign_missing_block_ids().expect("ids");
        let pages = lay_out(&doc, &MONO, &NoHyphenator);
        let id = doc.content[0].id();

        let mut attr_pages = std::collections::BTreeSet::new();
        for page in &pages {
            for b in &page.blocks {
                if let PlacedBlock::Text { source, lines, .. } = b {
                    if *source == id && lines[0].text.starts_with("Attr") {
                        attr_pages.insert(page.index);
                    }
                }
            }
        }
        assert_eq!(
            attr_pages.len(),
            1,
            "the attributes section must not be cut across pages, found on {attr_pages:?}"
        );
    }

    #[test]
    fn a_cut_stat_blocks_panel_closes_and_reopens() {
        // One rect per fragment, each bounded by its own fragment — not one rect spanning a page
        // break, which is what a naive implementation draws and what no text assertion notices.
        let mut doc = stat_doc();
        doc.content = vec![Block::Panel {
            id: BlockId::UNASSIGNED,
            panel: big_goblin(30),
            color: Color::Gray { v: 0.0 },
        }];
        doc.assign_missing_block_ids().expect("ids");
        let pages = lay_out(&doc, &MONO, &NoHyphenator);
        let bottom = Frame::full_page(&doc.page_setup).rect.h_pt;

        let mut panels = 0;
        for page in &pages {
            for b in &page.blocks {
                if let PlacedBlock::Rect { frame, fill, .. } = b {
                    if *fill == Some(PANEL_FILL()) {
                        panels += 1;
                        assert!(
                            frame.y_pt + frame.h_pt <= bottom + 0.01,
                            "page {} panel runs {:.1} past the frame bottom",
                            page.index,
                            frame.y_pt + frame.h_pt
                        );
                    }
                }
            }
        }
        assert!(
            panels >= 2,
            "each fragment must carry its own panel, got {panels}"
        );
    }

    /// Lines per group in the narrow-column fixtures below.
    ///
    /// **Eight, and it was twenty-two before spec 0060.** The number is a fixture detail; the
    /// reason is not. `Detail 0 about the creature.` measures 151.2 pt against the panel's 150 pt
    /// inner measure in a `rulebook` column, and until 0060 the breaker let a ragged line run
    /// 1.2 pt over on the strength of shrink that ragged setting never applies. Every such run was
    /// one line; each is two now, so a group is a little over twice as tall.
    ///
    /// That moved where spec 0046's *uncuttable* case began — a section taller than the column
    /// could not be cut, so the panel was placed whole and ran off the page. Measured on both
    /// builds with this exact fixture: **24 fitted and 26 overflowed before spec 0060; 8 fits and
    /// 10 overflowed after it.**
    ///
    /// **Spec 0075 removed the overflow, and the threshold with it**: at 10 the panel is now cut
    /// and fits. The constant stays at 8 because the fixture on this side of it is still the one
    /// that fits *whole*, and the two together are what make the narrow-column assertions cover
    /// both a placed block and a cut one.
    /// [`a_section_taller_than_its_column_is_cut_rather_than_overflowing`] is the other side, now
    /// asserting that it fits.
    const NARROW_COLUMN_SECTIONS: usize = 8;

    #[test]
    fn no_stat_block_fragment_overruns_a_narrow_column() {
        // Asserted over the real two-column template rather than a synthetic frame, because the
        // narrow 162 pt columns and their 378 pt height are what make a cut hard to find: a
        // fragment whose smallest legal cut is larger than the column falls back to being placed
        // whole, and runs off the bottom of the page.
        let t = quill_core_model::Template::by_name("reference").expect("bundled");
        let mut doc = Document::from_template(t);
        doc.content = vec![
            Block::body(
                "Some introductory prose, so the frame is not empty when the panel arrives and \
                 the engine has a real decision to make about where it goes.",
                Color::Gray { v: 0.0 },
            ),
            Block::Panel {
                id: BlockId::UNASSIGNED,
                panel: big_goblin(NARROW_COLUMN_SECTIONS),
                color: Color::Gray { v: 0.0 },
            },
        ];
        doc.assign_missing_block_ids().expect("ids");
        let pages = lay_out(&doc, &MONO, &NoHyphenator);
        let template = DocumentTemplate::new(&doc);

        for page in &pages {
            let bottom = template
                .frames(page.index)
                .iter()
                .map(|f| f.rect.y_pt + f.rect.h_pt)
                .fold(0.0_f32, f32::max);
            for b in &page.blocks {
                let (y, h) = match b {
                    PlacedBlock::Text { frame, .. }
                    | PlacedBlock::Rect { frame, .. }
                    | PlacedBlock::Image { frame, .. }
                    | PlacedBlock::Link { frame, .. } => (frame.y_pt, frame.h_pt),
                };
                assert!(
                    y + h <= bottom + 0.01,
                    "page {} content runs to {:.1}, past the column bottom {bottom:.1}",
                    page.index,
                    y + h
                );
            }
        }
    }

    /// The other side of [`NARROW_COLUMN_SECTIONS`]: a section taller than the column it must fit.
    ///
    /// **This test is spec 0046's inverted, exactly as its own doc comment promised it would be.**
    /// It used to assert that the panel overflowed — a stat block was cut between sections and
    /// nowhere else, so a single over-tall section had no legal cut, and the block was placed whole
    /// and ran off the bottom of the page. Spec 0075 shipped the fix, so the overflow assertion
    /// becomes an assertion that there is none. The test is rewritten rather than deleted, because
    /// what it pins is a *threshold*, and a threshold with nothing on either side of it is not
    /// pinned at all.
    ///
    /// Two separate changes had to land for this fixture to come out right, and the second was not
    /// the one the known issue named:
    ///
    /// 1. A composite may now be cut **inside** a section, at an element boundary, when no section
    ///    boundary fits (see [`PanelSplit::preferred`]).
    /// 2. Keep-together now *moves* the block it declined to cut. This fixture's remainder is
    ///    440 pt: too tall for page 0's 378 pt columns, comfortable in page 1's 540 pt one — so
    ///    keep-together suppressed the cut, and the empty-frame guard then placed it where it stood
    ///    and let it overflow. No amount of finer cutting would have reached that.
    ///
    /// What must hold either way, and is asserted unconditionally: **no content is lost.**
    #[test]
    fn a_section_taller_than_its_column_is_cut_rather_than_overflowing() {
        let t = quill_core_model::Template::by_name("reference").expect("bundled");
        let build = |n: usize| {
            let mut doc = Document::from_template(t);
            doc.content = vec![
                Block::body(
                    "Some introductory prose, so the frame is not empty when the panel arrives \
                     and the engine has a real decision to make about where it goes.",
                    Color::Gray { v: 0.0 },
                ),
                Block::Panel {
                    id: BlockId::UNASSIGNED,
                    panel: big_goblin(n),
                    color: Color::Gray { v: 0.0 },
                },
            ];
            doc.assign_missing_block_ids().expect("ids");
            doc
        };

        let doc = build(NARROW_COLUMN_SECTIONS + 2);
        let pages = lay_out(&doc, &MONO, &NoHyphenator);
        let template = DocumentTemplate::new(&doc);
        let mut worst: f32 = 0.0;
        for page in &pages {
            let bottom = template
                .frames(page.index)
                .iter()
                .map(|f| f.rect.y_pt + f.rect.h_pt)
                .fold(0.0_f32, f32::max);
            for b in &page.blocks {
                let (y, h) = match b {
                    PlacedBlock::Text { frame, .. }
                    | PlacedBlock::Rect { frame, .. }
                    | PlacedBlock::Image { frame, .. }
                    | PlacedBlock::Link { frame, .. } => (frame.y_pt, frame.h_pt),
                };
                worst = worst.max(y + h - bottom);
            }
        }
        assert!(
            worst <= 0.01,
            "a section taller than its column must no longer overflow it; worst overrun {worst:.1} pt"
        );

        // Content conservation, which holds on both sides of the threshold. Compared against the
        // *same* stat block laid into a frame tall enough to need no cut: same lines, same order.
        // Comparing laid-out lines rather than authored ones is what makes this robust to the
        // wrapping spec 0060 introduced — the authored `Detail 0 about the creature.` is now two
        // lines, and an assertion phrased over authored strings would be testing the wrap, not the
        // conservation.
        let mut tall = doc.clone();
        tall.page_setup.trim.h_pt = 4000.0;
        //
        // Compared as a *multiset*: `stat_runs` orders by (page, y), and two columns of one page
        // interleave in y, so run order across columns is not a property worth asserting here.
        let mut reference = stat_runs(&lay_out(&tall, &MONO, &NoHyphenator), tall.content[1].id());
        let mut placed = stat_runs(&pages, doc.content[1].id());
        reference.sort();
        placed.sort();
        assert!(!reference.is_empty(), "the reference layout placed nothing");
        assert_eq!(
            placed, reference,
            "a cut panel may be placed badly, never dropped"
        );
    }

    #[test]
    fn a_three_section_stat_block_still_splits() {
        // Why the section minimum is one and not two (spec 0046). With a two-section minimum a
        // block of three sections could not be cut at all — three is below the four that two-a-side
        // needs — so a creature with a name, an overview and one long actions list would be placed
        // whole and overflow. A section is a unit the reader recognises, so a fragment of one is
        // not a widow the way a single line is.
        let mut doc = stat_doc();
        doc.content = vec![Block::Panel {
            id: BlockId::UNASSIGNED,
            panel: quill_core_model::Panel {
                name: "Barrow Wight".into(),
                overview: vec!["Medium undead, lawful evil".into()],
                attributes: vec![],
                details: vec![],
                actions: (0..70).map(|i| format!("Attack {i}. +5 to hit.")).collect(),
                reactions: vec![],
            },
            color: Color::Gray { v: 0.0 },
        }];
        doc.assign_missing_block_ids().expect("ids");
        let pages = lay_out(&doc, &MONO, &NoHyphenator);
        let panels: usize = pages
            .iter()
            .flat_map(|p| p.blocks.iter())
            .filter(|b| matches!(b, PlacedBlock::Rect { fill, .. } if *fill == Some(PANEL_FILL())))
            .count();
        assert!(
            panels >= 2,
            "three sections must still admit a cut, got {panels} panel(s)"
        );
    }

    #[test]
    fn a_stat_block_of_one_oversized_run_is_placed_whole() {
        // The uncuttable fallback, at its real boundary — **and this is spec 0075's named
        // residual.** After 0075 a composite cuts between its *elements*, not merely between its
        // sections, so the floor is one element: a single authored run that wraps taller than a
        // frame still has no legal cut, and is placed whole and allowed to overflow rather than
        // emit an empty fragment and loop forever.
        //
        // Fixing this one needs a cut *inside* a `PanelPart`'s line list — splitting the part,
        // re-deriving its ink box, and threading run metrics into `Measured::split_at`, which takes
        // none today. That is a different change from this one and is recorded as a follow-up
        // rather than half-built here.
        //
        // Two sections would *not* be this case — spec 0046 allows a one-section fragment, so a
        // name and an overview do split. Requiring two sections per fragment instead is what made
        // the first version of that increment render a panel off the bottom of the page.
        let mut doc = stat_doc();
        doc.content = vec![Block::Panel {
            id: BlockId::UNASSIGNED,
            panel: quill_core_model::Panel {
                name: (0..400)
                    .map(|i| format!("word{i}"))
                    .collect::<Vec<_>>()
                    .join(" "),
                overview: vec![],
                attributes: vec![],
                details: vec![],
                actions: vec![],
                reactions: vec![],
            },
            color: Color::Gray { v: 0.0 },
        }];
        doc.assign_missing_block_ids().expect("ids");
        let pages = lay_out(&doc, &MONO, &NoHyphenator);
        let panels: usize = pages
            .iter()
            .flat_map(|p| p.blocks.iter())
            .filter(|b| matches!(b, PlacedBlock::Rect { fill, .. } if *fill == Some(PANEL_FILL())))
            .count();
        assert_eq!(panels, 1, "one run ⇒ no legal cut ⇒ placed whole");
    }

    #[test]
    fn a_cut_falls_inside_a_section_when_no_section_boundary_fits() {
        // Spec 0075's composite half, and the assertion that separates it from spec 0046's
        // behaviour rather than restating it. A creature with a name and one enormous actions list
        // has exactly one section boundary — after the name — and a fragment of just a name is
        // nowhere near a column. Everything after it has to be cut *inside* the actions section or
        // not at all, which is precisely what 0046 could not do.
        //
        // Asserted by where the seam falls: consecutive `Attack n.` runs landing on different
        // pages is a cut between two elements of one section, and no arrangement of section
        // boundaries can produce it.
        let mut doc = stat_doc();
        doc.content = vec![Block::Panel {
            id: BlockId::UNASSIGNED,
            panel: quill_core_model::Panel {
                name: "Barrow Wight".into(),
                overview: vec![],
                attributes: vec![],
                details: vec![],
                actions: (0..120)
                    .map(|i| format!("Attack {i}. +5 to hit."))
                    .collect(),
                reactions: vec![],
            },
            color: Color::Gray { v: 0.0 },
        }];
        doc.assign_missing_block_ids().expect("ids");
        let id = doc.content[0].id();
        let pages = lay_out(&doc, &MONO, &NoHyphenator);
        let template = DocumentTemplate::new(&doc);

        // Nothing overruns, on any page. This is the defect itself.
        for page in &pages {
            let bottom = template
                .frames(page.index)
                .iter()
                .map(|f| f.rect.y_pt + f.rect.h_pt)
                .fold(0.0_f32, f32::max);
            for b in &page.blocks {
                let (y, h) = match b {
                    PlacedBlock::Text { frame, .. }
                    | PlacedBlock::Rect { frame, .. }
                    | PlacedBlock::Image { frame, .. }
                    | PlacedBlock::Link { frame, .. } => (frame.y_pt, frame.h_pt),
                };
                assert!(
                    y + h <= bottom + 0.01,
                    "page {} runs to {:.1}, past {bottom:.1}",
                    page.index,
                    y + h
                );
            }
        }

        // The seam is inside the actions section: some `Attack n` is the last run on its page and
        // `Attack n+1` opens the next.
        let mut per_page: Vec<Vec<String>> = Vec::new();
        for page in &pages {
            let mut runs: Vec<(f32, String)> = page
                .blocks
                .iter()
                .filter_map(|b| match b {
                    PlacedBlock::Text {
                        source,
                        frame,
                        lines,
                        ..
                    } if *source == id => Some((frame.y_pt, lines[0].text.clone())),
                    _ => None,
                })
                .collect();
            runs.sort_by(|a, b| a.0.partial_cmp(&b.0).unwrap());
            per_page.push(runs.into_iter().map(|(_, t)| t).collect());
        }
        let seams: usize = per_page
            .windows(2)
            .filter(|w| {
                let Some(last) = w[0].last() else {
                    return false;
                };
                let Some(first) = w[1].first() else {
                    return false;
                };
                last.starts_with("Attack") && first.starts_with("Attack")
            })
            .count();
        assert!(
            seams >= 1,
            "a cut must fall between two actions when no section boundary can carry it: {per_page:?}"
        );

        // And conservation, which is the only failure that silently produces a wrong book.
        let placed: Vec<String> = per_page.concat();
        let expected: Vec<String> = std::iter::once("Barrow Wight".to_string())
            .chain((0..120).map(|i| format!("Attack {i}. +5 to hit.")))
            .collect();
        assert_eq!(
            placed, expected,
            "every run exactly once, in document order"
        );
    }

    #[test]
    fn a_stat_block_wider_than_its_frame_still_wraps_inside_the_padding() {
        // The padding must come off the measure, not just off the position — otherwise long prose
        // is broken to the full frame width and then drawn inset, so it overruns the panel.
        let mut doc = stat_doc();
        doc.content[0] = Block::Panel {
            id: doc.content[0].id(),
            panel: quill_core_model::Panel {
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
        // Asserted on the measure, not on the ink. Under spec 0069 a placed frame is what a run
        // drew, and a ragged run always draws short of its measure by up to a word — so "the ink
        // fits" would pass against the very defect this test names, a section broken to the frame's
        // full width and then drawn inset by the padding. The measure is what has to fit.
        let measured = measure_at(&doc.content[0], panel.w_pt);
        for (mx, mw, ix, iw) in panel_parts(&measured) {
            assert!(
                mx + mw <= panel.w_pt - PANEL_PADDING_PT + 0.01,
                "a run was broken to a measure that overruns the panel's right padding: \
                 {mx} + {mw} > {}",
                panel.w_pt - PANEL_PADDING_PT
            );
            assert!(
                ix + iw <= mx + mw + 0.01,
                "a run drew past the measure it was broken to: {ix} + {iw} > {mx} + {mw}"
            );
        }
    }

    // --- Tables (spec 0039) ---------------------------------------------------------------------

    fn table_doc(table: quill_core_model::Table) -> Document {
        let mut doc = doc_with_blocks(vec![Block::Table {
            id: BlockId::UNASSIGNED,
            table,
            color: Color::Gray { v: 0.0 },
        }]);
        doc.assign_missing_block_ids().expect("ids");
        doc
    }

    fn simple_table() -> quill_core_model::Table {
        quill_core_model::Table {
            columns: vec![0.25, 0.75],
            header: Some(vec!["Roll".into(), "Result".into()]),
            rows: vec![
                vec!["1-3".into(), "Goblins".into()],
                vec!["4-6".into(), "Bandits".into()],
            ],
            zebra: true,
        }
    }

    fn cells(page: &LaidOutPage) -> Vec<(f32, f32, String)> {
        page.blocks
            .iter()
            .filter_map(|b| match b {
                PlacedBlock::Text { frame, lines, .. } => {
                    Some((frame.x_pt, frame.w_pt, lines[0].text.clone()))
                }
                _ => None,
            })
            .collect()
    }

    #[test]
    fn table_columns_land_at_exact_fractions_of_the_measure() {
        // 432 pt frame, widths 0.25/0.75 ⇒ columns at x = 0 and x = 108, each inset by the 3 pt
        // cell padding: text is broken at 3 and 111, to measures of 108 - 6 = 102 and 324 - 6 = 318.
        //
        // Asserted on the **measurement**, which is where a column width is known. A placed cell
        // reports the ink it drew (spec 0069): `Roll` is 21.6 pt wide whether its column is 102 pt
        // or 108 pt, so a column width read back off placed output stopped being a column width.
        let doc = table_doc(simple_table());
        let measured = measure_at(&doc.content[0], 432.0);
        let parts = panel_parts(&measured);
        assert_eq!(parts.len(), 6, "header plus two rows, two columns each");
        for (i, (mx, mw, ix, iw)) in parts.iter().copied().enumerate() {
            let (want_x, want_w) = if i % 2 == 0 {
                (3.0, 102.0)
            } else {
                (111.0, 318.0)
            };
            assert!((mx - want_x).abs() < 0.01, "cell {i} column x: {mx}");
            assert!((mw - want_w).abs() < 0.01, "cell {i} column width: {mw}");
            assert!(
                ix >= mx - 0.01 && ix + iw <= mx + mw + 0.01,
                "cell {i} ink {ix}+{iw} left its column"
            );
        }

        // And the cells really do land there once placed, at the columns' left edges.
        let pages = lay_out(&doc, &MONO, &NoHyphenator);
        let c = cells(&pages[0]);
        assert_eq!(c.len(), 6);
        assert!((c[0].0 - 3.0).abs() < 0.01, "column 0 x: {:?}", c[0]);
        assert!((c[1].0 - 111.0).abs() < 0.01, "column 1 x: {:?}", c[1]);
    }

    #[test]
    fn table_column_widths_are_normalized_not_taken_literally() {
        // `[1, 3]` must mean the same as `[0.25, 0.75]` — an author should not have to make the
        // widths sum to one, and taking them literally would run the table off the frame.
        let doc = table_doc(quill_core_model::Table {
            columns: vec![1.0, 3.0],
            ..simple_table()
        });
        let pages = lay_out(&doc, &MONO, &NoHyphenator);
        let c = cells(&pages[0]);
        assert!((c[1].0 - 111.0).abs() < 0.01, "column 1 x: {:?}", c[1]);
    }

    #[test]
    fn a_degenerate_column_width_falls_back_to_an_equal_split() {
        // Authoring posture, matching the over-wide gutter in spec 0030: a bad width costs the
        // look, never the content. A zero-width column would silently swallow its cells.
        for columns in [vec![0.0, 1.0], vec![-1.0, 2.0], vec![1.0]] {
            let doc = table_doc(quill_core_model::Table {
                columns,
                ..simple_table()
            });
            let pages = lay_out(&doc, &MONO, &NoHyphenator);
            let c = cells(&pages[0]);
            assert_eq!(c.len(), 6, "no cell may be lost");
            assert!(
                (c[1].0 - 219.0).abs() < 0.01,
                "an equal split puts column 1 at 216 + 3: {:?}",
                c[1]
            );
        }
    }

    #[test]
    fn a_wrapped_cell_pushes_its_whole_row_down() {
        // Row height is the tallest cell's. Without that a wrapped cell overlaps the row beneath.
        let long = "a fairly long cell value that will certainly wrap to more than one line here";
        let doc = table_doc(quill_core_model::Table {
            columns: vec![0.5, 0.5],
            header: None,
            rows: vec![
                vec!["short".into(), long.into()],
                vec!["next".into(), "row".into()],
            ],
            zebra: false,
        });
        let pages = lay_out(&doc, &MONO, &NoHyphenator);
        let c = cells(&pages[0]);

        let row0_y = match &pages[0].blocks[0] {
            PlacedBlock::Text { frame, .. } => frame.y_pt,
            other => panic!("expected a cell, got {other:?}"),
        };
        let row1_y = pages[0]
            .blocks
            .iter()
            .filter_map(|b| match b {
                PlacedBlock::Text { frame, lines, .. } if lines[0].text == "next" => {
                    Some(frame.y_pt)
                }
                _ => None,
            })
            .next()
            .expect("the second row");

        let wrapped_lines = pages[0]
            .blocks
            .iter()
            .filter_map(|b| match b {
                PlacedBlock::Text { lines, .. } if lines.len() > 1 => Some(lines.len()),
                _ => None,
            })
            .next()
            .expect("the long cell must actually wrap");
        assert!(wrapped_lines >= 2);
        assert!(
            row1_y >= row0_y + wrapped_lines as f32 * 11.5,
            "the next row must clear the wrapped cell: {row0_y} -> {row1_y}"
        );
        assert_eq!(c.len(), 4);
    }

    #[test]
    fn zebra_shades_alternate_rows_and_nothing_when_switched_off() {
        // On: one band per odd row, behind the text. Off: no bands at all. Both directions, so this
        // cannot pass against an implementation that always (or never) bands.
        let banded = table_doc(quill_core_model::Table {
            rows: (0..5)
                .map(|i| vec![format!("{i}"), format!("row {i}")])
                .collect(),
            ..simple_table()
        });
        let pages = lay_out(&banded, &MONO, &NoHyphenator);
        let rects: Vec<&PlacedBlock> = pages[0]
            .blocks
            .iter()
            .filter(|b| matches!(b, PlacedBlock::Rect { .. }))
            .collect();
        // Two odd rows (indices 1 and 3) plus the header rule.
        assert_eq!(rects.len(), 3, "two bands and a header rule");

        let plain = table_doc(quill_core_model::Table {
            zebra: false,
            header: None,
            ..simple_table()
        });
        let pages = lay_out(&plain, &MONO, &NoHyphenator);
        assert!(
            !pages[0]
                .blocks
                .iter()
                .any(|b| matches!(b, PlacedBlock::Rect { .. })),
            "zebra off and no header ⇒ no decoration at all"
        );
    }

    #[test]
    fn decoration_paints_before_the_cells_it_sits_behind() {
        let doc = table_doc(simple_table());
        let pages = lay_out(&doc, &MONO, &NoHyphenator);
        let first_text = pages[0]
            .blocks
            .iter()
            .position(|b| matches!(b, PlacedBlock::Text { .. }))
            .expect("cells");
        let last_rect = pages[0]
            .blocks
            .iter()
            .rposition(|b| matches!(b, PlacedBlock::Rect { .. }))
            .expect("decoration");
        assert!(last_rect < first_text, "bands must sit behind the text");
    }

    #[test]
    fn an_empty_table_occupies_nothing_and_does_not_panic() {
        let doc = table_doc(quill_core_model::Table::default());
        let pages = lay_out(&doc, &MONO, &NoHyphenator);
        assert_eq!(pages.len(), 1);
        assert!(pages[0].blocks.is_empty());
    }

    #[test]
    fn a_range_table_lays_out_through_the_conversion() {
        // End to end: the component that already existed, on a page at last.
        let ranged = quill_core_model::RangeTable {
            max: 6,
            label: "d6".into(),
            entries: vec![
                quill_core_model::RangeEntry {
                    low: 1,
                    high: 3,
                    value: "Goblins".into(),
                },
                quill_core_model::RangeEntry {
                    low: 4,
                    high: 4,
                    value: "One bandit".into(),
                },
            ],
        };
        let doc = table_doc(quill_core_model::Table::from_range_table(&ranged));
        let pages = lay_out(&doc, &MONO, &NoHyphenator);
        let texts: Vec<String> = cells(&pages[0]).into_iter().map(|c| c.2).collect();
        assert_eq!(
            texts,
            ["d6", "Result", "1-3", "Goblins", "4", "One bandit"],
            "a one-value range must read `4`, not `4-4`"
        );
    }

    #[test]
    fn a_five_hundred_row_table_places_every_cell() {
        // Correctness at scale, strengthened by spec 0045 from "every cell is placed somewhere" to
        // "every cell is placed exactly once, in row order". The old wording was the most that could
        // honestly be claimed while a table was placed whole and ran off the bottom of the page;
        // now it paginates, so conservation is a real invariant with a real way to fail — a cut that
        // drops or duplicates a row produces a random table missing entries, which no geometric
        // assertion notices.
        let rows: Vec<Vec<String>> = (0..500)
            .map(|i| vec![format!("{i}"), format!("result {i}")])
            .collect();
        let doc = table_doc(quill_core_model::Table {
            rows,
            header: None,
            ..simple_table()
        });
        let pages = lay_out(&doc, &MONO, &NoHyphenator);
        assert!(
            pages.len() > 1,
            "a 500-row table must paginate, got {} page(s)",
            pages.len()
        );

        let placed: Vec<String> = pages
            .iter()
            .flat_map(|p| cells(p).into_iter().map(|(_, _, t)| t))
            .collect();
        assert_eq!(placed.len(), 1000, "every cell of every row must be placed");
        let expected: Vec<String> = (0..500)
            .flat_map(|i| [format!("{i}"), format!("result {i}")])
            .collect();
        assert_eq!(placed, expected, "every cell exactly once, in row order");
    }

    /// A table of `n` numbered rows, following a one-line paragraph so the frame is not empty.
    fn table_after_intro(n: usize, header: Option<Vec<String>>, zebra: bool) -> Document {
        let rows: Vec<Vec<String>> = (0..n)
            .map(|i| vec![format!("r{i}"), format!("row {i}")])
            .collect();
        let mut doc = doc_with_blocks(vec![
            Block::body("intro", Color::Gray { v: 0.0 }),
            Block::Table {
                id: BlockId::UNASSIGNED,
                table: quill_core_model::Table {
                    columns: vec![0.25, 0.75],
                    header,
                    rows,
                    zebra,
                },
                color: Color::Gray { v: 0.0 },
            },
        ]);
        doc.assign_missing_block_ids().expect("ids");
        doc
    }

    #[test]
    fn a_tables_header_repeats_once_at_the_top_of_every_continuation() {
        // Spec 0039 promised this and could not build it: a table was one block, so it could not
        // break between rows, and with no continuation there was nothing to repeat onto.
        let doc = table_after_intro(120, Some(vec!["Roll".into(), "Result".into()]), false);
        let pages = lay_out(&doc, &MONO, &NoHyphenator);
        assert!(pages.len() >= 3, "need several pages, got {}", pages.len());

        // One "Roll" per fragment and no more. A header that repeated twice, or that appeared on a
        // page it did not start, would be just as wrong as one that never repeated.
        let fragments: Vec<Vec<(f32, f32, String)>> =
            pages.iter().map(cells).filter(|c| !c.is_empty()).collect();
        let headers: usize = fragments
            .iter()
            .map(|c| c.iter().filter(|(_, _, t)| t == "Roll").count())
            .sum();
        assert_eq!(
            headers,
            fragments.len(),
            "exactly one header per fragment, got {headers} across {} fragments",
            fragments.len()
        );
        for (i, frag) in fragments.iter().enumerate() {
            assert_eq!(
                frag.iter().filter(|(_, _, t)| t == "Roll").count(),
                1,
                "fragment {i} must carry exactly one header row"
            );
        }
    }

    #[test]
    fn a_headerless_table_splits_with_nothing_repeated() {
        let doc = table_after_intro(120, None, false);
        let pages = lay_out(&doc, &MONO, &NoHyphenator);
        let placed: Vec<String> = pages
            .iter()
            .flat_map(|p| cells(p).into_iter().map(|(_, _, t)| t))
            .filter(|t| t != "intro")
            .collect();
        let expected: Vec<String> = (0..120)
            .flat_map(|i| [format!("r{i}"), format!("row {i}")])
            .collect();
        assert_eq!(placed, expected, "no repeat, no loss, row order preserved");
    }

    #[test]
    fn zebra_striping_survives_a_page_break() {
        // The row that starts a continuation must be striped by its index in the *whole* table, not
        // in the fragment. Getting this wrong puts two same-shaded rows next to each other at a page
        // boundary — it reads as a rendering bug, and a count of banded rows would never show it.
        let doc = table_after_intro(120, Some(vec!["Roll".into(), "Result".into()]), true);
        let pages = lay_out(&doc, &MONO, &NoHyphenator);
        assert!(pages.len() >= 2);

        // Every banded row, identified by the row text sharing its band's y span.
        let mut banded: Vec<usize> = Vec::new();
        for page in &pages {
            let bands: Vec<(f32, f32)> = page
                .blocks
                .iter()
                .filter_map(|b| match b {
                    PlacedBlock::Rect { frame, fill, .. } if *fill == Some(TABLE_ZEBRA_FILL()) => {
                        Some((frame.y_pt, frame.y_pt + frame.h_pt))
                    }
                    _ => None,
                })
                .collect();
            for b in &page.blocks {
                if let PlacedBlock::Text { frame, lines, .. } = b {
                    let Some(idx) = lines[0].text.strip_prefix("row ") else {
                        continue;
                    };
                    let Ok(idx) = idx.parse::<usize>() else {
                        continue;
                    };
                    if bands
                        .iter()
                        .any(|(t, b)| frame.y_pt >= *t - 0.01 && frame.y_pt < *b + 0.01)
                    {
                        banded.push(idx);
                    }
                }
            }
        }
        assert!(!banded.is_empty(), "the fixture must actually stripe rows");
        assert!(
            banded.iter().all(|i| i % 2 == 1),
            "every banded row must be odd-indexed in the whole table; got {:?}",
            &banded[..banded.len().min(12)]
        );
    }

    #[test]
    fn no_table_fragment_overruns_its_frame() {
        // The off-by-one this increment is most likely to have: the repeated header is charged to
        // the fragment but not to the continuation (or the reverse), and every continuation
        // overfills by exactly one header row. That is why `PanelSplit::items[0]` folds `repeat_h`
        // in. Asserted as "nothing sticks out of its frame", which is the symptom rather than the
        // arithmetic, and so survives a change to how the arithmetic is written.
        let doc = table_after_intro(120, Some(vec!["Roll".into(), "Result".into()]), true);
        let pages = lay_out(&doc, &MONO, &NoHyphenator);
        let bottom = Frame::full_page(&doc.page_setup).rect.h_pt;
        for page in &pages {
            for b in &page.blocks {
                let (y, h, what) = match b {
                    PlacedBlock::Text { frame, lines, .. } => {
                        (frame.y_pt, frame.h_pt, lines[0].text.clone())
                    }
                    PlacedBlock::Rect { frame, .. } => (frame.y_pt, frame.h_pt, "rect".into()),
                    PlacedBlock::Image { frame, .. } => (frame.y_pt, frame.h_pt, "image".into()),
                    PlacedBlock::Link { frame, .. } => (frame.y_pt, frame.h_pt, "link".into()),
                };
                assert!(
                    y + h <= bottom + 0.01,
                    "page {} `{what}` runs to {:.1} past the frame bottom {bottom:.1}",
                    page.index,
                    y + h
                );
            }
        }
    }

    #[test]
    fn a_split_table_is_measured_once() {
        // Spec 0044's cache contract, re-asserted for the variant 0045 taught to split. A table cut
        // across a dozen frames must not be re-measured once per fragment.
        let doc = table_after_intro(300, Some(vec!["Roll".into(), "Result".into()]), true);
        let mut session = LayoutSession::new();
        let result = session.relayout(&doc, &MONO, &NoHyphenator);
        assert!(result.pages.len() > 3, "the fixture must span pages");
        assert_eq!(
            result.stats.blocks_measured,
            doc.content.len(),
            "one measurement per block, however many fragments it produced"
        );
    }

    #[test]
    fn a_table_of_three_rows_never_splits() {
        // The two-item minimum from spec 0044 applies to rows as it does to lines.
        let doc = table_after_intro(3, Some(vec!["Roll".into(), "Result".into()]), false);
        let pages = lay_out(&doc, &MONO, &NoHyphenator);
        let with_cells = pages.iter().filter(|p| !cells(p).is_empty()).count();
        assert_eq!(
            with_cells, 1,
            "a three-row table must be placed in one piece"
        );
    }

    // --- Generated table of contents (spec 0041) ------------------------------------------------

    fn toc_doc(max_level: u8, chapters: &[(u8, &str)], filler: usize) -> Document {
        let mut content: Vec<Block> = vec![Block::Toc {
            id: BlockId::UNASSIGNED,
            title: "Contents".into(),
            max_level,
            color: Color::Gray { v: 0.0 },
        }];
        for (level, name) in chapters {
            content.push(Block::heading(*level, *name, Color::Gray { v: 0.0 }));
            content.extend(many_lines(filler));
        }
        let mut doc = doc_with_blocks(content);
        doc.assign_missing_block_ids().expect("ids");
        doc
    }

    /// Every contents entry as `(title, printed page number)`, read back off the page.
    ///
    /// The contents block emits, in order: its own title, then per entry a title run, an optional
    /// dot-leader run, and a page-number run. Dropping the leaders leaves a flat
    /// `[title, number, title, number, ...]` sequence.
    fn toc_entries(doc: &Document, pages: &[LaidOutPage]) -> Vec<(String, String)> {
        // Filtered by source id: a contents block shares its page with whatever follows it, so
        // reading every text run on page 0 would sweep up the body text too.
        let toc_id = doc
            .content
            .iter()
            .find(|b| matches!(b, Block::Toc { .. }))
            .map(|b| b.id())
            .expect("a contents block");
        let texts: Vec<String> = pages
            .iter()
            .flat_map(|p| p.blocks.iter())
            .filter_map(|b| match b {
                PlacedBlock::Text { lines, source, .. } if *source == toc_id => {
                    Some(lines[0].text.clone())
                }
                _ => None,
            })
            .filter(|t| !t.is_empty() && !t.chars().all(|c| c == '.'))
            .collect();
        texts[1..]
            .chunks(2)
            .filter(|c| c.len() == 2)
            .map(|c| (c[0].clone(), c[1].clone()))
            .collect()
    }

    /// Spec 0041's hand-rolled entry arithmetic, kept as the oracle spec 0070's re-expression is
    /// measured against.
    ///
    /// A deliberate transcription of the code 0070 deleted — indent, clip column, leader origin,
    /// dot count, right-aligned number — because the equivalence claim is exactly "the mechanism
    /// puts every part where this put it". A golden captured *after* the change would prove
    /// nothing; this is the thing the change has to agree with.
    ///
    /// Returns `(x, text)` per placed entry part, in emission order.
    fn toc_reference_geometry(
        width: f32,
        max_level: u8,
        headings: &[HeadingEntry],
        styles: &StyleSheet,
        metrics: &impl RunMetrics,
    ) -> Vec<(f32, String)> {
        let mut out = Vec::new();
        for h in headings.iter().filter(|h| h.level <= max_level) {
            let style = styles
                .paragraph
                .get(&toc_entry_style_name(h.level))
                .copied()
                .unwrap_or_default();
            let indent = (h.level.saturating_sub(1)) as f32 * TOC_INDENT_PT;
            let number = (h.page_index + 1).to_string();
            let number_w = metrics.measure_run(&number, style.font_size_pt);

            let title_max = (width - indent - TOC_NUMBER_COLUMN_PT - TOC_LEADER_GAP_PT).max(1.0);
            let mut text = h.text.clone();
            if metrics.measure_run(&text, style.font_size_pt) > title_max {
                while !text.is_empty()
                    && metrics.measure_run(&format!("{text}…"), style.font_size_pt) > title_max
                {
                    text.pop();
                }
                text.push('…');
            }
            let title_w = metrics.measure_run(&text, style.font_size_pt);
            out.push((indent, text));

            let leader_x = indent + title_w + TOC_LEADER_GAP_PT;
            let leader_end = width - number_w - TOC_LEADER_GAP_PT;
            if leader_end > leader_x {
                let dot_w = metrics.measure_run(".", style.font_size_pt).max(0.01);
                let dots = ((leader_end - leader_x) / dot_w).floor().max(0.0) as usize;
                if dots > 0 {
                    out.push((leader_x, ".".repeat(dots)));
                }
            }
            out.push((width - number_w, number));
        }
        out
    }

    /// A fixture that exercises every branch of the arithmetic above: four indent levels, page
    /// numbers of more than one width, and a title long enough to be clipped with an ellipsis.
    fn toc_levels_doc() -> Document {
        toc_doc(
            4,
            &[
                (1, "Alpha"),
                (2, "A second-level entry"),
                (3, "Third level, deeper still"),
                (
                    1,
                    "A chapter with a really quite long name that will certainly have to be \
                     clipped with an ellipsis before it reaches the number column",
                ),
                (2, "Beta"),
                (4, "Fourth"),
                (1, "Gamma"),
            ],
            60,
        )
    }

    /// Every placed contents part below the list's own title, as `(x, text)` in emission order.
    fn toc_entry_parts(doc: &Document, pages: &[LaidOutPage]) -> Vec<(f32, String)> {
        let toc_id = doc
            .content
            .iter()
            .find(|b| matches!(b, Block::Toc { .. }))
            .map(|b| b.id())
            .expect("a contents block");
        let all: Vec<(f32, String)> = pages
            .iter()
            .flat_map(|p| p.blocks.iter())
            .filter_map(|b| match b {
                PlacedBlock::Text {
                    source,
                    frame,
                    lines,
                    ..
                } if *source == toc_id => Some((frame.x_pt, lines[0].text.clone())),
                _ => None,
            })
            .collect();
        // The list's own title comes first and is not an entry.
        all[1..].to_vec()
    }

    #[test]
    fn every_contents_x_and_dot_count_is_what_the_hand_rolled_arithmetic_produced() {
        // Spec 0070's equivalence claim, and the thing that makes it a generalization rather than
        // a rewrite: one right tab stop with a dot leader puts every part of every entry at exactly
        // the x spec 0041 computed by hand, and draws exactly the same number of dots.
        //
        // Asserted on the *bits* of each f32 rather than to a tolerance. "Byte-identical" is the
        // claim, and a tolerance would let the two arithmetics drift apart by a hair each time
        // either is touched, which is how an equivalence quietly stops being one.
        let doc = toc_levels_doc();
        let (pages, status) = lay_out_with_fixpoint_status(
            &doc.content,
            &doc.assets,
            &doc.styles,
            &DocumentTemplate::new(&doc),
            &MONO,
            &NoHyphenator,
        );
        assert!(status.converged, "the fixpoint must settle: {status:?}");

        let actual = toc_entry_parts(&doc, &pages);
        // The default page setup has zero margins, so the text frame starts at the trim's left edge
        // and a frame-relative x from the reference is directly comparable to a placed one.
        let width = doc.page_setup.trim.w_pt;
        assert_eq!(doc.page_setup.margins, quill_core_model::Margins::default());
        let index = heading_index_of(&doc.content, &pages);
        let expected = toc_reference_geometry(width, 4, &index, &doc.styles, &MONO);

        assert!(
            expected.iter().any(|(_, t)| t.ends_with('…')),
            "the fixture must clip at least one title, or the ellipsis branch is untested"
        );
        assert!(
            expected
                .iter()
                .filter(|(_, t)| t.chars().all(|c| c == '.'))
                .count()
                >= 7,
            "every entry must draw a leader, or the dot count is untested: {expected:?}"
        );
        assert_eq!(
            actual.len(),
            expected.len(),
            "the same parts, in the same order:\n  got {actual:?}\n  want {expected:?}"
        );
        for (i, ((ax, at), (ex, et))) in actual.iter().zip(&expected).enumerate() {
            assert_eq!(at, et, "part {i}: text, and so the leader's dot count");
            assert_eq!(
                ax.to_bits(),
                ex.to_bits(),
                "part {i} ({at:?}): x is {ax} but spec 0041 put it at {ex}"
            );
        }
    }

    #[test]
    fn a_contents_entry_reports_the_ink_it_draws_rather_than_the_columns_it_was_given() {
        // The three widths spec 0070 deliberately moves, each checked against what it now claims to
        // be. They move because spec 0069 settled that `w_pt` is ink, which makes the mechanism's
        // values the correct ones and the hand-rolled columns the defect.
        let doc = toc_levels_doc();
        let (pages, status) = lay_out_with_fixpoint_status(
            &doc.content,
            &doc.assets,
            &doc.styles,
            &DocumentTemplate::new(&doc),
            &MONO,
            &NoHyphenator,
        );
        assert!(status.converged);
        let width = doc.page_setup.trim.w_pt;
        let toc_id = doc
            .content
            .iter()
            .find(|b| matches!(b, Block::Toc { .. }))
            .map(|b| b.id())
            .expect("a contents block");
        let placed: Vec<(Rect, String, f32)> = pages
            .iter()
            .flat_map(|p| p.blocks.iter())
            .filter_map(|b| match b {
                PlacedBlock::Text {
                    source,
                    frame,
                    lines,
                    font_size_pt,
                    ..
                } if *source == toc_id => Some((*frame, lines[0].text.clone(), *font_size_pt)),
                _ => None,
            })
            .collect();

        // 1. The list's *own* title: its measured advance, not the whole frame measure. Spec 0069
        //    already moved this one, so this verifies rather than changes it.
        let (frame, text, size) = &placed[0];
        assert_eq!(text, "Contents");
        assert_eq!(
            frame.w_pt.to_bits(),
            MONO.measure_run(text, *size).to_bits()
        );
        assert!(
            frame.w_pt < width,
            "the list's title must not report the frame measure"
        );

        let mut checked_titles = 0;
        let mut checked_leaders = 0;
        for (i, (frame, text, size)) in placed.iter().enumerate().skip(1) {
            if text.chars().all(|c| c == '.') {
                // 2. A leader: the dots it draws, not the gap it was asked to fill. The gap runs
                //    from here to the number's left edge less the leader's own inset, and the last
                //    whole dot always stops short of it — a partial dot is never drawn.
                let number = &placed[i + 1].0;
                let gap = (number.x_pt - TOC_LEADER_GAP_PT) - frame.x_pt;
                let dot_w = MONO.measure_run(".", *size);
                assert_eq!(
                    frame.w_pt.to_bits(),
                    (dot_w * text.chars().count() as f32).to_bits(),
                    "a leader reports its drawn dots"
                );
                assert!(
                    frame.w_pt <= gap && gap - frame.w_pt < dot_w,
                    "and they fill the gap to within one dot: {} of {gap}",
                    frame.w_pt
                );
                checked_leaders += 1;
            } else if text.chars().all(|c| c.is_ascii_digit()) {
                // 3. The page number was already ink before spec 0069 made it the rule, and is the
                //    control: it does not move.
                assert_eq!(
                    frame.w_pt.to_bits(),
                    MONO.measure_run(text, *size).to_bits()
                );
                assert_eq!(
                    (frame.x_pt + frame.w_pt).to_bits(),
                    width.to_bits(),
                    "and still ends at the measure's right edge"
                );
            } else {
                // 4. An entry title: its measured advance, not the column it was clipped to.
                let indent = frame.x_pt;
                let clip = (width - indent - TOC_NUMBER_COLUMN_PT - TOC_LEADER_GAP_PT).max(1.0);
                assert_eq!(
                    frame.w_pt.to_bits(),
                    MONO.measure_run(text, *size).to_bits(),
                    "an entry title reports its advance"
                );
                assert!(
                    frame.w_pt <= clip,
                    "which is inside the column it was clipped to ({clip})"
                );
                checked_titles += 1;
            }
        }
        assert_eq!(checked_titles, 7, "one title per entry");
        assert_eq!(checked_leaders, 7, "one leader per entry");
    }

    #[test]
    fn every_contents_entry_carries_a_link_candidate_over_its_own_title_run() {
        // Spec 0052. Three properties, and the third is the one that matters: a candidate per
        // entry, each pointing at the page its heading landed on, and each sharing a rectangle
        // *exactly* with the title run it belongs to. A link whose hot area has drifted off its
        // text is a defect no parser reports and no page-count test notices.
        let doc = toc_doc(2, &[(1, "Alpha"), (1, "Beta"), (1, "Gamma")], 60);
        let pages = lay_out(&doc, &MONO, &NoHyphenator);
        let index = heading_index(&doc, &pages);

        let links: Vec<(Rect, usize)> = pages
            .iter()
            .flat_map(|p| p.blocks.iter())
            .filter_map(|b| match b {
                PlacedBlock::Link {
                    frame, target_page, ..
                } => Some((*frame, *target_page)),
                _ => None,
            })
            .collect();
        assert_eq!(links.len(), index.len(), "one candidate per listed heading");

        for ((title, _), (rect, target)) in toc_entries(&doc, &pages).iter().zip(&links) {
            let heading = index
                .iter()
                .find(|h| &h.text == title)
                .expect("a heading for every entry");
            assert_eq!(
                *target, heading.page_index,
                "'{title}' must link to the page it is on"
            );
            // The title run with this text, on the contents page rather than the chapter opener.
            let run = pages
                .iter()
                .flat_map(|p| p.blocks.iter())
                .filter_map(|b| match b {
                    PlacedBlock::Text { frame, lines, .. } if lines[0].text == *title => {
                        Some(*frame)
                    }
                    _ => None,
                })
                .find(|f| *f == *rect);
            assert!(
                run.is_some(),
                "'{title}' link rect {rect:?} must coincide with its own placed title run"
            );
        }
    }

    #[test]
    fn a_contents_list_prints_the_pages_the_headings_actually_landed_on() {
        // The whole feature, and it must be asserted against the FINAL layout. Comparing a
        // first-pass contents list against first-pass numbers would agree with itself while being
        // wrong about the document.
        let doc = toc_doc(2, &[(1, "Alpha"), (1, "Beta"), (1, "Gamma")], 60);
        let (pages, status) = lay_out_with_fixpoint_status(
            &doc.content,
            &doc.assets,
            &doc.styles,
            &DocumentTemplate::new(&doc),
            &MONO,
            &NoHyphenator,
        );
        assert!(status.converged, "the fixpoint must settle: {status:?}");

        let printed = toc_entries(&doc, &pages);
        let actual = heading_index_of(&doc.content, &pages);
        assert_eq!(printed.len(), 3, "one entry per heading: {printed:?}");
        for (entry, heading) in printed.iter().zip(actual.iter()) {
            assert_eq!(entry.0, heading.text);
            assert_eq!(
                entry.1,
                (heading.page_index + 1).to_string(),
                "`{}` prints {} but is on page {}",
                heading.text,
                entry.1,
                heading.page_index + 1
            );
        }
    }

    /// A book with enough chapters that its contents list cannot fit one page.
    ///
    /// **Nothing in the workspace exercised this before spec 0075** — every contents fixture is a
    /// handful of chapters — which is exactly why the defect shipped. A real 500-page book's
    /// contents list is three pages.
    fn long_toc_doc(chapters: usize) -> Document {
        let names: Vec<(u8, String)> = (0..chapters)
            .map(|i| (1u8, format!("Chapter {i}")))
            .collect();
        let refs: Vec<(u8, &str)> = names.iter().map(|(l, n)| (*l, n.as_str())).collect();
        toc_doc(1, &refs, 3)
    }

    /// Which page each contents entry landed on, in placement order.
    fn toc_entry_pages(doc: &Document, pages: &[LaidOutPage]) -> Vec<(usize, String)> {
        let toc_id = doc
            .content
            .iter()
            .find(|b| matches!(b, Block::Toc { .. }))
            .map(|b| b.id())
            .expect("a contents block");
        pages
            .iter()
            .flat_map(|p| p.blocks.iter().map(move |b| (p.index, b)))
            .filter_map(|(i, b)| match b {
                PlacedBlock::Text { lines, source, .. } if *source == toc_id => {
                    Some((i, lines[0].text.clone()))
                }
                _ => None,
            })
            .filter(|(_, t)| !t.is_empty() && !t.chars().all(|c| c == '.'))
            .collect()
    }

    #[test]
    fn a_contents_list_taller_than_its_frame_is_split_across_pages() {
        // **The headline of spec 0075.** `measure_toc` returned an indivisible panel, so a contents
        // list taller than its frame reached the branch that places a block whole and lets it
        // overflow — silently, in the feature a long book most needs.
        let doc = long_toc_doc(150);
        let (pages, status) = lay_out_with_fixpoint_status(
            &doc.content,
            &doc.assets,
            &doc.styles,
            &DocumentTemplate::new(&doc),
            &MONO,
            &NoHyphenator,
        );
        assert!(
            status.converged,
            "the fixpoint must still settle: {status:?}"
        );

        let entries = toc_entry_pages(&doc, &pages);
        let carrying: std::collections::BTreeSet<usize> = entries.iter().map(|(p, _)| *p).collect();
        assert!(
            carrying.len() >= 3,
            "a 150-chapter contents list must span pages, not one: {carrying:?}"
        );

        // Nothing lost, nothing duplicated, nothing reordered — the only failure that silently
        // produces a wrong book. The list's own title opens it, then one entry per chapter in
        // document order, each with its page number.
        let texts: Vec<String> = entries.iter().map(|(_, t)| t.clone()).collect();
        let titles: Vec<&String> = texts[1..].iter().step_by(2).collect();
        let expected: Vec<String> = (0..150).map(|i| format!("Chapter {i}")).collect();
        assert_eq!(texts[0], "Contents", "the list opens with its own title");
        assert_eq!(
            titles.len(),
            150,
            "one entry per chapter, got {}",
            titles.len()
        );
        for (got, want) in titles.iter().zip(expected.iter()) {
            assert_eq!(*got, want);
        }

        // The title is re-stated nowhere: a repeated "Contents" would read as a second contents
        // list rather than the same one carrying on.
        assert_eq!(
            texts.iter().filter(|t| *t == "Contents").count(),
            1,
            "the list's own title belongs to the first fragment only"
        );

        // No fragment is a widow. Every page that carries entries carries at least the minimum.
        for page in &carrying {
            let n = entries.iter().filter(|(p, _)| p == page).count();
            // Two runs per entry (title, number), and the first page also carries the list title.
            let entries_here = if *page == entries[0].0 {
                (n - 1) / 2
            } else {
                n / 2
            };
            assert!(
                entries_here >= MIN_ENTRIES_PER_FRAGMENT,
                "page {page} carries {entries_here} contents entries, below the minimum"
            );
        }

        // And it fits: the defect itself was geometry running off the bottom of the page.
        let template = DocumentTemplate::new(&doc);
        for page in &pages {
            let bottom = template
                .frames(page.index)
                .iter()
                .map(|f| f.rect.y_pt + f.rect.h_pt)
                .fold(0.0_f32, f32::max);
            for b in &page.blocks {
                let (y, h) = match b {
                    PlacedBlock::Text { frame, .. }
                    | PlacedBlock::Rect { frame, .. }
                    | PlacedBlock::Image { frame, .. }
                    | PlacedBlock::Link { frame, .. } => (frame.y_pt, frame.h_pt),
                };
                assert!(
                    y + h <= bottom + 0.01,
                    "page {} runs to {:.1}, past the frame bottom {bottom:.1}",
                    page.index,
                    y + h
                );
            }
        }
    }

    #[test]
    fn a_split_contents_list_prints_the_pages_the_headings_actually_landed_on() {
        // The fixpoint, over a contents list that now changes the page count by *pages* rather than
        // by lines. A contents list that spans three pages pushes every chapter three pages later,
        // which changes the numbers it prints, which is the loop this feature has always been.
        // Bounded by `FIXPOINT_MAX_ITERATIONS`; this asserts it still settles inside the bound and that
        // what it settled on is true of the document it produced.
        let doc = long_toc_doc(150);
        let (pages, status) = lay_out_with_fixpoint_status(
            &doc.content,
            &doc.assets,
            &doc.styles,
            &DocumentTemplate::new(&doc),
            &MONO,
            &NoHyphenator,
        );
        assert!(status.converged, "must settle: {status:?}");
        assert!(
            status.iterations <= FIXPOINT_MAX_ITERATIONS,
            "converged is only meaningful inside the bound: {status:?}"
        );

        let printed = toc_entries(&doc, &pages);
        let actual = heading_index_of(&doc.content, &pages);
        assert_eq!(printed.len(), actual.len());
        for (entry, heading) in printed.iter().zip(actual.iter()) {
            assert_eq!(entry.0, heading.text);
            assert_eq!(
                entry.1,
                (heading.page_index + 1).to_string(),
                "`{}` prints {} but is on page {}",
                heading.text,
                entry.1,
                heading.page_index + 1
            );
        }
    }

    #[test]
    fn the_fixpoint_settles_in_few_passes() {
        // "It converged" as a measured claim rather than an assertion of faith.
        let doc = toc_doc(2, &[(1, "Alpha"), (1, "Beta")], 40);
        let (_, status) = lay_out_with_fixpoint_status(
            &doc.content,
            &doc.assets,
            &doc.styles,
            &DocumentTemplate::new(&doc),
            &MONO,
            &NoHyphenator,
        );
        assert!(status.converged);
        assert!(
            status.iterations <= 3,
            "a one-page contents list should settle quickly, took {}",
            status.iterations
        );
    }

    #[test]
    fn a_document_without_a_contents_block_takes_exactly_one_pass() {
        // The loop must not be entered at all, so no other document pays for this feature.
        let doc = doc_with_blocks(many_lines(200));
        let (_, status) = lay_out_with_fixpoint_status(
            &doc.content,
            &doc.assets,
            &doc.styles,
            &DocumentTemplate::new(&doc),
            &MONO,
            &NoHyphenator,
        );
        assert_eq!(status.iterations, 1);
        assert!(status.converged);
    }

    #[test]
    fn max_level_omits_deeper_headings() {
        let doc = toc_doc(2, &[(1, "One"), (2, "Two"), (3, "Three")], 5);
        let (pages, _) = lay_out_with_fixpoint_status(
            &doc.content,
            &doc.assets,
            &doc.styles,
            &DocumentTemplate::new(&doc),
            &MONO,
            &NoHyphenator,
        );
        let titles: Vec<String> = toc_entries(&doc, &pages).into_iter().map(|e| e.0).collect();
        assert!(titles.contains(&"One".to_string()));
        assert!(titles.contains(&"Two".to_string()));
        assert!(!titles.contains(&"Three".to_string()), "h3 must be omitted");
    }

    #[test]
    fn a_page_number_is_right_aligned_to_the_measure() {
        // Geometry, not appearance: a 1-digit and a 3-digit page must end in the same column.
        let doc = toc_doc(1, &[(1, "Alpha"), (1, "Beta")], 200);
        let (pages, _) = lay_out_with_fixpoint_status(
            &doc.content,
            &doc.assets,
            &doc.styles,
            &DocumentTemplate::new(&doc),
            &MONO,
            &NoHyphenator,
        );
        let rights: Vec<f32> = pages[0]
            .blocks
            .iter()
            .filter_map(|b| match b {
                PlacedBlock::Text { frame, lines, .. }
                    if !lines[0].text.is_empty()
                        && lines[0].text.chars().all(|c| c.is_ascii_digit()) =>
                {
                    Some(frame.x_pt + frame.w_pt)
                }
                _ => None,
            })
            .collect();
        assert!(rights.len() >= 2, "need two numbers to compare");
        for r in &rights {
            assert!(
                (r - doc.page_setup.trim.w_pt).abs() < 0.01,
                "page numbers must end at the measure's right edge: {rights:?}"
            );
        }
    }

    #[test]
    fn a_contents_list_with_no_headings_is_just_its_title() {
        let doc = toc_doc(2, &[], 0);
        let (pages, status) = lay_out_with_fixpoint_status(
            &doc.content,
            &doc.assets,
            &doc.styles,
            &DocumentTemplate::new(&doc),
            &MONO,
            &NoHyphenator,
        );
        assert!(status.converged);
        let texts: Vec<String> = pages[0]
            .blocks
            .iter()
            .filter_map(|b| match b {
                PlacedBlock::Text { lines, .. } => Some(lines[0].text.clone()),
                _ => None,
            })
            .collect();
        assert_eq!(texts, ["Contents"]);

        // And it is set in the contents *title* style, not at entry size — the built-in styles are
        // what make a generated contents list read as one with no authoring.
        let size = match &pages[0].blocks[0] {
            PlacedBlock::Text { font_size_pt, .. } => *font_size_pt,
            other => panic!("expected the title, got {other:?}"),
        };
        assert_eq!(
            size,
            doc.styles.paragraph[quill_core_model::TOC_TITLE_STYLE].font_size_pt
        );
        assert!(
            size > doc.styles.paragraph["toc-1"].font_size_pt,
            "the title must outrank its entries"
        );
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
                align: StaticAlign::Left,
                mirror: false,
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

    // ----- spec 0044: block fragmentation -----------------------------------------------------

    /// A paragraph of `n` distinct 12-character words.
    ///
    /// In a 120 pt measure under `MONO` (6 pt/char, so 20 characters) two such words plus a space
    /// need 25 characters and do not fit, so exactly one lands per line and the paragraph breaks
    /// into exactly `n` lines whose text is predictable word by word.
    fn n_line_paragraph(n: usize) -> String {
        (0..n)
            .map(|i| format!("w{i:02}aaaaaaaaa"))
            .collect::<Vec<_>>()
            .join(" ")
    }

    /// `count` equal columns, 120 pt wide and `h` tall, 24 pt apart.
    fn split_columns(count: usize, h: f32) -> Thread {
        Thread {
            frames: (0..count)
                .map(|i| Frame {
                    rect: Rect {
                        x_pt: i as f32 * 144.0,
                        y_pt: 0.0,
                        w_pt: 120.0,
                        h_pt: h,
                    },
                })
                .collect(),
        }
    }

    /// Every line placed for `source`, in reading order — the block as the reader receives it.
    ///
    /// Reading order through a threaded page is column then y, *not* y across the page: the left
    /// column is read to its foot before the right column starts. Sorting by y alone interleaves
    /// the two and would make a correctly conserved block look scrambled.
    fn lines_of(pages: &[LaidOutPage], source: BlockId) -> Vec<String> {
        let mut out = Vec::new();
        for page in pages {
            let mut placed: Vec<(f32, f32, &Vec<Line>)> = page
                .blocks
                .iter()
                .filter_map(|b| match b {
                    PlacedBlock::Text {
                        source: s,
                        frame,
                        lines,
                        ..
                    } if *s == source => Some((frame.x_pt, frame.y_pt, lines)),
                    _ => None,
                })
                .collect();
            placed.sort_by(|a, b| (a.0, a.1).partial_cmp(&(b.0, b.1)).unwrap());
            out.extend(
                placed
                    .into_iter()
                    .flat_map(|(_, _, l)| l.iter().map(|l| l.text.clone())),
            );
        }
        out
    }

    /// Lay `content` out through `thread`, with ids assigned.
    fn flow_columns(content: Vec<Block>, thread: &Thread) -> (Vec<Block>, Vec<LaidOutPage>) {
        let mut content = content;
        for (i, b) in content.iter_mut().enumerate() {
            b.set_id(BlockId(i as u64 + 1));
        }
        let pages = lay_out_in_thread(
            &content,
            &[],
            &StyleSheet::default(),
            thread,
            &MONO,
            &NoHyphenator,
        );
        (content, pages)
    }

    #[test]
    fn a_paragraph_fills_the_column_then_continues_in_the_next() {
        // A 12-line-tall column already holding one line has room for 11 more, so a 20-line
        // paragraph leaves 11 behind and carries 9 forward. Asserted by line *text*: a count would
        // pass just as happily if the continuation repeated the fragment.
        let thread = split_columns(2, 144.0);
        let (content, pages) = flow_columns(
            vec![
                Block::body("intro", Color::Gray { v: 0.0 }),
                Block::body(n_line_paragraph(20), Color::Gray { v: 0.0 }),
            ],
            &thread,
        );
        let para = content[1].id();

        let first: Vec<_> = pages[0]
            .blocks
            .iter()
            .filter_map(|b| match b {
                PlacedBlock::Text {
                    source,
                    frame,
                    lines,
                    ..
                } if *source == para => Some((frame.x_pt, lines.len(), lines[0].text.clone())),
                _ => None,
            })
            .collect();

        assert_eq!(
            first.len(),
            2,
            "the paragraph must be placed in two pieces, got {first:?}"
        );
        assert_eq!(first[0], (0.0, 11, "w00aaaaaaaaa".to_string()));
        assert_eq!(first[1], (144.0, 9, "w11aaaaaaaaa".to_string()));
    }

    #[test]
    fn every_line_of_every_split_block_survives_exactly_once() {
        // The conservation invariant, and the only test here that catches the failure that matters:
        // a fragment whose remainder is dropped produces a book missing a paragraph, and no
        // geometric assertion notices. Asserted against the unsplit break of each paragraph, over a
        // document that splits many times across many pages.
        let thread = split_columns(2, 144.0);
        let paragraphs: Vec<String> = (0..12).map(|i| n_line_paragraph(9 + i)).collect();
        let content: Vec<Block> = std::iter::once(Block::body("intro", Color::Gray { v: 0.0 }))
            .chain(
                paragraphs
                    .iter()
                    .map(|t| Block::body(t.clone(), Color::Gray { v: 0.0 })),
            )
            .collect();
        let (content, pages) = flow_columns(content, &thread);

        assert!(
            pages.len() > 1,
            "the fixture must span pages, got {}",
            pages.len()
        );

        let mut split_blocks = 0;
        for (i, text) in paragraphs.iter().enumerate() {
            let id = content[i + 1].id();
            let expected: Vec<String> = justify_paragraph_hyphenated(
                text,
                120.0,
                BODY_FONT_SIZE_PT,
                Alignment::Justified,
                &MONO,
                &NoHyphenator,
            )
            .into_iter()
            .map(|l| l.text)
            .collect();
            let got = lines_of(&pages, id);
            assert_eq!(got, expected, "paragraph {i} was not conserved");

            let pieces = pages
                .iter()
                .flat_map(|p| p.blocks.iter())
                .filter(|b| matches!(b, PlacedBlock::Text { source, .. } if *source == id))
                .count();
            if pieces > 1 {
                split_blocks += 1;
            }
        }
        assert!(
            split_blocks >= 4,
            "the fixture must exercise splitting, only {split_blocks} blocks split"
        );
    }

    /// Two columns of the same width and different heights, so a test can leave an exact amount of
    /// room at the foot of the first without also constraining the second.
    fn split_columns_h(h0: f32, h1: f32) -> Thread {
        Thread {
            frames: vec![
                Frame {
                    rect: Rect {
                        x_pt: 0.0,
                        y_pt: 0.0,
                        w_pt: 120.0,
                        h_pt: h0,
                    },
                },
                Frame {
                    rect: Rect {
                        x_pt: 144.0,
                        y_pt: 0.0,
                        w_pt: 120.0,
                        h_pt: h1,
                    },
                },
            ],
        }
    }

    /// Pieces of `source` placed in the first column of page 0.
    fn pieces_in_first_column(pages: &[LaidOutPage], source: BlockId) -> usize {
        pages[0]
            .blocks
            .iter()
            .filter(
                |b| matches!(b, PlacedBlock::Text { source: s, frame, .. } if *s == source && frame.x_pt == 0.0),
            )
            .count()
    }

    #[test]
    fn a_paragraph_never_strands_a_single_line() {
        // Three assertions of one rule. A widow left behind and an orphan carried forward are the
        // same defect seen from two sides, and a splitter that fixes ragged column feet by
        // producing them has made the page worse.
        //
        // The second column is deliberately tall enough to take each paragraph whole, so these
        // assert the widow rule and not the separate question of what happens to a block too big
        // for an empty frame (spec 0045).
        let ink = Color::Gray { v: 0.0 };

        // One line of room at the foot: leave it unset rather than strand a single line there.
        let thread = split_columns_h(24.0, 240.0);
        let (content, pages) = flow_columns(
            vec![
                Block::body("intro", ink),
                Block::body(n_line_paragraph(8), ink),
            ],
            &thread,
        );
        assert_eq!(
            pieces_in_first_column(&pages, content[1].id()),
            0,
            "one line of room must not produce a one-line fragment"
        );

        // Room for 7 of an 8-line paragraph: cut at 6 so the remainder keeps two, not one.
        let thread = split_columns_h(96.0, 240.0);
        let (content, pages) = flow_columns(
            vec![
                Block::body("intro", ink),
                Block::body(n_line_paragraph(8), ink),
            ],
            &thread,
        );
        let counts: Vec<usize> = pages[0]
            .blocks
            .iter()
            .filter_map(|b| match b {
                PlacedBlock::Text { source, lines, .. } if *source == content[1].id() => {
                    Some(lines.len())
                }
                _ => None,
            })
            .collect();
        assert_eq!(
            counts,
            vec![6, 2],
            "the cut must back off to leave two lines"
        );

        // Three lines or fewer never split, whatever the room: two full lines are available at the
        // foot of the first column and the paragraph still moves whole.
        let thread = split_columns_h(36.0, 240.0);
        let (content, pages) = flow_columns(
            vec![
                Block::body("intro", ink),
                Block::body(n_line_paragraph(3), ink),
            ],
            &thread,
        );
        let pieces = pages
            .iter()
            .flat_map(|p| p.blocks.iter())
            .filter(|b| matches!(b, PlacedBlock::Text { source, .. } if *source == content[1].id()))
            .count();
        assert_eq!(pieces, 1, "a three-line paragraph must never split");
        assert_eq!(pieces_in_first_column(&pages, content[1].id()), 0);
    }

    #[test]
    fn space_above_is_charged_to_the_fragment_and_space_below_to_the_remainder() {
        // Not a partition: the natural implementation subtracts, and is wrong in a way that makes
        // every continuation one space-after too tall and shows up only as slow page drift.
        let mut styles = StyleSheet::default();
        styles.paragraph.insert(
            BODY_STYLE.to_string(),
            ParagraphStyle {
                weight: Default::default(),
                italic: false,
                list: None,
                font_size_pt: BODY_FONT_SIZE_PT,
                leading_pt: BODY_LINE_HEIGHT_PT,
                align: TextAlign::Justified,
                space_before_pt: 9.0,
                space_after_pt: 5.0,
                indent: Default::default(),
            },
        );
        let mut content = vec![
            Block::body("intro", Color::Gray { v: 0.0 }),
            Block::body(n_line_paragraph(12), Color::Gray { v: 0.0 }),
        ];
        for (i, b) in content.iter_mut().enumerate() {
            b.set_id(BlockId(i as u64 + 1));
        }
        let thread = split_columns(2, 144.0);
        let pages = lay_out_in_thread(&content, &[], &styles, &thread, &MONO, &NoHyphenator);
        let para = content[1].id();

        let boxes: Vec<(f32, f32, usize)> = pages[0]
            .blocks
            .iter()
            .filter_map(|b| match b {
                PlacedBlock::Text {
                    source,
                    frame,
                    lines,
                    ..
                } if *source == para => Some((frame.y_pt, frame.h_pt, lines.len())),
                _ => None,
            })
            .collect();

        // The intro occupies 9 + 12 + 5 = 26 pt, leaving 118 pt of the 144 pt column. The fragment
        // is charged the 9 pt of space *above*, so nine of its twelve lines fit (9 + 108 = 117 pt)
        // and three carry forward. Twelve lines rather than ten so the cut is bounded by the height
        // and not by the two-line minimum — otherwise this asserts the widow rule a second time
        // instead of asserting the space arithmetic.
        assert_eq!(
            boxes.len(),
            2,
            "expected a fragment and a remainder, got {boxes:?}"
        );
        assert!(
            (boxes[0].0 - (26.0 + 9.0)).abs() < 0.01,
            "fragment y {}",
            boxes[0].0
        );
        assert_eq!(boxes[0].2, 9);
        assert!(
            (boxes[0].1 - 108.0).abs() < 0.01,
            "fragment height {}",
            boxes[0].1
        );
        // Each box is its lines and nothing else — 9 x 12 pt and 3 x 12 pt — because a placed frame
        // is the ink drawn and neither vertical space draws anything (spec 0069). The space
        // arithmetic this test is about is still visible in the *positions*: the fragment starts
        // 9 pt below the intro because it is charged the space above, and the remainder starts
        // flush at the next column's top because it is not.
        assert!(
            (boxes[1].0 - 0.0).abs() < 0.01,
            "remainder y {}",
            boxes[1].0
        );
        assert_eq!(boxes[1].2, 3);
        assert!(
            (boxes[1].1 - 36.0).abs() < 0.01,
            "remainder height {}",
            boxes[1].1
        );
    }

    #[test]
    fn a_block_facing_a_narrower_continuation_moves_whole() {
        // The cut is an index into the line list at *this* width; another width re-wraps to a
        // different list against which that index means something else. The guard is the reason
        // splitting is correct rather than approximately correct, so it gets its own test.
        let thread = Thread {
            frames: vec![
                Frame {
                    rect: Rect {
                        x_pt: 0.0,
                        y_pt: 0.0,
                        w_pt: 120.0,
                        h_pt: 144.0,
                    },
                },
                Frame {
                    rect: Rect {
                        x_pt: 144.0,
                        y_pt: 0.0,
                        w_pt: 90.0,
                        h_pt: 144.0,
                    },
                },
            ],
        };
        let (content, pages) = flow_columns(
            vec![
                Block::body("intro", Color::Gray { v: 0.0 }),
                Block::body(n_line_paragraph(20), Color::Gray { v: 0.0 }),
            ],
            &thread,
        );
        let pieces = pages
            .iter()
            .flat_map(|p| p.blocks.iter())
            .filter(|b| matches!(b, PlacedBlock::Text { source, .. } if *source == content[1].id()))
            .count();
        assert_eq!(
            pieces, 1,
            "a differing continuation width must fall back to moving whole"
        );
    }

    #[test]
    fn an_image_yields_no_break_opportunities() {
        // "Everything is splittable" must never become an assumption. An oversized image still
        // moves whole and, in an empty frame, still overflows rather than looping.
        let assets = vec![Asset {
            id: "big".into(),
            path: "big.png".into(),
            px_w: 400,
            px_h: 1200,
            dpi: 300.0,
            has_alpha: false,
            line_art: false,
        }];
        let mut content = vec![
            Block::body("intro", Color::Gray { v: 0.0 }),
            Block::image("big"),
        ];
        for (i, b) in content.iter_mut().enumerate() {
            b.set_id(BlockId(i as u64 + 1));
        }
        let thread = split_columns(2, 144.0);
        let pages = lay_out_in_thread(
            &content,
            &assets,
            &StyleSheet::default(),
            &thread,
            &MONO,
            &NoHyphenator,
        );
        let images = pages
            .iter()
            .flat_map(|p| p.blocks.iter())
            .filter(|b| matches!(b, PlacedBlock::Image { .. }))
            .count();
        assert_eq!(images, 1, "an image must be placed once, whole");
    }
    // --- Master static alignment and parity (spec 0047) ---------------------------------------

    /// A document whose `body` master carries one text static in the bottom margin band, spanning
    /// the text measure of a recto (inside 54, outside 40 on a 432 pt page).
    fn doc_with_folio(align: StaticAlign, mirror: bool, text: &str) -> Document {
        let margins = Margins {
            top_pt: 54.0,
            bottom_pt: 54.0,
            inside_pt: 54.0,
            outside_pt: 40.0,
        };
        let master = MasterPage {
            margins: Some(margins),
            statics: vec![MasterStatic::Text {
                rect: Rect {
                    x_pt: 54.0,
                    y_pt: 606.0,
                    w_pt: 338.0,
                    h_pt: 12.0,
                },
                text: text.into(),
                color: Color::Gray { v: 0.0 },
                style: Some(BODY_STYLE.into()),
                align,
                mirror,
            }],
            ..MasterPage::plain("body")
        };
        let mut doc = doc_with_master(master, many_lines(200));
        doc.page_setup.margins = margins;
        doc
    }

    /// What a page's first static actually prints.
    fn static_text(page: &LaidOutPage) -> String {
        match &page.statics[0] {
            PlacedBlock::Text { lines, .. } => {
                lines.iter().map(|l| l.text.as_str()).collect::<String>()
            }
            other => panic!("expected a text static, got {other:?}"),
        }
    }

    /// Spec 0073's headline, asserted on the **placed static text** rather than on
    /// `Document::folio` — the intermediate is unit-tested in `core-model`, and what matters here is
    /// that the number a reader sees printed on the page is the one the section asked for.
    ///
    /// Roman front matter, then a body that restarts at arabic 1. The restart is the part that
    /// cannot be faked by an offset: page 3 of the file prints `1`, not `4`.
    #[test]
    fn roman_front_matter_gives_way_to_a_body_that_restarts_at_arabic_one() {
        use quill_core_model::{Folio, NumberFormat, Section};

        let mut doc = doc_with_folio(StaticAlign::Left, false, PAGE_TOKEN);
        // Two anchors: the first block, and one far enough in to open a later page.
        let front = doc.content[0].id();
        let body = doc.content[60].id();
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

        let pages = lay_out(&doc, &MONO, &NoHyphenator);
        let starts = section_starts(&doc, &pages);
        let body_start = starts[1].expect("the body section is placed");
        assert!(
            body_start > 0 && body_start < pages.len(),
            "want the body to open on a later page, got {body_start} of {}",
            pages.len()
        );

        // The front matter counts in lower roman from i.
        for (n, page) in pages.iter().take(body_start).enumerate() {
            let want = NumberFormat::LowerRoman.format(n as u32 + 1);
            assert_eq!(static_text(page), want, "front matter page {}", page.index);
        }
        // And the body restarts at 1 — the assertion an offset cannot satisfy.
        for (n, page) in pages.iter().skip(body_start).enumerate() {
            assert_eq!(
                static_text(page),
                (n + 1).to_string(),
                "body page {}",
                page.index
            );
        }
    }

    /// The other half, and the one that protects every existing document: a document that states no
    /// section prints exactly what it printed before this spec — arabic, one-based, on every page.
    #[test]
    fn a_document_with_no_sections_prints_the_folios_it_always_did() {
        let doc = doc_with_folio(StaticAlign::Left, false, PAGE_TOKEN);
        assert!(doc.sections.is_empty());
        let pages = lay_out(&doc, &MONO, &NoHyphenator);
        assert!(pages.len() >= 3, "want a multi-page document");
        for page in &pages {
            assert_eq!(static_text(page), (page.index + 1).to_string());
        }
    }

    /// What a folio format costs the fixpoint, asserted rather than assumed — and the first draft of
    /// this test asserted the wrong thing, which is worth writing down.
    ///
    /// "A folio is furniture, so it consumes no flow space, so it cannot cost an iteration" conflates
    /// two claims. It cannot move a **line break** — true, and it is why this converges instead of
    /// oscillating. But a section's start page is not known until its anchor has been *placed*, so
    /// pass 1 necessarily prints the default folios and pass 2 is what prints the section's. The
    /// second pass is resolution, not instability, and no third is needed because the corrected
    /// folios move nothing.
    ///
    /// So the real invariant is a floor and a ceiling: a document with no sections still costs
    /// exactly one pass, and adding a folio-only section costs exactly one more — never two.
    #[test]
    fn a_folio_format_costs_the_fixpoint_exactly_one_resolving_pass() {
        use quill_core_model::{Folio, NumberFormat, Section};

        let plain = doc_with_folio(StaticAlign::Left, false, PAGE_TOKEN);
        let base = lay_out_with_fixpoint_status(
            &plain.content,
            &plain.assets,
            &plain.styles,
            &DocumentTemplate::new(&plain),
            &MONO,
            &NoHyphenator,
        )
        .1;

        let mut sectioned = plain.clone();
        sectioned.sections = vec![Section {
            name: "Front matter".into(),
            start: sectioned.content[0].id(),
            master: None,
            folio: Some(Folio {
                format: NumberFormat::UpperRoman,
                restart_at: Some(1),
            }),
        }];
        let with = lay_out_with_fixpoint_status(
            &sectioned.content,
            &sectioned.assets,
            &sectioned.styles,
            &DocumentTemplate::new(&sectioned),
            &MONO,
            &NoHyphenator,
        )
        .1;

        assert!(base.converged && with.converged);
        assert_eq!(
            base.iterations, 1,
            "a document that derives nothing still takes exactly one pass"
        );
        assert_eq!(
            with.iterations,
            base.iterations + 1,
            "one pass to place the anchor, one to print the folios that follow from it — and no \
             more, because a folio moves nothing"
        );
    }

    // --- Running heads derived from content (spec 0074) -----------------------------------------

    /// `doc_with_folio`'s master, over content that is `chapters` chapters of `lines` body blocks
    /// each — enough for a chapter boundary to fall inside the document rather than at its edge.
    fn doc_with_running_head(text: &str, chapters: &[&str], lines: usize) -> Document {
        let mut content = Vec::new();
        for (c, name) in chapters.iter().enumerate() {
            content.push(Block::heading(1, *name, Color::Gray { v: 0.0 }));
            content.extend(
                (0..lines).map(|i| Block::body(format!("c{c}l{i}"), Color::Gray { v: 0.0 })),
            );
        }
        let mut doc = doc_with_folio(StaticAlign::Left, false, text);
        doc.content = content;
        doc.next_block_id = 0;
        doc.assign_missing_block_ids().expect("fresh blocks");
        doc
    }

    /// The headline of spec 0074, asserted on the **placed static text**: a running head naming the
    /// current chapter says one thing before the chapter boundary and another after it.
    ///
    /// Both halves are asserted for spec 0072's reason. A test that only checked the second chapter's
    /// pages would pass against an implementation that printed the *last* heading of the document
    /// everywhere, and one that only checked the first would pass against one that never updated.
    #[test]
    fn a_running_head_names_the_current_chapter_and_changes_at_the_boundary() {
        let doc = doc_with_running_head("{heading:1}", &["The Ruined Keep", "The Deep Road"], 60);
        let pages = lay_out(&doc, &MONO, &NoHyphenator);

        let index = heading_index_of(&doc.content, &pages);
        assert_eq!(index.len(), 2, "two chapters");
        let second = index[1].page_index;
        assert!(
            second > 0 && second < pages.len(),
            "want the second chapter to open inside the document, got {second} of {}",
            pages.len()
        );

        for page in &pages {
            let want = if page.index < second {
                "The Ruined Keep"
            } else {
                "The Deep Road"
            };
            assert_eq!(static_text(page), want, "page {}", page.index);
        }
    }

    /// `{section}` and `{heading:1}` are different questions, so the fixture makes them disagree:
    /// the section is anchored to a **body** block a third of the way into chapter one, so its start
    /// page is not the chapter's start page and neither token can be mistaken for the other.
    ///
    /// It also pins the two edges the mechanism has. A page ahead of every section belongs to no
    /// section and prints nothing for `{section}` — it does not borrow the first section's name — and
    /// `{heading:2}` follows a sub-heading that `{heading:1}` ignores.
    #[test]
    fn a_section_name_and_a_chapter_name_are_two_different_strings() {
        use quill_core_model::Section;

        let mut doc = doc_with_running_head(
            "[{section}][{heading:1}][{heading:2}]",
            &["The Ruined Keep"],
            120,
        );
        // A sub-heading well inside the chapter, and a section anchored to a body block after it.
        // Neither coincides with the chapter's own start.
        doc.content.insert(
            60,
            Block::heading(2, "The Undercroft", Color::Gray { v: 0.0 }),
        );
        doc.assign_missing_block_ids()
            .expect("the inserted heading");
        let anchor = doc.content[90].id();
        doc.sections = vec![Section {
            name: "Part One".into(),
            start: anchor,
            master: None,
            folio: None,
        }];

        let pages = lay_out(&doc, &MONO, &NoHyphenator);
        let index = heading_index_of(&doc.content, &pages);
        let chapter = index[0].page_index;
        let sub = index[1].page_index;
        let section = section_starts(&doc, &pages)[0].expect("the section is placed");
        assert!(
            chapter < sub && sub < section && section < pages.len(),
            "the fixture must separate all three: chapter {chapter}, sub-heading {sub}, section \
             {section}, of {} pages",
            pages.len()
        );

        for page in &pages {
            let want = format!(
                "[{}][{}][{}]",
                if page.index >= section {
                    "Part One"
                } else {
                    ""
                },
                if page.index >= chapter {
                    "The Ruined Keep"
                } else {
                    ""
                },
                if page.index >= sub {
                    "The Undercroft"
                } else if page.index >= chapter {
                    "The Ruined Keep"
                } else {
                    ""
                },
            );
            assert_eq!(static_text(page), want, "page {}", page.index);
        }
        // And the fixture really does exercise the disagreement, rather than asserting a formula
        // against itself: at least one page prints a section name and a different chapter name.
        assert!(
            pages
                .iter()
                .any(|p| static_text(p).starts_with("[Part One][The Ruined Keep]")),
            "want a page where the two tokens print different strings"
        );
    }

    /// What a content-derived running head costs the fixpoint, measured rather than assumed.
    ///
    /// Spec 0073 corrected the M6 audit here once already: "furniture consumes no flow space" proves
    /// a folio cannot move a **line break**, but a *section's* start page is not known until its
    /// anchor is placed, so a folio still costs one resolving pass. The honest question this
    /// increment owes is whether a heading-derived running head costs another, and it does not — the
    /// heading index is derived from the finished page vector by a post-pass, so it is resolved
    /// against pages that already exist.
    ///
    /// Floor and ceiling, in `a_folio_format_costs_the_fixpoint_exactly_one_resolving_pass`'s style:
    /// a document with a `{heading:1}` running head and no sections takes exactly the one pass a
    /// document with no tokens at all takes, and adding a section takes exactly the one more that
    /// section already cost before this increment.
    #[test]
    fn a_content_derived_running_head_costs_the_fixpoint_no_extra_pass() {
        use quill_core_model::Section;

        let passes = |doc: &Document| {
            lay_out_with_fixpoint_status(
                &doc.content,
                &doc.assets,
                &doc.styles,
                &DocumentTemplate::new(doc),
                &MONO,
                &NoHyphenator,
            )
            .1
        };

        let plain = doc_with_running_head("Chapter", &["The Ruined Keep", "The Deep Road"], 60);
        let derived = doc_with_running_head(
            "{section} {heading:1}",
            &["The Ruined Keep", "The Deep Road"],
            60,
        );
        let mut sectioned = derived.clone();
        sectioned.sections = vec![Section {
            name: "Part One".into(),
            start: sectioned.content[0].id(),
            master: None,
            folio: None,
        }];

        let (base, with_tokens, with_section) =
            (passes(&plain), passes(&derived), passes(&sectioned));
        assert!(base.converged && with_tokens.converged && with_section.converged);
        assert_eq!(base.iterations, 1, "a literal running head derives nothing");
        assert_eq!(
            with_tokens.iterations, 1,
            "a content-derived running head is resolved after the flow, so it costs zero further \
             passes — this is the audit's 'not a fixpoint', asserted"
        );
        assert_eq!(
            with_section.iterations, 2,
            "a section still costs its one resolving pass (spec 0073), and the running head adds \
             nothing to it"
        );
    }

    /// A document that uses none of spec 0074's tokens must lay out exactly as it did before, and
    /// "exactly" is asserted against the rule that shipped rather than against a golden captured
    /// after the change: every static resolves to its text with `{page}` replaced by the page's
    /// folio, and nothing else.
    ///
    /// The export half of the same claim is `SAMPLE_EXPORT_DIGEST`, which did not move.
    #[test]
    fn a_document_using_no_new_token_resolves_exactly_as_before() {
        for text in [
            "Folio {page}",
            "The Ruined Keep",
            "{page}",
            "{unknown} {page}",
        ] {
            let doc = doc_with_folio(StaticAlign::Left, false, text);
            let pages = lay_out(&doc, &MONO, &NoHyphenator);
            assert!(pages.len() >= 3);
            for page in &pages {
                assert_eq!(
                    static_text(page),
                    text.replace(PAGE_TOKEN, &(page.index + 1).to_string()),
                    "page {}",
                    page.index
                );
            }
        }
    }

    /// The x of a page's first static.
    fn static_x(page: &LaidOutPage) -> f32 {
        match &page.statics[0] {
            PlacedBlock::Text { frame, .. } => frame.x_pt,
            other => panic!("expected a text static, got {other:?}"),
        }
    }

    #[test]
    fn a_centred_running_head_is_centred_in_its_rect() {
        // 8 characters at 0.6 em x 10 pt = 48 pt in a 338 pt band: 54 + (338 - 48) / 2 = 199.
        let doc = doc_with_folio(StaticAlign::Center, false, "The Keep");
        let pages = lay_out(&doc, &MONO, &NoHyphenator);
        assert!(pages.len() >= 3, "want a three-page document");
        for page in pages.iter().take(3) {
            assert!(
                (static_x(page) - 199.0).abs() < 0.01,
                "page {}: centred head at {}",
                page.index,
                static_x(page)
            );
        }
    }

    #[test]
    fn a_right_aligned_static_is_flush_to_its_rects_right_edge() {
        // 54 + 338 - 48 = 344.
        let doc = doc_with_folio(StaticAlign::Right, false, "The Keep");
        let pages = lay_out(&doc, &MONO, &NoHyphenator);
        assert!((static_x(&pages[0]) - 344.0).abs() < 0.01);
        assert!(
            (static_x(&pages[1]) - 344.0).abs() < 0.01,
            "an unmirrored `right` is the same edge on both halves of the spread"
        );
    }

    #[test]
    fn an_outside_folio_sits_at_opposite_corners_on_a_recto_and_a_verso() {
        // The defect this increment exists to fix, end to end. "1" and "2" are one 6 pt character.
        let doc = doc_with_folio(StaticAlign::Outside, true, PAGE_TOKEN);
        let pages = lay_out(&doc, &MONO, &NoHyphenator);
        let (recto, verso) = (static_x(&pages[0]), static_x(&pages[1]));
        // Recto: flush to the fore-edge margin on the right — 432 - 40 - 6 = 386.
        assert!((recto - 386.0).abs() < 0.01, "recto folio at {recto}");
        // Verso: the rect mirrors, so the fore-edge is on the left — x = 40.
        assert!((verso - 40.0).abs() < 0.01, "verso folio at {verso}");
        assert_ne!(recto, verso, "a spread must not repeat one corner");
        // Both clear the trim by at least the smaller side margin, which is the press rule the
        // spec-0036 inset was standing in for.
        for (page, x) in [(0, recto), (1, verso)] {
            assert!(x >= 40.0, "page {page} folio at {x} is inside the margin");
            assert!(x + 6.0 <= 392.0, "page {page} folio runs past the margin");
        }
    }

    #[test]
    fn a_single_sided_document_never_mirrors_a_static() {
        let mut doc = doc_with_folio(StaticAlign::Outside, true, PAGE_TOKEN);
        doc.page_setup.facing_pages = false;
        let pages = lay_out(&doc, &MONO, &NoHyphenator);
        assert!((static_x(&pages[0]) - static_x(&pages[1])).abs() < 0.01);
    }

    #[test]
    fn a_user_template_naming_a_missing_style_still_lays_the_page_out() {
        // Spec 0053. A user-authored template is far likelier than a bundled one to name a style
        // its own stylesheet does not carry — a renamed `folio`, a `sidebar` copied from another
        // book. The repo's authoring posture is that losing the *styling* beats losing the page, so
        // this asserts the fallback end to end through the real engine rather than through
        // `StyleSheet::resolve` alone: both a flowed block and a master static name styles that do
        // not exist, and both still come out on the page with their text intact.
        let template = quill_core_model::Template::from_json(
            r#"{
                "template_version": 1,
                "name": "house", "title": "House", "description": "names styles it does not have",
                "page_setup": {"trim": {"w_pt": 432.0, "h_pt": 648.0}, "bleed_pt": 9.0,
                               "facing_pages": true,
                               "margins": {"top_pt": 54.0, "bottom_pt": 54.0,
                                           "inside_pt": 54.0, "outside_pt": 40.0}},
                "styles": {"paragraph": {"body": {"font_size_pt": 10.0, "leading_pt": 12.0}}},
                "master_pages": [
                  {"name": "body", "columns": 1,
                   "statics": [{"kind": "text",
                                "rect": {"x_pt": 54.0, "y_pt": 606.0, "w_pt": 338.0, "h_pt": 12.0},
                                "text": "{page}", "color": {"space": "gray", "v": 0.0},
                                "style": "no-such-folio-style"}]}],
                "default_master": "body"
            }"#,
        )
        .expect("the template must load");
        assert!(
            !template
                .styles
                .paragraph
                .contains_key("no-such-folio-style"),
            "the fixture must actually be missing the style it names"
        );

        let mut doc = Document::from_template(&template);
        doc.content.push(Block::Body {
            id: quill_core_model::BlockId::UNASSIGNED,
            runs: vec![quill_core_model::Run::plain(
                "A dank corridor stretches into darkness.",
            )],
            color: Color::Gray { v: 0.0 },
            style: Some("no-such-body-style".into()),
        });
        doc.assign_missing_block_ids().expect("ids");

        let pages = lay_out(&doc, &MONO, &NoHyphenator);
        assert!(!pages.is_empty(), "a missing style must not lose the page");
        assert_eq!(
            first_text_lines(&pages)[0].text,
            "A dank corridor stretches into darkness."
        );
        // The furniture survives too, set in the default treatment rather than dropped.
        match &pages[0].statics[0] {
            PlacedBlock::Text { lines, .. } => assert_eq!(lines[0].text, "1"),
            other => panic!("expected the folio, got {other:?}"),
        }
    }

    #[test]
    fn a_placed_static_reports_the_line_it_holds_not_the_band_it_was_aligned_in() {
        // The frame is narrowed to the measured line, so a preflight over placed geometry
        // (spec 0050) sees where the ink is rather than the whole 338 pt band.
        let doc = doc_with_folio(StaticAlign::Center, false, "The Keep");
        let pages = lay_out(&doc, &MONO, &NoHyphenator);
        match &pages[0].statics[0] {
            PlacedBlock::Text { frame, .. } => {
                assert!((frame.w_pt - 48.0).abs() < 0.01, "width {}", frame.w_pt);
                assert_eq!(frame.y_pt, 606.0, "the vertical band is untouched");
                assert_eq!(frame.h_pt, 12.0);
            }
            other => panic!("expected a text static, got {other:?}"),
        }
    }

    #[test]
    fn ordered_items_number_in_document_order_and_renumber_on_insert() {
        use quill_core_model::LIST_NUMBER_STYLE;
        let styles = StyleSheet::default();
        let item = |t: &str| {
            let mut b = Block::body(t, Color::Gray { v: 0.0 });
            if let Block::Body { style, .. } = &mut b {
                *style = Some(LIST_NUMBER_STYLE.to_string());
            }
            b
        };
        let mut content = vec![item("alpha"), item("beta"), item("gamma")];
        for (i, b) in content.iter_mut().enumerate() {
            b.set_id(BlockId(i as u64 + 1));
        }
        let markers = list_markers(&content, &styles);
        let of = |id: u64| markers.get(&BlockId(id)).cloned().unwrap_or_default();
        assert_eq!(
            (of(1), of(2), of(3)),
            ("1.".into(), "2.".into(), "3.".into())
        );

        // Insert at the top: every marker below it moves, and none of their text changed. This is
        // why the ordinal is in the measure key and not derived from the block alone.
        let mut inserted = content.clone();
        inserted.insert(0, item("new first"));
        inserted[0].set_id(BlockId(99));
        let m2 = list_markers(&inserted, &styles);
        assert_eq!(m2.get(&BlockId(99)).unwrap(), "1.");
        assert_eq!(m2.get(&BlockId(1)).unwrap(), "2.");
        assert_eq!(m2.get(&BlockId(3)).unwrap(), "4.");
    }

    #[test]
    fn a_paragraph_between_two_lists_makes_them_two_lists() {
        use quill_core_model::LIST_NUMBER_STYLE;
        let styles = StyleSheet::default();
        let item = |t: &str| {
            let mut b = Block::body(t, Color::Gray { v: 0.0 });
            if let Block::Body { style, .. } = &mut b {
                *style = Some(LIST_NUMBER_STYLE.to_string());
            }
            b
        };
        let mut content = vec![
            item("a"),
            item("b"),
            Block::body("an interruption", Color::Gray { v: 0.0 }),
            item("c"),
        ];
        for (i, b) in content.iter_mut().enumerate() {
            b.set_id(BlockId(i as u64 + 1));
        }
        let markers = list_markers(&content, &styles);
        assert_eq!(markers.get(&BlockId(1)).unwrap(), "1.");
        assert_eq!(markers.get(&BlockId(2)).unwrap(), "2.");
        assert!(!markers.contains_key(&BlockId(3)), "not a list item");
        assert_eq!(
            markers.get(&BlockId(4)).unwrap(),
            "1.",
            "an interrupted list restarts"
        );
    }

    #[test]
    fn nested_levels_count_independently_and_a_shallower_item_closes_them() {
        use quill_core_model::{ListMarker, ListSpec, NumberFormat};
        let mut styles = StyleSheet::default();
        for (name, level, format) in [
            ("l0", 0u8, NumberFormat::Decimal),
            ("l1", 1, NumberFormat::LowerAlpha),
        ] {
            styles.paragraph.insert(
                name.to_string(),
                ParagraphStyle {
                    list: Some(ListSpec {
                        marker: ListMarker::Number {
                            format,
                            suffix: Some('.'),
                        },
                        start: 1,
                        level,
                    }),
                    ..ParagraphStyle::default()
                },
            );
        }
        let item = |t: &str, s: &str| {
            let mut b = Block::body(t, Color::Gray { v: 0.0 });
            if let Block::Body { style, .. } = &mut b {
                *style = Some(s.to_string());
            }
            b
        };
        // 1. / a. / b. / 2. / a.  — the inner list restarts under the second outer item.
        let mut content = vec![
            item("one", "l0"),
            item("inner a", "l1"),
            item("inner b", "l1"),
            item("two", "l0"),
            item("inner again", "l1"),
        ];
        for (i, b) in content.iter_mut().enumerate() {
            b.set_id(BlockId(i as u64 + 1));
        }
        let m = list_markers(&content, &styles);
        let got: Vec<String> = (1..=5).map(|i| m[&BlockId(i)].clone()).collect();
        assert_eq!(got, ["1.", "a.", "b.", "2.", "a."]);
    }

    #[test]
    fn number_formats_are_right_at_their_boundaries() {
        use quill_core_model::NumberFormat::*;
        // Bijective base-26 rather than ordinary base-26: 27 is `aa`, not `ba`.
        assert_eq!(LowerAlpha.format(1), "a");
        assert_eq!(LowerAlpha.format(26), "z");
        assert_eq!(LowerAlpha.format(27), "aa");
        assert_eq!(UpperAlpha.format(28), "AB");
        // The subtractive forms are the ones a naive table gets wrong.
        assert_eq!(LowerRoman.format(4), "iv");
        assert_eq!(LowerRoman.format(9), "ix");
        assert_eq!(UpperRoman.format(1994), "MCMXCIV");
        // Above the standard range, decimal rather than something a reader must decode.
        assert_eq!(UpperRoman.format(4000), "4000");
    }

    #[test]
    fn a_list_marker_is_drawn_in_the_gutter_and_not_repeated_on_a_continuation() {
        use quill_core_model::LIST_NUMBER_STYLE;
        // The marker sits at the frame's left edge; the text is inset by the hanging indent, so a
        // wrapped line lines up under the text rather than under the marker.
        let mut b = Block::body(
            "a list item long enough to wrap onto a second line in a narrow frame indeed",
            Color::Gray { v: 0.0 },
        );
        if let Block::Body { style, .. } = &mut b {
            *style = Some(LIST_NUMBER_STYLE.to_string());
        }
        let mut content = vec![b];
        content[0].set_id(BlockId(1));
        let assets: Vec<quill_core_model::Asset> = Vec::new();
        let pages = lay_out_in_frame(
            &content,
            &assets,
            &StyleSheet::default(),
            &Frame {
                rect: Rect {
                    x_pt: 0.0,
                    y_pt: 0.0,
                    w_pt: 160.0,
                    h_pt: 400.0,
                },
            },
            &MONO,
            &NoHyphenator,
        );
        let texts: Vec<(&str, f32)> = pages[0]
            .blocks
            .iter()
            .filter_map(|b| match b {
                PlacedBlock::Text { lines, frame, .. } => {
                    Some((lines[0].text.as_str(), frame.x_pt + lines[0].indent_pt))
                }
                _ => None,
            })
            .collect();
        assert_eq!(texts[0].0, "1.", "the marker is placed first");
        assert_eq!(texts[0].1, 0.0, "and sits at the frame's left edge");
        assert!(
            texts[1].1 > texts[0].1,
            "the item's text is inset past the marker: {texts:?}"
        );
    }

    #[test]
    fn a_migrated_v2_document_lays_out_where_it_did_before_the_bump() {
        // Migration correctness as *geometry*, not as "it parses". The fixture is committed bytes
        // (`crates/core-model/assets/v2-masters.json`) written by a build that predates spec 0047,
        // and the numbers below are what that build placed: a static drawn from its rect's left
        // edge, in the same rect on both halves of the spread, inside a text frame whose margins
        // mirror.
        const V2: &str = include_str!("../../core-model/assets/v2-masters.json");
        let doc = Document::from_json(V2).expect("the v2 fixture must load");

        // The recto goes through the whole flow, so what the template says and what a laid-out page
        // gets cannot disagree; the verso is asked of the template directly, because the fixture is
        // deliberately one short paragraph — a migration fixture should be readable, not padded to
        // two pages.
        let pages = lay_out(&doc, &MONO, &NoHyphenator);
        let template = DocumentTemplate::new(&doc);
        match &pages[0].blocks[0] {
            PlacedBlock::Text { frame, lines, .. } => {
                // The paragraph starts at the migrated frame's top-left, exactly where the v2 build
                // put it. Its *width* is the ink it draws (spec 0069) rather than the 338 pt
                // measure it was broken to — that measure is asserted from the template below, and
                // one short paragraph does not fill it.
                assert_eq!((frame.x_pt, frame.y_pt), (54.0, 54.0));
                assert_eq!(lines.len(), 1, "the fixture is one short paragraph");
                assert_eq!(
                    frame.w_pt,
                    MONO.measure_run(&lines[0].text, BODY_FONT_SIZE_PT)
                );
                assert!(frame.w_pt <= 338.0);
            }
            other => panic!("expected text, got {other:?}"),
        }
        assert_eq!(
            pages[0].statics,
            template.statics(0, &MONO, &StaticContext::EMPTY)
        );

        for (page, expected_left) in [(0usize, 54.0f32), (1, 40.0)] {
            // The text frame: margins mirror across the spread, as they did in v2.
            let frames = template.frames(page);
            assert_eq!(frames.len(), 1);
            assert_eq!(frames[0].rect.x_pt, expected_left, "page {page} text frame");
            assert_eq!(frames[0].rect.y_pt, 54.0);
            assert_eq!(frames[0].rect.w_pt, 338.0);

            // The folio: unmirrored and left-aligned, so x is the authored 40 on *both* pages —
            // spec 0036's fore-edge inset, preserved exactly.
            let statics = template.statics(page, &MONO, &StaticContext::EMPTY);
            match &statics[0] {
                PlacedBlock::Text { frame, lines, .. } => {
                    assert_eq!(frame.x_pt, 40.0, "page {page} folio");
                    assert_eq!(frame.y_pt, 606.0);
                    assert_eq!(lines[0].text, format!("{}", page + 1));
                }
                other => panic!("expected a text static, got {other:?}"),
            }
            // And the background image static is placed full-bleed, unmoved by parity.
            match &statics[1] {
                PlacedBlock::Image {
                    frame, asset_id, ..
                } => {
                    assert_eq!(asset_id, "bg");
                    assert_eq!((frame.x_pt, frame.w_pt), (0.0, 432.0));
                }
                other => panic!("expected an image static, got {other:?}"),
            }
        }
    }

    /// Spec 0064, and the third face of the same defect: a paragraph *style* that is bold must reach
    /// the breaker.
    ///
    /// `run_formats` is empty when every run resolves to the breaker's own default — regular,
    /// upright, untracked. It is *not* empty merely because every run matches the paragraph: a bold
    /// paragraph style with plain runs still has a format the breaker has to be told about, or the
    /// line is measured regular and drawn bold, and a line drawn wider than it was measured is
    /// exactly what spec 0060 forbids.
    #[test]
    fn a_bold_paragraph_style_reaches_the_breaker() {
        use quill_core_model::{Run, Weight};

        let plain = quill_core_model::ParagraphStyle::default();
        let bold = quill_core_model::ParagraphStyle {
            weight: Weight::BOLD,
            ..plain
        };
        let runs = vec![Run::plain(
            "the vault door had not opened in three hundred years",
        )];

        assert!(
            run_formats(&runs, &plain, &StyleSheet::default()).is_empty(),
            "a regular paragraph of plain runs takes the pre-family path"
        );
        let formats = run_formats(&runs, &bold, &StyleSheet::default());
        assert_eq!(
            formats,
            vec![RunFormat {
                size_pt: bold.font_size_pt,
                weight: 700,
                italic: false,
                tracking_pt: 0.0,
            }],
            "a bold paragraph style must be spelled out for the breaker"
        );

        // That the bold format then *measures* wider is `quill-fonts`' claim, asserted there
        // (`a_run_measures_in_the_face_it_names`) — this crate is generic over `RunMetrics` and
        // names no font, which is the property that keeps it testable with a monospace stub.
    }

    /// Spec 0065's precedence, one field at a time: paragraph < character style < inline override.
    ///
    /// Exhaustive by field rather than by example, because the failure this guards against is
    /// exactly a field that resolves in the wrong order while the others look right — and because
    /// "the override wins" is not the rule. The rule is that it wins *for the field it names*.
    #[test]
    fn precedence_is_paragraph_then_character_style_then_override_field_by_field() {
        use quill_core_model::{CharacterStyle, InlineStyle, Run, Weight};

        let mut styles = StyleSheet::default();
        styles.character.insert(
            "named".into(),
            CharacterStyle {
                size_pt: Some(12.0),
                color: Some(Color::Gray { v: 0.5 }),
                tracking_pt: Some(1.0),
                baseline_shift_pt: Some(2.0),
                weight: Some(Weight::BOLD),
                italic: Some(true),
            },
        );
        let paragraph = quill_core_model::ParagraphStyle {
            font_size_pt: 10.0,
            ..quill_core_model::ParagraphStyle::default()
        };
        let ink = Color::Gray { v: 0.0 };

        let run = |style: InlineStyle, named: bool| Run {
            text: "x".into(),
            style,
            character: named.then(|| "named".to_string()),
            source: Default::default(),
            index: None,
        };
        // An empty result means "the breaker's own default", which is the contract `run_formats`
        // documents — so the helper spells that out rather than indexing into nothing.
        let fmt = |r: &Run| {
            run_formats(std::slice::from_ref(r), &paragraph, &styles)
                .first()
                .copied()
                .unwrap_or(RunFormat::plain(paragraph.font_size_pt))
        };
        let shift = |r: &Run| {
            run_shifts(std::slice::from_ref(r), &styles)
                .first()
                .copied()
                .unwrap_or(0.0)
        };
        let colour = |r: &Run| run_inks(std::slice::from_ref(r), ink, &styles)[0];

        // Layer 1 only: the paragraph.
        let plain = run(InlineStyle::EMPTY, false);
        assert_eq!(fmt(&plain).size_pt, 10.0);
        assert_eq!(fmt(&plain).weight, 400);
        assert!(!fmt(&plain).italic);
        assert_eq!(fmt(&plain).tracking_pt, 0.0);
        assert_eq!(shift(&plain), 0.0);
        assert_eq!(colour(&plain), ink);

        // Layer 2: the named style fills every field the paragraph left.
        let named = run(InlineStyle::EMPTY, true);
        assert_eq!(fmt(&named).size_pt, 12.0);
        assert_eq!(fmt(&named).weight, 700);
        assert!(fmt(&named).italic);
        assert_eq!(fmt(&named).tracking_pt, 1.0);
        assert_eq!(shift(&named), 2.0);
        assert_eq!(colour(&named), Color::Gray { v: 0.5 });

        // Layer 3, field by field: each override displaces its own field and *only* its own.
        let over_size = run(
            InlineStyle {
                size_pt: Some(14.0),
                ..InlineStyle::EMPTY
            },
            true,
        );
        assert_eq!(fmt(&over_size).size_pt, 14.0);
        assert_eq!(fmt(&over_size).weight, 700, "the style's weight survives");
        assert_eq!(fmt(&over_size).tracking_pt, 1.0);

        let over_weight = run(
            InlineStyle {
                weight: Some(Weight::REGULAR),
                ..InlineStyle::EMPTY
            },
            true,
        );
        assert_eq!(fmt(&over_weight).weight, 400);
        assert_eq!(fmt(&over_weight).size_pt, 12.0, "the style's size survives");

        let over_italic = run(
            InlineStyle {
                italic: Some(false),
                ..InlineStyle::EMPTY
            },
            true,
        );
        assert!(!fmt(&over_italic).italic);
        assert!(fmt(&over_italic).weight == 700);

        let over_tracking = run(
            InlineStyle {
                tracking_pt: Some(0.25),
                ..InlineStyle::EMPTY
            },
            true,
        );
        assert_eq!(fmt(&over_tracking).tracking_pt, 0.25);

        let over_shift = run(
            InlineStyle {
                baseline_shift_pt: Some(-1.0),
                ..InlineStyle::EMPTY
            },
            true,
        );
        assert_eq!(shift(&over_shift), -1.0);

        let over_colour = run(
            InlineStyle {
                color: Some(Color::Gray { v: 0.9 }),
                ..InlineStyle::EMPTY
            },
            true,
        );
        assert_eq!(colour(&over_colour), Color::Gray { v: 0.9 });
        assert_eq!(fmt(&over_colour).weight, 700, "recolouring stays bold");
    }

    /// A run naming a style nothing defines lays out with the paragraph's treatment, and its own
    /// overrides still apply. Losing text because a style was renamed — or because the pack that
    /// defined it is missing — would be far worse than setting it plainly.
    #[test]
    fn an_unknown_character_style_falls_back_without_losing_the_overrides() {
        use quill_core_model::{InlineStyle, Run, Weight};

        let styles = StyleSheet::default();
        let paragraph = quill_core_model::ParagraphStyle::default();
        let run = Run {
            text: "x".into(),
            style: InlineStyle {
                weight: Some(Weight::BOLD),
                ..InlineStyle::EMPTY
            },
            character: Some("no-such-style".into()),
            source: Default::default(),
            index: None,
        };
        let fmt = run_formats(std::slice::from_ref(&run), &paragraph, &styles)[0];
        assert_eq!(fmt.size_pt, paragraph.font_size_pt, "the paragraph's size");
        assert_eq!(fmt.weight, 700, "the run's own override still applies");
    }

    /// The four built-in names resolve without any document authoring them — which is what lets an
    /// imported document be *styled* rather than overridden.
    #[test]
    fn the_built_in_character_styles_resolve_out_of_the_box() {
        use quill_core_model::{InlineStyle, Run};

        let styles = StyleSheet::default();
        assert!(
            styles.character.is_empty(),
            "the built-ins are resolved, not written into every document"
        );
        let paragraph = quill_core_model::ParagraphStyle::default();
        let of = |name: &str| {
            let run = Run {
                text: "x".into(),
                style: InlineStyle::EMPTY,
                character: Some(name.into()),
                source: Default::default(),
                index: None,
            };
            run_formats(std::slice::from_ref(&run), &paragraph, &styles)[0]
        };
        assert!(of(quill_core_model::EMPHASIS_STYLE).italic);
        assert_eq!(of(quill_core_model::EMPHASIS_STYLE).weight, 400);
        assert_eq!(of(quill_core_model::STRONG_STYLE).weight, 700);
        assert!(!of(quill_core_model::STRONG_STYLE).italic);
        let both = of(quill_core_model::STRONG_EMPHASIS_STYLE);
        assert!(both.italic && both.weight == 700);
        assert_eq!(of(quill_core_model::LEAD_IN_STYLE).weight, 700);
    }

    /// A document may redefine a built-in, and its own definition wins.
    #[test]
    fn a_document_can_redefine_a_built_in_style() {
        use quill_core_model::{CharacterStyle, InlineStyle, Run};

        let mut styles = StyleSheet::default();
        styles.character.insert(
            quill_core_model::EMPHASIS_STYLE.into(),
            CharacterStyle {
                tracking_pt: Some(0.5),
                ..CharacterStyle::EMPTY
            },
        );
        let run = Run {
            text: "x".into(),
            style: InlineStyle::EMPTY,
            character: Some(quill_core_model::EMPHASIS_STYLE.into()),
            source: Default::default(),
            index: None,
        };
        let fmt = run_formats(
            std::slice::from_ref(&run),
            &quill_core_model::ParagraphStyle::default(),
            &styles,
        )[0];
        assert_eq!(fmt.tracking_pt, 0.5);
        assert!(
            !fmt.italic,
            "the house definition replaces the built-in rather than merging with it"
        );
    }

    // --- Sections (spec 0072) -----------------------------------------------------------------

    /// The two masters a chapter opener needs: a deep top margin on the opener, an ordinary one on
    /// the body. Deliberately differing in *margins* rather than in columns, because the margin is
    /// what changes a page's capacity and therefore what puts the assignment in the fixpoint.
    fn opener_and_body() -> Vec<MasterPage> {
        vec![
            MasterPage {
                name: "chapter-opener".into(),
                margins: Some(Margins {
                    top_pt: 216.0,
                    bottom_pt: 54.0,
                    inside_pt: 54.0,
                    outside_pt: 54.0,
                }),
                columns: 1,
                gutter_pt: 0.0,
                statics: Vec::new(),
            },
            MasterPage {
                name: "body".into(),
                margins: Some(Margins {
                    top_pt: 54.0,
                    bottom_pt: 54.0,
                    inside_pt: 54.0,
                    outside_pt: 54.0,
                }),
                columns: 1,
                gutter_pt: 0.0,
                statics: Vec::new(),
            },
        ]
    }

    /// `filler` one-line paragraphs, then the block a section is anchored to, then a little more.
    ///
    /// The anchor is a **body** block, not a heading: a section anchors to a `BlockId`, and nothing
    /// about the mechanism is specific to headings. Anchoring the fixture to a paragraph is what
    /// asserts that.
    fn sectioned_doc(filler: usize) -> Document {
        let mut content = many_lines(filler);
        content.push(Block::body(
            "the chapter opens here",
            Color::Gray { v: 0.0 },
        ));
        content.extend(many_lines(4));
        let mut doc = doc_with_blocks(content);
        doc.master_pages = opener_and_body();
        doc.default_master = Some("body".into());
        doc.assign_missing_block_ids().expect("ids");
        let anchor = doc.content[filler].id();
        doc.sections = vec![quill_core_model::Section {
            name: "Chapter One".into(),
            start: anchor,
            master: Some("chapter-opener".into()),
            folio: None,
        }];
        doc
    }

    /// Lay a document out through its own template, keeping the template so its settled derivation
    /// can be asked about afterwards.
    fn lay_out_sectioned(doc: &Document) -> (Vec<LaidOutPage>, FixpointStatus, Vec<Option<usize>>) {
        let template = DocumentTemplate::new(doc);
        let (pages, status) = lay_out_with_fixpoint_status(
            &doc.content,
            &doc.assets,
            &doc.styles,
            &template,
            &MONO,
            &NoHyphenator,
        );
        let starts = section_starts(doc, &pages);
        (pages, status, starts)
    }

    /// The y the first block on `page` was placed at — where that page's text frame begins.
    fn top_of(pages: &[LaidOutPage], page: usize) -> f32 {
        match &pages[page].blocks[0] {
            PlacedBlock::Text { frame, .. } => frame.y_pt,
            other => panic!("expected text on page {page}, got {other:?}"),
        }
    }

    #[test]
    fn a_section_puts_its_master_on_the_page_its_anchor_landed_on() {
        let doc = sectioned_doc(50);
        let (pages, status, starts) = lay_out_sectioned(&doc);
        assert!(status.converged, "must settle: {status:?}");
        let start = starts[0].expect("the anchor is in the document, so it was placed");
        assert!(
            start > 0,
            "the fixture exists to put the chapter past page 0, got {start}"
        );
        assert_eq!(top_of(&pages, start), 216.0, "the opener's top margin");
        assert_eq!(top_of(&pages, 0), 54.0, "and every other page is body");
    }

    #[test]
    fn a_section_survives_repagination_and_a_positional_assignment_does_not() {
        // **The defect spec 0035 recorded and could not fix**, asserted directly and from both
        // sides. `pages[i]` addresses page `i`, so inserting content that pushes the book slides
        // every assignment off the content it was authored for; an anchor does not move, because a
        // `BlockId` is not a position.
        let doc = sectioned_doc(50);
        let (_, _, starts) = lay_out_sectioned(&doc);
        let before = starts[0].expect("placed");

        // The edit: content inserted *before* the section.
        let mut edited = doc.clone();
        let inserted: Vec<Block> = many_lines(40);
        edited.content.splice(0..0, inserted);
        edited.assign_missing_block_ids().expect("ids");
        let (pages, status, starts) = lay_out_sectioned(&edited);
        let after = starts[0].expect("placed");
        assert!(status.converged, "must settle: {status:?}");
        assert!(
            after > before,
            "the insert must move the chapter: {before} -> {after}"
        );

        // The master went with it, and did not stay behind.
        assert_eq!(
            top_of(&pages, after),
            216.0,
            "the opener is on the page the chapter now opens on"
        );
        assert_eq!(
            top_of(&pages, before),
            54.0,
            "and no longer on the page it used to open on"
        );

        // The same document expressed the spec-0035 way — a positional entry for the page the
        // chapter opened on before the edit — gets this wrong, which is why sections exist.
        let mut positional = edited.clone();
        positional.sections.clear();
        positional.pages = vec![PageOverride::default(); before + 1];
        positional.pages[before].master = Some("chapter-opener".into());
        let (pages, _, _) = lay_out_sectioned(&positional);
        assert_eq!(
            top_of(&pages, before),
            216.0,
            "the positional entry stayed on the page it names"
        );
        assert_ne!(
            top_of(&pages, after),
            216.0,
            "…which is not where the chapter is any more"
        );
    }

    #[test]
    fn a_section_whose_anchor_is_gone_assigns_nothing() {
        let mut doc = sectioned_doc(50);
        doc.sections[0].start = BlockId(9999);
        let (pages, status, starts) = lay_out_sectioned(&doc);
        assert_eq!(starts, vec![None]);
        assert!(status.converged);
        // Every page is the document default: losing the furniture beats refusing the document.
        for page in 0..pages.len() {
            assert_eq!(top_of(&pages, page), 54.0, "page {page}");
        }
    }

    #[test]
    fn a_document_with_no_sections_takes_exactly_one_pass() {
        // The claim that this increment costs every other document nothing. A second pass would be
        // a doubling of layout work for every book in existence.
        let doc = doc_with_blocks(many_lines(60));
        let (_, status, _) = lay_out_sectioned(&doc);
        assert_eq!(status.iterations, 1);
        assert!(status.converged);

        // And a settling sectioned document costs exactly one extra pass: derive, re-lay, agree.
        let (_, status, _) = lay_out_sectioned(&sectioned_doc(50));
        assert_eq!(status.iterations, 2);
        assert!(status.converged);
    }

    #[test]
    fn a_section_assignment_that_will_not_settle_stops_at_the_cap() {
        // The oscillation is real rather than contrived, and it is *why* master assignment has to be
        // bounded: the opener's deep top margin shrinks the page the chapter opens on, which pushes
        // the anchor onto the next page, where the same master then applies and gives the previous
        // page its capacity back. The anchor sits exactly in the band between the two capacities.
        //
        // The requirement is that it terminates and says so — a guess presented as settled is worse
        // than a document that admits it is one page out.
        let doc = sectioned_doc(40);
        let (pages, status, _) = lay_out_sectioned(&doc);
        assert!(
            !status.converged,
            "this fixture is chosen to oscillate; if it settles the fixture has stopped \
             testing the bound: {status:?}"
        );
        assert_eq!(status.iterations, FIXPOINT_MAX_ITERATIONS);

        // And the last iterate is a whole document, not a half-laid-out one: every block placed.
        let placed: BTreeSet<BlockId> = pages
            .iter()
            .flat_map(|p| p.blocks.iter())
            .filter_map(|b| match b {
                PlacedBlock::Text { source, .. } => Some(*source),
                _ => None,
            })
            .collect();
        for block in &doc.content {
            assert!(placed.contains(&block.id()), "{:?} was dropped", block.id());
        }
    }

    // ---------------------------------------------------------------------------------------
    // Cross-references (spec 0076)
    // ---------------------------------------------------------------------------------------

    /// A paragraph reading "See page N." where N is a reference to `target`.
    fn refers_to(target: BlockId) -> Block {
        use quill_core_model::Run;
        Block::body_runs(
            vec![
                Run::plain("See page "),
                Run::reference(target),
                Run::plain(" for more."),
            ],
            Color::Gray { v: 0.0 },
        )
    }

    /// What a block actually printed, joined across its placed lines.
    fn printed(pages: &[LaidOutPage], id: BlockId) -> String {
        let mut out = String::new();
        for page in pages {
            for placed in &page.blocks {
                if let PlacedBlock::Text { source, lines, .. } = placed {
                    if *source == id {
                        for l in lines {
                            out.push_str(&l.text);
                        }
                    }
                }
            }
        }
        out
    }

    /// A body block that carries no reference and one that does, for `reference_fingerprint`'s
    /// distinguished zero.
    pub(crate) fn oscillating_doc() -> Document {
        use quill_core_model::{Folio, NumberFormat, Run, Section};
        // The construction, stated because the test is worthless if the fixture stops oscillating
        // and nobody notices. Page capacity here is 54 lines; the target block is the 431st line of
        // the document when the referring paragraph sets in **one** line and the 432nd when it sets
        // in two, and 431 is the last line of page 7 — so the target sits astride the 7/8 boundary
        // and the paragraph's own width is what decides which side it falls on.
        //
        // The paragraph's last word is the reference plus a full stop, and the 30 filler words
        // leave exactly four columns for it. A **lower-roman** folio is what closes the loop, and
        // it is the load-bearing choice: roman width is not monotone in the page number, so page 8
        // writes `viii` (four columns, does not fit, paragraph takes a second line) and page 9
        // writes `ix` (two columns, fits, paragraph takes one). Decimal cannot oscillate this way —
        // a wider reference pushes its target later and a later page has a wider number, which only
        // ever grows — and this is exactly the case spec 0075 recorded that a contents list has no
        // free variable for.
        let mut content: Vec<Block> = vec![Block::body("placeholder", Color::Gray { v: 0.0 })];
        content.extend(many_lines(430));
        content.push(Block::body("TARGET", Color::Gray { v: 0.0 }));
        let mut doc = doc_with_blocks(content);
        let target = doc.content.last().expect("target").id();
        let first = doc.content[0].id();
        let mut p = Block::body_runs(
            vec![
                Run::plain("w ".repeat(30)),
                Run::plain("See page "),
                Run::reference(target),
                Run::plain("."),
            ],
            Color::Gray { v: 0.0 },
        );
        p.set_id(first);
        doc.content[0] = p;
        doc.sections = vec![Section {
            name: "Front".into(),
            start: first,
            master: None,
            folio: Some(Folio {
                format: NumberFormat::LowerRoman,
                restart_at: Some(1),
            }),
        }];
        doc
    }

    /// Which page a block was placed on, or `None`.
    fn page_of(pages: &[LaidOutPage], id: BlockId) -> Option<usize> {
        pages.iter().find_map(|p| {
            p.blocks
                .iter()
                .any(|b| match b {
                    PlacedBlock::Text { source, .. }
                    | PlacedBlock::Image { source, .. }
                    | PlacedBlock::Link { source, .. } => *source == id,
                    PlacedBlock::Rect { .. } => false,
                })
                .then_some(p.index)
        })
    }

    /// A document whose first block refers to a block `after` paragraphs further on.
    fn referring_doc(after: usize) -> Document {
        let mut content = vec![Block::body("placeholder", Color::Gray { v: 0.0 })];
        content.extend(many_lines(after));
        content.push(Block::heading(
            1,
            "The Sunken Vault",
            Color::Gray { v: 0.0 },
        ));
        let mut doc = doc_with_blocks(content);
        let target = doc.content.last().expect("target").id();
        let first = doc.content[0].id();
        let mut p = refers_to(target);
        p.set_id(first);
        doc.content[0] = p;
        doc
    }

    #[test]
    fn a_cross_reference_prints_the_folio_of_the_page_its_target_landed_on() {
        let doc = referring_doc(120);
        let (pages, status, _) = lay_out_sectioned(&doc);
        assert!(status.converged, "{status:?}");
        let target = doc.content.last().expect("target").id();
        let landed = page_of(&pages, target).expect("the target is in the document");
        assert!(
            landed > 0,
            "the fixture exists to put the target off page 0"
        );
        assert_eq!(
            printed(&pages, doc.content[0].id()),
            format!("See page {} for more.", landed + 1),
            "the reference prints where the target actually is"
        );
    }

    #[test]
    fn a_cross_reference_prints_the_folio_rather_than_the_page_index() {
        // Spec 0073's split, applied. A reference is a number a *reader* is told, so it follows the
        // folio; a `/Link` destination is a number a *machine* is told and keeps the index. A
        // document numbering its pages in roman is where the two visibly differ, and printing `8`
        // for a page printed `viii` would send the reader to a page that does not answer to it.
        use quill_core_model::{Folio, NumberFormat, Section};
        let mut doc = referring_doc(120);
        let first = doc.content[0].id();
        doc.sections = vec![Section {
            name: "Front".into(),
            start: first,
            master: None,
            folio: Some(Folio {
                format: NumberFormat::LowerRoman,
                restart_at: Some(1),
            }),
        }];
        let (pages, status, _) = lay_out_sectioned(&doc);
        assert!(status.converged, "{status:?}");
        let landed = page_of(&pages, doc.content.last().expect("t").id()).expect("placed");
        assert_eq!(landed, 2, "the fixture is chosen so the folio is `iii`");
        assert_eq!(printed(&pages, first), "See page iii for more.");
    }

    #[test]
    fn a_cross_reference_survives_repagination() {
        // The whole feature, asserted from both sides on spec 0072's reasoning: a test that only
        // checked the new number would pass against an implementation that printed the *last* page
        // of the document, and one that only checked that it changed would pass against one that
        // printed anything at all.
        let doc = referring_doc(120);
        let (pages, _, _) = lay_out_sectioned(&doc);
        let first = doc.content[0].id();
        let before = printed(&pages, first);

        let mut edited = doc.clone();
        edited.content.splice(1..1, many_lines(120));
        edited.assign_missing_block_ids().expect("ids");
        let (pages, status, _) = lay_out_sectioned(&edited);
        assert!(status.converged, "{status:?}");
        let landed = page_of(&pages, edited.content.last().expect("t").id()).expect("placed");
        let after = printed(&pages, first);
        assert_ne!(after, before, "content inserted before the target moved it");
        assert_eq!(
            after,
            format!("See page {} for more.", landed + 1),
            "and the printed number followed it"
        );
    }

    #[test]
    fn a_cross_reference_may_name_any_block_not_only_a_heading() {
        // The deliberate scope decision. "See the table on page 42" is the same sentence as "see the
        // chapter on page 42", and `section_starts` already anchors to every placed variant that
        // carries an identity — so a heading-only rule would be a structure-shaped restriction on a
        // mechanism that is general for free.
        let mut content = vec![Block::body("placeholder", Color::Gray { v: 0.0 })];
        content.extend(many_lines(120));
        content.push(Block::Table {
            id: BlockId::UNASSIGNED,
            table: quill_core_model::Table {
                columns: vec![1.0, 1.0],
                header: Some(vec!["Roll".into(), "Result".into()]),
                rows: vec![vec!["1".into(), "a locked door".into()]],
                zebra: false,
            },
            color: Color::Gray { v: 0.0 },
        });
        let mut doc = doc_with_blocks(content);
        let target = doc.content.last().expect("target").id();
        let first = doc.content[0].id();
        let mut p = refers_to(target);
        p.set_id(first);
        doc.content[0] = p;

        let (pages, status, _) = lay_out_sectioned(&doc);
        assert!(status.converged, "{status:?}");
        let landed = page_of(&pages, target).expect("a table is placed like anything else");
        assert_eq!(
            printed(&pages, first),
            format!("See page {} for more.", landed + 1)
        );
    }

    #[test]
    fn a_cross_reference_to_a_block_that_is_not_there_prints_a_visible_marker() {
        // `CLAUDE.md`: prefer a visible failure over silent press-corruption. The alternatives were
        // an empty string — "See page  for more." in a sentence that reads as finished — and
        // refusing the document, which spec 0072 already rejected for a dangling section anchor
        // because losing a whole book to one stale id is worse than losing the thing it named.
        // A cross-reference is content rather than furniture, so it cannot take 0072's *silent*
        // half: it says so on the page.
        let mut doc = referring_doc(60);
        let first = doc.content[0].id();
        let mut p = refers_to(BlockId(9999));
        p.set_id(first);
        doc.content[0] = p;

        let (pages, status, _) = lay_out_sectioned(&doc);
        assert!(status.converged, "an unresolvable reference is not a hang");
        // The literal, not `format!("…{UNRESOLVED_REFERENCE}…")`: a test written against the
        // constant is a formula checked against itself, and would pass just as happily if the
        // marker were changed to the empty string — which is the silent option this decision
        // rejects.
        assert_eq!(printed(&pages, first), "See page [?] for more.");
        // And the rest of the document is intact: this is a marker, not a refusal.
        for block in &doc.content {
            assert!(
                page_of(&pages, block.id()).is_some(),
                "{:?} was dropped",
                block.id()
            );
        }
    }

    #[test]
    fn a_document_with_no_cross_reference_takes_exactly_one_pass() {
        // The claim that this increment costs every other document nothing, and the layout half of
        // "lays out and exports exactly as before" — the export half is `SAMPLE_EXPORT_DIGEST`.
        let doc = doc_with_blocks(many_lines(60));
        let (_, status, _) = lay_out_sectioned(&doc);
        assert_eq!(status.iterations, 1);
        assert!(status.converged);
        assert!(
            quill_core_model::reference_targets(&doc.content).is_empty(),
            "and the derivation is skipped outright"
        );

        // A referring document costs exactly one extra pass: resolve, re-lay, agree. That is the
        // same single resolving pass spec 0073 charged for a section, for the same reason — a
        // reference's target has to be *placed* before its page is known.
        let (_, status, _) = lay_out_sectioned(&referring_doc(120));
        assert_eq!(status.iterations, 2);
        assert!(status.converged);
    }

    #[test]
    fn a_cross_reference_that_will_not_settle_stops_at_the_cap() {
        // Unlike spec 0073's folios, a cross-reference is genuinely *in the flow*: its width moves a
        // line break, which moves a page, which moves the number. Spec 0075 could construct no
        // oscillating contents fixture because an entry's height does not depend on its number;
        // this one has exactly that free variable, and the bound and its `converged: false` report
        // finally have the case they were kept for.
        let doc = oscillating_doc();
        let (pages, status, _) = lay_out_sectioned(&doc);
        assert!(
            !status.converged,
            "this fixture is chosen to oscillate; if it settles it has stopped testing the \
             bound: {status:?}"
        );
        assert_eq!(status.iterations, FIXPOINT_MAX_ITERATIONS);

        // The last iterate is a whole document — nothing missing — and it is *honestly* one out:
        // the number printed disagrees with the page the target actually landed on, which is what
        // `converged: false` is telling the caller.
        for block in &doc.content {
            assert!(
                page_of(&pages, block.id()).is_some(),
                "{:?} was dropped",
                block.id()
            );
        }
        let target = doc.content.last().expect("target").id();
        let landed = page_of(&pages, target).expect("placed");
        let printed_here = printed(&pages, doc.content[0].id());
        assert!(
            printed_here.ends_with("See page ix."),
            "the last iterate printed {printed_here:?}"
        );
        assert_eq!(landed, 7, "…while the target is on the page printed `viii`");
    }

    // ---------------------------------------------------------------------------------------
    // Footnotes (spec 0077)
    // ---------------------------------------------------------------------------------------

    /// Lay a document out through the path that carries its footnotes — what [`lay_out`] does,
    /// with the fixpoint status the tests need.
    fn lay_out_noted(doc: &Document) -> (Vec<LaidOutPage>, FixpointStatus) {
        let template = DocumentTemplate::new(doc);
        lay_out_resolved(
            &doc.content,
            &doc.footnote_blocks(),
            &doc.breaks,
            &doc.assets,
            &doc.styles,
            &doc.component_library(),
            &template,
            &MONO,
            &NoHyphenator,
        )
    }

    /// A document of `before` filler lines, then a paragraph carrying one footnote anchor.
    ///
    /// Returns the document, the anchoring block's id and the note's id.
    fn noted_doc(before: usize, note_text: &str) -> (Document, BlockId, BlockId) {
        use quill_core_model::{Footnote, Run};
        let ink = Color::Gray { v: 0.0 };
        let mut content = many_lines(before);
        content.push(Block::body_runs(
            vec![Run::plain("anchored"), Run::footnote(BlockId(1))],
            ink,
        ));
        let mut doc = doc_with_blocks(content);
        // The anchor names a note id that does not exist yet, so it is patched after the notes are
        // given ids — which is exactly what an editor would do the other way round.
        doc.footnotes = vec![Footnote::plain(note_text, ink)];
        doc.assign_missing_block_ids().expect("ids");
        let note = doc.footnotes[0].id;
        let anchor = doc.content.last().expect("anchor").id();
        if let Some(Block::Body { runs, .. }) = doc.content.last_mut() {
            runs[1] = Run::footnote(note);
        }
        (doc, anchor, note)
    }

    /// **The defining property.** Asserted across a *sweep* of the page boundary rather than at one
    /// filler count, because the interesting case is the one where reserving the band is what pushed
    /// the anchor onto the next page — and where that happens depends on arithmetic a metrics change
    /// could move. A sweep tests the property; a single count tests one arrangement of it.
    #[test]
    fn an_anchor_and_its_note_land_on_the_same_page() {
        let mut pushed = 0;
        for before in 45..58 {
            let (doc, anchor, note) = noted_doc(before, "a note at the foot of the page");
            let (pages, status) = lay_out_noted(&doc);
            assert!(status.converged, "{status:?}");
            let a = page_of(&pages, anchor).expect("the anchor is placed");
            let n = page_of(&pages, note).expect("the note is placed");
            assert_eq!(
                a, n,
                "before={before}: the anchor is on page {a} and its note on page {n}"
            );
            // Without a band, 54 lines fill a page here; with one, fewer do. Count the fillers that
            // the band moved onto the next page, so a change that quietly stopped reserving
            // anything fails this rather than passing every equality above.
            if a > 0 {
                pushed += 1;
            }
        }
        assert!(
            pushed >= 2,
            "the sweep must cross the page boundary or it is asserting nothing; {pushed} of 13 \
             put the anchor past page 0"
        );
    }

    /// The band really does take height off the frame, stated as the difference it makes rather
    /// than as an absolute — the number would move with the metrics, the *difference* is the
    /// mechanism.
    #[test]
    fn the_note_band_reduces_the_frame_and_nothing_else() {
        let (noted, _, _) = noted_doc(52, "a note at the foot of the page");
        let mut bare = noted.clone();
        bare.footnotes.clear();
        if let Some(Block::Body { runs, .. }) = bare.content.last_mut() {
            runs.truncate(1);
        }

        let (with, _) = lay_out_noted(&noted);
        let (without, _) = lay_out_noted(&bare);
        assert!(
            with.len() > without.len(),
            "reserving a band must cost the document space: {} pages with, {} without",
            with.len(),
            without.len()
        );
        // And a document with no note reserves nothing: the band is `0.0`, so `bottom` is the
        // number it has always been.
        assert!(bare.footnote_blocks().is_empty());
    }

    /// The band's height is *reserved*, not merely drawn: no body block may reach into it. This is
    /// the direct statement of what reducing `bottom` buys, and it is the assertion that fails if a
    /// later change draws the band without taking the height off the frame.
    #[test]
    fn no_body_block_reaches_into_the_note_band() {
        let notes: BTreeSet<BlockId> = {
            let (doc, _, _) = noted_doc(52, "a note at the foot of the page");
            let (pages, _) = lay_out_noted(&doc);
            let ids: BTreeSet<BlockId> = doc.footnote_blocks().iter().map(Block::id).collect();
            check_no_overlap(&pages, &ids);
            ids
        };
        assert_eq!(notes.len(), 1);

        // …and over the sweep, so it is a property of the mechanism rather than of one arrangement.
        for before in 45..58 {
            let (doc, _, _) = noted_doc(before, "a note at the foot of the page");
            let (pages, _) = lay_out_noted(&doc);
            let ids: BTreeSet<BlockId> = doc.footnote_blocks().iter().map(Block::id).collect();
            check_no_overlap(&pages, &ids);
        }
    }

    /// Every page's body ink must end above the band its notes occupy.
    ///
    /// The band's top is derived from the notes actually placed rather than from the separator
    /// rule, because a stat block draws `PlacedBlock::Rect`s of its own and the two are
    /// indistinguishable by variant.
    fn check_no_overlap(pages: &[LaidOutPage], notes: &BTreeSet<BlockId>) {
        for page in pages {
            let top = page
                .blocks
                .iter()
                .filter_map(|b| match b {
                    PlacedBlock::Text { source, frame, .. } if notes.contains(source) => {
                        Some(frame.y_pt)
                    }
                    _ => None,
                })
                .fold(f32::INFINITY, f32::min);
            if !top.is_finite() {
                continue;
            }
            let rule = top - NOTE_RULE_SPACE_BELOW_PT - NOTE_RULE_THICKNESS_PT;
            for placed in &page.blocks {
                if let PlacedBlock::Text { source, frame, .. } = placed {
                    if notes.contains(source) {
                        continue;
                    }
                    assert!(
                        frame.y_pt + frame.h_pt <= rule + 0.01,
                        "page {}: body ink ends at {} but the note band opens at {rule}",
                        page.index,
                        frame.y_pt + frame.h_pt
                    );
                }
            }
        }
    }

    /// A composite among the footnotes. A stat block would rather move whole than be cut, and the
    /// height it compares itself against is the *next* frame's — which a carried note will have
    /// taken part of before the block ever reaches it. Whatever it decides, it may not end up drawn
    /// over a band.
    #[test]
    fn a_keep_together_composite_never_lands_on_top_of_a_band() {
        use quill_core_model::{Footnote, Panel, Run};
        let ink = Color::Gray { v: 0.0 };
        for words in [400usize, 900, 1600] {
            let mut content = many_lines(6);
            content.push(Block::body_runs(
                vec![Run::plain("a claim"), Run::footnote(BlockId(1))],
                ink,
            ));
            content.push(Block::Panel {
                id: BlockId::UNASSIGNED,
                panel: Panel {
                    name: "Cave Bear".into(),
                    attributes: vec![("AC".into(), "13".into()), ("HP".into(), "42".into())],
                    overview: (0..12).map(|i| format!("overview line {i}")).collect(),
                    details: (0..12).map(|i| format!("detail line {i}")).collect(),
                    actions: (0..12).map(|i| format!("action line {i}")).collect(),
                    reactions: Vec::new(),
                },
                color: ink,
            });
            content.extend(many_lines(40));
            let mut doc = doc_with_blocks(content);
            doc.footnotes = vec![Footnote::plain("word ".repeat(words), ink)];
            doc.assign_missing_block_ids().expect("ids");
            let note = doc.footnotes[0].id;
            if let Some(Block::Body { runs, .. }) = doc.content.get_mut(6) {
                runs[1] = Run::footnote(note);
            }

            let (pages, status) = lay_out_noted(&doc);
            assert!(status.converged, "{status:?}");
            let ids: BTreeSet<BlockId> = doc.footnote_blocks().iter().map(Block::id).collect();
            check_no_overlap(&pages, &ids);
            assert!(
                page_of(&pages, doc.content[7].id()).is_some(),
                "words={words}: the composite must still be placed"
            );
        }
    }

    /// Spec 0075's mechanism, over a note. Conservation is the assertion that matters: the lines
    /// placed across every page, in order, are exactly the lines one unsplit measurement produces.
    #[test]
    fn a_note_too_tall_for_its_band_splits_across_pages_and_loses_nothing() {
        let long = "word ".repeat(1400);
        let (doc, _, note) = noted_doc(10, &long);
        let (pages, status) = lay_out_noted(&doc);
        assert!(status.converged, "{status:?}");

        let on: Vec<usize> = pages
            .iter()
            .filter(|p| {
                p.blocks
                    .iter()
                    .any(|b| matches!(b, PlacedBlock::Text { source, .. } if *source == note))
            })
            .map(|p| p.index)
            .collect();
        assert!(
            on.len() >= 2,
            "the fixture exists to make the note span frames; it is on {on:?}"
        );

        // The unsplit line list, measured the same way the flow measures it.
        let notes = doc.footnote_blocks();
        let frame = DocumentTemplate::new(&doc).frames(0)[0];
        let markers = BTreeMap::new();
        let assets = AssetIndex::new();
        let ctx = BlockContext {
            assets: &assets,
            styles: &doc.styles,
            headings: &[],
            index: &[],
            components: &builtin_components(),
            markers: &markers,
            resolved: &quill_core_model::footnote_numbers(&notes),
        };
        let (whole, _) = measure_block(&notes[0], frame.rect.w_pt, &ctx, &MONO, &NoHyphenator)
            .expect("measured");
        let Measured::Text { lines: want, .. } = whole else {
            panic!("a note is a paragraph")
        };

        let mut got: Vec<String> = Vec::new();
        for page in &pages {
            for placed in &page.blocks {
                if let PlacedBlock::Text { source, lines, .. } = placed {
                    if *source == note {
                        got.extend(lines.iter().map(|l| l.text.clone()));
                    }
                }
            }
        }
        assert_eq!(
            got,
            want.iter().map(|l| l.text.clone()).collect::<Vec<_>>(),
            "every line of the note, once, in order"
        );
    }

    /// Two notes called by **one** paragraph, both too tall to sit whole in what is left of the
    /// band. Only one of them may be carried forward, and it must be the last — otherwise the
    /// second cut overwrites the first carry and the first note's tail is silently lost.
    ///
    /// Swept over several note lengths, because which of the two ends up needing the cut depends on
    /// arithmetic a metrics change could move; the property is that **nothing is lost**, whichever
    /// one it is.
    #[test]
    fn two_notes_in_one_paragraph_lose_nothing_between_them() {
        use quill_core_model::{Footnote, Run};
        let ink = Color::Gray { v: 0.0 };
        for words in [700usize, 800, 900, 1000] {
            let mut content = many_lines(4);
            content.push(Block::body_runs(
                vec![
                    Run::plain("two claims"),
                    Run::footnote(BlockId(1)),
                    Run::plain(" and another"),
                    Run::footnote(BlockId(2)),
                ],
                ink,
            ));
            content.extend(many_lines(40));
            let mut doc = doc_with_blocks(content);
            doc.footnotes = vec![
                Footnote::plain(format!("alpha {}", "word ".repeat(words)), ink),
                Footnote::plain(format!("beta {}", "word ".repeat(words)), ink),
            ];
            doc.assign_missing_block_ids().expect("ids");
            let (a, b) = (doc.footnotes[0].id, doc.footnotes[1].id);
            if let Some(Block::Body { runs, .. }) = doc.content.get_mut(4) {
                runs[1] = Run::footnote(a);
                runs[3] = Run::footnote(b);
            }

            let (pages, status) = lay_out_noted(&doc);
            assert!(status.converged, "{status:?}");
            let notes = doc.footnote_blocks();
            let frame = DocumentTemplate::new(&doc).frames(0)[0];
            let markers = BTreeMap::new();
            let assets = AssetIndex::new();
            let ctx = BlockContext {
                assets: &assets,
                styles: &doc.styles,
                headings: &[],
                index: &[],
                components: &builtin_components(),
                markers: &markers,
                resolved: &quill_core_model::footnote_numbers(&notes),
            };
            for note in &notes {
                let (whole, _) =
                    measure_block(note, frame.rect.w_pt, &ctx, &MONO, &NoHyphenator).expect("m");
                let Measured::Text { lines: want, .. } = whole else {
                    panic!("a note is a paragraph")
                };
                let mut got: Vec<String> = Vec::new();
                for page in &pages {
                    for placed in &page.blocks {
                        if let PlacedBlock::Text { source, lines, .. } = placed {
                            if *source == note.id() {
                                got.extend(lines.iter().map(|l| l.text.clone()));
                            }
                        }
                    }
                }
                assert_eq!(
                    got,
                    want.iter().map(|l| l.text.clone()).collect::<Vec<_>>(),
                    "words={words}: every line of {:?}, once, in order",
                    note.id()
                );
            }
        }
    }

    /// A note whose remainder is taller than a whole frame carries through several frames, and
    /// terminates. **This is the fixture for the progress bound**: a band that grew without
    /// consuming its note would page for ever, and `open_band`'s assertion is what says so.
    #[test]
    fn a_note_taller_than_a_frame_is_consumed_one_frame_at_a_time() {
        let long = "word ".repeat(4000);
        let (doc, _, note) = noted_doc(2, &long);
        let (pages, status) = lay_out_noted(&doc);
        assert!(status.converged, "{status:?}");
        let bands: Vec<usize> = pages
            .iter()
            .filter(|p| {
                p.blocks
                    .iter()
                    .any(|b| matches!(b, PlacedBlock::Text { source, .. } if *source == note))
            })
            .map(|p| p.index)
            .collect();
        assert!(
            bands.len() >= 4,
            "the fixture must span several frames to exercise the carry; it spans {bands:?}"
        );
        // Consecutive, and it ends: a carry that failed to advance would either loop for ever
        // (caught by the assertion in `open_band`) or leave a gap here.
        for w in bands.windows(2) {
            assert_eq!(w[1], w[0] + 1, "the carry must take the very next frame");
        }
    }

    /// **The resume contract, for the state a footnote adds.** Spec 0044 asserts that resuming from
    /// a checkpoint whose `split_at > 0` reproduces a full pass; a page that *opens* in the middle
    /// of a note owes the same assertion, and it is the reason `FlowState` grew a field.
    ///
    /// Written against `flow` directly rather than through a session, because the session chooses
    /// its own resume point and would have to be coaxed into choosing this one — which would make
    /// the test about the choice rather than about the contract.
    #[test]
    fn resuming_from_a_checkpoint_inside_a_carried_note_reproduces_a_full_pass() {
        // Filler *after* the anchor as well as before it, so the note is carried across page
        // boundaries the main loop takes rather than only across the ones the end-of-content drain
        // adds. Both paths record a checkpoint; only this fixture reaches the first.
        let long = "word ".repeat(3000);
        let (mut doc, _, _) = noted_doc(2, &long);
        doc.content.extend(many_lines(200));
        doc.assign_missing_block_ids().expect("ids");
        let notes = doc.footnote_blocks();
        let numbers = quill_core_model::footnote_numbers(&notes);
        let template = DocumentTemplate::new(&doc);
        let components = doc.component_library();
        let run = |start: FlowState| {
            flow(
                &doc.content,
                &notes,
                &doc.breaks,
                &doc.assets,
                &doc.styles,
                &components,
                &[],
                &[],
                &numbers,
                &template,
                &MONO,
                &NoHyphenator,
                start,
                &mut NoCache,
                None,
            )
        };

        let full = run(FlowState::start(&template));
        let carried: Vec<usize> = full
            .checkpoints
            .iter()
            .enumerate()
            .filter(|(_, c)| c.note_carry.is_some())
            .map(|(i, _)| i)
            .collect();
        assert!(
            !carried.is_empty(),
            "the fixture exists to record a checkpoint inside a note; it recorded none"
        );

        for i in &carried {
            let resumed = run(full.checkpoints[*i]);
            assert_eq!(
                resumed.pages,
                full.pages[*i..],
                "resuming at page {i} — whose band opens with {:?} — must reproduce the full pass",
                full.checkpoints[*i].note_carry
            );
        }
    }

    /// The fragment case, and the one a coarser implementation gets wrong: a paragraph is cut across
    /// a page boundary and its anchor is in the **remainder**, so the note belongs to the second
    /// page and not to the first.
    #[test]
    fn a_note_follows_its_anchor_into_the_frame_the_anchor_lands_in() {
        use quill_core_model::{Footnote, Run};
        let ink = Color::Gray { v: 0.0 };
        // One paragraph tall enough to be cut, with the anchor as its last run.
        let mut content = many_lines(50);
        content.push(Block::body_runs(
            vec![Run::plain("body ".repeat(200)), Run::footnote(BlockId(1))],
            ink,
        ));
        let mut doc = doc_with_blocks(content);
        doc.footnotes = vec![Footnote::plain("the note", ink)];
        doc.assign_missing_block_ids().expect("ids");
        let note = doc.footnotes[0].id;
        let para = doc.content.last().expect("para").id();
        let anchor_run = 1usize;
        if let Some(Block::Body { runs, .. }) = doc.content.last_mut() {
            runs[anchor_run] = Run::footnote(note);
        }

        let (pages, status) = lay_out_noted(&doc);
        assert!(status.converged, "{status:?}");

        // The paragraph really is cut.
        let on: Vec<usize> = pages
            .iter()
            .filter(|p| {
                p.blocks
                    .iter()
                    .any(|b| matches!(b, PlacedBlock::Text { source, .. } if *source == para))
            })
            .map(|p| p.index)
            .collect();
        assert!(
            on.len() >= 2,
            "the fixture must split the paragraph: {on:?}"
        );

        // The page the anchor's *line* landed on, found through the spans rather than by looking
        // for the number's characters — "1" appears in filler text too.
        let anchor_page = pages
            .iter()
            .find(|p| {
                p.blocks.iter().any(|b| match b {
                    PlacedBlock::Text { source, lines, .. } if *source == para => lines
                        .iter()
                        .any(|l| l.spans.iter().any(|s| s.run == anchor_run)),
                    _ => false,
                })
            })
            .map(|p| p.index)
            .expect("the anchor run is drawn somewhere");
        assert_eq!(
            page_of(&pages, note),
            Some(anchor_page),
            "the note belongs to the fragment its anchor is in, not to the block's first page"
        );
        assert!(
            anchor_page > on[0],
            "the fixture exists to put the anchor in the *remainder*: it is on page \
             {anchor_page} and the paragraph starts on {}",
            on[0]
        );
    }

    /// Numbering is document-sequential and derived from where the **anchors** are, not from the
    /// order of `Document::footnotes` — which is what makes the list an unordered store.
    #[test]
    fn footnotes_are_numbered_by_anchor_order_not_by_list_order() {
        use quill_core_model::{Footnote, Run};
        let ink = Color::Gray { v: 0.0 };
        let mut doc = doc_with_blocks(vec![
            Block::body_runs(vec![Run::plain("first"), Run::footnote(BlockId(1))], ink),
            Block::body_runs(vec![Run::plain("second"), Run::footnote(BlockId(2))], ink),
        ]);
        doc.footnotes = vec![Footnote::plain("BETA", ink), Footnote::plain("ALPHA", ink)];
        doc.assign_missing_block_ids().expect("ids");
        let (beta, alpha) = (doc.footnotes[0].id, doc.footnotes[1].id);
        // The *second* note in the list is anchored first.
        if let Block::Body { runs, .. } = &mut doc.content[0] {
            runs[1] = Run::footnote(alpha);
        }
        if let Block::Body { runs, .. } = &mut doc.content[1] {
            runs[1] = Run::footnote(beta);
        }

        let numbers = doc.footnote_numbers();
        assert_eq!(numbers.get(&alpha).map(String::as_str), Some("1"));
        assert_eq!(numbers.get(&beta).map(String::as_str), Some("2"));

        let (pages, _) = lay_out_noted(&doc);
        assert_eq!(printed(&pages, doc.content[0].id()), "first1");
        assert_eq!(printed(&pages, doc.content[1].id()), "second2");
        assert!(printed(&pages, alpha).starts_with("1. ALPHA"));
        assert!(printed(&pages, beta).starts_with("2. BETA"));
    }

    /// An anchor naming a note the document does not hold prints the marker rather than a number
    /// nothing answers to — spec 0076's posture, at the site that inherits it. And the notes that
    /// *are* there stay numbered without a hole.
    #[test]
    fn an_anchor_with_no_note_prints_a_visible_marker() {
        use quill_core_model::{Footnote, Run};
        let ink = Color::Gray { v: 0.0 };
        let mut doc = doc_with_blocks(vec![
            Block::body_runs(vec![Run::plain("a"), Run::footnote(BlockId(999_999))], ink),
            Block::body_runs(vec![Run::plain("b"), Run::footnote(BlockId(1))], ink),
        ]);
        doc.footnotes = vec![Footnote::plain("real", ink)];
        doc.assign_missing_block_ids().expect("ids");
        let real = doc.footnotes[0].id;
        if let Block::Body { runs, .. } = &mut doc.content[1] {
            runs[1] = Run::footnote(real);
        }
        let (pages, status) = lay_out_noted(&doc);
        assert!(status.converged, "{status:?}");
        // Against the literal, not the constant: a test written as `format!("a{UNRESOLVED}")` is a
        // formula checked against itself and would pass just as happily if the marker became "".
        assert_eq!(printed(&pages, doc.content[0].id()), "a[?]");
        assert_eq!(
            printed(&pages, doc.content[1].id()),
            "b1",
            "the note that exists is still number one"
        );
    }

    /// A document with no footnote takes the identical path: the notes list is empty, the band is
    /// never non-empty, and the pages are the ones the no-footnote entry point produces.
    #[test]
    fn a_document_with_no_footnote_lays_out_exactly_as_before() {
        let doc = doc_with_blocks(many_lines(200));
        assert!(doc.footnote_blocks().is_empty());
        let (noted, status) = lay_out_noted(&doc);
        assert_eq!(status.iterations, 1, "no extra pass");
        let template = DocumentTemplate::new(&doc);
        let (bare, _) = lay_out_with_fixpoint_status(
            &doc.content,
            &doc.assets,
            &doc.styles,
            &template,
            &MONO,
            &NoHyphenator,
        );
        assert_eq!(noted, bare, "the footnote path must be a no-op without one");
        assert!(
            noted
                .iter()
                .flat_map(|p| &p.blocks)
                .all(|b| !matches!(b, PlacedBlock::Rect { .. })),
            "no band separator is drawn for a document with no note"
        );
    }

    // --- The index (spec 0078) ------------------------------------------------------------------

    /// A body paragraph whose one run marks `term`.
    fn marked(text: &str, term: &str) -> Block {
        let mut run = quill_core_model::Run::plain(text);
        run.index = Some(IndexMark::new(term));
        Block::body_runs(vec![run], Color::Gray { v: 0.0 })
    }

    /// A document with an index block, then `blocks`, with ids assigned.
    fn index_doc(ignore_leading: Vec<String>, blocks: Vec<Block>) -> Document {
        let mut content: Vec<Block> = vec![Block::Index {
            id: BlockId::UNASSIGNED,
            title: "Index".into(),
            ignore_leading,
            color: Color::Gray { v: 0.0 },
        }];
        content.extend(blocks);
        let mut doc = doc_with_blocks(content);
        doc.assign_missing_block_ids().expect("ids");
        doc
    }

    /// Every index entry as `(term, printed pages)`, read back off the page.
    ///
    /// The index block emits its own title, then per entry a term run and a numbers run — the two
    /// segments of one tabbed line. Read by source id, because an index shares its page with
    /// whatever precedes it (spec 0041's lesson, found by a failing test then too).
    fn index_entry_texts(doc: &Document, pages: &[LaidOutPage]) -> Vec<(String, String)> {
        let id = doc
            .content
            .iter()
            .find(|b| matches!(b, Block::Index { .. }))
            .map(|b| b.id())
            .expect("an index block");
        let texts: Vec<String> = pages
            .iter()
            .flat_map(|p| p.blocks.iter())
            .filter_map(|b| match b {
                PlacedBlock::Text { lines, source, .. } if *source == id => {
                    Some(lines[0].text.clone())
                }
                _ => None,
            })
            .filter(|t| !t.is_empty())
            .collect();
        texts[1..]
            .chunks(2)
            .filter(|c| c.len() == 2)
            .map(|c| (c[0].clone(), c[1].clone()))
            .collect()
    }

    fn laid_out(doc: &Document) -> (Vec<LaidOutPage>, FixpointStatus) {
        lay_out_with_fixpoint_status(
            &doc.content,
            &doc.assets,
            &doc.styles,
            &DocumentTemplate::new(doc),
            &MONO,
            &NoHyphenator,
        )
    }

    #[test]
    fn marked_terms_are_collated_rather_than_byte_ordered() {
        // The rule the workspace did not have. Every one of these is in a different place under
        // `BTreeMap<String, _>`: `Zebra` would lead (capitals sort first), `Ale` would follow
        // `Ålesund` (U+00C5 is past every ASCII letter), and `The Ruined Keep` would file under T.
        let doc = index_doc(
            ["A", "An", "The"].iter().map(|s| s.to_string()).collect(),
            vec![
                marked("one", "Zebra"),
                marked("two", "Ålesund"),
                marked("three", "The Ruined Keep"),
                marked("four", "apple"),
                marked("five", "Ale"),
            ],
        );
        let (pages, status) = laid_out(&doc);
        assert!(status.converged, "{status:?}");
        let terms: Vec<String> = index_entry_texts(&doc, &pages)
            .into_iter()
            .map(|(t, _)| t)
            .collect();
        assert_eq!(
            terms,
            ["Ale", "Ålesund", "apple", "The Ruined Keep", "Zebra"],
            "collated, not byte-ordered"
        );
    }

    #[test]
    fn a_term_marked_twice_on_one_page_appears_once() {
        let doc = index_doc(
            Vec::new(),
            vec![
                marked("first mention", "ravens"),
                marked("second mention", "ravens"),
                marked("elsewhere", "sorcery"),
            ],
        );
        let (pages, status) = laid_out(&doc);
        assert!(status.converged, "{status:?}");
        let entries = index_entry_texts(&doc, &pages);
        assert_eq!(entries.len(), 2, "two terms, three marks: {entries:?}");
        assert_eq!(entries[0].0, "ravens");
        // One page, printed once — not `1, 1`, and not a range of a page with itself.
        assert_eq!(entries[0].1, "1");
    }

    #[test]
    fn a_marked_term_prints_the_folio_and_follows_repagination() {
        use quill_core_model::{Folio, NumberFormat, Section};

        // Roman front matter, so the folio and the page index visibly differ, and the assertion
        // cannot pass against an implementation that printed `page_index + 1`.
        let mut blocks = many_lines(60);
        blocks.push(marked("the marked paragraph", "ravens"));
        let mut doc = index_doc(Vec::new(), blocks);
        doc.sections = vec![Section {
            name: "Front matter".into(),
            start: doc.content[1].id(),
            master: None,
            folio: Some(Folio {
                format: NumberFormat::LowerRoman,
                restart_at: Some(1),
            }),
        }];
        // The template the pages were laid out with, because it is the one that has *derived* the
        // section's start page; a fresh one would still be answering with the default numbering.
        let template = DocumentTemplate::new(&doc);
        let (pages, status) = lay_out_with_fixpoint_status(
            &doc.content,
            &doc.assets,
            &doc.styles,
            &template,
            &MONO,
            &NoHyphenator,
        );
        assert!(status.converged, "{status:?}");
        let before = index_entry_texts(&doc, &pages);
        assert_eq!(before.len(), 1);
        let printed = before[0].1.clone();
        assert!(
            printed.chars().all(|c| "ivxlcdm".contains(c)),
            "a roman folio, not an index: {printed}"
        );

        // Asserted from both sides, on spec 0072's reasoning: a test checking only that the number
        // changed would pass against an implementation that printed anything at all, and one
        // checking only the new value would pass against one that printed the document's last page.
        let marked_id = doc.content.last().expect("the marked block").id();
        let want = template.folio(
            pages
                .iter()
                .find(|p| {
                    p.blocks.iter().any(
                        |b| matches!(b, PlacedBlock::Text { source, .. } if *source == marked_id),
                    )
                })
                .expect("the marked block is placed")
                .index,
        );
        assert_eq!(
            printed, want,
            "the entry prints the page it actually landed on"
        );

        // Insert a page of content above it; the entry must follow.
        let mut moved = doc.clone();
        let tail = moved.content.split_off(1);
        moved.content.extend(many_lines(60));
        moved.content.extend(tail);
        moved.assign_missing_block_ids().expect("ids");
        moved.sections[0].start = moved.content[1].id();
        let (moved_pages, _) = laid_out(&moved);
        let after = index_entry_texts(&moved, &moved_pages);
        assert_ne!(after[0].1, printed, "the folio must move with the content");
    }

    #[test]
    fn a_page_range_coalesces_and_stops_at_a_folio_format_change() {
        use quill_core_model::{Folio, NumberFormat, Section};

        // Five pages' worth of marks on one term, with a section boundary in the middle that
        // changes the format. Consecutive *page indices* straddle it; consecutive *folios* do not,
        // so the range must break there. This is the case that looks correct on an arabic-only
        // document and is wrong the first time someone sets roman front matter.
        let mut blocks: Vec<Block> = Vec::new();
        // ~40 lines fill a page at the default setup, so a mark every 40 lines is a mark per page.
        for _ in 0..6 {
            blocks.push(marked("mention", "ravens"));
            blocks.extend(many_lines(40));
        }
        let mut doc = index_doc(Vec::new(), blocks);
        // The body section opens at the fourth marked paragraph, a few pages in.
        let fourth = doc
            .content
            .iter()
            .filter(|b| matches!(b, Block::Body { runs, .. } if runs[0].index.is_some()))
            .nth(3)
            .expect("six marked paragraphs")
            .id();
        doc.sections = vec![
            Section {
                name: "Front matter".into(),
                start: doc.content[1].id(),
                master: None,
                folio: Some(Folio {
                    format: NumberFormat::LowerRoman,
                    restart_at: Some(1),
                }),
            },
            Section {
                name: "Body".into(),
                start: fourth,
                master: None,
                folio: Some(Folio {
                    format: NumberFormat::Decimal,
                    restart_at: Some(1),
                }),
            },
        ];
        let (pages, status) = laid_out(&doc);
        assert!(status.converged, "{status:?}");
        let entries = index_entry_texts(&doc, &pages);
        assert_eq!(entries.len(), 1, "one term: {entries:?}");
        let printed = &entries[0].1;

        // It coalesces at all...
        assert!(
            printed.contains(quill_core_model::INDEX_RANGE_SEPARATOR),
            "a run of consecutive pages must print as a range: {printed}"
        );
        // ...and it breaks at the format change rather than printing a range across it. Every
        // range in the output has both ends in one numeral system, which is the property that
        // fails the moment folios are coalesced as strings or as page indices.
        for part in printed.split(quill_core_model::INDEX_PAGE_SEPARATOR) {
            let Some((lo, hi)) = part.split_once(quill_core_model::INDEX_RANGE_SEPARATOR) else {
                continue;
            };
            let roman = |s: &str| s.chars().all(|c| "ivxlcdm".contains(c));
            assert_eq!(
                roman(lo),
                roman(hi),
                "range `{part}` spans a folio format change, in `{printed}`"
            );
        }
        assert!(
            printed
                .split(quill_core_model::INDEX_PAGE_SEPARATOR)
                .count()
                >= 2,
            "the section boundary must break the run: {printed}"
        );
    }

    #[test]
    fn a_mark_reports_the_page_its_own_run_landed_on() {
        // The case "first page of the block" gets wrong, and the one an index is most for: a
        // paragraph long enough to be cut across a page boundary (spec 0044), marked in a run that
        // lands on the *second* page. The block's first page is one earlier.
        let long: String = (0..900).map(|i| format!("word{i} ")).collect::<String>();
        let mut tail = quill_core_model::Run::plain(" and finally the marked ending.");
        tail.index = Some(IndexMark::new("ending"));
        let block = Block::body_runs(
            vec![quill_core_model::Run::plain(long), tail],
            Color::Gray { v: 0.0 },
        );
        let doc = index_doc(Vec::new(), vec![block]);
        let (pages, status) = laid_out(&doc);
        assert!(status.converged, "{status:?}");

        let id = doc.content[1].id();
        let carrying: Vec<usize> = pages
            .iter()
            .filter(|p| {
                p.blocks
                    .iter()
                    .any(|b| matches!(b, PlacedBlock::Text { source, .. } if *source == id))
            })
            .map(|p| p.index)
            .collect();
        assert!(
            carrying.len() >= 2,
            "the fixture must actually split the paragraph, got {carrying:?}"
        );
        let entries = index_entry_texts(&doc, &pages);
        assert_eq!(entries.len(), 1);
        let first_page_of_block = (carrying[0] + 1).to_string();
        let last_page_of_block = (carrying[carrying.len() - 1] + 1).to_string();
        assert_eq!(
            entries[0].1, last_page_of_block,
            "the mark is on the paragraph's last run and must report that run's page"
        );
        assert_ne!(
            entries[0].1, first_page_of_block,
            "reporting the block's first page is the defect this asserts against"
        );
    }

    #[test]
    fn an_index_longer_than_a_frame_splits_and_loses_no_entry() {
        // Spec 0075's mechanism, over the block it exists most for: a real book's index is longer
        // than its contents list, and an indivisible panel taller than its frame reaches the branch
        // that places a block whole and lets it run off the page.
        let terms: Vec<String> = (0..200).map(|i| format!("term{i:03}")).collect();
        let blocks: Vec<Block> = terms
            .iter()
            .map(|t| marked(&format!("about {t}"), t))
            .collect();
        let doc = index_doc(Vec::new(), blocks);
        let (pages, status) = laid_out(&doc);
        assert!(status.converged, "the fixpoint must settle: {status:?}");

        let id = doc.content[0].id();
        let placed: Vec<(usize, String)> = pages
            .iter()
            .flat_map(|p| p.blocks.iter().map(move |b| (p.index, b)))
            .filter_map(|(i, b)| match b {
                PlacedBlock::Text { lines, source, .. } if *source == id => {
                    Some((i, lines[0].text.clone()))
                }
                _ => None,
            })
            .filter(|(_, t)| !t.is_empty())
            .collect();
        let carrying: BTreeSet<usize> = placed.iter().map(|(p, _)| *p).collect();
        assert!(
            carrying.len() >= 2,
            "a 200-term index must span pages: {carrying:?}"
        );

        // Conservation: one entry per term, in collated order, nothing dropped or duplicated.
        let got: Vec<&String> = placed[1..].iter().map(|(_, t)| t).step_by(2).collect();
        assert_eq!(got.len(), terms.len(), "one entry per term");
        for (got, want) in got.iter().zip(terms.iter()) {
            assert_eq!(*got, want);
        }
        // The index's own title opens it and is re-stated nowhere.
        assert_eq!(placed[0].1, "Index");
        assert_eq!(placed.iter().filter(|(_, t)| t == "Index").count(), 1);
        // Nothing overruns a frame.
        let bottom = Frame::full_page(&doc.page_setup).rect.h_pt;
        for page in &pages {
            for b in &page.blocks {
                if let PlacedBlock::Text { frame, .. } = b {
                    assert!(
                        frame.y_pt + frame.h_pt <= bottom + 0.01,
                        "page {} runs past the frame bottom",
                        page.index
                    );
                }
            }
        }
    }

    #[test]
    fn a_document_with_no_index_lays_out_exactly_as_before() {
        // The claim every increment here owes: a document without the feature is untouched. Not
        // "looks the same" — the same page vector, and the same single pass.
        let doc = doc_with_blocks(many_lines(200));
        let (pages, status) = laid_out(&doc);
        assert_eq!(status.iterations, 1, "no extra pass: {status:?}");
        assert!(status.converged);
        // The derivation is skipped outright: nothing is marked, so it returns empty without a walk.
        let template = DocumentTemplate::new(&doc);
        assert!(index_entries(&doc.content, &pages, &template, &[]).is_empty());
        // And a marked document with no index block still lays out identically, because the block
        // is what makes the entries reachable.
        let mut marked_only = doc.clone();
        marked_only.content.push(marked("a mention", "ravens"));
        marked_only.assign_missing_block_ids().expect("ids");
        let (with_mark, mark_status) = laid_out(&marked_only);
        assert_eq!(mark_status.iterations, 1, "a mark alone derives nothing");
        assert_eq!(&with_mark[..pages.len() - 1], &pages[..pages.len() - 1]);
    }

    #[test]
    fn a_template_folio_is_its_own_numbering_formatted() {
        use quill_core_model::{Folio, NumberFormat, Section};

        // The contract `PageTemplate::folio_number` owes `PageTemplate::folio`: one number, printed
        // one way, whichever question is asked. Without it an index could coalesce a run of pages
        // whose printed folios say something else entirely.
        let mut doc = doc_with_blocks(many_lines(200));
        doc.sections = vec![Section {
            name: "Front matter".into(),
            start: doc.content[0].id(),
            master: None,
            folio: Some(Folio {
                format: NumberFormat::LowerRoman,
                restart_at: Some(1),
            }),
        }];
        let (pages, _) = laid_out(&doc);
        let template = DocumentTemplate::new(&doc);
        assert!(pages.len() > 2, "the fixture must span pages");
        for page in &pages {
            let (format, n) = template.folio_number(page.index);
            assert_eq!(template.folio(page.index), format.format(n));
        }
        // And the parity implementation, whose defaults are the ones every other template inherits.
        let uniform = UniformTemplate::new(Thread::columns(&doc.page_setup, 1, 0.0));
        for i in 0..5 {
            let (format, n) = uniform.folio_number(i);
            assert_eq!(uniform.folio(i), format.format(n));
        }
    }

    // ---------------------------------------------------------------------------------------
    // Forced page breaks (spec 0080)
    // ---------------------------------------------------------------------------------------

    /// Whether a block's first line sits at the very top of the frame it was placed in.
    fn at_frame_top(pages: &[LaidOutPage], id: BlockId, template: &impl PageTemplate) -> bool {
        pages.iter().any(|p| {
            p.blocks.iter().any(|b| match b {
                PlacedBlock::Text { source, frame, .. } if *source == id => {
                    (frame.y_pt - frames_for(template, p.index)[0].rect.y_pt).abs() < 0.01
                }
                _ => false,
            })
        })
    }

    /// How many blocks a page of `many_lines` filler holds, derived rather than assumed — the
    /// fixtures below need to place a break mid-page and on a page of a chosen parity, and a
    /// hard-coded line count would silently stop meaning that if a metric moved.
    fn lines_per_page() -> usize {
        let doc = doc_with_blocks(many_lines(400));
        let pages = lay_out(&doc, &MONO, &NoHyphenator);
        pages[0].blocks.len()
    }

    /// `filler` lines, then a marked block carrying `kind`, then `after` more lines.
    fn doc_with_break(filler: usize, kind: BreakKind, after: usize) -> (Document, BlockId) {
        let mut content = many_lines(filler);
        content.push(Block::body("MARK", Color::Gray { v: 0.0 }));
        content.extend(many_lines(after));
        let mut doc = doc_with_blocks(content);
        doc.next_block_id = 0;
        doc.assign_missing_block_ids().expect("ids");
        let mark = doc.content[filler].id();
        doc.breaks = vec![PageBreak { before: mark, kind }];
        (doc, mark)
    }

    /// **The general mechanism**: content resumes on a new page, and the page it left keeps
    /// everything that was already on it.
    ///
    /// Both halves, for spec 0072's reason. A test asserting only that the marked block starts a
    /// page would pass against an implementation that started a page for every block; one asserting
    /// only that the previous page kept its lines would pass against one that broke nowhere. The
    /// same document without the break is asserted to run on, so the defect is in the test file
    /// rather than described in the spec.
    #[test]
    fn a_break_before_a_block_starts_a_page_and_the_previous_page_keeps_its_content() {
        let per_page = lines_per_page();
        let filler = per_page / 2;
        let (doc, mark) = doc_with_break(filler, BreakKind::Page, 4);
        let template = DocumentTemplate::new(&doc);

        let pages = lay_out(&doc, &MONO, &NoHyphenator);
        assert_eq!(page_of(&pages, mark), Some(1), "the mark starts a new page");
        assert!(
            at_frame_top(&pages, mark, &template),
            "and starts it at the top of the frame, not part way down"
        );
        assert_eq!(
            pages[0].blocks.len(),
            filler,
            "the page it left keeps every line it had — a break moves what follows, not what \
             preceded it"
        );

        // The same document with no break: the mark continues on page 0. This is what shipped
        // before this increment, and it is what spec 0079's known issue describes.
        let mut run_on = doc.clone();
        run_on.breaks.clear();
        let pages = lay_out(&run_on, &MONO, &NoHyphenator);
        assert_eq!(
            page_of(&pages, mark),
            Some(0),
            "without the break it runs on"
        );
        assert!(!at_frame_top(&pages, mark, &template));
    }

    /// A break at the top of a page the flow has not written to inserts nothing.
    ///
    /// The guard the whole mechanism rests on: without it, *every* break would emit a spurious blank
    /// page, because the first thing a break does is put the flow at the top of an empty page.
    /// Asserted from both ends — the document's very first block, which has never had a page to
    /// leave, and a block whose break the flow has already honoured.
    #[test]
    fn a_break_at_the_top_of_an_empty_page_inserts_nothing() {
        let mut doc = doc_with_blocks(many_lines(30));
        doc.next_block_id = 0;
        doc.assign_missing_block_ids().expect("ids");
        let first = doc.content[0].id();
        let second = doc.content[1].id();
        let plain = lay_out(&doc, &MONO, &NoHyphenator);

        // A break before the first block of the document.
        doc.breaks = vec![PageBreak {
            before: first,
            kind: BreakKind::Page,
        }];
        assert_eq!(
            lay_out(&doc, &MONO, &NoHyphenator),
            plain,
            "a break at the very start of a document is a no-op, page for page"
        );

        // Two breaks in a row: the second block's break arrives at the top of the page the first
        // one just started, and must add nothing.
        doc.breaks = vec![
            PageBreak {
                before: first,
                kind: BreakKind::Page,
            },
            PageBreak {
                before: second,
                kind: BreakKind::Page,
            },
        ];
        let pages = lay_out(&doc, &MONO, &NoHyphenator);
        assert_eq!(
            page_of(&pages, second),
            Some(1),
            "the second block breaks once"
        );
        assert_eq!(
            pages.len(),
            2,
            "and exactly once — no blank page between them"
        );
        assert_eq!(pages[1].blocks.len(), doc.content.len() - 1);
    }

    /// **A break advances a page, not a frame.** On a two-column page a break in the second column
    /// must not settle for the next column: the page still carries the previous chapter's text.
    #[test]
    fn a_break_advances_to_the_next_page_not_the_next_column() {
        // A two-column document, and a mark that lands part way down the *second* column of page 0
        // — the one arrangement in which advancing a frame and advancing a page give different
        // answers. The fixture is built by measuring where the mark actually falls rather than by
        // arithmetic over a line height, and the arrangement is asserted before the claim is: a
        // fixture that quietly stopped reaching the second column would make this test pass against
        // a frame-advancing implementation.
        let two_column = |doc: &mut Document| {
            doc.master_pages = vec![MasterPage {
                columns: 2,
                gutter_pt: 12.0,
                margins: Some(Margins::uniform(36.0)),
                ..MasterPage::plain("body")
            }];
            doc.default_master = Some("body".into());
        };
        let column_x = |pages: &[LaidOutPage], id: BlockId| {
            pages.iter().find_map(|p| {
                p.blocks.iter().find_map(|b| match b {
                    PlacedBlock::Text { source, frame, .. } if *source == id => {
                        Some((p.index, frame.x_pt, frame.y_pt))
                    }
                    _ => None,
                })
            })
        };

        // Find a filler count that puts the mark in the second column of page 0, part way down it.
        let mut fixture = None;
        for filler in 1..400 {
            let (mut doc, mark) = doc_with_break(filler, BreakKind::None, 4);
            two_column(&mut doc);
            let pages = lay_out(&doc, &MONO, &NoHyphenator);
            let columns = frames_for(&DocumentTemplate::new(&doc), 0);
            let Some((page, x, y)) = column_x(&pages, mark) else {
                continue;
            };
            // Part way down the first column: the flow has written to this frame and to no other.
            if page == 0
                && (x - columns[0].rect.x_pt).abs() < 0.01
                && y > columns[0].rect.y_pt + 0.01
                && pages[0].blocks.len() > filler
            {
                fixture = Some(filler);
                break;
            }
        }
        let filler = fixture.expect("no filler count puts the mark part way down the first column");

        let (mut doc, mark) = doc_with_break(filler, BreakKind::Page, 4);
        two_column(&mut doc);
        let template = DocumentTemplate::new(&doc);
        let pages = lay_out(&doc, &MONO, &NoHyphenator);

        let (page, x, y) = column_x(&pages, mark).expect("the mark");
        assert_eq!(page, 1, "the next page, not the next column");
        let columns = frames_for(&template, 1);
        assert_eq!(columns.len(), 2, "the fixture must really be two-column");
        assert!(
            (x - columns[0].rect.x_pt).abs() < 0.01,
            "and in the first column of it, at {x} against {}",
            columns[0].rect.x_pt
        );
        assert!((y - columns[0].rect.y_pt).abs() < 0.01, "at the top of it");
        // The page it left keeps both its columns' content.
        assert!(
            pages[0].blocks.len() >= filler,
            "the page it left keeps every line it had"
        );
    }

    /// **Next recto**: a blank page is inserted when the break lands on a verso, and is not when it
    /// lands on a recto. Both directions, because a test that only asserted the first would pass
    /// against an implementation that inserted a blank page every time.
    #[test]
    fn a_recto_break_inserts_a_blank_page_only_when_it_lands_on_a_verso() {
        let per_page = lines_per_page();

        // Landing on a verso: the flow is part way down page 0, so the next page is page 1.
        let (doc, mark) = doc_with_break(per_page / 2, BreakKind::Recto, 4);
        assert!(doc.page_setup.facing_pages, "parity needs a spread");
        let pages = lay_out(&doc, &MONO, &NoHyphenator);
        assert_eq!(
            page_of(&pages, mark),
            Some(2),
            "a recto, skipping the verso"
        );
        assert!(
            pages[1].blocks.is_empty(),
            "page 1 is the inserted blank: {:?}",
            pages[1].blocks.len()
        );
        assert!(
            quill_core_model::is_recto(2, true),
            "and page 2 really is a recto"
        );

        // Landing on a recto: the flow is part way down page 1, so the next page is page 2 and
        // nothing is inserted.
        let (doc, mark) = doc_with_break(per_page + per_page / 2, BreakKind::Recto, 4);
        let pages = lay_out(&doc, &MONO, &NoHyphenator);
        assert_eq!(page_of(&pages, mark), Some(2));
        assert!(
            !pages[1].blocks.is_empty(),
            "no blank page: page 1 carries the filler it always did"
        );

        // And the mirror rule, so `Recto` is a parity mechanism rather than a right-hand-page one.
        let (doc, mark) = doc_with_break(per_page / 2, BreakKind::Verso, 4);
        let pages = lay_out(&doc, &MONO, &NoHyphenator);
        assert_eq!(
            page_of(&pages, mark),
            Some(1),
            "a verso needs no blank here"
        );
        let (doc, mark) = doc_with_break(per_page + per_page / 2, BreakKind::Verso, 4);
        let pages = lay_out(&doc, &MONO, &NoHyphenator);
        assert_eq!(page_of(&pages, mark), Some(3), "and skips the recto");
        assert!(pages[2].blocks.is_empty(), "page 2 is the inserted blank");
    }

    /// **A blank page is still a page**: it takes its master's furniture and it consumes a folio.
    ///
    /// The second half is the one that would be a printed defect rather than a cosmetic one. If an
    /// inserted page did not count, every folio after it would be one out — the whole book, silently.
    #[test]
    fn a_blank_page_inserted_for_a_recto_break_carries_its_furniture_and_consumes_a_folio() {
        let per_page = {
            let doc = doc_with_folio(StaticAlign::Left, false, "{page}");
            lay_out(&doc, &MONO, &NoHyphenator)[0].blocks.len()
        };
        let mut doc = doc_with_folio(StaticAlign::Left, false, "{page}");
        let mut content = many_lines(per_page / 2);
        content.push(Block::body("MARK", Color::Gray { v: 0.0 }));
        content.extend(many_lines(4));
        doc.content = content;
        doc.next_block_id = 0;
        doc.assign_missing_block_ids().expect("ids");
        let mark = doc.content[per_page / 2].id();
        doc.breaks = vec![PageBreak {
            before: mark,
            kind: BreakKind::Recto,
        }];

        let pages = lay_out(&doc, &MONO, &NoHyphenator);
        assert_eq!(page_of(&pages, mark), Some(2));
        assert!(pages[1].blocks.is_empty(), "page 1 is blank of content");
        // Furniture: the blank page draws the master's folio like every other page.
        assert_eq!(
            static_text(&pages[1]),
            "2",
            "the blank page prints its folio"
        );
        // And it consumed a number: the page after it is 3, not 2.
        let printed: Vec<String> = pages.iter().map(static_text).collect();
        assert_eq!(
            printed,
            (1..=pages.len()).map(|n| n.to_string()).collect::<Vec<_>>(),
            "every page, once, in order — a blank page that consumed no number would repeat one"
        );
    }

    /// **A section starts on a new page**, which is what a chapter opener is: the opener master
    /// lands on a page that carries the chapter and nothing before it.
    ///
    /// Spec 0072 could put the master on the right page; it could not stop the previous chapter's
    /// tail sharing that page with it. Asserted on placed geometry, and against the same document
    /// without the break — which is exactly the run-on spec 0079 recorded.
    #[test]
    fn a_section_that_starts_on_a_new_page_gets_a_page_of_its_own() {
        use quill_core_model::Section;
        let per_page = lines_per_page();
        let mut content = many_lines(per_page / 2);
        content.push(Block::heading(1, "Chapter Two", Color::Gray { v: 0.0 }));
        content.extend(many_lines(6));
        let mut doc = doc_with_opener_and_body(content);
        doc.next_block_id = 0;
        doc.assign_missing_block_ids().expect("ids");
        let opener_block = doc.content[per_page / 2].id();
        doc.sections = vec![Section {
            name: "Chapter Two".into(),
            start: opener_block,
            master: Some("opener".into()),
            folio: None,
        }];

        // Without the break: the chapter opens part way down a page it shares with chapter one.
        let run_on = lay_out(&doc, &MONO, &NoHyphenator);
        let shared = page_of(&run_on, opener_block).expect("placed");
        assert!(
            run_on[shared]
                .blocks
                .iter()
                .any(|b| !matches!(b, PlacedBlock::Text { source, .. } if *source == opener_block)),
            "the recorded defect: the opener master's page still carries the previous chapter"
        );

        doc.breaks = vec![PageBreak {
            before: opener_block,
            kind: BreakKind::Page,
        }];
        let (pages, status) = lay_out_noted(&doc);
        assert!(status.converged, "{status:?}");
        let opened = page_of(&pages, opener_block).expect("placed");
        assert_eq!(
            pages[opened].blocks.first().and_then(|b| match b {
                PlacedBlock::Text { source, .. } => Some(*source),
                _ => None,
            }),
            Some(opener_block),
            "the chapter is the first thing on its page"
        );
        // The opener master really is the one in force there, and the chapter sits at the top of
        // *its* frame: the opener's 108 pt top margin puts the first line lower than the body
        // master's 36 pt would, which is what makes this an assertion about the master rather than
        // about the page number.
        let opener_y = pages[opened]
            .blocks
            .iter()
            .find_map(|b| match b {
                PlacedBlock::Text { source, frame, .. } if *source == opener_block => {
                    Some(frame.y_pt)
                }
                _ => None,
            })
            .expect("the chapter opener");
        assert!(
            opener_y > 100.0,
            "the opener master's deep top margin, not the body master's: {opener_y}"
        );
        let body_y = pages[opened - 1]
            .blocks
            .iter()
            .find_map(|b| match b {
                PlacedBlock::Text { frame, .. } => Some(frame.y_pt),
                _ => None,
            })
            .expect("the previous page's first line");
        assert!(body_y < 60.0, "against the body master's: {body_y}");
        assert!(
            pages[opened - 1]
                .blocks
                .iter()
                .all(|b| !matches!(b, PlacedBlock::Text { source, .. } if *source == opener_block)),
            "and the page before it is the previous chapter's, entire"
        );
    }

    /// **The resume contract, for a break.** Spec 0077's lesson, applied without waiting to be
    /// bitten by it: the contract is that resuming from *any* checkpoint reproduces a full pass, so
    /// the test is written against `flow` and every checkpoint the document records — not against
    /// the session, which picks its own resume points and would never have to pick the one that
    /// matters here (the checkpoint of a blank page inserted for parity).
    #[test]
    fn resuming_from_any_checkpoint_across_a_forced_break_reproduces_a_full_pass() {
        let per_page = lines_per_page();
        let mut content = many_lines(per_page / 2);
        content.push(Block::body("MARK ONE", Color::Gray { v: 0.0 }));
        content.extend(many_lines(per_page + 3));
        content.push(Block::body("MARK TWO", Color::Gray { v: 0.0 }));
        content.extend(many_lines(per_page * 2));
        // A third break, on a block tall enough to be **cut** across pages, so that the document
        // records checkpoints whose `split_at` is non-zero *inside* a block that carries a break.
        // Those are the checkpoints that distinguish "a break belongs to the seam in front of a
        // block" from "a break fires whenever the flow reaches this block".
        content.push(Block::body("long ".repeat(4000), Color::Gray { v: 0.0 }));
        let mut doc = doc_with_blocks(content);
        doc.next_block_id = 0;
        doc.assign_missing_block_ids().expect("ids");
        doc.breaks = vec![
            PageBreak {
                before: doc.content[per_page / 2].id(),
                kind: BreakKind::Recto,
            },
            PageBreak {
                before: doc.content[per_page / 2 + per_page + 4].id(),
                kind: BreakKind::Recto,
            },
            PageBreak {
                before: doc.content.last().expect("the long block").id(),
                kind: BreakKind::Recto,
            },
        ];
        let notes = doc.footnote_blocks();
        let template = DocumentTemplate::new(&doc);
        let components = doc.component_library();
        let run = |start: FlowState| {
            flow(
                &doc.content,
                &notes,
                &doc.breaks,
                &doc.assets,
                &doc.styles,
                &components,
                &[],
                &[],
                &BTreeMap::new(),
                &template,
                &MONO,
                &NoHyphenator,
                start,
                &mut NoCache,
                None,
            )
        };

        let full = run(FlowState::start(&template));
        assert!(
            full.pages.iter().any(|p| p.blocks.is_empty()),
            "the fixture exists to insert a blank page for parity; it inserted none"
        );
        assert_eq!(
            full.checkpoints.len(),
            full.pages.len(),
            "every page records where the flow had reached, blank pages included"
        );
        assert!(
            full.checkpoints.iter().any(|c| c.split_at > 0),
            "the fixture exists to record a checkpoint inside a block that carries a break"
        );
        for (i, checkpoint) in full.checkpoints.iter().enumerate() {
            let resumed = run(*checkpoint);
            assert_eq!(
                resumed.pages,
                full.pages[i..],
                "resuming at page {i} must reproduce the full pass"
            );
        }
    }

    /// A break belongs to the seam in **front of** a block, so a block that is cut across pages must
    /// not break again for its own remainder.
    ///
    /// The failure is not a hang — a re-fired break at a page start is a no-op for `Page`, and for
    /// `Recto` it is worse than a hang because it is quiet: the paragraph's continuation skips to
    /// the next recto and leaves a blank page in the middle of a paragraph. So the fixture is a
    /// parity break on a block tall enough to span pages, and the assertion is that its pages are
    /// consecutive.
    #[test]
    fn a_block_cut_across_pages_does_not_break_again_for_its_own_remainder() {
        let mut content = many_lines(lines_per_page() / 2);
        content.push(Block::body("long ".repeat(4000), Color::Gray { v: 0.0 }));
        let mut doc = doc_with_blocks(content);
        doc.next_block_id = 0;
        doc.assign_missing_block_ids().expect("ids");
        let long = doc.content.last().expect("the long block").id();
        doc.breaks = vec![PageBreak {
            before: long,
            kind: BreakKind::Recto,
        }];

        let pages = lay_out(&doc, &MONO, &NoHyphenator);
        let on: Vec<usize> = pages
            .iter()
            .filter(|p| {
                p.blocks
                    .iter()
                    .any(|b| matches!(b, PlacedBlock::Text { source, .. } if *source == long))
            })
            .map(|p| p.index)
            .collect();
        assert!(
            on.len() > 2,
            "the fixture must span several pages, got {on:?}"
        );
        for w in on.windows(2) {
            assert_eq!(
                w[1],
                w[0] + 1,
                "a paragraph continues on the very next page: {on:?}"
            );
        }
        assert!(
            pages[on[0]..].iter().all(|p| !p.blocks.is_empty()),
            "and no blank page is inserted inside it"
        );
    }

    /// A break is forward-only, not a derived quantity: it costs no fixpoint pass at all.
    #[test]
    fn a_break_costs_no_extra_fixpoint_pass() {
        let (doc, _) = doc_with_break(lines_per_page() / 2, BreakKind::Recto, 40);
        let (_, status) = lay_out_noted(&doc);
        assert!(status.converged, "{status:?}");
        assert_eq!(status.iterations, 1, "a break is settled inside the pass");
    }

    /// The claim every increment here owes: a document without the feature is untouched — the same
    /// page vector, not merely a similar one.
    #[test]
    fn a_document_with_no_break_lays_out_exactly_as_before() {
        let doc = doc_with_blocks(many_lines(200));
        assert!(doc.breaks.is_empty());
        let with_path = lay_out(&doc, &MONO, &NoHyphenator);
        let template = DocumentTemplate::new(&doc);
        let (bare, status) = lay_out_with_fixpoint_status(
            &doc.content,
            &doc.assets,
            &doc.styles,
            &template,
            &MONO,
            &NoHyphenator,
        );
        assert_eq!(status.iterations, 1);
        assert_eq!(
            with_path, bare,
            "the break path must be a no-op without one"
        );
        // A break naming a block the document does not contain is not an error and changes nothing
        // — a dangling section anchor's posture (spec 0072).
        let mut dangling = doc.clone();
        dangling.breaks = vec![PageBreak {
            before: BlockId(999_999),
            kind: BreakKind::Recto,
        }];
        assert_eq!(lay_out(&dangling, &MONO, &NoHyphenator), with_path);
        // And so is a break that states `None`, which is what a book file uses to decline one.
        let mut declined = doc.clone();
        declined.breaks = vec![PageBreak {
            before: doc.content[10].id(),
            kind: BreakKind::None,
        }];
        assert_eq!(lay_out(&declined, &MONO, &NoHyphenator), with_path);
    }
}
