//! Text shaping and line breaking.
//!
//! Spec 0015 replaced the character-count stand-in with real **width-based** greedy line breaking:
//! words are packed onto a line while the line's *measured* advance stays within the frame width.
//!
//! Spec 0016 (increment 1) moves the measurement unit from the individual character to the whole
//! **run**: [`break_by_width`] now measures each prospective line through a [`RunMetrics`]
//! implementation, so once a real shaper is wired the width test captures kerning and ligatures
//! *across* the line rather than summing isolated per-char advances. The per-char [`CharMetrics`]
//! trait stays as the fallback seam and as what a monospace test double implements. This first part
//! of increment 1 introduces the seam at parity — [`MonospaceRunMetrics`] and the export font's
//! run measurement both reproduce the per-char sum exactly, so no line break moves. The
//! `rustybuzz`-backed [`RunMetrics`] that actually shapes lands in the follow-up.
//!
//! Spec 0017 (increment 1) adds [`break_paragraph`]: **Knuth-Plass total-fit** line breaking over a
//! box/glue model on the same [`RunMetrics`] seam. It chooses all of a paragraph's breakpoints
//! together to minimize summed per-line demerits — a global balance greedy [`break_by_width`] cannot
//! reach — while still returning ragged, left-aligned lines (the writer is unchanged; only *which
//! words fall on which line* moves). `break_by_width` is retained as the fallback for the
//! no-feasible-breaking case and as the parity oracle. Justified rendering and hyphenation arrive in
//! later spec-driven increments.

/// Body/heading font size, in points. Shared by the layout engine (to measure and to reserve row
/// height) and the writer (to set the font size), so text is measured at the size it is drawn.
pub const BODY_FONT_SIZE_PT: f32 = 10.0;
/// Body/heading line advance (leading), in points. Deriving leading from font metrics is deferred
/// (spec 0015 non-goal); increment 1 keeps a fixed value.
pub const BODY_LINE_HEIGHT_PT: f32 = 12.0;

/// Per-line penalty added to badness before squaring into demerits — TeX's `\linepenalty`
/// (spec 0017). Biases total-fit toward fewer lines when badness is otherwise equal.
pub const LINE_PENALTY: f32 = 10.0;

/// Badness ceiling. A near-empty line's cubic badness would explode; TeX caps "infinitely bad"
/// at 10000 (spec 0017's "clamped to a ceiling for the near-infinite single-word case").
const BADNESS_CEIL: f32 = 10_000.0;

/// Per-character advance-width metrics. Implemented by the export crate over the embedded font;
/// [`MonospaceMetrics`] is a deterministic stub for tests and headless fallback.
pub trait CharMetrics {
    /// Advance width of `ch` at `size_pt`, in points.
    fn advance_pt(&self, ch: char, size_pt: f32) -> f32;
}

/// A fixed-advance metrics stub: every char advances `em_ratio * size_pt`. Deterministic and
/// font-free — useful for tests and as a fallback before a real font is available.
#[derive(Debug, Clone, Copy)]
pub struct MonospaceMetrics {
    /// Advance as a fraction of the em (e.g. `0.6` ≈ a typical monospace figure width).
    pub em_ratio: f32,
}

impl CharMetrics for MonospaceMetrics {
    fn advance_pt(&self, _ch: char, size_pt: f32) -> f32 {
        self.em_ratio * size_pt
    }
}

/// Measures the shaped advance width of a whole **run** (a string at a size), in points.
///
/// This is the seam a real shaper plugs into: unlike [`CharMetrics`], which measures one codepoint
/// in isolation, `measure_run` sees the whole string, so a `rustybuzz`-backed implementation can
/// account for kerning pairs and ligatures across the run (spec 0016). The per-char `CharMetrics`
/// trait remains for the monospace stub and as a fallback; a run with no kerning measures identically
/// under either.
pub trait RunMetrics {
    /// Total shaped advance width of `text` at `size_pt`, in points.
    fn measure_run(&self, text: &str, size_pt: f32) -> f32;
}

