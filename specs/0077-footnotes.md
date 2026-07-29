# 0077 — Footnotes: a band at the foot of the frame, and the one term it may reduce

**Milestone:** M6 · **Status:** implemented

## Why

A footnote is an anchor in the text flow and note text set at the foot of the frame the anchor lands
in. Quill had neither: no second flow, nowhere for a note to live, and no way for anything but a
`Block` to consume vertical space.

It is the M6 audit's **only item that changes the flow loop** rather than reading its output. Spec
0073 was careful that a folio consumes no flow space; spec 0074 measured that a running head costs
zero passes for the same reason; spec 0076 put text *in* the flow but only ever as characters inside
a paragraph. A footnote takes height off the frame, which is a different kind of change and the
reason this increment is sequenced last among M6's features and after 0075.

## What

### The model: a note is not in `content`

```rust
pub struct Footnote { pub id: BlockId, pub runs: Vec<Run>, pub color: Color,
                      pub style: Option<String> }

pub struct Document { …, pub footnotes: Vec<Footnote> }

pub enum RunSource { Authored, Reference { target: BlockId }, Footnote { note: BlockId } }
```

`footnotes` is a **store, not a second content sequence**: order in the list is not the numbering, so
re-ordering it changes nothing a reader sees. A note nothing anchors is not an error and is not
placed — the posture spec 0072 gave a dangling section anchor. A note shares the one `BlockId` space
with the blocks, because an anchor names it by id, the placed note reports that id as its `source`,
and the resolved-text map is keyed by it; `assign_missing_block_ids` mints and collision-checks
across both, refusing rather than repairing exactly as it does for two blocks.

`RunSource::Footnote` is a **third variant of spec 0076's enum, not a second mechanism**. The anchor
in the flow and the number that opens the note itself carry the *same* variant, so there is one
resolver, one contribution to the font subset, and no way for the two to print different numbers.

### The note is a `Block`, synthesised in one place

`Document::footnote_blocks()` turns each anchored note into the `Block::Body` that is measured and
placed for it: a `RunSource::Footnote` run holding the number, an authored `FOOTNOTE_NUMBER_SUFFIX`
run (`". "`), then the note's own runs, all set in the `footnote` paragraph style.

**One synthesis site, read by both the layout engine and the font-subset collector**, which is the
structural half of this increment's answer to spec 0074's class (see "The font subset" below). It is
also what makes a note split across pages come free: a note is a `Measured::Text`, so spec 0075's
`\vsplit` cuts it exactly as it cuts a paragraph, with the same `MIN_LINES_PER_FRAGMENT` widow rule
and the same absolute-offset invariant. No second splitting mechanism exists.

### Numbering: document-sequential, and that is a decision

**The engine has exactly two dependency shapes today.** `list_markers` is derived *before* the loop
from content; `heading_index_of` is derived *after* it from the final pages. A footnote number that
**restarts per page is neither**: the page is unknown before the flow, and the number's width is part
of the anchor's paragraph, so it moves a line break and cannot be applied after.

This ships **document-sequential decimal**, which escapes that entirely and is exactly
`list_markers`-shaped: one walk over `content`, in order, assigning 1, 2, 3… to each distinct note
the first time an anchor names it. It is known before the first pass, it is constant across the
fixpoint's passes, and a document whose only new feature is footnotes therefore converges in
**one** pass — `layout.fixpoint_iterations` did not move.

**Per-page restart is a named non-goal**, and the cost is worth stating rather than implying. It
would be a fixpoint *inside* `flow` rather than around it: a note's number depends on the page its
anchor lands on, the number's width changes where the anchor's paragraph breaks, and the break
changes the page. The outer `FIXPOINT_MAX_ITERATIONS` loop cannot bound it because the quantity is
consumed mid-pass, so it would need either a second bounded loop per page or a pass structured as
"lay out, renumber, re-lay" — and it would put a page-dependent value in `MeasureKey`, which is one
step from the height dimension the next section forbids. Document-sequential numbering is also
correct typography for a long book; per-page restart is a house style. If it is ever built, the
place it goes is a `FootnoteNumbering` on the document, and `RunSource::contributes` will not compile
until it has been told what the new numbering can draw.

