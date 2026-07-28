# 0036 — Document templates

**Milestone:** M2 · **Status:** implemented

## Why

There is no way to start a document. `AppState` offers `open` and `sample`; the CLI offers
`sample`, `preflight`, `export`, `pack`, `render` and `synth-icc`. A beginner's only starting points
are `Document::sample()`'s two hardcoded blocks or hand-written JSON — and the document they would
get from either has **zero margins on every edge**, because `PageSetup::default()` still describes
the pre-0030 world where the text frame was the whole trim area. Text running to the trim edge is
the first thing a print-on-demand reviewer rejects.

M2's premise is that someone who has never used a page-layout tool ends up with a book that looks
like a book. That starts here: not with a blank page, but with a document that already has margins,
a type scale, a body master and a chapter opener.

## What

### `Template`

Everything a document has *except content*:

```rust
pub struct Template {
    pub name: String,        // slug: `quill new --template <name>`
    pub title: String,       // human label
    pub description: String, // one line, shown by `--list`
    pub page_setup: PageSetup,
    pub styles: StyleSheet,
    pub master_pages: Vec<MasterPage>,
    pub default_master: Option<String>,
    pub pages: Vec<PageOverride>,
}
```

`Document::from_template(&Template)` produces an empty document carrying all of it.
`Template::bundled()` returns the built-in set and `Template::by_name(name)` looks one up.

### The three bundled templates

| name | trim | columns | opener |
|---|---|---|---|
| `adventure` | 6×9 in | 1 | yes |
| `rulebook` | 6×9 in | 2, 14 pt gutter | yes |
| `playtest` | US Letter | 1 | no |

All three carry non-zero margins on all four edges, a `body` master with a `{page}` folio, and a
stylesheet with a `folio` style alongside the built-in `body`/`h1`..`h6`.

The folio's rect is constrained on **both** axes, and the horizontal one was found by rendering the
page rather than by arithmetic. Vertically it sits inside the bottom margin band, below the text
area, because a static does not participate in the flow and nothing at layout time would catch it
landing on the last line. Horizontally it must be *inset*: a `MasterStatic::Text` is drawn as a
single line starting at its rect's left edge — statics carry no alignment and are not mirrored by
page parity — so a full-width rect does not centre the number, it prints it hard against the trim,
where the guillotine goes. Inset by the fore-edge margin (the smaller side) it clears the trim on a
recto and a verso alike without landing inside the text column.

That statics have neither alignment nor parity mirroring is a real limitation of the 0030 model, not
of these templates. It is recorded in the roadmap's known issues; fixing it belongs to the master
static, not to a template working around it. `adventure` and `rulebook` additionally
assign a `chapter-opener` master to page 0 through spec 0035's page list — which is what 0035 was
built for, and the reason it had to land first.

Margins are inside-heavy (the spine margin exceeds the fore-edge) because that is what a bound book
needs and it is the detail a beginner is least likely to know to add.

### Templates are Rust data, not files

No template directory, no path to resolve, no new dependency, nothing to ship alongside the binary.
`Template::bundled()` builds the set once through a `OnceLock` and hands out `&'static [Template]`.

The cost is that a user cannot write their own template, which is a real limitation and an explicit
M3 follow-up. It is the right trade for M2: user-authored templates need a file format, a search
path, and a decision about what happens when a template is missing at open time — three problems the
beginner on-ramp does not need solved to be useful.

### `PageSetup::default()` does not change

This is the increment that answers the roadmap's standing margins question, and the answer is that
the default stays at zero **permanently**. `Document::sample()` is the CI Ghostscript golden fixture
and the export byte-hash guard is derived from it; giving the default margins would move that path
in the same commit as a feature, which is exactly the coupling every M1 increment avoided.

Templates make the question moot rather than answering it: the shipped default still lets text run
to the trim edge, but nobody reaches it by accident any more, because the on-ramp never starts
there. Asserted, so a later increment cannot quietly change it.

### `quill new`

