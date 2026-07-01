---
name: handoff
description: Write or refresh HANDOFF.md, a session-bridge doc for resuming this project in a fresh session (or on another machine). Use when the user says "update handoff", "write a handoff for next session", or similar — not for routine commits.
disable-model-invocation: true
---

# /handoff — session-bridge doc for this project

`HANDOFF.md` (repo root, untracked/gitignored) is a bridge for the *next* session, not a
project doc — CLAUDE.md, specs/, and memory already own durable facts. Delete-or-absorb, not
maintain-forever.

## Before writing
Don't restate the previous HANDOFF.md's claims about external state as still true — **re-verify
live**: `git status`/`git log`/`gh pr list` for repo state, `gh api repos/{owner}/{repo}` and
`.../branches/main/protection` for auto-merge/branch-protection/token-scope facts. These change
independent of git history and a stale carried-forward claim is worse than no claim.

## Sections to include
1. **What this project is** — one paragraph, pointer to CLAUDE.md/the approved plan for detail.
2. **Current state (all green)** — repo/branch sync status, recent PR history, merge-gate
   status (verified live, not assumed), build/test status, current milestone and its concrete
   next unimplemented piece.
3. **Environment gotchas** — anything about *this container* that cost time to discover
   (toolchain installs, auth quirks, required env vars) — only include what actually recurred
   or would surprise a fresh session, not routine setup.
4. **Automation system summary** — skills/agents/permissions relevant to this project, and
   anything about them that changed this session.
5. **Outstanding / needs the user** — anything blocked on human action (admin approval, secrets,
   product decisions), stated as still-blocked only if just re-verified.
6. **Next steps** — the concrete next unit of work, referencing the spec/plan that defines it.
7. **How to resume** — the minimal command sequence to get back to a working state.

## After writing
Confirm `HANDOFF.md` is gitignored (add it if not) so it doesn't linger as untracked noise.
