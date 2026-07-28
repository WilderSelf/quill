# 0071 — Content streams are `FlateDecode`'d

**Milestone:** M5 (closeout) · **Size:** small · **Status:** implemented

## Why

Every stream in a quill PDF that carries bulk is compressed except the one that carries the page.
Image XObjects have been `/FlateDecode`d since spec 0005 (`writer.rs`, the `deflate(bytes)` beside
every `image_xobject`) and font programs since spec 0002; the XMP packet and the `/ToUnicode` CMap
are uncompressed on purpose and are fixed small costs. The page content stream never was.

That was affordable while it held nothing but operators and glyph bytes. Spec 0068 filled it with
per-pair kern adjustments — short integers drawn from a handful of values, repeated once per glyph
pair in the document — and measured what it cost: `Document::sample()` **8786 → 10220 bytes
(+16.3%)**, the bundled `reference` template 5977 → 7087 (+18.6%), and the 500-page synthetic
document **13,763,105 → 15,791,758 bytes (+14.7%)**. That measurement is why this increment exists;
0068 named it as its own separate spec rather than absorbing it, because the two changes fail in
different ways and one of them breaks every test that greps the finished file.

Repetitive integers are close to a best case for deflate, so the growth 0068 added is exactly the
kind a compressor eats. The claim worth making is therefore not "smaller than it is now" but
**smaller than it was before 0068**.

Nothing guards export size today, which is how a 14.7% regression on the product's stated workload
— 500 pages, art-heavy — reached `main` and was found by a hand measurement rather than by the
gate. That is the second half of this increment.

## What

### The writer

One stream gains one filter, using the `deflate()` helper the image and font paths already call:

```rust
let compressed = deflate(&content);
let mut s = pdf.stream(*content_id, &compressed);
s.filter(Filter::FlateDecode);
s.finish();
```

Nothing upstream of the writer changes. The same pages, the same shaped runs, the same subset, the
same operators — only their encoding in the file.

### PDF/X-1a is unaffected

`FlateDecode` is a PDF **1.2** filter. The header stays `%PDF-1.3`, which PDF/X-1a:2001 pins, and
the same filter is already on this very file's image XObjects and `FontFile2`. What X-1a constrains
is colour, transparency, font embedding, annotations, external references and encryption — it says
nothing about Flate-compressing content, and could not, since the standard's own reference output
does it. There is nothing to argue here beyond stating it: the file's other two stream classes have
carried this filter through every Ghostscript gate the project has run.

### The cost: a check can no longer grep the finished file for an operator

This is the real content of the increment, and it is the trap spec 0068 met twice. A `grep` that
stops matching does not fail — it goes **blind**, and a negative assertion built on it starts
passing vacuously. So every check that reads the emitted PDF was enumerated and classified rather
than left to the suite to discover.

**Affected — reads operator text out of a finished PDF:**

| check | what it did | what it does now |
|---|---|---|
| `stream_read::content_streams` | filtered raw stream payloads for `BT` + a show operator | inflates first, via a new `decoded_streams` |
| `a_runs_colour_override_reaches_the_content_stream` (`lib.rs`) | `String::from_utf8_lossy(&bytes).contains("0 1 1 0 k")` over the whole file | reads through `content_streams`, so it searches the inflated operators |

`content_streams` is the one that mattered structurally: `extract_text` and the `/ToUnicode`
round-trip test are built on it, and it would have returned an empty list rather than an error.
`to_unicode_map` moved to `decoded_streams` too — the CMap is written uncompressed *deliberately*,
so a human can open it by hand, but a reader that only works while that stays true is the same
latent blindness one layer along.

**Unaffected, verified rather than assumed.** Every `grep -aq` in CI's `pdf-preflight` job and
every negative byte assertion in `export-pdf` looks for a *dictionary or metadata* string, and all
of those objects are still written uncompressed. Each was run against output from both sides of the
change; each behaves identically. The full list is in *Verification* below.

**The writer's own tests are unaffected by construction.** They assert over `render_page`'s
return value — the operators before they reach a stream — which was never compressed and is the
right level to test a writer at anyway.

### A new CI check, because nothing was watching page content

No check in `pdf-preflight` greps for an operator, so nothing there went blind — but that means
nothing there ever asserted the press file *draws* anything. Ghostscript's `-dPDFSTOPONERROR` gate
cannot see it either: an empty page is perfectly well-formed. The job gains a step that asserts
**both halves**, because either alone is satisfiable by a defect:

