//! Corpus equivalence for Knuth-Plass line breaking — see `specs/0051-line-break-pruning.md`.
//!
//! Spec 0051 makes `break_paragraph_hyphenated` prune active nodes and stop carrying a cloned
//! `Vec` of line starts per DP candidate. Both are meant to be *invisible*: a pruned node is one
//! that provably could not have won, so every paragraph must break exactly where it broke before.
//!
//! That is the increment's central risk. Pruning that is a hair too aggressive moves a line break
//! in a rare paragraph, which moves a page break, which moves the exported bytes — a silent
//! typographic regression that the perf bench would report as a *success*. So the whole 500-page
//! synthetic corpus is broken at four measures, with and without hyphenation, and digested; the
//! digest below was recorded from the pre-0051 breaker and is asserted, not recomputed.
//!
//! If this test fails after a line-breaker change, the breaker is wrong — do not re-pin the digest
//! unless the change is a deliberate, documented typographic decision (in which case
//! `Document::sample()`'s export byte-hash moves too, and both are updated in the same commit).
//! Spec 0060 is so far the only such decision; see `CORPUS_DIGEST` for both values.

use quill_testdoc::{document_with_blocks, SynthSpec, HARNESS_METRICS};
use quill_text_layout::{break_paragraph_hyphenated, Hyphenator, NoHyphenator, BODY_FONT_SIZE_PT};

/// The measures the corpus is broken against, in points.
///
/// - `432.0` — the default 6x9in trim's full body width (what `lay_out` actually uses).
/// - `198.0` — a column of spec 0036's two-column `rulebook` template, where lines are short and
///   the active set turns over fastest.
/// - `96.0` — narrower than several corpus words are long, so hyphenation carries real weight.
/// - `54.0` — narrower than *most* corpus words, which drives the no-feasible-breaking fallback to
///   `break_by_width`. Pruning must not silently convert a feasible paragraph into a fallback one.
const MEASURES_PT: &[f32] = &[432.0, 198.0, 96.0, 54.0];

/// Digest of every line of the corpus at every measure, with and without hyphenation.
///
/// Recorded from the **pre-0051 breaker**, by checking out `text-layout`'s pre-pruning source,
/// running this test, and taking the value it reported — then restoring pruning and confirming it
/// reports the same. That is the whole proof, and it is why the digest may only ever be re-derived
/// that way and never simply pasted from a failing run.
///
/// Re-derived once for a corpus that legitimately moved. Specs 0044-0046 pack columns tighter, and
/// the corpus was then sized by *measuring* to ~500 pages, so more blocks were needed to reach that
/// target: 3,078 paragraphs / 690,915 lines became 3,251 / 730,328. The guard assertions below are
/// what caught it — they refuse to compare a digest against a corpus that is not the one it was
/// recorded from, which is exactly the failure a bare digest would have hidden.
///
/// Re-derived a second time by **spec 0060**, which is the first *deliberate* move. Forbidding a
/// last line wider than its measure changes where 254 of 730,328 lines break. Both values are on
/// the record:
///
/// | | paragraphs | lines | digest |
/// |---|---|---|---|
/// | before 0060 | 3,251 | 730,328 | `0x9ff1d0465473aa9c` |
/// | after 0060  | 3,251 | 730,582 | `0xacb8c7ec672a3410` |
///
/// Derived by the documented procedure: the pre-change tree, with *this file exactly as it is now*,
/// reproduced `0x9ff1d0465473aa9c` — which is what proves the corpus pinning below did not itself
/// change the workload — and the post-change tree then reported the new value.
const CORPUS_DIGEST: u64 = 0xacb8_c7ec_672a_3410;
/// Total number of lines the corpus produces, digested alongside as a human-legible cross-check —
/// a digest mismatch says "something moved", this says how much text there was.
const CORPUS_LINES: usize = 730_582;
/// Paragraph count actually exercised, so a corpus that quietly shrank cannot pass by breaking
/// nothing.
const CORPUS_PARAGRAPHS: usize = 3_251;

