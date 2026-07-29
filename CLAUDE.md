# CLAUDE.md

This file provides guidance to Claude Code (claude.ai/code) when working with code in this repository.

## What this is

An open-source, cross-platform (Linux/macOS/Windows) **desktop publishing application** — long,
art-heavy books up to ~500 pages that must export **press-ready PDF/X** for print-on-demand
(Lulu, IngramSpark, DriveThruRPG).

**Quill is a general-purpose desktop publishing application first, and a TTRPG publishing
application second.** Illustrated game books are the flagship use case — the audience it is designed
for, the corpus its fixtures come from, and the reason its POD presets exist — but every *mechanism*
must be a general one that a game book happens to use, never one only a game book can use. A stat
block is a panelled multi-section record; a random table is a range-lookup table; a rulebook
template is a two-column reference template. **A genre-shaped mechanism is a defect here, not a
feature** — and M4 proved it is the worse mechanism for the genre too: `StatBlock`'s six fixed
fields could not express a game system whose creatures are set differently, and spec 0054's
declarations could. Apply this test to every new type, field, style name and template slug. The
argument and the audit behind it are in `docs/roadmap.md` under "M5 increments".

**Status: M0–M5 complete, through spec 0071.** M0 (headless PDF/X export) is
code-complete and green — specs 0001–0013 and 0015, indexed in `specs/README.md`. The one
remaining M0 item is manual and non-automatable: a real POD upload (DriveThruRPG/Lulu/
IngramSpark) validated with a B2A-equipped CMYK profile (CI's synthesized ICC has no B2A
tables). The **M1** arc (shaping → Knuth-Plass justification → hyphenation → text frames/threading
→ master pages → incremental layout → perf harness → screen render) is well underway: shaping
(0016), Knuth-Plass justification (0017), hyphenation (0018), text frames/threading (0019),
multi-column threads (0020), linked-image proxy pixels (0021–0023), incremental proxy-cache
invalidation (0024), the `.tpub` container and versioned load contract (0025), block identity
(0026), the perf harness (0027), paragraph styles (0028), master pages (0029–0030,
`FORMAT_VERSION` 2), incremental dependency-tracked layout (0031), the shared fonts crate (0032),
screen render (0033) and the app shell (0034) have all shipped. **The M1 arc is complete.** **M2 — the beginner on-ramp — is COMPLETE**: specs 0035–0043 shipped (per-page masters → document templates →
decoration primitive → stat blocks → tables → heading index → generated TOC → PDF outline →
authoring import). A `.md` source now imports to a templated book with stat blocks, tables, a
generated contents list and PDF bookmarks, and exports press-clean. **M3 (pro polish + POD presets) is
COMPLETE** — specs 0044–0053: block fragmentation → table and stat-block continuation →
master-static alignment → hanging indent → POD presets → geometry preflight → line-break pruning →
the screen export profile → user-authored templates. A block now splits across frames, furniture
sits where a bound book puts it, preflight checks the printer's numbers against placed geometry, and
a second export profile carries clickable links while the press file stays provably annotation-free.
`FORMAT_VERSION` is 3. **M4 — the ecosystem — is COMPLETE**: specs 0054–0061 shipped. A component is
now a *declaration* rather than a Rust type, and one interpreter lays any of them out; a `.qpack`
carries templates, styles, definitions and assets with mandatory provenance; a document names the
packs it needs and **refuses to lay out** rather than falling back when one is missing; `quill pack
extract` turns a finished book into a reusable pack. The two defects M3 found are fixed — screen and
press now share one hyphenator as they share one shaper (0059), and no line, ragged or last, is drawn
past its measure (0060) — and the baseline grid finally exists (0058), opt-in so no existing book
moves. `FORMAT_VERSION` stays **3** throughout: every model change is additive.

**M4 deliberately does not build executable plugins**, and that decision is now written where an author
will find it — `docs/pack-authoring.md` states it and why. A pack is *declarative* — templates, styles,
component definitions and assets, no code — because an executable extension that emits geometry can emit
geometry that is wrong, and every mechanism M3 built to make press errors visible assumes quill produced
the geometry. `DefColor` has no RGB family at all, so a pack cannot even express a colour space PDF/X-1a
forbids.

The authoritative sequenced plan — milestones, the M1 increment order (specs 0025–0034), and the
reasoning behind that order — is **`docs/roadmap.md`**, tracked in this repository. Read it before
making architectural decisions. This file holds architecture, constraints and conventions; the
roadmap holds what gets built, in what order, and what "done" means.

