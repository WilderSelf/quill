# 0002 — Real PDF/X byte generation

- **Milestone:** M0
- **Status:** implemented
- **Crates:** `quill-export-pdf` (owner), `quill-layout-engine`, `quill-text-layout`,
  `quill-color`, `quill-core-model`, `quill-cli`

> **Implementation notes (as landed):**
> - **Bundled font:** Source Serif 4 (SIL OFL-1.1), a static `glyf` TrueType instance — matches
>   the CIDFontType2/FontFile2 path. (Adobe's Source Serif distribution is CFF/OTF; the bundled
>   file is the static `glyf` build served by Google Fonts / fontsource.)
> - **Subsetter GIDs:** `subsetter` 0.2.x **remaps** glyph IDs (it does not preserve them). The
>   content stream is encoded with the *remapped* GIDs and `CIDToGIDMap` is `/Identity`; a
>   GID-consistency unit test guards this (the failure veraPDF cannot see).
> - **ICC synthesis:** built via the safe `lcms2` API (`Profile::new_placeholder` + CMYK/Output
>   class + `desc`/`cprt`/`wtpt`), exposed as `quill export`'s `synth-icc` subcommand for CI.
> - **PDF/X-1a has no free certified validator** (veraPDF does PDF/A, not PDF/X). The CI job runs
>   a Ghostscript well-formedness gate; the export-pdf unit tests assert the PDF/X structure
>   (header 1.3, OutputIntent, CIDFontType2, TrimBox/BleedBox, XMP) and are the real gate.
>   Certified conformance is the manual POD upload (spec 0001) / commercial preflight.

## Goal

Turn `export-pdf::export()` from its current `ExportError::NotImplemented` stub into a real,
**veraPDF-valid PDF/X-1a:2001** file: paginated content, embedded/subset font, CMYK/gray content
operators, correctly placed images, correct `TrimBox`/`BleedBox`/`MediaBox`, and an embedded ICC
`OutputIntent`. Still a **trivial, fixed layout** — no real text shaping or Knuth-Plass line
breaking yet (that remains out of scope, carried over from spec 0001). PDF/X-3 golden-file
verification is deferred to a follow-up spec; the writer path is shared and X-1a is the
stricter target.

## Background / why

Spec 0001 built and tested `preflight()` against the DriveThruRPG/PDF/X requirements but
intentionally stopped before writing PDF bytes ("M0 may use a trivial fixed layout to feed the
exporter"). This spec is that next commit: the first time Quill produces an actual file a press
or veraPDF can validate. It also resolves two of spec 0001's open questions (default ICC
profile, transparency-flattening approach) and finally implements its previously-unmet
acceptance criterion of veraPDF/Ghostscript golden-file checks in CI.

## Current state (verified against code)

- `layout-engine::lay_out()` does naive **single-page** stacking of `Block::Heading`/`Body` via
  `quill_text_layout::greedy_break`; it `continue`s past `Block::Image` (images are never
  placed) and never starts a second page.
- `text-layout::greedy_break()` is a placeholder word-wrap breaker (no hyphenation, no
  Knuth-Plass) — unchanged by this spec.
- `export-pdf` depends only on `quill-core-model`, `quill-color`, `thiserror`. None of
  `pdf-writer`, `subsetter`, `ttf-parser`, `fontdb`, or `lcms2` are in the workspace yet, and no
  font/ICC assets exist in the repo.

## Hard requirements (the acceptance spec)

1. **Pagination:** extend `layout-engine::lay_out()` from single-page to flow-until-full —
   track a running `y` and start a new `LaidOutPage` when `y + height` would exceed
   `doc.page_setup.trim.h_pt`. No new `core-model` fields are needed
   (`PageSetup.trim.h_pt` already exists).
2. **Images:** add an image variant to `PlacedBlock` (currently text-only). Size it from the
   asset's native pixel dimensions and `dpi` (`w_pt = px_w / dpi * 72.0`) and stack it into the
   same vertical flow as text blocks.
3. **Fonts:** `fontdb` locates/loads a font → `ttf-parser` reads glyph metrics/cmap → `subsetter`
   (the `typst` crate) subsets to only the glyphs used → `pdf-writer` emits `Type0`/
   `CIDFontType2` + `FontDescriptor` + `FontFile2` + widths. Exactly **one bundled font**, single
   weight, is shipped for the sample document and CI golden tests. It will be **SIL OFL-1.1**
   licensed (the norm for embeddable font files) — a data asset, not a Cargo dependency, so it
   doesn't conflict with the code's MIT/Apache-2.0 dual license, but is called out explicitly
   since CLAUDE.md's "every dependency must be permissive" language was written with crates in
   mind. User-supplied fonts are a fast-follow, not required here.
4. **Color:** no numeric conversion is needed — `Color::Cmyk`/`Color::Gray` already pass
   `preflight()` before reaching export, so they map directly to PDF `k`/`K` and `g`/`G`
   operators. `lcms2` is used narrowly to **validate** the user-supplied `--icc` file (CMYK,
   output/colorspace class) as a new preflight check, `CheckId::IccProfileInvalid`.
