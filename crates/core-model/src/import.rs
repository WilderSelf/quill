//! The authoring on-ramp: a small line-oriented syntax that becomes a [`Document`] (spec 0043).
//!
//! Everything else in M2 made a good book *possible*. Nothing made it easy to type: the only
//! document-producing paths were `Document::sample()` and hand-written JSON.
//!
//! ## Deliberately a subset, not CommonMark
//!
//! Hand-rolled, with no markdown dependency. Two reasons, and the second is the real one:
//!
//! 1. The workspace's dependency rule is minimal-and-permissive, and this needs no library.
//! 2. **A subset can be enumerated.** "Markdown-ish" invites unbounded creep toward CommonMark, and
//!    an importer that half-supports nested lists and reference links is worse than one that
//!    supports eight constructs completely and says so. The supported set is the list below;
//!    everything else is an explicit non-goal.
//!
//! Round-tripping *out* to markdown is a non-goal.
//!
//! ## The syntax
//!
//! - `#` … `######` — a heading of that level.
//! - Blank-line-separated runs of text — a body paragraph. Newlines inside one are soft.
//! - `![](asset-id)` — an image block referencing a linked asset by id.
//! - `:::panel` … `:::` — a panel: a titled record of named sections, one `key: value` per line.
//!   `:::statblock` is the same fence under its pre-0062 name and still parses.
//! - `:::table` … `:::` — a table, as pipe-delimited rows; the first row is the header.
//! - `- item` / `* item` / `1. item` — a list item. The ordinal you write is ignored: markers are
//!   derived from document order at layout time, so an inserted item renumbers what follows.
//! - `:::toc` … `:::` — a generated table of contents.
//! - `**bold**` / `__bold__` and `*italic*` / `_italic_` inside a heading or a paragraph — a run set
//!   in the family's bold or italic face (spec 0064). They nest: `***both***` is bold italic, and
//!   `**bold with *italic* inside**` is what it reads as. An **unmatched** delimiter is literal
//!   text, which is the same rule the rest of this importer follows — an author who typed a
//!   lone asterisk gets a lone asterisk, not an error and not a swallowed line.
//!
//! ## What happens to input it does not understand
//!
//! Never silently dropped — that is the worst failure this feature can have, and the easy one to
//! write. Two rules, chosen by how much the author has told us:
//!
//! - An **unknown fence** (`:::foo`) is an **error**. The author clearly meant a structured object;
//!   guessing which, or discarding it, both lose real content.
//! - **Anything else** is kept as body text with a **warning** naming the line. A paragraph that
//!   came out as plain prose is visible and fixable; a paragraph that vanished is not.

use crate::{
    Block, BlockId, Color, Document, InlineStyle, Panel, Run, Table, Template, Weight,
    LIST_BULLET_STYLE, LIST_NUMBER_STYLE,
};

/// Something the importer could not honor, with the line it was on.
#[derive(Debug, Clone, PartialEq)]
pub struct Diagnostic {
    /// One-based, as an editor counts.
    pub line: usize,
    pub message: String,
}

/// The result of an import: a document, plus everything that was not understood.
#[derive(Debug, Clone)]
pub struct Imported {
    pub document: Document,
    /// Non-fatal. Every one names a line, and none of them means content was lost.
    pub warnings: Vec<Diagnostic>,
}

/// A fatal import problem.
#[derive(Debug, Clone, PartialEq)]
pub struct ImportError {
    pub line: usize,
    pub message: String,
}

impl std::fmt::Display for ImportError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "line {}: {}", self.line, self.message)
    }
}

impl std::error::Error for ImportError {}

/// The ink imported content is set in.
///
/// Grayscale black, not CMYK: it is press-legal in both PDF/X levels, it is what a beginner means
/// by "black", and a rich black would be a registration problem on press for no gain.
const INK: Color = Color::Gray { v: 0.0 };

