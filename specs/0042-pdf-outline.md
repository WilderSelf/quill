# 0042 — PDF outline and the annotation/bleed guard

**Milestone:** M2 · **Status:** implemented

## Why

A 500-page PDF with no bookmarks is unusable, and a repo-wide grep for `Outlines`, `Dests`,
`Annots` and `bookmark` returned **zero hits**. The writer emitted a catalog and a page tree and
nothing navigational at all.

Spec 0040 answered the only question this needed — which page each heading landed on — so the
outline is a direct consumer of it and does not depend on the TOC's fixpoint loop. That was
deliberate slack in the plan, and it is what let this ship before 0041.

## What

### The outline tree

`/Outlines` built from the heading index, nested by heading level, with `/First`, `/Last`,
`/Parent`, `/Prev`, `/Next` and `/Count` set, and a `/Dest` per item pointing at its page with
`/Fit`.

Two details are computed rather than approximated, because both are the kind of thing a *parser*
accepts and a *viewer* renders wrongly:

- **`/Count` on an open item is its visible descendants, not its direct children.** An item with two
  children that each have three is 8, not 2.
- **Levels may skip and may start deep.** An `h3` directly under an `h1`, or a document whose first
  heading is an `h2`, are authoring realities. The builder attaches each heading to its nearest
  shallower ancestor rather than assuming a well-formed hierarchy.

A document with **no** headings emits no `/Outlines` entry at all, not an empty one — an empty
outline tree renders as an empty, confusing bookmark pane.

### Why there are no clickable links

PDF/X-1a requires annotations to sit **outside the BleedBox**, and a clickable table-of-contents
entry sits in the middle of the text block by definition. So this increment ships outlines and
destinations — document structure, always legal — and not internal links.

`annotation_finding(rect, page_setup, page_index)` makes that rule enforceable rather than
remembered. Nothing in the model can express an annotation yet, so it has nothing to guard today,
which is exactly when it is cheap to add and exactly the spec-0013 lesson: the validator and the
writer must agree on the rule *before* anything relies on it. When links arrive — behind a
non-PDF/X screen profile, most likely — this is the gate they have to pass.

## Acceptance criteria

- The `Document::sample()` export byte-hash **changes**, and this is the first **structural** move
  in M2 rather than an identifier-only one: 8559 → 8786 bytes, the catalog gaining `/Outlines` and
  the document gaining an outline root plus one item for the sample's single `h1`. Verified that
  nothing else moved: every `/Length` in the file is unchanged (`[1017, 376, 2981, 2017]` before and
  after), so no content, font or metadata stream was touched, and the object count rose by exactly
  two. Ghostscript CI green.
- `h1` / `h2` / `h1` produces a root with `/Count 3`, the first `h1` carrying `/Count 1` and
  `/First`/`/Last` at its child, and top-level siblings linked by `/Prev`/`/Next`. Asserted on the
  emitted objects.
- A skipped level (`h1` then `h3`) still nests under the nearest ancestor; two `h2`s with no `h1`
  both sit at the root with `/Count 2`. Both asserted.
- A document with no headings emits no `/Outlines`.
- A non-ASCII title is written as a UTF-16BE hex string with a BOM (`<FEFF0043…00E1…>`). Outline
  strings are a *different* encoding path from page content and get this wrong independently of the
  font subset, so it has its own test rather than riding on the tree one.
- `annotation_finding` flags a rect intersecting the BleedBox as a `Severity::Error` and returns
  `None` for one entirely off the page. Both directions, so the check cannot pass by flagging
  everything.
- `benches/budgets.toml` unchanged.

### One criterion changed during implementation

The roadmap said the sample should gain headings at two levels "so the gate actually exercises the
tree". It does not, and should not: changing `Document::sample()` would move its **content stream**,
not just its structure, and the golden fixture's whole value is that it does not move for reasons
unrelated to the change under test. The sample's single `h1` gives Ghostscript a real `/Outlines`
entry to parse, and nested-tree correctness is asserted by unit tests over the emitted objects,
which is stronger than a parser's silence anyway.

## Test strategy

Assertions parse the emitted PDF, mirroring the existing writer tests' approach. Counting items by
`/Dest [` rather than `/Title` matters and was found by a failing test: `/Title` also appears in the
document info dictionary, `/Parent` in every page object, and a bare `/Dest` also matches the
OutputIntent's `/DestOutputProfile`.

The no-headings and non-ASCII cases are the two that ship broken otherwise. The annotation test
constructs its input because the feature it guards does not exist — that is the point.

## Risks

- **Outline link fields are wrong-in-a-viewer rather than wrong-in-a-parser.** Ghostscript passing is
  necessary and not sufficient, which is why `/Count` is computed over the subtree and the sibling
  links are asserted individually.
- **Moving the golden hash means every test pinning it must move in the same PR**, and a reviewer
  must be able to tell this move from an accidental one. The stream-length comparison is what makes
  that tellable rather than a matter of trust.
