//! Deterministic synthetic documents for the performance harness — see `specs/0027-perf-harness.md`.
//!
//! `CLAUDE.md` states the target the whole architecture is built around: **500 pages, art-heavy,
//! must stay smooth**, benchmark-gated in CI. That claim was unmeasurable — `cargo bench` was
//! documented as the M1 gate but was a silent no-op, with no `benches/` directory and no bench
//! target anywhere in the workspace. This crate is the measuring stick, landed *before* the layout
//! work it exists to gate.
//!
//! ## Why the page count is measured, not assumed
//!
//! A synthetic document is specified by the number of **pages** it should produce, not the number of
//! blocks it contains. Those are not the same thing: leading, margins, hyphenation and frame
//! geometry all change how many pages a fixed block count fills, so a hardcoded block count would
//! quietly stop being a 500-page workload the first time any of them changed — and the benchmark
//! would keep reporting numbers as if nothing had happened. [`synthetic_document`] therefore lays
//! its candidate out and converges on the requested page count.
//!
//! ## Determinism
//!
//! Same seed ⇒ byte-identical document. The generator uses a small xorshift PRNG rather than `rand`
//! (a dependency for eight lines of arithmetic, and one whose output is not guaranteed stable across
//! versions — which would silently change the workload under the benchmark).

use quill_core_model::{Asset, Block, Color, Document, PageSetup};
use quill_layout_engine::lay_out;
use quill_text_layout::{MonospaceRunMetrics, NoHyphenator};

/// The metrics the harness measures against.
///
/// Deliberately *not* the real shaped font: `rustybuzz` shaping would dominate the timing and make
/// the layout benchmark a font-shaping benchmark. A fixed em ratio keeps the measurement on the
/// engine under test, and keeps results comparable across machines that have different fonts.
pub const HARNESS_METRICS: MonospaceRunMetrics = MonospaceRunMetrics { em_ratio: 0.6 };

/// What a synthetic document should look like.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct SynthSpec {
    /// How many laid-out pages the document should produce. This is a *target*, converged on by
    /// measurement — see the module docs.
    pub target_pages: usize,
    /// PRNG seed. Same seed ⇒ identical document.
    pub seed: u64,
    /// Emit a heading every N blocks (0 = never).
    pub heading_every_n: usize,
    /// Emit an image every N blocks (0 = never). Art-heavy is the stated workload, so the default
    /// is deliberately image-dense.
    pub image_every_n: usize,
}

impl Default for SynthSpec {
    fn default() -> Self {
        Self {
            target_pages: 500,
            seed: 0x5EED,
            heading_every_n: 12,
            image_every_n: 25,
        }
    }
}

/// How close to `target_pages` counts as converged. A tolerance is needed at all because pages are
/// quantized — one added paragraph can push a page boundary — so demanding exactness could loop
/// forever.
pub const PAGE_TOLERANCE: usize = 5;

/// A tiny xorshift64* PRNG. Stable by construction, unlike a third-party generator whose stream may
/// change between versions and silently alter the benchmark workload.
struct Rng(u64);

impl Rng {
    fn new(seed: u64) -> Self {
        // A zero state is a fixed point for xorshift; substitute a constant.
        Rng(if seed == 0 {
            0x9E37_79B9_7F4A_7C15
        } else {
            seed
        })
    }

    fn next(&mut self) -> u64 {
        let mut x = self.0;
        x ^= x << 13;
        x ^= x >> 7;
        x ^= x << 17;
        self.0 = x;
        x.wrapping_mul(0x2545_F491_4F6C_DD1D)
    }

    fn in_range(&mut self, lo: usize, hi: usize) -> usize {
        debug_assert!(hi > lo);
        lo + (self.next() % (hi - lo) as u64) as usize
    }
}

/// Vocabulary for generated prose. A fixed pool rather than lorem ipsum so word lengths — which are
/// what line breaking actually costs — resemble the genre's real text.
const WORDS: &[&str] = &[
    "dungeon",
    "goblin",
    "lantern",
    "corridor",
    "torchlight",
    "rusted",
    "portcullis",
    "whispering",
    "ancient",
    "rune",
    "chamber",
    "shadow",
    "flagstone",
    "cobweb",
    "iron",
    "hoard",
    "wyrm",
    "parchment",
    "sigil",
    "reliquary",
    "moss",
    "vault",
    "echo",
    "brazier",
    "adventurer",
    "initiative",
    "saving",
    "throw",
    "damage",
    "necrotic",
    "spellbook",
    "incantation",
    "warding",
    "grimoire",
    "cartographer",
    "subterranean",
];

/// Build a paragraph of `n` words from the pool.
fn paragraph(rng: &mut Rng, n: usize) -> String {
    let mut s = String::with_capacity(n * 8);
    for i in 0..n {
        if i > 0 {
            s.push(' ');
        }
        s.push_str(WORDS[rng.next() as usize % WORDS.len()]);
    }
    s
}

