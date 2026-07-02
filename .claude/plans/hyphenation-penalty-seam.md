# Plan — spec 0018 incr. 1: penalty item stream + `Hyphenator` seam at parity

**Spec:** `specs/0018-hyphenation.md` (increment 1). **Crate:** `text-layout` only (no other crate touched).

## Goal
Generalize Knuth-Plass line breaking from a box/glue model to a box/glue/**penalty** item stream and
introduce a `Hyphenator` seam whose `NoHyphenator` default changes nothing. Dependency-free.

## Design decisions
- **Keep `break_paragraph`/`justify_paragraph` signatures unchanged** as thin wrappers that pass
  `NoHyphenator`, and add hyphenator-aware siblings `break_paragraph_hyphenated` /
  `justify_paragraph_hyphenated`. This keeps every existing spec-0017 test and the `layout-engine`
  caller literally unchanged (parity), so the increment stays inside one crate. incr. 2 switches
  `lay_out` to the `_hyphenated` sibling with `HypherHyphenator`.
- **Item stream:** each word splits at the hyphenator's interior byte offsets into segment **boxes**
  separated by **flagged penalties** (width = `measure_run("-")`, cost `HYPHEN_PENALTY`); inter-word
  **glue** unchanged (`g`, `g/2`, `g/3`). With `NoHyphenator` a word is one box ⇒ byte-identical to
  spec 0017.
- **DP over legal breakpoints** (glue + penalty item indices, plus a forced terminal at end-of-stream)
  with prefix sums of natural/stretch/shrink for O(1) line cost. Demerits reuse the spec-0017 badness
  formula; add `+HYPHEN_PENALTY²` when a line ends at a penalty and `+DOUBLE_HYPHEN_DEMERIT` when this
  line and the previous both end flagged. `f64` accumulation + `(demerits, fewest-lines, earliest
  starts)` tie-break carry over.
- **Reconstruction:** walk each line's items (box text, glue → space, un-taken penalty → nothing);
  append `-` when the line ends at a penalty.

## Constants (TeX defaults)
`HYPHEN_PENALTY = 50`, `DOUBLE_HYPHEN_DEMERIT = 10_000`.

## Tests
1. Parity: `break_paragraph_hyphenated(&NoHyphenator)` == spec-0017 outputs on representative paragraphs.
2. Penalty tightens fit under a stub: hyphenated breaking chosen, broken line ends in `-`, natural
   width includes the hyphen, total demerits (incl. `HYPHEN_PENALTY²`) < the non-hyphenated breaking.
3. Double-hyphen demerit: a case where the ladder has lower *base* demerits but a fewer-consecutive-
   hyphens breaking wins once `DOUBLE_HYPHEN_DEMERIT` is applied.
4. Deterministic across runs with a hyphenator.
5. Justified hyphenated line fills the frame (hyphen counted).

## Acceptance
Workspace green (`fmt`, `clippy --all-targets --all-features -D warnings`, `build`, `test`); **no new
dependency**; Ghostscript golden unmoved (real pipeline still `NoHyphenator`).
