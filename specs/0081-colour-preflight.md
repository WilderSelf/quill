# 0081 — Every colour that reaches the page is checked

**Milestone:** M7 · **Status:** implemented

## Why

Two colours reached the press file without ever being looked at. Both are the same defect wearing
different clothes — **a producer of ink that the checker did not know about** — and both ship today,
which is why this is the first M7 increment and why it is press-correctness rather than a feature.
It is 0075's precedent: a live defect outranks a new feature.

**(A) A master static's colour is never checked, so an over-inked folio prints on every page.**
`preflight()` walks `doc.content` and nothing else, so a master page is never visited.
`preflight_pages()` — added by spec 0037 *precisely because* "colour checks on the model cannot see
geometry the engine synthesized" — does walk `statics.chain(blocks)`, but its colour loop opened with

```rust
let PlacedBlock::Rect { fill, stroke, .. } = block else { continue; };
```

and a `MasterStatic::Text` becomes a `PlacedBlock::Text`. So a running head at CMYK 90/90/90/90 —
**360% ink**, half again past the limit `CLAUDE.md` names as non-negotiable — passed preflight with
zero findings and was emitted on every page its master governed. That is the exact scenario spec
0037 was written to prevent, one variant over.

Reachable in practice rather than theoretically: master statics are authored in `document.json`,
user-authored templates carry `master_pages` and validate versions and trims but never colours, and
a `.qpack` carries templates. No test in the repo gave a master static an illegal colour.

The RGB case was milder and worse documented. The writer turned it silently black under a comment
saying *"preflight rejects RGB before export, so `Rgb` is unreachable here"* — **which was false for
a master static**. A comment asserting an invariant that does not hold is worse than no comment: it
is what stops the next reader from checking.

**(B) A character style's colour is never checked, and it falsified a claim in `CLAUDE.md`.** The run
half of the colour check read each run's *direct* override (`run.style.color`) and `continue`d on a
run that had none. The colour that reaches the page is the **resolved** one, folding the named
character style — `resolved_style(r, styles).color.unwrap_or(paragraph)`, which is what fills
`run_colors` and thence what the writer draws. None of the four bundled character styles carries a
colour, so no shipped document tripped it; a document, template or pack that defines one escaped
preflight entirely.

`CLAUDE.md` said *"`DefColor` has no RGB family at all, so a pack cannot even express a colour space
PDF/X-1a forbids."* True of `DefColor` — but a `.qpack` also carries a `StyleSheet`, whose
`character` map holds a full `Color` including `Rgb`, and templates whose master statics do too. **A
pack could express RGB by two routes, and (A) meant one of them reached the press file.** That
sentence is corrected in this increment rather than left standing.

## What

**One enumeration of every colour a page can put on paper, and the compiler holds it closed.**

```rust
// quill-layout-engine
pub enum InkSite { Text, Run(usize), Fill, Stroke }
pub struct PlacedInk { pub site: InkSite, pub color: Color }

impl PlacedBlock {
    pub fn inks(&self) -> Vec<PlacedInk>;
}
```

`preflight_pages` iterates `page.statics.chain(page.blocks)` and, for each, every entry of
`block.inks()`. That is the whole of the colour pass.

### Why one enumeration rather than two more arms

Both defects were "a colour that exists in one place is checked by neither of the two checkers", and
adding an arm for master statics and an arm for character styles would have left the *next*
colour-bearing site in exactly the position these two were in. The workspace has already answered
this shape of problem three times — spec 0074 turned layout-time tokens into one enum that the
resolver and the font-subset collector both read, and 0076, 0077 and 0078 each found a **new path**
into the same collector and gave it the same treatment rather than a special case. This is the same
shape at a fourth site, so it gets the same answer.

**Three properties make `PlacedBlock::inks` a closed enumeration, and all three are load-bearing:**

1. **`PlacedBlock` is the only thing the writer draws.** `writer::write_pdf` matches on it and on
   nothing else, so a colour not reachable from here cannot reach the page. That is what makes
   "every colour" a provable claim rather than a hopeful one — the same argument spec 0037 made for
   checking placed geometry at all, carried to its conclusion.
2. **The match is exhaustive over variants**, so a new kind of placed geometry does not compile until
   it says what ink it draws (**`E0004`**).
3. **Every arm destructures every field by name, with no `..`**, so *adding a field* to an existing
   variant does not compile either (**`E0027`**). This is the half a plain exhaustive match would
   miss, and it is the half that mattered: the defect was a colour field (`Text::color`,
   `Text::run_colors`) on a variant the loop already skipped, not a new variant.

Both were verified by reintroduction rather than asserted — see *Test strategy*.

`MasterStatic` gets the same treatment in the model pass, for the same reason and with the same
no-`..` destructuring.

### How (B) is closed by the same enumeration

`run_colors` on a `PlacedBlock::Text` is *already* the resolved ink: `run_inks` computes
`resolved_style(r, styles).color.unwrap_or(paragraph)` for every run, folding the named character
style. So checking `run_colors` checks the colour that reaches the page, and (B) needed no separate
mechanism at the placed level at all — only a checker that looked at text.

