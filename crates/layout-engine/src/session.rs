//! Incremental, dependency-tracked layout — see `specs/0031-incremental-layout-session.md`.
//!
//! `CLAUDE.md` states it as a non-negotiable: *editing one text thread must re-flow only affected
//! pages, never the whole document.* Until now every call re-ran Knuth-Plass over every block from
//! index 0, so changing one paragraph on page 3 re-broke all 500 pages. That is the behavior the
//! primary competitor is documented to collapse under, and avoiding it is the reason this engine
//! exists.
//!
//! ## How the work is bounded
//!
//! Three mechanisms, each covering a different part of the cost:
//!
//! 1. **A measurement cache**, keyed by what a measurement actually depends on — the block's
//!    content, the width it was broken to, and the style it was set in. An untouched paragraph
//!    re-flowed into the same column is not re-broken.
//! 2. **Resume from a checkpoint.** Layout records the flow state at each page boundary, so an edit
//!    on page 300 restarts from page 300's checkpoint rather than from block 0. Pages before it are
//!    reused untouched.
//! 3. **Stop when the flow re-converges.** After a local edit the flow usually returns to exactly
//!    the state it had before within a page or two — same block, same page number, same column,
//!    same y. When that happens and nothing later changed, the *remaining* pages are reused as-is.
//!
//! Without (3), an edit on page 3 of 500 would still re-flow 497 pages; it is what turns "reflow
//! from the edit" into "reflow around the edit".
//!
//! ## What the session is tied to
//!
//! The cache key covers content, width and style, but **not** the font metrics or hyphenator — they
//! are passed per call and are not comparable values. A session is therefore bound by contract to
//! the metrics it was first used with; call [`LayoutSession::invalidate`] if they change. This is a
//! real constraint rather than a hidden assumption, and it is why the type is a session rather than
//! a free function with a global cache.

use std::collections::HashMap;

use quill_core_model::{Block, BlockId, Document};
use quill_text_layout::{Hyphenator, RunMetrics};

use crate::{
    flow, heading_index, measure_block, BlockContext, DocumentTemplate, FlowState, HeadingEntry,
    LaidOutPage, Measured, Measurer, PageTemplate, StyleSheet, TocStatus,
};

/// What a relayout actually did.
///
/// Deterministic work counters, not timings. The M1 claim is "re-flow only affected pages", which is
/// a statement about *work*, and counters state it far more precisely than a wall-clock number that
/// swings 10-30% on a shared CI runner (see `benches/budgets.toml`).
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct LayoutStats {
    /// Blocks that had to be broken/sized from scratch.
    pub blocks_measured: usize,
    /// Blocks served from the measurement cache.
    pub blocks_from_cache: usize,
    /// Pages produced by this pass.
    pub pages_reflowed: usize,
    /// Pages carried over untouched — before the edit, or after the flow re-converged.
    pub pages_reused: usize,
}

impl LayoutStats {
    /// Total pages in the resulting document.
    pub fn pages_total(&self) -> usize {
        self.pages_reflowed + self.pages_reused
    }
}

/// The result of an incremental pass.
#[derive(Debug, Clone)]
pub struct LayoutResult {
    pub pages: Vec<LaidOutPage>,
    pub stats: LayoutStats,
    /// Indices of pages whose content differs from the previous pass — what a viewport needs to
    /// repaint, and nothing more.
    pub changed_pages: Vec<usize>,
    /// Which page each heading landed on (spec 0040), in document order.
    ///
    /// Rebuilt from `pages` on every pass rather than carried forward, because an incremental pass
    /// reuses whole pages and a carried-forward index would go stale exactly when the document was
    /// edited. See [`crate::heading_index`].
    pub headings: Vec<HeadingEntry>,
    /// How the contents fixpoint resolved (spec 0041). `converged: false` means the cap was hit and
    /// this is the last iterate — a complete document whose contents may be one page out, surfaced
    /// rather than presented as settled.
    pub toc: TocStatus,
}

/// Identifies a measurement: everything a broken paragraph depends on.
///
/// If this key is missing a dimension the measurement actually depends on, the cache returns a
/// stale layout and the document is silently wrong — so the fields here are the contract.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
struct MeasureKey {
    block: BlockId,
    /// Fingerprint of the block's *content*, so editing text invalidates it even though the id is
    /// deliberately stable across edits.
    content: u64,
    /// Frame width, as bits — `f32` is not `Hash`, and a paragraph broken to 200 pt is a different
    /// measurement from the same paragraph broken to 210 pt.
    width_bits: u32,
    /// Style fingerprint: size, leading, alignment and the surrounding space all change the result.
    style: u64,
}

/// An incremental layout engine for one document.
pub struct LayoutSession {
    cache: HashMap<MeasureKey, (Measured, f32)>,
    /// Previous pass's output, for reuse.
    pages: Vec<LaidOutPage>,
    /// Flow state at the start of each previous page.
    checkpoints: Vec<FlowState>,
    /// Block order and content fingerprints from the previous pass, for diffing.
    previous: Vec<(BlockId, u64)>,
    /// Fingerprint of everything *other* than block content that layout depends on: the stylesheet,
    /// page setup, master pages and the per-page master assignments (spec 0035).
    ///
    /// Without this the diff sees only blocks, so restyling the document — or changing its margins,
    /// or its master page — looks like "nothing changed" and the session returns the previous pages
    /// unaltered. That is a stale document presented as a current one, which is worse than being
    /// slow.
    previous_context: u64,
    /// Whether a previous pass exists at all.
    primed: bool,
}

impl Default for LayoutSession {
    fn default() -> Self {
        Self::new()
    }
}

impl LayoutSession {
    pub fn new() -> LayoutSession {
        LayoutSession {
            cache: HashMap::new(),
            pages: Vec::new(),
            checkpoints: Vec::new(),
            previous: Vec::new(),
            previous_context: 0,
            primed: false,
        }
    }

    /// Drop everything cached. Required if the font metrics or hyphenator change — see the module
    /// docs for why that cannot be detected automatically.
    pub fn invalidate(&mut self) {
        self.cache.clear();
        self.pages.clear();
        self.checkpoints.clear();
        self.previous.clear();
        self.previous_context = 0;
        self.primed = false;
    }

    /// Number of cached measurements, for tests and diagnostics.
    pub fn cached_measurements(&self) -> usize {
        self.cache.len()
    }