- the stored bytes of every stream carry no `BT`, and the file no ` Tj`/` TJ` — so the compression
  really happened;
- every stream that inflates to `BT` declares `/Filter /FlateDecode`, sets a font and shows text —
  so the page still draws, and the reader kept up.

A check that only did the second would pass over a writer that had quietly stopped compressing; one
that only did the first would pass over a page that drew nothing.

### The export-size budget

`benches/budgets.toml` gains an `[export]` section — the only one in the file that measures an
artefact rather than a cost — fed by a new `export_size` bench in `quill-testdoc`:

| entry | measured | ceiling |
|---|---|---|
| `export.sample_bytes` | 8454 | 9000 |
| `export.synthetic_500_page_bytes` | 1,308,263 | 1,600,000 |

Both are exported against the committed parity ICC
(`crates/export-pdf/assets/parity-outputintent.icc`), for the reason the byte-parity digest already
commits it: `synth_cmyk_profile()` stamps the current time into the ICC header, and an OutputIntent
is embedded verbatim, so a synthesized profile would make the size a clock.

**Both are checked with `Budgets::check_exact`, not against `tolerance_factor`** — spec 0051's
lesson, applied deliberately rather than by default. The tolerance exists for one stated reason:
shared runners vary by 10–30% between runs. An exported byte count does not vary at all; the same
document, the same committed font and the same committed profile produce the same file, which is
already what `SAMPLE_EXPORT_DIGEST` rests on. And the tolerance would be actively harmful here:
doubling `synthetic_500_page_bytes` puts the limit at 3.2 MB — above anything the writer can
currently produce — and a second doubling clears the 13.7 MB this increment exists to get back
under. A budget whose limit is unreachable is not a budget.

The 22% headroom on the 500-page entry is for ordinary content growth, and still leaves the gate
firing **9.9× below** what a writer that stopped compressing would emit (1.6 MB against 15.79 MB).
That is the regression the line is for.

## Acceptance criteria

- [x] The 500-page synthetic export is **smaller than it was before spec 0068** — the claim, not
      merely smaller than 0068 left it.
- [x] An export-size budget exists, in `benches/budgets.toml` beside the work counters, checked
      exactly rather than with the 2× timing tolerance, with the choice argued rather than defaulted.
- [x] Every test and every CI check that reads the finished PDF is enumerated: the ones that read
      operator text decompress first, and the ones that do not are shown to be unaffected by
      measurement rather than by assertion.
- [x] `SAMPLE_EXPORT_DIGEST` is re-derived under the ledger's **structural** template, and the
      decompressed stream is shown byte-identical to the old uncompressed one.
- [x] PDF/X-1a:2001 conformance is unaffected.

## Verification

### The three-way size measurement

Pre-0068 is a `git worktree` at `9811d6c`, the commit before 0068 landed, carrying the same
`export_size` probe. All three exports use the committed parity ICC and the same
`SynthSpec::default()` document — **3346 blocks, 499 pages on every build**, confirmed so the
comparison is like for like.

| document | pre-0068 (`9811d6c`) | `main` (post-0068) | this branch |
|---|---|---|---|
| `Document::sample()` | 8786 | 10220 | **8454** |
| 500-page synthetic | 13,763,105 | 15,791,758 | **1,308,263** |

**The claim holds, and by more than it needed to.** The 500-page file is **9.5%** of its pre-0068
size and **8.3%** of `main`'s; the sample is 3.8% below pre-0068 and 17.3% below `main`. The margin
is not the kern adjustments alone — a page content stream is mostly repeated `Tf`/`Td`/`Tj`
scaffolding and decimal coordinates, which is what compresses. What 0068 added is simply the part
that made anyone look.

### The digest move, verified structurally

`cmp` proves nothing about a change whose entire effect is on lengths and offsets, so the two files
were compared for what their objects *are*. `Document::sample()`, parity ICC, `main` in a worktree
against this build:

- **Fourteen objects both sides, and exactly one differs**: object 12, the page content stream.
  Catalog, page tree, page, Type0/CIDFont/descriptor, `FontFile2`, `/ToUnicode` CMap, ICC, XMP,
  outline root and item, and info are byte-identical.
