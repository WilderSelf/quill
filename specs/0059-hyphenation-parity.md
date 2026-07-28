# Spec 0059 — One hyphenator, as there is one shaper

**Milestone:** M4 · **Size:** small · **Status:** implemented

## Problem

The roadmap's known issue, found while rendering spec 0044's verification page:

`quill render` lays out with `NoHyphenator` while `quill export` uses the real en-US
`HypherHyphenator`. So the two paths break lines differently, and a document can have a different
line count — and therefore a different **page count** — on screen than in the file that goes to the
printer.

`CLAUDE.md` states the rule this breaks in as many words: *one shaper for screen and press, so they
cannot drift.* The shaper is shared. The hyphenator is not, because `hyphenate::HypherHyphenator` is
private to `export-pdf`, and the CLI's `render` path cannot reach it.

## What this builds

The hyphenator moves to `quill-fonts`, beside the shaper, and every layout path uses it.

`quill-fonts` is exactly the right home and its own doc comment says so: it is the crate for
"shaping, metrics and glyph outlines … one shaper for screen *and* press, so they cannot drift". A
hyphenator is the same kind of thing — a decision about where text may break that both paths must
answer identically — and `hypher` is a `no_std` pattern table with zero transitive dependencies, so
moving it adds nothing to the screen-rendering path's dependency graph.

`export-pdf` re-exports `HypherHyphenator` from its old path so nothing downstream breaks, and drops
its direct `hypher` dependency.

Callers changed from `NoHyphenator` to the real one:

| Caller | Was | Why it mattered |
|---|---|---|
| `quill render` (CLI) | `NoHyphenator` | the reported defect: a different page count on screen |
| `quill-app`'s shell and `LayoutSession` | `NoHyphenator` | the canvas is the same screen path |
| `quill-render`'s own tests | `NoHyphenator` | they were asserting the wrong layout |

`NoHyphenator` itself stays. It is the parity default spec 0018 built the seam around, and the perf
harness deliberately uses it: a benchmark that hyphenates measures `hypher` as much as it measures
the engine.

## Acceptance criteria

- `quill render` and `quill export` produce the **same page count and the same line breaks** for a
  corpus of documents — asserted directly, which is the whole increment.
- The export byte-hash is unchanged: export already used the real hyphenator.
- The *render* fixtures change, and are re-derived deliberately.
- The known-issue entry is deleted from `docs/roadmap.md` in this PR.

## Non-goals

- Any language but en-US. A document-driven language seam remains spec 0018's named non-goal.
- Changing what the perf harness hyphenates with. See above.
