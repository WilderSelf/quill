//! Reading a content stream back, for tests only (spec 0068).
//!
//! The acceptance criterion this exists for is that the width the PDF *draws* equals the width
//! layout *measured*. That can only be honestly asserted against the bytes the writer emitted —
//! re-deriving it from the shaper would assert the shaper against itself and would pass even if the
//! writer emitted nothing at all. So the operators are parsed back out.

/// One thing a content stream showed: a run of subset glyph ids, or a `TJ` adjustment.
#[derive(Debug, PartialEq)]
pub enum Shown {
    Glyphs(Vec<u16>),
    Adjust(f32),
}

/// A token of a PDF content stream, to the depth these tests need.
#[derive(Debug, PartialEq, Clone)]
enum Tok {
    Str(Vec<u8>),
    Num(f32),
    Op(String),
    Open,
    Close,
}

/// Tokenize a content stream. Literal strings carry the writer's own escaping (`\\n`, `\\(`,
/// `\\ddd`), and a string with any non-ASCII byte is written as hex instead — both forms appear
/// in real output because a subset glyph id is two arbitrary bytes.
fn tokenize(bytes: &[u8]) -> Vec<Tok> {
    let mut out = Vec::new();
    let mut i = 0;
    while i < bytes.len() {
        match bytes[i] {
            b'(' => {
                let mut s = Vec::new();
                let mut depth = 1;
                i += 1;
                while i < bytes.len() {
                    match bytes[i] {
                        b'\\' => {
                            i += 1;
                            let c = bytes[i];
                            match c {
                                b'n' => s.push(b'\n'),
                                b'r' => s.push(b'\r'),
                                b't' => s.push(b'\t'),
                                b'b' => s.push(0x08),
                                b'f' => s.push(0x0c),
                                b'0'..=b'7' => {
                                    let mut v = 0u32;
                                    let mut n = 0;
                                    while n < 3
                                        && i < bytes.len()
                                        && (b'0'..=b'7').contains(&bytes[i])
                                    {
                                        v = v * 8 + (bytes[i] - b'0') as u32;
                                        i += 1;
                                        n += 1;
                                    }
                                    i -= 1;
                                    s.push(v as u8);
                                }
                                other => s.push(other),
                            }
                            i += 1;
                        }
                        b'(' => {
                            depth += 1;
                            s.push(b'(');
                            i += 1;
                        }
                        b')' => {
                            depth -= 1;
                            i += 1;
                            if depth == 0 {
                                break;
                            }
                            s.push(b')');
                        }
                        c => {
                            s.push(c);
                            i += 1;
                        }
                    }
                }
                out.push(Tok::Str(s));
            }
            b'<' => {
                let mut s = Vec::new();
                let mut hi: Option<u8> = None;
                i += 1;
                while i < bytes.len() && bytes[i] != b'>' {
                    if let Some(d) = (bytes[i] as char).to_digit(16) {
                        match hi.take() {
                            None => hi = Some(d as u8),
                            Some(h) => s.push(h * 16 + d as u8),
                        }
                    }
                    i += 1;
                }
                if let Some(h) = hi {
                    s.push(h * 16);
                }
                i += 1;
                out.push(Tok::Str(s));
            }
            b'[' => {
                out.push(Tok::Open);
                i += 1;
            }
            b']' => {
                out.push(Tok::Close);
                i += 1;
            }
            b'/' => {
                i += 1;
                while i < bytes.len()
                    && !bytes[i].is_ascii_whitespace()
                    && !b"[]()<>/".contains(&bytes[i])
                {
                    i += 1;
                }
            }
            c if c.is_ascii_whitespace() => i += 1,
            c if c == b'-' || c == b'+' || c == b'.' || c.is_ascii_digit() => {
                let start = i;
                i += 1;
                while i < bytes.len()
                    && (bytes[i].is_ascii_digit() || bytes[i] == b'.' || bytes[i] == b'-')
                {
                    i += 1;
                }
                let text = String::from_utf8_lossy(&bytes[start..i]).into_owned();
                out.push(Tok::Num(text.parse().unwrap_or(0.0)));
            }
            _ => {
                let start = i;
                while i < bytes.len()
                    && !bytes[i].is_ascii_whitespace()
                    && !b"[]()<>/".contains(&bytes[i])
                {
                    i += 1;
                }
                out.push(Tok::Op(
                    String::from_utf8_lossy(&bytes[start..i]).into_owned(),
                ));
            }
        }
    }
    out
}

