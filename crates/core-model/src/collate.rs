//! **The quill index collation** — the named, tested rule an index's terms are filed in
//! (spec 0078).
//!
//! Nothing in the workspace sorted text before this module. `BTreeMap<String, _>` is byte order,
//! and byte order is wrong in three ways an index makes immediately visible: `Zebra` files before
//! `apple` (every capital sorts before every lower-case letter), `Zürich` files after `Zyklon`
//! (`ü` is U+00FC, past every ASCII letter), and `The Ruined Keep` files under T. The roadmap's M6
//! audit named collation as the one thing in the index increment with **no analogue anywhere in the
//! workspace**, and `Cargo.toml`'s rule — every dependency is permissive and carries a note saying
//! which spec brought it in and why — makes adding a collator a decision rather than an import.
//!
//! The decision, and the reasoning, are in `specs/0078-index.md`. What matters here is the shape:
//!
//! - **Every consumer compares [`CollationKey`]s, never strings.** Producing the key is one
//!   function. Replacing this rule with a full Unicode Collation Algorithm implementation — the
//!   growth path the spec names, and prices — changes that function and nothing else.
//! - **A per-entry exception is authored, not inferred.** `IndexMark::sort_as` supplies the key's
//!   text where the printed form is not what the entry files under: `de Gaulle` under `Gaulle`,
//!   `1984` under `Nineteen Eighty-Four`, `St Albans` under `Saint Albans`. No collation rule in
//!   any library can know those, so the escape hatch is not a workaround for this rule being
//!   limited — every index in publishing has one.
//! - **A language-wide exception is declared data.** `Block::Index::ignore_leading` is the article
//!   list to drop from the front of a term, empty by default, so quill never guesses a language it
//!   was not told. An English book states `["A", "An", "The"]`; a French one states
//!   `["Le", "La", "Les", "L'"]`.
//!
//! ## The rule, stated
//!
//! A term's key is four levels, compared in order and each lexicographically:
//!
//! 1. **Primary** — the base letters. Case is folded away, diacritics are folded away, and
//!    everything that is neither a letter nor a digit is **ignored entirely**, so `co-operate`,
//!    `co operate` and `cooperate` file together and `Keep, the Ruined` files under K. Digits form
//!    a class that sorts before letters, so a numeric entry heads the index rather than landing
//!    wherever its code points fall.
//! 2. **Secondary** — the accents that were folded away, so `resume` files before `résumé` rather
//!    than at random.
//! 3. **Tertiary** — case, lower before upper, so `apple` files before `Apple`.
//! 4. **The term itself**, so the order is **total**: two distinct terms never compare `Equal`, and
//!    collating a set is therefore deterministic rather than dependent on the order the marks were
//!    found in.
//!
//! ## The scope, stated
//!
//! **Diacritic folding covers U+00C0–U+024F** — Latin-1 Supplement, Latin Extended-A, and the part
//! of Latin Extended-B that decomposes to an ASCII letter — plus the digraphs and stroked letters
//! that have no decomposition at all ([`SPECIAL_FOLDS`]). Outside that range a character keeps its
//! own lower-cased self as its primary weight.
//!
//! That is right wherever a script's alphabetical order agrees with its Unicode order after folding
//! — which is true of Greek and of Cyrillic, whose blocks are laid out alphabetically — and it is
//! **wrong wherever a language tailors that order**: Swedish files `å ä ö` after `z`, not among
//! `a` and `o`; Spanish once filed `ch` and `ll` as letters of their own. Those are tailorings, and
//! the fix for a tailoring is a collator, not a patch to this table. `specs/0078-index.md` states
//! what lifting it would take, and what it would cost.

use crate::IndexMark;

