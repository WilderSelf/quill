# 0080 — The forced page break: a chapter opens a page, and opens it on the right one

**Milestone:** M6 (closeout) · **Status:** implemented

## Why

Three increments have now met the same absence and named it rather than built it.

- **0072** — *"Start this section on the next recto."* Named a non-goal: *"a forced page break, which
  the model has no mechanism for anywhere, and a forward-only rule rather than a fixpoint."*
- **0073** — the same sentence again, because per-section folios are the other half of the same
  feature and the request arrives with them.
- **0079** — the milestone's **principal residual**, and the one with a consequence in a press file:
  a composed book numbers continuously and correctly, its contents list sends a reader to the right
  page, and *chapter 2 nevertheless begins halfway down the page chapter 1 ended on*. Worse, a
  chapter-opener master — the thing spec 0072 exists to place correctly — is applied to a page still
  carrying the previous chapter's tail.

That is one absence, three requests, and 0079's acceptance test states the run-on in as many words.
This increment closes all three, which is why it exists as a closeout rather than as a patch to
0079: the mechanism a chapter opener needs and the mechanism "next recto" needs are the same
mechanism, and building it inside `Book::compose` would have been the first of two implementations.

## What

**A break is a property of a block — anchored to it, not stored inside it — and not a `Block`
variant.**

```rust
pub struct PageBreak { pub before: BlockId, pub kind: BreakKind }

pub enum BreakKind { None, Page, Recto, Verso }   // `Page` is the default

pub struct Document { …, pub breaks: Vec<PageBreak> }
```

### The three candidates, and why this one

`CLAUDE.md`'s rule is that a mechanism only one kind of book can use is a defect, so the primitive
has to be "content resumes on a new page **here**", with a chapter opener and a recto opener as
things an author *says with it* rather than as three features.

**A `Block::PageBreak` variant** — rejected. It is a block with no content that every consumer of a
block acquires an arm for: an id, a measurement, a placement, a fragment rule, a font-subset
contribution, a `plain_text`, a `remap`. Worse, it expresses the wrong relation. A break belongs to
the *seam in front of* a block; a sibling block that happens to sit before the heading breaks the
page in front of whatever moves up when that heading is deleted. The model has already rejected this
shape once, and said so: a section is a **marker anchored to a block**, not a container block
between two others.

**A field on the block** — the honest first answer, and the one CSS gives (`break-before` is a
property of the box). Rejected on a specific technical ground rather than on taste: `MeasureKey`
hashes the block's content, so a `break_before` field on `Block::Body` would put the break inside
the value the measurement cache keys on. **A break changes where a block goes, never what it
measures** — the same distinction spec 0077 drew when it kept a note's text out of `MeasureKey`
("only where the paragraph fits") — so a break edit would re-break a paragraph, at the same width,
to the identical line list. Anchoring makes that *unrepresentable* rather than merely avoided. It
also costs the workspace ~150 struct literals for a field seven of the eight variants would never
carry meaningfully.

**A property of the `Section`** — rejected, and this is the one worth arguing, because 0072/0073
phrase the request as a section's ("start this *section* on the next recto"). A section is a *named*
thing: `Section::name` is what `{section}` prints in a running head, and its `master` governs the
opening page. So expressing "this table starts a page" as a section would force an author to invent
a name for a break, and that name would then print in the running head of every page of the run. A
break has to be sayable about any block without saying anything else, and a section says three other
things. **The section therefore gains no field**: "this section starts on a new page" is a break
anchored to the block the section is already anchored to, which is one statement in one place, and
it is exactly what `Book::compose` synthesises.

**This is spec 0066's finding at a second site**, and it is why the answer came out cleanly: a list
is a property of a paragraph rather than a block type, and a break is a property of a block rather
than a block type. What differs is *where the property is stored*, and quill already has one answer
for authored intent about a block that the block cannot cheaply carry — anchor it by `BlockId`, as
`sections` does and as `footnotes` does.

### The rule, in one sentence

**The flow advances pages until it is at the top of a page the break's kind accepts.**

