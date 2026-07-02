# Plan — spec 0018 increment 2: real en-US hyphenation (`hypher`) + rendered hyphen + over-wide words

## Task
Land spec 0018 increment 2: add the `hypher` dependency, implement `HypherHyphenator` (en-US)
behind the existing `Hyphenator` seam, switch the export/layout pipeline from `NoHyphenator` to it,
ensure the hyphen glyph (`U+002D`) is always in the font subset, and let over-wide words break at
hyphenation points. Increment 1 already built the penalty item stream, the rendered trailing hyphen
in `break_paragraph_hyphenated`, and the `NoHyphenator` parity default — this increment supplies the
real patterns and wires them in.

## Acceptance criteria (from specs/0018-hyphenation.md, increment 2)
- `hypher` wired, en-US: a paragraph with a long word breaks at a hypher-reported syllable point
  when it improves the fit; the line renders a trailing hyphen.
- Hyphen in subset: exported PDF's font subset always includes `U+002D`; a justified hyphenated line
  fills the frame.
- Over-wide word broken: a word wider than the frame with a hyphenation point splits across lines
  instead of overflowing; a word with no hyphenation point still lays out via the greedy fallback and
  never panics.
- Golden validity: exported sample stays valid under the Ghostscript gate (CI re-exports; no committed
  golden bytes). Record `hypher` version, that it adds no transitive deps, `cargo tree -d` clean.
- Workspace green (fmt, clippy -D warnings, build, test).

## Files to touch
- `Cargo.toml` (workspace) — add `hypher` to `[workspace.dependencies]`.
- `crates/export-pdf/Cargo.toml` — depend on `hypher`.
- `crates/export-pdf/src/hyphenate.rs` (new) — `HypherHyphenator` impl over `hypher`, en-US, with
  bounded left/right minimums; unit tests against real patterns.
- `crates/export-pdf/src/lib.rs` — module decl; `collect_doc_chars` adds `U+002D`; `export()` builds a
  `HypherHyphenator` and passes it to `lay_out`.
- `crates/layout-engine/src/lib.rs` — `lay_out` gains a `&impl Hyphenator` param; calls
  `justify_paragraph_hyphenated`; test call sites pass `&NoHyphenator`.
- `crates/text-layout/src/lib.rs` — (only if needed) confirm over-wide-word breaking already works via
  the penalty DP; add/keep a test proving a hyphenatable over-wide word splits.

## Test strategy
- `HypherHyphenator` unit tests: "hyphenation" yields interior offsets on char boundaries, strictly
  ascending, `0 < off < len`; a word with no legal break returns empty; bounded minimums respected.
- Layout wiring: a doc whose body has a long word breaks with a trailing hyphen under the real
  hyphenator; over-wide hyphenatable word splits rather than overflows.
- `collect_doc_chars` includes `'-'` even when the source has no literal hyphen; the bundled font maps
  `'-'` to a real glyph.
- Keep all spec-0017 / increment-1 parity tests green (NoHyphenator path unchanged).
- Verify `hypher` API against installed source in `~/.cargo` before coding.