/// A fixed-advance run-metrics stub: `width = em_ratio * size_pt * text.chars().count()`.
///
/// Deterministic and font-free, it reproduces the per-char monospace sum exactly — so line breaking
/// under it is byte-for-byte identical to spec 0015's per-char breaker (no shaping means no change).
/// The no-shaper fallback and the test double.
#[derive(Debug, Clone, Copy)]
pub struct MonospaceRunMetrics {
    /// Advance as a fraction of the em (e.g. `0.6` ≈ a typical monospace figure width).
    pub em_ratio: f32,
}

impl RunMetrics for MonospaceRunMetrics {
    fn measure_run(&self, text: &str, size_pt: f32) -> f32 {
        self.em_ratio * size_pt * text.chars().count() as f32
    }
}

/// Break `text` into lines whose measured advance fits within `max_width_pt`, using a greedy
/// word-based strategy at `size_pt` under `metrics`.
///
/// A word is appended to the current line while the **whole prospective line** `current + " " + word`
/// measures `<= max_width_pt` under [`RunMetrics::measure_run`] — measuring the entire candidate (not
/// the incremental word) is what lets a real shaper count kerning/ligatures across the joining space.
/// Otherwise a new line starts. A word wider than `max_width_pt` on its own is placed alone (it
/// overflows — breaking oversized words / hyphenation is deferred). Whitespace is normalized to
/// single spaces between words; empty text yields no lines.
pub fn break_by_width(
    text: &str,
    max_width_pt: f32,
    size_pt: f32,
    metrics: &impl RunMetrics,
) -> Vec<String> {
    let mut lines = Vec::new();
    let mut current = String::new();
    for word in text.split_whitespace() {
        if current.is_empty() {
            current.push_str(word);
            continue;
        }
        // Measure the full candidate line so cross-word shaping is captured once a shaper exists.
        let mut candidate = String::with_capacity(current.len() + 1 + word.len());
        candidate.push_str(&current);
        candidate.push(' ');
        candidate.push_str(word);
        if metrics.measure_run(&candidate, size_pt) <= max_width_pt {
            current = candidate;
        } else {
            lines.push(std::mem::take(&mut current));
            current.push_str(word);
        }
    }
    if !current.is_empty() {
        lines.push(current);
    }
    lines
}

