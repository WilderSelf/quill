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
| 0036 | [Document templates + `quill new`](0036-document-templates.md) | M2 | implemented |
| 0037 | [Decoration primitive: fills, rules, borders](0037-decoration-primitive.md) | M2 | implemented |
| 0038 | [`Block::StatBlock`](0038-stat-block.md) | M2 | implemented |
| 0039 | [`Block::Table` — tables and random tables](0039-tables.md) | M2 | implemented |
| 0040 | [Heading index](0040-heading-index.md) | M2 | implemented |
| 0041 | [`Block::Toc` — generated contents](0041-generated-toc.md) | M2 | implemented |
| 0042 | [PDF outline + annotation/bleed guard](0042-pdf-outline.md) | M2 | implemented |
| 0043 | [Authoring on-ramp: `quill import`](0043-markdown-import.md) | M2 | implemented |
| 0044 | [Block fragmentation](0044-block-fragmentation.md) | M3 | implemented |
| 0045 | [Table continuation](0045-table-continuation.md) | M3 | implemented |
| 0046 | [Stat-block continuation](0046-stat-block-continuation.md) | M3 | implemented |
| 0047 | [Master statics: alignment and page-parity mirroring; `FORMAT_VERSION` 3](0047-master-static-alignment.md) | M3 | implemented |
| 0048 | [Hanging indent and non-breaking spaces](0048-hanging-indent.md) | M3 | implemented |
| 0049 | [POD presets: the printer's requirements as data](0049-pod-presets.md) | M3 | implemented |
| 0050 | [Preflight over placed geometry](0050-geometry-preflight.md) | M3 | implemented |
| 0051 | [Knuth-Plass active-node pruning](0051-line-break-pruning.md) | M3 | implemented |
| 0052 | [The screen export profile — clickable internal links](0052-screen-profile.md) | M3 | implemented |
| 0053 | [User-authored templates: `quill new --from`](0053-user-authored-templates.md) | M3 | implemented |
| 0054 | [Component definitions as data](0054-component-definitions.md) | M4 | implemented |
| 0055 | [The `.qpack` container](0055-pack-container.md) | M4 | implemented |
| 0056 | [Pack resolution](0056-pack-resolution.md) | M4 | implemented |
| 0057 | [`quill pack extract`](0057-pack-extract.md) | M4 | implemented |
| 0058 | [The baseline grid](0058-baseline-grid.md) | M4 | implemented |
| 0059 | [Screen/press hyphenation parity](0059-hyphenation-parity.md) | M4 | implemented |
| 0060 | [A line may not be drawn past its measure](0060-last-line-measure.md) | M4 | implemented |
| 0061 | [The gallery and the pack authoring guide](0061-gallery-and-guide.md) | M4 | implemented |
| 0062 | [The neutral core: a mechanism is general or it is a bug](0062-neutral-core.md) | M5 | implemented |
| 0063 | [Inline runs: the paragraph stops being a `String`](0063-inline-runs.md) | M5 | implemented |
| 0064 | [The font family, and the overrides that move a glyph](0064-font-family.md) | M5 | implemented |
| 0066 | [Lists: bullets, numbering, and the counter that survives repagination](0066-lists.md) | M5 | implemented |

Related: the open file-format specification lives in [`../docs/format-spec.md`](../docs/format-spec.md),
and the content-pack authoring guide in [`../docs/pack-authoring.md`](../docs/pack-authoring.md).
The sequenced plan these specs implement — milestones, increment order, and the reasoning behind
that order — lives in [`../docs/roadmap.md`](../docs/roadmap.md).