## Non-negotiable constraints (these drive every design choice)

- **Press output is the reason the product exists.** Exports must be valid **PDF/X-1a:2001 or
  PDF/X-3:2002**: CMYK color only for color content (no RGB/Lab/spot), grayscale for B&W
  interiors, all fonts embedded/subset, 0.125" bleed on the three non-binding edges, 300 dpi
  images (600 dpi line art), **≤240% total ink coverage**, ICC OutputIntent, no crop marks.
  A preflight step must validate against this spec before export.
- **500 pages, art-heavy, must stay smooth.** The primary competitor (Affinity Publisher) is
  documented to collapse on long docs. Performance is a feature, benchmark-gated in CI.
- **Permissive license (MIT/Apache-2.0 dual).** Every dependency must be permissive-compatible.
  Deliberately avoid GPL-only deps (no Qt; avoid FreeType by using pure-Rust font crates).
- **Hybrid paradigm.** Easy structured-content authoring (Homebrewery-like on-ramp) that flows
  into a real frame/master-page layout engine (InDesign-like ceiling). Both, not either.

## Architecture

Rust workspace, layered as crates so the **PDF/X pipeline is buildable and testable headless
(via `cli`) before any UI exists**. Data flows: `core-model` (document) → `text-layout` +
`layout-engine` (positioned content) → `color` (CMYK/ICC) → `export-pdf` (PDF/X) / `render`
(screen).

| Crate | Responsibility |
|---|---|
| `core-model` | Document tree; open, versioned `.tpub` file format (zip + JSON/TOML manifest + linked `assets/`, `fonts/`). Two linked views: semantic content and layout. |
| `text-layout` | Shaping (`rustybuzz`), **custom Knuth-Plass line breaking** for press-quality justification, hyphenation, bidi. |
| `layout-engine` | Frames, text threading, master pages, layers, baseline grid. **Incremental & dependency-tracked.** |
| `fonts` | Shared shaping, metrics and glyph outlines (`rustybuzz`/`ttf-parser`). One shaper for screen *and* press, so they cannot drift. |
| `color` | `lcms2`: ICC, RGB→CMYK, grayscale, soft-proof, **ink-coverage (240%) enforcement**. |
| `render` | On-screen viewport (backend-neutral paint list → `tiny-skia` CPU raster) + **linked-image downsampled proxy cache**. |
| `export-pdf` | **The differentiator.** PDF/X writer on `pdf-writer` + `subsetter`; preflight. |
| `components-ttrpg` | Stat blocks, random tables — and, since spec 0054, `ComponentDef`: a component *declared* as styled sections in a panel, which is what a `.qpack` ships. The two bundled components are themselves definitions. |
| `app` | `egui` shell + document canvas (paints the `render` crate's op list). |
| `cli` | Headless render/export; drives M0 and CI. |

### Decisions that are easy to get wrong

- **Never render press output through a screen canvas.** Screen backends are RGB-oriented and
  cannot meet PDF/X-1a. Screen rendering and press export are two separate paths that share
  geometry and font metrics but nothing else; press export goes through the `export-pdf` writer.
- **The screen canvas is `tiny-skia`, behind a backend-neutral paint list** (`render` emits
  `Vec<PaintOp>`, then rasterizes). Pure Rust and permissive, with no native build on the
  three-OS CI matrix, and a GPU backend can replace the rasterizer without touching layout.
  See the decisions log in `docs/roadmap.md`.
- **Images are linked, not embedded, with cached downsampled proxies.** Never composite
  full-res on screen — full-res is only touched at export. This is the core perf strategy.
- **Layout is incremental.** Editing one text thread must re-flow only affected pages, never
  the whole document. Baseline-grid snapping is per-frame/local — avoid global grid recompute.
- **Pure-Rust font stack** (`rustybuzz`, `ttf-parser`, `fontdb`) — chosen partly to keep the
  dependency graph permissive (no FreeType/GPL).
- **Mine Typst (Apache-2.0)** for reusable crates (`pdf-writer`, `subsetter`, `ttf-parser`) and
  for its incremental-layout approach.
- **Prefer a visible failure over silent press-corruption.** When you can't be *certain* output
  is press-correct, skip or reject loudly rather than emit possibly-wrong color/geometry. Two
  shapes seen: an input you can't disambiguate (spec 0012 — a CMYK vs YCCK JPEG both decode to
  `CMYK32`, so only the provably-safe Adobe transform-0 case is embedded, the rest skipped), and
  a validator that reads a different field than the writer emits (spec 0013 — preflight must
  validate the *same* `page_setup.bleed_pt` the BleedBox is built from; one source of truth per
  checked property). A dropped image or a preflight error is recoverable; a mis-colored file
  already uploaded to POD is not.

