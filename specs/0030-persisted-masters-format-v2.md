# 0030 — Authored master pages and margins; `FORMAT_VERSION` 2

**Milestone:** M1 · **Status:** implemented

## Why

Spec 0029 built the seam where a page can have its own geometry and furniture. Nothing could reach
it: `PageTemplate` was implemented only by `UniformTemplate` and by tests, so a `.tpub` could not
express a master page, a margin, or a running head. This is the increment that makes authored layout
reachable from a document — the "pro layer" half of the hybrid paradigm, and what makes a 500-page
book consistent without touching 500 pages.

## What

### Margins, relative to the binding

`Margins { top, bottom, inside, outside }` on `PageSetup`.

**Inside/outside rather than left/right**, because a bound book's margins are relative to the
*spine*: the inside margin sits at the spine on both halves of a spread, so it falls on the left of
a recto and the right of a verso. Left/right would force every layout rule to special-case page
parity, and getting it wrong makes text drift toward the gutter on every other page.

Defaults are zero on all edges — the pre-0030 behavior, where the text frame was the whole trim
area. Zero is not a sane *design* default (text runs to the trim edge), but changing it would
silently reflow every existing document. It is a template concern for M2, and the roadmap records it
as an explicit open question rather than leaving it to look like an oversight.

### Master pages

`MasterPage { name, margins, columns, gutter_pt, statics }`, with `Document::master_pages` and a
`default_master`.

`MasterStatic` is either `Text` — a running head, folio or footer, whose text may contain a `{page}`
token resolved to the one-based page number — or `Image`, for background art and rules. A token
rather than a separate "folio" element type, so a running head can read `The Dungeon — 42` without
needing two elements.

`DocumentTemplate` implements spec 0029's `PageTemplate` from the document: margins and columns
become per-page frames, statics get stamped with `{page}` resolved. With no master and zero margins
it produces exactly `Frame::full_page`, so a document declaring neither lays out as it always did.

### Two degradations, chosen deliberately

Both follow `CLAUDE.md`'s rule about visible failure versus silent corruption — but note the rule
cuts the *other* way here than it does for press output, and for a reason worth stating.

- **An unknown `default_master` resolves to no master**, not an error. A renamed master should
  degrade to the document's own page setup, not refuse to open the book.
- **An over-wide gutter falls back to one column** instead of panicking. `Thread::columns` panics on
  this, which is correct for a *programmatic* caller passing a computed value. Here the gutter is
  **authored** — a user can type any number — and a document that cannot be opened is worse than one
  that looks wrong and can be fixed.

The distinction: refusing loudly is right when the alternative is *shipping* something wrong to a
print shop. Refusing to open a document is not that; it just loses the user's work.

### `FORMAT_VERSION` 2

The bump is deliberate even though every new field is `serde(default)` and a v1 manifest therefore
loads unchanged.

A version's purpose is to stop an **older build** from opening a document it would silently
mis-lay-out. A build predating master pages would ignore `master_pages` entirely — producing the
book without its running heads, folios or column geometry — and could then save that back over the
original. Refusing to open is the correct outcome.

The v1 → v2 migration is structurally a no-op, and is written as one rather than skipped: it
defaults `margins` and `master_pages` explicitly, so the chain reads as a record of what each
version changed and a later step that *does* need to rewrite a field has an obvious home. This is
the first real exercise of the machinery spec 0025 built.

## Acceptance criteria

- [x] A document with no master lays out in the full-page frame, with no statics — parity.
- [x] Margins inset the text frame on all four edges.
- [x] Inside/outside margins mirror across a spread (recto 60/20, verso 20/60), and do **not** mirror when `facing_pages` is off.
- [x] A master's column count and gutter divide the text area into frames.
- [x] A master's statics are stamped on every page with `{page}` resolved to the one-based number.
- [x] An unknown `default_master` degrades to the page setup rather than failing.
- [x] An over-wide gutter falls back to one column rather than panicking.
- [x] A master overrides the document's margins when it sets its own.
- [x] Masters, margins and statics round-trip through JSON.
- [x] A v1 manifest migrates forward, loads, and *means what it meant* — no margins, no masters.
- [x] The version-refusal test is expressed relative to `FORMAT_VERSION`, so it keeps testing "one newer than we understand" across future bumps. (Written with a literal `2`, it silently stopped testing anything the moment this spec bumped to 2 — which is exactly what happened, and is why it now reads `FORMAT_VERSION + 1`.)
- [x] The exported PDF changes only in its document identifier: 124 bytes across 8 runs, all inside the XMP `DocumentID`/`InstanceID` or the trailer `/ID`; length unchanged, no content stream moved.
- [x] Performance budgets still met.

## Non-goals

- **Per-page master assignment.** One default master governs the document. A chapter-opener master
  applied to specific pages needs a page list in the model, which is a bigger change; one consistent
  master is the case worth having first.
- **Authored frames and threads** — arbitrary author-drawn text boxes chained together, persisted in
  the manifest. Frames are still derived from margins and column count. The `Frame`/`Thread` types
  remain layout-engine-owned and non-serde.
- Master-page inheritance, or overriding individual master properties on one page.
- Line breaking of master statics. A static is a single line at a fixed position; a running head
  that overflows its rect is an authoring problem, and a visible one.