Whatever the scheme, the anchor's rendered number is **derived from position, therefore context and
not content** — spec 0066's `marker` and spec 0076's reference, a third time — so it reuses 0076's
fingerprint mechanism rather than inventing one. `MeasureKey::references` is renamed `generated` and
its doc comment now covers both quantities; `reference_fingerprint` becomes `generated_fingerprint`
and hashes whatever each non-authored run resolves to, through the new `RunSource::referent()`.
`BlockContext::references` becomes `BlockContext::resolved`: **one map**, keyed by the thing a run
points at, holding a cross-referenced block's folio and a footnote's number together. They are
disjoint by construction because a document has one id space, and merging them means one resolver,
one per-block fingerprint and one fixpoint comparison instead of two of each.

### The band reduces `bottom`, and nothing else

**This is the increment, and the constraint is the sharpest in the milestone.**
`Measured::break_items`' doc comment states it: *"A variant whose measurement depends on the available
height must return `None`… A height-dependent measurement that offered break opportunities anyway
would not merely split badly — it would make the measurement cache wrong."*

So the band touches exactly one expression. `crates/layout-engine/src/lib.rs` has three
available-height terms, all derived from a `Copy` of the frame taken once per placement attempt, and
only the first changes:

```rust
let bottom = frame.rect.y_pt + frame.rect.h_pt - trial.height();   // was: no third term
if y + height > bottom { … }                                        // unchanged
measured.cut_fitting(bottom - y)                                    // unchanged
```

`if y + height > bottom` and `cut_fitting(bottom - y)` are untouched *as expressions* and now see a
smaller `bottom`. Nothing else moves. Concretely:

- **`MeasureKey` gains no height dimension.** It gains `generated: u64` (a rename of 0076's
  `references`), and nothing else. A note is measured at a frame **width**, through the same
  `Measurer`, exactly as any other block is; it is cached like one, keyed by its own id and content.
- **Nothing is passed an available height into measurement.** `Band::commit` takes `frame_h`, but it
  spends it on `Measured::cut_fitting` — a *derivation over an already-cached measurement*, which is
  spec 0044's whole design and is the one thing a height is allowed to decide.
- **`benches/budgets.toml`'s `incremental_blocks_measured` still measures 1**, and that is the
  standing guard against this leaking: it is the line spec 0044 wrote specifically to catch "a height
  dimension leaking into `MeasureKey`".

**`continuation_frame` was the trap.** It returns the *next* frame's unreduced height, and
`keep_whole` compares against it — so a composite could decline a cut on the strength of a height a
carried note will already have spent. Fixed by reducing `next_h` by the band the next frame will open
with, computed the same way and skipped entirely when nothing is carried, so no document without a
footnote sees a different number.

**The reservation is made on a copy.** The block only fits if the space is reserved and the space is
only reserved if the block fits; the circularity is broken by `reserve`, which commits onto a clone of
the band and returns it. The clone is kept when the placement is taken and thrown away when it is
not — which is what stops a reservation from leaving a hole at the foot of a frame the block moved
out of. When a block is *cut*, the fit check reserves conservatively against the whole block's notes
and then commits only the notes the **fragment** actually holds, read off the fragment's lines'
spans. That is what makes *"an anchor and its note are on the same page"* true across a cut rather
than usually true: the anchors in the remainder travel with it, and their notes with them.

**A band takes at most three quarters of a frame** (`NOTE_BAND_MAX_FRACTION`), and that is a
correctness rule rather than a taste one: a band allowed to fill the frame leaves the anchor's own
paragraph nowhere to sit, and the flow's only answer to a block that fits nowhere is to place it
anyway and let it overflow — printing the paragraph over its own note. The cap is a *preference*: a
note with no legal cut inside it is cut against the whole frame instead, so progress never depends on
it.

The band is placed into `LaidOutPage::blocks`, not `statics`, because a note is content: it consumes
flow space, it carries the note's id, and `preflight_pages` walks blocks — so a note gets the
safe-area, dpi and ink checks every other placed thing gets for free.

### `FlowState` grows by exactly one field, and working out that it is one is the obligation

`FlowState`'s doc comment says resuming from it is sound *"only because this is genuinely **all** the
state the loop carries: capture the wrong subset and resumed layout silently diverges from a full
pass."*

The accrued band height is **not** state. A checkpoint is only ever taken at a page boundary, where
the first frame is empty and no anchor has been placed — so the accrued height there is always zero
and is re-derived, not carried.

What is not zero is a note too tall for the band it was called into. Its remainder continues at the
foot of the next frame, and a page boundary can fall in the middle of one:

