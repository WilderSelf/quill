# Spec 0057 — `quill pack extract`

**Milestone:** M4 · **Size:** medium · **Status:** implemented

## Problem

Specs 0054–0056 can declare a component, bundle it and resolve it. Nothing *produces* a pack except
hand-written JSON. The publisher this milestone is for has already built a book — that book is the
house style — and asking them to transcribe it into a manifest is asking them to do the work twice
and get it subtly different the second time.

## What this builds

`quill pack extract <document> --name <slug> --version <v> --source <s> --license <l>
--output <file.qpack>`: turn a finished book into a reusable pack.

### What is extracted

| From the document | Into the pack | Why |
|---|---|---|
| `page_setup`, `master_pages`, `default_master`, `pages` | one `Template` | the furniture and the trim *are* the look |
| `styles` | `styles` | the type scale |
| `components` | `components` | the document's **own** definitions |
| assets referenced by a `MasterStatic::Image` | `assets` | furniture art is part of the look |

### What is not

**Content.** No blocks, no paragraph text, no stat block, no table, no image placed in the flow. A
pack is a *look*, not a book, and a pack that carried its author's prose would make every book built
from it a derivative of that prose.

The bundled component definitions are not extracted either, even though `Document::component_library`
resolves them: every quill already has them, and shipping a copy would mean a pack that silently
pins a definition its author never edited to whatever this build's version of it happened to be.
Only `Document::components` — what this document actually declares — travels.

Assets referenced only by `Block::Image` are content and stay behind. Assets referenced by master
furniture travel, because a running head with a rule ornament is not a book's content, it is its
design. The rule is stated this precisely because "extract the assets" would take the art.

### The inverse of `Document::from_template`

`Template::from_document` is added and is the exact inverse of the existing constructor, asserted as
such: a template extracted from a document built from a template reproduces that template's
document. Without the round-trip assertion, "extract" and "apply" are two functions that happen to
share field names.

## Acceptance criteria

- Extracting from a document produces a pack whose templates, styles and definitions reproduce that
  document's look when applied to a **different** document — asserted by laying a second document
  out under the extracted pack and comparing its placed styling to the first's.
- **Content is not extracted**: no block text survives, asserted over the serialized pack rather
  than over a field list, so a future field that happens to carry text is caught too.
- A pack referencing furniture art carries that asset; one referencing flow art does not.
- Round-trips through spec 0055's container and installs under spec 0056.
- Provenance is required at the point of extraction — `--source` and `--license` are mandatory
  arguments, not optional ones defaulted to empty, because spec 0055 would refuse the result anyway
  and a failure at write time is a worse place to learn it.

## Non-goals

- Extracting *some* of a document. The unit is the whole look.
- Deriving component definitions from usage. `quill pack extract` carries the definitions a document
  declares; it does not infer a definition from a stat block.
