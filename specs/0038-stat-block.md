# 0038 — `Block::StatBlock`

**Milestone:** M2 · **Status:** implemented

## Why

`quill-components-ttrpg` defined `StatBlock` and `RandomTable` with real logic and tests, and **no
crate in the workspace depended on it**. It was a data model with no layout, no export and no way to
put one in a document — the TTRPG-native content object this product exists for, unreachable from a
page.

Spec 0037 landed the primitive that makes a stat block drawable. This is the increment that connects
the two.

## What

### The model

```rust
Block::StatBlock { id: BlockId, stat: StatBlock, color: Color }
```

The portable `StatBlock` verbatim rather than flattened into fields, so the same value can be
authored, exchanged and rolled on without a document in sight. The document adds only what placing
it on a page needs: identity and ink. `quill-core-model` gains a dependency on
`quill-components-ttrpg` and re-exports the component types, so naming a field of `Block` does not
require callers to add a dependency of their own. The direction is deliberate — the model depends on
components, never the reverse, because a component is portable content that must be expressible
without knowing about documents.

One colour for the whole block rather than one per section: a stat block is a single typographic
object, and per-section colour would multiply the preflight surface for no authoring gain.

### The composite seam

`Measured::Panel { fill, stroke, parts, rules }` — every offset relative to the block's own origin,
so the whole thing is placed by adding the flow cursor once. Pagination still sees **one block**: it
moves whole to the next frame when it does not fit, exactly as any other block does.

Placement emits the panel first, then the rules, then the runs — decoration behind its own content,
the same order the writer and the paint list already rely on.

Three built-in styles (`statblock-title`, `statblock-attr`, `statblock-body`) join
`StyleSheet::default()`. Built in rather than left to the author, because the point of a first-class
component is that dropping one in produces something that already looks like a stat block; restyling
the whole book is still one edit, to those three names.

### Section order, and why it is asserted

Name → overview → attributes → details → actions → reactions: the order `StatBlock`'s own doc
comment states. The first draft put attributes before overview, which prints a creature's armour
class above its type. That is visibly not a stat block, and it passed every assertion — it was caught
by rendering the page.

Sections are separated by hairline rules. Without them the panel reads as a tinted paragraph rather
than as a stat block.

## Scope: splitting is not in this increment

The roadmap's 0038 entry described keep-together **and** splitting a stat block across frames at a
section boundary. Only keep-together ships here, and the reason is not effort:

**Keep-together needed no new mechanism.** The pagination loop already moves a block whole to the
next frame when it does not fit, so a stat block gets it by being one block. The tests assert that it
genuinely *is* one block to that rule — that no run is orphaned onto another page — rather than
assert a mechanism that was written.

**Splitting is a different, larger change than the roadmap implied.** `measure_block` is given a
width and returns a height; it has no notion of available height, so splitting would mean asking
"measure this for at most H points" and getting back a fragment plus a remainder. That puts height
into the measurement-cache key, where it would thrash: the same block measured against a
half-full frame and an empty one becomes two entries, on the hot path spec 0031 exists to keep cold.

It is also **not specific to stat blocks**. The roadmap already records "a block never splits across
frames" as a known issue found while building 0036, where it leaves the two-column `rulebook`
template with ragged column feet. Paragraphs, stat blocks and tables all want the same mechanism.
Building a stat-block-only splitter here would be the second of three, and would have to be undone.

Splitting is therefore left as the existing known issue, now with a note that it is wanted by three
callers. An oversized stat block is placed and overflows rather than looping — the `frame_empty`
guard already handles that, and it is the same behavior any oversized block has today.

## Acceptance criteria

- Regression: the `Document::sample()` export byte-hash **changes**, because
  `StyleSheet::default()` gained three styles and the sample serializes its stylesheet. Verified as
  identifier-only rather than accepted: before and after are both 8559 bytes and differ in exactly
  108 bytes, every one inside the XMP `DocumentID`/`InstanceID` or the trailer `/ID`. No content
  stream moved. Ghostscript CI green.
- Non-ASCII guard: a stat block with accented characters in **every** section reaches the font
  subset and exports — the spec-0026 silent-failure case, where a character `collect_doc_chars`
  misses is not an error anywhere, it just renders as a `.notdef` box.
- Every exhaustive `Block` match site is updated deliberately and named: `Block::id`/`set_id`,
  `with_style`, `StyleSheet::resolve`, `measure_block`, the session's content fingerprint, the
  export colour check, `collect_doc_chars`, and the app's `edit_text`.
- Placement: one panel, one rule per section boundary, one text run per section line, and nothing
  else. Runs are inset by the padding on all four sides, and the last run's bottom clears the
  panel's bottom padding.
- The padding comes off the **measure**, not only the position: a long action wraps inside the
  panel rather than being broken to the full frame width and then drawn inset.
- Order: a stat block with one line per section lays out as name, overview, attribute, details,
  actions, reactions — asserted on the rendered strings.
- Keep-together: with 52 lines of body text ahead of it, every run of the stat block lands on the
  same page as its panel and none is orphaned.
- The built-in styles exist and the title is set larger than the prose; asserted in the model and
  again through a laid-out page.
- Cache correctness: **each of the seven independently editable parts** (name, overview, attribute
  key, attribute value, details, actions, reactions) invalidates the measurement when edited, and an
  unchanged document measures nothing. Both directions, per part.
- A stat block in RGB fails preflight like any other block.
- Round trip: a stat block survives `to_json`/`from_json` as a component, not as flattened text.
- `benches/budgets.toml` unchanged; `quill-testdoc` emits no stat blocks, so the 500-page workload is
  the same one every prior budget was measured against.

## Test strategy

Geometry through `MonospaceRunMetrics`, as the crate does throughout. The two that carry the
increment are the keep-together test — which asserts *no run is orphaned*, by source id, rather than
counting fragments — and the seven-way cache test, which asserts both directions per section so it
cannot pass against a fingerprint that invalidates on everything.

The order test exists because that defect shipped through every other assertion and was caught only
by looking at a rendered page.

## Risks

- **The composite is the first block to emit more than one placed item.** The flow loop now extends
  rather than pushes. Anything downstream that assumed one placed block per content block would be
  wrong — nothing did, but it is the shape of bug to look for when adding tables (0039).
- **A double space is not a separator.** `break_by_width` normalizes every run of inter-word
  whitespace to a single U+0020, so the `"{key}  {value}"` this first used collapsed to an ordinary
  word space and an attribute read as one sentence. It now uses a colon. With no bold weight
  available, punctuation is the only thing that distinguishes a key.
- **Attribute keys still wrap mid-key in a narrow measure.** "Armour Class: 15 (leather, shield)"
  can break after "Armour" in a 150 pt column. Proper key/value columns or a hanging indent would fix
  it and neither exists; recorded in the roadmap's known issues rather than left as a surprise.
