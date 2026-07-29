# 0074 — Running heads derived from content

**Milestone:** M6 · **Status:** implemented

## Why

`MasterStatic::Text` resolved `{page}` and nothing else, so a running head naming a chapter had to be
typed onto a master of its own and that master assigned to the chapter's pages by hand. Spec 0072
authored `Section::name` precisely for this and said so in the field's doc comment; spec 0073 was the
first thing a section could be *seen* doing. This is the second, and it is the one an author notices,
because a book's verso says what chapter you are in on every page.

It also carries two corrections the roadmap assigned to it by name, and both are the point of the
increment rather than tidying beside it.

## What

Two tokens beside `{page}`, resolved when the page is laid out:

- **`{section}`** — the `Section::name` of the section the page belongs to: the last section whose
  anchor was placed at or before this page. Empty ahead of every section, because a half-title does
  not belong to chapter one and should not borrow its name.
- **`{heading:N}`** — the text of the last heading of level ≤ `N` at or before this page, from spec
  0040's heading index. `{heading:1}` is "the current chapter".

A page that opens with a heading names *that* heading, not the one it came from. That is a decision
rather than a fallout of `<=`: the alternative is a real house style for a verso, and it is
expressible by authoring the token on one master of a spread rather than by changing the rule.

### The token set, and why adding one to the resolver alone does not compile

**This is the increment's structural claim, and the roadmap's known issue is closed by it rather than
narrowed again.** The font-subset collector runs *before* layout, so every character a token will
become has to be predicted; a character it misses is a `.notdef` box in a press file with no error
anywhere. That is `CLAUDE.md`'s silent-press-corruption class, and this exact function has now been
caught by it twice — PR #107 (a master's static text was never collected at all) and spec 0073 (a
hardcoded `'0'..='9'` against a roman folio). Both were fixed as *instances*.

`quill_core_model::StaticToken` is the class fix: one enum, one parser, two exhaustive matches.

```rust
pub enum StaticToken { Page, Section, Heading { level: u8 } }

impl StaticToken {
    pub fn scan(text: &str) -> Vec<(Range<usize>, StaticToken)>;   // the only parser
    pub fn contributes(&self, doc: &Document) -> TokenText;        // the collector's question
}
```

Three properties, and the enforcement is the compiler:

1. **`scan` is the only way to find a token in a string**, and both `resolve_static_text` (layout) and
   `collect_doc_faces` (export) call it. A spelling the parser does not know is resolved by nobody, so
   the two cannot disagree about what a token *is*. Neither carries a copy of a spelling — spec 0073
   had to fix a literal `"{page}"` at this same site that would have broken digit embedding silently
   on a rename.
2. **`contributes` is an exhaustive `match`.** A fourth variant does not compile until it states what
   characters it can draw. It is in `core-model` beside the enum, not in the exporter, so the variant
   cannot exist anywhere in the workspace without it.
3. **The resolver's `match` is exhaustive too**, so the variant does not compile there either.

**No runtime test can fail "a token was added to the resolver alone", because that state does not
compile** — the variant is what both matches are over, and neither is a lookup table with a default
arm. Verified by adding a fourth variant locally: `cargo build -p quill-core-model` fails at
`contributes` (`non-exhaustive patterns`) before the resolver is even reached.

What the compiler cannot check is whether a written arm is *right* — an arm returning nothing would
build. `every_character_a_master_static_prints_is_in_the_subset` closes that half end to end: it lays a
document out through the real press path and asserts every character any static actually printed was
collected. That test needs its fixture extended when a token is added, and the omission costs coverage
rather than correctness, which is the honest split.

What each token contributes is asked of the **document**, which is what keeps this free where the
feature is unused:

| Token | Contribution |
|---|---|
| `Page` | Each configured `NumberFormat`'s alphabet (`Document::folio_formats`, spec 0073) — loose characters, because what a folio says is arithmetic. |
| `Section` | Every `Section::name`, as **runs** — authored text, knowable exactly, so its ligatures are cut into the subset like any other run's (spec 0068). |
| `Heading { level }` | Every heading of level ≤ `level`, as runs. A deeper level really is a superset, so the level narrows what is carried rather than being decorative. |

A book with no sections and no `{heading:N}` therefore carries exactly the digits it always did.
`SAMPLE_EXPORT_DIGEST` did not move.

### The content channel: a post-pass, not a parameter on the flow

`PageTemplate::statics` had no channel for content, and the two candidates were "grow the trait method
a parameter" and "move statics to a post-pass". **Both, and the ordering is what matters.** The method
grows a `&StaticContext` — mirroring `BlockContext::headings`, the same quantity one layer down — and
it is called from exactly one place: `place_statics`, once per page, *after* the flow.

