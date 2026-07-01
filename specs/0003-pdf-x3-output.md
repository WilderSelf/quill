# 0003 — PDF/X-3:2002 output

- **Milestone:** M0
- **Status:** implemented
- **Crates:** `quill-export-pdf` (owner), `quill-cli`

## Goal

Make `quill export --pdfx x3` emit a genuine, correctly-identified **PDF/X-3:2002** file.
This closes spec 0001's hard requirement #1 — conformance is "PDF/X-1a:2001 **or**
PDF/X-3:2002 (**selectable**)" — and is the follow-up spec 0002 explicitly deferred ("PDF/X-3
golden-file verification (deferred to a follow-up spec — X-1a is the stricter target and the
writer path is shared)").

## Background / why

Spec 0002 shipped the X-1a:2001 writer and wired a `PdfxVersion { X1a2001, X3_2002 }` enum plus
a CLI `--pdfx x1a|x3` flag that flows into `ExportOptions::version`. But the writer and XMP
builder **ignored** `opts.version` and hardcoded `PDF/X-1a:2001` in three places (the document
info dict's two `GTS_PDFX*` pairs and the XMP `pdfxid:GTS_PDFXVersion` /
`pdfx:GTS_PDFXConformance` elements). So `--pdfx x3` produced a file that *silently identified
as PDF/X-1a:2001* — an unfinished feature and a latent correctness bug (the tool misreported
its own output).

## Scope — metadata-only, a strict subset

Because M0's trivial layout emits only opaque CMYK/grayscale content, PDF/X-3:2002 output is a
**strict subset** of what the X-1a writer already produces. Only the identification metadata
changes:

| Aspect | X-1a:2001 | X-3:2002 | Change? |
|---|---|---|---|
| PDF header | 1.3 | 1.3 | none |
| `GTS_PDFXVersion` | `PDF/X-1a:2001` | `PDF/X-3:2002` | **yes** |
| `GTS_PDFXConformance` | `PDF/X-1a:2001` | *(omitted)* | **yes** |
| OutputIntent (`GTS_PDFX`, CMYK ICC) | required | required | none |
| Transparency | forbidden | forbidden (X-4+ only) | none |
| Live/managed RGB, Lab | forbidden | *permitted* w/ ICC | none¹ |

¹ X-3 *permits* device-independent/ICC-tagged RGB but does not *require* it. Preflight keeps
rejecting RGB (spec 0001 check `ColorSpace`); a CMYK/gray-only file is fully conformant for both
levels. Relaxing RGB for X-3 is a **non-goal** here.

## Hard requirements

1. `opts.version` drives the emitted `GTS_PDFXVersion` string in **both** the document info dict
   and the XMP packet: `PDF/X-3:2002` for `X3_2002`, `PDF/X-1a:2001` for `X1a2001`.
2. `GTS_PDFXConformance` is emitted **only for X-1a** (in both info dict and XMP). PDF/X-3
   (ISO 15930-3) defines only `GTS_PDFXVersion`, so the conformance key is omitted for X-3.
3. No other bytes change for X-3 vs X-1a on the M0 layout: same PDF 1.3 header, OutputIntent,
   font embedding, geometry, and content operators.
4. No preflight change; RGB stays rejected for both levels.

## Public surface

No new public types. `PdfxVersion` (already public) gains two helpers:

```text
impl PdfxVersion {
    fn identifier(self) -> &'static str;         // GTS_PDFXVersion string
    fn conformance(self) -> Option<&'static str>; // GTS_PDFXConformance, None for X-3
}
```

`xmp::build_xmp` takes a leading `version: PdfxVersion` argument. `export()` / `ExportOptions`
signatures are unchanged (the CLI already populates `version`).

## Acceptance criteria

- `xmp` unit tests: X-3 packet contains `PDF/X-3:2002` and **omits** `GTS_PDFXConformance`;
  X-1a packet carries both keys as `PDF/X-1a:2001`.
- `export()` unit test (`export_writes_pdfx3_identifier`): exporting `Document::sample()` with
  `version = X3_2002` + a synth ICC yields bytes containing `PDF/X-3:2002` (info dict + XMP),
  no `PDF/X-1a` string, no `GTS_PDFXConformance`, a `%PDF-1.3` header, and the `GTS_PDFX`
  OutputIntent marker. The existing X-1a export test additionally asserts no `PDF/X-3` leaks in.
- CI `pdf-preflight` job generates `--pdfx x3` alongside x1a, asserts the X-3 file identifies as
  X-3 (and not X-1a), and Ghostscript interprets both without error.
- No change to spec 0001/0002 preflight or X-1a byte behavior.

## Non-goals

RGB/Lab-for-X-3 color relaxation; PDF/X-3:**2003** (PDF 1.4); real ICC color management or
RGB→CMYK conversion in the export path; certified PDF/X-3 validation (no free tool certifies
PDF/X — the unit tests are the structural gate; certified conformance is the manual
DriveThruRPG upload or a commercial preflight, same caveat as spec 0002).