/// Parse the on-ramp syntax into a document laid out to `template`.
pub fn import(source: &str, template: &Template) -> Result<Imported, ImportError> {
    let mut doc = Document::from_template(template);
    let mut warnings = Vec::new();
    let mut content: Vec<Block> = Vec::new();
    let mut paragraph: Vec<String> = Vec::new();
    let mut paragraph_line = 0usize;

    let lines: Vec<&str> = source.lines().collect();
    let mut i = 0;

    // A paragraph ends at a blank line or at anything that is not more prose.
    macro_rules! flush {
        () => {
            if !paragraph.is_empty() {
                content.push(Block::body_runs(emphasis(&paragraph.join(" ")), INK));
                paragraph.clear();
                let _ = paragraph_line;
            }
        };
    }

    while i < lines.len() {
        let raw = lines[i];
        let line_no = i + 1;
        let trimmed = raw.trim();

        if trimmed.is_empty() {
            flush!();
            i += 1;
            continue;
        }

        if let Some(rest) = trimmed.strip_prefix(":::") {
            flush!();
            let kind = rest.trim();
            let (body, end) = fence_body(&lines, i + 1);
            match kind {
                // `statblock` is `panel`'s pre-0062 spelling. Retained rather than deprecated:
                // it is a published authoring syntax and there is no released version to remove
                // it in, so silently refusing to parse it would be the failure this repo exists
                // to avoid (spec 0062).
                "panel" | "statblock" => content.push(parse_panel(&body, i + 2, &mut warnings)),
                "table" => content.push(parse_table(&body, i + 2, &mut warnings)?),
                "toc" => content.push(parse_toc(&body, i + 2, &mut warnings)),
                other => {
                    // Fatal on purpose: the author asked for a structured object we do not have.
                    return Err(ImportError {
                        line: line_no,
                        message: format!(
                            "unknown block `:::{other}`; supported: panel, table, toc"
                        ),
                    });
                }
            }
            i = end;
            continue;
        }

        if let Some(level) = heading_level(trimmed) {
            flush!();
            let text = trimmed[level as usize..].trim();
            if text.is_empty() {
                warnings.push(Diagnostic {
                    line: line_no,
                    message: "empty heading; kept as an empty heading rather than dropped".into(),
                });
            }
            content.push(Block::heading_runs(level, emphasis(text), INK));
            i += 1;
            continue;
        }

        if let Some(asset) = image_asset(trimmed) {
            flush!();
            content.push(Block::image(asset));
            i += 1;
            continue;
        }

        // Constructs the subset does not cover, kept verbatim so nothing is lost.
        if trimmed.starts_with('|') {
            warnings.push(Diagnostic {
                line: line_no,
                message: "a pipe row outside a `:::table` block is kept as text; wrap it in \
                          `:::table` to lay it out as a table"
                    .into(),
            });
        } else if let Some(item) = list_item(trimmed) {
            // A list item ends whatever paragraph was accumulating and becomes its own block, since
            // a list is a run of consecutive marked paragraphs rather than a container (spec 0066).
            flush!();
            content.push(Block::Body {
                id: BlockId::UNASSIGNED,
                runs: vec![Run::plain(item.text)],
                color: INK,
                style: Some(item.style.to_string()),
            });
            i += 1;
            continue;
        } else if trimmed.starts_with("> ") {
            warnings.push(Diagnostic {
                line: line_no,
                message: "block quotes are not supported; kept as body text".into(),
            });
        }

        if paragraph.is_empty() {
            paragraph_line = line_no;
        }
        paragraph.push(trimmed.to_string());
        i += 1;
    }
    flush!();

    doc.content = content;
    doc.assign_missing_block_ids()
        .expect("imported blocks are constructed unassigned, so cannot collide");
    Ok(Imported {
        document: doc,
        warnings,
    })
}

/// `#`-count for a heading line, or `None`.
/// Split authored text into runs at `**bold**` and `*italic*` delimiters (spec 0064).
///
/// Hand-rolled, for the reason the whole importer is hand-rolled: the supported set has to be
/// enumerable. It knows exactly two delimiters, each in two spellings, and it nests them.
///
/// **Two passes, because one cannot be correct.** A single left-to-right scan has to decide whether
/// a delimiter opens at the moment it sees it, and the only evidence available then is "some later
/// delimiter could close this" — which pairs the wrong two. In `2*d6 for a *big* hit` it pairs the
/// `*` in `2*d6` with the one before `big`, consuming both and emphasising nothing a reader asked
/// for. So the delimiters are collected first, then matched **closer to nearest opener**, which is
/// the rule that makes `*big*` the pair and leaves `2*d6` alone.
///
/// Three rules decide what may pair at all, and all three are ones a reader already expects:
///
/// - **An opener is followed by text and a closer is preceded by it.** That is what keeps
///   `2 * 3 * 4` arithmetic.
/// - **`_` does not work inside a word.** `snake_case_word` is an identifier and `café_x` is a
///   word. `*` still does, so `un*frozen*` remains available.
/// - **A pair is the same character.** `_a*` opens nothing: `*` cannot close what `_` opened.
///
/// Anything left unmatched is literal text — not an error, and never silently dropped, which is the
/// posture the rest of this importer takes toward input it cannot honour.
fn emphasis(text: &str) -> Vec<Run> {
    let delims = scan_delimiters(text);
    let pairs = match_delimiters(&delims);
    build_runs(text, &delims, &pairs)
}

/// A maximal run of one delimiter character, with whether it may open and may close.
#[derive(Debug, Clone, Copy)]
struct Delim {
    at: usize,
    len: usize,
    ch: char,
    can_open: bool,
    can_close: bool,
}