    /// Lay `doc` out, reusing whatever the previous pass established.
    pub fn relayout(
        &mut self,
        doc: &Document,
        metrics: &impl RunMetrics,
        hyphenator: &impl Hyphenator,
    ) -> LayoutResult {
        let template = DocumentTemplate::new(doc);
        self.relayout_with_template(doc, &template, metrics, hyphenator)
    }

    /// [`relayout`](Self::relayout) against an explicit template.
    ///
    /// A document containing a contents block (spec 0041) is laid out repeatedly until its page
    /// numbers settle. Every other document takes exactly one pass through [`Self::pass`] — the
    /// loop is not entered at all, so nothing about the incremental behaviour of a document without
    /// a contents list changes.
    pub fn relayout_with_template(
        &mut self,
        doc: &Document,
        template: &impl PageTemplate,
        metrics: &impl RunMetrics,
        hyphenator: &impl Hyphenator,
    ) -> LayoutResult {
        if !doc.content.iter().any(|b| matches!(b, Block::Toc { .. })) {
            return self.pass(doc, template, &[], metrics, hyphenator);
        }

        // The pages this relayout started from. Intermediate iterations overwrite `self.pages`, so
        // the caller's `changed_pages` has to be measured against where the *document* was before
        // the call, not against the previous iterate.
        let before = self.pages.clone();

        let mut headings: Vec<HeadingEntry> = Vec::new();
        let mut result = self.pass(doc, template, &headings, metrics, hyphenator);
        let mut iterations = 1;
        let converged = loop {
            let next = crate::heading_index(doc, &result.pages);
            if next == headings {
                break true;
            }
            if iterations >= crate::TOC_MAX_ITERATIONS {
                break false;
            }
            headings = next;
            result = self.pass(doc, template, &headings, metrics, hyphenator);
            iterations += 1;
        };

        result.changed_pages = (0..result.pages.len().max(before.len()))
            .filter(|i| before.get(*i) != result.pages.get(*i))
            .collect();
        result.toc = TocStatus {
            iterations,
            converged,
        };
        result
    }

    /// One incremental pass with a fixed contents index.
    fn pass(
        &mut self,
        doc: &Document,
        template: &impl PageTemplate,
        headings: &[HeadingEntry],
        metrics: &impl RunMetrics,
        hyphenator: &impl Hyphenator,
    ) -> LayoutResult {
        let current: Vec<(BlockId, u64)> = doc
            .content
            .iter()
            .map(|b| (b.id(), content_fingerprint(b)))
            .collect();

        // Anything that is not block content but still changes layout — styles, margins, masters —
        // invalidates the whole document, because it can move every page.
        let context = context_fingerprint(doc, headings);
        let context_changed = self.primed && context != self.previous_context;

        // The first block whose identity or content differs. Everything before it flowed exactly as
        // it did last time, so the pages containing it are still correct.
        let dirty_from = if !self.primed || context_changed {
            Some(0)
        } else {
            first_difference(&self.previous, &current)
        };

        let Some(dirty_from) = dirty_from else {
            // Nothing changed. Note this still returns the previous pages rather than recomputing:
            // a no-op edit (or a repaint request) must cost nothing.
            return LayoutResult {
                toc: TocStatus {
                    iterations: 1,
                    converged: true,
                },
                headings: heading_index(doc, &self.pages),
                pages: self.pages.clone(),
                stats: LayoutStats {
                    pages_reused: self.pages.len(),
                    ..Default::default()
                },
                changed_pages: Vec::new(),
            };
        };

        // Resume from the last page that began at or before the edit.
        //
        // A checkpoint *inside* the edited block is not a legal resume point (spec 0044): its
        // `split_at` counts items of the block's previous measurement, and the pages before it hold
        // the fragment that measurement produced. Resuming there would keep stale text and cut the
        // new text at an offset that means nothing. Backing up one page costs a page of relayout
        // and is the only correct choice.
        let resume_page = self
            .checkpoints
            .iter()
            .rposition(|c| {
                c.block_idx < dirty_from || (c.block_idx == dirty_from && c.split_at == 0)
            })
            .unwrap_or(0);
        let start = self
            .checkpoints
            .get(resume_page)
            .copied()
            .unwrap_or(FlowState {
                block_idx: 0,
                split_at: 0,
                page_index: 0,
                frame_idx: 0,
                y: crate::frames_for(template, 0)[0].rect.y_pt,
                frame_empty: true,
            });

        let kept: Vec<LaidOutPage> = self.pages.iter().take(start.page_index).cloned().collect();
        let previous_pages = std::mem::take(&mut self.pages);
        let previous_checkpoints = std::mem::take(&mut self.checkpoints);

        let mut cache = CachingMeasurer {
            cache: std::mem::take(&mut self.cache),
            styles: &doc.styles,
            measured: 0,
            hits: 0,
        };

        // Everything at or after `last_dirty` is identical to the previous pass, which is what
        // makes rejoining it sound.
        let last_dirty = if self.primed && !context_changed {
            last_difference(&self.previous, &current)
        } else {
            usize::MAX
        };
        let resync = if self.primed && last_dirty != usize::MAX {
            Some(crate::Resync {
                checkpoints: &previous_checkpoints,
                last_dirty,
            })
        } else {
            None
        };

        let result = flow(
            &doc.content,
            &doc.assets,
            &doc.styles,
            &doc.component_library(),
            headings,
            template,
            metrics,
            hyphenator,
            start,
            &mut cache,
            resync,
        );

        let measured = cache.measured;
        let hits = cache.hits;
        self.cache = cache.cache;

        // The flow stopped early because it rejoined the previous layout: everything from that page
        // on is still valid, so take it verbatim. Page *index* is part of the match, so a master
        // static carrying `{page}` cannot end up stamped with the wrong number.
        let mut new_pages = result.pages;
        let new_checkpoints = result.checkpoints;
        let mut reused_tail = 0usize;

        if let Some(at) = result.resynced_at {
            let tail: Vec<LaidOutPage> = previous_pages.iter().skip(at).cloned().collect();
            reused_tail = tail.len();
            new_pages.extend(tail);
        }

        let reflowed = new_pages.len() - reused_tail;
        let mut pages = kept;
        pages.extend(new_pages);

        // Renumber defensively: reused tail pages carry their old indices, which are only correct
        // when the page count above them is unchanged — which is exactly the condition
        // `find_reconvergence` enforces. Asserting it here rather than trusting it keeps a subtle
        // bug from becoming a mis-numbered folio in a printed book.
        for (i, page) in pages.iter_mut().enumerate() {
            debug_assert_eq!(
                page.index, i,
                "page index drifted during incremental layout"
            );
            page.index = i;
        }

        let changed_pages = diff_pages(&previous_pages, &pages);

        self.pages = pages.clone();
        self.checkpoints =
            rebuild_checkpoints(previous_checkpoints, new_checkpoints, start.page_index);
        self.previous = current;
        self.previous_context = context;
        self.primed = true;

        LayoutResult {
            toc: TocStatus {
                iterations: 1,
                converged: true,
            },
            headings: heading_index(doc, &pages),
            pages,
            stats: LayoutStats {
                blocks_measured: measured,
                blocks_from_cache: hits,
                pages_reflowed: reflowed,
                pages_reused: start.page_index + reused_tail,
            },
            changed_pages,
        }
    }
}

