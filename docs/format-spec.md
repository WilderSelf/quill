# Quill file format (`.tpub`) — draft spec

**Status:** draft. The format is intentionally **open, documented, and versioned** — a
differentiator versus proprietary `.afpub` / `.indd`.

## Container

A `.tpub` file is a **Zip container** (like `.idml`/ODF). Layout:

```
document.json      # manifest + document model (see below)
assets/            # linked originals (images, etc.), referenced by relative path
fonts/             # fonts used by the document (for portability + embedding at export)
thumbnails/        # optional cached preview images
```

Rationale: linked assets (not inlined) keep the manifest small and diffable and keep large
art out of the model — the same principle that lets the editor stay fast on 500-page,
image-heavy books. The manifest is JSON (text) so projects are **git-friendly**.

## Manifest (`document.json`)

Top-level shape (the authoritative schema is the `serde` types in `quill-core-model`; this example
is **parsed by a test**, so it cannot drift from what the reader accepts):

```json
{
  "format_version": 5,
  "metadata": { "title": "The Ruined Keep", "authors": ["Anon"] },
  "page_setup": { "trim": { "w_pt": 468.0, "h_pt": 720.0 }, "bleed_pt": 9.0, "facing_pages": true,
                  "margins": { "top_pt": 36.0, "bottom_pt": 36.0, "inside_pt": 54.0, "outside_pt": 36.0 } },
  "master_pages": [
    { "name": "body", "columns": 2, "gutter_pt": 12.0,
      "margins": { "top_pt": 36.0, "bottom_pt": 36.0, "inside_pt": 54.0, "outside_pt": 36.0 },
      "statics": [
        { "kind": "text", "rect": { "x_pt": 54.0, "y_pt": 18.0, "w_pt": 378.0, "h_pt": 12.0 },
          "text": "The Dungeon", "color": { "space": "gray", "v": 0.0 },
          "style": "folio", "align": "center" },
        { "kind": "text", "rect": { "x_pt": 54.0, "y_pt": 690.0, "w_pt": 378.0, "h_pt": 12.0 },
          "text": "{page}", "color": { "space": "gray", "v": 0.0 },
          "style": "folio", "align": "outside", "mirror": true }
      ] },
    { "name": "chapter-opener", "columns": 1, "gutter_pt": 0.0, "statics": [] }
  ],
  "default_master": "body",
  "pages": [ {}, {} ],
  "sections": [ { "name": "The Ruined Keep", "start": 1, "master": "chapter-opener" } ],
  "styles": { "paragraph": {
    "body": { "font_size_pt": 10.0, "leading_pt": 12.0, "align": "justified" },
    "folio": { "font_size_pt": 9.0, "leading_pt": 12.0, "align": "left" } } },
  "content": [
    { "kind": "heading", "id": 1, "level": 1,
      "runs": [ { "text": "The Ruined Keep" } ],
      "color": { "space": "gray", "v": 0.0 } },
    { "kind": "body", "id": 2,
      "runs": [ { "text": "A dank corridor stretches into " },
                { "text": "darkness", "style": { "italic": true } },
                { "text": "." } ],
      "color": { "space": "cmyk", "c": 0.0, "m": 0.0, "y": 0.0, "k": 1.0 } },
    { "kind": "image", "id": 3, "asset": "map1" }
  ],
  "revision": 0,
  "next_block_id": 4,
  "assets": [ { "id": "map1", "path": "assets/map1.png", "px_w": 1500, "px_h": 1200, "dpi": 300.0 } ]
}
```

## Margins and master pages

Margins are `inside`/`outside`, not left/right: a bound book's margins are relative to the spine, so
the inside margin falls on the left of a recto and the right of a verso. A `MasterPage` names the
margins, column count, gutter and repeating furniture shared by the pages it governs; `{page}` in a
static's text resolves to the one-based page number. `default_master` names the master applied to
the document; an unknown name degrades to the document's own page setup rather than refusing to open
the file.

