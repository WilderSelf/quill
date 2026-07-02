# Self-driving harness — spec

Status: **draft** · Scope: Claude tooling (not product code) · Owner: this repo's `.claude/`

## Goal

Reduce the human to an **exception handler**. The recurring session pattern — *open session →
ask to plan next task → accept the recommendation → execute → PR → wrap* — is automated into a
loop that ships M0 increments **one atomic PR at a time**, continuously, and only surfaces to the
user on a **genuine blocker**. The user's job becomes: read notifications, answer the occasional
question, flip a kill switch if needed.

This spec is the source of truth for the harness design. The `/advance` skill (`SKILL.md` beside
this file) implements **Layer 0**. Layers 1–2 are configuration/cadence built on top once Layer 0
has earned trust.

## Environment constraints (verified — do not design against CLI assumptions)

This is the **desktop "local-agent-mode" (Cowork) build**, not the Claude Code CLI. Verified by
filesystem audit. Consequences that shape the design:

- **No `/loop`, no `ScheduleWakeup`, no `<<autonomous-loop>>` sentinels, no `CronCreate`.** The
  only reliable scheduling primitive is the **`scheduled-tasks` MCP** (`create_scheduled_task` /
  `update_scheduled_task` / `list_scheduled_tasks`).
- **Scheduled tasks run only while the app is open.** App closed when a task is due → it runs on
  next launch. This is **local, app-gated execution — not unattended cloud execution.** "One-shot
  the whole project overnight with the lid closed" is *not* achievable here. "Grind through
  increments while the app is open, hands-off" *is*.
- **Every scheduled run is memoryless** — a cold start with no access to the creating session. So
  the driver prompt and `/advance` must be **fully self-contained and idempotent**: reconstruct
  state from git/PR/specs every run; detect already-done work and continue rather than redo.
- **Permissions are `dontAsk` / fail-closed.** Anything not on the allowlist is auto-denied,
  never prompted, never run. The deny list already blocks `rm -rf`, `sudo`, `curl`/`wget`,
  force-push, push to `main`, `gh pr merge --admin`, and writes to `settings.json` /
  `.github/workflows/`. **This is the real safety boundary** — the harness relies on it rather
  than on prose instructions. Container isolation backs it up.
- **`/ship` runs on Sonnet 4.6** (opusplan auto-downshifts on exiting plan mode; a skill cannot
  pin the main-loop model). For correctness-critical export work, a human may manually `/model`
  to Opus before a hands-off session. Subagents *can* pin a model.

## Architecture — concentric layers, atomic unit at the center

```
Layer 2  Periodic quality gates — every N increments: review / security / simplify (independent context)
Layer 1  Driver — recurring scheduled-task fires /advance; reads the status token; kill switch = enabled:false
Layer 0  /advance — ONE atomic increment, cold-start & idempotent, exits with a status token
```

The unit of work is **one atomic increment in a fresh context**. Every documented failure mode of
long autonomous runs — context rot, quadratic token blowup, scope creep, runaway retry loops —
traces to long-lived context doing too much. Fresh-context-per-increment is the load-bearing
decision, not merely tidy.

### Layer 0 — `/advance` (this skill)

One invocation = one atomic PR (or a clean stop). Cold-start reconciliation first, then at most one
increment. Full procedure lives in `SKILL.md`. Contract: it **exits with a status token** the
driver reads — `MERGED` · `BLOCKED:<reason>` · `WINDOW_LIMIT` · `NOTHING_TO_DO`.

### Layer 1 — driver (built after Layer 0 is proven)

A recurring `scheduled-task` (e.g. every 20–30 min while the app is open) whose self-contained
prompt is essentially "run `/advance`." `notifyOnCompletion: true` pings the user each tick.
`enabled: false` (via `update_scheduled_task`) is the kill switch. Resume-after-rate-limit is
**free**: a throttled tick dies, the next cold tick retries — the recurring schedule *is* the
resume mechanism. No checkpoint/continue logic needed.

### Layer 2 — periodic quality gates (cadence overlay)