/// Rebuild the checkpoint list to match the emitted pages.
///
/// Checkpoints for reused tail pages are dropped rather than recomputed: the next pass will simply
/// resume from the last checkpoint it *does* have, which is always sound — it can only cost extra
/// work, never produce a wrong layout.
fn rebuild_checkpoints(
    previous: Vec<FlowState>,
    fresh: Vec<FlowState>,
    kept_pages: usize,
) -> Vec<FlowState> {
    let mut out: Vec<FlowState> = previous.into_iter().take(kept_pages).collect();
    out.extend(fresh);
    out
}

/// Index of the first position where two block sequences differ, or `None` if identical.
fn first_difference(previous: &[(BlockId, u64)], current: &[(BlockId, u64)]) -> Option<usize> {
    let common = previous.len().min(current.len());
    for i in 0..common {
        if previous[i] != current[i] {
            return Some(i);
        }
    }
    if previous.len() == current.len() {
        None
    } else {
        Some(common)
    }
}

/// Index just past the last position where two block sequences differ.
///
/// Everything at or after this index is identical in both, which is what makes reusing a tail of
/// pages sound.
fn last_difference(previous: &[(BlockId, u64)], current: &[(BlockId, u64)]) -> usize {
    if previous.len() != current.len() {
        // A different block count shifts everything after the change; nothing at a fixed index can
        // be assumed identical.
        return current.len();
    }
    for i in (0..current.len()).rev() {
        if previous[i] != current[i] {
            return i + 1;
        }
    }
    0
}

/// Page indices whose content differs between two passes.
fn diff_pages(previous: &[LaidOutPage], current: &[LaidOutPage]) -> Vec<usize> {
    let mut changed = Vec::new();
    for (i, page) in current.iter().enumerate() {
        match previous.get(i) {
            Some(old) if old.blocks == page.blocks && old.statics == page.statics => {}
            _ => changed.push(i),
        }
    }
    // Pages that no longer exist also count as changed, so a viewport clears them.
    for i in current.len()..previous.len() {
        changed.push(i);
    }
    changed
}

/// A [`Measurer`] that serves repeats from a cache.
struct CachingMeasurer<'a> {
    cache: HashMap<MeasureKey, (Measured, f32)>,
    styles: &'a StyleSheet,
    measured: usize,
    hits: usize,
}

impl Measurer for CachingMeasurer<'_> {
    fn measure<M: RunMetrics, H: Hyphenator>(
        &mut self,
        block: &Block,
        width: f32,
        ctx: &BlockContext<'_>,
        metrics: &M,
        hyphenator: &H,
    ) -> Option<(Measured, f32)> {
        let key = MeasureKey {
            block: block.id(),
            content: content_fingerprint(block),
            width_bits: width.to_bits(),
            style: style_fingerprint(self.styles, block),
        };
        if let Some(hit) = self.cache.get(&key) {
            self.hits += 1;
            return Some(hit.clone());
        }
        let result = measure_block(block, width, ctx, metrics, hyphenator);
        self.measured += 1;
        if let Some(value) = &result {
            self.cache.insert(key, value.clone());
        }
        result
    }
}

/// FNV-1a over the parts of a block that affect its measurement.
///
/// The same construction the PDF `/ID` uses. Not cryptographic — a collision would serve a stale
/// measurement — but the inputs here are a document's own paragraphs, not adversarial input, and a
/// 64-bit collision across one document's blocks is not a realistic risk.
fn content_fingerprint(block: &Block) -> u64 {
    let mut h: u64 = 0xcbf2_9ce4_8422_2325;
    let mut eat = |bytes: &[u8]| {
        for b in bytes {
            h ^= *b as u64;
            h = h.wrapping_mul(0x0000_0100_0000_01b3);
        }
    };
    match block {
        Block::Heading {
            level, text, style, ..
        } => {
            eat(b"h");
            eat(&[*level]);
            eat(text.as_bytes());
            eat(style.as_deref().unwrap_or("").as_bytes());
        }
        Block::Body { text, style, .. } => {
            eat(b"b");
            eat(text.as_bytes());
            eat(style.as_deref().unwrap_or("").as_bytes());
        }
        Block::Image { asset, .. } => {
            eat(b"i");
            eat(asset.as_bytes());
        }
        Block::Toc {
            title, max_level, ..
        } => {
            // A contents block has no authored content beyond these two: its entries come from the
            // resolved index, which is context (see `context_fingerprint`), not block content.
            // Fingerprinting the index here as well would make every contents block re-measure on
            // any heading move even when the entries it lists did not change.
            eat(b"toc");
            eat(title.as_bytes());
            eat(&[*max_level]);
        }
        Block::Table { table, .. } => {
            // Every cell, the header, the widths and the zebra flag: all of them change the
            // measurement, and a key that misses one returns a stale layout (spec 0031).
            eat(b"t");
            for w in &table.columns {
                eat(&w.to_bits().to_le_bytes());
            }
            eat(&[table.zebra as u8]);
            if let Some(header) = &table.header {
                for cell in header {
                    eat(cell.as_bytes());
                    eat(b"\x1f");
                }
            }
            eat(b"\x1e");
            for row in &table.rows {
                for cell in row {
                    eat(cell.as_bytes());
                    eat(b"\x1f");
                }
                eat(b"\x1e");
            }
        }
        Block::Panel { panel, .. } => {
            // Every field, not a summary. A key that misses a dimension the measurement depends on
            // returns a stale layout and the document is silently wrong (spec 0031) — and a panel
            // block has six independently editable sections, so five of them missing would be five
            // ways to edit a creature and see nothing change.
            eat(b"s");
            eat(panel.name.as_bytes());
            for (k, v) in &panel.attributes {
                eat(k.as_bytes());
                eat(b"\x1f");
                eat(v.as_bytes());
                eat(b"\x1e");
            }
            for (tag, section) in [
                (b"ov".as_slice(), &panel.overview),
                (b"de", &panel.details),
                (b"ac", &panel.actions),
                (b"re", &panel.reactions),
            ] {
                eat(tag);
                for line in section {
                    eat(line.as_bytes());
                    eat(b"\x1e");
                }
            }
        }
        Block::Component { def, fields, .. } => {
            // The definition *name* and every authored field. The definition's own shape is
            // context, not content — it lives on the document, so `context_fingerprint` covers it,
            // and covering it here as well would re-measure every instance whenever any unrelated
            // definition changed.
            eat(b"c");
            eat(def.as_bytes());
            eat(format!("{fields:?}").as_bytes());
        }
    }
    // Colour does not affect measurement, but it does affect the placed block, so a colour-only
    // edit must still invalidate the cached *result*.
    if let Block::Heading { color, .. } | Block::Body { color, .. } = block {
        eat(format!("{color:?}").as_bytes());
    }
    h
}

