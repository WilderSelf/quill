# 0066 — Lists: bullets, numbering, and the counter that survives repagination

**Milestone:** M5 · **Status:** implemented

## Why

The importer refused lists outright — `"lists are not supported; kept as body text"` — because the
model had none. A bulleted or numbered list is table stakes for any application that sets text, and
its absence was visible at the on-ramp: the one construct an author is most likely to type was the
one that came out as prose with a stray hyphen in front of it.

It runs ahead of 0064 and 0065 in the milestone for a reason recorded in the roadmap: 0064 is blocked
on font assets, and 0065 would ship a `CharacterStyle` most of whose fields stay inert until 0064
lands — the same mostly-declared-and-not-honoured shape spec 0063 split itself to avoid. A list needs
neither faces nor per-run metrics.

## What

### A list is a paragraph property, not a block type

```rust
pub struct ListSpec { pub marker: ListMarker, pub start: u32, pub level: u8 }

pub enum ListMarker {
    Bullet { glyph: char },
    Number { format: NumberFormat, suffix: Option<char> },
}

pub enum NumberFormat { Decimal, LowerAlpha, UpperAlpha, LowerRoman, UpperRoman }
```

`ParagraphStyle` gains `list: Option<ListSpec>`, defaulted and skipped when absent, so a document
that names no list serializes and lays out exactly as it did before — `FORMAT_VERSION` does not move.

A **list is a run of consecutive paragraphs that happen to be marked**, not a container. Making it a
container would mean a second flow model, and nothing about breaking, fragmentation, the baseline
grid or the measurement cache works differently inside one. It also makes "a paragraph interrupts a
list" fall out rather than needing a rule.

A single `char` for the glyph and the suffix, rather than `String`s, because `ParagraphStyle` is
`Copy` and is passed by value through the whole measurement path — a heap allocation there would be
paid per block per re-layout. It is also what markers are: a bullet is one character, and `1.` is a
counter plus one.

### The counter is derived, never accumulated

`list_markers(content, styles)` walks the document's blocks **once, before anything is placed**, and
returns each item's marker text keyed by `BlockId`. It keeps one counter per nesting level; a level's
counter restarts when a shallower item or a non-list paragraph interrupts it.

This is spec 0041's rule, and it is here for the same reason: an incremental pass reuses whole pages,
so anything counted while placing goes missing on a reused page — and goes missing exactly when the
document was just edited, which is always.

Because a marker depends on the blocks *before* it, it is **context, not content**: inserting an item
at the top of a list changes every marker below it while changing none of their text. So the marker
joins `MeasureKey` alongside the content and style fingerprints. Without that, a renumber would serve
stale ordinals straight from the cache.

### The marker is drawn in the gutter, not in the flow

`Measured::Text` gains `marker: Option<String>`, and placement emits a second `PlacedBlock::Text` for
it at the frame's left edge on the item's first baseline. It never enters the text, so it cannot move
a break and no measurement changes shape.

The item's indent is **uniform** — every line inset by the gutter, the marker outdented — not spec
0048's hanging shape. A hanging indent leaves the first line flush with the marker and insets only
the wraps, which is the key/value shape and reads as a broken list.

A fragment keeps the marker on the **head** and drops it from the continuation: a continuation is not
a new item, which is the rule spec 0045 applies to a table's repeated header from the other direction.

### Number formats

Bijective base-26 for alpha (1 → `a`, 26 → `z`, 27 → `aa`), because ordinary base-26 needs a zero
digit and would write the 27th item as `ba`. Additive-subtractive roman, falling back to decimal above
3999 where there is no standard form — a marker that silently vanished or that a reader had to decode
are both worse than a number.

### The importer

`- item`, `* item` and `1. item` import as paragraphs named `list-bullet` or `list-number`. **The
ordinal in the source is not honoured**: markers are derived from document order at layout time, so
an author who writes `1.` three times gets `1. 2. 3.` — which is what every markdown implementation
does and what they meant. Two built-in styles ship in `StyleSheet::default()` so there is something
to name.

## Acceptance criteria

- An ordered list numbers 1..n in document order, and inserting an item at the top renumbers
  everything below it while changing none of their text.
- A paragraph between two lists makes them two lists: the second restarts.
- Nested levels count independently and a shallower item closes the deeper ones — `1. a. b. 2. a.`
- Number formats are right at their boundaries: `z`/`aa`, `AB`, `iv`, `ix`, `MCMXCIV`, and decimal
  above the roman range.
- The marker is placed at the frame's left edge and the item's text is inset past it.
- A continuation is not marked again.
- `quill import` maps `-`, `*` and `1.` to marked paragraphs, with no warning, and the source ordinal
  is ignored.
- `Document::sample()`'s export moves **only by its identifiers**: 8786 bytes both sides, 108
  differing bytes, all inside the XMP `DocumentID`/`InstanceID` or the trailer `/ID`. The sample has
  no list, so nothing it draws could have changed — the move is `StyleSheet::default()` gaining two
  entries, which is the shape spec 0038 first recorded.

## Non-goals

- Continuing a list across an interrupting paragraph (`@list-continue`). The rule here is that an
  interruption restarts; an explicit continuation marker is a model addition with no caller yet.
- Multi-level compound markers (`1.1`, `1.a`). Each level counts independently and prints its own
  ordinal only.
- Author-set marker glyphs per item, list-item spacing distinct from paragraph spacing, and
  right-aligned markers in the gutter. All are stylesheet work on a mechanism that now exists.
- `quill import` nesting by indentation: one level, which is what the six-constructs-completely
  posture allows until nesting has a syntax worth committing to.
