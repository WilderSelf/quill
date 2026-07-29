# 0078 — The index: marked terms, collated, page-ranged

**Milestone:** M6 · **Status:** implemented

## Why

A 500-page reference book is unusable without an index, and quill had no way to say that a paragraph
is *about* something.

The M6 audit priced this increment correctly, and its finding is the whole shape of the work: **the
index is less new than assumed, and new in a different place.** Its derivation is `Block::Toc`'s — a
block that stores nothing and derives its entries from the final page vector (spec 0041). Its mark is
a run-level annotation, the slot a character style already established (spec 0065). Its rendering is
a tabbed line against a right stop, which is what spec 0070 made the contents list. Its splitting is
spec 0075's, which was sequenced before this increment precisely so the index would not inherit the
contents list's overflow defect.

What has **no analogue anywhere in the workspace** is **collation** and **page-range coalescing**.
Nothing in quill sorts text: `BTreeMap<String, _>` is byte order, which is wrong for case, for
diacritics and for leading articles, and there is no locale or collator anywhere in the dependency
graph. Nothing coalesces `42–45, 47` out of a set of pages. Both are pure functions with no engine
coupling, which — as the audit said — makes them the *safest* new machinery in M6 rather than the
riskiest. It also makes them the two decisions this spec is really about.

## What

### The mark is a field on `Run`, not a `RunSource` variant

```rust
pub struct Run { pub text: String, pub style: InlineStyle, pub character: Option<String>,
                 pub source: RunSource, pub index: Option<IndexMark> }

pub struct IndexMark { pub term: String, pub sort_as: Option<String> }
```

`RunSource` was the obvious home and is the wrong one, and the reason is a property of the enum
rather than a preference. **`RunSource`'s variants are mutually exclusive by construction**: a run
draws its own characters, *or* a cross-reference's folio, *or* a footnote's number. It cannot draw
two. A mark is not that kind of thing. "…see the *bestiary* on page 42" is one run that is both a
cross-reference and an index term, and a `RunSource::Index` variant would make the model unable to
say so — a restriction invented by the representation rather than by the domain, which is the shape
`CLAUDE.md` calls a defect.

It therefore sits where `Run::character` sits, for `Run::character`'s stated reason: an annotation on
a run is not a replacement for what the run *is*. `#[serde(skip_serializing_if)]`-omitted, so an
unmarked run's manifest text does not move — which matters here for the same reason it mattered in
0076, since the `/ID` is a hash of that text.

**`term` is authored rather than taken from the run's text**, and both consequences are load-bearing.
An author can index a concept the sentence never names, and can file an inverted heading against text
that reads normally. It is also what makes a mark a genuinely *new* font-subset path — see below.

**`sort_as` is the escape hatch every index in publishing has.** `de Gaulle, Charles` files under
`Gaulle`; `1984` files under `Nineteen Eighty-Four`; `St Albans` under `Saint Albans`. No collation
rule — not this one, not a full UCA implementation — can derive any of those, because they are facts
about the language and about the author's intent rather than about the characters.

### The block is `Block::Toc`'s shape

```rust
Block::Index { id, title, ignore_leading: Vec<String>, color }
```

No entries, and spec 0041's rule governs them: **derived from the final pages, never accumulated
during pagination.** An incremental pass reuses whole pages, so anything counted while placing goes
missing on a reused page — and goes missing precisely when the document has just been edited.

`ignore_leading` is the one thing that cannot be derived, and it is discussed under *Collation*.

**A term's page is the page its own run landed on**, not the first page of its block. A placed
paragraph carries the span map spec 0063 built — which authored run every stretch of every line came
from — so a paragraph cut across a page boundary (spec 0044) reports a term marked in its last
sentence on the page that sentence is actually on. Falling back to the block's first page would be
right for every paragraph that fits a frame and wrong for exactly the long ones an index is for. The
fallback survives for the case with no span at all: a mark on an empty run, or on a paragraph
positioned by tab stops, which is laid as panel parts and carries no run map.

A term marked in a block the pass did not place contributes no page, and a term with no page
contributes no entry — spec 0076's answer for an unplaced reference target, at the one other site
that asks the question.

### The rendering is the tab mechanism, with three differences that are decisions