Resolving during the flow was the option that looked cheaper and is not. The heading index is derived
from the laid-out pages, so a static resolved while the page is being built can only read the
*previous* iterate's index — which is stale by exactly the amount the pass just changed, and
correcting it is another whole-document pass. Resolving afterwards is sound because furniture consumes
no flow space and cannot move a line break (the M6 audit's finding), so it costs **zero** further
passes. That is the honest form of "not a fixpoint", and spec 0073's correction is respected: a
*section* still costs its one resolving pass, because a section's start page changes page geometry.
A running head adds nothing to it. Measured, not assumed — see the acceptance criteria.

`place_statics` also takes `PageTemplate::section_name(page_index)`, which is `PageTemplate::folio`'s
shape for `PageTemplate::folio`'s reason: the section start pages are derived by `reassign` and live
on the template, so the template is the only thing that can answer. It reads the same `starts` the
master assignment and the folio runs come from, so a running head, a chapter opener's master and a
roman folio cannot disagree about where a section begins.

The one cost this design has is that the heading index must be derived from the final page vector, and
that is a walk over every placed block. `PageTemplate::statics_read_headings()` — itself derived from
`StaticToken::scan` over the master's statics, so it cannot disagree with the resolver — skips it for
every document that uses no heading token.

### Tail-page reuse was unsound, and it really was broken

`LayoutSession::pass` reuses whole tail pages verbatim once the flow re-converges, re-asserting
nothing but `page.index`. That is correct exactly while a static is a pure function of the page
number, and `LaidOutPage::statics`' doc comment said so in as many words: statics can be left alone
because they "do not depend on where the text happened to break". **That sentence became false the day
this shipped, and it is deleted rather than softened.**

The defect was not theoretical. Rename chapter one in a two-chapter document — same id, same one-line
height, so nothing repaginates — and the flow re-converges at page 1, which is then reused with the
*old* chapter title in its running head. Reintroduced and watched fail before the fix was trusted:
`page 1 · left: "The Ruined Keep" · right: "The Sunken Vault"`. It is spec 0075's shape (a derived
thing outliving the context it was derived from, silently) at a site 0075 did not reach.

The fix is the post-pass itself rather than a guard on it: `place_statics` runs over **every** page the
session emits — kept, reflowed and reused-tail alike — so a stale running head is unrepresentable
rather than defended against. The narrow alternative (recompute only where a heading above the page
moved) is every page below an edit anyway, so it is the broad version plus a way to get it wrong. It
costs one measured line per static per page and re-measures no block:
`incremental_blocks_measured` is still 1.

## `FORMAT_VERSION`

**Stays 6**, and this is stated rather than passed over.

`docs/format-spec.md`'s rule is whether an older build would open the document and *silently* lay it
out wrongly. A v6 build meeting `{section}` does not recognise it and prints the literal text
`{section}` on the page. That is wrong output — and it is **loud**: visible on screen, visible in the
press file, visible in a proof. The rule turns on silence, and the loudness is not an accident of this
implementation but the documented posture of the parser: a `{…}` group spelling no token is printed as
written, the same fallback a dangling master name and a dangling style name already have.

The second half of the argument is that nothing is added to the model at all. `{section}` is characters
inside a `text` string a user can type today, in a build that predates this spec — so no version gate
could stop them arriving, and a bump would state a compatibility fact that is not true.

`TEMPLATE_VERSION` stays **1**, and the check is not a formality: a template file *does* carry master
pages and therefore *can* carry these tokens. Neither trigger fires — the template envelope gains no
field, and trigger 2 needs a `format_version` bump changing one of the four embedded structures, which
there is not. What a token in a template resolves to is decided by the document it is used for, since a
template has no sections and no content; that is now stated in `docs/format-spec.md`.

## Acceptance criteria

- [x] **A running head naming the current chapter changes at the chapter boundary**, asserted on the
      **placed static text**. Both sides asserted, on spec 0072's reasoning: a test checking only the
      second chapter's pages would pass against an implementation that printed the document's last
      heading everywhere, and one checking only the first would pass against one that never updated.
- [x] **`{section}` and `{heading:1}` on a document where they disagree.** The section is anchored to
      a *body* block a third of the way into chapter one, so its start page is neither the chapter's
      nor the sub-heading's, and the test asserts a page exists that prints two different strings —
      so it is a statement about the two tokens rather than a formula checked against itself. The
      no-section edge (`{section}` empty ahead of every section) and the level edge (`{heading:2}`
      following a sub-heading `{heading:1}` ignores) ride the same fixture.