/// A term's sort position under the quill index collation.
///
/// Compared field by field in declaration order, which is what `derive(Ord)` means for a struct and
/// is exactly the levelled comparison the module documentation describes. Opaque on purpose: the
/// only thing a caller may do with a key is compare it with another, so the *representation* of a
/// weight is free to change — and would change wholesale if the rule were ever replaced by a UCA
/// collator.
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord)]
pub struct CollationKey {
    /// Base letters and digits, class-tagged. See [`primary_weight`].
    primary: Vec<u32>,
    /// The original character wherever a fold discarded an accent, `0` where it did not — so two
    /// terms that agree on their base letters are separated by their accents and by nothing else.
    secondary: Vec<u32>,
    /// `1` for an upper-case character, `0` otherwise: lower before upper.
    tertiary: Vec<u8>,
    /// The printed term, so the order is total and collating a set is deterministic.
    tiebreak: String,
}

/// Digits sort before letters, and both sort after nothing else — punctuation and spaces are
/// ignored rather than weighted. Shifted past every Unicode scalar value (21 bits), so a class tag
/// and a character can share a `u32` without either colliding with the other.
const CLASS_SHIFT: u32 = 24;
const CLASS_DIGIT: u32 = 1 << CLASS_SHIFT;
const CLASS_LETTER: u32 = 2 << CLASS_SHIFT;

/// Characters that fold to a single ASCII base letter, and what they fold to.
///
/// Generated from Unicode's own canonical decomposition over U+00C0–U+024F — every character in
/// that range whose NFD form begins with an ASCII letter — rather than typed out by hand, because a
/// hand-typed table of 244 entries is a table with a mistake in it. The two constants are parallel
/// and a test asserts they are the same length, which is the one way this pairing can go wrong.
const FOLD_FROM: &str = concat!(
    "ÀÁÂÃÄÅÇÈÉÊËÌÍÎÏÑÒÓÔÕÖÙÚÛÜÝàáâãäåçèéêëìíîïñòóôõöùúûüýÿĀāĂăĄąĆ",
    "ćĈĉĊċČčĎďĒēĔĕĖėĘęĚěĜĝĞğĠġĢģĤĥĨĩĪīĬĭĮįİĴĵĶķĹĺĻļĽľŃńŅņŇňŌōŎŏŐő",
    "ŔŕŖŗŘřŚśŜŝŞşŠšŢţŤťŨũŪūŬŭŮůŰűŲųŴŵŶŷŸŹźŻżŽžƠơƯưǍǎǏǐǑǒǓǔǕǖǗǘǙǚǛ",
    "ǜǞǟǠǡǦǧǨǩǪǫǬǭǰǴǵǸǹǺǻȀȁȂȃȄȅȆȇȈȉȊȋȌȍȎȏȐȑȒȓȔȕȖȗȘșȚțȞȟȦȧȨȩȪȫȬȭȮȯ",
    "ȰȱȲȳ",
);
const FOLD_TO: &str = concat!(
    "aaaaaaceeeeiiiinooooouuuuyaaaaaaceeeeiiiinooooouuuuyyaaaaaac",
    "cccccccddeeeeeeeeeegggggggghhiiiiiiiiijjkkllllllnnnnnnoooooo",
    "rrrrrrssssssssttttuuuuuuuuuuuuwwyyyzzzzzzoouuaaiioouuuuuuuuu",
    "uaaaaggkkoooojggnnaaaaaaeeeeiiiioooorrrruuuusstthhaaeeoooooo",
    "ooyy",
);

/// Letters with **no** canonical decomposition, which therefore cannot come from the table above.
///
/// A ligature is two letters (`Æ` files as `ae`, `ß` as `ss`); a stroked letter is its base
/// (`Ø` files as `o`, `Ł` as `l`). Both are what a European-language index expects, and neither is
/// derivable — `Ø` decomposes to nothing at all, so no amount of normalisation reaches it.
///
/// **Danish, Norwegian and Swedish disagree with this**, and that is the scope limit rather than a
/// bug: they file `ø`/`ö` as a letter after `z`. That is a *tailoring*, and the module docs say what
/// answering it takes.
const SPECIAL_FOLDS: &[(char, &str)] = &[
    ('Æ', "ae"),
    ('æ', "ae"),
    ('Ǽ', "ae"),
    ('ǽ', "ae"),
    ('Ǣ', "ae"),
    ('ǣ', "ae"),
    ('Œ', "oe"),
    ('œ', "oe"),
    ('ß', "ss"),
    ('Þ', "th"),
    ('þ', "th"),
    ('Ð', "d"),
    ('ð', "d"),
    ('Đ', "d"),
    ('đ', "d"),
    ('Ø', "o"),
    ('ø', "o"),
    ('Ǿ', "o"),
    ('ǿ', "o"),
    ('Ł', "l"),
    ('ł', "l"),
    ('Ħ', "h"),
    ('ħ', "h"),
    ('Ŧ', "t"),
    ('ŧ', "t"),
    ('Ŋ', "n"),
    ('ŋ', "n"),
    ('Ĳ', "ij"),
    ('ĳ', "ij"),
    ('ı', "i"),
    ('ſ', "s"),
];

