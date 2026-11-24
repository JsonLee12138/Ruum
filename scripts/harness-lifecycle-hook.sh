#!/bin/bash
set -euo pipefail

# AGY Harness Lifecycle Hook (PreInvocation, PostToolUse, Stop)
# - --plugin-mode: no-op unless workspace contains .agents/rules/agy-harness.md
#   or .artifacts/{SPEC,PLAN}.md; yields if workspace .agents/hooks.json already
#   declares "harness-lifecycle-guard".
# - PreInvocation: injects concise deduplicated state reminders for parent only
#   (ACTIVE_UNAPPROVED, ACTIVE_APPROVED, or INACTIVE 5-tier XS/S/M/L/XL routing).
# - PostToolUse: tracks workspace-scoped lastSourceEditAtMs (ignoring XS-exempt
#   docs/manifests/lockfiles/media) and lastFullGatePassAtMs.
# - Stop: enforces full gate + subagent cleanup once source files were edited
#   after the last full gate pass, with anti-loop and Phase-1 unapproved exemptions.

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
REPO_ROOT="$(cd "$SCRIPT_DIR/.." && pwd)"

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

export AGY_LIFECYCLE_REPO_ROOT="$REPO_ROOT"
export AGY_LIFECYCLE_PLUGIN_MODE="$PLUGIN_MODE"
export AGY_LIFECYCLE_STDIN="$HOOK_STDIN"

if command -v node >/dev/null 2>&1; then
  exec node -e '
"use strict";
const fs = require("node:fs");
const os = require("node:os");
const path = require("node:path");
const crypto = require("node:crypto");

const emit = (obj) => {
  process.stdout.write(JSON.stringify(obj) + "\n");
  process.exit(0);
};

let input = {};
try {
  input = JSON.parse(process.env.AGY_LIFECYCLE_STDIN || "{}");
} catch {
  emit({});
}

const eventName = String(input.hookEventName || input.hook_event_name || "");
const emitYield = () => emit(eventName === "Stop" ? { decision: "allow" } : {});

const resolveReal = (targetPath, baseDir) => {
  const parts = [];
  let cur = path.resolve(baseDir, targetPath);
  while (!fs.existsSync(cur)) {
    const parent = path.dirname(cur);
    if (parent === cur) break;
    parts.unshift(path.basename(cur));
    cur = parent;
  }
  let realBase = cur;
  try {
    realBase = fs.realpathSync(cur);
  } catch {}
  return parts.length ? path.join(realBase, ...parts) : realBase;
};

const pluginMode = process.env.AGY_LIFECYCLE_PLUGIN_MODE === "true";
const wsList = Array.isArray(input.workspacePaths) ? input.workspacePaths : [];
const rawWs =
  (wsList[0] && String(wsList[0])) ||
  (input.cwd && String(input.cwd)) ||
  (pluginMode ? process.cwd() : process.env.AGY_LIFECYCLE_REPO_ROOT) ||
  process.cwd();
const workspaceRoot = resolveReal(rawWs, process.cwd());

const hasHarnessInWs =
  fs.existsSync(path.join(workspaceRoot, ".agents", "rules", "agy-harness.md")) ||
  fs.existsSync(path.join(workspaceRoot, ".artifacts", "SPEC.md")) ||
  fs.existsSync(path.join(workspaceRoot, ".artifacts", "PLAN.md"));

if (pluginMode && !hasHarnessInWs) {
  emitYield();
}

if (pluginMode) {
  const wsHooksPath = path.join(workspaceRoot, ".agents", "hooks.json");
  if (fs.existsSync(wsHooksPath)) {
    try {
      const rawHooks = fs.readFileSync(wsHooksPath, "utf8");
      let hasLocalLifecycle = false;
      try {
        const parsedHooks = JSON.parse(rawHooks);
        hasLocalLifecycle = Boolean(
          parsedHooks &&
            typeof parsedHooks === "object" &&
            Object.prototype.hasOwnProperty.call(parsedHooks, "harness-lifecycle-guard")
        );
      } catch {
        hasLocalLifecycle = rawHooks.includes("\"harness-lifecycle-guard\"");
      }
      if (hasLocalLifecycle) {
        emitYield();
      }
    } catch {}
  }
}

const sha256 = (s) => crypto.createHash("sha256").update(s).digest("hex");
const stateDir =
  process.env.AGY_HARNESS_STATE_DIR ||
  process.env.PLUGIN_DATA ||
  path.join(os.tmpdir(), "agy-harness-approvals");
const convKey = sha256(workspaceRoot + "::" + String(input.conversationId || "default")).slice(0, 24);
const wsHash = sha256(workspaceRoot).slice(0, 24);

const EDIT_TOOLS = new Set([
  "write_file",
  "edit_file",
  "create_file",
  "delete_file",
  "sed_file",
  "write_to_file",
  "replace_file_content",
  "multi_replace_file_content",
]);

const isHarnessInternal = (relPath) =>
  relPath === ".artifacts" ||
  relPath.startsWith(".artifacts/") ||
  relPath === ".agents" ||
  relPath.startsWith(".agents/");

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

const extractTargetPath = (a) => {
  if (!a || typeof a !== "object") return "";
  return String(
    a.Path ||
      a.TargetFile ||
      a.TargetPath ||
      a.File ||
      a.path ||
      a.file ||
      a.file_path ||
      a.filePath ||
      a.target_file ||
      ""
  )
    .replace(/^"+|"+$/g, "")
    .trim();
};

const detectIsParent = (transcriptPath) => {
  if (!transcriptPath || !fs.existsSync(transcriptPath)) return false;
  let content = "";
  try {
    content = fs.readFileSync(transcriptPath, "utf8");
  } catch {
    return false;
  }
  let sawFirstSystem = false;
  let hasUserInput = false;
  for (const rawLine of content.split(/\r?\n/)) {
    if (!rawLine.trim()) continue;
    let obj;
    try {
      obj = JSON.parse(rawLine);
    } catch {
      continue;
    }
    if (!obj || typeof obj !== "object") continue;
    if (!sawFirstSystem && obj.type === "SYSTEM_MESSAGE") {
      sawFirstSystem = true;
      const msg =
        typeof obj.content === "string" ? obj.content : JSON.stringify(obj.content || "");
      if (msg.includes("[Message]") && msg.includes("sender=")) {
        return false;
      }
    }
    if (obj.type === "USER_INPUT" || obj.source === "USER_EXPLICIT") {
      hasUserInput = true;
    }
  }
  return hasUserInput;
};

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

const cleanUserMessage = (raw) =>
  String(raw || "")
    .replace(/<ADDITIONAL_METADATA>[\s\S]*?<\/ADDITIONAL_METADATA>/gi, "")
    .replace(/<USER_SETTINGS_CHANGE>[\s\S]*?<\/USER_SETTINGS_CHANGE>/gi, "")
    .replace(/<\/?USER_REQUEST>/gi, "")
    .trim();

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

