# Authoring a content pack

A **content pack** is how one publisher's work becomes usable by another: a versioned bundle of
templates, styles, component definitions and assets, with a manifest that says where it came from.
This is the guide to writing one.

Specs: [0054](../specs/0054-component-definitions.md) (component definitions),
[0055](../specs/0055-pack-container.md) (the `.qpack` container),
[0056](../specs/0056-pack-resolution.md) (resolution),
[0057](../specs/0057-pack-extract.md) (extraction).

Every JSON example below is parsed by `crates/core-model/tests/pack_guide.rs`. A guide whose
examples have quietly stopped compiling is worse than no guide, so they are tested rather than
proof-read.

---

## The decision this format turns on: a pack contains no code

"Plugin" usually means an *executable extension* — a dynamic library, a scripting API, a WASM
module. **Quill deliberately does not build that**, and this is the reasoning, recorded here so the
next person who wants one finds an argument rather than a gap.

Quill's first rule is *prefer a visible failure over silent press-corruption*. An executable plugin
that can emit geometry can emit geometry that is wrong — off the trim, over the ink limit, in the
wrong colour space — and a plugin author debugging on screen has no way to know. Every mechanism the
tool has for making press errors visible (placed-geometry preflight, POD preset thresholds, provable
annotation-freedom in the press profile) assumes **quill** produced the geometry. Handing that to
third-party code would either bypass those checks or force every one of them to run against an
adversary, which is a different product.

A declarative pack cannot do any of that. It supplies *inputs* — a trim, a style, a component's
shape — and quill lays them out through the same engine it lays everything else out through. So
preflight governs a community component exactly as it governs a bundled one. The colour type a
definition uses has no RGB family at all, so a pack cannot even *express* a colour space PDF/X-1a
forbids.

The audience wants to share a look, not to ship a program. If executable extensions are wanted
later, this format is the right substrate to hang them on, and the sandboxing question can be
answered once rather than assumed away.

---

## The manifest

A pack is a zip holding `pack.json` and an `assets/` directory. `pack.json` is the whole
declaration:

```json pack-manifest
{
  "pack_version": 1,
  "name": "ashen-vault",
  "title": "The Ashen Vault",
  "description": "A house style for grim dungeon-crawl books.",
  "version": "1.2.0",
  "source": "https://example.invalid/ashen-vault",
  "license": "CC-BY-4.0",
  "styles": { "paragraph": {} }
}
```

**Two versions, and they mean different things.** `pack_version` is quill's container format; a pack
declaring one newer than the build understands is refused, not best-effort loaded. `version` is
*your* content version — "Ashen Vault 1.2.0" makes no claim about quill's format.

**`source` and `license` are required and must not be empty.** Content arriving from a stranger with
no provenance is content nobody should install, so this is checked when a pack is read *and* when it
is written: a pack that cannot be installed cannot be produced in the first place.

---

## Component definitions

A component is a declared sequence of styled sections inside a panel. The two components quill
ships — the stat block and the table — are *defined this way themselves*, which is the guarantee
that the format is not a second-class one.

Here is a shape quill has no Rust type for at all: a PbtA **move**.

```json component-def
{
  "version": 1,
  "name": "move",
  "panel": {
    "fill": { "space": "gray", "v": 0.94 },
    "stroke": { "color": { "space": "gray", "v": 0.4 }, "width_pt": 0.75 },
    "padding_pt": 8.0
  },
  "sections": [
    { "source": "name", "style": "move-name", "shape": "text" },
    {
      "source": "trigger",
      "style": "move-trigger",
      "shape": "lines",
      "rule_above": {
        "color": { "space": "gray", "v": 0.45 },
        "thickness_pt": 0.5,
        "gap_above_pt": 3.0,
        "gap_below_pt": 3.0
      }
    },
    {
      "source": "outcomes",
      "style": "move-outcome",
      "shape": "pairs",
      "separator": ":\u00a0",
      "rule_above": {
        "color": { "space": "gray", "v": 0.45 },
        "thickness_pt": 0.5,
        "gap_above_pt": 3.0,
        "gap_below_pt": 3.0
      }
    }
  ],
  "split": { "granularity": "sections", "min_items": 1, "keep_together": true }
}
```

### Sections

Each section names an instance field (`source`), a paragraph style (`style`), and a **shape** saying
how that field's value becomes runs:

| `shape` | field value | emits |
|---|---|---|
| `text` | a string | exactly one run, **always** — even when the field is missing |
| `lines` | a list of strings | one run per line, and **nothing at all** when the list is empty |
| `pairs` | a list of `[key, value]` | one run per pair, `key⟨separator⟩value` |
| `rows` | a list of lists of strings | one row of cells per element, across the component's columns |

The `text`/`lines` distinction is the one worth internalising. A `text` section is what *opens* a
component and always emits, so an unnamed one is still a panel. A `lines` section that is empty
emits nothing — **and therefore draws no rule and offers no cut point**, which is what stops an
absent `overview` from leaving a stray hairline behind.

A `pairs` section joins the key's own words with U+00A0, so `Armour Class:` cannot break across
lines. Give the style a hanging indent and a wrapped value lines up under the value rather than under
the key.

### Rules

`gap_above_pt` and `gap_below_pt` advance the vertical cursor; **the rule's own thickness does not**.
That looks odd until you need both bundled behaviours from one shape: a stat block's hairline sits
*inside* its lower gap, while a table's header rule advances by exactly its thickness — expressed by
setting `gap_below_pt` to the thickness. `full_width` spans the panel edge to edge instead of the
padded measure.

The first section that actually emits never draws its `rule_above`. It has the panel's own edge
above it.

### Splitting

`granularity` says what a fragment is made of when the component is cut across a frame boundary:

- `sections` — one item per emitted section. A stat block, whose attributes list is never separated
  from itself.
- `elements` — one item per element of a non-repeated section. A table's rows.
- `whole` — indivisible; it moves or it does not.

`min_items` is the fewest items a fragment may contain: **one** for coarse items (a whole section)
and **two** for fine ones (a row). Two *sections* per fragment can make the smallest legal cut larger
than a frame, at which point nothing is cut and the panel runs off the page.

A section marked `"repeat": true` is the prefix every continuation re-states — a table's header row.
It must come **before every ordinary section**: the interpreter builds the prefix from everything
emitted so far, so a repeated section with content above it would re-state that content too and a cut
component would duplicate it on every continuation. Refused when the pack is read.

### Colour

Two families, `gray` and `cmyk`:

```json def-color
{ "space": "cmyk", "c": 0.1, "m": 0.9, "y": 0.8, "k": 0.05 }
```

There is deliberately no RGB. A press file must not contain it, and the cheapest way to guarantee a
pack never introduces one is for the format to be unable to say it.

### Measurements

Every measurement — `padding_pt`, `width_pt`, `thickness_pt`, `gap_above_pt`, `gap_below_pt`,
`cell_padding_pt` — must be **finite and non-negative**, and a pack declaring otherwise is refused.

Unlike a style name, geometry has no sane fallback. A negative panel padding draws the component's
text outside its own panel and off the page, and the safe-area preflight exempts anything outward of
trim as deliberate bleed — so a bad number would reach paper with nothing reporting it.

### The styles a definition names

Ship them. A definition names style *names*; if the pack does not carry them, the component sets in
the default face on every machine but its author's.

```json styles
{
  "paragraph": {
    "move-name": {
      "font_size_pt": 12.0,
      "leading_pt": 15.0,
      "align": "left",
      "space_before_pt": 0.0,
      "space_after_pt": 2.0,
      "indent": { "first_pt": 0.0, "rest_pt": 0.0 }
    },
    "move-trigger": {
      "font_size_pt": 9.5,
      "leading_pt": 12.5,
      "align": "left",
      "space_before_pt": 0.0,
      "space_after_pt": 3.0,
      "indent": { "first_pt": 0.0, "rest_pt": 0.0 }
    },
    "move-outcome": {
      "font_size_pt": 9.5,
      "leading_pt": 12.5,
      "align": "left",
      "space_before_pt": 0.0,
      "space_after_pt": 2.0,
      "indent": { "first_pt": 0.0, "rest_pt": 12.0 }
    }
  }
}
```

The hanging indent on `move-outcome` is what makes a wrapped `10+: hold 3` line up under the outcome
rather than under the roll.

---

## Using a component

An instance is a `Block` naming the definition and carrying the fields:

```json block
{
  "kind": "component",
  "def": "move",
  "color": { "space": "gray", "v": 0.0 },
  "fields": {
    "name": { "kind": "text", "value": "Read a Sitch" },
    "trigger": {
      "kind": "lines",
      "value": ["When you size up a charged situation, roll +sharp."]
    },
    "outcomes": {
      "kind": "pairs",
      "value": [
        ["10+", "hold 3"],
        ["7-9", "hold 1"],
        ["6-", "the MC holds 1 on you"]
      ]
    }
  }
}
```