```rust
pub note_carry: Option<NoteCarry>,   // { note: BlockId, split_at: usize }
```

An absolute item offset, exactly as `split_at` is for a block, and interpreted the same way — measured
again at the resumed frame's width and cut again. Two `Copy` fields, so a checkpoint stays numbers
rather than a measured payload.

**Did the session diverge before this was fixed? No — and it was checked rather than assumed.**
Reintroducing `note_carry: None` at the page-advance checkpoint leaves every session parity test
green and fails
`resuming_from_a_checkpoint_inside_a_carried_note_reproduces_a_full_pass`, which is written against
`flow` directly. The session's own tests do not catch it because the session picks its resume point
and does not have to pick one inside a note; the contract is that resuming from *any* checkpoint
reproduces a full pass, so the test is written against the contract rather than against the chooser.
Spec 0072's warning ("assume there is another defect until you have proven otherwise") was taken
seriously and this is the honest result: the defect exists, it is in the flow rather than in the
session, and the session is correct **because** the flow is.

**A note's text is not in `content`, so the per-block diff gains a third term.** Editing a note
changes the band the anchor's frame reserves, and a diff that walked only the blocks would call that
"nothing changed" — spec 0075's defect, at a new site. It goes in the diff and **not** in
`context_fingerprint`, for spec 0076's reason exactly: a changed context sets `dirty_from = Some(0)`,
so editing one note in a book with hundreds would reflow the whole document. It is also deliberately
**not** in `MeasureKey`: a note's text does not change what its anchor's paragraph measures, only
where the paragraph fits, so hashing it into the key would re-break a paragraph that has not moved.

### The fixpoint, and the new way to fail to make progress

**The band cannot oscillate, and the argument is structural rather than empirical.** A footnote
reduces the frame its anchor lands in, which can push the anchor to the next frame, which moves the
note, which gives the first frame its height back — but the flow never goes back with it. The loop's
only two actions are (a) place a fragment, which strictly increases the absolute item offset, and
(b) advance to the next frame or page. Neither can be undone, and a band is only ever committed for a
placement that is *taken*, so a frame the anchor left keeps none of its reservation. The oscillation
is real as a *description* and unreachable as a *state*, because the quantity that would have to
oscillate is discarded rather than carried.

**What can genuinely fail to make progress is the carry**, and it is a new failure that spec 0044's
assertions do not cover: they bound a cut of the *block*, and a note band that grew each time its
note moved would page for ever without any block-level offset failing to advance. Three things bound
it, and all three are asserted rather than reasoned about:

1. `Band::commit`'s cut branch asserts `k > 0`, so a carry that is cut strictly advances its own
   absolute offset — spec 0044's invariant, at the second site that needs it.
2. `open_band` asserts that a frame opening with a carry advances it, so a note is consumed within
   its own item count and cannot occupy an unbounded number of frames.
3. `Band::commit` asserts that a band which overruns its frame **does not also carry forward**. That
   is the third branch — no legal cut fits at all, take the note whole and let it overflow — and it
   is the only way this could fail to terminate. It is the same answer the `frame_empty` guard
   already gives a block too tall for an empty frame, and for the same reason.

Two supporting changes fall out. The empty-frame guard now asks whether the **committed** band leaves
any room, not the prospective one: a frame whose band is already full has nowhere to put the block,
and forcing it in would print the block over the note. Moving on is safe because it is bounded by (2).
And a frame's band may carry **at most one** note forward, and it must be the last one in the band —
without that, two cuts in one frame would leave the second overwriting the first carry and a note
would silently disappear. `reserve` allows a cut only on the last note a placement calls for and only
while no carry exists.

Finally, a carry left at the end of the content is **drained**: a note called near the end of a book
can be taller than the space left for it and there is no block after it to advance the frame, so the
flow adds the frames the note still needs itself. Bounded by (2), which is what makes that loop safe
to write.

The documents that exercise the bound are in the test file:
`a_note_taller_than_a_frame_is_consumed_one_frame_at_a_time` (a note spanning four-plus frames, whose
band pages are asserted to be *consecutive*, so a carry that failed to advance would either hang or
leave a gap) and `two_notes_in_one_paragraph_lose_nothing_between_them` (two notes called by one
paragraph, swept over four lengths, asserting conservation of both).

### The font subset: the same class, a new site