/// Break `text` into lines using **Knuth-Plass total-fit** (spec 0017, increment 1): all
/// breakpoints in the paragraph are chosen together to minimize the sum of per-line *demerits*,
/// rather than greedily stuffing each line ([`break_by_width`]). The returned shape is identical —
/// a `Vec<String>` of ragged, left-aligned lines — so the writer is unchanged; only *which words
/// land on which line* differs. Justified rendering (stretching inter-word space) is increment 2.
///
/// # Model
///
/// The paragraph becomes a box/glue item stream measured under `metrics` at `size_pt`: each word is
/// a **box** of width `measure_run(word)`, each inter-word space a **glue** of natural width
/// `g = measure_run(" ")` with `stretch = g/2`, `shrink = g/3`. A candidate line spanning a run of
/// words has natural width `W = Σ box + Σ interior glue`, total stretch `Y`, total shrink `Z`, against
/// target `L = max_width_pt`:
///
/// - **Adjustment ratio** `r`: `0` if `Y = Z = 0`; `(L−W)/Y` if `W ≤ L`; `(L−W)/Z` if `W > L`.
/// - **Feasible** iff `r ≥ −1` (a line cannot shrink past its shrink). An over-wide single word has
///   `Z = 0` and `W > L`, so it is infeasible.
/// - **Badness** `b(r) = 100·|r|³`, clamped to [`BADNESS_CEIL`]. The paragraph's last line is not
///   penalized for being short: badness is `0` when it fits (`W ≤ L`).
/// - **Demerits** `= (LINE_PENALTY + b)²`.
///
/// Total-fit returns the feasible breaking of least summed demerits. Ties break deterministically by
/// **fewest lines**, then the **lexicographically earliest line-start sequence**, so identical input
/// always yields identical lines.
///
/// # Degenerate & fallback behavior (parity with [`break_by_width`])
///
/// Whitespace is normalized to single spaces; empty text yields no lines; text that fits yields one
/// line. If *no* fully-feasible breaking exists — some word is wider than `max_width_pt` and forces an
/// overflow line — this **falls back to the greedy [`break_by_width`] result** rather than failing:
/// laying text out (even overflowing) is always recoverable. `break_by_width` is retained as this
/// fallback and as the parity oracle.
pub fn break_paragraph(
    text: &str,
    max_width_pt: f32,
    size_pt: f32,
    metrics: &impl RunMetrics,
) -> Vec<String> {
    let words: Vec<&str> = text.split_whitespace().collect();
    if words.is_empty() {
        return Vec::new();
    }

    let l = max_width_pt;
    let g = metrics.measure_run(" ", size_pt);
    let stretch = g / 2.0;
    let shrink = g / 3.0;

    // Prefix sums of box widths so a line's natural box total is O(1): box_prefix[k] = Σ_{i<k} box_i.
    let mut box_prefix = Vec::with_capacity(words.len() + 1);
    box_prefix.push(0.0f32);
    for &w in &words {
        let prev = *box_prefix.last().unwrap();
        box_prefix.push(prev + metrics.measure_run(w, size_pt));
    }

    // Demerits of the line covering words[i..=j] (0-based, inclusive). `None` if infeasible.
    // Returned as `f64`: a ceiling-badness line squares to ≈ 1e8, where `f32`'s ulp (~8) is coarse
    // enough to mask the small demerit differences total-fit must compare in the all-bad regime, so
    // the DP accumulates in `f64` to stay exactly optimal there too.
    let line_cost = |i: usize, j: usize, is_last: bool| -> Option<f64> {
        let glues = (j - i) as f32; // interior glue count
        let natural = (box_prefix[j + 1] - box_prefix[i]) + glues * g;
        let y = glues * stretch;
        let z = glues * shrink;
        let badness = if is_last && natural <= l {
            // Last line is not penalized for being short — its trailing glue stretches freely.
            0.0
        } else if natural > l {
            // Overfull: must shrink. Infeasible if it shrinks past its shrink (r < −1) or has none.
            if z <= 0.0 {
                return None; // an over-wide single word: r < −1 unavoidable → breaking infeasible
            }
            let r = (l - natural) / z; // r < 0
            if r < -1.0 {
                return None;
            }
            (100.0 * r.abs().powi(3)).min(BADNESS_CEIL)
        } else if y > 0.0 {
            // Underfull with glue to stretch.
            let r = (l - natural) / y; // r ≥ 0
            (100.0 * r.powi(3)).min(BADNESS_CEIL)
        } else if (l - natural).abs() < f32::EPSILON {
            0.0 // single word filling the frame exactly (Y = Z = 0, r = 0)
        } else {
            // Underfull single word with no glue to stretch: cannot be justified → the near-infinite
            // single-word case the badness ceiling exists for (spec 0017). Total-fit thus avoids
            // stranding a lone short word on an interior line, keeping the paragraph balanced.
            BADNESS_CEIL
        };
        let d = f64::from(LINE_PENALTY + badness);
        Some(d * d)
    };

    // Total-fit DP over word prefixes. best[k] = the least-cost breaking of the first k words, with
    // a break after word k−1. `starts` records each line's first-word index (for reconstruction and
    // the lexicographic tie-break). best[0] is the empty prefix.
    #[derive(Clone)]
    struct Node {
        demerits: f64,
        lines: usize,
        starts: Vec<usize>,
    }
    let n = words.len();
    let mut best: Vec<Option<Node>> = vec![None; n + 1];
    best[0] = Some(Node {
        demerits: 0.0,
        lines: 0,
        starts: Vec::new(),
    });

    for k in 1..=n {
        let is_last = k == n;
        for i in 0..k {
            let Some(prev) = best[i].clone() else {
                continue;
            };
            let Some(cost) = line_cost(i, k - 1, is_last) else {
                continue;
            };
            let mut starts = prev.starts.clone();
            starts.push(i);
            let cand = Node {
                demerits: prev.demerits + cost,
                lines: prev.lines + 1,
                starts,
            };
            let better = match &best[k] {
                None => true,
                Some(cur) => {
                    // Absolute tolerance: with f64 accumulation, genuinely-equal breakings differ by
                    // at most a few ulps (≈ 1e-8 at these magnitudes), while the smallest meaningful
                    // demerit gap is ≫ 1e-6 — so this treats only true ties as ties, then applies the
                    // deterministic tie-break (fewest lines, then earliest line-start sequence).
                    let eps = 1e-6;
                    if cand.demerits < cur.demerits - eps {
                        true
                    } else if cand.demerits > cur.demerits + eps {
                        false
                    } else if cand.lines != cur.lines {
                        cand.lines < cur.lines
                    } else {
                        cand.starts < cur.starts
                    }
                }
            };
            if better {
                best[k] = Some(cand);
            }
        }
    }

    // No fully-feasible breaking (an over-wide word forced every line infeasible) → greedy fallback.
    let Some(solution) = best[n].take() else {
        return break_by_width(text, max_width_pt, size_pt, metrics);
    };

    // Reconstruct lines: consecutive start indices delimit each line's word range; the last runs to n.
    let starts = solution.starts;
    let mut lines = Vec::with_capacity(starts.len());
    for (idx, &from) in starts.iter().enumerate() {
        let to = starts.get(idx + 1).copied().unwrap_or(n);
        lines.push(words[from..to].join(" "));
    }
    lines
}

