# 0029 — Per-page template seam

**Milestone:** M1 · **Status:** implemented

## Why

Master pages were blocked by two facts of the pagination loop, not by any missing data model.

**Every page was geometrically identical by construction.** On overflow, the page-advance branch
reset the frame cursor back into the *same* frame list. There was no point at which a page could be
asked what its own geometry was, so a verso could not mirror a recto and a chapter opener could not
drop its text frame.

**A page had no identity.** `LaidOutPage` was `{ blocks }`. A running head cannot print "42" and
nothing can vary by page number when the page does not know which one it is.

This lands both as a **seam at parity**, following the pattern the repo already uses for every risky
change (specs 0016, 0018, 0019, 0020): introduce the structural join first, proven to produce
identical output, then add the capability on top. Splitting it this way keeps spec 0030's large
model change from also carrying an engine redesign.

## What

### `PageTemplate`

```rust
pub trait PageTemplate {
    fn frames(&self, page_index: usize) -> Vec<Frame>;
    fn statics(&self, page_index: usize) -> Vec<PlacedBlock> { Vec::new() }
}
```

A master page is an implementation of this trait. `UniformTemplate` returns the same frames for
every page and no statics — exactly the previous behavior, which is what makes the parity claim
testable rather than asserted.

`lay_out_with_template` is `lay_out_in_thread` generalized: each new page asks the template for its
geometry instead of reusing the previous page's. `lay_out_in_thread` now delegates through a
`UniformTemplate`, so every existing caller is unaffected.

### `LaidOutPage` gains `index` and `statics`

`index` is the page identity that running heads, folios and recto/verso rules all need — and that
spec 0031 needs in order to report *which* pages a change reflowed.

`statics` is template-contributed content, kept **separate** from flowed `blocks` for two reasons
that both matter later: it is drawn first, so master art sits behind the text that flows over it;
and incremental relayout can leave it alone, because it does not depend on where the text happened
to break.

The PDF writer draws `statics` before `blocks`, and both its image-collection pass and its per-page
resource dictionary now walk both — otherwise a master page's background art would be referenced by
a content stream without ever being embedded.

### An empty frame list is a panic

A page with nowhere to put content would silently drop it. That is the failure class `CLAUDE.md`
forbids, and a loud failure is the correct response.

## Acceptance criteria

- [x] Layout through a `UniformTemplate` is **equal** to layout through the plain thread it wraps — the parity claim, asserted on a two-column thread over 40 blocks.
- [x] The export byte-parity digest is unchanged, so PDF output did not move.
- [x] Pages are numbered `0..n` in order.
- [x] A template whose frames narrow per page produces pages of 400 / 350 / 300 pt — geometry really is asked for per page, which is the capability a uniform thread cannot express.
- [x] A template's statics land on every page and vary by page index (a folio printing `1`, `2`, …).
- [x] Statics stay separate from flowed blocks.
- [x] A template returning no frames panics rather than dropping content.
- [x] The writer draws statics before flowed content, and embeds images referenced by either.

## Non-goals

- **Authored** master pages — serde types, a `master_pages` collection, named masters assigned to
  pages, `{page}` token resolution. That is spec 0030, along with `FORMAT_VERSION` 2. Nothing in
  this increment is reachable from a `.tpub` yet; the trait is implemented only by `UniformTemplate`
  and by tests.
- Recto/verso mirroring as a built-in template. The seam supports it — `page_index` is all it needs
  — but which margins mirror is an authoring decision that belongs with the authored model.
- Master-page inheritance or overriding a master on a single page.
