# 0072 — The section: an anchor the model does not have; `FORMAT_VERSION` 5

**Milestone:** M6 · **Status:** implemented

## Why

`PageOverride`'s doc comment has named this gap since M2, in as many words: *"anchoring a master to
the chapter it opens needs a notion of 'section' the model does not have"*. Spec 0035 repeats it, and
the roadmap has carried it as an open question asked of M3 and deferred through M4 and M5.

The consequence is a defect that ships today. `pages[i]` addresses page `i`, so a document whose
chapter opener is assigned positionally has that assignment slide off its chapter the moment anything
above it grows by a page — and nothing complains, because a master assignment cannot be wrong, only
somewhere else. The generated contents list (spec 0041) is the feature that triggers it routinely:
adding a chapter lengthens the list, which moves every chapter.

Four of M6's six features are downstream of a section — a folio format and a restart are *per
section* by definition (0073), `{section}` in a running head is its name (0074), and a book's chapters
are sections with a page offset (0079) — which is why this is the milestone's first increment.

## What

**A section is anchored to a `BlockId`, and it *generates* the per-page assignment rather than
replacing it.**

```rust
pub struct Section {
    pub name: String,
    pub start: BlockId,
    pub master: Option<String>,
}

pub struct Document {
    // ...
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub sections: Vec<Section>,
}
```

`Document::master_for(page_index)` resolves `self.pages.get(page_index)`. A section anchored to a
block yields a derived page index from the final page vector — exactly `heading_index_of`'s output,
generalised to any block — and `Document::page_assignment(&starts)` synthesises the same
`Vec<PageOverride>` from it each pass. So `DocumentTemplate::frames`, `statics` and
`Document::master_for` are all **unchanged**, and the whole increment is one derivation plus the loop
that runs it.

That answers the open question, and the answer is *neither of the two candidates*: index assignment
is the right **representation** and the wrong **authoring surface**. They were never a fork.

### What a section is, and what it deliberately is not

- **A marker, not a container.** It does not own its blocks and there is no tree; a section runs
  until the next section's anchor. Making it a container would change every consumer of
  `Document::content` for a nesting nobody needs yet, and a part-inside-a-chapter is expressible as
  two sections with adjacent anchors.
- **`name` is authored, not derived** from the anchor's text. A running head says "The Ruined Keep"
  where the opener says "Chapter One: The Ruined Keep". Spec 0074 is what prints it; it is here
  because a section without a name is not a section, and adding the field later would mean a second
  bump.
- **`master` governs the opening page only.** A chapter opener has a deeper top margin and no folio,
  and page two of the chapter is ordinary body. A master for the *rest* of a section would have to
  lose to the next section's opener, which is a different rule; left to whoever needs it.
- **An anchor naming no block is not an error.** The section is not placed and contributes no
  assignment — the posture a dangling master name already has (`Document::master_for`) and a dangling
  style name (`StyleSheet::resolve`). Losing furniture beats refusing the document.
- **A section wins over a positional entry for the same page.** The only precedence decision here.
  They disagree only in a document carrying both, and the positional entry is by construction the
  stale statement of the same intent — it was written when the chapter opened on that page and did
  not move when the chapter did.

### `FORMAT_VERSION` 5, and why the bump is owed

`docs/format-spec.md`'s rule: a bump is warranted whenever an **older** build would open the document
and silently lay it out wrongly, even when the new fields are optional. Judged against this
increment, and only against this increment — 0073's roman folios are 0073's argument to make:

A pre-0072 build reading a v5 document drops `sections` as an unknown key. Every chapter opener then
resolves to `default_master`, so a book whose openers have a 216 pt top margin and no folio sets every
one of them as body — a *different book*, produced quietly, with no error anywhere. It can then be
saved back over the original, and what is lost is **authored intent**, not derived state: spec 0035
drew exactly that distinction when it declined to bump for `pages` (regenerable ids were the
precedent it had; a page list was not), and a section list is on the losing side of it. This is the
same class as spec 0047's verso gutter, which bumped.

The M2 rule that an increment which cannot stay additive **says so in its spec and bumps on its own**
is why this is stated here rather than smuggled into a later increment.

`migrate_4_to_5` defaults `sections` to empty — structurally a no-op, since a v4 document had no
sections and every assignment it had was positional — and is written out anyway, on the chain's
standing convention that it is the record of what each version changed. The written-out default does
not survive back into the manifest (`skip_serializing_if`), so a migrated document re-serializes
identically and its exported `/ID` does not move.

**`TEMPLATE_VERSION` stays 1**, and the check is not a formality. Trigger 2 fires when a
`FORMAT_VERSION` bump changes the serialized shape of `PageSetup`, `StyleSheet`, `MasterPage` or
`PageOverride` — the four structures a template file embeds. This bump changes none of them: it adds
a field beside them. Trigger 1 does not fire either, because a template file **cannot carry a
section**: it has no content, therefore no block ids to anchor to. `Document::from_template` says so
where a reader will hit it.

### Master assignment joins the fixpoint — the real cost

