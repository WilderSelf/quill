//! Text shaping and line breaking.
//!
//! Increment 1 of spec 0015 replaces the character-count stand-in with real **width-based**
//! greedy line breaking: words are packed onto a line while the line's *measured* advance stays
//! within the frame width. Widths come from a [`CharMetrics`] implementation supplied by the
//! caller (the embedded font in the export path; [`MonospaceMetrics`] in tests) — see
//! `specs/0015-text-metrics-line-breaking.md` for why metrics are passed in rather than stored on
//! the document. Press-quality Knuth-Plass justification, hyphenation, and shaping (`rustybuzz`)
//! arrive in later spec-driven increments.

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

/// Measured advance of `s` at `size_pt` under `metrics`, in points (sum of per-char advances; no
/// kerning/shaping in increment 1).
fn measure(s: &str, size_pt: f32, metrics: &impl CharMetrics) -> f32 {
    s.chars().map(|ch| metrics.advance_pt(ch, size_pt)).sum()
}

/// Break `text` into lines whose measured advance fits within `max_width_pt`, using a greedy
/// word-based strategy at `size_pt` under `metrics`.
///
/// A word is appended to the current line while the measured advance of `current + " " + word`
/// stays `<= max_width_pt`; otherwise a new line starts. A word wider than `max_width_pt` on its
/// own is placed alone (it overflows — breaking oversized words / hyphenation is deferred).
/// Whitespace is normalized to single spaces between words; empty text yields no lines.
pub fn break_by_width(
    text: &str,
    max_width_pt: f32,
    size_pt: f32,
    metrics: &impl CharMetrics,
) -> Vec<String> {
    let space = metrics.advance_pt(' ', size_pt);
    let mut lines = Vec::new();
    let mut current = String::new();
    let mut current_w = 0.0f32;
    for word in text.split_whitespace() {
        let word_w = measure(word, size_pt, metrics);
        if current.is_empty() {
            current.push_str(word);
            current_w = word_w;
        } else if current_w + space + word_w <= max_width_pt {
            current.push(' ');
            current.push_str(word);
            current_w += space + word_w;
        } else {
            lines.push(std::mem::take(&mut current));
            current.push_str(word);
            current_w = word_w;
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
    /// stand-in, so these tests read against a familiar 6-pt-per-character grid.
    const MONO: MonospaceMetrics = MonospaceMetrics { em_ratio: 0.6 };
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
}
