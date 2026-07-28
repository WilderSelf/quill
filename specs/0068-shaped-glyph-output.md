# 0068 — The PDF draws the glyphs that were measured

**Milestone:** M5 (closeout) · **Status:** in progress

## Why

Measurement has shaped since spec 0016. The screen has drawn shaped glyphs since spec 0033. The PDF
writer still encodes character by character, so the press file — the artefact the product exists to
produce — is the one surface in the workspace that disagrees with the other two.

`CLAUDE.md` says the `fonts` crate exists so that there is "one shaper for screen *and* press, so
they cannot drift." There is one shaper. Press does not use it for drawing, and the drift is
measurable.

### What the gap actually is

Measurement calls `rustybuzz::shape` with an empty feature list, which means HarfBuzz's defaults for
horizontal LTR — **`liga`/GSUB as well as `kern`/GPOS**. `EmbeddedFont::encode_line` walks
`text.chars()`, and the subset is cut from a `BTreeSet<char>`. So the drawn glyph *sequence* differs
from the measured one, not merely its spacing, and the ligature's glyph id is not even in the subset.

Measured on the bundled face at 10 pt:

| case | chars | glyphs | measured | drawn | gap |
|---|---|---|---|---|---|
| ordinary prose | 59 | 59 | 285.22 | 286.38 | +1.16 pt |
| kern-heavy `AV Wa To Yo, Pa.` | 16 | 16 | 76.85 | 81.38 | +4.53 pt |
| `office` | 6 | **4** | 26.10 | 26.99 | +0.89 pt |
| ligature-rich prose | 62 | **52** | 260.93 | 265.83 | +4.90 pt |
| control — neither feature fires | 17 | 17 | 113.59 | 113.59 | 0.0000 |

The control row is what makes the others attributable rather than noise. An ordinary sentence loses
ten glyphs to ligatures and draws 4.90 pt wider than it measured — 1.1% of a 432 pt measure, and
four times the figure the known issue carried when it recorded the kerning half alone.

This is spec 0060's rule leaking: a last line *proven* to fit its measure can still be drawn past it.

### Why the recorded fix could not work

The known issue said: emit the kerning as `TJ` adjustments, the same array justification already
uses. A `TJ` array interleaves numbers between pieces of **one encoded string**, so it can only
express a correction that is positional. With 52 shaped glyphs against 62 encoded characters there is
no per-piece correspondence to hang the adjustments on.

Two ways out:

1. **Stop applying `liga` in measurement.** The character-encoded draw becomes correct and the fix
   reduces to per-pair `TJ`. **Rejected** — a desktop publishing application whose press output
   cannot set an `fi` is not one, and the defect would be paid for in typography rather than in code.
2. **Draw the shaped glyph run.** This is spec 0016's named non-goal, deferred there, again in 0032,
   and once more as an open question in the roadmap. The kern adjustments fall out of it for free,
   because shaping yields the GPOS deltas at the same moment it yields the glyph ids.

This spec is (2).

## What

### The subset is keyed by glyph, not by character

`collect_doc_faces` returns `BTreeMap<FaceKey, BTreeSet<char>>` today. It becomes a set of
**original glyph ids**, obtained by shaping each run in the face it is set in — which is the only way
a ligature's glyph can enter the subset at all.

`build_from_bytes` takes those gids. `char_to_gid` is no longer the encoding path and is replaced by
the remapper's `orig_gid → subset_gid`. The invariant `gids_are_consistent` pins — that
`widths[subset_gid]` is the original `hmtx` advance — is unchanged and still what makes `/W` correct.

### The writer encodes the shaped run and corrects the advances

For each piece of text drawn, the writer shapes it through the same `quill-fonts` entry point
measurement used, encodes the resulting glyph ids, and emits the advance corrections as `TJ` numbers.

The viewer advances by `/W[gid]`, which is the raw `hmtx` advance. The correction after a glyph is
therefore, in thousandths of the text-space unit:

```
adjust = hmtx_units(gid) − shaped_advance_units(glyph)
```

`TJ` amounts are **subtracted** from the position (`writer.rs:796-808` already documents this for
justification), so a tightening kern — where the shaped advance is the smaller — yields a positive
number, which moves the pen left. That is the wanted direction.

The amount is **rounded to an integer** explicitly. With the bundled faces' `upem == 1000` it is
already integral, but a user font at 2048 would otherwise emit `ryu` shortest-round-trip floats,
making both the file size and the digest's cross-platform reproducibility depend on `f32` ordering.
Relying on the bundled font's `upem` is exactly the kind of accident spec 0025 was bitten by.

