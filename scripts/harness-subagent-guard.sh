#!/bin/bash
set -euo pipefail

# AGY Harness Subagent PreToolUse Guard.
# Enforces physical role isolation for slice-builder and staff-gatekeeper:
#   - slice-builder: unconditionally blocks edits to .artifacts/{SPEC,CONSTRAINTS,PLAN}.md;
#     blocks edits to .agents/* and scripts/harness-*.sh unless workspace plugin.json has
#     name === "agy-harness" ([GUARDRAIL-1]); blocks IsDaemon=true, WaitMsBeforeAsync > 10000,
#     and container/install/watch/streaming/interactive commands with [HOST_CMD_REQUEST] guidance.
#   - staff-gatekeeper: restricts run_command to short read-only checks (blocks daemon/slow,
#     containers, installs, shell writes/redirections, sed -i, rm/mv/cp, and mutating git).

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
REPO_ROOT="$(cd "$SCRIPT_DIR/.." && pwd)"

ROLE=""
while [ $# -gt 0 ]; do
  case "$1" in
    --role=*)
      ROLE="${1#--role=}"
      shift
      ;;
    --role)
      ROLE="${2:-}"
      shift 2 || break
      ;;
    *)
      shift
      ;;
  esac
done

if [ -t 0 ]; then
  HOOK_STDIN="{}"
else
  HOOK_STDIN="$(cat || true)"
  [ -z "$HOOK_STDIN" ] && HOOK_STDIN="{}"
fi

export AGY_SUBAGENT_REPO_ROOT="$REPO_ROOT"
export AGY_SUBAGENT_ROLE="$ROLE"
export AGY_SUBAGENT_STDIN="$HOOK_STDIN"

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
  input = JSON.parse(process.env.AGY_SUBAGENT_STDIN || "{}");
} catch {
  emit("allow");
}

const role = String(process.env.AGY_SUBAGENT_ROLE || "").trim();
if (role !== "slice-builder" && role !== "staff-gatekeeper") {
  emit("allow");
}

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
const rawWs =
  (wsList[0] && String(wsList[0])) ||
  (input.cwd && String(input.cwd)) ||
  process.env.AGY_SUBAGENT_REPO_ROOT ||
  process.cwd();
const workspaceRoot = resolveReal(rawWs, process.cwd());

let isSelfHosting = false;
try {
  const pjPath = path.join(workspaceRoot, "plugin.json");
  if (fs.existsSync(pjPath)) {
    const pj = JSON.parse(fs.readFileSync(pjPath, "utf8"));
    if (pj && pj.name === "agy-harness") {
      isSelfHosting = true;
    }
  }
} catch {}

const tc = input.toolCall || {};
const toolName = String(tc.name || "");
const args = tc.args || {};

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

const LOCKED_CONTRACTS = new Set([
  ".artifacts/SPEC.md",
  ".artifacts/CONSTRAINTS.md",
  ".artifacts/PLAN.md",
]);

const isForbiddenForBuilder = (relPath) => {
  if (!relPath) return "";
  if (LOCKED_CONTRACTS.has(relPath)) return "contract";
  if (
    !isSelfHosting &&
    (relPath === ".agents" ||
      relPath.startsWith(".agents/") ||
      /^scripts\/harness-[^/]*\.sh$/.test(relPath))
  ) {
    return "harness";
  }
  return "";
};

const resolveWorkspaceRel = (rawPath) => {
  const cleaned = String(rawPath || "").trim().replace(/^[\x27"]|[\x27"]$/g, "");
  if (!cleaned) return { inside: false, rel: "", raw: "" };
  if (cleaned === "/dev/null") return { inside: false, rel: "", devNull: true };
  if (cleaned.startsWith("$") || cleaned.startsWith("~")) return { inside: false, rel: "", dynamic: true };
  const abs = resolveReal(cleaned, workspaceRoot);
  if (!abs.startsWith(workspaceRoot + path.sep) && abs !== workspaceRoot) {
    return { inside: false, rel: "" };
  }
  const rel = path.relative(workspaceRoot, abs).replace(/\\/g, "/");
  return { inside: true, rel };
};

if (EDIT_TOOLS.has(toolName)) {
  if (role === "staff-gatekeeper") {
    emit(
      "deny",
      "BLOCKED by AGY Subagent Guard (staff-gatekeeper): staff-gatekeeper is strictly read-only and cannot execute `" +
        toolName +
        "`."
    );
  }
  const rawTarget = extractTargetPath(args);
  if (!rawTarget) {
    emit("allow");
  }
  const info = resolveWorkspaceRel(rawTarget);
  if (!info.inside) {
    emit("allow");
  }
  const kind = isForbiddenForBuilder(info.rel);
  if (kind === "contract") {
    emit(
      "deny",
      "BLOCKED by AGY Subagent Guard (slice-builder): Locked contract `" +
        info.rel +
        "` is read-only for slice-builder. Report contract gaps to the parent coordinator instead."
    );
  }
  if (kind === "harness") {
    emit(
      "deny",
      "BLOCKED by AGY Subagent Guard (slice-builder): Harness control file `" +
        info.rel +
        "` cannot be modified by slice-builder in target workspaces."
    );
  }
  emit("allow");
}