Field values are tagged with `kind` and carry their payload under `value`.

A field the definition names and the instance omits contributes nothing; a field the instance
carries and the definition does not name is ignored. Both are authoring conveniences, not errors.
A style name that does not resolve falls back to the default style — a missing style costs the
*look*, never the content.

---

## Requiring a pack

A document names what it needs:

```json requires
[
  { "name": "grimdark", "version": "1" },
  { "name": "pbta-moves", "version": "1.0.0" }
]
```

`version` is an exact version or a **dotted prefix** of one. `"1.2"` matches `1.2.0` and `1.2.9` but
not `1.3.0`; `"1"` matches every `1.x` and — because matching is on whole dotted components, never on
characters — does **not** match `10.0.0`. An empty string accepts any installed version. Among
matches the highest wins, compared numerically where both components parse, so `1.10.0` beats
`1.9.0`.

A requirement that does not resolve **stops the document from laying out**, with an error naming the
pack, the version asked for, and what is actually installed. It is never a quiet fallback to the
default style: a book that looks subtly wrong is worse than one that refuses to open, and the
difference is usually discovered at the print shop.

### Precedence

```
bundled  <  packs  <  the document's own
```

A pack defining `statblock` is a legitimate restyle of the bundled one. A document defining it beats
both, because the document is the thing being edited.

**Two installed packs defining the same component name is an error naming both** — not
last-one-wins, which would make the winner depend on the packs' names. Paragraph *styles* are
deliberately exempt and merge: a style name is shared vocabulary (`body`, `h1`), and refusing there
would make any two packs uninstallable together.

---

## The commands

```
quill pack info <file>              # identity, provenance, contents
quill pack install <file> [--packs <dir>] [--force]
quill pack list [--packs <dir>]
quill pack extract <document> --name <slug> --version <v> \
      --source <s> --license <l> --output <file.qpack>
```

`info` and `install` take either a zipped `.qpack` or a bare `pack.json` — which is what you have on
disk while you are writing one, and zipping between every edit is a build step nobody needs.

Packs are installed under `$QUILL_PACKS`, else the platform data directory
(`$XDG_DATA_HOME/quill/packs`, `$HOME/.local/share/quill/packs`, or `%APPDATA%\quill\packs`), at
`<root>/<name>/<version>/`. Name *and* version in the path, so two versions coexist.

### Extracting a pack from a book you already made

You do not have to write any of this by hand. If the house style already exists as a document:

```
quill pack extract book.tpub --name my-house --version 1.0.0 \
      --source https://example.com/me --license CC-BY-4.0 --output my-house.qpack
```

Its templates, styles and component definitions travel. **Its content does not** — a pack is a look,
not a book. Assets referenced by *master furniture* travel, because a rule ornament in a running head
is part of the design; assets placed in the flow stay behind, because they are the art.

---

## Worked examples

Two packs ship in [`../examples/packs/`](../examples/packs), both installable as-is:

- **`grimdark.json`** — a house style. A 6×9 template with a baseline grid, a full type scale, and a
  restyled stat block with a heavier panel. This is the shape most packs will be.
- **`pbta-moves.json`** — a component pack. The `move` definition above, plus the three styles it
  resolves. It ships no template at all: it is a vocabulary, not a design.

```
quill pack install examples/packs/grimdark.json
quill pack install examples/packs/pbta-moves.json
quill pack list
```

A definition ships with the styles it names, and should: a definition whose styles were left behind
produces a component set in the default face on every machine but its author's.

---

## Things that will refuse

Deliberately, and each with an error naming the thing that is wrong:

| What | Why |
|---|---|
| a `pack_version` newer than the build | a pack half-understood lays out geometry that goes to a printer |
| an empty `source` or `license` | provenance is not optional |
| an asset path that is absolute or contains `..` | a pack is the one artifact that routinely comes from someone else |
| a definition with no sections, no `source`, or no `style` | it cannot lay anything out |
| a `repeat` section with ordinary content above it | every continuation would duplicate that content |
| a negative or non-finite measurement | it would draw off the page with nothing reporting it |
| a pack `name` or `version` containing `..`, `/`, or nothing | they build the install path, so either would escape the pack root |
| two sections rendering the same field | it silently sets the content twice |
| a definition filed under a name it does not answer to | the definition validated is not the one that would be laid out |
| a required pack that is not installed | see above — never a quiet fallback |
| two packs defining the same component | the winner would depend on the packs' names |
