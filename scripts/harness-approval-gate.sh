#!/bin/bash
# harness-approval-gate.sh — Antigravity PreToolUse Software 1.0 Gate
#
# Enforces human-in-the-loop confirmation (Proceed click or explicit affirmative
# user approval) whenever .artifacts/SPEC.md or .artifacts/PLAN.md core contract
# sections are created or substantively modified, gating non-allowlisted
# invoke_subagent, define_subagent(EnableWriteTools=true), swarm(add),
# run_workflow, and Orchestrator exit_plan_mode.

set -euo pipefail

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
SCRIPT_REPO_ROOT="$(cd "$SCRIPT_DIR/.." && pwd)"

PLUGIN_MODE="false"
for arg in "$@"; do
  case "$arg" in
    --plugin-mode)
      PLUGIN_MODE="true"
      ;;
  esac
done

if [ -t 0 ]; then
  HOOK_STDIN="{}"
else
  HOOK_STDIN="$(cat || true)"
  [ -z "$HOOK_STDIN" ] && HOOK_STDIN="{}"
fi

export AGY_HARNESS_SCRIPT_REPO_ROOT="$SCRIPT_REPO_ROOT"
export AGY_HARNESS_PLUGIN_MODE="$PLUGIN_MODE"
export AGY_HARNESS_HOOK_STDIN="$HOOK_STDIN"

if command -v node >/dev/null 2>&1; then
  exec node -e '
"use strict";
const fs = require("node:fs");
const os = require("node:os");
const path = require("node:path");
const crypto = require("node:crypto");

const emit = (decision, reason) => {
  const out = reason ? { decision, reason } : { decision };
  process.stdout.write(JSON.stringify(out) + "\n");
  process.exit(0);
};

let input = {};
try {
  input = JSON.parse(process.env.AGY_HARNESS_HOOK_STDIN || "{}");
} catch {
  emit("allow");
}

const pluginMode = process.env.AGY_HARNESS_PLUGIN_MODE === "true";
const wsList = Array.isArray(input.workspacePaths) ? input.workspacePaths : [];
const rawWorkspaceRoot =
  (wsList[0] && String(wsList[0])) ||
  (pluginMode ? process.cwd() : process.env.AGY_HARNESS_SCRIPT_REPO_ROOT) ||
  process.cwd();
let workspaceRoot = rawWorkspaceRoot;
try {
  workspaceRoot = fs.realpathSync(rawWorkspaceRoot);
} catch {}

const hasHarnessInWs =
  fs.existsSync(path.join(workspaceRoot, ".agents", "rules", "agy-harness.md")) ||
  fs.existsSync(path.join(workspaceRoot, ".artifacts", "SPEC.md")) ||
  fs.existsSync(path.join(workspaceRoot, ".artifacts", "PLAN.md"));
if (pluginMode && !hasHarnessInWs) {
  emit("allow");
}

const toolCall = input.toolCall || {};
const toolName = String(toolCall.name || "");
const args = toolCall.args || {};

const READ_ONLY_SUBAGENTS = new Set(["arch-redteam", "staff-gatekeeper", "research"]);
const subagentType = String(args.TypeName || args.typeName || args.subagent_type || "").trim();

const isExitPlanMode = toolName === "exit_plan_mode";
const isAllowlistedSubagent =
  toolName === "invoke_subagent" && READ_ONLY_SUBAGENTS.has(subagentType);
const isGatedSubagent =
  toolName === "invoke_subagent" && !READ_ONLY_SUBAGENTS.has(subagentType);
const isGatedDefine =
  toolName === "define_subagent" && args.EnableWriteTools === true;
const isGatedSwarm =
  toolName === "swarm" && String(args.action || "").trim() === "add";
const isGatedWorkflow = toolName === "run_workflow";
const isGated = isGatedSubagent || isGatedDefine || isGatedSwarm || isGatedWorkflow;

if (!isGated && !isExitPlanMode && !isAllowlistedSubagent) {
  emit("allow");
}

const stripFences = (text, stripInlineBackticks) => {
  const out = [];
  let inFence = false;
  for (const line of String(text || "").replace(/\r\n/g, "\n").split("\n")) {
    if (/^\s*```/.test(line)) {
      inFence = !inFence;
      out.push("");
      continue;
    }
    if (inFence) {
      out.push("");
      continue;
    }
    out.push(stripInlineBackticks ? line.replace(/`[^`]*`/g, "") : line);
  }
  return out.join("\n");
};

const stripNonContractSections = (text) => {
  const cleaned = String(text || "")
    .replace(/\r\n/g, "\n")
    .replace(/<!--\s*non-contract\s*-->[\s\S]*?<!--\s*\/non-contract\s*-->/gi, "");
  const nonContractHeadingRe =
    /^##\s+(?:Implementation Notes(?:\s*\/\s*Guardrails)?|Guardrails|实施备注|风险护栏)\b/i;
  const out = [];
  let skipping = false;
  for (const rawLine of cleaned.split("\n")) {
    if (/^##\s+/.test(rawLine)) {
      skipping = nonContractHeadingRe.test(rawLine);
    }
    if (!skipping) out.push(rawLine);
  }
  return out.join("\n");
};

const normalizeContract = (text) => {
  const contractOnly = stripNonContractSections(text);
  const out = [];
  for (const rawLine of contractOnly.split("\n")) {
    const normalized = rawLine
      .replace(/^(\s*-\s*)\[[ xX\/-]\](\s+)/, "$1[ ]$2")
      .replace(/^(\s*-\s*\*\*)(?:Status|状态)(:\*\*\s*).+$/, "$1Status$2<STATUS>");
    if (normalized.trim().length > 0) {
      out.push(normalized);
    }
  }
  return out.join("\n");
};

