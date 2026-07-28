# 0062 — The neutral core: a mechanism is general or it is a bug

**Milestone:** M5 · **Status:** implemented

## Why

Quill is a general-purpose desktop publishing application first, and a TTRPG publishing application
second. Illustrated game books are the flagship use case — the audience it is designed for, the
corpus its fixtures come from, and the reason its POD presets exist — but every *mechanism* must be
one a cookbook, a field guide, a hardware manual or a thesis could use on the same terms.

The genre-shaped mechanism is also the worse mechanism for the genre, and M4 has just demonstrated
it: `StatBlock` was a struct with six fixed fields, so a game system whose creatures are set
differently — a PbtA move, a Blades clock, an OSR monster line, which is the normal case and not the
exception — could not be expressed at all, and spec 0054's declarations fixed that. What this crate
*does* is lay out a titled, bordered, multi-section record. That is what it should be called.

An audit on 2026-07-28 found the coupling is shallower than the vocabulary suggests: one crate, one
enum variant, one measurement function, one import fence, three template slugs and a quantity of
prose. M4 generalised the *behaviour*; the names it is expressed in still describe one genre. This
increment makes them match. It is deliberately **only** a rename, and it goes first in M5 because
every increment after it adds call sites to the surfaces it touches.

## What

### The crate

`quill-components-ttrpg` → `quill-components` (`crates/components-ttrpg/` → `crates/components/`).
Its crate doc describes portable content components rather than TTRPG ones.

### The types

| Was | Is | Note |
|---|---|---|
| `StatBlock` | `Panel` | A titled, bordered record of named sections |
| `RandomTable` | `RangeTable` | Entries partitioning `1..=max` |
| `RandomTable::die` | `RangeTable::max` | The upper bound of the partition |
| `TableEntry` | `RangeEntry` | |
| `TableEntry::result` | `RangeEntry::value` | |
| `Block::StatBlock { stat, .. }` | `Block::Panel { panel, .. }` | On-disk names unchanged; see below |
| `STATBLOCK_*_STYLE` | `PANEL_*_STYLE` | Const *names*; their string values unchanged |
| `measure_stat_block` | `measure_panel` | |
| `Table::from_random` | `Table::from_range_table` | |
| `STATBLOCK_PADDING_PT`/`_FILL`/`_STROKE` | `PANEL_*` | |

`Panel`'s field names are the one place a pure rename is not enough: `overview`, `attributes`,
`details`, `actions` and `reactions` are a creature's sections, not a record's. They keep their
names here. Renaming them is a *format* change with no benefit: spec 0054 already made sections
declarable, and a publisher who wants different ones declares a component rather than reaching for
this type — which is also what will eventually retire these fields. Two migrations where one will do
is a worse outcome than a field called `reactions` surviving, and this is recorded rather than left
as an oversight.

`RangeTable` gains a `label` field (defaulted, so nothing breaks): the heading column 0 is given
when the table is rendered. `from_range_table` uses it, falling back to `1–{max}`. This replaces the
hard-coded `d{die}` header, which asserted the range was a die roll. A d100 table is now expressible
as `label: "d100"` — the genre is *content* rather than *mechanism*, which is the whole point.

### What does not change, and why

**`FORMAT_VERSION` stays 3.** Two surfaces are on-disk contract and are deliberately left alone:

- The serde tag `"kind": "stat_block"` and the field name `"stat"` inside it. The Rust variant is
  `Block::Panel { panel, .. }` with `#[serde(rename = "stat_block")]` and `#[serde(rename = "stat")]`
  holding the wire form fixed.
- The three style-sheet keys `statblock-title`, `statblock-attr`, `statblock-body`. The constants
  naming them are `PANEL_TITLE_STYLE` and friends; their values are unchanged.

Both retire with `Block::Panel` itself, which spec 0054 made a bundled specialization of
`Block::Component`. A migration to a name with a
known expiry date is a migration nobody should write, and every `.tpub` and template file in
existence would pay for it twice. A test asserts the wire form so this is enforced rather than
remembered.

**`Document::sample()` and the `testdoc` word bank keep their content.** Both are *fixtures*, not
features. `Document::sample()` is the anchor every export byte-hash assertion in the workspace has
been measured against since spec 0001; re-wording it to read neutrally would spend that anchor for a
cosmetic gain, in the one increment whose entire claim is that it moved nothing. The `testdoc` word
bank is calibrated on word-length distribution, so swapping its vocabulary moves line breaking and
therefore every number in `benches/budgets.toml`. Neither is a mechanism a document can be built on,
which is the test this increment applies.

**The `:::statblock` import fence keeps working.** `:::panel` is the preferred spelling and both
parse identically. The old spelling is *retained*, not deprecated-with-a-date: there is no released
version to remove it in, and a published authoring syntax that silently stops parsing is the kind of
failure this repo exists to avoid.

### The template slugs

`rulebook` → `reference`, `adventure` → `digest`, `playtest` → `draft`. Each keeps its old slug as a
resolving alias, so `quill new --template rulebook` and any script or document that names one keeps
working. The templates' titles and descriptions describe their geometry — a two-column 6×9
reference, a single-column 6×9 digest, a single-sided US Letter draft — rather than a genre.

The CLI's default becomes `digest`, which is the same template `adventure` named.

## Acceptance criteria

- **`Document::sample()`'s export byte-hash is unchanged.** This increment renames things; if the
  hash moves, something other than a name changed. This is the criterion the whole increment stands
  on.
- `FORMAT_VERSION` is 3, and a test asserts the serialized form of a `Block::Panel` still carries
  `"kind": "stat_block"` and `"stat"`, and that a v3 JSON fixture written before the rename loads.
- Every old template slug resolves to its renamed template, asserted per slug; `Template::all()`
  lists the new names.
- Both `:::panel` and `:::statblock` import to the same document, asserted by equality of the two
  parsed documents.
- `RangeTable::lookup` and `is_complete` behave exactly as `RandomTable`'s did, including the
  `max`-boundary case.
- `Table::from_range_table` uses `label` when set and `1–{max}` when not, both asserted.
- No file under `crates/` contains the string `ttrpg` (case-insensitive), asserted by a test that
  walks the tree — the anti-drift precedent specs 0030, 0043 and 0053 use for documentation.

## Non-goals

- Generalising the *mechanism*. A `Panel` still has six fixed section fields; spec 0054's
  `ComponentDef` is the general mechanism and already shipped, and this increment does not retire
  the bundled specialization behind it.
- Any format change. See above.
- Renaming `Panel`'s section fields, the `stat_block` wire tag, or the `statblock-*` style keys.
- Re-wording fixtures (`Document::sample()`, the `testdoc` word bank, layout-engine test data).