const getContractState = () => {
  const specPath = path.join(workspaceRoot, ".artifacts", "SPEC.md");
  const planPath = path.join(workspaceRoot, ".artifacts", "PLAN.md");
  const specExists = fs.existsSync(specPath);
  const planExists = fs.existsSync(planPath);
  const specTxt = specExists ? fs.readFileSync(specPath, "utf8") : "";
  const planTxt = planExists ? fs.readFileSync(planPath, "utf8") : "";

  const isExplicitlyArchived =
    (specExists && isExplicitArchiveMarker(specTxt)) ||
    (planExists && isExplicitArchiveMarker(planTxt));
  const specFilled = specExists && !hasSpecTemplateMarker(specTxt);
  const planFilled = planExists && !hasPlanTemplateMarker(planTxt);
  const specCounts = countSpecCheckboxes(specTxt);
  const planCounts = countPlanStatuses(planTxt);
  const isAutoArchived =
    specFilled &&
    planFilled &&
    specCounts.done >= 1 &&
    specCounts.open === 0 &&
    planCounts.done >= 1 &&
    planCounts.open === 0;

  if (isExplicitlyArchived || isAutoArchived || (!specFilled && !planFilled)) {
    return { status: "INACTIVE", semanticHash: "" };
  }
  if (!specFilled || !planFilled) {
    return { status: "ACTIVE_UNAPPROVED", semanticHash: sha256("PARTIAL\n" + specTxt + "\n" + planTxt) };
  }

  const semanticHash = sha256(
    "SPEC\n" + normalizeContract(specTxt) + "\nPLAN\n" + normalizeContract(planTxt)
  );

  const approvalStatePath = path.join(stateDir, convKey + ".json");
  let approvalState = null;
  if (fs.existsSync(approvalStatePath)) {
    try {
      approvalState = JSON.parse(fs.readFileSync(approvalStatePath, "utf8"));
    } catch {
      approvalState = null;
    }
  }
  if (approvalState && approvalState.approvedHash === semanticHash) {
    return { status: "ACTIVE_APPROVED", semanticHash };
  }

  let lastUserStep = null;
  let modifiedAfterUserInTranscript = false;
  const tp = input.transcriptPath ? String(input.transcriptPath) : "";
  if (tp && fs.existsSync(tp)) {
    const lines = fs.readFileSync(tp, "utf8").split(/\r?\n/).filter(Boolean);
    let lastUserIdx = -1;
    const parsed = [];
    for (const line of lines) {
      try {
        const obj = JSON.parse(line);
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
          if (EDIT_TOOLS.has(tcName)) {
            if (/\.artifacts[\\/]+(SPEC|PLAN)\.md$/.test(extractTargetPath(tcArgs))) {
              modifiedAfterUserInTranscript = true;
            }
          } else if (tcName === "run_command") {
            const c = String(tcArgs.CommandLine || tcArgs.command || "");
            if (
              /\.artifacts[\\/]+(SPEC|PLAN)\.md/.test(c) &&
              /(?:>|>>|\btee\b|\bsed\b|\bperl\b|\bmv\b|\bcp\b|\brm\b|\bnode\b|\bpython3?\b)/.test(c)
            ) {
              modifiedAfterUserInTranscript = true;
            }
          }
        }
      }
    }
  }

  const rawTranscriptMsg = lastUserStep ? extractText(lastUserStep.content) : "";
  const fallbackUserMsg = typeof input.lastUserInput === "string" ? input.lastUserInput : "";
  const rawUserMsg = rawTranscriptMsg.trim() ? rawTranscriptMsg : fallbackUserMsg;
  const cleanedUserMsg = cleanUserMessage(rawUserMsg);
  const hasArtifactComments = cleanedUserMsg.includes("Comments on artifact URI:");
  const hasDocApproved = cleanedUserMsg.includes("The user has approved this document.");
  const isUnapprovedComment = hasArtifactComments && !hasDocApproved;
  const lastUserTimeMs =
    lastUserStep && lastUserStep.created_at ? Date.parse(lastUserStep.created_at) : NaN;
  const maxContractMtimeMs = Math.max(
    fs.statSync(specPath).mtimeMs,
    fs.statSync(planPath).mtimeMs
  );
  const isModifiedAfterUser = Boolean(
    modifiedAfterUserInTranscript ||
      (lastUserStep && (!Number.isFinite(lastUserTimeMs) || maxContractMtimeMs > lastUserTimeMs))
  );
  const isApprovedViaTurn = Boolean(
    !isUnapprovedComment &&
      !isModifiedAfterUser &&
      (lastUserStep ? Number.isFinite(lastUserTimeMs) : Boolean(fallbackUserMsg.trim())) &&
      isExplicitUserApproval(cleanedUserMsg)
  );

  return {
    status: isApprovedViaTurn ? "ACTIVE_APPROVED" : "ACTIVE_UNAPPROVED",
    semanticHash,
  };
};

const readJsonSafe = (p) => {
  if (!fs.existsSync(p)) return {};
  try {
    const parsed = JSON.parse(fs.readFileSync(p, "utf8"));
    return parsed && typeof parsed === "object" ? parsed : {};
  } catch {
    return {};
  }
};

const writeJsonSafe = (p, obj) => {
  try {
    fs.mkdirSync(stateDir, { recursive: true });
    fs.writeFileSync(p, JSON.stringify(obj, null, 2) + "\n");
  } catch {}
};

// Shell inspection helpers for PostToolUse
const classifyPathTarget = (rawP) => {
  const cleaned = String(rawP || "").trim().replace(/^[\x27"]|[\x27"]$/g, "");
  if (!cleaned) return { exempt: false };
  if (cleaned === "/dev/null" || /^\/(?:private\/)?tmp(?:\/|$)/.test(cleaned)) {
    return { exempt: true };
  }
  if (cleaned.startsWith("$") || cleaned.startsWith("~")) return { exempt: false };
  const abs = resolveReal(cleaned, workspaceRoot);
  if (abs === workspaceRoot) {
    return { exempt: false };
  }
  if (!abs.startsWith(workspaceRoot + path.sep)) {
    return { exempt: true };
  }
  const r = path.relative(workspaceRoot, abs).replace(/\\/g, "/");
  if (!r) return { exempt: false };
  if (isHarnessInternal(r) || isTierXsExemptPath(r)) return { exempt: true };
  return { exempt: false };
};

const splitHeredocs = (cmd) => {
  const lines = String(cmd || "").replace(/\r\n/g, "\n").split("\n");
  const items = [];
  let i = 0;
  while (i < lines.length) {
    const line = lines[i];
    const m = line.match(/<<(-?)\s*[\x27"]?([A-Za-z_][A-Za-z0-9_-]*)[\x27"]?/);
    if (m) {
      const stripTabs = m[1] === "-";
      const tag = m[2];
      const bodyLines = [];
      i++;
      while (i < lines.length) {
        const cmp = stripTabs ? lines[i].replace(/^\t+/, "") : lines[i];
        if (cmp === tag) {
          i++;
          break;
        }
        bodyLines.push(lines[i]);
        i++;
      }
      items.push({ line, heredoc: bodyLines.join("\n") });
    } else {
      items.push({ line, heredoc: "" });
      i++;
    }
  }
  return items;
};

const readWord = (s, startIdx) => {
  let i = startIdx;
  while (i < s.length && (s[i] === " " || s[i] === "\t")) i++;
  let out = "";
  let inS = false;
  let inD = false;
  while (i < s.length) {
    const ch = s[i];
    if (inS) {
      if (ch === "\x27") inS = false;
      else out += ch;
      i++;
    } else if (inD) {
      if (ch === "\\" && i + 1 < s.length) {
        out += s[i + 1];
        i += 2;
      } else if (ch === "\"") {
        inD = false;
        i++;
      } else {
        out += ch;
        i++;
      }
    } else {
      if (ch === "\\" && i + 1 < s.length) {
        out += s[i + 1];
        i += 2;
      } else if (ch === "\x27") {
        inS = true;
        i++;
      } else if (ch === "\"") {
        inD = true;
        i++;
      } else if (" \t;&|><".includes(ch)) {
        break;
      } else {
        out += ch;
        i++;
      }
    }
  }
  return { word: out, nextIdx: i };
};

const tokenizeShellLine = (line) => {
  const segments = [];
  let tokens = [];
  let redirects = [];
  let hasHeredoc = false;
  let pipedFromPrev = false;
  let cur = "";
  let curQuoted = false;
  let hasCur = false;
  let inS = false;
  let inD = false;

  const flushToken = () => {
    if (hasCur) {
      tokens.push({ value: cur, quoted: curQuoted });
      cur = "";
      curQuoted = false;
      hasCur = false;
    }
  };
  const flushSegment = (isPipe) => {
    flushToken();
    if (tokens.length > 0 || redirects.length > 0) {
      segments.push({ tokens, redirects, hasHeredoc, pipedFromPrev });
    }
    tokens = [];
    redirects = [];
    hasHeredoc = false;
    pipedFromPrev = isPipe;
  };

  let i = 0;
  while (i < line.length) {
    const ch = line[i];
    if (inS) {
      if (ch === "\x27") inS = false;
      else cur += ch;
      i++;
      continue;
    }
    if (inD) {
      if (ch === "\\" && i + 1 < line.length) {
        cur += line[i + 1];
        i += 2;
      } else if (ch === "\"") {
        inD = false;
        i++;
      } else {
        cur += ch;
        i++;
      }
      continue;
    }
    if (ch === "\\" && i + 1 < line.length) {
      hasCur = true;
      cur += line[i + 1];
      i += 2;
      continue;
    }
    if (ch === "\x27") {
      inS = true;
      curQuoted = true;
      hasCur = true;
      i++;
      continue;
    }
    if (ch === "\"") {
      inD = true;
      curQuoted = true;
      hasCur = true;
      i++;
      continue;
    }
    if (ch === " " || ch === "\t") {
      flushToken();
      i++;
      continue;
    }
    if (ch === ";") {
      flushSegment(false);
      i++;
      continue;
    }
    if (ch === "&" && line[i + 1] === "&") {
      flushSegment(false);
      i += 2;
      continue;
    }
    if (ch === "|" && line[i + 1] === "|") {
      flushSegment(false);
      i += 2;
      continue;
    }
    if (ch === "|") {
      flushSegment(true);
      i++;
      continue;
    }
    if (ch === "<") {
      if (line[i + 1] === "<") {
        flushToken();
        hasHeredoc = true;
        i += 2;
        if (line[i] === "-" || line[i] === "<") i++;
        const w = readWord(line, i);
        i = w.nextIdx;
      } else {
        if (hasCur && /^\d+$/.test(cur)) {
          cur = "";
          hasCur = false;
        } else {
          flushToken();
        }
        i++;
        if (line[i] === "&") {
          i++;
          while (i < line.length && /[0-9-]/.test(line[i])) i++;
        } else {
          const w = readWord(line, i);
          i = w.nextIdx;
        }
      }
      continue;
    }
    if (ch === ">") {
      if (hasCur && (cur === "=" || cur === "-") && !curQuoted) {
        cur += ch;
        i++;
        continue;
      }
      if (hasCur && (/^\d+$/.test(cur) || cur === "&") && !curQuoted) {
        cur = "";
        hasCur = false;
      } else {
        flushToken();
      }
      i++;
      if (line[i] === ">") i++;
      if (line[i] === "&") {
        i++;
        while (i < line.length && /[0-9-]/.test(line[i])) i++;
      } else {
        const w = readWord(line, i);
        redirects.push(w.word);
        i = w.nextIdx;
      }
      continue;
    }
    if (ch === "&" && line[i + 1] === ">") {
      flushToken();
      i += 2;
      if (line[i] === ">") i++;
      const w = readWord(line, i);
      redirects.push(w.word);
      i = w.nextIdx;
      continue;
    }
    hasCur = true;
    cur += ch;
    i++;
  }
  flushSegment(false);
  return segments;
};

