# 0053 — User-authored templates: `quill new --from`

**Milestone:** M3 · **Status:** implemented

## Why

`Template::bundled()` says it in the code (crates/core-model/src/template.rs:7-9): templates are
Rust data, and user-authored ones are an M3 follow-up. Spec 0036 took that trade deliberately —
a template directory, a search path and a missing-template policy are three problems the beginner
on-ramp did not need solved to be useful — and named the cost: **a user cannot write their own
template**. A publisher with a house style has to hand-edit a `document.json` per book, which is
exactly the manual, drift-prone step templates exist to remove.

A `Template` is already a serializable bundle of page setup, styles and master pages. The only
thing missing is loading one from a path instead of from the three compiled-in constructors. That
is what this increment adds, and it is the increment that turns 0036 from three starters into an
extensible system.

## What

### The template file is a published format, and is versioned as one

The load-bearing decision. The moment `quill new --from` ships, every template file anyone writes
is a file this build has promised to keep opening — so it gets the same version discipline as
`.tpub` (spec 0025) rather than being an ad-hoc JSON dump of a Rust struct.

```json
{
  "template_version": 1,
  "name": "house-style",
  "title": "House style (6×9)",
  "description": "The one true two-column 6×9.",
  "page_setup": { "trim": {...}, "bleed_pt": 9.0, "facing_pages": true, "margins": {...} },
  "styles": { "paragraph": { "body": {...}, "h1": {...}, "folio": {...} } },
  "master_pages": [ ... ],
  "default_master": "body",
  "pages": [ { "master": "chapter-opener" } ]
}
```

`TEMPLATE_VERSION` is **1**, and it is a *separate integer from* `FORMAT_VERSION`. They version
different artifacts: a template file is not a document, and coupling them would force every
template ever written to be re-versioned whenever the document model changed in a way templates
never see (a new `Block` variant, say).

The gate is spec 0025's, arm for arm — it runs on the untyped `serde_json::Value` before
deserialization, because an older file by definition does not fit the current `serde` types:

| `template_version` | Behavior |
|---|---|
| absent | treated as current; tolerated so a hand-written file can be short |
| `< TEMPLATE_VERSION` | migrated forward through a chain, one arm per version step |
| `== TEMPLATE_VERSION` | loaded as-is |
| `> TEMPLATE_VERSION` | **refused** with `LoadError::UnsupportedTemplateVersion { found, supported }` |

The migration chain is empty today (nothing is older than v1), and is written as
`version::migrate_template` beside `version::migrate` so the second chain cannot be built to a
different shape than the first.

**Migration posture, stated up front.** A template file embeds four document types — `PageSetup`,
`StyleSheet`, `MasterPage`, `PageOverride`. So `TEMPLATE_VERSION` bumps on *either* trigger:

1. the template envelope changes (a field added to or removed from `Template` itself), **or**
2. a `FORMAT_VERSION` bump changes the serialized shape of one of those four embedded types.

Trigger 2 is the one that is easy to miss, and it is why the rule is written here rather than
inferred: spec 0047's `FORMAT_VERSION` 2 → 3 changed `MasterStatic`, which is inside a
`MasterPage`, which is inside every template file with furniture. Had template files existed then,
they would have needed the same migration. A `FORMAT_VERSION` bump that leaves all four alone (a
new `Block` variant, `next_block_id`, anything under `content`) needs no template bump.

### Loading

```rust
pub const TEMPLATE_VERSION: u32 = 1;
impl Template {
    pub fn to_json(&self) -> Result<String, serde_json::Error>;
    pub fn from_json(s: &str) -> Result<Template, LoadError>;
}
```

`LoadError` gains two variants rather than reusing the document ones, because an error has to name
the thing that is wrong: a malformed template file reported as "malformed document manifest" sends
the reader to the wrong file.

- `LoadError::TemplateParse(String)` — not well-formed, or does not match the schema.
- `LoadError::UnsupportedTemplateVersion { found, supported }` — newer than this build.

