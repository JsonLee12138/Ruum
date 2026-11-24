#!/bin/bash
set -euo pipefail

# AGY Harness Software 1.0 PreToolUse Parent Source-Edit Guard.
# Blocks the top-level coordinator session from directly modifying project
# source/test files via edit tools or shell write bypasses when an active
# (non-template, non-archived) contract exists in .artifacts/.
# When INACTIVE (unfilled template or archived), parent direct edits (Tier XS/S)
# and run_command are allowed without cumulative dirty-file/line locks.
# Subagents (first SYSTEM_MESSAGE contains [Message] and sender=, or no USER_INPUT),
# edits under .artifacts/, .agents/, /tmp/, /dev/null, or outside repoRoot are allowed.

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

export AGY_GUARD_REPO_ROOT="$REPO_ROOT"
export AGY_GUARD_PLUGIN_MODE="$PLUGIN_MODE"
export AGY_GUARD_STDIN="$HOOK_STDIN"

if command -v node >/dev/null 2>&1; then
  exec node -e '
"use strict";
const fs = require("node:fs");
const path = require("node:path");

const emit = (decision, reason) => {
  process.stdout.write(JSON.stringify(reason ? { decision, reason } : { decision }) + "\n");
  process.exit(0);
};

let input = {};
try {
  input = JSON.parse(process.env.AGY_GUARD_STDIN || "{}");
} catch {
  emit("allow");
}

const pluginMode = process.env.AGY_GUARD_PLUGIN_MODE === "true";

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

const wsList = Array.isArray(input.workspacePaths) ? input.workspacePaths : [];
const rawRepo =
  (wsList[0] && fs.existsSync(String(wsList[0])))
    ? String(wsList[0])
    : (pluginMode ? process.cwd() : (process.env.AGY_GUARD_REPO_ROOT || process.cwd()));
const repoRoot = resolveReal(rawRepo, process.cwd());

const hasHarnessInWs =
  fs.existsSync(path.join(repoRoot, ".agents", "rules", "agy-harness.md")) ||
  fs.existsSync(path.join(repoRoot, ".artifacts", "SPEC.md")) ||
  fs.existsSync(path.join(repoRoot, ".artifacts", "PLAN.md"));
if (pluginMode && !hasHarnessInWs) {
  emit("allow");
}

const tc = input.toolCall || {};
const toolName = String(tc.name || "");
const args = tc.args || {};

const EDIT_TOOLS = new Set([
  "edit_file",
  "write_file",
  "create_file",
  "delete_file",
  "sed_file",
  "write_to_file",
  "replace_file_content",
  "multi_replace_file_content",
]);
const isEditTool = EDIT_TOOLS.has(toolName);
const isRunCommand = toolName === "run_command";

if (!isEditTool && !isRunCommand) {
  emit("allow");
}

const isHarnessInternal = (relPath) =>
  relPath === ".artifacts" ||
  relPath.startsWith(".artifacts/") ||
  relPath === ".agents" ||
  relPath.startsWith(".agents/");

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

let rawTarget = "";
let absTarget = "";
let rel = "";
if (isEditTool) {
  rawTarget = extractTargetPath(args);
  if (rawTarget) {
    absTarget = resolveReal(rawTarget, repoRoot);
    if (!absTarget.startsWith(repoRoot + path.sep)) {
      emit("allow");
    }
    rel = path.relative(repoRoot, absTarget).replace(/\\/g, "/");
    if (!rel || isHarnessInternal(rel)) {
      emit("allow");
    }
  }
}

// Deterministic 2-rule isParent:
// 1) If first SYSTEM_MESSAGE contains [Message] and sender= -> false (subagent)
// 2) Else if any entry has type === "USER_INPUT" or source === "USER_EXPLICIT" -> true (parent)
// 3) Else -> false
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
        typeof obj.content === "string"
          ? obj.content
          : JSON.stringify(obj.content || "");
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

