# Plan — Bleed single source of truth (spec 0013)

## Task (restated)

Preflight's `Bleed` check validates `ExportOptions::bleed_pt`, but the exported page geometry
(`geom::page_geom`) builds the `BleedBox`/`MediaBox` from `doc.page_setup.bleed_pt`. These are two
disconnected sources of truth: the CLI never wires the document's bleed into `ExportOptions` (it
builds `opts` with `..Default::default()`, so `opts.bleed_pt == 9.0` always, and there is no
`--bleed` flag). So a document whose `page_setup.bleed_pt` is below the required 9 pt exports an
**invalid, too-small BleedBox** while preflight passes it clean — the exact class of press defect
M0's preflight exists to catch. This reconciles specs 0001 (which modeled bleed as a preflight
option) and 0002 (which built geometry from `PageSetup`), which have drifted.

## Fix

Make `doc.page_setup.bleed_pt` the single source of truth:
- Preflight validates `doc.page_setup.bleed_pt` (the value geometry actually writes), not
  `opts.bleed_pt`.
- Remove the redundant `ExportOptions::bleed_pt` field (read nowhere except the old, wrong check;
  geometry already ignores it). No CLI flag referenced it.

## Acceptance criteria

- A `Document` with `page_setup.bleed_pt` below `DEFAULT_BLEED_PT` (e.g. 2.0) → preflight produces a
  `CheckId::Bleed` **error** and `export` refuses (unless `force`).
- A `Document` with `page_setup.bleed_pt >= DEFAULT_BLEED_PT` → no `Bleed` finding; sample still
  passes preflight.
- The `Bleed` finding message reports the document's bleed value.
- `ExportOptions` no longer has a `bleed_pt` field; the CLI and all constructors still compile
  (`..Default::default()` unaffected).
- Regression: existing suite stays green; export bytes for the sample unchanged (sample bleed = 9.0,
  same geometry).

## Files to touch

- `specs/0013-bleed-single-source.md` (new), `specs/README.md` (index row).
- `crates/export-pdf/src/lib.rs` — drop `ExportOptions::bleed_pt` (struct + `Default`); change the
  `Bleed` check to read `doc.page_setup.bleed_pt`; update the `ExportOptions` doc comment. Add tests.
- `specs/0001-pdf-x-export.md` — update the stale input-model line that lists `bleed_pt` as a
  preflight option to point at `page_setup.bleed_pt` (one-line reconciliation note).

## Test strategy

- `preflight` unit tests: insufficient-bleed document → `CheckId::Bleed` error; adequate-bleed
  document → none; `export` refuses the insufficient-bleed document without `force`.
- Existing geometry tests (`geom.rs`) are unchanged (they already use `PageSetup.bleed_pt`).

## Non-goals

- A per-export bleed override / CLI `--bleed` flag (not needed for M0; can be re-added deliberately
  later if a real use case appears).
- Per-edge or variable bleed; POD-preset bleeds (M3).