if (toolName !== "run_command") {
  emit("allow");
}

if (args.IsDaemon === true) {
  emit(
    "deny",
    "BLOCKED by AGY Subagent Guard (" +
      role +
      "): IsDaemon=true is forbidden. Stop and emit ## [HOST_CMD_REQUEST] to delegate background/daemon commands."
  );
}

const waitMsRaw = args.WaitMsBeforeAsync;
if (waitMsRaw !== undefined && waitMsRaw !== null && waitMsRaw !== "" && Number(waitMsRaw) > 10000) {
  emit(
    "deny",
    "BLOCKED by AGY Subagent Guard (" +
      role +
      "): WaitMsBeforeAsync=" +
      waitMsRaw +
      " exceeds the 10000ms limit. Stop and emit ## [HOST_CMD_REQUEST] to delegate heavy/slow commands."
  );
}

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

const CONTAINER_BINS = new Set([
  "docker",
  "docker-compose",
  "podman",
  "colima",
  "orbstack",
  "kubectl",
  "helm",
  "kind",
  "minikube",
  "testcontainers",
]);

const SEARCH_BINS = new Set(["grep", "egrep", "fgrep", "rg", "ag", "ack"]);

const firstNonFlagArgs = (argVals, valueFlags) => {
  const out = [];
  for (let i = 0; i < argVals.length; i++) {
    const a = argVals[i];
    if (a === "--") break;
    if (valueFlags.has(a) && i + 1 < argVals.length) {
      i++;
      continue;
    }
    if (a.startsWith("-")) continue;
    out.push(a);
  }
  return out;
};

const detectBlacklistedCommand = (cmd, depth = 0) => {
  for (const item of splitHeredocs(cmd)) {
    const segs = tokenizeShellLine(item.line);
    for (const seg of segs) {
      let idx = 0;
      const WRAPPERS = new Set(["command", "sudo", "env", "time", "nohup", "exec"]);
      const envTokens = [];
      while (idx < seg.tokens.length) {
        const val = seg.tokens[idx].value;
        if (/^[A-Za-z_][A-Za-z0-9_]*=/.test(val)) {
          envTokens.push(val);
          idx++;
          continue;
        }
        if (WRAPPERS.has(val)) {
          idx++;
          continue;
        }
        break;
      }
      if (idx >= seg.tokens.length) continue;

      const cmdWord = path.posix.basename(seg.tokens[idx].value);
      const argVals = seg.tokens.slice(idx + 1).map((t) => t.value);
      const isSearch = SEARCH_BINS.has(cmdWord);

      if (CONTAINER_BINS.has(cmdWord)) {
        return cmdWord;
      }

      if (!isSearch) {
        for (const t of [...envTokens, ...argVals]) {
          if (/\btestcontainers\b/i.test(t)) return "testcontainers";
        }
        if (argVals.some((a) => a === "--watch" || (/^--watch=/.test(a) && !/^--watch=(?:false|0|off|none)$/i.test(a)))) {
          return "--watch";
        }
        if (argVals.some((a) => a === "-it" || a === "-ti")) {
          return "-it";
        }
      }

      if (cmdWord === "npm" || cmdWord === "pnpm" || cmdWord === "yarn" || cmdWord === "bun") {
        const pos = firstNonFlagArgs(argVals, new Set(["--prefix", "-C", "--cwd", "--filter"]));
        if (pos[0] && /^(?:install|i|add|ci)$/.test(pos[0])) {
          return cmdWord + " " + pos[0];
        }
      } else if (cmdWord === "pip" || cmdWord === "pip3" || cmdWord === "poetry") {
        const pos = firstNonFlagArgs(argVals, new Set(["--python", "-C", "--directory"]));
        if (pos[0] === "install") {
          return cmdWord + " install";
        }
      } else if (cmdWord === "uv") {
        const pos = firstNonFlagArgs(argVals, new Set(["--python", "--directory"]));
        if (pos[0] === "install" || ((pos[0] === "pip" || pos[0] === "pip3") && pos[1] === "install")) {
          return "uv install";
        }
      } else if (cmdWord === "python" || cmdWord === "python3") {
        for (let k = 0; k < argVals.length - 2; k++) {
          if (argVals[k] === "-m" && (argVals[k + 1] === "pip" || argVals[k + 1] === "pip3") && argVals[k + 2] === "install") {
            return cmdWord + " -m pip install";
          }
        }
      } else if (cmdWord === "go") {
        const pos = firstNonFlagArgs(argVals, new Set(["-C"]));
        if (pos[0] === "mod" && (pos[1] === "download" || pos[1] === "tidy")) {
          return "go mod " + pos[1];
        }
      } else if (cmdWord === "flutter") {
        const pos = firstNonFlagArgs(argVals, new Set([]));
        if (pos[0] === "pub" && pos[1] === "get") {
          return "flutter pub get";
        }
      } else if (cmdWord === "pod") {
        const pos = firstNonFlagArgs(argVals, new Set([]));
        if (pos[0] === "install") {
          return "pod install";
        }
      } else if (cmdWord === "tail") {
        if (argVals.some((a) => /^-[A-Za-z]*[fF]/.test(a) || /^--follow(?:=|$)/.test(a))) {
          return "tail -f";
        }
      } else if (cmdWord === "logs" || argVals[0] === "logs") {
        if (argVals.some((a) => /^-[A-Za-z]*f/.test(a) || /^--follow(?:=|$)/.test(a))) {
          return "logs -f";
        }
      } else if ((cmdWord === "sh" || cmdWord === "bash" || cmdWord === "zsh") && depth < 1) {
        for (let k = 0; k < argVals.length - 1; k++) {
          if (argVals[k] === "-c") {
            const inner = detectBlacklistedCommand(argVals[k + 1], depth + 1);
            if (inner) return inner;
          }
        }
      }
    }
  }
  return "";
};

