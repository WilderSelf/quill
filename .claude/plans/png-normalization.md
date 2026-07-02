# Plan: normalize PNG inputs to 8-bit (spec 0010)

## Task (restated)
`decode_png` silently drops (`return None`) any PNG that is **indexed** (`ColorType::Indexed`)
or **not 8-bit** (`bit_depth != Eight`). A dropped image means "art missing, no warning" — the
worst failure mode (called out by spec 0008). Specs 0005/0006 explicitly deferred indexed/16-bit
PNG as later specs. Fix by asking the `png` decoder to normalize every input to 8-bit color:
`Decoder::set_transformations(Transformations::normalize_to_color8())` (= `EXPAND | STRIP_16`),
which expands palette→RGB(A), sub-8-bit grayscale→8-bit, tRNS→alpha, and strips 16-bit→8-bit.
All PNGs then flow through the existing Gray / RGB→CMYK(+240% clamp) paths.

## Acceptance criteria
- An **indexed** PNG decodes (palette expanded to RGB → CMYK), not dropped.
- A **16-bit** PNG decodes (stripped to 8-bit), not dropped.
- Existing 8-bit grayscale / RGB / RGBA PNG behavior is unchanged (transforms are no-ops there).
- Ink-coverage clamp (spec 0006) still applies (unchanged conversion path).
- New spec `specs/0010-png-normalization.md` (status implemented) + row in `specs/README.md`.

## Files to touch
- `crates/export-pdf/src/images.rs` — set the transform in `decode_png`; refresh module/fn doc
  comments (they currently say non-8-bit/indexed PNG is skipped); add tests + in-memory
  indexed-PNG and 16-bit-PNG encode helpers (png is already a dep → synthesize in-memory per the
  CLAUDE.md fixture convention).
- `specs/0010-png-normalization.md` — new.
- `specs/README.md` — index row.

## Test strategy
- `decodes_indexed_png_to_cmyk`: 2x1 indexed (palette white/black) → CMYK white=[0,0,0,0],
  black=[0,0,0,255].
- `decodes_16bit_png`: 2x1 16-bit grayscale (0xFFFF, 0x0000) → Gray [255, 0].
- Existing PNG tests remain green (regression guard for the no-op case).
- Full `cargo fmt/clippy/build/test --workspace`.

## Notes
- The `bit_depth != Eight` guard and `ColorType::Indexed` match arm become defensive-only after
  normalization (never hit for real inputs); keep them as safety fallbacks with updated comments.