const hasSpecTemplateMarker = (specTxt) => {
  if (!specTxt.trim()) return true;
  const stripped = stripFences(specTxt, true);
  return (
    /^\s*-\s+\*\*Goal:\*\*\s+_Describe the concrete user or system problem being solved\._\s*$/m.test(stripped) ||
    /<!--\s*HARNESS_TEMPLATE_UNFILLED\b/i.test(stripped)
  );
};

const hasPlanTemplateMarker = (planTxt) => {
  if (!planTxt.trim()) return true;
  const stripped = stripFences(planTxt, true);
  return (
    /^\s*##\s+Slice 1:\s+\[Title\]\s*$/m.test(stripped) ||
    /<!--\s*HARNESS_TEMPLATE_UNFILLED\b/i.test(stripped)
  );
};

const isExplicitArchiveMarker = (text) =>
  /<!--\s*HARNESS_STATUS:\s*(?:ARCHIVED|DONE)\s*-->/i.test(stripFences(text, true));

const countSpecCheckboxes = (specTxt) => {
  const body = stripFences(stripNonContractSections(specTxt), true);
  let done = 0;
  let open = 0;
  for (const line of body.split("\n")) {
    const m = line.match(/^\s*[-*]\s+\[([ xX\/])\](?:\s|$)/);
    if (!m) continue;
    if (m[1] === "x" || m[1] === "X") done++;
    else open++;
  }
  return { done, open };
};