`pages` (spec 0035) overrides that default per page, positionally — `pages[i]` governs page `i`.
A page's master is its own override, else `default_master`, else none, and a name matching no master
falls through to the next step rather than failing, on the same principle: a renamed master costs
the page its furniture, never the author their page. The list need not match the document's length;
pages beyond it fall back, and entries beyond the document are ignored. The list is omitted from the
manifest entirely when empty, so a document that never assigns a master reads exactly as it did
before spec 0035 — which is why this addition needed no `format_version` bump.

Because assignment is positional, content that pushes the book by a page slides every subsequent
assignment. That was the accepted trade for M2, and spec 0072 removes it — see below.

## Sections (spec 0072, v5)

A **section** is a named division of the document anchored to the block that opens it:

```json
"sections": [ { "name": "The Ruined Keep", "start": 1, "master": "chapter-opener" } ]
```

- `name` — what the section is called. Authored rather than taken from the anchor's text, because a
  running head ("The Ruined Keep") and a chapter opener ("Chapter One: The Ruined Keep") are
  routinely different strings.
- `start` — the `id` of the block the section opens with. Any block, not only a heading.
- `master` — optional; the master applied to the section's **opening page**.

A section does not replace the `pages` list: it **generates** it. Each layout pass reads the page
each anchor was placed on, and synthesises the same `Vec<PageOverride>` described above from those
page numbers, overlaid on whatever the document authored positionally. Resolution is unchanged —
`pages[i]`, else `default_master`, else the document's page setup — so a section is an *authoring
surface* over the representation the format already had.

That is what makes the assignment survive repagination: a block id is stable across every edit that
does not delete the block, so inserting a chapter in front of another moves the second chapter's
opener with it, where a positional entry would stay on the page number it names. Where a section and
a positional entry claim the same page, the section wins — it is the one that tracked the content.

Two consequences worth stating:

- **Master assignment is part of the layout fixpoint.** A section's opener master changes that
  page's margins and column count, which changes where later content falls, which can move the
  anchor. Layout therefore iterates — sharing the contents list's loop and its bound (8 passes) —
  and reports whether it settled rather than presenting the last iterate as an answer.
- **A section is a marker, not a container.** It does not own its blocks; there is no tree, and a
  section runs until the next section's anchor. An anchor naming a block the document does not
  contain is not an error: the section is simply not placed, on the same principle a dangling master
  name falls through rather than failing.

A template file carries no sections and cannot: it has no content, therefore no block ids to anchor
to. This is why `TEMPLATE_VERSION` did not move when `format_version` became 5.

Not in this increment, and named rather than forgotten: **"start this section on the next recto"**.
That is a forced page break, which the model has no mechanism for at all, and it is a forward-only
rule rather than a fixpoint.

## Master statics: alignment and page parity (spec 0047, v3)

A text static carries two optional fields, both defaulting to the pre-v3 behavior and both omitted
from the manifest when they do:

- `align` — `left` (default), `center`, `right`, `inside`, `outside`. Where the line sits **within
  its rect**. `inside`/`outside` resolve to left/right by page parity, the same rule the margins
  use: a recto has the spine on its left, and with `facing_pages` off every page is a recto.
  A static is one unbroken line, so there is no `justified`.
- `mirror` — when true, the rect itself is reflected about the page's vertical centre on a verso
  (`x' = trim.w - (x + w)`). The rect is authored **as it looks on a recto**, exactly as margins are
  authored inside/outside.

Both are needed to place a folio at the outside corner of a spread, because the band it sits in is
itself asymmetric whenever the inside and outside margins differ: `mirror` moves the band to the
right half of the page, `align` puts the number at its fore-edge end. Alignment is resolved at
layout, against the same font metrics the text is broken with, so a placed static's frame reports
the measured line rather than the band it was aligned in.

## Versioning

`format_version` is an integer. Readers reject formats newer than they understand and migrate
older ones forward. Migrations are documented per bump.

Implemented in `quill-core-model` (spec 0025): the gate runs on the untyped JSON *before*
deserialization, since an older manifest by definition does not fit the current `serde` types. A
manifest newer than this build is refused with `LoadError::UnsupportedVersion` rather than loaded
with its unknown fields dropped — a half-loaded document saved back over the original would destroy
whatever this build did not understand.