/// What `c` files as, or `None` if it files as itself lower-cased.
fn fold(c: char) -> Option<&'static str> {
    if let Some((_, to)) = SPECIAL_FOLDS.iter().find(|(from, _)| *from == c) {
        return Some(to);
    }
    let idx = FOLD_FROM.char_indices().position(|(_, f)| f == c)?;
    // Both tables are ASCII on the `to` side, so a char index is a byte index there.
    FOLD_TO.get(idx..idx + 1)
}

/// The primary weight of one already-folded, already-lower-cased character.
fn primary_weight(c: char) -> Option<u32> {
    if c.is_numeric() {
        Some(CLASS_DIGIT | c as u32)
    } else if c.is_alphabetic() {
        Some(CLASS_LETTER | c as u32)
    } else {
        // Ignored entirely, which is what makes this a **letter-by-letter** filing rule: spaces,
        // hyphens and commas do not separate. `New York` therefore files after `Newark`, which is
        // one of the two conventions Chicago sanctions; word-by-word is the other and is a named
        // non-goal in the spec rather than an accident here.
        None
    }
}

/// The text a term files under: its authored `sort_as` if it has one, else the term itself with any
/// declared leading article removed.
///
/// The article match is on the *folded* text, so `L'étoile` and `l'Etoile` strip alike, and the
/// **longest** match wins so that `["A", "An"]` cannot make `An Atlas` file under `n` by taking the
/// shorter article first. An article must be followed by a space or an apostrophe: `Theatre` is not
/// `The` + `atre`.
fn sort_text(mark: &IndexMark, ignore_leading: &[String]) -> String {
    if let Some(explicit) = &mark.sort_as {
        return explicit.clone();
    }
    strip_longest_article(&mark.term, ignore_leading).unwrap_or_else(|| mark.term.clone())
}

/// The term with the longest declared leading article removed, or `None` if none applies.
fn strip_longest_article(term: &str, ignore_leading: &[String]) -> Option<String> {
    let folded: String = term.chars().flat_map(char::to_lowercase).collect();
    let mut best: Option<usize> = None;
    for article in ignore_leading {
        let a: String = article.chars().flat_map(char::to_lowercase).collect();
        if a.is_empty() || !folded.starts_with(&a) {
            continue;
        }
        let chars = a.chars().count();
        let Some(next) = term.chars().nth(chars) else {
            continue;
        };
        if !(next.is_whitespace() || next == '\'' || next == '\u{2019}') {
            continue;
        }
        if best.is_none_or(|b| chars > b) {
            best = Some(chars);
        }
    }
    let chars = best?;
    let rest: String = term
        .chars()
        .skip(chars + 1)
        .skip_while(|c| c.is_whitespace())
        .collect();
    // An entry that is *only* an article files under the article: dropping every character would
    // give it an empty primary key and file it ahead of the whole index.
    (!rest.is_empty()).then_some(rest)
}

