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
//! no-feasible-breaking case and as the parity oracle.
//!
//! Spec 0017 (increment 2) adds [`justify_paragraph`]: it keeps `break_paragraph`'s breakpoints but
//! **resolves each line's inter-word adjustment** so the writer can stretch/shrink the spaces to fill
//! the frame ([`Alignment::Justified`]) — the paragraph's last line and single-word lines stay ragged,
//! and [`Alignment::Left`] leaves everything ragged. The resolved adjustment ([`Line::space_adjust_pt`])
//! is the classic adjustment ratio expressed in points; because every inter-word glue here is
//! identical, filling a line of `spaces` gaps to width `L` reduces to adding `(L − W) / spaces` per gap.
//!
//! Spec 0018 (increment 1) generalizes the breaker from a box/glue model to a box/glue/**penalty**
//! item stream and introduces the [`Hyphenator`] seam: each legal in-word break point becomes a
//! *flagged penalty* (cost [`HYPHEN_PENALTY`], materializing a hyphen glyph on break) that Knuth-Plass
//! can choose alongside inter-word glue. The hyphenator-aware entry points are
//! [`break_paragraph_hyphenated`] / [`justify_paragraph_hyphenated`]; the original
//! [`break_paragraph`] / [`justify_paragraph`] are now thin wrappers passing [`NoHyphenator`], so with
//! no hyphenator every word is a single box and breaking is byte-identical to spec 0017 (parity).
//!
//! Spec 0018 (increment 2) supplies the real patterns: the export crate implements the [`Hyphenator`]
//! trait over `hypher` (Knuth-Liang en-US) and threads it through `lay_out`, so long words break at
//! syllable points (rendering a trailing `-`) and an over-wide word with a legal break splits across
//! lines instead of overflowing — narrowing the greedy fallback to the genuinely unbreakable case.
//! The algorithm here is unchanged; only which [`Hyphenator`] the pipeline passes moved.

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

/// Cost of ending a line at a hyphenation break — TeX's `\hyphenpenalty` (spec 0018). Added as `p²`
/// to the line's demerits so total-fit only hyphenates when it meaningfully tightens the paragraph.
pub const HYPHEN_PENALTY: f32 = 50.0;

/// Extra demerit when two consecutive lines both end at a flagged (hyphen) break — TeX's
/// `\doublehyphendemerits` (spec 0018). Discourages "hyphen ladders" (three-plus hyphenated lines
/// in a row).
pub const DOUBLE_HYPHEN_DEMERIT: f32 = 10_000.0;

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

/// Supplies the legal in-word break points a line breaker may hyphenate at (spec 0018).
///
/// This is the seam a real Knuth-Liang hyphenator (`hypher`, increment 2) plugs into. Increment 1 is
/// trait-only and dependency-free: [`NoHyphenator`] never hyphenates (parity with spec 0017), and a
/// deterministic stub exercises the penalty machinery in tests.
pub trait Hyphenator {
    /// Byte offsets **inside** `word` at which a hyphen may be inserted: strictly interior
    /// (`0 < off < word.len()`), ascending, on `char` boundaries. Empty = do not hyphenate. The
    /// breaker defensively ignores any offset violating these rules, so a loose implementation cannot
    /// corrupt a line.
    fn hyphenate(&self, word: &str) -> Vec<usize>;
}

/// The parity default: never hyphenates, so every word is a single box and breaking stays
/// byte-identical to spec 0017.
#[derive(Debug, Clone, Copy)]
pub struct NoHyphenator;