const countPlanStatuses = (planTxt) => {
  const body = stripFences(stripNonContractSections(planTxt), false);
  let done = 0;
  let open = 0;
  for (const line of body.split("\n")) {
    const m =
      line.match(/^\s*(?:[-*]\s*)?(?:\*\*)?(?:Status|状态)(?:\*\*)?\s*:\s*(?:\*\*)?\s*`?\[([ xX\/])\]/i) ||
      line.match(/^\s*[-*]\s+\[([ xX\/])\](?:\s|$)/);
    if (!m) continue;
    if (m[1] === "x" || m[1] === "X") done++;
    else open++;
  }
  return { done, open };
};

const specPath = path.join(workspaceRoot, ".artifacts", "SPEC.md");
const planPath = path.join(workspaceRoot, ".artifacts", "PLAN.md");
const specExists = fs.existsSync(specPath);
const planExists = fs.existsSync(planPath);
const specContent = specExists ? fs.readFileSync(specPath, "utf8") : "";
const planContent = planExists ? fs.readFileSync(planPath, "utf8") : "";

const hasTemplatePlaceholder =
  !specExists ||
  !planExists ||
  hasSpecTemplateMarker(specContent) ||
  hasPlanTemplateMarker(planContent);

const isExplicitlyArchived =
  (specExists && isExplicitArchiveMarker(specContent)) ||
  (planExists && isExplicitArchiveMarker(planContent));

const specCounts = countSpecCheckboxes(specContent);
const planCounts = countPlanStatuses(planContent);
const isAutoArchived =
  !hasTemplatePlaceholder &&
  specCounts.done >= 1 &&
  specCounts.open === 0 &&
  planCounts.done >= 1 &&
  planCounts.open === 0;

const isArchived = isExplicitlyArchived || isAutoArchived;
const isContractActive = !hasTemplatePlaceholder && !isArchived;

const sha256 = (s) => crypto.createHash("sha256").update(s).digest("hex");
const semanticHash = sha256(
  "SPEC\n" + normalizeContract(specContent) + "\nPLAN\n" + normalizeContract(planContent)
);

const stateDir =
  process.env.AGY_HARNESS_STATE_DIR ||
  process.env.PLUGIN_DATA ||
  path.join(os.tmpdir(), "agy-harness-approvals");
const stateKey = sha256(workspaceRoot + "::" + String(input.conversationId || "default")).slice(0, 24);
const statePath = path.join(stateDir, stateKey + ".json");

let state = null;
if (fs.existsSync(statePath)) {
  try {
    state = JSON.parse(fs.readFileSync(statePath, "utf8"));
  } catch {
    state = null;
  }
}

const FILE_WRITE_TOOLS = new Set([
  "write_file",
  "edit_file",
  "create_file",
  "delete_file",
  "sed_file",
  "write_to_file",
  "replace_file_content",
  "multi_replace_file_content",
]);

const extractTargetArg = (tcArgs) => {
  if (!tcArgs || typeof tcArgs !== "object") return "";
  return String(
    tcArgs.Path ||
      tcArgs.TargetFile ||
      tcArgs.TargetPath ||
      tcArgs.File ||
      tcArgs.path ||
      tcArgs.file ||
      tcArgs.file_path ||
      tcArgs.filePath ||
      tcArgs.target_file ||
      ""
  )
    .replace(/^"+|"+$/g, "")
    .trim();
};

const isTierXsExemptPath = (relPath) => {
  const rel = String(relPath || "").replace(/\\/g, "/").replace(/^(?:\.\/)+/, "");
  if (!rel || /^(?:agents|skills|commands)(?:\/|$)/.test(rel)) return false;
  const base = path.posix.basename(rel);
  return (
    /\.(?:md|mdx|txt|rst|adoc)$/i.test(rel) ||
    /^(?:LICENSE(?:\.[A-Za-z0-9._-]+)?|NOTICE|AUTHORS|CONTRIBUTORS|PATENTS|CODEOWNERS|\.(?:gitignore|gitattributes|editorconfig|prettierignore|eslintignore|dockerignore|npmrc|nvmrc|node-version|python-version|tool-versions))$/i.test(base) ||
    /(?:^|\/)(?:plugin|marketplace)\.json$/i.test(rel) ||
    /(?:^|\/)(?:version\.txt|VERSION)$/.test(rel) ||
    /(?:^|\/)(?:package-lock\.json|npm-shrinkwrap\.json|pnpm-lock\.yaml|yarn\.lock|bun\.lockb?|Cargo\.lock|poetry\.lock|uv\.lock|Pipfile\.lock|go\.sum|pubspec\.lock|Gemfile\.lock|composer\.lock)$/.test(rel) ||
    /\.(?:svg|png|jpe?g|gif|webp|ico|woff2?|ttf|otf|eot)$/i.test(rel)
  );
};

const extractText = (val) => {
  if (typeof val === "string") return val;
  if (Array.isArray(val)) return val.map(extractText).join("\n");
  if (val && typeof val === "object") {
    if (typeof val.text === "string") return val.text;
    if (typeof val.content === "string") return val.content;
    return JSON.stringify(val);
  }
  return "";
};

let lastUserStep = null;
let modifiedAfterUserInTranscript = false;
const transcriptPath = input.transcriptPath ? String(input.transcriptPath) : "";

if (transcriptPath && fs.existsSync(transcriptPath)) {
  const lines = fs.readFileSync(transcriptPath, "utf8").split(/\r?\n/).filter(Boolean);
  let lastUserIdx = -1;
  const parsed = [];
  for (let i = 0; i < lines.length; i++) {
    try {
      const obj = JSON.parse(lines[i]);
      parsed.push(obj);
      if (obj && (obj.type === "USER_INPUT" || obj.source === "USER_EXPLICIT")) {
        lastUserIdx = parsed.length - 1;
      }
    } catch {}
  }
  if (lastUserIdx >= 0) {
    lastUserStep = parsed[lastUserIdx];
    for (let j = lastUserIdx + 1; j < parsed.length; j++) {
      const step = parsed[j];
      if (!step || !Array.isArray(step.tool_calls)) continue;
      for (const tc of step.tool_calls) {
        const tcName = String((tc && tc.name) || "");
        const tcArgs = (tc && tc.args) || {};
        if (FILE_WRITE_TOOLS.has(tcName)) {
          const targetPath = extractTargetArg(tcArgs);
          if (/\.artifacts[\\/]+(SPEC|PLAN)\.md$/.test(targetPath)) {
            modifiedAfterUserInTranscript = true;
          }
        } else if (tcName === "run_command") {
          const cmd = String(tcArgs.CommandLine || tcArgs.command || "");
          if (
            /\.artifacts[\\/]+(SPEC|PLAN)\.md/.test(cmd) &&
            /(?:>|>>|\btee\b|\bsed\b|\bperl\b|\bmv\b|\bcp\b|\brm\b|\bnode\b|\bpython3?\b)/.test(cmd)
          ) {
            modifiedAfterUserInTranscript = true;
          }
        }
      }
    }
  }
}

const cleanUserMessage = (raw) =>
  String(raw || "")
    .replace(/<ADDITIONAL_METADATA>[\s\S]*?<\/ADDITIONAL_METADATA>/gi, "")
    .replace(/<USER_SETTINGS_CHANGE>[\s\S]*?<\/USER_SETTINGS_CHANGE>/gi, "")
    .replace(/<\/?USER_REQUEST>/gi, "")
    .trim();

const rawTranscriptUserMsg = lastUserStep ? extractText(lastUserStep.content) : "";
const fallbackUserMsg = typeof input.lastUserInput === "string" ? input.lastUserInput : "";
const rawUserMsg = rawTranscriptUserMsg.trim() ? rawTranscriptUserMsg : fallbackUserMsg;
const cleanedUserMsg = cleanUserMessage(rawUserMsg);

const hasArtifactComments = cleanedUserMsg.includes("Comments on artifact URI:");
const hasDocApprovedPhrase = cleanedUserMsg.includes("The user has approved this document.");
const isUnapprovedArtifactComment = hasArtifactComments && !hasDocApprovedPhrase;

const isExplicitUserApproval = (msg) => {
  if (!msg) return false;
  if (msg.includes("The user has approved this document.")) return true;
  if (/[?？]\s*$/.test(msg)) return false;
  if (/(?:吗|呢|么)[。.!！?？]*\s*$/.test(msg)) return false;
  if (
    /\b(?:not\s+(?:approved?|confirmed?)|don\x27?t\s+(?:approve|proceed|start|execute)|do\s+not\s+(?:approve|proceed|start|execute)|hold\s+on|wait|pause|hesitat\w*|reconsider|revise|modify|change\s+the\s+plan|instead|except|however|but\s+(?:first|before|wait|not)|maybe|not\s+sure|uncertain|let\x27?s\s+(?:rethink|discuss|wait|pause))\b/i.test(
      msg
    )
  ) {
    return false;
  }
  if (
    /(?:不同意|未同意|没同意|不批准|未批准|没批准|未确认|不确认|暂不|别开始|先别|先不要|暂缓|暂停|等一下|等等|再想想|修改|改一下|调整|重写|补充|疑问|不确定|有问题|不过|但是先|不要按|不能)/.test(
      msg
    )
  ) {
    return false;
  }
  if (/^(?:can|could|should|would|why|what|how|when|where|which|who|is|are|do|does|did|will)\b/i.test(msg)) {
    return false;
  }
  return (
    /\b(?:proceed|approved?|lgtm|confirmed?|go\s+ahead|looks\s+good)\b/i.test(msg) ||
    /(?:同意|批准|确认|继续执行|按方案执行|可以开始|按此执行|准许)/.test(msg) ||
    /^(?:yes|ok|okay|好的|可以|行)[.!。！]?$/i.test(msg)
  );
};

const lastUserTimeMs =
  lastUserStep && lastUserStep.created_at ? Date.parse(lastUserStep.created_at) : NaN;
const maxContractMtimeMs =
  specExists && planExists
    ? Math.max(fs.statSync(specPath).mtimeMs, fs.statSync(planPath).mtimeMs)
    : 0;

const isModifiedAfterUser = Boolean(
  modifiedAfterUserInTranscript ||
    (lastUserStep && (!Number.isFinite(lastUserTimeMs) || maxContractMtimeMs > lastUserTimeMs))
);

const isApprovedViaState = Boolean(state && state.approvedHash === semanticHash);
const isApprovedViaTurn = Boolean(
  isContractActive &&
    !isUnapprovedArtifactComment &&
    !isModifiedAfterUser &&
    (lastUserStep ? Number.isFinite(lastUserTimeMs) : Boolean(fallbackUserMsg.trim())) &&
    isExplicitUserApproval(cleanedUserMsg)
);

const saveApprovedState = () => {
  try {
    fs.mkdirSync(stateDir, { recursive: true });
    fs.writeFileSync(
      statePath,
      JSON.stringify(
        {
          approvedHash: semanticHash,
          approvedAtMs: Date.now(),
          approvedStepIdx: (lastUserStep && lastUserStep.step_index) ?? 0,
        },
        null,
        2
      ) + "\n"
    );
  } catch {}
};

if (isAllowlistedSubagent) {
  if (isApprovedViaTurn && !isApprovedViaState) {
    saveApprovedState();
  }
  emit("allow");
}

if (isExitPlanMode) {
  const planText = String(args.plan || args.Plan || args.planContent || "");
  if (isContractActive) {
    if (isApprovedViaState || isApprovedViaTurn) {
      if (isApprovedViaTurn && !isApprovedViaState) saveApprovedState();
      emit("allow");
    }
    if (isUnapprovedArtifactComment || /\b(?:slice-builder|swarm|run_workflow)\b/i.test(planText)) {
      emit(
        "force_ask",
        "AGY Harness: Active .artifacts/SPEC.md and .artifacts/PLAN.md are not yet approved by the user. Confirm proceeding with this plan?"
      );
    }
    emit("allow");
  } else {
    const planNoUrls = planText
      .replace(/https?:\/\/[^\s`\x27")]+/gi, " ")
      .replace(/file:\/\/[^\s`\x27")]+/gi, " ");
    const pathRe =
      /(?:^|[\s`\x27"(\[,])(?:\.\/)?([A-Za-z0-9_.-]+(?:\/[A-Za-z0-9_.-]+)*(?:\/[A-Za-z0-9_.*-]+\.[A-Za-z][A-Za-z0-9]{0,9}|\/\*))(?=$|[\s`\x27"),:;\].])/g;
    const distinctPaths = new Set();
    let match;
    while ((match = pathRe.exec(planNoUrls)) !== null) {
      const p = match[1].replace(/\\/g, "/");
      if (
        p.startsWith(".artifacts/") ||
        p.startsWith(".agents/") ||
        p === "scripts/gate-check.sh" ||
        /(?:^|\/)harness_plan\.md$/i.test(p) ||
        isTierXsExemptPath(p)
      ) {
        continue;
      }
      distinctPaths.add(p);
    }
    const exceedsTierSPaths = distinctPaths.size >= 5;
    const mentionsLargeWorkflow = /\b(?:Tier\s*(?:M|L|XL)|slice-builder|swarm)\b/i.test(planText);
    const hasPhase1Stop =
      /(?:\.artifacts[\\/](?:SPEC|PLAN)\.md|harness_plan\.md|RequestFeedback)/i.test(planText) &&
      /(?:Phase\s*1|contract[- ]only|stop(?:\s+your|\s+the)?\s+turn|wait\s+for[\s\S]{0,40}?(?:Proceed|approval|confirm)|await[\s\S]{0,40}?(?:Proceed|approval|confirm)|停轮|等待[\s\S]{0,20}?(?:确认|Proceed)|仅起草契约)/i.test(
        planText
      );
    if (!hasPhase1Stop && (exceedsTierSPaths || mentionsLargeWorkflow)) {
      emit(
        "force_ask",
        "AGY Harness: Plan exceeds Tier S without an active approved contract. Draft .artifacts/{SPEC,PLAN}.md and wait for Proceed first, or confirm continuing?"
      );
    }
    emit("allow");
  }
}

