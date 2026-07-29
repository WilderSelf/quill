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

use std::collections::{BTreeMap, HashMap};

use quill_core_model::{Block, BlockId, Document};
use quill_text_layout::{Hyphenator, RunMetrics};

use crate::{
    flow, heading_index, measure_block, BlockContext, DocumentTemplate, FixpointStatus, FlowState,
    HeadingEntry, LaidOutPage, Measured, Measurer, PageTemplate, StyleSheet,
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
    /// How the layout fixpoint resolved (specs 0041, 0072). `converged: false` means the cap was hit
    /// and this is the last iterate — a complete document whose contents may be one page out, or
    /// whose chapter opener may be on the wrong page, surfaced rather than presented as settled.
    pub fixpoint: FixpointStatus,
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
    /// The block's list marker, hashed (spec 0066). Derived from the blocks *before* it in document
    /// order, so it is context rather than content: inserting an item at the top of a list changes
    /// every marker below it while changing none of their text. Without this in the key, a renumber
    /// would serve stale ordinals from the cache — the exact failure spec 0041 records for anything
    /// derived from position.
    marker: u64,
    /// The text this block's **generated** runs currently resolve to, hashed (specs 0076, 0077).
    /// `0` — and therefore the key this block has always had — when it contains none.
    ///
    /// **This is the increment's design decision, and it is a decision about where *not* to put
    /// it.** A cross-reference is derived from where its target landed, so it is context rather than
    /// content, exactly as a list marker is; the obvious home for context is
    /// [`context_fingerprint`], which is where the resolved heading index lives. That is affordable
    /// for a contents list because a document has *one*, and a changed context sets
    /// `dirty_from = Some(0)` — a whole-document reflow — once. A book has **hundreds** of
    /// cross-references, and any edit anywhere that moves any page moves at least one of them, so the
    /// same treatment would reflow and re-measure the entire document on every keystroke and put
    /// `benches/budgets.toml`'s `incremental_blocks_measured` — "editing one paragraph must not
    /// re-break the document" — permanently out of reach.
    ///
    /// Per-block, in the key, is spec 0066's `marker` at scale: the blocks whose *reference text*
    /// changed get a different key and re-measure, and every other block in the book keeps the key
    /// it had and is served from cache. It also owes no eviction, which is spec 0075's lesson — a
    /// changed derivation produces a different key rather than an entry that has to be found and
    /// dropped.
    /// Spec 0077 puts a second quantity through the identical mechanism rather than inventing one:
    /// a footnote number is derived from where the anchors sit in document order, so it is context
    /// exactly as a list marker is, and a note inserted at the front of a book renumbers every
    /// anchor after it without changing one character of their text.
    generated: u64,
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
    /// A document containing a contents block (spec 0041), or a section (spec 0072), is laid out
    /// repeatedly until both settle. Every other document takes exactly one pass through
    /// [`Self::pass`]: the loop is entered but exits on its first comparison, so nothing about the
    /// incremental behaviour of a document with neither changes.
    ///
    /// Deliberately the same loop as [`crate::lay_out_with_fixpoint_status`]'s, arm for arm — two
    /// fixpoints over the same two quantities, converging by different rules, would be a way for
    /// the app's pages and the exporter's to disagree.
    pub fn relayout_with_template(
        &mut self,
        doc: &Document,
        template: &impl PageTemplate,
        metrics: &impl RunMetrics,
        hyphenator: &impl Hyphenator,
    ) -> LayoutResult {
        let has_toc = doc.content.iter().any(|b| matches!(b, Block::Toc { .. }));
        let targets = quill_core_model::reference_targets(&doc.content);
        // Synthesised once per relayout rather than once per fixpoint pass (spec 0077), and empty
        // for every document without a footnote. Their numbers are derived from content order, so
        // they are known here and are constant across the loop's passes — a footnote is not a fourth
        // derived quantity and costs the fixpoint nothing.
        let notes = doc.footnote_blocks();
        let numbers = quill_core_model::footnote_numbers(&notes);

        // The pages this relayout started from. Intermediate iterations overwrite `self.pages`, so
        // the caller's `changed_pages` has to be measured against where the *document* was before
        // the call, not against the previous iterate.
        let before = self.pages.clone();

        // **Seeded from the previous relayout's pages, not from nothing.** Spec 0072 records that
        // both existing fixpoints restart from the underived state on every relayout, so a sectioned
        // document's first pass is always thrown away; it named that a cost sections *share* rather
        // than introduce, and left the optimisation untaken. A cross-reference cannot leave it
        // untaken. Starting from an empty map means every reference in the book prints `[?]` on the
        // first pass and its real folio on the second, so **every referring block would re-measure
        // twice on every keystroke** — the exact cost this increment exists to avoid, arriving
        // through the loop instead of through the cache key.
        //
        // It is sound because it only moves the loop's starting point: the exit condition still
        // compares two consecutive derivations, and the map that ships is always derived from the
        // pages that ship.
        let mut resolved = crate::reference_folios(&targets, &self.pages, template);
        resolved.extend(numbers.iter().map(|(k, v)| (*k, v.clone())));
        let mut headings: Vec<HeadingEntry> = Vec::new();
        let mut result = self.pass(
            doc, &notes, template, &headings, &resolved, metrics, hyphenator,
        );
        let mut reassigned = template.reassign(&result.pages);
        let mut iterations = 1;
        // Work counters accumulate across the whole relayout; the page counts below stay the final
        // pass's, because they describe the page vector this call emits. A fixpoint's cost is the
        // sum of its passes, and reporting only the last one would report a converged pass that
        // measured nothing and call it the price of the edit.
        let mut measured = result.stats.blocks_measured;
        let mut from_cache = result.stats.blocks_from_cache;
        let converged = loop {
            let next = if has_toc {
                crate::heading_index(doc, &result.pages)
            } else {
                Vec::new()
            };
            let mut next_refs = crate::reference_folios(&targets, &result.pages, template);
            next_refs.extend(numbers.iter().map(|(k, v)| (*k, v.clone())));
            if next == headings && next_refs == resolved && !reassigned {
                break true;
            }
            if iterations >= crate::FIXPOINT_MAX_ITERATIONS {
                break false;
            }
            headings = next;
            resolved = next_refs;
            result = self.pass(
                doc, &notes, template, &headings, &resolved, metrics, hyphenator,
            );
            reassigned = template.reassign(&result.pages);
            measured += result.stats.blocks_measured;
            from_cache += result.stats.blocks_from_cache;
            iterations += 1;
        };

        result.changed_pages = (0..result.pages.len().max(before.len()))
            .filter(|i| before.get(*i) != result.pages.get(*i))
            .collect();
        result.stats.blocks_measured = measured;
        result.stats.blocks_from_cache = from_cache;
        result.fixpoint = FixpointStatus {
            iterations,
            converged,
        };
        result
    }

    /// One incremental pass with a fixed contents index.
    #[allow(clippy::too_many_arguments)]
    fn pass(
        &mut self,
        doc: &Document,
        notes: &[Block],
        template: &impl PageTemplate,
        headings: &[HeadingEntry],
        resolved: &BTreeMap<BlockId, String>,
        metrics: &impl RunMetrics,
        hyphenator: &impl Hyphenator,
    ) -> LayoutResult {
        // The diff is over content, what each block's generated runs currently print, and the text
        // of the notes it anchors (specs 0076, 0077).
        //
        // Without the second term a pass whose references moved but whose text did not would see
        // "nothing changed" and hand back the previous pages — spec 0075's shape, in the one place
        // 0076 could reintroduce it.
        //
        // The third term exists because a footnote's **text is not in `content`**: editing a note
        // changes the band the frame its anchor lands in reserves, and a diff that walked only the
        // blocks would call that "nothing changed". The obvious home would be
        // `context_fingerprint` — and it is the wrong one for exactly 0076's reason: a changed
        // context sets `dirty_from = Some(0)`, so editing one note in a book with hundreds would
        // reflow the whole document. Per-block, it dirties the paragraph that anchors the note and
        // nothing else. It is deliberately **not** in `MeasureKey`: a note's text does not change
        // what its anchor's paragraph *measures*, only where the paragraph fits, so hashing it into
        // the key would re-break a paragraph that has not moved.
        //
        // All three are folded into the same `u64` rather than added as tuple elements, because the
        // diff only ever asks whether two entries differ.
        let current: Vec<(BlockId, u64)> = doc
            .content
            .iter()
            .map(|b| {
                (
                    b.id(),
                    content_fingerprint(b)
                        ^ generated_fingerprint(b, resolved)
                        ^ note_fingerprint(b, doc),
                )
            })
            .collect();

        // Anything that is not block content but still changes layout — styles, margins, masters —
        // invalidates the whole document, because it can move every page. Including what the
        // template derived this pass (spec 0072): the section-driven assignment is not on the
        // document, so a fingerprint over the document alone would call a pass that moved a chapter
        // opener "nothing changed" and hand back the previous iterate's pages.
        let context = context_fingerprint(doc, headings, template.derived_fingerprint());
        let context_changed = self.primed && context != self.previous_context;

        // A contents block is measured from the heading index, and the index is deliberately *not*
        // in `MeasureKey` — `content_fingerprint` says why: it is context, and hashing it into the
        // key would re-measure every contents block whenever any heading moved, even when the
        // entries it lists did not change.
        //
        // Nothing then evicted it, so a cached contents measurement outlived the index it was
        // derived from. **The fixpoint's second pass served the first pass's list from cache**, and
        // the first pass runs with an empty index by construction — so every document laid out
        // through a session placed a contents list consisting of nothing but its own title, on
        // every pass, for ever. Found by spec 0075, which could not otherwise show a contents list
        // spanning frames in the path the app actually uses.
        //
        // The eviction is exactly as narrow as the derivation: contents blocks only, and only when
        // the context they derive from moved. That is one re-measure per contents block per pass —
        // what a derived block costs — and it leaves every other cache entry, and therefore
        // `incremental_blocks_measured`, alone.
        if context_changed {
            let derived: std::collections::BTreeSet<BlockId> = doc
                .content
                .iter()
                .filter(|b| matches!(b, Block::Toc { .. }))
                .map(|b| b.id())
                .collect();
            if !derived.is_empty() {
                self.cache.retain(|k, _| !derived.contains(&k.block));
            }
        }

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
                fixpoint: FixpointStatus {
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
        // A resume at page 0 is a cold start, and a cold start belongs to the *current* template —
        // never to the checkpoint the previous pass recorded. A checkpoint carries the `y` the flow
        // began at, and page 0's `y` is the top of its text frame; when the context changed because
        // a master was reassigned (which is what a section does, spec 0072), that number moved.
        // Resuming on the stale one flowed page 0 from the old master's top margin under the new
        // master's frame — silently, and only where the two masters differ at the top.
        //
        // This was reachable before sections existed — reassigning `pages[0]` through a session does
        // it — and nothing caught it, because the session tests for spec 0035 assert that the pages
        // were *recomputed*, not where the recomputed text landed. Sections made it the common case
        // rather than an unusual edit.
        let start = if resume_page == 0 {
            FlowState::start(template)
        } else {
            self.checkpoints
                .get(resume_page)
                .copied()
                .unwrap_or_else(|| FlowState::start(template))
        };

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
            notes,
            &doc.assets,
            &doc.styles,
            &doc.component_library(),
            headings,
            resolved,
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

        // Furniture, for every page this pass emits — kept, reflowed and reused-tail alike (spec
        // 0074). This is where the tail-page reuse defect is actually closed: the block above takes
        // whole pages from the previous pass verbatim, including their statics, which was only ever
        // correct while a static was a pure function of `page_index`. A `{section}` or `{heading:N}`
        // running head is not, so a reused page kept the *previous* chapter's name — a wrong running
        // head on a printed page, with nothing anywhere to say so.
        //
        // Resolving it here rather than trying to work out which reused pages are still valid is
        // deliberate: the condition is "did any heading at or before this page move", which is every
        // page below an edit, so the narrow version is the broad one plus a way to get it wrong.
        // It costs one measured line per static per page and no re-measurement of any block.
        //
        // `heading_index` is computed once and used twice — here and in the result — rather than
        // walked twice.
        let headings_now = heading_index(doc, &pages);
        crate::place_statics(template, &mut pages, &headings_now, metrics);

        let changed_pages = diff_pages(&previous_pages, &pages);

        self.pages = pages.clone();
        self.checkpoints =
            rebuild_checkpoints(previous_checkpoints, new_checkpoints, start.page_index);
        self.previous = current;
        self.previous_context = context;
        self.primed = true;

        LayoutResult {
            fixpoint: FixpointStatus {
                iterations: 1,
                converged: true,
            },
            headings: headings_now,
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
            marker: marker_fingerprint(ctx.markers.get(&block.id())),
            // Off the same `ctx` the measurement itself resolves from, so the key and the
            // measurement cannot disagree about what a generated run says.
            generated: generated_fingerprint(block, ctx.resolved),
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
    // A run's *text* changes a measurement; its colour does not, and joins the colour tail below so
    // a colour-only edit invalidates the placed result without forcing a re-measure. Keeping that
    // two-tier split is the point of spec 0031's key, and spec 0063 must not blunt it.

    let mut eat = |bytes: &[u8]| {
        for b in bytes {
            h ^= *b as u64;
            h = h.wrapping_mul(0x0000_0100_0000_01b3);
        }
    };
    match block {
        Block::Heading {
            level, runs, style, ..
        } => {
            eat(b"h");
            eat(&[*level]);
            for r in runs {
                eat(r.text.as_bytes());
                // Everything about a run that changes what it measures (spec 0064). Size, tracking,
                // weight and slant each move an advance, so each belongs here rather than in the
                // colour tail below — a key missing a dimension the measurement depends on returns
                // a stale layout and the document is silently wrong.
                eat(measured_style(r).as_bytes());
                // Which block a cross-reference points at is *authored* (spec 0076): re-pointing a
                // reference is an edit, and it must mark this block dirty even when the two targets
                // happen to sit on the same page today. What the reference currently *says* is
                // context and lives in `MeasureKey::references` instead.
                eat(format!("{:?}", r.source).as_bytes());
                // A boundary byte, so ["ab"] and ["a","b"] cannot collide: same text, different
                // runs, and one of them can be recoloured mid-word.
                eat(&[0xff]);
            }
            eat(style.as_deref().unwrap_or("").as_bytes());
        }
        Block::Body { runs, style, .. } => {
            eat(b"b");
            for r in runs {
                eat(r.text.as_bytes());
                // Everything about a run that changes what it measures (spec 0064). Size, tracking,
                // weight and slant each move an advance, so each belongs here rather than in the
                // colour tail below — a key missing a dimension the measurement depends on returns
                // a stale layout and the document is silently wrong.
                eat(measured_style(r).as_bytes());
                // Which block a cross-reference points at is *authored* (spec 0076): re-pointing a
                // reference is an edit, and it must mark this block dirty even when the two targets
                // happen to sit on the same page today. What the reference currently *says* is
                // context and lives in `MeasureKey::references` instead.
                eat(format!("{:?}", r.source).as_bytes());
                // A boundary byte, so ["ab"] and ["a","b"] cannot collide: same text, different
                // runs, and one of them can be recoloured mid-word.
                eat(&[0xff]);
            }
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
    // edit must still invalidate the cached *result*. A baseline shift is the same shape of thing:
    // it moves a glyph vertically without changing an advance, so it cannot move a break either
    // (spec 0064).
    if let Block::Heading { color, runs, .. } | Block::Body { color, runs, .. } = block {
        eat(format!("{color:?}").as_bytes());
        for r in runs {
            eat(format!("{:?}", r.style.color).as_bytes());
            eat(format!("{:?}", r.style.baseline_shift_pt).as_bytes());
        }
    }
    h
}

/// The part of a run's style that changes what it measures (spec 0064).
///
/// Spelled out field by field rather than `Debug`-ing the whole `InlineStyle`, because the two
/// fields it leaves out — colour and baseline shift — must land in the *tail* instead. A run
/// recoloured mid-word has to invalidate the placed result without invalidating the line breaking
/// that the measurement cache holds.
fn measured_style(run: &quill_core_model::Run) -> String {
    format!(
        "{:?}|{:?}|{:?}|{:?}",
        run.style.size_pt, run.style.tracking_pt, run.style.weight, run.style.italic
    )
}

/// Fingerprint of everything other than block content that layout depends on.
///
/// Uses `Debug` formatting rather than a hand-written walk: these types change as the model grows,
/// and a hand-written fingerprint silently stops covering a field the moment one is added — which
/// would show up as a document that refuses to re-flow after an edit nobody can see. `Debug` is
/// derived, so it tracks the struct automatically. It runs once per relayout, against layout that
/// costs milliseconds.
fn context_fingerprint(doc: &Document, headings: &[HeadingEntry], derived: u64) -> u64 {
    // `doc.pages` belongs here for exactly the reason the rest of this list does (spec 0035):
    // reassigning page 7's master changes page 7's geometry without touching a single block, so a
    // fingerprint blind to it would see "nothing changed" and hand back the previous pages.
    // The resolved contents index belongs here too (spec 0041): a contents block's *content* is
    // derived from it, so a fixpoint iteration that fed a different index must not reuse the
    // previous iterate's pages.
    // `doc.components` belongs here for the same reason `doc.styles` does (spec 0054): editing a
    // component definition changes every instance's geometry without touching a single block.
    // `doc.sections` belongs here because a section drives the master assignment (spec 0072):
    // re-anchoring a section, or renaming the master it opens with, changes page geometry without
    // touching a block. Note what that costs and why it is accepted: a changed context sets
    // `dirty_from = Some(0)`, a whole-document reflow. A section list is authored and changes about
    // as often as a master page does, which is the company it is keeping here — unlike a
    // cross-reference (spec 0076), of which a book has hundreds and which must therefore go in
    // `MeasureKey` instead.
    let text = format!(
        "{:?}|{:?}|{:?}|{:?}|{:?}|{:?}|{:?}|{:?}|{derived}",
        doc.page_setup,
        doc.styles,
        doc.master_pages,
        doc.default_master,
        doc.pages,
        doc.sections,
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
/// Hash of a block's list marker, or of its absence.
fn marker_fingerprint(marker: Option<&String>) -> u64 {
    let mut h: u64 = 0xcbf2_9ce4_8422_2325;
    for b in marker.map_or(&b""[..], |m| m.as_bytes()) {
        h ^= *b as u64;
        h = h.wrapping_mul(0x0000_0100_0000_01b3);
    }
    h
}

/// Hash of what this block's generated runs currently print (specs 0076, 0077).
///
/// `0` for a block with no generated run — a distinguished value rather than a hash of nothing, so
/// that every block in every document without the feature keeps the exact [`MeasureKey`] it had and
/// its cached measurement is untouched. That is the whole perf claim, stated as one branch.
///
/// The runs' resolved values in run order, so two references to the same target in one paragraph are
/// not confused with one, and so a target whose folio is *absent* — unresolved — hashes differently
/// from one that resolves to the empty string.
fn generated_fingerprint(block: &Block, resolved: &BTreeMap<BlockId, String>) -> u64 {
    let runs = block.runs();
    if runs.iter().all(|r| r.source.is_authored()) {
        return 0;
    }
    let mut h: u64 = 0xcbf2_9ce4_8422_2325;
    let mut eat = |bytes: &[u8]| {
        for b in bytes {
            h ^= *b as u64;
            h = h.wrapping_mul(0x0000_0100_0000_01b3);
        }
    };
    for run in runs {
        let Some(referent) = run.source.referent() else {
            continue;
        };
        match resolved.get(&referent) {
            Some(folio) => eat(folio.as_bytes()),
            None => eat(quill_core_model::UNRESOLVED_REFERENCE.as_bytes()),
        }
        eat(&[0xff]);
    }
    h
}

/// Hash of the text of the notes this block anchors (spec 0077).
///
/// `0` — the key the block has always had — for a block with no footnote anchor, which is every
/// block in every document without the feature. Hashed in note-anchor order, so moving an anchor
/// within the paragraph changes it.
///
/// A note the document does not hold hashes as its absence, so *adding* the missing note dirties the
/// paragraph that called it.
fn note_fingerprint(block: &Block, doc: &Document) -> u64 {
    let runs = block.runs();
    if runs.iter().all(|r| r.source.note().is_none()) {
        return 0;
    }
    let mut h: u64 = 0xcbf2_9ce4_8422_2325;
    let mut eat = |bytes: &[u8]| {
        for b in bytes {
            h ^= *b as u64;
            h = h.wrapping_mul(0x0000_0100_0000_01b3);
        }
    };
    for id in runs.iter().filter_map(|r| r.source.note()) {
        match doc.footnotes.iter().find(|f| f.id == id) {
            Some(note) => {
                for run in &note.runs {
                    eat(run.text.as_bytes());
                    eat(format!("{:?}", run.style).as_bytes());
                    eat(run.character.as_deref().unwrap_or("").as_bytes());
                    eat(&[0xff]);
                }
                eat(format!("{:?}|{:?}", note.color, note.style).as_bytes());
            }
            None => eat(b"\x00missing"),
        }
        eat(&[0xfe]);
    }
    h
}

/// Everything about the *styles* a block resolves against that could move its layout.
///
/// `Debug` over the whole resolved `ParagraphStyle` rather than a hand-picked list of fields, for
/// the reason the ambient fingerprint below gives: a hand-written list silently stops covering a
/// field the moment the model grows one, and the symptom is a document that refuses to re-flow after
/// an edit nobody can see. It had already stopped covering three — `indent` (spec 0048), `list`
/// (0066) and `weight`/`italic` (0064) — each of which moves a glyph.
///
/// Plus the character styles this block's runs actually name (spec 0065), resolved, so that editing
/// one invalidates the blocks that use it and leaves the rest of the document cached.
fn style_fingerprint(styles: &StyleSheet, block: &Block) -> u64 {
    let mut h: u64 = 0xcbf2_9ce4_8422_2325;
    let mut eat = |bytes: &[u8]| {
        for b in bytes {
            h ^= *b as u64;
            h = h.wrapping_mul(0x0000_0100_0000_01b3);
        }
    };
    eat(format!("{:?}", styles.resolve(block)).as_bytes());
    if let Block::Heading { runs, .. } | Block::Body { runs, .. } = block {
        for name in runs.iter().filter_map(|r| r.character.as_deref()) {
            eat(name.as_bytes());
            eat(format!("{:?}", styles.character(name)).as_bytes());
        }
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
    fn incremental_matches_a_full_relayout_around_a_split_contents_list() {
        // Spec 0075 gave the contents list break opportunities, so a page boundary can now fall
        // *inside* one — a checkpoint with `split_at > 0` on a derived block. That is the resume
        // contract spec 0044 flagged as the hazard, reached by a new route: the contents list is
        // regenerated from the heading index on every pass, so a stale offset into it would resume
        // against a different item list and diverge silently.
        //
        // The edit is placed after the contents list, where it moves page numbers and therefore
        // re-derives the very block the checkpoint sits inside.
        use quill_core_model::Color;
        let mut doc = doc_of(500);
        doc.content.insert(
            0,
            Block::Toc {
                id: BlockId::UNASSIGNED,
                title: "Contents".into(),
                max_level: 6,
                color: Color::Gray { v: 0.0 },
            },
        );
        for i in (5..500).step_by(4) {
            let id = doc.content[i].id();
            doc.content[i] = Block::heading(1, format!("Chapter number {i}"), INK);
            doc.content[i].set_id(id);
        }
        doc.assign_missing_block_ids().expect("ids");

        let mut probe = LayoutSession::new();
        let first = probe.relayout(&doc, &MONO, &NoHyphenator);
        let toc_id = doc.content[0].id();
        let toc_pages: std::collections::BTreeSet<usize> = first
            .pages
            .iter()
            .filter(|p| {
                p.blocks.iter().any(
                    |b| matches!(b, crate::PlacedBlock::Text { source, .. } if *source == toc_id),
                )
            })
            .map(|p| p.index)
            .collect();
        assert!(
            toc_pages.len() >= 2,
            "the fixture must produce a contents list that straddles a page boundary: {toc_pages:?}"
        );

        let mut edited = doc.clone();
        edit(
            &mut edited,
            400,
            "a replacement paragraph of a quite different length, long enough that it too runs \
             over several lines and rebreaks everything after it",
        );
        let mut session = LayoutSession::new();
        session.relayout(&doc, &MONO, &NoHyphenator);
        let incremental = session.relayout(&edited, &MONO, &NoHyphenator);
        let full = crate::lay_out(&edited, &MONO, &NoHyphenator);
        assert_eq!(
            incremental.pages, full,
            "incremental diverged from a full layout across a split contents list"
        );
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

    /// Spec 0064: a run edit that moves a glyph must invalidate the *measurement*, not just the
    /// placed result.
    ///
    /// The failure this guards against is the one `MeasureKey`'s own doc comment names: a key
    /// missing a dimension the measurement depends on returns a stale layout, and the document is
    /// silently wrong. Size, tracking, weight and slant each change an advance and so each belongs
    /// in the content hash; colour and baseline shift move nothing horizontally and belong in the
    /// tail beside it.
    #[test]
    fn an_edit_that_moves_a_glyph_re_measures_and_one_that_does_not_still_repaints() {
        use quill_core_model::{InlineStyle, Run, Weight};

        let moves_a_glyph = [
            InlineStyle {
                size_pt: Some(14.0),
                ..InlineStyle::EMPTY
            },
            InlineStyle {
                tracking_pt: Some(0.4),
                ..InlineStyle::EMPTY
            },
            InlineStyle {
                weight: Some(Weight::BOLD),
                ..InlineStyle::EMPTY
            },
            InlineStyle {
                italic: Some(true),
                ..InlineStyle::EMPTY
            },
        ];
        for style in moves_a_glyph {
            let mut doc = doc_of(40);
            let mut session = LayoutSession::new();
            session.relayout(&doc, &MONO, &NoHyphenator);

            let id = doc.content[3].id();
            let mut edited = Block::body_runs(
                vec![
                    Run::plain("paragraph 3 with "),
                    Run {
                        text: "enough".into(),
                        style,
                        character: None,
                        source: Default::default(),
                    },
                    Run::plain(" words in it to occupy a line or so of text"),
                ],
                INK,
            );
            edited.set_id(id);
            doc.content[3] = edited;
            doc.bump_revision();

            let after = session.relayout(&doc, &MONO, &NoHyphenator);
            assert!(
                after.stats.blocks_measured > 0,
                "{style:?} changed an advance and must re-measure"
            );
        }
    }

    #[test]
    fn a_baseline_shift_repaints_without_re_breaking_the_paragraph() {
        // The other side of the same split: a shift moves a glyph *vertically*, so it cannot move a
        // break — but it does reach the page, so the cached placed result must not be reused.
        use quill_core_model::{InlineStyle, Run};

        let mut doc = doc_of(40);
        let mut session = LayoutSession::new();
        session.relayout(&doc, &MONO, &NoHyphenator);

        let plain_runs = vec![
            Run::plain("paragraph 3 with "),
            Run::plain("enough"),
            Run::plain(" words in it to occupy a line or so of text"),
        ];
        let id = doc.content[3].id();
        let mut same_text = Block::body_runs(plain_runs.clone(), INK);
        same_text.set_id(id);
        doc.content[3] = same_text;
        doc.bump_revision();
        session.relayout(&doc, &MONO, &NoHyphenator);

        let mut shifted_runs = plain_runs;
        shifted_runs[1].style = InlineStyle {
            baseline_shift_pt: Some(3.0),
            ..InlineStyle::EMPTY
        };
        let mut shifted = Block::body_runs(shifted_runs, INK);
        shifted.set_id(id);
        doc.content[3] = shifted;
        doc.bump_revision();

        let after = session.relayout(&doc, &MONO, &NoHyphenator);
        assert!(
            !after.changed_pages.is_empty(),
            "a shifted run still changes what is drawn"
        );
    }

    #[test]
    fn recolouring_a_run_invalidates_the_page_it_is_on() {
        // A run's colour is ink that reaches the page, so an edit to it has to invalidate the
        // cached result — exactly as a *block's* colour already does (`content_fingerprint` folds
        // colour in for this reason, accepting a re-measure to keep one key rather than two).
        // Without spec 0063 folding the run's own override in, recolouring a word would change
        // nothing the cache could see and the page would keep its stale ink.
        let mut doc = doc_of(20);
        doc.content[3] = Block::body_runs(
            vec![
                quill_core_model::Run::plain("A lead-in phrase "),
                quill_core_model::Run::plain("and the rest of the sentence carries on."),
            ],
            Color::Gray { v: 0.0 },
        );
        doc.assign_missing_block_ids().expect("ids");
        let mut session = LayoutSession::new();
        session.relayout(&doc, &MONO, &NoHyphenator);

        let Block::Body { runs, .. } = &mut doc.content[3] else {
            unreachable!()
        };
        runs[0].style.color = Some(Color::Cmyk {
            c: 0.0,
            m: 1.0,
            y: 1.0,
            k: 0.0,
        });
        doc.bump_revision();
        let after = session.relayout(&doc, &MONO, &NoHyphenator);
        assert!(
            !after.changed_pages.is_empty(),
            "the page must repaint, or the new ink never reaches it"
        );
    }

    #[test]
    fn editing_a_runs_text_does_re_measure_the_block() {
        // The other half: the two-tier split is only worth anything if the measurement tier still
        // fires. Text is metrics.
        let mut doc = doc_of(20);
        doc.content[3] = Block::body_runs(
            vec![quill_core_model::Run::plain("A phrase that will grow.")],
            Color::Gray { v: 0.0 },
        );
        doc.assign_missing_block_ids().expect("ids");
        let mut session = LayoutSession::new();
        session.relayout(&doc, &MONO, &NoHyphenator);

        let Block::Body { runs, .. } = &mut doc.content[3] else {
            unreachable!()
        };
        runs.push(quill_core_model::Run::plain(
            " And another clause after it.",
        ));
        doc.bump_revision();
        let after = session.relayout(&doc, &MONO, &NoHyphenator);
        assert!(
            after.stats.blocks_measured > 0,
            "changing a run's text changes the measurement"
        );
    }

    #[test]
    fn splitting_a_paragraph_into_runs_is_not_the_same_content() {
        // ["ab"] and ["a","b"] set the same characters but are not the same document: one of them
        // can be recoloured mid-word. The fingerprint has to be able to tell them apart, or an
        // edit that only moves a boundary would serve a stale placed result.
        let one = Block::body_runs(
            vec![quill_core_model::Run::plain("abcd")],
            Color::Gray { v: 0.0 },
        );
        let two = Block::body_runs(
            vec![
                quill_core_model::Run::plain("ab"),
                quill_core_model::Run::plain("cd"),
            ],
            Color::Gray { v: 0.0 },
        );
        assert_ne!(content_fingerprint(&one), content_fingerprint(&two));
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

    // ----- spec 0074: a reused page's furniture is derived from content, not from its number -----

    /// A document of two chapters, whose `body` master carries a `{heading:1}` running head.
    fn doc_with_running_head(chapters: &[&str], lines: usize) -> Document {
        let margins = Margins {
            top_pt: 54.0,
            bottom_pt: 54.0,
            inside_pt: 54.0,
            outside_pt: 40.0,
        };
        let mut doc = doc_of(0);
        doc.master_pages = vec![MasterPage {
            margins: Some(margins),
            statics: vec![quill_core_model::MasterStatic::text(
                quill_core_model::Rect {
                    x_pt: 54.0,
                    y_pt: 606.0,
                    w_pt: 338.0,
                    h_pt: 12.0,
                },
                "{heading:1}",
                INK,
            )],
            ..MasterPage::plain("body")
        }];
        doc.default_master = Some("body".into());
        doc.pages.clear();
        doc.page_setup.margins = margins;
        doc.content = Vec::new();
        for (c, name) in chapters.iter().enumerate() {
            doc.content.push(Block::heading(1, *name, INK));
            doc.content.extend((0..lines).map(|i| {
                Block::body(
                    format!("chapter {c} paragraph {i} with enough words to occupy a line or so"),
                    INK,
                )
            }));
        }
        doc.next_block_id = 0;
        doc.assign_missing_block_ids().expect("ids");
        doc
    }

    /// What a page's first static prints.
    fn head_of(page: &crate::LaidOutPage) -> String {
        match &page.statics[0] {
            crate::PlacedBlock::Text { lines, .. } => {
                lines.iter().map(|l| l.text.as_str()).collect::<String>()
            }
            other => panic!("expected a text static, got {other:?}"),
        }
    }

    /// **The defect spec 0074 had to fix before it could ship, and it was real.**
    ///
    /// `pass` reuses whole tail pages verbatim once the flow re-converges, re-asserting nothing but
    /// `page.index`. That is sound exactly while a static is a pure function of the page number — and
    /// `LaidOutPage::statics`' own doc comment used to say so. A `{heading:1}` running head is not:
    /// rename chapter one and every reused page below the edit keeps printing the **old** title, in
    /// the path the app actually uses, with nothing anywhere to say so. It is spec 0075's shape (a
    /// derived thing outliving its context) at a site 0075 did not reach.
    ///
    /// The rename is chosen not to move a single line break, which is what makes the flow re-converge
    /// at the very next page and the tail reuse actually happen — asserted, because a fixture that
    /// quietly stopped reusing pages would pass this test while proving nothing.
    #[test]
    fn a_reused_tail_page_carries_the_current_chapters_running_head() {
        let mut doc = doc_with_running_head(&["The Ruined Keep", "The Deep Road"], 60);
        let mut session = LayoutSession::new();
        let before = session.relayout(&doc, &MONO, &NoHyphenator);
        let total = before.pages.len();
        assert!(total > 3, "need a multi-page document, got {total}");
        assert_eq!(head_of(&before.pages[0]), "The Ruined Keep");

        // Rename chapter one in place: same id, same one-line height, so nothing repaginates.
        let id = doc.content[0].id();
        doc.content[0] = Block::heading(1, "The Sunken Vault", INK);
        doc.content[0].set_id(id);
        doc.bump_revision();

        let after = session.relayout(&doc, &MONO, &NoHyphenator);
        assert_eq!(after.pages.len(), total, "the rename must not repaginate");
        assert!(
            after.stats.pages_reused > 0,
            "the fixture must actually reuse pages, or this proves nothing"
        );

        let second = after.headings[1].page_index;
        for page in &after.pages {
            let want = if page.index < second {
                "The Sunken Vault"
            } else {
                "The Deep Road"
            };
            assert_eq!(head_of(page), want, "page {}", page.index);
        }

        // And the strongest form of the same claim: the incremental pages are the cold pass's,
        // furniture included.
        assert_eq!(
            after.pages,
            crate::lay_out(&doc, &MONO, &NoHyphenator),
            "a session's pages must equal a full pass's, statics included"
        );
    }

    /// The other half: a document whose furniture uses no spec-0074 token is untouched by any of
    /// this. Same reuse, same statics, and the running head is still the literal string it was.
    #[test]
    fn a_literal_running_head_is_unchanged_by_the_statics_post_pass() {
        let mut doc = doc_with_running_head(&["The Ruined Keep", "The Deep Road"], 60);
        let quill_core_model::MasterStatic::Text { text, .. } = &mut doc.master_pages[0].statics[0]
        else {
            panic!("the fixture's static is text");
        };
        *text = "The Ruined Keep — {page}".into();

        let mut session = LayoutSession::new();
        let before = session.relayout(&doc, &MONO, &NoHyphenator);
        let total = before.pages.len();
        edit(&mut doc, 5, "a short replacement paragraph");
        let after = session.relayout(&doc, &MONO, &NoHyphenator);

        assert!(after.stats.pages_reused > 0);
        for page in &after.pages {
            assert_eq!(
                head_of(page),
                format!("The Ruined Keep — {}", page.index + 1),
                "page {}",
                page.index
            );
        }
        assert_eq!(after.pages.len(), total);
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
        assert!(
            first.fixpoint.converged,
            "must settle: {:?}",
            first.fixpoint
        );
        assert!(
            first.fixpoint.iterations > 1,
            "a document with a contents block runs the fixpoint"
        );
        assert_eq!(first.headings.len(), 2);

        // **The entries have to reach the page**, not merely the index. Before spec 0075 this test
        // asserted only `first.headings`, and a session placed a contents list of nothing but its
        // own title — the cached measurement outlived the index it came from. An assertion about a
        // derived value that never looks at what was drawn is the shape of test that lets that
        // through.
        let toc_id = doc.content[0].id();
        let entries: Vec<String> = first
            .pages
            .iter()
            .flat_map(|p| p.blocks.iter())
            .filter_map(|b| match b {
                crate::PlacedBlock::Text { source, lines, .. } if *source == toc_id => {
                    Some(lines[0].text.clone())
                }
                _ => None,
            })
            .filter(|t| t.starts_with("Chapter"))
            .collect();
        assert_eq!(
            entries,
            ["Chapter 30", "Chapter 70"],
            "the session must place the entries, not just index them"
        );

        // Renaming a chapter must reach the contents list.
        let id = doc.content[31].id();
        doc.content[31] = Block::heading(1, "Renamed chapter", INK);
        doc.content[31].set_id(id);
        doc.bump_revision();
        let after = session.relayout(&doc, &MONO, &NoHyphenator);
        assert!(after.fixpoint.converged);
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
            result.fixpoint.iterations <= crate::FIXPOINT_MAX_ITERATIONS,
            "the loop must be bounded, took {}",
            result.fixpoint.iterations
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
            b @ Block::Body { .. } => b.plain_text().unwrap_or_default(),
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

    /// Spec 0065's cache claim, and spec 0031's: editing a character style reflows the blocks that
    /// use it.
    ///
    /// Asserted with the work counters rather than a timing, which is the discipline this module's
    /// own doc comment sets. The invalidation is deliberately coarse — the style fingerprint covers
    /// the whole `StyleSheet`, as it has since spec 0028's paragraph map — so what is asserted is
    /// that the blocks that use the style *are* re-measured, and that a document is not re-measured
    /// when nothing changed at all.
    #[test]
    fn editing_a_character_style_reflows_the_blocks_that_use_it() {
        use quill_core_model::{CharacterStyle, InlineStyle, Run, Weight};

        let mut doc = doc_of(40);
        for (i, block) in doc.content.iter_mut().enumerate() {
            if i % 4 != 0 {
                continue;
            }
            let id = block.id();
            *block = Block::body_runs(
                vec![
                    Run {
                        text: "lead in".into(),
                        style: InlineStyle::EMPTY,
                        character: Some("house-lead".into()),
                        source: Default::default(),
                    },
                    Run::plain(" and the rest of a paragraph long enough to occupy a line"),
                ],
                INK,
            );
            block.set_id(id);
        }
        doc.styles.character.insert(
            "house-lead".into(),
            CharacterStyle {
                weight: Some(Weight::BOLD),
                ..CharacterStyle::EMPTY
            },
        );
        doc.bump_revision();

        let mut session = LayoutSession::new();
        session.relayout(&doc, &MONO, &NoHyphenator);
        let quiet = session.relayout(&doc, &MONO, &NoHyphenator);
        assert_eq!(quiet.stats.blocks_measured, 0, "nothing changed");

        doc.styles.character.insert(
            "house-lead".into(),
            CharacterStyle {
                weight: Some(Weight::BOLD),
                size_pt: Some(14.0),
                ..CharacterStyle::EMPTY
            },
        );
        doc.bump_revision();
        let after = session.relayout(&doc, &MONO, &NoHyphenator);
        assert!(
            after.stats.blocks_measured > 0,
            "the blocks naming the style must be re-measured"
        );
        assert!(
            !after.changed_pages.is_empty(),
            "and what they draw must change"
        );
    }

    /// The fingerprint had stopped covering three fields that move a glyph — a hanging indent
    /// (spec 0048), a list marker (0066) and a paragraph weight (0064) — because it listed the
    /// fields it hashed by hand. Each of these edits must re-measure.
    #[test]
    fn a_paragraph_style_edit_that_moves_a_glyph_re_measures() {
        use quill_core_model::{Indent, Weight};

        type Edit = fn(&mut quill_core_model::ParagraphStyle);
        let edits: Vec<(&str, Edit)> = vec![
            ("size", |s| s.font_size_pt = 14.0),
            ("indent", |s| s.indent = Indent::hanging(18.0)),
            ("weight", |s| s.weight = Weight::BOLD),
            ("italic", |s| s.italic = true),
            ("align", |s| s.align = quill_core_model::TextAlign::Left),
        ];
        for (what, edit) in edits {
            let mut doc = doc_of(40);
            let mut session = LayoutSession::new();
            session.relayout(&doc, &MONO, &NoHyphenator);
            edit(
                doc.styles
                    .paragraph
                    .get_mut(quill_core_model::BODY_STYLE)
                    .expect("body style"),
            );
            doc.bump_revision();
            let after = session.relayout(&doc, &MONO, &NoHyphenator);
            assert!(
                after.stats.blocks_measured > 0,
                "editing {what} must re-measure"
            );
        }
    }

    // ----- spec 0072: sections on the incremental path ------------------------------------------

    /// A document whose chapter opens at a known block, with an opener master and a body master.
    ///
    /// The same shape as `lib.rs`'s `sectioned_doc`, built here from this module's helpers rather
    /// than shared, because the two are asserting different things: there, that the derivation is
    /// right; here, that the *session* sees it.
    fn sectioned_doc(filler: usize) -> Document {
        let mut doc = Document::sample();
        doc.content = (0..filler + 5)
            .map(|i| Block::body(format!("L{i}"), INK))
            .collect();
        doc.assets.clear();
        doc.assign_missing_block_ids().expect("ids");
        doc.master_pages = vec![
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
        ];
        doc.default_master = Some("body".into());
        doc.sections = vec![quill_core_model::Section {
            name: "Chapter One".into(),
            start: doc.content[filler].id(),
            master: Some("chapter-opener".into()),
            folio: None,
        }];
        doc
    }

    #[test]
    fn a_session_lays_a_sectioned_document_out_exactly_as_a_cold_pass_does() {
        // The lesson spec 0075 paid for: a derived quantity that is right in `lay_out` and wrong
        // through `LayoutSession` is wrong *in the path the app uses*, and no test of the former
        // notices. Parity is asserted page for page, not by page count.
        let doc = sectioned_doc(30);
        let cold = crate::lay_out(&doc, &MONO, &NoHyphenator);

        let mut session = LayoutSession::new();
        let result = session.relayout(&doc, &MONO, &NoHyphenator);
        assert!(result.fixpoint.converged, "{:?}", result.fixpoint);
        assert_eq!(result.pages, cold);

        // And a second relayout of an unchanged document reaches the same place. (It does not reuse
        // pages: the fixpoint's first pass starts from the underived assignment every time, exactly
        // as the contents fixpoint starts from an empty heading index. That is a cost sections
        // share with the contents list, not one they introduce.)
        let again = session.relayout(&doc, &MONO, &NoHyphenator);
        assert_eq!(again.pages, cold);
    }

    #[test]
    fn the_session_sees_the_section_list() {
        // Spec 0035's both-directions fingerprint test, for the field spec 0072 adds. Asserting only
        // that a change invalidates would pass against an implementation that invalidates on
        // everything; asserting only reuse would pass against one that never invalidates.
        let doc = sectioned_doc(30);
        let mut session = LayoutSession::new();
        let first = session.relayout(&doc, &MONO, &NoHyphenator);

        let mut renamed = doc.clone();
        renamed.sections[0].master = Some("body".into());
        let after = session.relayout(&renamed, &MONO, &NoHyphenator);
        assert_ne!(
            after.pages, first.pages,
            "re-pointing a section's master must move the pages, not return the previous ones"
        );

        // The other direction: a document that has not changed at all still reaches a stable answer
        // rather than a different one each call.
        let a = session.relayout(&renamed, &MONO, &NoHyphenator);
        let b = session.relayout(&renamed, &MONO, &NoHyphenator);
        assert_eq!(a.pages, b.pages);
    }

    #[test]
    fn a_session_reports_a_section_assignment_that_will_not_settle() {
        // The bound, on the incremental path too — and the same requirement: terminate, return a
        // complete document, and say it did not settle rather than presenting the last guess as an
        // answer. The fixture oscillates because the anchor sits between the two masters' page
        // capacities; see the cold-path test of the same name in `lib.rs`.
        let doc = sectioned_doc(40);
        let mut session = LayoutSession::new();
        let result = session.relayout(&doc, &MONO, &NoHyphenator);
        assert!(
            !result.fixpoint.converged,
            "this fixture is chosen to oscillate: {:?}",
            result.fixpoint
        );
        assert_eq!(result.fixpoint.iterations, crate::FIXPOINT_MAX_ITERATIONS);
        assert!(!result.pages.is_empty());
    }

    // ---------------------------------------------------------------------------------------
    // Cross-references (spec 0076)
    // ---------------------------------------------------------------------------------------

    /// A 400-paragraph document carrying cross-references to **two** targets: one early in the
    /// document and one late.
    ///
    /// Two rather than one, and this is what makes the counter below a statement about the design
    /// rather than about the fixture. An edit in the middle moves the late target's page and leaves
    /// the early one exactly where it was, so a cache invalidated by "the derivation changed"
    /// re-measures both groups while one keyed on *what this block's references say* re-measures
    /// only the second. Those two designs are indistinguishable on a document with one target.
    ///
    /// Returns `(doc, early target, late target, referrers to the early one, referrers to the late
    /// one)`.
    #[allow(clippy::type_complexity)]
    fn referring_doc() -> (Document, BlockId, BlockId, Vec<BlockId>, Vec<BlockId>) {
        use quill_core_model::Run;
        let mut doc = doc_of(400);
        let early = doc.content[50].id();
        let late = doc.content[380].id();
        let mut to_early = Vec::new();
        let mut to_late = Vec::new();
        for (i, target) in [(5usize, early), (6, early), (7, late), (8, late), (9, late)] {
            let id = doc.content[i].id();
            doc.content[i] = Block::body_runs(
                vec![
                    Run::plain("A paragraph that carries a cross-reference: see page "),
                    Run::reference(target),
                    Run::plain(" for the rest of it."),
                ],
                INK,
            );
            doc.content[i].set_id(id);
            if target == early {
                to_early.push(id);
            } else {
                to_late.push(id);
            }
        }
        (doc, early, late, to_early, to_late)
    }

    fn printed(pages: &[LaidOutPage], id: BlockId) -> String {
        let mut out = String::new();
        for page in pages {
            for placed in &page.blocks {
                if let crate::PlacedBlock::Text { source, lines, .. } = placed {
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

    fn page_of(pages: &[LaidOutPage], id: BlockId) -> usize {
        pages
            .iter()
            .find_map(|p| {
                p.blocks
                    .iter()
                    .any(|b| matches!(b, crate::PlacedBlock::Text { source, .. } if *source == id))
                    .then_some(p.index)
            })
            .expect("the block is placed")
    }

    #[test]
    fn an_edit_that_moves_a_page_re_measures_only_the_blocks_whose_reference_moved() {
        // **The increment's reason for existing, stated as a counter.**
        //
        // A cross-reference is derived from where its target landed, so the obvious home for it is
        // `context_fingerprint` — where the contents list's resolved index lives. A contents list
        // can afford that: there is one of it, and a changed context costs one whole-document
        // reflow. A book has hundreds of cross-references and *any* edit that moves *any* page
        // changes the derivation, so the same treatment reflows the whole document on every
        // keystroke and — because a fingerprint outside `MeasureKey` needs spec 0075's eviction to
        // stay correct — re-measures **every referring block in the book**, including all the ones
        // whose number did not change.
        //
        // Per-block, in the key, is spec 0066's `marker` at scale: the two references to a block
        // that did not move keep the key they had.
        let (doc, early, late, to_early, to_late) = referring_doc();
        let mut session = LayoutSession::new();
        let first = session.relayout(&doc, &MONO, &NoHyphenator);
        assert!(first.fixpoint.converged, "{:?}", first.fixpoint);
        let early_before = printed(&first.pages, to_early[0]);
        let late_before = printed(&first.pages, to_late[0]);
        let early_page = page_of(&first.pages, early);

        // The edit: one paragraph, between the early target and the late one, replaced by a longer
        // one — by more than a page, so the late target certainly moves and the early one certainly
        // does not. One block's text changes and nothing else does, which is exactly the edit
        // `incremental_blocks_measured` is about.
        let mut edited = doc.clone();
        let long = "an edited paragraph with a great many more words in it than the one it \
                    replaced had, so that it occupies more than a page on its own and every page \
                    below it moves down. "
            .repeat(30);
        edit(&mut edited, 200, &long);
        let after = session.relayout(&edited, &MONO, &NoHyphenator);
        assert!(after.fixpoint.converged, "{:?}", after.fixpoint);

        // The fixture really is asymmetric — without this the counter below would be measuring a
        // document in which nothing had to be re-resolved.
        assert_eq!(
            page_of(&after.pages, early),
            early_page,
            "the early target must sit above the edit and not move"
        );
        assert_eq!(
            printed(&after.pages, to_early[0]),
            early_before,
            "…so its references must print the same number they did"
        );
        let late_now = printed(&after.pages, to_late[0]);
        assert_ne!(late_now, late_before, "the late target must have moved");
        assert!(
            late_now.contains(&format!("see page {} for", page_of(&after.pages, late) + 1)),
            "printed {late_now:?}"
        );

        // The claim. One edited paragraph, plus the three blocks whose reference text changed —
        // and neither the two whose reference text did not, nor the 400-block document. Counted
        // across **every** pass of the fixpoint, because the cost of a relayout is the sum of its
        // passes and the last one measures nothing.
        assert_eq!(
            after.stats.blocks_measured,
            1 + to_late.len(),
            "only the edit and the blocks whose reference text changed may re-measure"
        );
        assert!(
            after.stats.blocks_from_cache > 300,
            "…and the rest of the document came from cache: {}",
            after.stats.blocks_from_cache
        );
        assert!(
            after.stats.pages_reused > 0,
            "…and the pages above the edit were reused rather than reflowed"
        );
    }

    #[test]
    fn a_relayout_of_an_unchanged_referring_document_measures_nothing() {
        // The half that proves the fixpoint is *seeded* from the previous relayout's pages rather
        // than restarted from the underived state, which is the shortcut spec 0072 left untaken for
        // sections and this increment cannot. Restarting prints `[?]` on the first pass of every
        // relayout and the real folio on the second, so **every relayout of every referring
        // document costs a second whole-document pass** — and a cold one costs a second measurement
        // of every referring block as well. Reintroducing it is caught by the `iterations`
        // assertion below rather than by the counter, because a warm cache holds both states.
        let (doc, _, _, _, _) = referring_doc();
        let mut session = LayoutSession::new();
        session.relayout(&doc, &MONO, &NoHyphenator);
        let again = session.relayout(&doc, &MONO, &NoHyphenator);
        assert_eq!(
            again.stats.blocks_measured, 0,
            "an unchanged document must not re-measure, references included"
        );
        assert_eq!(again.fixpoint.iterations, 1, "and must not iterate");
    }

    // ----- spec 0077: footnotes on the incremental path -----------------------------------------

    /// A document of `n` paragraphs, three of which anchor a footnote — one early, one in the
    /// middle, one late — so that an edit above the middle one moves it without moving the first.
    fn noted_doc(n: usize) -> (Document, Vec<BlockId>) {
        use quill_core_model::{Footnote, Run};
        let mut doc = doc_of(n);
        doc.footnotes = (0..3)
            .map(|i| {
                Footnote::plain(
                    format!("note {i}, with a sentence in it long enough to wrap once or twice"),
                    INK,
                )
            })
            .collect();
        doc.assign_missing_block_ids().expect("ids");
        let notes: Vec<BlockId> = doc.footnotes.iter().map(|f| f.id).collect();
        for (i, at) in [10usize, 150, 300].into_iter().enumerate() {
            let id = doc.content[at].id();
            let mut b = Block::body_runs(
                vec![
                    Run::plain("a paragraph that calls a note"),
                    Run::footnote(notes[i]),
                ],
                INK,
            );
            b.set_id(id);
            doc.content[at] = b;
        }
        (doc, notes)
    }

    /// **The parity assertion this increment owes**, and the reason `FlowState` grew: the session
    /// resumes from a checkpoint, and a checkpoint that did not carry the part-set note would give
    /// the resumed page back the height the note's tail was occupying.
    #[test]
    fn the_session_and_the_cold_path_agree_about_footnotes() {
        let (doc, _) = noted_doc(400);
        let mut session = LayoutSession::new();
        let result = session.relayout(&doc, &MONO, &NoHyphenator);
        assert_eq!(
            result.pages,
            crate::lay_out(&doc, &MONO, &NoHyphenator),
            "page for page, before any edit"
        );

        let long = "an edited paragraph with a great many more words in it than the one it \
                    replaced had, so that it occupies more than a page on its own and every page \
                    below it moves down. "
            .repeat(30);
        let mut edited = doc.clone();
        edit(&mut edited, 100, &long);
        let after = session.relayout(&edited, &MONO, &NoHyphenator);
        assert!(after.stats.pages_reused > 0, "the fixture must reuse pages");
        assert_eq!(
            after.pages,
            crate::lay_out(&edited, &MONO, &NoHyphenator),
            "…and page for page across an edit that moved two of the three notes"
        );
    }

    /// A note that splits across a page boundary is the case a checkpoint can actually sit inside,
    /// so it gets its own parity assertion rather than riding on the one above.
    #[test]
    fn the_session_agrees_with_the_cold_path_over_a_note_that_spans_pages() {
        use quill_core_model::{Footnote, Run};
        let mut doc = doc_of(120);
        doc.footnotes = vec![Footnote::plain("word ".repeat(2000), INK)];
        doc.assign_missing_block_ids().expect("ids");
        let note = doc.footnotes[0].id;
        let id = doc.content[60].id();
        let mut b = Block::body_runs(
            vec![Run::plain("calls a long note"), Run::footnote(note)],
            INK,
        );
        b.set_id(id);
        doc.content[60] = b;

        let cold = crate::lay_out(&doc, &MONO, &NoHyphenator);
        let spans = cold
            .iter()
            .filter(|p| {
                p.blocks.iter().any(
                    |b| matches!(b, crate::PlacedBlock::Text { source, .. } if *source == note),
                )
            })
            .count();
        assert!(spans >= 2, "the fixture must span pages: {spans}");

        let mut session = LayoutSession::new();
        assert_eq!(session.relayout(&doc, &MONO, &NoHyphenator).pages, cold);

        let mut edited = doc.clone();
        edit(
            &mut edited,
            10,
            &"a much longer paragraph here. ".repeat(40),
        );
        let after = session.relayout(&edited, &MONO, &NoHyphenator);
        assert_eq!(
            after.pages,
            crate::lay_out(&edited, &MONO, &NoHyphenator),
            "resuming across a page boundary inside a note must reproduce a full pass"
        );
    }

    /// Editing a note's *text* is an edit. The note is not in `content`, so nothing in the block
    /// diff would see it without the third fingerprint term — and the session would hand back the
    /// previous pages with the previous note on them.
    #[test]
    fn editing_a_notes_text_re_lays_the_document() {
        let (doc, _) = noted_doc(400);
        let mut session = LayoutSession::new();
        session.relayout(&doc, &MONO, &NoHyphenator);

        let mut edited = doc.clone();
        edited.footnotes[1].runs = vec![quill_core_model::Run::plain(
            "a completely different note, and a much longer \
                 one, long enough that the band it reserves is taller than the one it replaces",
        )];
        edited.bump_revision();
        let after = session.relayout(&edited, &MONO, &NoHyphenator);
        assert_eq!(
            after.pages,
            crate::lay_out(&edited, &MONO, &NoHyphenator),
            "a note edit must reach the pages"
        );
        assert!(
            !after.changed_pages.is_empty(),
            "…and must be reported as a change"
        );
        // Proportional to the edit, not to the book: the blocks above the anchor keep their cached
        // measurements. This is why the term is in the per-block diff and not in
        // `context_fingerprint`.
        assert!(
            after.stats.pages_reused > 0,
            "a note edit must not reflow the whole document, reused {}",
            after.stats.pages_reused
        );
    }

    /// A document with no footnote must be untouched by any of it, incremental behaviour included.
    #[test]
    fn a_document_with_no_footnote_measures_exactly_what_it_did() {
        let doc = doc_of(400);
        assert!(doc.footnote_blocks().is_empty());
        let mut session = LayoutSession::new();
        session.relayout(&doc, &MONO, &NoHyphenator);
        let mut edited = doc.clone();
        edit(
            &mut edited,
            200,
            "a paragraph with one word changed in it, and nothing else at all",
        );
        let after = session.relayout(&edited, &MONO, &NoHyphenator);
        assert_eq!(
            after.stats.blocks_measured, 1,
            "editing one paragraph must re-break exactly one block"
        );
    }

    #[test]
    fn the_session_and_the_cold_path_agree_about_cross_references() {
        // Across an edit as well as before one, which is where spec 0074's lesson lands: a session
        // reuses whole tail pages verbatim, so anything it reuses must not depend on derived state
        // it did not re-check. A cross-reference *is* derived state — and it is safe here because
        // the resolved value is folded into the per-pass diff, so a block whose reference moved is
        // inside the dirty range by construction and cannot sit in a reused tail. This asserts the
        // consequence rather than the argument.
        let (doc, _, _, _, _) = referring_doc();
        let mut session = LayoutSession::new();
        let result = session.relayout(&doc, &MONO, &NoHyphenator);
        assert_eq!(
            result.pages,
            crate::lay_out(&doc, &MONO, &NoHyphenator),
            "two fixpoints over the same quantity must not settle differently"
        );

        let mut edited = doc.clone();
        let long = "an edited paragraph with a great many more words in it than the one it \
                    replaced had, so that it occupies more than a page on its own and every page \
                    below it moves down. "
            .repeat(30);
        edit(&mut edited, 200, &long);
        let after = session.relayout(&edited, &MONO, &NoHyphenator);
        assert!(after.stats.pages_reused > 0, "the fixture must reuse pages");
        assert_eq!(
            after.pages,
            crate::lay_out(&edited, &MONO, &NoHyphenator),
            "…and every page it kept must still say what a cold pass would say"
        );
    }

    #[test]
    fn the_session_reports_the_same_non_convergence_the_cold_path_does() {
        // The one fixture, shared rather than rebuilt: two documents that oscillate for two
        // slightly different reasons would make a disagreement between the paths look like
        // agreement.
        let doc = crate::tests::oscillating_doc();
        let mut session = LayoutSession::new();
        let result = session.relayout(&doc, &MONO, &NoHyphenator);
        assert!(
            !result.fixpoint.converged,
            "the session must report what the cold path reports: {:?}",
            result.fixpoint
        );
        assert_eq!(result.fixpoint.iterations, crate::FIXPOINT_MAX_ITERATIONS);
    }

    #[test]
    fn a_reference_whose_value_moved_is_not_mistaken_for_an_unchanged_block() {
        // Spec 0075's shape, in the one place this increment could reintroduce it. The per-pass diff
        // is over `(id, content ^ references)`; with the second term missing, a pass whose
        // references moved but whose *text* did not would see "nothing changed" and hand back the
        // previous pages with the previous numbers on them, for ever.
        let (doc, _, _, _, referrers) = referring_doc();
        let mut session = LayoutSession::new();
        let first = session.relayout(&doc, &MONO, &NoHyphenator);
        let cold = crate::lay_out(&doc, &MONO, &NoHyphenator);
        assert_eq!(
            printed(&first.pages, referrers[0]),
            printed(&cold, referrers[0]),
            "the session's first relayout must already print the resolved number"
        );
        assert!(
            !printed(&first.pages, referrers[0]).contains(quill_core_model::UNRESOLVED_REFERENCE),
            "and not the unresolved marker it started from"
        );
    }
}