Everything the increment claims falls out of that sentence and out of what "accepts" means:

- `Page` accepts every page. `Recto` accepts an odd/right-hand page, `Verso` its mirror; both resolve
  through `quill_core_model::is_recto` — **the one place page parity is decided** (spec 0047) — so
  the side a break lands on and the side a margin mirrors to cannot disagree.
- **With `facing_pages` off, every kind accepts every page.** A single-sided document has no spread,
  which is the answer `Margins::left_right` already gives; it is also what bounds the loop, since a
  `Verso` break on a document with no versos would otherwise look for a page that cannot exist.
- `None` asks for nothing. It exists so a **book file** can decline the break its chapters get by
  default, and it is filtered out when the flow's break index is built, so inside the loop "there is
  an entry" and "the flow does something" are the same statement.

### The three properties that make it correct

**1. A break advances a *page*, never a frame.** On a two-column page a break in the first column
does not settle for the second: the page carries the previous chapter's text. The test for this is
written against a mark part way down the **first** column, because a mark in the *last* column of a
page is a fixture that cannot tell the two implementations apart — the next frame and the next page
are the same place there. (That mistake was made first, and the defect-reintroduction pass is what
found it.)

**2. A break at the top of a page the flow has not written to is a no-op.** `frame_idx == 0 &&
frame_empty` is exactly "this page has received nothing" — `frame_empty` is set true on every frame
and page advance and frames are filled in order, so an empty first frame means an empty page.
Without this rule *every* break inserts a spurious blank page, because the first thing a break does
is put the flow at the top of an empty one. It is also what makes a chapter opened **on its own**
cost nothing: page 0 has received nothing, so a chapter carrying its own opener break lays out
exactly as it did before it had one.

This is the `frame_empty` guard's third posture, and unlike spec 0045's and spec 0077's changes it
does not touch the guard itself — the break's emptiness test is a *separate* condition evaluated
before the placement loop, and the guard inside that loop is untouched.

**3. A break belongs to the seam in front of a block, so a continuation must not re-fire it.**
Guarded by `split_at == 0`. The failure this prevents is not a hang: for `Page` a re-fired break at
a page start is a no-op, and for `Recto` it is worse than a hang because it is quiet — a paragraph
cut across pages would skip to the next recto and leave a blank page *inside* a paragraph. It is
reachable only on the **resume** path (mid-pass, `split_at` is updated inside the placement loop,
which the break check sits outside of), which is why the only test that distinguishes it is the flow
contract test below. See "What no test caught until the contract was tested".

### Progress: what bounds a break

Spec 0044's invariants bound a *cut* — a fragment must be non-empty and the absolute item offset
must strictly increase — and spec 0075 turned them into runtime assertions. **A break is a new way
to advance without consuming content, and a blank page inserted for parity consumes none at all**,
so neither invariant covers it and a new bound is owed.

The bound is **two page advances per break**, asserted:

```rust
assert!(advances <= 2,
    "a forced page break must be satisfied within one page advance and at most one blank page, \
     or the flow cannot terminate");
```

The argument: each turn of the loop either (a) advances to a page that has received nothing, making
property 2's test true, or (b) having already done that, flips the page's parity — and parity
alternates page by page, so at most one blank page can ever be inserted. The third possibility, a
kind that no page satisfies, is closed by the facing-pages rule above rather than by the assertion.
And the break is evaluated **once per block**, outside the placement loop, so the flow adds at most
two pages per break-marked block: bounded by the content, exactly as the cut is.

A blank page is therefore the one page in the document that carries no content, and it terminates
because it does not need to consume any: it is not the flow failing to progress, it is the flow
progressing through page space rather than through item space. That distinction is the whole of why
this is safe, and it is why the assertion is on the *advance count* rather than on any offset.

### `FlowState` grows by nothing, and that is the obligation this increment owes

`FlowState`'s doc comment: resuming from it is sound *"only because this is genuinely **all** the
state the loop carries: capture the wrong subset and resumed layout silently diverges from a full
pass."* Spec 0077 added exactly one field and argued for it. **This increment adds none, and the
argument is that the break is a pure function of state already in the checkpoint.**