// Gated execution tools (non-allowlisted invoke_subagent, define_subagent(EnableWriteTools), swarm(add), run_workflow)
if (!specExists || !planExists) {
  emit(
    "deny",
    "BLOCKED by AGY Harness Approval Gate: .artifacts/SPEC.md and .artifacts/PLAN.md must both exist and be approved by the user before invoking gated subagents or workflows."
  );
}

if (hasTemplatePlaceholder) {
  emit(
    "deny",
    "BLOCKED by AGY Harness Approval Gate: .artifacts/SPEC.md or .artifacts/PLAN.md still contains unfilled template placeholders. Draft the contract, sync <ArtifactDir>/harness_plan.md with ArtifactMetadata.RequestFeedback=true, stop your turn, and wait for the user to click Proceed."
  );
}

if (isArchived) {
  emit(
    "deny",
    "BLOCKED by AGY Harness Approval Gate: Current .artifacts/SPEC.md and .artifacts/PLAN.md are archived or completed. Draft a new contract and obtain user approval before invoking gated subagents or workflows."
  );
}

if (isUnapprovedArtifactComment) {
  emit(
    "deny",
    "BLOCKED by AGY Harness Approval Gate: The user commented on the review artifact without approving it. Revise .artifacts/SPEC.md or .artifacts/PLAN.md as requested, update <ArtifactDir>/harness_plan.md with RequestFeedback=true, end your turn, and wait for user approval."
  );
}