`Template::from_json` never leaks `serde_json::Error`, on spec 0025's reasoning: the file being
JSON is an implementation detail of the format, and a caller matching on a `serde` error type would
make the encoding impossible to change later.

### `quill new --from`

```
quill new --from house-style.json --output book.tpub
```

`--from` and `--template` are mutually exclusive (`clap` enforces it): one document has one
starting point, and silently preferring one would be the "typo produces the wrong template" failure
spec 0036 already refused. `--list` and `--template <slug>` are **unchanged** — `--from` is purely
additive, and the bundled three remain the answer to "I have no template of my own".

The success line names the template by its own `name` field, whichever way it was found, so a
template file that claims to be `rulebook` says so.

There is **no search path and no template directory.** `--from` takes a path, the shell resolves
it, and a file that is not there is an error naming the path. A search path is a second problem
(precedence between directories, what a bare name means, what happens when two directories both
have `house-style.json`) that this increment does not need to solve to be useful — the same trade
spec 0036 made, one step further along.

### Composition with `--preset` (spec 0049), and the precedence between them

A POD preset carries the printer's geometry — a trim and a bleed. A template carries a *design*.
Both can state a trim, so the precedence has to be stated or the feature becomes confusing.

**The rule, in two halves:**

1. **Trim: the template wins.** A template's masters, margins and furniture are authored *against*
   a specific trim — the bundled templates compute a folio's `y_pt` from the page height and its
   rect width from the trim width, and say so in comments. Re-trimming a template from underneath
   moves every one of those numbers without moving the geometry that was authored for them: the
   folio lands on the last line of text, or past the trim, and nothing at layout time catches it
   because furniture does not participate in the flow. That is the silent press failure `CLAUDE.md`
   forbids, so a preset never re-trims a template.
2. **Bleed: the larger of the two wins.** Bleed is a *floor*, not a design choice — it is the
   distance art must extend past the trim so the guillotine cannot cut white. A preset asking for
   more bleed than the template states is stating a press requirement, and honoring it costs the
   design nothing, because bleed lives entirely outside the trim box. Lowering it is what would
   cost something, so a preset never lowers it either.

