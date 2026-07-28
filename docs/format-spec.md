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

Top-level shape (illustrative; the authoritative schema is the `serde` types in
`quill-core-model`):

```json
{
  "format_version": 2,
  "metadata": { "title": "...", "authors": ["..."] },
  "page_setup": { "trim": { "w_pt": 468, "h_pt": 720 }, "bleed_pt": 9.0, "facing_pages": true,
                  "margins": { "top_pt": 36, "bottom_pt": 36, "inside_pt": 54, "outside_pt": 36 } },
  "master_pages": [ { "name": "body", "columns": 2, "gutter_pt": 12,
                      "statics": [ { "kind": "text", "rect": { ... }, "text": "The Dungeon — {page}" } ] } ],
  "default_master": "body",
  "pages": [ { "master": "chapter-opener" }, {} ],
  "styles": { "paragraph": { "body": { "font_size_pt": 10.0, "leading_pt": 12.0, "align": "justified" }, "h1": { ... } } },
  "content": [ /* semantic blocks: headings, body, stat blocks, tables, random tables */ ],
  "revision": 0,
  "next_block_id": 4,
  "assets": [ { "id": "...", "path": "assets/....png", "px_w": 1500, "px_h": 1200, "dpi": 300 } ]
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