`measure_index` is `measure_toc` almost line for line: one tabbed line per entry against one right
stop, a term too long for its column clipped with an ellipsis rather than wrapped, and the block cut
between entries. Reusing the shape is the point — an index is a contents list with different entries,
and two mechanisms producing different geometry is the state spec 0070 ended.

- **No dot leader.** A contents list has a dozen long entries and the leader carries the eye across
  the gap; an index has hundreds of short ones, and a page of dot leaders reads as a contents list.
  The right stop stays, because keeping the numbers in one scannable column is what a right stop is
  *for*.
- **The number column is measured, not reserved.** `TOC_NUMBER_COLUMN_PT` is wide enough for any page
  number; an index entry ends in `iv, 42–45, 47`, whose width is a property of the entry. The term is
  clipped against what this entry's own folios measure.
- **No `link_page`.** A contents entry points at one page; an index entry points at several, and a
  hot area over the whole run of numbers would navigate to whichever one the implementation picked.
  Named a non-goal below rather than guessed at.

Two built-in styles, `index-title` and `index-entry`, on spec 0041's precedent for `toc-*`. **One
entry style, where a contents list has six**: a contents list's levels are the heading levels it
mirrors and are therefore given, and an index has no such structure to mirror.

### Splitting is 0075's, and this is the block it exists most for

`MIN_ENTRIES_PER_FRAGMENT`, `repeat_h: 0`, `keep_together: true`, the title folded into item 0 — the
contents list's answers, for the contents list's reasons. A 500-page book's contents list is three
pages and its index is longer, so the mechanism 0075 shipped to fix a defect nobody had exercised is
load-bearing here from the first document.

## Collation is the real decision

`Cargo.toml`'s rule is that every dependency "is permissive (MIT/Apache-2.0 or compatible) and
carries an inline note saying which spec brought it in and why". Adding a collator is therefore an
argued decision, and byte order arriving by accident and being found later as a bug is the one
outcome that is not defensible.

### What was evaluated