A zero adjustment is not emitted. Most glyph pairs do not kern, so emitting `0` for each would grow
the file for no effect — and the control-case assertion below is what proves the suppression is
correct rather than hiding a bug.

### `/ToUnicode`

No CMap is written today for any font. With `Identity-H` and no `/ToUnicode`, a copy or a search over
a quill PDF already yields subset glyph ids rather than text. That is survivable while the encoding
is one glyph per character, because the mapping is recoverable in principle.

Drawing ligatures makes it unrecoverable from outside the file: nothing downstream can learn that one
glyph was three characters. The cluster map that states it exists **only at the moment of shaping**,
so reconstructing it later means shaping the whole document again.

So the CMap ships here, built from the shaping cluster map, with `bfchar`/`bfrange` entries mapping
each subset glyph id to the UTF-16BE string it came from — one character for an ordinary glyph, three
for `ffi`.

## Acceptance criteria

- **Drawn width equals measured width to 0.01 pt** over a corpus spanning the five cases above,
  asserted by summing the `/W` entries of the glyphs actually emitted plus the `TJ` adjustments
  actually written — *not* by re-deriving from the shaper, which would assert the shaper against
  itself. The control case must come out at exactly zero adjustment, or the instrument is measuring
  its own machinery.
- `ffi` in the source draws as one glyph, and that glyph's id is present in the subset. Today it
  cannot be, because the subset is cut from a set of characters.
- A `/ToUnicode` CMap maps every emitted glyph back to its characters, with the ligature mapping to
  the three it came from. Asserted by extracting text from the exported PDF and comparing it to the
  source — which nothing in the workspace can do today.
- Both show paths carry it: `show_line` **and** `show_line_by_span`. A kern pair straddling a span
  boundary is neither lost nor doubled; there is no test for that boundary today, and the trap is the
  one already documented at `writer.rs:920-924` for justification.
- **`SAMPLE_EXPORT_DIGEST` is re-derived under the content-stream template** — the third of the three
  classifications in the ledger at `crates/export-pdf/src/lib.rs:1622-1690`, and the first
  content-stream move since spec 0028. The new operators are *inspected and stated*, not accepted.
  Exported against the committed parity ICC (never `synth_cmyk_profile()`, which stamps a timestamp),
  and diffed against a build of `main` in a worktree.
- Content streams are not `FlateDecode`'d, so every added byte lands in the file. The size change for
  `Document::sample()` and for the 500-page synthetic document is **measured and recorded**. No
  budget guards export size today; if the growth warrants one, that is a finding for this spec rather
  than a silent regression.
- Two tests assert the current contract and are **rewritten rather than deleted**:
  `ragged_line_uses_a_single_show` asserts a ragged line contains no `TJ`, which this invalidates by
  design; `every_line_is_shown_in_the_face_it_was_set_in` walks the stream matching `" Tj"` and so
  goes structurally blind rather than failing.
- The Ghostscript CI gate is the external check that matters here. `-dPDFSTOPONERROR` is what catches
  a malformed `TJ` array — an unbalanced `[ ] TJ`, or an adjustment emitted beside a zero-length
  string — which is the actual risk. A legal-but-wrong array is caught by the width assertion.

## Non-goals, with the residual stated

Spec 0016's style: the known issue may only be deleted if what remains is written down.

- **The screen renderer's word splitting.** `crates/render/src/raster.rs:221` splits on `' '`
  unconditionally and shapes each word separately, so a kern pair straddling a space is lost on
  screen while `measure_run` — which shapes the whole line — counts it. Same class of defect, opposite
  direction, and *not* what this increment is about. It is a small change and it ships here with its
  own assertion if it is genuinely small; otherwise its magnitude is measured and recorded. Either
  way the stale comment at `raster.rs:186-191` — "the word positions are derived the same way the
  writer derives its `TJ` offsets" — stops being true the moment this lands, and is corrected here.
- **GPOS `x_offset`/`y_offset`.** `Font::shape` currently discards them, keeping only the advance.
  They are zero for `kern` on the bundled faces and matter for mark attachment, which the bundled
  faces' `mark`/`mkmk` features could produce for combining diacritics. Out of scope; recorded so
  that the next person to meet a misplaced accent finds this line rather than rediscovering it.
- **Tracking and ligatures.** `measure_format` spends tracking per *shaped* glyph while the PDF's
  `Tc` applies per *encoded* glyph. Those agree once the encoding is the shaped run, which is this
  spec — so the pre-existing mismatch closes as a side effect rather than as a claim.
- **Compressing content streams.** `FlateDecode` on the content stream would absorb the size growth,
  and would break every test that greps the finished PDF for operator text plus the CI `grep -aq`
  checks. Separate increment, named here because this is the increment that makes it worth doing.
