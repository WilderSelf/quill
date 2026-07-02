---
name: advance
description: Autonomously advance the project by ONE atomic increment — reconcile repo/PR/spec state on a cold start, select the next increment of the active milestone, ship it via /ship (plan→PR→CI-gated auto-merge), then exit with a status token. Stops cleanly on any blocker. Use when the user runs /advance, or as the body of the recurring self-driving scheduled task. Explicit-invoke only.
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

**1a. Clean-tree gate — check this BEFORE switching branches.** This project's working tree is
**shared across sessions**: a concurrent session (another `/advance`, or the user editing) may have
uncommitted work here, and switching branches over it corrupts their state. So the very first thing:

```
git status --porcelain
```

If it reports **any** uncommitted or untracked changes, they are **not yours** — every run starts
cold and has produced nothing yet — so **do not switch branches and do not touch them** →
`STATUS: BLOCKED:dirty-tree`. The driver notifies the user; the foreign WIP is left exactly as found.
(For out-of-band work you *must* do while the tree is dirty, use an isolated `git worktree` on a
different device — e.g. the `/tmp` scratchpad — and stage only your own files.)

**1b. Rebuild state cold** — only once the tree is verified clean. Run read-only:

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

Pick the **smallest independently shippable** next piece of the **current milestone (M1** — M0 is
code-complete; the M1 text-layout arc is authorized and open, starting with spec 0016), favouring
specs marked `accepted`/`in-progress` and the next unimplemented fast-follow. Then gate on
stop-and-ask:

- Needs a **net-new spec** or an **architectural decision** not already in the approved plan
  (`~/.claude/plans/i-want-to-create-prancy-bee.md`) or an existing spec →
  `STATUS: BLOCKED:needs-spec:<short-desc>`. Do not invent product scope autonomously. (M1 is less
  spec-bound than M0 — lean toward this stop when the next piece isn't already specced.)
- The next work crosses into the next **unentered** milestone (**M1 → M2**) →
  `STATUS: BLOCKED:milestone-boundary-<current>-complete`. The new milestone needs the user back in
  the loop. (M0→M1 has already been authorized by the user; that boundary no longer blocks.)
- Selection is **ambiguous or low-confidence** → `STATUS: BLOCKED:ambiguous-next-<short-desc>`.
- A **clear fast-follow with an existing/derivable spec** → append/refine the spec entry under
  `specs/` if needed, then continue. Check `git log` and open PRs to be sure it isn't already done
  (redoing resolved work is a documented failure mode).

## 3. SHIP it — execute the `/ship` pipeline INLINE (do not Skill-invoke it)

`/ship` is `disable-model-invocation: true`, so **you cannot launch it via the Skill tool** — that
fails with "cannot be used with Skill tool." Since you *are* the main agent, **execute its
documented pipeline directly.** Read `~/.claude/skills/ship/SKILL.md` and perform its steps as your
own, treating it as the single source of truth (don't paraphrase a divergent pipeline):

plan (`.claude/plans/<slug>.md`) → `feat/<slug>` branch → implement (scoped) → validate to green
(`cargo fmt --check`, `cargo clippy --all-targets --all-features -- -D warnings`, `cargo build`,
`cargo test`) **hard cap 5 attempts** → dispatch the `reviewer` subagent (Agent tool) and address
blocking findings → conventional commit + push → `gh pr create` → verify branch protection
(`gh api repos/{owner}/{repo}/branches/main/protection`) → `gh pr merge --auto --squash
--delete-branch` **only when the gate is confirmed live**.

Honour `/ship`'s stop behaviour exactly: if validation can't reach green within the **5-attempt
cap**, commit WIP, push, open a **draft PR** with the failing command + error in the body →
`STATUS: BLOCKED:ship-validate-cap:<slug>`. If the **branch-protection gate is
absent/unverifiable** (403/404), leave the PR open **without** `--auto` →
`STATUS: BLOCKED:gate-missing-pr-#<n>`. Never enable `--auto` ungated, never `--admin`, never merge
red or draft.

## 4. Bounded fix-forward (only for a reconciled red PR from §1)

At most **one** attempt: pull the branch, read the failing check's log
(`gh run view <run-id> --log-failed`), make the minimal fix, re-run `/ship`'s validate set locally
(`cargo fmt --check`, `cargo clippy --all-targets --all-features -- -D warnings`, `cargo build`,
`cargo test`), commit, push. If green locally, let server-side CI + `--auto` take it →
`STATUS: NOTHING_TO_DO` (merge will complete server-side). If still red → `STATUS: BLOCKED:ci-red-pr-#<n>`.

**Escalation cap:** if this run has already produced 3 consecutive failures (across §3/§4), stop:
`STATUS: BLOCKED:escalation-3-consecutive`.

## 5. WRAP tail — run inline, auto-applying (the user has authorized auto-accept)

When an increment merges, capture learnings **autonomously** — do not defer to the user with a "go
run `/wrap`" note. `/reflect` and `/curate` are `disable-model-invocation: true`, so (like `/ship`)
you **execute their pipelines inline** as the main agent, reading their `SKILL.md` files as the
source of truth. The user has **authorized auto-accepting proposals**, so **apply each change
directly** instead of pausing for per-change approval. Be judicious: this is auto-*accept*, not
auto-*invent* — promote only a durable learning that generalizes; if the increment surfaced nothing
worth keeping, record "no new learnings" and move on. Under the permission model these edits are
already allowed (bare `Edit`/`Write`; deny still guards `settings.json`/secrets).

Route each learning to its home:

- **Non-repo targets — edit in place, no PR, auto-applied:** project memory
  (`…/memory/*.md` + its `MEMORY.md` index) and user-scope config (`~/.claude/CLAUDE.md`,
  `~/.claude/rules/*`). Then run `/curate`'s pipeline over the same targets — dedupe/condense within
  CLAUDE.md's ~200-line budget, prune stale entries. Apply safe condensations directly.
- **Repo-tracked targets** (project `CLAUDE.md`, `specs/`, `.claude/rules/`, `.claude/skills/`): a
  durable learning here is a real repo change. Land **at most one** small capture PR per run via the
  same gated `/ship`-inline flow (§3) — `docs:`/`chore:` commit, gate-verified `--auto` merge. Never
  push config straight to `main`. If there's no repo-tracked learning, skip this.
- **`/handoff`** — optional and low-risk (writes the untracked `HANDOFF.md`). Refresh it if the run
  produced state worth bridging; otherwise skip.

**Still don't guess.** Auto-accept covers *applying your own proposals*, not resolving genuine
judgment calls: a `/curate`-flagged **contradiction**, an architectural learning, or anything that
would rewrite an approved decision → do **not** auto-apply. Note it in the exit summary (or
`STATUS: BLOCKED:<reason>` if it actually blocks progress) and leave it for the user.

The Layer 2 quality gates (`/code-review`, `/security-review`, `/simplify`) are model-invocable and
**may** be run via the Skill tool on the periodic cadence.

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

- **One atomic increment per run.** Never chain a second *feature* increment in the same run — the
  driver fires the next run. (The §5 wrap tail may land **one** small `docs:`/`chore:` capture PR
  plus in-place memory/config edits; that is wrapping the increment, not new feature work.)
- **Wrap runs autonomously.** After a merge, run `/reflect`+`/curate` inline and auto-apply (§5) —
  never defer them to the user. Still surface (don't auto-resolve) contradictions and architectural
  judgment calls.
- **Idempotent.** Re-running after a merge selects the *next* increment; re-running with an open
  blocking PR reports the blocker, never starts parallel work.
- **Never** push to `main`, force-push, `--admin`-merge, or enable `--auto` on an unverified gate.
- **Never switch branches over a dirty shared tree.** Uncommitted changes you didn't make belong to
  a concurrent session → `BLOCKED:dirty-tree` (see §1a).
- **Milestone-aware.** Crossing into the next **unentered** milestone is an unconditional `BLOCKED`
  stop (currently **M1 → M2**); the new milestone needs the user back in the loop. M0→M1 has been
  authorized and no longer blocks — M1 is the active milestone.