fn scan_delimiters(text: &str) -> Vec<Delim> {
    let chars: Vec<(usize, char)> = text.char_indices().collect();
    let mut out = Vec::new();
    let mut i = 0usize;
    while i < chars.len() {
        let (at, ch) = chars[i];
        if ch != '*' && ch != '_' {
            i += 1;
            continue;
        }
        let mut j = i;
        while j < chars.len() && chars[j].1 == ch {
            j += 1;
        }
        let before = i.checked_sub(1).map(|k| chars[k].1);
        let after = chars.get(j).map(|(_, c)| *c);
        // Flanking: an opener needs text after it, a closer needs text before it. `_` additionally
        // refuses to work inside a word, which is what protects identifiers and `café_x`.
        let open_flank = after.is_some_and(|c| !c.is_whitespace());
        let close_flank = before.is_some_and(|c| !c.is_whitespace());
        let intraword_ok = |side: Option<char>| !side.is_some_and(char::is_alphanumeric);
        out.push(Delim {
            at,
            len: j - i,
            ch,
            can_open: open_flank && (ch == '*' || intraword_ok(before)),
            can_close: close_flank && (ch == '*' || intraword_ok(after)),
        });
        i = j;
    }
    out
}

/// What a matched pair does: `(opener index, closer index, characters used)`.
type Pair = (usize, usize, usize);

/// Match closers to the nearest compatible opener, consuming one or two characters per pair.
fn match_delimiters(delims: &[Delim]) -> Vec<Pair> {
    let mut left: Vec<usize> = delims.iter().map(|d| d.len).collect();
    let mut stack: Vec<usize> = Vec::new();
    let mut pairs: Vec<Pair> = Vec::new();
    for (i, d) in delims.iter().enumerate() {
        if d.can_close {
            while left[i] > 0 {
                let Some(k) = stack
                    .iter()
                    .rposition(|&j| delims[j].ch == d.ch && left[j] > 0)
                else {
                    break;
                };
                let j = stack[k];
                // Two characters make a strong pair, one an emphasis pair — the greedy choice, so
                // `***x***` is bold italic rather than three nested emphases.
                let used = left[i].min(left[j]).min(2);
                left[i] -= used;
                left[j] -= used;
                pairs.push((j, i, used));
                // Everything opened after the opener we just matched can no longer be closed: its
                // closer would have to cross this pair.
                stack.truncate(k + if left[j] > 0 { 1 } else { 0 });
            }
        }
        if d.can_open && left[i] > 0 {
            stack.push(i);
        }
    }
    pairs
}

/// Walk the text, applying the matched pairs, and emit one run per stretch of constant emphasis.
///
/// A delimiter run can be partly consumed — `**a*` matches one character and leaves one — so each
/// run is split into "characters used as a closer", "characters nobody matched", and "characters
/// used as an opener", in that order. The unmatched middle is literal text, which is what keeps a
/// stray asterisk on the page.
fn build_runs(text: &str, delims: &[Delim], pairs: &[Pair]) -> Vec<Run> {
    let mut consumed: std::collections::BTreeMap<usize, (i32, i32)> =
        std::collections::BTreeMap::new();
    for &(o, c, used) in pairs {
        consumed.entry(delims[o].at).or_insert((0, 0)).0 += used as i32;
        consumed.entry(delims[c].at).or_insert((0, 0)).1 += used as i32;
    }
    let by_offset: std::collections::BTreeMap<usize, Delim> =
        delims.iter().map(|d| (d.at, *d)).collect();

    let mut runs: Vec<Run> = Vec::new();
    let mut buf = String::new();
    let mut bold = 0i32;
    let mut italic = 0i32;
    let flush = |buf: &mut String, bold: i32, italic: i32, runs: &mut Vec<Run>| {
        if buf.is_empty() {
            return;
        }
        runs.push(Run {
            text: std::mem::take(buf),
            style: InlineStyle {
                weight: (bold > 0).then_some(Weight::BOLD),
                italic: (italic > 0).then_some(true),
                ..InlineStyle::EMPTY
            },
        });
    };

    let mut i = 0usize;
    while i < text.len() {
        if let Some(d) = by_offset.get(&i) {
            let (opened, closed) = consumed.get(&i).copied().unwrap_or((0, 0));
            let literal = d.len as i32 - opened - closed;
            if closed > 0 {
                flush(&mut buf, bold, italic, &mut runs);
                apply(&mut bold, &mut italic, closed, -1);
            }
            for _ in 0..literal.max(0) {
                buf.push(d.ch);
            }
            if opened > 0 {
                flush(&mut buf, bold, italic, &mut runs);
                apply(&mut bold, &mut italic, opened, 1);
            }
            i += d.len * d.ch.len_utf8();
            continue;
        }
        let ch = text[i..].chars().next().expect("byte index is a boundary");
        buf.push(ch);
        i += ch.len_utf8();
    }
    flush(&mut buf, bold, italic, &mut runs);
    if runs.is_empty() {
        runs.push(Run::plain(text));
    }
    runs
}