- **That object differs only in its dictionary and in the encoding of its payload**:
  `<< /Length 2214 >>` → `<< /Length 426 /Filter /FlateDecode >>`. The file's `/Length` set moves
  from `[1017, 376, 2981, 1010, 2214]` to `[1017, 376, 2981, 1010, 426]` — XMP, ICC, font program
  and CMap all where they were.
- **The payload inflates to the old bytes exactly**: all 2214 of them, compared byte for byte
  against the uncompressed stream `main` wrote. That is the strongest statement available here, and
  it is why no glyph, `TJ` amount or operator needed inspecting one at a time — not one moved.
  Deflate took the stream to **19.2%** of its size.

`SAMPLE_EXPORT_DIGEST`: `0x8e3c_3d98_9471_cf23` → `0xc1b5_3543_e96c_8692`.

### The stream framing

The content stream is framed exactly as the `FontFile2` beside it, which has passed every
Ghostscript gate the project has run: `/Length` bytes of zlib, one `\n` before `endstream`, and
`zlib.decompressobj().unused_data` empty on both. The risk `-dPDFSTOPONERROR` exists to catch here
is a length that disagrees with the payload, and the two objects agree in the same way.

### Every check that reads a finished PDF

Run against output from both sides of the change. `MATCH`/`NOMATCH` is the same on both.

| check | where | verdict |
|---|---|---|
| `%PDF-1.3` header (×3) | CI | unaffected — file header |
| `PDF/X-3:2002` in the X-3 file | CI | unaffected — info dict + XMP, both uncompressed |
| `! PDF/X-1a` in the X-3 file | CI | unaffected — control: the string *does* match in `sample.pdf` |
| `/Annots`, `/Subtype /Link` in the screen file | CI | unaffected — page dict and annotation object |
| `! GTS_PDFXVersion`, `! npes.org`, `! /OutputIntents` in the screen file | CI | unaffected — controls: all three match in `sample.pdf` |
| `PDF/X-1a:2001` in the press files | CI | unaffected — info dict + XMP |
| `! /Annots` in the press files | CI | unaffected — control: it matches in `screen.pdf` |
| `! /Outlines`, `! /Subtype /Image`, `! PDF/X-3`, `! GTS_PDFXConformance`, `! /CIDFontType2`, `! /FontFile2`, `! /CIDToGIDMap` | `export-pdf` | unaffected — every one has a passing positive counterpart in the same suite, and all still grep out of the raw file |
| `! /Annots`, `! /Annot` (`the_press_profile_emits_no_annotations_at_all`) | `export-pdf` | unaffected — control: the screen test finds the annotation |
| `objects()` / `ref_array()` object-graph helpers | `export-pdf` | unaffected — they parse dictionaries, and dictionaries are not compressed |
| `content_streams` → `extract_text`, `to_unicode_map` | `export-pdf` | **fixed** — inflates |
| `a_runs_colour_override_reaches_the_content_stream` | `export-pdf` | **fixed** — reads through `content_streams`. It **failed** before the fix, which is how it was found: it was the only in-tree test grepping a finished file for an operator, and the initial audit missed it because the string it looks for is `"0 1 1 0 k"` rather than an operator token |
| every `writer::tests` operator assertion | `export-pdf` | unaffected **by construction** — they assert over `render_page`'s return value, before it reaches a stream |

The last row is the reason this increment was small. A writer test that inspects the writer's own
output, rather than the file the writer ends up inside, is immune to how that file encodes it.

## Non-goals

- **Object streams and cross-reference streams.** PDF 1.5, and PDF/X-1a:2001 pins the header at
  1.3. They would compress the *dictionaries* — the remaining uncompressed bulk in a small file —
  and they are not available at this conformance level. Not a deferral: a decision the format makes
  for us.
- **Compressing the XMP packet or the `/ToUnicode` CMap.** Both are deliberately uncompressed. The
  XMP packet is what makes the PDF/X identification greppable, including by the CI checks above,
  and the CMap is the one object a person may have to open by hand to answer "what does this glyph
  say". Together they are ~1.5 kB of a small file and nothing at 500 pages.
- **Image recompression.** Image XObjects are already `FlateDecode`d, and re-encoding a JPEG is a
  colour-management decision, not a size one.
- **A compression *level* knob.** `flate2::Compression::default()` is what the image and font paths
  use, and export must stay byte-reproducible — `SAMPLE_EXPORT_DIGEST` and the export-size budget
  both depend on the level being fixed rather than chosen. The workspace manifest already records
  what happened the last time a flate backend moved underneath this crate.