/// Blocks in the corpus document. Chosen as what `synthetic_document`'s 500-page target converged
/// to when the corpus was pinned; held fixed from then on, so the workload is a constant and only
/// the breaker can move the digest.
const CORPUS_BLOCKS: usize = 3_374;

/// A deterministic, font-free hyphenator: every word longer than four characters may break after
/// each third character.
///
/// Deliberately *not* `hypher` — the real Knuth-Liang tables live behind `quill-export-pdf` and
/// would add a dependency edge to this crate. What the equivalence test needs from a hyphenator is
/// a dense, deterministic supply of flagged penalties (a `hypher` word yields 0-3, this yields many
/// more), which stresses the penalty half of the DP far harder than en-US would.
struct EveryThirdChar;

impl Hyphenator for EveryThirdChar {
    fn hyphenate(&self, word: &str) -> Vec<usize> {
        if word.chars().count() <= 4 {
            return Vec::new();
        }
        word.char_indices()
            .map(|(i, _)| i)
            .filter(|&i| i > 0 && i % 3 == 0)
            .collect()
    }
}

/// FNV-1a 64: a few lines of arithmetic rather than a hashing dependency, and stable by
/// construction (`DefaultHasher` explicitly is not, so it cannot be pinned across toolchains).
struct Fnv(u64);

impl Fnv {
    fn new() -> Self {
        Fnv(0xcbf2_9ce4_8422_2325)
    }
    fn write(&mut self, bytes: &[u8]) {
        for &b in bytes {
            self.0 ^= u64::from(b);
            self.0 = self.0.wrapping_mul(0x0000_0100_0000_01b3);
        }
    }
}

/// The corpus: every text-bearing paragraph of a **fixed-size** synthetic document, in order.
///
/// Fixed by block count rather than by `synthetic_document`'s measure-to-500-pages convergence, and
/// that is a correction spec 0060 had to make before it could prove anything. The old corpus was
/// sized by *laying the document out*, so a deliberate line-breaking change moved the corpus and the
/// breaking at once and there was no way to tell the two apart — the digest would have had to be
/// re-recorded against a workload that was no longer the same workload.
///
/// `document_with_blocks` generates block `i` from a seeded PRNG in index order, so a fixed count
/// pins the exact text forever, whatever layout does.
fn corpus() -> Vec<String> {
    let doc = document_with_blocks(&SynthSpec::default(), CORPUS_BLOCKS);
    doc.content
        .iter()
        .filter_map(|b| match b {
            quill_core_model::Block::Body { text, .. }
            | quill_core_model::Block::Heading { text, .. } => Some(text.clone()),
            _ => None,
        })
        .collect()
}

/// Break the whole corpus every way the test cares about and digest the result.
fn measure_corpus(paragraphs: &[String]) -> (u64, usize) {
    let mut fnv = Fnv::new();
    let mut lines_total = 0usize;
    for text in paragraphs {
        for &width in MEASURES_PT {
            let plain = break_paragraph_hyphenated(
                text,
                width,
                BODY_FONT_SIZE_PT,
                &HARNESS_METRICS,
                &NoHyphenator,
            );
            let hyphenated = break_paragraph_hyphenated(
                text,
                width,
                BODY_FONT_SIZE_PT,
                &HARNESS_METRICS,
                &EveryThirdChar,
            );
            for lines in [&plain, &hyphenated] {
                lines_total += lines.len();
                for line in lines.iter() {
                    fnv.write(line.as_bytes());
                    fnv.write(b"\n");
                }
                fnv.write(b"\x1e"); // paragraph terminator, so an empty run cannot alias
            }
        }
    }
    (fnv.0, lines_total)
}

