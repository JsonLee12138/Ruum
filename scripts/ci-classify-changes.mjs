import { execFileSync } from "node:child_process";
import fs from "node:fs";
import path from "node:path";
import { pathToFileURL } from "node:url";

export const SEMVER_VERSION_LINE_RE =
  /^\s*version\s*=\s*"\d+\.\d+\.\d+(?:-[0-9A-Za-z.-]+)?(?:\+[0-9A-Za-z.-]+)?"\s*(?:#.*)?$/;

export const LOCK_SEMVER_VERSION_LINE_RE =
  /^version\s*=\s*"\d+\.\d+\.\d+(?:-[0-9A-Za-z.-]+)?(?:\+[0-9A-Za-z.-]+)?"\s*$/;

const DOC_OR_METADATA_EXACT = new Set([
  "cliff.toml",
  "LICENSE",
  ".gitignore",
  ".editorconfig",
]);

function normalizeRepoPath(filePath) {
  if (typeof filePath !== "string") return "";
  return filePath.replace(/\\/g, "/").replace(/^(?:\.\/)+/, "");
}

function stripTomlComment(line) {
  let inDouble = false;
  let inSingle = false;
  let escaped = false;

  for (let i = 0; i < line.length; i += 1) {
    const ch = line[i];
    if (inDouble) {
      if (escaped) {
        escaped = false;
      } else if (ch === "\\") {
        escaped = true;
      } else if (ch === '"') {
        inDouble = false;
      }
    } else if (inSingle) {
      if (ch === "'") {
        inSingle = false;
      }
    } else if (ch === '"') {
      inDouble = true;
    } else if (ch === "'") {
      inSingle = true;
    } else if (ch === "#") {
      return line.slice(0, i);
    }
  }
  return line;
}

function parseTomlSectionHeader(line) {
  const trimmed = line.trim();
  if (!trimmed || trimmed.startsWith("#")) return null;
  const withoutComment = stripTomlComment(trimmed).trim();
  const arrayMatch = /^\[\[([^\]]+)\]\]$/.exec(withoutComment);
  if (arrayMatch) {
    return `[[${arrayMatch[1].trim()}]]`;
  }
  const tableMatch = /^\[([^\]]+)\]$/.exec(withoutComment);
  if (tableMatch) {
    return tableMatch[1].trim();
  }
  return null;
}

export function isDocOrMetadataPath(filePath) {
  const normalized = normalizeRepoPath(filePath);
  if (!normalized) return false;
  if (DOC_OR_METADATA_EXACT.has(normalized)) return true;
  if (normalized.endsWith(".md")) return true;
  return (
    normalized.startsWith("docs/") ||
    normalized.startsWith(".agents/") ||
    normalized.startsWith(".artifacts/")
  );
}

export function isCargoTomlVersionOnly(oldText, newText) {
  if (
    typeof oldText !== "string" ||
    typeof newText !== "string" ||
    !oldText ||
    !newText
  ) {
    return false;
  }

  const oldLines = oldText.split(/\r?\n/);
  const newLines = newText.split(/\r?\n/);
  if (oldLines.length !== newLines.length) {
    return false;
  }

  let currentSection = "";
  let changedCount = 0;

  for (let i = 0; i < oldLines.length; i += 1) {
    if (oldLines[i] === newLines[i]) {
      const header = parseTomlSectionHeader(oldLines[i]);
      if (header !== null) {
        currentSection = header;
      }
      continue;
    }

    if (
      currentSection !== "workspace.package" ||
      !SEMVER_VERSION_LINE_RE.test(oldLines[i]) ||
      !SEMVER_VERSION_LINE_RE.test(newLines[i])
    ) {
      return false;
    }
    changedCount += 1;
  }

  return changedCount === 1;
}

function parseTomlMembersArray(lines, startIndex, initialRhs) {
  const members = [];
  let inArray = false;
  let closed = false;

  for (let lineIdx = startIndex; lineIdx < lines.length; lineIdx += 1) {
    const rawSegment = lineIdx === startIndex ? initialRhs : lines[lineIdx];
    const segment = stripTomlComment(rawSegment);

    for (let i = 0; i < segment.length; i += 1) {
      const ch = segment[i];
      if (!inArray) {
        if (/\s/.test(ch)) continue;
        if (ch === "[") {
          inArray = true;
          continue;
        }
        return null;
      }

      if (/\s/.test(ch) || ch === ",") continue;
      if (ch === "]") {
        closed = true;
        break;
      }

      if (ch === '"' || ch === "'") {
        const quote = ch;
        let value = "";
        let foundEnd = false;
        let escaped = false;
        for (i += 1; i < segment.length; i += 1) {
          const inner = segment[i];
          if (quote === '"' && escaped) {
            value += inner;
            escaped = false;
          } else if (quote === '"' && inner === "\\") {
            escaped = true;
          } else if (inner === quote) {
            foundEnd = true;
            break;
          } else {
            value += inner;
          }
        }
        if (!foundEnd) return null;
        members.push(value);
        continue;
      }

      return null;
    }

    if (closed) {
      return members;
    }
  }

  return null;
}