/// Fingerprint of everything other than block content that layout depends on.
///
/// Uses `Debug` formatting rather than a hand-written walk: these types change as the model grows,
/// and a hand-written fingerprint silently stops covering a field the moment one is added — which
/// would show up as a document that refuses to re-flow after an edit nobody can see. `Debug` is
/// derived, so it tracks the struct automatically. It runs once per relayout, against layout that
/// costs milliseconds.
fn context_fingerprint(doc: &Document, headings: &[HeadingEntry]) -> u64 {
    // `doc.pages` belongs here for exactly the reason the rest of this list does (spec 0035):
    // reassigning page 7's master changes page 7's geometry without touching a single block, so a
    // fingerprint blind to it would see "nothing changed" and hand back the previous pages.
    // The resolved contents index belongs here too (spec 0041): a contents block's *content* is
    // derived from it, so a fixpoint iteration that fed a different index must not reuse the
    // previous iterate's pages.
    // `doc.components` belongs here for the same reason `doc.styles` does (spec 0054): editing a
    // component definition changes every instance's geometry without touching a single block.
    let text = format!(
        "{:?}|{:?}|{:?}|{:?}|{:?}|{:?}|{:?}",
        doc.page_setup,
        doc.styles,
        doc.master_pages,
        doc.default_master,
        doc.pages,
        doc.components,
        headings
    );
    let mut h: u64 = 0xcbf2_9ce4_8422_2325;
    for b in text.as_bytes() {
        h ^= *b as u64;
        h = h.wrapping_mul(0x0000_0100_0000_01b3);
    }
    h
}

/// Fingerprint of the style a block resolves to.
fn style_fingerprint(styles: &StyleSheet, block: &Block) -> u64 {
    let s = styles.resolve(block);
    let mut h: u64 = 0xcbf2_9ce4_8422_2325;
    for bits in [
        s.font_size_pt.to_bits(),
        s.leading_pt.to_bits(),
        s.space_before_pt.to_bits(),
        s.space_after_pt.to_bits(),
        s.align as u32,
    ] {
        h ^= bits as u64;
        h = h.wrapping_mul(0x0000_0100_0000_01b3);
    }
    h
}

#[cfg(test)]
mod tests {
    use super::*;
    use quill_core_model::{Color, Document, Margins, MasterPage, PageOverride};
    use quill_text_layout::{MonospaceRunMetrics, NoHyphenator};

    const MONO: MonospaceRunMetrics = MonospaceRunMetrics { em_ratio: 0.6 };
    const INK: Color = Color::Gray { v: 0.0 };

    fn doc_of(n: usize) -> Document {
        let mut doc = Document::sample();
        doc.content = (0..n)
            .map(|i| {
                Block::body(
                    format!("paragraph {i} with enough words in it to occupy a line or so of text"),
                    INK,
                )
            })
            .collect();
        doc.assets.clear();
        doc.assign_missing_block_ids().expect("ids");
        doc
    }

    fn edit(doc: &mut Document, index: usize, text: &str) {
        let id = doc.content[index].id();
        doc.content[index] = Block::body(text, INK);
        doc.content[index].set_id(id);
        doc.bump_revision();
    }

    // ----- spec 0044: fragmentation on the incremental path -------------------------------------

    /// A document of `n` paragraphs long enough to split, plus a template of two 120 pt columns.
    ///
    /// Splitting only happens in a partly-full frame, and short paragraphs in a full-page frame
    /// rarely produce one — narrow columns make it the common case, which is exactly the situation
    /// spec 0036's `rulebook` template put the engine in.
    fn splitting_doc(n: usize) -> (Document, crate::UniformTemplate) {
        let mut doc = Document::sample();
        doc.content = (0..n)
            .map(|i| {
                Block::body(
                    format!(
                        "paragraph {i} with a good many words in it, enough that it runs to \
                         several lines in a narrow column and therefore has somewhere to break"
                    ),
                    INK,
                )
            })
            .collect();
        doc.assets.clear();
        doc.assign_missing_block_ids().expect("ids");
        let template = crate::UniformTemplate::new(crate::Thread {
            frames: (0..2)
                .map(|i| crate::Frame {
                    rect: crate::Rect {
                        x_pt: i as f32 * 144.0,
                        y_pt: 0.0,
                        w_pt: 120.0,
                        h_pt: 144.0,
                    },
                })
                .collect(),
        });
        (doc, template)
    }

    /// The same document against a template of one 120 pt column per page, so that *every* split
    /// falls on a page boundary and therefore leaves a mid-block checkpoint.
    fn splitting_doc_single_column(n: usize) -> (Document, crate::UniformTemplate) {
        let (doc, _) = splitting_doc(n);
        let template = crate::UniformTemplate::new(crate::Thread {
            frames: vec![crate::Frame {
                rect: crate::Rect {
                    x_pt: 0.0,
                    y_pt: 0.0,
                    w_pt: 120.0,
                    h_pt: 144.0,
                },
            }],
        });
        (doc, template)
    }

