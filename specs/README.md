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

Related: the open file-format specification lives in [`../docs/format-spec.md`](../docs/format-spec.md).