/// Build a document with exactly `block_count` blocks, deterministically from `spec.seed`.
///
/// Public because the benches need a document of a *known block count* (to compare like with like
/// across sizes) as well as one of a known page count.
pub fn document_with_blocks(spec: &SynthSpec, block_count: usize) -> Document {
    let mut rng = Rng::new(spec.seed);
    let ink = Color::Cmyk {
        c: 0.0,
        m: 0.0,
        y: 0.0,
        k: 1.0,
    };

    let mut content = Vec::with_capacity(block_count);
    let mut assets: Vec<Asset> = Vec::new();

    for i in 0..block_count {
        let is_heading = spec.heading_every_n > 0 && i % spec.heading_every_n == 0;
        let is_image = spec.image_every_n > 0 && i % spec.image_every_n == 0 && !is_heading;

        if is_heading {
            let words = rng.in_range(2, 6);
            content.push(Block::heading(2, paragraph(&mut rng, words), ink));
        } else if is_image {
            let id = format!("img{}", assets.len());
            assets.push(Asset {
                path: format!("assets/{id}.png"),
                id: id.clone(),
                // 1500x1200 at 300 dpi — a plate that scales to the content width, matching the
                // art-heavy workload the perf target is stated against.
                px_w: 1500,
                px_h: 1200,
                dpi: 300.0,
                line_art: false,
                has_alpha: false,
            });
            content.push(Block::image(id));
        } else {
            let words = rng.in_range(30, 90);
            content.push(Block::body(paragraph(&mut rng, words), ink));
        }
    }

    let mut doc = Document {
        format_version: quill_core_model::FORMAT_VERSION,
        metadata: quill_core_model::Metadata {
            title: format!("Synthetic {block_count}-block document"),
            authors: vec!["quill-testdoc".into()],
        },
        page_setup: PageSetup::default(),
        content,
        assets,
        fonts_embeddable: true,
        revision: 0,
        next_block_id: 0,
        styles: quill_core_model::StyleSheet::default(),
        master_pages: Vec::new(),
        default_master: None,
        // No per-page overrides: the synthetic workload deliberately carries no furniture, so every
        // budget in `benches/budgets.toml` keeps measuring the same thing across spec 0035.
        pages: Vec::new(),
        // And no sections (spec 0072), for the same reason: a section puts the document in the
        // layout fixpoint, and a workload that took two passes where it used to take one would
        // re-baseline every budget in the file against a change to the measurement rather than to
        // the engine.
        sections: Vec::new(),
        footnotes: Vec::new(),
        breaks: Vec::new(),
        components: Default::default(),
        requires: Vec::new(),
    };
    doc.assign_missing_block_ids()
        .expect("generated blocks are unassigned, so cannot collide");
    doc
}

/// Lay `doc` out with the harness metrics and report its page count.
pub fn page_count(doc: &Document) -> usize {
    lay_out(doc, &HARNESS_METRICS, &NoHyphenator).len()
}

/// A deterministic document that lays out to approximately `spec.target_pages` pages.
///
/// Converges by measurement rather than by a hardcoded block count — see the module docs for why.
/// The search is a damped fixed-point iteration on `blocks ≈ blocks * target / measured`, which
/// converges in a handful of steps because pages are very nearly linear in block count.
pub fn synthetic_document(spec: &SynthSpec) -> Document {
    assert!(spec.target_pages > 0, "target_pages must be positive");

    // Probe with a small document to learn the pages-per-block rate, rather than guessing a
    // constant that would go stale the moment leading or margins change.
    const PROBE_BLOCKS: usize = 200;
    let probe = document_with_blocks(spec, PROBE_BLOCKS);
    let probe_pages = page_count(&probe).max(1);

    let mut blocks =
        (PROBE_BLOCKS as f64 * spec.target_pages as f64 / probe_pages as f64).ceil() as usize;
    blocks = blocks.max(1);

    // Bounded: converge or give up loudly. An unbounded loop here could hang CI if the relationship
    // between blocks and pages ever stopped being monotonic.
    for _ in 0..24 {
        let doc = document_with_blocks(spec, blocks);
        let pages = page_count(&doc);
        if pages.abs_diff(spec.target_pages) <= PAGE_TOLERANCE {
            return doc;
        }
        let scaled = blocks as f64 * spec.target_pages as f64 / pages.max(1) as f64;
        let next = scaled.round() as usize;
        // Always move at least one block, or the iteration can stall on a rounding fixed point
        // that is outside the tolerance.
        blocks = if next == blocks {
            if pages < spec.target_pages {
                blocks + 1
            } else {
                blocks.saturating_sub(1).max(1)
            }
        } else {
            next.max(1)
        };
    }

    panic!(
        "synthetic_document failed to converge on {} pages (last block count {blocks}); \
         the block-count/page-count relationship may no longer be monotonic",
        spec.target_pages
    );
}

