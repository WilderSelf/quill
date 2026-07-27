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
| **M2** | Beginner on-ramp — templates, stat blocks, TOC | not started |
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

## Sequencing rationale

The order is forced by three hard dependency chains that the surveys make explicit.

**Chain 1 — the model must settle before anything keys on it.** `Document::from_json` performs no version check at all (crates/core-model/src/lib.rs:190) while docs/format-spec.md:39-42 promises reject-newer/migrate-older. Every increment from 0026 on changes the serialized shape, so the version gate has to exist before the first change or v1 files become undiagnosable. It ships with the `.tpub` container (0025) because the container is what supplies the asset root that both `export-pdf` (hardcoded `Path::new(".")` at writer.rs:61) and `ProxyCache::populate_from_assets` need, and because the container is where every later schema addition lands. Then stable `BlockId`s (0026): incremental layout has literally nothing to key on today — blocks are addressed by index, so an insert at index 0 renumbers everything, and no core-model type derives `Eq`/`Hash` because every geometry field is `f32`. Styles (0028) come next rather than later because they become part of the measurement-cache key; retrofitting a key dimension after the cache hardens is strictly more expensive, and styles are also what stops `measure_block` from discarding `Block::Heading.level` (layout-engine/src/lib.rs:217).

**Chain 2 — the perf harness must precede the work it gates.** `cargo bench` is documented in CLAUDE.md:112 as the M1 gate but is a silent no-op; there is no `benches/` dir, no `[[bench]]`, and zero `[dev-dependencies]` workspace-wide. Landing it at position 3 means the master-page, persistence and incremental increments each have a measuring stick and a re-baseline point, instead of the milestone ending with an unverifiable performance claim. It sits after `BlockId` so the synthetic generator emits ids, and it is deliberately the *third* increment rather than the first because the generator would otherwise be rewritten twice.

**Chain 3 — parity seam, then capability, then persistence.** This is the repo's own house pattern (spec 0016 incr. 1, 0018 incr. 1, 0019 incr. 1, 0020: introduce the structural seam at byte-parity, add behavior next). So master pages split into 0029 (layout-engine-only `PageTemplate` supplier + `LaidOutPage` identity/geometry/statics, proved equal to today's output) and 0030 (the serde types, `FORMAT_VERSION` 2 and the v1→v2 migration that makes masters authorable). 0029 before 0030 keeps the large model change from also carrying an engine redesign. Incremental layout (0031) is after both, because the measurement-cache key must already contain the final set of dimensions (block id, style, frame width, metrics identity) and because per-page checkpointing has to checkpoint the master-aware flow state, not the old single-`Thread` state.

**The render track is last and self-contained.** Screen render is blocked on a fact of crate structure, not on the model: the only real `RunMetrics` is `ShapingContext` inside `export-pdf`'s private `mod fonts`, and `page_geom` is private too, so an app canvas would have to lay out with `MonospaceRunMetrics` and disagree with the exported PDF. 0032 extracts both (a pure refactor guarded by the export byte-hash), 0033 builds the paint list and rasterizer on top, and 0034 — the app shell — is last because it consumes every prior increment: the container to open a document, the model for masters and styles, the `LayoutSession` for responsive editing, `quill-fonts` for WYSIWYG text, and the paint list to draw.

**Two cross-cutting rules shaped the acceptance criteria.** First, every increment carries an explicit regression bullet asserting the `Document::sample()` export stays byte-identical (SHA-256, introduced in 0025) — because `Document::sample()` is the CI Ghostscript golden fixture and `write_pdf` hashes `doc.to_json()` into the PDF `/ID`, so a model change silently moves the golden path. Second, the incremental-layout gate is stated as deterministic work counters (`LayoutStats`, mirroring `PopulateReport`) plus same-run ratios, never absolute wall-clock: measured idle variance here is ±6% on `lay_out`, shared GitHub runners swing 10-30%, and `rust-toolchain.toml` pins the floating `stable` channel so any absolute ms ceiling drifts on every Rust release. Counters also state the actual M1 claim — "re-flow only affected pages" — far more precisely than a timer.

## Increment detail

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

## Open questions

Deliberately unresolved; each would change work if answered differently. Recorded so they are
decided explicitly rather than by accident.

- Which 3 of CI's 4 emitted check-runs does branch protection actually require? CI emits `fmt + clippy + test (ubuntu-latest)`, `(macos-latest)`, `(windows-latest)` and `PDF preflight (Ghostscript)`, but CLAUDE.md:159 says 3 contexts are required — so the required set is already a strict subset and a *new* job would not gate merges. Spec 0027 folds the bench step into the existing Linux-only job to avoid an out-of-band admin change, but that only works if that job is one of the required three. Needs `gh api repos/.../branches/main/protection` (gh CLI is not installed in this sandbox).
- Canvas backend: `tiny-skia` (MIT/Apache, pure Rust, CPU, zero native build across the three-OS matrix) vs `skia-safe` (what CLAUDE.md names; GPU, but a heavy bundled-native build on ubuntu/macos/windows) vs `vello`/`wgpu`. render/Cargo.toml:9 still says "evaluating vello", so this is genuinely undecided. The plan proposes tiny-skia behind the paint-list seam so a GPU backend can swap in later — but CLAUDE.md explicitly says screen rendering uses Skia, so this needs a human ruling.
- `BlockId` representation: a `u64` newtype (compact, cheap `Hash`, ideal cache key) or a stable string id (human-readable in the JSON manifest, better for the stated git-diffability goal of the text manifest). This choice is baked into every cache key and every frame→content reference from spec 0026 onward.
- Should `.tpub` be read in place (streaming out of the zip) or extracted to a working directory on open? This determines what `asset_root` actually is, whether edits are staged in a temp tree, and what "save" means (rewrite the zip vs. incremental update). It also decides whether the proxy cache can be persisted alongside the extracted assets.
- Are master-page statics (running heads, folios, background art) semantic `Block`s — styled, reflowable, participating in the style system — or pre-placed geometry-only `PlacedBlock`s? Spec 0030's `{page}` token resolution and the styling of running heads both hang on this.
- Should the 500-page synthetic document target page count by measurement (robust: stays ~500 when leading, margins or hyphenation change, but the workload silently changes size) or fix the block count (stable workload, drifting page count)? Spec 0027 assumes measure-to-target; the alternative makes bench numbers comparable across increments that change layout.
- Does the CI perf gate assert any wall-clock at all, or only work counters plus same-run ratios? The plan uses ratios and a 2x blowup ceiling; a stricter gate would need self-hosted or pinned runners.
- Deferred by design: `text-layout::Line` still carries no glyph ids or positions, so spec 0033's renderer re-shapes each line through the shared `quill-fonts` shaper and derives word positions from `space_adjust_pt`, exactly as `writer::render_page` does for `TJ`. That keeps one shaper but two derivation sites. Is that acceptable through M1, or should `text-layout` emit positioned glyph runs (spec 0016's still-open named non-goal, including shaping-GID ↔ subset-GID reconciliation) before the app shell ships?
- Does `PageSetup::default()` eventually gain non-zero margins? Every increment here keeps it at zero so `Document::sample()` and the CI Ghostscript golden path never move — which means the shipped default still lets text bleed to the trim edge. Changing it is an M2 template concern but should be an explicit decision, not an oversight.

## Beyond M1

M2–M4 are not yet decomposed into increments. They are sketched in `CLAUDE.md` and will be
sequenced here once M1 closes, following the same rule: a spec per feature, ordered by dependency,
each independently shippable.