/// Turn `n` consumed characters into emphasis levels: two make a strong, one an emphasis.
fn apply(bold: &mut i32, italic: &mut i32, consumed: i32, sign: i32) {
    let mut left = consumed;
    while left >= 2 {
        *bold += sign;
        left -= 2;
    }
    if left == 1 {
        *italic += sign;
    }
}

fn heading_level(line: &str) -> Option<u8> {
    let hashes = line.chars().take_while(|c| *c == '#').count();
    // A `#` with no space after it is a word, not a heading — `#1` is how people write numbers.
    if (1..=6).contains(&hashes) && line[hashes..].starts_with(' ') {
        Some(hashes as u8)
    } else {
        None
    }
}

/// The asset id in `![](id)` or `![alt](id)`.
fn image_asset(line: &str) -> Option<String> {
    let rest = line.strip_prefix("![")?;
    let close = rest.find("](")?;
    let id = rest[close + 2..].strip_suffix(')')?;
    (!id.is_empty()).then(|| id.to_string())
}

/// Lines up to the closing `:::`, and the index just past it.
///
/// An unterminated fence runs to the end of the file rather than failing: the content is all there,
/// and refusing the whole document over a missing three characters is the wrong trade for an
/// authoring tool.
fn fence_body(lines: &[&str], start: usize) -> (Vec<String>, usize) {
    let mut body = Vec::new();
    let mut i = start;
    while i < lines.len() {
        if lines[i].trim() == ":::" {
            return (body, i + 1);
        }
        body.push(lines[i].to_string());
        i += 1;
    }
    (body, i)
}

/// Split `key: value`, trimming both.
fn key_value(line: &str) -> Option<(&str, &str)> {
    let (k, v) = line.split_once(':')?;
    Some((k.trim(), v.trim()))
}

/// A markdown list item: `- text`, `* text`, or `1. text` (spec 0066).
///
/// Ordinals in the source are *not* honoured — the marker a reader sees is derived from document
/// order at layout time, for the reason spec 0041 records: anything counted while placing goes
/// missing on a page an incremental pass reuses. An author who writes `1.` three times still gets
/// `1. 2. 3.`, which is what every markdown implementation does and what they meant.
struct ListItem<'a> {
    text: &'a str,
    style: &'static str,
}

fn list_item(line: &str) -> Option<ListItem<'_>> {
    if let Some(rest) = line.strip_prefix("- ").or_else(|| line.strip_prefix("* ")) {
        return Some(ListItem {
            text: rest.trim(),
            style: LIST_BULLET_STYLE,
        });
    }
    // `12. text` — digits, a dot, a space.
    let digits = line.len() - line.trim_start_matches(|c: char| c.is_ascii_digit()).len();
    if digits > 0 {
        if let Some(rest) = line[digits..].strip_prefix(". ") {
            return Some(ListItem {
                text: rest.trim(),
                style: LIST_NUMBER_STYLE,
            });
        }
    }
    None
}

fn parse_panel(body: &[String], first_line: usize, warnings: &mut Vec<Diagnostic>) -> Block {
    let mut panel = Panel::default();
    for (offset, line) in body.iter().enumerate() {
        let line_no = first_line + offset;
        let t = line.trim();
        if t.is_empty() {
            continue;
        }
        let Some((key, value)) = key_value(t) else {
            warnings.push(Diagnostic {
                line: line_no,
                message: "expected `key: value` in a panel; kept as an overview line".into(),
            });
            panel.overview.push(t.to_string());
            continue;
        };
        match key {
            "name" => panel.name = value.to_string(),
            "overview" => panel.overview.push(value.to_string()),
            "detail" | "details" => panel.details.push(value.to_string()),
            "action" | "actions" => panel.actions.push(value.to_string()),
            "reaction" | "reactions" => panel.reactions.push(value.to_string()),
            "attr" => match value.split_once('=') {
                Some((k, v)) => panel.attributes.push((k.trim().into(), v.trim().into())),
                None => warnings.push(Diagnostic {
                    line: line_no,
                    message: "an `attr` needs `name = value`; skipped".into(),
                }),
            },
            other => {
                warnings.push(Diagnostic {
                    line: line_no,
                    message: format!("unknown panel field `{other}`; kept as a detail"),
                });
                panel.details.push(t.to_string());
            }
        }
    }
    if panel.name.is_empty() {
        warnings.push(Diagnostic {
            line: first_line,
            message: "panel has no `name:`".into(),
        });
    }
    Block::Panel {
        id: BlockId::UNASSIGNED,
        panel,
        color: INK,
    }
}