The decision "does this break fire here" reads: the block's kind (from the document), whether the
page has received anything (`frame_idx`, `frame_empty` — both in `FlowState`), and the page's parity
(`page_index` — likewise). Nothing accumulates.

The case that decides it is the checkpoint of an **inserted blank page**, and it is worth spelling
out because the obvious implementation gets it wrong. A blank page records a checkpoint like any
other, and that checkpoint's `block_idx` is the block whose break is *mid-application*. A rule of
the form "the break has been taken, do not take it again" would have needed a `FlowState` field —
and resuming from a blank page's checkpoint would then have filled the blank page in, producing a
different document after an edit than after a full pass. Making the rule **parity-aware instead of
stateful** removes the need: resuming at a blank verso with a `Recto` break simply finds that the
current page does not satisfy the break, advances one page, and leaves the verso blank — which is
byte for byte what the full pass produced.

**Was there a defect before this? No — there was nothing to have a defect in.** No `FlowState` field
was added and none was needed, and the claim is tested rather than asserted: the contract test
resumes from **every** checkpoint the document records, blank pages included.

### The session

`doc.breaks` joins `context_fingerprint`, beside `doc.sections`, and the reasoning is that entry's
with more force: a break moves every page after it without touching a single block, so a fingerprint
blind to it would call "this chapter now opens recto" *nothing changed* and hand back pages laid out
the old way — spec 0075's defect shape, at a new site. It keeps the same company for the same cost: a
break list is authored, it changes about as often as a master page does, and a whole-document reflow
is the right answer to a change that genuinely repaginates everything after it. It is deliberately
**not** in `MeasureKey` and not in the per-block diff, for the reason above: a break changes where a
block goes, not what it measures.

Nothing derived is cached, so nothing owes spec 0075's eviction. No tail page can be reused across a
break that moved, because a changed context sets `dirty_from = Some(0)` — spec 0074's tail-reuse
lesson, satisfied by the fingerprint rather than by a second rule.

### `Book::compose`, which is the payoff

`BookChapter` gains `break_before: BreakKind`, **defaulting to `Page`**, and `compose` emits one
`PageBreak` per chapter anchored to the same block the chapter's `Section` is. A book states nothing
and reads as a book; `"recto"` gets the opener a printed book usually wants; `"none"` declines it.

A chapter's *own* `breaks` are concatenated and rebased with its ids — beside `Section::start` and
`RunSource`'s targets, and for the same reason: an unrebased anchor would name a block another
chapter now owns. The book's break is appended **after** the chapter's own, so where both name the
chapter's first block the book wins, which is spec 0072's "later authored wins" used deliberately
for the second time in this file.

`BOOK_VERSION` goes to **2**, and `migrate_book` grows the first arm of the chain 0079 left empty.
The rule is the document chain's, applied to the book envelope: a v1 build handed a book that says
`"break_before": "recto"` drops the key as unknown and opens every chapter on whichever side the flow
reached, silently. The migration is a structural no-op — the absent field already means `page`.

## `FORMAT_VERSION`

**10**, decided by the **silence** half of `docs/format-spec.md`'s rule, with the **loss** half
riding with it (the two halves spec 0076 separated and 0077 weighed).

The silence half fires at its plainest. A v9 build meeting a v10 document drops `breaks` as an
unknown key and sets the whole book as one continuous run of text: every chapter opener lands
halfway down a page, a chapter-opener master is applied to a page carrying the previous chapter's
tail, and every recto opener falls on whichever side the flow happened to reach. Nothing is missing,
nothing errors, nothing on the page says so. That is not the *loud* condition spec 0074's entry
turns on — it is its exact opposite.

The loss half fires with it, in 0076's sense: a break is **authored intent** ("this chapter opens
right"), nothing left in the file can regenerate it, and one open-and-save through a v9 build
deletes the structure of the book.

