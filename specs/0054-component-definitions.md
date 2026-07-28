# Spec 0054 — Component definitions as data

**Milestone:** M4 · **Size:** large · **Status:** implemented

## Problem

`StatBlock` and `Table` are Rust structs in `quill-components-ttrpg`, measured by hand-written code
in `layout-engine` (`measure_stat_block`, `measure_table`). Every number those two functions use —
the panel's tint, its padding, the hairline between sections, the cell inset, the zebra shade, which
runs open a section, which style each run resolves — is a crate constant or a literal in a loop.

A publisher whose game system sets its creatures differently — a PbtA move, a Blades clock, an OSR
monster line — cannot express it at all. A pack format (spec 0055) that could carry only *those two*
shapes would be a theming system, not an ecosystem.

## What this builds

A **`ComponentDef`**: a declaration of exactly what those two functions hard-code, and **one
interpreter** in `layout-engine` that lays any definition out. The two bundled components are
re-expressed as definitions, and their measurement functions retire behind the interpreter.

### The declaration

In `quill-components-ttrpg`, `def.rs`:

```rust
pub const COMPONENT_DEF_VERSION: u32 = 1;

pub struct ComponentDef {
    pub version: u32,            // COMPONENT_DEF_VERSION; a newer one is a typed refusal
    pub name: String,            // resolved by `Block::Component::def`
    pub panel: PanelDef,         // fill, stroke, padding
    pub sections: Vec<SectionDef>,
    pub split: SplitDef,         // granularity, min_items, keep_together
}
```

A `SectionDef` names the instance field it renders (`source`), the paragraph style it resolves
(`style`), a *shape* saying how that field's value becomes runs, optional rules above and below, and
whether it is re-stated at the top of every continuation (`repeat`, spec 0045).

Four shapes, which is what the two bundled components need and no more:

| Shape | Field value | Emits |
|---|---|---|
| `text` | `Text(String)` | exactly one run, always (an empty string still opens the component) |
| `lines` | `Lines(Vec<String>)` | one run per line; **nothing at all when empty** |
| `pairs` | `Pairs(Vec<(String,String)>)` | one run per pair, `key⟨sep⟩value`, the key's words joined by U+00A0 so it cannot break |
| `rows` | `Rows(Vec<Vec<String>>)` | one row of cells per element, across the component's columns |

A `repeat` section must come **before every ordinary one**. `repeat` means "the prefix each
continuation re-states", and the interpreter implements it by capturing everything emitted so far —
so a repeated section with content above it re-states that content too, and a cut component
duplicates it on every continuation. Refused by `ComponentDef::validate`.

Every declared measurement — panel padding, stroke width, rule thickness and gaps, cell padding —
must be finite and non-negative. Geometry has no sane fallback the way a style name does: a negative
padding draws text outside its own panel and off the page, and spec 0050's safe-area check exempts
anything outward of trim as deliberate bleed, so nothing downstream would report it.

The instance carries `ComponentFields`, a `BTreeMap<String, FieldValue>`. A field the definition
names and the instance omits contributes nothing; a field the instance carries and the definition
does not name is ignored. Both are the authoring posture, not errors.

### The interpreter

`measure_component` walks the sections once and emits the same `Measured::Panel` the two functions
emit today. Its rules, stated because they are what the byte-identical criterion turns on:

- `y` starts at `panel.padding_pt`; the final height is `y + panel.padding_pt`.
- A section that emits no runs emits **no rule and no cut boundary** — an absent `overview` must not
  leave a hairline behind.
