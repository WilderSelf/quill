# 0076 — Cross-references: "see page 42", and where a derived value may not live

**Milestone:** M6 · **Status:** implemented

## Why

A book says "see page 42", and the page it means moves every time anything above it grows. Quill had
the plumbing for the *link* since spec 0044 — `link_page` → `PlacedBlock::Link` → `/Link`, already
gated for PDF/X legality and already ink-accurate after 0069 — and no way at all to print the
*number*.

The M6 audit priced this wrongly in the roadmap and corrected itself: "cross-references are cheap
once sections exist" is true about the dependency *shape* and false about the cost. The shape is the
contents list's — a value derived from where content landed, fed back into the layout that produced
it. The cost is not, and the whole increment is that difference.

It is sequenced before footnotes because a footnote number that restarts per page is the same
"derived from position, therefore context and not content" problem, and the riskiest increment in the
milestone should inherit a settled answer rather than invent one. It is also where the roadmap
assigns the fixpoint-iteration budget, because it is where the second derived quantity arrives.

## What

### A cross-reference is a run whose characters are generated at layout time

```rust
pub struct Run { pub text: String, pub style: InlineStyle, pub character: Option<String>,
                 pub source: RunSource }

pub enum RunSource { Authored, Reference { target: BlockId } }
```

`source` is defaulted and `skip_serializing_if`-omitted, so an authored run's manifest text does not
move — which matters more than usual here, because the `/ID` is a hash of that text and every
document in existence is made of authored runs.

**The target is any `BlockId`, not a heading's.** Nothing about the mechanism is heading-specific:
`section_starts` already anchors to every placed variant that carries an identity, and the derivation
here reuses that walk. "See the table on page 42" is the same sentence as "see the chapter on page
42", and a heading-only rule would be a structure-shaped restriction on a mechanism that is general
for free — the test `CLAUDE.md` applies to every new type and field.

**It prints the folio, not the page index.** Spec 0073's split, and this falls on the reader's side
of it: a reference that says `4` for a page printed `iv` sends them to a page that does not answer to
the number they were given. `/Link` destinations still carry the index, because a destination is
resolved positionally by a viewer.

**`Run::text` is not a cached page number.** It holds the unresolved marker and is never read while
laying out. Spec 0041's rule for a contents list, applied to a run: a stored number is stale the
moment anything is edited, and one that was right an edit ago is worse than none.

### The cache: `MeasureKey`, not `context_fingerprint`

**This is the increment.** Everything else in it is wiring.

A resolved reference is derived from where its target landed, so it is *context* rather than content
— the same category as a list marker and the contents list's heading index. The workspace has two
homes for context and they are not interchangeable:

- **`context_fingerprint`** is where the resolved heading index lives. A change to it sets
  `dirty_from = Some(0)` — a whole-document reflow — and, because it is outside `MeasureKey`, it
  needs spec 0075's eviction of every block derived from it to stay *correct*. A contents list can
  afford both: there is one of it, and the eviction is one block.
- **`MeasureKey`** is where spec 0066 put a list marker, and its doc comment states the rule this
  increment generalises: a marker is derived from the blocks *before* it, so it is context rather
  than content, and "without this in the key, a renumber would serve stale ordinals from the cache."

A book has **hundreds** of cross-references scattered through it, and *any* edit that moves *any*
page changes the derivation. Through `context_fingerprint` that is a whole-document reflow on every
keystroke, plus an eviction that re-measures every referring block in the book — including all the
ones whose number did not change, because the fingerprint is a statement about the document rather
than about a block. `benches/budgets.toml`'s `incremental_blocks_measured` — "editing one paragraph
must not re-break the document" — is the line that says that must not happen.

So `MeasureKey` gains `references: u64`, and it is spec 0066's `marker` at scale:

```rust
struct MeasureKey { block, content, width_bits, marker, references, style }
```

- `reference_fingerprint(block, map)` hashes the resolved value of each of that block's reference
  runs, in run order, and returns a **distinguished `0`** for a block that has none. Every block in
  every document without the feature therefore keeps the exact key it had, and its cached
  measurement is untouched.
- It owes **no eviction**, which is spec 0075's lesson checked rather than assumed: a changed
  derivation produces a *different key* rather than a cache entry that has to be found and dropped.
  That is the same argument spec 0072 made for frame widths.
- The per-pass diff is over `(id, content ^ references)`. Without the second term a pass whose
  references moved but whose text did not would see "nothing changed" and hand back the previous
  pages with the previous numbers on them, for ever — spec 0075's defect in the one place this
  increment could reintroduce it.