`migrate_9_to_10` is a structural no-op that writes nothing into the object, for `migrate_6_to_7`'s
reason: inserting `"breaks": []` into every document in existence would rewrite its manifest text and
move its exported `/ID` with it.

**`TEMPLATE_VERSION` stays 1**, checked rather than assumed. Trigger 2 fires when a bump changes the
serialized shape of `PageSetup`, `StyleSheet`, `MasterPage` or `PageOverride`; `breaks` is a field on
the document beside those four and changes none of them. Trigger 1 does not fire either: a template
file has no content, therefore no block ids for a break to anchor to — the same argument that keeps a
section out of a template. `PACK_VERSION` stays 1 for the same reasons.

## The font subset

**Nothing new reaches the collector, and that was audited rather than assumed** — 0074 closed the
class structurally, and 0076, 0077 and 0078 each found a new *path* into it.

The question this increment owes is the **blank page**: it carries no content and yet its master
prints a folio, a section name and a running head on it. The answer is that an inserted page is not a
new path, and the reason is structural rather than lucky: `StaticToken::contributes` is deliberately
*not* conditioned on where anything landed ("a section name is in the set whether or not that section
is placed today"), and `folio_formats()` carries each configured format's whole alphabet — so a page
that did not exist when the collector ran draws only characters it had already carried. A break adds
no `RunSource` variant, no `Run` field, no block kind and no style; it draws no characters of its own
at all.

Asserted end to end anyway, on the glyphs actually drawn (`drawn_gids` panics on any `.notdef`): a
document with a recto break is laid out through the real press path, the inserted page is found by
its emptiness, and every character its furniture printed is checked against what the collector
carried.

## The fixpoint

**A break costs no extra pass, and it is not a derived quantity.** It is forward-only — read from the
document, consumed inside the flow, never re-derived from where anything landed — so it does not join
the loop that settles the contents list, the sections, the cross-references and the index. Measured:
`layout.fixpoint_iterations` is **3** and `layout.book_fixpoint_iterations` is **3**, both unmoved,
both against a budget of 4; `layout.book_chapter_ratio` is still **1.0**.

A break in fact makes the section fixpoint *steadier* rather than shakier: spec 0072's oscillation
comes from an anchor sitting between two masters' page capacities, and an anchor at the top of its
own page cannot be pushed off it by the opener's deeper top margin.

## Acceptance criteria

- [x] **A block with a break before it starts a page, and the page it left keeps its content.** Both
      halves, plus the same document without the break asserted to run on — so the defect is in the
      test file rather than described here.
- [x] **A section starts on a new page**, asserted on placed geometry: the chapter opener is the
      first block on its page, sits at the top of the *opener master's* frame (108 pt, against the
      body master's 36 pt), and the page before it holds none of it. The run-on is asserted first, on
      the same fixture.
- [x] **A recto break inserts a blank page when it lands on a verso and does not when it lands on a
      recto** — both directions, because either alone passes against a wrong implementation. Plus the
      verso mirror, so the rule is parity and not right-hand-ness.
- [x] **The blank page carries its furniture and consumes a folio**: it prints the folio its position
      implies, and the whole document's printed folios are `1..=n` with no gap and no repeat — which
      is the assertion that would fail, for every page after it, if an inserted page did not count.
- [x] **A break at the top of an empty page inserts nothing** — the document's first block, and a
      second break arriving at the top of the page the first one just started.
- [x] **Multi-column: a break advances to the next page, not the next column**, over a fixture that
      is *searched for* rather than computed, and whose arrangement is asserted before the claim is.
- [x] **`flow` checkpoint/resume parity**: resuming from **every** checkpoint a document with two
      parity breaks and one cut block records reproduces the full pass. Written against `flow`
      directly, per spec 0077.
- [x] **A block cut across pages does not break again for its own remainder** — its pages are
      consecutive and no blank page appears inside it.
- [x] **A document with no break lays out exactly as before** — page vector *equality* against the
      entry point that carries no breaks, one fixpoint pass, and the same for a dangling anchor and
      for a `None` entry.
- [x] **A book's chapters open pages**, asserted at the composition site (one break per chapter, on
      the block its section anchors to) and end to end (the composed book is exactly its chapters'
      pages, and chapter 2's opening page starts with chapter 2's heading). **Spec 0079's acceptance
      test that stated the run-on is updated**, and it fails if `compose` stops setting the break.