function extractPackageName(memberCargoToml) {
  if (typeof memberCargoToml !== "string" || !memberCargoToml) return null;
  const lines = memberCargoToml.split(/\r?\n/);
  let currentSection = "";

  for (const line of lines) {
    const header = parseTomlSectionHeader(line);
    if (header !== null) {
      currentSection = header;
      continue;
    }
    if (currentSection !== "package") continue;
    const cleaned = stripTomlComment(line).trim();
    const match = /^name\s*=\s*(?:"([^"]+)"|'([^']+)')$/.exec(cleaned);
    if (match) {
      return match[1] ?? match[2] ?? null;
    }
  }

  return null;
}

export function parseWorkspaceCrateNames(rootCargoToml, readMemberCargoToml) {
  if (
    typeof rootCargoToml !== "string" ||
    !rootCargoToml ||
    typeof readMemberCargoToml !== "function"
  ) {
    return new Set();
  }

  const lines = rootCargoToml.split(/\r?\n/);
  let currentSection = "";
  let members = null;

  for (let i = 0; i < lines.length; i += 1) {
    const header = parseTomlSectionHeader(lines[i]);
    if (header !== null) {
      currentSection = header;
      continue;
    }
    if (currentSection !== "workspace") continue;
    const cleaned = stripTomlComment(lines[i]).trim();
    const match = /^members\s*=\s*(.*)$/.exec(cleaned);
    if (match) {
      members = parseTomlMembersArray(lines, i, match[1]);
      break;
    }
  }

  if (!Array.isArray(members) || members.length === 0) {
    return new Set();
  }

  const crateNames = new Set();
  for (const rawMember of members) {
    if (!rawMember || /[*?[\]{}]/.test(rawMember)) {
      return new Set();
    }
    const normalizedMember = normalizeRepoPath(rawMember).replace(/\/+$/, "");
    const memberCargoPath = normalizedMember
      ? `${normalizedMember}/Cargo.toml`
      : "Cargo.toml";
    let memberText;
    try {
      memberText = readMemberCargoToml(memberCargoPath);
    } catch {
      return new Set();
    }
    const crateName = extractPackageName(memberText);
    if (!crateName) {
      return new Set();
    }
    crateNames.add(crateName);
  }

  return crateNames;
}

function parseCargoLockLineMeta(lines) {
  const meta = new Array(lines.length).fill(null);
  let currentBlock = null;

  for (let i = 0; i < lines.length; i += 1) {
    const trimmed = lines[i].trim();
    if (trimmed === "[[package]]") {
      currentBlock = { pkgName: null, hasSource: false };
      meta[i] = currentBlock;
      continue;
    }
    if (trimmed.startsWith("[")) {
      currentBlock = null;
      continue;
    }
    if (currentBlock !== null) {
      meta[i] = currentBlock;
      if (currentBlock.pkgName === null) {
        const nameMatch = /^\s*name\s*=\s*"([^"]+)"\s*$/.exec(lines[i]);
        if (nameMatch) {
          currentBlock.pkgName = nameMatch[1];
        }
      }
      if (/^\s*source\s*=/.test(lines[i])) {
        currentBlock.hasSource = true;
      }
    }
  }

  return meta;
}