5. **ICC OutputIntent:** embed the validated ICC bytes as an `ICCBased` stream referenced from a
   hand-built `/OutputIntent` dict (`GTS_PDFX`, `OutputConditionIdentifier`, `Info`,
   `DestOutputProfile`) — `pdf-writer` has no PDF/X template for this, so it's constructed
   directly. **No vendor CMYK profile is bundled** (Adobe/FOGRA/ECI profiles carry
   redistribution terms needing separate confirmation); `--icc` is required for real output. CI
   and sample-doc golden tests instead **synthesize a minimal CMYK ICC profile programmatically
   via `lcms2`**, so tests need no network access or licensed file.
6. **Bleed/trim geometry:** per page, `MediaBox = BleedBox`; `TrimBox` is centered inside bleed
   on the three non-binding edges with zero bleed on the binding edge. Binding-edge-by-parity
   (which side is "inside" for a given page in a facing-pages layout) is computed from page
   index only when `PageSetup.facing_pages` is true; non-facing documents bleed all four edges.
7. **Transparency:** a no-op for this spec, explicitly — the trivial layout only emits opaque
   fills, text, and raster XObjects, so there is nothing to flatten. Content-stream generation
   asserts it never emits `/SMask` or `ca`/`CA` < 1, documenting and enforcing the invariant
   rather than silently deferring it.
8. **No marks:** content-stream generation never emits crop/registration/printer marks (already
   true by construction — nothing in this spec draws them).

## Public surface (changes from spec 0001)

```text
// export-pdf
enum CheckId { ..., IccProfileInvalid }   // new preflight check

fn export(doc: &Document, opts: &ExportOptions, out: &mut impl Write) -> Result<(), ExportError>
// now actually writes PDF/X-1a bytes to `out` on a clean preflight, instead of NotImplemented.

// layout-engine
enum PlacedBlock { Text { frame: Rect, lines: Vec<String> }, Image { frame: Rect, asset_id: String } }
fn lay_out(doc: &Document) -> Vec<LaidOutPage>   // now paginates instead of returning one page
```

## CI verification

> **Correction (as implemented):** veraPDF validates **PDF/A** and **PDF/UA**, *not* PDF/X — its
> `--flavour 1a` is PDF/A-1a (ISO 19005-1), a different standard. There is **no free, scriptable
> tool that certifies PDF/X-1a**; certified checking is Adobe Acrobat Preflight / callas
> pdfToolbox (commercial) or the manual DriveThruRPG upload (spec 0001). The CI job was therefore
> built around Ghostscript, not veraPDF.

A **Linux-only** job (separate from the fmt/clippy/test matrix) that:
- installs Ghostscript (`apt-get install -y ghostscript`);
- runs `quill synth-icc` + `quill export` on `Document::sample()` and checks the file is a
  non-empty `%PDF-1.3`;
- **gate:** fully interprets the PDF with Ghostscript (`-sDEVICE=nullpage -dPDFSTOPONERROR`) — a
  malformed file fails the build;
- **informational:** a Ghostscript `-dPDFX -sDEVICE=pdfwrite` re-distill, logged (not gated,
  since it re-distills rather than validates).

The detailed PDF/X-1a object structure (OutputIntent, embedded CIDFontType2, `TrimBox`/`BleedBox`,
XMP identification, no transparency) is asserted by the **export-pdf unit tests**, which are the
real structural gate; the Ghostscript job is a well-formedness smoke test.

## Acceptance criteria

- `export()` on `Document::sample()` with a valid `--icc` produces a non-empty PDF that Ghostscript
  interprets without error in CI, and whose PDF/X structure the unit tests assert.
- Unit tests cover: pagination boundary (content that exactly fills one page vs. overflows to a
  second), image placement sizing from `dpi`, the new `IccProfileInvalid` check (valid CMYK
  profile passes, an RGB or display-class profile fails), and binding-edge bleed asymmetry for
  both a facing-pages and a non-facing-pages document.
- `quill-cli export` continues to refuse (no file written) on a failing preflight, and now
  produces a real, non-empty PDF file on a clean one instead of the prior
  "preflight ok; PDF writing not implemented yet" message.
- No change to existing spec 0001 preflight tests' pass/fail behavior.

## Non-goals (this spec)

PDF/X-3 golden-file verification (deferred to a follow-up spec — X-1a is the stricter target
and the writer path is shared), real text shaping/Knuth-Plass line breaking, multi-font/bold/
italic, spot-color separations, an RGB→CMYK conversion pipeline (`naive_rgb_to_cmyk` stays
unused by export since RGB is already rejected at preflight), Windows/macOS golden-file CI, XMP/
metadata beyond the minimal PDF/X-required keys, and any real transparency-flattening algorithm
(nothing produces transparency yet).

## Open questions

- Which OFL font to bundle (needs a concrete pick before implementation — e.g. an OFL serif or
  sans suitable for body text; a licensing/attribution line in `README.md`/`LICENSE` may be
  needed alongside the dual code license).
- Exact veraPDF install/cache mechanics in GitHub Actions (install4j silent-install flags,
  cache key) — needs a short implementation-time spike to confirm against the current veraPDF
  release.