fn parse_table(
    body: &[String],
    first_line: usize,
    warnings: &mut Vec<Diagnostic>,
) -> Result<Block, ImportError> {
    let mut rows: Vec<Vec<String>> = Vec::new();
    for (offset, line) in body.iter().enumerate() {
        let t = line.trim();
        if t.is_empty() {
            continue;
        }
        if !t.starts_with('|') {
            warnings.push(Diagnostic {
                line: first_line + offset,
                message: "a table row must start with `|`; skipped".into(),
            });
            continue;
        }
        let cells: Vec<String> = t
            .trim_matches('|')
            .split('|')
            .map(|c| c.trim().to_string())
            .collect();
        // A separator row (`|---|---|`) is markdown furniture, not data.
        if cells
            .iter()
            .all(|c| !c.is_empty() && c.chars().all(|ch| ch == '-' || ch == ':'))
        {
            continue;
        }
        rows.push(cells);
    }
    if rows.is_empty() {
        return Err(ImportError {
            line: first_line,
            message: "a `:::table` block has no rows".into(),
        });
    }
    let header = rows.remove(0);
    let columns = vec![1.0; header.len()];
    Ok(Block::Table {
        id: BlockId::UNASSIGNED,
        table: Table {
            columns,
            header: Some(header),
            rows,
            zebra: true,
        },
        color: INK,
    })
}