- A rule advances `y` by `gap_above_pt` before it is drawn and `gap_below_pt` after; **its thickness
  does not advance `y`**. This is not a rounding convenience: it is what the shipped stat block does
  (the hairline sits inside its lower gap) and what the shipped table does (`gap_below_pt` is set to
  the rule's own thickness, so the header rule *does* advance by it).
- The first *emitted* section never draws its `rule_above`. It has the panel's own edge above it.
- Every run applies its style's `space_before_pt`, its indent (spec 0048) and `space_after_pt`.
- `repeat_h` is `y` after the last `repeat: true` section; `repeat_parts`/`repeat_decorations` are
  everything emitted up to that point. With no repeated section that is exactly `panel.padding_pt`,
  the panel's top inset — which is what a cut stat block re-states.
- Cut item 0 begins at `0.0`, not at `repeat_h`, so item 0 absorbs the prefix. `PanelSplit` documents
  that invariant; this is where it is produced.
- `trailing_pt` is `panel.padding_pt`.
- A component that emits no parts and no decorations is a zero-height panel with `split: None` — the
  empty-table case, which must not draw an empty box.

`SplitGranularity::Sections` records a cut boundary at each emitted section (the stat block);
`Elements` records one at each element of **every non-repeated section** (the table's rows). A
`repeat` section records none at all — it is the prefix every fragment begins with, not content that
can be left behind, and giving it a boundary would hand item 0 the prefix alone and overfill every
continuation by exactly one row.

Columns are shared across every `rows` section, so a header and its body agree: the count is the
widest element seen, and the widths come from the `Widths` field the definition names, through the
existing `Table::normalized_columns` fallback.

### The document

- `Block::Component { id, def, fields, color }` — a new `#[serde(tag = "kind")]` variant. Additive:
  a v3 manifest that has none loads and lays out exactly as before, so `FORMAT_VERSION` stays **3**.
- `Document::components: BTreeMap<String, ComponentDef>` — `#[serde(default)]`, likewise additive.
  Packs (0055–0056) will populate it; a document may also carry its own.
- `Block::StatBlock` and `Block::Table` keep their authored shape and their serialization. They are
  **sugar**: `measure_block` converts each to the bundled definition plus fields and calls the one
  interpreter. Nothing measures a stat block twice.

Retiring the authored variants would be a `FORMAT_VERSION` bump that buys an author nothing — a
`stat` field is a better thing to write by hand than a field map — so the generalization happens at
the *measurement* seam, which is the seam that was hard-coded.

### Errors

Following spec 0025's posture. `ComponentDef::validate` returns a typed `ComponentDefError` naming
the definition:

- `UnsupportedVersion` — a definition declaring a `version` newer than this build understands.
- `EmptyName`, `SectionMissingSource`, `SectionMissingStyle`, `NoSections` — malformed shape.
- `DuplicateSectionSource` — two sections rendering the same field, which is always an authoring
  mistake and silently doubles content.
- `RepeatNotAPrefix` — a `repeat` section with ordinary content above it. See above.
- `BadMeasure` — a negative or non-finite measurement. See above.

Validation runs on document load (`Document::from_json`), so a malformed definition is refused
before it can produce geometry. A style name that does not resolve is **not** an error: it falls
back to `ParagraphStyle::default()`, the authoring posture the rest of the model takes.

An instance naming a definition that does not exist is skipped, exactly as an unresolved image asset
is (`measure_block` returns `None`). Layout has no error channel by design; the refusal happens at
load.

## Acceptance criteria

- `Document::sample()`'s export byte-hash unchanged.
- **The bundled stat block and table, re-expressed as definitions, produce byte-identical placed
  geometry to today** — asserted against captured `PlacedBlock` output for a corpus of fixtures that
  exercises wrapped cells, zebra bands, section rules, an absent section, a header-less table, an
  empty table and both split paths.
- A user-defined component with three sections lays out, splits at its section boundaries (0046) and
  preflights (0050) exactly as a built-in one does.
- A definition naming a style that does not exist still lays out.
- A malformed definition, and one naming a newer version, each fail with a typed error naming the
  definition.
- `quill import`'s `:::statblock` and `:::table` fences keep working unchanged.
- `benches/budgets.toml`: measurement cost per component unchanged — a definition is interpreted
  once per measurement, not per line.

## Non-goals

- **Executable extensions.** See `docs/roadmap.md`, "The decision this milestone turns on".
- A definition may not introduce a *new kind of geometry* — only new arrangements of the panel,
  runs, rules and bands the engine already emits. That is what keeps spec 0050's preflight
  authoritative over a packed component.
- Nested components. A section renders a field, not another component.