const tp = input.transcriptPath ? String(input.transcriptPath) : "";
if (!detectIsParent(tp)) {
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

const checkContractActive = () => {
  const specPath = path.join(repoRoot, ".artifacts", "SPEC.md");
  const planPath = path.join(repoRoot, ".artifacts", "PLAN.md");
  const specExists = fs.existsSync(specPath);
  const planExists = fs.existsSync(planPath);
  const specTxt = specExists ? fs.readFileSync(specPath, "utf8") : "";
  const planTxt = planExists ? fs.readFileSync(planPath, "utf8") : "";

  if ((specExists && isExplicitArchiveMarker(specTxt)) || (planExists && isExplicitArchiveMarker(planTxt))) {
    return false;
  }
  const specFilled = specExists && !hasSpecTemplateMarker(specTxt);
  const planFilled = planExists && !hasPlanTemplateMarker(planTxt);
  if (!specFilled && !planFilled) {
    return false;
  }
  if (specFilled && planFilled) {
    const specCounts = countSpecCheckboxes(specTxt);
    const planCounts = countPlanStatuses(planTxt);
    if (specCounts.done >= 1 && specCounts.open === 0 && planCounts.done >= 1 && planCounts.open === 0) {
      return false;
    }
  }
  return true;
};

const isContractActive = checkContractActive();

if (!isContractActive) {
  emit("allow");
}

if (isEditTool) {
  if (!rawTarget) {
    emit(
      "force_ask",
      "AGY Harness Source-Edit Guard: Edit tool `" +
        toolName +
        "` has no recognizable target path while a contract is active. Confirm allowing this edit?"
    );
  }

  emit(
    "deny",
    "BLOCKED by AGY Harness Source-Edit Guard: Active contract found in .artifacts/SPEC.md or .artifacts/PLAN.md. Top-level coordinator MUST NOT directly edit `" +
      rel +
      "`. Wait for user Proceed and delegate ALL implementation/rework edits to `slice-builder` via `invoke_subagent` or `send_message`."
  );
}

// run_command shell write-bypass guard
const classifyPathTarget = (rawP) => {
  const cleaned = String(rawP || "").trim().replace(/^[\x27"]|[\x27"]$/g, "");
  if (!cleaned) return { exempt: false, rel: "" };
  if (cleaned === "/dev/null") return { exempt: true, rel: "" };
  if (/^\/(?:private\/)?tmp(?:\/|$)/.test(cleaned)) return { exempt: true, rel: "" };
  if (cleaned.startsWith("$") || cleaned.startsWith("~")) return { exempt: false, rel: "" };
  const abs = resolveReal(cleaned, repoRoot);
  if (abs === repoRoot) {
    return { exempt: false, rel: "" };
  }
  if (!abs.startsWith(repoRoot + path.sep)) {
    return { exempt: true, rel: "" };
  }
  const r = path.relative(repoRoot, abs).replace(/\\/g, "/");
  if (!r) {
    return { exempt: false, rel: "" };
  }
  if (isHarnessInternal(r)) {
    return { exempt: true, rel: "" };
  }
  return { exempt: false, rel: r };
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

const inspectNodeCode = (code) => {
  const writeCallRe =
    /\b(?:writeFileSync|writeFile|appendFileSync|appendFile|createWriteStream|rmSync|rm|unlinkSync|unlink|renameSync|rename|copyFileSync|copyFile|cpSync|cp|truncateSync|truncate|openSync)\s*\(/g;
  const targets = [];
  let untargeted = false;
  let matched = false;
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
    if (fn === "openSync" && /[\x27"]r/.test(argSlice) && !/[\x27"][wa+]/.test(argSlice)) {
      continue;
    }
    const literals = [];
    const litRe = /[\x27"`]([^\x27"`\r\n]+)[\x27"`]/g;
    let lm;
    while ((lm = litRe.exec(argSlice)) !== null) {
      literals.push(lm[1]);
    }
    const pathCandidates =
      fn === "renameSync" || fn === "rename" || fn === "copyFileSync" || fn === "copyFile" || fn === "cpSync" || fn === "cp"
        ? literals.slice(0, 2)
        : literals.slice(0, 1);
    if (pathCandidates.length === 0) {
      matched = true;
      untargeted = true;
    } else {
      for (const p of pathCandidates) {
        const cls = classifyPathTarget(p);
        if (!cls.exempt) {
          matched = true;
          if (cls.rel) targets.push(cls.rel);
          else untargeted = true;
        }
      }
    }
  }
  return { matched, targets, untargeted };
};

const inspectPythonCode = (code) => {
  const targets = [];
  let untargeted = false;
  let matched = false;
  const checkLit = (p) => {
    const cls = classifyPathTarget(p);
    if (!cls.exempt) {
      matched = true;
      if (cls.rel) targets.push(cls.rel);
      else untargeted = true;
    }
  };
  // Path("...").write_text(...) etc.
  const pathMethodRe =
    /Path\s*\(\s*([^\)]*)\)\s*\.\s*(?:write_text|write_bytes|unlink|rename|replace|mkdir|rmdir)\s*\(([^\)]*)\)/g;
  let coveredRanges = [];
  let pm;
  while ((pm = pathMethodRe.exec(code)) !== null) {
    coveredRanges.push([pm.index, pm.index + pm[0].length]);
    const pathLit = pm[1].match(/[\x27"]([^\x27"\r\n]+)[\x27"]/);
    if (!pathLit) {
      matched = true;
      untargeted = true;
    } else {
      checkLit(pathLit[1]);
    }
    if (/\.(?:rename|replace)\s*\(/.test(pm[0])) {
      const dstLit = pm[2].match(/[\x27"]([^\x27"\r\n]+)[\x27"]/);
      if (!dstLit) { matched = true; untargeted = true; }
      else checkLit(dstLit[1]);
    }
  }
  // Bare .write_text / .write_bytes / .writelines when Path was assigned to a variable
  const bareWriteRe = /\.\s*(?:write_text|write_bytes|writelines)\s*\(/g;
  let bw;
  while ((bw = bareWriteRe.exec(code)) !== null) {
    const insideCovered = coveredRanges.some(([s, e]) => bw.index >= s && bw.index <= e);
    if (!insideCovered) {
      matched = true;
      untargeted = true;
    }
  }
  // open(path, mode)
  const openRe = /\bopen\s*\(([^\)]*)\)/g;
  let om;
  while ((om = openRe.exec(code)) !== null) {
    const inside = om[1];
    const modeMatch = inside.match(/,\s*(?:mode\s*=\s*)?[\x27"]([rwaAxbtU+]+)[\x27"]/);
    if (modeMatch && !/[wax+]/i.test(modeMatch[1])) continue;
    if (!modeMatch && !/,/.test(inside)) continue;
    const pLit = inside.match(/^\s*[\x27"]([^\x27"\r\n]+)[\x27"]/);
    if (!pLit) { matched = true; untargeted = true; }
    else checkLit(pLit[1]);
  }
  // shutil / os file mutations
  const osShutilRe = /\b(?:shutil\.(?:copy\w*|move|rmtree)|os\.(?:remove|unlink|rename|replace|rmdir|makedirs?))\s*\(([^\)]*)\)/g;
  let sm;
  while ((sm = osShutilRe.exec(code)) !== null) {
    const lits = [...sm[1].matchAll(/[\x27"]([^\x27"\r\n]+)[\x27"]/g)].map((x) => x[1]);
    if (lits.length === 0) { matched = true; untargeted = true; }
    else lits.forEach(checkLit);
  }
  return { matched, targets, untargeted };
};

const detectShellWriteBypass = (cmd, depth = 0) => {
  const result = { matched: false, targets: [], untargeted: false, summaries: [] };
  const record = (cls, label) => {
    if (!cls.exempt) {
      result.matched = true;
      result.summaries.push(label);
      if (cls.rel) result.targets.push(cls.rel);
      else result.untargeted = true;
    }
  };
  const recordUntargeted = (label) => {
    result.matched = true;
    result.untargeted = true;
    result.summaries.push(label);
  };

  for (const item of splitHeredocs(cmd)) {
    const segs = tokenizeShellLine(item.line);
    for (let sIdx = 0; sIdx < segs.length; sIdx++) {
      const seg = segs[sIdx];
      for (const rTarget of seg.redirects) {
        record(classifyPathTarget(rTarget), "redirection");
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

      const cmdWord = path.posix.basename(seg.tokens[idx].value);
      const argTokens = seg.tokens.slice(idx + 1);
      const argVals = argTokens.map((t) => t.value);

      if (cmdWord === "tee") {
        const files = argVals.filter((a) => !a.startsWith("-"));
        if (files.length === 0) recordUntargeted("tee");
        else files.forEach((f) => record(classifyPathTarget(f), "tee"));
      } else if (cmdWord === "rm" || cmdWord === "unlink" || cmdWord === "touch") {
        let afterDashDash = false;
        const files = [];
        for (let k = 0; k < argVals.length; k++) {
          const a = argVals[k];
          if (!afterDashDash && a === "--") { afterDashDash = true; continue; }
          if (!afterDashDash && a.startsWith("-")) {
            if (cmdWord === "touch" && (a === "-d" || a === "-t" || a === "-r")) k++;
            continue;
          }
          files.push(a);
        }
        files.forEach((f) => record(classifyPathTarget(f), cmdWord));
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
        if (files.length === 0) recordUntargeted("mv");
        else files.forEach((f) => record(classifyPathTarget(f), "mv"));
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
        if (targetDir) record(classifyPathTarget(targetDir), cmdWord);
        else if (files.length > 0) record(classifyPathTarget(files[files.length - 1]), cmdWord);
        else if (cmdWord === "install" && argVals.some((a) => !a.startsWith("-"))) recordUntargeted("install");
      } else if (cmdWord === "sed") {
        const inPlace = argVals.some((a) => /^-[A-Za-z]*i/.test(a) || a === "--in-place" || a.startsWith("--in-place="));
        if (inPlace) {
          let sawScriptFlag = false;
          const pos = [];
          for (let k = 0; k < argTokens.length; k++) {
            const a = argTokens[k].value;
            if (a === "-i") {
              if (k + 1 < argTokens.length && argTokens[k + 1].value === "") k++;
              continue;
            }
            if (/^-[A-Za-z]*i/.test(a) || a === "--in-place" || a.startsWith("--in-place=")) continue;
            if (a === "-e" || a === "-f") { sawScriptFlag = true; k++; continue; }
            if (a.startsWith("--expression=") || a.startsWith("--file=")) { sawScriptFlag = true; continue; }
            if (a.startsWith("-")) continue;
            pos.push(a);
          }
          const files = sawScriptFlag ? pos : pos.slice(1);
          if (files.length === 0) recordUntargeted("sed -i");
          else files.forEach((f) => record(classifyPathTarget(f), "sed -i"));
        }
      } else if (cmdWord === "perl") {
        const inPlace = argVals.some((a) => /^-[A-Za-z]*i/.test(a) || a === "--in-place" || a.startsWith("--in-place="));
        if (inPlace) {
          const files = [];
          for (let k = 0; k < argVals.length; k++) {
            const a = argVals[k];
            if (a === "-e" || a === "-E" || a === "-I" || a === "-M") { k++; continue; }
            if (a.startsWith("-")) continue;
            files.push(a);
          }
          if (files.length === 0) recordUntargeted("perl -i");
          else files.forEach((f) => record(classifyPathTarget(f), "perl -i"));
        }
      } else if (cmdWord === "patch") {
        const isDry = argVals.some((a) => a === "--dry-run" || a === "-C");
        if (!isDry) {
          let dirTarget = "";
          let outTarget = "";
          for (let k = 0; k < argVals.length; k++) {
            const a = argVals[k];
            if ((a === "-d" || a === "--directory") && k + 1 < argVals.length) dirTarget = argVals[++k];
            else if (a.startsWith("--directory=")) dirTarget = a.slice(12);
            else if ((a === "-o" || a === "--output") && k + 1 < argVals.length) outTarget = argVals[++k];
          }
          if (outTarget) record(classifyPathTarget(outTarget), "patch");
          else if (dirTarget) record(classifyPathTarget(dirTarget), "patch");
          else recordUntargeted("patch");
        }
      } else if (cmdWord === "dd") {
        for (const a of argVals) {
          if (a.startsWith("of=")) record(classifyPathTarget(a.slice(3)), "dd of=");
        }
      } else if (cmdWord === "truncate") {
        const files = [];
        for (let k = 0; k < argVals.length; k++) {
          const a = argVals[k];
          if (a === "-s" || a === "-r" || a === "--size" || a === "--reference") { k++; continue; }
          if (a.startsWith("-")) continue;
          files.push(a);
        }
        if (files.length === 0) recordUntargeted("truncate");
        else files.forEach((f) => record(classifyPathTarget(f), "truncate"));
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
        if (gitDirOpt && classifyPathTarget(gitDirOpt).exempt) continue;
        const sub = argVals[k] || "";
        const subArgs = argVals.slice(k + 1);
        if (sub === "apply") {
          const dry = subArgs.some((a) => a === "--check" || a === "--stat" || a === "--numstat" || a === "--summary");
          if (!dry) recordUntargeted("git apply");
        } else if (sub === "restore") {
          const files = subArgs.filter((a) => a !== "--" && !a.startsWith("-"));
          if (files.length === 0) recordUntargeted("git restore");
          else files.forEach((f) => record(classifyPathTarget(f), "git restore"));
        } else if (sub === "checkout") {
          const dd = subArgs.indexOf("--");
          if (dd >= 0) {
            const files = subArgs.slice(dd + 1);
            if (files.length === 0) recordUntargeted("git checkout --");
            else files.forEach((f) => record(classifyPathTarget(f), "git checkout --"));
          }
        } else if (sub === "reset") {
          if (subArgs.some((a) => a === "--hard" || a === "--merge")) recordUntargeted("git reset --hard");
        } else if (sub === "clean") {
          const dry = subArgs.some((a) => a === "--dry-run" || /^-[A-Za-z]*n/.test(a));
          if (!dry) recordUntargeted("git clean");
        } else if (sub === "merge" || sub === "cherry-pick" || sub === "am" || sub === "revert" || sub === "rebase" || sub === "mv" || sub === "rm") {
          recordUntargeted("git " + sub);
        }
      } else if (cmdWord === "node") {
        const codes = [];
        for (let k = 0; k < argVals.length; k++) {
          const a = argVals[k];
          if ((a === "-e" || a === "--eval" || a === "-p" || a === "--print") && k + 1 < argVals.length) {
            codes.push(argVals[++k]);
          } else if (a.startsWith("--eval=")) {
            codes.push(a.slice(7));
          } else if (a === "-") {
            if (item.heredoc) codes.push(item.heredoc);
          }
        }
        if ((seg.hasHeredoc || seg.pipedFromPrev) && item.heredoc) codes.push(item.heredoc);
        if (seg.pipedFromPrev && sIdx > 0) {
          codes.push(segs[sIdx - 1].tokens.map((t) => t.value).join(" "));
        }
        for (const c of codes) {
          const res = inspectNodeCode(c);
          if (res.matched) {
            result.matched = true;
            result.summaries.push("node inline write");
            result.targets.push(...res.targets);
            if (res.untargeted) result.untargeted = true;
          }
        }
      } else if (cmdWord === "python" || cmdWord === "python3") {
        const codes = [];
        for (let k = 0; k < argVals.length; k++) {
          const a = argVals[k];
          if (a === "-c" && k + 1 < argVals.length) codes.push(argVals[++k]);
          else if (a.startsWith("-c") && a.length > 2) codes.push(a.slice(2));
          else if (a === "-") {
            if (item.heredoc) codes.push(item.heredoc);
          }
        }
        if ((seg.hasHeredoc || seg.pipedFromPrev) && item.heredoc) codes.push(item.heredoc);
        if (seg.pipedFromPrev && sIdx > 0) {
          codes.push(segs[sIdx - 1].tokens.map((t) => t.value).join(" "));
        }
        for (const c of codes) {
          const res = inspectPythonCode(c);
          if (res.matched) {
            result.matched = true;
            result.summaries.push("python inline write");
            result.targets.push(...res.targets);
            if (res.untargeted) result.untargeted = true;
          }
        }
      } else if ((cmdWord === "sh" || cmdWord === "bash" || cmdWord === "zsh") && depth < 1) {
        for (let k = 0; k < argVals.length; k++) {
          if (argVals[k] === "-c" && k + 1 < argVals.length) {
            const subRes = detectShellWriteBypass(argVals[k + 1], depth + 1);
            if (subRes.matched) {
              result.matched = true;
              result.targets.push(...subRes.targets);
              if (subRes.untargeted) result.untargeted = true;
              result.summaries.push(...subRes.summaries);
            }
          }
        }
      }
    }
  }
  return result;
};

const cmd = String(args.CommandLine || args.command || "");
if (!cmd.trim()) {
  emit("allow");
}

const bypass = detectShellWriteBypass(cmd, 0);
if (!bypass.matched) {
  emit("allow");
}

const detail = bypass.summaries[0] || "shell write";
emit(
  "deny",
  "BLOCKED by AGY Harness Source-Edit Guard: Active contract found in .artifacts/SPEC.md or .artifacts/PLAN.md. Top-level coordinator MUST NOT bypass via `run_command` (" +
    detail +
    "). Delegate ALL source/test edits to `slice-builder`."
);
'
fi

if command -v python3 >/dev/null 2>&1; then
  exec python3 - <<'PY'
import json, os, pathlib, re, sys

def emit(decision, reason=None):
    out = {"decision": decision}
    if reason:
        out["reason"] = reason
    sys.stdout.write(json.dumps(out, ensure_ascii=False) + "\n")
    sys.exit(0)

try:
    data = json.loads(os.environ.get("AGY_GUARD_STDIN") or "{}")
except Exception:
    emit("allow")

plugin_mode = os.environ.get("AGY_GUARD_PLUGIN_MODE") == "true"

def resolve_real(target_path, base_dir):
    cur = pathlib.Path(os.path.abspath(os.path.join(str(base_dir), str(target_path))))
    parts = []
    while not cur.exists():
        parent = cur.parent
        if parent == cur:
            break
        parts.insert(0, cur.name)
        cur = parent
    real_base = pathlib.Path(os.path.realpath(str(cur)))
    return real_base.joinpath(*parts) if parts else real_base

ws_list = data.get("workspacePaths") or []
raw_repo = (
    str(ws_list[0])
    if (ws_list and ws_list[0] and os.path.exists(str(ws_list[0])))
    else (os.getcwd() if plugin_mode else (os.environ.get("AGY_GUARD_REPO_ROOT") or os.getcwd()))
)
repo_root = resolve_real(raw_repo, os.getcwd())

has_harness_in_ws = (
    (repo_root / ".agents" / "rules" / "agy-harness.md").exists()
    or (repo_root / ".artifacts" / "SPEC.md").exists()
    or (repo_root / ".artifacts" / "PLAN.md").exists()
)
if plugin_mode and not has_harness_in_ws:
    emit("allow")

tc = data.get("toolCall") or {}
tool_name = str(tc.get("name") or "")
args = tc.get("args") or {}

EDIT_TOOLS = {
    "edit_file",
    "write_file",
    "create_file",
    "delete_file",
    "sed_file",
    "write_to_file",
    "replace_file_content",
    "multi_replace_file_content",
}
is_edit_tool = tool_name in EDIT_TOOLS
is_run_command = tool_name == "run_command"

if not is_edit_tool and not is_run_command:
    emit("allow")

def is_harness_internal(rel_path):
    return rel_path in (".artifacts", ".agents") or rel_path.startswith(".artifacts/") or rel_path.startswith(".agents/")

def extract_target_path(a):
    if not isinstance(a, dict):
        return ""
    for key in ("Path", "TargetFile", "TargetPath", "File", "path", "file", "file_path", "filePath", "target_file"):
        v = a.get(key)
        if v:
            return str(v).strip('"').strip()
    return ""

raw_target = ""
abs_target = None
rel = ""
if is_edit_tool:
    raw_target = extract_target_path(args)
    if raw_target:
        abs_target = resolve_real(raw_target, repo_root)
        if abs_target == repo_root:
            emit("allow")
        try:
            rel = abs_target.relative_to(repo_root).as_posix()
        except ValueError:
            emit("allow")
        if not rel or rel == "." or is_harness_internal(rel):
            emit("allow")

def detect_is_parent(tp):
    if not tp or not os.path.exists(tp):
        return False
    try:
        with open(tp, "r", encoding="utf-8", errors="ignore") as fh:
            lines = fh.read().splitlines()
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

if not detect_is_parent(str(data.get("transcriptPath") or "")):
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

def check_contract_active():
    spec_path = repo_root / ".artifacts" / "SPEC.md"
    plan_path = repo_root / ".artifacts" / "PLAN.md"
    spec_exists = spec_path.exists()
    plan_exists = plan_path.exists()
    spec_txt = spec_path.read_text(encoding="utf-8") if spec_exists else ""
    plan_txt = plan_path.read_text(encoding="utf-8") if plan_exists else ""

    if (spec_exists and is_explicit_archive_marker(spec_txt)) or (plan_exists and is_explicit_archive_marker(plan_txt)):
        return False
    spec_filled = spec_exists and not has_spec_template_marker(spec_txt)
    plan_filled = plan_exists and not has_plan_template_marker(plan_txt)
    if not spec_filled and not plan_filled:
        return False
    if spec_filled and plan_filled:
        sd, so = count_spec_checkboxes(spec_txt)
        pd, po = count_plan_statuses(plan_txt)
        if sd >= 1 and so == 0 and pd >= 1 and po == 0:
            return False
    return True

is_contract_active = check_contract_active()

if not is_contract_active:
    emit("allow")

if is_edit_tool:
    if not raw_target:
        emit(
            "force_ask",
            f"AGY Harness Source-Edit Guard: Edit tool `{tool_name}` has no recognizable target path while a contract is active. Confirm allowing this edit?",
        )

    emit(
        "deny",
        f"BLOCKED by AGY Harness Source-Edit Guard: Active contract found in .artifacts/SPEC.md or .artifacts/PLAN.md. Top-level coordinator MUST NOT directly edit `{rel}`. Wait for user Proceed and delegate ALL implementation/rework edits to `slice-builder` via `invoke_subagent` or `send_message`.",
    )

def classify_path_target(raw_p):
    cleaned = str(raw_p or "").strip().strip("'\"")
    if not cleaned:
        return (False, "")
    if cleaned == "/dev/null" or re.match(r"^/(?:private/)?tmp(?:/|$)", cleaned):
        return (True, "")
    if cleaned.startswith("$") or cleaned.startswith("~"):
        return (False, "")
    abs_p = resolve_real(cleaned, repo_root)
    if abs_p == repo_root:
        return (False, "")
    try:
        r = abs_p.relative_to(repo_root).as_posix()
    except ValueError:
        return (True, "")
    if not r or r == ".":
        return (False, "")
    if is_harness_internal(r):
        return (True, "")
    return (False, r)

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
    in_s = False
    in_d = False
    while i < n:
        ch = s[i]
        if in_s:
            if ch == "'":
                in_s = False
            else:
                out.append(ch)
            i += 1
        elif in_d:
            if ch == "\\" and i + 1 < n:
                out.append(s[i + 1])
                i += 2
            elif ch == '"':
                in_d = False
                i += 1
            else:
                out.append(ch)
                i += 1
        else:
            if ch == "\\" and i + 1 < n:
                out.append(s[i + 1])
                i += 2
            elif ch == "'":
                in_s = True
                i += 1
            elif ch == '"':
                in_d = True
                i += 1
            elif ch in " \t;&|><":
                break
            else:
                out.append(ch)
                i += 1
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
    in_s = False
    in_d = False

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
            if ch == "'":
                in_s = False
            else:
                cur.append(ch)
            i += 1
            continue
        if in_d:
            if ch == "\\" and i + 1 < n:
                cur.append(line[i + 1])
                i += 2
            elif ch == '"':
                in_d = False
                i += 1
            else:
                cur.append(ch)
                i += 1
            continue
        if ch == "\\" and i + 1 < n:
            has_cur = True
            cur.append(line[i + 1])
            i += 2
            continue
        if ch == "'":
            in_s = True
            cur_quoted = True
            has_cur = True
            i += 1
            continue
        if ch == '"':
            in_d = True
            cur_quoted = True
            has_cur = True
            i += 1
            continue
        if ch in (" ", "\t"):
            flush_token()
            i += 1
            continue
        if ch == ";":
            flush_segment(False)
            i += 1
            continue
        if ch == "&" and i + 1 < n and line[i + 1] == "&":
            flush_segment(False)
            i += 2
            continue
        if ch == "|" and i + 1 < n and line[i + 1] == "|":
            flush_segment(False)
            i += 2
            continue
        if ch == "|":
            flush_segment(True)
            i += 1
            continue
        if ch == "<":
            if i + 1 < n and line[i + 1] == "<":
                flush_token()
                has_heredoc = True
                i += 2
                if i < n and line[i] in ("-", "<"):
                    i += 1
                _, i = read_word(line, i)
            else:
                if has_cur and "".join(cur).isdigit():
                    cur = []
                    has_cur = False
                else:
                    flush_token()
                i += 1
                if i < n and line[i] == "&":
                    i += 1
                    while i < n and (line[i].isdigit() or line[i] == "-"):
                        i += 1
                else:
                    _, i = read_word(line, i)
            continue
        if ch == ">":
            cur_s = "".join(cur)
            if has_cur and cur_s in ("=", "-") and not cur_quoted:
                cur.append(ch)
                i += 1
                continue
            if has_cur and (cur_s.isdigit() or cur_s == "&") and not cur_quoted:
                cur = []
                has_cur = False
            else:
                flush_token()
            i += 1
            if i < n and line[i] == ">":
                i += 1
            if i < n and line[i] == "&":
                i += 1
                while i < n and (line[i].isdigit() or line[i] == "-"):
                    i += 1
            else:
                w, i = read_word(line, i)
                redirects.append(w)
            continue
        if ch == "&" and i + 1 < n and line[i + 1] == ">":
            flush_token()
            i += 2
            if i < n and line[i] == ">":
                i += 1
            w, i = read_word(line, i)
            redirects.append(w)
            continue
        has_cur = True
        cur.append(ch)
        i += 1
    flush_segment(False)
    return segments

def inspect_node_code(code):
    write_re = re.compile(r"\b(?:writeFileSync|writeFile|appendFileSync|appendFile|createWriteStream|rmSync|rm|unlinkSync|unlink|renameSync|rename|copyFileSync|copyFile|cpSync|cp|truncateSync|truncate|openSync)\s*\(")
    targets = []
    untargeted = False
    matched = False
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
        if not cands:
            matched = True
            untargeted = True
        else:
            for p in cands:
                ex, r = classify_path_target(p)
                if not ex:
                    matched = True
                    if r: targets.append(r)
                    else: untargeted = True
    return matched, targets, untargeted

def inspect_python_code(code):
    targets = []
    untargeted = False
    matched = False
    def check_lit(p):
        nonlocal matched, untargeted
        ex, r = classify_path_target(p)
        if not ex:
            matched = True
            if r: targets.append(r)
            else: untargeted = True
    covered = []
    for pm in re.finditer(r"Path\s*\(\s*([^)]*)\)\s*\.\s*(?:write_text|write_bytes|unlink|rename|replace|mkdir|rmdir)\s*\(([^)]*)\)", code):
        covered.append((pm.start(), pm.end()))
        plit = re.search(r"['\"]([^'\"\r\n]+)['\"]", pm.group(1))
        if not plit:
            matched = True; untargeted = True
        else:
            check_lit(plit.group(1))
        if re.search(r"\.(?:rename|replace)\s*\(", pm.group(0)):
            dlit = re.search(r"['\"]([^'\"\r\n]+)['\"]", pm.group(2))
            if not dlit: matched = True; untargeted = True
            else: check_lit(dlit.group(1))
    for bw in re.finditer(r"\.\s*(?:write_text|write_bytes|writelines)\s*\(", code):
        if not any(s <= bw.start() <= e for s, e in covered):
            matched = True; untargeted = True
    for om in re.finditer(r"\bopen\s*\(([^)]*)\)", code):
        inside = om.group(1)
        mm = re.search(r",\s*(?:mode\s*=\s*)?['\"]([rwaAxbtU+]+)['\"]", inside)
        if mm and not re.search(r"[wax+]", mm.group(1), re.IGNORECASE):
            continue
        if not mm and "," not in inside:
            continue
        plit = re.match(r"^\s*['\"]([^'\"\r\n]+)['\"]", inside)
        if not plit: matched = True; untargeted = True
        else: check_lit(plit.group(1))
    for sm in re.finditer(r"\b(?:shutil\.(?:copy\w*|move|rmtree)|os\.(?:remove|unlink|rename|replace|rmdir|makedirs?))\s*\(([^)]*)\)", code):
        lits = re.findall(r"['\"]([^'\"\r\n]+)['\"]", sm.group(1))
        if not lits: matched = True; untargeted = True
        else:
            for l in lits: check_lit(l)
    return matched, targets, untargeted

def detect_shell_write_bypass(cmd_str, depth=0):
    matched = False
    targets = []
    untargeted = False
    summaries = []

    def record(cls, label):
        nonlocal matched, untargeted
        ex, r = cls
        if not ex:
            matched = True
            summaries.append(label)
            if r: targets.append(r)
            else: untargeted = True

    def record_untargeted(label):
        nonlocal matched, untargeted
        matched = True; untargeted = True; summaries.append(label)

    WRAPPERS = {"command", "sudo", "env", "time", "nohup", "exec"}
    for line, heredoc in split_heredocs(cmd_str):
        segs = tokenize_shell_line(line)
        for s_idx, (tokens, redirects, has_heredoc, piped_from_prev) in enumerate(segs):
            for rt in redirects:
                record(classify_path_target(rt), "redirection")
            idx = 0
            while idx < len(tokens):
                v = tokens[idx][0]
                if re.match(r"^[A-Za-z_][A-Za-z0-9_]*=", v) or v in WRAPPERS:
                    idx += 1
                else:
                    break
            if idx >= len(tokens):
                continue
            cmd_word = os.path.basename(tokens[idx][0])
            arg_tokens = tokens[idx + 1:]
            arg_vals = [t[0] for t in arg_tokens]

            if cmd_word == "tee":
                files = [a for a in arg_vals if not a.startswith("-")]
                if not files: record_untargeted("tee")
                else:
                    for f in files: record(classify_path_target(f), "tee")
            elif cmd_word in ("rm", "unlink", "touch"):
                after_dd = False
                files = []
                k = 0
                while k < len(arg_vals):
                    a = arg_vals[k]
                    if not after_dd and a == "--":
                        after_dd = True; k += 1; continue
                    if not after_dd and a.startswith("-"):
                        if cmd_word == "touch" and a in ("-d", "-t", "-r"):
                            k += 2; continue
                        k += 1; continue
                    files.append(a); k += 1
                for f in files: record(classify_path_target(f), cmd_word)
            elif cmd_word == "mv":
                files = []
                k = 0
                while k < len(arg_vals):
                    a = arg_vals[k]
                    if a in ("-t", "-S"):
                        if a == "-t" and k + 1 < len(arg_vals): files.append(arg_vals[k + 1])
                        k += 2; continue
                    if a.startswith("--target-directory="):
                        files.append(a[19:]); k += 1; continue
                    if a.startswith("-"):
                        k += 1; continue
                    files.append(a); k += 1
                if not files: record_untargeted("mv")
                else:
                    for f in files: record(classify_path_target(f), "mv")
            elif cmd_word in ("cp", "install"):
                target_dir = ""
                files = []
                k = 0
                while k < len(arg_vals):
                    a = arg_vals[k]
                    if a == "-t" and k + 1 < len(arg_vals):
                        target_dir = arg_vals[k + 1]; k += 2; continue
                    if a.startswith("--target-directory="):
                        target_dir = a[19:]; k += 1; continue
                    if cmd_word == "install" and a in ("-m", "-o", "-g"):
                        k += 2; continue
                    if a.startswith("-"):
                        k += 1; continue
                    files.append(a); k += 1
                if target_dir: record(classify_path_target(target_dir), cmd_word)
                elif files: record(classify_path_target(files[-1]), cmd_word)
                elif cmd_word == "install" and any(not a.startswith("-") for a in arg_vals): record_untargeted("install")
            elif cmd_word == "sed":
                if any(re.match(r"^-[A-Za-z]*i", a) or a == "--in-place" or a.startswith("--in-place=") for a in arg_vals):
                    saw_sf = False
                    pos = []
                    k = 0
                    while k < len(arg_vals):
                        a = arg_vals[k]
                        if a == "-i":
                            if k + 1 < len(arg_vals) and arg_vals[k + 1] == "":
                                k += 2; continue
                            k += 1; continue
                        if re.match(r"^-[A-Za-z]*i", a) or a == "--in-place" or a.startswith("--in-place="):
                            k += 1; continue
                        if a in ("-e", "-f"):
                            saw_sf = True; k += 2; continue
                        if a.startswith("--expression=") or a.startswith("--file="):
                            saw_sf = True; k += 1; continue
                        if a.startswith("-"):
                            k += 1; continue
                        pos.append(a); k += 1
                    files = pos if saw_sf else pos[1:]
                    if not files: record_untargeted("sed -i")
                    else:
                        for f in files: record(classify_path_target(f), "sed -i")
            elif cmd_word == "perl":
                if any(re.match(r"^-[A-Za-z]*i", a) or a == "--in-place" or a.startswith("--in-place=") for a in arg_vals):
                    files = []
                    k = 0
                    while k < len(arg_vals):
                        a = arg_vals[k]
                        if a in ("-e", "-E", "-I", "-M"):
                            k += 2; continue
                        if a.startswith("-"):
                            k += 1; continue
                        files.append(a); k += 1
                    if not files: record_untargeted("perl -i")
                    else:
                        for f in files: record(classify_path_target(f), "perl -i")
            elif cmd_word == "patch":
                if not any(a in ("--dry-run", "-C") for a in arg_vals):
                    dir_t = ""
                    out_t = ""
                    k = 0
                    while k < len(arg_vals):
                        a = arg_vals[k]
                        if a in ("-d", "--directory") and k + 1 < len(arg_vals):
                            dir_t = arg_vals[k + 1]; k += 2; continue
                        if a.startswith("--directory="):
                            dir_t = a[12:]; k += 1; continue
                        if a in ("-o", "--output") and k + 1 < len(arg_vals):
                            out_t = arg_vals[k + 1]; k += 2; continue
                        k += 1
                    if out_t: record(classify_path_target(out_t), "patch")
                    elif dir_t: record(classify_path_target(dir_t), "patch")
                    else: record_untargeted("patch")
            elif cmd_word == "dd":
                for a in arg_vals:
                    if a.startswith("of="): record(classify_path_target(a[3:]), "dd of=")
            elif cmd_word == "truncate":
                files = []
                k = 0
                while k < len(arg_vals):
                    a = arg_vals[k]
                    if a in ("-s", "-r", "--size", "--reference"):
                        k += 2; continue
                    if a.startswith("-"):
                        k += 1; continue
                    files.append(a); k += 1
                if not files: record_untargeted("truncate")
                else:
                    for f in files: record(classify_path_target(f), "truncate")
            elif cmd_word == "git":
                git_dir = ""
                k = 0
                while k < len(arg_vals):
                    a = arg_vals[k]
                    if a == "-C" and k + 1 < len(arg_vals):
                        git_dir = arg_vals[k + 1]; k += 2; continue
                    if a in ("-c", "--git-dir", "--work-tree"):
                        k += 2; continue
                    if a.startswith("-"):
                        k += 1; continue
                    break
                if git_dir and classify_path_target(git_dir)[0]:
                    continue
                sub = arg_vals[k] if k < len(arg_vals) else ""
                sub_args = arg_vals[k + 1:]
                if sub == "apply":
                    if not any(a in ("--check", "--stat", "--numstat", "--summary") for a in sub_args):
                        record_untargeted("git apply")
                elif sub == "restore":
                    files = [a for a in sub_args if a != "--" and not a.startswith("-")]
                    if not files: record_untargeted("git restore")
                    else:
                        for f in files: record(classify_path_target(f), "git restore")
                elif sub == "checkout":
                    if "--" in sub_args:
                        dd = sub_args.index("--")
                        files = sub_args[dd + 1:]
                        if not files: record_untargeted("git checkout --")
                        else:
                            for f in files: record(classify_path_target(f), "git checkout --")
                elif sub == "reset":
                    if any(a in ("--hard", "--merge") for a in sub_args):
                        record_untargeted("git reset --hard")
                elif sub == "clean":
                    if not any(a == "--dry-run" or re.match(r"^-[A-Za-z]*n", a) for a in sub_args):
                        record_untargeted("git clean")
                elif sub in ("merge", "cherry-pick", "am", "revert", "rebase", "mv", "rm"):
                    record_untargeted(f"git {sub}")
            elif cmd_word == "node":
                codes = []
                k = 0
                while k < len(arg_vals):
                    a = arg_vals[k]
                    if a in ("-e", "--eval", "-p", "--print") and k + 1 < len(arg_vals):
                        codes.append(arg_vals[k + 1]); k += 2; continue
                    if a.startswith("--eval="):
                        codes.append(a[7:])
                    elif a == "-" and heredoc:
                        codes.append(heredoc)
                    k += 1
                if (has_heredoc or piped_from_prev) and heredoc:
                    codes.append(heredoc)
                if piped_from_prev and s_idx > 0:
                    codes.append(" ".join(t[0] for t in segs[s_idx - 1][0]))
                for c in codes:
                    m, ts, u = inspect_node_code(c)
                    if m:
                        matched = True; summaries.append("node inline write"); targets.extend(ts)
                        if u: untargeted = True
            elif cmd_word in ("python", "python3"):
                codes = []
                k = 0
                while k < len(arg_vals):
                    a = arg_vals[k]
                    if a == "-c" and k + 1 < len(arg_vals):
                        codes.append(arg_vals[k + 1]); k += 2; continue
                    if a.startswith("-c") and len(a) > 2:
                        codes.append(a[2:])
                    elif a == "-" and heredoc:
                        codes.append(heredoc)
                    k += 1
                if (has_heredoc or piped_from_prev) and heredoc:
                    codes.append(heredoc)
                if piped_from_prev and s_idx > 0:
                    codes.append(" ".join(t[0] for t in segs[s_idx - 1][0]))
                for c in codes:
                    m, ts, u = inspect_python_code(c)
                    if m:
                        matched = True; summaries.append("python inline write"); targets.extend(ts)
                        if u: untargeted = True
            elif cmd_word in ("sh", "bash", "zsh") and depth < 1:
                for k, a in enumerate(arg_vals):
                    if a == "-c" and k + 1 < len(arg_vals):
                        m, ts, u, sm = detect_shell_write_bypass(arg_vals[k + 1], depth + 1)
                        if m:
                            matched = True; targets.extend(ts); summaries.extend(sm)
                            if u: untargeted = True
    return matched, targets, untargeted, summaries

cmd_line = str(args.get("CommandLine") or args.get("command") or "")
if not cmd_line.strip():
    emit("allow")

bypass_matched, bypass_targets, bypass_untargeted, bypass_summaries = detect_shell_write_bypass(cmd_line, 0)
if not bypass_matched:
    emit("allow")

detail = bypass_summaries[0] if bypass_summaries else "shell write"
emit(
    "deny",
    f"BLOCKED by AGY Harness Source-Edit Guard: Active contract found in .artifacts/SPEC.md or .artifacts/PLAN.md. Top-level coordinator MUST NOT bypass via `run_command` ({detail}). Delegate ALL source/test edits to `slice-builder`.",
)
PY
fi

echo '{"decision":"allow"}'
