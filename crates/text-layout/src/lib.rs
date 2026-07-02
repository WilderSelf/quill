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
//! `rustybuzz`-backed [`RunMetrics`] that actually shapes lands in the follow-up. Press-quality
//! Knuth-Plass justification and hyphenation arrive in later spec-driven increments.

/// Body/heading font size, in points. Shared by the layout engine (to measure and to reserve row
/// height) and the writer (to set the font size), so text is measured at the size it is drawn.
pub const BODY_FONT_SIZE_PT: f32 = 10.0;
/// Body/heading line advance (leading), in points. Deriving leading from font metrics is deferred
/// (spec 0015 non-goal); increment 1 keeps a fixed value.
pub const BODY_LINE_HEIGHT_PT: f32 = 12.0;

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
}
