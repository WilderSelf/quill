//! en-US hyphenation backed by [`hypher`] (spec 0018 increment 2).
//!
//! Increment 1 built the penalty item stream and the [`Hyphenator`] seam with a [`NoHyphenator`]
//! parity default. This module supplies the real patterns: [`HypherHyphenator`] adapts Typst's
//! `hypher` (Knuth-Liang, patterns embedded as `no_std` data — zero transitive deps) to the trait.
//! It is built once in `export()` next to the shaper and passed to the layout pass, which breaks
//! words at legal syllable points and renders a trailing hyphen (the breaker already materializes
//! the `-` on a flagged break; the writer draws it like any other glyph, which is why `export`
//! unconditionally subsets `U+002D`).
//!
//! Scope is en-US only (a document-driven language seam is a named spec non-goal). Punctuation-
//! bearing tokens (`"gloom."`) are hyphenated as-is; stripping trailing punctuation is deferred.

use hypher::Lang;
use quill_text_layout::Hyphenator;

/// en-US Knuth-Liang hyphenator over [`hypher`]. Zero-sized: the pattern trie is embedded static
/// data reached through `Lang::English`, so this holds no state and is cheap to pass by reference.
#[derive(Debug, Clone, Copy, Default)]
pub struct HypherHyphenator;

impl Hyphenator for HypherHyphenator {
    /// Interior byte offsets at which `word` may be hyphenated, ascending and on char boundaries.
    ///
    /// `hypher::hyphenate` segments the word into syllables using en-US's own embedded default
    /// bounds (2 left / 3 right — sensible `\lefthyphenmin`/`\righthyphenmin` stubs, spec 0018; we
    /// take hypher's defaults rather than calling `hyphenate_bounded`). The syllables are contiguous
    /// substrings of `word`, so accumulating their byte lengths (dropping the final total) yields the
    /// interior break offsets the [`Hyphenator`] contract wants. hypher's bounds already guarantee
    /// `0 < off < word.len()` on char boundaries; the breaker re-validates anyway.
    fn hyphenate(&self, word: &str) -> Vec<usize> {
        let syllables = hypher::hyphenate(word, Lang::English);
        let n = syllables.len();
        if n <= 1 {
            return Vec::new();
        }
        let mut offsets = Vec::with_capacity(n - 1);
        let mut acc = 0usize;
        for (i, syllable) in syllables.enumerate() {
            acc += syllable.len();
            // The break *after* the last syllable is the word end, not an interior point.
            if i + 1 < n {
                offsets.push(acc);
            }
        }
        offsets
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Every offset a real en-US word reports must satisfy the trait contract: strictly interior,
    /// strictly ascending, and on a `char` boundary of the original word.
    fn assert_valid_offsets(word: &str, offsets: &[usize]) {
        let mut prev = 0usize;
        for &off in offsets {
            assert!(
                off > 0 && off < word.len(),
                "{word}: off {off} out of range"
            );
            assert!(
                off > prev,
                "{word}: offsets not strictly ascending at {off}"
            );
            assert!(
                word.is_char_boundary(off),
                "{word}: off {off} not on a char boundary"
            );
            prev = off;
        }
    }

    #[test]
    fn hyphenates_a_long_word_at_syllable_points() {
        // "hyphenation" segments into syllables under en-US patterns; assert the offsets partition
        // the word (each prefix + the remainder reconstruct it) rather than pinning exact syllable
        // boundaries (those are hypher's to define, not ours to hard-code).
        let hy = HypherHyphenator;
        let offsets = hy.hyphenate("hyphenation");
        assert!(!offsets.is_empty(), "a long word should have break points");
        assert_valid_offsets("hyphenation", &offsets);
        // The segments between offsets, joined, are exactly the word (contiguous partition).
        let mut reconstructed = String::new();
        let mut prev = 0;
        for &off in &offsets {
            reconstructed.push_str(&"hyphenation"[prev..off]);
            prev = off;
        }
        reconstructed.push_str(&"hyphenation"[prev..]);
        assert_eq!(reconstructed, "hyphenation");
    }

    #[test]
    fn short_word_has_no_break_points() {
        // en-US bounds (2 left, 3 right) leave nothing to break in a very short word.
        let hy = HypherHyphenator;
        assert!(hy.hyphenate("cat").is_empty());
        assert!(hy.hyphenate("the").is_empty());
        assert!(hy.hyphenate("a").is_empty());
        assert!(hy.hyphenate("").is_empty());
    }

    #[test]
    fn offsets_are_valid_across_a_vocabulary() {
        let hy = HypherHyphenator;
        for word in [
            "corridor",
            "darkness",
            "creeping",
            "somewhere",
            "extensive",
            "wonderful",
            "dungeon",
            "adventure",
        ] {
            assert_valid_offsets(word, &hy.hyphenate(word));
        }
    }

    #[test]
    fn is_deterministic() {
        let hy = HypherHyphenator;
        let first = hy.hyphenate("hyphenation");
        for _ in 0..8 {
            assert_eq!(hy.hyphenate("hyphenation"), first);
        }
    }
}
