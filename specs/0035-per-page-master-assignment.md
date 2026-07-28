# 0035 — Per-page master assignment

**Milestone:** M2 · **Status:** implemented

## Why

Spec 0030 made master pages authorable, but only one master at a time. `Document::default_master`
applies a single master to every page in the book, and its own doc comment records per-page
assignment as a follow-up. So a document can have consistent furniture or no furniture, and nothing
in between: no title page without a folio, no chapter opener with a deeper top margin, no front
matter set differently from the body.

That is the gap that makes M2's first real deliverable impossible. A "beginner template" (spec 0036)
whose whole promise is "this already looks like a book" needs a chapter opener; without per-page
assignment a template is a margin preset with a stylesheet attached.

## What

### The page list

```rust
pub struct PageOverride {
    pub master: Option<String>,
}

pub struct Document {
    // ...
    #[serde(default)]
    pub pages: Vec<PageOverride>,
}
```

`pages[i]` governs page `i`. Resolution order for a page's master is:

1. `pages[i].master`, if the list reaches index `i` and that entry names a master that exists;
2. else `default_master`, if it names a master that exists;
3. else no master — the document's own `page_setup` governs, which is the pre-0030 behavior.

**Additive, so `FORMAT_VERSION` stays 2.** An absent or empty `pages` list reproduces today's
behavior exactly, so no migration is written and every v2 manifest keeps loading. The roadmap
records the general form of this decision: spec 0030's migration is the one-way door of the format,
and there is no reason to open a second one for a field that defaults cleanly.

**What that costs, stated plainly.** The version gate exists to stop a newer manifest being
"loaded with its unknown fields dropped" and then saved back over the original — `version.rs` calls
that the failure mode `CLAUDE.md` forbids. Not bumping means a build predating this spec reads a
0035 document, silently discards `pages`, and can save over it. Spec 0026 set the precedent by
adding block ids without a bump, but ids are *regenerable* and a page list is authored intent, so
the loss here is user work rather than derived state.

It is still the right call — bumping would force a migration on every reader for a field that is
empty in every document written so far, and the roadmap has pre-authorized additive change for all
of M2 — but the asymmetry is real and it is inherited by specs 0036–0043. If a later M2 increment
adds a field whose silent loss would be worse than this one's, that increment bumps rather than
citing this precedent.

### Assignment is by index

`pages[i]` addresses page `i`, so inserting content that pushes the book by a page slides every
subsequent assignment. That is a real consequence and it is the accepted semantics for this
increment — spec 0041's generated TOC is the case that will actually trigger it, and it says so.

The alternative — anchoring a master to the heading that opens a chapter, so it survives
repagination — needs a notion of "section" that the model does not have. It is recorded as an open
question in `docs/roadmap.md` for M3 rather than smuggled in here.

### There is no way to say "this page has no master"

Once `default_master` is set, every page has it unless overridden by another master — an override
cannot opt *out*. A full-bleed title page is therefore modelled as an override naming a master with
no statics and no margins, not as an absence. That is deliberate (the alternative, a tri-state
`Option<Option<String>>`, serializes badly and reads worse) and adequate for M2, since spec 0036's
templates can ship exactly such a `plain` master.

Note also that assignment is **zero-based** — `pages[0]` governs the page a reader calls page 1,
which is what the `{page}` token prints. Predictable, but worth stating, because the two indices sit
next to each other in every authored master.

### Fallback is silent, not an error

A `PageOverride` naming a master that no longer exists falls back, exactly as an unknown
`default_master` already does (`DocumentTemplate::new`) and as an unknown style name does
(`StyleSheet::resolve`). This is the authoring posture the repo has applied consistently: losing the
page because its master was renamed would be far worse than losing the furniture. A renamed master
is visible on screen the moment it is looked at; a refused document is not recoverable by the
person who typed the name.

### Statics resolve per page

`DocumentTemplate::statics(page_index)` now stamps *that page's* master's statics, so an opener with
no folio and a body master with a `{page}` folio produce different furniture on different pages.
This is the first thing in the engine to vary statics *between* pages, so spec 0029's invariant —
statics never touch the flow cursor — is re-asserted here rather than assumed to still hold.

### Incremental safety