/// A synthetic **book** (spec 0079): `chapters` chapters totalling `spec.target_pages` pages, with a
/// contents list in the first one, composed into a single document.
///
/// The contents list is what makes this the workload the M6 audit was worried about: a book's
/// contents list names headings from chapters it does not itself contain, so the layout fixpoint's
/// derived quantity spans every chapter. Splitting a fixed total across a varying number of chapters
/// is what turns "does the cost multiply by the chapter count?" into a measurement — the total
/// content is held constant and only the number of chapters moves.
pub fn synthetic_book(spec: &SynthSpec, chapters: usize) -> quill_core_model::ComposedBook {
    assert!(chapters > 0, "a book must have at least one chapter");
    let per_chapter = (spec.target_pages / chapters).max(1);
    let docs: Vec<Document> = (0..chapters)
        .map(|i| {
            let mut doc = synthetic_document(&SynthSpec {
                target_pages: per_chapter,
                seed: spec.seed.wrapping_add(i as u64 * 1013),
                ..*spec
            });
            if i == 0 {
                doc.content.insert(
                    0,
                    Block::Toc {
                        id: quill_core_model::BlockId::UNASSIGNED,
                        title: "Contents".into(),
                        max_level: 2,
                        color: Color::Gray { v: 0.0 },
                    },
                );
                doc.assign_missing_block_ids().expect("ids");
            }
            doc
        })
        .collect();

    let book = quill_core_model::Book {
        book_version: quill_core_model::BOOK_VERSION,
        metadata: Default::default(),
        requires: Vec::new(),
        chapters: (0..chapters)
            .map(|i| quill_core_model::BookChapter {
                path: format!("ch{i}.tpub"),
                name: format!("Chapter {i}"),
                folio: None,
                master: None,
                // What a book file that states nothing gets (spec 0080), which is what this
                // workload is for: the benched book is the ordinary one, breaks included.
                break_before: quill_core_model::BreakKind::default(),
            })
            .collect(),
    };
    book.compose(&docs).expect("a synthetic book must compose")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_synthetic_book_is_its_chapters() {
        let spec = SynthSpec {
            target_pages: 20,
            ..SynthSpec::default()
        };
        let book = synthetic_book(&spec, 4);
        assert_eq!(book.chapter_anchors.len(), 4);
        assert_eq!(book.document.sections.len(), 4);
        assert!(book
            .document
            .content
            .iter()
            .any(|b| matches!(b, Block::Toc { .. })));
    }

    #[test]
    fn hits_the_page_target() {
        let spec = SynthSpec::default();
        let doc = synthetic_document(&spec);
        let pages = page_count(&doc);
        assert!(
            pages.abs_diff(spec.target_pages) <= PAGE_TOLERANCE,
            "wanted ~{} pages, got {pages}",
            spec.target_pages
        );
    }

    #[test]
    fn hits_a_smaller_page_target_too() {
        // The bench's scaling check compares 250 against 500, so the generator has to be right at
        // both sizes, not tuned for one.
        let spec = SynthSpec {
            target_pages: 250,
            ..SynthSpec::default()
        };
        let doc = synthetic_document(&spec);
        assert!(page_count(&doc).abs_diff(250) <= PAGE_TOLERANCE);
    }

    #[test]
    fn the_same_seed_produces_an_identical_document() {
        let spec = SynthSpec {
            target_pages: 20,
            ..SynthSpec::default()
        };
        let a = synthetic_document(&spec).to_json().expect("serialize");
        let b = synthetic_document(&spec).to_json().expect("serialize");
        assert_eq!(a, b, "generation must be deterministic");
    }

    #[test]
    fn different_seeds_produce_different_documents() {
        // Guards against a seed that is accepted and then ignored — which would make every
        // "different workload" bench secretly the same workload.
        let a = synthetic_document(&SynthSpec {
            target_pages: 20,
            seed: 1,
            ..SynthSpec::default()
        });
        let b = synthetic_document(&SynthSpec {
            target_pages: 20,
            seed: 2,
            ..SynthSpec::default()
        });
        assert_ne!(a.to_json().unwrap(), b.to_json().unwrap());
    }

    #[test]
    fn the_document_is_art_heavy_as_the_perf_target_assumes() {
        let doc = synthetic_document(&SynthSpec::default());
        let images = doc
            .content
            .iter()
            .filter(|b| matches!(b, Block::Image { .. }))
            .count();
        assert!(
            images > 100,
            "a 500-page art-heavy book should carry many images, got {images}"
        );
        assert_eq!(images, doc.assets.len(), "every image needs its asset");
    }

    #[test]
    fn every_block_has_an_id() {
        // The generator has to produce documents that look like loaded ones (spec 0026), or the
        // benches would measure a code path real documents never take.
        let doc = synthetic_document(&SynthSpec {
            target_pages: 10,
            ..SynthSpec::default()
        });
        assert!(doc.content.iter().all(|b| b.id().is_assigned()));
    }

    #[test]
    fn a_generated_document_round_trips_through_json() {
        let doc = synthetic_document(&SynthSpec {
            target_pages: 10,
            ..SynthSpec::default()
        });
        let back = Document::from_json(&doc.to_json().expect("serialize")).expect("load");
        assert_eq!(doc, back);
    }
}