const inspectNodeWrite = (code) => {
  const writeCallRe =
    /\b(?:writeFileSync|writeFile|appendFileSync|appendFile|createWriteStream|rmSync|rm|unlinkSync|unlink|renameSync|rename|copyFileSync|copyFile|cpSync|cp|truncateSync|truncate|openSync)\s*\(/g;
  let m;
  while ((m = writeCallRe.exec(code)) !== null) {
    const fn = m[0].replace(/\s*\($/, "");
    const openIdx = m.index + m[0].length - 1;
    let depth = 1;
    let j = openIdx + 1;
    let inS = false;
    let inD = false;
    let inB = false;
    while (j < code.length && depth > 0) {
      const c = code[j];
      if (inS) {
        if (c === "\\" && j + 1 < code.length) j += 2;
        else if (c === "\x27") { inS = false; j++; }
        else j++;
      } else if (inD) {
        if (c === "\\" && j + 1 < code.length) j += 2;
        else if (c === "\"") { inD = false; j++; }
        else j++;
      } else if (inB) {
        if (c === "\\" && j + 1 < code.length) j += 2;
        else if (c === "`") { inB = false; j++; }
        else j++;
      } else {
        if (c === "\x27") inS = true;
        else if (c === "\"") inD = true;
        else if (c === "`") inB = true;
        else if (c === "(") depth++;
        else if (c === ")") depth--;
        j++;
      }
    }
    const argSlice = code.slice(openIdx + 1, Math.max(openIdx + 1, j - 1));
    if (fn === "openSync" && /[\x27"]r/.test(argSlice) && !/[\x27"][wa+]/.test(argSlice)) continue;
    const literals = [];
    const litRe = /[\x27"`]([^\x27"`\r\n]+)[\x27"`]/g;
    let lm;
    while ((lm = litRe.exec(argSlice)) !== null) literals.push(lm[1]);
    const cands =
      fn === "renameSync" || fn === "rename" || fn === "copyFileSync" || fn === "copyFile" || fn === "cpSync" || fn === "cp"
        ? literals.slice(0, 2)
        : literals.slice(0, 1);
    if (cands.length === 0) return true;
    if (cands.some((p) => !classifyPathTarget(p).exempt)) return true;
  }
  return false;
};

const inspectPythonWrite = (code) => {
  const covered = [];
  const pathMethodRe =
    /Path\s*\(\s*([^\)]*)\)\s*\.\s*(?:write_text|write_bytes|unlink|rename|replace|mkdir|rmdir)\s*\(([^\)]*)\)/g;
  let pm;
  while ((pm = pathMethodRe.exec(code)) !== null) {
    covered.push([pm.index, pm.index + pm[0].length]);
    const pLit = pm[1].match(/[\x27"]([^\x27"\r\n]+)[\x27"]/);
    if (!pLit || !classifyPathTarget(pLit[1]).exempt) return true;
    if (/\.(?:rename|replace)\s*\(/.test(pm[0])) {
      const dLit = pm[2].match(/[\x27"]([^\x27"\r\n]+)[\x27"]/);
      if (!dLit || !classifyPathTarget(dLit[1]).exempt) return true;
    }
  }
  const bareWriteRe = /\.\s*(?:write_text|write_bytes|writelines)\s*\(/g;
  let bw;
  while ((bw = bareWriteRe.exec(code)) !== null) {
    if (!covered.some(([s, e]) => bw.index >= s && bw.index <= e)) return true;
  }
  const openRe = /\bopen\s*\(([^\)]*)\)/g;
  let om;
  while ((om = openRe.exec(code)) !== null) {
    const inside = om[1];
    const modeMatch = inside.match(/,\s*(?:mode\s*=\s*)?[\x27"]([rwaAxbtU+]+)[\x27"]/);
    if (modeMatch && !/[wax+]/i.test(modeMatch[1])) continue;
    if (!modeMatch && !/,/.test(inside)) continue;
    const pLit = inside.match(/^\s*[\x27"]([^\x27"\r\n]+)[\x27"]/);
    if (!pLit || !classifyPathTarget(pLit[1]).exempt) return true;
  }
  const osShutilRe = /\b(?:shutil\.(?:copy\w*|move|rmtree)|os\.(?:remove|unlink|rename|replace|rmdir|makedirs?))\s*\(([^\)]*)\)/g;
  let sm;
  while ((sm = osShutilRe.exec(code)) !== null) {
    const lits = [...sm[1].matchAll(/[\x27"]([^\x27"\r\n]+)[\x27"]/g)].map((x) => x[1]);
    if (lits.length === 0 || lits.some((p) => !classifyPathTarget(p).exempt)) return true;
  }
  return false;
};

const analyzeRunCommand = (cmd, depth = 0) => {
  let sawEdit = false;
  let sawFullGate = false;
  let editAfterGate = false;
  const markEdit = () => {
    sawEdit = true;
    editAfterGate = true;
  };
  const markGate = () => {
    sawFullGate = true;
    editAfterGate = false;
  };

  for (const item of splitHeredocs(cmd)) {
    const segs = tokenizeShellLine(item.line);
    for (let sIdx = 0; sIdx < segs.length; sIdx++) {
      const seg = segs[sIdx];
      for (const rTarget of seg.redirects) {
        if (!classifyPathTarget(rTarget).exempt) markEdit();
      }
      let idx = 0;
      const WRAPPERS = new Set(["command", "sudo", "env", "time", "nohup", "exec"]);
      while (idx < seg.tokens.length) {
        const val = seg.tokens[idx].value;
        if (/^[A-Za-z_][A-Za-z0-9_]*=/.test(val) || WRAPPERS.has(val)) {
          idx++;
          continue;
        }
        break;
      }
      if (idx >= seg.tokens.length) continue;

      const rawCmd = seg.tokens[idx].value;
      const cmdWord = path.posix.basename(rawCmd);
      const argVals = seg.tokens.slice(idx + 1).map((t) => t.value);

      // Full gate check detection: scripts/gate-check.sh without --fast
      const isDirectGate = /(?:^|\/)gate-check\.sh$/.test(rawCmd.replace(/\\/g, "/"));
      const isShellGate =
        (cmdWord === "sh" || cmdWord === "bash" || cmdWord === "zsh") &&
        argVals.some((a) => /(?:^|\/)gate-check\.sh$/.test(a.replace(/\\/g, "/")));
      if ((isDirectGate || isShellGate) && !argVals.includes("--fast")) {
        markGate();
      }

      if (cmdWord === "tee") {
        const files = argVals.filter((a) => !a.startsWith("-"));
        if (files.length === 0 || files.some((f) => !classifyPathTarget(f).exempt)) markEdit();
      } else if (cmdWord === "rm" || cmdWord === "unlink" || cmdWord === "touch") {
        let afterDd = false;
        for (let k = 0; k < argVals.length; k++) {
          const a = argVals[k];
          if (!afterDd && a === "--") { afterDd = true; continue; }
          if (!afterDd && a.startsWith("-")) {
            if (cmdWord === "touch" && (a === "-d" || a === "-t" || a === "-r")) k++;
            continue;
          }
          if (!classifyPathTarget(a).exempt) markEdit();
        }
      } else if (cmdWord === "mv") {
        const files = [];
        for (let k = 0; k < argVals.length; k++) {
          const a = argVals[k];
          if (a === "-t" || a === "-S") {
            if (a === "-t" && k + 1 < argVals.length) files.push(argVals[k + 1]);
            k++;
            continue;
          }
          if (a.startsWith("--target-directory=")) { files.push(a.slice(19)); continue; }
          if (a.startsWith("-")) continue;
          files.push(a);
        }
        if (files.length === 0 || files.some((f) => !classifyPathTarget(f).exempt)) markEdit();
      } else if (cmdWord === "cp" || cmdWord === "install") {
        let targetDir = "";
        const files = [];
        for (let k = 0; k < argVals.length; k++) {
          const a = argVals[k];
          if (a === "-t" && k + 1 < argVals.length) { targetDir = argVals[++k]; continue; }
          if (a.startsWith("--target-directory=")) { targetDir = a.slice(19); continue; }
          if (cmdWord === "install" && (a === "-m" || a === "-o" || a === "-g")) { k++; continue; }
          if (a.startsWith("-")) continue;
          files.push(a);
        }
        if (targetDir) { if (!classifyPathTarget(targetDir).exempt) markEdit(); }
        else if (files.length > 0) { if (!classifyPathTarget(files[files.length - 1]).exempt) markEdit(); }
        else if (cmdWord === "install" && argVals.some((a) => !a.startsWith("-"))) markEdit();
      } else if (cmdWord === "sed") {
        if (argVals.some((a) => /^-[A-Za-z]*i/.test(a) || a === "--in-place" || a.startsWith("--in-place="))) {
          let sawSf = false;
          const pos = [];
          for (let k = 0; k < argVals.length; k++) {
            const a = argVals[k];
            if (a === "-i") {
              if (k + 1 < argVals.length && argVals[k + 1] === "") k++;
              continue;
            }
            if (/^-[A-Za-z]*i/.test(a) || a === "--in-place" || a.startsWith("--in-place=")) continue;
            if (a === "-e" || a === "-f") { sawSf = true; k++; continue; }
            if (a.startsWith("--expression=") || a.startsWith("--file=")) { sawSf = true; continue; }
            if (a.startsWith("-")) continue;
            pos.push(a);
          }
          const files = sawSf ? pos : pos.slice(1);
          if (files.length === 0 || files.some((f) => !classifyPathTarget(f).exempt)) markEdit();
        }
      } else if (cmdWord === "perl") {
        if (argVals.some((a) => /^-[A-Za-z]*i/.test(a) || a === "--in-place" || a.startsWith("--in-place="))) {
          const files = [];
          for (let k = 0; k < argVals.length; k++) {
            const a = argVals[k];
            if (a === "-e" || a === "-E" || a === "-I" || a === "-M") { k++; continue; }
            if (a.startsWith("-")) continue;
            files.push(a);
          }
          if (files.length === 0 || files.some((f) => !classifyPathTarget(f).exempt)) markEdit();
        }
      } else if (cmdWord === "patch") {
        if (!argVals.some((a) => a === "--dry-run" || a === "-C")) markEdit();
      } else if (cmdWord === "dd") {
        for (const a of argVals) {
          if (a.startsWith("of=") && !classifyPathTarget(a.slice(3)).exempt) markEdit();
        }
      } else if (cmdWord === "truncate") {
        const files = [];
        for (let k = 0; k < argVals.length; k++) {
          const a = argVals[k];
          if (a === "-s" || a === "-r" || a === "--size" || a === "--reference") { k++; continue; }
          if (a.startsWith("-")) continue;
          files.push(a);
        }
        if (files.length === 0 || files.some((f) => !classifyPathTarget(f).exempt)) markEdit();
      } else if (cmdWord === "git") {
        let gitDirOpt = "";
        let k = 0;
        while (k < argVals.length) {
          const a = argVals[k];
          if (a === "-C" && k + 1 < argVals.length) { gitDirOpt = argVals[++k]; k++; continue; }
          if (a === "-c" || a === "--git-dir" || a === "--work-tree") { k += 2; continue; }
          if (a.startsWith("-")) { k++; continue; }
          break;
        }
        if (!(gitDirOpt && classifyPathTarget(gitDirOpt).exempt)) {
          const sub = argVals[k] || "";
          const subArgs = argVals.slice(k + 1);
          if (sub === "apply" && !subArgs.some((a) => a === "--check" || a === "--stat" || a === "--numstat" || a === "--summary")) {
            markEdit();
          } else if (sub === "restore") {
            const files = subArgs.filter((a) => a !== "--" && !a.startsWith("-"));
            if (files.length === 0 || files.some((f) => !classifyPathTarget(f).exempt)) markEdit();
          } else if (sub === "checkout" && subArgs.includes("--")) {
            const files = subArgs.slice(subArgs.indexOf("--") + 1);
            if (files.length === 0 || files.some((f) => !classifyPathTarget(f).exempt)) markEdit();
          } else if (sub === "reset" && subArgs.some((a) => a === "--hard" || a === "--merge")) {
            markEdit();
          } else if (sub === "clean" && !subArgs.some((a) => a === "--dry-run" || /^-[A-Za-z]*n/.test(a))) {
            markEdit();
          } else if (sub === "merge" || sub === "cherry-pick" || sub === "am" || sub === "revert" || sub === "rebase" || sub === "mv" || sub === "rm") {
            markEdit();
          }
        }
      } else if (cmdWord === "node") {
        const codes = [];
        for (let k = 0; k < argVals.length; k++) {
          const a = argVals[k];
          if ((a === "-e" || a === "--eval" || a === "-p" || a === "--print") && k + 1 < argVals.length) {
            codes.push(argVals[++k]);
          } else if (a.startsWith("--eval=")) codes.push(a.slice(7));
          else if (a === "-" && item.heredoc) codes.push(item.heredoc);
        }
        if ((seg.hasHeredoc || seg.pipedFromPrev) && item.heredoc) codes.push(item.heredoc);
        if (seg.pipedFromPrev && sIdx > 0) codes.push(segs[sIdx - 1].tokens.map((t) => t.value).join(" "));
        if (codes.some((c) => inspectNodeWrite(c))) markEdit();
      } else if (cmdWord === "python" || cmdWord === "python3") {
        const codes = [];
        for (let k = 0; k < argVals.length; k++) {
          const a = argVals[k];
          if (a === "-c" && k + 1 < argVals.length) codes.push(argVals[++k]);
          else if (a.startsWith("-c") && a.length > 2) codes.push(a.slice(2));
          else if (a === "-" && item.heredoc) codes.push(item.heredoc);
        }
        if ((seg.hasHeredoc || seg.pipedFromPrev) && item.heredoc) codes.push(item.heredoc);
        if (seg.pipedFromPrev && sIdx > 0) codes.push(segs[sIdx - 1].tokens.map((t) => t.value).join(" "));
        if (codes.some((c) => inspectPythonWrite(c))) markEdit();
      } else if ((cmdWord === "sh" || cmdWord === "bash" || cmdWord === "zsh") && depth < 1) {
        for (let k = 0; k < argVals.length - 1; k++) {
          if (argVals[k] === "-c") {
            const subRes = analyzeRunCommand(argVals[k + 1], depth + 1);
            if (subRes.sawEdit) {
              sawEdit = true;
              editAfterGate = subRes.editAfterGate;
            }
            if (subRes.sawFullGate) {
              sawFullGate = true;
              if (!subRes.editAfterGate) editAfterGate = false;
            }
          }
        }
      }
    }
  }
  return { sawEdit, sawFullGate, editAfterGate };
};

if (eventName === "PreInvocation") {
  const tp = input.transcriptPath ? String(input.transcriptPath) : "";
  if (!detectIsParent(tp)) {
    emit({});
  }
  const contract = getContractState();
  let signature = "";
  let message = "";
  if (contract.status === "ACTIVE_UNAPPROVED") {
    signature = "ACTIVE_UNAPPROVED:" + contract.semanticHash;
    message =
      "[AGY Harness] State: ACTIVE_UNAPPROVED. Sync .artifacts/{SPEC,PLAN}.md + <ArtifactDir>/harness_plan.md (RequestFeedback=true) and wait for Proceed before invoking slice-builder or editing source files.";
  } else if (contract.status === "ACTIVE_APPROVED") {
    signature = "ACTIVE_APPROVED:" + contract.semanticHash;
    message =
      "[AGY Harness] State: ACTIVE_APPROVED. Delegate source/test edits to slice-builder, run full bash scripts/gate-check.sh, verify with staff-gatekeeper, and kill finished slice subagents before finishing.";
  } else {
    signature = "INACTIVE_ROUTING_5TIER";
    message =
      "[AGY Harness] State: INACTIVE. Route strictly: Tier XS (doc/manifest/version/literal chore -> direct edit + fast check) | Tier S (<=3 code files bug/logic -> diagnose root cause + failing test RED->GREEN + gate-check.sh) | Tier M/L/XL (new feature/API/>3 code files -> draft .artifacts/{SPEC,PLAN}.md + Proceed first).";
  }

  const lifecyclePath = path.join(stateDir, convKey + "-lifecycle.json");
  const prev = readJsonSafe(lifecyclePath);
  if (prev.lastSignature === signature) {
    emit({});
  }
  writeJsonSafe(lifecyclePath, { lastSignature: signature, updatedAtMs: Date.now() });
  if (message) {
    emit({ injectSteps: [{ systemMessage: { systemMessage: message } }] });
  }
  emit({});
}

if (eventName === "PostToolUse") {
  if (input.error) {
    emit({});
  }
  const tc = input.toolCall || {};
  const toolName = String(tc.name || "");
  const args = tc.args || {};
  const wsStatePath = path.join(stateDir, wsHash + "-ws-lifecycle.json");
  const wsState = readJsonSafe(wsStatePath);
  let changed = false;

  if (EDIT_TOOLS.has(toolName)) {
    const rawTarget = extractTargetPath(args);
    if (!rawTarget) {
      wsState.lastSourceEditAtMs = Math.max(Date.now(), Number(wsState.lastFullGatePassAtMs || 0) + 1);
      changed = true;
    } else {
      const absTarget = resolveReal(rawTarget, workspaceRoot);
      if (absTarget.startsWith(workspaceRoot + path.sep) || absTarget === workspaceRoot) {
        const rel = path.relative(workspaceRoot, absTarget).replace(/\\/g, "/");
        if (rel && !isHarnessInternal(rel) && !isTierXsExemptPath(rel)) {
          wsState.lastSourceEditAtMs = Math.max(Date.now(), Number(wsState.lastFullGatePassAtMs || 0) + 1);
          changed = true;
        }
      }
    }
  } else if (toolName === "run_command") {
    const cmd = String(args.CommandLine || args.command || "");
    if (cmd.trim()) {
      const res = analyzeRunCommand(cmd, 0);
      if (res.sawFullGate) {
        wsState.lastFullGatePassAtMs = Math.max(Date.now(), Number(wsState.lastSourceEditAtMs || 0));
        changed = true;
      }
      if (res.sawEdit) {
        if (res.editAfterGate) {
          wsState.lastSourceEditAtMs = Math.max(Date.now(), Number(wsState.lastFullGatePassAtMs || 0) + 1);
        } else if (!res.sawFullGate) {
          wsState.lastSourceEditAtMs = Math.max(Date.now(), Number(wsState.lastFullGatePassAtMs || 0) + 1);
        } else {
          wsState.lastSourceEditAtMs = Number(wsState.lastFullGatePassAtMs || Date.now());
        }
        changed = true;
      }
    }
  }

  if (changed) {
    writeJsonSafe(wsStatePath, wsState);
  }
  emit({});
}

if (eventName === "Stop") {
  if (input.continuationActive || input.continuation_active) {
    emit({ decision: "allow" });
  }
  const tp = input.transcriptPath ? String(input.transcriptPath) : "";
  if (!detectIsParent(tp)) {
    emit({ decision: "allow" });
  }
  if (input.terminationReason && input.terminationReason !== "NO_TOOL_CALL") {
    emit({ decision: "allow" });
  }
  if (input.fullyIdle === false) {
    emit({ decision: "allow" });
  }
  const contract = getContractState();
  if (contract.status === "ACTIVE_UNAPPROVED") {
    emit({ decision: "allow" });
  }
  if (!fs.existsSync(path.join(workspaceRoot, "scripts", "gate-check.sh"))) {
    emit({ decision: "allow" });
  }
  const wsState = readJsonSafe(path.join(stateDir, wsHash + "-ws-lifecycle.json"));
  const lastSourceEditAtMs = Number(wsState.lastSourceEditAtMs || 0);
  const lastFullGatePassAtMs = Number(wsState.lastFullGatePassAtMs || 0);
  if (!(lastSourceEditAtMs > lastFullGatePassAtMs)) {
    emit({ decision: "allow" });
  }
  emit({
    decision: "continue",
    reason:
      "AGY Harness Stop Gate: Source files were modified since the last full gate pass. Run `bash scripts/gate-check.sh` and verify `manage_subagents(Action=\"list\")` has 0 leftover subagents before finishing.",
  });
}

emit({});
'
fi

if command -v python3 >/dev/null 2>&1; then
  exec python3 - <<'PY'
import datetime, hashlib, json, os, pathlib, posixpath, re, sys, tempfile, time

def emit(obj):
    sys.stdout.write(json.dumps(obj, ensure_ascii=False) + "\n")
    sys.exit(0)

try:
    data = json.loads(os.environ.get("AGY_LIFECYCLE_STDIN") or "{}")
except Exception:
    emit({})

event_name = str(data.get("hookEventName") or data.get("hook_event_name") or "")
def emit_yield():
    emit({"decision": "allow"} if event_name == "Stop" else {})

def resolve_real(target_path, base_dir):
    cur = pathlib.Path(os.path.abspath(os.path.join(str(base_dir), str(target_path))))
    parts = []
    while not cur.exists():
        parent = cur.parent
        if parent == cur:
            break
        parts.insert(0, cur.name)
        cur = parent
    try:
        real_base = pathlib.Path(os.path.realpath(str(cur)))
    except Exception:
        real_base = cur
    return real_base.joinpath(*parts) if parts else real_base

plugin_mode = os.environ.get("AGY_LIFECYCLE_PLUGIN_MODE") == "true"
ws_list = data.get("workspacePaths") or []
raw_ws = (
    str(ws_list[0])
    if ws_list and ws_list[0]
    else (str(data.get("cwd") or "") or (os.getcwd() if plugin_mode else (os.environ.get("AGY_LIFECYCLE_REPO_ROOT") or os.getcwd())))
)
ws_root = resolve_real(raw_ws, os.getcwd())

has_harness_in_ws = (
    (ws_root / ".agents" / "rules" / "agy-harness.md").exists()
    or (ws_root / ".artifacts" / "SPEC.md").exists()
    or (ws_root / ".artifacts" / "PLAN.md").exists()
)
if plugin_mode and not has_harness_in_ws:
    emit_yield()

if plugin_mode:
    ws_hooks_path = ws_root / ".agents" / "hooks.json"
    if ws_hooks_path.exists():
        try:
            raw_hooks = ws_hooks_path.read_text(encoding="utf-8")
            has_local = False
            try:
                parsed_hooks = json.loads(raw_hooks)
                has_local = isinstance(parsed_hooks, dict) and "harness-lifecycle-guard" in parsed_hooks
            except Exception:
                has_local = '"harness-lifecycle-guard"' in raw_hooks
            if has_local:
                emit_yield()
        except Exception:
            pass

def sha256_hex(s):
    return hashlib.sha256(s.encode("utf-8")).hexdigest()

state_dir = pathlib.Path(
    os.environ.get("AGY_HARNESS_STATE_DIR")
    or os.environ.get("PLUGIN_DATA")
    or (pathlib.Path(tempfile.gettempdir()) / "agy-harness-approvals")
)
conv_key = sha256_hex(str(ws_root) + "::" + str(data.get("conversationId") or "default"))[:24]
ws_hash = sha256_hex(str(ws_root))[:24]

EDIT_TOOLS = {
    "write_file",
    "edit_file",
    "create_file",
    "delete_file",
    "sed_file",
    "write_to_file",
    "replace_file_content",
    "multi_replace_file_content",
}

def is_harness_internal(rel_path):
    return rel_path in (".artifacts", ".agents") or rel_path.startswith(".artifacts/") or rel_path.startswith(".agents/")

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

def extract_target_path(a):
    if not isinstance(a, dict):
        return ""
    for key in ("Path", "TargetFile", "TargetPath", "File", "path", "file", "file_path", "filePath", "target_file"):
        v = a.get(key)
        if v:
            return str(v).strip('"').strip()
    return ""

def detect_is_parent(tp):
    if not tp or not os.path.exists(tp):
        return False
    try:
        lines = pathlib.Path(tp).read_text(encoding="utf-8", errors="ignore").splitlines()
    except Exception:
        return False
    saw_first_system = False
    has_user_input = False
    for line in lines:
        if not line.strip():
            continue
        try:
            obj = json.loads(line)
        except Exception:
            continue
        if not isinstance(obj, dict):
            continue
        if not saw_first_system and obj.get("type") == "SYSTEM_MESSAGE":
            saw_first_system = True
            c = obj.get("content")
            msg = c if isinstance(c, str) else json.dumps(c or "", ensure_ascii=False)
            if "[Message]" in msg and "sender=" in msg:
                return False
        if obj.get("type") == "USER_INPUT" or obj.get("source") == "USER_EXPLICIT":
            has_user_input = True
    return has_user_input

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

def norm_contract(t):
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

def extract_text(val):
    if isinstance(val, str):
        return val
    if isinstance(val, list):
        return "\n".join(extract_text(i) for i in val)
    if isinstance(val, dict):
        if isinstance(val.get("text"), str):
            return val["text"]
        if isinstance(val.get("content"), str):
            return val["content"]
        return json.dumps(val, ensure_ascii=False)
    return ""

def clean_user_message(raw):
    s = re.sub(r"<ADDITIONAL_METADATA>[\s\S]*?</ADDITIONAL_METADATA>", "", str(raw or ""), flags=re.IGNORECASE)
    s = re.sub(r"<USER_SETTINGS_CHANGE>[\s\S]*?</USER_SETTINGS_CHANGE>", "", s, flags=re.IGNORECASE)
    s = re.sub(r"</?USER_REQUEST>", "", s, flags=re.IGNORECASE)
    return s.strip()

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

def get_contract_state():
    spec_path = ws_root / ".artifacts" / "SPEC.md"
    plan_path = ws_root / ".artifacts" / "PLAN.md"
    spec_exists = spec_path.exists()
    plan_exists = plan_path.exists()
    spec_txt = spec_path.read_text(encoding="utf-8") if spec_exists else ""
    plan_txt = plan_path.read_text(encoding="utf-8") if plan_exists else ""

    is_explicitly_archived = (spec_exists and is_explicit_archive_marker(spec_txt)) or (plan_exists and is_explicit_archive_marker(plan_txt))
    spec_filled = spec_exists and not has_spec_template_marker(spec_txt)
    plan_filled = plan_exists and not has_plan_template_marker(plan_txt)
    sd, so = count_spec_checkboxes(spec_txt)
    pd, po = count_plan_statuses(plan_txt)
    is_auto_archived = spec_filled and plan_filled and sd >= 1 and so == 0 and pd >= 1 and po == 0

    if is_explicitly_archived or is_auto_archived or (not spec_filled and not plan_filled):
        return ("INACTIVE", "")
    if not spec_filled or not plan_filled:
        return ("ACTIVE_UNAPPROVED", sha256_hex("PARTIAL\n" + spec_txt + "\n" + plan_txt))

    sem_hash = sha256_hex("SPEC\n" + norm_contract(spec_txt) + "\nPLAN\n" + norm_contract(plan_txt))
    approval_path = state_dir / f"{conv_key}.json"
    if approval_path.exists():
        try:
            st = json.loads(approval_path.read_text(encoding="utf-8"))
            if isinstance(st, dict) and st.get("approvedHash") == sem_hash:
                return ("ACTIVE_APPROVED", sem_hash)
        except Exception:
            pass

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
                    if tc_name in EDIT_TOOLS:
                        if re.search(r"\.artifacts[\\/]+(SPEC|PLAN)\.md$", extract_target_path(tc_args)):
                            mod_after = True
                    elif tc_name == "run_command":
                        c = str(tc_args.get("CommandLine") or tc_args.get("command") or "")
                        if re.search(r"\.artifacts[\\/]+(SPEC|PLAN)\.md", c) and re.search(r"(?:>|>>|\btee\b|\bsed\b|\bperl\b|\bmv\b|\bcp\b|\brm\b|\bnode\b|\bpython3?\b)", c):
                            mod_after = True

    raw_transcript_msg = extract_text(last_user.get("content")) if last_user else ""
    fallback_user_msg = data.get("lastUserInput") if isinstance(data.get("lastUserInput"), str) else ""
    raw_user_msg = raw_transcript_msg if raw_transcript_msg.strip() else fallback_user_msg
    cleaned_user_msg = clean_user_message(raw_user_msg)
    has_comments = "Comments on artifact URI:" in cleaned_user_msg
    has_doc_approved = "The user has approved this document." in cleaned_user_msg
    is_unapproved_comment = has_comments and not has_doc_approved

    last_ms = None
    if last_user and last_user.get("created_at"):
        try:
            last_ms = datetime.datetime.fromisoformat(str(last_user["created_at"]).replace("Z", "+00:00")).timestamp() * 1000.0
        except Exception:
            last_ms = None
    max_mtime_ms = max(spec_path.stat().st_mtime, plan_path.stat().st_mtime) * 1000.0
    is_modified_after = bool(mod_after or (last_user is not None and (last_ms is None or max_mtime_ms > last_ms)))
    is_approved_via_turn = bool(
        not is_unapproved_comment
        and not is_modified_after
        and ((last_ms is not None) if last_user is not None else bool(fallback_user_msg.strip()))
        and is_explicit_user_approval(cleaned_user_msg)
    )
    return ("ACTIVE_APPROVED" if is_approved_via_turn else "ACTIVE_UNAPPROVED", sem_hash)

def read_json_safe(p):
    if not p.exists():
        return {}
    try:
        obj = json.loads(p.read_text(encoding="utf-8"))
        return obj if isinstance(obj, dict) else {}
    except Exception:
        return {}

def write_json_safe(p, obj):
    try:
        state_dir.mkdir(parents=True, exist_ok=True)
        p.write_text(json.dumps(obj, indent=2) + "\n", encoding="utf-8")
    except Exception:
        pass

def classify_path_target(raw_p):
    cleaned = str(raw_p or "").strip().strip("'\"")
    if not cleaned:
        return False
    if cleaned == "/dev/null" or re.match(r"^/(?:private/)?tmp(?:/|$)", cleaned):
        return True
    if cleaned.startswith("$") or cleaned.startswith("~"):
        return False
    abs_p = resolve_real(cleaned, ws_root)
    if abs_p == ws_root:
        return False
    try:
        r = abs_p.relative_to(ws_root).as_posix()
    except ValueError:
        return True
    if not r or r == ".":
        return False
    return is_harness_internal(r) or is_tier_xs_exempt_path(r)

def split_heredocs(cmd):
    lines = str(cmd or "").replace("\r\n", "\n").split("\n")
    items = []
    i = 0
    while i < len(lines):
        line = lines[i]
        m = re.search(r"<<(-?)\s*['\"]?([A-Za-z_][A-Za-z0-9_-]*)['\"]?", line)
        if m:
            strip_tabs = m.group(1) == "-"
            tag = m.group(2)
            body = []
            i += 1
            while i < len(lines):
                cmp = lines[i].lstrip("\t") if strip_tabs else lines[i]
                if cmp == tag:
                    i += 1
                    break
                body.append(lines[i])
                i += 1
            items.append((line, "\n".join(body)))
        else:
            items.append((line, ""))
            i += 1
    return items

def read_word(s, idx):
    i = idx
    n = len(s)
    while i < n and s[i] in (" ", "\t"):
        i += 1
    out = []
    in_s = in_d = False
    while i < n:
        ch = s[i]
        if in_s:
            if ch == "'": in_s = False
            else: out.append(ch)
            i += 1
        elif in_d:
            if ch == "\\" and i + 1 < n: out.append(s[i + 1]); i += 2
            elif ch == '"': in_d = False; i += 1
            else: out.append(ch); i += 1
        else:
            if ch == "\\" and i + 1 < n: out.append(s[i + 1]); i += 2
            elif ch == "'": in_s = True; i += 1
            elif ch == '"': in_d = True; i += 1
            elif ch in " \t;&|><": break
            else: out.append(ch); i += 1
    return "".join(out), i

def tokenize_shell_line(line):
    segments = []
    tokens = []
    redirects = []
    has_heredoc = False
    piped_from_prev = False
    cur = []
    cur_quoted = False
    has_cur = False
    in_s = in_d = False

    def flush_token():
        nonlocal cur, cur_quoted, has_cur
        if has_cur:
            tokens.append(("".join(cur), cur_quoted))
            cur = []
            cur_quoted = False
            has_cur = False

    def flush_segment(is_pipe):
        nonlocal tokens, redirects, has_heredoc, piped_from_prev
        flush_token()
        if tokens or redirects:
            segments.append((tokens, redirects, has_heredoc, piped_from_prev))
        tokens = []
        redirects = []
        has_heredoc = False
        piped_from_prev = is_pipe

    i = 0
    n = len(line)
    while i < n:
        ch = line[i]
        if in_s:
            if ch == "'": in_s = False
            else: cur.append(ch)
            i += 1; continue
        if in_d:
            if ch == "\\" and i + 1 < n: cur.append(line[i + 1]); i += 2
            elif ch == '"': in_d = False; i += 1
            else: cur.append(ch); i += 1
            continue
        if ch == "\\" and i + 1 < n:
            has_cur = True; cur.append(line[i + 1]); i += 2; continue
        if ch == "'":
            in_s = True; cur_quoted = True; has_cur = True; i += 1; continue
        if ch == '"':
            in_d = True; cur_quoted = True; has_cur = True; i += 1; continue
        if ch in (" ", "\t"):
            flush_token(); i += 1; continue
        if ch == ";":
            flush_segment(False); i += 1; continue
        if ch == "&" and i + 1 < n and line[i + 1] == "&":
            flush_segment(False); i += 2; continue
        if ch == "|" and i + 1 < n and line[i + 1] == "|":
            flush_segment(False); i += 2; continue
        if ch == "|":
            flush_segment(True); i += 1; continue
        if ch == "<":
            if i + 1 < n and line[i + 1] == "<":
                flush_token(); has_heredoc = True; i += 2
                if i < n and line[i] in ("-", "<"): i += 1
                _, i = read_word(line, i)
            else:
                if has_cur and "".join(cur).isdigit(): cur = []; has_cur = False
                else: flush_token()
                i += 1
                if i < n and line[i] == "&":
                    i += 1
                    while i < n and (line[i].isdigit() or line[i] == "-"): i += 1
                else:
                    _, i = read_word(line, i)
            continue
        if ch == ">":
            cur_s = "".join(cur)
            if has_cur and cur_s in ("=", "-") and not cur_quoted:
                cur.append(ch); i += 1; continue
            if has_cur and (cur_s.isdigit() or cur_s == "&") and not cur_quoted:
                cur = []; has_cur = False
            else:
                flush_token()
            i += 1
            if i < n and line[i] == ">": i += 1
            if i < n and line[i] == "&":
                i += 1
                while i < n and (line[i].isdigit() or line[i] == "-"): i += 1
            else:
                w, i = read_word(line, i)
                redirects.append(w)
            continue
        if ch == "&" and i + 1 < n and line[i + 1] == ">":
            flush_token(); i += 2
            if i < n and line[i] == ">": i += 1
            w, i = read_word(line, i)
            redirects.append(w)
            continue
        has_cur = True
        cur.append(ch)
        i += 1
    flush_segment(False)
    return segments

def inspect_node_write(code):
    write_re = re.compile(r"\b(?:writeFileSync|writeFile|appendFileSync|appendFile|createWriteStream|rmSync|rm|unlinkSync|unlink|renameSync|rename|copyFileSync|copyFile|cpSync|cp|truncateSync|truncate|openSync)\s*\(")
    for m in write_re.finditer(code):
        fn = m.group(0).rstrip("( \t")
        open_idx = m.end() - 1
        depth = 1
        j = open_idx + 1
        in_s = in_d = in_b = False
        while j < len(code) and depth > 0:
            c = code[j]
            if in_s:
                if c == "\\" and j + 1 < len(code): j += 2
                elif c == "'": in_s = False; j += 1
                else: j += 1
            elif in_d:
                if c == "\\" and j + 1 < len(code): j += 2
                elif c == '"': in_d = False; j += 1
                else: j += 1
            elif in_b:
                if c == "\\" and j + 1 < len(code): j += 2
                elif c == "`": in_b = False; j += 1
                else: j += 1
            else:
                if c == "'": in_s = True
                elif c == '"': in_d = True
                elif c == "`": in_b = True
                elif c == "(": depth += 1
                elif c == ")": depth -= 1
                j += 1
        arg_slice = code[open_idx + 1 : max(open_idx + 1, j - 1)]
        if fn == "openSync" and re.search(r"['\"]r", arg_slice) and not re.search(r"['\"][wa+]", arg_slice):
            continue
        literals = re.findall(r"['\"`]([^'\"`\r\n]+)['\"`]", arg_slice)
        cands = literals[:2] if fn in ("renameSync", "rename", "copyFileSync", "copyFile", "cpSync", "cp") else literals[:1]
        if not cands or any(not classify_path_target(p) for p in cands):
            return True
    return False

def inspect_python_write(code):
    covered = []
    for pm in re.finditer(r"Path\s*\(\s*([^)]*)\)\s*\.\s*(?:write_text|write_bytes|unlink|rename|replace|mkdir|rmdir)\s*\(([^)]*)\)", code):
        covered.append((pm.start(), pm.end()))
        plit = re.search(r"['\"]([^'\"\r\n]+)['\"]", pm.group(1))
        if not plit or not classify_path_target(plit.group(1)):
            return True
        if re.search(r"\.(?:rename|replace)\s*\(", pm.group(0)):
            dlit = re.search(r"['\"]([^'\"\r\n]+)['\"]", pm.group(2))
            if not dlit or not classify_path_target(dlit.group(1)):
                return True
    for bw in re.finditer(r"\.\s*(?:write_text|write_bytes|writelines)\s*\(", code):
        if not any(s <= bw.start() <= e for s, e in covered):
            return True
    for om in re.finditer(r"\bopen\s*\(([^)]*)\)", code):
        inside = om.group(1)
        mm = re.search(r",\s*(?:mode\s*=\s*)?['\"]([rwaAxbtU+]+)['\"]", inside)
        if mm and not re.search(r"[wax+]", mm.group(1), re.IGNORECASE):
            continue
        if not mm and "," not in inside:
            continue
        plit = re.match(r"^\s*['\"]([^'\"\r\n]+)['\"]", inside)
        if not plit or not classify_path_target(plit.group(1)):
            return True
    for sm in re.finditer(r"\b(?:shutil\.(?:copy\w*|move|rmtree)|os\.(?:remove|unlink|rename|replace|rmdir|makedirs?))\s*\(([^)]*)\)", code):
        lits = re.findall(r"['\"]([^'\"\r\n]+)['\"]", sm.group(1))
        if not lits or any(not classify_path_target(l) for l in lits):
            return True
    return False

def analyze_run_command(cmd_str, depth=0):
    saw_edit = False
    saw_full_gate = False
    edit_after_gate = False
    def mark_edit():
        nonlocal saw_edit, edit_after_gate
        saw_edit = True; edit_after_gate = True
    def mark_gate():
        nonlocal saw_full_gate, edit_after_gate
        saw_full_gate = True; edit_after_gate = False

    WRAPPERS = {"command", "sudo", "env", "time", "nohup", "exec"}
    for line, heredoc in split_heredocs(cmd_str):
        segs = tokenize_shell_line(line)
        for s_idx, (tokens, redirects, has_heredoc, piped_from_prev) in enumerate(segs):
            for rt in redirects:
                if not classify_path_target(rt):
                    mark_edit()
            idx = 0
            while idx < len(tokens):
                v = tokens[idx][0]
                if re.match(r"^[A-Za-z_][A-Za-z0-9_]*=", v) or v in WRAPPERS:
                    idx += 1
                else:
                    break
            if idx >= len(tokens):
                continue
            raw_cmd = tokens[idx][0]
            cmd_word = os.path.basename(raw_cmd)
            arg_vals = [t[0] for t in tokens[idx + 1:]]

            is_direct_gate = bool(re.search(r"(?:^|/)gate-check\.sh$", raw_cmd.replace("\\", "/")))
            is_shell_gate = cmd_word in ("sh", "bash", "zsh") and any(
                re.search(r"(?:^|/)gate-check\.sh$", a.replace("\\", "/")) for a in arg_vals
            )
            if (is_direct_gate or is_shell_gate) and "--fast" not in arg_vals:
                mark_gate()

            if cmd_word == "tee":
                files = [a for a in arg_vals if not a.startswith("-")]
                if not files or any(not classify_path_target(f) for f in files): mark_edit()
            elif cmd_word in ("rm", "unlink", "touch"):
                after_dd = False
                k = 0
                while k < len(arg_vals):
                    a = arg_vals[k]
                    if not after_dd and a == "--": after_dd = True; k += 1; continue
                    if not after_dd and a.startswith("-"):
                        if cmd_word == "touch" and a in ("-d", "-t", "-r"): k += 2; continue
                        k += 1; continue
                    if not classify_path_target(a): mark_edit()
                    k += 1
            elif cmd_word == "mv":
                files = []
                k = 0
                while k < len(arg_vals):
                    a = arg_vals[k]
                    if a in ("-t", "-S"):
                        if a == "-t" and k + 1 < len(arg_vals): files.append(arg_vals[k + 1])
                        k += 2; continue
                    if a.startswith("--target-directory="): files.append(a[19:]); k += 1; continue
                    if a.startswith("-"): k += 1; continue
                    files.append(a); k += 1
                if not files or any(not classify_path_target(f) for f in files): mark_edit()
            elif cmd_word in ("cp", "install"):
                target_dir = ""
                files = []
                k = 0
                while k < len(arg_vals):
                    a = arg_vals[k]
                    if a == "-t" and k + 1 < len(arg_vals): target_dir = arg_vals[k + 1]; k += 2; continue
                    if a.startswith("--target-directory="): target_dir = a[19:]; k += 1; continue
                    if cmd_word == "install" and a in ("-m", "-o", "-g"): k += 2; continue
                    if a.startswith("-"): k += 1; continue
                    files.append(a); k += 1
                if target_dir:
                    if not classify_path_target(target_dir): mark_edit()
                elif files:
                    if not classify_path_target(files[-1]): mark_edit()
                elif cmd_word == "install" and any(not a.startswith("-") for a in arg_vals):
                    mark_edit()
            elif cmd_word == "sed":
                if any(re.match(r"^-[A-Za-z]*i", a) or a == "--in-place" or a.startswith("--in-place=") for a in arg_vals):
                    saw_sf = False
                    pos = []
                    k = 0
                    while k < len(arg_vals):
                        a = arg_vals[k]
                        if a == "-i":
                            if k + 1 < len(arg_vals) and arg_vals[k + 1] == "": k += 2; continue
                            k += 1; continue
                        if re.match(r"^-[A-Za-z]*i", a) or a == "--in-place" or a.startswith("--in-place="): k += 1; continue
                        if a in ("-e", "-f"): saw_sf = True; k += 2; continue
                        if a.startswith("--expression=") or a.startswith("--file="): saw_sf = True; k += 1; continue
                        if a.startswith("-"): k += 1; continue
                        pos.append(a); k += 1
                    files = pos if saw_sf else pos[1:]
                    if not files or any(not classify_path_target(f) for f in files): mark_edit()
            elif cmd_word == "perl":
                if any(re.match(r"^-[A-Za-z]*i", a) or a == "--in-place" or a.startswith("--in-place=") for a in arg_vals):
                    files = []
                    k = 0
                    while k < len(arg_vals):
                        a = arg_vals[k]
                        if a in ("-e", "-E", "-I", "-M"): k += 2; continue
                        if a.startswith("-"): k += 1; continue
                        files.append(a); k += 1
                    if not files or any(not classify_path_target(f) for f in files): mark_edit()
            elif cmd_word == "patch":
                if not any(a in ("--dry-run", "-C") for a in arg_vals): mark_edit()
            elif cmd_word == "dd":
                for a in arg_vals:
                    if a.startswith("of=") and not classify_path_target(a[3:]): mark_edit()
            elif cmd_word == "truncate":
                files = []
                k = 0
                while k < len(arg_vals):
                    a = arg_vals[k]
                    if a in ("-s", "-r", "--size", "--reference"): k += 2; continue
                    if a.startswith("-"): k += 1; continue
                    files.append(a); k += 1
                if not files or any(not classify_path_target(f) for f in files): mark_edit()
            elif cmd_word == "git":
                git_dir = ""
                k = 0
                while k < len(arg_vals):
                    a = arg_vals[k]
                    if a == "-C" and k + 1 < len(arg_vals): git_dir = arg_vals[k + 1]; k += 2; continue
                    if a in ("-c", "--git-dir", "--work-tree"): k += 2; continue
                    if a.startswith("-"): k += 1; continue
                    break
                if not (git_dir and classify_path_target(git_dir)):
                    sub = arg_vals[k] if k < len(arg_vals) else ""
                    sub_args = arg_vals[k + 1:]
                    if sub == "apply" and not any(a in ("--check", "--stat", "--numstat", "--summary") for a in sub_args):
                        mark_edit()
                    elif sub == "restore":
                        files = [a for a in sub_args if a != "--" and not a.startswith("-")]
                        if not files or any(not classify_path_target(f) for f in files): mark_edit()
                    elif sub == "checkout" and "--" in sub_args:
                        files = sub_args[sub_args.index("--") + 1:]
                        if not files or any(not classify_path_target(f) for f in files): mark_edit()
                    elif sub == "reset" and any(a in ("--hard", "--merge") for a in sub_args):
                        mark_edit()
                    elif sub == "clean" and not any(a == "--dry-run" or re.match(r"^-[A-Za-z]*n", a) for a in sub_args):
                        mark_edit()
                    elif sub in ("merge", "cherry-pick", "am", "revert", "rebase", "mv", "rm"):
                        mark_edit()
            elif cmd_word == "node":
                codes = []
                k = 0
                while k < len(arg_vals):
                    a = arg_vals[k]
                    if a in ("-e", "--eval", "-p", "--print") and k + 1 < len(arg_vals):
                        codes.append(arg_vals[k + 1]); k += 2; continue
                    if a.startswith("--eval="): codes.append(a[7:])
                    elif a == "-" and heredoc: codes.append(heredoc)
                    k += 1
                if (has_heredoc or piped_from_prev) and heredoc: codes.append(heredoc)
                if piped_from_prev and s_idx > 0: codes.append(" ".join(t[0] for t in segs[s_idx - 1][0]))
                if any(inspect_node_write(c) for c in codes): mark_edit()
            elif cmd_word in ("python", "python3"):
                codes = []
                k = 0
                while k < len(arg_vals):
                    a = arg_vals[k]
                    if a == "-c" and k + 1 < len(arg_vals):
                        codes.append(arg_vals[k + 1]); k += 2; continue
                    if a.startswith("-c") and len(a) > 2: codes.append(a[2:])
                    elif a == "-" and heredoc: codes.append(heredoc)
                    k += 1
                if (has_heredoc or piped_from_prev) and heredoc: codes.append(heredoc)
                if piped_from_prev and s_idx > 0: codes.append(" ".join(t[0] for t in segs[s_idx - 1][0]))
                if any(inspect_python_write(c) for c in codes): mark_edit()
            elif cmd_word in ("sh", "bash", "zsh") and depth < 1:
                for k in range(len(arg_vals) - 1):
                    if arg_vals[k] == "-c":
                        se, sg, eg = analyze_run_command(arg_vals[k + 1], depth + 1)
                        if se:
                            saw_edit = True; edit_after_gate = eg
                        if sg:
                            saw_full_gate = True
                            if not eg: edit_after_gate = False
    return saw_edit, saw_full_gate, edit_after_gate

now_ms = lambda: int(time.time() * 1000)

if event_name == "PreInvocation":
    if not detect_is_parent(str(data.get("transcriptPath") or "")):
        emit({})
    status, sem_hash = get_contract_state()
    signature = ""
    message = ""
    if status == "ACTIVE_UNAPPROVED":
        signature = "ACTIVE_UNAPPROVED:" + sem_hash
        message = "[AGY Harness] State: ACTIVE_UNAPPROVED. Sync .artifacts/{SPEC,PLAN}.md + <ArtifactDir>/harness_plan.md (RequestFeedback=true) and wait for Proceed before invoking slice-builder or editing source files."
    elif status == "ACTIVE_APPROVED":
        signature = "ACTIVE_APPROVED:" + sem_hash
        message = "[AGY Harness] State: ACTIVE_APPROVED. Delegate source/test edits to slice-builder, run full bash scripts/gate-check.sh, verify with staff-gatekeeper, and kill finished slice subagents before finishing."
    else:
        signature = "INACTIVE_ROUTING_5TIER"
        message = "[AGY Harness] State: INACTIVE. Route strictly: Tier XS (doc/manifest/version/literal chore -> direct edit + fast check) | Tier S (<=3 code files bug/logic -> diagnose root cause + failing test RED->GREEN + gate-check.sh) | Tier M/L/XL (new feature/API/>3 code files -> draft .artifacts/{SPEC,PLAN}.md + Proceed first)."

    lc_path = state_dir / f"{conv_key}-lifecycle.json"
    prev = read_json_safe(lc_path)
    if prev.get("lastSignature") == signature:
        emit({})
    write_json_safe(lc_path, {"lastSignature": signature, "updatedAtMs": now_ms()})
    if message:
        emit({"injectSteps": [{"systemMessage": {"systemMessage": message}}]})
    emit({})

if event_name == "PostToolUse":
    if data.get("error"):
        emit({})
    tc = data.get("toolCall") or {}
    tool_name = str(tc.get("name") or "")
    args = tc.get("args") or {}
    ws_state_path = state_dir / f"{ws_hash}-ws-lifecycle.json"
    ws_state = read_json_safe(ws_state_path)
    changed = False

    if tool_name in EDIT_TOOLS:
        raw_target = extract_target_path(args)
        if not raw_target:
            ws_state["lastSourceEditAtMs"] = max(now_ms(), int(ws_state.get("lastFullGatePassAtMs") or 0) + 1)
            changed = True
        else:
            abs_t = resolve_real(raw_target, ws_root)
            try:
                rel = abs_t.relative_to(ws_root).as_posix()
                if rel == ".":
                    rel = ""
                if rel and not is_harness_internal(rel) and not is_tier_xs_exempt_path(rel):
                    ws_state["lastSourceEditAtMs"] = max(now_ms(), int(ws_state.get("lastFullGatePassAtMs") or 0) + 1)
                    changed = True
            except ValueError:
                pass
    elif tool_name == "run_command":
        cmd = str(args.get("CommandLine") or args.get("command") or "")
        if cmd.strip():
            se, sg, eg = analyze_run_command(cmd, 0)
            if sg:
                ws_state["lastFullGatePassAtMs"] = max(now_ms(), int(ws_state.get("lastSourceEditAtMs") or 0))
                changed = True
            if se:
                if eg or not sg:
                    ws_state["lastSourceEditAtMs"] = max(now_ms(), int(ws_state.get("lastFullGatePassAtMs") or 0) + 1)
                else:
                    ws_state["lastSourceEditAtMs"] = int(ws_state.get("lastFullGatePassAtMs") or now_ms())
                changed = True

    if changed:
        write_json_safe(ws_state_path, ws_state)
    emit({})

if event_name == "Stop":
    if data.get("continuationActive") or data.get("continuation_active"):
        emit({"decision": "allow"})
    if not detect_is_parent(str(data.get("transcriptPath") or "")):
        emit({"decision": "allow"})
    term_reason = data.get("terminationReason")
    if term_reason and term_reason != "NO_TOOL_CALL":
        emit({"decision": "allow"})
    if data.get("fullyIdle") is False:
        emit({"decision": "allow"})
    status, _ = get_contract_state()
    if status == "ACTIVE_UNAPPROVED":
        emit({"decision": "allow"})
    if not (ws_root / "scripts" / "gate-check.sh").exists():
        emit({"decision": "allow"})
    ws_state = read_json_safe(state_dir / f"{ws_hash}-ws-lifecycle.json")
    last_edit = float(ws_state.get("lastSourceEditAtMs") or 0)
    last_pass = float(ws_state.get("lastFullGatePassAtMs") or 0)
    if not (last_edit > last_pass):
        emit({"decision": "allow"})
    emit({
        "decision": "continue",
        "reason": 'AGY Harness Stop Gate: Source files were modified since the last full gate pass. Run `bash scripts/gate-check.sh` and verify `manage_subagents(Action="list")` has 0 leftover subagents before finishing.',
    })

emit({})
PY
fi

echo '{}'
