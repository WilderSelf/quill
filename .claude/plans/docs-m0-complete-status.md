# Plan — refresh CLAUDE.md M0 status line

## Context
PR #31 (spec 0015, real width-based line breaking) closed what spec 0015 calls "the last
substantive M0 export-correctness gap." Per the reconciled roadmap, M0 is now code-complete:
specs 0001–0013 and 0015 are implemented; the only remaining M0 item is a manual,
non-automatable real-POD upload with a B2A-equipped CMYK profile. The project `CLAUDE.md`
status line still reads "M0 in progress … fast-follows continue," which is stale drift.

## Change (docs only — no spec, no code, no architectural decision)
- Update the `## What this is` status paragraph in `CLAUDE.md` to state M0 is code-complete
  (headless PDF/X export), name the sole remaining manual M0 item (real POD upload), and note
  the M1 editing/text-layout arc is next (begins with a new spec).

## Acceptance
- Status line accurately reflects post-#31 reality.
- `fmt`/`clippy`/`build`/`test` remain green (no code touched).

## Non-goals
- No spec status changes (spec 0015 stays multi-part in-progress; its follow-ups are M1).
- No M1 feature work — that needs the user + a new spec (0016).