if (isApprovedViaState) {
  emit("allow");
}

if (isModifiedAfterUser) {
  emit(
    "deny",
    "BLOCKED by AGY Harness Approval Gate: .artifacts/SPEC.md or .artifacts/PLAN.md was created or substantively modified after the last user confirmation. Sync <ArtifactDir>/harness_plan.md with ArtifactMetadata.RequestFeedback=true, stop calling tools to end your turn, and wait for the user to click Proceed (or confirm) in the next turn."
  );
}

if (!isApprovedViaTurn) {
  emit(
    "force_ask",
    "AGY Harness: Confirm starting implementation against current .artifacts/SPEC.md and .artifacts/PLAN.md?"
  );
}

saveApprovedState();
emit("allow");
'
fi

if command -v python3 >/dev/null 2>&1; then
  exec python3 - <<'PY'
import datetime, hashlib, json, os, pathlib, posixpath, re, sys, tempfile

def emit(decision, reason=None):
    out = {"decision": decision}
    if reason:
        out["reason"] = reason
    sys.stdout.write(json.dumps(out, ensure_ascii=False) + "\n")
    sys.exit(0)

try:
    data = json.loads(os.environ.get("AGY_HARNESS_HOOK_STDIN") or "{}")
except Exception:
    emit("allow")

plugin_mode = os.environ.get("AGY_HARNESS_PLUGIN_MODE") == "true"
ws_list = data.get("workspacePaths") or []
raw_ws_root = (
    str(ws_list[0])
    if ws_list and ws_list[0]
    else (os.getcwd() if plugin_mode else (os.environ.get("AGY_HARNESS_SCRIPT_REPO_ROOT") or os.getcwd()))
)
try:
    ws_root = pathlib.Path(os.path.realpath(raw_ws_root))
except Exception:
    ws_root = pathlib.Path(raw_ws_root)

has_harness_in_ws = (
    (ws_root / ".agents" / "rules" / "agy-harness.md").exists()
    or (ws_root / ".artifacts" / "SPEC.md").exists()
    or (ws_root / ".artifacts" / "PLAN.md").exists()
)
if plugin_mode and not has_harness_in_ws:
    emit("allow")

tc = data.get("toolCall") or {}
tool_name = str(tc.get("name") or "")
args = tc.get("args") or {}

READ_ONLY_SUBAGENTS = {"arch-redteam", "staff-gatekeeper", "research"}
subagent_type = str(args.get("TypeName") or args.get("typeName") or args.get("subagent_type") or "").strip()

is_exit_plan_mode = tool_name == "exit_plan_mode"
is_allowlisted_sub = tool_name == "invoke_subagent" and subagent_type in READ_ONLY_SUBAGENTS
is_sub = tool_name == "invoke_subagent" and subagent_type not in READ_ONLY_SUBAGENTS
is_def = tool_name == "define_subagent" and args.get("EnableWriteTools") is True
is_swm = tool_name == "swarm" and str(args.get("action") or "").strip() == "add"
is_wf = tool_name == "run_workflow"
is_gated = is_sub or is_def or is_swm or is_wf

if not is_gated and not is_exit_plan_mode and not is_allowlisted_sub:
    emit("allow")

def strip_fences(text, strip_inline):
    out = []
    in_fence = False
    for line in str(text or "").replace("\r\n", "\n").split("\n"):
        if re.match(r"^\s*```", line):
            in_fence = not in_fence
            out.append("")
            continue
        if in_fence:
            out.append("")
            continue
        out.append(re.sub(r"`[^`]*`", "", line) if strip_inline else line)
    return "\n".join(out)

def strip_non_contract(text):
    cleaned = re.sub(
        r"<!--\s*non-contract\s*-->[\s\S]*?<!--\s*/non-contract\s*-->",
        "",
        str(text or "").replace("\r\n", "\n"),
        flags=re.IGNORECASE,
    )
    non_contract_re = re.compile(
        r"^##\s+(?:Implementation Notes(?:\s*/\s*Guardrails)?|Guardrails|实施备注|风险护栏)\b",
        re.IGNORECASE,
    )
    out = []
    skipping = False
    for ln in cleaned.split("\n"):
        if re.match(r"^##\s+", ln):
            skipping = bool(non_contract_re.match(ln))
        if not skipping:
            out.append(ln)
    return "\n".join(out)

def norm(t):
    lines = []
    for ln in strip_non_contract(t).split("\n"):
        ln = re.sub(r"^(\s*-\s*)\[[ xX/-]\](\s+)", r"\1[ ]\2", ln)
        ln = re.sub(r"^(\s*-\s*\*\*)(?:Status|状态)(:\*\*\s*).+$", r"\1Status\2<STATUS>", ln)
        if ln.strip():
            lines.append(ln)
    return "\n".join(lines)

def has_spec_template_marker(spec_txt):
    if not spec_txt.strip():
        return True
    stripped = strip_fences(spec_txt, True)
    return bool(
        re.search(r"^\s*-\s+\*\*Goal:\*\*\s+_Describe the concrete user or system problem being solved\._\s*$", stripped, re.MULTILINE)
        or re.search(r"<!--\s*HARNESS_TEMPLATE_UNFILLED\b", stripped, re.IGNORECASE)
    )

