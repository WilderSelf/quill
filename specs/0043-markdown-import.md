# 0043 — The authoring on-ramp: `quill import`

**Milestone:** M2 · **Status:** implemented

## Why

Everything else in M2 made a good book *possible*. Nothing made it easy to type. The only
document-producing paths were `Document::sample()`'s two hardcoded blocks and hand-written JSON —
so a beginner could get a templated book with stat blocks, tables and a contents list, provided
they were willing to author it as a serde manifest.

This is last in its chain on purpose: a syntax is only worth designing once the things it imports
can actually be laid out. Written first, it would have been designed against types that did not
exist.

## What

A line-oriented syntax and `import(source, template) -> Result<Imported, ImportError>` in
`quill-core-model`, with `quill import doc.md -o out.tpub --template rulebook` as a thin caller.

- `#` … `######` + a space — a heading. `#1` is a word, not a heading, because that is how people
  write numbers.
- Blank-line-separated runs of text — a body paragraph; newlines inside one are soft.
- `![alt](asset-id)` — an image block.
- `:::statblock` … `:::` — `key: value` lines (`name`, `overview`, `detail`, `action`, `reaction`,
  `attr` as `name = value`).
- `:::table` … `:::` — pipe rows, first is the header, `|---|` separators ignored as furniture.
- `:::toc` … `:::` — `title:` and `max_level:`.

### Hand-rolled, and a subset on purpose

No markdown dependency. The workspace's dependency rule is one reason; the real one is that **a
subset can be enumerated**. "Markdown-ish" invites unbounded creep toward CommonMark, and an
importer that half-supports emphasis, nested lists and reference links is worse than one that
supports six constructs completely and says which. Round-tripping *out* is a named non-goal.

### Nothing is ever silently dropped

The worst failure this feature could have, and the easy one to write. Two rules, chosen by how much
the author has told us:

- An **unknown fence** (`:::spellcard`) is an **error** naming the line and listing what is
  supported. The author clearly meant a structured object; guessing which, or discarding it, both
  lose real content.
- **Everything else** is kept as body text with a **warning** naming the line — lists, block quotes,
  a stray pipe row, an unknown stat-block field. A paragraph that came out as plain prose is visible
  and fixable; one that vanished is not.

An unterminated fence runs to end-of-file rather than failing: every line the author typed is right
there, and refusing the document over three missing characters is the wrong trade for an authoring
tool.

## Acceptance criteria

- Regression: `Document::sample()`'s export byte-hash unchanged — this increment adds no styles and
  touches no other crate's behavior.
- Every supported construct imports to the block it names, asserted **block by block** with field
  contents, not by count: a count would pass while every block was the wrong kind.
- The template supplies page setup, styles and masters; the source supplies only content.
- An unknown fence is fatal, names its line, and lists the supported kinds.
- Unsupported inline syntax warns with a **line number** and the text is **kept** — asserted by
  reading the body blocks back and comparing them to the input lines.
- A malformed `attr` and an unknown stat-block field each warn and keep their line.
- `#1` is a body paragraph, not a heading.
- An unterminated fence still yields its content.
- An empty source imports to an empty document with no warnings.
- 5,000 paragraphs import to 5,000 blocks — the importer must not be the new bottleneck and must not
  lose a paragraph at scale.
- **The example in `docs/format-spec.md` is parsed by a test**, extracted from the document itself
  rather than duplicated, and asserted to exercise every construct it shows. The repo's anti-drift
  guard from spec 0030, applied to the syntax a user actually copies.
- The CLI handler is a thin caller over the library function, so the app can reuse the importer
  without going through a process. Warnings are printed, never swallowed.
- No new dependency.

## Test strategy

Table-driven parser tests over small inputs plus one whole-document fixture asserted block by block.
The diagnostics tests matter as much as the happy path and are written as "the text is still there",
not merely "a warning was produced".

## Risks

- **Scope.** The supported set is enumerated in the module docs, in `docs/format-spec.md` and in
  this spec; anything not on that list is a non-goal rather than a gap. That is the only thing
  keeping this increment closed.
- **The doc example is load-bearing.** It is what a user copies, so it is extracted from the doc and
  parsed rather than trusted — a duplicated copy in the test would drift from the doc within one
  change.
- The stat-block and table fences describe the same shapes `quill-components-ttrpg` defines in Rust.
  They are parsed into those types directly rather than into a second schema, so the two cannot
  disagree about what a stat block is — but the *syntax* is hand-written and would need updating if
  a component gains a field.
