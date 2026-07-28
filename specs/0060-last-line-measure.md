# Spec 0060 — A line may not be drawn past its measure

**Milestone:** M4 · **Size:** medium · **Status:** implemented

## Problem

The roadmap's known issue, measured by spec 0048: `base_demerits` permits a **last line** up to
`measure + shrink`, on the strength of shrink that `justify_paragraph_*` never applies to it — a
last line gets `space_adjust_pt: 0.0` and is drawn at its natural width. A 120 pt measure with a
12 pt hanging indent draws a last line to 126 pt.

Writing the corpus assertion for that found the rest of it. **Every line of a ragged paragraph has
the same problem**, for the same reason and with the same one-line cause: ragged setting shrinks
nothing either. That is not a second defect, it is the whole shape of the first one — and it
matters more in practice, because stat blocks, table cells, headings and contents entries are all
set ragged. Spec 0054's verification render shows it: text touching the panel edge.

## The rule

**A line that will be drawn at its natural width may not exceed its measure.**

Knuth-Plass permits a line whose natural width exceeds the measure by up to its available shrink,
*on the understanding that the renderer tightens the spaces to pull it back*. That understanding
holds for exactly one case: an interior line of a justified paragraph. It does not hold for a last
line, and it does not hold for any line of a ragged one.

So the breaker is told which case it is in. `break_paragraph_shrinkable` takes a `shrinkable` flag;
`justify_paragraph_indented` passes `align == Justified`. Inside `base_demerits`:

```rust
let drawn_at_natural = is_last || !shrinkable;
if drawn_at_natural && natural > l { return None; }
```

A ragged interior line is then scored as an ordinary underfull line, so the breaker still prefers an
even rag to a jagged one.

This is the one place the rule can live. Clamping at paint time would overlap glyphs; clamping in
`justify_*` would disagree with the breaker about how many lines the paragraph has.

**The greedy fallback is exempt and must be.** `break_by_width` fires precisely when some single
word is wider than the measure, at which point no breaking can keep every line inside it, and laying
out visibly beats not laying out (spec 0018).

## The blast radius — which is the increment

### Spec 0051's equivalence digest, re-derived

Derived by 0051's documented procedure: the pre-change tree, with the test file exactly as it now
stands, reproduced the recorded value; the post-change tree then reported the new one.

| | paragraphs | lines | digest |
|---|---|---|---|
| before | 3,251 | 730,328 | `0x9ff1d0465473aa9c` |
| after | 3,251 | 730,582 | `0xacb8c7ec672a3410` |

254 of 730,328 lines break differently — 0.03%.

**The corpus is now pinned by block count**, not by `synthetic_document`'s measure-to-500-pages
convergence, and that correction had to come first. The old corpus was sized by *laying the document
out*, so a deliberate line-breaking change moved the corpus and the breaking at once and there was
no way to tell them apart. Pinning it at the 3,374 blocks the 500-page target had converged to
reproduces the recorded digest exactly on the pre-change tree, which is what proves the pinning
changed no workload.

### Spec 0054's component-parity digests, re-derived

Six of ten fixtures move; the four that do not are the table cases whose cells were never
over-measure. Both sets are recorded in `crates/layout-engine/tests/component_parity.rs`.

### Spec 0046's narrow-column case

`Document::sample()`'s export is **byte-identical** (8,786 bytes, same SHA-256): its body is
justified and does not move.

What does move is where spec 0046's *uncuttable* case begins. `Detail 0 about the creature.`
measures 151.2 pt against the 150 pt inner measure of a `rulebook` panel, so every such run was one
line and is now two — a section is a little over twice as tall. Measured on both builds with the
same fixture: **24 sections fitted and 26 overflowed before; 8 fits and 10 overflows now.**

The limitation itself is unchanged and is spec 0046's: a stat block is cut *between sections and
nowhere else*, so a single section taller than its frame has no legal cut and the panel is placed
whole, running off the page. The fixture moved to 8; and
`a_section_taller_than_its_column_is_placed_whole` now pins the other side of the threshold, so the
limitation is **asserted rather than merely survived** — including the property that holds
regardless, that an uncuttable panel is placed badly and never dropped. It is recorded in the
roadmap's known issues, and the test says in as many words that it inverts when per-section
splitting ships.

## Acceptance criteria

- No line, including a last line and including every ragged line, is drawn wider than its own
  measure — asserted over the corpus (`no_line_is_drawn_past_its_measure`), which checks >100,000
  lines at four measures in both alignments and reports how many paragraphs took the greedy
  fallback rather than skipping them silently.
- 0051's equivalence digest is re-derived by the documented procedure and **both** values recorded.
- 0054's parity digests likewise.
- 0046's narrow-column behaviour is adjusted deliberately and said so, with both thresholds
  measured.
- `Document::sample()`'s export byte-hash is unchanged.
- `benches/budgets.toml` is unchanged and every budget still met.
- The known-issue entry is deleted from `docs/roadmap.md`.

## Non-goals

- Per-section paragraph splitting inside a composite. That is the roadmap's open question and the
  fix for the limitation above; spec 0044's mechanism exists, wiring it through the composite does
  not.
- Changing the greedy fallback. See above.