Every N increments (default 3–4) and always before a milestone tag, run in **fresh independent
contexts** (a builder reviewing its own work in the same session is the weakest possible gate):
`/code-review` (fed *diff + spec only*), `/security-review`, `/simplify`. Any HIGH finding becomes
the next increment, or a `BLOCKED` if it needs the user's call.

## CI monitoring — reconcile, never watch

`/ship` does **not** poll CI; it enables GitHub's **server-side** `--auto` merge and exits (CI is
the merge authority). The harness therefore **never runs a live `gh pr checks --watch`** — the
exact fragile pattern (no-checks race, premature-success, pending-forever) that wedges. Instead each
run **reconciles state at its start** with SHA-pinned `gh pr checks` / `gh pr list`:

- last PR **merged** → pick the next increment;
- open + CI **pending** → do nothing this run; the next run rechecks;
- open + CI **red** → bounded fix-forward, else `BLOCKED`;
- **draft** PR (ship hit its 5-attempt cap) → `BLOCKED`, notify, pause.

"Pending" is always *come back later*, never *done*. This dissolves every CI-watch failure mode.

## Guardrails — deterministic, outside the model

Most already exist as configuration (see Environment constraints). The through-line of every
runaway story ($47k 11-day loop; PocketOS prod-DB deleted in 9s) is identical: the only guardrail
was prose the model was "supposed to obey." Enforcement must be structural:

1. **Fail-closed permissions + deny list** (already active) — the destructive-shell / self-escalation
   blocked-class is enforced by `dontAsk`, not by this skill.
2. **`/ship`'s 5-attempt hard cap** (already active) — anti-infinite-burn on validate+review.
3. **Escalation threshold** — 3 consecutive failed increments in a run → stop the run. (Matches
   Anthropic auto-mode's 3-consecutive / 20-total rule.)
4. **One atomic increment per run** — bounds blast radius; small diffs correlate strongly with
   merge success, and scope-creep bundling is a top real-world failure.
5. **Kill switch** — `enabled: false` on the scheduled task.

## Stop-and-ask conditions (exit `BLOCKED:<reason>` — never guess)

1. Next task needs a **net-new spec or an architectural decision** not in the approved plan.
2. `/ship` validation/CI can't reach green within its bounded attempts (draft PR left open).
3. **Branch-protection gate missing or changed** — never enable `--auto` ungated; never `--admin`.
4. A **blocked-class action** would be required (destructive / external-publish e.g. POD upload /
   security-posture / permission-config change).
5. A **security-review finding** at HIGH severity.
6. **Milestone boundary reached (M0 → M1)** — an unconditional stop; M1 is far less spec-bound and
   needs the user back in the loop.
7. **Budget / iteration ceiling** or the 3-consecutive-failure threshold hit.
8. **Ambiguity the plan doesn't resolve** / low confidence on task selection.

## Acceptance criteria (Layer 0)

- `/advance` is invocable and `disable-model-invocation: true` (explicit invoke only, like `/ship`).
- It reconciles repo/PR/spec state on a cold start before doing anything.
- It performs **at most one** atomic increment, then exits with exactly one status token.
- On any stop-and-ask condition it exits `BLOCKED:<reason>` and leaves the repo in a clean state
  (draft PR if mid-flight), never force-merging or pushing to `main`.
- It is idempotent: re-running after a completed increment selects the *next* one, not a redo;
  re-running with an open blocking PR reports the blocker rather than starting new work.
- It reuses `/ship` for the plan→PR→auto-merge machinery rather than reimplementing it.

## Build order (each its own atomic increment)

1. **`/advance` skill** — this increment. Usable manually from day one.
2. Robust reconciliation helpers (SHA-pinned status checks) if `/advance` needs more than `gh` one-liners.
3. Driver `scheduled-task` (Layer 1) + documented kill switch.
4. Periodic quality-gate cadence (Layer 2).

**Prove Layer 0 manually over several real increments before enabling Layer 1.** Autonomy is
earned: classifier false-negative rates on "dangerous action" run ~1 in 6, and AI PRs pass CI while
being materially worse — the mitigations here make M0's tightly-specified export work *safe enough*,
not risk-free.