| `format_version` | Behavior |
|---|---|
| absent | treated as current |
| older | migrated forward, one step per version |
| current | loaded as-is |
| newer | refused |

### The migrations

Each step brings a manifest forward exactly one version and falls through to the next, so a
document written by any released build reaches the current types in one load. All but one are
structurally no-ops — every field they add is `serde(default)` and the default is what the older
version *meant* — and they are written out anyway, so the chain reads as a record of what each
version changed.

| From → to | Spec | What changed | Migration |
|---|---|---|---|
| 1 → 2 | 0030 | `page_setup.margins`, `master_pages`, `default_master` | defaults `margins` to zero on every edge and `master_pages` to empty: a v1 document had no margins and no masters |
| 2 → 3 | 0047 | `align` and `mirror` on a text master static | defaults `align` to `left` and `mirror` to `false` on every text static: a v2 static was drawn from its rect's left edge in the same rect on both halves of a spread |
| 3 → 4 | 0063 | a paragraph's single `text` becomes an ordered list of styled `runs` | **the one migration that is not a structural no-op**: `text` is removed from every `heading` and `body` block and replaced by `runs: [{"text": …}]`. A `toc` title and a panel's fields are not paragraphs and keep their strings. Still a serialization no-op *in effect* — one plain run carries no style object — so a migrated v3 document lays out and exports as it did |
| 4 → 5 | 0072 | `sections` | defaults `sections` to empty: a v4 document had no sections, and every master assignment it had was positional |

A bump is warranted whenever an **older** build would open the document and silently lay it out
wrongly, even when the new fields are optional. A pre-0030 build would drop a document's running
heads and column geometry; a pre-0047 build would put every verso's folio in the gutter; a pre-0072
build would drop `sections` and set every chapter opener in the body master. Refusing to open beats
any of them, because a half-understood document can be saved back over the original — and the loss
is worse for authored intent than for derived state: block ids can be regenerated, a section list
cannot.

Note what the rule does *not* turn on. Not "did a struct gain a field" — spec 0035's `pages`, spec
0054's `components` and spec 0056's `requires` were all additive without a bump, because an older
build that drops them lays the document out exactly as the author would have got before writing
them. And not "is the field optional" — every field in the table above is optional too. The question
is only whether the older build's silence produces a *different book*.

> **Bumping `FORMAT_VERSION`? Check whether `TEMPLATE_VERSION` is owed one too.** A template file
> (below) embeds `page_setup`, `styles`, `master_pages` and `pages`. A document bump that changes
> the serialized shape of any of those four changes every template file as well, and the template
> format has its own version and its own migration chain. Spec 0047's 2 → 3 was exactly such a bump.
> Spec 0072's 4 → 5 was checked against this and is **not**: it adds a field to the document beside
> those four and changes none of them, and a template cannot carry a section anyway.

## Template files (spec 0053)

A **template file** is everything a document has except its content: trim, margins, a type scale,
master pages and the per-page assignments. `quill new --from house-style.json -o book.tpub` starts a
document from one. It is a separate published format from `.tpub` — a plain JSON file, not a
container, because a template links no assets.

```json
{
  "template_version": 1,
  "name": "house-style",
  "title": "House style (6×9, two columns)",
  "description": "The house two-column 6×9, with a chapter opener and page numbers.",
  "page_setup": { "trim": { "w_pt": 432.0, "h_pt": 648.0 }, "bleed_pt": 9.0, "facing_pages": true,
                  "margins": { "top_pt": 54.0, "bottom_pt": 54.0, "inside_pt": 54.0, "outside_pt": 40.0 } },
  "styles": { "paragraph": {
    "body": { "font_size_pt": 9.5, "leading_pt": 12.5, "align": "justified" },
    "h1": { "font_size_pt": 22.0, "leading_pt": 26.0, "align": "left", "space_before_pt": 19.5 },
    "folio": { "font_size_pt": 8.5, "leading_pt": 12.5, "align": "left" } } },
  "master_pages": [
    { "name": "chapter-opener", "columns": 2, "gutter_pt": 14.0,
      "margins": { "top_pt": 216.0, "bottom_pt": 54.0, "inside_pt": 54.0, "outside_pt": 40.0 },
      "statics": [] },
    { "name": "body", "columns": 2, "gutter_pt": 14.0,
      "margins": { "top_pt": 54.0, "bottom_pt": 54.0, "inside_pt": 54.0, "outside_pt": 40.0 },
      "statics": [
        { "kind": "text", "rect": { "x_pt": 54.0, "y_pt": 606.0, "w_pt": 338.0, "h_pt": 12.0 },
          "text": "{page}", "color": { "space": "gray", "v": 0.0 },
          "style": "folio", "align": "outside", "mirror": true }
      ] }
  ],
  "default_master": "body",
  "pages": [ { "master": "chapter-opener" } ]
}
```

