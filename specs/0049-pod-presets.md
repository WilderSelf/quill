# 0049 — POD presets: the printer's requirements as data

**Milestone:** M3 · **Status:** implemented

## Why

Every number quill checks at preflight is one vendor's, hard-coded and invisible:
`MAX_INK_COVERAGE_PCT = 240.0` (crates/color/src/lib.rs), `min_dpi`'s 300/600
(crates/export-pdf/src/lib.rs), `DEFAULT_BLEED_PT = 9.0` (crates/core-model/src/lib.rs), and
`PdfxVersion` defaulting to X-1a. They are reasonable, they are what `CLAUDE.md` attributes to
DriveThruRPG — and a user printing with a vendor that wants something different has no way to say
so and no way to discover they should have.

This increment turns those four numbers into fields of a named `PodPreset`, and adds the one number
the workspace has never had: a **safety margin**, the distance inside the trim within which content
is at risk from the guillotine. Nothing reads the safety margin yet; spec 0050 adds the check that
does. It is introduced here because a preset is where the number belongs, and because 0050 needs a
preset to exist before it can read one.

## What

### The type

```rust
pub struct PodPreset {
    pub name: String,
    pub title: String,
    /// Where these numbers came from. Read it before trusting them.
    pub source: String,
    /// The date `source` was consulted (`YYYY-MM-DD`).
    pub retrieved: String,
    /// True only when the numbers were read from `source` itself.
    pub confirmed: bool,
    /// The trims this preset states. **Empty means it states none** — the check is then inert.
    pub trim_sizes: Vec<Size>,
    pub bleed_pt: Pt,
    /// Content nearer the trim than this is at risk. `0.0` means the preset states no margin.
    pub safety_pt: Pt,
    pub max_ink_pct: f32,
    pub min_dpi_color: f32,
    pub min_dpi_line_art: f32,
    pub pdfx: PdfxVersion,
}
```

It lives in `quill-export-pdf` (`preset.rs`), next to `PdfxVersion` and next to the only code that
reads it. `ExportOptions` gains a `preset` field defaulting to `PodPreset::generic()`.

### `generic` is today's constants, exactly

`PodPreset::generic()` is the default and is numerically identical to the constants it replaces —
9 pt bleed, 240% ink, 300/600 dpi, PDF/X-1a:2001 — so adding presets changes no existing behaviour.
An integration test asserts each field against the constant it came from, and a **golden report**
asserts `quill preflight` with no `--preset` produces byte-identical stdout to the pre-preset build.
This increment is a refactor plus a flag; any behavioural difference is a bug, and the golden report
is what says so.

### Provenance is a field, not a comment

Vendor requirements change. A preset that cannot be audited against its source becomes wrong
silently — the same failure class as a CI job that is not a required context. Every bundled preset
carries a non-empty `source` and `retrieved`, asserted for all of them, and `quill presets` prints
both.

### Honesty about the numbers — what is shipped and what is not

**Vendor presets are a convenience to be confirmed against the vendor's current specification. They
are not a warranty.** This is `CLAUDE.md`'s "prefer a visible failure over silent press-corruption"
applied to data: a preset that misstates a vendor's requirement is worse than no preset, because it
looks authoritative.

Concretely, the bundled catalogue:

| preset | numbers | confirmed |
|---|---|---|
| `generic` | 9 pt bleed · 18 pt safety · 240% ink · 300/600 dpi · PDF/X-1a:2001 · common trade trims | yes, against quill's own spec 0001 |
| `drivethrurpg` | identical to `generic`; no trim catalogue | **no** |
| `lulu` | identical to `generic`; no trim catalogue | **no** |
| `ingramspark` | identical to `generic`; no trim catalogue | **no** |

- `generic`'s four press numbers are quill's own, sourced to `specs/0001-pdf-x-export.md` and
  `CLAUDE.md`, which is a source that can actually be re-read.
- `generic`'s **18 pt (0.25 in) safety margin** is the one number with no in-repo predecessor. It is
  stated as **print-trade convention, not a vendor requirement**, and is labelled that way in the
  `source` string. It is conservative, it is not attributed to anyone, and nothing reads it until
  spec 0050.
- `generic`'s `trim_sizes` are **common trade trims** (6×9, 5.5×8.5, 7×10, 8.5×11 in, A4, A5) —
  a convenience list, explicitly not a vendor catalogue.
- The three vendor presets carry `generic`'s values *because the vendors' current published numbers
  were not re-read when this shipped*. Their `source` says exactly that, `confirmed` is `false`, and
  the CLI prints a note to stderr whenever an unconfirmed preset is selected. No number in them is
  invented: they are named slots, filled conservatively, that say out loud that they are.
- `trim_sizes` is **empty** on all three, because quill will not put a catalogue in a vendor's
  mouth. An empty list states no catalogue and makes the trim check inert, rather than warning about
  a size the vendor may well offer.

