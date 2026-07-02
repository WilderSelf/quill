# 0013 — Bleed single source of truth

- **Milestone:** M0
- **Status:** implemented
- **Crates:** `quill-export-pdf` (preflight + geometry), `quill-core-model` (owns `PageSetup.bleed_pt`)

## Goal

Make the exported page geometry and the preflight `Bleed` check read the **same** bleed value —
`doc.page_setup.bleed_pt` — so preflight actually validates the `BleedBox` that export writes. Remove
the redundant `ExportOptions::bleed_pt`, which created a second, disconnected source of truth.

## Background / why

Specs 0001 and 0002 drifted. Spec 0001 modeled bleed as a **preflight option**
(`ExportOptions.bleed_pt`, default 9 pt); spec 0002 built the per-page `MediaBox`/`BleedBox` from
`PageSetup` (`geom::page_geom` reads `setup.bleed_pt`). Nothing wires the document's page-setup bleed
into `ExportOptions`: the CLI constructs `opts` with `..Default::default()`, so `opts.bleed_pt` is
**always** the 9 pt default, and there is no `--bleed` flag. `opts.bleed_pt` is read in exactly one
place — the preflight `Bleed` check — and is ignored by the geometry that produces real output.

The consequence is a silent press defect: a document whose `page_setup.bleed_pt` is below the
required 9 pt (0.125 in) exports a **too-small `BleedBox`** — art within the intended bleed zone is
lost at trim — while preflight checks the phantom `opts.bleed_pt` (9 pt) and reports the document
**clean**. Preflight exists precisely to reject press-invalid geometry before export; here it
validated a value that had no effect on the file.

## Approach

`doc.page_setup.bleed_pt` is the single source of truth. The `Bleed` check validates it; geometry
already uses it (`geom::page_geom`). The `ExportOptions::bleed_pt` field is removed — it is read
nowhere else and only enabled the divergence.

## Hard requirements

1. **Preflight validates the geometry's bleed.** The `Bleed` check reads `doc.page_setup.bleed_pt`
   (the exact value `geom::page_geom` writes into the `BleedBox`), comparing it against
   `DEFAULT_BLEED_PT` (9 pt) with the existing `f32::EPSILON` tolerance. Below → `CheckId::Bleed`
   **error**; the message reports the document's bleed value.
2. **Single source of truth.** `ExportOptions` no longer has a `bleed_pt` field. No code path reads a
   bleed value from anywhere but `doc.page_setup`.
3. **Export gate honored.** With preflight now reading the real bleed, `export` refuses a document
   whose `page_setup.bleed_pt` is below 9 pt (unless `force`), consistent with every other error-level
   check.
4. **No behavior change for valid documents.** The sample (`bleed_pt == 9.0`) and every
   `page_setup.bleed_pt >= 9.0` document preflight and export exactly as before; the sample's export
   bytes are unchanged (geometry already used `page_setup.bleed_pt`).
5. **No CLI / geometry / public-model change beyond the removed field.** `PageSetup`, `Document`,
   `geom::page_geom`, `CheckId`, and the CLI surface are otherwise unchanged.

## Acceptance criteria

- **`preflight` unit:** a document with `page_setup.bleed_pt = 2.0` yields a `CheckId::Bleed` error
  and `report.passed() == false`; a document with `page_setup.bleed_pt = 9.0` (the sample) yields no
  `Bleed` finding.
- **Export-level:** `export` of the 2 pt-bleed document without `force` returns
  `ExportError::PreflightFailed` and writes nothing; with adequate bleed it succeeds.
- **Regression:** the existing suite stays green; the sample export is byte-identical to before this
  change (bleed source was already `page_setup` in geometry).

## Non-goals

- A per-export bleed override or CLI `--bleed` flag — not needed for M0; can be added deliberately
  later if a real use case appears.
- Per-edge / variable bleed and POD-preset bleeds (M3 POD presets).