## Milestone order (build the risky/differentiating part first)

**M0** press-output spike (headless PDF/X export, proven with a Ghostscript preflight + a real POD upload) →
**M1** editing core + 500-page performance → **M2** beginner on-ramp (templates, stat blocks,
TOC) → **M3** pro polish + POD presets → **M4** ecosystem (shareable component definitions and
content packs) → **M5** the general typographic core (the neutral core, inline runs, character
styles, lists, tabs) → **M6** the long document (sections and folios, footnotes, cross-references,
an index, a book) → **M7** graphics and colour.
**M0–M5 done**; M0's sole open item is the manual POD upload. **M5 shipped specs 0062–0070**: the
typographic core (0062–0067) plus a three-increment closeout — the writer draws the shaped glyph run
rather than the characters (0068), a placed part reports the ink it draws rather than the slot it was
given (0069), and the contents list is re-expressed through the tab mechanism (0070). They ran in a
hard order: 0069 would have traded preflight false positives for false *negatives* until 0068 made
measured and drawn the same number, and 0070 was not expressible until 0069 settled that `w_pt` is
ink. **Spec 0071 then compressed the content streams** — added by a measurement 0068 took rather
than assumed, and the last stream in the file that was not `FlateDecode`'d: the 500-page synthetic
export is now 1.31 MB against the 13.76 MB it was *before* 0068, and export size is budget-gated in
`benches/budgets.toml` rather than unwatched. **M6 is decomposed into specs 0072–0079** (the section
is the load-bearing gap, and four of its six features are downstream of it). **0075 shipped first and
out of sequence**, because it was the one M6 increment that fixed defects already shipping rather
than adding a feature: a contents list taller than its frame overflowed the page, a composite could
not cut inside a section, and — found while proving the first — a contents list laid out through
`LayoutSession`, the path the app uses, had *always* been empty but for its own title. M7 is named,
not decomposed.

**Why M5 existed: quill could not bold a word.** `Block::Body` and `Block::Heading` each carried one
`String` and one `Color`, so there was no styled run anywhere in the workspace — which is why the
importer refused emphasis on purpose. Spec 0063 gave the paragraph runs and spec 0064 gave the
workspace a font family, four bundled faces, and per-run size, tracking and baseline shift, so a word
can now be bolded and imported as bold. Character styles, drop caps, small caps and OpenType feature
control are the rest of that list, and are what 0065 onward are for. M4 shipped a mechanism for
sharing a look before the look could include an italic.

**Not scheduled, and deliberately so:** the direct-manipulation authoring surface (move, resize,
rotate, group, guides, snap, layers, undo/redo). The `app` crate opens a document, scrolls it and
edits the text of a block; a WYSIWYG object editor is a milestone in its own right and needs the
content model to carry inline formatting, sections and anchored objects first — which is M5–M7.

## Planning: spec-driven development

Non-trivial work starts with a **spec, not code**. Write or update a markdown spec under
`specs/` (what the feature must do, inputs/outputs, acceptance criteria, edge cases) and agree
it before implementing. Code and tests are written to satisfy the spec; the spec is the source
of truth and is revised when behavior changes. `specs/README.md` indexes the specs. Commits
and PRs should reference the spec they implement.

## Commands

**Toolchain (install once — Rust is not preinstalled in this environment):**

```bash
curl --proto '=https' --tlsv1.2 -sSf https://sh.rustup.rs | sh -s -- -y
source "$HOME/.cargo/env"
```

> The cargo commands below are the intended workflow; several only become meaningful once the
> M0 workspace is scaffolded. Standard cargo workspace:

```bash
cargo build                      # build all crates
cargo test                       # run all tests
cargo test -p <crate>            # test one crate, e.g. -p export-pdf
cargo test -p <crate> <name>     # run a single test by name substring
cargo run -p quill-cli -- <args> # headless render/export (primary M0 entrypoint)
cargo run -p quill-cli -- new --list          # built-in document templates (spec 0036)
cargo run -p quill-cli -- new --template reference --output book.tpub
cargo run -p quill-cli -- new --from my-template.json --output book.tpub  # user-authored (0053)
cargo run -p quill-cli -- import doc.md --output book.tpub --template reference
cargo run -p quill-cli -- tpub document.json --output book.tpub   # was `pack`, renamed by 0055
cargo run -p quill-cli -- pack install examples/packs/pbta-moves.json   # content packs (0055-0057)
cargo run -p quill-cli -- pack list
cargo run -p quill-cli -- pack extract book.tpub --name house --version 1.0.0 \
    --source https://example.com --license CC-BY-4.0 --output house.qpack
cargo bench -p quill-testdoc     # perf harness vs benches/budgets.toml; non-zero exit on a blowup
cargo clippy --all-targets       # lint
cargo fmt                        # format
```

