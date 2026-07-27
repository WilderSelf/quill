# 0032 — `quill-fonts`, and page geometry promoted out of `export-pdf`

**Milestone:** M1 · **Status:** implemented

## Why

Everything that decides where a glyph goes has to agree, or the screen shows one thing and the
printed page another.

The only real `RunMetrics` implementation in the workspace lived inside `export-pdf`'s **private**
`mod fonts`, and `page_geom` was private too. A screen renderer therefore had two options, both bad:

- depend on `export-pdf`, dragging `pdf-writer`, `subsetter`, `lcms2` and the entire press pipeline
  into the paint path; or
- lay out with the monospace approximation used by tests — and disagree with the exported PDF about
  every line break.

Neither is a layering accident to be worked around later; the second is a WYSIWYG editor that
doesn't.

## What

### `quill-fonts`

A new crate holding font *facts*: advances, shaped run widths, ascent and descent, glyph outlines,
and a font identity for cache keys. Its whole dependency tree is `ttf-parser`, `rustybuzz` and
`quill-text-layout` — no PDF types at all.

Subsetting, Identity-H encoding and FontDescriptor flags stay in `export-pdf`, because they are
about the *file format*, not about the font.

**`export-pdf` now measures through this crate**, so there is exactly one shaper in the workspace
and no way for the two paths to drift. That is the substantive part of the change — a shared crate
that the exporter did not actually use would leave the drift it was created to prevent.

Glyph outlines are returned as a flat `PathCmd` list rather than a backend path type. The screen
rasterizer is deliberately swappable (see the decisions log in `docs/roadmap.md`), and a font crate
that named a specific canvas type would pin that choice here.

### Page geometry

`PageGeom` and `page_geom` move to `quill-core-model`, beside the `PageSetup` they are derived from.
`export-pdf`'s private copy is **deleted**, not left behind — spec 0013 exists because a validator
once read a different field than the writer emitted, and the fix was one source of truth per checked
property. Two copies of the bleed geometry would be the same bug waiting to recur.

The top-left → bottom-left **flip** stays in `export-pdf`. It is a PDF coordinate convention, not a
fact about the page.

## Acceptance criteria

- [x] Exporting `Document::sample()` is **byte-identical** after the shaper swap — the digest test passes unchanged, which is the whole guarantee a refactor of this kind can offer.
- [x] All 73 pre-existing `export-pdf` tests pass untouched.
- [x] `measure_run` and the summed advances of `shape` agree to within 0.001 pt for the same input — the invariant screen rendering rests on. If drawing and measuring could disagree, text would be wrapped by one and positioned by the other.
- [x] Shaping is genuinely kerning-aware: `AV` measures narrower than the sum of its per-character advances.
- [x] Shaped glyph positions advance left to right, starting at 0.
- [x] A letter has a non-empty outline; a **space** has an empty command list with a non-zero advance. Empty is not the same as missing, and a renderer that conflates them drops every space.
- [x] An uncovered character reports no glyph rather than a wrong one.
- [x] `identity()` is stable for identical bytes.
- [x] Garbage bytes are rejected rather than panicking.
- [x] `cargo tree -p quill-fonts` shows only `ttf-parser`, `rustybuzz` and `quill-text-layout` — no PDF crates.
- [x] Neither `render` nor `app` depends on `quill-export-pdf` (asserted against their manifests).
- [x] Every crate still builds standalone, and clippy is clean.

## Non-goals

- **`text-layout` emitting positioned glyph runs.** `Line` still carries text plus a justification
  adjustment, so the renderer re-shapes each line through this crate rather than receiving glyphs
  from layout. That keeps one shaper but two derivation sites, which is a real (recorded) wart —
  fixing it means reconciling shaping GIDs with subset GIDs, which is spec 0016's still-open
  follow-up and much larger than this refactor.
- Font *selection*: fallback chains, per-block faces, `fontdb` integration. There is still one font
  per document.
- Vertical writing modes and complex-script itemization, both inherited non-goals from spec 0016.