- Which block a reference *points at* is authored, so it joins `content_fingerprint`: re-pointing a
  reference is an edit, and must mark the block dirty even when the two targets happen to sit on the
  same page today.

**Measured, not argued.** On a 400-block document with references to two targets — one above a
mid-document edit and one below it — an edit that moves the lower target by more than a page
re-measures **4** blocks: the edited paragraph and the three whose reference text changed. The two
whose target did not move keep their cached measurements. Routing the same derivation through
`context_fingerprint` (with the eviction it then requires) measures **6** and reuses **0** pages;
the difference is the two references that did not need to change, and it is proportional to the
book's reference count rather than to the edit.

### The session seeds the loop from its previous pages

Spec 0072 recorded that both existing fixpoints restart from the underived state on every relayout —
an empty heading index, an underived assignment — so a sectioned document's first pass is always
thrown away, and named it a cost sections *share* rather than introduce. A cross-reference cannot
leave that untaken. Starting from an empty map prints `[?]` on the first pass of every relayout and
the real folio on the second, so **every relayout of every referring document costs a second
whole-document pass**, and a cold one costs a second measurement of every referring block as well —
the exact cost this increment exists to avoid, arriving through the loop instead of through the key.

`LayoutSession::relayout` therefore derives the reference map from the pages it already has. It is
sound because it only moves the loop's starting point: the exit condition still compares two
consecutive derivations, and the map that ships is always derived from the pages that ship.

`LayoutStats::blocks_measured` and `blocks_from_cache` now **accumulate across the fixpoint's
passes**, where the page counts stay the final pass's. The cost of a relayout is the sum of its
passes, and reporting only the last one would report a converged pass that measured nothing and call
it the price of the edit. Every single-pass document — which is every entry in the perf harness — is
unaffected.

### A bounded fixpoint that really can oscillate

A cross-reference is the first derived quantity that is genuinely *in the flow*. Spec 0073 was
careful that a folio consumes no flow space and so cannot move a line break; spec 0074 measured that
a running head costs zero extra passes for the same reason. A cross-reference's rendered number is
**text in a paragraph** — "see page 142" is three digits where "see page 42" is two — so its width
moves a line break, which moves a page, which moves the number.

It joins the one shared loop rather than getting its own, on spec 0072's arithmetic: nesting
multiplies passes where sharing adds to them, and the quantities are not independent. A referring
document costs exactly **one** extra pass to settle, which is the same single resolving pass 0073
charged for a section and for the same reason — a target has to be *placed* before its page is known.

**And it oscillates, provably.** Spec 0075 could construct no oscillating contents fixture because an
entry's height does not depend on its number, so there was no free variable; this has exactly that
variable. The fixture is in the test file and its construction is the point:

- page capacity is 54 lines, and the target block is the document's 431st line when the referring
  paragraph sets in one line and its 432nd when it sets in two — so it sits astride the 7/8 page
  boundary and the paragraph's own width decides which side it falls on;
- the paragraph's last word is the reference plus a full stop, with exactly four columns left for it;
- the pages are numbered **lower-roman**, and that is the load-bearing choice. Roman width is not
  monotone in the page number, so page 8 writes `viii` (four columns, does not fit, the paragraph
  takes a second line) and page 9 writes `ix` (two columns, fits, the paragraph takes one). Decimal
  cannot oscillate this way: a wider reference pushes its target later, and a later page has a wider
  number, which only ever grows.

It reports `converged: false` at exactly `FIXPOINT_MAX_ITERATIONS`, ships a complete document with
nothing missing, and the test asserts the *honest* residual — the last iterate prints `ix` while the
target is in fact on the page printed `viii`, which is precisely what the flag is telling the caller.

### An unresolvable reference is visible, not silent

A reference whose target is not in the document prints `UNRESOLVED_REFERENCE` — `[?]`.

The reasoning is `CLAUDE.md`'s "prefer a visible failure over silent press-corruption", and it lands
differently from spec 0072's dangling section anchor, which is deliberately *not* an error. That
posture is right for **furniture**: losing a chapter opener's master is recoverable and visible by
comparison across pages, while refusing to open a whole book because one anchor went stale is
neither. A cross-reference is **content, in the text flow**, so the two silent options are both worse:

- rendering nothing leaves "see page ." in a sentence that reads as finished, and nothing anywhere
  says otherwise;
- refusing the document loses the book to one stale id, which is the outcome 0072 already rejected —
  and it is not press protection either, because the wrong page number never reaches the press: a
  proof shows `[?]`.

The marker is three characters *generated at layout time*, so it is subject to exactly the hazard
below and `RunSource::contributes` carries it. A marker that renders as three `.notdef` boxes would
be a less legible failure than the one it replaces.

