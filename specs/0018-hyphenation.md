# 0018 — Hyphenation (Knuth-Liang, en-US)

- **Milestone:** M1
- **Status:** in-progress (increment 1 implemented — penalty item stream + `Hyphenator` seam at
  parity; increment 2 — real en-US `hypher` hyphenation + rendered hyphen — pending)
- **Crates:** `quill-text-layout` (owner), `quill-layout-engine`, `quill-export-pdf`

## Problem

Knuth-Plass line breaking (spec 0017) chooses breakpoints only at **inter-word spaces**: whole words
are indivisible boxes. Without hyphenation, a long word can only move to the next line as a unit,
which forces loose interior lines (large `space_adjust_pt` under justification, spec 0017 incr. 2) and
strands over-wide words on their own overflowing line (the greedy fallback). Real typesetting breaks
words at legal syllable boundaries so lines fill more evenly and over-long words fit at all.

Knuth-Plass was *designed* around hyphenation: its item stream is boxes, glue, **and penalties**, and
each legal in-word break is a flagged penalty. Spec 0017 built the box/glue half and deliberately left
the penalty slot empty ("KP's penalty slot is designed for it but it is not populated here"). This
spec populates it.

## Decisions agreed up front

- **Pattern source: [`hypher`](https://github.com/typst/hypher)** — Typst's Knuth-Liang hyphenation
  crate, `MIT OR Apache-2.0`, **zero dependencies** (patterns embedded as `no_std` data). It matches
  the project's "mine Typst" strategy (like `pdf-writer`/`subsetter`/`ttf-parser`) and the
  minimal-permissive-deps constraint. The heavier `hyphenation` crate (runtime pattern-file loading,
  extra deps) was rejected on dependency-graph grounds. The dep is added **only at increment 2**;
  increment 1 is trait-only and dependency-free.
- **Language scope: en-US only** for the first cut (TTRPG books are English-first). The hyphenator is
  constructed for US English; a document-driven language-selection seam is a named non-goal for a
  later increment.

## Architectural decision (machinery first, patterns + rendering second)

Following specs 0016 (measure-at-parity, then shape) and 0017 (optimal-break, then justify), the risky
DP generalization lands first behind a seam whose **default changes nothing**, then the real patterns
and the visible hyphen glyph land second:

- **Increment 1 — the penalty item stream + `Hyphenator` seam at parity (no new dependency).** Add a
  `Hyphenator` trait (`hyphenate(word) -> break offsets`) with a **`NoHyphenator` default that returns
  no breaks**. Generalize `break_paragraph`/`justify_paragraph` from "one box per word" to a
  box/glue/**penalty** item stream: each hyphenator break point becomes a *flagged penalty* whose
  materialized-on-break width is a hyphen glyph. With `NoHyphenator`, every word is a single box and
  the breaking is **byte-identical to spec 0017** (parity). The machinery is exercised by a
  deterministic **stub hyphenator** in tests (mirroring `MonospaceRunMetrics`). No patterns, no
  `hypher`, no output change in the real pipeline.
- **Increment 2 — real en-US hyphenation + rendered hyphen + oversized words (output changes).** Add
  the `hypher` dependency; implement `HypherHyphenator` behind the seam; `lay_out`/`export` switch
  from `NoHyphenator` to it. Lines that break inside a word render a trailing hyphen glyph (`U+002D`),
  which must be added to the font subset. Over-wide words are broken at hyphenation points where
  possible (shrinking the spec-0017 greedy-fallback surface to only the genuinely-unbreakable case).
  Regenerate the Ghostscript golden if breakpoints move.

Measurement stays on the **spec-0016 `RunMetrics` seam**; the acyclic crate seam
(`export-pdf → layout-engine → text-layout`) is unchanged — `text-layout` owns the algorithm and the
`Hyphenator` trait, `export-pdf` supplies both the shaper and (in incr. 2) the `hypher`-backed
hyphenator, `layout-engine` calls the breaker.

## Behavior

### The box / glue / penalty item stream (both increments)

Spec 0017's model gains **penalty** items. A paragraph is a sequence of:

- **Box** — a maximal run of a word between hyphenation points. A word with no break points is one
  box (today's behavior); a word with `k` interior break points is `k + 1` boxes separated by penalties.
- **Glue** — one inter-word space (natural `g = measure_run(" ")`, `stretch = g/2`, `shrink = g/3`),
  unchanged from spec 0017.
- **Penalty** — a potential breakpoint that is **not** an inter-word space. A hyphenation penalty is
  **flagged**, carries cost `HYPHEN_PENALTY`, and has a **break width** `measure_run("-", size_pt)`
  that is added to a line's natural width **only if the line breaks at that penalty** (standard KP:
  a penalty's width materializes on break, is zero otherwise). Inter-word glue remains the only
  *unflagged*, zero-cost breakpoint.

### `Hyphenator` seam (increment 1)

```rust
pub trait Hyphenator {
    /// Byte offsets inside `word` at which a hyphen may be inserted. Strictly interior
    /// (`0 < off < word.len()`), ascending, on `char` boundaries. Empty = do not hyphenate.
    fn hyphenate(&self, word: &str) -> Vec<usize>;
}

/// The parity default: never hyphenates. Keeps breaking byte-identical to spec 0017.
pub struct NoHyphenator;
impl Hyphenator for NoHyphenator { fn hyphenate(&self, _: &str) -> Vec<usize> { Vec::new() } }
```

`break_paragraph`/`justify_paragraph` gain a `&impl Hyphenator` parameter (or a sibling entry point;
the exact signature is an implementation detail, but the parity path must be preserved and covered).

### Demerits with penalties (increment 1)

The spec-0017 line demerit `d = (LINE_PENALTY + b(r))²` is extended in the classic TeX way:

- If the line ends at a **hyphenation penalty** `p = HYPHEN_PENALTY` (≥ 0): `d = (LINE_PENALTY + b)² + p²`.
- If the line ends at inter-word glue (the paragraph-internal word boundary) or is the paragraph's
  last line: `d = (LINE_PENALTY + b)²` (unchanged).
- **Consecutive flagged breaks:** if this line *and* the previous line both end at a flagged (hyphen)
  penalty, add `DOUBLE_HYPHEN_DEMERIT` (TeX's `\doublehyphendemerits`) — this discourages "hyphen
  ladders" (three-plus hyphenated lines in a row).

Named constants (TeX defaults): `HYPHEN_PENALTY = 50`, `DOUBLE_HYPHEN_DEMERIT = 10_000`. Feasibility
(`r ≥ −1`), badness (`100·|r|³` clamped to `BADNESS_CEIL`), and the `f64` demerit accumulation +
deterministic tie-break all carry over unchanged from spec 0017.

### Line reconstruction with hyphens (both increments)

A line that ends at an **inter-word** break joins its words with single spaces (today). A line that
ends at a **hyphenation** break emits `prefix + "-"` (the hyphen glyph), and the next line begins with
the remainder of the split word with **no leading space**. The line's natural width used for
justification (spec 0017 incr. 2) **includes** the hyphen width, so a hyphenated justified line still
fills the frame exactly. With `NoHyphenator` no line ever ends at a hyphenation break, so no hyphen is
ever emitted — the increment-1 real pipeline output is unchanged.

### Real hyphenation + rendering (increment 2)

- `HypherHyphenator` wraps `hypher`: `hypher::hyphenate(word, Lang::English)` yields the word's
  **syllables** (`&str`), so the adapter accumulates syllable byte-lengths into the interior break
  offsets the `Hyphenator` trait returns (dropping the trailing syllable's offset = word length).
  `hypher::hyphenate_bounded(word, lang, left_min, right_min)` supplies sensible minimum stub lengths
  (e.g. 2/3) for free. Built once (like the shaper), it lives in `export-pdf` next to the font/shaper.
- `lay_out`/`export` pass the `HypherHyphenator` instead of `NoHyphenator`. Body and heading text are
  both hyphenated (alignment is orthogonal — spec 0017's `Alignment` still governs spacing).
- `collect_doc_chars` (export) adds `U+002D` unconditionally (as it already does for the space), so the
  hyphen glyph is always in the subset even when the source text contains no literal hyphen.
- **Over-wide words:** a word wider than the frame is broken at the hyphenation point that best fits;
  only a word with *no* usable hyphenation point still overflows and triggers the spec-0017 greedy
  fallback. The fallback is thus narrowed, not removed (a visible overflow still beats refusing to lay
  out — the spec-0017 principle).

## Inputs / outputs

- **Input:** a `Document`, a `RunMetrics` implementation (the rustybuzz shaper in export; the
  monospace stub in tests), and a `Hyphenator` (`NoHyphenator` in incr. 1's real pipeline and in
  parity tests; a stub in incr. 1 machinery tests; `HypherHyphenator` in incr. 2's real pipeline).
- **Output:** `Vec<LaidOutPage>` whose lines may break inside words at legal syllable points, each such
  line ending in a rendered hyphen. In incr. 1 the real pipeline output is byte-identical to spec 0017.

## Acceptance criteria

### Increment 1 (machinery at parity)

- **Parity with spec 0017 under `NoHyphenator`:** for a representative set of paragraphs,
  `break_paragraph`/`justify_paragraph` with `NoHyphenator` return exactly what spec 0017 returns
  (all existing 0017 tests pass unchanged; the real export pipeline emits identical bytes — the
  Ghostscript golden does not move).
- **Penalty breaking works under a stub hyphenator:** with a deterministic stub that breaks a crafted
  long word at a known offset, a paragraph where hyphenating tightens the fit returns the hyphenated
  breaking, the broken line ends in `-`, its natural width includes the hyphen, and its total demerits
  (including `HYPHEN_PENALTY²`) are lower than the non-hyphenated breaking. Hand-computed / brute-forced.
- **Double-hyphen demerit:** a case where two adjacent hyphenated lines are penalized enough that a
  breaking with fewer consecutive hyphens wins.
- **Deterministic:** identical `(text, width, size, metrics, hyphenator)` → identical lines across runs.
- Workspace green (`fmt`, `clippy --all-targets --all-features -D warnings`, `build`, `test`); **no new
  dependency** added in this increment.

### Increment 2 (real en-US hyphenation + rendering)

- **`hypher` wired, en-US:** a paragraph with a long word (e.g. "hyphenation") breaks at a
  hypher-reported syllable point when it improves the fit; the line renders a trailing hyphen.
- **Hyphen in subset:** the exported PDF's font subset always includes `U+002D`; a justified hyphenated
  line fills the frame (hyphen width counted).
- **Over-wide word broken:** a word wider than the frame that has a hyphenation point is split across
  lines rather than overflowing; a word with no hyphenation point still lays out via the (narrowed)
  greedy fallback and never panics.
- **Golden validity:** the exported sample remains valid under the Ghostscript gate; regenerate the
  golden if breakpoints move. Dependency facts recorded: `hypher` version, that it adds no transitive
  deps, and `cargo tree -d` stays clean.
- Workspace green as above.

## Non-goals (named follow-ups)

- **Multi-language / document-driven language selection** — a `lang` field on the document/paragraph
  and a language→`hypher::Lang` mapping. This increment is en-US only.
- **Hyphenation quality controls** — `\hyphenpenalty` vs `\exhyphenpenalty` distinction,
  explicit-hyphen (`U+2010`) handling, and non-breaking runs. Sensible built-in minimum stub lengths
  ship via `hypher::hyphenate_bounded` (e.g. 2/3), but *exposing* `\lefthyphenmin`/`\righthyphenmin`
  as tunable settings is later.
- **Discretionary/soft hyphens (`U+00AD`) in source text** — honoring author-inserted break hints.
- **Cross-frame/column and multi-paragraph optimization** — hyphenation optimizes one paragraph against
  one frame width, as in spec 0017.
- **Non-Latin / complex-script hyphenation and bidi interaction** — LTR single-script only.

## Performance note

`hypher` is fast and allocation-light, but `hyphenate(word) -> Vec<usize>` allocates per word; the
per-candidate `Vec<usize>` line-start clone flagged in spec 0017's review compounds this. Both are
acceptable for body paragraphs now and fold into the **M1 perf-harness** increment (back-pointer
reconstruction; reuse a scratch buffer for break offsets). Hyphenation runs **per paragraph**, so it
composes with the incremental/dependency-tracked layout engine — only a reflowed paragraph re-breaks.
