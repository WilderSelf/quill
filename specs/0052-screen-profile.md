# 0052 — The screen profile: a second export target, with clickable internal links

**Milestone:** M3 · **Status:** implemented

## Why

The open question the roadmap has carried since spec 0042 — "do clickable internal links ever
ship?" — has a real answer: **not in the press file, and yes in a second one.**

PDF/X-1a requires annotations to sit outside the BleedBox. Quill writes `MediaBox == BleedBox`
(spec 0013), and a table-of-contents entry sits in the middle of the text block by definition. A
link and a press-conformant file are therefore mutually exclusive **on the same page**, not merely
awkward together. Every publisher in this audience already ships two PDFs — a press file to the
printer and a screen file to customers — so the honest design is two profiles rather than one
compromise that satisfies neither.

Spec 0042 built the gate for this and left it with nothing to guard:
`annotation_finding(rect, page_setup, page_index)` states the PDF/X rule as code so that the
validator and the writer agree *before* anything relies on it (the spec-0013 lesson). This is the
increment that relies on it.

## What

### `ExportProfile`

```rust
pub enum ExportProfile { Press, Screen }
```

`ExportOptions` gains `profile: ExportProfile`, defaulting to **`Press`** — today's behaviour,
byte-for-byte. `Press` is the default in the `Default` impl, in the CLI, and in every existing
caller, so no path changes profile by omission.

| | `Press` | `Screen` |
|---|---|---|
| `/Annots` link annotations | never | one per TOC entry |
| `GTS_PDFXVersion` (info dict + XMP) | present | **absent** |
| `GTS_PDFXConformance` | per level | absent |
| `/OutputIntents` + `DestOutputProfile` | required | not written |
| `--icc` | required | optional |
| colour model | CMYK / grayscale | **CMYK / grayscale — identical** |
| every other check, box, font, stream | — | unchanged |

### Non-goals, stated so the increment can close

`Screen` is **deliberately not an RGB profile.** Colour conversion is a separate question and
*converting is how you ship the wrong colours*: a screen PDF whose blacks have been round-tripped
through an unmanaged RGB space is a worse artefact than a CMYK one that looks slightly flat on a
monitor. The screen profile changes **what a viewer can click**, and nothing else.

Also explicitly out of scope, for the same reason — each is its own question and each would keep
this increment open indefinitely: **image downsampling / compression**, **reader spreads**,
**embedded thumbnails**, **web-optimised linearisation**, **document open actions**, **outline
colour or style**, **URI (external) links**, and **relaxed bleed, dpi, ink or font rules**. The
screen profile relaxes exactly two things — the PDF/X identification and the OutputIntent
requirement — because those two are what an annotation is incompatible with. Nothing else.

### Where a link comes from

The layout engine emits `PlacedBlock::Link { source, frame, target_page }` for each TOC entry's
title run. It is a **candidate** annotation, not an annotation: it paints nothing, on screen or on
press. `measure_toc` already knows every heading's page (spec 0041 hands it the index), so the
destination is data the layout pass already has rather than something the writer re-derives by
matching strings.

Emitting the candidate in *both* profiles is deliberate. Layout must not depend on the export
target — a document that paginates differently per profile would make the two files disagree about
what is on which page, which is the one thing a reader comparing them would notice. The profile
decides what the **writer** does with the candidate, not what the engine produces.

### Where the press file's emptiness comes from

The writer does not ask "is this the screen profile?" before emitting an annotation. It asks
spec 0042's question:

```rust
ExportProfile::Press  => annotation_finding(&frame, &doc.page_setup, page).is_none(),
ExportProfile::Screen => true,
```

Under `Press` every candidate is put through `annotation_finding`, and because `MediaBox ==
BleedBox` every one of them fails — so the press file contains no annotation at all. Stating it as
the *rule* rather than as `if screen { … }` means the press file's emptiness is a consequence of
the PDF/X requirement, and it survives a future page geometry in which the two boxes differ.

### Preflight under `Screen` reports what it did not check

A profile that silently passes a file it barely examined is the failure mode this repo's
"prefer a visible failure" rule exists to prevent. `PreflightReport` gains

```rust
pub skipped: Vec<Skipped>,   // Skipped { check: CheckId, reason: String }
```

