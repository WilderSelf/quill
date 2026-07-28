# 0047 — Master statics: alignment and page-parity mirroring; `FORMAT_VERSION` 3

**Milestone:** M3 · **Status:** implemented

## Why

A [`MasterStatic::Text`](../crates/core-model/src/lib.rs) (spec 0030,
`crates/core-model/src/lib.rs:588-605`) is stamped as one line drawn from its rect's **left edge**
(`crates/layout-engine/src/lib.rs:383-410`), and the **same rect** is used on rectos and versos
alike. So the two most conventional placements in a bound book are both unexpressible: a running
head cannot be centred, and a folio cannot sit at the outside corner of a spread.

Spec 0036's bundled templates work around it by insetting the folio to the fore-edge margin
(`crates/core-model/src/template.rs:150-175`). That is correct — the number clears the trim on both
halves of the spread — but it is not the design a publisher would choose, and it was arrived at by
*rendering* a template page: every numeric test in the workspace passed while the folio printed hard
against the trim, which is where the guillotine goes. The defect is recorded in `docs/roadmap.md`
("Known issues") and scheduled here.

The fix belongs on the static, not on each template. A template that has to compute an inset because
the model cannot say "outside" is a template encoding a missing model concept, and the third
template does it again.

## What

### `StaticAlign` — where the line sits inside its rect

```rust
pub enum StaticAlign { Left, Center, Right, Inside, Outside }
```

`Left` is the default and is exactly the pre-v3 behaviour, so a static that says nothing is placed
where it always was. `Inside`/`Outside` resolve to left/right **by page parity**, on the same rule
`Margins` has used since spec 0030 (`crates/core-model/src/lib.rs:132-139`): a recto has the spine
on the left, and with `facing_pages` off every page is a recto because a single-sided document has
no spread to mirror across. That rule is now one function, `is_recto`, called by both — the parity
of a margin and the parity of a folio cannot disagree, because they are the same line of code.

Alignment needs the line's width, so it is resolved where widths are known: at layout, against the
same `RunMetrics` the text is broken with (spec 0016). `PageTemplate::statics` therefore takes
`&dyn RunMetrics`. A placed static's frame is narrowed to the measured line, so it reports where the
line **actually** sits rather than the box it was aligned in — which is what spec 0050's geometry
preflight will need to ask.

An over-long line keeps its aligned edge and overflows *inward*, rather than being clamped. A
right-aligned running head that is too wide then runs into the page, not off the trim: visible and
fixable, never silently cut off. This is `CLAUDE.md`'s visible-failure rule applied to furniture.

### `mirror` — the rect itself flips across the spine

Alignment alone is not enough, and the reason is the asymmetry margins already have. The `rulebook`
body master sets `inside: 54, outside: 40`, so the band a folio may sit in runs `x = 54..392` on a
recto and `x = 40..378` on a verso. An `Outside`-aligned static in a fixed rect would be flush right
at 392 on the recto and flush left at 54 on the verso — inside the fore-edge on one half of the
spread and *inset by the gutter margin* on the other.

`mirror: bool` (default `false`) mirrors the rect about the page's vertical centre on a verso:
`x' = trim.w - (x + w)`. The rect is authored as it looks on a **recto**, exactly as `Margins` are
authored inside/outside and resolved per page. With `facing_pages` off nothing mirrors.

Mirroring applies to `MasterStatic::Text` only; see non-goals.

### `FORMAT_VERSION` 3, and the v2 → v3 migration

Both fields are serialized on a spec-0030 type, so this is the first format bump since 0030
(`crates/core-model/src/version.rs:87-110`).

The bump is deliberate even though both fields are `serde(default)` and a v2 manifest therefore
loads unchanged. As in 0030, a version's purpose is to stop an **older** build from opening a
document it would silently mis-lay-out: a pre-0047 build reading a v3 document would ignore `align`
and `mirror` and draw every static left-aligned in an unmirrored rect — putting the folio in the
gutter on every verso, and doing it quietly. Refusing to open is the correct outcome.

Migration is structurally a no-op (a v2 static *meant* left-aligned and unmirrored, which is what
the defaults produce) and, following 0030's precedent, is written as one anyway: `migrate_2_to_3`
walks `master_pages[].statics[]` and defaults `align`/`mirror` on every text static explicitly, so
the chain stays readable as a record of what each version changed.

Both fields are omitted from the manifest when they hold their defaults
(`skip_serializing_if`), on the precedent of `pages` in spec 0035. This is not cosmetic: the
exported PDF's `/ID` is an FNV hash of the manifest text (`crates/export-pdf/src/writer.rs:624-636`),
so a migration that added two keys to every existing document would change the identifier of every
document that has furniture, and every `.tpub` in existence would re-export as a different file.