The example above is parsed by a test, so it cannot drift from what the loader accepts.

`name` is the slug the success line reports; `title` and `description` are what `quill new --list`
shows for the bundled three. `styles`, `master_pages`, `default_master` and `pages` all default, so
the shortest useful template is a `page_setup` and a name — but `styles` is built on the default
sheet whenever it is omitted, so a template can never be missing a style the resolver expects. A
style name nothing resolves falls back to `body` and then to the default treatment: losing a
paragraph's *styling* beats losing the paragraph, which is the same posture a dangling master name
gets.

### Versioning a template file

`template_version` is an integer, and a **separate one** from the document's `format_version`: a
template file is not a document, and coupling them would re-version every template ever written
whenever the document model changed in a way templates never see. The rules are otherwise identical
to the document's — the gate runs on the untyped JSON before deserialization, and a file newer than
this build is refused (`LoadError::UnsupportedTemplateVersion`) rather than half-loaded.

| `template_version` | Behavior |
|---|---|
| absent | treated as current |
| older | migrated forward, one step per version |
| current | loaded as-is |
| newer | refused |

`TEMPLATE_VERSION` is **1** and the migration chain is empty, because nothing is older than v1. It
is owed a bump on either of two triggers:

1. the template envelope changes — a field added to or removed from the template itself; or
2. a `format_version` bump changes the serialized shape of `page_setup`, `styles`, `master_pages` or
   `pages`, the four document structures a template file embeds.

Trigger 2 is the one that is easy to miss, which is why it is also flagged beside the document
migration table above.

### Composing a template with a POD preset

A POD preset (spec 0049) carries the printer's geometry; a template carries a design. Both can state
a trim, so the precedence is fixed rather than left to the invocation:

- **Trim: the template wins.** A template's furniture is authored against a specific trim — a folio's
  `y_pt` is derived from the page height and its rect from the trim width. Re-trimming from
  underneath moves the page without moving the geometry authored for it, and nothing at layout time
  catches it, because furniture does not participate in the flow. A preset whose trim differs is
  *reported*; an unusual trim is a conversation with the printer, not a corrupt file.
- **Bleed: the larger of the two wins.** Bleed is a floor, not a design choice, and it lives entirely
  outside the trim box — so honoring a stricter press requirement costs the design nothing, and
  lowering it would cost something.

With no template at all, a preset seeds both outright.

## Reading a container

`Tpub::read_manifest` reads `document.json` alone; `Tpub::open_into` also extracts the payload to a
caller-named directory and returns the `asset_root` that relative `Asset.path` values resolve
against. Extraction is explicit rather than into a hidden temp directory, so "where are this
document's assets right now" stays answerable. Entry names that would escape the extraction
directory (`..`, absolute paths, drive prefixes) are refused rather than sanitized.

## Block identity

Every content block carries an integer `id`, unique within the document and stable across saves
(spec 0026). Identity is what lets incremental layout cache per block: an index would renumber on
every insert. `revision` is a monotonic edit counter; `next_block_id` is the allocator's state,
persisted so a reload can never hand a deleted block's id to a new one.

All three fields default when absent, so manifests written before they existed still load without a
version bump. Two blocks sharing an id is an error — a duplicate identity would let a cache return
the wrong block's layout.

## What the format deliberately does *not* carry: the POD preset