/// The sort position of `mark` under the quill index collation.
///
/// `ignore_leading` is the index block's declared article list — empty for every document that does
/// not state one, which is what makes the rule language-neutral by default rather than English by
/// default.
pub fn collation_key(mark: &IndexMark, ignore_leading: &[String]) -> CollationKey {
    let text = sort_text(mark, ignore_leading);
    let mut primary = Vec::new();
    let mut secondary = Vec::new();
    let mut tertiary = Vec::new();
    for c in text.chars() {
        let upper = c.is_uppercase();
        match fold(c) {
            Some(base) => {
                for (i, b) in base.chars().enumerate() {
                    let Some(w) = primary_weight(b) else { continue };
                    primary.push(w);
                    // The accent is recorded once, against the first of the letters a ligature
                    // expands to, so `æ` and `ae` differ at the secondary level and nowhere else.
                    secondary.push(if i == 0 { c as u32 } else { 0 });
                    tertiary.push(u8::from(upper));
                }
            }
            None => {
                for b in c.to_lowercase() {
                    let Some(w) = primary_weight(b) else { continue };
                    primary.push(w);
                    secondary.push(0);
                    tertiary.push(u8::from(upper));
                }
            }
        }
    }
    CollationKey {
        primary,
        secondary,
        tertiary,
        tiebreak: mark.term.clone(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn key(term: &str) -> CollationKey {
        collation_key(&IndexMark::new(term), &[])
    }

    fn sorted(terms: &[&str]) -> Vec<String> {
        sorted_with(terms, &[])
    }

    fn sorted_with(terms: &[&str], articles: &[String]) -> Vec<String> {
        let mut v: Vec<&str> = terms.to_vec();
        v.sort_by_key(|t| collation_key(&IndexMark::new(*t), articles));
        v.into_iter().map(str::to_string).collect()
    }

    #[test]
    fn the_two_fold_tables_are_parallel() {
        // The one way a pair of parallel constants goes wrong. Both are generated from Unicode's
        // canonical decomposition, so an edit to either without the other is the realistic mistake.
        assert_eq!(FOLD_FROM.chars().count(), FOLD_TO.chars().count());
        assert!(FOLD_TO.is_ascii(), "every fold target is an ASCII letter");
    }

    #[test]
    fn case_does_not_decide_the_order() {
        // Byte order puts every capital before every lower-case letter, so this is `Zebra, apple`
        // under the rule the workspace had before this module.
        assert_eq!(sorted(&["Zebra", "apple"]), ["apple", "Zebra"]);
        // ...and case is still a tie-break rather than nothing, so the order is deterministic.
        assert_eq!(sorted(&["Apple", "apple"]), ["apple", "Apple"]);
    }

    #[test]
    fn a_diacritic_does_not_throw_a_term_to_the_end() {
        // `ü` is U+00FC, past every ASCII letter, so byte order gives `Zyklon, Zürich`.
        assert_eq!(sorted(&["Zyklon", "Zürich"]), ["Zürich", "Zyklon"]);
        // And it is not *ignored* either: it separates two otherwise equal terms.
        assert_eq!(sorted(&["résumé", "resume"]), ["resume", "résumé"]);
    }

    #[test]
    fn a_ligature_files_as_the_letters_it_stands_for() {
        assert_eq!(
            sorted(&["Ævar", "Adams", "Afton"]),
            ["Adams", "Ævar", "Afton"]
        );
        assert_eq!(sorted(&["Straße", "Strasse"]), ["Strasse", "Straße"]);
    }

    #[test]
    fn punctuation_and_spacing_do_not_separate() {
        // The comma an inverted entry carries must not decide anything: `Keep, the Ruined` files
        // as `keeptheruined`, after both of these. Byte order puts it *first* of the three,
        // because `,` is U+002C and sorts under every letter.
        assert_eq!(
            sorted(&["Keep, the Ruined", "Keeper", "Keepsakes"]),
            ["Keeper", "Keepsakes", "Keep, the Ruined"]
        );
        // Letter-by-letter filing, stated as a test rather than left implicit: the space in
        // `New York` does not separate, so it files as `newyork` — after `Newark`. Word-by-word
        // filing, the other convention, gives the opposite answer and is a named non-goal.
        assert_eq!(sorted(&["New York", "Newark"]), ["Newark", "New York"]);
        // A hyphen is ignored the same way, so the two spellings of one word file together and
        // separate only at the tie-break.
        assert_eq!(
            sorted(&["cooperate", "co-operate", "cop"]),
            ["co-operate", "cooperate", "cop"]
        );
    }

    #[test]
    fn digits_file_ahead_of_letters() {
        assert_eq!(
            sorted(&["armour", "3d6", "zephyr"]),
            ["3d6", "armour", "zephyr"]
        );
    }

    #[test]
    fn a_declared_leading_article_is_dropped_and_an_undeclared_one_is_not() {
        let english: Vec<String> = ["A", "An", "The"].iter().map(|s| s.to_string()).collect();
        assert_eq!(
            sorted_with(&["The Ruined Keep", "Ravens", "Sorcery"], &english),
            ["Ravens", "The Ruined Keep", "Sorcery"]
        );
        // Nothing is guessed: with no list, `The Ruined Keep` files under T, which is what a book
        // that never told quill its language must get.
        assert_eq!(
            sorted(&["The Ruined Keep", "Ravens", "Sorcery"]),
            ["Ravens", "Sorcery", "The Ruined Keep"]
        );
        // A word that merely starts with an article is not an article.
        assert_eq!(
            sorted_with(&["Theatre", "Thanes"], &english),
            ["Thanes", "Theatre"]
        );
        // The longest match wins: `An Atlas` must not file under `n`.
        assert_eq!(
            sorted_with(&["An Atlas", "Axe"], &english),
            ["An Atlas", "Axe"]
        );
    }

    #[test]
    fn a_french_book_states_its_own_articles() {
        // The mechanism is data, not English. This is the `CLAUDE.md` test applied to the rule: a
        // collation only one language can use would be the defect.
        let french: Vec<String> = ["Le", "La", "Les", "L'"]
            .iter()
            .map(|s| s.to_string())
            .collect();
        assert_eq!(
            sorted_with(&["Le Havre", "Gascogne", "Île-de-France"], &french),
            ["Gascogne", "Le Havre", "Île-de-France"]
        );
        assert_eq!(
            sorted_with(&["L'étoile", "Lyon"], &french),
            ["L'étoile", "Lyon"]
        );
    }

    #[test]
    fn an_entry_that_is_only_an_article_files_under_it() {
        // Stripping every character would leave an empty primary key, which files the entry ahead
        // of the whole index — a place no reader would look for it.
        let english: Vec<String> = vec!["The".to_string()];
        assert_eq!(sorted_with(&["The", "Thanes"], &english), ["Thanes", "The"]);
    }

    #[test]
    fn sort_as_overrides_the_derivation_entirely() {
        let mut de = IndexMark::new("de Gaulle, Charles");
        de.sort_as = Some("Gaulle, Charles de".into());
        let mut orwell = IndexMark::new("1984");
        orwell.sort_as = Some("Nineteen Eighty-Four".into());
        let mut v = [
            IndexMark::new("Franks"),
            de.clone(),
            IndexMark::new("Norsemen"),
            orwell.clone(),
        ];
        v.sort_by_key(|m| collation_key(m, &[]));
        let terms: Vec<&str> = v.iter().map(|m| m.term.as_str()).collect();
        assert_eq!(terms, ["Franks", "de Gaulle, Charles", "1984", "Norsemen"]);
    }

    #[test]
    fn the_order_is_total_so_collation_is_deterministic() {
        // Two terms that agree at every level above the tie-break still order, and order the same
        // way whichever order they arrive in. Without the tie-break a stable sort would preserve
        // the order the marks happened to be found in, and the index would depend on where in the
        // book each term was first mentioned.
        assert_ne!(key("apple"), key("Apple"));
        assert_eq!(sorted(&["Apple", "apple"]), sorted(&["apple", "Apple"]));
    }

    #[test]
    fn a_script_outside_the_fold_table_still_orders_alphabetically() {
        // Greek and Cyrillic blocks are laid out in alphabetical order, so folding nothing and
        // lower-casing is the right answer for them — the scope limit is *tailoring*, not script.
        assert_eq!(sorted(&["βῆτα", "άλφα"]), ["άλφα", "βῆτα"]);
        assert_eq!(sorted(&["Волга", "Ангара"]), ["Ангара", "Волга"]);
    }
}
