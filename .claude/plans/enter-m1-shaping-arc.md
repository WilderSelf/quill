# Plan — cross into M1 and open the text-shaping arc

## Context
M0 is code-complete (specs 0001–0013 + 0015 increment 1; sole remaining M0 item is a manual,
non-automatable real-POD upload). The user has authorized crossing the M0→M1 boundary and asked to
"update needed files to allow this seamlessly." Per `quill-m1-roadmap`, the first M1 increment is
**rustybuzz shaping** on the existing `CharMetrics` / `break_by_width` seam (next free spec number =
0016).

Today three things still hard-block M1: the project `CLAUDE.md` status still reads "Currently at
M0"; the `/advance` harness treats **M0→M1 as an unconditional `BLOCKED` stop** (SKILL §2 + invariant,
SPEC stop-condition 7); and there is no authored M1 spec, so even with the gate lifted the loop would
re-block on `needs-spec`.

## Scope — enablement only (docs / spec / harness; NO product Rust code)
This increment opens the arc; it does **not** implement shaping. Shaping is the next, separately
reviewable increment (implements spec 0016).

1. **`specs/0016-rustybuzz-shaping.md`** (new) — author increment 1 of the shaping spec per the
   roadmap: shaped-run **measurement** via `rustybuzz` (kerning/ligature-aware line widths) on the
   `CharMetrics`/`break_by_width` seam. Status `accepted`, milestone M1. Carrying shaped glyphs into
   PDF output + shaping-GID↔subset-GID reconciliation are named non-goals (a later increment).
2. **`specs/README.md`** — add the 0016 row (M1, accepted).
3. **`CLAUDE.md`** — status paragraph reframed to "now in M1"; "Currently at **M0**" → "**M1**".
4. **`.claude/skills/advance/SKILL.md`** — §2 selects the **current milestone (M1)**; the
   milestone-boundary block fires at the next *unentered* boundary (M1→M2), not M0→M1; invariant
   updated.
5. **`.claude/skills/advance/SPEC.md`** — stop-condition 7 + Goal generalized from "M0" to the
   current/next milestone; note M0→M1 authorized.
6. **Memory (non-repo, wrap tail):** update `quill-m1-roadmap.md` — M1 entered, spec 0016 authored,
   next task = implement 0016 increment 1.

## Acceptance
- `git grep` shows no remaining "Currently at M0" / "M0→M1 ... unconditional BLOCKED" claims.
- Spec 0016 exists, is indexed, `accepted`, M1, with a small shippable increment-1 scope.
- The harness gate now blocks at M1→M2, and would *select* spec 0016 rather than block, on the next
  `/advance` tick.
- `fmt`/`clippy`/`build`/`test` remain green (no Rust touched — validated to confirm).

## Non-goals
- Implementing rustybuzz shaping (the next increment).
- Adding the `rustybuzz` dependency (belongs with the implementation).
- Any change to export output or the layout/measurement code.