### The font subset: a new path, so a new instance of the class — not a special case

**This is the part that could quietly have been a special case, and the increment's second structural
claim.**

Spec 0074 closed a class rather than an instance: the font-subset collector runs *before* layout, so
every character a layout-time token can become has to be predicted, and a character it misses is a
`.notdef` box in a press file with no error anywhere. Its answer was `StaticToken` — one enum, one
parser, two exhaustive matches — so that adding a token without teaching the collector does not
compile (`E0004`).

A cross-reference is a **new path**. `StaticToken` is a token parsed out of a *master static's text
string*, resolved once per page; a cross-reference is a typed field on a *body run*, resolved per
instance, and it reaches the collector through the `Heading`/`Body` arm rather than through the
master-page walk. It is therefore a second instance of the class 0074 closed, and it gets the same
structural treatment rather than a hand-written `if`:

1. **`RunSource::contributes(&Document) -> TokenText` is an exhaustive `match`**, and it lives in
   `core-model` beside the enum, so a new variant cannot exist anywhere in the workspace without
   saying what characters it can draw.
2. **The resolver — `resolve_run_texts` — matches exhaustively too**, so a new variant does not
   compile there either.
3. **`TokenText` is the shared currency**, so the collector has one way to absorb a contribution and
   two producers that each have to be exhaustive in order to build.
4. **There is one answer to "what can a folio draw"**: `Document::folio_formats()`, which
   `StaticToken::Page` already asks. A cross-reference does not get a second one. This is asserted —
   the two contributions are compared for equality in a test.