`frames_for(template, page_index)` is read *inside* the flow loop. A section's opener master changes
that page's margins and column count, which changes where later content falls, which can move the
anchor — so the assignment cannot be resolved before layout, only *with* it.

**It shares the contents list's loop rather than getting its own**, and the reason is arithmetic. A
pass is a whole-document flow; nesting the two loops would multiply the pass count where sharing adds
to it, which is precisely the growth the M6 audit found nothing measures. They are also not
independent — a section's opener changes the page count, which changes the numbers the contents list
prints — so a nested loop would settle the inner quantity against a stale outer one and re-run it
anyway. The loop is now:

```
lay out → re-derive (heading index, section starts) → stop when neither moved
```

The shared bound is `FIXPOINT_MAX_ITERATIONS = 8` (renamed from `TOC_MAX_ITERATIONS`, with
`TocStatus` → `FixpointStatus` and `LayoutResult::toc` → `fixpoint`, because a status called `toc`
would now be wrong at half its call sites). Non-convergence is **reported, not hidden**: the last
iterate is a complete document with nothing missing, whose chapter opener may be on the wrong page,
and `converged: false` says so.

Sections oscillate far more readily than a contents list does, and the mechanism is worth stating
because the test depends on it: the opener's deep top margin shrinks the page the chapter opens on,
which pushes the anchor onto the *next* page, where the master then applies and gives the previous
page its capacity back. An anchor sitting between the two masters' page capacities never settles.
Spec 0075 could not construct an oscillating contents fixture at all; this one is three lines.

The derivation lives on the template, behind two defaulted `PageTemplate` methods —
`reassign(&self, pages) -> bool` and `derived_fingerprint(&self) -> u64`. A template that derives
nothing answers `false` and `0`, so **a document with neither sections nor a contents list still
takes exactly one pass**, and a settling sectioned document costs exactly one extra. `DocumentTemplate`
holds its derived state in a `RefCell` because the flow loop only ever has `&template`; the
alternative was `&mut` at forty call sites for a capability one implementation has.

### The measurement cache, and a defect this found

`doc.sections` joins `context_fingerprint`, which is `Debug`-derived precisely so a new field cannot
be missed. `derived_fingerprint()` joins it too, and that one is load-bearing rather than tidy: the
section-driven assignment is **not on the document**, so a fingerprint over the document alone would
call a pass that moved a chapter opener "nothing changed" and hand back the previous iterate's pages
— spec 0075's defect in the one place it could recur.

A changed context sets `dirty_from = Some(0)`, a whole-document reflow. That is accepted deliberately
here: a section list is authored and changes about as often as a master page does, which is the
company it keeps in that fingerprint. It is *not* the answer for spec 0076, where a book has hundreds
of cross-references and the same treatment would blow `incremental_blocks_measured` immediately.

Nothing derived is cached, so nothing owes spec 0075's eviction: a section changes frame *widths*,
which are already in `MeasureKey`, so a re-measure happens by key rather than by eviction.

**What this found.** A session resuming at page 0 took its start `y` from the previous pass's
checkpoint rather than from the current template. When a master reassignment moved page 0's text
frame, the flow began at the old master's top margin under the new master's frame — silently, and
only where the two masters differ at the top. It was reachable before sections existed (reassign
`pages[0]` through a `LayoutSession` and it happens), and nothing caught it, because the spec-0035
session tests assert the pages were *recomputed*, not where the recomputed text landed. Sections made
it the common case: the session converged in 2 passes on a document the cold path could not settle at
all, and produced a different page count. A resume at page 0 is a cold start and now takes
`FlowState::start(template)`.

### `docs/format-spec.md` is brought current

It was stale in three ways, and this is the increment judged against its bump rule, so it is the
increment that fixes it:

- the example manifest said `format_version: 3` and carried pre-0063 `text` paragraphs;
- the migration table stopped at 2 → 3 and never recorded 3 → 4 — spec 0063's `text` → `runs`
  rewrite, the only migration so far that is *not* a structural no-op;
- the authoring appendix did not mention `**bold**` / `*italic*`, which spec 0064 shipped.

All three fixed, plus 4 → 5, a "Sections" section, and a statement of what the bump rule does *not*
turn on (not "a struct gained a field" — 0035's `pages`, 0054's `components` and 0056's `requires`
were all additive without one). The example manifest is parsed by a test, so it cannot drift again.

## Acceptance criteria

- [x] **A section anchored to a `BlockId` survives repagination.** Content inserted before it moves
      the section's start page, and the opener master lands on the new page and is gone from the old
      one — asserted on placed geometry. The same document expressed the spec-0035 way is asserted to
      get it *wrong* in the same test, which is what makes it a statement about the defect rather
      than about the fixture.
- [x] The anchor is a **body block**, not a heading, in every layout fixture: nothing about the
      mechanism is heading-specific and `section_starts` covers every placed variant that carries an
      identity.
