# 0065 — A named run treatment, as there is a named paragraph treatment

**Milestone:** M5 · **Status:** implemented

## Why

Spec 0028's argument, one level down. Before paragraph styles existed, size and leading were crate
constants and every block in every document was set at body size; naming a treatment is what turned
"make every heading bigger" from an edit per heading into an edit per stylesheet.

Spec 0064 gave a *run* a treatment — a weight, a slant, a size, tracking, a baseline shift — and gave
it in exactly one form: an override written on the run itself. So a book that sets every lead-in bold
has that decision written out once per lead-in, and changing it is a search-and-replace across the
document. That is the state paragraph styles were rescued from.

It is also what makes `quill import` lossy in a way that matters. `**bold**` imports as a run whose
weight is 700 — the *value*, not the *intent*. The document no longer records that the author meant
emphasis, so a house style that sets emphasis as italic-not-bold has nothing to change.

## What

### The stylesheet gains a second map

```rust
pub struct StyleSheet {
    pub paragraph: BTreeMap<String, ParagraphStyle>,
    pub character: BTreeMap<String, CharacterStyle>,   // new
}
```

`CharacterStyle` carries exactly the fields `InlineStyle` carries, and every one of them is an
`Option`:

```rust
pub struct CharacterStyle {
    pub size_pt: Option<Pt>,
    pub color: Option<Color>,
    pub tracking_pt: Option<Pt>,
    pub baseline_shift_pt: Option<Pt>,
    pub weight: Option<Weight>,
    pub italic: Option<bool>,
}
```

The same shape and not a different one, because they are the same thing said in two places: a named
treatment and an unnamed one. A field that a character style could express but an inline override
could not would be a field an author could not opt out of.

`Run` gains `character: Option<String>` — the name of the style it is set in. It is beside
`style: InlineStyle`, not inside it, for the same reason `Block::style` is beside a block's fields:
a name is not an override, it is what the overrides are applied *to*.

### Precedence: paragraph, then character, then override, field by field

A run's resolved treatment is built in that order, and each layer only fills what the layer under it
left. Field by field, not wholesale: a run naming `strong` and overriding only `color` is bold *and*
recoloured, because the character style's weight is not displaced by an override of something else.

Written out, for one field:

| paragraph | character style | inline override | result |
|---|---|---|---|
| 10 pt | — | — | 10 pt |
| 10 pt | 12 pt | — | 12 pt |
| 10 pt | 12 pt | 14 pt | 14 pt |
| 10 pt | — | 14 pt | 14 pt |

**A run naming a style that does not exist lays out with the paragraph's treatment**, and its own
overrides still apply. This is the authoring-posture fallback specs 0028 and 0054 both take: a
missing name must not lose the text, and a document that names a style a pack was supposed to supply
should look wrong rather than fail to open. It is *reported* by the pack-resolution path (spec 0056),
which is where a missing name has a source to blame.

### The built-in character styles

The stylesheet's `Default` ships four names, and they are the ones `quill import` resolves to:

| name | what it sets |
|---|---|
| `emphasis` | italic |
| `strong` | bold |
| `strong-emphasis` | bold italic |
| `lead-in` | bold |

`emphasis` and `strong` are the names, not `italic` and `bold`: the point of naming a treatment is
that the name survives a change to the treatment. A house style that sets emphasis as letterspaced
small caps edits `emphasis`; a document that said `italic` would have to be re-authored.

**`code` is deliberately not among them.** The roadmap named it, and it is the one of the four that
cannot be honoured: a code style is a *monospace face*, and the bundled family has one design. A
`code` that set nothing but a slightly smaller size would look like a mistake rather than like code,
and shipping a named treatment that does not produce the treatment is the failure spec 0064's
announced-substitution rule exists to avoid. It becomes available the moment a document can name a
second family — spec 0004's work — and a `.qpack` can define it today under its own name once it can
carry a font.

`quill import` maps `**bold**` to `strong`, `*italic*` to `emphasis` and `***both***` to
`strong-emphasis`, rather than to the weight and slant values it wrote before. The values are what
the built-in styles resolve to, so nothing about the output moves.

### Packs carry them, and `pack extract` extracts them

A `.qpack` (spec 0055) carries a `StyleSheet`, so it carries character styles by construction. What
does not come free is `quill pack extract` (spec 0057): it walks a finished book and lifts the styles
it uses, and it now lifts the character styles that the book's runs name. A pack whose "look" is half
a look — paragraph treatments but not the run treatments they were designed against — is the defect
this criterion exists to catch.

### The cache

A character style is a *shared* input: editing one must reflow every block that names it and no
others. `LayoutSession`'s style fingerprint already covers the whole `StyleSheet` by `Debug`, so
editing the `character` map invalidates everything that measured against it — which is correct but
coarse. It is left coarse, and said so: a stylesheet edit is an author action at editing frequency,
the same trade spec 0028's paragraph map already makes, and narrowing it means a per-style
dependency edge that no other style map has. What the increment does assert is the claim spec 0031
makes: that editing a character style reflows the blocks that use it, measured with a work counter.

## Acceptance criteria

- A run naming a character style resolves it; a run naming one that does not exist still lays out
  with the paragraph's treatment, and its own overrides still apply.
- Precedence is asserted **field by field**, with a test per field, over all three layers.
- A document that names no character style exports **byte-identically** to what 0064 produced —
  `SAMPLE_EXPORT_DIGEST` does not move.
- The four built-in styles ship in `StyleSheet::default()` and are what `quill import` resolves
  emphasis to; an imported document is therefore *styled* rather than overridden, asserted by
  reading the run's name rather than its weight.
- Editing a character style reflows the blocks that use it, asserted with a work counter rather than
  a timing.
- `quill pack extract` extracts the character styles a book's runs name, and a pack that carries them
  installs them.
- `FORMAT_VERSION` stays **4**: both new fields are additive with serde defaults.

## Non-goals

- **A `code` style**, for the reason above. Named, not forgotten.
- **Nested or inherited character styles.** A style names values, not another style. Inheritance is a
  second resolution order to get wrong, and no evidence yet says an author wants it.
- **Character styles inside a declared component's sections.** Those are plain strings until a later
  increment gives them runs; spec 0063 named that boundary and this spec does not move it.