For the pre-layout pass, the resolution moved rather than being reimplemented:
**`StyleSheet::resolve_run`** now lives in `core-model` and `quill_layout_engine::resolved_style`
forwards to it. A second implementation of "how a run's style resolves" living in `export-pdf` is
*precisely* how a named style's colour went unchecked, so there is one function and both consumers
call it.

### The two checks, one statement each

`ink_check(color, preset) -> Option<CheckId>` and `colour_message(check, preset, subject)` state the
rule and the wording once, and the model pass and the placed pass both read them. The two checks are
**ordered, not independent**: `ink_coverage_pct` returns `None` for RGB, so reporting an RGB colour
as *also* over the limit would be two findings for one mistake and the second would be meaningless.

The two decoration phrasings are the pre-0081 wording byte for byte, because a message an author has
learned to search for is part of the interface.

### The model pass is deliberately a subset, and says so

`preflight()` still runs a colour pass, extended to master statics and to resolved run colours. It is
**not** the authoritative check and its doc comment says which one is: it runs before layout, so it
can only see what the model declares.

It was extended rather than deleted because `quill preflight` lays nothing out. A model pass that
stayed blind to a master static would print *"preflight: no findings"* about a document `quill
export` is about to refuse — a false pass of exactly the shape spec 0052 built `Skipped` to avoid,
and the more dangerous half of defect (A) for an author who preflights before exporting.

What it owes is stated as an obligation: it never reports a colour that is legal, and never stays
silent about one a `Document` alone is enough to condemn.

### Deduplication, and why it arrived with this increment

Colour findings are deduplicated **document-wide** on (check, site, colour), not per page. A running
head is stamped on every page its master governs, so reporting per page would answer one authoring
mistake with 500 copies of one sentence — the argument the safe-area and dpi checks in the same
function already make, and one that only became load-bearing when furniture entered this loop.

The site is identified by the finding's phrase, which carries the **block id** for content and
nothing for furniture — master statics have no identity by construction (`PlacedBlock::Text::source`
is `BlockId::UNASSIGNED` for them, spec 0040). So two over-inked paragraphs are two findings while
one over-inked running head on 500 pages is one, which is what each of them actually is.

## Severity

**`Error`, both checks, at both sites** — matching what a block's colour and a decoration's colour
have always been classified as. An over-inked folio is a press defect, not a style preference: at
360% the ink does not dry, it sets off onto the facing sheet, and no printer accepts it. Neither is
a `Warning` candidate in the sense spec 0049 uses that severity (a trim size the preset does not
list, which is *possibly* fine and needs confirming with the printer) or spec 0001 does (image alpha
that will be flattened, where the output is still conformant).

The consequence follows from the classification and is the point of it: `PreflightReport::passed()`
is false, `export` returns `PreflightFailed`, and **no bytes are written** unless the caller passes
`--force`.

## `FORMAT_VERSION`

**Stays 10**, and this was checked rather than assumed. The bump rule turns on a *silence* — an older
build reading a newer document and being wrong about it quietly. This increment adds **no model
surface at all**: no field, no variant, no serialized key anywhere in `Document`, `StyleSheet`,
`MasterPage` or `PageTemplate`. `StyleSheet::resolve_run` is a method over data that already existed
and `PlacedBlock::inks` is a method over the layout engine's output, which is not persisted. There is
no document an older build could read differently, so neither half of the rule fires.

`TEMPLATE_VERSION` stays **1** and `PACK_VERSION` stays **1**, by the same argument: no serialized
shape moved. Note that a *template* is now transitively better guarded — a user-authored template's
`master_pages` validates versions and trims but never colours, and its statics are now checked when
the document made from it is preflighted.

## Digests

**`SAMPLE_EXPORT_DIGEST` did not move**, verified rather than assumed: the constant is untouched and
`the_sample_export_is_byte_identical` passes. That is the expected result and it is a real check
rather than a formality — nothing in this increment touches the writer's output path, and the
sample's colours are legal, so a moved digest would have meant the preflight change had reached the
bytes and something was wrong. The only writer edit is two comments.

`benches/budgets.toml` is unchanged and `cargo bench -p quill-testdoc` passes: preflight is not on
any benchmarked path, and the colour pass is one `Vec` per placed block over a page set that was
already being walked twice.

## Acceptance criteria

- [x] A master static at CMYK 90/90/90/90 (360% ink) is **reported** by `preflight_pages`, by
      `preflight`, and **not emitted** — `export` returns `PreflightFailed` and writes zero bytes.
- [x] A master static with an RGB colour is reported by both passes.
- [x] A run whose **named character style** carries RGB is reported by both passes.
- [x] A run whose named character style pushes it over 240% is reported by both passes.
- [x] A document whose colours are all legal produces **no** colour findings, at either pass, for
      grayscale, a tint and a 150% CMYK — and neither does `Document::sample()`.