def has_plan_template_marker(plan_txt):
    if not plan_txt.strip():
        return True
    stripped = strip_fences(plan_txt, True)
    return bool(
        re.search(r"^\s*##\s+Slice 1:\s+\[Title\]\s*$", stripped, re.MULTILINE)
        or re.search(r"<!--\s*HARNESS_TEMPLATE_UNFILLED\b", stripped, re.IGNORECASE)
    )

def is_explicit_archive_marker(text):
    return bool(re.search(r"<!--\s*HARNESS_STATUS:\s*(?:ARCHIVED|DONE)\s*-->", strip_fences(text, True), re.IGNORECASE))

def count_spec_checkboxes(spec_txt):
    body = strip_fences(strip_non_contract(spec_txt), True)
    done, open_cnt = 0, 0
    for ln in body.split("\n"):
        m = re.match(r"^\s*[-*]\s+\[([ xX/])\](?:\s|$)", ln)
        if not m:
            continue
        if m.group(1) in ("x", "X"):
            done += 1
        else:
            open_cnt += 1
    return done, open_cnt

def count_plan_statuses(plan_txt):
    body = strip_fences(strip_non_contract(plan_txt), False)
    done, open_cnt = 0, 0
    for ln in body.split("\n"):
        m = (
            re.match(r"^\s*(?:[-*]\s*)?(?:\*\*)?(?:Status|状态)(?:\*\*)?\s*:\s*(?:\*\*)?\s*`?\[([ xX/])\]", ln, re.IGNORECASE)
            or re.match(r"^\s*[-*]\s+\[([ xX/])\](?:\s|$)", ln)
        )
        if not m:
            continue
        if m.group(1) in ("x", "X"):
            done += 1
        else:
            open_cnt += 1
    return done, open_cnt

spec_path = ws_root / ".artifacts" / "SPEC.md"
plan_path = ws_root / ".artifacts" / "PLAN.md"
spec_exists = spec_path.exists()
plan_exists = plan_path.exists()
spec_txt = spec_path.read_text(encoding="utf-8") if spec_exists else ""
plan_txt = plan_path.read_text(encoding="utf-8") if plan_exists else ""

has_template_placeholder = (
    not spec_exists
    or not plan_exists
    or has_spec_template_marker(spec_txt)
    or has_plan_template_marker(plan_txt)
)
is_explicitly_archived = (
    (spec_exists and is_explicit_archive_marker(spec_txt))
    or (plan_exists and is_explicit_archive_marker(plan_txt))
)
spec_done, spec_open = count_spec_checkboxes(spec_txt)
plan_done, plan_open = count_plan_statuses(plan_txt)
is_auto_archived = (
    not has_template_placeholder
    and spec_done >= 1
    and spec_open == 0
    and plan_done >= 1
    and plan_open == 0
)
is_archived = is_explicitly_archived or is_auto_archived
is_contract_active = (not has_template_placeholder) and (not is_archived)

sem_hash = hashlib.sha256(("SPEC\n" + norm(spec_txt) + "\nPLAN\n" + norm(plan_txt)).encode("utf-8")).hexdigest()
state_dir = pathlib.Path(os.environ.get("AGY_HARNESS_STATE_DIR") or os.environ.get("PLUGIN_DATA") or (pathlib.Path(tempfile.gettempdir()) / "agy-harness-approvals"))
state_key = hashlib.sha256((str(ws_root) + "::" + str(data.get("conversationId") or "default")).encode("utf-8")).hexdigest()[:24]
state_path = state_dir / f"{state_key}.json"

state = None
if state_path.exists():
    try:
        state = json.loads(state_path.read_text(encoding="utf-8"))
    except Exception:
        state = None

FILE_WRITE_TOOLS = {
    "write_file",
    "edit_file",
    "create_file",
    "delete_file",
    "sed_file",
    "write_to_file",
    "replace_file_content",
    "multi_replace_file_content",
}

def extract_target_arg(tc_args):
    if not isinstance(tc_args, dict):
        return ""
    for key in ("Path", "TargetFile", "TargetPath", "File", "path", "file", "file_path", "filePath", "target_file"):
        v = tc_args.get(key)
        if v:
            return str(v).strip('"').strip()
    return ""

def is_tier_xs_exempt_path(rel_path):
    rel = re.sub(r"^(?:\./)+", "", str(rel_path or "").replace("\\", "/"))
    if not rel or re.search(r"^(?:agents|skills|commands)(?:/|\Z)", rel):
        return False
    base = posixpath.basename(rel)
    return bool(
        re.search(r"\.(?:md|mdx|txt|rst|adoc)\Z", rel, re.IGNORECASE)
        or re.search(r"^(?:LICENSE(?:\.[A-Za-z0-9._-]+)?|NOTICE|AUTHORS|CONTRIBUTORS|PATENTS|CODEOWNERS|\.(?:gitignore|gitattributes|editorconfig|prettierignore|eslintignore|dockerignore|npmrc|nvmrc|node-version|python-version|tool-versions))\Z", base, re.IGNORECASE)
        or re.search(r"(?:^|/)(?:plugin|marketplace)\.json\Z", rel, re.IGNORECASE)
        or re.search(r"(?:^|/)(?:version\.txt|VERSION)\Z", rel)
        or re.search(r"(?:^|/)(?:package-lock\.json|npm-shrinkwrap\.json|pnpm-lock\.yaml|yarn\.lock|bun\.lockb?|Cargo\.lock|poetry\.lock|uv\.lock|Pipfile\.lock|go\.sum|pubspec\.lock|Gemfile\.lock|composer\.lock)\Z", rel)
        or re.search(r"\.(?:svg|png|jpe?g|gif|webp|ico|woff2?|ttf|otf|eot)\Z", rel, re.IGNORECASE)
    )

