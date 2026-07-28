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
| **M2** | Beginner on-ramp — templates, stat blocks, TOC | **in progress** — nine increments, specs 0035–0043, sequenced below; 0035–0037 and 0040 shipped |
| **M3** | Pro polish + POD presets | not started |
| **M4** | Plugins / ecosystem | not started |

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
  with `quill pack` (crates/cli/src/main.rs:243-280). The shell can open and scroll it, edit the
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
termination-bound hazard spec 0031 recorded. Knuth-Plass superlinearity (see Known issues) is more
exposed here than anywhere: a stat block's `details` entries are prose and could plausibly be long.

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

## Known issues

Found by the work, not yet fixed. Recorded so they are decided on rather than forgotten.

- **Knuth-Plass line breaking is superlinear in paragraph length.** An 8× longer paragraph costs
  ~36× the time (linear would be 8×, quadratic 64×), so active-node pruning is missing or
  ineffective. Found by spec 0027's harness on its first run. Low severity in practice — real
  paragraphs are 30–90 words, ~64 µs each — but a genuine cliff for pathological input such as a
  stat block or table flattened into one very long paragraph, which this product's users plausibly
  produce. Deliberately not fixed inside 0027: changing the line breaker and the measurement in one
  commit would leave neither trustworthy. `benches/budgets.toml` pins today's value so a further
  regression is still caught.

- **A master static has no alignment and is not mirrored by page parity.** `MasterStatic::Text`
  (spec 0030) is drawn as one line starting at its rect's left edge, and the same rect is used on
  rectos and versos alike. So a running head cannot be centred or set flush to the fore-edge, and a
  folio cannot sit at the outside corner of a spread — the two most conventional placements in a
  bound book. Spec 0036's bundled templates work around it by insetting the folio to the fore-edge
  margin, which is correct but is not the design a publisher would choose. Found by rendering a
  template page: every numeric test passed while the folio printed hard against the trim.
  The fix belongs on the static (an `align`, and an `x` that resolves inside/outside like `Margins`
  already does), not on each template. Not done in 0036 because it changes a serialized 0030 type
  and would have carried a model change inside a templates increment.

- **A block never splits across frames, so a two-column page ends ragged.** The pagination loop
  moves a whole block to the next frame when it does not fit, rather than breaking the paragraph
  across the column boundary. On a single-column page this is invisible; on spec 0036's two-column
  `rulebook` template it leaves a visible gap at the foot of a column whenever the next paragraph is
  taller than the space left. Long-standing engine behavior rather than a regression — nothing in
  the workspace produced a two-column page as its default before templates existed, which is why it
  had not been seen. This is a real typographic defect for the milestone's flagship template and
  should be sequenced into M2 or M3 explicitly; it is not a template setting that can work around it.

## Open questions

Deliberately unresolved; each would change work if answered differently. Recorded so they are
decided explicitly rather than by accident.

- Should the 500-page synthetic document target page count by measurement (robust: stays ~500 when leading, margins or hyphenation change, but the workload silently changes size) or fix the block count (stable workload, drifting page count)? Spec 0027 assumes measure-to-target; the alternative makes bench numbers comparable across increments that change layout.
- Does the CI perf gate assert any wall-clock at all, or only work counters plus same-run ratios? The plan uses ratios and a 2x blowup ceiling; a stricter gate would need self-hosted or pinned runners.
- Deferred by design: `text-layout::Line` still carries no glyph ids or positions, so spec 0033's renderer re-shapes each line through the shared `quill-fonts` shaper and derives word positions from `space_adjust_pt`, exactly as `writer::render_page` does for `TJ`. That keeps one shaper but two derivation sites. Is that acceptable through M1, or should `text-layout` emit positioned glyph runs (spec 0016's still-open named non-goal, including shaping-GID ↔ subset-GID reconciliation) before the app shell ships?
- Is per-page master assignment by **index** (spec 0035) the right anchor, or should a master be attached to the chapter it opens? Index-based assignment means a TOC that grows by a page slides every chapter opener by one (spec 0041 names this). Anchoring to a heading's `BlockId` would survive repagination but needs a notion of "section" the model does not have. M2 ships index-based; M3 should decide whether that survives contact with a real book.
- Should stat blocks and tables share one splitting mechanism? Spec 0038 defines keep-together-else-split-at-a-section-boundary and 0039 defines break-between-rows-repeat-the-header. They are the same shape. If 0038's turns out to generalize, 0039 reuses it; if it does not, the repo carries two breaking rules and should say why.
- Do clickable internal links ever ship? Spec 0042 excludes them because PDF/X-1a requires annotations outside the BleedBox and a TOC entry sits mid-text-block by definition. The options are a non-PDF/X "screen PDF" export profile alongside the press one, or never. This is an M3 POD-preset question.

Answered by M2's decomposition, kept here as pointers: master statics are pre-placed geometry that
resolves a style name (moved to the decisions log), and `PageSetup::default()` keeps zero margins
permanently — spec 0036 gives templates real margins instead, so the on-ramp never starts at the
trim edge while the CI golden path never moves.

## Beyond M2

M3 (pro polish + POD presets) and M4 (plugins / ecosystem) are not yet decomposed into increments.
They are sketched in `CLAUDE.md` and will be sequenced here once M2 closes, following the same rule:
a spec per feature, ordered by dependency, each independently shippable. The open questions above
are the M3 inputs.
