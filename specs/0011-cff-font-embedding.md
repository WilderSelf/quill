# 0011 — CFF / OpenType-CFF (`.otf`) font embedding

**Milestone:** M0 **Status:** implemented

## Problem

Spec 0004 added user-supplied font embedding but accepted **TrueType (`glyf`) only**: a CFF-outline
OpenType font (`.otf`, the format most modern retail and many libre fonts ship in) was rejected with
_"OpenType/CFF fonts (.otf) are not yet supported; supply a TrueType (.ttf) font."_ Both spec 0004's
prose and `fonts.rs` named this a fast-follow needing "a different `FontFile3`/`CIDFontType0` writer
path." This spec is that follow-up: embed CFF fonts so `quill export --font foo.otf` works.

## Background / the PDF 1.3 constraint

A CFF font is embedded in PDF as a composite font whose descendant is a **`CIDFontType0`**, with the
font program in a **`FontFile3`** stream. `FontFile3` identifies the program by its `/Subtype`:

- `/CIDFontType0C` — a **bare CFF** table (CID-keyed). Legal since PDF 1.2/1.3.
- `/OpenType` — a full sfnt wrapper containing the CFF. **PDF 1.6+ only.**

Quill's PDF/X-1a:2001 / X-3:2002 output is pinned to **PDF 1.3** (`set_version(1, 3)`), so the
`OpenType` wrapper is **illegal** here. The embedded program must therefore be the **bare `CFF `
table**, extracted from the sfnt the subsetter emits.

## Behavior

`fonts::build_from_bytes` classifies the font's outline flavour (`glyf` → TrueType, else `CFF ` →
CFF; `glyf` wins if both are present) and records it on `EmbeddedFont`:

- **TrueType** (unchanged): the subset sfnt is embedded whole as `FontFile2` under a `CIDFontType2`
  descendant with `CIDToGIDMap /Identity` and a `Length1`.
- **CFF** (new): `subsetter::subset` still returns an sfnt wrapper (it subsets the `CFF ` table and
  converts SID-keyed → CID-keyed with an identity GID=CID map, so the remapped GID is usable
  directly as the CID). The bare `CFF ` table is extracted from that sfnt
  (`ttf_parser::RawFace::table`) and embedded as `FontFile3` (`/Subtype /CIDFontType0C`) under a
  `CIDFontType0` descendant. `CIDToGIDMap` is **omitted** (meaningful only for `CIDFontType2`; a
  `CIDFontType0` maps CID→glyph through the CFF charset) and there is no `Length1`.

Glyph measurement, width tables, the char→subset-GID map, subset tagging, descriptor flags, name
derivation, and the `fsType` embeddability guard are shared with the TrueType path unchanged.

## Public surface (delta)

```text
// quill-export-pdf::fonts
pub enum OutlineKind { TrueType, Cff }
struct EmbeddedFont { …, outlines: OutlineKind }   // subset holds sfnt (TT) or bare CFF (CFF)
```

No CLI change: `--font` already takes an arbitrary path; `.otf` now succeeds instead of erroring.

## Acceptance criteria

- A CFF `.otf` font builds: `outlines == Cff`, and `subset` is a bare CFF program (starts with the
  CFF header byte `0x01`, is **not** an sfnt — no `OTTO` / `00 01 00 00` prefix).
- Exporting a document with a CFF `--font` yields a PDF containing `/CIDFontType0C` and `/FontFile3`,
  and **no** `/CIDFontType2`, `/FontFile2`, or `/CIDToGIDMap`. Ghostscript's CI well-formedness gate
  accepts it.
- The GID↔glyph width invariant holds on the CFF path (as [spec 0004]'s `gids_are_consistent` does
  for TrueType).
- The TrueType path is byte-stable: every pre-0011 font test passes unchanged.
- A font with neither `glyf` nor `CFF ` outlines is still rejected with a clear message.

## Test fixture

A tiny synthetic CFF `.otf` (`assets/test-cff.otf`, ~0.9 KB: `.notdef` + `A`–`E` + `space`) built
from scratch with `fontTools.fontBuilder` **out-of-tree** (scratchpad) — no third-party outlines, so
no license encumbrance, and no generator added to the workspace (per the fixture convention in
`CLAUDE.md`).

## Non-goals

- **CFF2 / variable fonts** — `subsetter`'s CFF2 support is feature-gated and OpenType variations are
  an M1+ concern; only static CFF1 is in scope.
- **The `FontFile3 /Subtype /OpenType` wrapper** — would require raising the emitted PDF version
  past 1.3, which PDF/X-1a:2001 forbids.
- **Type1 (`.pfb`) legacy fonts** — obsolete; not a hobbyist-publisher input.