- [x] A section whose anchor is not in the document assigns nothing and every page keeps the default.
- [x] Round-trip: a document with sections saves and loads equal to itself. Plus both empty cases
      (spec 0053's lesson) — a document with **no sections**, whose manifest must not contain the key
      at all, and a document with **zero blocks** carrying a section that anchors to nothing.
- [x] `FORMAT_VERSION` 5: a **committed v4 fixture** (`crates/core-model/assets/v4-masters.json`,
      bytes) loads, migrates, has no sections, resolves the masters it was authored with, and
      re-serializes to a manifest identical to the same document read natively as v5 — the identity
      direction, so no existing document's exported bytes move. The whole chain v1 → v2 → v4 → v5
      migrates, and `FORMAT_VERSION + 1` is still refused by name.
- [x] Convergence: a settling sectioned document reports `converged: true` in exactly 2 passes; a
      document with neither sections nor a contents list reports 1; a **pathological** document
      reports `converged: false` at exactly `FIXPOINT_MAX_ITERATIONS` rather than hanging, and its
      last iterate still contains every block.
- [x] The session and the cold path agree page for page on a sectioned document, and the session
      reports the same non-convergence on the pathological one.
- [x] `context_fingerprint` sees the section list, asserted in both directions (spec 0035's rule):
      re-pointing a section's master moves the pages, and an unchanged document reaches the same
      pages twice.
- [x] `SAMPLE_EXPORT_DIGEST` moves and is classified **identifier-only** under the ledger's template:
      8454 bytes both sides, 124 differing bytes in 8 runs, every one inside the XMP
      `DocumentID`/`InstanceID` or the trailer `/ID`. `component_parity`'s digest sets do not move.
- [x] `benches/budgets.toml`: every entry within budget, `incremental_blocks_measured` still 1,
      `export.sample_bytes` still 8454.

## Test strategy

The defect first, and from both sides. A test asserting only that the opener lands on the section's
page would pass against an implementation that put the opener everywhere; a test asserting only that
the old page lost it would pass against one that assigned nothing. Both are asserted, and the
positional expression of the same document is asserted to fail — the defect is *in* the test file, so
the increment's claim is checkable rather than described.

Each behaviour was then proved against its own defect by reintroducing it and watching the right
tests go red, then restoring:

| Defect reintroduced | Result |
|---|---|
| `DocumentTemplate::reassign` returns `false` — sections never reach the assignment | 6 tests fail, including both repagination tests and both convergence tests |
| the page-0 resume takes the previous checkpoint again | 3 session tests fail (parity with the cold path, the fingerprint test, and non-convergence reporting) |
| `derived_fingerprint()` dropped from `context_fingerprint` | the same 3 session tests fail |

**One thing no test distinguishes, stated rather than claimed:** removing `doc.sections` from
`context_fingerprint` fails nothing. It is currently redundant, because the derived fingerprint
transitions from "no section placed" to the real starts within every relayout and forces a recompute
on both passes regardless. It stays, because that fingerprint's whole argument is that it covers the
model structurally rather than by reasoning about which fields happen to matter today — a hand-picked
list is what had already stopped covering three fields that move a glyph.

The v4 fixture is committed **as bytes** for spec 0047's reason: a fixture the current serializer
wrote would migrate correctly by construction and prove nothing.

## Risks

- **The fixpoint is the cost, and it is per-pass over the whole document.** Bounded at 8 and reported,
  but the budget line that counts iterations is still owed — the M6 audit's finding, assigned to spec
  0076, where the second derived quantity arrives. This increment does not add one, and says so
  rather than leaving the reader to assume the bound is measured.
- **Non-convergence is a real outcome, not a theoretical one.** The oscillating fixture is three
  lines of ordinary authoring. What ships in that case is the last iterate — complete, possibly one
  page out on its opener — with `converged: false`. Presenting a guess as settled would be worse; so
  would refusing to lay the document out.
- **A whole-document reflow on any section edit.** Accepted for an authored list that changes rarely,
  and it is the same treatment `doc.pages` and `doc.master_pages` already get. Wrong for
  cross-references, which is 0076's problem and is called out there.
- **A stale first pass.** Both fixpoints restart from the underived state on every relayout — an
  empty heading index, an underived assignment — so a sectioned document's first pass is always
  thrown away. That is a cost sections *share* with the contents list rather than introduce, and
  seeding the loop from the previous pass is an optimisation neither has taken.

## Non-goals

- **"Start this section on the next recto."** Named rather than forgotten, as the roadmap entry
  requires. It is a **forced page break**, which the model has no mechanism for anywhere, and it is a
  forward-only rule rather than a fixpoint — a different shape of change from this one.
- **Folio formats and restart per section** (spec 0073) and **`{section}` in a running head**
  (spec 0074). Both are downstream of this and are their own increments. The shape landed here is
  what they need: `section_starts` gives a page → section mapping directly, and `Section::name` is
  already the string a running head prints.
- **A section as a container**, with its own content list, styles or nesting. See above.
- **A master for a section's continuation pages.** One page, deliberately.
- **Sections in a template file.** Impossible by construction, not deferred.
