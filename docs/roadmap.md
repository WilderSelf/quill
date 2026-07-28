# Quill roadmap — plan of record

This file is the **authoritative sequenced plan** for Quill. It exists because the design that
`CLAUDE.md` previously pointed at lived outside the repository, in a user-scope plan file, and was
therefore unavailable to any fresh checkout or container. A plan that cannot be read from the repo
cannot govern the repo. Architecture, constraints and conventions stay in `CLAUDE.md`; *what gets
built, in what order, and what "done" means* lives here.

`specs/` remains the source of truth for individual features. This file sequences them and records
why the order is what it is. When an increment ships, its row moves to `implemented` in
`specs/README.md` and the detail section here stays as the historical rationale.

## Milestones

| Milestone | Theme | Status |
|---|---|---|
| **M0** | Press-output spike — headless PDF/X export, Ghostscript-gated | code-complete; one manual item open (a real POD upload validated with a B2A-equipped CMYK profile) |
| **M1** | Editing core + 500-page performance | **complete** — specs 0016–0034 shipped |
| **M2** | Beginner on-ramp — templates, stat blocks, TOC | **complete** — specs 0035–0043 shipped |
| **M3** | Pro polish + POD presets | **complete** — specs 0044–0053 shipped |
| **M4** | Ecosystem — shareable component definitions and content packs | **complete** — specs 0054–0061 shipped |
| **M5** | The general typographic core — the neutral core, inline runs, character styles, lists, tabs | decomposed — specs 0062–0066 sequenced below |
| **M6** | The long document — sections and folios, running heads, footnotes, cross-references, an index, a book | named, not decomposed |
| **M7** | Graphics and colour — image-format breadth, fitting and transforms, anchored objects and runaround, spot colours, vector primitives | named, not decomposed |

**Quill is a general-purpose desktop publishing application first, and a TTRPG publishing
application second.** Game books are the flagship use case; every mechanism must be one a cookbook,
a field guide or a manual could use on the same terms. The argument, and the audit that set M5's
contents, are under "M5 increments" below.

## Decisions log

Decisions that are settled, and would otherwise be re-litigated every time someone reads the code.

- **Canvas backend: `tiny-skia`, behind a backend-neutral paint list.** `CLAUDE.md` originally named
  Skia and `crates/render/Cargo.toml` still said "evaluating vello" — the choice had never actually
  been made. `skia-safe` bundles a heavy native C++ build on every leg of the three-OS CI matrix;
  `tiny-skia` is pure Rust, MIT/Apache, carries the same geometry semantics, and adds no native
  build. Screen rendering emits a backend-neutral `Vec<PaintOp>` first and rasterizes second, so a
  GPU backend (vello/wgpu) can be swapped in later without touching layout. Spec 0033.
- **`BlockId` is a `u64` newtype, not a string.** It is `Copy + Eq + Hash` with no allocation, which
  is what a measurement-cache key wants. It serializes as a plain number, so the text manifest stays
  readable and git-diffable. Spec 0026.
- **A `.tpub` is extracted to a working directory on open.** `asset_root` is then a real filesystem
  path, which is the contract `export-pdf` and `ProxyCache::populate_from_assets` already expect.
  Spec 0025.
- **The perf gate asserts work counters and same-run ratios, never absolute wall-clock.** Shared CI
  runners swing 10–30% and `rust-toolchain.toml` pins the floating `stable` channel, so any absolute
  millisecond ceiling drifts on every Rust release. Counters also state the actual M1 claim —
  "re-flow only affected pages" — far more precisely than a timer. Spec 0027.
- **The bench step folds into an existing CI job rather than adding a new one.** A new job is not
  automatically a required branch-protection context, so a new job would gate nothing. Spec 0027.
- **All 4 of CI's emitted check-runs are required contexts.** This resolves an open question that
  spec 0027 reasoned from and got wrong. CI emits `fmt + clippy + test` on ubuntu/macos/windows plus
  `PDF preflight (Ghostscript)`; until 2026-07-27 only the first three were required, so the
  Ghostscript PDF/X parse check, the `.tpub` export check, the screen-render check, the per-crate
  feature-unification guard and spec 0027's own performance-budget gate were all advisory — a PR
  that broke PDF/X conformance or blew a budget could still auto-merge. 0027 folded the bench step
  into that job *believing it was one of the required three*. `PDF preflight (Ghostscript)` is now
  the 4th required context. It costs no merge latency: it runs 55–109 s while the Windows leg runs
  68–250 s, so it is never the critical path, and the repo is public so Actions minutes are free.
  If the bench step ever flakes on runner noise, split it into its own advisory job rather than
  dropping the whole context — the other four checks in that job are deterministic.
- **A template file is versioned separately from a document, and a preset never re-trims a
  template.** Two decisions from spec 0053, both of which would otherwise be re-argued. (1)
  `TEMPLATE_VERSION` is its own integer, not `FORMAT_VERSION`: a template is not a document, and
  coupling them would re-version every template ever written whenever the document model changed in
  a way templates never see. The cost is a rule to remember, so it is written in two places — a
  template bump is owed either when the template envelope changes *or* when a `FORMAT_VERSION` bump
  changes the serialized shape of `PageSetup`, `StyleSheet`, `MasterPage` or `PageOverride`, the
  four types a template file embeds. Spec 0047's 2 → 3 was exactly the second kind. (2) When a POD
  preset (0049) and a template both state a trim, the **template wins**, because a template's
  furniture is authored *against* its trim — a folio's `y_pt` comes from the page height — and
  re-trimming moves the page without moving the geometry authored for it, silently, since furniture
  does not participate in the flow. Bleed goes the other way: the **larger** wins, because bleed is
  a press floor living entirely outside the trim box, so honoring a stricter one costs the design
  nothing. A disagreeing trim is reported, not refused, matching 0049's own severity choice.
- **Master statics are pre-placed geometry that resolves a style name — not semantic blocks.** This
  answers an M1 open question, by implementation: `MasterStatic::Text { rect, text, color, style }`
  (crates/core-model/src/lib.rs:426-434) positions absolutely against the trim box and resolves
  `style` against the document stylesheet. So a running head is typographically consistent with the
  book, but never enters the flow, never gets a `BlockId`, and never appears in a measurement-cache
  key. Both halves matter: a folio that consumed flow space would shift the text it labels, and
  giving furniture block identity would put non-content into every cache key in spec 0031's session.
- **M2 changes the document model only additively; `FORMAT_VERSION` stays 2.** Five M2 increments add
  serialized shape (per-page master assignment, template provenance, stat blocks, tables, TOC
  blocks). Every one of them is a `#[serde(default)]` field or a new `#[serde(tag = "kind")]` `Block`
  variant whose absence reproduces today's behavior exactly, so no migration is needed and v2 files
  keep loading. This is deliberate: spec 0030's migration is the milestone's one-way door, and there
  is no reason to open a second one for features that default cleanly. An increment that finds it
  *cannot* stay additive must say so in its spec and bump on its own, not smuggle a bump in.
- **Decoration (fills, rules, borders) is press content and goes through the same preflight as
  everything else.** A tinted stat-block panel is ink. The moment `PlacedBlock` can carry a filled
  rectangle, the color preflight at crates/export-pdf/src/lib.rs:195-196 and the 240% ink-coverage
  check stop being complete, because both walk `Block` variants and neither knows about geometry the
  layout engine synthesized. Spec 0035 therefore lands the primitive *and* extends the checks in the
  same increment — a rectangle at 280% total ink is exactly the silent-press-corruption class
  `CLAUDE.md` forbids, and it is invisible to every test that only looks at text and images.
- **M2 is finished when a beginner can make a book without hand-writing JSON.** The milestone's
  three named features (templates, stat blocks, TOC) are the *content* of that claim, but the claim
  itself is the exit criterion, and it is what forces the last two increments: today the only way to
  produce a document is to author `document.json` by hand against `core-model`'s schema and pack it
  with `quill tpub` (renamed from `quill pack` by spec 0055, which reserves `pack` for the `.qpack` content pack). The shell can open and scroll it, edit the
  text of an existing `Heading`/`Body` block (crates/app/src/lib.rs:212-247), and nothing else — no
  block creation, no save, and `document_mut()` (crates/app/src/lib.rs:260-262) is an escape hatch
  called only from tests. A stat block nobody can insert is not an on-ramp.

## M1 increments (all shipped)

Each is independently shippable: it compiles, its tests pass, and it is a coherent PR on its own.
No increment may leave the workspace broken.