/// Everything the stream actually drew, in order: the glyphs of each `Tj`/`TJ` show and the
/// adjustments written beside them. Read back out of the emitted bytes rather than re-derived
/// from the shaper — asserting the shaper against itself would prove nothing.
pub fn shown_items(content: &[u8]) -> Vec<Shown> {
    let toks = tokenize(content);
    let gids = |s: &[u8]| -> Vec<u16> {
        s.chunks(2)
            .map(|c| u16::from_be_bytes([c[0], *c.get(1).unwrap_or(&0)]))
            .collect()
    };
    let mut out = Vec::new();
    let mut i = 0;
    while i < toks.len() {
        match &toks[i] {
            Tok::Open => {
                let mut items = Vec::new();
                i += 1;
                while i < toks.len() && toks[i] != Tok::Close {
                    match &toks[i] {
                        Tok::Str(s) => items.push(Shown::Glyphs(gids(s))),
                        Tok::Num(n) => items.push(Shown::Adjust(*n)),
                        _ => {}
                    }
                    i += 1;
                }
                i += 1; // ]
                if matches!(toks.get(i), Some(Tok::Op(op)) if op == "TJ") {
                    out.extend(items);
                    i += 1;
                }
            }
            Tok::Str(s) => {
                if matches!(toks.get(i + 1), Some(Tok::Op(op)) if op == "Tj") {
                    out.push(Shown::Glyphs(gids(s)));
                    i += 2;
                } else {
                    i += 1;
                }
            }
            _ => i += 1,
        }
    }
    out
}

/// Every `stream … endstream` payload in a finished PDF, in file order.
///
/// Crude by design: the only alternative is a PDF parser, and a test that needed one to read what
/// the writer wrote would be testing the parser.
pub fn streams(pdf: &[u8]) -> Vec<Vec<u8>> {
    let mut out = Vec::new();
    let mut i = 0;
    while let Some(rel) = find(&pdf[i..], b"stream") {
        let mut start = i + rel + b"stream".len();
        if pdf[..i + rel].ends_with(b"end") {
            i = start;
            continue;
        }
        while start < pdf.len() && (pdf[start] == b'\r' || pdf[start] == b'\n') {
            start += 1;
        }
        let Some(end) = find(&pdf[start..], b"endstream") else {
            break;
        };
        out.push(pdf[start..start + end].to_vec());
        i = start + end;
    }
    out
}

/// The streams that are page content — the ones carrying text operators.
pub fn content_streams(pdf: &[u8]) -> Vec<Vec<u8>> {
    streams(pdf)
        .into_iter()
        .filter(|s| {
            find(s, b"BT").is_some() && (find(s, b" Tj").is_some() || find(s, b" TJ").is_some())
        })
        .collect()
}

/// The `/ToUnicode` CMap, read back as subset glyph id → the characters it stands for.
///
/// Parses the `beginbfchar … endbfchar` blocks of every CMap in the file. A document set in one
/// face has one, which is what the round-trip test uses; a multi-face document's maps are merged,
/// and the test that cares asserts on a single-face export.
pub fn to_unicode_map(pdf: &[u8]) -> std::collections::BTreeMap<u16, String> {
    let mut out = std::collections::BTreeMap::new();
    for stream in streams(pdf) {
        let mut at = 0;
        while let Some(rel) = find(&stream[at..], b"beginbfchar") {
            let start = at + rel + b"beginbfchar".len();
            let Some(end) = find(&stream[start..], b"endbfchar") else {
                break;
            };
            for tokens in tokenize(&stream[start..start + end]).chunks(2) {
                let [Tok::Str(code), Tok::Str(dest)] = tokens else {
                    continue;
                };
                if code.len() != 2 {
                    continue;
                }
                let gid = u16::from_be_bytes([code[0], code[1]]);
                let utf16: Vec<u16> = dest
                    .chunks(2)
                    .map(|c| u16::from_be_bytes([c[0], *c.get(1).unwrap_or(&0)]))
                    .collect();
                if let Ok(text) = String::from_utf16(&utf16) {
                    out.insert(gid, text);
                }
            }
            at = start + end;
        }
    }
    out
}

/// The text a finished PDF draws, recovered the way a reader's copy-and-paste does: decode each
/// show as Identity-H glyph ids and put them through the `/ToUnicode` CMap.
///
/// This is what nothing in the workspace could do before spec 0068, because nothing wrote a CMap.
pub fn extract_text(pdf: &[u8]) -> String {
    let map = to_unicode_map(pdf);
    let mut out = String::new();
    for stream in content_streams(pdf) {
        for item in shown_items(&stream) {
            if let Shown::Glyphs(gids) = item {
                for gid in gids {
                    match map.get(&gid) {
                        Some(text) => out.push_str(text),
                        None => out.push('\u{fffd}'),
                    }
                }
            }
        }
    }
    out
}

fn find(haystack: &[u8], needle: &[u8]) -> Option<usize> {
    haystack
        .windows(needle.len())
        .position(|window| window == needle)
}