A preset whose trim differs from the template's is **reported, not silent** — `Template::disagrees_
on_trim` answers it and the CLI prints which trim the document got. A warning rather than a
refusal, matching spec 0049's own choice of severity for a trim outside a preset's list: an unusual
trim is a conversation with the printer, not a corrupt file.

With **no** template — `quill new --preset lulu` alone — the preset seeds both outright. That case
belongs to spec 0049 and is one of its acceptance criteria.

**What ships here, honestly:** spec 0049 has not landed, so there is no `PodPreset` and no
`--preset` flag yet. This increment ships the seam and the rule — `PageGeometrySeed`,
`Template::seeded_with`, `Template::disagrees_on_trim` — with the precedence asserted at the
library level in both directions. 0049 supplies the flag and the presets that feed it. The seam is
built here because the precedence is a *template* question, and answering it inside 0049 would mean
deciding how a template composes in the increment that is not about templates.

### The style fallback is inherited, not re-decided

A template naming a style that no stylesheet resolves still lays out: `StyleSheet::resolve` falls
back to `body` and then to `ParagraphStyle::default()`, and a master static's `style` falls back to
`ParagraphStyle::default()`. Losing a paragraph's treatment is recoverable and visible; losing the
paragraph is not. A user-authored template is *more* likely to name a missing style than a bundled
one, which is why this is asserted here rather than assumed.

## Acceptance criteria

- [x] Regression: `Document::sample()` export byte-hash unchanged — 8,786 bytes, sha256
      `48ead0fc…`, `cmp`-identical before and after. Nothing this increment touches is serialized
      into a document.
- [x] `quill new --list` still lists the three bundled templates; `quill new --template <slug>`
      still produces the document it did, asserted against `Document::from_template`.
- [x] `quill new --from t.json -o book.tpub` produces a document whose `page_setup`, `styles`,
      `master_pages`, `default_master` and `pages` **each** equal the file's — asserted field by
      field, not by "it produced a file".
- [x] **Round-trip**: every bundled template serializes to a file that loads back to an equal
      `Template`. Table-driven over all three, so a fourth cannot skip it. This is the test that
      proves the format is real rather than write-only.
- [x] A malformed template file fails with `LoadError::TemplateParse`, and one declaring
      `TEMPLATE_VERSION + 1` fails with `LoadError::UnsupportedTemplateVersion { found, supported }`.
      Both asserted; the version case is written relative to the constant so it keeps testing
      "one newer than we understand" across every future bump.
- [x] A file whose JSON is well-formed but whose *schema* is wrong (a missing required field) is
      also a typed `TemplateParse`, not a panic and not a silently defaulted template.
- [x] An absent `template_version` loads as current.
- [x] A template referencing a style nothing resolves still lays out — asserted through the real
      layout engine, on both a content block and a master static, with the text still present.
- [x] Precedence: `seeded_with` keeps the template's trim against a differing seed, raises the
      template's bleed to a larger seed's, and never lowers it to a smaller one. `disagrees_on_trim`
      is true exactly when the trims differ, so the CLI can report rather than swallow it.
- [x] `docs/format-spec.md` documents the template file as a versioned published format, with its
      own version table and the two-trigger bump rule; the doc's own example is **parsed by a test**
      (the anti-drift precedent `import.rs` and `version.rs` already set).
- [x] No new dependency.

## Test strategy

Round-trip first — it is the one that proves the format exists in both directions — then the error
cases, then the precedence. The round-trip and the "every bundled template survives a file" tests
are loops over `Template::bundled()` for spec 0036's reason: adding a fourth template must be
caught by the assertions that exist rather than needing new ones.

The field-by-field acceptance lives in a CLI integration test (`crates/cli/tests/new_from_file.rs`)
rather than a library test, because the claim is about what `quill new --from` *writes*, and the
only way to support that is to run the binary and open the container it produced.

The style-fallback test lives in `layout-engine` rather than `core-model`, for spec 0036's reason:
the criterion protects the author and needs the real engine, not a re-implementation of it.

## Risks

- **A published format on day one.** The whole risk of the increment, and the reason the version
  field and the two-trigger bump rule are in this spec rather than discovered later. The trigger-2
  case (a `FORMAT_VERSION` bump reaching an embedded type) is the one a future increment will walk
  into; it is stated in `docs/format-spec.md` as well as here, next to the document version table
  where someone bumping `FORMAT_VERSION` will actually be reading.
- **Precedence stated but only half-wired.** `--preset` does not exist yet, so the composition is
  asserted at the library level and not end to end. That is recorded above rather than implied, and
  0049 inherits one job: call `seeded_with` and print what `disagrees_on_trim` reports.
- **A user-authored template can be wrong in ways a bundled one cannot** — a folio on top of the
  text, a dangling master name, zero margins. `core-model`'s bundled-template assertions
  deliberately do *not* run over loaded files: spec 0035's posture is that a dangling name degrades
  to the next fallback rather than refusing the document, and refusing to *start a document* over a
  furniture rect would be worse than the misplaced folio. Validating an authored template is a
  preflight concern (specs 0049/0050 add geometry checks over placed content), not a load concern.

## Non-goals

- A template search path, a template directory, or `--template` resolving a user file by bare name.
  `--from` takes a path.
- Writing a template *out* of an existing document (`quill template --from-document book.tpub`).
  `Template::to_json` is public and the round-trip test exercises it, so the mechanism is there;
  the CLI verb is not, because "which of this document's properties are the template" is a design
  question and not a serialization one.
- Any change to `FORMAT_VERSION`, which stays 3. A template file is not a document.
