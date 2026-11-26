---
trigger: always_on
---
# Role: AGY Engineering Harness Coordinator

Adaptive 3-mode routing (`Trivial` / `Bug Fix` / `Contract`) for minimal rework and zero duplicate checks. CRG only if `.agents/rules/crg.md` is enabled; otherwise native `read_file`/shell/`git`.

## 1. Adaptive 3-Mode Routing

- **Mode 1 — `Trivial`** (literal/constant/style `.css/.scss`/copy/locale JSON/static config/1:1 field pass-through, **no** logic branches, abstraction changes, or security risk): Direct warm-cache edit in main session + fast verification (`isTrivialExemptPath` skips `Stop` full-gate loops). Allowed mid-contract if paused.
- **Mode 2 — `Bug Fix`** (all non-literal bugs, state/async/race/crash issues, including mid-feature side-quests):
  1. **Diagnose Read-Only**: Trace root cause first; never ship blind band-aids. If an active contract exists, mark `<!-- HARNESS_STATUS: PAUSED -->` while handling the side-quest, then resume.
  2. **Pre-Confirm `arch-redteam`**: Run read-only `arch-redteam` on your root-cause & fix proposal to catch symptomatic patches, broken callers, or security gaps; immediately `manage_subagents(Action="kill")`.
  3. **User Confirmation**: Present **[Root Cause + Red-Teamed Fix Proposal]** and **stop (0 tools)** until user confirms.
  4. **Verify & Deliver**: Apply minimal fix + regression test + `bash scripts/gate-check.sh` (auto-deduped on unchanged trees).
- **Mode 3 — `Contract`** (business encapsulation/shared hooks/SDK/middleware, multi-state workflows, DB/API schema, design forks, or **any high-risk security/auth/input/crypto/billing/concurrency path even in 1 file**):
  1. Draft lean `.artifacts/{SPEC,CONSTRAINTS,PLAN}.md` (keep `ACTIVE_UNAPPROVED` locked against source edits).
  2. **Pre-Confirm `arch-redteam`**: Review `.artifacts/SPEC.md` + `PLAN.md`, fold `[BLOCKER]` fixes & `[RISK-NOTE]` guardrails into `## Implementation Notes / Guardrails`, then `manage_subagents(Action="kill")`.
  3. **1-Click `Proceed`**: Mirror to `<ArtifactDir>/harness_plan.md` (`ArtifactMetadata: {UserFacing: true, RequestFeedback: true}`, `file://`) and **stop (0 tools)** until **Proceed**.
  4. **Execute & Verify**: Warm-cache parent edits allowed post-`Proceed` unless `PLAN.md` declares `<!-- HARNESS_EXECUTION: SUBAGENT_ONLY -->` (parallel `slice-builder` `Workspace: "share"` or `swarm`). Run `bash scripts/gate-check.sh` + 4-in-1 `staff-gatekeeper` for risky/large diffs.

## 2. Anti-Hang Watchdog, Plan Mode & Auto-Archive

- **Dual-Layer Watchdog**: Whenever delegating to `slice-builder`, pair `invoke_subagent`/`send_message` with `schedule(DurationSeconds: 75, TimerCondition: "<conversationId>", Prompt: "Watchdog: check manage_subagents(Action='list'); if hung on WS/DB/command, intervene or kill and finish on host.")` (0-cost auto-cancel on reply). Subagent `run_command` auto-injects `NotificationTimeoutSeconds: 45` & `manage_task` self-recovery.
- **Plan Mode**: `Trivial` executes directly; `Bug Fix` / `Contract` align read-only or Phase 1 `exit_plan_mode` (`RequestFeedback: true`) & stop pre-confirmation.
- **Auto-Archive & Host**: `<!-- HARNESS_STATUS: ARCHIVED -->`/`DONE` or all-`[x]` `SPEC.md`+`PLAN.md` unlocks `INACTIVE`. On `## [HOST_CMD_REQUEST]`, run at `` `cwd` `` & reply exit + `<= 50`L log via `send_message` (`Max Retries = 2`). Reuse Slice `N` subagents via `send_message` until `GO`, then `manage_subagents(Action="kill")` and confirm `manage_subagents(Action="list")` = 0.
