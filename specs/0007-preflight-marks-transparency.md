# 0007 — Preflight: Marks & Transparency checks

- **Milestone:** M0
- **Status:** implemented
- **Crates:** `quill-export-pdf` (owner), `quill-core-model`

## Goal

Complete the **1:1 mapping between preflight checks and spec 0001's nine hard requirements**.
[Spec 0001](0001-pdf-x-export.md) states that preflight checks map 1:1 to its requirements and
lists the ids `ColorSpace, FontEmbedding, Bleed, ImageResolution, InkCoverage, Marks,
OutputIntent, Transparency` — but the shipped `CheckId` enum omitted **`Marks`** (req #7) and
**`Transparency`** (req #9). This spec adds those two checks so preflight reports on every
requirement it claims to cover.

## Background / why

`Marks` and `Transparency` are the two requirements Quill satisfies *by construction* today: the
writer emits no crop/printer/registration marks, and `images.rs` drops image alpha to keep the
"no live transparency" invariant. Both are correct — but preflight was silent about them, so its
report understated its own coverage. This is an integrity fix on already-shipped work, not new
export behavior.

## Behavior

### `Marks` (req #7 — no crop/printer/registration marks)

A **structural invariant**. Quill's PDF writer emits no marks, and the document model has no field
that could request them. The check therefore **never produces a finding** for any document; it
exists to give the requirement a named `CheckId` that tests and other layers can reference, and to
complete the 1:1 mapping. It has **no failing input by design** — that is correct, not a stub.

### `Transparency` (req #9 — live transparency flattened)

The only way transparency can enter a Quill document is an image **alpha channel**. Both
PDF/X-1a:2001 and PDF/X-3:2002 forbid live transparency, so export flattens it (alpha is dropped).
Because the flattened output is still conformant, an asset that declares an alpha channel produces
a **`Warning`** (not an `Error`): preflight still passes and export still succeeds, but the author
is told their transparency will be flattened. Applies to **both** conformance levels.

Transparency is detected from a declared `Asset.has_alpha` flag — the same author-declared-metadata
pattern as the existing `Asset.dpi` and `Asset.line_art`. Preflight does **not** decode image
bytes; it stays file-free and independently testable.

## Public surface (delta)

```text
// quill-core-model
struct Asset { …, has_alpha: bool }   // #[serde(default)] → backward-compatible manifests

// quill-export-pdf
enum CheckId { …, Marks, Transparency }
```

## Acceptance criteria

- A document with an asset `has_alpha: true` yields a `Transparency` finding of severity
  `Warning`, and `PreflightReport::passed()` is still `true`.
- A document whose assets are all opaque yields no `Transparency` finding.
- No document yields a `Marks` finding (invariant documented by test).
- The existing "clean document passes preflight" behavior is unchanged (warnings do not fail).
- `#[serde(default)]` on `has_alpha` keeps pre-0007 `.tpub`/JSON manifests loading unchanged.

## Non-goals

- **Auto-probing** image files for an alpha channel at ingest — `has_alpha` is author-declared for
  M0, exactly like `dpi`. Populating it by decoding assets is future work.
- Any CLI flag or new dependency; `quill preflight` / `quill export` pick the checks up unchanged.
- Post-export byte-level assertion that the PDF contains no marks (preflight is pre-export and
  document-level).