Under `Press`, `skipped` is always empty. Under `Screen` with no ICC supplied, exactly two checks
are recorded as skipped — `OutputIntent` and `IccProfileInvalid` — each with the reason and its
consequence (with no profile, RGB image conversion falls back to `quill_color`'s naive path). Every
other check still runs: colour space, ink coverage, bleed, image resolution, font embedding,
transparency, marks. Supplying `--icc` under `Screen` un-skips the ICC checks and the profile is
used for RGB→CMYK conversion, but **no OutputIntent dictionary is written** — writing one would be
claiming a conformance the file does not have.

`PreflightReport::applied()` returns the checks that did run, so the CLI can print both halves
rather than an unqualified "no findings".

### CLI

```
quill export --profile screen --output book-screen.pdf     # no --icc needed
quill export --output book-press.pdf --icc press.icc       # unchanged; --icc still required
```

`--icc` becomes optional at the parser level and **required by the `Press` profile at run time**:
omitting it under `Press` is a hard error naming the flag, not a preflight finding. Under `Screen`
the CLI prints, before anything else, that the file is **not press-ready** and what it is for, then
the applied/skipped check lists. A user who exports the screen file and uploads it to a printer has
to have ignored a line that says so.

## Acceptance criteria

- **`Document::sample()`'s press export is byte-identical.** 8786 bytes before and after, `cmp`
  clean, `SAMPLE_EXPORT_DIGEST` unchanged. This is the load-bearing property of the increment.
- **The press profile emits zero annotations, asserted by parsing the emitted PDF for any
  `/Annots` key** — over a document that *does* have a TOC and therefore does produce link
  candidates. Written as a press test, not a screen test, and structural over the output rather
  than "this code path was not taken", because only the structural form survives a refactor.
- Under `Screen`, a TOC entry is a `/Subtype /Link` annotation whose `/Rect` covers the placed text
  of the entry (asserted against the run's own laid-out frame, flipped into PDF space) and whose
  `/A` `GoTo` destination resolves — by following the reference to a page object and finding its
  index in the page tree's `/Kids` — to the page the heading is actually on.
- `GTS_PDFXVersion` present under `Press` and absent under `Screen`, asserted **both directions**
  in both the info dictionary and the XMP packet. Likewise `/OutputIntents`.
- `Screen` with no ICC exports successfully; `Press` with no ICC still fails.
- Preflight under `Screen` reports `OutputIntent` and `IccProfileInvalid` as skipped-with-reason and
  still reports a low-dpi image, an RGB block and an over-inked colour as errors — so "skipped" is
  provably narrow rather than a blanket pass.
- The number of link candidates a document produces equals the number of TOC entries it lists.
- `benches/budgets.toml` unchanged.
- CI: the existing (and **already required**) `PDF preflight (Ghostscript)` job gains a screen-file
  leg — export with `--profile screen` and no `--icc`, interpret it with Ghostscript, and assert
  structurally that it carries `/Annots` and `/Subtype /Link` and carries no `GTS_PDFXVersion`,
  while the press file in the same job still does carry it.

## Test strategy

Assertions **parse the emitted PDF**, in the style spec 0042 established. Three of them are
different in kind from anything before:

- **Zero annotations under press** is asserted by scanning the whole file for the `/Annots` byte
  sequence. A test that asserted "the annotation builder returned an empty vector" would pass for a
  writer that emitted annotations by another route; this one cannot.
- **The destination is followed, not pattern-matched.** The test reads the link's `/A` action, takes
  the `/D [n 0 R /Fit]` object number, and looks that reference up in the page tree's `/Kids` array
  to get a page *index*. Asserting the raw object number would pin an allocation order rather than a
  destination, and would keep passing if the pages were allocated differently.
- **The rect is checked against the layout**, not against a constant: the test lays the document out
  itself, finds the `PlacedBlock::Link`, flips its frame through the same `geom` helpers the writer
  uses, and requires the parsed `/Rect` to match. A constant would be re-derivable from the bug.

The CI job is added to the existing Ghostscript job rather than as a new one, for the reason already
recorded in `.github/workflows/ci.yml` at the performance-budget step: a new job emits a new
check-run, and a new check-run is **not** automatically a required branch-protection context, so it
could fail silently while PRs kept merging. Folding the screen leg into a job that is already one of
the four required contexts makes the gate real the moment this merges, with no out-of-band admin
change.

## Risks

- **A link is wrong-in-a-viewer rather than wrong-in-a-parser.** Ghostscript will happily interpret a
  file whose every link points at page 1. That is why the destination test resolves the reference to
  a page index instead of asserting the annotation exists.
- **Layout now emits something the press writer ignores.** The mitigation is the byte-identity
  assertion, which is exactly the property that would break if the press path ever started noticing
  the candidates.
- **`ExportProfile` is a second axis alongside `PdfxVersion`.** `version` is meaningless under
  `Screen` and is ignored rather than rejected, so a caller that sets both gets the screen file it
  asked for. If a third profile ever appears, the pair should become one enum rather than two fields.
