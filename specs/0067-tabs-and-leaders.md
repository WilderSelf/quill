# 0067 — Tab stops and leaders

**Milestone:** M5 · **Status:** implemented (mechanism); the contents-list re-expression is deferred
and named below

## Why

This is what sets a contents entry's page number against the right margin with dots between, a price
list, a bibliography or a specification sheet. Quill has drawn exactly one of those since spec 0041,
and drew it by hand.

## What

### Where the stops live, and why not on the paragraph style

`ParagraphStyle` is `Copy`, and is looked up and copied per block per measurement. Putting a
`Vec<TabStop>` on it would either cost a heap allocation on that path or force the type to stop being
`Copy`, and both are worse than the alternative. A fixed-size array was considered and rejected:
eight stops is both an arbitrary ceiling and ~96 bytes copied per block.

**The stops live in `StyleSheet` as their own map, keyed by paragraph-style name:**

```rust
pub struct StyleSheet {
    pub paragraph: BTreeMap<String, ParagraphStyle>,
    pub tabs: BTreeMap<String, Vec<TabStop>>,      // new
    pub character: BTreeMap<String, CharacterStyle>,
}

pub struct TabStop {
    pub position_pt: Pt,
    pub align: TabAlign,          // Left | Centre | Right | Decimal
    pub leader: Option<Leader>,   // a glyph, and the gap it leaves at each end
}
```

They are resolved beside the paragraph style and passed by reference. `ParagraphStyle` stays `Copy`,
the measurement path allocates nothing new, and a stylesheet remains the one place a treatment is
named.

### The mechanism

`text-layout` gains `lay_tabs(text, stops, measure) -> Vec<TabSegment>`, with its own copies of the
three types for the reason it has its own `RunFormat`: it depends on no other quill crate, and the
mechanism needs numbers rather than a document. `measure` is passed in, so a tabbed line in a
mixed-face paragraph measures each stretch in its own face (spec 0064).

The rules, each asserted to **0.01 pt**:

- **Left** puts the segment's left edge at the stop; **centre** its centre; **right** its right edge.
- **Decimal** puts the *separator* at the stop, so `7.5` and `1234.5` line their points up. What
  counts as the separator is stated — `.` — rather than read from a locale, because a document laid
  out on one machine and exported on another would otherwise put the same number in two places, and
  the second one is a press file. A segment with no separator aligns by its right edge, which is what
  a bare integer in a decimal column wants.
- **Overrun goes to the next stop**, not over the text already there: a stretch that runs past its
  stop takes the next one. A stretch with no stop left butts against what precedes it, so nothing is
  ever dropped.
- **Alignment may not pull a segment back over the text before it.** Right-aligning a wide segment to
  a near stop would give a negative start; the pen wins, because two stretches of text in the same
  place is the one outcome no rule here may produce.
- **A leader fills its gap with a whole number of repetitions**, inset by its own gap at each end, so
  it touches neither side. A partial glyph at the end of a contents entry is the thing a reader
  notices, so a gap with room for none draws none.

### In a document

A paragraph is laid out through the stops when **both** its style names stops and its text contains a
tab. Both conditions, so a document that names stops it never uses lays out exactly as it did, and a
document with no stops is not changed by acquiring a tab character.

A tabbed paragraph is **one line**, and deliberately: a tabbed paragraph is a *row* — a contents
entry, a price line, a bibliography entry, a key and its value. Wrapping one asks a question this
spec does not answer (which stop does a continuation line start at?), and guessing at it is worse
than naming it.

**Justification composes by construction rather than by rule.** A tabbed line is positioned by its
stops and carries no `space_adjust_pt` at all, so the gap in front of a stop cannot be stretched —
stretching it would move text away from the stop it was placed at. Asserted on a justified paragraph.

## Acceptance criteria

- Each alignment is asserted to 0.01 pt: left/centre/right against the stop, decimal against the
  separator's position, with a locale-independent rule for what the separator is.
- A leader fills the gap with a whole number of repetitions, clipped to the gap, and never overlaps
  the text on either side.
- Text that overruns its stop goes to the next stop rather than overlapping.
- Justification and tabs compose: a justified line containing a tab does not stretch the tabbed gap.
- A document that names no stops, or names stops it does not use, is unmoved — `SAMPLE_EXPORT_DIGEST`
  does not move.

## Deferred, and named rather than half-done

**The generated contents list (spec 0041) is not yet re-expressed through this mechanism.** The
roadmap sets that as this increment's acceptance criterion, with the current placed geometry as the
test — byte-identical, or the generalization is wrong. The analysis is done and the answer is *nearly*
yes, which is exactly why it is not being claimed here:

- Every **x position** matches. Laying `"{clipped title}\t{number}"` against one right stop at
  `width - indent`, with a `.` leader of `gap_pt: TOC_LEADER_GAP_PT`, and offsetting by `indent`,
  reproduces the title's origin, the leader's origin, the leader's dot *count* and the number's
  origin exactly — the arithmetic is the same arithmetic.
- Two **widths** do not. `measure_toc` gives the title part `w_pt: title_max` (the column it was
  clipped to, not the text's width) and the leader part `w_pt: leader_end - leader_x` (the gap, not
  the drawn dots). The mechanism reports each segment's measured width. Those fields are placed
  geometry — spec 0050's preflight reads them — so taking the mechanism's values moves the geometry
  the criterion says must not move, and keeping the hand-rolled values means the re-expression is not
  one.
- The ellipsis clipping stays TOC-specific either way. It is not a tab rule; it is the contents
  list's own "an entry does not wrap".

That is a decision about what `w_pt` on a placed part *means* — the text's width or the column's —
and it should be made deliberately, with the preflight consequences looked at, rather than settled by
whichever value happens to keep a digest green. It is its own increment, and it is recorded in
`docs/roadmap.md`'s known issues with this analysis so it starts from here.

## Non-goals

- **A wrapping tabbed paragraph**, for the reason above.
- **Tab stops on a run.** A stop is a property of the paragraph's geometry, not of a stretch of text
  inside it.
- **Default stops every half inch.** Quill has no notion of a default stop, and inventing one would
  give every existing document a tab behaviour it never asked for.