A **POD preset** (spec 0049 — the bleed, safety margin, ink limit, minimum resolutions, trim
catalogue and PDF/X level one printer states) is an **export-time** concern and is deliberately not
serialized into `.tpub`. A document is not bound to one printer: the same manuscript is routinely
quoted to two, and a preset baked into the manifest would make "which vendor is this book for?" a
property of the file rather than of the export the author is doing right now.

So a preset travels on the command line (`quill preflight --preset …`, `quill export --preset …`),
never in the document. What *is* persisted is the **effect** a preset had at authoring time:
`quill new --preset …` seeds `page_setup.bleed_pt` (and the trim, when the template's own trim is
not one the preset lists) — ordinary page-setup fields that any later export re-checks against
whatever preset it is given. A test asserts a manifest written this way contains no preset.

The presets themselves are code, not data files, and each carries the `source` it was taken from
and the date it was `retrieved`. **Bundled vendor presets are a convenience to be confirmed against
the vendor's current specification, not a warranty**; `quill presets` prints that provenance, and a
preset whose numbers were not read from the vendor is marked unconfirmed and says so when selected.

## Two linked representations

The model carries both a **semantic content** tree (the easy authoring layer) and a **layout**
(master pages, the per-page assignment list, frames and threads — the pro layer); frames reference
content, so editing content reflows layout. See `CLAUDE.md` and the roadmap for how these interact.

("Spreads" appeared in earlier drafts of this document as the layout container. No such type was
ever built: the layout layer is `master_pages` + `pages`, described above. Facing-page behavior
lives in `page_setup.facing_pages` and in the inside/outside margins, not in a spread object.)

## Appendix: the authoring syntax (spec 0043)

A small line-oriented syntax that imports to a document. Deliberately a **subset**, not CommonMark:
the constructs below are all of it, and everything else is an explicit non-goal. Round-tripping back
out to this syntax is also a non-goal.

- `#` … `######` followed by a space — a heading of that level. `#1` is a word, not a heading.
- Blank-line-separated runs of text — a body paragraph; newlines inside one are soft.
- `**bold**` / `__bold__` and `*italic*` / `_italic_` inside a heading or a paragraph (spec 0064) —
  a run set in the family's bold or italic face. They nest: `***both***` is bold italic, and
  `**bold with *italic* inside**` is what it reads as. An **unmatched** delimiter is literal, so
  `2*d6` and `a_b_c` are the text they look like: an opener must be followed by text and a closer
  preceded by it, `_` does not pair inside a word, and a pair is always the same character.
- `![alt](asset-id)` — an image block referencing a linked asset by id.
- `:::panel` … `:::` — a titled record of named sections, one `key: value` per line, where `key` is
  `name`, `overview`, `detail`, `action`, `reaction`, or `attr` (whose value is `name = value`).
  `:::statblock` is the same fence under its pre-0062 name and parses identically; both spellings
  are permanent.
- `:::table` … `:::` — pipe-delimited rows; the first is the header, and a `|---|` separator row is
  ignored as markdown furniture.
- `- item` / `* item` / `1. item` — a list item. The ordinal written in the source is ignored:
  markers are derived from document order when the document is laid out, so inserting an item
  renumbers what follows.
- `:::toc` … `:::` — a generated contents list, taking `title:` and `max_level:`.

Input the importer does not understand is **never silently dropped**. An unknown fence (`:::foo`) is
an error, because the author clearly meant a structured object and both guessing and discarding lose
real content. Everything else is kept as body text with a warning naming the line — a paragraph that
came out as plain prose is visible and fixable; one that vanished is not.

```quill-import
# The Ruined Keep

A dank corridor stretches
into darkness.

![a map](map1)

:::panel
name: Astrolabe, mariner's
overview: Cast brass, Iberian, c. 1580.
attr: Diameter = 180 mm
detail: Alidade replaced; limb graduated in single degrees.
:::

:::table
| d20 | Encounter |
|-----|-----------|
| 1-4 | Nothing   |
:::

:::toc
title: Contents
max_level: 2
:::
```

The example above is parsed by a test, so it cannot drift from what the importer accepts.
