# CLAUDE.md

This file provides guidance to Claude Code (claude.ai/code) when working with code in this repository.

## What this is

An open-source, cross-platform (Linux/macOS/Windows) **desktop publishing app for
semi-professional hobbyist TTRPG publishers** — art-heavy game books up to ~500 pages that
must export **press-ready PDF/X** for print-on-demand (DriveThruRPG, Lulu, IngramSpark).

**Status: M1 in progress — editing core + text-layout.** M0 (headless PDF/X export) is
code-complete and green — specs 0001–0013 and 0015, indexed in `specs/README.md`. The one
remaining M0 item is manual and non-automatable: a real POD upload (DriveThruRPG/Lulu/
IngramSpark) validated with a B2A-equipped CMYK profile (CI's synthesized ICC has no B2A
tables). The **M1** arc (shaping → Knuth-Plass justification → hyphenation → text frames/threading
→ master pages → incremental layout → perf harness → screen render) is well underway: shaping
(0016), Knuth-Plass justification (0017), hyphenation (0018), text frames/threading (0019),
multi-column threads (0020), and linked-image proxy pixels (0021–0023) have shipped. Next up are
master pages, incremental dependency-tracked layout, the perf harness, and screen render — plus the
`core-model` data-model + `FORMAT_VERSION` work that persisting frames/styles/master pages requires.
The authoritative design is the approved plan at
`~/.claude/plans/i-want-to-create-prancy-bee.md`. Read it before making architectural
decisions. This file summarizes the parts that shape day-to-day work.

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
| `color` | `lcms2`: ICC, RGB→CMYK, grayscale, soft-proof, **ink-coverage (240%) enforcement**. |
| `render` | On-screen viewport (`skia-safe`, GPU) + **linked-image downsampled proxy cache**. |
| `export-pdf` | **The differentiator.** PDF/X writer on `pdf-writer` + `subsetter`; preflight. |
| `components-ttrpg` | Stat blocks, random tables, reusable snippets — portable, first-class objects. |
| `app` | `egui` shell + Skia document canvas. |
| `cli` | Headless render/export; drives M0 and CI. |

### Decisions that are easy to get wrong

- **Do NOT use Skia's built-in PDF backend for export.** It is RGB-oriented and cannot meet
  PDF/X-1a. Screen rendering uses Skia; press export uses the dedicated `export-pdf` writer.
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
TOC) → **M3** pro polish + POD presets → **M4** plugins/ecosystem. Currently at **M1** (M0 code-complete;
sole open M0 item is the manual POD upload).

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
cargo run -p cli -- <args>       # headless render/export (primary M0 entrypoint)
cargo bench                      # perf harness (500-page synthetic doc; gates M1+)
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
  and `plan_path` → the approved plan `~/.claude/plans/i-want-to-create-prancy-bee.md`). `/ship`
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
  `main` is confirmed live (3 required CI contexts, `allow_auto_merge`, admin token — see PR #4).
  Every PR opened here auto-enables `gh pr merge --auto --squash --delete-branch` once that gate
  is verified — no confirmation asked per PR. Re-verify the gate rather than assuming it's still
  live if branch protection could plausibly have changed.