fn parse_toc(body: &[String], first_line: usize, warnings: &mut Vec<Diagnostic>) -> Block {
    let mut title = "Contents".to_string();
    let mut max_level = 2u8;
    for (offset, line) in body.iter().enumerate() {
        let t = line.trim();
        if t.is_empty() {
            continue;
        }
        match key_value(t) {
            Some(("title", v)) => title = v.to_string(),
            Some(("max_level", v)) => match v.parse::<u8>() {
                Ok(n) if (1..=6).contains(&n) => max_level = n,
                _ => warnings.push(Diagnostic {
                    line: first_line + offset,
                    message: format!("`max_level` must be 1-6, got `{v}`; using {max_level}"),
                }),
            },
            _ => warnings.push(Diagnostic {
                line: first_line + offset,
                message: "a `:::toc` block takes `title:` and `max_level:` only; ignored".into(),
            }),
        }
    }
    Block::Toc {
        id: BlockId::UNASSIGNED,
        title,
        max_level,
        color: INK,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn tpl() -> &'static Template {
        Template::by_name("digest").expect("bundled")
    }

    fn import_ok(src: &str) -> Imported {
        import(src, tpl()).expect("import")
    }

    #[test]
    fn every_supported_construct_round_trips_to_the_blocks_it_names() {
        // Block by block, not by count: a count would pass while every block was the wrong kind.
        let src = "\
# The Ruined Keep

A dank corridor stretches
into darkness.

![a map](map1)

:::statblock
name: Goblin
overview: Small humanoid, chaotic evil
attr: Armour Class = 15
detail: Nimble Escape.
action: Scimitar. +4 to hit.
reaction: Warren Cunning.
:::

:::table
| d20 | Encounter |
|-----|-----------|
| 1-4 | Nothing   |
:::

:::toc
title: What Is Inside
max_level: 3
:::
";
        let out = import_ok(src);
        assert!(out.warnings.is_empty(), "unexpected: {:?}", out.warnings);
        let c = &out.document.content;
        assert_eq!(c.len(), 6);

        match &c[0] {
            b @ Block::Heading { level, .. } => {
                assert_eq!(
                    (*level, b.plain_text().unwrap()),
                    (1, "The Ruined Keep".to_string())
                )
            }
            other => panic!("{other:?}"),
        }
        match &c[1] {
            // Soft newlines inside a paragraph become spaces.
            b @ Block::Body { .. } => {
                let text = b.plain_text().unwrap();
                assert_eq!(text, "A dank corridor stretches into darkness.")
            }
            other => panic!("{other:?}"),
        }
        match &c[2] {
            Block::Image { asset, .. } => assert_eq!(asset, "map1"),
            other => panic!("{other:?}"),
        }
        match &c[3] {
            Block::Panel { panel, .. } => {
                assert_eq!(panel.name, "Goblin");
                assert_eq!(panel.overview, ["Small humanoid, chaotic evil"]);
                assert_eq!(panel.attributes, [("Armour Class".into(), "15".into())]);
                assert_eq!(panel.details, ["Nimble Escape."]);
                assert_eq!(panel.actions, ["Scimitar. +4 to hit."]);
                assert_eq!(panel.reactions, ["Warren Cunning."]);
            }
            other => panic!("{other:?}"),
        }
        match &c[4] {
            Block::Table { table, .. } => {
                assert_eq!(
                    table.header.as_deref(),
                    Some(&["d20".to_string(), "Encounter".to_string()][..])
                );
                assert_eq!(table.rows, [["1-4".to_string(), "Nothing".to_string()]]);
            }
            other => panic!("{other:?}"),
        }
        match &c[5] {
            Block::Toc {
                title, max_level, ..
            } => assert_eq!((title.as_str(), *max_level), ("What Is Inside", 3)),
            other => panic!("{other:?}"),
        }
    }

    #[test]
    fn the_template_supplies_everything_the_source_does_not() {
        let out = import_ok("# Hi\n");
        assert_eq!(out.document.page_setup, tpl().page_setup);
        assert_eq!(out.document.master_pages, tpl().master_pages);
        assert_eq!(out.document.styles, tpl().styles);
        assert!(out.document.fonts_embeddable);
    }

    #[test]
    fn an_unknown_fence_is_fatal_rather_than_dropped() {
        // The author clearly meant a structured object. Guessing which, or discarding it, both lose
        // real content — so this is the one case that refuses.
        let err = import(":::spellcard\nname: Fireball\n:::\n", tpl()).unwrap_err();
        assert_eq!(err.line, 1);
        assert!(err.message.contains("spellcard"), "{}", err.message);
        assert!(err.message.contains("panel"), "must list what is supported");
    }

    #[test]
    fn unsupported_inline_syntax_warns_with_a_line_number_and_keeps_the_text() {
        // The worst failure this feature could have is a paragraph that silently vanishes. Lists
        // used to be in this list and are now imported (spec 0066), so the remaining two are the
        // constructs the importer still does not understand.
        let out = import_ok("intro\n\n> a quote\n\n| stray | row |\n");
        assert_eq!(out.warnings.len(), 2, "{:?}", out.warnings);
        assert_eq!(out.warnings[0].line, 3);
        assert_eq!(out.warnings[1].line, 5);

        let kept: Vec<String> = out
            .document
            .content
            .iter()
            .filter_map(|b| match b {
                b @ Block::Body { .. } => b.plain_text(),
                _ => None,
            })
            .collect();
        assert_eq!(kept, ["intro", "> a quote", "| stray | row |"]);
    }

    #[test]
    fn markdown_lists_import_as_marked_paragraphs() {
        let out = import_ok("intro\n\n- first\n- second\n\n1. one\n7. two\n");
        assert!(out.warnings.is_empty(), "{:?}", out.warnings);
        let items: Vec<(Option<&str>, String)> = out
            .document
            .content
            .iter()
            .filter_map(|b| match b {
                Block::Body { style, .. } => Some((style.as_deref(), b.plain_text()?)),
                _ => None,
            })
            .collect();
        assert_eq!(
            items,
            vec![
                (None, "intro".to_string()),
                (Some(LIST_BULLET_STYLE), "first".to_string()),
                (Some(LIST_BULLET_STYLE), "second".to_string()),
                (Some(LIST_NUMBER_STYLE), "one".to_string()),
                (Some(LIST_NUMBER_STYLE), "two".to_string()),
            ],
            "the marker character is consumed; `7.` is still the second item, because the ordinal \
             a reader sees is derived at layout time and not copied from the source"
        );
    }

    #[test]
    fn the_pre_0062_fence_spelling_parses_identically() {
        // `:::statblock` is `:::panel`'s old name. A published authoring syntax that silently stops
        // parsing is the failure this repo exists to avoid, so both spellings are permanent and
        // must produce the same document rather than merely both succeeding.
        let src = "name: Astrolabe\noverview: A brass instrument.\nattr: Diameter = 180 mm\n";
        let new_spelling = import_ok(&format!(":::panel\n{src}:::\n"));
        let old_spelling = import_ok(&format!(":::statblock\n{src}:::\n"));
        assert_eq!(new_spelling.document, old_spelling.document);
        assert_eq!(
            new_spelling.warnings.len(),
            0,
            "{:?}",
            new_spelling.warnings
        );
        let Block::Panel { panel, .. } = &new_spelling.document.content[0] else {
            panic!("expected a panel")
        };
        assert_eq!(panel.name, "Astrolabe");
    }

    #[test]
    fn a_malformed_stat_block_field_warns_and_keeps_the_line() {
        let out =
            import_ok(":::statblock\nname: Goblin\nattr: no equals sign\nwibble: value\n:::\n");
        assert_eq!(out.warnings.len(), 2, "{:?}", out.warnings);
        assert_eq!(out.warnings[0].line, 3);
        assert!(out.warnings[0].message.contains("name = value"));
        assert_eq!(out.warnings[1].line, 4);
        let Block::Panel { panel, .. } = &out.document.content[0] else {
            panic!("expected a stat block")
        };
        assert_eq!(
            panel.details,
            ["wibble: value"],
            "the unknown field is kept"
        );
    }

    #[test]
    fn a_table_with_no_rows_is_fatal() {
        let err = import(":::table\n:::\n", tpl()).unwrap_err();
        assert_eq!(err.line, 2);
    }

    #[test]
    fn a_hash_without_a_space_is_a_word_not_a_heading() {
        // `#1` is how people write numbers, and turning it into a heading would be worse than
        // ignoring it.
        let out = import_ok("#1 on the list\n");
        assert!(matches!(out.document.content[0], Block::Body { .. }));
    }

    #[test]
    fn an_unterminated_fence_still_yields_its_content() {
        // Refusing a whole document over three missing characters is the wrong trade when every
        // line the author typed is right there.
        let out = import_ok(":::statblock\nname: Goblin\n");
        let Block::Panel { panel, .. } = &out.document.content[0] else {
            panic!("expected a stat block")
        };
        assert_eq!(panel.name, "Goblin");
    }

    #[test]
    fn the_documented_example_parses() {
        // The anti-drift guard the repo already uses for the manifest schema (spec 0030): the
        // example in `docs/format-spec.md` is *the* example a user copies, so it is parsed here
        // rather than trusted. Extracted from the doc itself, not duplicated — a copy would drift.
        const DOC: &str = include_str!("../../../docs/format-spec.md");
        let example = DOC
            .split("```quill-import")
            .nth(1)
            .and_then(|rest| rest.split("```").next())
            .expect("docs/format-spec.md must carry a ```quill-import example");

        let out = import(example, tpl()).expect("the documented example must import");
        assert!(
            out.warnings.is_empty(),
            "the documented example must not warn: {:?}",
            out.warnings
        );
        // And it must exercise every construct it claims to show.
        let kinds: Vec<&str> = out
            .document
            .content
            .iter()
            .map(|b| match b {
                Block::Heading { .. } => "heading",
                Block::Body { .. } => "body",
                Block::Image { .. } => "image",
                Block::Panel { .. } => "panel",
                Block::Table { .. } => "table",
                Block::Component { .. } => "component",
                Block::Toc { .. } => "toc",
            })
            .collect();
        assert_eq!(kinds, ["heading", "body", "image", "panel", "table", "toc"]);
    }

    #[test]
    fn an_empty_source_imports_to_an_empty_document() {
        let out = import_ok("");
        assert!(out.document.content.is_empty());
        assert!(out.warnings.is_empty());
    }

    #[test]
    fn a_large_source_imports_to_the_expected_block_count() {
        // The importer must not be the new bottleneck, and must not lose a paragraph at scale.
        let src: String = (0..5_000)
            .map(|i| format!("paragraph number {i}\n\n"))
            .collect();
        let out = import_ok(&src);
        assert_eq!(out.document.content.len(), 5_000);
    }

    /// Spec 0064: `**bold**` and `*italic*`, the two constructs the importer refused for want of a
    /// face to import them into.
    #[test]
    fn emphasis_becomes_runs() {
        let doc = import("a **bold** word and an *italic* one", tpl())
            .expect("imports")
            .document;
        let Block::Body { runs, .. } = &doc.content[0] else {
            panic!("expected a body paragraph")
        };
        let texts: Vec<&str> = runs.iter().map(|r| r.text.as_str()).collect();
        assert_eq!(texts, ["a ", "bold", " word and an ", "italic", " one"]);
        assert_eq!(runs[1].style.weight, Some(Weight::BOLD));
        assert_eq!(runs[1].style.italic, None);
        assert_eq!(runs[3].style.italic, Some(true));
        assert_eq!(runs[3].style.weight, None);
    }

    #[test]
    fn emphasis_nests() {
        let doc = import("***both*** and **bold with *italic* inside**", tpl())
            .expect("imports")
            .document;
        let Block::Body { runs, .. } = &doc.content[0] else {
            panic!("expected a body paragraph")
        };
        let both = &runs[0];
        assert_eq!(both.text, "both");
        assert_eq!(both.style.weight, Some(Weight::BOLD));
        assert_eq!(both.style.italic, Some(true));
        let inner = runs
            .iter()
            .find(|r| r.text == "italic")
            .expect("the nested run survives");
        assert_eq!(inner.style.weight, Some(Weight::BOLD));
        assert_eq!(inner.style.italic, Some(true));
    }

    #[test]
    fn an_unmatched_delimiter_is_literal_text() {
        // The rule the rest of this importer follows: keep what the author typed. `2 * 3 * 4` is
        // arithmetic, and a paragraph that opens `**bold` and never closes it is a paragraph about
        // asterisks — not an error, and not a swallowed line.
        for source in [
            "**bold and no close",
            "a lone * asterisk",
            "snake_case_word",
            "2 * 3 * 4 is arithmetic",
            "a file_name_with_underscores and another_one",
        ] {
            let doc = import(source, tpl()).expect("imports").document;
            let Block::Body { runs, .. } = &doc.content[0] else {
                panic!("expected a body paragraph")
            };
            let joined: String = runs.iter().map(|r| r.text.as_str()).collect();
            assert_eq!(joined, source, "text was altered");
            assert!(
                runs.iter().all(|r| r.style.is_empty()),
                "{source:?} should carry no emphasis"
            );
        }
    }

    #[test]
    fn a_heading_takes_emphasis_too() {
        let doc = import("# a *slanted* title", tpl())
            .expect("imports")
            .document;
        let Block::Heading { runs, .. } = &doc.content[0] else {
            panic!("expected a heading")
        };
        assert_eq!(runs.len(), 3);
        assert_eq!(runs[1].style.italic, Some(true));
    }

    #[test]
    fn text_with_no_emphasis_is_still_exactly_one_run() {
        // The parser runs over every paragraph, so the shape of an unemphasised one must not move:
        // a second run would change every span map in the workspace for no reason.
        let doc = import("just some ordinary prose", tpl())
            .expect("imports")
            .document;
        let Block::Body { runs, .. } = &doc.content[0] else {
            panic!("expected a body paragraph")
        };
        assert_eq!(runs.len(), 1);
        assert!(runs[0].style.is_empty());
    }

    /// Every one of these was measured losing *ordinary* characters under the first, one-pass
    /// version of the scanner, which decided that a delimiter opened as soon as any later delimiter
    /// could close it — so `2*3` inside a paragraph became `23`.
    ///
    /// A matched delimiter is legitimately consumed; nothing else may be. The check is therefore on
    /// the text with its delimiters removed, which is exactly "did any real character vanish".
    #[test]
    fn no_input_loses_a_character_to_the_emphasis_scanner() {
        let cases = [
            "the _damage die_ is 2*d6 for a *big* hit",
            "**bold with a 2*3 inside**",
            "_multiply 2*3 here_",
            "_*a*_",
            "*_a_*",
            "a * b * c",
            "***",
            "****",
            "**a*",
            "*a**",
            "_a*b_",
            "café_x_ and cafe_x_",
            "2*3*4",
            "a**b**c",
        ];
        for source in cases {
            let doc = import(source, tpl()).expect("imports").document;
            let Block::Body { runs, .. } = &doc.content[0] else {
                panic!("expected a body paragraph for {source:?}")
            };
            let joined: String = runs.iter().map(|r| r.text.as_str()).collect();
            let strip = |s: &str| s.replace(['*', '_'], "");
            assert_eq!(
                strip(&joined),
                strip(source),
                "a real character vanished from {source:?} (got {joined:?})"
            );
        }
    }

    /// The pairing rule, stated as outcomes rather than as an algorithm: a closer takes the
    /// *nearest* opener, which is what leaves an unrelated earlier delimiter alone.
    #[test]
    fn a_closer_pairs_with_the_nearest_opener() {
        let doc = import("the 2*d6 for a *big* hit", tpl())
            .expect("imports")
            .document;
        let Block::Body { runs, .. } = &doc.content[0] else {
            panic!("expected a body paragraph")
        };
        let emphasised: Vec<&str> = runs
            .iter()
            .filter(|r| r.style.italic == Some(true))
            .map(|r| r.text.as_str())
            .collect();
        assert_eq!(emphasised, ["big"], "the wrong pair was matched");
        let joined: String = runs.iter().map(|r| r.text.as_str()).collect();
        assert_eq!(
            joined, "the 2*d6 for a big hit",
            "only the pair is consumed"
        );
    }

    #[test]
    fn one_delimiter_cannot_close_another_kind() {
        // `*` must not close what `_` opened, or `_a*` would emphasise and eat both. (`_a* b_`
        // *does* emphasise, and correctly: the two underscores are the pair, and the `*` inside is
        // ordinary text.)
        let doc = import("_a* b", tpl()).expect("imports").document;
        let Block::Body { runs, .. } = &doc.content[0] else {
            panic!("expected a body paragraph")
        };
        assert!(runs.iter().all(|r| r.style.is_empty()));
        let joined: String = runs.iter().map(|r| r.text.as_str()).collect();
        assert_eq!(joined, "_a* b");
    }
}