Spec 0074 closed a class — every character a layout-time token can become has to be predicted, because
the collector runs *before* layout and a character it misses is a `.notdef` box in a press file with
no error anywhere. Spec 0076 found a *new path* (a typed field on a body run) and gave it the same
structural treatment rather than a special case.

A footnote is **both halves at once**, and the honest answer is that they are different:

- **The anchor is the same path.** It is a `RunSource` variant, so it rides 0076's exhaustive
  `contributes` unchanged, and adding it made `resolve_run_texts` fail to compile (`E0004`) before a
  single test was written. Nothing new was needed.
- **The note's text is a new site.** It is not in `doc.content`, so a collector that walked the
  content alone would embed no glyph for a word appearing only in a note. The answer is the same
  shape as the other two: the collector walks `doc.content.iter().chain(doc.footnote_blocks().iter())`
  — the *very list the layout engine measures* — so what a note is made of has one definition and not
  two. The number is a `RunSource::Footnote` run and the `". "` separator is an authored run, so both
  are carried by machinery that already existed.

**What a footnote number contributes is `NumberFormat::Decimal.alphabet()`, deliberately not
`folio_formats()`.** Spec 0076's property 4 says there is one answer to what a *folio* can draw; this
is a different question with its own one answer, and tying them together would let roman front matter
change what a footnote can print. Asserted both ways in
`a_footnote_number_contributes_its_own_alphabet_and_not_the_folios`.

**0076 found a latent defect at this site and this increment assumed nothing**, as instructed. The
audit this time turned up none: every site that draws layout-time characters now asks an exhaustive
`contributes`, and the two remaining hand-written alphabets (`'.'`/`'…'` for a contents leader) are
authored constants rather than derived values. The end-to-end half the compiler cannot check — an arm
that returns the *wrong* characters still builds — is covered as 0076 covered it: lay the document
out through the real press path and assert every character a footnote printed, in the flow *and* in
the band, was collected.

## `FORMAT_VERSION`

**8**, and it is `docs/format-spec.md`'s rule read the way spec 0076 refined it — two halves, silence
*and* whether the data is regenerable.

The **silence** half does not fire. A v7 build meeting a v8 document drops `source` as an unknown key
and prints the anchor's stored `text`, which is `[?]`, and drops `footnotes` entirely so the notes
visibly vanish. The page is loudly unfinished exactly where a note was called, which is spec 0074's
condition for the rule *not* firing.

The **loss** half fires harder than it ever has. `footnotes` is not derived state, and it is not even
intent: it is **prose** — the author's own sentences, in a list a v7 build does not know exists — and
one open-and-save deletes every one of them with nothing left in the file to regenerate them from.
0076 bumped for losing which block a reference pointed at; this loses the note.

`migrate_7_to_8` is a structural no-op and writes nothing into the object, for `migrate_6_to_7`'s
reason: inserting `"footnotes": []` into every document in existence would rewrite its manifest text
and move its exported `/ID` with it.

**`TEMPLATE_VERSION` stays 1**, checked rather than assumed. Trigger 2 fires when a `FORMAT_VERSION`
bump changes the serialized shape of `PageSetup`, `StyleSheet`, `MasterPage` or `PageOverride`. A
`Footnote` is in none of them, and a `Run` is in none of them either (a master static's text is a
`String`). `StyleSheet` gains a **default entry** — `footnote`, on spec 0066's precedent for
`list-bullet` — which is a value in an existing map rather than a change of shape; a template file
written before this loads unchanged and a note in a document using it falls through to `body`, which
is the posture a renamed style already has. Trigger 1 does not fire: a template file has no content,
therefore no anchors and no notes.

## Digests

`SAMPLE_EXPORT_DIGEST` moved and is classified **identifier-only**. This one had *two* candidate
causes rather than one, so both are named: `FORMAT_VERSION` became 8, and `StyleSheet::default()`
gained a `footnote` entry. Both are in `doc.to_json()` and neither reaches the page — the sample has
no footnote and no anchor, so `footnotes` is `skip_serializing_if`-omitted, `footnote_blocks()` is
empty, the collector's new walk contributes nothing, no band reserves anything, and a style nobody
names draws nothing.

Measured on the pair of files the ledger always uses — the sample exported against the committed
parity ICC on a build of `main` and on this one: **8454 bytes both sides**, **124 differing bytes in
8 runs**, every run inside the XMP `DocumentID`/`InstanceID` (1510..1541, 1588..1619) or the trailer
`/ID` (8361..8392, 8396..8427). **Zero** differing bytes outside those regions, so no content stream,
font, ICC or metadata stream moved. `component_parity` did not move.