def extract_text(val):
    if isinstance(val, str):
        return val
    if isinstance(val, list):
        return "\n".join(extract_text(item) for item in val)
    if isinstance(val, dict):
        if isinstance(val.get("text"), str):
            return val["text"]
        if isinstance(val.get("content"), str):
            return val["content"]
        return json.dumps(val, ensure_ascii=False)
    return ""

last_user = None
mod_after = False
tp = data.get("transcriptPath")
if tp and pathlib.Path(tp).exists():
    parsed = []
    last_idx = -1
    for raw in pathlib.Path(tp).read_text(encoding="utf-8", errors="ignore").splitlines():
        if not raw.strip():
            continue
        try:
            obj = json.loads(raw)
            parsed.append(obj)
            if isinstance(obj, dict) and (obj.get("type") == "USER_INPUT" or obj.get("source") == "USER_EXPLICIT"):
                last_idx = len(parsed) - 1
        except Exception:
            pass
    if last_idx >= 0:
        last_user = parsed[last_idx]
        for step in parsed[last_idx + 1:]:
            if not isinstance(step, dict):
                continue
            for call in (step.get("tool_calls") or []):
                tc_name = str((call or {}).get("name") or "")
                tc_args = (call or {}).get("args") or {}
                if tc_name in FILE_WRITE_TOOLS:
                    target_path = extract_target_arg(tc_args)
                    if re.search(r"\.artifacts[\\/]+(SPEC|PLAN)\.md$", target_path):
                        mod_after = True
                elif tc_name == "run_command":
                    cmd_s = str(tc_args.get("CommandLine") or tc_args.get("command") or "")
                    if re.search(r"\.artifacts[\\/]+(SPEC|PLAN)\.md", cmd_s) and re.search(
                        r"(?:>|>>|\btee\b|\bsed\b|\bperl\b|\bmv\b|\bcp\b|\brm\b|\bnode\b|\bpython3?\b)", cmd_s
                    ):
                        mod_after = True

def clean_user_message(raw):
    s = re.sub(r"<ADDITIONAL_METADATA>[\s\S]*?</ADDITIONAL_METADATA>", "", str(raw or ""), flags=re.IGNORECASE)
    s = re.sub(r"<USER_SETTINGS_CHANGE>[\s\S]*?</USER_SETTINGS_CHANGE>", "", s, flags=re.IGNORECASE)
    s = re.sub(r"</?USER_REQUEST>", "", s, flags=re.IGNORECASE)
    return s.strip()

raw_transcript_msg = extract_text(last_user.get("content")) if last_user else ""
fallback_user_msg = data.get("lastUserInput") if isinstance(data.get("lastUserInput"), str) else ""
raw_user_msg = raw_transcript_msg if raw_transcript_msg.strip() else fallback_user_msg
cleaned_user_msg = clean_user_message(raw_user_msg)

has_artifact_comments = "Comments on artifact URI:" in cleaned_user_msg
has_doc_approved = "The user has approved this document." in cleaned_user_msg
is_unapproved_artifact_comment = has_artifact_comments and not has_doc_approved

def is_explicit_user_approval(msg):
    if not msg:
        return False
    if "The user has approved this document." in msg:
        return True
    if re.search(r"[?？]\s*$", msg):
        return False
    if re.search(r"(?:吗|呢|么)[。.!！?？]*\s*$", msg):
        return False
    if re.search(
        r"\b(?:not\s+(?:approved?|confirmed?)|don'?t\s+(?:approve|proceed|start|execute)|do\s+not\s+(?:approve|proceed|start|execute)|hold\s+on|wait|pause|hesitat\w*|reconsider|revise|modify|change\s+the\s+plan|instead|except|however|but\s+(?:first|before|wait|not)|maybe|not\s+sure|uncertain|let'?s\s+(?:rethink|discuss|wait|pause))\b",
        msg,
        re.IGNORECASE,
    ):
        return False
    if re.search(
        r"(?:不同意|未同意|没同意|不批准|未批准|没批准|未确认|不确认|暂不|别开始|先别|先不要|暂缓|暂停|等一下|等等|再想想|修改|改一下|调整|重写|补充|疑问|不确定|有问题|不过|但是先|不要按|不能)",
        msg,
    ):
        return False
    if re.match(r"^(?:can|could|should|would|why|what|how|when|where|which|who|is|are|do|does|did|will)\b", msg, re.IGNORECASE):
        return False
    return bool(
        re.search(r"\b(?:proceed|approved?|lgtm|confirmed?|go\s+ahead|looks\s+good)\b", msg, re.IGNORECASE)
        or re.search(r"(?:同意|批准|确认|继续执行|按方案执行|可以开始|按此执行|准许)", msg)
        or re.match(r"^(?:yes|ok|okay|好的|可以|行)[.!。！]?$", msg, re.IGNORECASE)
    )

last_ms = None
if last_user and last_user.get("created_at"):
    try:
        ts_raw = str(last_user["created_at"]).replace("Z", "+00:00")
        last_ms = datetime.datetime.fromisoformat(ts_raw).timestamp() * 1000.0
    except Exception:
        last_ms = None

max_mtime_ms = (
    max(spec_path.stat().st_mtime, plan_path.stat().st_mtime) * 1000.0
    if spec_exists and plan_exists
    else 0.0
)

