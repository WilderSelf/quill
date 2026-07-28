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
  "format_version": 3,
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
  "pages": [ { "master": "chapter-opener" }, {} ],
  "styles": { "paragraph": {
    "body": { "font_size_pt": 10.0, "leading_pt": 12.0, "align": "justified" },
    "folio": { "font_size_pt": 9.0, "leading_pt": 12.0, "align": "left" } } },
  "content": [
    { "kind": "heading", "id": 1, "level": 1, "text": "The Ruined Keep",
      "color": { "space": "gray", "v": 0.0 } },
    { "kind": "body", "id": 2, "text": "A dank corridor stretches into darkness.",
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
assignment. That is the accepted trade for M2; anchoring a master to the chapter it opens is
recorded as an open question in `docs/roadmap.md`.

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
document written by any released build reaches the current types in one load. Both migrations so far
are structurally no-ops — every field they add is `serde(default)` and the default is what the older
version *meant* — and both are written out anyway, so the chain reads as a record of what each
version changed.

| From → to | Spec | What changed | Migration |
|---|---|---|---|
| 1 → 2 | 0030 | `page_setup.margins`, `master_pages`, `default_master` | defaults `margins` to zero on every edge and `master_pages` to empty: a v1 document had no margins and no masters |
| 2 → 3 | 0047 | `align` and `mirror` on a text master static | defaults `align` to `left` and `mirror` to `false` on every text static: a v2 static was drawn from its rect's left edge in the same rect on both halves of a spread |

A bump is warranted whenever an **older** build would open the document and silently lay it out
wrongly, even when the new fields are optional. A pre-0030 build would drop a document's running
heads and column geometry; a pre-0047 build would put every verso's folio in the gutter. Refusing to
open beats either, because a half-understood document can be saved back over the original.

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
- `![alt](asset-id)` — an image block referencing a linked asset by id.
- `:::statblock` … `:::` — one `key: value` per line, where `key` is `name`, `overview`, `detail`,
  `action`, `reaction`, or `attr` (whose value is `name = value`).
- `:::table` … `:::` — pipe-delimited rows; the first is the header, and a `|---|` separator row is
  ignored as markdown furniture.
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

:::statblock
name: Goblin
overview: Small humanoid, chaotic evil
attr: Armour Class = 15
action: Scimitar. +4 to hit.
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
