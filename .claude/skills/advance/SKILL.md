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
git fetch origin --prune --quiet
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

**1c. Prune merged branches + worktrees (squash-safe — do this every reconcile).** PRs merge via
**squash**, so a merged branch's tip is *never* an ancestor of `main` — `git branch --merged` will
**not** list it, and stale `feat/*`/`docs/*` branches (and their `/tmp` worktrees) accumulate. Do
**not** use `git branch --merged`; use PR/remote state, which is squash-correct. Never delete a
branch with an **open** PR (a concurrent session's in-flight or draft work) or unpushed local-only
work:

```
git worktree prune                                   # drop admin entries for gone worktree dirs
# (a) remote branch was deleted by `--delete-branch` → local upstream shows "gone":
git branch -vv | awk '/: gone]/{print $1}' | grep -vx main | xargs -r git branch -D
# (b) PR merged but its remote branch survived deletion (like #50): match by headRefName:
for b in $(gh pr list --state merged --limit 50 --json headRefName --jq '.[].headRefName'); do
  [ "$b" = main ] && continue
  git show-ref --verify --quiet "refs/heads/$b" && git branch -D "$b"
  git ls-remote --exit-code --heads origin "$b" >/dev/null 2>&1 && git push origin --delete "$b"
done
```

`git branch -D` is safe here: git refuses to delete a branch checked out in another worktree, and
we only target branches whose PR is **merged** or whose upstream is **gone**. (`git push origin
--delete` is *not* a `main`/force push, so it clears the deny list.)

Only ever pin CI status to the **head SHA** of the PR (`statusCheckRollup` is already head-pinned
in the query above). Never resolve "latest run by workflow name" — that watches stale results.

## 2. SELECT the next atomic increment

Read `specs/README.md`, the milestone table, and recent history:

```
git log --oneline -15
```

**The approved plan is the authorization boundary — not the set of spec files that happen to exist
yet.** M0 is code-complete; the project is in **M1**, whose scope the approved plan
(`~/.claude/plans/i-want-to-create-prancy-bee.md`) fixes up front: *text frames + threading,
paragraph/character styles, master pages, linked-image proxy cache, incremental layout* (plus the
`.claude/plans/*` increment plans). Within that boundary you **decompose and execute without
stopping for per-increment approval — the plan *is* the approval.** (This is the documented
autonomous-agent pattern: bounded autonomy through an up-front constraint, *not* a runtime approval
gate at every step.) Pick the **smallest independently shippable** next piece of the current
milestone, favouring specs marked `accepted`/`in-progress` and the next unimplemented step of the
arc. First `git log`/open-PR check so you don't redo resolved work. Then classify the piece:

- **In the approved plan (or a fast-follow of an accepted spec), but no spec file yet** → this is
  authorized work whose spec just hasn't been written. **Author it as step 0 of the increment:**
  write `specs/00NN-<slug>.md` — deriving behaviour + acceptance criteria from the plan, and citing
  the plan line it satisfies — add its row to `specs/README.md` as `in-progress`, then continue to
  SHIP (§3). Writing a plan-authorized spec **is executing the plan**; do **not**
  `BLOCKED:needs-spec` here. (E.g. `0019-text-frames-threading` is enumerated in the plan → write it
  and proceed. Next free number is 0019; 0015 is the in-progress M0 text-metrics spec, 0016–0018 are
  the shipped M1 shaping/Knuth-Plass/hyphenation specs.)
- **Genuinely outside every approved plan** (net-new product scope nothing covers) **or needs an
  architectural decision the plan leaves open** (a real fork — data-model change, new dependency,
  new colorspace-intent semantics) → `STATUS: BLOCKED:needs-spec:<short-desc>` (or
  `BLOCKED:arch-decision:<short-desc>`). This is the *true* boundary: don't invent scope, don't
  silently decide architecture.
- Next work crosses into the next **unentered** milestone (**M1 → M2**) →
  `STATUS: BLOCKED:milestone-boundary-<current>-complete`. (M0→M1 is already authorized; it no longer
  blocks.)
- Selection is **ambiguous or low-confidence** → `STATUS: BLOCKED:ambiguous-next-<short-desc>`.

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
- **`/handoff`** — **refresh `HANDOFF.md` whenever an increment merged (i.e. `main` advanced)**, not
  "optionally." A skipped refresh is exactly how it went stale (pointed at a merged PR as still-open,
  13 commits behind). Execute the `/handoff` pipeline inline (it's `disable-model-invocation`): it is
  **re-verify-live, not carry-forward** — restate repo/PR/gate state from `git`/`gh` as of *now*,
  never copy the previous `HANDOFF.md`'s claims. The file is untracked/gitignored, so this is a
  local write, no PR. Skip only when nothing merged this run (`NOTHING_TO_DO`/`BLOCKED` with no
  merge).

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