### The bundled templates stop working around it

`folio()` becomes the text band — `x = inside`, `w = trim.w - inside - outside` — with
`align: Outside, mirror: true`. The page number lands at the fore-edge corner of whichever half of
the spread it is on, one margin clear of the trim, and the templates no longer encode a model
limitation as an inset.

## Acceptance criteria

- [x] `Document::sample()`'s exported PDF is unchanged apart from its document identifier — the
      length, every content stream and every other object byte-identical. The sample has no masters;
      the identifier moves because the manifest text carries `format_version`, exactly as spec 0030
      recorded for the v1 → v2 bump.
- [x] A **committed v2 fixture** (`crates/core-model/assets/v2-masters.json`, bytes, so it cannot
      drift with the code that writes it) loads, migrates to v3, and **lays out to the same placed
      geometry** — asserted against the frames a v2 build produced, not merely "it loads".
- [x] The migrated fixture re-serializes to a manifest identical to the equivalent v3 document's,
      so its `/ID` — and therefore its exported bytes — do not move.
- [x] The whole version chain is asserted: v1 → v2 → v3 all migrate and load; a `FORMAT_VERSION + 1`
      document is still refused with the typed `LoadError::UnsupportedVersion` of spec 0025.
- [x] A centred running head is centred within its rect to 0.01 pt on a 3-page document; a
      right-aligned one is flush right to 0.01 pt.
- [x] An `Outside`-aligned, mirrored folio sits at **two different x values** on two adjacent pages,
      each at the fore-edge corner — the defect this increment exists to fix.
- [x] `Inside`/`Outside` do not mirror when `facing_pages` is off: every page is a recto.
- [x] Bundled templates use real alignment rather than the fore-edge inset, and every bundled folio
      clears the trim by at least the smaller side margin on a recto *and* on a verso.
- [x] `docs/format-spec.md` documents v3 and carries a per-version migration table; its manifest
      example is parsed by a test, so it cannot drift (the spec-0030 anti-drift precedent already
      used for the authoring syntax).
- [x] The roadmap's "A master static has no alignment…" known issue is deleted in this PR.
- [x] A rendered recto and verso show the folios at opposite outside corners, clear of the trim.
- [x] Performance budgets still met (`cargo bench -p quill-testdoc`).

## Test strategy

Migration first, geometry second, render third — in that order, because each one is worthless if the
one before it is wrong.

The migration fixture is committed **as bytes**. A fixture generated by the current serializer would
migrate correctly by construction and prove nothing; the point is that a document written by a build
that no longer exists still lays out the way that build laid it out.

Alignment is unit-tested in `core-model` against `StaticAlign::x_for`, which takes a line width
rather than measuring one — parity resolution is arithmetic and does not need a font to be checked.
The layout tests then assert the placed geometry end-to-end through `MonospaceRunMetrics`, so a
mistake in the wiring between the two cannot hide.

The render is not a formality. Every numeric test passed the last time this was wrong.

## Risks

- **A format bump touches load, save, migrate and every fixture.** The specific hazard is a
  migration that is lossy *in the identity direction*: adding a defaulted key to every static
  changes the manifest text, hence the `/ID`, hence every exported byte, for every document that has
  furniture. `skip_serializing_if` plus a fixture-level assertion that the migrated manifest equals
  the native v3 one is what closes it.
- **Two mechanisms that overlap.** `Outside` alignment and `mirror` both respond to parity, and a
  static that sets one without the other is easy to author by accident. They are separable on
  purpose (a centred head wants neither; a mirrored image band wants only the rect flip), and the
  templates demonstrate the pair that a folio wants.
- **`PageTemplate::statics` gains a parameter.** The trait is public, so an out-of-tree
  implementation breaks. Accepted: the alternative is measuring text in two places (screen and
  press) and hoping they agree, which is the exact drift `quill-fonts` exists to prevent.

## Non-goals

- **Vertical alignment inside the rect.** A static is one line at a fixed baseline offset; a
  top/middle/bottom axis needs the line's ascent and descent to mean anything, and no caller wants
  it yet.
- **Line breaking of master statics.** Unchanged from spec 0030: a static is a single line, and a
  running head that overflows its rect is a visible authoring problem.
- **Mirroring `MasterStatic::Image`.** A decorative fore-edge rule wants it, and the same `Rect`
  helper would serve. Left out so the format bump adds fields to exactly one variant; recorded here
  rather than smuggled in.
- **Justified or optically-adjusted furniture.** `StaticAlign` has no `Justified`: stretching a
  two-character folio across a 338 pt band is never what anyone means.