- [x] **`FORMAT_VERSION` 10**: a **committed v9 fixture** (`crates/core-model/assets/v9-index.json`,
      bytes) loads, migrates, has no breaks, keeps its index marks, its footnote, its
      cross-reference and its roman folios, and re-serializes to a manifest identical to the same
      document read natively as v10. `FORMAT_VERSION + 1` is still refused by name.
- [x] **`BOOK_VERSION` 2**: a v1 book file loads, its chapters default to `Page`, and it
      re-serializes without the key.
- [x] Round-trip: a document with breaks saves and loads equal to itself, plus both empty cases
      (spec 0053's lesson) — no `breaks` key at all in a document without one, and a document with no
      blocks carrying a break that anchors to nothing.
- [x] `SAMPLE_EXPORT_DIGEST` moves and is classified **identifier-only** — see Digests.
- [x] `benches/budgets.toml`: every entry within budget, nothing re-baselined.

## Digests

`SAMPLE_EXPORT_DIGEST` moved and is classified **identifier-only**, the fifth time. One candidate
cause, named so the shape is checked rather than pattern-matched: `FORMAT_VERSION` became 10, so
`doc.to_json()` differs by one character and the `/ID` hashed from it moves. Nothing else can reach
the file — the sample states no break, so `breaks` is `skip_serializing_if`-omitted from the manifest
entirely, the flow's break index is empty and answers nothing, no page is inserted, and the flow
takes the single pass it always did. Unlike 0077 and 0078, `StyleSheet::default()` did **not** gain
an entry.

Measured on the pair of files the ledger always uses — the sample exported against the committed
parity ICC on a build of `main` (`743ea77`) and on this one: **8454 bytes both sides**, **128
differing bytes in 4 runs**, every run inside the XMP `DocumentID`/`InstanceID` (1510..1542,
1588..1620) or the trailer `/ID` (8361..8393, 8396..8428). **Zero** differing bytes outside those
regions, so no content stream, font, ICC or metadata stream moved. `component_parity`'s digest sets
did not move.

## Test strategy

Each behaviour was proved against its own defect by reintroducing it, watching the right tests go
red, and restoring:

| Defect reintroduced | Result |
|---|---|
| the empty-page no-op removed (a break always advances at least one page) | **2** tests fail: `a_break_at_the_top_of_an_empty_page_inserts_nothing` and the resume-parity contract test |
| the break advances a **frame** when one is left, rather than a page | `a_break_advances_to_the_next_page_not_the_next_column` fails — and only after the fixture was corrected; see below |
| parity ignored (`Recto`/`Verso` accept every page) | **6** tests fail across three crates: both recto tests, the blank-page furniture test, the parity unit test, the collector's inserted-page test, and the resume-parity contract test |
| an inserted page records no checkpoint | the resume-parity contract test fails; every session test stays green |
| `doc.breaks` dropped from `context_fingerprint` | `changing_a_break_re_lays_the_document` fails: the session hands back the pages laid out the old way |
| the `split_at == 0` continuation guard removed | the resume-parity contract test fails — **and nothing else, at any point**; see below |
| `compose` does not rebase a chapter's breaks | `a_chapters_own_breaks_are_rebased_with_its_blocks` fails |
| `compose` adds no break per chapter | **3** tests fail, including spec 0079's updated acceptance test — which is what makes "the run-on is closed" checkable rather than claimed |

### What no test caught until the contract was tested

Two findings, both from the reintroduction pass rather than from writing the code:

**The multi-column fixture was wrong in a way that passed.** The first version put the mark part way
down the **second** column of a two-column page — which reads like the harder case and is in fact the
one case that cannot distinguish a frame-advance from a page-advance, because in the last column of a
page the next frame *is* the next page. It passed against a deliberately frame-advancing
implementation. The fixture now searches for a filler count that puts the mark part way down the
**first** column and asserts that arrangement before asserting the claim.

**The `split_at == 0` guard is invisible to every test but the contract one.** Within a single pass
the break is evaluated once per block, outside the placement loop, so `split_at` is only non-zero
there on a **resume** into a mid-block checkpoint — which the session never has to choose. Removing
the guard left the entire suite green, including a dedicated test for a cut block with a parity
break, until the resume-parity fixture was extended to include a block tall enough to be cut *and*
carrying a break. That is spec 0077's lesson repeating exactly: *the contract is that resuming from
any checkpoint reproduces a full pass, so the test is written against the contract rather than
against the chooser.*

The v9 fixture is committed **as bytes** for spec 0047's reason: a fixture the current serializer
wrote would migrate correctly by construction and prove nothing.

## Risks

- **A break can make a page emptier than an author expects.** A break part way down a page leaves the
  rest of that page unset, which is what a break *is*; but combined with a parity break it can leave
  a whole blank page, and a book that sets `recto` on every chapter of a 40-chapter volume may add up
  to 40 pages. That is a typographic decision the author made, it is visible immediately, and it is
  what printed books do — but it is a page count they should know about, and quill does not warn.
- **A blank page is blank of content, not of furniture.** It prints its folio and its running head,
  which is correct for a book's own numbering and is *not* what a designer means by a "deliberately
  blank" page in front matter. Suppressing furniture on an inserted page is a master-assignment
  question and is a named non-goal below.
- **A break inside a keep-together composite's frame is untested territory in one respect**: a break
  before a composite behaves exactly as a break before a paragraph (the composite is then placed into
  an empty frame, where the `frame_empty` guard's existing posture governs), but a document whose
  break lands on a page whose band is already full by a carried footnote will place the block
  against 0077's rules rather than this increment's. Both mechanisms are bounded independently; their
  interaction is bounded because each is.
- **A book with many chapters now measures a slightly different page count** than the same book did
  under 0079 — 203 pages at five chapters and 204 at ten, against ~200 before. That is the feature,
  and `book_ms_per_page` moved from 0.231 to 0.225 (the same work over more pages), so no budget was
  re-baselined.

## Non-goals

- **A break *after* a block.** Expressible as a break before the next one, and a second spelling of
  the same page boundary is a second thing to keep consistent. What it cannot express is a break
  after the *last* block, which is a trailing blank page and is a page-count decision rather than a
  flow one.
- **Column and frame breaks.** "Start the next column" is a real request and it is a different
  mechanism: it advances a frame, so it needs no parity, no blank page and no page-count reasoning,
  and it belongs with whoever wants it. This increment's whole claim is that a *page* break advances
  a page.
- **Suppressing furniture on an inserted blank page.** A truly blank page — no folio, no running head
  — is a master, and `Section::master` already assigns one to a page. What is missing is a way to
  name a master for a page that is not a section's opener, which is the "master for a section's
  continuation pages" question spec 0072 left open.
- **Keep-with-next, widow/orphan control beyond `MIN_ITEMS_PER_FRAGMENT`, and "do not break inside
  this block".** All three are the *opposite* mechanism — constraints on where the flow may break,
  rather than a place it must — and they belong to the fragmentation policy spec 0044 owns. A
  composite's `prefers_keep_together` is the one that already exists.
- **A break on a footnote.** A note is placed in a band by the frame its anchor lands in; "start this
  note on a new page" is not a thing the band model can express, and would mean moving the anchor.
- **`quill import` syntax for a break.** Markdown has no spelling for one, and the importer's
  six-constructs-completely posture says a construct arrives whole or not at all. A `.qbook`'s
  `break_before` is where a book says it today.
- **Balancing columns before a break.** A break in the first of two columns leaves the second empty,
  which is correct — the page is being left deliberately. Balancing the *last* page of a chapter is a
  separate typographic feature and needs a second pass over a page the flow has finished with.
