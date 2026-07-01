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
  "format_version": 1,
  "metadata": { "title": "...", "authors": ["..."] },
  "page_setup": { "trim": { "w_pt": 468, "h_pt": 720 }, "bleed_pt": 9.0, "facing_pages": true },
  "master_pages": [ ... ],
  "spreads": [ { "pages": [ { "master": "...", "frames": [ ... ] } ] } ],
  "styles": { "paragraph": { ... }, "character": { ... } },
  "content": [ /* semantic blocks: headings, body, stat blocks, tables, random tables */ ],
  "assets": [ { "id": "...", "path": "assets/....png", "dpi": 300 } ]
}
```

## Versioning

`format_version` is an integer. Readers reject formats newer than they understand and migrate
older ones forward. Migrations are documented per bump.

## Two linked representations

The model carries both a **semantic content** tree (the easy authoring layer) and a **layout**
(spreads/frames/threads — the pro layer); frames reference content, so editing content reflows
layout. See `CLAUDE.md` and the plan for how these interact.