`LayoutSession`'s context fingerprint covers everything that is not block content but still moves
pages: page setup, styles, master pages, `default_master`. The page list joins that set. Its own
doc comment states why this is not optional: a context the fingerprint misses makes an edit look
like "nothing changed", and the session returns the previous pages — a stale document presented as
a current one, which is worse than being slow.

## Acceptance criteria

- Regression: exporting `Document::sample()` produces PDF bytes whose SHA-256 matches the committed
  spec-0025 constant; the CI Ghostscript job stays green.
- `Document` with masters `opener` (108 pt top margin) and `body` (36 pt), `default_master: "body"`,
  and `pages[0].master = "opener"` lays out page 0 with a frame at `y_pt == 108.0` and pages 1+ at
  `y_pt == 36.0` — asserted to 0.01 pt on a 3-page document.
- A `PageOverride` naming an unknown master falls back to `default_master`; with `default_master`
  also unknown it falls back to the document's page setup. Both steps asserted.
- A `pages` list shorter than the page count governs the pages it covers and leaves the rest on
  `default_master`; a list longer than the page count is not an error and the surplus is ignored.
  Both asserted.
- `PageOverride { master: None }` is a no-op that falls through to `default_master` — an explicit
  entry that declines to override is not the same as "no master".
- Statics resolve per page: an opener with no statics and a body master with a `{page}` folio give
  page 0 zero statics and page 1 one static reading `2`. Asserted.
- Statics still do not consume flow space: the same content flows to identical y-positions with and
  without a statics-bearing master (spec 0029's invariant, re-asserted now that statics vary by
  page).
- `LayoutSession` sees the page list: changing `pages[1].master` and calling `relayout` reports the
  document as changed (`pages_reused == 0`) rather than returning stale pages; calling `relayout`
  twice with an unchanged document gives `blocks_measured == 0`. Both directions asserted.
- A v2 manifest with no `pages` key loads — asserted against a literal v2 fixture, not only against
  the v1 one that reaches the struct through `migrate` — and `pages` round-trips through
  `to_json`/`from_json` and through a `.tpub` carrying real masters and overrides (the existing
  container round-trip uses `Document::sample()`, whose masters and page list are both empty, so it
  proves nothing here).
- `quill-testdoc` changes only to complete the `Document` struct literal with an empty `pages`; its
  500-page target assertion still holds and no budget in `benches/budgets.toml` is re-baselined —
  this increment adds no default furniture and no work to any measured path.

## Test strategy

Inline `#[cfg(test)] mod tests`, per repo convention. Serde defaulting and the fallback chain are
tested in `core-model`; per-page geometry, per-page statics and the flow-cursor invariant in
`layout-engine`; the fingerprint in `session`.

Geometry is asserted as exact arithmetic with the computation written above the assertion, in the
style the crate already uses. The load-bearing test is the fingerprint one, in **both** directions:
asserting only that a changed page list invalidates would pass vacuously against an implementation
that invalidates on everything, and asserting only reuse would pass against one that never
invalidates.

## Risks

- **The fingerprint omission is the silent-wrongness bug.** It fails in the stale direction, which
  is the direction nothing complains about. Hence the both-directions test.
- **Index-based assignment shifts under repagination.** Accepted semantics, documented above and in
  the roadmap's open questions; spec 0041 must cover the interaction rather than discover it.
- `DocumentTemplate` resolved its master once in `new()`; it now resolves per page, which turns a
  field read into a linear scan of `master_pages` inside the pagination loop.

  **This risk is closed by argument, not by the bench, and the distinction matters.** The 500-page
  synthetic document in `quill-testdoc` carries no masters at all, so `master_for` returns on its
  first `pages.get()` and never scans — the green `layout.ms_per_page` says nothing about the
  scanning path. The actual cost is three `master_for` calls per page (`frames`, `content_rect →
  margins`, and `statics`), each `O(master_pages)` string comparisons: ~30k comparisons for a
  500-page book with 20 masters, against ~155 ms of layout. Negligible, but asserted by reasoning.
  A future reader must not treat the passing budget as coverage of this path; if masters ever become
  numerous, resolve once per page and pass the result down rather than re-deriving it three times.