#[cfg(test)]
mod tests {
    use super::*;

    /// `em_ratio` chosen so 10 pt text advances 6 pt/char — matching the old `APPROX_CHAR_WIDTH_PT`
    /// stand-in, so these tests read against a familiar 6-pt-per-character grid. `MONO` measures runs
    /// (drives `break_by_width`); the per-char `MonospaceMetrics` sibling is exercised inline by
    /// `monospace_metrics_scales_with_size`.
    const MONO: MonospaceRunMetrics = MonospaceRunMetrics { em_ratio: 0.6 };
    const SIZE: f32 = BODY_FONT_SIZE_PT; // 10.0 → 6 pt/char under MONO

    #[test]
    fn wraps_on_word_boundaries_by_width() {
        // 6 pt/char. "the quick" = 9 chars = 54 pt; adding " brown" (6 chars incl. space) = 90 pt.
        // A 54 pt cap fits "the quick" (54) but not "the quick brown" (90) → breaks after "quick".
        let lines = break_by_width("the quick brown fox", 54.0, SIZE, &MONO);
        assert_eq!(
            lines,
            vec!["the quick".to_string(), "brown fox".to_string()]
        );
    }

    #[test]
    fn parity_with_old_char_count_breaker() {
        // Old breaker: max_chars = width / 6.0. For width 54 pt that is 9 chars — the same boundary
        // this width breaker finds under MONO, confirming the stub reproduces prior behavior.
        let lines = break_by_width("the quick brown fox", 54.0, SIZE, &MONO);
        assert!(lines.iter().all(|l| l.chars().count() <= 9));
    }

    #[test]
    fn oversized_word_placed_alone() {
        // "elephantine" = 11 chars = 66 pt > 30 pt cap → it gets its own (overflowing) line.
        let lines = break_by_width("a elephantine cat", 30.0, SIZE, &MONO);
        assert_eq!(
            lines,
            vec![
                "a".to_string(),
                "elephantine".to_string(),
                "cat".to_string()
            ]
        );
    }

