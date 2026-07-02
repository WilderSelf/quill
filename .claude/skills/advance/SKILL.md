---
name: advance
description: Autonomously advance the project by ONE atomic increment — reconcile repo/PR/spec state on a cold start, select the next M0 increment, ship it via /ship (plan→PR→CI-gated auto-merge), then exit with a status token. Stops cleanly on any blocker. Use when the user runs /advance, or as the body of the recurring self-driving scheduled task. Explicit-invoke only.
disable-model-invocation: true
---

# /advance — one atomic increment, cold-start & idempotent

Advance the project by **exactly one** atomic increment, then stop. Design and rationale:
[`SPEC.md`](SPEC.md) beside this file. You are the **Layer 0** unit; a recurring `scheduled-task`
(Layer 1) re-fires you. Every run is **memoryless** — reconstruct all state from git/PR/specs.

**End every run by printing exactly one status token on its own final line:**
`STATUS: MERGED` · `STATUS: BLOCKED:<reason>` · `STATUS: WINDOW_LIMIT` · `STATUS: NOTHING_TO_DO`.
The driver reads this line to decide whether to continue, stop-and-notify, or retry next tick.

Safety is structural, not prose: `dontAsk` fail-closed permissions + the deny list are the real
boundary (no `rm -rf`/`sudo`/force-push/push-to-`main`/`--admin`/settings/workflow writes). Do not
attempt to work around a denied call — treat a denial as a `BLOCKED` signal.

## 1. RECONCILE (always first — never skip)

Rebuild state cold. Run read-only:

```
git fetch origin --quiet
git switch main && git pull --ff-only origin main
gh pr list --state open --json number,title,headRefName,isDraft,mergeStateStatus,statusCheckRollup
```

Interpret **before touching anything**:

- **An open `feat/*` PR authored by this harness exists** → do NOT start new work. Resolve it first:
  - **draft** (a prior `/ship` hit its 5-attempt cap) → this is a human/AI blocker. `STATUS: BLOCKED:draft-pr-#<n>-<one-line-reason-from-PR-body>`.
  - **open, checks failing** → one bounded fix-forward attempt on that branch (see §4); if still red, `STATUS: BLOCKED:ci-red-pr-#<n>`.
  - **open, checks pending/queued** → CI hasn't finished. Do nothing. `STATUS: NOTHING_TO_DO` (the next tick rechecks; *pending is never done*).
  - **open, checks green, not yet merged** → confirm `--auto` merge is enabled; if the gate is verified and it is, `STATUS: NOTHING_TO_DO` (server-side merge is imminent). If `--auto` is not enabled and the gate is verified, enable it, then `STATUS: NOTHING_TO_DO`.
- **No open harness PR** → the previous increment merged (or none started). Proceed to SELECT.

Only ever pin CI status to the **head SHA** of the PR (`statusCheckRollup` is already head-pinned
in the query above). Never resolve "latest run by workflow name" — that watches stale results.

## 2. SELECT the next atomic increment

Read `specs/README.md`, the milestone table, and recent history:

```
git log --oneline -15
```

Pick the **smallest independently shippable** next piece of **M0**, favouring specs marked
`accepted`/`in-progress` and the next unimplemented fast-follow. Then gate on stop-and-ask:

- Needs a **net-new spec** or an **architectural decision** not already in the approved plan
  (`~/.claude/plans/i-want-to-create-prancy-bee.md`) or an existing spec →
  `STATUS: BLOCKED:needs-spec:<short-desc>`. Do not invent product scope autonomously.
- **All M0 work is implemented** / the next work crosses into **M1** →
  `STATUS: BLOCKED:milestone-boundary-M0-complete`. M1 needs the user back in the loop.
- Selection is **ambiguous or low-confidence** → `STATUS: BLOCKED:ambiguous-next-<short-desc>`.
- A **clear fast-follow with an existing/derivable spec** → append/refine the spec entry under
  `specs/` if needed, then continue. Check `git log` and open PRs to be sure it isn't already done
  (redoing resolved work is a documented failure mode).

## 3. SHIP it

Invoke **`/ship <the selected increment>`**. Do not reimplement its pipeline. `/ship` will:
plan → `feat/<slug>` branch → implement (scoped) → validate (hard cap 5 attempts) → `reviewer`
subagent → conventional commit + push → `gh pr create` → verify branch protection → enable
`--auto --squash --delete-branch` when the gate is confirmed live.

Honour `/ship`'s own stop behaviour: if it stops at the 5-attempt cap it leaves a **draft PR** with
the blocker in the body → `STATUS: BLOCKED:ship-validate-cap:<slug>`. If the **branch-protection
gate is absent/unverifiable**, `/ship` leaves the PR open without `--auto` →
`STATUS: BLOCKED:gate-missing-pr-#<n>` (never enable `--auto` ungated, never `--admin`).

## 4. Bounded fix-forward (only for a reconciled red PR from §1)

At most **one** attempt: pull the branch, read the failing check's log
(`gh run view <run-id> --log-failed`), make the minimal fix, re-run `/ship`'s validate set locally
(`cargo fmt --check`, `cargo clippy --all-targets --all-features -- -D warnings`, `cargo build`,
`cargo test`), commit, push. If green locally, let server-side CI + `--auto` take it →
`STATUS: NOTHING_TO_DO` (merge will complete server-side). If still red → `STATUS: BLOCKED:ci-red-pr-#<n>`.

**Escalation cap:** if this run has already produced 3 consecutive failures (across §3/§4), stop:
`STATUS: BLOCKED:escalation-3-consecutive`.

## 5. WRAP tail

Only after a successful ship whose PR is green + auto-merging (or already merged): run the wrap
learning tail — `/reflect` then `/curate` — to promote any learnings and keep config lean. Skip if
`/ship` ended blocked (a blocked ship still carries signal, but don't run cleanup on a red tree).
Keep each phase's own human-approval gate. Do **not** run `/handoff` here — that's the user's
session-bridge tool, not part of the per-increment loop.

## 6. EXIT

Print a one-paragraph summary (what was selected, what shipped, merge/CI state, any reviewer
findings), then the final status line. Exactly one of:

- `STATUS: MERGED` — increment merged, or green + `--auto` enabled and merge is server-side-imminent.
- `STATUS: BLOCKED:<reason>` — stopped on a blocker; repo left clean (draft/open PR is fine). The
  driver notifies the user and pauses; do not start further work.
- `STATUS: WINDOW_LIMIT` — a usage/rate limit was hit mid-run. The recurring schedule resumes cold
  next tick; nothing to clean up.
- `STATUS: NOTHING_TO_DO` — nothing actionable this tick (CI pending, or merge already in flight).

## Invariants

- **One atomic increment per run.** Never chain a second increment in the same run — the driver
  fires the next run.
- **Idempotent.** Re-running after a merge selects the *next* increment; re-running with an open
  blocking PR reports the blocker, never starts parallel work.
- **Never** push to `main`, force-push, `--admin`-merge, or enable `--auto` on an unverified gate.
- **Milestone-aware.** M0→M1 is an unconditional `BLOCKED` stop.
