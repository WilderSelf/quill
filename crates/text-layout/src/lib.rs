//! Text shaping and line breaking.
//!
//! Real shaping will use `rustybuzz` and press-quality justification will use a Knuth-Plass
//! line breaker; both arrive in later spec-driven commits. This module currently provides a
//! trivial greedy breaker so downstream crates can compile and be exercised.

/// Break `text` into lines that each fit within `max_chars` using a simple greedy strategy.
///
/// Placeholder for the Knuth-Plass optimal breaker (see the plan). Word-based, no hyphenation.
pub fn greedy_break(text: &str, max_chars: usize) -> Vec<String> {
    let mut lines = Vec::new();
    let mut current = String::new();
    for word in text.split_whitespace() {
        if current.is_empty() {
            current.push_str(word);
        } else if current.chars().count() + 1 + word.chars().count() <= max_chars {
            current.push(' ');
            current.push_str(word);
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

    #[test]
    fn wraps_on_word_boundaries() {
        let lines = greedy_break("the quick brown fox", 9);
        assert_eq!(
            lines,
            vec!["the quick".to_string(), "brown fox".to_string()]
        );
    }

    #[test]
    fn empty_text_yields_no_lines() {
        assert!(greedy_break("", 10).is_empty());
    }
}