## Verifying press output

The acceptance test for any export change is external validation, not just unit tests: a
**Ghostscript** well-formedness gate on generated PDFs (golden-file tests in CI), plus periodic
real test-uploads to DriveThruRPG/Lulu/IngramSpark for certified conformance (no free tool
certifies PDF/X — veraPDF validates PDF/A, not PDF/X). Color code (`color` crate) needs unit
tests on ICC round-trips and ink-coverage math.

**Test fixtures & the dependency graph.** When a test needs a binary fixture (a specific image
format, a font) that only a *generator* dependency can produce, generate it once **out-of-tree**
(a throwaway project in the scratchpad) and commit the artifact — don't add the generator to the
workspace just for tests. Keeps the dependency graph minimal and permissive (e.g. spec 0008's
single-component grayscale JPEG was made with `jpeg-encoder`, which carries an `AND IJG` clause,
without adding it as a dep). If a needed encoder is *already* a dep (like `png`), synthesize the
fixture in-memory in the test instead.

## Automation & learning (Claude Code)

- **`/ship <task>`** — autonomous plan→merge cycle: plan → `feat/<slug>` branch → implement →
  validate (fmt/clippy/build/test, bounded to 5 attempts) → `reviewer` subagent → PR →
  auto-merge deferring to CI. Blocked ⇒ draft PR, never a forced merge. Merge gate = GitHub
  branch protection + CI, not the permission list. Reviewer/planner live in `.claude/agents/`.
- **Workflow kit + profile.** This repo uses the shared user-scope workflow kit; its per-repo
  profile is `.claude/workflow.json` (`validate` commands, `merge_model: pr-gated`, `main_branch`,
  and `plan_path` → the tracked plan `docs/roadmap.md`). `/ship`
  reads it for the validate gate and merge model. **`/advance`** (user-scope) is the Layer-0
  self-driving unit: reconcile → select ONE atomic increment from the approved plan → ship inline
  → wrap tail → exit with a `STATUS:` token. This repo keeps its own `planner`/`reviewer` in
  `.claude/agents/` (domain-specific overrides of the generic user-scope agents).
- **`/reflect`** — after a session or `/ship` cycle, promotes learnings into the right home
  (this file, `.claude/rules/`, a skill, an agent, or a hook), one human-approved change at a
  time. **`/curate`** — dedupe/condense this file (200-line budget), flag contradictions,
  archive stale skills. User-scope config, the permission model, and a disabled reflection Stop
  hook are documented in `~/.claude/settings.reference.md`.
- **`/handoff`** — writes/refreshes the untracked `HANDOFF.md` session-bridge doc for resuming
  work in a fresh session; re-verifies live external state (repo/CI/GitHub settings) rather
  than restating the previous handoff's claims. User-scoped: `~/.claude/skills/handoff/`
  (promoted from project scope so `/wrap` is portable across all repos, not just this one).
- **`/wrap [task]`** — chains `/ship` (if `task` given) → `/reflect` → `/curate` → `/handoff` as
  one invocation instead of four, loading shared config once instead of per-phase. Keeps each
  phase's own approval gates. User-scoped: `~/.claude/skills/wrap/`.
- **Merge behavior — any PR in this repo, not just `/ship`'s**: the branch-protection gate on
  `main` is confirmed live (**4** required CI contexts — all of CI's emitted check-runs, i.e. the
  three `fmt + clippy + test (<os>)` legs plus `PDF preflight (Ghostscript)` — `allow_auto_merge`,
  admin token; see PR #4, 4th context added 2026-07-27). Every PR opened here auto-enables
  `gh pr merge --auto --squash --delete-branch` once that gate is verified — no confirmation asked
  per PR. Re-verify the gate rather than assuming it's still live if branch protection could
  plausibly have changed. **Adding a CI job does not make it a required context** — a new job gates
  nothing until it is added to the required set via
  `gh api -X PATCH repos/:owner/:repo/branches/main/protection/required_status_checks`.