    #[test]
    fn empty_text_yields_no_lines() {
        assert!(break_by_width("", 100.0, SIZE, &MONO).is_empty());
        assert!(break_by_width("   \t\n ", 100.0, SIZE, &MONO).is_empty());
    }

    #[test]
    fn all_words_fit_on_one_line() {
        let lines = break_by_width("tiny bit here", 1000.0, SIZE, &MONO);
        assert_eq!(lines, vec!["tiny bit here".to_string()]);
    }

    #[test]
    fn monospace_metrics_scales_with_size() {
        let m = MonospaceMetrics { em_ratio: 0.5 };
        assert_eq!(m.advance_pt('W', 20.0), 10.0);
        assert_eq!(m.advance_pt('.', 20.0), 10.0); // width is char-independent for the stub
    }

    #[test]
    fn monospace_run_metrics_is_char_count_times_em() {
        let m = MonospaceRunMetrics { em_ratio: 0.5 };
        // 4 chars × 0.5 em × 20 pt = 40 pt; empty run is zero-width.
        assert_eq!(m.measure_run("WXYZ", 20.0), 40.0);
        assert_eq!(m.measure_run("", 20.0), 0.0);
        // The joining space counts: "ab cd" is 5 chars → 5 × 6 pt = 30 pt at SIZE under MONO.
        assert_eq!(MONO.measure_run("ab cd", SIZE), 30.0);
    }

    // --- spec 0017: Knuth-Plass total-fit line breaking -------------------------------------------

    /// Recompute a breaking's total demerits under `MONO`, replicating `break_paragraph`'s cost
    /// model (last line free; box/glue widths are additive under `MONO`, so `measure_run(line)` is
    /// the line's natural width exactly). Lets the crafted test assert KP < greedy directly.
    fn total_demerits(lines: &[String], l: f32) -> f32 {
        let g = MONO.measure_run(" ", SIZE); // 6 pt
        let (stretch, shrink) = (g / 2.0, g / 3.0);
        let last = lines.len().saturating_sub(1);
        lines
            .iter()
            .enumerate()
            .map(|(idx, line)| {
                let glues = line.split_whitespace().count().saturating_sub(1) as f32;
                let natural = MONO.measure_run(line, SIZE);
                let is_last = idx == last;
                let badness = if is_last && natural <= l {
                    0.0
                } else if natural > l {
                    let r = (l - natural) / (glues * shrink);
                    assert!(r >= -1.0, "test breaking has an infeasible line");
                    (100.0 * r.abs().powi(3)).min(10_000.0)
                } else if glues > 0.0 {
                    let r = (l - natural) / (glues * stretch);
                    (100.0 * r.powi(3)).min(10_000.0)
                } else {
                    0.0
                };
                let d = LINE_PENALTY + badness;
                d * d
            })
            .sum()
    }

    #[test]
    fn optimal_beats_greedy_on_crafted_paragraph() {
        // "an ox in the mud": char counts [2, 2, 2, 3, 3]; 6 pt/char, 6 pt glue under MONO at SIZE.
        // At L = 69 pt greedy overfills line 1 loosely and KP tightens it by pulling one word up:
        //   greedy "an ox in"        W = 3·12 + 2·6 = 48 → r = (69−48)/6 = +3.5  (very loose)
        //   KP     "an ox in the"    W = 54  + 3·6 = 72 → r = (69−72)/6 = −0.5  (mild shrink)
        // Demerits (last line free in both, contributing (10+0)² = 100):
        //   greedy = (10 + 100·3.5³)² + 100 = 18 468 506.25 + 100 = 18 468 606.25
        //   KP     = (10 + 100·0.5³)² + 100 =        506.25 + 100 =          606.25
        // KP is the hand-computed total-fit optimum (brute-force-verified).
        const L: f32 = 69.0;
        let text = "an ox in the mud";
        let kp = break_paragraph(text, L, SIZE, &MONO);
        let greedy = break_by_width(text, L, SIZE, &MONO);

        assert_eq!(kp, vec!["an ox in the".to_string(), "mud".to_string()]);
        assert_eq!(greedy, vec!["an ox in".to_string(), "the mud".to_string()]);
        assert!(
            total_demerits(&kp, L) < total_demerits(&greedy, L),
            "KP demerits {} should be < greedy {}",
            total_demerits(&kp, L),
            total_demerits(&greedy, L),
        );
    }

