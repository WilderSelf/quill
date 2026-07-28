# Specs

Quill is built **spec-driven**: non-trivial features begin as a spec here, agreed before
implementation. Each spec defines *what* must be true (behavior, inputs/outputs, acceptance
criteria, edge cases), not *how* it is coded. Code and tests are written to satisfy the spec;
when behavior changes, the spec changes first. Commits and PRs reference the spec they advance.

Numbering is sequential (`NNNN-short-slug.md`). Status is one of: `draft`, `accepted`,
`in-progress`, `implemented`, `superseded`.

| # | Spec | Milestone | Status |
|---|------|-----------|--------|
| 0001 | [Press-ready PDF/X export](0001-pdf-x-export.md) | M0 | implemented |
| 0002 | [Real PDF/X byte generation](0002-pdf-byte-generation.md) | M0 | implemented |
| 0003 | [PDF/X-3:2002 output](0003-pdf-x3-output.md) | M0 | implemented |
| 0004 | [User-supplied font embedding](0004-user-font-embedding.md) | M0 | implemented |
| 0005 | [Color CMYK image embedding](0005-color-cmyk-images.md) | M0 | implemented |
| 0006 | [Per-pixel image ink-coverage clamping](0006-image-ink-clamping.md) | M0 | implemented |
| 0007 | [Preflight: Marks & Transparency checks](0007-preflight-marks-transparency.md) | M0 | implemented |
| 0008 | [JPEG image input](0008-jpeg-image-input.md) | M0 | implemented |
| 0009 | [Image placement sizing (true aspect ratio)](0009-image-sizing.md) | M0 | implemented |
| 0010 | [PNG input normalization (indexed + 16-bit)](0010-png-normalization.md) | M0 | implemented |
| 0011 | [CFF / OpenType-CFF (.otf) font embedding](0011-cff-font-embedding.md) | M0 | implemented |
| 0012 | [CMYK JPEG input](0012-cmyk-jpeg-input.md) | M0 | implemented |
| 0013 | [Bleed single source of truth](0013-bleed-single-source.md) | M0 | implemented |
| 0015 | [Real width-based line breaking (font metrics)](0015-text-metrics-line-breaking.md) | M0 | implemented |
| 0016 | [Rustybuzz text shaping (kerning/ligature measurement)](0016-rustybuzz-shaping.md) | M1 | implemented |
| 0017 | [Knuth-Plass optimal line breaking](0017-knuth-plass-line-breaking.md) | M1 | implemented |
| 0018 | [Hyphenation (Knuth-Liang, en-US)](0018-hyphenation.md) | M1 | implemented |
| 0019 | [Text frames + threading](0019-text-frames-threading.md) | M1 | implemented |
| 0020 | [Multi-column thread from page setup](0020-multi-column-thread.md) | M1 | implemented |
| 0021 | [Linked-image proxy pixels (PNG)](0021-png-proxy-pixels.md) | M1 | implemented |
| 0022 | [Linked-image proxy pixels (JPEG)](0022-jpeg-proxy-pixels.md) | M1 | implemented |
| 0023 | [Populate the proxy cache from a document's linked assets](0023-proxy-cache-from-assets.md) | M1 | implemented |
| 0024 | [Incremental proxy-cache invalidation (skip unchanged assets)](0024-proxy-cache-invalidation.md) | M1 | implemented |
| 0025 | [`.tpub` container + versioned load contract](0025-tpub-container-and-load-contract.md) | M1 | implemented |
| 0026 | [Stable `BlockId`, document revision, O(1) asset lookup](0026-block-identity-and-revision.md) | M1 | implemented |
| 0027 | [Performance harness + CI budget gate](0027-perf-harness.md) | M1 | implemented |
| 0028 | [Persisted paragraph styles](0028-paragraph-styles.md) | M1 | implemented |
| 0029 | [Per-page template seam (at parity)](0029-page-template-seam.md) | M1 | implemented |
| 0030 | [Authored master pages and margins; `FORMAT_VERSION` 2](0030-persisted-masters-format-v2.md) | M1 | implemented |
| 0031 | [Incremental, dependency-tracked layout](0031-incremental-layout-session.md) | M1 | implemented |
| 0032 | [`quill-fonts` + shared page geometry](0032-shared-fonts-and-page-geometry.md) | M1 | implemented |
| 0033 | [Screen render: paint list, rasterizer, `quill render`](0033-screen-render.md) | M1 | implemented |
| 0034 | [egui app shell](0034-egui-app-shell.md) | M1 | implemented |
| 0035 | [Per-page master assignment](0035-per-page-master-assignment.md) | M2 | implemented |

Related: the open file-format specification lives in [`../docs/format-spec.md`](../docs/format-spec.md).
The sequenced plan these specs implement — milestones, increment order, and the reasoning behind
that order — lives in [`../docs/roadmap.md`](../docs/roadmap.md).