Both candidates were checked against the live registry rather than from memory (`cargo info`,
`cargo add` into a throwaway project outside the workspace, per `CLAUDE.md`'s fixture rule).

| | licence | transitive deps | locale tailoring | MSRV |
|---|---|---|---|---|
| **`icu_collator` 2.2.1** | Unicode-3.0 | **34** crates | yes — the full CLDR set | **1.86** |
| **`feruca` 0.12.0** | MIT | **10** crates (`bstr`, `regex-automata`, `postcard`, `heapless`, `spin`, …) | no — DUCET/CLDR *root* only | unstated |
| **in-house rule** | — | 0 | no | — |

Both licences are compatible; neither was rejected on licensing. What decided it:

- **`icu_collator` is the only one that can express another language's rule**, and it is also the
  only one that cannot be adopted today. Its MSRV is **1.86** against the workspace's declared
  `rust-version = "1.75"`, so taking it raises the floor for every crate here; it brings 34
  transitive crates into a graph whose hyphenator was chosen partly for having *zero*; and
  `icu_collator_data` compiles the CLDR collation tables into every binary, including the CLI.
- **`feruca` does not actually buy the thing that mattered.** It implements the UCA over the root
  collation, which orders `ü` sensibly — but the root order is not Swedish's, not Danish's and not
  traditional Spanish's, so a book in any of those still needs a tailoring `feruca` has no way to
  express. Ten transitive crates for an answer that is *better* than byte order and still not
  language-tailored is the worst of the three trades.

### What was chosen, and its documented scope

**A named, tested, documented in-house rule** — `quill_core_model::collate`, "the quill index
collation" — with the growth path stated rather than implied.

A term's key is four levels, compared in order:

1. **Primary** — base letters. Case folded, diacritics folded, everything that is neither a letter
   nor a digit **ignored entirely**, digits forming a class that sorts ahead of letters.
2. **Secondary** — the accents that were folded away, so `resume` files before `résumé`.
3. **Tertiary** — case, lower before upper, so `apple` files before `Apple`.
4. **The term itself**, so the order is **total**. This is not decoration: without it two terms
   equal at every level above would compare `Equal`, a stable sort would preserve the order the
   marks were *found* in, and the index would silently reorder itself when a paragraph moved.

Diacritic folding is generated from Unicode's own canonical decomposition over **U+00C0–U+024F** —
every character in that range whose NFD form begins with an ASCII letter, 244 of them — plus a
hand-listed table of the digraphs and stroked letters that have *no* decomposition and therefore
cannot come from it (`Æ`→`ae`, `ß`→`ss`, `Ø`→`o`, `Ł`→`l`, `Þ`→`th`, …). The table was generated
rather than typed, because a hand-typed table of 244 entries is a table with a mistake in it, and a
test asserts the two parallel constants are the same length.

**The scope is stated as a non-goal, not left as a silence.** Two limits:

- **Filing is letter-by-letter.** Spaces and punctuation do not separate, so `New York` files after
  `Newark` and `Keep, the Ruined` files under K. Word-by-word filing is the other convention
  publishing sanctions; it is not implemented, and lifting it means a second primary weight for a
  word boundary rather than dropping it.
- **The rule is not language-tailored.** It is correct wherever a script's alphabetical order agrees
  with its Unicode order after folding — which covers Latin, and also Greek and Cyrillic, whose
  blocks are laid out alphabetically — and it is wrong wherever a language *tailors* that order:
  Swedish files `å ä ö` after `z`; traditional Spanish filed `ch` and `ll` as letters of their own.
  **Lifting this is a collator swap, not a patch to the table**, and the design is shaped so that it
  is exactly that: every consumer compares `CollationKey`s and never strings, so replacing
  `collation_key` replaces the rule. The concrete path is `icu_collator` behind that one function,
  once the workspace's MSRV can move to 1.86 and 34 crates plus the CLDR data are worth their weight
  — which is a judgement for a book that needs it, not one to make speculatively.

### Why this is not a mechanism only an English book can use

`CLAUDE.md` is explicit that a mechanism only one kind of book can use is a defect, and a collation
that *cannot express* another language's rule is different from one that ships one locale first. Two
growth points keep this on the right side of that line, and both are **data rather than code**:

- **`ignore_leading` is declared per index block and is empty by default.** quill never guesses a
  language. An English book states `["A", "An", "The"]`; a French one states `["Le", "La", "Les",
  "L'"]`. A hardcoded English article list would have been exactly the defect — and an English list
  applied to a French book files entries where nobody will look for them, so the harm is real rather
  than theoretical. A word is an article only when the term continues with a space or an apostrophe
  (`Theatre` is not `The` + `atre`), and the longest match wins so `["A", "An"]` cannot file
  `An Atlas` under `n`. An entry that is *only* an article files under it, because stripping every
  character would give it an empty key and file it ahead of the whole book.
- **`sort_as` is per entry**, and covers every exception no rule can reach.

This is the same posture the workspace already took for hyphenation: `hypher` with `english` only,
"en-US is the first-cut scope", behind a general mechanism.

## Page-range coalescing

`coalesce_folios(&[(NumberFormat, u32)]) -> String` — pure, in `core-model`, unit-tested directly
rather than only through a laid-out document.

**It coalesces folios, not page indices, and that is the whole of the function.** Consecutive page
indices need not be consecutive folios: a section boundary can restart the count (`restart_at`) or
change the format, so pages 9 and 10 of a file can print `ix` and `1`. Two entries join a run only
when they share a **format** and their **numbers** are consecutive, which is why the input is the
numbering behind the folio rather than the printed string — the two strings alone cannot say whether
`ix` and `1` are adjacent.

That needed one new accessor at each layer, and each is defined so it cannot disagree with what the
page actually prints:

- `Document::folio_number_in(runs, page)` returns `(format, number)`, and `Document::folio_in` — the
  one place a page number becomes text — is now one call to `NumberFormat::format` over it.
- `PageTemplate::folio_number(page)` is the same split at the layout layer, where the fixpoint holds
  a template and not a document. `DocumentTemplate` implements both off the same `FolioRun` list.
  `a_template_folio_is_its_own_numbering_formatted` asserts `folio(p) == folio_number(p).0.format(…)`
  for every page of a sectioned document and for the parity template's defaults.

A run of **two** or more consecutive folios prints as `first–last`. Two rather than three
deliberately: `42–43` is what an index sets for two adjacent pages, and a threshold of three would
print `42, 43` beside `42–45` for no reason a reader could see. The separators are two consts
(`INDEX_RANGE_SEPARATOR`, `INDEX_PAGE_SEPARATOR`) read by both the coalescer and the font-subset
collector, so the string that is *written* and the string that is *embedded* cannot drift.

## The fixpoint

An index is the **fourth** derived quantity, and it joins the one shared loop on spec 0072's
arithmetic: nesting multiplies passes where sharing adds to them, and the quantities are not
independent — a section's opener master changes the page count, which changes what page every term
is on.

It is the contents list's shape rather than the cross-reference's: an entry's height does not depend
on its page numbers (one clipped, never-wrapped line), so the index's own height is fixed once the
term set is, and only the printed digits move. It sits at the back, so it mostly moves only itself —
but a *multi-page* index still shifts its own later entries, and the bound is what covers that. It
reports `converged: false` and ships the last iterate, exactly as everything else in the loop does.

**It costs no further pass.** `benches/budgets.toml`'s `layout.fixpoint_iterations` was measuring
**3** against a ceiling of 4 on the worst workload the engine can be handed; the workload now also
carries **200 index marks over 120 terms and an index block** — enough that the index itself spans
pages — and it still measures **3**. The ceiling does not move. The reason it does not is the
contents list's: place, resolve what the first pass could not know, agree.

**The session seeds the loop from its previous pages**, which is spec 0076's move applied at this
site. The marks come from `doc.content`, so a seeded index already reflects any edit; only the pages
the terms are attributed to can be stale, and the exit condition still compares two consecutive
derivations. Without the seed, `next_index` differs from an empty seed on the **first** comparison of
every relayout of every indexed document, so an unchanged document pays a second whole-document pass
for ever. Asserted: an unchanged indexed document relayouts in exactly one iteration and measures
zero blocks.

## The cache: `context_fingerprint`, and one term that turned out not to be needed

**The contents list's route, and the decision is the arithmetic 0076 did coming out the other way.**

Spec 0076 established that a per-block *resolved value* belongs in `MeasureKey` rather than in
`context_fingerprint`, because a book has hundreds of cross-references and any edit that moves any
page moves at least one of them — so the context route would be a whole-document reflow per
keystroke. The index is one block, like the contents list and unlike a cross-reference: a changed
derivation costs one reflow and one eviction, which is what a derived block costs.

It is also the *only correct* route here, not merely the cheaper one. A `MeasureKey` term is a
property of the block being measured; the index's entries are derived from marks scattered through
every **other** block in the document, so there is no per-block value to key on.

So `context_fingerprint` gains the collated entries, and — spec 0075's lesson, checked rather than
assumed — the session's eviction of derived blocks extends from `Block::Toc` to `Block::Toc |
Block::Index`. Without it the fixpoint's second pass would serve the first pass's measurement from
cache, and since the first pass runs with an empty index by construction, every document laid out
through a `LayoutSession` — the path the app uses — would place an index consisting of nothing but
its own title, on every pass, for ever. That is 0075's defect verbatim, in the one place this
increment could reproduce it.

**Two other homes were considered and are empty, deliberately.**

- **`MeasureKey`** gains nothing. A mark does not change how its own paragraph sets, so hashing it in
  would re-break a paragraph that has not moved — the same argument spec 0077 made for a note's text.
  `incremental_blocks_measured` is still 1.
- **The per-pass diff** gains nothing either, **and this one is a correction.** A `mark_fingerprint`
  was written, on 0077's `note_fingerprint` precedent, and then could not be made to fail: removing
  it broke no test. The reason is that the two cases are not analogous. A note's text is not in
  `content` and changes the *band height*, which is layout the diff must see; an index mark changes
  only the derived entries, which are re-derived from `doc.content` on every pass and land in
  `context_fingerprint` — so a mark that changes what the index prints changes the context, and a
  mark that does not needs no reflow. The term was removed rather than kept as machinery nothing can
  justify, and the asymmetry is documented at `note_fingerprint`'s site so it does not read as an
  oversight.

## The font subset: a fourth path, and the first drawn away from where it was authored

Spec 0074 closed this class structurally (one enum, exhaustive matches, `E0004` rather than memory);
0076 found a second instance in a body run's generated characters and 0077 a third in a note's prose.
Each was a **new path** to the same collector, and each got the same treatment rather than a special
case. An index has two, and one of them is a kind the first three did not have.

**1. The marked terms.** This is the new kind: the characters are drawn *somewhere other than where
they were authored*. It fails two independent ways if treated as a case the run walk already covers:

- **The term need not be in the run's text at all.** It is authored on the mark, so a paragraph about
  routing can be indexed under "morale". Nothing in the body text carries those characters. This is
  the question the increment was told to check specifically, and the answer is **no** — a term's own
  characters are *not* already collected via its run.
- **Even a term that does match its run's text is in the wrong bucket.** A cross-reference draws
  inside its run and so takes the run's face; an index term is drawn by the index block, in the
  index's own style, in the **regular** face — exactly as a contents entry and a table cell are. A
  term marked inside a bold run would have its characters only in the bold subset.

So `IndexMark::contributes()` lives in `core-model` beside the struct, returns the same `TokenText`
currency the other two producers use, and the collector merges it into `everywhere` (the union folded
into every bucket) and flags the regular face as used. It travels as a **run**, not loose characters,
because it is authored text knowable exactly, so its ligatures are cut in like any other run's (spec
0068). It is unconditional on whether the document holds an index block, for
`RunSource::contributes`' stated reason: embedding a few characters nobody draws costs a few hundred
bytes, and failing to embed one that is drawn is a `.notdef` box in a press file.

**2. What the index block draws itself.** Its title, the **folio alphabet**, the two separators, and
the ellipsis a clipped term ends in. The folio is asked through `StaticToken::Page` rather than by
reaching for `folio_formats()` here, for spec 0076's property 4: there must be exactly **one** answer
to what a folio can draw, because the `{page}` token, a contents entry, a cross-reference and now an
index entry are four ways of printing the same number. The separators come from the same two consts
the coalescer writes.

**No latent defect was found at this site this time**, and the audit that would have found one was
run: every producer of layout-time characters was re-checked, and 0076's fix (the contents entry
asking `StaticToken::Page`) is still the whole answer for the contents list. The end-to-end half —
which the compiler cannot cover, since an arm returning the *wrong* characters still builds — is
asserted by laying a document out through the real press path and checking every character the index
actually printed against what was collected, plus `drawn_gids`, which panics on any `.notdef`.

## `FORMAT_VERSION`

**9**, and it is the first bump where both halves of `docs/format-spec.md`'s rule fire — which is
worth stating, because the half that decides it is the quiet one.

The **silence** half looks satisfied at a glance and is not. A v8 build meeting `Block::Index`
refuses the document outright: `Block` is a tagged enum and `"kind": "index"` is a tag it does not
have. That is as loud as a failure gets. But the marks are not in the index block — they are on the
**runs**, where a v8 build drops them as unknown keys, lays the book out with no visible difference
anywhere, and deletes the entire index on the first save.

The **loss** half is 0076's argument exactly. `index` is model, and it is authored intent: which
terms an author chose to index, and what each files under, cannot be regenerated from anything left
in the file. A v8 build that opens and saves destroys every mark permanently.

`migrate_8_to_9` is a structural no-op and writes nothing into the object, for `migrate_6_to_7`'s
reason: inserting `"index": null` into every run of every paragraph would rewrite the manifest text
of every document in existence and move its exported `/ID` with it.

**`TEMPLATE_VERSION` stays 1**, and the check is not a formality. Trigger 2 fires when a
`FORMAT_VERSION` bump changes the serialized shape of `PageSetup`, `StyleSheet`, `MasterPage` or
`PageOverride`. A `Run` is in none of them (a master static's text is a `String`); the index block is
content, and a template file has none; and the two new `index-*` styles add entries to a map without
changing `StyleSheet`'s shape — the same reasoning 0041, 0066 and 0077 applied to their own built-in
styles. A template written before them resolves them through the defaults, exactly as it does
`toc-*`.

## Digests

`SAMPLE_EXPORT_DIGEST` moves, and it is the **identifier-only** template for the fourth time. This
one had *three* candidate causes rather than one, so each is named and ruled out rather than the
shape being pattern-matched: `FORMAT_VERSION` became 9, `StyleSheet::default()` gained two entries,
and `Run` gained a field. All three are in `doc.to_json()`; none reaches the page. The sample has no
index block and no marked run, so `index` is omitted from every run, the collector's new path
contributes nothing, no entry is derived, and the flow takes the single pass it always did.

Verified rather than accepted, on the pair of files the ledger always uses — the sample exported
against the committed parity ICC on a build of `main` and on this one: **8454 bytes both sides**,
**128 differing bytes in 4 runs**, every run inside the XMP `DocumentID`/`InstanceID` (1510..1542,
1588..1620) or the trailer `/ID` (8361..8393, 8396..8428). Zero differing bytes outside them.
`export.sample_bytes` is still 8454 and `export.synthetic_500_page_bytes` is still 1,308,263.

`component_parity`'s three digest sets do not move: its corpus has no index and no mark.

## Acceptance criteria

- [x] **Marked terms collate in the documented order, including the cases byte order gets wrong.**
      `Ale`, `Ålesund`, `apple`, `The Ruined Keep`, `Zebra` — every one of which byte order puts
      somewhere else. Plus ten unit tests on the rule itself: case, diacritics, ligatures,
      punctuation, digits, articles (English *and* French), an entry that is only an article,
      `sort_as`, totality, and a non-Latin script.
- [x] **Page ranges coalesce, including across a folio format change.** Unit-tested directly
      (`vii–viii, 1–2`, and `iv, 5` where the arithmetic *would* have joined) and end to end on a
      document whose section boundary falls inside a term's run of pages, asserting that no printed
      range has one roman end and one arabic one. A restart with no format change is covered too —
      the case a format comparison alone would miss.
- [x] **An index longer than a frame splits with nothing lost.** A 200-term index across pages: one
      entry per term in collated order, the title placed exactly once and re-stated nowhere, nothing
      overrunning a frame.
- [x] **A term marked on the same page twice appears once**, and prints `1`, not `1, 1`.
- [x] **A term's page is its own run's page**, asserted on a paragraph split across a page boundary
      and marked in its last run — and asserted *against* the block's first page, so the defect it
      guards is named rather than implied.
- [x] **It prints the folio, not the page index**, on a document with roman front matter, and it
      follows repagination — asserted from both sides on spec 0072's reasoning.
- [x] **The subset case**, on the glyphs actually drawn: a term spelled in characters no other part
      of the document uses, marked on a **bold** run, with a roman folio and a coalesced range, plus
      the assertion that the *regular* face carries the term.
- [x] **A document with no index lays out and exports exactly as before.** Layout: exactly one pass,
      the derivation skipped outright, and a marked document with no index block laying out
      identically too. Export: `SAMPLE_EXPORT_DIGEST` moves identifier-only, proved byte by byte.
- [x] `FORMAT_VERSION` 9: a committed **v8 fixture** (`crates/core-model/assets/v8-footnote.json`,
      bytes) loads, migrates, marks nothing, keeps its footnote, its cross-reference and its roman
      folios, and re-serializes to a manifest identical to the same document read natively as v9. The
      whole chain v1 → … → v8 migrates, and `FORMAT_VERSION + 1` is still refused by name.
- [x] `benches/budgets.toml`: every entry within budget, `incremental_blocks_measured` still 1,
      `export.sample_bytes` still 8454, `layout.fixpoint_iterations` still **3** against a ceiling
      of 4 — on a workload that now carries an index as well.

## Test strategy

Each behaviour was proved against its own defect by reintroducing it and watching the right tests go
red, then restoring:

| Defect reintroduced | Result |
|---|---|
| terms sorted by byte order instead of by the collation key | `marked_terms_are_collated_rather_than_byte_ordered` fails; nothing else does |
| the folio **format** dropped from the coalescing comparison | `a_range_never_spans_a_folio_format_change` fails — the arabic-only case still passes, which is the point |
| `measure_index` returns an indivisible panel (`split: None`) | `an_index_longer_than_a_frame_splits_and_loses_no_entry` fails |
| the session eviction narrowed back to `Block::Toc` only | `an_index_stays_current_through_the_session` fails: the index is its own title and nothing else, spec 0075's defect verbatim |
| the fixpoint seeded from nothing instead of from the previous pages | same test fails on **iterations: 2 against 1** — a whole extra pass per relayout of every indexed document |
| the collector contributes nothing for a mark | `an_index_is_in_the_subset_and_draws_no_notdef` fails |
| the mark contributed to the **run's** bucket instead of `everywhere` | the same test fails, on the regular-face assertion — which is what establishes the second failure mode as real rather than theorised |
| a mark reports its block's first page rather than its run's page | `a_mark_reports_the_page_its_own_run_landed_on` fails |

One reintroduction **did not fail anything**, and it changed the design rather than being written
around: removing the mark term from the per-pass diff broke no test, because the entries are
re-derived from the content every pass and live in `context_fingerprint`. The term was deleted. See
*The cache*.

The v8 fixture is committed **as bytes**, for spec 0047's reason: a fixture the current serializer
wrote would migrate correctly by construction and prove nothing.

The dependency evaluation was done against the live registry — `cargo info`, and `cargo add` into a
throwaway project **outside the workspace**, per `CLAUDE.md`'s rule about generator dependencies —
rather than from memory, which is how the MSRV mismatch and the transitive-crate counts are facts
rather than recollections.

## Risks

- **The collation is not language-tailored**, and a Swedish or Danish book files `å ä ö` in the
  wrong place. Stated above as a non-goal with its lifting path, and mitigated per entry by
  `sort_as`. The failure is visible to anyone who reads the index, which is the mitigating half.
- **`ignore_leading` is per index block, not per document.** There is one index, so the two are the
  same thing today; a book with two indexes (names and subjects, say) would want them separately
  anyway, which is the direction this already points.
- **A very large index is a large single block.** It splits, so it cannot overflow, but it is
  measured whole on every fixpoint pass in which the context moved. That is what a derived block
  costs and it is one block; a 120-term index in the 500-page bench workload does not move
  `ms_per_page` or the iteration count.
- **A term whose entry is wider than its column is clipped**, term first. The page numbers are the
  part a reader cannot reconstruct, so clipping the term is the right way round — but a book with
  very long inverted headings in a narrow column will lose their tails to an ellipsis.
- **Two marks with the same `term` and different `sort_as` are one entry**, filed by the first
  `sort_as` in document order. An author who has filed a term two ways has contradicted themselves,
  and taking the earlier statement is the only tie-break that does not depend on where in the book
  each mention happened to land.

## Non-goals

- **Sub-entries** (`morale, and routing` under `morale`). The mark gains a field and the entry gains
  a level — exactly the shape a contents entry's level already has — plus a second entry style and a
  collation that orders sub-entries within a term. It is a coherent increment on its own and is not
  smuggled into this one.
- **`see` and `see also` cross-references between index entries.** A different relation: an entry
  that points at another entry rather than at a page, which needs no layout at all but does need the
  mark to be able to say it.
- **Multiple indexes** (names, subjects, places). The block would need a name and the mark would need
  to say which index it belongs to. Nothing in the derivation resists it; there is just no second
  index to build against.
- **A `/Link` from an index entry to its pages.** The plumbing exists (`link_page` →
  `PlacedBlock::Link`) and is per rectangle, so the honest version is one link per number in the
  coalesced list. That is a placement change and belongs with whoever wants clickable index entries
  in the screen profile — the same answer spec 0076 gave for a clickable cross-reference.
- **`quill import` syntax.** Markdown has no spelling for "this stretch is about X", and the
  importer's six-constructs-completely posture says not to invent one — spec 0076's answer for a
  cross-reference, at the same site. Every imported run is unmarked, stated at the site. The `:::index`
  fence is not offered either, since an index block with no marks anywhere would produce an empty
  index and teach the on-ramp a construct that does nothing.
- **Word-by-word filing**, and **numeric-aware ordering** (`10` after `9`). Both are conventions
  rather than corrections; the second is what `sort_as` is for.
- **An index that lists something other than pages** — a section name, a paragraph number. That is a
  different derivation over the same marks, and the entry type is where it would go.
