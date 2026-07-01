# Specs

Quill is built **spec-driven**: non-trivial features begin as a spec here, agreed before
implementation. Each spec defines *what* must be true (behavior, inputs/outputs, acceptance
criteria, edge cases), not *how* it is coded. Code and tests are written to satisfy the spec;
when behavior changes, the spec changes first. Commits and PRs reference the spec they advance.

Numbering is sequential (`NNNN-short-slug.md`). Status is one of: `draft`, `accepted`,
`in-progress`, `implemented`, `superseded`.

| # | Spec | Milestone | Status |
|---|------|-----------|--------|
| 0001 | [Press-ready PDF/X export](0001-pdf-x-export.md) | M0 | draft |
| 0002 | [Real PDF/X byte generation](0002-pdf-byte-generation.md) | M0 | draft |

Related: the open file-format specification lives in [`../docs/format-spec.md`](../docs/format-spec.md).
