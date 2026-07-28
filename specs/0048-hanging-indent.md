# 0048 — Hanging indent and non-breaking spaces

**Milestone:** M3 · **Status:** implemented

## Why

Two related gaps, both found by rendering spec 0038's stat-block panel in the two-column `rulebook`
template's narrow measure.

A wrapped attribute had nothing to align to: `Armour Class: 15 (leather armour, shield)` set its
continuation flush with its key, so the value and its wrap read as two unrelated lines. And nothing
in the engine could express "these words belong together" — `break_by_width` normalizes every run of
inter-word whitespace to a single space, so the wider separator the first draft used collapsed to an
ordinary word space and separated nothing.

Neither is a stat-block feature. A hanging indent is how every reference book sets a key/value list,
and a first-line indent is how every book sets a paragraph opener. They belong in `text-layout`.

## What

### The measure is per line, not per paragraph

```rust
pub struct Indent { pub first_pt: f32, pub rest_pt: f32 }
```

Two numbers rather than a flag, because both conventional shapes fall out of one type: a *first-line
indent* is `first_pt > 0, rest_pt = 0`; a *hanging indent* is the reverse.

The indent narrows the **measure**, not just the drawn position. A justified line must fill
`frame − indent`; filling `frame` instead overruns by exactly the indent, and no ragged-right test
can see it because ragged text has no fill to be wrong about.

Knuth-Plass needs no line counter for this. A line is the first exactly when it starts at item 0, so
`base_demerits` picks its measure from the line's own start index and the DP state is unchanged.
Spec 0051's active-node pruning stays sound for the same reason: its monotonicity argument is
per-start ("a start too far back now is too far back forever"), and each start is retired against
its own measure.

`Line` gains `indent_pt`, carried per line rather than per paragraph so that the screen painter and
the PDF writer read one value and cannot disagree — the same reason `space_adjust_pt` lives there.
It also survives fragmentation for free: a spec 0044 continuation's lines are not first lines and
already carry the right value.

### U+00A0 binds, and is emitted as an ordinary space

Words break at ordinary whitespace only. A NO-BREAK SPACE binds its neighbours into a single
unbreakable box, so `Armour\u{a0}Class:` can never be the boundary itself whatever the demerits
would otherwise prefer.

The replacement matters as much as the binding: the box text carries an ordinary space, because
leaving U+00A0 in it would reach the PDF, where `collect_doc_chars` never gathered it and it would
subset to a `.notdef` box — a visible defect in a press file.

A bound unit wider than the measure is exactly the no-feasible-breaking case, so the greedy fallback
takes over. That is pinned by a test: the alternative — silently honouring a break the author
forbade — would make U+00A0 advisory, and an author who writes one means it.

### The built-in stat-block treatment uses both

`StyleSheet::default()` gives `statblock-attr` a 10 pt hanging indent, and `measure_stat_block`
joins each key's words with U+00A0. Dropping a stat block in gets both with no authoring, which is
what spec 0038 says a first-class component is for.

## What this does not do, and why

**No tab stop.** The roadmap's entry for this increment promised one. It is descoped, and the reason
generalises: `Line` is `{ text, space_adjust_pt, indent_pt }` — one string with evenly distributed
gaps. A tab stop sets *part* of a line at an absolute x, which needs a line to be a sequence of
positioned segments. That is a model change rippling through both painters and the PDF writer, and
it is a much larger change than the defect warrants now that the hanging indent aligns the wrap and
the non-breaking space keeps the key whole.

**The original key-splitting sighting could not be reproduced.** With the current code and real font
metrics, `Armour Class:` does not split in the `rulebook` measure — spec 0038's colon (which
replaced the collapsing double space) already removed the common case. Under the monospace test
metrics it can only split when the key is *wider* than the measure, where binding cannot help
either. So the honest claim is not "this fixes a reproducible break": it is that the key is now
unbreakable **by construction** rather than by luck, and a demerit-driven split is no longer
possible.

## Acceptance criteria

- Regression: every existing test passes. `Document::sample()`'s export moves **only** in its
  identity — `StyleSheet::default()` changed, so `doc.to_json()` and the `/ID` derived from it
  change. Verified rather than accepted: 8786 bytes both sides, 116 differing bytes in 16 runs, all
  inside the XMP `DocumentID`/`InstanceID` or the trailer `/ID`. The sample has no stat block, so
  nothing it draws could have changed.
- A hanging indent narrows the measure of every line after the first — asserted against the
  *broken* line widths, not just the drawn positions.
- A justified indented line fills its own measure exactly, and draws inside the frame.
- `Indent::default()` is exactly the unindented breaker, asserted at four measures. The indented
  entry point must not become a second implementation.
- A first-line indent insets only the first line; a hanging indent only the rest.
- A bound unit is never the whole of a line, and no line ever carries U+00A0.
- A bound unit wider than the measure falls back rather than splitting quietly.
- The indent reaches **placed** geometry, asserted on `PlacedBlock`, and a stat block's wrapped
  attribute hangs under its value.
- The rendered panel shows the wrap indented under the value — attached to the PR.

## Test strategy

Metric assertions in `text-layout`, then placed-geometry assertions in `layout-engine`, then the
render. The parity test (`Indent::default()` ≡ the old breaker) is written first, because the whole
change is only safe if the un-indented path is untouched.

## Risks

**Indents interact with justification, hyphenation and now fragmentation, and the failure mode is a
line slightly too long** — which no test sees unless it asserts the measure. The fill-your-own-
measure criterion exists for that.

**The pruning interaction.** Spec 0051 retires a start once it cannot fit; with two measures the
retirement must use the retiring start's own measure or a first line could be dropped against the
narrower one. Done, and the parity test is what would catch it.

## Found and not fixed

**A paragraph's last line may be drawn past its measure.** `base_demerits` permits a last line up to
`measure + shrink`, on the strength of shrink that `justify_paragraph_*` never applies to it —
last lines get `space_adjust_pt: 0.0` and are drawn at natural width. Measured here: a 120 pt
measure with a 12 pt hanging indent draws a last line to 126 pt.

Not fixed in this increment, and the attempt is what showed why. Returning `None` for an over-wide
last line is a one-line change and it is *correct*, but it moves line breaking across the corpus —
it would move spec 0051's equivalence digest — and it makes stat-block sections tall enough to stop
fitting a narrow `rulebook` column, which sends a block down spec 0046's uncuttable path and off the
bottom of the page. A 0046 test caught exactly that. It wants its own increment, with 0051's
equivalence machinery re-derived deliberately. Recorded in the roadmap's known issues.