is_modified_after = bool(
    mod_after or (last_user is not None and (last_ms is None or max_mtime_ms > last_ms))
)
is_approved_via_state = bool(state and state.get("approvedHash") == sem_hash)
is_approved_via_turn = bool(
    is_contract_active
    and not is_unapproved_artifact_comment
    and not is_modified_after
    and ((last_ms is not None) if last_user is not None else bool(fallback_user_msg.strip()))
    and is_explicit_user_approval(cleaned_user_msg)
)

def save_approved_state():
    try:
        state_dir.mkdir(parents=True, exist_ok=True)
        state_path.write_text(json.dumps({"approvedHash": sem_hash, "approvedStepIdx": (last_user or {}).get("step_index", 0)}, indent=2) + "\n", encoding="utf-8")
    except Exception:
        pass

if is_allowlisted_sub:
    if is_approved_via_turn and not is_approved_via_state:
        save_approved_state()
    emit("allow")

if is_exit_plan_mode:
    plan_text = str(args.get("plan") or args.get("Plan") or args.get("planContent") or "")
    if is_contract_active:
        if is_approved_via_state or is_approved_via_turn:
            if is_approved_via_turn and not is_approved_via_state:
                save_approved_state()
            emit("allow")
        if is_unapproved_artifact_comment or re.search(r"\b(?:slice-builder|swarm|run_workflow)\b", plan_text, re.IGNORECASE):
            emit("force_ask", "AGY Harness: Active .artifacts/SPEC.md and .artifacts/PLAN.md are not yet approved by the user. Confirm proceeding with this plan?")
        emit("allow")
    else:
        plan_no_urls = re.sub(r"https?://[^\s`'\")]+", " ", plan_text, flags=re.IGNORECASE)
        plan_no_urls = re.sub(r"file://[^\s`'\")]+", " ", plan_no_urls, flags=re.IGNORECASE)
        path_re = re.compile(
            r"(?:^|[\s`'\"(\[,])(?:\./)?([A-Za-z0-9_.-]+(?:/[A-Za-z0-9_.-]+)*(?:/[A-Za-z0-9_.*-]+\.[A-Za-z][A-Za-z0-9]{0,9}|/\*))(?=$|[\s`'\"),:;\].])"
        )
        distinct_paths = set()
        for m in path_re.finditer(plan_no_urls):
            p = m.group(1).replace("\\", "/")
            if (
                p.startswith(".artifacts/")
                or p.startswith(".agents/")
                or p == "scripts/gate-check.sh"
                or re.search(r"(?:^|/)harness_plan\.md$", p, re.IGNORECASE)
                or is_tier_xs_exempt_path(p)
            ):
                continue
            distinct_paths.add(p)
        exceeds_paths = len(distinct_paths) >= 5
        mentions_large = bool(re.search(r"\b(?:Tier\s*(?:M|L|XL)|slice-builder|swarm)\b", plan_text, re.IGNORECASE))
        has_phase1_stop = bool(
            re.search(r"(?:\.artifacts[\\/](?:SPEC|PLAN)\.md|harness_plan\.md|RequestFeedback)", plan_text, re.IGNORECASE)
            and re.search(
                r"(?:Phase\s*1|contract[- ]only|stop(?:\s+your|\s+the)?\s+turn|wait\s+for[\s\S]{0,40}?(?:Proceed|approval|confirm)|await[\s\S]{0,40}?(?:Proceed|approval|confirm)|停轮|等待[\s\S]{0,20}?(?:确认|Proceed)|仅起草契约)",
                plan_text,
                re.IGNORECASE,
            )
        )
        if not has_phase1_stop and (exceeds_paths or mentions_large):
            emit("force_ask", "AGY Harness: Plan exceeds Tier S without an active approved contract. Draft .artifacts/{SPEC,PLAN}.md and wait for Proceed first, or confirm continuing?")
        emit("allow")

if not spec_exists or not plan_exists:
    emit("deny", "BLOCKED by AGY Harness Approval Gate: .artifacts/SPEC.md and .artifacts/PLAN.md must both exist and be approved by the user before invoking gated subagents or workflows.")

if has_template_placeholder:
    emit("deny", "BLOCKED by AGY Harness Approval Gate: .artifacts/SPEC.md or .artifacts/PLAN.md still contains unfilled template placeholders. Draft the contract, sync <ArtifactDir>/harness_plan.md with ArtifactMetadata.RequestFeedback=true, stop your turn, and wait for the user to click Proceed.")

if is_archived:
    emit("deny", "BLOCKED by AGY Harness Approval Gate: Current .artifacts/SPEC.md and .artifacts/PLAN.md are archived or completed. Draft a new contract and obtain user approval before invoking gated subagents or workflows.")

if is_unapproved_artifact_comment:
    emit("deny", "BLOCKED by AGY Harness Approval Gate: The user commented on the review artifact without approving it. Revise .artifacts/SPEC.md or .artifacts/PLAN.md as requested, update <ArtifactDir>/harness_plan.md with RequestFeedback=true, end your turn, and wait for user approval.")

if is_approved_via_state:
    emit("allow")

if is_modified_after:
    emit("deny", "BLOCKED by AGY Harness Approval Gate: .artifacts/SPEC.md or .artifacts/PLAN.md was created or substantively modified after the last user confirmation. Sync <ArtifactDir>/harness_plan.md with ArtifactMetadata.RequestFeedback=true, stop calling tools to end your turn, and wait for the user to click Proceed (or confirm) in the next turn.")

if not is_approved_via_turn:
    emit("force_ask", "AGY Harness: Confirm starting implementation against current .artifacts/SPEC.md and .artifacts/PLAN.md?")

save_approved_state()
emit("allow")
PY
fi

echo '{"decision":"force_ask","reason":"AGY Harness: Confirm starting implementation against current .artifacts/SPEC.md and .artifacts/PLAN.md?"}'