```
quill new --template rulebook --output book.tpub
quill new --list
```

Writes a `.tpub` containing the template's document and no assets. An unknown name exits non-zero
listing the valid ones — a typo must not silently produce a document from the wrong template.

### `AppState::new_from_template`

The app can start a document. This is also the first time anything in the shell handles a document
with **no content**, a path nothing exercised before: every existing app test opens something that
already has blocks.

## Acceptance criteria

- Regression: `Document::sample()` export byte-hash matches the committed constant; the CI
  Ghostscript job stays green; `PageSetup::default().margins == Margins::default()` (all zero),
  asserted, so the golden path provably cannot move.
- Every bundled template round-trips: for each, `Document::from_template(t)` → `to_json` →
  `from_json` is `assert_eq!`-equal. Asserted in a loop over `Template::bundled()`.
- Every bundled template is **press-clean**: `preflight` reports zero `Severity::Error` findings for
  a document built from it. A starter that fails preflight teaches the beginner that the error panel
  is noise.
- Every bundled template has non-zero margins on all four edges, a `body` master, a `default_master`
  that resolves, and a stylesheet containing `body` and `h1`..`h3`. Asserted in the same loop, so a
  fourth template cannot be added without them.
- Every bundled template's masters and page overrides resolve: every name in `pages` and in
  `default_master` exists in `master_pages` (asserted — a template shipping a dangling name would
  silently degrade, and silently is the problem).
- Furniture stays in the margins on both axes: for each template, every `MasterStatic` rect lies
  outside the page's computed text area *and* clears the side margins, so it can neither overlap
  flowed text nor be trimmed off. Asserted by geometry, in two separate tests, because they are two
  different failure modes and only the first was obvious before the page was rendered.
- `adventure` and `rulebook` put `chapter-opener` on page 0 and `body` on pages 1+; laying out a
  from-template document with 3 pages of body text gives page 0 the opener's geometry and pages 1+
  the body's. Asserted to 0.01 pt.
- An empty from-template document lays out to at least one page and paints without panicking.
- `quill new --template rulebook --output <tmp>.tpub` exits 0 and writes a `.tpub` that `Tpub::open`
  reads back to an equal `Document`; `quill new --template nope` exits non-zero and names the valid
  templates; `quill new --list` prints all three.
- `AppState::new_from_template(t)` returns a state with `page_count() >= 1` that paints without
  panicking on empty content.
- No new dependency; `cargo tree -d` reports no new duplicates.

## Test strategy

The loop-over-`bundled()` tests are the design point: adding a fourth template must be caught by the
existing assertions rather than needing new ones. Five of the criteria are written that way
deliberately — margins, styles, master resolution, round-trip and preflight-cleanliness.

Preflight-cleanliness lives in `quill-export-pdf` (the crate that owns preflight) rather than in
`core-model`, because it is the criterion that actually protects the beginner and it needs the real
checker, not a re-implementation.

The empty-document path through layout, paint and export is genuinely untested today, so it gets its
own assertions rather than being assumed to work.

## Risks

- **The empty-content path.** Nothing in the workspace has ever laid out, painted or exported a
  document with zero blocks. Expect at least one place that reasons from `content[0]`.
- **Bundling as Rust data** makes templates non-editable by users. Named as an M3 follow-up above,
  not left implicit.
- **Folio geometry is authored, not derived.** A folio rect is written as literal points against a
  literal margin; change one without the other and furniture lands on top of text, or off the trim.
  The two geometric assertions are the guard, and both criteria are geometric rather than visual
  claims for that reason.

  Worth recording how the horizontal one was found: every numeric test passed while the folio
  printed hard against the trim edge, because the tests asserted the property that had been thought
  about (vertical clearance) and the defect was in the one that had not (a static has no alignment,
  so "centred" was never what a full-width rect meant). It took rendering the page. This is the
  repo's own rule — visual correctness is renderable, so render it — landing on a feature whose
  entire value proposition is that the output looks right without the author doing anything.