    #[test]
    fn degenerate_parity_with_break_by_width() {
        // Empty / whitespace → no lines.
        assert!(break_paragraph("", 100.0, SIZE, &MONO).is_empty());
        assert!(break_paragraph("  \t\n ", 100.0, SIZE, &MONO).is_empty());
        // All fits → exactly one line.
        assert_eq!(
            break_paragraph("tiny bit here", 1000.0, SIZE, &MONO),
            vec!["tiny bit here".to_string()]
        );
        // A single over-wide word is emitted alone (overflows), same as greedy.
        assert_eq!(
            break_paragraph("elephantine", 30.0, SIZE, &MONO),
            vec!["elephantine".to_string()]
        );
    }

    #[test]
    fn no_feasible_breaking_falls_back_to_greedy() {
        // "elephantine" (66 pt) can never sit on a feasible line at L = 30 → no full feasible
        // breaking exists → break_paragraph falls back to the greedy result and never panics/empties.
        let text = "a elephantine cat";
        let kp = break_paragraph(text, 30.0, SIZE, &MONO);
        assert_eq!(kp, break_by_width(text, 30.0, SIZE, &MONO));
        assert_eq!(
            kp,
            vec![
                "a".to_string(),
                "elephantine".to_string(),
                "cat".to_string()
            ]
        );
        assert!(!kp.is_empty());
    }

    #[test]
    fn deterministic_across_runs() {
        // Same (text, width, size, metrics) → identical lines every time (tie-break is pinned).
        let text = "the balanced paragraph must break the same way each and every run without fail";
        let first = break_paragraph(text, 120.0, SIZE, &MONO);
        for _ in 0..8 {
            assert_eq!(break_paragraph(text, 120.0, SIZE, &MONO), first);
        }
    }

    #[test]
    fn optimal_even_when_a_line_hits_the_badness_ceiling() {
        // Regression for the all-bad regime: when a breaking is forced to include a ceiling-badness
        // line, total-fit must still pick the least-demerit option among the bad ones. Demerits here
        // sit at ≈ 1e8, where f32's ulp would mask the ~33.7 gap between the two candidates — the DP
        // accumulates in f64 to compare them correctly.
        // "fox a fox ox" ([3,1,3,2] chars) at L = 47 pt:
        //   ["fox a", "fox ox"]  → line1 clamped (r≈5.67), last free  → 100 200 100 + 100 = 100 200 200
        //   ["fox", "a fox ox"]  → line1 lone "fox" = CEIL, last shrinks → 100 200 100 + 133.69 ≈ 100 200 234
        // The first is strictly better and must be chosen.
        let lines = break_paragraph("fox a fox ox", 47.0, SIZE, &MONO);
        assert_eq!(lines, vec!["fox a".to_string(), "fox ox".to_string()]);
    }

    #[test]
    fn total_fit_avoids_stranding_a_short_word_on_an_interior_line() {
        // Total-fit keeps interior lines full: an underfull single-word interior line is near-
        // infinitely bad (BADNESS_CEIL), so KP fills line 1 rather than leaving "abc" alone.
        // "abc def" = 3+1+3 = 7 chars = 42 pt = L exactly (r = 0, badness 0); "ghi" trails free.
        // The alternative ["abc", "def ghi"] would strand "abc" (18 pt ≪ 42 pt) → CEIL badness.
        let lines = break_paragraph("abc def ghi", 42.0, SIZE, &MONO);
        assert_eq!(lines, vec!["abc def".to_string(), "ghi".to_string()]);
    }
}