export function isCargoLockWorkspaceVersionOnly(
  oldText,
  newText,
  workspaceCrateNames,
) {
  if (
    typeof oldText !== "string" ||
    typeof newText !== "string" ||
    !oldText ||
    !newText ||
    !(workspaceCrateNames instanceof Set) ||
    workspaceCrateNames.size === 0
  ) {
    return false;
  }

  const oldLines = oldText.split(/\r?\n/);
  const newLines = newText.split(/\r?\n/);
  if (oldLines.length !== newLines.length) {
    return false;
  }

  const oldMeta = parseCargoLockLineMeta(oldLines);
  const newMeta = parseCargoLockLineMeta(newLines);
  let changedCount = 0;

  for (let i = 0; i < oldLines.length; i += 1) {
    if (oldLines[i] === newLines[i]) continue;

    const om = oldMeta[i];
    const nm = newMeta[i];
    if (!om || !nm || !om.pkgName || om.pkgName !== nm.pkgName) {
      return false;
    }
    if (!workspaceCrateNames.has(om.pkgName)) {
      return false;
    }
    if (om.hasSource || nm.hasSource) {
      return false;
    }
    if (
      !LOCK_SEMVER_VERSION_LINE_RE.test(oldLines[i]) ||
      !LOCK_SEMVER_VERSION_LINE_RE.test(newLines[i])
    ) {
      return false;
    }
    changedCount += 1;
  }

  return changedCount >= 1;
}

export function classifyChanges({
  eventName,
  changedFiles,
  includeMacosIntel = false,
  cargoTomlVersionOnly = false,
  cargoLockWorkspaceVersionOnly = false,
  fallback = false,
} = {}) {
  if (eventName === "workflow_dispatch") {
    return {
      run_rust: true,
      run_macos_bundle: true,
      run_worker: true,
      run_ci_scripts: true,
      run_macos_intel: Boolean(includeMacosIntel),
      run_windows_release_check: true,
      reason: "workflow_dispatch",
    };
  }

  const files = Array.isArray(changedFiles)
    ? changedFiles.map(normalizeRepoPath).filter(Boolean)
    : [];

  if (fallback || files.length === 0) {
    return {
      run_rust: true,
      run_macos_bundle: true,
      run_worker: true,
      run_ci_scripts: true,
      run_macos_intel: false,
      run_windows_release_check: true,
      reason: "fallback: unable to narrow diff range",
    };
  }

  let run_rust = false;
  let run_macos_bundle = false;
  let run_worker = false;
  let run_ci_scripts = false;
  const run_macos_intel = Boolean(includeMacosIntel);
  let run_windows_release_check = false;

  for (const p of files) {
    if (isDocOrMetadataPath(p)) {
      continue;
    }
    if (p.startsWith("infra/update-worker/")) {
      run_worker = true;
      continue;
    }
    if (
      p === "scripts/ci-classify-changes.mjs" ||
      p === "scripts/ci-classify-changes.test.mjs"
    ) {
      run_ci_scripts = true;
      continue;
    }
    if (p === "Cargo.toml") {
      if (!cargoTomlVersionOnly) {
        run_rust = true;
        run_macos_bundle = true;
        run_windows_release_check = true;
      }
      continue;
    }
    if (p === "Cargo.lock") {
      if (!cargoLockWorkspaceVersionOnly) {
        run_rust = true;
        run_macos_bundle = true;
        run_windows_release_check = true;
      }
      continue;
    }

    run_rust = true;
    if (p.startsWith(".github/workflows/")) {
      run_macos_bundle = true;
      run_worker = true;
      run_ci_scripts = true;
      run_windows_release_check = true;
    } else if (
      p.startsWith("packaging/") ||
      p.startsWith("assets/") ||
      p.startsWith("xtask/") ||
      p === "rustfmt.toml"
    ) {
      run_macos_bundle = true;
      run_windows_release_check = true;
    } else if (p.startsWith("crates/ruum-scan/")) {
      run_macos_bundle = true;
    } else if (p === "crates/ruum-app/build.rs") {
      run_windows_release_check = true;
    }
  }

  const active = [];
  if (run_rust) active.push("rust");
  if (run_macos_bundle) active.push("macos-bundle");
  if (run_worker) active.push("worker");
  if (run_ci_scripts) active.push("ci-scripts");
  if (run_macos_intel) active.push("macos-intel");
  if (run_windows_release_check) active.push("windows-release-check");

  const reason =
    active.length > 0
      ? `triggered: ${active.join(", ")}`
      : "docs, metadata, or workspace version-only changes";

  return {
    run_rust,
    run_macos_bundle,
    run_worker,
    run_ci_scripts,
    run_macos_intel,
    run_windows_release_check,
    reason,
  };
}

function readGitFile(rev, repoPath, cwd) {
  try {
    return execFileSync("git", ["show", `${rev}:${repoPath}`], {
      cwd,
      encoding: "utf8",
      stdio: ["ignore", "pipe", "pipe"],
      maxBuffer: 32 * 1024 * 1024,
    });
  } catch {
    return null;
  }
}