## Acceptance criteria

- [x] **An anchor and its note land on the same page** — asserted over a *sweep* of filler counts
      across the page boundary rather than at one count, with a guard that the sweep really does
      cross it. A single count would test one arrangement of the property rather than the property.
- [x] **The band is reserved, not merely drawn**: no body block's ink reaches into it, over the same
      sweep, and over a document containing a keep-together composite.
- [x] **A long note splits across pages and loses nothing** — conservation against the unsplit line
      list, the assertion spec 0044 wrote first and may never weaken.
- [x] **A note taller than a frame is consumed one frame at a time**, its band pages consecutive.
- [x] **The reference-pushed-to-the-next-frame case**: a paragraph cut across a page boundary with
      its anchor in the *remainder* puts the note on the second page, found through the placed lines'
      spans rather than by looking for the number's characters.
- [x] **Two notes in one paragraph lose nothing between them**, swept over four lengths.
- [x] **Resuming from a checkpoint inside a carried note reproduces a full pass**, for every such
      checkpoint the document records.
- [x] **Session and cold path agree page-for-page across an edit** — over a document with three
      anchors, and again over one whose note spans pages.
- [x] **Editing a note's text reaches the pages, is reported as a change, and reuses pages** —
      proportional to the edit, not to the book.
- [x] **`incremental_blocks_measured` unmoved**: still 1. The cache gained no height dimension.
      `layout.fixpoint_iterations` still 3, `export.sample_bytes` still 8454,
      `export.synthetic_500_page_bytes` still 1,308,263.
- [x] **A document with no footnote lays out and exports exactly as before.** Layout: asserted as
      *equality* against the entry point that carries no notes, plus exactly one fixpoint pass, plus
      no band separator anywhere. Incrementally: editing one paragraph still measures exactly one
      block. Export: `SAMPLE_EXPORT_DIGEST` identifier-only, above.
- [x] **Numbering is by anchor order, not list order**, asserted with the list deliberately reversed.
- [x] **An anchor with no note prints `[?]`** — against the string literal, not the constant — and
      the notes that do exist stay numbered without a hole.
- [x] **The subset case**, on the glyphs actually drawn (`drawn_gids` panics on any `.notdef`): a
      note's own prose spelled in characters nothing else in the document uses, the number, the
      suffix, and the unresolved marker. Plus the end-to-end half.