- [x] `PlacedBlock::inks` enumerates the expected sites for all four variants, including the two that
      draw no authored colour.
- [x] Adding a variant to `PlacedBlock` fails to compile at `inks` with `E0004`; adding a *field* to
      an existing variant fails with `E0027`; adding a field to a `MasterStatic` variant fails with
      `E0027` in `preflight`. All three verified by reintroduction.
- [x] The pre-0081 decoration findings are unchanged in check, severity, wording and count.
- [x] `quill preflight`'s golden report over the sample is byte-identical (`preset_cli.rs`).
- [x] `SAMPLE_EXPORT_DIGEST` unmoved; `FORMAT_VERSION` unmoved at 10.
- [x] Full workspace validate green: fmt, clippy `-D warnings`, build, test, bench.

## Test strategy

`crates/export-pdf/tests/colour_preflight.rs`. **These tests were written against the defects as they
shipped, so they fail on the parent commit rather than only against a reintroduction** — which is
the stronger form of the proof, and the reason the file was written before the fix.

Measured on `4f80a8f` with only the test file added: **7 of 8 failed**, and the eighth is the one
that must pass on both sides.

| Test | Against the shipped defect |
|---|---|
| `an_over_inked_master_static_is_reported_over_placed_geometry` | fails — `[]`, no findings at all |
| `an_rgb_master_static_is_reported_over_placed_geometry` | fails — `[]` |
| `an_over_inked_master_static_is_reported_by_the_model_check_too` | fails — only the unrelated `OutputIntent` finding |
| `an_rgb_master_static_is_reported_by_the_model_check_too` | fails — same |
| `an_over_inked_master_static_is_reported_not_emitted` | fails at `expect_err` — **the export succeeded**, which is defect (A) in one line |
| `an_rgb_character_style_is_reported` | fails |
| `an_over_inked_character_style_is_reported` | fails |
| `a_document_whose_colours_are_all_legal_produces_no_colour_findings` | **passes**, as it must — spec 0050's expensive failure mode, and the test that stops the other seven passing against a checker that reports everything |

The compiler-enforcement claims were verified the same way, by temporarily editing the model and
reading the error:

| Change made | Result |
|---|---|
| `shadow: Option<Color>` added to `PlacedBlock::Rect` | `E0027: pattern does not mention field 'shadow'` at `PlacedBlock::inks` (plus three `E0063`s at construction sites) |
| `PlacedBlock::Gradient { frame, from, to }` added | `E0004: non-exhaustive patterns: '&PlacedBlock::Gradient { .. }' not covered` at `inks` |
| `tint: Color` added to `MasterStatic::Image` | `E0027` at the master-static arm in `preflight` |

All three were reverted; the workspace builds clean.

## Risks

- **The dedupe can merge two genuinely different sites.** Two content blocks sharing a block id
  cannot happen, but a `PlacedBlock::Rect` has no id at all, so two decorations of the same illegal
  colour anywhere in the document report once. That is a reduction in report volume, never in
  refusal: `passed()` is false either way and export refuses either way.
- **A block's own `color` is reported even when every run overrides it.** `run_colors` folds the
  paragraph colour, so a run resolving to the paragraph's ink is enumerated twice and a paragraph
  whose colour is illegal but wholly overridden is still flagged. That is a duplicate finding, never
  a missed one, and it matches what the pre-0081 model pass already reported for the same document.
- **Images remain out of scope for the colour check, at both passes.** An image's ink is its pixels,
  and those are converted and clamped in `images.rs` (spec 0006) rather than checked as a `Color`.
  This increment does not change that; defect (D) in `docs/roadmap.md` is where the image path's
  colour question lives, and spec 0082 owns it.
- **`--force` still writes RGB as solid black.** That arm is reachable *by design* — `--force` exists
  precisely to write a file preflight refused — which is why the writer's comment now explains the
  fallback instead of claiming it is unreachable.

## Non-goals

- **Making `quill preflight` lay the document out.** The model pass now catches everything a
  `Document` alone can condemn, which covers both defects here. A CLI that laid out to preflight
  would also need a font, and would make `quill preflight` and `quill export` two paths to one
  answer — the drift spec 0059 fixed. If a colour is ever synthesized by the engine from something
  the model does not state, `preflight_pages` is where it is caught and `export` is where it is run.
- **A colour that is not a device colour.** A named swatch or a spot separation is spec 0083, and it
  is a fourth `Color` variant — at which point `inks` and both checks stop compiling until they say
  what a spot contributes to the 240% limit, which is exactly the machinery this increment installs
  for it.
- **An ink limit that varies by object type.** A printer states one limit and a panel and a paragraph
  reach the same printer (spec 0037).
- **Warning on a colour that is legal but close to the limit.** A "230% is nearly 240%" warning is a
  house-style preference rather than a press requirement, and spec 0050 is explicit that a check
  which cries wolf costs more than it is worth.