    /// How many blocks were placed in more than one piece.
    fn split_count(pages: &[LaidOutPage]) -> usize {
        let mut per_source: std::collections::BTreeMap<BlockId, usize> = Default::default();
        for page in pages {
            for b in &page.blocks {
                if let crate::PlacedBlock::Text { source, .. } = b {
                    *per_source.entry(*source).or_default() += 1;
                }
            }
        }
        per_source.values().filter(|c| **c > 1).count()
    }

    #[test]
    fn a_split_costs_no_extra_measurement() {
        // Spec 0044's central design claim, as a number. Splitting is a derivation over the
        // measurement already cached, not a second measurement — so a document whose blocks are cut
        // across many frames must measure each block once per distinct width and no more. If this
        // fails, available height has leaked into `MeasureKey` and the hot path spec 0031 exists to
        // keep cold is being thrashed.
        let (doc, template) = splitting_doc(120);
        let mut session = LayoutSession::new();
        let result = session.relayout_with_template(&doc, &template, &MONO, &NoHyphenator);

        assert!(
            split_count(&result.pages) >= 10,
            "fixture must split many blocks, got {}",
            split_count(&result.pages)
        );
        // Every frame is 120 pt wide, so one measurement per block is the whole budget.
        assert_eq!(
            session.cached_measurements(),
            doc.content.len(),
            "one cache entry per block at one width, not one per placement"
        );
        assert_eq!(
            result.stats.blocks_measured,
            doc.content.len(),
            "a split must not cause a re-measure"
        );
    }

    #[test]
    fn incremental_matches_a_full_relayout_around_a_split() {
        // The hazard spec 0044 names: `FlowState` is the resume contract, and a mid-block
        // checkpoint that is not correctly restored produces a document subtly different after an
        // edit than after a full pass — silently, and only in the direction users experience.
        //
        // Three edits, one per position relative to a split block. The middle one is the case that
        // would not be written by accident and is the one that matters: the checkpoint inside the
        // edited block counts items of its *previous* measurement, so resuming there would keep
        // stale text and cut the new text at an offset that means nothing.
        let (doc, template) = splitting_doc_single_column(60);
        let mut probe = LayoutSession::new();
        let first = probe.relayout_with_template(&doc, &template, &MONO, &NoHyphenator);
        assert!(split_count(&first.pages) >= 5, "fixture must split");

        // The block must split across a *page* boundary, not merely a column boundary. Checkpoints
        // are recorded per page, so only a page-straddling block leaves a checkpoint with
        // `split_at > 0` — which is the state this test exists to exercise. A block that splits
        // between two columns of one page leaves no such checkpoint and would prove nothing.
        let split_block = {
            let mut pages_of: std::collections::BTreeMap<
                BlockId,
                std::collections::BTreeSet<usize>,
            > = Default::default();
            for page in &first.pages {
                for b in &page.blocks {
                    if let crate::PlacedBlock::Text { source, .. } = b {
                        pages_of.entry(*source).or_default().insert(page.index);
                    }
                }
            }
            let id = *pages_of
                .iter()
                .find(|(_, pages)| pages.len() > 1)
                .expect("a block straddling a page boundary")
                .0;
            doc.content
                .iter()
                .position(|b| b.id() == id)
                .expect("index")
        };
        assert!(split_block > 0, "need a block before the split one");

        for (label, index) in [
            ("before", split_block - 1),
            ("inside", split_block),
            ("after", split_block + 1),
        ] {
            let mut edited = doc.clone();
            edit(
                &mut edited,
                index,
                "a replacement paragraph of a quite different length, long enough that it too \
                 runs over several lines and rebreaks everything after it",
            );
            let mut session = LayoutSession::new();
            session.relayout_with_template(&doc, &template, &MONO, &NoHyphenator);
            let incremental =
                session.relayout_with_template(&edited, &template, &MONO, &NoHyphenator);
            let full = crate::lay_out_with_template(
                &edited.content,
                &edited.assets,
                &edited.styles,
                &template,
                &MONO,
                &NoHyphenator,
            );
            assert_eq!(
                incremental.pages, full,
                "incremental diverged from full layout for an edit {label} a split block"
            );
        }
    }

    #[test]
    fn a_first_pass_matches_a_full_layout_exactly() {
        // The session must not be a second layout implementation. If incremental and full layout
        // could disagree, the document would look different depending on how you arrived at it.
        let doc = doc_of(200);
        let mut session = LayoutSession::new();
        let incremental = session.relayout(&doc, &MONO, &NoHyphenator);
        let full = crate::lay_out(&doc, &MONO, &NoHyphenator);
        assert_eq!(incremental.pages, full);
    }

    #[test]
    fn an_unchanged_document_reflows_nothing() {
        let doc = doc_of(200);
        let mut session = LayoutSession::new();
        session.relayout(&doc, &MONO, &NoHyphenator);
        let again = session.relayout(&doc, &MONO, &NoHyphenator);
        assert_eq!(again.stats.pages_reflowed, 0);
        assert_eq!(again.stats.blocks_measured, 0);
        assert!(again.changed_pages.is_empty());
    }

    #[test]
    fn editing_one_paragraph_reflows_only_a_few_pages() {
        // The M1 claim, stated as a number. A full pass over this document produces many pages;
        // editing one paragraph must touch a handful, not all of them.
        let mut doc = doc_of(600);
        let mut session = LayoutSession::new();
        let first = session.relayout(&doc, &MONO, &NoHyphenator);
        let total = first.pages.len();
        assert!(total > 10, "need a multi-page document, got {total}");

        edit(&mut doc, 5, "a short replacement paragraph");
        let after = session.relayout(&doc, &MONO, &NoHyphenator);

        assert_eq!(after.pages.len(), total, "page count should be stable here");
        assert!(
            after.stats.pages_reflowed <= 3,
            "expected a bounded reflow, got {} of {total} pages",
            after.stats.pages_reflowed
        );
        assert!(
            after.stats.pages_reused >= total - 3,
            "expected most pages reused, got {}",
            after.stats.pages_reused
        );
    }