- [x] Round-trip: a document with a footnote saves and loads equal to itself, including one whose
      anchor names a note that is not there. Both empty cases (spec 0053's lesson): a document with
      no note must not write `footnotes`.
- [x] `FORMAT_VERSION` 8: a **committed v7 fixture** (`crates/core-model/assets/v7-reference.json`,
      bytes) loads, migrates, has no footnote, keeps its cross-reference and its roman folios, and
      re-serializes to a manifest identical to the same document read natively as v8. The whole chain
      v1 → v2 → v4 → v5 → v6 → v7 migrates, and `FORMAT_VERSION + 1` is still refused by name.
- [x] A footnote id collides with a block id ⇒ `DuplicateBlockId`, refused rather than repaired.

## Test strategy

Each behaviour was proved against its own defect by reintroducing it, watching the right tests go
red, and restoring:

| Defect reintroduced | Result |
|---|---|
| `bottom` not reduced by the band — the band is drawn but reserves nothing | **3 tests** fail: `the_note_band_reduces_the_frame_and_nothing_else`, `no_body_block_reaches_into_the_note_band`, `a_keep_together_composite_never_lands_on_top_of_a_band`. Nothing else, which is the point: the anchor and its note are still on the same page, they are simply drawn on top of each other |
| the page-boundary checkpoint records `note_carry: None` | `resuming_from_a_checkpoint_inside_a_carried_note_reproduces_a_full_pass` fails. **Every session test stays green**, which is why the test is written against `flow` and the contract rather than against the session |
| the note-text term dropped from the per-pass diff | `editing_a_notes_text_re_lays_the_document` fails: the session hands back the previous pages with the previous note on them. Spec 0075's shape at a new site |
| notes attributed to the whole block rather than to the fragment placed | `a_note_follows_its_anchor_into_the_frame_the_anchor_lands_in` fails — the note stays on the page the paragraph *started* on while its anchor is on the next |
| the collector does not walk the note blocks | `a_footnote_is_in_the_subset_and_draws_no_notdef` fails; every other test passes |
| the end-of-content carry is dropped instead of drained | **2 tests** fail: the tail of every note called near the end of a document silently vanishes |
| a second cut allowed in one band (`allow_cut = true`) | `two_notes_in_one_paragraph_lose_nothing_between_them` fails — the second carry overwrites the first and a note's tail is lost |

Two notes on what this table does **not** claim. The `continuation_frame`/`keep_whole` correction has
no dedicated witness: reintroducing the unreduced `next_h` leaves the whole suite green, because the
two decisions only diverge when a carry survives into the frame *after* next, and no fixture reaches
that. It is implemented because it is correct — the comparison was against a height that will no
longer exist — and the harmful *outcome* it could produce is covered by
`a_keep_together_composite_never_lands_on_top_of_a_band`. And the "a new `RunSource` variant reaches
the resolver without reaching the collector" case is deliberately not a runtime test, on spec 0074's
precedent: adding `Footnote` failed to compile at `resolve_run_texts` with `E0004` before any test
existed.

The v7 fixture is committed **as bytes** for spec 0047's reason: a fixture the current serializer
wrote would migrate correctly by construction and prove nothing.

## Risks

- **The band cap is a tuned number.** Three quarters is a judgement, and a document whose notes are
  routinely 80% of a page will see them split where a taller cap would not have. The *correctness*
  claim does not rest on the value — progress is bounded by the item count, not by the cap — so a
  change to it is a typographic decision rather than an engine one.
- **A block force-placed into a frame whose own notes it cannot fit beside still overflows.** The
  `frame_empty` guard's existing posture, extended rather than replaced: it is loud, bounded and
  visible on screen, and the alternative is a loop. The cap makes it much harder to reach.
- **A note inside a note is not prevented.** A `Footnote`'s runs may carry a `RunSource::Footnote`,
  and nothing anchors the inner note because the derivation walks `content` — so it is numbered
  nowhere and prints `[?]`. Visible rather than silent, but it is a state the model can hold and the
  engine will not do anything useful with.
- **The reservation is conservative at a cut.** A block cut mid-paragraph reserves against *all* its
  notes and commits only the fragment's, so a frame can end with slightly more room than it needed.
  Conservative in the safe direction — a fragment can end up with more room than was reserved, never
  less — and the alternative is a second circularity between the cut index and the band height.
- **A note called from a tabbed paragraph is attributed to the block rather than to a line.** A
  tabbed paragraph measures as a `Measured::Panel`, which has no lines to read spans off, and is
  indivisible — so it is placed whole and every note it calls belongs to the frame it lands in, which
  is correct for that case. It stops being correct the day a tabbed paragraph can be cut.

## Non-goals

- **Per-page footnote numbering.** Argued at length above: it is a fixpoint inside `flow`, and the
  place it would go is a `FootnoteNumbering` on the document that `RunSource::contributes` will not
  compile without.
- **A footnote area that spans the columns of a multi-column page.** The band is per *frame*, so on a
  two-column page a note sits at the foot of the column its anchor is in. That is a real house style
  and it is the one the engine's geometry expresses; a page-wide area is a different object — it is
  not in any frame — and would need the flow to reserve height in every frame of a page from a
  quantity none of them owns.
- **A superscript anchor by default.** `Run::footnote` sets no `InlineStyle`. A raise and a size
  chosen here would be chosen without knowing the paragraph's, and the mechanism for "this run is set
  differently" is a character style, which spec 0065 already built. A house style names one.
- **Endnotes**, and a per-section or per-chapter note run. Both are placement rules over the same
  model; neither needs a new one.
- **`quill import` syntax.** Markdown's `[^1]` is a plausible spelling, but the importer's
  six-constructs-completely posture says a construct arrives whole or not at all, and a footnote
  needs an anchor *and* a definition block. Every imported run is `Authored`, stated at the site.
- **A `/Link` from an anchor to its note.** The plumbing exists (`link_page` → `PlacedBlock::Link`),
  and it belongs with whoever wants clickable notes in the screen profile — spec 0076 named the same
  non-goal for a cross-reference.
- **Note reference marks other than a number** (`*`, `†`, `‡`). That is a second numbering scheme and
  the enum is where it goes; it will not compile until the collector knows its alphabet.