export function runCli(env = process.env, cwd = process.cwd()) {
  const eventName = String(env.GITHUB_EVENT_NAME || "").trim();
  const includeMacosIntel =
    String(env.INCLUDE_MACOS_INTEL || "").trim().toLowerCase() === "true";

  let changedFiles = [];
  let cargoTomlVersionOnly = false;
  let cargoLockWorkspaceVersionOnly = false;
  let fallback = false;

  if (eventName !== "workflow_dispatch") {
    const rawBase =
      eventName === "pull_request"
        ? env.PR_BASE_SHA
        : eventName === "push"
          ? env.EVENT_BEFORE
          : "";
    const base = String(rawBase || "").trim();

    if (!base || /^0+$/.test(base) || base.startsWith("-")) {
      fallback = true;
    } else {
      try {
        execFileSync("git", ["cat-file", "-e", `${base}^{commit}`], {
          cwd,
          stdio: "ignore",
        });
      } catch {
        fallback = true;
      }
    }

    if (!fallback) {
      try {
        const diffOutput = execFileSync(
          "git",
          [
            "-c",
            "core.quotepath=false",
            "diff",
            "--name-only",
            "-z",
            base,
            "HEAD",
          ],
          {
            cwd,
            encoding: "utf8",
            stdio: ["ignore", "pipe", "pipe"],
          },
        );
        changedFiles = diffOutput.split("\0").filter(Boolean);
      } catch {
        fallback = true;
      }
    }

    if (!fallback && changedFiles.length > 0) {
      const normalized = changedFiles.map(normalizeRepoPath);
      if (normalized.includes("Cargo.toml")) {
        cargoTomlVersionOnly = isCargoTomlVersionOnly(
          readGitFile(base, "Cargo.toml", cwd),
          readGitFile("HEAD", "Cargo.toml", cwd),
        );
      }
      if (normalized.includes("Cargo.lock")) {
        const headRootCargoToml = readGitFile("HEAD", "Cargo.toml", cwd);
        const crateNames = parseWorkspaceCrateNames(
          headRootCargoToml,
          (memberPath) => readGitFile("HEAD", memberPath, cwd),
        );
        cargoLockWorkspaceVersionOnly = isCargoLockWorkspaceVersionOnly(
          readGitFile(base, "Cargo.lock", cwd),
          readGitFile("HEAD", "Cargo.lock", cwd),
          crateNames,
        );
      }
    }
  }

  const result = classifyChanges({
    eventName,
    changedFiles,
    includeMacosIntel,
    cargoTomlVersionOnly,
    cargoLockWorkspaceVersionOnly,
    fallback,
  });

  const outputs = [
    `run_rust=${result.run_rust}`,
    `run_macos_bundle=${result.run_macos_bundle}`,
    `run_worker=${result.run_worker}`,
    `run_ci_scripts=${result.run_ci_scripts}`,
    `run_macos_intel=${result.run_macos_intel}`,
    `run_windows_release_check=${result.run_windows_release_check}`,
  ].join("\n");

  if (env.GITHUB_OUTPUT) {
    fs.appendFileSync(env.GITHUB_OUTPUT, `${outputs}\n`, "utf8");
  }
  process.stdout.write(`${outputs}\nreason=${result.reason}\n`);

  if (env.GITHUB_STEP_SUMMARY) {
    const summary = [
      "### CI Change Classification",
      "",
      `- **Event:** \`${eventName || "unknown"}\``,
      `- **Changed files:** ${changedFiles.length}`,
      `- **Reason:** ${result.reason}`,
      "",
      "| Output | Value |",
      "| --- | --- |",
      `| \`run_rust\` | \`${result.run_rust}\` |`,
      `| \`run_macos_bundle\` | \`${result.run_macos_bundle}\` |`,
      `| \`run_worker\` | \`${result.run_worker}\` |`,
      `| \`run_ci_scripts\` | \`${result.run_ci_scripts}\` |`,
      `| \`run_macos_intel\` | \`${result.run_macos_intel}\` |`,
      `| \`run_windows_release_check\` | \`${result.run_windows_release_check}\` |`,
      "",
    ].join("\n");
    fs.appendFileSync(env.GITHUB_STEP_SUMMARY, summary, "utf8");
  }

  return result;
}

if (
  process.argv[1] &&
  import.meta.url === pathToFileURL(path.resolve(process.argv[1])).href
) {
  runCli();
}