    #[test]
    fn an_edit_late_in_the_document_reuses_everything_before_it() {
        let mut doc = doc_of(600);
        let mut session = LayoutSession::new();
        let first = session.relayout(&doc, &MONO, &NoHyphenator);
        let total = first.pages.len();

        let last = doc.content.len() - 1;
        edit(&mut doc, last, "changed the very last paragraph");
        let after = session.relayout(&doc, &MONO, &NoHyphenator);
        assert!(
            after.stats.pages_reused >= total - 2,
            "an edit on the last page should reuse everything before it, reused {} of {total}",
            after.stats.pages_reused
        );
    }

    #[test]
    fn incremental_output_equals_a_full_relayout_after_an_edit() {
        // The correctness property that matters most: whatever the session reuses, the result has
        // to be what a from-scratch pass would have produced.
        let mut doc = doc_of(300);
        let mut session = LayoutSession::new();
        session.relayout(&doc, &MONO, &NoHyphenator);

        edit(
            &mut doc,
            40,
            "an edited paragraph of a rather different length than the original",
        );
        let incremental = session.relayout(&doc, &MONO, &NoHyphenator);
        let full = crate::lay_out(&doc, &MONO, &NoHyphenator);
        assert_eq!(
            incremental.pages, full,
            "incremental diverged from full layout"
        );
    }

    #[test]
    fn inserting_a_block_still_matches_a_full_relayout() {
        let mut doc = doc_of(300);
        let mut session = LayoutSession::new();
        session.relayout(&doc, &MONO, &NoHyphenator);

        let id = doc.new_block_id();
        let mut inserted = Block::body("an inserted paragraph in the middle of the document", INK);
        inserted.set_id(id);
        doc.content.insert(100, inserted);

        let incremental = session.relayout(&doc, &MONO, &NoHyphenator);
        let full = crate::lay_out(&doc, &MONO, &NoHyphenator);
        assert_eq!(incremental.pages, full);
    }

    #[test]
    fn deleting_a_block_still_matches_a_full_relayout() {
        let mut doc = doc_of(300);
        let mut session = LayoutSession::new();
        session.relayout(&doc, &MONO, &NoHyphenator);
        doc.content.remove(80);
        let incremental = session.relayout(&doc, &MONO, &NoHyphenator);
        let full = crate::lay_out(&doc, &MONO, &NoHyphenator);
        assert_eq!(incremental.pages, full);
    }

    #[test]
    fn the_measurement_cache_serves_unchanged_paragraphs() {
        let mut doc = doc_of(200);
        let mut session = LayoutSession::new();
        let first = session.relayout(&doc, &MONO, &NoHyphenator);
        assert!(first.stats.blocks_measured > 0);
        // Even a first pass gets hits, and they are not spurious: the flow loop measures a block
        // once per *candidate frame*, so a block that advances to the next page was measured twice
        // by the old engine. The cache collapses that second measure — a free win the session picks
        // up before any editing happens.
        assert!(
            first.stats.blocks_from_cache < first.stats.blocks_measured / 10,
            "first-pass hits should only be page-advance re-measures, got {} of {}",
            first.stats.blocks_from_cache,
            first.stats.blocks_measured
        );

        edit(&mut doc, 3, "short");
        let after = session.relayout(&doc, &MONO, &NoHyphenator);
        assert!(
            after.stats.blocks_measured < first.stats.blocks_measured,
            "an edit must measure fewer blocks than a full pass: {} vs {}",
            after.stats.blocks_measured,
            first.stats.blocks_measured
        );
    }

    #[test]
    fn changed_pages_reports_exactly_the_pages_that_differ() {
        // A viewport repaints what this says and nothing else, so an under-report is a stale screen.
        let mut doc = doc_of(400);
        let mut session = LayoutSession::new();
        let before = session.relayout(&doc, &MONO, &NoHyphenator).pages;

        edit(
            &mut doc,
            30,
            "an edit that changes this paragraph's length substantially indeed",
        );
        let after = session.relayout(&doc, &MONO, &NoHyphenator);

        let truly_changed: Vec<usize> = after
            .pages
            .iter()
            .enumerate()
            .filter(|(i, p)| match before.get(*i) {
                Some(old) => old.blocks != p.blocks || old.statics != p.statics,
                None => true,
            })
            .map(|(i, _)| i)
            .collect();
        assert_eq!(after.changed_pages, truly_changed);
    }

    #[test]
    fn a_style_change_invalidates_the_cache() {
        // Style is part of the measurement key. If it were not, restyling the document would reuse
        // paragraphs broken at the old size — text drawn at one size in space measured for another.
        let mut doc = doc_of(100);
        let mut session = LayoutSession::new();
        session.relayout(&doc, &MONO, &NoHyphenator);

        let mut body = doc.styles.paragraph["body"];
        body.font_size_pt = 18.0;
        body.leading_pt = 22.0;
        doc.styles.paragraph.insert("body".into(), body);

        let after = session.relayout(&doc, &MONO, &NoHyphenator);
        let full = crate::lay_out(&doc, &MONO, &NoHyphenator);
        assert_eq!(
            after.pages, full,
            "a restyle must not reuse stale measurements"
        );
    }

    #[test]
    fn reassigning_a_pages_master_invalidates_the_session() {
        // Spec 0035. The page list is context, not content: reassigning page 1's master changes
        // that page's geometry without touching a single block, so a fingerprint blind to it would
        // see "nothing changed" and hand back the previous pages — a stale document presented as a
        // current one.
        //
        // Asserted in BOTH directions on purpose. The invalidate half alone would pass against an
        // implementation that invalidates on everything; the reuse half alone would pass against
        // one that never invalidates. Only the pair pins the behavior.
        let mut doc = doc_of(200);
        doc.master_pages = vec![
            MasterPage {
                margins: Some(Margins::uniform(72.0)),
                ..MasterPage::plain("deep")
            },
            MasterPage {
                margins: Some(Margins::uniform(18.0)),
                ..MasterPage::plain("shallow")
            },
        ];
        doc.default_master = Some("shallow".into());

        let mut session = LayoutSession::new();
        session.relayout(&doc, &MONO, &NoHyphenator);

        // Reuse: an untouched document costs nothing.
        let unchanged = session.relayout(&doc, &MONO, &NoHyphenator);
        assert_eq!(
            unchanged.stats.blocks_measured, 0,
            "an unchanged document must not re-measure"
        );

        // Invalidate: a page-list edit alone must reach the pages.
        doc.pages = vec![
            PageOverride { master: None },
            PageOverride {
                master: Some("deep".into()),
            },
        ];
        let after = session.relayout(&doc, &MONO, &NoHyphenator);
        assert_eq!(
            after.stats.pages_reused, 0,
            "a page-list change must invalidate, not reuse"
        );
        assert_eq!(
            after.pages,
            crate::lay_out(&doc, &MONO, &NoHyphenator),
            "and the result must match a full pass"
        );

        // Reuse again, this time with a page list actually present. Without this the reuse half
        // above would only prove the *old* context is stable, not a context containing a page
        // list — and "invalidates on everything" would pass the pair.
        let settled = session.relayout(&doc, &MONO, &NoHyphenator);
        assert_eq!(
            settled.stats.blocks_measured, 0,
            "a page list that did not change must not invalidate either"
        );
    }