const inspectNodeTargets = (code) => {
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
    if (fn === "openSync" && /[\x27"]r/.test(argSlice) && !/[\x27"][wa+]/.test(argSlice)) continue;
    matched = true;
    const literals = [];
    const litRe = /[\x27"`]([^\x27"`\r\n]+)[\x27"`]/g;
    let lm;
    while ((lm = litRe.exec(argSlice)) !== null) literals.push(lm[1]);
    const cands =
      fn === "renameSync" || fn === "rename" || fn === "copyFileSync" || fn === "copyFile" || fn === "cpSync" || fn === "cp"
        ? literals.slice(0, 2)
        : literals.slice(0, 1);
    if (cands.length === 0) untargeted = true;
    else targets.push(...cands);
  }
  return { matched, targets, untargeted };
};

const inspectPythonTargets = (code) => {
  const targets = [];
  let untargeted = false;
  let matched = false;
  const covered = [];
  const pathMethodRe =
    /Path\s*\(\s*([^\)]*)\)\s*\.\s*(?:write_text|write_bytes|unlink|rename|replace|mkdir|rmdir)\s*\(([^\)]*)\)/g;
  let pm;
  while ((pm = pathMethodRe.exec(code)) !== null) {
    covered.push([pm.index, pm.index + pm[0].length]);
    matched = true;
    const pLit = pm[1].match(/[\x27"]([^\x27"\r\n]+)[\x27"]/);
    if (pLit) targets.push(pLit[1]);
    else untargeted = true;
    if (/\.(?:rename|replace)\s*\(/.test(pm[0])) {
      const dLit = pm[2].match(/[\x27"]([^\x27"\r\n]+)[\x27"]/);
      if (dLit) targets.push(dLit[1]);
      else untargeted = true;
    }
  }
  const bareWriteRe = /\.\s*(?:write_text|write_bytes|writelines)\s*\(/g;
  let bw;
  while ((bw = bareWriteRe.exec(code)) !== null) {
    if (!covered.some(([s, e]) => bw.index >= s && bw.index <= e)) {
      matched = true;
      untargeted = true;
    }
  }
  const openRe = /\bopen\s*\(([^\)]*)\)/g;
  let om;
  while ((om = openRe.exec(code)) !== null) {
    const inside = om[1];
    const modeMatch = inside.match(/,\s*(?:mode\s*=\s*)?[\x27"]([rwaAxbtU+]+)[\x27"]/);
    if (modeMatch && !/[wax+]/i.test(modeMatch[1])) continue;
    if (!modeMatch && !/,/.test(inside)) continue;
    matched = true;
    const pLit = inside.match(/^\s*[\x27"]([^\x27"\r\n]+)[\x27"]/);
    if (pLit) targets.push(pLit[1]);
    else untargeted = true;
  }
  const osShutilRe = /\b(?:shutil\.(?:copy\w*|move|rmtree)|os\.(?:remove|unlink|rename|replace|rmdir|makedirs?))\s*\(([^\)]*)\)/g;
  let sm;
  while ((sm = osShutilRe.exec(code)) !== null) {
    matched = true;
    const lits = [...sm[1].matchAll(/[\x27"]([^\x27"\r\n]+)[\x27"]/g)].map((x) => x[1]);
    if (lits.length === 0) untargeted = true;
    else targets.push(...lits);
  }
  return { matched, targets, untargeted };
};

const inspectShellMutations = (cmd, depth = 0) => {
  const writes = [];
  const mutatingOps = [];
  const recordTarget = (rawP, op) => {
    const cleaned = String(rawP || "").trim().replace(/^[\x27"]|[\x27"]$/g, "");
    if (cleaned === "/dev/null") return;
    writes.push({ raw: cleaned, op });
  };

  for (const item of splitHeredocs(cmd)) {
    const segs = tokenizeShellLine(item.line);
    for (let sIdx = 0; sIdx < segs.length; sIdx++) {
      const seg = segs[sIdx];
      for (const rTarget of seg.redirects) {
        recordTarget(rTarget, "redirection");
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
      const argVals = seg.tokens.slice(idx + 1).map((t) => t.value);

      if (cmdWord === "tee") {
        const files = argVals.filter((a) => !a.startsWith("-"));
        if (files.length === 0) mutatingOps.push("tee");
        else files.forEach((f) => recordTarget(f, "tee"));
      } else if (cmdWord === "rm" || cmdWord === "unlink" || cmdWord === "touch" || cmdWord === "mkdir" || cmdWord === "rmdir") {
        let afterDd = false;
        const files = [];
        for (let k = 0; k < argVals.length; k++) {
          const a = argVals[k];
          if (!afterDd && a === "--") { afterDd = true; continue; }
          if (!afterDd && a.startsWith("-")) {
            if (cmdWord === "touch" && (a === "-d" || a === "-t" || a === "-r")) k++;
            continue;
          }
          files.push(a);
        }
        if (files.length === 0) mutatingOps.push(cmdWord);
        else files.forEach((f) => recordTarget(f, cmdWord));
      } else if (cmdWord === "mv" || cmdWord === "ln") {
        const files = argVals.filter((a) => !a.startsWith("-"));
        if (files.length === 0) mutatingOps.push(cmdWord);
        else files.forEach((f) => recordTarget(f, cmdWord));
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
        if (targetDir) recordTarget(targetDir, cmdWord);
        else if (files.length > 0) recordTarget(files[files.length - 1], cmdWord);
        else mutatingOps.push(cmdWord);
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
          if (files.length === 0) mutatingOps.push("sed -i");
          else files.forEach((f) => recordTarget(f, "sed -i"));
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
          if (files.length === 0) mutatingOps.push("perl -i");
          else files.forEach((f) => recordTarget(f, "perl -i"));
        }
      } else if (cmdWord === "patch") {
        if (!argVals.some((a) => a === "--dry-run" || a === "-C")) {
          mutatingOps.push("patch");
        }
      } else if (cmdWord === "dd") {
        for (const a of argVals) {
          if (a.startsWith("of=")) recordTarget(a.slice(3), "dd of=");
        }
      } else if (cmdWord === "truncate") {
        const files = argVals.filter((a) => !a.startsWith("-"));
        if (files.length === 0) mutatingOps.push("truncate");
        else files.forEach((f) => recordTarget(f, "truncate"));
      } else if (cmdWord === "chmod" || cmdWord === "chown") {
        mutatingOps.push(cmdWord);
      } else if (cmdWord === "git") {
        let k = 0;
        while (k < argVals.length) {
          const a = argVals[k];
          if ((a === "-C" || a === "-c" || a === "--git-dir" || a === "--work-tree" || a === "--namespace") && k + 1 < argVals.length) {
            k += 2;
            continue;
          }
          if (a.startsWith("-")) {
            k++;
            continue;
          }
          break;
        }
        const sub = argVals[k] || "";
        const subArgs = argVals.slice(k + 1);
        const MUTATING_GIT = new Set([
          "commit",
          "push",
          "reset",
          "checkout",
          "restore",
          "apply",
          "merge",
          "cherry-pick",
          "am",
          "revert",
          "rebase",
          "mv",
          "rm",
          "add",
          "stage",
          "clean",
          "switch",
          "pull",
          "fetch",
          "init",
          "clone",
        ]);
        if (MUTATING_GIT.has(sub)) {
          if (sub === "apply" && subArgs.some((a) => a === "--check" || a === "--stat" || a === "--numstat" || a === "--summary")) {
            // read-only check
          } else if (sub === "clean" && subArgs.some((a) => a === "--dry-run" || /^-[A-Za-z]*n/.test(a))) {
            // dry-run
          } else {
            mutatingOps.push("git " + sub);
          }
        } else if (sub === "stash" && subArgs[0] !== "list" && subArgs[0] !== "show") {
          mutatingOps.push("git stash");
        } else if (sub === "branch" && subArgs.some((a) => /^-(?:[dDmMcC]|-delete|-move|-copy)$/.test(a))) {
          mutatingOps.push("git branch");
        } else if (sub === "diff" && subArgs.some((a) => a === "--output" || a.startsWith("--output="))) {
          mutatingOps.push("git diff --output");
        }
      } else if (cmdWord === "node") {
        const codes = [];
        for (let k = 0; k < argVals.length; k++) {
          const a = argVals[k];
          if ((a === "-e" || a === "--eval" || a === "-p" || a === "--print") && k + 1 < argVals.length) codes.push(argVals[++k]);
          else if (a.startsWith("--eval=")) codes.push(a.slice(7));
          else if (a === "-" && item.heredoc) codes.push(item.heredoc);
        }
        if ((seg.hasHeredoc || seg.pipedFromPrev) && item.heredoc) codes.push(item.heredoc);
        if (seg.pipedFromPrev && sIdx > 0) codes.push(segs[sIdx - 1].tokens.map((t) => t.value).join(" "));
        for (const c of codes) {
          const res = inspectNodeTargets(c);
          if (res.matched) {
            if (res.untargeted || res.targets.length === 0) mutatingOps.push("node inline write");
            else res.targets.forEach((t) => recordTarget(t, "node inline write"));
          }
        }
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
        for (const c of codes) {
          const res = inspectPythonTargets(c);
          if (res.matched) {
            if (res.untargeted || res.targets.length === 0) mutatingOps.push("python inline write");
            else res.targets.forEach((t) => recordTarget(t, "python inline write"));
          }
        }
      } else if ((cmdWord === "sh" || cmdWord === "bash" || cmdWord === "zsh") && depth < 1) {
        for (let k = 0; k < argVals.length - 1; k++) {
          if (argVals[k] === "-c") {
            const subRes = inspectShellMutations(argVals[k + 1], depth + 1);
            writes.push(...subRes.writes);
            mutatingOps.push(...subRes.mutatingOps);
          }
        }
      }
    }
  }
  return { writes, mutatingOps };
};

const cmdLine = String(args.CommandLine || args.command || "");
if (!cmdLine.trim()) {
  emit("allow");
}

const blacklisted = detectBlacklistedCommand(cmdLine, 0);
if (blacklisted) {
  emit(
    "deny",
    "BLOCKED by AGY Subagent Guard (" +
      role +
      "): Blacklisted command `" +
      blacklisted +
      "`. Stop and emit ## [HOST_CMD_REQUEST] to delegate container, install, watch/streaming, or interactive commands to the parent coordinator."
  );
}

const mutations = inspectShellMutations(cmdLine, 0);

if (role === "staff-gatekeeper") {
  if (mutations.mutatingOps.length > 0 || mutations.writes.length > 0) {
    const op = mutations.mutatingOps[0] || (mutations.writes[0] && mutations.writes[0].op) || "write";
    emit(
      "deny",
      "BLOCKED by AGY Subagent Guard (staff-gatekeeper): Mutating command (`" +
        op +
        "`) is forbidden. staff-gatekeeper may only execute short read-only checks; delegate mutating or heavy checks via ## [HOST_CMD_REQUEST]."
    );
  }
  emit("allow");
}

// slice-builder: block run_command writes targeting locked contracts or locked harness files
for (const w of mutations.writes) {
  const info = resolveWorkspaceRel(w.raw);
  if (!info.inside) continue;
  const kind = isForbiddenForBuilder(info.rel);
  if (kind === "contract") {
    emit(
      "deny",
      "BLOCKED by AGY Subagent Guard (slice-builder): run_command (" +
        w.op +
        ") MUST NOT modify locked contract `" +
        info.rel +
        "`."
    );
  }
  if (kind === "harness") {
    emit(
      "deny",
      "BLOCKED by AGY Subagent Guard (slice-builder): run_command (" +
        w.op +
        ") MUST NOT modify harness control file `" +
        info.rel +
        "` in target workspaces."
    );
  }
}

emit("allow");
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
    data = json.loads(os.environ.get("AGY_SUBAGENT_STDIN") or "{}")
except Exception:
    emit("allow")

role = str(os.environ.get("AGY_SUBAGENT_ROLE") or "").strip()
if role not in ("slice-builder", "staff-gatekeeper"):
    emit("allow")

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

ws_list = data.get("workspacePaths") or []
raw_ws = (
    str(ws_list[0])
    if ws_list and ws_list[0]
    else (str(data.get("cwd") or "") or os.environ.get("AGY_SUBAGENT_REPO_ROOT") or os.getcwd())
)
ws_root = resolve_real(raw_ws, os.getcwd())

is_self_hosting = False
try:
    pj_path = ws_root / "plugin.json"
    if pj_path.exists():
        pj = json.loads(pj_path.read_text(encoding="utf-8"))
        if isinstance(pj, dict) and pj.get("name") == "agy-harness":
            is_self_hosting = True
except Exception:
    pass

tc = data.get("toolCall") or {}
tool_name = str(tc.get("name") or "")
args = tc.get("args") or {}

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

def extract_target_path(a):
    if not isinstance(a, dict):
        return ""
    for key in ("Path", "TargetFile", "TargetPath", "File", "path", "file", "file_path", "filePath", "target_file"):
        v = a.get(key)
        if v:
            return str(v).strip('"').strip()
    return ""

LOCKED_CONTRACTS = {".artifacts/SPEC.md", ".artifacts/CONSTRAINTS.md", ".artifacts/PLAN.md"}

def is_forbidden_for_builder(rel_path):
    if not rel_path:
        return ""
    if rel_path in LOCKED_CONTRACTS:
        return "contract"
    if not is_self_hosting and (
        rel_path == ".agents"
        or rel_path.startswith(".agents/")
        or bool(re.match(r"^scripts/harness-[^/]*\.sh$", rel_path))
    ):
        return "harness"
    return ""

def resolve_workspace_rel(raw_path):
    cleaned = str(raw_path or "").strip().strip("'\"")
    if not cleaned or cleaned == "/dev/null" or cleaned.startswith("$") or cleaned.startswith("~"):
        return (False, "")
    abs_p = resolve_real(cleaned, ws_root)
    if abs_p == ws_root:
        return (True, "")
    try:
        rel = abs_p.relative_to(ws_root).as_posix()
        return (True, rel)
    except ValueError:
        return (False, "")

if tool_name in EDIT_TOOLS:
    if role == "staff-gatekeeper":
        emit("deny", f"BLOCKED by AGY Subagent Guard (staff-gatekeeper): staff-gatekeeper is strictly read-only and cannot execute `{tool_name}`.")
    raw_target = extract_target_path(args)
    if not raw_target:
        emit("allow")
    inside, rel = resolve_workspace_rel(raw_target)
    if not inside:
        emit("allow")
    kind = is_forbidden_for_builder(rel)
    if kind == "contract":
        emit("deny", f"BLOCKED by AGY Subagent Guard (slice-builder): Locked contract `{rel}` is read-only for slice-builder. Report contract gaps to the parent coordinator instead.")
    if kind == "harness":
        emit("deny", f"BLOCKED by AGY Subagent Guard (slice-builder): Harness control file `{rel}` cannot be modified by slice-builder in target workspaces.")
    emit("allow")

if tool_name != "run_command":
    emit("allow")

if args.get("IsDaemon") is True:
    emit("deny", f"BLOCKED by AGY Subagent Guard ({role}): IsDaemon=true is forbidden. Stop and emit ## [HOST_CMD_REQUEST] to delegate background/daemon commands.")

wait_raw = args.get("WaitMsBeforeAsync")
if wait_raw is not None and wait_raw != "":
    try:
        if float(wait_raw) > 10000:
            emit("deny", f"BLOCKED by AGY Subagent Guard ({role}): WaitMsBeforeAsync={wait_raw} exceeds the 10000ms limit. Stop and emit ## [HOST_CMD_REQUEST] to delegate heavy/slow commands.")
    except Exception:
        pass

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

CONTAINER_BINS = {
    "docker", "docker-compose", "podman", "colima", "orbstack",
    "kubectl", "helm", "kind", "minikube", "testcontainers",
}
SEARCH_BINS = {"grep", "egrep", "fgrep", "rg", "ag", "ack"}

def first_non_flag_args(arg_vals, value_flags):
    out = []
    i = 0
    while i < len(arg_vals):
        a = arg_vals[i]
        if a == "--":
            break
        if a in value_flags and i + 1 < len(arg_vals):
            i += 2
            continue
        if a.startswith("-"):
            i += 1
            continue
        out.append(a)
        i += 1
    return out

def detect_blacklisted_command(cmd, depth=0):
    WRAPPERS = {"command", "sudo", "env", "time", "nohup", "exec"}
    for line, _ in split_heredocs(cmd):
        for tokens, _, _, _ in tokenize_shell_line(line):
            idx = 0
            env_tokens = []
            while idx < len(tokens):
                v = tokens[idx][0]
                if re.match(r"^[A-Za-z_][A-Za-z0-9_]*=", v):
                    env_tokens.append(v)
                    idx += 1
                    continue
                if v in WRAPPERS:
                    idx += 1
                    continue
                break
            if idx >= len(tokens):
                continue
            cmd_word = os.path.basename(tokens[idx][0])
            arg_vals = [t[0] for t in tokens[idx + 1:]]
            is_search = cmd_word in SEARCH_BINS

            if cmd_word in CONTAINER_BINS:
                return cmd_word
            if not is_search:
                for t in env_tokens + arg_vals:
                    if re.search(r"\btestcontainers\b", t, re.IGNORECASE):
                        return "testcontainers"
                if any(a == "--watch" or (a.startswith("--watch=") and not re.match(r"^--watch=(?:false|0|off|none)$", a, re.IGNORECASE)) for a in arg_vals):
                    return "--watch"
                if any(a in ("-it", "-ti") for a in arg_vals):
                    return "-it"
            if cmd_word in ("npm", "pnpm", "yarn", "bun"):
                pos = first_non_flag_args(arg_vals, {"--prefix", "-C", "--cwd", "--filter"})
                if pos and pos[0] in ("install", "i", "add", "ci"):
                    return f"{cmd_word} {pos[0]}"
            elif cmd_word in ("pip", "pip3", "poetry"):
                pos = first_non_flag_args(arg_vals, {"--python", "-C", "--directory"})
                if pos and pos[0] == "install":
                    return f"{cmd_word} install"
            elif cmd_word == "uv":
                pos = first_non_flag_args(arg_vals, {"--python", "--directory"})
                if pos and (pos[0] == "install" or (len(pos) > 1 and pos[0] in ("pip", "pip3") and pos[1] == "install")):
                    return "uv install"
            elif cmd_word in ("python", "python3"):
                for k in range(len(arg_vals) - 2):
                    if arg_vals[k] == "-m" and arg_vals[k + 1] in ("pip", "pip3") and arg_vals[k + 2] == "install":
                        return f"{cmd_word} -m pip install"
            elif cmd_word == "go":
                pos = first_non_flag_args(arg_vals, {"-C"})
                if len(pos) > 1 and pos[0] == "mod" and pos[1] in ("download", "tidy"):
                    return f"go mod {pos[1]}"
            elif cmd_word == "flutter":
                pos = first_non_flag_args(arg_vals, set())
                if len(pos) > 1 and pos[0] == "pub" and pos[1] == "get":
                    return "flutter pub get"
            elif cmd_word == "pod":
                pos = first_non_flag_args(arg_vals, set())
                if pos and pos[0] == "install":
                    return "pod install"
            elif cmd_word == "tail":
                if any(re.match(r"^-[A-Za-z]*[fF]", a) or re.match(r"^--follow(?:=|$)", a) for a in arg_vals):
                    return "tail -f"
            elif cmd_word == "logs" or (arg_vals and arg_vals[0] == "logs"):
                if any(re.match(r"^-[A-Za-z]*f", a) or re.match(r"^--follow(?:=|$)", a) for a in arg_vals):
                    return "logs -f"
            elif cmd_word in ("sh", "bash", "zsh") and depth < 1:
                for k in range(len(arg_vals) - 1):
                    if arg_vals[k] == "-c":
                        inner = detect_blacklisted_command(arg_vals[k + 1], depth + 1)
                        if inner:
                            return inner
    return ""

def inspect_node_targets(code):
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
        matched = True
        literals = re.findall(r"['\"`]([^'\"`\r\n]+)['\"`]", arg_slice)
        cands = literals[:2] if fn in ("renameSync", "rename", "copyFileSync", "copyFile", "cpSync", "cp") else literals[:1]
        if not cands:
            untargeted = True
        else:
            targets.extend(cands)
    return matched, targets, untargeted

def inspect_python_targets(code):
    targets = []
    untargeted = False
    matched = False
    covered = []
    for pm in re.finditer(r"Path\s*\(\s*([^)]*)\)\s*\.\s*(?:write_text|write_bytes|unlink|rename|replace|mkdir|rmdir)\s*\(([^)]*)\)", code):
        covered.append((pm.start(), pm.end()))
        matched = True
        plit = re.search(r"['\"]([^'\"\r\n]+)['\"]", pm.group(1))
        if plit: targets.append(plit.group(1))
        else: untargeted = True
        if re.search(r"\.(?:rename|replace)\s*\(", pm.group(0)):
            dlit = re.search(r"['\"]([^'\"\r\n]+)['\"]", pm.group(2))
            if dlit: targets.append(dlit.group(1))
            else: untargeted = True
    for bw in re.finditer(r"\.\s*(?:write_text|write_bytes|writelines)\s*\(", code):
        if not any(s <= bw.start() <= e for s, e in covered):
            matched = True
            untargeted = True
    for om in re.finditer(r"\bopen\s*\(([^)]*)\)", code):
        inside = om.group(1)
        mm = re.search(r",\s*(?:mode\s*=\s*)?['\"]([rwaAxbtU+]+)['\"]", inside)
        if mm and not re.search(r"[wax+]", mm.group(1), re.IGNORECASE):
            continue
        if not mm and "," not in inside:
            continue
        matched = True
        plit = re.match(r"^\s*['\"]([^'\"\r\n]+)['\"]", inside)
        if plit: targets.append(plit.group(1))
        else: untargeted = True
    for sm in re.finditer(r"\b(?:shutil\.(?:copy\w*|move|rmtree)|os\.(?:remove|unlink|rename|replace|rmdir|makedirs?))\s*\(([^)]*)\)", code):
        matched = True
        lits = re.findall(r"['\"]([^'\"\r\n]+)['\"]", sm.group(1))
        if not lits: untargeted = True
        else: targets.extend(lits)
    return matched, targets, untargeted

def inspect_shell_mutations(cmd_str, depth=0):
    writes = []
    mutating_ops = []
    def record_target(raw_p, op):
        cleaned = str(raw_p or "").strip().strip("'\"")
        if cleaned == "/dev/null":
            return
        writes.append((cleaned, op))
    WRAPPERS = {"command", "sudo", "env", "time", "nohup", "exec"}
    MUTATING_GIT = {
        "commit", "push", "reset", "checkout", "restore", "apply", "merge",
        "cherry-pick", "am", "revert", "rebase", "mv", "rm", "add", "stage",
        "clean", "switch", "pull", "fetch", "init", "clone",
    }
    for line, heredoc in split_heredocs(cmd_str):
        segs = tokenize_shell_line(line)
        for s_idx, (tokens, redirects, has_heredoc, piped_from_prev) in enumerate(segs):
            for rt in redirects:
                record_target(rt, "redirection")
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
            arg_vals = [t[0] for t in tokens[idx + 1:]]

            if cmd_word == "tee":
                files = [a for a in arg_vals if not a.startswith("-")]
                if not files: mutating_ops.append("tee")
                else:
                    for f in files: record_target(f, "tee")
            elif cmd_word in ("rm", "unlink", "touch", "mkdir", "rmdir"):
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
                if not files: mutating_ops.append(cmd_word)
                else:
                    for f in files: record_target(f, cmd_word)
            elif cmd_word in ("mv", "ln"):
                files = [a for a in arg_vals if not a.startswith("-")]
                if not files: mutating_ops.append(cmd_word)
                else:
                    for f in files: record_target(f, cmd_word)
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
                if target_dir: record_target(target_dir, cmd_word)
                elif files: record_target(files[-1], cmd_word)
                else: mutating_ops.append(cmd_word)
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
                    if not files: mutating_ops.append("sed -i")
                    else:
                        for f in files: record_target(f, "sed -i")
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
                    if not files: mutating_ops.append("perl -i")
                    else:
                        for f in files: record_target(f, "perl -i")
            elif cmd_word == "patch":
                if not any(a in ("--dry-run", "-C") for a in arg_vals):
                    mutating_ops.append("patch")
            elif cmd_word == "dd":
                for a in arg_vals:
                    if a.startswith("of="): record_target(a[3:], "dd of=")
            elif cmd_word == "truncate":
                files = [a for a in arg_vals if not a.startswith("-")]
                if not files: mutating_ops.append("truncate")
                else:
                    for f in files: record_target(f, "truncate")
            elif cmd_word in ("chmod", "chown"):
                mutating_ops.append(cmd_word)
            elif cmd_word == "git":
                k = 0
                while k < len(arg_vals):
                    a = arg_vals[k]
                    if a in ("-C", "-c", "--git-dir", "--work-tree", "--namespace") and k + 1 < len(arg_vals):
                        k += 2; continue
                    if a.startswith("-"):
                        k += 1; continue
                    break
                sub = arg_vals[k] if k < len(arg_vals) else ""
                sub_args = arg_vals[k + 1:]
                if sub in MUTATING_GIT:
                    if sub == "apply" and any(a in ("--check", "--stat", "--numstat", "--summary") for a in sub_args):
                        pass
                    elif sub == "clean" and any(a == "--dry-run" or re.match(r"^-[A-Za-z]*n", a) for a in sub_args):
                        pass
                    else:
                        mutating_ops.append(f"git {sub}")
                elif sub == "stash" and (not sub_args or sub_args[0] not in ("list", "show")):
                    mutating_ops.append("git stash")
                elif sub == "branch" and any(re.match(r"^-(?:[dDmMcC]|-delete|-move|-copy)$", a) for a in sub_args):
                    mutating_ops.append("git branch")
                elif sub == "diff" and any(a == "--output" or a.startswith("--output=") for a in sub_args):
                    mutating_ops.append("git diff --output")
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
                for c in codes:
                    m, ts, u = inspect_node_targets(c)
                    if m:
                        if u or not ts: mutating_ops.append("node inline write")
                        else:
                            for t in ts: record_target(t, "node inline write")
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
                for c in codes:
                    m, ts, u = inspect_python_targets(c)
                    if m:
                        if u or not ts: mutating_ops.append("python inline write")
                        else:
                            for t in ts: record_target(t, "python inline write")
            elif cmd_word in ("sh", "bash", "zsh") and depth < 1:
                for k in range(len(arg_vals) - 1):
                    if arg_vals[k] == "-c":
                        sub_w, sub_ops = inspect_shell_mutations(arg_vals[k + 1], depth + 1)
                        writes.extend(sub_w)
                        mutating_ops.extend(sub_ops)
    return writes, mutating_ops

cmd_line = str(args.get("CommandLine") or args.get("command") or "")
if not cmd_line.strip():
    emit("allow")

blacklisted = detect_blacklisted_command(cmd_line, 0)
if blacklisted:
    emit("deny", f"BLOCKED by AGY Subagent Guard ({role}): Blacklisted command `{blacklisted}`. Stop and emit ## [HOST_CMD_REQUEST] to delegate container, install, watch/streaming, or interactive commands to the parent coordinator.")

writes, mutating_ops = inspect_shell_mutations(cmd_line, 0)
if role == "staff-gatekeeper":
    if mutating_ops or writes:
        op = mutating_ops[0] if mutating_ops else writes[0][1]
        emit("deny", f"BLOCKED by AGY Subagent Guard (staff-gatekeeper): Mutating command (`{op}`) is forbidden. staff-gatekeeper may only execute short read-only checks; delegate mutating or heavy checks via ## [HOST_CMD_REQUEST].")
    emit("allow")

for raw_p, op in writes:
    inside, rel = resolve_workspace_rel(raw_p)
    if not inside:
        continue
    kind = is_forbidden_for_builder(rel)
    if kind == "contract":
        emit("deny", f"BLOCKED by AGY Subagent Guard (slice-builder): run_command ({op}) MUST NOT modify locked contract `{rel}`.")
    if kind == "harness":
        emit("deny", f"BLOCKED by AGY Subagent Guard (slice-builder): run_command ({op}) MUST NOT modify harness control file `{rel}` in target workspaces.")

emit("allow")
PY
fi

echo '{"decision":"allow"}'