#[test]
fn the_corpus_breaks_exactly_where_it_broke_before_pruning() {
    let paragraphs = corpus();
    assert_eq!(
        paragraphs.len(),
        CORPUS_PARAGRAPHS,
        "the corpus itself moved — the generator changed, so this digest no longer means what it \
         meant when it was recorded"
    );

    let (digest, lines) = measure_corpus(&paragraphs);
    assert_eq!(
        lines, CORPUS_LINES,
        "the corpus broke into a different number of lines ({lines} vs {CORPUS_LINES}) — line \
         breaking changed"
    );
    assert_eq!(
        digest, CORPUS_DIGEST,
        "corpus line breaking changed: digest {digest:#018x}, expected {CORPUS_DIGEST:#018x}. \
         Active-node pruning that drops a node which could still have won produces exactly this \
         failure. Fix the breaker, not the digest."
    );
}

#[test]
fn an_unbreakable_word_still_falls_back_to_greedy() {
    // Spec 0018's fallback: no feasible breaking exists, so the breaker must return the greedy
    // `break_by_width` result rather than nothing. Pruning removes DP transitions, so this is the
    // path most at risk of being turned from "greedy fallback" into "no lines at all".
    let text = "aaaa supercalifragilisticexpialidocious bbbb";
    let lines = break_paragraph_hyphenated(
        text,
        60.0,
        BODY_FONT_SIZE_PT,
        &HARNESS_METRICS,
        &NoHyphenator,
    );
    assert_eq!(
        lines,
        vec![
            "aaaa".to_string(),
            "supercalifragilisticexpialidocious".to_string(),
            "bbbb".to_string()
        ],
        "the over-wide word must still be placed alone by the greedy fallback"
    );
}

/// Spec 0060's actual claim, asserted directly rather than through a digest: **no line, including a
/// last line, is drawn wider than its own measure.**
///
/// The digest above says "line breaking changed in exactly the way it was recorded as changing";
/// this says *what* the change buys, and it would still be meaningful if the corpus were replaced
/// wholesale. A last line has `space_adjust_pt: 0.0` — it is drawn at its natural width — so its
/// natural width is the number that must fit.
///
/// The greedy fallback is exempt and has to be: `break_by_width` fires precisely when some single
/// word is wider than the measure, at which point no breaking can keep every line inside it, and
/// laying out visibly beats laying out not at all (spec 0018). Those paragraphs are counted and
/// reported rather than silently skipped.
#[test]
fn no_line_is_drawn_past_its_measure() {
    use quill_text_layout::{justify_paragraph_hyphenated, Alignment, RunMetrics};

    let mut checked = 0usize;
    let mut fallbacks = 0usize;
    let mut over: Vec<String> = Vec::new();

    for text in corpus() {
        for &width in MEASURES_PT {
            for ragged in [Alignment::Justified, Alignment::Left] {
                // A paragraph containing a word wider than the measure is a fallback paragraph.
                let unbreakable = text
                    .split_whitespace()
                    .any(|w| HARNESS_METRICS.measure_run(w, BODY_FONT_SIZE_PT) > width);
                if unbreakable {
                    fallbacks += 1;
                    continue;
                }
                let lines = justify_paragraph_hyphenated(
                    &text,
                    width,
                    BODY_FONT_SIZE_PT,
                    ragged,
                    &HARNESS_METRICS,
                    &NoHyphenator,
                );
                for line in &lines {
                    let natural = HARNESS_METRICS.measure_run(&line.text, BODY_FONT_SIZE_PT);
                    let gaps = line.text.split(' ').count().saturating_sub(1) as f32;
                    let drawn = natural + gaps * line.space_adjust_pt + line.indent_pt;
                    checked += 1;
                    // A hair of tolerance: a justified line is *meant* to reach the measure exactly,
                    // and `f32` accumulation over ~15 gaps lands a fraction of a point either side.
                    if drawn > width + 0.01 {
                        over.push(format!(
                            "  measure {width}: drawn {drawn:.2} — {:?}",
                            line.text
                        ));
                    }
                }
            }
        }
    }

    assert!(checked > 100_000, "the corpus checked only {checked} lines");
    assert!(
        over.is_empty(),
        "{} of {checked} lines are drawn past their measure ({fallbacks} fallback paragraphs \
         skipped):\n{}",
        over.len(),
        over.iter().take(10).cloned().collect::<Vec<_>>().join("\n")
    );
}