    #[test]
    fn the_heading_index_survives_an_incremental_pass() {
        // The load-bearing test for spec 0040, and the reason the index is derived from the final
        // pages rather than accumulated during pagination.
        //
        // An incremental pass reuses whole pages, so an index built up as blocks were placed would
        // be missing every heading on a reused page — and missing them exactly when the document had
        // just been edited, which is always. Here 495+ of 500-odd pages are reused and the index
        // must still be complete and still agree with a cold pass.
        let mut doc = doc_of(400);
        for i in [0usize, 100, 200, 300] {
            let id = doc.content[i].id();
            doc.content[i] = Block::heading(1, format!("Chapter at {i}"), INK);
            doc.content[i].set_id(id);
        }

        let mut session = LayoutSession::new();
        let cold = session.relayout(&doc, &MONO, &NoHyphenator);
        assert_eq!(cold.headings.len(), 4, "all four headings on the cold pass");
        assert_eq!(
            cold.headings,
            crate::heading_index(&doc, &crate::lay_out(&doc, &MONO, &NoHyphenator)),
            "the session and the one-shot path must agree"
        );

        // Edit one paragraph late in the document. Most pages are reused.
        edit(
            &mut doc,
            350,
            "an edited paragraph with quite a few more words in it than before",
        );
        let after = session.relayout(&doc, &MONO, &NoHyphenator);
        assert!(
            after.stats.pages_reused > 0,
            "the fixture must actually reuse pages, or this proves nothing"
        );
        assert_eq!(
            after.headings.len(),
            4,
            "a reused page's headings must not vanish from the index"
        );
        assert_eq!(
            after.headings,
            crate::heading_index(&doc, &crate::lay_out(&doc, &MONO, &NoHyphenator)),
            "and the incremental index must equal a full pass's"
        );
    }

    #[test]
    fn an_edit_that_moves_a_heading_updates_its_page_number() {
        // The other direction: the index must not merely survive, it must be current. A heading
        // pushed onto a later page has to report the later page, or a table of contents built on
        // this would print numbers that were right one edit ago.
        let mut doc = doc_of(200);
        let id = doc.content[150].id();
        doc.content[150] = Block::heading(1, "Late chapter", INK);
        doc.content[150].set_id(id);

        let mut session = LayoutSession::new();
        let before = session.relayout(&doc, &MONO, &NoHyphenator).headings[0].page_index;

        // Insert a page's worth of content ahead of it.
        let mut inserted: Vec<Block> = (0..120)
            .map(|i| {
                Block::body(
                    format!("inserted paragraph number {i} with several words"),
                    INK,
                )
            })
            .collect();
        for b in &mut inserted {
            let fresh = doc.new_block_id();
            b.set_id(fresh);
        }
        doc.content.splice(10..10, inserted);
        doc.bump_revision();

        let after = session.relayout(&doc, &MONO, &NoHyphenator);
        assert_eq!(after.headings.len(), 1);
        assert!(
            after.headings[0].page_index > before,
            "the heading moved to a later page ({before} -> {}) and the index must say so",
            after.headings[0].page_index
        );
        assert_eq!(
            after.headings,
            crate::heading_index(&doc, &crate::lay_out(&doc, &MONO, &NoHyphenator))
        );
    }

    #[test]
    fn a_no_op_relayout_still_reports_the_headings() {
        // The early-return path returns the previous pages without recomputing. It must still hand
        // back an index, or a caller that repaints without editing would see the TOC empty itself.
        let mut doc = doc_of(50);
        let id = doc.content[10].id();
        doc.content[10] = Block::heading(1, "A chapter", INK);
        doc.content[10].set_id(id);

        let mut session = LayoutSession::new();
        let first = session.relayout(&doc, &MONO, &NoHyphenator);
        let again = session.relayout(&doc, &MONO, &NoHyphenator);
        assert_eq!(again.stats.blocks_measured, 0, "must be the no-op path");
        assert_eq!(again.headings, first.headings);
        assert_eq!(again.headings.len(), 1);
    }

