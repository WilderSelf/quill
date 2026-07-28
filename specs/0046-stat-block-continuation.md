# 0046 — Stat-block continuation

**Milestone:** M3 · **Status:** implemented

## Why

Spec 0038's roadmap entry promised keep-together-else-split-at-a-section-boundary and shipped only
the first half. Splitting was descoped, correctly: it is not a stat-block feature, and building a
stat-block-only splitter would have been the second of three implementations. Spec 0044 built the
mechanism and 0045 gave tables their rows. This is the last of the three callers.

Until now a stat block that fits no frame was placed into one anyway and allowed to overflow the
page — the only thing the engine could do with a block it could not cut.

## What

### Sections are the unit

A stat block is a composite of named sections — name, overview, attributes, details, actions,
reactions (`crates/components-ttrpg/src/lib.rs`). Its break opportunities are the boundaries
*between* those sections and nowhere else, so an attributes list is never separated from itself and
a prose section never breaks mid-run.

`measure_stat_block` already knows where a section starts: it is where it draws a separating rule.
The same flag now records a `PanelSplit` item boundary, so the two cannot drift.

### Keep-together is preserved, and becomes a policy rather than an accident

This is the part that is easy to get backwards. Before 0044 a stat block kept together for a reason
that had nothing to do with stat blocks: *nothing* split. Now that it can be cut, keeping it
together has to be chosen.

`PanelSplit` gains `keep_together`, and the flow loop prefers moving a block whole when it would fit
the continuation frame **entire**, cutting only when it fits nowhere. That is the right policy for a
composite and the wrong one for a paragraph — moving a paragraph whole is what left the hole 0044
exists to fix — which is exactly why it is a per-block flag rather than a global rule. Tables take
it too: a small random table that would fit the next column intact should move there rather than
break across two.

### The panel's own inset, on both sides of a cut

A stat block is a padded box. A fragment must close with the same inset it opens with, and a
continuation must not start with its text sitting on the rule. So `PanelSplit` gains `trailing_pt`
— space a fragment reserves below its last item — as the mirror of `repeat_h`, which 0045 added as
space a continuation reserves above its first. A stat block sets both to the panel padding and
repeats no content; a table repeats its header and reserves nothing below.

`cut_fitting` counts `trailing_pt` as part of what must fit, or a cut panel's bottom edge is drawn
past the frame.

### One section per fragment, not two

Spec 0044's two-item minimum is a widow-and-orphan rule, and it is about *lines*. A section is not a
line: it is a unit the reader recognises, and one alone at the top of a column reads as a section,
not as a stranded fragment.

Demanding two sections a side is also actively harmful. Three sections is below the four that
two-a-side needs, so a creature with a name, an overview and one long actions list could not be cut
at all and would be placed whole and overflow the page. The minimum is therefore per-variant:
`MIN_LINES_PER_FRAGMENT = 2`, `MIN_ROWS_PER_FRAGMENT = 2`, `MIN_SECTIONS_PER_FRAGMENT = 1`.

### What still overflows

A stat block whose *single* section is taller than a frame — one enormous actions list, with nothing
else — has no legal cut and is placed whole, overflowing. That is the documented fallback and it is
deliberate: an empty fragment would advance nothing and loop forever. Splitting *inside* a section
is a paragraph problem that 0044 already solves for paragraphs, and wiring it through the composite
is recorded in the roadmap's open questions rather than half-built here.

## Acceptance criteria

- Regression: `Document::sample()`'s export byte-hash unchanged; every existing test passes.
- `a_stat_block_moves_whole_to_the_next_frame_rather_than_splitting` is *replaced*, not deleted, by
  two tests. A stat block that fits the next frame still moves whole to it; one that fits no frame
  splits at a section boundary. **The preference order is the assertion**, because "splits
  correctly" is worthless if it splits things that should have moved.
- Conservation: every section's text appears exactly once, in document order, across the fragments.
- No cut falls inside a section — asserted by a six-attribute stat block whose attributes all land
  on one page.
- A three-section block still admits a cut. Asserted, and the assertion fails if the section minimum
  is raised to two — which is what makes the minimum a decision rather than a number.
- A one-section block too tall for a frame is placed whole and overflows.
- The panel closes and reopens per fragment: one rect each, each within its own frame.
- Nothing overruns its column in the two-column `rulebook` template — the narrow measure is what
  makes a legal cut hard to find, so it gets its own assertion over the real template.
- A rendered image of a stat block breaking across a column is attached to the PR.

## Test strategy

The preference-order test is written first, because it pins the behaviour that could most easily be
lost. Conservation next. The three-section test is written *against* the section minimum: it was
checked by raising the constant to two and confirming the test fails, so it measures the decision
rather than restating it.

## Risks

**Getting the preference backwards**, so a stat block that would have fitted the next column intact
is cut instead. Silent, and only visible as a page that reads worse. The keep-together test is the
guard.

**The panel inset.** Charged on the wrong side of a cut, a fragment's bottom edge draws past the
frame or a continuation's first line sits on the rule. The first is asserted; the second was checked
by rendering.

**Fixtures that do not exercise what they claim.** The first render of this increment used
`AC: 14` inside a `:::statblock` fence, which is not the authoring syntax — the importer warned and
folded those lines into `details`, producing one 412 pt section that could not be cut and a panel
that ran off the page. The engine was behaving correctly; the fixture was wrong. Worth recording
because the failure looked exactly like an engine bug, and the probe that settled it took less time
than the second guess would have.