- [x] **The session tail-page reuse test**, with `pages_reused > 0` asserted so a fixture that quietly
      stopped reusing pages could not pass it, and with the session's pages asserted equal to a cold
      pass's — furniture included.
- [x] **A section name and a heading text drawn in characters no content block uses reach the subset
      and draw no `.notdef`**, asserted on the glyphs actually drawn (`drawn_gids` panics on any
      `.notdef`), following `a_roman_folio_is_in_the_subset_and_draws_no_notdef`.
- [x] **A document using no new token lays out and exports exactly as before.** Layout: every static
      resolves to its text with `{page}` replaced by the page's folio and nothing else — asserted
      against the rule that shipped rather than against a golden captured afterwards, over four
      fixtures including one containing an unknown `{…}` group. Export: `SAMPLE_EXPORT_DIGEST` did not
      move, and `export.sample_bytes` is still 8454.
- [x] **The fixpoint cost, measured.** A document with a `{heading:1}` running head and no sections
      converges in exactly **1** pass — the same as a document with no tokens at all — and adding a
      section makes it exactly **2**, which is the one resolving pass spec 0073 already charged. The
      running head adds nothing.
- [x] `benches/budgets.toml`: every entry within budget, `incremental_blocks_measured` still 1,
      `export.sample_bytes` still 8454.

## Test strategy

Each of the two defects was proved against its own reintroduction, and neither claim is made on
reasoning alone:

| Defect reintroduced | Result |
|---|---|
| `place_statics` runs only over the freshly flowed pages, so a reused tail page keeps its own statics — the pre-0074 semantics exactly | `a_reused_tail_page_carries_the_current_chapters_running_head` fails: `page 1 · left "The Ruined Keep", right "The Sunken Vault"`. Nothing else fails, which is the point: the whole existing suite was blind to it |
| the collector contributes for `Page` only, i.e. spec 0073's fix without spec 0074's | `a_section_name_and_a_chapter_title_are_in_the_subset_and_draw_no_notdef` fails on `'Æ'`; every other test passes |

Both restored, and the full suite is green after.

The "token added to the resolver alone" case is deliberately **not** a runtime test, because it is not
a runtime state: adding a variant fails to compile at `StaticToken::contributes` before any resolver
sees it. Checked by doing it. The residual — an arm that compiles but contributes the wrong text — is
covered end to end instead, by comparing what the resolver printed on the page against what the
collector carried.

## Risks

- **The end-to-end subset test needs its fixture extended when a token is added.** Named rather than
  hidden: the compiler covers the structural half and this covers the semantic half, and only the
  second can be forgotten. Forgetting it loses coverage, not correctness.
- **`place_statics` re-resolves furniture for every page of every incremental pass**, including pages
  the flow never touched. That is one measured line per static per page — no block re-measurement, so
  `incremental_blocks_measured` is unmoved — and it is what makes the reuse defect unrepresentable.
  A 500-page book with two statics per page is a thousand short measurements beside a pass that
  measures none of its blocks.
- **`{section}` on a document whose sections have not converged** prints the last iterate's answer,
  like every other derived quantity in a `converged: false` layout. Surfaced by `FixpointStatus`
  rather than hidden, as spec 0072 established.

## Non-goals

- **A running head cannot carry a chapter title's inline runs.** `HeadingEntry::text` is flattened by
  `Block::plain_text`, so a chapter titled "The *Ruined* Keep" reaches the running head with the
  italic lost. **Named with the residual rather than fixed**, and the reason is that fixing it is not
  local: the heading index would have to carry `Vec<Run>`, which changes what a contents entry
  measures and what the PDF outline reports (an outline title is a `String` by PDF's own definition),
  and `PlacedBlock::Text`'s `run_formats`/`run_shifts` would have to be derived for furniture that is
  measured as one line in one face. The residual is exactly this: **inline emphasis inside a heading
  is dropped in a running head, silently.** It is visible on screen, which is the mitigating half.
- **A running head that differs between recto and verso** — "chapter on the verso, section on the
  recto" is the classic setting. Already expressible: two masters, or one master and page parity,
  which is what spec 0047's `align: outside` and `mirror` are for. Nothing is owed here.
- **A token in body content.** These are furniture tokens. A cross-reference in running text is spec
  0076 and is a different mechanism — it resolves per *instance* rather than per page, which is why
  its fingerprint has to live in `MeasureKey` and not in `context_fingerprint`.
- **`{heading:N}` counting from the *end* of a page** ("the last heading that *starts* on this page,
  else the one before"), which some house styles use for a recto. One rule, and the other is
  authorable as a second master.