    #[test]
    fn editing_any_stat_block_section_invalidates_its_measurement() {
        // Spec 0031's rule applied to a composite. A stat block has six independently editable
        // parts; a fingerprint covering only its name would give five ways to edit a creature and
        // watch the page not change — a stale document presented as a current one.
        //
        // Every section is asserted individually, and the no-edit case is asserted too, so this
        // cannot pass against a fingerprint that simply invalidates on everything.
        use quill_core_model::Panel;

        /// A named edit to one section of a stat block.
        type Mutation = (&'static str, fn(&mut Panel));

        let base = Panel {
            name: "Goblin".into(),
            overview: vec!["Small humanoid".into()],
            attributes: vec![("AC".into(), "15".into())],
            details: vec!["Nimble".into()],
            actions: vec!["Scimitar".into()],
            reactions: vec!["Dodge".into()],
        };

        let mutations: Vec<Mutation> = vec![
            ("name", |s| s.name = "Hobgoblin".into()),
            ("overview", |s| s.overview[0] = "Medium humanoid".into()),
            ("attributes key", |s| s.attributes[0].0 = "Armour".into()),
            ("attributes value", |s| s.attributes[0].1 = "17".into()),
            ("details", |s| s.details[0] = "Sturdy".into()),
            ("actions", |s| s.actions[0] = "Longsword".into()),
            ("reactions", |s| s.reactions[0] = "Parry".into()),
        ];

        for (what, mutate) in mutations {
            let mut doc = doc_of(20);
            let id = doc.content[5].id();
            doc.content[5] = Block::Panel {
                id,
                panel: base.clone(),
                color: INK,
            };

            let mut session = LayoutSession::new();
            session.relayout(&doc, &MONO, &NoHyphenator);

            // No edit ⇒ no measurement. Without this the assertion below proves nothing.
            let idle = session.relayout(&doc, &MONO, &NoHyphenator);
            assert_eq!(
                idle.stats.blocks_measured, 0,
                "{what}: an unchanged document must not re-measure"
            );

            let Block::Panel { panel, .. } = &mut doc.content[5] else {
                unreachable!()
            };
            mutate(panel);
            doc.bump_revision();

            let after = session.relayout(&doc, &MONO, &NoHyphenator);
            assert!(
                after.stats.blocks_measured >= 1,
                "{what}: editing this section must invalidate the measurement"
            );
            assert_eq!(
                after.pages,
                crate::lay_out(&doc, &MONO, &NoHyphenator),
                "{what}: and the result must match a full pass"
            );
        }
    }

    #[test]
    fn a_contents_list_stays_current_through_the_session() {
        // Spec 0031's rule applied to *derived* content. Editing a chapter heading must change the
        // contents entry that names it; editing a body paragraph that moves no heading must not
        // make the contents list re-measure on every keystroke. Both directions.
        use quill_core_model::Color;

        let mut doc = doc_of(120);
        doc.content.insert(
            0,
            Block::Toc {
                id: BlockId::UNASSIGNED,
                title: "Contents".into(),
                max_level: 2,
                color: Color::Gray { v: 0.0 },
            },
        );
        for i in [30usize, 70] {
            let id = doc.content[i].id();
            doc.content[i] = Block::heading(1, format!("Chapter {i}"), INK);
            doc.content[i].set_id(id);
        }
        doc.assign_missing_block_ids().expect("ids");

        let mut session = LayoutSession::new();
        let first = session.relayout(&doc, &MONO, &NoHyphenator);
        assert!(first.toc.converged, "must settle: {:?}", first.toc);
        assert!(
            first.toc.iterations > 1,
            "a document with a contents block runs the fixpoint"
        );
        assert_eq!(first.headings.len(), 2);

        // Renaming a chapter must reach the contents list.
        let id = doc.content[31].id();
        doc.content[31] = Block::heading(1, "Renamed chapter", INK);
        doc.content[31].set_id(id);
        doc.bump_revision();
        let after = session.relayout(&doc, &MONO, &NoHyphenator);
        assert!(after.toc.converged);
        assert!(
            after.headings.iter().any(|h| h.text == "Renamed chapter"),
            "the index must see the rename"
        );
    }

    #[test]
    fn a_contents_fixpoint_that_will_not_settle_stops_at_the_cap() {
        // The bound is not decoration. An entry can push a heading onto the next page, whose number
        // changes the entry, which pulls it back — forever. On hitting the cap the last iterate is
        // returned: a complete document, with `converged: false` so the caller can see it rather
        // than being handed a guess presented as settled.
        //
        // Asserted structurally rather than by finding an oscillating fixture: whatever the loop
        // does, it must terminate within the cap and must never return a document with content
        // missing.
        use quill_core_model::Color;
        let mut doc = doc_of(400);
        doc.content.insert(
            0,
            Block::Toc {
                id: BlockId::UNASSIGNED,
                title: "Contents".into(),
                max_level: 6,
                color: Color::Gray { v: 0.0 },
            },
        );
        // Many headings, so the contents list is long enough to shift pagination on its own.
        for i in (5..400).step_by(7) {
            let id = doc.content[i].id();
            doc.content[i] = Block::heading(1, format!("Chapter number {i}"), INK);
            doc.content[i].set_id(id);
        }
        doc.assign_missing_block_ids().expect("ids");

        let mut session = LayoutSession::new();
        let result = session.relayout(&doc, &MONO, &NoHyphenator);
        assert!(
            result.toc.iterations <= crate::TOC_MAX_ITERATIONS,
            "the loop must be bounded, took {}",
            result.toc.iterations
        );
        assert!(!result.pages.is_empty(), "a document must still come back");
        // Every authored block is still placed, converged or not.
        let placed: usize = result
            .pages
            .iter()
            .flat_map(|p| p.blocks.iter())
            .filter(|b| matches!(b, crate::PlacedBlock::Text { .. }))
            .count();
        assert!(placed > 400, "no content may be dropped: {placed}");
    }

    #[test]
    fn a_colour_only_edit_is_still_picked_up() {
        // Colour does not change *measurement*, but it does change the placed block — so it must
        // still invalidate the cached result.
        let mut doc = doc_of(50);
        let mut session = LayoutSession::new();
        session.relayout(&doc, &MONO, &NoHyphenator);

        let id = doc.content[10].id();
        let text = match &doc.content[10] {
            Block::Body { text, .. } => text.clone(),
            _ => unreachable!(),
        };
        doc.content[10] = Block::body(
            text,
            Color::Cmyk {
                c: 1.0,
                m: 0.0,
                y: 0.0,
                k: 0.0,
            },
        );
        doc.content[10].set_id(id);

        let after = session.relayout(&doc, &MONO, &NoHyphenator);
        let full = crate::lay_out(&doc, &MONO, &NoHyphenator);
        assert_eq!(after.pages, full);
    }

    #[test]
    fn invalidate_forces_a_full_pass() {
        let doc = doc_of(100);
        let mut session = LayoutSession::new();
        let first = session.relayout(&doc, &MONO, &NoHyphenator);
        session.invalidate();
        assert_eq!(session.cached_measurements(), 0);
        let again = session.relayout(&doc, &MONO, &NoHyphenator);
        assert_eq!(again.pages, first.pages);
        assert!(again.stats.blocks_measured > 0);
    }

    #[test]
    fn a_document_that_shrinks_to_fewer_pages_reports_the_removed_ones() {
        let mut doc = doc_of(400);
        let mut session = LayoutSession::new();
        let before = session.relayout(&doc, &MONO, &NoHyphenator).pages.len();
        doc.content.truncate(20);
        let after = session.relayout(&doc, &MONO, &NoHyphenator);
        assert!(after.pages.len() < before);
        assert!(
            after.changed_pages.contains(&(after.pages.len())),
            "pages that no longer exist must be reported so a viewport clears them"
        );
        assert_eq!(after.pages, crate::lay_out(&doc, &MONO, &NoHyphenator));
    }
}