What `Reference` contributes is the alphabet of every configured folio format (arithmetic, so only
the alphabet is knowable) plus the unresolved marker as a **run** (authored text, knowable exactly,
so its ligatures are cut in like any other run's). `Authored` contributes nothing, which is what
keeps every existing document's subset — and therefore its exported bytes — exactly where it was.

As with 0074, the compiler covers the structural half and cannot cover the semantic one: an arm that
returns the *wrong* characters would build. That half is covered end to end, by laying a document out
through the real press path and asserting every character a reference actually printed was collected.

### A latent instance of the same class, found and fixed here

Auditing every site that draws layout-time characters turned one up. Spec 0073 made a **contents
entry** print the folio rather than the page index, and the collector's contribution for a
`Block::Toc` still said `'0'..='9'` — the whole answer only while every folio was arabic. A book with
roman front matter and a contents list, but no `{page}` static anywhere, would have set every roman
numeral in its own contents list as a `.notdef` box.

Reproduced with a failing test before it was fixed, and fixed by property 3 above rather than by a
second alphabet: the contents entry now asks `StaticToken::Page`. Because `folio_formats()` always
includes `Decimal`, a document that states no folio format carries exactly the digits it always did
and no digest moves.

## `FORMAT_VERSION`

**7**, and this is the case where the two halves of `docs/format-spec.md`'s rule split — which is
why it is argued rather than pattern-matched off 0072 and 0073.

The rule's first half turns on **silence**, and it does not fire. A v6 build meeting a cross-reference
drops `source` as an unknown key and prints the run's stored `text`, which is `[?]`: the book is
visibly unfinished wherever a reference was, on screen, in the press file and in a proof. That is
spec 0074's condition for the rule *not* firing, met exactly.

What fires it is the half 0074's argument also rested on, and which is false here. 0074 added nothing
to the model — `{section}` is characters inside a string a user can type in any build, so no version
gate could have stopped them arriving, and a bump would have stated a compatibility fact that was not
true. `source` **is** model. It is authored intent — which block the author pointed at — it cannot be
regenerated from anything left in the file, and a v6 build that opens the document and saves it
destroys every reference in it permanently. `docs/format-spec.md` states that half in as many words:
a half-understood document can be saved back over the original, and the loss is worse for authored
intent than for derived state. Loudness mitigates the proof; it does not undo the deletion.

`migrate_6_to_7` is a structural no-op and writes nothing into the object, for `migrate_5_to_6`'s
reason: inserting `"source": {"kind": "authored"}` into every run of every paragraph would rewrite
the manifest text of every document in existence and move its exported `/ID` with it.

**`TEMPLATE_VERSION` stays 1**, and the check is not a formality. Trigger 2 fires when a
`FORMAT_VERSION` bump changes the serialized shape of `PageSetup`, `StyleSheet`, `MasterPage` or
`PageOverride` — the four structures a template file embeds. A `Run` is in none of them: a master
static's text is a `String`, not runs. Trigger 1 does not fire either, because a template file has no
content, therefore no block ids to refer to — the same argument 0072 made for sections.

## The fixpoint-iteration budget

`benches/budgets.toml` gains `layout.fixpoint_iterations = 4`, fed by `crates/testdoc/benches/layout.rs`.

The gap the M6 audit found: `FIXPOINT_MAX_ITERATIONS` *bounds* the loop and nothing *measured* it,
and each derived quantity a document carries can add passes — a pass being a whole-document flow, so
this multiplies the single most expensive thing the engine does. `layout.scaling_ratio`, "the single
most valuable line in the file", cannot catch it: that measures the shape of one pass, and the growth
is in the number of passes.

The workload is the worst one the engine can currently be handed — the 500-page synthetic document
with all three derived quantities at once: a generated contents list, two sections with folio
formats, and forty cross-references. It measures **3** (place; resolve what the first pass could not
know; agree) and is pinned at 4, one pass of headroom, on the file's stated posture of a blowup
detector rather than a micro-regression detector.

**Checked with `check_exact`, not against `tolerance_factor`, and that is the decision worth stating.**
An iteration count is a deterministic work counter — the same document takes the same passes on
every machine — so the runner variance the tolerance exists for does not apply. It would also be
actively harmful: doubling a ceiling of 4 puts the limit at 8, which *is* `FIXPOINT_MAX_ITERATIONS`,
so the gate could only fire on a document that already reports `converged: false` — and reports it
loudly, needing no budget line. A budget whose limit is unreachable is not a budget, which is spec
0051's lesson and the reason the export-size entries are counters too.

## Acceptance criteria

- [x] **A reference survives repagination.** Content inserted before the target moves it, and the
      printed number follows — asserted from both sides on spec 0072's reasoning: a test checking
      only the new number would pass against an implementation that printed the document's last page,
      and one checking only that it changed would pass against one that printed anything at all.
- [x] It prints the **folio**, not the index — asserted on a document with roman front matter, where
      the two visibly differ.
- [x] Any block may be a target: the same assertion over a `Block::Table`.
- [x] **The perf claim, as a counter.** An edit that moves pages re-measures the edited block plus
      exactly the blocks whose reference text changed — 4 of 400 — and neither the references whose
      target did not move nor the document. Pages above the edit are reused.
- [x] **The oscillating case reports `converged: false`** at exactly `FIXPOINT_MAX_ITERATIONS`, ships
      a complete document, and is honestly one out. The session reports the same, over the same
      fixture.
- [x] **An unresolvable reference prints `[?]`**, converges, drops nothing, and is asserted against
      the string literal rather than against the constant — a test written as
      `format!("…{UNRESOLVED_REFERENCE}…")` is a formula checked against itself and would pass just
      as happily if the marker became the empty string.
- [x] **The subset case**, on the glyphs actually drawn (`drawn_gids` panics on any `.notdef`): a
      roman folio reached through a *reference* — with no `{page}` static anywhere, so only the new
      path can carry it — and the unresolved marker. Plus the end-to-end half: every character the
      resolver printed was collected.
- [x] **A document with no cross-reference lays out and exports exactly as before.** Layout: exactly
      one pass, `reference_targets` empty so the derivation is skipped outright, and every block's
      `MeasureKey` unchanged by construction (`reference_fingerprint` returns a distinguished `0`).
      Export: `SAMPLE_EXPORT_DIGEST` moves **identifier-only** — 8454 bytes both sides, 128 differing
      bytes in 4 runs, every one inside the XMP `DocumentID`/`InstanceID` or the trailer `/ID` —
      because `FORMAT_VERSION` is hashed into the id and nothing else moved. `export.sample_bytes` is
      still 8454.
- [x] Round-trip: a document with a reference saves and loads equal to itself, including one whose
      target is not in the document. Both empty cases (spec 0053's lesson): an authored run must not
      write a `source` key, and the sample's manifest must not contain the string.
- [x] `FORMAT_VERSION` 7: a **committed v6 fixture** (`crates/core-model/assets/v6-folio.json`,
      bytes) loads, migrates, has no reference, keeps its roman folios, and re-serializes to a
      manifest identical to the same document read natively as v7. The whole chain v1 → v2 → v4 → v5
      → v6 migrates, and `FORMAT_VERSION + 1` is still refused by name.
- [x] `benches/budgets.toml`: every entry within budget, `incremental_blocks_measured` still 1,
      `export.sample_bytes` still 8454, `layout.fixpoint_iterations` 3 against a ceiling of 4.

## Test strategy

The design decision first, because it is the one a test can be written *against*: the counter test
uses references to **two** targets, one above a mid-document edit and one below it. On a document
with a single target the keyed design and the context-fingerprint design are indistinguishable — both
re-measure the same blocks — so a one-target fixture would have asserted nothing about the choice.

Each behaviour was then proved against its own defect by reintroducing it and watching the right
tests go red, then restoring:

| Defect reintroduced | Result |
|---|---|
| the resolved value routed through `context_fingerprint` (with the eviction it then needs) instead of `MeasureKey` — i.e. the contents list's treatment | `an_edit_that_moves_a_page_re_measures_only_the_blocks_whose_reference_moved` fails: **6 blocks measured against 4**, and `pages_reused: 0` against 8. Nothing else fails, which is the point — the wrong design is *correct*, just proportional to the book |
| the reference term dropped from the per-pass diff | **4 session tests** fail; the session prints `[?]` for ever, having decided nothing changed. Spec 0075's shape exactly |
| the fixpoint restarted from the underived state instead of being seeded from the previous pages | `a_relayout_of_an_unchanged_referring_document_measures_nothing` fails on **iterations: 2 against 1** — a whole extra full-document pass per relayout. Caught by the iteration count rather than the counter, because a warm cache holds both states |
| the collector contributes nothing for a reference run | `a_cross_reference_is_in_the_subset_and_draws_no_notdef` fails on `'v'`; every other test passes |
| `UNRESOLVED_REFERENCE` becomes the empty string — the silent option | 3 tests fail, including the marker test, which is why it is written against the literal |
| the contents-entry folio alphabet (the latent defect above) | `a_contents_entry_carries_the_folio_alphabet_it_will_print` fails on `'v'` — written **before** the fix, which is how the defect was established as real rather than theorised |

The "run source added to the resolver alone" case is deliberately **not** a runtime test, on spec
0074's precedent: it is not a runtime state. A new `RunSource` variant fails to compile at
`RunSource::contributes` before any resolver sees it.

The v6 fixture is committed **as bytes** for spec 0047's reason: a fixture the current serializer
wrote would migrate correctly by construction and prove nothing.

## Risks

- **The oscillating fixture is tuned, and a change to the default metrics or page capacity could
  make it settle.** It would then stop testing the bound while still passing every other assertion,
  so the test asserts `!converged` first and says in as many words that a settling fixture has
  stopped testing anything. The construction is written out above and in the fixture's own comment so
  it can be re-tuned rather than re-derived.
- **`blocks_measured` now accumulates across fixpoint passes.** A truer number, and it changes what
  every multi-pass session test reports. Single-pass documents — the whole perf harness — are
  unaffected, and the entries in `benches/budgets.toml` did not move.
- **Seeding the loop from the previous pages means the first pass of a relayout can be laid out
  against a stale map.** Bounded by the same fixpoint that bounds everything else here: the exit
  condition compares two consecutive derivations, so a stale seed costs an iteration at worst and
  cannot survive into what ships.
- **A reference inside a heading reaches a running head as `[?]`.** `Block::plain_text` flattens runs
  without resolving them, and the heading index is built from it — the same site as spec 0074's named
  residual about inline emphasis, and it is left with it rather than half-fixed. A cross-reference in
  a chapter title is not a thing anyone authors, and fixing it means the heading index carrying
  resolved runs, which changes what a contents entry measures and what the PDF outline reports.
- **A reference to a block that is placed but never *drawn*** — an image whose asset is missing, say —
  resolves to nothing and prints the marker, because the derivation reads the placed page vector.
  That is the same answer a deleted target gets, and it is the right one: the reader cannot turn to a
  page that has nothing on it.

## Non-goals

- **A cross-reference that prints anything but a page number.** "See *The Ruined Keep* on page 42"
  needs the target's text, which is `Block::plain_text` for two variants and undefined for the rest,
  and it would want the same run-carrying heading index the residual above describes. The enum is the
  place a second kind goes, and it will not compile until the collector is taught about it.
- **`quill import` syntax.** Markdown has no cross-reference spelling worth committing to, and the
  importer's six-constructs-completely posture says not to invent one. Every imported run is
  `Authored`, stated at the site.
- **A `/Link` from the reference to its target.** The plumbing exists (`link_page` →
  `PlacedBlock::Link`) and the destination would be the target's page *index*, not its folio — spec
  0073's other side. It is a placement change rather than a model one and belongs with whoever wants
  clickable cross-references in the screen profile.
- **Numbered figures, tables and equations** ("see figure 3.2"). That is a counter derived from
  document order, which is spec 0066's `list_markers` with a different reset rule, plus a caption
  model neither exists yet.
- **A reference to a *range*** ("see pages 42–47"), which needs the target's last page as well as its
  first and is the same page-range coalescing spec 0078 owes the index.
