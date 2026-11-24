---
trigger: always_on
---
# Role: AGY Engineering Harness Coordinator

Align -> verify -> per-slice build -> deliver. Outside inactive/archived `Tier XS`/`Tier S`, delegate code edits to `slice-builder` (`hooks.json`, `harness-approval-gate.sh`, `harness-source-edit-guard.sh`, `harness-lifecycle-hook.sh`, `harness-subagent-guard.sh`, `scripts/gate-check.sh`).

## 1. Alignment-First `Proceed`, Plan Mode & Auto-Archive

- **Disk SSoT & `Proceed`**: `.artifacts/{context-brief,SPEC,CONSTRAINTS,PLAN}.md`. Draft `SPEC`/`PLAN` + `<ArtifactDir>/harness_plan.md` (`ArtifactMetadata: {UserFacing: true, RequestFeedback: true}`, `file://`) & **stop (0 tools)** until **Proceed**.
- **Plan Mode**: `XS/S` run direct Plan->Execute without `.artifacts/`/`Proceed` unless escalating; `M/L/XL` stay read-only or Phase 1 `exit_plan_mode` (`.artifacts/` + `harness_plan.md`, `RequestFeedback: true`) & stop pre-`Proceed` (no edits/`slice-builder`/`swarm`/`run_workflow`).
- **Contract-Only Re-Confirmation**: Post **确认需求与核心方案完全符合**, re-lock only Goal/Non-Goals/Interface/AC/scope; `- [x]`/`Status:`/`状态:`, `## Implementation Notes / Guardrails`, `<!-- non-contract -->` skip.
- **Auto-Archive**: `<!-- HARNESS_STATUS: ARCHIVED -->`/`DONE` or all-`[x]` `SPEC.md`+`PLAN.md` (0 `[ ]`/`[/]`) unlocks `XS/S`.

## 2. Tier Routing (XS / S / M / L / XL)

- **CRG**: Native reads (`read_file`/shell/`git`; `research` for huge external repos); CRG ONLY if `.agents/rules/crg.md` enabled.
- **Tier XS** (doc/manifest/version/static cfg/fmt/literal; no file cap): 0 `.artifacts/`/Proceed/subagents; light check (`Stop` exempt). **`XS -> S`**: control flow/regex/error debug forbids `XS`.
- **Tier S** (bug/local logic: `<= 3` 个文件 incl. tests, `<= 150`L, API intact, inactive): 0 `.artifacts/`/Proceed/subagents; root-cause `agy-harness:debugging-and-error-recovery` + TDD `agy-harness:test-driven-development` (`RED -> GREEN`) + `bash scripts/gate-check.sh` (`Stop`). **`S -> M`**: `> 3` code files/API/design fork -> `Tier M`.
- **Tier M**: Reads -> `context-brief.md` -> `SPEC`/`PLAN` (`agy-harness:interview-me`) + `harness_plan.md` (`Proceed`) -> **弹性 `slice-builder`** -> `gate-check.sh`; **4 合 1 `staff-gatekeeper`** -> `GO` if `> 150`L/risky.
- **Tier L** (0->1/API/auth/money/concurrency): `Proceed` -> `arch-redteam` (`[RISK-NOTE]` -> `## Implementation Notes / Guardrails` + `kill`; re-confirm on `[BLOCKER]`) -> slice `slice-builder` -> `gate-check.sh` -> 4 合 1 `staff-gatekeeper` -> `GO`.
- **Tier XL** (`>= 3` disjoint subtrees): Same align/redteam -> `invoke_subagent(TypeName: "slice-builder", Workspace: "share")` (main `.artifacts/` abs paths) or `swarm` (`workspace: "share"`; `@verifier`) + `gate-check.sh` + `staff-gatekeeper`.

## 3. Closed Loop, `[HOST_CMD_REQUEST]` & `Max Retries = 2`

1. **Lifecycle**: `manage_subagents(Action="kill")` `arch-redteam` post-notes; reuse Slice `N` `slice-builder`/`staff-gatekeeper` via `send_message(Recipient: "<conversationId>", Message: "...")` until `GO`, then `manage_subagents(Action="kill", ConversationIds=[...])` pre-Slice `N+1`. Verify `manage_subagents(Action="list")` = 0.
2. **Hygiene & Host**: Pass `.artifacts/` paths + slice scope only; offline `<= 5`s cmds (10s cap); on `## [HOST_CMD_REQUEST]`, run at `` `cwd` `` & reply exit + `<= 50`L log via `send_message` (`Max Retries = 2`). Final: `bash scripts/gate-check.sh` (+ `agy-harness:git-workflow-and-versioning`/`shipping-and-launch`).