| # | Spec | Increment | Size |
|---|---|---|---|
| 1 | 0025 | [`.tpub` zip container + versioned load contract (typed LoadError, reject-newer/migrate-older, document asset root)](#0025-tpub-container-and-load-contract) | medium |
| 2 | 0026 | [Stable `BlockId` on every block, a document revision counter, and O(1) asset lookup](#0026-block-identity-and-revision) | small |
| 3 | 0027 | [Perf harness](#0027-perf-harness) | medium |
| 4 | 0028 | [Persisted paragraph/character styles; layout and the PDF writer honor per-block size and leading](#0028-paragraph-and-character-styles) | medium |
| 5 | 0029 | [Per-page template seam in layout-engine](#0029-page-template-seam) | medium |
| 6 | 0030 | [Persist margins, columns, frames, threads, master pages and the page list; `FORMAT_VERSION` 2 + v1→v2 migration](#0030-persisted-frames-masters-format-v2) | large |
| 7 | 0031 | [Incremental, dependency-tracked layout](#0031-incremental-layout-session) | large |
| 8 | 0032 | [`quill-fonts`](#0032-shared-fonts-and-page-geometry) | medium |
| 9 | 0033 | [Screen render](#0033-screen-render) | large |
| 10 | 0034 | [egui + Skia-family app shell](#0034-egui-app-shell) | large |

## M1 sequencing rationale

The order is forced by three hard dependency chains that the surveys make explicit.

**Chain 1 — the model must settle before anything keys on it.** `Document::from_json` performs no version check at all (crates/core-model/src/lib.rs:190) while docs/format-spec.md:39-42 promises reject-newer/migrate-older. Every increment from 0026 on changes the serialized shape, so the version gate has to exist before the first change or v1 files become undiagnosable. It ships with the `.tpub` container (0025) because the container is what supplies the asset root that both `export-pdf` (hardcoded `Path::new(".")` at writer.rs:61) and `ProxyCache::populate_from_assets` need, and because the container is where every later schema addition lands. Then stable `BlockId`s (0026): incremental layout has literally nothing to key on today — blocks are addressed by index, so an insert at index 0 renumbers everything, and no core-model type derives `Eq`/`Hash` because every geometry field is `f32`. Styles (0028) come next rather than later because they become part of the measurement-cache key; retrofitting a key dimension after the cache hardens is strictly more expensive, and styles are also what stops `measure_block` from discarding `Block::Heading.level` (layout-engine/src/lib.rs:217).

**Chain 2 — the perf harness must precede the work it gates.** `cargo bench` is documented in CLAUDE.md:112 as the M1 gate but is a silent no-op; there is no `benches/` dir, no `[[bench]]`, and zero `[dev-dependencies]` workspace-wide. Landing it at position 3 means the master-page, persistence and incremental increments each have a measuring stick and a re-baseline point, instead of the milestone ending with an unverifiable performance claim. It sits after `BlockId` so the synthetic generator emits ids, and it is deliberately the *third* increment rather than the first because the generator would otherwise be rewritten twice.

**Chain 3 — parity seam, then capability, then persistence.** This is the repo's own house pattern (spec 0016 incr. 1, 0018 incr. 1, 0019 incr. 1, 0020: introduce the structural seam at byte-parity, add behavior next). So master pages split into 0029 (layout-engine-only `PageTemplate` supplier + `LaidOutPage` identity/geometry/statics, proved equal to today's output) and 0030 (the serde types, `FORMAT_VERSION` 2 and the v1→v2 migration that makes masters authorable). 0029 before 0030 keeps the large model change from also carrying an engine redesign. Incremental layout (0031) is after both, because the measurement-cache key must already contain the final set of dimensions (block id, style, frame width, metrics identity) and because per-page checkpointing has to checkpoint the master-aware flow state, not the old single-`Thread` state.

**The render track is last and self-contained.** Screen render is blocked on a fact of crate structure, not on the model: the only real `RunMetrics` is `ShapingContext` inside `export-pdf`'s private `mod fonts`, and `page_geom` is private too, so an app canvas would have to lay out with `MonospaceRunMetrics` and disagree with the exported PDF. 0032 extracts both (a pure refactor guarded by the export byte-hash), 0033 builds the paint list and rasterizer on top, and 0034 — the app shell — is last because it consumes every prior increment: the container to open a document, the model for masters and styles, the `LayoutSession` for responsive editing, `quill-fonts` for WYSIWYG text, and the paint list to draw.

**Two cross-cutting rules shaped the acceptance criteria.** First, every increment carries an explicit regression bullet asserting the `Document::sample()` export stays byte-identical (SHA-256, introduced in 0025) — because `Document::sample()` is the CI Ghostscript golden fixture and `write_pdf` hashes `doc.to_json()` into the PDF `/ID`, so a model change silently moves the golden path. Second, the incremental-layout gate is stated as deterministic work counters (`LayoutStats`, mirroring `PopulateReport`) plus same-run ratios, never absolute wall-clock: measured idle variance here is ±6% on `lay_out`, shared GitHub runners swing 10-30%, and `rust-toolchain.toml` pins the floating `stable` channel so any absolute ms ceiling drifts on every Rust release. Counters also state the actual M1 claim — "re-flow only affected pages" — far more precisely than a timer.

## M1 increment detail

### 0025 tpub-container-and-load-contract

**`.tpub` zip container + versioned load contract (typed LoadError, reject-newer/migrate-older, document asset root)** · size: medium · branch: `feat/tpub-container-and-load-contract`

Every subsequent increment changes the serialized document shape, so the version gate and a single owner of "the document's asset root" must exist first. Add a `.tpub` reader/writer to `quill-core-model` (zip: `document.json` + `assets/` + `fonts/`, per docs/format-spec.md:7-20), replace the bare `serde_json::Error` leaking out of `Document::from_json` (crates/core-model/src/lib.rs:190) with a typed `LoadError`, and implement the reject-newer/migrate-older contract that docs/format-spec.md:39-42 promises but nothing implements. Loading now yields `(Document, asset_root)`, killing the hardcoded `let base_dir = Path::new(".")` at crates/export-pdf/src/writer.rs:61 and giving `ProxyCache::populate_from_assets` (crates/render/src/lib.rs:314) a real base directory. `FORMAT_VERSION` stays 1 and no serialized field changes, so export output is byte-identical.

**Acceptance criteria**

- `Document::from_json(r#"{"format_version":2,...}"#)` returns `Err(LoadError::UnsupportedVersion { found: 2, supported: 1 })`; a v0/absent version migrates forward through a `migrate()` chain that is identity today, asserted by unit test.
- Malformed JSON returns `LoadError::Parse(..)`; the public signature no longer exposes `serde_json::Error` (asserted by the test binding the error to `LoadError`).
- `Tpub::write(&Document::sample(), path, &[("assets/map1.png", bytes)])` followed by `Tpub::open(path)` returns a `Document` `assert_eq!`-equal to the input and an `asset_root` where `asset_root.join("assets/map1.png")` exists and has the same bytes. Temp dir per repo convention (`std::env::temp_dir()` + `std::process::id()`), cleaned with `let _ = fs::remove_dir_all(..)`.
- Opening a `.tpub` whose `document.json` declares `format_version: 99` fails with `UnsupportedVersion` and extracts no assets (asserted: the target dir is absent/empty).
- `export()` resolves `Asset.path` against a caller-supplied document root instead of `Path::new(".")`; exporting `Document::sample()` from the repo root produces byte-identical PDF bytes to the pre-change build (SHA-256 asserted in a test against a committed constant).
- `quill export doc.tpub` and `quill export document.json` both work; CI's `PDF preflight (Ghostscript)` job is unmodified and green.
- The `zip` dependency is declared once in `Cargo.toml` `[workspace.dependencies]` with `default-features = false` (deflate only), an inline comment justifying its MIT license and feature choice, and `cargo tree -d` reports no new duplicate crates.

**Test strategy** — Inline `#[cfg(test)] mod tests` per repo convention (no `tests/` dirs). Version-gate tests are pure string-in/error-out. Container tests write and re-read a real temp `.tpub`. The byte-identity guard is a SHA-256 of `export()` output on `Document::sample()`, committed as a constant — this is the regression bullet every spec in this repo carries, and it is the tripwire for the base_dir change silently altering which images embed.

**Risks** — Changing asset resolution can silently change whether an image embeds (a dropped image is exactly the failure class CLAUDE.md forbids) — the export byte-hash test is the mitigation. `zip` pulls compression backends; must stay permissive and dedupe-clean. `Document::sample()` must not change, or the CI Ghostscript golden path moves.

### 0026 block-identity-and-revision

**Stable `BlockId` on every block, a document revision counter, and O(1) asset lookup** · size: small · branch: `feat/block-identity-and-revision`

Incremental, dependency-tracked layout has nothing to key on: `Block` (crates/core-model/src/lib.rs:107-120) has no id, so blocks are addressable only by index and an insert at index 0 renumbers everything; `Document` has no revision; and `measure_block` resolves image ids with a linear `assets.iter().find(..)` (crates/layout-engine/src/lib.rs:244) on every candidate frame. Add `BlockId` (a `Copy`+`Eq`+`Hash` newtype — note nothing in core-model derives `Eq` today because every geometry field is `f32`) as a `#[serde(default)]` field on all three `Block` variants, a `Document::next_block_id` allocator, a `Document::revision: u64`, and an id→index asset index. All fields are additive and defaulted, so v1 manifests still load and `FORMAT_VERSION` stays 1.

**Acceptance criteria**

- `Block::id()` returns the id for all three variants (`Heading`/`Body`/`Image`); unit test covers each.
- A manifest with no `id` fields loads and every block receives a unique nonzero id assigned in document order (asserted `1, 2, 3`); serializing that document and re-loading it yields the *same* ids (stability across a save/load round trip).
- A manifest containing two blocks with the same id fails with `LoadError::DuplicateBlockId(id)` — loud failure, not a silent overwrite.
- `Document::new_block_id()` returns 1000 distinct ids none of which collide with an existing block, and `next_block_id` is persisted so ids are never reused after a reload.
- `Document::bump_revision()` (called by every mutating helper added here) strictly increases `revision`; a load preserves the stored revision.
- Layout is unchanged: every existing test in crates/layout-engine/src/lib.rs passes untouched, and an unknown image asset id is still skipped without panicking.
- `measure_block` resolves assets through a prebuilt index rather than `iter().find`; a test with 2,000 assets and 2,000 `Block::Image` blocks lays out and asserts every block placed (the O(n²) version is the thing this makes cheap; the timing claim is deferred to spec 0027's bench).
- Spec records that the exported `/ID` and XMP `DocumentID` change, because crates/export-pdf/src/writer.rs:420-434 hashes `doc.to_json()`; the affected assertion is updated and the Ghostscript CI job stays green.

**Test strategy** — Inline unit tests in core-model for id assignment, uniqueness, stability and duplicate rejection; inline tests in layout-engine proving placement behavior is unchanged (the existing `MonospaceRunMetrics{0.6}` + `NoHyphenator` fixtures are the parity oracle). Adding a field to each `Block` variant breaks the exhaustive matches at export-pdf/src/lib.rs:181-184 (color preflight) and :326-327 (`collect_doc_chars`, which feeds the font subset — missing it renders glyphs as `.notdef`), so a test that exports a document with a non-ASCII character and asserts the glyph is present guards that specific miss.

**Risks** — Touching every `Block` match site; the font-subset charset collector is the silent-failure one. The `/ID` hash change means any test pinning a document id must be updated in the same PR.

### 0027 perf-harness

**Perf harness: deterministic 500-page synthetic document, bench targets, ratio budgets, CI bench gate** · size: medium · branch: `feat/perf-harness`

`cargo bench` is documented in CLAUDE.md:112 as the M1 gate but is a silent no-op — there is no `benches/` dir, no `[[bench]]` target, and zero dev-dependencies workspace-wide. Land the measuring stick *before* the layout work it must gate. Add a workspace-internal `quill-testdoc` crate that generates a seeded synthetic document which measures-to-target ~500 laid-out pages (page count is an output, not a model field — 5,500 paragraphs currently produce 424 pages, so a fixed block count is a drifting workload). Add hand-rolled `std::time::Instant` bench targets (zero dev-deps, matching the workspace's zero-`[dev-dependencies]` convention; criterion's ~30 transitive crates contradict Cargo.toml:13-15's minimal-graph rule) for `lay_out`, `justify_paragraph_hyphenated`, and `ProxyCache::populate_from_assets`. Budgets live in a committed `benches/budgets.toml` as per-page ratios, and the CI gate is a blowup detector, not a micro-regression detector.

**Acceptance criteria**

- `quill_testdoc::synthetic_document(SynthSpec { target_pages: 500, seed, heading_every_n, image_every_n })` returns a `Document` that, laid out with `MonospaceRunMetrics { em_ratio: 0.6 }` + `NoHyphenator` at the default `PageSetup`, produces between 495 and 505 pages — asserted by a unit test, so the generator stays correct when leading/margins later change.
- Same seed ⇒ byte-identical `to_json()` across two calls in one process and across two processes (determinism test).
- `cargo bench` builds, runs, prints ms/page for `lay_out`, and exits 0.
- `cargo test --workspace --all-features` does **not** execute the bench binaries: every `[[bench]]` declares `harness = false, test = false`, asserted by `cargo test --workspace --all-features -- --list` containing no bench-target names.
- `cargo clippy --all-targets --all-features -- -D warnings` is clean *including* the bench targets on ubuntu, macos and windows (clippy compiles `--all-targets`, so a Windows-only lint here would redden a required context).
- `#[test] layout_scales_linearly`: min-of-3 timings for 250-page and 500-page synthetic docs satisfy `t(500)/t(250) < 2.4` (measured baseline is exactly linear at ~0.61 ms/page release, so this is a superlinearity detector robust to ±30% runner noise).
- The CI Linux-only job runs `cargo bench --workspace` in release and fails if any measured value exceeds its `benches/budgets.toml` ceiling by more than 100%; the OS matrix cost is unchanged (bench does not run on macos/windows).
- No new runtime dependency and no `[dev-dependencies]` section is introduced; the only new crate is the workspace-internal `quill-testdoc` (depends on `quill-core-model` only).
- CLAUDE.md's `cargo run -p cli -- <args>` is corrected to `-p quill-cli` (there is no package named `cli`), and the `cargo bench` line now describes what actually exists.

**Test strategy** — Two layers. (1) Correctness of the generator: inline unit tests asserting the page-count target and seed determinism — these run in the normal `cargo test` gate on all three OSes. (2) Performance: `benches/*.rs` with `harness = false, test = false`, invoked only by `cargo bench` in the Linux-only CI job, comparing against `benches/budgets.toml`. Ratio and scaling assertions (not absolute ms) because `rust-toolchain.toml` pins the floating `stable` channel and GitHub runners swing 10-30%; measured idle variance here is ±6% on `lay_out` and ±2% on `export`.

**Risks** — A new CI job is NOT automatically a required branch-protection context — CI emits 4 check-runs while CLAUDE.md:159 says 3 are required, so the required set is already a strict subset. Folding the bench step into the existing Linux-only `PDF preflight (Ghostscript)` job avoids needing an out-of-band admin change, but only if that job is one of the required 3. `Swatinem/rust-cache@v2` does not persist bench baselines, which is why budgets are committed rather than compared run-to-run.

### 0028 paragraph-and-character-styles

**Persisted paragraph/character styles; layout and the PDF writer honor per-block size and leading** · size: medium · branch: `feat/paragraph-and-character-styles`

Font size and leading are crate constants (`BODY_FONT_SIZE_PT = 10.0`, `BODY_LINE_HEIGHT_PT = 12.0`, crates/text-layout/src/lib.rs:45,48), `measure_block` discards `Block::Heading.level` (crates/layout-engine/src/lib.rs:217) so headings render at body size, and `docs/format-spec.md:34` promises a `styles` table that does not exist. Add named paragraph and character styles to `core-model`, a style reference on blocks, and style resolution in `measure_block`. Critically, `PlacedBlock::Text` must carry the resolved `size_pt`/`leading_pt` so crates/export-pdf/src/writer.rs:350-351 stops using module constants for the `Tf` operand and baseline advance — otherwise layout measures at one size and the PDF draws at another. Built-in defaults reproduce today's output exactly, so export stays byte-identical; the capability is proved by direct unit tests. Styles must land before the measurement cache, because they are part of its key.

**Acceptance criteria**

- Regression: exporting `Document::sample()` produces PDF bytes whose SHA-256 matches the spec-0025 constant; CI Ghostscript stays green.
- Authoring `ParagraphStyle { size_pt: 18.0, leading_pt: 22.0, align: Left, space_after_pt: 6.0 }` on a heading produces a `PlacedBlock::Text` with `size_pt == 18.0`, `leading_pt == 22.0`, `frame.h_pt == lines.len() as f32 * 22.0 + 6.0`, and the following block's `frame.y_pt` exactly 6.0 pt lower than without `space_after_pt` — asserted with `MonospaceRunMetrics`.
- `Block::Heading { level: n }` with no explicit style resolves to a built-in `h{n}` style for n in 1..=6, and the six built-ins have strictly decreasing `size_pt` (asserted).
- Exporting the 18 pt heading document yields a content stream containing `18 Tf` and consecutive baselines 22 pt apart (asserted by parsing the decompressed stream, as existing writer tests do).
- A block naming an unknown style resolves to the built-in default and is laid out anyway — no panic, no `Err` (screen/authoring posture; a missing style is recoverable, unlike a mis-sized press export). Asserted.
- Styles round-trip through `document.json` and `.tpub`; a manifest with no `styles` key still loads (additive `#[serde(default)]`), asserted.
- `docs/format-spec.md`'s `styles` example deserializes verbatim into the new serde types (test parses the literal JSON from the doc).

**Test strategy** — Inline unit tests. Layout assertions use `MonospaceRunMetrics { em_ratio: 0.6 }` so heights are hand-computable (6 pt/char at 10 pt), per the existing layout-engine convention. Writer assertions decompress the page content stream and match on `Tf`/`Td` operands, mirroring the existing export-pdf tests. The parity guard is the `Document::sample()` export byte-hash from spec 0025.

**Risks** — The size/leading must flow all the way to the writer or screen and press disagree — the `18 Tf` assertion is the tripwire. Adding style fields to `PlacedBlock::Text` is a breaking change to layout-engine's public enum; all existing tests constructing it must be updated. `quill-testdoc`'s generator must be updated in the same PR or the 500-page target drifts.

### 0029 page-template-seam

**Per-page template seam in layout-engine: page identity, per-page frames, master static content (at parity)** · size: medium · branch: `feat/page-template-seam`

Two concrete things block master pages, both inside `lay_out_in_thread` (crates/layout-engine/src/lib.rs:268-351): the page-advance branch resets `frame_idx = 0` into the *same* `thread.frames` slice (:311-315) so every page is geometrically identical, and `LaidOutPage { blocks }` (:134) has no page index, no geometry, and no slot for non-flowing master-supplied content. Introduce a `PageTemplate` supplier that is asked for page N's frames and page N's static blocks, and grow `LaidOutPage` to carry `index`, its resolved frames, and its statics. This is a layout-engine-only change with no model change and no `FORMAT_VERSION` bump: a `RepeatingTemplate` wrapping today's `Thread` is the parity implementation, and `lay_out_in_thread` becomes a thin wrapper over `lay_out_with_template` — the same delegation-chain-as-parity-proof pattern used at :149/:171/:268. It also lets the PDF writer stop sharing one `PageGeom` across all pages (crates/export-pdf/src/writer.rs:327).

**Acceptance criteria**

- Parity: for every existing layout-engine test fixture, `lay_out_with_template(content, assets, &RepeatingTemplate::from(&thread), ..)` returns pages whose `blocks` are `assert_eq!`-equal to `lay_out_in_thread(content, assets, &thread, ..)` — one explicit whole-value equality test, matching the `assert_eq!(via_frame, via_thread)` precedent at lib.rs:687-703.
- Regression: `Document::sample()` export byte-hash unchanged; Ghostscript CI green.
- A template returning 1 frame for even page indices and 2 frames for odd ones produces `pages[0].frames.len() == 1` and `pages[1].frames.len() == 2`, and the content partition is asserted by count per frame (e.g. 12 lines on page 0; 8 then 4 across page 1's two columns), not by mere presence.
- A template supplying one folio `PlacedBlock::Text` per page seeds it on *every* emitted page including the trailing empty one; the flow cursor is unaffected — a 3-page flow places identical content y-positions with and without statics (asserted), proving statics do not consume flow space.
- `LaidOutPage.index` is 0-based and strictly increasing across the returned `Vec`; `pages[i].frames` equals `template.frames(i)`.
- `writer::write_pdf` derives each page's `PageGeom` from that page's `LaidOutPage` rather than one shared value; the existing recto/verso bleed-asymmetry assertions in crates/export-pdf/src/geom.rs tests remain green.

**Test strategy** — Inline tests with hand-rolled `PageTemplate` stubs (the repo's established way of proving a parameter is genuinely threaded through — see the `HalfStub` metrics stub at layout-engine/src/lib.rs:410-418). The load-bearing test is the whole-`Vec<LaidOutPage>` equality between the old and new entry points; the capability tests use alternating templates and a statics-supplying template. Layout-engine is the natural first crate to split into modules (export-pdf's per-concern module naming is the precedent).

**Risks** — Statics must not participate in the flow cursor or overflow accounting, or content silently shifts. Growing `LaidOutPage` is a breaking public-type change consumed by export-pdf. Per-page `PageGeom` in the writer touches the bleed geometry that spec 0013 exists to keep single-sourced.

### 0030 persisted-frames-masters-format-v2

**Persist margins, columns, frames, threads, master pages and the page list; `FORMAT_VERSION` 2 + v1→v2 migration** · size: large · branch: `feat/persisted-frames-masters-format-v2`

Make authored layout reachable from a `Document`. Today `Frame`/`Thread` are layout-engine-owned, non-serde, derived values (crates/layout-engine/src/lib.rs:24,53) and `lay_out` always builds `Frame::full_page` (:150-163), so multi-column and threaded layout can never reach export and text bleeds to the trim edge; `docs/format-spec.md:31-33` sketches `master_pages` and `spreads[].pages[].{master, frames}` and a repo-wide grep for those identifiers returns zero hits. Add serde `Frame`/`FrameId`/`Thread`, `PageSetup` margins + column count + gutter, `MasterPage { name, frames, statics }`, and a `pages: Vec<Page { master, overrides }>` list; bump `FORMAT_VERSION` to 2 with a documented v1→v2 migration (v1 ⇒ zero margins, one column, one implicit full-trim master), and build the spec-0029 `PageTemplate` from the document's masters. This is the increment both spec 0019:186-192 and spec 0020:84-89 explicitly deferred to.

**Acceptance criteria**

- `FORMAT_VERSION == 2`; a v1 manifest loads through the migration and, laid out, produces a `Vec<LaidOutPage>` `assert_eq!`-equal to what the pre-migration `lay_out` produced for the same document (migration-parity test over `Document::sample()` and two multi-block fixtures).
- A `format_version: 3` manifest is still rejected with `LoadError::UnsupportedVersion` (regression on spec 0025).
- Regression: `Document::sample()` keeps zero margins and one column, so its export byte-hash is unchanged and the Ghostscript CI path does not move.
- Exact geometry: `PageSetup { trim: 432x648, margins: 36 all round, columns: 2, gutter_pt: 12 }` derives two frames of width `(432 - 72 - 12) / 2 = 174` pt at `x = 36` and `x = 222`, `y = 36`, `h = 576` — asserted to 0.01 pt.
- A `columns: 0` or a gutter that drives column width to `<= 0` fails loudly at load with a `LoadError`, not by emitting degenerate geometry (mirroring the `Thread::columns` asserts at layout-engine/src/lib.rs:69-83).
- A document assigning master `chapter-opener` to page 0 and `body` to pages 1.. produces page 0 with the opener's frames and pages 1+ with the body's, asserted on a 3-page synthetic document.
- Master statics render on every page using that master, and a folio static containing a `{page}` token resolves to the 1-based page number (page 3 shows `3`) — asserted across 3 pages.
- With `facing_pages: true`, a mirrored master's recto frames are the verso frames reflected about the spine — asserted by x-coordinate arithmetic.
- `docs/format-spec.md`'s manifest example (`master_pages`, `pages`, `styles`, frames) deserializes verbatim into the serde types, asserted by a test parsing the literal JSON from the doc.
- `quill-testdoc` emits masters, margins and columns; the 500-page target assertion from spec 0027 still holds and `benches/budgets.toml` is re-baselined in the same PR.

**Test strategy** — Migration-parity is the central test: capture `lay_out` output for a v1 fixture, migrate, lay out again, `assert_eq!`. Geometry assertions are exact arithmetic (repo convention: hand-computed in a comment above the assertion, `(a-b).abs() < 0.01`). Master assignment and statics are asserted by partition (frames per page, statics per page), not presence. The `docs/format-spec.md` example is parsed by a test so the doc cannot drift from the schema again.

**Risks** — Largest serialized-shape change in the milestone; who owns `Frame` (core-model owns the serde type and layout-engine borrows it, vs. layout-engine converting) must be decided in the spec, not during implementation. The migration is the one-way door — if v1 files migrate wrong there is no recovery, hence the parity test. Adding margins to `PageSetup::default()` would move the CI golden path, so defaults must stay at zero this increment.

### 0031 incremental-layout-session

**Incremental, dependency-tracked layout: `LayoutSession` with a measurement cache, flow checkpoints, and changed-page reporting** · size: large · branch: `feat/incremental-layout-session`

`lay_out_in_thread` (crates/layout-engine/src/lib.rs:268-351) is a single forward pass from block 0 that re-runs Knuth-Plass for every block on every call (and twice for a block that advances frames, :299-301) — editing one paragraph re-flows all 500 pages. There is zero caching anywhere in the crate. Introduce a `LayoutSession` that owns (a) a measurement cache keyed by `(BlockId, frame width, style, metrics identity, hyphenator identity)` and (b) per-page flow checkpoints of the 4-tuple `(content index, frame_idx, y, frame_empty)` (:283-288) so a relayout restarts at the first affected page and terminates once the checkpoint state re-converges. `RunMetrics` and `Hyphenator` (crates/text-layout/src/lib.rs:95,122) gain an identity token so a font swap provably invalidates the cache — without it a cached layout would silently survive a font change, exactly the silent-press-corruption class CLAUDE.md forbids. `relayout` returns the changed page indices plus deterministic `LayoutStats` counters, mirroring `PopulateReport` (crates/render/src/lib.rs:242) — counters, not wall-clock, are the CI gate signal.

**Acceptance criteria**

- Parity: `LayoutSession::new().relayout(&doc)` returns pages `assert_eq!`-equal to `lay_out(&doc, ..)` for every existing layout-engine fixture and for the 500-page synthetic document.
- Regression: `Document::sample()` export byte-hash unchanged; Ghostscript CI green.
- Local edit: changing the text of one block on page 250 of the 500-page synthetic doc gives `stats.measured == 1`, `stats.pages_reflowed <= 3`, `stats.pages_reused >= 495`, and the returned changed-page set has length <= 3.
- Insert-at-front: inserting a block at index 0 that shifts every page gives `stats.measured == 1` (every other block's measurement is reused, since the cache keys on `BlockId` not index) while `pages_reflowed == total_pages`.
- Changed-page reporting is exact: the returned index set equals the set of pages whose `LaidOutPage` differs from the previous run, computed by brute-force comparison in the test (no over- or under-reporting).
- Font-swap safety: relaying out with a metrics implementation whose `identity()` differs gives `stats.reused == 0`; relaying out with the identical metrics gives `stats.measured == 0`. Asserted both directions.
- Structural invalidation: changing `PageSetup.columns` or a master's frames gives `stats.reused == 0` for affected threads only — an edit in one thread does not dirty an independent thread (asserted with two threads).
- Bench-gated: `relayout` after a single-paragraph edit on the 500-page synthetic doc is at least 20x faster than a cold `lay_out` of the same document (min-of-5, ratio budget in `benches/budgets.toml`, not an absolute ms ceiling).
- Cold-path improvement: the Knuth-Plass DP replaces the whole-`Node` clone at crates/text-layout/src/lib.rs:375 and the `starts` clone at :388 with back-pointer reconstruction, producing byte-identical line breaks on every existing text-layout fixture (asserted) and improving the cold `lay_out` bench by >= 1.5x versus the spec-0027 baseline.

**Test strategy** — Counter assertions are the primary gate: they are exact, deterministic and 100% stable on noisy CI runners, and they state the actual M1 claim ("re-flow only affected pages") far more precisely than a timing budget. Timing appears only as a ratio (incremental vs cold on the same run, same machine, same process). The changed-page set is validated against a brute-force diff rather than trusted. The font-identity test is the press-safety test and must assert both the invalidate and the reuse direction so it cannot pass vacuously.

**Risks** — Cache-key omissions are silent-wrongness bugs (style, alignment, frame width, metrics, hyphenator all belong in the key); the both-directions identity test is the guard. `f32` geometry means frame widths must be compared with an explicit epsilon or bit pattern, not `Eq`. Adding `identity()` to `RunMetrics`/`Hyphenator` is a breaking trait change rippling into export-pdf's `ShapingContext` and `HypherHyphenator`. Termination of the resume pass ("reflow until state matches") needs an explicit bound or a pathological document loops.

### 0032 shared-fonts-and-page-geometry

**`quill-fonts`: public shaping/metrics/outlines, and page geometry promoted out of `export-pdf`'s private modules** · size: medium · branch: `feat/shared-fonts-and-page-geometry`

Screen render and the app shell cannot lay out or draw text: `mod fonts;` and `mod geom;` are private (crates/export-pdf/src/lib.rs:16-17), `EmbeddedFont::program` is a private field (fonts.rs:74), and the only public re-export is `synth_cmyk_profile` (:28). The only alternative is `MonospaceRunMetrics`, which would make on-screen line breaks differ from the exported PDF — a WYSIWYG correctness break. Extract a `quill-fonts` crate exposing font loading, a shaping-backed `RunMetrics` carrying the spec-0031 identity token, `ascent_pt`, glyph outlines (via `ttf-parser` — no FreeType, per CLAUDE.md), and positioned glyph runs; and promote `page_geom`/`PageGeom` to a shared public location so screen and press compute bleed/trim/media from one source (the spec-0013 rule), with the PDF bottom-left `flip` staying inside export-pdf. `export-pdf` becomes a consumer and its output stays byte-identical.

**Acceptance criteria**

- Regression: every existing export-pdf test passes unchanged and `Document::sample()`'s export byte-hash is unchanged (this is the whole point of the extraction being a refactor, not a rewrite).
- `quill_fonts::Font::bundled().metrics().measure_run("AVA Waffle", 10.0)` equals the value the old private `ShapingContext` returned for the same input (committed expected constant) — kerning and ligatures preserved.
- `Font::shape(text, size_pt) -> Vec<PositionedGlyph { gid, x_pt, advance_pt }>` returns glyphs whose summed advances equal `measure_run` of the same text to within 0.001 pt (asserted), so the drawing path and the measuring path cannot diverge.
- `Font::glyph_outline(gid)` returns a non-empty outline for a letterform and `None` for `.notdef`-absent ids; no FreeType, no C dependency — `cargo tree` shows only `ttf-parser`/`rustybuzz`/`subsetter` and `cargo tree -d` still reports a single `ttf-parser`.
- Identity: two `Font`s built from the same bytes have equal `identity()`; from different bytes, different — asserted, and reused by the spec-0031 cache test.
- `page_geom(&PageSetup, page_index) -> PageGeom` is public in the shared location, exposes top-left-origin rects (media/bleed/trim), and `export-pdf`'s private copy is deleted; the existing recto/verso bleed-asymmetry tests move with it and stay green.
- `crates/render` and `crates/app` can depend on `quill-fonts` without depending on `quill-export-pdf` (asserted structurally: `quill-export-pdf` does not appear in either manifest).

**Test strategy** — Pure refactor discipline: the export byte-hash and the 73 existing export-pdf tests are the parity proof. New public-API tests live inline in `quill-fonts` and assert the measure/shape consistency invariant (summed advances == measured width), which is the property screen render will depend on. The bundled `SourceSerif4-Regular.ttf` fixture moves or is shared via `include_bytes!` as today.

**Risks** — The bundled font asset and its SIL OFL license file must move together. Moving `PageGeom` touches bleed geometry — spec 0013 exists because a validator once read a different field than the writer emitted, so the promoted function must be the *only* one and export-pdf's copy must be deleted, not left behind. Screen render is deliberately kept on re-shaping each `Line` through this shared shaper rather than emitting glyph runs from `text-layout`; that keeps this increment medium and is a named follow-up.

### 0033 screen-render

**Screen render: backend-neutral paint list, CPU rasterizer, proxy blitting, CMYK→sRGB preview, `quill render` CLI** · size: large · branch: `feat/screen-render`

`quill-render` today is purely a CPU image-decode + downsample + memoization library — no canvas, no `render_page`, no display list — and nothing in the workspace even depends on it (Cargo.toml:48 is the only reference), so the proxy-cache perf strategy is unproven. The only `LaidOutPage`→drawing traversal is the private, PDF-specific `writer::render_page` (crates/export-pdf/src/writer.rs:326). Add `paint_page(&LaidOutPage, &PageGeom, &Font, &ProxyCache, zoom) -> Vec<PaintOp>` (top-left space, never the PDF flip) plus a `rasterize` backed by `tiny-skia` — MIT/Apache, pure Rust, no native build on the three-OS matrix, no FreeType, and glyph outlines come from `quill-fonts` rather than any canvas text stack. Add `cmyk_to_srgb`/`gray_to_srgb` to `quill-color` (which today only converts RGB→CMYK, crates/color/src/lib.rs:86-166) so authored press colors can be previewed. Add a `quill render` subcommand so screen output is CI-gateable the way export is.

**Acceptance criteria**

- `paint_page` is deterministic: the same inputs twice produce an identical op list (structural equality asserted), stable across ubuntu/macos/windows — the paint list, not pixels, is the golden artifact, because pixel goldens would be flaky across the matrix.
- Baseline agreement: for the same page, the text-op baseline computed by `paint_page` equals `export-pdf`'s baseline (`frame.y_pt + ascent_pt(size) + i * leading`) to within 0.001 pt, computed through one shared helper so the two paths cannot drift.
- A `PlacedBlock::Image` with a cached proxy emits an `Image` op scaled into `frame`; with no cached proxy it emits nothing and never panics (screen posture: a missing proxy is recoverable). Both asserted.
- Full-res is never composited on screen: rasterizing a page whose linked asset is 6000 px wide reads only the <= `PROXY_MAX_EDGE_PX` proxy — asserted via the emitted op's source dimensions and a `PopulateReport { generated: 1, .. }`.
- `quill_color::cmyk_to_srgb(Color::Cmyk { k: 1.0 })` returns a near-black sRGB triple (each channel <= 20) and `Color::Gray { v: 0.5 }` maps to 128 ± 2; `cmyk_to_srgb(naive_rgb_to_cmyk(c))` round-trips within a stated tolerance for 8 sample colors.
- `quill render --page 0 --scale 2 -o /tmp/p0.png doc.tpub` exits 0 and writes a PNG whose dimensions are exactly `trim_pt * 2 / 72 * 72` as specified, which decodes and whose ink coverage is non-zero (the page is not blank).
- CI: the Linux-only job invokes `quill render` on the sample document and fails if it errors or emits a blank page; `cargo test --workspace --all-features` stays green on all three OSes with **no** new system libraries installed.
- Rasterizing 20 pages of the 500-page synthetic doc at 1x stays under the per-page budget recorded in `benches/budgets.toml` (blowup detector).

**Test strategy** — Paint-list assertions (op kind, count, geometry) are the headless-stable golden; rasterization is tested only for coarse invariants (dimensions, non-blankness, a known-color fill landing in a known pixel) to avoid cross-OS AA differences. The baseline-agreement test is the WYSIWYG guard and must call the same helper both paths call. `quill render` in CI gives the screen path the same external gate `export` gets from Ghostscript.

**Risks** — Canvas backend choice is genuinely unmade in the repo ("evaluating vello" is still live text at crates/render/Cargo.toml:9) and CLAUDE.md names Skia; `tiny-skia` is proposed because it is permissive, pure Rust, and adds no native build to the three-OS matrix, with the paint-list seam keeping a GPU backend swappable later. CMYK JPEG proxies still decode to `None` (render/src/lib.rs:147) so such images stay blank on screen — a named follow-up, not fixed here. Anti-aliasing differences across platforms make any pixel golden flaky; do not add one.

### 0034 egui-app-shell

**egui + Skia-family app shell: lib/bin split, document canvas, viewport, incremental repaint** · size: large · branch: `feat/egui-app-shell`

`crates/app/src/main.rs` is a 17-line `println!` stub whose only dependency is `quill-core-model`; there is no window, no event loop, no viewport, nothing for a renderer to draw into. Turn `quill-app` into a library plus a thin binary: the library holds all shell logic (open a `.tpub`, populate the `ProxyCache`, own a `LayoutSession`, viewport scroll/zoom state, page↔screen mapping, preflight display) and depends on `egui`, which runs fully headless via `egui::Context::run(RawInput::default(), ..)` — no window, no GPU, no display server, so every behavior is testable in the existing three-OS `check` job. `eframe`/`winit` are confined to the binary behind a non-default `gui` feature so CI needs no system libraries. The canvas paints spec-0033's op list into an `egui` texture. This is last because it consumes the container, the model, masters, the layout session, the shared fonts, and the renderer.

**Acceptance criteria**

- `quill-app` has a lib target and `cargo test -p quill-app` runs at least 8 unit tests with no display server and no GPU on ubuntu, macos and windows.
- `AppState::open(path)` on a `.tpub` returns a loaded document plus a `PopulateReport`; a `.tpub` with one missing linked asset yields `skipped >= 1` and the app still opens and lays out (screen posture: a broken link must not abort loading a 500-page document).
- Viewport math round-trips: `screen_to_doc(doc_to_screen(page, rect))` is within 0.01 pt of the input at zoom 0.25, 1.0 and 4.0 (asserted for all three).
- Virtualized painting: scrolled to page 499 of the 500-page synthetic document, only the visible page range is painted — a `painted_pages` counter equals the visible count (<= 4), not 500.
- A headless `egui::Context::run(RawInput::default(), ..)` frame drives the whole shell over the 500-page synthetic document and returns a `FullOutput` with a non-empty shape list; asserted in CI.
- Editing a paragraph through the app's edit API routes to `LayoutSession::relayout` and repaints only the returned changed pages: `stats.pages_reflowed <= 3` and the repaint set equals the changed-page set (asserted, reusing spec 0031's counters).
- Preflight findings (`CheckId`/`Severity`/`Finding`, crates/export-pdf/src/lib.rs:85-131) are surfaced by the shell with errors and warnings counted separately; asserted on a document with a known ink-coverage violation.
- `eframe` is behind `required-features = ["gui"]` on the `[[bin]]` and is **not** a default feature; `cargo build --workspace` and `cargo clippy --all-targets --all-features -- -D warnings` stay green on all three OSes. If `--all-features` would enable `gui`, the ubuntu leg gains the `libxkbcommon-dev`/`libwayland-dev`/X11/GL apt step in the same PR and CI is verified green before merge.
- Workspace `rust-version` (Cargo.toml:11 currently declares 1.75, already violated by `u32::is_multiple_of` at export-pdf/src/geom.rs:57) is corrected to the true MSRV and to egui's floor.
- License audit recorded in the spec: no Qt, no FreeType, no GPL in the new dependency subtree, and every added dependency carries the workspace's inline license/justification comment.

**Test strategy** — Everything meaningful is a headless unit test in the lib target: `egui::Context::run` with a default `RawInput` executes real frames and returns tessellated output with no windowing system, so the existing `check` job gains real per-frame assertions at zero CI cost. Viewport math, page virtualization, document open and incremental repaint are pure-logic tests. The binary is a ~10-line `eframe` launcher and is covered only by compilation.

**Risks** — `cargo clippy --all-targets --all-features` will enable a `gui` feature if it exists, dragging `eframe`/`winit` into the three-OS matrix and breaking the ubuntu leg until system libraries are installed — this is the single most likely way this increment reddens required contexts, and the spec must decide between an apt step and dropping `--all-features`. egui's MSRV floor forces the stale `rust-version` fix. Painting must be virtualized from the first commit or the 500-page smoothness constraint is violated by the shell itself.

## M2 increments

M1 built a layout engine an expert can drive from hand-written JSON. M2 is about someone who has
never used a page-layout tool getting a book that looks like a book: **start from a template rather
than a blank page**, drop in the two content objects this product exists for (**stat blocks** and
**random tables**), and get a **table of contents** and PDF bookmarks without laying either out by
hand.

Same rule as M1: each increment compiles, its tests pass, and it is a coherent PR on its own. No
increment may leave the workspace broken, and every one carries the `Document::sample()` export
byte-hash regression bullet — that hash is the CI Ghostscript golden path, so any increment that
moves it has changed press output without meaning to.

| # | Spec | Increment | Size |
|---|---|---|---|
| 1 | 0035 | [Per-page master assignment — chapter openers and front matter](#0035-per-page-master-assignment) | medium |
| 2 | 0036 | [Document templates: bundled starters, `Document::from_template`, `quill new`](#0036-document-templates) | medium |
| 3 | 0037 | [Decoration primitive: fills, rules and borders, with preflight extended to cover them](#0037-decoration-primitive) | medium |
| 4 | 0038 | [`Block::StatBlock` — the composite, keep-together TTRPG component](#0038-stat-block) | large |
| 5 | 0039 | [`Block::Table` — random tables and row/column layout](#0039-tables) | medium |
| 6 | 0040 | [Heading index: which page each heading landed on](#0040-heading-index) | small |
| 7 | 0041 | [`Block::Toc` — generated contents with a bounded stabilization loop](#0041-generated-toc) | large |
| 8 | 0042 | [PDF outline and named destinations, plus the annotation/bleed preflight guard](#0042-pdf-outline) | medium |
| 9 | 0043 | [Markdown-ish import — the actual on-ramp](#0043-markdown-import) | large |

## M2 sequencing rationale

Three dependency chains force the order, and one of them is a cycle that has to be broken
deliberately rather than discovered during implementation.

**Chain 1 — templates are a model capability before they are a preset.** A "template" in the
beginner sense is a document that already has margins, a stylesheet, masters, and a chapter-opener
page. Two of those four do not exist yet. `Margins::default()` is zero on every edge
(crates/core-model/src/lib.rs:95-107) so the shipped default still lets text run to the trim edge,
and `Document.default_master: Option<String>` (crates/core-model/src/lib.rs:509-515) applies **one**
master to **every** page — spec 0030's `pages: Vec<Page>` list was planned and not built, so there
is currently no way to say "page 0 is a chapter opener." A starter template with no chapter opener
is just a margin preset, which is why 0035 precedes 0036. 0036 is also where the roadmap's
long-standing margins question gets answered without moving the CI golden path: templates carry
non-zero margins, `PageSetup::default()` stays at zero, and `Document::sample()` never changes.

**Chain 2 — the drawing primitive, then the component, then the authoring syntax.** A stat block is
a tinted, ruled, padded box containing several differently-styled lines that must not be split
mid-box if it can be helped. The workspace cannot draw any of that: `PlacedBlock` has exactly two
variants, `Text` and `Image` (crates/layout-engine/src/lib.rs:125-142), and `PaintOp` has four,
none of which is a rectangle fill or a stroke (crates/render/src/paint.rs:27-60). So 0037 lands the
primitive at parity — nothing emits it yet, export stays byte-identical — and, per the decisions log
above, extends the color and ink-coverage preflight to see it in the same PR. Only then can 0038
build the component. `quill-components-ttrpg` already defines `StatBlock` and `RandomTable`
(crates/components-ttrpg/src/lib.rs) with real logic and tests, and **nothing in the workspace
depends on that crate** — it is a data model with no layout, no export and no way to put one in a
document. 0038 and 0039 are what connect it. 0043 is last in this chain because a markdown-ish
importer is only worth writing once the things it would import can actually be laid out; writing the
syntax first would mean designing it against types that do not exist.

**Chain 3 — the TOC cycle must be broken by an explicit fixpoint, not by hoping.** (0040 was taken
ahead of 0038/0039 in the build order: the chains are independent by construction, and 0040 is the
milestone's smallest increment while 0038 is its largest, so shipping it first kept a clean boundary
around the stat-block work rather than splitting it.) A table of
contents lists page numbers, its own length changes where every subsequent page break falls, and
that changes the page numbers it lists. Nothing in the engine can express this today: layout is a
single forward pass and `LaidOutPage` carries no mapping back to the blocks that produced it, so
"which page is heading X on" is unanswerable (crates/layout-engine/src/session.rs:69-77 —
`LayoutResult` reports `pages`, `stats` and `changed_pages`, and nothing else). 0040 answers that
question alone, as a small additive change, and is deliberately separate from 0041 for the same
reason spec 0027 separated the measuring stick from the work it gates: if the index and the fixpoint
loop land together and the TOC comes out wrong, neither is trustworthy. 0041 then iterates
layout→resolve→regenerate under an explicit iteration bound and a documented behavior on
non-convergence — spec 0031 already flagged unbounded "reflow until state matches" as the way a
pathological document loops forever, and a TOC is the case that actually oscillates (an entry that
pushes a heading onto the next page, whose new number shortens the entry, which pulls it back).
0042 consumes 0040's index too, but not 0041's loop, so it could ship before the TOC if the fixpoint
proves harder than estimated — that ordering freedom is intentional slack.

**Two cross-cutting rules, both inherited from M1.** First, `Document::sample()`'s export byte-hash
is a bullet in every increment: five M2 increments add `Block` variants or serialized fields, and
the sample is the Ghostscript golden fixture whose `/ID` is a hash of `doc.to_json()`. Second, every
new `Block` variant breaks four exhaustive match sites that the compiler will point at, and one that
it will not care about but which fails silently: `collect_doc_chars`
(crates/export-pdf/src/lib.rs:334-341) feeds the font subset, so a variant whose text it does not
collect exports as `.notdef` boxes rather than as an error. Spec 0026 hit exactly this. Every
variant-adding increment here carries a non-ASCII-glyph export test for that reason.

## M2 increment detail

### 0035 per-page-master-assignment

**Per-page master assignment: a page list, front matter, and chapter openers** · size: medium ·
branch: `feat/per-page-master-assignment`

`Document.default_master` applies one master to the whole book (crates/core-model/src/lib.rs:509-515,
which says in as many words that per-page assignment is a follow-up), and `DocumentTemplate`
(crates/layout-engine/src/lib.rs:211-260) resolves that single master for every page index. So a
document can have consistent furniture or no furniture, and nothing else — no title page without a
folio, no chapter opener with a deeper top margin, no front matter in roman numerals. Add a
`pages: Vec<PageOverride>` list to `Document` where `PageOverride { master: Option<String> }` applies
to a page by index, and make `DocumentTemplate` consult it before falling back to `default_master`.
Additive and `#[serde(default)]`: an absent or empty list is exactly today's behavior, so
`FORMAT_VERSION` stays 2 and no migration is written.

**Acceptance criteria**

- Regression: `Document::sample()` export byte-hash unchanged; the Ghostscript CI job stays green.
- A document with `master_pages` `opener` (2-column, 108 pt top margin) and `body` (2-column, 36 pt),
  `default_master: "body"`, and `pages[0].master = "opener"` lays out page 0 with a text frame whose
  `y_pt` is 108.0 and pages 1+ at 36.0 — asserted to 0.01 pt on a 3-page document.
- A `PageOverride` naming a master not in `master_pages` falls back to `default_master`, and then to
  the document's own page setup — laid out, not rejected. Asserted for both fallback steps. (A
  missing master is the authoring-posture case, like a missing style at
  crates/core-model/src/lib.rs:401-414: losing the page would be worse than losing the furniture.)
- A `pages` list shorter than the page count governs the pages it covers and leaves the rest on
  `default_master`; a list *longer* than the page count is not an error (the document shrank) and the
  surplus entries are ignored — asserted both ways.
- Statics resolve per page against the *page's* master, so a page-0 opener with no folio and a body
  master with a `{page}` folio produces zero statics on page 0 and one on page 1 — asserted, and the
  flow cursor is identical with and without statics (the spec-0029 invariant, re-asserted here
  because 0035 is the first thing to vary statics *between* pages).
- Incremental safety: `LayoutSession`'s `previous_context` fingerprint
  (crates/layout-engine/src/session.rs:104-113) includes the page list, so changing `pages[7].master`
  and calling `relayout` reports the affected pages as changed rather than returning stale pages.
  Asserted in both directions — changed context ⇒ `pages_reused == 0` for the affected run, identical
  context ⇒ `blocks_measured == 0`.
- `quill-testdoc` is unchanged and the 500-page target assertion still holds (this increment adds no
  default furniture).

**Test strategy** — Inline tests in core-model for the serde default and the fallback chain, and in
layout-engine for geometry-per-page, asserted as exact arithmetic in the repo's style. The
load-bearing test is the `previous_context` one: spec 0031's own comment says a context the
fingerprint misses is "a stale document presented as a current one", and a page list is precisely
such a context.

**Risks** — The fingerprint omission is the silent-wrongness bug and the only real hazard here.
Assigning by index means an inserted page shifts every subsequent assignment; that is the accepted
semantics for this increment (the alternative, anchoring assignment to a chapter's first block, is
0041's territory and is recorded as a follow-up, not smuggled in here).

### 0036 document-templates

**Document templates: a `Template` type, bundled starters, `Document::from_template`, `quill new`,
and the app's New-from-template** · size: medium · branch: `feat/document-templates`

There is no way to start a document. `AppState` offers `open` and `sample`
(crates/app/src/lib.rs:81-90) and the CLI offers `sample`, `preflight`, `export`, `pack`, `render`
and `synth-icc` (crates/cli/src/main.rs) — a beginner's only starting point is
`Document::sample()`'s two hardcoded blocks, or hand-written JSON. Add a `Template` = everything a
document has *except* content: `page_setup` (with real margins), `styles`, `master_pages`,
`default_master` and an initial `pages` list. Bundle three as data — a 6×9 single-column adventure,
a 6×9 two-column rulebook, and a US-Letter one-column playtest doc — expose
`Template::bundled() -> &[Template]` and `Document::from_template(&Template)`, add `quill new
--template <name> -o out.tpub`, and an `AppState::new_from_template`.

This is where the roadmap's margins question is answered: templates carry non-zero margins,
`PageSetup::default()` stays at zero, and `Document::sample()` is untouched — so the shipped default
is still "text to the trim edge", but nobody reaches it by accident any more, because the on-ramp
never starts there.

**Acceptance criteria**

- Regression: `Document::sample()` export byte-hash unchanged; `PageSetup::default()` still returns
  `Margins::default()` (all zero), asserted, so the CI golden path provably cannot move.
- Every bundled template round-trips: `Document::from_template(t)` → `to_json` → `from_json` is
  `assert_eq!`-equal, for each template, in a loop over `Template::bundled()`.
- Every bundled template produces a **press-clean** document: `preflight(&Document::from_template(t))`
  reports zero `Severity::Error` findings for each one — a starter that fails preflight is worse than
  no starter, because it teaches the beginner that the error panel is noise.
- Every bundled template has non-zero margins on all four edges and a stylesheet containing `body`
  and `h1`..`h3` at minimum — asserted in the same loop, so a template cannot be added without them.
- A template with a chapter-opener master assigns it to page 0 via the spec-0035 `pages` list;
  laying out a from-template document with 3 pages of body text puts the opener geometry on page 0
  only (asserted).
- `quill new --template rulebook -o out.tpub` exits 0, writes a `.tpub` that `Tpub::open` reads back
  to an equal `Document`, and `quill new --template nope` exits non-zero listing the valid names.
  `quill new --list` prints them.
- `AppState::new_from_template(t)` returns a state whose `page_count() >= 1` and which paints without
  panicking on an empty content list — asserted headlessly (the empty-document case is new: every
  existing app test opens a document that has content).
- Templates are data in `quill-core-model`, not files on disk: no new runtime dependency, no asset
  path to resolve, and `cargo tree -d` reports no new duplicates.

**Test strategy** — The loop-over-all-bundled-templates tests are the design point: adding a fourth
template must be caught by the same assertions rather than needing new ones. Preflight-cleanliness is
the acceptance criterion that actually protects the beginner. The app test covers the empty-content
path, which nothing exercises today.

**Risks** — Bundling templates as Rust data keeps the dependency graph clean but makes them
non-editable by users; a user-authored template format is explicitly deferred to M3 and recorded as
a follow-up. The empty-document path through layout, paint and preflight is genuinely untested
today, so expect this increment to surface at least one unwrap on `content[0]`-shaped reasoning.

### 0037 decoration-primitive

**Decoration primitive: filled and stroked rectangles through layout, paint list and PDF writer —
with the color and ink-coverage preflight extended to cover them** · size: medium · branch:
`feat/decoration-primitive`

Nothing in the workspace can draw a line or a box. `PlacedBlock` is `Text | Image`
(crates/layout-engine/src/lib.rs:125-142) and `PaintOp` is `Page | TrimGuide | Text | Image`
(crates/render/src/paint.rs:27-60) — `TrimGuide` is a screen-only guide, not press content, and there
is no stroke anywhere. Add `PlacedBlock::Rect { frame, fill: Option<Color>, stroke:
Option<(Color, Pt)> }`, a matching `PaintOp::Rect`, `tiny-skia` rasterization, and PDF emission
(`re` plus `f`/`S`/`B` with the correct color operators per space). Land it **at parity**: no
`Block` variant produces one yet, so no existing document changes and the export byte-hash holds.

The increment's real content is the second half. Both press checks currently walk `Block`, not
geometry: the color check matches `Block::Heading | Block::Body` for a color and returns `None` for
images (crates/export-pdf/src/lib.rs:195-196), and ink coverage is enforced per-image and per-text
color. A synthesized rectangle is neither, so on the day 0038 emits a tinted panel at 280% total ink
it would sail past preflight into a print shop. Extend both checks to walk the laid-out geometry,
and prove it with a rectangle that violates each rule.

**Acceptance criteria**

- Regression: `Document::sample()` export byte-hash unchanged; every existing export-pdf and render
  test passes untouched; Ghostscript CI green.
- A page carrying one `PlacedBlock::Rect { fill: Cmyk(0,0,0,0.1), stroke: (Cmyk(0,0,0,1), 0.5) }`
  exports a content stream containing the fill color operator, `re`, and `B` (or `f` then `S`), with
  operands equal to the frame geometry to 0.01 pt — asserted by decompressing the stream, as the
  existing writer tests do.
- Rect ordering is deterministic and behind text: within a page, statics paint first, then blocks in
  order, and a `Rect` emitted before a `Text` in the same list stays before it. Asserted on the op
  list, which is the golden artifact (spec 0033).
- Rasterization: a page with a single black-filled rect at a known position produces a black pixel at
  the rect's center and a white pixel outside it, at 1× — the coarse-invariant style spec 0033
  established, no pixel golden.
- **Preflight sees it.** A document whose laid-out geometry contains a rect filled at 280% total ink
  produces a `Severity::Error` `InkCoverage` finding naming the offending element; an RGB-filled rect
  produces the same `Severity::Error` the color check gives RGB text. Both asserted, and both fail
  before this increment's preflight change (stated in the spec as the reason the check moved).
- A rect with neither fill nor stroke emits nothing (no empty `re n`), and a zero-width or
  zero-height rect emits nothing — asserted, so degenerate geometry cannot produce a malformed
  content stream.
- Stroke width is in points and unscaled by any CTM the writer sets; a 0.5 pt stroke measures 0.5 pt
  in the emitted `w` operand — asserted, because a hairline that silently becomes 0.5 px is the
  classic press bug here.
- `benches/budgets.toml` unchanged (this increment adds no work to any measured path) — asserted by
  the CI bench step staying green without a re-baseline.

**Test strategy** — Parity first: the byte-hash plus the untouched existing suites prove the seam is
inert. Capability tests construct `PlacedBlock::Rect` directly rather than through a document, which
is exactly how the repo proves a parameter is threaded (the `HalfStub` precedent at
layout-engine/src/lib.rs:410-418). The two preflight tests are the load-bearing ones and must be
written to fail against the pre-change checker.

**Risks** — Preflight walking laid-out geometry rather than the document is a structural change to
what preflight *is*, and export currently preflights the model. If that turns out to require laying
out inside `export()`, say so in the spec and decide it there rather than during implementation —
the fallback is that `Block`-level checks stay on the model and geometry-level checks run on the
already-computed pages the writer is about to draw. PDF color-space operators differ per space
(`k`/`K`, `g`/`G`, and RGB must not appear at all in PDF/X); getting the stroke/fill pair wrong is a
silent color bug, which is why the operand assertions are exact.

### 0038 stat-block

**`Block::StatBlock`: the composite, keep-together TTRPG component** · size: large · branch:
`feat/stat-block`

`quill-components-ttrpg` defines `StatBlock { name, overview, attributes, details, actions,
reactions }` with serde and tests, and **no crate in the workspace depends on it** — it cannot be put
in a document, laid out, drawn or exported. Add `Block::StatBlock` carrying that type (plus a
`BlockId` and an optional style-prefix name), and teach `measure_block` to break it into a sequence
of styled lines inside a padded, tinted, ruled panel built from spec 0037's rect. Built-in styles
`statblock-title`, `statblock-attr` and `statblock-body` extend `StyleSheet::default()`
(crates/core-model/src/lib.rs:364-392) so a stat block looks right with zero authoring.

The layout question this increment exists to answer is **breaking**. A stat block is taller than a
column more often than not in a 6×9 book, so "never split" is not implementable; the rule is
keep-together-if-it-fits-in-an-empty-frame, otherwise split at a section boundary and repeat the
panel decoration on the continuation.

**Acceptance criteria**

- Regression: `Document::sample()` export byte-hash unchanged; Ghostscript CI green.
- Non-ASCII guard: a stat block whose name contains `é` exports with that glyph present in the font
  subset, not `.notdef` — the spec-0026 silent-failure test, because `collect_doc_chars`
  (crates/export-pdf/src/lib.rs:334-341) must learn the new variant.
- The four other exhaustive `Block` match sites compile-error until updated and are each updated
  deliberately: the color preflight (export-pdf/src/lib.rs:195-196), `measure_block`
  (layout-engine/src/lib.rs:410-442), the session's content fingerprint (session.rs:428-448) and
  `StyleSheet::resolve` (core-model/src/lib.rs:401-414). The spec names all five.
- Geometry: a stat block with a 6 pt padding, one title line and three attribute lines measures to
  `padding*2 + title_leading + 3*attr_leading` in height, to 0.01 pt, with `MonospaceRunMetrics
  { em_ratio: 0.6 }` — hand-computed in a comment above the assertion, per repo convention.
- Panel decoration: the placed output contains exactly one `PlacedBlock::Rect` whose frame is the
  full panel including padding, emitted *before* the panel's text blocks, and the text is inset by
  the padding on all four edges (asserted on x, y and width).
- Keep-together: a stat block that does not fit in the remaining space of a partly-filled frame, but
  *does* fit in an empty one, moves whole to the next frame — asserted by placing it on the next page
  with no split, and by asserting the previous page's other content is unchanged.
- Split: a stat block taller than an empty frame splits at a section boundary (never mid-attribute),
  every fragment carries its own panel rect, and concatenating the fragments' lines equals the
  unsplit line sequence — asserted, so splitting cannot silently drop a line.
- Cache correctness: the session's content fingerprint covers every field of the stat block —
  editing only `reactions[0]` gives `blocks_measured == 1` on relayout, and editing nothing gives
  `blocks_measured == 0`. Both directions asserted (the spec-0031 rule: a key that misses a
  dimension is a silently stale document).
- `quill-testdoc` gains a `statblock_every_n` knob defaulting to 0 (off), so the 500-page target and
  every existing budget in `benches/budgets.toml` are unchanged; a separate unit test lays out a
  100-stat-block document and asserts every block placed.

**Test strategy** — Inline, `MonospaceRunMetrics`-based geometry assertions for measurement; op-list
assertions for decoration order; and the split test asserted by line-sequence concatenation rather
than by fragment count, because fragment count is an implementation detail and dropped lines are the
failure that matters. The keep-together and split tests are the two that justify the increment's
"large" size — everything else is plumbing a variant through five match sites.

**Risks** — The split rule is where this increment can quietly go wrong: a fragment that repeats the
panel but loses a line, or a keep-together that infinite-loops by moving a block that never fits, are
both plausible. The "never fits anywhere" case needs an explicit escape (place it and overflow, do
not loop) or a 500-page document with one oversized stat block hangs the app — the same
termination-bound hazard spec 0031 recorded. Knuth-Plass superlinearity (spec 0051, since fixed) was
more exposed here than anywhere: a stat block's `details` entries are prose and could plausibly be long.

### 0039 tables

**`Block::Table`: random tables, column widths, header rows and page-breaking rows** · size: medium
· branch: `feat/table-block`

The other half of `quill-components-ttrpg`: `RandomTable { die, entries: Vec<TableEntry { low, high,
result }> }` with `lookup` and `is_complete` already implemented and tested
(crates/components-ttrpg/src/lib.rs), and no way to put one on a page. Add `Block::Table` carrying a
general two-column-or-more table (a random table is the special case where column 0 is a die range),
with per-column widths as fractions of the frame, an optional header row that repeats on
continuation, zebra fill via spec 0037's rect, and row-granular page breaking.

**Acceptance criteria**

- Regression: `Document::sample()` export byte-hash unchanged; the non-ASCII font-subset guard as in
  0038; all five `Block` match sites updated deliberately and named in the spec.
- Column geometry: a table with widths `[0.25, 0.75]` in a 200 pt frame places column 0 at `x = 0`
  width 50 and column 1 at `x = 50` width 150, to 0.01 pt. Widths that do not sum to 1.0 are
  normalized, and a zero or negative width fails loudly at load with a `LoadError` rather than
  emitting degenerate geometry (mirroring the `columns: 0` rule in spec 0030).
- A cell whose text is wider than its column wraps to multiple lines and the row's height is the
  tallest cell's height — asserted with `MonospaceRunMetrics`.
- Breaking: a table that overflows a frame breaks between rows, never inside one, and the header row
  repeats at the top of the continuation — asserted by comparing the first row of the continuation to
  the header.
- `RandomTable` → `Block::Table` conversion is a tested function: a d100 table with 6 entries
  produces 6 rows whose column 0 reads `1-10`, `11-25`, … (single values render as `7`, not `7-7`) —
  asserted, including the singleton case.
- `is_complete() == false` is surfaced, not silently laid out: a gappy random table lays out fine but
  preflight emits a `Severity::Warning` naming the gap. (Authoring posture — a gap in a d100 table is
  a content mistake, not a press defect, so it must not block export.)
- Zebra fill emits one `PlacedBlock::Rect` per shaded row, behind that row's text, and none when
  zebra is off — asserted on the op list.
- Cache correctness: editing one cell gives `blocks_measured == 1`; editing nothing gives `0`.
- `benches/budgets.toml` unchanged; a unit test lays out a 500-row table and asserts every row
  placed across the pages it needs.

**Test strategy** — Geometry as exact arithmetic; breaking asserted by header-repetition and by
row-sequence concatenation (as in 0038, dropped rows are the failure that matters). The
`RandomTable` conversion test covers the singleton range because `7-7` is the kind of detail that
ships and then embarrasses.

**Risks** — Row-height-from-tallest-cell interacts with the keep-together machinery from 0038; if
0038's splitting turns out to generalize, this increment should reuse it rather than fork it, and the
spec must say which. A 500-row table in one frame is a plausible user input and a plausible
performance cliff.

### 0040 heading-index

**Heading index: which page each heading landed on, reported by layout** · size: small · branch:
`feat/heading-index`

"Which page is chapter 3 on" is currently unanswerable. `LayoutResult` reports `pages`, `stats` and
`changed_pages` (crates/layout-engine/src/session.rs:69-77) and `LaidOutPage` carries `index`,
`blocks` and `statics` (crates/layout-engine/src/lib.rs:146-161) — a `PlacedBlock` has geometry but
no back-reference to the `BlockId` that produced it, so nothing downstream can map content to a page.
Add that mapping: an ordered `Vec<HeadingEntry { id, level, text, page_index }>` produced by both the
one-shot `lay_out*` entry points and `LayoutSession::relayout`. Purely additive — no export change,
no model change, no new `Block` variant.

**Acceptance criteria**

- Regression: `Document::sample()` export byte-hash unchanged; every existing layout test passes
  untouched.
- On a 3-page document with headings on pages 0, 1 and 1, the index has three entries with
  `page_index` `0, 1, 1`, in document order, carrying the right `BlockId`, `level` and resolved text.
- A heading that splits across a page boundary (its first line on page 4, its second on page 5)
  reports the page its **first line** landed on — asserted, because that is what a TOC entry and a
  bookmark both mean, and the alternative is a silent off-by-one that only shows up in a real book.
- Parity between paths: the index from `lay_out_with_template` and from
  `LayoutSession::relayout` is `assert_eq!`-equal for the same document, including the 500-page
  synthetic one.
- Incremental correctness: after an edit that pushes a heading from page 250 to page 251, the index
  reflects the new number even though `stats.pages_reused >= 495` — this is the one that proves the
  index is not accidentally cached alongside the pages it describes.
- Empty document ⇒ empty index, no panic.
- `benches/budgets.toml` unchanged: building the index is one push per heading during a pass already
  walking every block, and the CI bench step must stay green without a re-baseline.

**Test strategy** — Straight assertions on the returned vector; the load-bearing two are the
split-heading page attribution and the incremental-edit case. Both are cheap and both are the bugs
this would otherwise ship with.

**Risks** — Small increment, one real hazard: the index must be rebuilt (or correctly patched) on an
incremental pass that reuses pages, or a TOC built on it goes stale exactly when the document is
edited, which is always.

### 0041 generated-toc

**`Block::Toc`: generated contents with a bounded stabilization loop** · size: large · branch:
`feat/generated-toc`

The cyclic increment. Add `Block::Toc { id, title, max_level, style_prefix }` which generates its own
content from spec 0040's heading index: one line per heading at or above `max_level`, with a leader
and a page number, styled by `toc-1`..`toc-6` built-ins. Because the generated content changes the
document's pagination, layout becomes a fixpoint: lay out, read the index, regenerate the TOC's
lines, lay out again, and stop when the index stops changing — with an explicit iteration cap and
documented behavior when it is hit.

Two things make this tractable rather than open-ended. First, the TOC's *content* is derived, so it
never enters the measurement cache keyed on authored text — its cache key must include the index it
was generated from, or an edit elsewhere in the book leaves a stale TOC. Second, oscillation is real
and must be handled, not assumed away: an entry that pushes a heading to the next page can shorten
the TOC on the next iteration and pull it back, forever. On hitting the cap, the loop takes the last
iterate and records it, rather than looping or failing the document.

**Acceptance criteria**

- Regression: `Document::sample()` export byte-hash unchanged; Ghostscript CI green; non-ASCII
  font-subset guard as in 0038; all five `Block` match sites updated and named in the spec.
- A 3-chapter document with a TOC at the front lists three entries whose page numbers equal the
  numbers those headings actually appear on in the *final* layout — asserted by cross-checking the
  TOC's rendered text against spec 0040's index of the same final pass. This is the whole feature and
  it must be asserted against the final state, not the first pass.
- Convergence: the loop reaches a fixpoint in `<= 2` iterations on a document whose TOC fits on one
  page, and `<= 4` on one whose TOC spans three pages — asserted via an exposed iteration counter, so
  "it converged" is a measured claim.
- Oscillation: a hand-built document constructed to oscillate terminates at the cap, returns the last
  iterate, produces a laid-out document with no missing content, and reports the non-convergence
  through a counter or flag the caller can see (asserted). It must not loop, panic, or silently
  present an inconsistent TOC as converged.
- `max_level: 2` lists h1 and h2 and omits h3 — asserted.
- Leaders and numbers: an entry's page number is right-aligned to the frame's right edge to 0.01 pt
  and the leader fills the gap without overlapping either side — asserted on geometry, not on
  rendered appearance.
- Cache correctness: editing a chapter heading's text updates that TOC entry's text on the next
  relayout (`blocks_measured >= 1` for the TOC block); editing an unrelated body paragraph that does
  not move any heading leaves the TOC unmeasured (`blocks_measured` for the TOC block `== 0`). Both
  directions asserted — this is spec 0031's rule applied to derived content.
- A document with a TOC and no headings lays out an empty TOC (title only) and does not panic.
- `benches/budgets.toml` gains a `toc.iterations` ceiling; the 500-page synthetic document's existing
  budgets are unchanged, because `quill-testdoc` does not emit a TOC by default.

**Test strategy** — The final-state cross-check against spec 0040's index is the primary test and is
deliberately written so it cannot pass by comparing a first-pass TOC to first-pass numbers. The
oscillating fixture is hand-built rather than found, and its purpose is to prove the cap is real. The
two cache-direction assertions prevent both a stale TOC and a TOC that re-measures on every keystroke.

**Risks** — The fixpoint is the risk and the cap is the mitigation; the spec must state the cap's
value, the tie-break, and what the user sees on non-convergence *before* implementation starts.
Derived content in a cache keyed on authored content is the second hazard, and it fails silently in
the stale direction. Interaction with spec 0035's index-based page assignment is real: a TOC that
grows by a page shifts every subsequent `PageOverride`, which is a genuine consequence of 0035's
accepted semantics and must be called out in the spec, and covered by a test, rather than discovered
by a user whose chapter openers all slid by one.

### 0042 pdf-outline

**PDF outline and named destinations, plus the annotation/bleed preflight guard** · size: medium ·
branch: `feat/pdf-outline`

A 500-page PDF with no bookmarks is unusable, and a grep for `Outlines`, `Dests`, `Annots` and
`bookmark` across the workspace returns **zero hits** — the writer emits a catalog and a page tree and
nothing navigational. Build an `/Outlines` tree from spec 0040's heading index (nested by level, with
correct `/First`, `/Last`, `/Count` and `/Parent` links) plus a named destination per heading, and
point each outline item at its page.

The press constraint decides the scope. Outlines and destinations are document-level structure and
are fine in PDF/X. **Link annotations are not** — PDF/X-1a requires annotations to sit outside the
BleedBox, and a clickable TOC entry sits in the middle of the text block by definition. So this
increment ships outlines and destinations, does **not** ship clickable TOC links, and adds the
preflight check that makes the rule enforceable rather than remembered: any annotation whose rect
intersects the BleedBox is a `Severity::Error`. That check has nothing to guard today, which is
exactly when it is cheap to add and exactly the spec-0013 lesson — the validator and the writer must
agree on the rule before anything relies on it.

*(Since shipped: spec 0052 is what relies on it. The screen profile emits the link annotations this
increment could not, and the press profile runs every one of them through `annotation_finding` — so
a press file's emptiness is now a consequence of this check rather than of there being nothing to
check.)*

**Acceptance criteria**

- Regression: `Document::sample()` export byte-hash **changes** (the catalog gains `/Outlines`), so
  the committed constant is updated in this PR with the reason in the commit message, and the
  Ghostscript CI job stays green. This is the one M2 increment expected to move the hash; it says so
  here so a reviewer does not treat it as an accident.
- A document with h1/h2/h1 produces an outline tree of two top-level items, the first with one child;
  `/Count` on the root is correct and each item's `/Parent`, `/First`, `/Last`, `/Prev` and `/Next`
  are consistent — asserted by parsing the emitted objects, not by eyeballing.
- Each outline item's destination resolves to the page the heading is on, verified against spec
  0040's index for the same layout.
- A document with no headings emits **no** `/Outlines` entry at all (not an empty one) — asserted,
  because an empty outline tree renders as an empty, confusing bookmark pane.
- Ghostscript parses every generated file with outlines without warnings; the CI job's existing
  parse gate covers it and the sample gains headings at two levels so the gate actually exercises the
  tree.
- Preflight: a document carrying an annotation whose rect intersects the BleedBox produces a
  `Severity::Error`; one entirely outside it does not. Both asserted against a hand-constructed
  annotation, since nothing emits annotations yet.
- Outline titles are the heading text with the font subset covering their glyphs, and PDF text
  strings are encoded correctly for non-ASCII (UTF-16BE with BOM) — asserted on a heading containing
  `é`, because outline strings are a *different* encoding path from page content and get this wrong
  independently.
- `benches/budgets.toml` unchanged.

**Test strategy** — Object-graph assertions by parsing the emitted PDF, mirroring the existing writer
tests' decompress-and-match approach. The no-headings and non-ASCII cases are the two that ship
broken otherwise. The annotation preflight test constructs its input directly because the feature it
guards does not exist — that is the point.

**Risks** — Outline tree links (`/Count` semantics for closed vs open items in particular) are fiddly
and wrong-in-a-viewer rather than wrong-in-a-parser, so Ghostscript passing is necessary and not
sufficient. Moving the golden hash means every test pinning it must be updated in the same PR, and a
reviewer must be able to tell this move from an accidental one.

### 0043 markdown-import

**Markdown-ish import: `quill import`, the actual on-ramp** · size: large · branch:
`feat/markdown-import`

Everything above makes a good book *possible*; nothing above makes it *easy to type*. The current
authoring input is hand-written JSON — `Document::sample()` and `quill sample` are the only
document-producing paths in the CLI, and a grep for markdown, import or parsing across the workspace
finds nothing. Add a small, explicitly-specified line-oriented syntax — `#`..`######` headings, blank-
line-separated body paragraphs, `![](asset-id)` images, and fenced `:::statblock` / `:::table`
blocks carrying the component data — and a `quill import doc.md -o out.tpub --template rulebook` that
composes it with spec 0036's templates. Hand-rolled: no markdown crate, per the workspace's
minimal-and-permissive dependency rule, and because the syntax is deliberately a subset rather than
CommonMark.

**Acceptance criteria**

- Regression: no change to any existing crate's behavior; `Document::sample()` export byte-hash
  unchanged.
- Round-trip fidelity on the constructs it claims: a fixture document containing every supported
  construct imports to a `Document` whose `content` matches a hand-written expected `Vec<Block>`
  exactly (ids excepted) — asserted block by block, not by count.
- Unsupported syntax is reported, never silently dropped: an unknown `:::foo` fence, a malformed
  stat-block field, or an inline construct the subset does not cover produces a diagnostic with a
  **line number**, and the import either fails or completes with warnings per a rule the spec states
  explicitly. Asserted for each case. (Silently dropping a paragraph a beginner typed is the worst
  failure this feature can have, and it is the easy one to write.)
- `quill import` composes with a template: `--template rulebook` produces a document with the
  template's margins, styles and masters and the file's content; `--template` omitted uses a
  documented default and says which in the output.
- The importer is a library function in its own module with the CLI as a thin caller, so the app can
  reuse it (asserted structurally: the CLI's handler is under 20 lines).
- A 5,000-paragraph input imports in well under the 500-page layout budget and is asserted to produce
  the expected block count — the importer must not be the new bottleneck.
- No new dependency; `cargo tree -d` clean.
- `docs/format-spec.md` gains the import syntax as a documented, versioned appendix, and a test parses
  the doc's own example, per the spec-0030 precedent that stops docs drifting from code.

**Test strategy** — Table-driven parser tests over small inputs, plus one whole-document fixture
asserted block by block. The diagnostics tests are as important as the happy path and are written
first. The doc-example-parses test is the anti-drift guard the repo already uses.

**Risks** — Scope. A "markdown-ish" syntax invites unbounded creep toward CommonMark; the spec must
enumerate exactly what is supported and state everything else as an explicit non-goal, or this
increment never closes. Round-tripping *out* to markdown is not in scope and should be named as a
non-goal. The stat-block and table fences duplicate structure that 0038/0039 defined in Rust types —
that duplication must be a serde derive over the same types, not a second hand-written schema.

## M3 increments

M2 got a beginner from a `.md` file to a templated book that exports press-clean. M3 is about the
book being *right* rather than merely produced: **content that does not fit stops being abandoned at
the foot of a column**, **furniture sits where a bound book puts it**, and **the file that goes to
the printer is checked against the printer's actual requirements** rather than against one
hard-coded set of numbers that happens to be DriveThruRPG's.

Every item left in "Known issues" below is M3 work, and the largest of them — a block never splits
across frames — has now been hit by three separate increments. M3 opens by building it once. (The
list was four items when M3 was decomposed; spec 0051 has since shipped and deleted its own.)

Same rule as M1 and M2: each increment compiles, its tests pass, and it is a coherent PR on its own.
Every increment carries the `Document::sample()` export byte-hash regression bullet.

| # | Spec | Increment | Size |
|---|---|---|---|
| 1 | 0044 | [Block fragmentation — the vertical list, break opportunities, and `split_at`](#0044-block-fragmentation) | large |
| 2 | 0045 | [Table continuation — break between rows, repeat the header](#0045-table-continuation) | medium |
| 3 | 0046 | [Stat-block continuation — keep together, else break at a section](#0046-stat-block-continuation) | medium |
| 4 | 0047 | [Master statics: alignment and page-parity mirroring; `FORMAT_VERSION` 3](#0047-master-static-alignment) | medium |
| 5 | 0048 | [Hanging indent and non-breaking spaces — key/value pairs that stay paired](#0048-hanging-indent-and-tab-stops) | medium |
| 6 | 0049 | [POD presets — the printer's requirements as data](#0049-pod-presets) | medium |
| 7 | 0050 | [Preflight over placed geometry — effective dpi and the live area](#0050-geometry-preflight) | medium |
| 8 | 0051 | [Knuth-Plass active-node pruning — closing the superlinear cliff](#0051-line-break-pruning) | small |
| 9 | 0052 | [The screen profile — a second export target, with clickable links](#0052-screen-profile) | large |
| 10 | 0053 | [User-authored templates — `quill new --from`](#0053-user-authored-templates) | small |

## M3 sequencing rationale

**Chain 1 — fragmentation is one mechanism, built once, then consumed twice.** The roadmap has
recorded three callers wanting a block to split: paragraphs (spec 0036's ragged two-column feet),
stat blocks (descoped from 0038) and tables (descoped from 0039). Building it inside any one of them
would be the second of three implementations. 0044 therefore builds the mechanism and lands it on
the simplest caller — a paragraph, where `text-layout` already emits a `Vec<Line>` that is a list of
independently paintable items — and 0045 and 0046 are then thin: each teaches its own `Measured`
variant where its break opportunities are, and inherits everything else. If 0044's design is wrong,
it is wrong before two more increments are built on it, which is why the paragraph case ships alone.

This chain also answers the open question "should stat blocks and tables share one splitting
mechanism?" — **yes, and paragraphs share it too**; see the decision recorded in 0044 below.

**Chain 2 — typographic polish is independent of fragmentation and can interleave.** 0047 (master
static alignment) and 0048 (hanging indent) both fix defects found by *rendering* an M2 page, and
neither touches the flow loop. 0047 is sequenced ahead of 0048 because it is the one that changes a
serialized type — it takes `FORMAT_VERSION` to 3 and writes the v2→v3 migration — and a format bump
is cheaper to land while no other increment is mid-flight against the model. 0048 then depends on
nothing but `text-layout`.

**Chain 3 — the presets are data before they are checks.** 0049 turns four numbers that are
currently compile-time constants (`MAX_INK_COVERAGE_PCT`, `min_dpi`'s 300/600, `DEFAULT_BLEED_PT`)
into fields of a named `PodPreset`, and adds the one number the workspace has never had: a **safety
margin**, the distance from trim inside which POD vendors will not guarantee content survives
trimming. 0050 then adds the two checks that need a preset *and* a laid-out page to be meaningful —
an image's dpi at the size it was actually placed, and content intruding into the safety margin.
0050 cannot precede 0049 because both of its checks need a number only a preset carries; and both
are `preflight_pages`-shaped, i.e. over placed geometry, which is where spec 0037 already proved
model-level preflight is blind.

**0051 and 0052 are deliberately unchained.** 0051 is a self-contained algorithmic fix to a cliff
spec 0027 measured and `benches/budgets.toml` already pins; it can ship at any point and is placed
late only because it is the increment whose absence hurts least. 0052 is placed last of the
substantial work because it is the one that changes what "export" means — a second profile, a second
conformance target — and it should not be in flight while 0049/0050 are changing what export
*checks*. 0053 is the milestone's smallest increment and closes a follow-up already named in the
code (`crates/core-model/src/template.rs:7-9`).

**Cross-cutting: fragmentation is the one that can silently corrupt.** Five of M3's ten increments
touch the flow loop or export, but only the fragmentation chain can lose content — a split that
drops its remainder produces a book that is missing a paragraph, which no numeric test notices
unless it is asked to. Every increment in that chain therefore carries a **conservation** assertion:
the concatenation of every fragment equals the unsplit block, asserted over the whole document, not
per block. This is the same posture as `a_five_hundred_row_table_places_every_cell`
(crates/layout-engine/src/lib.rs:3510) — no cell may be lost — generalized to the mechanism that can
now lose them.

## M3 increment detail

### 0044 block-fragmentation

**Block fragmentation: measured blocks become vertical lists with break opportunities, and the flow
loop splits rather than abandons** · size: large · branch: `feat/block-fragmentation`

The pagination loop at crates/layout-engine/src/lib.rs:1311-1459 measures a block against the frame
width and, if `y + height > bottom && !frame_empty`, moves the *whole* block to the next frame. On a
single-column page that is invisible; on spec 0036's two-column `rulebook` template it leaves a
visible hole at the foot of a column whenever the next paragraph is taller than the space left.

The recorded objection to fixing it is that splitting means "measure this for at most H points",
which puts height into `MeasureKey` (crates/layout-engine/src/session.rs:93-99) and turns one cache
entry per block into one per block *per available height* — thrashing the hot path spec 0031 exists
to keep cold.

**That objection is avoidable, and avoiding it is this increment's central design decision.**
Splitting is not a second kind of measurement; it is a *derivation over the measurement already
cached*. A paragraph broken by Knuth-Plass at width W yields a line list that does not depend on how
much vertical space is available — the optimal break is a function of the measure, not of the
column's remaining height. Choosing where to cut that list is therefore a pure function of the
cached `Measured` plus an available height, and needs no cache entry of its own. This is exactly
TeX's separation between `\linebreak` (breaks a paragraph into a vertical list, once, for a measure)
and `\vsplit` (cuts an already-built vertical list to a height), and quill adopts it by name.

Concretely, `Measured` (crates/layout-engine/src/lib.rs:485) gains two methods and no new field:

- `break_opportunities(&self) -> Vec<BreakPoint>`, where `BreakPoint { at: usize, height_before:
  f32, penalty: Penalty }` — `at` is an index into the variant's own item list (lines, rows,
  sections), `height_before` is the height consumed by everything before it, and `penalty` says how
  bad a break here is (`Penalty::Forbidden` is not returned at all; a widow/orphan violation is
  returned with a discouraging penalty so the chooser can take it only when nothing else fits).
- `split_at(&self, at: usize) -> (Measured, Measured)` — the fragment and the remainder, both fully
  measured, at the same width.

The flow loop's doesn't-fit branch then tries `split_at` at the best break opportunity that fits
before falling back to today's move-whole behavior. `FlowState`
(crates/layout-engine/src/lib.rs:1175) gains `split_at: usize` (0 = the block's start) so a
checkpoint can resume mid-block, which is what makes the incremental path
(crates/layout-engine/src/session.rs:220) work across a split.

**Widows and orphans.** A paragraph may not leave one line behind or carry one line forward: the
minimum on each side is 2 lines, a named constant, and a paragraph of 3 or fewer lines does not
split at all. This is a real typographic rule, not a nicety — a single line stranded at the top of a
column is the defect a reader notices first.

**Acceptance criteria**

- Regression: `Document::sample()` export byte-hash unchanged (the sample is single-column and its
  blocks fit), and the Ghostscript CI job stays green.
- **Conservation, asserted document-wide.** For a document laid out into pages, concatenating the
  text of every `PlacedBlock::Text` whose `source` is block B, in page then y order, reproduces B's
  full line list exactly — asserted over a multi-page, multi-column document where at least four
  blocks split. No line duplicated, none lost, none reordered.
- A 20-line paragraph entering a frame with room for 12 lines places 12 lines in that frame and 8 in
  the next, at the same width, with the 13th line's text starting the continuation — asserted by
  line text, not just by count.
- Widow/orphan: a paragraph with room for exactly 1 line at the foot of a column moves whole instead
  of splitting; a split that would leave 1 line in the remainder takes a break one line earlier.
  Both asserted directly, and a 3-line paragraph never splits.
- The two-column `rulebook` template's ragged-foot defect is gone: laying out continuous body text
  into a 2-column page fills the left column to within one line's leading of its bottom before the
  right column starts — asserted numerically, and shown in a rendered page image attached to the PR
  (this increment's whole value proposition is "the page looks right", which is the class of defect
  spec 0036 proved numbers miss).
- `MeasureKey` is unchanged — asserted structurally by a test that lays out a document with many
  splits and checks `blocks_measured` equals the count of *distinct* (block, width) pairs, not the
  count of placements. A split must cost zero extra measurements.
- Incremental parity: `LayoutSession`'s output equals a full relayout for a document containing
  splits, after an edit before, inside and after a split block — three separate assertions, matching
  session.rs's existing parity tests.
- Resuming from a checkpoint that lands mid-block reproduces the same pages as resuming from the
  block's start — the `FlowState.split_at` invariant, asserted directly.
- The heading index still reports the page a heading's *first* line landed on when the heading
  splits, and `crates/layout-engine/src/lib.rs:217-220`'s doc comment — which currently says a
  heading cannot appear twice "because a block is placed whole into one frame and never split" — is
  corrected in the same PR. A doc comment that becomes false is a defect.
- `benches/budgets.toml`: `ms_per_page` and `scaling_ratio` stay within budget; the flow loop gained
  a branch, not a cost.
- An image block still moves whole — `Measured::Image` returns no break opportunities. Asserted, so
  that "everything is splittable" is never assumed.

**Test strategy** — The conservation test is written first and is the one that must never be
weakened; it is the only test that catches the failure mode that matters (content silently lost).
Geometry assertions are exact arithmetic in the repo's style. The rendered-page check follows spec
0036's precedent: produce the image, look at it, attach it.

**Risks** — The incremental path is the hazard. `FlowState` is the resume contract and
`rebuild_checkpoints`/`diff_pages` (crates/layout-engine/src/session.rs) both assume a checkpoint
sits on a block boundary; a mid-block checkpoint that is not correctly restored produces a document
that is subtly different after an edit than after a full relayout — silent, and only in the
incremental direction, which is the direction users actually experience. The parity tests are the
guard and must cover an edit *inside* a split block specifically. Second risk: the "derivation, not
re-measurement" decision is only sound because line breaking is height-independent; if a later
increment introduces a block whose measurement genuinely depends on available height, that block
must return no break opportunities rather than quietly violating the cache contract, and the spec
must say so.

### 0045 table-continuation

**Table continuation: break between rows, repeat the header** · size: medium · branch:
`feat/table-continuation`

Spec 0039 promised breaking between rows with the header repeating on the continuation, and both
were descoped onto 0044's mechanism. With `break_opportunities`/`split_at` in place this becomes
what it should always have been: `Measured::Panel` for a table reports a break opportunity between
each pair of rows, and `split_at` returns a fragment ending at that row plus a remainder that
*re-emits the header* at its top.

The header repeat is what makes this more than a mechanical application of 0044: the remainder is
not a suffix of the fragment's item list, it is a suffix with the header prepended, and its height
is therefore not `total - height_before`. `split_at` returning both halves fully measured — rather
than an index the caller re-measures — is what makes this expressible.

**Acceptance criteria**

- Regression: `Document::sample()` export byte-hash unchanged.
- Conservation: every row of a 500-row table appears exactly once across all fragments, in order,
  and no cell text is lost — the existing `a_five_hundred_row_table_places_every_cell` test
  (crates/layout-engine/src/lib.rs:3510) is strengthened from "every cell placed somewhere" to
  "every cell placed exactly once, in row order", and its comment about blocks not splitting is
  removed.
- A table with a header that spans three frames repeats the header at the top of frames 2 and 3 and
  not anywhere else — asserted by counting header-text placements (exactly 3) and by their y
  positions being the top of each fragment.
- A table with `header: None` splits with no repeat and loses nothing.
- Zebra striping continues correctly across a break: the row that starts a continuation is striped
  according to its index in the *whole* table, not its index in the fragment — asserted, because
  getting this wrong produces two adjacent same-colored rows at a page boundary, which looks like a
  rendering bug and is the sort of thing only a render shows.
- A table row taller than a whole empty frame is placed whole and overflows rather than looping —
  the `frame_empty` guard still holds. Asserted.
- The panel decoration (`PlacedBlock::Rect`) closes at the bottom of each fragment and reopens at
  the top of the continuation, rather than one rect spanning a page break. Asserted by rect count
  and bounds.
- A rendered image of a table breaking across a page is attached to the PR.

**Test strategy** — Row-conservation first, then header-repeat counting, then zebra parity at the
seam. The zebra test is the one that would not have been written without asking "what does this look
like", so it is written deliberately.

**Risks** — The header's height is charged to every fragment, so a naive "does the next row fit"
check that forgets the repeated header will overfill the continuation by exactly one header. That is
an off-by-one with a visible symptom and needs its own assertion. Second: a table whose header alone
plus one row exceeds a frame has no valid break at all and must fall back to placing whole rather
than producing an empty fragment and looping forever.

### 0046 stat-block-continuation

**Stat-block continuation: keep together when it fits, break at a section boundary when it does
not** · size: medium · branch: `feat/stat-block-continuation`

Spec 0038's original promise, now buildable. A stat block is a composite of named sections — name,
overview, attributes, details, actions, reactions (crates/components-ttrpg/src/lib.rs:10) — and its
break opportunities are the boundaries *between* those sections, never inside one. Keep-together
stays the default and remains free: a stat block that fits a frame is placed whole exactly as today,
because 0044's loop only tries to split a block that does not fit.

**Acceptance criteria**

- Regression: `Document::sample()` export byte-hash unchanged.
- `a_stat_block_moves_whole_to_the_next_frame_rather_than_splitting`
  (crates/layout-engine/src/lib.rs:3221) is *replaced*, not deleted, by two tests: a stat block that
  fits the next frame still moves whole to it (keep-together preferred over splitting), and one that
  fits no frame splits at a section boundary. The preference order is the assertion.
- Conservation: every section's text appears exactly once across the fragments.
- No break falls inside a section — asserted by checking every fragment boundary against the section
  list, so an attributes list is never cut between two attributes.
- A stat block whose *first* section alone exceeds a frame is placed whole and overflows, rather
  than producing an empty fragment.
- The panel closes and reopens per fragment (as 0045), and the continuation's panel starts at the
  frame top.
- A rendered image of a stat block breaking across a column is attached to the PR.

**Test strategy** — The preference-order test is the one that pins the actual behavior change and is
written first, because "splits correctly" is worthless if it splits things that should have moved.

**Risks** — Sections are coarse: a stat block with a very long `actions` section and nothing else
will still overflow. That is the accepted behavior for this increment and must be stated as a
non-goal rather than half-fixed; splitting *inside* a section is a paragraph problem and 0044
already solves it for paragraphs, but wiring per-section paragraph splitting through the composite
is a follow-up, recorded not smuggled.

### 0047 master-static-alignment

**Master statics gain alignment and page-parity mirroring; `FORMAT_VERSION` 3** · size: medium ·
branch: `feat/master-static-alignment`

`MasterStatic::Text` (spec 0030) is drawn as one line from its rect's left edge, and the same rect
is used on rectos and versos alike. A running head cannot be centred and a folio cannot sit at the
outside corner of a spread — the two most conventional placements in a bound book. Spec 0036's
templates work around it by insetting the folio to the fore-edge margin, which was found by
*rendering* a template page: every numeric test passed while the folio printed hard against the
trim, where the guillotine goes.

The fix belongs on the static, not on each template: an `align: Alignment` resolved within the
static's rect, and an x that resolves inside/outside by page parity exactly as `Margins` already
does (crates/core-model/src/lib.rs:132-139). Both are serialized fields on a 0030 type, so this
takes `FORMAT_VERSION` to 3 and writes the v2→v3 migration — the first format bump since 0030.

**Acceptance criteria**

- Regression: `Document::sample()` export byte-hash unchanged (the sample has no masters).
- A v2 document loads, migrates and lays out identically to before — asserted by laying out a
  committed v2 fixture and comparing placed geometry to the pre-migration expectation, not merely by
  "it loads". Migration correctness is the whole point of the version bump.
- A `FORMAT_VERSION` 4 document is still rejected with the typed 0025 error; a v1 document still
  migrates through v2 to v3. The full chain is asserted.
- A centred running head is centred to 0.01 pt in its rect on a 3-page document; a right-aligned one
  is flush right.
- A folio with parity-resolved x sits at the *outside* corner on both a recto and a verso — asserted
  as two different x values on two adjacent pages, which is the defect this increment exists to fix.
- Bundled templates (spec 0036) are updated to use real alignment rather than the fore-edge inset
  workaround, and the roadmap's known-issue entry is removed in the same PR.
- A rendered spread showing a recto and a verso side by side is attached to the PR, with the folios
  visibly at opposite corners and clear of the trim.
- `docs/format-spec.md` documents v3 and its migration row (the spec-0030 precedent), and a test
  parses the doc's own example.

**Test strategy** — Migration first, geometry second, render third. The migration fixture is
committed as bytes so it cannot drift with the code that writes it.

**Risks** — A format bump touches load, save, migrate and every fixture. The specific hazard is a
migration that is *lossy in the identity direction*: a v2 document that migrates to v3 and back out
to JSON must not change the document `/ID` for a document that has no statics, or every existing
`.tpub` silently re-exports as a different file. That is the byte-hash bullet, and here it needs
asserting on a masters-bearing fixture as well as on the sample.

### 0048 hanging-indent-and-tab-stops

> **Shipped without the tab stop.** `Line` is one string with evenly distributed gaps; a tab stop
> sets *part* of a line at an absolute x, which needs a line to be a sequence of positioned
> segments — a model change rippling through both painters and the PDF writer. Spec 0048 ships the
> indent plus U+00A0 binding instead, which is what actually keeps a key whole, and records the
> reasoning. The original key-splitting sighting could not be reproduced with current metrics.


**A hanging indent and a single tab stop: key/value pairs that stay paired** · size: medium ·
branch: `feat/hanging-indent`

`Armour Class: 15 (leather, shield)` breaks after `Armour` in the ~150 pt column of the two-column
`rulebook` template, so the key/value pairing is lost — found by rendering spec 0038's panel. The
related cause is that `break_by_width` normalizes every run of inter-word whitespace to a single
space, so a wider-looking `"{key}  {value}"` separator collapses to an ordinary word space and
separates nothing.

Two additions to `text-layout`, both paragraph-level and both serialized on `ParagraphStyle`:

- **`indent: Indent`** — a first-line indent and a hanging indent (a negative first-line indent
  against a body indent), which is what makes a wrapped attribute line up under its value rather
  than under its key.
- **A single tab stop** — a position at which a key/value separator sets the value's left edge, so
  the values in a list of attributes form a column. One stop, not a stop list: the stat-block case
  needs exactly one, and a general stop list is a non-goal to be named explicitly.

**Acceptance criteria**

- Regression: `Document::sample()` export byte-hash unchanged (`Indent::default()` is zero on both
  edges and no default style sets a tab stop, so nothing that exists today moves).
- A paragraph with a hanging indent of 12 pt puts its first line at x and every subsequent line at
  x + 12, asserted to 0.01 pt for both the screen paint list and the PDF writer's output — one
  shaper, two derivation sites, and this is exactly the kind of change that drifts them.
- A key/value line whose value starts at a 60 pt tab stop places the value's first glyph at x + 60,
  and a wrapped value's continuation lines align to the stop rather than to the key.
- The stat-block attribute defect is fixed: rendering spec 0038's panel in a ~150 pt measure shows
  no attribute key broken across lines — asserted structurally (no line ends inside a key) *and*
  shown in an attached render.
- Justified text with a hanging indent still justifies to the correct measure: the available width
  for a continuation line is the frame width minus the indent, not the frame width — asserted,
  because getting this wrong produces lines that overrun the frame by exactly the indent and is
  invisible in a ragged-right test.
- Interaction with 0044: a paragraph with a hanging indent that splits keeps the indent on the
  continuation's lines (they are not first lines). Asserted.
- The roadmap's stat-block known-issue entry is removed in the same PR.

**Test strategy** — Metric assertions in `text-layout`, then a placed-geometry assertion in
`layout-engine`, then the same geometry through both painters. The both-painters test is the
load-bearing one.

**Risks** — Indents interact with justification, with hyphenation and now with fragmentation, and
the failure mode is a line that is slightly too long — which no test sees unless it asserts the
measure. The justified-width criterion above exists for that reason. Second: `break_by_width`'s
whitespace normalization is long-standing behavior other code may rely on; the tab stop is added
*beside* it rather than by changing it.

### 0049 pod-presets

**POD presets: the printer's requirements as data, not as constants** · size: medium · branch:
`feat/pod-presets`

Everything quill checks at preflight is one vendor's numbers, hard-coded: `MAX_INK_COVERAGE_PCT =
240.0` (crates/color/src/lib.rs:15), `min_dpi`'s 300/600 (crates/export-pdf/src/lib.rs:181-187),
`DEFAULT_BLEED_PT = 9.0` (crates/core-model/src/lib.rs:41), and `PdfxVersion` defaulting to X-1a.
They are DriveThruRPG's, they are reasonable, and they are invisible — a user printing with a vendor
that wants something different has no way to say so and no way to find out they should have.

Introduce `PodPreset`, a named bundle of the requirements one vendor states:

```
PodPreset {
    name, source: String, retrieved: String,   // provenance: which page, which date
    trim_sizes: Vec<Size>,                     // the trims this vendor offers
    bleed_pt: Pt,
    safety_pt: Pt,                             // NEW: content inside this of trim is at risk
    max_ink_pct: f32,
    min_dpi_color: f32, min_dpi_line_art: f32,
    pdfx: PdfxVersion,
}
```

The `source`/`retrieved` fields are not decoration. Vendor requirements change, and a preset that
cannot be audited against its source becomes wrong silently — the same failure class as a CI job
that is not a required context. Every bundled preset states where its numbers came from and when,
and a test asserts every preset carries both.

Bundled: `generic` (the conservative intersection — the default, and what the current constants
become), plus one preset per named vendor the product targets. `PodPreset::generic()` must be
numerically identical to today's constants so that adding presets changes no existing behavior.

**Acceptance criteria**

- Regression: `Document::sample()` export byte-hash unchanged, and `quill preflight` with no
  `--preset` produces byte-identical output to today — asserted by a golden report, because the
  whole increment is a refactor plus a flag and any behavior change is a bug.
- `MAX_INK_COVERAGE_PCT`, `min_dpi` and the bleed floor are read from a preset at every call site;
  a grep-style structural test asserts no preflight code path still reads the bare constants.
  (`clamp_cmyk_u8`'s per-pixel clamp is a separate question and is explicitly *out* of scope: it
  rewrites pixels, so making it preset-dependent changes image bytes. Named as a follow-up.)
- `quill preflight --preset lulu` and `quill export --preset lulu` accept the flag, and an unknown
  preset name fails with the available names listed, not with a panic.
- A document whose ink is within `generic`'s limit but over a stricter preset's fails under that
  preset and passes under `generic` — asserted both directions, which is the only test that proves
  presets do anything.
- A document whose trim is not among a preset's `trim_sizes` produces a Warning, not an Error: an
  unusual trim is a conversation with the printer, not a corrupt file. The severity choice is
  asserted, and follows the repo's "prefer a visible failure over silent corruption" rule in the
  direction that does not block a legitimate document.
- Every bundled preset carries a non-empty `source` and `retrieved`; asserted for all of them.
- `docs/format-spec.md` states that a preset is an *export-time* concern and is deliberately not
  serialized into `.tpub` (a document is not bound to one printer), and a test asserts a preset name
  round-trips through the CLI rather than through the document.
- `quill new --preset <name>` seeds `PageSetup` from the preset's first trim and bleed, so the
  on-ramp starts printable for the chosen vendor. **Composed with `--from` this goes through the
  seam spec 0053 already built** — `Template::seeded_with` for the numbers,
  `Template::disagrees_on_trim` for the warning to print. The precedence is settled (see the
  decisions log); 0049 supplies the preset and the flag, not a second answer.

**Test strategy** — The golden-report test first (this is a refactor and must prove it), then the
strict-vs-generic pair, then the provenance assertion. Table-driven over the bundled presets.

**Risks** — The numbers themselves. A preset that misstates a vendor's requirement is worse than no
preset, because it looks authoritative — this is the "prefer a visible failure" rule applied to
data. Mitigations, all in-spec: `generic` is the default and is the conservative intersection, so a
user who never picks a vendor is never *loosened*; every vendor preset carries its source and
retrieval date; and the spec states plainly that vendor presets are a convenience to be confirmed
against the vendor's current specification, not a warranty. Second risk: turning constants into
parameters touches every preflight call site and the temptation is to thread a preset through
functions that do not need one — the `clamp_cmyk_u8` exclusion above is the boundary.

### 0050 geometry-preflight

**Preflight over placed geometry: effective dpi and the live area** · size: medium · branch:
`feat/geometry-preflight`

Two press defects quill cannot currently see, both of which need a laid-out page rather than a
document:

- **Effective dpi.** `ImageResolution` checks `Asset.dpi` — the image's *native* resolution as
  authored (crates/export-pdf/src/lib.rs:240-253). A 300 dpi image scaled up to twice its natural
  size prints at 150 dpi and passes today. The real quantity is pixels ÷ placed inches, and it is
  only knowable after layout.
- **The live area.** Nothing checks that content stays clear of the trim by the vendor's safety
  margin. Text that is inside the page but 2 mm from the guillotine is the defect spec 0036's folio
  had, caught by eye rather than by a check — and every book has dozens of chances to reproduce it.

Both extend `preflight_pages` (crates/export-pdf/src/lib.rs:392-429), which spec 0037 already
established as the pass that sees synthesized geometry, and both read their thresholds from 0049's
preset.

**Acceptance criteria**

- Regression: `Document::sample()` export byte-hash unchanged; the sample passes both new checks
  under `generic` (asserted, so the golden path is proven clean rather than assumed).
- A 300 dpi image placed at 2× its natural size reports an `ImageResolution` Error stating the
  effective dpi (150) and the required one — the message names both numbers, because a preflight
  message that does not say by how much you missed is a message the user cannot act on.
- The same image placed at 1× passes. The boundary (exactly at the threshold) passes.
- A `PlacedBlock` whose frame intrudes into the safety margin reports a new `CheckId::SafeArea`
  finding naming the page index and the edge; content wholly inside passes.
- Master statics are checked too — the folio defect was furniture, not flowed content — and a test
  reproduces spec 0036's original fore-edge folio and asserts it now *fails* preflight. That test is
  the increment's proof of worth.
- Bleed-side content is not flagged: a full-bleed image deliberately extends past trim, and flagging
  it would train users to ignore the check. A placed block extending *outward* past trim is exempt;
  one falling *inward* of the safety line is not. The distinction is asserted.
- `--preset` selects the thresholds; under a preset with `safety_pt: 0.0` the safe-area check is
  inert.
- Findings are per-page and deduplicated: a 500-page document with one systematically misplaced
  master static reports it once per page, not once per placed block, and the report stays readable.

**Test strategy** — The reproduce-the-0036-folio test is written first and is the acceptance
criterion that matters; the rest are boundary cases around it.

**Risks** — False positives destroy a preflight's value faster than false negatives, because a user
who learns to ignore the report ignores the real finding too. The bleed-exemption criterion is the
main guard. Second: effective dpi needs the placed rect, and an image's placed size is derived from
`px_w`/`px_h` and `dpi` today — the check must not become circular by deriving the placed size from
the very field it is checking. It must read the *laid-out* rect.

### 0051 line-break-pruning

**Knuth-Plass active-node pruning: closing the superlinear cliff** · size: small · branch:
`feat/line-break-pruning`

Spec 0027's harness measured it on its first run: an 8× longer paragraph costs ~36× the time, where
linear would be 8× and quadratic 64×. Active-node pruning is missing or ineffective. Low severity
for 30–90 word paragraphs at ~64 µs each, but a genuine cliff for pathological input — a stat block
or table flattened into one very long paragraph, which this product's users plausibly produce, and
which 0043's importer makes easy to produce by accident.

Classic Knuth-Plass prunes the active list two ways: drop nodes whose line cannot reach the current
position without exceeding the badness threshold, and cap the active set, retrying with a looser
threshold if no feasible breakpoint survives. The fix is bounded and local to
`break_paragraph_hyphenated` (crates/text-layout/src/lib.rs:231).

**Acceptance criteria**

- **Output is unchanged for every paragraph in the test corpus** — pruning must remove only nodes
  that could not have won. Asserted by breaking a corpus of paragraphs before and after and
  comparing line lists exactly; this is the increment's central risk and its central test. If any
  output moves, the pruning is wrong, not the test.
- `Document::sample()` export byte-hash unchanged — which follows from the above, and is asserted
  separately because it is the one that reaches press.
- The scaling ratio in `benches/budgets.toml` improves measurably and the new value is pinned:
  an 8× longer paragraph costs no more than ~12× (approaching linear with a log factor), replacing
  today's ~36×. The budget file records both the old and new numbers so the improvement is legible.
- A pathological input — one paragraph of 20,000 words — completes within a stated wall-clock bound
  on CI rather than being untestable. Today's behavior on that input is measured and recorded in the
  spec before the fix, so the improvement is a number and not an impression.
- No behavior change when a paragraph has no feasible breaking: the existing fallback still fires,
  asserted by an unbreakable-long-word case.

**Test strategy** — Corpus equivalence first, and it gates everything else. Then the bench.

**Risks** — Pruning that is slightly too aggressive changes line breaks in rare paragraphs, which
changes page breaks, which changes the export hash — a silent typographic regression that a
performance test would call a success. Corpus equivalence is the only thing standing in front of it
and the corpus must be large enough to be meaningful (the 500-page testdoc is the natural source).

### 0052 screen-profile

**The screen profile: a second export target, with clickable internal links** · size: large ·
branch: `feat/screen-profile` · **shipped** — see `specs/0052-screen-profile.md` for what was built
and the non-goals it is bounded by. The press export of `Document::sample()` is byte-identical at
8786 bytes, verified with `cmp`.

The open question "do clickable internal links ever ship?" has a real answer: **not in the press
file, and yes in a second one.** PDF/X-1a requires annotations outside the BleedBox and a TOC entry
sits mid-text-block by definition, so a link and a press-conformant file are mutually exclusive on
the same page. Every publisher in this product's audience ships two PDFs — a press file to the
printer and a screen file to customers — so the honest design is two profiles, not a compromise
neither is happy with.

`ExportOptions` gains a `profile: ExportProfile` of `Press` (today's behavior, unchanged and
default) or `Screen`. The screen profile: emits `/Annots` link annotations for TOC entries and
outline destinations (0041/0042 already produce the destinations), does not claim PDF/X
conformance — no `GTS_PDFXVersion`, no OutputIntent requirement — and relaxes nothing else. It is
deliberately *not* an RGB profile: colour conversion is a separate question and converting is how
you ship the wrong colours.

The load-bearing property is that adding the screen profile must make the press path *provably*
unchanged, and the guard is spec 0042's `annotation_finding` — which already exists, checks exactly
this, and has never had anything to check.

**Acceptance criteria**

- Regression: `Document::sample()` export byte-hash unchanged under the default (`Press`) profile.
  Byte-identical, not "equivalent".
- **The press profile emits zero annotations, asserted by parsing the emitted PDF** for any
  `/Annots` key — not by asserting the code path is not taken. A structural assertion over the
  output is the only one that survives refactoring.
- Under `Screen`, a TOC entry is a link annotation whose rect covers the entry's placed text and
  whose destination is the heading's page — asserted by parsing the PDF and following the
  destination to the expected page index.
- Under `Screen`, `GTS_PDFXVersion` is absent and the XMP does not claim conformance; under `Press`
  both are present exactly as today. Asserted both ways.
- `quill export --profile screen` works without `--icc`, and the CLI says clearly that the result is
  not press-ready. Under `Press`, `--icc` stays required.
- The Ghostscript CI job gains a *second* invocation asserting the screen file is a valid PDF (not
  PDF/X) while the press file remains PDF/X-conformant — and, per the lesson this repo already paid
  for, the new job is added to the required contexts in the same change. A check that is not a
  required context is not a gate.
- Preflight under `Screen` reports the checks that still apply and states which it skipped, rather
  than silently passing a file it barely examined.
- A page count and file size comparison between the two profiles is in the PR, so the difference is
  visible rather than asserted.

**Test strategy** — Parse the produced PDFs. This increment's claims are all about what is or is not
in the file, and only reading the file can support them.

**Risks** — Two profiles is two things to keep correct, and the failure that matters is the press
file quietly acquiring a screen feature. The parse-the-output assertion is the guard, and it is
written as a *press* test rather than a screen test for that reason. Second risk: scope. Clickable
links are the point; a screen profile that also grows RGB, compression, or reader-spreads never
closes. Everything but annotations and the conformance keys is an explicit non-goal.

### 0053 user-authored-templates

**User-authored templates: `quill new --from`** · size: small · branch: `feat/user-templates`

`Template::bundled()` says it in the code (crates/core-model/src/template.rs:7-9): user-authored
templates are an M3 follow-up. A `Template` is already a serializable bundle of page setup, styles
and master pages; the only thing missing is loading one from a path instead of from the three
compiled-in constructors. This is the increment that turns 0036 from three starters into an
extensible system, and it is last because it is worth nothing until the things a template can
express — aligned statics (0047), indents (0048), preset-seeded geometry (0049) — actually exist.

**Acceptance criteria**

- Regression: `Document::sample()` export byte-hash unchanged; `quill new --list` still lists the
  three bundled templates and `--template <slug>` is unchanged.
- `quill new --from my-template.json -o book.tpub` produces a document whose page setup, styles and
  masters equal the file's — asserted field by field, not by "it produced a file".
- A round-trip: every bundled template serializes to a file that loads back to an equal `Template`.
  This is the test that proves the format is real rather than write-only, and it is table-driven over
  all three.
- A malformed or newer-versioned template file fails with a typed error naming the problem, matching
  spec 0025's load-contract posture. Asserted for both a syntax error and a version mismatch.
- A template referencing a style a document later fails to resolve still lays out, per the
  authoring-posture fallback the repo already uses — losing the styling beats losing the page.
- `quill new --from` composes with `--preset` (0049): the preset seeds trim and bleed, the template
  supplies styles and masters, and the precedence between them when both specify a trim is stated in
  the spec and asserted. Undefined precedence is how this feature becomes confusing.
- `docs/format-spec.md` documents the template file as a versioned, published format, and a test
  parses the doc's own example.

**Test strategy** — Round-trip first, then the error cases, then composition with `--preset`.

**Risks** — Small increment, one real hazard: a template file is a *published format* the moment it
ships, so it needs the same version discipline as `.tpub` rather than being an ad-hoc JSON dump.
The spec must state its version field and its migration posture up front.

**Shipped** (spec 0053, `TEMPLATE_VERSION` 1) — out of the sequenced order, ahead of 0048–0050 and
0052, because it depends on none of them. One criterion is therefore only half-wired and it is
recorded rather than implied: **0049 has not landed, so there is no `--preset` flag to compose
with.** What shipped is the seam and the rule — `PageGeometrySeed`, `Template::seeded_with`,
`Template::disagrees_on_trim`, with the precedence asserted at the library level in both directions
and stated in the spec, the format spec and the decisions log. 0049 inherits one job: call them.
The seam is here rather than in 0049 because the precedence is a *template* question, and answering
it inside 0049 would decide how a template composes in the increment that is not about templates.

## M4 increments

M3 finished the tool. M4 is about a publisher not starting from scratch — and, more precisely, about
**one person's work being usable by another**. A house style, a stat-block layout for a particular
game system, a set of masters that matches a line of books: all of these exist today only as
whatever the author happened to build, unshareable.

### The decision this milestone turns on

`CLAUDE.md` says "plugins / ecosystem", and "plugin" usually means *executable extension* — a
dynamic library, a scripting API, a WASM module. **M4 deliberately does not build that.** It builds a
**declarative content pack**: a versioned bundle of templates, styles, component definitions and
assets, with no code in it.

The reason is this repo's own first rule. *Prefer a visible failure over silent press-corruption.*
An executable plugin that can emit geometry can emit geometry that is wrong — off the trim, over the
ink limit, in the wrong colour space — and a plugin author debugging on screen has no way to know.
Every mechanism M3 built to make press errors visible (0050's placed-geometry preflight, 0049's
preset thresholds, 0052's provable annotation-freedom) assumes quill produced the geometry. Handing
that to third-party code would either bypass those checks or force every one of them to run against
an adversary, which is a different product.

A declarative pack cannot do any of that. It supplies *inputs* — a trim, a style, a component's
shape — and quill lays them out through the same engine, so preflight governs a community template
exactly as it governs a bundled one. The audience wants to share a look, not to ship a program.

This is the one M4 decision most worth overruling deliberately rather than by accident, so it is
stated here rather than implied by the increments. If executable extensions are wanted later, the
pack format is the right substrate to hang them on, and the sandboxing question can be answered once
rather than assumed away.

Same rule as M1–M3: each increment compiles, its tests pass, it is a coherent PR, and every one
carries the `Document::sample()` export byte-hash bullet.

| # | Spec | Increment | Size |
|---|---|---|---|
| 1 | 0054 | [Component definitions as data — a stat block stops being a Rust type](#0054-component-definitions) | large |
| 2 | 0055 | [The `.qpack` container — a versioned, signed-by-provenance content pack](#0055-pack-container) | medium |
| 3 | 0056 | [Pack resolution — install, list, and a document that names what it needs](#0056-pack-resolution) | medium |
| 4 | 0057 | [Authoring a pack from a document — `quill pack extract`](#0057-pack-extract) | medium |
| 5 | 0058 | [The baseline grid](#0058-baseline-grid) | large |
| 6 | 0059 | [Screen/press hyphenation parity — one hyphenator, as there is one shaper](#0059-hyphenation-parity) | small |
| 7 | 0060 | [The over-long last line](#0060-last-line-measure) | medium |
| 8 | 0061 | [A starter gallery and the pack authoring guide](#0061-gallery-and-guide) | small |

## M4 sequencing rationale

**Chain 1 — a component must be data before a pack can carry one.** 0054 is the milestone's real
work and its riskiest increment. `quill-components-ttrpg` defines `StatBlock` and `Table` as Rust
types with hand-written measurement in `layout-engine`; a pack that could only ship *those two*
shapes would be a theming system, not an ecosystem, and a game system whose stat block looks
different is the normal case rather than the exception. 0054 turns a component into a declared
sequence of named, styled sections with a declared panel — which is, not coincidentally, exactly
what `measure_stat_block` already builds at runtime. The two existing components become the first
two *definitions*, and their current behaviour is the acceptance criterion: byte-identical output,
or the generalization is wrong.

0055 then packages definitions with templates and styles; 0056 makes a document able to say which
pack it needs and resolve it; 0057 closes the loop by extracting a pack *from* a document, which is
how a publisher who has already built a book gets a reusable pack without hand-writing JSON. 0057 is
last in the chain because it can only extract what 0054–0056 can express.

**Chain 2 — the two defects M3 found, plus the one it deferred.** 0059 and 0060 are the roadmap's
current known issues and they are small and independent; they are sequenced into M4 rather than left
in the list because a milestone that ends with its own findings unfixed teaches the wrong thing.
0059 is genuinely small — promote the hyphenator beside the shaper. 0060 is the one-line breaker fix
whose blast radius spec 0048 measured, so it carries the cost of re-deriving 0051's equivalence
digest and re-checking 0046's narrow-column behaviour; that work is the increment.

0058 (baseline grid) is placed after them and after 0054 because `CLAUDE.md` has named it a
`layout-engine` responsibility since the beginning and nothing implements it, and because it
interacts with everything that changed in M3 — leading, fragmentation, indents, and now declared
component sections. It is the last large increment for that reason.

**0061 is documentation and is last**, because a guide to a format that is still moving is a guide
that will be wrong.

**Cross-cutting: a pack is content from a stranger.** Every increment in chain 1 carries the same
posture, inherited from spec 0025's load contract: a malformed or newer-versioned pack fails with a
typed error naming the file, never a panic and never a silent default. And a pack may not make
output less press-correct — 0050's preflight runs over a packed component's geometry exactly as it
runs over a bundled one, which is asserted rather than assumed.

## M4 increment detail

### 0054 component-definitions

**A component becomes a declaration: named sections, a panel, and the styles they resolve** ·
size: large · branch: `feat/component-definitions`

`StatBlock` and `Table` are Rust structs in `quill-components-ttrpg`, measured by hand-written code
in `layout-engine` (`measure_stat_block`, `measure_table`). A publisher whose game system sets its
creatures differently — a PbtA move, a Blades clock, an OSR monster line — cannot express it at all.

A `ComponentDef` declares what those functions currently hard-code: an ordered list of sections,
each with a style name, a source field and whether it opens a new section; a panel with a fill,
a stroke and a padding; and the rules a section list needs (a repeated prefix for tables, spec 0045;
a section boundary for cuts, spec 0046). A `Block::Component { def: String, fields: … }` carries the
authored content.

The two built-in components are re-expressed as definitions and their Rust types retire behind them.

**Acceptance criteria**

- Regression: `Document::sample()`'s export byte-hash unchanged.
- **The bundled stat block and table, re-expressed as definitions, produce byte-identical placed
  geometry to today** — asserted against the current `PlacedBlock` output for a corpus of fixtures,
  not against a re-derived expectation. If the generalization cannot reproduce them exactly it is
  the wrong generalization, and that is the whole test.
- A user-defined component with three sections lays out, splits at its section boundaries (0046) and
  preflights (0050) exactly as a built-in one does.
- A definition naming a style that does not exist still lays out — the authoring-posture fallback.
- A definition that is malformed, or names a component version quill does not understand, fails with
  a typed error naming the definition, not a panic.
- `quill import`'s `:::statblock` and `:::table` fences keep working unchanged; the importer resolves
  them through the definitions.
- A rendered page of a *user-defined* component is attached to the PR.
- `benches/budgets.toml`: measurement cost per component unchanged — a definition is interpreted
  once per measurement, not per line.

**Risks** — This is a re-implementation of two shipped features behind a general mechanism, and the
failure mode is subtle geometric drift rather than a broken build. The byte-identical criterion is
the only thing standing in front of it, and it must be asserted over fixtures that exercise wrapped
cells, zebra bands, section rules and the split paths, not over a single simple case.

### 0055 pack-container

**`.qpack`: a versioned bundle of templates, styles, component definitions and assets** · size:
medium · branch: `feat/pack-container`

Follows spec 0025's `.tpub` precedent exactly — a zip, a manifest, a version, a typed load contract —
because a pack is the same kind of object and a second, different container would be a second thing
to get wrong. Its manifest carries a name, a version, a `source` and a licence, because content
arriving from a stranger with no provenance is content nobody should install.

**Acceptance criteria**

- A pack round-trips: written, read back, equal, and every bundled template exports as a pack that
  reloads identically (spec 0053's precedent — a format that round-trips the struct but builds a
  different document proves nothing).
- Malformed, newer-versioned, and missing-manifest packs each fail with a distinct typed error.
- A pack may not carry an absolute path or a `..` traversal in an asset path; asserted, because a
  container from a stranger is the one place that matters.
- Licence and source are required, non-empty, and surfaced by `quill pack info`.

### 0056 pack-resolution

**A document names the packs it needs; `quill pack install` and `list` resolve them** · size:
medium · branch: `feat/pack-resolution`

**Acceptance criteria**

- A `.tpub` naming a pack that is not installed fails to lay out with a typed error naming the pack
  and its version — not a silent fallback to a default style, which would produce a book that looks
  subtly wrong rather than one that refuses to open.
- Version resolution is stated and asserted: exact match, or a documented compatibility rule.
- Two packs defining the same component name is an error naming both, not last-one-wins.
- `quill pack list` shows name, version, source and licence for each installed pack.

### 0057 pack-extract

**`quill pack extract` — turn a finished book into a reusable pack** · size: medium · branch:
`feat/pack-extract`

**Acceptance criteria**

- Extracting from a document produces a pack whose templates, styles and definitions reproduce that
  document's look when applied to a different one — asserted by laying out a second document under
  the extracted pack and comparing to the first's placed styling.
- Content is not extracted. A pack is a *look*, not a book, and the test asserts no block text
  survives extraction.
- Round-trips with 0055 and installs with 0056.

### 0058 baseline-grid

**A baseline grid** · size: large · branch: `feat/baseline-grid`

Named as a `layout-engine` responsibility in `CLAUDE.md` since the beginning; deferred explicitly by
specs 0019, 0020 and 0028. Facing pages whose baselines do not align is the defect that separates a
book from a document, and it is the last of the big typographic gaps.

**Acceptance criteria**

- Every text baseline on a gridded page falls on a grid line, asserted to 0.01 pt across a
  multi-page, two-column document.
- Grid snapping composes with fragmentation (0044): a continuation's first baseline is on the grid.
- It composes with indents (0048) and with declared components (0054) — a component's sections snap
  as body text does.
- Snapping is per frame and local, per `CLAUDE.md`; a global recompute is a named non-goal, and the
  incremental budget is what proves it.
- A rendered spread showing two facing pages with aligned baselines is attached.

**Risks** — Grid snapping changes every baseline in the document, so it changes the export hash and
every geometric fixture in the repo. It needs the same deliberate re-derivation discipline spec 0051
established for its equivalence digest.

### 0059 hyphenation-parity

**One hyphenator, as there is one shaper** · size: small · branch: `feat/hyphenation-parity`

The roadmap's known issue: `quill render` lays out with `NoHyphenator` while `export` uses the real
en-US `HypherHyphenator`, so screen and press break lines differently and a document can have a
different page count in each. `CLAUDE.md` states the rule this breaks in as many words.

`hyphenate::HypherHyphenator` is private to `export-pdf`, which is why the CLI cannot reach it.
Promote it beside the shaper.

**Acceptance criteria**

- `quill render` and `quill export` produce the same page count and the same line breaks for a
  corpus of documents — asserted directly, which is the whole increment.
- The export byte-hash is unchanged (export already used the real hyphenator); the *render* output
  changes, and the render fixtures are re-derived deliberately.
- The known-issue entry is deleted in this PR.

### 0060 last-line-measure

**A last line may not be drawn past its measure** · size: medium · branch: `feat/last-line-measure`

The other known issue, measured by spec 0048: `base_demerits` permits a last line up to
`measure + shrink` on the strength of shrink that `justify_paragraph_*` never applies to it, so it
is drawn at natural width — 126 pt in a 120 pt frame, in the case that found it.

The fix is one line. **The increment is the blast radius**, which 0048 measured before deciding not
to absorb it: it moves line breaking across the corpus, so spec 0051's equivalence digest must be
re-derived from the pre-change breaker; and it makes stat-block sections tall enough to stop fitting
a narrow `rulebook` column, which sends a block down spec 0046's uncuttable path and off the page.

**Acceptance criteria**

- No line, including a last line, is drawn wider than its own measure — asserted over a corpus.
- 0051's equivalence digest is re-derived by the documented procedure (from the pre-change breaker,
  then confirmed against the post-change one), and the spec records both values.
- 0046's narrow-column test still passes, or the section-fitting behaviour is adjusted deliberately
  and said so.
- The known-issue entry is deleted in this PR.

### 0061 gallery-and-guide

**A starter gallery and the pack authoring guide** · size: small · branch: `feat/gallery-and-guide`

**Acceptance criteria**

- `docs/` gains a pack authoring guide whose every example is parsed by a test — the anti-drift
  precedent specs 0030, 0043 and 0053 all use.
- At least two packs ship as worked examples, each installing and laying out under 0056.
- The guide states the executable-extension decision above and why, so the next person to want one
  finds the reasoning rather than re-deriving it.

## M5 increments

M0–M3 built the press pipeline and the engine. M4 made a look shareable. M5 is about what the look
can *contain* — and it exists in this form because of a positioning decision taken on 2026-07-28 and
recorded here rather than left implied by the increments.

### The decision this milestone turns on

**Quill is a general-purpose desktop publishing application first, and a TTRPG publishing
application second.** Illustrated game books stay the flagship use case: the audience the product is
designed for, the corpus its fixtures come from, and the reason its POD presets exist. But every
*mechanism* must be a general one that a game book happens to use, never a mechanism only a game
book can use. A stat block is a panelled multi-section record; a random table is a range-lookup
table; a rulebook template is a two-column reference-book template. Each of those is a thing a
cookbook, a field guide, a hardware manual or a thesis wants on exactly the same terms.

The reason is not modesty about the niche. **The niche-shaped mechanism is also the worse mechanism
for the niche** — which M4 has just demonstrated at first hand. `StatBlock` was a Rust struct with
six fixed fields, so a game system whose creatures are set differently could not be expressed at
all; spec 0054 turned it into a declaration and the bundled shapes came out byte-identical. The
generalisation is what made it usable by the audience it was built for. This milestone applies that
same test to what M4 left.

### What the audit found

Two read-only audits ran on 2026-07-28: one inventorying TTRPG coupling, one measuring quill against
the capability set of a general DTP application (InDesign, Affinity Publisher, Scribus). Their
results set this milestone's contents, so the load-bearing findings are recorded here rather than
left in a session transcript.

**The vocabulary now lags the mechanism.** M4 generalised the *behaviour* — a component is a
declaration, and `examples/packs/` ships two — while the names it is expressed in still describe one
genre: a crate called `components-ttrpg`, a `StatBlock` type, `statblock-*` style keys, template
slugs called `rulebook` and `playtest`, and a CLI that announces itself as "Quill TTRPG desktop
publishing". Type-level coupling is confined to one crate, two enum variants, one import fence and
three template slugs; only two surfaces are gated by `FORMAT_VERSION` 3. It is a rename, and it is
worth doing before the next feature adds call sites to every surface it touches.

**One capability gap disqualifies the general claim on its own: quill cannot bold a word.**
`Block::Body` and `Block::Heading` each carry a single `String` and a single `Color`
(crates/core-model/src/lib.rs). There is no styled run, no span, no inline formatting of any kind
anywhere in the workspace, and the markdown importer refuses emphasis on purpose
(crates/core-model/src/import.rs) because there is nothing to import it *into*. Nearly every other
absent typographic feature is downstream of that one: character styles, drop caps, small caps,
OpenType feature control, tracking, baseline shift and inline notes are all properties of a *run*,
and the model has no runs. M4 shipped a mechanism for sharing a look before the look could include
an italic; M5 closes that, and it is the largest thing standing between quill and the claim its
README makes.

**The rest of the gaps sort cleanly into two later milestones** — the long document (sections and
folios, running heads, footnotes, cross-references, an index, a multi-document book) and graphics
and colour (image-format breadth, fitting and transforms, anchored objects and runaround, spot
colours, vector primitives). Those are sequenced under "Beyond M5" rather than decomposed now,
because a decomposition written two milestones ahead is a decomposition that will be wrong.

**What the audit did *not* find is worth recording too.** The press pipeline, incremental layout,
threading, multi-column, master pages, generated contents, fragmentation, preflight, declared
components and the baseline grid are all real, tested and byte-hash-guarded. The engine is not the
weak half; the content model is.

### What this milestone is not

M5 does not build the direct-manipulation authoring surface — move, resize, rotate, group, guides,
snap, undo/redo. The `app` crate opens a document, scrolls it and edits the text of a block, and a
WYSIWYG object editor is a milestone in its own right rather than an increment inside one. It is
named under "Beyond M5" so its absence is a decision. The content model has to carry inline
formatting before an editor can offer it, which is the same ordering argument as everything else
here.

| # | Spec | Increment | Size |
|---|---|---|---|
| 1 | 0062 | [The neutral core — a mechanism is general or it is a bug](#0062-neutral-core) | medium |
| 2 | 0063 | [Inline runs — the paragraph stops being a `String`](#0063-inline-runs) | large |
| 3 | 0064 | [Character styles — a named run treatment, as there is a named paragraph treatment](#0064-character-styles) | medium |
| 4 | 0065 | [Lists — bullets, numbering, and the counter that survives repagination](#0065-lists) | medium |
| 5 | 0066 | [Tab stops and leaders](#0066-tabs-and-leaders) | medium |

## M5 sequencing rationale

**0062 first, because renaming after the fact is a bigger diff than renaming before it.** Every
increment below adds call sites to the surfaces 0062 renames; doing it last would mean renaming them
twice. It is also the increment that carries no risk — it must not move a single byte of output —
which makes it the right one to establish the milestone's regression discipline on.

**0063 → 0064, the run model and what names it.** 0063 is the milestone's spine and its only format
break (`FORMAT_VERSION` 4). It changes the type every other crate reads: shaping, measurement,
justification, the PDF writer, the screen renderer, the component interpreter and the importer all
consume block text today as one string. The rule that makes it tractable is that a paragraph of one
run must lay out *byte-identically* to the same paragraph as a string — the generalisation is proven
by the absence of a diff, exactly as spec 0044's splitting mechanism and spec 0054's component
interpreter both were. 0064 then gives runs named, reusable treatments, which is what turns "bold
this word" into "this is a `lead-in`", and is the character-level twin of spec 0028's paragraph
styles.

**0065 → 0066, the paragraph features that need runs to exist.** A list marker is a run in a
different style at a tab position; a leader is a run repeated to fill a measure. Building either
before 0063 would mean building it twice. They are last because they are the two most visible
remaining holes in what a publisher can type, and because each is small once the run model is there.

**Cross-cutting: every increment carries the export byte-hash bullet**, as M1–M4 did. Only 0063
legitimately moves it, and it states *what* moved and proves it was only that — the discipline spec
0038 established and spec 0042 extended to structural change.

**Cross-cutting: everything here must compose with what M4 shipped.** A declared component's
sections are paragraphs, so they get runs, character styles, lists and tabs for free — or they do
not, and that is a bug in this milestone rather than in M4. Each increment asserts against a packed
component as well as a bundled one, which is the same posture spec 0054 took toward preflight.

## M5 increment detail

### 0062 neutral-core

**A mechanism is general or it is a bug** · size: medium · branch: `feat/neutral-core`

Turn the audit's coupling inventory into names that match what the code actually does. A rename and
a re-documentation, not a redesign — M4 already did the redesign.

- Crate `quill-components-ttrpg` → `quill-components`; its crate doc and `def.rs`'s describe
  portable content components rather than TTRPG ones.
- `StatBlock` → `Panel`; `RandomTable` → `RangeTable` with `die` → `max`; `TableEntry` →
  `RangeEntry` with `result` → `value`; `Table::from_random` → `from_range_table`.
- `RangeTable` gains a `label` (defaulted) for the heading its column 0 is given, replacing the
  hard-coded `d{die}`. A roll table sets `label: "d100"` — the genre becomes *content*, which is the
  whole test this increment applies.
- `measure_stat_block` → `measure_panel`; `STATBLOCK_*` constants → `PANEL_*`, values unchanged.
- Template slugs `rulebook` → `reference`, `adventure` → `digest`, `playtest` → `draft`, each
  keeping its old slug as a permanent resolving alias.
- The CLI's about-line, and `README.md`/`CLAUDE.md`, state the positioning above.

**Acceptance criteria**

- **`Document::sample()`'s export byte-hash is unchanged.** This increment renames things; if the
  hash moves, something other than a name changed. This is the criterion the whole increment stands
  on.
- `FORMAT_VERSION` stays 3. The `"kind": "stat_block"` serde tag, its `"stat"` field and the three
  `statblock-*` style keys are **not** renamed, and a test pins the wire form. They are on-disk
  contract with a known expiry — the declared-component migration that retires `Block::StatBlock`
  outright — and a migration to a name that is itself scheduled for removal is one nobody should
  write.
- Every old template slug resolves to its renamed template, asserted per slug.
- Both `:::panel` and `:::statblock` import to the same document, asserted by equality of the two
  parsed documents. The old spelling is retained permanently rather than deprecated with a date:
  there is no released version to remove it in, and a published authoring syntax that silently stops
  parsing is the failure this repo exists to avoid.
- No file under `crates/` contains the string `ttrpg` (case-insensitive), asserted by a test that
  walks the tree — the anti-drift precedent specs 0030, 0043 and 0053 use for documentation.

**Deliberately out of scope: the fixtures.** `Document::sample()`'s text and the `testdoc` word bank
keep their content. Both are fixtures, not mechanisms. `Document::sample()` is the anchor every
export byte-hash assertion in the workspace has been measured against since spec 0001, and spending
that anchor for a cosmetic re-wording — in the one increment whose entire claim is that it moved
nothing — is a bad trade. The word bank is calibrated on word-length distribution, so changing its
vocabulary moves line breaking and every number in `benches/budgets.toml`. Genre-flavoured fixture
content is content; the test this milestone applies is about mechanism.

### 0063 inline-runs

**The paragraph stops being a `String`** · size: large · branch: `feat/inline-runs`

`Block::Body` and `Block::Heading` carry `text: String` and one `Color`. They gain instead an
ordered `Vec<Run>`, where a `Run` is a string plus an optional set of *inline overrides*: weight,
style (italic), size, colour, tracking, and baseline shift. A run with no overrides is exactly the
paragraph style resolved for the block, which is what makes the no-diff criterion below reachable.

The break is real and gets a version: `FORMAT_VERSION` 4, with a v3 → v4 load migration that turns
`{"text": "…", "color": …}` into a single run. A v3 document loaded, migrated, laid out and exported
must produce **byte-identical** output to the same document under the v3 code — that is the whole
proof that the run model is a generalisation and not a rewrite.

Shaping is where the cost lands. `quill-fonts` shapes a string against one face; a mixed-weight
paragraph is several shaping calls whose results must be concatenated with correct advances, and
Knuth-Plass must break across a run boundary as if it were not there. The measurement cache
(spec 0031) keys on block content, so its fingerprint grows a run dimension.

**Acceptance criteria**

- Regression: `Document::sample()`'s export byte-hash *moves only by its identifiers* — the sample is
  unchanged content in a new format version, so the document `/ID` and XMP identifiers move and
  nothing else. Proven by the spec 0038 procedure: export against the committed parity ICC before
  and after, `cmp -l`, and confirm every differing offset falls inside the XMP `DocumentID`/
  `InstanceID` or the trailer `/ID`, with length unchanged.
- **A single-run paragraph lays out byte-identically to the same paragraph as a string** — asserted
  over the full fixture corpus at the `PlacedBlock` level, not just the exported bytes.
- A v3 `.tpub` loads, migrates and exports byte-identically to what the v3 code produced from it. A
  v4 file loaded by v3 code fails with the typed newer-version error spec 0025 defines.
- A paragraph mixing regular and bold shapes correctly across the boundary: the advance at the join
  is the sum of the two runs' advances, kerning is not applied across faces (they are different
  fonts; a cross-face kern pair does not exist), and a rendered line is attached to the PR.
- Line breaking is unchanged by run structure: the same paragraph split into three runs at arbitrary
  points breaks at the same places, to 0.01 pt, as the one-run form.
- Hyphenation crosses a run boundary correctly — a word split across two runs is one word to the
  hyphenator, or the spec states and tests the opposite rule.
- A run whose overrides name a font variant the family does not have falls back to the nearest
  available and says so once per export on stderr, rather than silently setting it regular. Visible
  failure over silent wrongness, per `CLAUDE.md`.
- Preflight (0050) and ink coverage run over every run's colour, not just the block's — asserted with
  a paragraph whose fourth run is over the ink limit.
- A **declared component** (0054) whose section text carries runs measures and splits correctly, and
  a packed component from `examples/packs/` is in the corpus.
- The baseline grid (0058) is unmoved by run structure: a gridded line's position is set by its own
  leading, not by its tallest run, and a mixed-size paragraph on a grid is asserted.
- `quill import` gains `**bold**` and `*italic*`, which the importer previously refused for want of a
  target; its "six constructs completely" posture is updated in the same PR.
- `benches/budgets.toml`: shaping cost for a single-run paragraph unchanged; a documented,
  proportionate budget for the mixed-run case. `incremental_blocks_measured` unchanged.

**Risks** — This is the largest change to `core-model` since the format existed and it touches every
downstream crate. The byte-identity criteria are the only thing that will catch a subtle drift in
advances or breaking, and they must be asserted over the corpus that exercises justification,
hyphenation, fragmentation, the grid and the component interpreter — not a simple case.

### 0064 character-styles

**A named run treatment, as there is a named paragraph treatment** · size: medium · branch:
`feat/character-styles`

Spec 0028's argument, one level down: changing "every lead-in in the book" has to be one edit. The
stylesheet gains `character: BTreeMap<String, CharacterStyle>`; a run names a style, an override, or
both, with the override winning field by field.

**Acceptance criteria**

- A run naming a character style resolves it; a run naming one that does not exist still lays out
  with the paragraph's treatment — the authoring-posture fallback specs 0028 and 0054 both take.
- Precedence is asserted exhaustively: paragraph style < character style < inline override, field by
  field, with a test per field.
- Editing a character style reflows only the blocks that use it — the spec 0031 dependency claim,
  asserted with a work counter, not a timing.
- A document that names no character style exports byte-identically to before this increment.
- Built-in character styles ship as the stylesheet's defaults (`emphasis`, `strong`, `code`,
  `lead-in`) and are the ones the importer resolves to, so an imported document is styled rather
  than overridden.
- A content pack (0055) can carry character styles, and `quill pack extract` (0057) extracts them —
  otherwise a pack's "look" is missing the half this milestone added.

### 0065 lists

**Bullets, numbering, and the counter that survives repagination** · size: medium · branch:
`feat/lists`

The importer refuses lists today (`crates/core-model/src/import.rs`) because the model has none. A
list is a paragraph property, not a block type: a `list: Option<ListSpec>` on the paragraph style
carrying a marker (a bullet glyph, or a number format and a start), an indent pair reusing spec
0048's `Indent`, and a level.

The hard part is the counter. Numbering must be derived from the document order of the blocks, never
accumulated during pagination — spec 0041's lesson, and for the same reason: an incremental pass
reuses whole pages, so anything counted while placing goes missing exactly when the document was
just edited.

**Acceptance criteria**

- An ordered list numbers 1..n in document order; inserting an item at the top renumbers everything
  below it, asserted after an *incremental* re-layout, not a cold one.
- A list that breaks across a frame continues its numbering on the next frame (spec 0044's
  fragmentation), and the continuation's first marker is right.
- Nested levels number independently and indent cumulatively; number formats cover decimal, lower and
  upper alpha, and lower and upper roman, each asserted at a boundary value (i/iv/ix, y/z/aa).
- The marker is drawn at the hanging indent's origin, so wrapped text lines up under the text and not
  under the marker — which is what 0048's `Indent::hanging` already does.
- A list inside a declared component's section lays out and splits at the same boundaries it would
  outside one.
- `quill import` maps `-` and `1.` markdown lists to it, replacing the "kept as body text" fallback.
- A document with no list exports byte-identically to before this increment.

### 0066 tabs-and-leaders

**Tab stops and leaders** · size: medium · branch: `feat/tabs-and-leaders`

A paragraph style gains an ordered list of tab stops, each with a position, an alignment (left,
centre, right, decimal) and an optional leader string. A `\t` in a run advances to the next stop.
This is what sets a contents entry's page number against the right margin with dots between, a price
list, a bibliography or a specification sheet — and quill currently draws its generated contents
(spec 0041) without one.

**Acceptance criteria**

- Each alignment is asserted to 0.01 pt: left/centre/right against the stop, decimal against the
  decimal separator's position, with a locale-independent rule for what the separator is.
- A leader fills the gap with a whole number of repetitions, clipped to the gap, and never overlaps
  the text on either side.
- Text that overruns its stop goes to the next stop rather than overlapping — the standard rule,
  asserted.
- Justification and tabs compose: a justified line containing a tab does not stretch the tabbed gap,
  because a tab is a hard position and not a space.
- The generated contents list (0041) is re-expressed on a right tab with a dot leader, and the change
  to its rendered output is shown in the PR.
- A document with no tab stop exports byte-identically to before this increment.

## Known issues

Found by the work, not yet fixed. Recorded so they are decided on rather than forgotten. An entry
whose increment ships is deleted in that increment's PR, not left here as a fixed-but-still-listed
defect — which is why the four entries M3 opened with are gone, and why both entries M3 *found* are
gone too: specs 0059 and 0060 shipped them. The entry below is one M4 found in their place.

- **A stat-block section taller than its column cannot be cut, so the panel overflows the page.**
  Spec 0046 cuts a composite *between sections and nowhere else*, so a single section — a long
  actions list — that is itself taller than a frame has no legal cut. The panel is placed whole and
  runs off the bottom. Content is never lost, which is asserted; it is placed badly, which is also
  asserted (`a_section_taller_than_its_column_is_placed_whole`).

  Not new, and not spec 0060's doing — but 0060 moved the threshold, so it is now reachable at a
  quarter of the section count it used to take. Measured on both builds with the same fixture in a
  `rulebook` column: 24 sections fitted and 26 overflowed before; 8 fits and 10 overflows now. The
  cause is that forbidding an over-measure ragged line makes each run in a narrow panel wrap to two
  lines instead of one.

  The fix is the open question below — per-section paragraph splitting inside a composite. Spec
  0044's `\vsplit` mechanism already exists; wiring it through the composite so a cut may fall
  *inside* a section when no section boundary fits is the part that does not. Until then the
  limitation is written down and asserted rather than rediscovered as a bug, and the test that pins
  it says in as many words that it inverts when the fix ships.

## Open questions

Deliberately unresolved; each would change work if answered differently. Recorded so they are
decided explicitly rather than by accident.

- Should the 500-page synthetic document target page count by measurement (robust: stays ~500 when leading, margins or hyphenation change, but the workload silently changes size) or fix the block count (stable workload, drifting page count)? Spec 0027 assumes measure-to-target and the *benches* still do. **Answered for the line-breaking equivalence corpus by spec 0060**, which had to pin it by block count before it could prove anything: a corpus sized by laying a document out moves whenever the thing it is testing moves, and the two are then indistinguishable. Whether the benches should follow is still open — they measure throughput, where a workload that tracks a page target is arguably the point.
- Does the CI perf gate assert any wall-clock at all, or only work counters plus same-run ratios? The plan uses ratios and a 2x blowup ceiling; a stricter gate would need self-hosted or pinned runners.
- Deferred by design: `text-layout::Line` still carries no glyph ids or positions, so spec 0033's renderer re-shapes each line through the shared `quill-fonts` shaper and derives word positions from `space_adjust_pt`, exactly as `writer::render_page` does for `TJ`. That keeps one shaper but two derivation sites. Is that acceptable through M1, or should `text-layout` emit positioned glyph runs (spec 0016's still-open named non-goal, including shaping-GID ↔ subset-GID reconciliation) before the app shell ships?
- Is per-page master assignment by **index** (spec 0035) the right anchor, or should a master be attached to the chapter it opens? Index-based assignment means a TOC that grows by a page slides every chapter opener by one (spec 0041 names this). Anchoring to a heading's `BlockId` would survive repagination but needs a notion of "section" the model does not have. M2 ships index-based; M3 should decide whether that survives contact with a real book.
- Should a `PodPreset` ever be recorded *in* a `.tpub`? Spec 0049 says no — a document is not bound
  to one printer, and a persisted preset would go stale inside a file nobody re-opens. The cost is
  that the vendor a book was built for is not recoverable from the book. If that turns out to matter
  in practice, the answer is a non-authoritative hint field, not a binding.
- Does per-section paragraph splitting inside a stat block ever ship? Spec 0046 breaks between
  sections only, so a stat block whose single `actions` section overflows a frame still overflows.
  The mechanism to fix it exists after 0044; wiring it through the composite is the open part.

Answered by M2's decomposition, kept here as pointers: master statics are pre-placed geometry that
resolves a style name (moved to the decisions log), and `PageSetup::default()` keeps zero margins
permanently — spec 0036 gives templates real margins instead, so the on-ramp never starts at the
trim edge while the CI golden path never moves.

Answered by M3's decomposition: **stat blocks, tables and paragraphs share one splitting mechanism**
— spec 0044 builds it as a `\vsplit` over an already-measured vertical list, so the measurement
cache gains no height dimension, and 0045/0046 consume it.

**Closed by spec 0052 (shipped): clickable internal links ship in a second export profile.** No
longer an open question in either direction. `ExportOptions::profile` selects `Press` (the default,
byte-identical to what shipped before) or `Screen`, which carries `/Link` annotations for contents
entries and claims no PDF/X conformance. The press file is *provably* annotation-free — asserted by
parsing the emitted PDF for any `/Annots` key over a document that really does produce link
candidates — because the writer gates every annotation on spec 0042's `annotation_finding` rather
than on a per-profile branch. `Screen` is deliberately not an RGB profile, and does not grow
compression, downsampling or reader spreads; see the non-goals in `specs/0052-screen-profile.md`.

## Beyond M3

M3 closed on 2026-07-28. What it shipped, and what it left:

- **The splitting mechanism was the milestone's spine.** Three increments had wanted it; it was built
  once (0044) as a `\vsplit` over an already-measured vertical list, so the measurement cache gained
  no height dimension, and tables (0045) and stat blocks (0046) then cost little. The rule that
  bounds it — a fragment is non-empty and the absolute offset strictly increases — is what makes the
  flow loop terminate, and it turned out that emptiness of the frame never was.
- **Two perf results were larger than planned.** 0051's pruning was expected to close a superlinear
  cliff; it also revealed the DP was cloning a growing path vector into every candidate, so the true
  exponent was ~2.6. One 20,000-word paragraph went from 49.7 s to 4.2 ms, and full-document layout
  from 0.31 to 0.073 ms/page.
- **Two budgets stopped meaning what they said, and were repaired rather than raised.**
  `incremental_pages_reflowed` was measuring looseness, not efficiency, once columns packed tight;
  `incremental_blocks_measured` now carries the M1 claim. And a counter re-baselined to 260 with the
  file's 2× *timing* tolerance had a failure threshold above every value it can physically take —
  work counters are now checked exactly.
- **Three numbers were deliberately not invented**: the vendor presets' figures, their trim
  catalogues, and `generic`'s safety margin. Each is a named slot with its provenance recorded, and
  the CLI says so out loud. Filling them from the printers' current specifications is the obvious
  next non-code task.

## Beyond M4

M4 closed on 2026-07-28. What it shipped, and what it left:

- **The generalization held exactly.** 0054's whole risk was subtle geometric drift, and the
  byte-identical criterion caught nothing because there was nothing to catch: the two bundled
  components, re-expressed as declarations, produce the same placed geometry over a ten-case corpus.
  What the criterion *did* buy was the confidence to route both through one interpreter and delete
  three hundred lines of hand-written measurement.
- **The corpus assertion found more than the known issue did.** 0060 was scoped to last lines. The
  test written for it found that every *ragged* line had the same defect, for the same reason — and
  that is the case that matters, since stat blocks, table cells, headings and contents entries are
  all ragged. A known issue described the symptom someone happened to hit; the assertion described
  the rule.
- **The equivalence corpus was measuring the wrong thing.** It was sized by laying a document out to
  ~500 pages, so a deliberate line-breaking change moved the corpus and the breaking at once, with no
  way to tell them apart. Pinned by block count now. This was not visible until a change came along
  that was *meant* to move the digest.
- **The baseline grid did not need the blast radius the plan budgeted for.** The roadmap expected
  0058 to move every fixture in the repo and require 0051's re-derivation discipline. Making the
  grid opt-in avoided all of it, and cost nothing a publisher wanted — a grid is a design choice, and
  a document that does not ask for one has no reason to move.
- **One number was deliberately not invented**: whether the bundled templates should opt in to a
  baseline grid. That is a decision to make with a designer's eye, not as a side effect of building
  the mechanism.

M4's two deferred M3 items — the baseline grid and the two known issues — were sequenced into it
rather than left floating, because a milestone that ends with its own findings unfixed teaches the
wrong thing. All three shipped. The one known issue now open is one M4 found in their place.

Two things M3 deliberately did *not* attempt, recorded so their absence is a decision:

- **A baseline grid.** `CLAUDE.md` names it as a `layout-engine` responsibility and nothing
  implements it; specs 0019, 0020 and 0028 each deferred it explicitly. It interacts with leading,
  with fragmentation (0044) and with indents (0048), so it is cheaper after all three than beside
  any of them — which is why it is spec 0058 rather than an M3 increment.
- **The M0 manual item.** A real POD upload validated with a B2A-equipped CMYK profile is not
  automatable and is not an increment; 0049's presets narrow what that upload has to prove, but do
  not replace it.

### After M5

The two milestones after M5 are named here with their contents, and deliberately **not** decomposed
into specs. A decomposition written two milestones ahead is one that will be wrong; what belongs
here is the ordering argument and the list of things not to forget.

**M6 — the long document.** The apparatus a book needs beyond its typography. Every item is a
fixpoint over laid-out pages, and spec 0041's rule governs all of them: derive from the final page
vector, never accumulate during pagination.

- **Sections with independent page numbering and folio formats.** Roman front matter, arabic body,
  restart at a part opener. `PageOverride`'s own doc comment names the gap: the model has no notion
  of a section. This also answers the open question about whether master assignment by index
  survives contact with a real book — a section is the anchor that index-based assignment lacks.
- **Running headers derived from content.** `MasterStatic::Text` resolves `{page}` and nothing else,
  so a chapter title in a running head must be typed onto every master. A `{section}` /
  `{heading:1}` variable, resolved from spec 0040's heading index, is the same machinery.
- **Footnotes and endnotes.** A second flow with an anchor in the first, and the only feature in this
  list that changes the flow loop rather than reading its output: a footnote reduces the frame its
  reference lands in, which can push the reference to the next frame, which is a fixpoint.
- **Cross-references.** "See page 42" that survives repagination. The same dependency shape as the
  contents list, and cheap once sections exist.
- **An index.** Marked terms, collated, page-ranged and alphabetised. The one long-document feature
  with no existing analogue in the codebase.
- **A book: multiple documents, shared styles, continuous pagination.** The unit a 500-page game
  book or a textbook is actually authored in, and the natural consumer of M4's packs. Last in M6
  because it is a composition over everything before it.

**M7 — graphics and colour.** The breadth that separates "lays out text correctly" from "produces
the artefact". Ordered by how often its absence stops a real book.

- **Image-format breadth.** PNG and JPEG are the only decoders; TIFF and PSD are what art actually
  arrives as, and SVG is what a logo arrives as. Each is a decode path into the existing CMYK
  pipeline, and each must respect spec 0012's posture: a colour space that cannot be disambiguated is
  skipped loudly, never guessed.
- **Frame fitting and transforms.** One fixed rule today (scale to content width, preserve aspect).
  Fill, fit, centre and crop; rotate and scale a placed object.
- **Anchored objects and text runaround.** The pull-quote and the figure that text flows around. This
  changes the flow model rather than adding to it, which is why it is not in M5.
- **Spot colours and named swatches.** A named colour resolved at export to a separation, with ink
  coverage accounting for it. PDF/X-1a admits spot colours; quill's `Color` enum does not.
- **Vector primitives.** Lines, polygons and beziers, with the rules spec 0037's decoration primitive
  already established for rectangles.
- **Gradients, transparency and blend modes** — and the PDF/X-4 export path they imply, since
  PDF/X-1a requires flattening. Last in M7 because it is the item that changes what conformance quill
  can claim, and that decision deserves its own spec.

### Named and not scheduled

Real gaps, deliberately unscheduled, so their absence is a decision rather than an oversight:

- **The direct-manipulation authoring surface** — move, resize, rotate, group, guides, snap, layers,
  and the undo/redo that becomes architecturally invasive the moment they exist. The `app` crate
  today opens a document, scrolls it and edits the text of a block. This is a milestone in its own
  right and probably the largest one left; it is unscheduled because the content model has to carry
  inline formatting, sections and anchored objects before an editor can offer them, and that is
  M5–M7.
- **Tagged PDF / PDF-UA**, increasingly a procurement requirement, with no groundwork in the writer.
- **EPUB or HTML export, IDML interchange, imposition, separations preview, soft-proofing, data
  merge, conditional text, spell-check, bidi/RTL and CJK.** Each is real; none is on the critical
  path to a general DTP application that produces a correct press file, which is what M5–M7 are for.
