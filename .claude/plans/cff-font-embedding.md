# Plan — CFF/OpenType-CFF (.otf) user font embedding (spec 0011)

## Goal
Let `quill export --font foo.otf` embed a **CFF-outline** OpenType font, closing the gap where
`.otf` fonts are rejected today (`fonts.rs::check_outline_format`). Named fast-follow of spec 0004.

## Key facts (verified against installed source)
- `subsetter` 0.2.6 subsets CFF too; `subset()` returns an **sfnt wrapper** whose `CFF ` table is
  subset. It converts SID-keyed → CID-keyed with identity GID=CID mapping, so remapped GID == CID.
- Output PDF is **PDF 1.3** (`set_version(1,3)`). The `FontFile3 /Subtype /OpenType` form is PDF
  1.6+, so it is **illegal** here → must embed the **bare `CFF ` table** as
  `FontFile3 /Subtype /CIDFontType0C`, with the descendant font `CIDFontType0`.
- Extract the bare table via `ttf_parser::RawFace::parse(&subset,0).table(Tag::from_bytes(b"CFF "))`.
- pdf-writer 0.15: `CidFontType::Type0`, `FontDescriptor::font_file3` both present.
- CIDFontType0 must **omit** `CIDToGIDMap` (Identity is only meaningful for CIDFontType2).

## Changes
1. **fonts.rs**
   - Add `pub enum OutlineKind { TrueType, Cff }`; add `outlines: OutlineKind` to `EmbeddedFont`.
   - `check_outline_format`: accept CFF (return the kind) instead of rejecting; still reject
     no-outlines. Keep glyf-wins-if-both.
   - After subsetting: TrueType → `subset` = full sfnt (unchanged). CFF → extract bare `CFF ` table
     from the subset sfnt; `subset` = those bytes.
2. **writer.rs `write_font`**: branch on `font.outlines`:
   - TrueType (unchanged): `CidFontType::Type2`, `CIDToGIDMap /Identity`, `font_file2`, `Length1`.
   - Cff: `CidFontType::Type0`, no `CIDToGIDMap`, `font_file3`, stream `/Subtype /CIDFontType0C`,
     no `Length1`.
3. **Fixture**: generate a tiny synthetic CFF `.otf` out-of-tree with fontTools (fontBuilder +
   T2CharStringPen; glyphs .notdef + a few ASCII letters + cmap). Commit
   `crates/export-pdf/assets/test-cff.otf`. Synthetic ⇒ no third-party license.

## Tests
- `fonts.rs`: CFF fixture builds; `outlines == Cff`; `subset` starts with a CFF header byte
  (major version 1 == `0x01`) and is **not** an sfnt (`\0\x01\0\0`/`OTTO`); GID consistency holds;
  glyf path (`build`) still `TrueType`; `check_outline_format` cases.
- Export-level: a `Document` exported with the CFF fixture yields a PDF containing
  `/CIDFontType0` and `/CIDFontType0C`; Ghostscript well-formedness gate (CI) passes.

## Acceptance
- `.otf` (CFF) font embeds; PDF has `CIDFontType0` + `FontFile3/CIDFontType0C`, no `Length1`,
  no `CIDToGIDMap` on the CIDFont.
- TrueType path byte-stable (existing font tests unchanged).
- Ghostscript accepts the CFF export.
