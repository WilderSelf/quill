# 0001 — Press-ready PDF/X export

- **Milestone:** M0
- **Status:** implemented — preflight landed with this spec; byte generation in
  [spec 0002](0002-pdf-byte-generation.md); the "PDF/X-1a **or** PDF/X-3 (selectable)"
  requirement (#1) is completed by [spec 0003](0003-pdf-x3-output.md); the `Marks` (#7) and
  `Transparency` (#9) preflight checks are completed by
  [spec 0007](0007-preflight-marks-transparency.md).
- **Crates:** `quill-export-pdf` (owner), `quill-color`, `quill-core-model`, `quill-cli`

## Goal

Take a document from `quill-core-model` and emit a **press-ready PDF that passes the
DriveThruRPG print check** (and, by extension, Lulu / IngramSpark, which are similar). This is
the product's core differentiator and the first thing M0 proves — headless, via `quill-cli`,
before any editor UI exists.

## Background / why

DriveThruRPG (the dominant TTRPG print-on-demand marketplace) recommends *only* Affinity
Publisher or InDesign and rejects files that miss its spec. No open-source tool reliably
produces compliant PDF/X. Meeting this spec is the reason Quill can exist.

## Hard requirements (the acceptance spec)

A conformant export MUST satisfy all of:

1. **PDF/X conformance:** output is valid **PDF/X-1a:2001** or **PDF/X-3:2002** (selectable).
2. **Color:** color content is **CMYK**; no RGB, Lab, or spot/Pantone in color output. B&W
   interiors are **grayscale**. (PDF/X-1a implies no unmanaged RGB at all.)
3. **Fonts:** every font used is **embedded and subset**. No unembedded fonts.
4. **Bleed:** **0.125 in (9 pt)** bleed on the three outside (non-binding) edges; **no bleed on
   the binding edge**. `BleedBox` and `TrimBox` are written correctly per page.
5. **Resolution:** raster images are **≥ 300 dpi** (CMYK/grayscale) or **≥ 600 dpi** for
   bilevel line art, measured at placed size. Under-resolution is a preflight failure.
6. **Ink coverage:** **total ink ≤ 240%** anywhere (sum of C+M+Y+K). Over-limit is a failure.
7. **Marks:** **no** crop, printer, or registration marks embedded in the file.
8. **Output intent:** a valid **ICC OutputIntent** is present (required for PDF/X).
9. **Transparency:** for PDF/X-1a, live transparency is flattened.

## Preflight

`export-pdf` exposes a **preflight** that checks a document against the above and returns a
structured report (per-item pass/fail with locations) **before** writing the PDF. Export of a
failing document is refused unless explicitly forced. Preflight is independently testable
without generating a PDF.

Preflight checks map 1:1 to the requirements above:
`ColorSpace`, `FontEmbedding`, `Bleed`, `ImageResolution`, `InkCoverage`, `Marks`,
`OutputIntent`, `Transparency`.

## Public surface (intended)

```text
enum PdfxVersion { X1a2001, X3_2002 }

struct ExportOptions {
    version: PdfxVersion,
    output_intent_icc: PathBuf,   // e.g. a CMYK profile such as a FOGRA/GRACoL ICC
    bleed_pt: f32,                // default 9.0 (0.125 in)
    force: bool,                  // export even if preflight fails
}

fn preflight(doc: &Document, opts: &ExportOptions) -> PreflightReport
fn export(doc: &Document, opts: &ExportOptions, out: &mut impl Write) -> Result<(), ExportError>
```

`PreflightReport { passed: bool, findings: Vec<Finding> }`, where each `Finding` has a check id,
severity (error/warning), human message, and an optional page/frame location.

## Acceptance criteria

- Unit tests cover each preflight check independently, including boundary cases: ink coverage
  exactly 240% (pass) vs 241% (fail); image at exactly 300 dpi (pass) vs 299 (fail); a page
  with a font flagged un-embeddable (fail).
- A generated sample PDF validates as PDF/X-1a **and** as PDF/X-3 under **veraPDF** and passes a
  **Ghostscript** preflight, checked as golden-file tests in CI.
- `quill-cli export <in> <out> --pdfx x1a --icc <profile>` runs preflight then writes the file,
  exiting non-zero (and writing nothing) when preflight fails without `--force`.
- Manual acceptance: the sample uploads cleanly to DriveThruRPG's automated print check.

## Non-goals (this spec)

Interactive editing, real text layout quality (Knuth-Plass), spot-color separations, and the
GUI. Those are later specs/milestones. M0 may use a trivial fixed layout to feed the exporter.

## Open questions

- Which default CMYK OutputIntent profile to ship (licensing of ICC profiles).
- Transparency flattening approach for X-1a (pre-flatten in layout vs. at export).