impl Hyphenator for NoHyphenator {
    fn hyphenate(&self, _word: &str) -> Vec<usize> {
        Vec::new()
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

/// Break `text` into lines using **Knuth-Plass total-fit** (spec 0017): all breakpoints in the
/// paragraph are chosen together to minimize the sum of per-line *demerits*, rather than greedily
/// stuffing each line ([`break_by_width`]). Returns a `Vec<String>` of ragged, left-aligned lines.
///
/// This is the parity entry point: it never hyphenates (delegates to [`break_paragraph_hyphenated`]
/// with [`NoHyphenator`]), so its output is byte-identical to before spec 0018.
pub fn break_paragraph(
    text: &str,
    max_width_pt: f32,
    size_pt: f32,
    metrics: &impl RunMetrics,
) -> Vec<String> {
    break_paragraph_hyphenated(text, max_width_pt, size_pt, metrics, &NoHyphenator)
}

/// Knuth-Plass total-fit line breaking over a box/glue/**penalty** item stream (spec 0018,
/// increment 1). Generalizes [`break_paragraph`] so that `hyphenator`'s legal in-word break points
/// become flagged penalties the breaker may choose alongside inter-word glue. The returned shape is
/// unchanged — a `Vec<String>` of ragged, left-aligned lines — but a line that ends at a hyphenation
/// break emits a trailing `-`, and the next line begins with the word's remainder (no leading space).
///
/// # Item stream
///
/// The paragraph is measured under `metrics` at `size_pt` into items:
///
/// - **Box** — a run of a word between hyphenation points, width `measure_run(segment)`. A word with
///   no break points is a single box (spec 0017); a word with `k` interior offsets is `k + 1` boxes
///   separated by penalties.
/// - **Glue** — one inter-word space: natural `g = measure_run(" ")`, `stretch = g/2`, `shrink = g/3`.
/// - **Penalty** — a flagged hyphenation break of cost [`HYPHEN_PENALTY`] whose width
///   `measure_run("-")` is added to a line's natural width **only if the line breaks there**.
///
/// With [`NoHyphenator`] there are no penalties, every word is one box, and both the breakpoints and
/// the reconstructed strings are byte-identical to spec 0017 (parity).
///
/// # Cost
///
/// Per-line badness (adjustment ratio `r`, `b(r) = 100·|r|³` clamped to [`BADNESS_CEIL`], last line
/// free when it fits, feasibility `r ≥ −1`) is spec 0017 unchanged. Demerits extend it in the classic
/// TeX way: a line ending at a hyphen penalty adds `HYPHEN_PENALTY²`, and two consecutive flagged
/// lines add [`DOUBLE_HYPHEN_DEMERIT`]. The DP accumulates in `f64`; ties break by **fewest lines**
/// then the **lexicographically earliest line-start sequence**, so identical input yields identical
/// lines.
///
/// # Fallback
///
/// Whitespace is normalized; empty text yields no lines. If *no* fully-feasible breaking exists (a
/// word wider than `max_width_pt` with no usable hyphenation point forces an overflow), this falls
/// back to the greedy [`break_by_width`] result rather than failing — laying text out, even
/// overflowing, is always recoverable.
pub fn break_paragraph_hyphenated(
    text: &str,
    max_width_pt: f32,
    size_pt: f32,
    metrics: &impl RunMetrics,
    hyphenator: &impl Hyphenator,
) -> Vec<String> {
    let words: Vec<&str> = text.split_whitespace().collect();
    if words.is_empty() {
        return Vec::new();
    }

    let l = max_width_pt;
    let g = metrics.measure_run(" ", size_pt);
    let stretch = g / 2.0;
    let shrink = g / 3.0;
    let hyphen_w = metrics.measure_run("-", size_pt);

    // Build the box/glue/penalty item stream. A word splits at its (validated) hyphenation offsets
    // into segment boxes separated by flagged penalties; inter-word glue joins words.
    enum Item<'a> {
        Boxed { text: &'a str, width: f32 },
        Glue,
        Penalty,
    }
    let mut items: Vec<Item> = Vec::new();
    for (wi, &word) in words.iter().enumerate() {
        if wi > 0 {
            items.push(Item::Glue);
        }
        let mut prev = 0usize;
        for off in hyphenator.hyphenate(word) {
            // Defensively ignore offsets that are not strictly-interior ascending char boundaries.
            if off <= prev || off >= word.len() || !word.is_char_boundary(off) {
                continue;
            }
            let seg = &word[prev..off];
            items.push(Item::Boxed {
                text: seg,
                width: metrics.measure_run(seg, size_pt),
            });
            items.push(Item::Penalty);
            prev = off;
        }
        let seg = &word[prev..];
        items.push(Item::Boxed {
            text: seg,
            width: metrics.measure_run(seg, size_pt),
        });
    }

    // Prefix sums so a line's natural/stretch/shrink over items[s..e] is O(1). Penalty width is 0
    // here (it only materializes when a line *breaks* at the penalty — added via `extra_w` below).
    let n_items = items.len();
    let mut wsum = vec![0.0f32; n_items + 1];
    let mut ysum = vec![0.0f32; n_items + 1];
    let mut zsum = vec![0.0f32; n_items + 1];
    for (i, it) in items.iter().enumerate() {
        let (w, y, z) = match it {
            Item::Boxed { width, .. } => (*width, 0.0, 0.0),
            Item::Glue => (g, stretch, shrink),
            Item::Penalty => (0.0, 0.0, 0.0),
        };
        wsum[i + 1] = wsum[i] + w;
        ysum[i + 1] = ysum[i] + y;
        zsum[i + 1] = zsum[i] + z;
    }

    // Spec-0017 badness → `(LINE_PENALTY + b)²` for the line spanning items[s..e], with `extra_w`
    // added to the natural width (the hyphen when breaking at a penalty). `None` if infeasible.
    // `f64`: a ceiling-badness line squares to ≈ 1e8, where `f32`'s ulp is coarse enough to mask the
    // small differences total-fit compares in the all-bad regime.
    let base_demerits = |s: usize, e: usize, extra_w: f32, is_last: bool| -> Option<f64> {
        let natural = (wsum[e] - wsum[s]) + extra_w;
        let y = ysum[e] - ysum[s];
        let z = zsum[e] - zsum[s];
        let badness = if is_last && natural <= l {
            0.0
        } else if natural > l {
            if z <= 0.0 {
                return None; // over-wide with no shrink: r < −1 unavoidable → infeasible
            }
            let r = (l - natural) / z;
            if r < -1.0 {
                return None;
            }
            (100.0 * r.abs().powi(3)).min(BADNESS_CEIL)
        } else if y > 0.0 {
            let r = (l - natural) / y;
            (100.0 * r.powi(3)).min(BADNESS_CEIL)
        } else if (l - natural).abs() < f32::EPSILON {
            0.0
        } else {
            BADNESS_CEIL // underfull line with no glue to stretch (a lone short segment)
        };
        let d = f64::from(LINE_PENALTY + badness);
        Some(d * d)
    };

    // Total-fit DP over legal breakpoints (glue + penalty item indices, plus a forced terminal at
    // end-of-stream). best[s] = least-cost breaking whose next line starts at item index `s`.
    // `starts` records each line's first-item index; `ended_flagged` drives the double-hyphen rule.
    #[derive(Clone)]
    struct Node {
        demerits: f64,
        lines: usize,
        starts: Vec<usize>,
        ended_flagged: bool,
    }
    let is_better = |cand: &Node, cur: &Option<Node>| -> bool {
        match cur {
            None => true,
            Some(c) => {
                let eps = 1e-6;
                if cand.demerits < c.demerits - eps {
                    true
                } else if cand.demerits > c.demerits + eps {
                    false
                } else if cand.lines != c.lines {
                    cand.lines < c.lines
                } else {
                    cand.starts < c.starts
                }
            }
        }
    };

    let mut best: Vec<Option<Node>> = vec![None; n_items + 1];
    best[0] = Some(Node {
        demerits: 0.0,
        lines: 0,
        starts: Vec::new(),
        ended_flagged: false,
    });

    // Interior breakpoints, processed in increasing item order so every reachable line-start `s ≤ e`
    // is finalized before `e` reads it (a line s..e ends at e; the next line starts at e+1 > e).
    for e in 0..n_items {
        let (extra_w, flagged) = match &items[e] {
            Item::Glue => (0.0, false),
            Item::Penalty => (hyphen_w, true),
            Item::Boxed { .. } => continue, // a box is never a line end
        };
        for s in 0..=e {
            let Some(prev) = best[s].clone() else {
                continue;
            };
            let Some(base) = base_demerits(s, e, extra_w, false) else {
                continue;
            };
            let mut d = prev.demerits + base;
            if flagged {
                d += f64::from(HYPHEN_PENALTY) * f64::from(HYPHEN_PENALTY);
                if prev.ended_flagged {
                    d += f64::from(DOUBLE_HYPHEN_DEMERIT);
                }
            }
            let mut starts = prev.starts.clone();
            starts.push(s);
            let cand = Node {
                demerits: d,
                lines: prev.lines + 1,
                starts,
                ended_flagged: flagged,
            };
            if is_better(&cand, &best[e + 1]) {
                best[e + 1] = Some(cand);
            }
        }
    }

    // Forced terminal line: from any reachable start to end-of-stream, never flagged, last line free.
    let mut solution: Option<Node> = None;
    for (s, slot) in best.iter().enumerate() {
        let Some(prev) = slot.clone() else {
            continue;
        };
        let Some(base) = base_demerits(s, n_items, 0.0, true) else {
            continue;
        };
        let mut starts = prev.starts.clone();
        starts.push(s);
        let cand = Node {
            demerits: prev.demerits + base,
            lines: prev.lines + 1,
            starts,
            ended_flagged: false,
        };
        if is_better(&cand, &solution) {
            solution = Some(cand);
        }
    }

    // No fully-feasible breaking (an over-wide, unbreakable word) → greedy fallback.
    let Some(solution) = solution else {
        return break_by_width(text, max_width_pt, size_pt, metrics);
    };

    // Reconstruct each line's string. A line runs from its start item to just before the next line's
    // start (the break item); the last line runs to end-of-stream. Boxes concatenate, glue emits a
    // space, an un-taken interior penalty emits nothing, and a line that ends at a penalty gets a `-`.
    let starts = solution.starts;
    let mut lines = Vec::with_capacity(starts.len());
    for (idx, &from) in starts.iter().enumerate() {
        let (content_end, ends_at_penalty) = match starts.get(idx + 1) {
            Some(&next) => (next - 1, matches!(items[next - 1], Item::Penalty)),
            None => (n_items, false),
        };
        let mut line = String::new();
        for it in &items[from..content_end] {
            match it {
                Item::Boxed { text, .. } => line.push_str(text),
                Item::Glue => line.push(' '),
                Item::Penalty => {}
            }
        }
        if ends_at_penalty {
            line.push('-');
        }
        lines.push(line);
    }
    lines
}

/// Paragraph alignment (spec 0017, increment 2). Only the two modes this increment renders are
/// present; `Right`/`Center` ride with a later increment (spec 0017 non-goal).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Alignment {
    /// Stretch/shrink inter-word space so each line (except the paragraph's last) fills the frame.
    Justified,
    /// Ragged-right: words sit at their natural advances, no inter-word adjustment.
    Left,
}

/// One laid-out line: its text plus the per-gap inter-word adjustment needed to justify it.
///
/// `space_adjust_pt` is the number of **points to add to each inter-word space** so the line fills
/// the frame — positive stretches, negative shrinks, `0.0` leaves the natural spacing (a ragged
/// last line, a single-word line, or [`Alignment::Left`]). The writer distributes it with a
/// positioned `TJ` (word spacing / `Tw` is unusable: the export font is a Type0/Identity-H
/// composite, and PDF word spacing applies only to single-byte code 32).
#[derive(Debug, Clone, PartialEq)]
pub struct Line {
    /// The line's words joined by single spaces (identical to what [`break_paragraph`] returns).
    pub text: String,
    /// Points to add to each inter-word gap to justify the line (see the struct docs).
    pub space_adjust_pt: f32,
}

/// Break `text` with [`break_paragraph`] (Knuth-Plass total-fit, no hyphenation), then **resolve each
/// line's inter-word adjustment** for `align` (spec 0017, increment 2). The parity entry point —
/// delegates to [`justify_paragraph_hyphenated`] with [`NoHyphenator`].
pub fn justify_paragraph(
    text: &str,
    max_width_pt: f32,
    size_pt: f32,
    align: Alignment,
    metrics: &impl RunMetrics,
) -> Vec<Line> {
    justify_paragraph_hyphenated(text, max_width_pt, size_pt, align, metrics, &NoHyphenator)
}

/// Break `text` with [`break_paragraph_hyphenated`] (Knuth-Plass total-fit over the box/glue/penalty
/// item stream, spec 0018), then **resolve each line's inter-word adjustment** for `align`. The
/// breakpoints are unchanged from the breaker; this only decides how much to stretch/shrink each
/// line's spaces at render time.
///
/// For [`Alignment::Justified`], every line except the paragraph's last is stretched or shrunk to
/// fill `max_width_pt`: with `spaces` interior gaps of natural width `W`, the per-gap add is
/// `(L − W) / spaces` (all glues are identical, so the classic adjustment ratio collapses to an even
/// split). A hyphenated line's natural width `W` includes its trailing hyphen (it is part of the
/// measured text), so such a line still fills the frame exactly. The last line and any single-word
/// line keep natural spacing (`0.0`). [`Alignment::Left`] leaves every line ragged.
///
/// **Fallback:** if any word is wider than `max_width_pt`, the breaker falls back to a greedy breaking
/// whose overflow line cannot be justified without over-shrinking; the whole paragraph is then
/// rendered ragged (`0.0` everywhere) — laying out visibly beats corrupting the spacing. (Breaking an
/// over-wide word at a hyphenation point to narrow this fallback is spec 0018 increment 2.)
pub fn justify_paragraph_hyphenated(
    text: &str,
    max_width_pt: f32,
    size_pt: f32,
    align: Alignment,
    metrics: &impl RunMetrics,
    hyphenator: &impl Hyphenator,
) -> Vec<Line> {
    let lines = break_paragraph_hyphenated(text, max_width_pt, size_pt, metrics, hyphenator);

    // Ragged: Left alignment, or the greedy fallback (some word overflows the frame — its line
    // would need to shrink past its glue, so justifying it would push spaces negative).
    let ragged = align == Alignment::Left
        || text
            .split_whitespace()
            .any(|w| metrics.measure_run(w, size_pt) > max_width_pt);
    if ragged {
        return lines
            .into_iter()
            .map(|text| Line {
                text,
                space_adjust_pt: 0.0,
            })
            .collect();
    }

    let last = lines.len().saturating_sub(1);
    lines
        .into_iter()
        .enumerate()
        .map(|(idx, text)| {
            let spaces = text.split_whitespace().count().saturating_sub(1);
            // Last line stays ragged; a single-word line has no gap to adjust.
            let space_adjust_pt = if idx == last || spaces == 0 {
                0.0
            } else {
                // `measure_run` counts a trailing hyphen, so a hyphenated line fills the frame too.
                let natural = metrics.measure_run(&text, size_pt);
                (max_width_pt - natural) / spaces as f32
            };
            Line {
                text,
                space_adjust_pt,
            }
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    // --- spec 0018 increment 1: penalty item stream + Hyphenator seam ------------------------------

    /// Deterministic test hyphenator: breaks only the crafted words it knows, at fixed byte offsets.
    /// Mirrors how a real `hypher`-backed hyphenator will report interior break points (increment 2).
    struct Stub;
    impl Hyphenator for Stub {
        fn hyphenate(&self, word: &str) -> Vec<usize> {
            match word {
                "defgh" => vec![2],                       // de-fgh
                "bbbbbbbb" | "dddddddd" => vec![2, 4, 6], // every 2 chars
                _ => Vec::new(),
            }
        }
    }

    /// Recompute a hyphenated breaking's total demerits from the rendered lines, mirroring
    /// `break_paragraph_hyphenated`'s cost model (a trailing `-` marks a flagged line; `measure_run`
    /// counts the hyphen in the natural width). `apply_double` toggles the double-hyphen term so a
    /// test can show that term is exactly what flips the optimum.
    fn hy_demerits(lines: &[&str], l: f32, apply_double: bool) -> f64 {
        let g = MONO.measure_run(" ", SIZE);
        let (stretch, shrink) = (g / 2.0, g / 3.0);
        let last = lines.len().saturating_sub(1);
        let mut prev_flagged = false;
        let mut total = 0.0f64;
        for (idx, line) in lines.iter().enumerate() {
            let flagged = line.ends_with('-');
            let glues = line.split_whitespace().count().saturating_sub(1) as f32;
            let natural = MONO.measure_run(line, SIZE); // counts a trailing '-'
            let is_last = idx == last;
            let badness = if is_last && natural <= l {
                0.0
            } else if natural > l {
                let r = (l - natural) / (glues * shrink);
                assert!(r >= -1.0, "test breaking has an infeasible line");
                (100.0 * r.abs().powi(3)).min(BADNESS_CEIL)
            } else if glues > 0.0 {
                let r = (l - natural) / (glues * stretch);
                (100.0 * r.powi(3)).min(BADNESS_CEIL)
            } else if (l - natural).abs() < f32::EPSILON {
                0.0
            } else {
                BADNESS_CEIL
            };
            let d = f64::from(LINE_PENALTY + badness);
            let mut cost = d * d;
            if flagged {
                cost += f64::from(HYPHEN_PENALTY) * f64::from(HYPHEN_PENALTY);
                if prev_flagged && apply_double {
                    cost += f64::from(DOUBLE_HYPHEN_DEMERIT);
                }
            }
            total += cost;
            prev_flagged = flagged;
        }
        total
    }

    #[test]
    fn no_hyphenator_is_parity_with_spec_0017() {
        // With NoHyphenator every word is a single box, so the item-stream DP must reproduce the
        // exact spec-0017 line strings. These expectations are pinned independently of the breaker
        // (mirroring the hand-computed spec-0017 cases), so this guards parity by output rather than
        // by delegating to the wrapper.
        let cases: [(&str, f32, &[&str]); 4] = [
            ("an ox in the mud", 69.0, &["an ox in the", "mud"]),
            ("abc def ghi", 42.0, &["abc def", "ghi"]),
            ("fox a fox ox", 47.0, &["fox a", "fox ox"]),
            ("a elephantine cat", 30.0, &["a", "elephantine", "cat"]), // greedy-fallback path
        ];
        for (text, l, expected) in cases {
            assert_eq!(
                break_paragraph_hyphenated(text, l, SIZE, &MONO, &NoHyphenator),
                expected,
            );
        }
    }

    #[test]
    fn penalty_break_tightens_the_fit() {
        // "abc defgh" at L = 42 (7 chars). Without hyphenation the only feasible breaking strands
        // "abc" alone (a lone short box → badness ceiling); hyphenating "defgh" at offset 2 fills
        // line 1 exactly:
        //   hyph  ["abc de-", "fgh"]  line1 = abc18 + glue6 + de12 + hyphen6 = 42 = L (badness 0),
        //                             + HYPHEN_PENALTY² = 100 + 2500 = 2600; last "fgh" free = 100.
        //   plain ["abc", "defgh"]    line1 lone "abc" = badness ceiling → (10 + 10000)² ≈ 1.0e8.
        const L: f32 = 42.0;
        let hy = break_paragraph_hyphenated("abc defgh", L, SIZE, &MONO, &Stub);
        let plain = break_paragraph("abc defgh", L, SIZE, &MONO);
        assert_eq!(hy, vec!["abc de-".to_string(), "fgh".to_string()]);
        assert_eq!(plain, vec!["abc".to_string(), "defgh".to_string()]);
        // The broken line ends in a hyphen whose width is counted in the (frame-filling) line.
        assert!(hy[0].ends_with('-'));
        assert!((MONO.measure_run(&hy[0], SIZE) - L).abs() < 1e-3);
        // Total demerits (including HYPHEN_PENALTY²) are far lower than the non-hyphenated breaking.
        let plain_ref: Vec<&str> = plain.iter().map(String::as_str).collect();
        let hy_ref: Vec<&str> = hy.iter().map(String::as_str).collect();
        assert!(hy_demerits(&hy_ref, L, true) < hy_demerits(&plain_ref, L, true));
    }

    #[test]
    fn double_hyphen_demerit_breaks_a_hyphen_ladder() {
        // "aa bbbbbbbb cccc dddddddd" at L = 69, both 8-char words hyphenatable every 2 chars.
        // Without \doublehyphendemerits the optimum is a two-consecutive-hyphen ladder; the
        // double-hyphen term (10000) flips it to a breaking with a single, non-adjacent hyphen.
        // Both are feasible; the chosen one has higher *base* demerits but wins once the term applies
        // (all numbers brute-force-verified against an exhaustive breaker):
        //   chosen ["aa bbbbbbbb", "cccc dddd-", "dddd"]    → ≈ 7 358 800 (1 hyphen, no ladder)
        //   ladder ["aa bbbbbb-", "bb cccc dd-", "dddddd"]  → base ≈ 7 349 706 (2 adjacent hyphens)
        // (last line is free in both, so the differing remainder does not affect the comparison.)
        const L: f32 = 69.0;
        let chosen = ["aa bbbbbbbb", "cccc dddd-", "dddd"];
        let ladder = ["aa bbbbbb-", "bb cccc dd-", "dddddd"];

        let got = break_paragraph_hyphenated("aa bbbbbbbb cccc dddddddd", L, SIZE, &MONO, &Stub);
        assert_eq!(got, chosen);

        // Without the double-hyphen term the ladder is cheaper; with it, the chosen breaking wins.
        assert!(hy_demerits(&ladder, L, false) < hy_demerits(&chosen, L, false));
        assert!(hy_demerits(&chosen, L, true) < hy_demerits(&ladder, L, true));
        // The chosen breaking never has two consecutive hyphenated lines.
        assert!(!(chosen[0].ends_with('-') && chosen[1].ends_with('-')));
    }

    #[test]
    fn hyphenated_breaking_is_deterministic() {
        let text = "abc defgh abc defgh abc defgh";
        let first = break_paragraph_hyphenated(text, 54.0, SIZE, &MONO, &Stub);
        for _ in 0..8 {
            assert_eq!(
                break_paragraph_hyphenated(text, 54.0, SIZE, &MONO, &Stub),
                first
            );
        }
    }

    #[test]
    fn over_wide_word_splits_at_hyphenation_point_instead_of_overflowing() {
        // A single word wider than the frame that HAS a hyphenation point must break across lines
        // rather than overflow (spec 0018 incr. 2 narrows the greedy fallback). "bbbbbbbb" = 8 chars
        // = 48 pt > L = 30 pt; Stub breaks it every 2 chars. The only feasible optimum packs two
        // 2-char segments per line: line 1 = "bbbb-" (4·6 + hyphen 6 = 30 = L, badness 0), last line
        // = "bbbb" (24 pt, free). The greedy fallback would strand the whole 48 pt word on one
        // overflowing line — this must NOT happen.
        const L: f32 = 30.0;
        let hy = break_paragraph_hyphenated("bbbbbbbb", L, SIZE, &MONO, &Stub);
        assert_eq!(hy, vec!["bbbb-".to_string(), "bbbb".to_string()]);
        // It is the DP result, not the greedy overflow fallback.
        assert_ne!(hy, break_by_width("bbbbbbbb", L, SIZE, &MONO));
        // No line overflows the frame (the hyphen width is counted on the broken line).
        assert!(hy.iter().all(|l| MONO.measure_run(l, SIZE) <= L + 1e-3));
    }

    #[test]
    fn unbreakable_over_wide_word_still_falls_back_to_greedy() {
        // A word wider than the frame with NO hyphenation point (Stub returns nothing for it) has no
        // feasible breaking, so the greedy fallback still lays it out (overflowing) rather than
        // panicking or emptying — the spec-0017 principle survives, just with a narrowed surface.
        const L: f32 = 30.0;
        let hy = break_paragraph_hyphenated("elephantine", L, SIZE, &MONO, &Stub);
        assert_eq!(hy, break_by_width("elephantine", L, SIZE, &MONO));
        assert_eq!(hy, vec!["elephantine".to_string()]);
    }

    #[test]
    fn justified_hyphenated_line_fills_the_frame() {
        // The double-hyphen paragraph, justified: the interior hyphenated line "cccc dddd-" is
        // underfull (natural 60 < 69) and stretches to fill the frame — proving the hyphen's width
        // is counted (natural includes the trailing '-').
        const L: f32 = 69.0;
        let lines = justify_paragraph_hyphenated(
            "aa bbbbbbbb cccc dddddddd",
            L,
            SIZE,
            Alignment::Justified,
            &MONO,
            &Stub,
        );
        assert_eq!(lines[1].text, "cccc dddd-");
        assert!(
            (lines[1].space_adjust_pt - 9.0).abs() < 1e-4,
            "adjust = {}",
            lines[1].space_adjust_pt
        );
        let filled = MONO.measure_run(&lines[1].text, SIZE) + lines[1].space_adjust_pt; // 1 gap
        assert!((filled - L).abs() < 1e-3);
    }

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

    // --- spec 0017 increment 2: justified rendering (adjustment carried out of layout) -----------

    /// The measured width of `line` under MONO plus the total inter-word adjustment `line` carries.
    fn filled_width(line: &Line) -> f32 {
        let spaces = line.text.split_whitespace().count().saturating_sub(1) as f32;
        MONO.measure_run(&line.text, SIZE) + spaces * line.space_adjust_pt
    }

    #[test]
    fn justified_interior_lines_fill_the_frame_last_stays_ragged() {
        // Same crafted paragraph as the KP test: KP breaks "an ox in the mud" into
        //   ["an ox in the", "mud"] at L = 69. Interior line "an ox in the" has W = 72 (overfull),
        //   3 gaps → per-gap add = (69 − 72)/3 = −1.0 (mild shrink). Last line "mud" stays ragged.
        const L: f32 = 69.0;
        let lines = justify_paragraph("an ox in the mud", L, SIZE, Alignment::Justified, &MONO);
        assert_eq!(lines.len(), 2);
        assert_eq!(lines[0].text, "an ox in the");
        assert!(
            (lines[0].space_adjust_pt - (-1.0)).abs() < 1e-4,
            "adjust = {}",
            lines[0].space_adjust_pt
        );
        // Interior line now fills the frame exactly; last line is untouched.
        assert!((filled_width(&lines[0]) - L).abs() < 1e-3);
        assert_eq!(lines[1].text, "mud");
        assert_eq!(lines[1].space_adjust_pt, 0.0);
    }

    #[test]
    fn justified_underfull_line_stretches() {
        // "aa bb cc dd" then a trailing word forcing two lines. At L = 90, KP puts the first four
        // 2-char words on line 1: W = 4·12 + 3·6 = 66, 3 gaps → add (90 − 66)/3 = +8.0 each.
        let lines = justify_paragraph(
            "aa bb cc dd eeeeeeeeeeeeee",
            90.0,
            SIZE,
            Alignment::Justified,
            &MONO,
        );
        assert_eq!(lines[0].text, "aa bb cc dd");
        assert!(
            (lines[0].space_adjust_pt - 8.0).abs() < 1e-4,
            "adjust = {}",
            lines[0].space_adjust_pt
        );
        assert!((filled_width(&lines[0]) - 90.0).abs() < 1e-3);
    }

    #[test]
    fn left_alignment_is_all_ragged() {
        let lines = justify_paragraph("an ox in the mud", 69.0, SIZE, Alignment::Left, &MONO);
        assert!(lines.iter().all(|l| l.space_adjust_pt == 0.0));
        // Text/breakpoints match break_paragraph exactly.
        let plain = break_paragraph("an ox in the mud", 69.0, SIZE, &MONO);
        assert_eq!(
            lines.iter().map(|l| l.text.clone()).collect::<Vec<_>>(),
            plain
        );
    }

    #[test]
    fn single_word_line_is_not_justified() {
        // One word wider than a narrow frame's other content still can't be justified (no gap).
        let lines = justify_paragraph("hello", 1000.0, SIZE, Alignment::Justified, &MONO);
        assert_eq!(
            lines,
            vec![Line {
                text: "hello".to_string(),
                space_adjust_pt: 0.0
            }]
        );
    }

    #[test]
    fn fallback_paragraph_renders_ragged() {
        // "elephantine" (66 pt) overflows L = 30 → break_paragraph falls back to greedy; justify
        // must leave every line ragged rather than over-shrink the overflow line's neighbours.
        let lines = justify_paragraph("a elephantine cat", 30.0, SIZE, Alignment::Justified, &MONO);
        assert!(lines.iter().all(|l| l.space_adjust_pt == 0.0));
        assert_eq!(
            lines.iter().map(|l| l.text.clone()).collect::<Vec<_>>(),
            break_by_width("a elephantine cat", 30.0, SIZE, &MONO)
        );
    }

    #[test]
    fn justify_is_deterministic() {
        let text = "the balanced paragraph must break the same way each and every run without fail";
        let first = justify_paragraph(text, 120.0, SIZE, Alignment::Justified, &MONO);
        for _ in 0..8 {
            assert_eq!(
                justify_paragraph(text, 120.0, SIZE, Alignment::Justified, &MONO),
                first
            );
        }
    }
}