### The checks

Every preflight threshold now comes from `opts.preset`:

- ink coverage → `preset.max_ink_pct` (via `quill_color::within_ink_limit_pct`);
- bleed floor → `preset.bleed_pt`;
- image resolution → `preset.min_dpi(line_art)`;
- and one new check, `CheckId::TrimSize`.

`within_ink_limit(color)` is **removed** from `quill-color` in favour of
`within_ink_limit_pct(color, max_pct)`. Deleting the un-parameterised function is what makes it
structurally impossible for a preflight path to fall back to the bare constant — the compiler is the
check, not a convention.

### A trim the preset does not list is a Warning

Not an Error. An unusual trim is a conversation with the printer, not a corrupt file, and preflight
that blocks a legitimate document teaches users to pass `--force`. The severity is asserted in both
directions (a listed trim produces nothing; an unlisted one produces exactly a `Warning`).

### `clamp_cmyk_u8` is out of scope

`quill-color`'s per-pixel clamp still reads `MAX_INK_COVERAGE_PCT`. Making it preset-dependent
changes **image bytes**, which is a different kind of change from moving a threshold, and it would
break the byte-hash regression this increment is built to protect. **Follow-up:** thread the preset
through `RgbToCmyk`/`clamp_cmyk_u8` so a stricter press profile clamps images to its own limit —
with its own byte-hash discussion, because that one legitimately moves bytes.

### A preset is not part of the document

It is an **export-time** concern and is deliberately not serialized into `.tpub`: a document is not
bound to one printer, and the same manuscript is routinely quoted to two. It travels on the command
line. `docs/format-spec.md` records this, and a test asserts a document written by
`quill new --preset …` carries no preset in its manifest.

### CLI

- `--preset <name>` on `preflight`, `export` and `new`; unknown names fail listing the available
  ones, never panic.
- `export`'s `--pdfx` becomes optional and defaults to the preset's conformance level.
- `quill presets` lists every bundled preset with its numbers, `source` and `retrieved` — provenance
  a user cannot read is provenance that does not do its job.
- `new --preset` sets the bleed from the preset, and sets the trim **only when the template's own
  trim is not one the preset lists** (printing a warning when it does). The roadmap's wording was
  "seeds from the preset's first trim"; applying that unconditionally would retrim the US-Letter
  `playtest` template to 6×9 and leave its folio off the page — silently breaking master furniture
  is exactly what this repo forbids.

## Acceptance criteria

- Regression: `Document::sample()`'s export byte-hash unchanged (8786 bytes,
  `081cb937…ae25b0`), and `quill preflight` with no `--preset` is byte-identical to the pre-preset
  build — asserted by a committed golden report, and again with an explicit `--preset generic`.
- `PodPreset::generic()` equals today's constants field by field, asserted against
  `MAX_INK_COVERAGE_PCT`, `DEFAULT_BLEED_PT` and 300/600.
- A structural test asserts no preflight code path still reads the bare constants: the non-test
  portion of `crates/export-pdf/src/lib.rs` mentions neither `MAX_INK_COVERAGE_PCT` nor
  `DEFAULT_BLEED_PT`, and contains neither dpi literal.
- A document within `generic`'s ink limit but over a stricter preset's fails under the stricter one
  and passes under `generic` — asserted both directions, which is the only test that proves presets
  do anything.
- An unlisted trim is a `Warning`; a listed one produces no finding; a preset with an empty
  `trim_sizes` never warns.
- Every bundled preset has a non-empty `source` and `retrieved`; every unconfirmed one says so in
  its `source`.
- `quill preflight --preset lulu` and `quill export --preset lulu` accept the flag; an unknown name
  exits non-zero listing the available names.
- `quill new --preset` seeds the page setup, and the resulting manifest contains no preset.

## Test strategy

The golden report first, because this is a refactor and must prove it; then the strict-versus-generic
pair; then the provenance and trim-severity assertions, table-driven over the bundled presets. The
CLI surface is tested through the real binary (`CARGO_BIN_EXE_quill`), since "the flag is accepted"
and "an unknown name does not panic" are claims about the binary, not about a library function.

## Risks

- **The numbers themselves.** Addressed above: `generic` is the default and is quill's own
  conservative set, so a user who names no vendor is never loosened; every preset states its source
  and date; unconfirmed presets carry conservative values, say so in `source`, and announce
  themselves on stderr when selected.
- **Threading a preset everywhere.** The temptation is to pass a preset into functions that do not
  need one. `clamp_cmyk_u8` is the stated boundary.
- **A new default-path check.** `CheckId::TrimSize` fires under `generic` for a document whose trim
  is not a common trade size. It is a Warning, so `passed()` and every exit code are unchanged; the
  golden report pins the sample's output.
