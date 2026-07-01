# 0004 — User-supplied font embedding

- **Milestone:** M0
- **Status:** implemented
- **Crates:** `quill-export-pdf` (owner), `quill-cli`

## Goal

Let `quill export` embed an **arbitrary user-supplied TrueType (`glyf`) font file** instead of
being locked to the single bundled Source Serif 4. The supplied font is subset, measured, named
from its own metadata, and permission-checked, then flows through the existing
Type0/CIDFontType2/FontFile2 writer chain unchanged. The bundled font stays the default, so
omitting the new flag reproduces today's byte-for-byte output.

## Background / why

Spec 0002 shipped font embedding but scoped it to "exactly **one bundled font**", noting
"user-supplied fonts are a fast-follow, not required here" (req 3). Today `fonts::build`
hardcodes both the font program (`include_bytes!(".../SourceSerif4-Regular.ttf")`) and the
PostScript name `"SourceSerif4"`. This is the biggest functional gap keeping the exporter from
real-world use: a TTRPG author cannot export a book in their own typeface. This spec closes that
for the common case (TrueType outlines) while leaving CFF/OpenType as an explicit fast-follow.

## Scope — a load-seam refactor plus one CLI flag

The subsetting, GID-remap, Identity-H encoding, and PDF font-object writer are already
font-source-agnostic and do **not** change. Only the font *load* seam and its inputs change.

## Hard requirements

1. **Selectable source.** `ExportOptions` gains `font_path: Option<String>` (`#[serde(default)]`,
   `Default = None`). `None` embeds the bundled Source Serif 4 exactly as today; `Some(path)`
   reads that file and embeds it. `quill export` gains `--font <PATH>`.
2. **Byte-stable default.** With `font_path = None`, output is unchanged: BaseFont
   `<tag>+SourceSerif4`, same subset tag, same descriptor flags (`SERIF | NON_SYMBOLIC`). No
   CI/golden regression.
3. **TrueType only.** The supplied font must have `glyf` outlines. A CFF/OpenType-CFF font
   (`.otf`, `cff` table present, no `glyf`) is rejected with a clear `ExportError::Font`. This is
   the fast-follow boundary — CFF needs a different `FontFile3`/`CIDFontType0` path (non-goal).
4. **Embedding permission gate.** A supplied font whose `OS/2` `fsType` forbids subset-embedding
   (`ttf-parser` `Face::is_subsetting_allowed()` is false) is rejected with `ExportError::Font`.
   Restricted-license fonts must never be embedded in a press file. The bundled OFL font is
   installable and unaffected.
5. **Name derived from the font.** For a user font, the embedded BaseFont family is derived from
   the face's own `name` table — prefer name ID 6 (PostScript name), fall back to ID 1 (family) —
   then **sanitised** to a valid PDF name: drop spaces and any character outside `[A-Za-z0-9-]`;
   if the result is empty, use `"Font"`. The existing six-letter `subset_tag` prefix is unchanged,
   so the BaseFont is `<tag>+<sanitised-name>`.
6. **Descriptor flags from the face** (user font only): `NON_SYMBOLIC` always; `ITALIC` from
   `Face::is_italic()`; `FIXED_PITCH` from `Face::is_monospaced()`. `SERIF` is set only for the
   bundled default (reliable serif classification across arbitrary fonts isn't cheap; omitting it
   is PDF/X-legal — the flags are metadata, not a conformance gate). `stem_v` keeps its heuristic.
7. **Errors, not silent fallback.** A missing/unreadable file, a parse failure, a CFF font, or a
   permission-forbidden font is a hard `ExportError::Font` — export writes no file and the CLI
   exits non-zero. It never silently falls back to the bundled font.

## Public surface

```text
// export-pdf::fonts
pub fn build(chars: &BTreeSet<char>) -> Result<EmbeddedFont, ExportError>;            // bundled (unchanged signature)
pub fn build_from_bytes(program: &[u8], name_override: Option<&str>,
                        chars: &BTreeSet<char>) -> Result<EmbeddedFont, ExportError>; // new core
// EmbeddedFont gains `flags: FontFlags` so the writer consumes derived flags directly.

// export-pdf::lib
pub struct ExportOptions { /* ... */ pub font_path: Option<String> }  // new field
```

`build(chars)` becomes a thin wrapper: `build_from_bytes(FONT_TTF, Some("SourceSerif4"), chars)`
with the `SERIF` flag. `export()`'s signature is unchanged; the CLI populates `font_path`.

## Acceptance criteria

- **Regression:** existing `fonts.rs` tests (`subsets_and_measures`, `encodes_*`,
  `unknown_char_maps_to_notdef`, `gids_are_consistent`) still pass unchanged; default export bytes
  (BaseFont `…+SourceSerif4`, `SERIF | NON_SYMBOLIC` flags) unchanged.
- **User-path round-trip:** `build_from_bytes(FONT_TTF, None, chars)` subsets, measures, and
  derives its BaseFont name from the face's name table (exercises the user plumbing with no second
  asset).
- **CFF rejection & permission gate:** a format-detection helper (`&Face → Result`) rejects a CFF
  face; a permission helper rejects a not-subsettable face. Tested directly (over the helper's
  inputs) since crafting real restricted/CFF fixtures inline is impractical — documented here as a
  deliberate testing choice.
- **Name sanitisation:** names with spaces/punctuation yield valid PDF names (no spaces, allowed
  chars only); empty derivation yields `"Font"`.
- **Export-level:** `ExportOptions { font_path: Some(<bundled ttf on disk>), .. }` produces a
  non-empty PDF whose BaseFont reflects the derived (not hardcoded) name and that Ghostscript
  interprets without error.
- **CLI:** `--font <ttf>` embeds it; `--font <otf>` and a missing file fail cleanly with no file
  written; omitting `--font` is byte-identical to before.

## Non-goals (fast-follows)

CFF/OpenType (`FontFile3`/`CIDFontType0`) embedding; `fontdb` by-name / system-font lookup;
multiple fonts or bold+italic runs within one document; per-block font selection in `core-model`
(the whole document still renders in one face); and any `core-model` `fonts_embeddable` /
preflight `FontEmbedding` change — font-embeddability is enforced at export time as an
`ExportError::Font`, deliberately not a preflight `CheckId`, since a bad font file is an export
error rather than a document-content finding. Each named item is its own later spec.
