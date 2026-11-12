#!/usr/bin/env sh
set -eu

# AGY Code Graph deterministic pure-CLI lifecycle & git hook runner
# Actions:
#   mark-dirty                (PostToolUse hook: fast O(1) dirty flag in initialized workspaces)
#   sync-on-stop              (Stop hook: non-blocking detached incremental sync when fullyIdle)
#   sync-now [workspace_root] (Background worker & git post-commit incremental sync via pure CLI)

PATH="/opt/homebrew/bin:/usr/local/bin:${HOME:-}/.local/bin:${HOME:-}/.cargo/bin:/Library/Frameworks/Python.framework/Versions/3.13/bin:/Library/Frameworks/Python.framework/Versions/3.12/bin:/Library/Frameworks/Python.framework/Versions/3.11/bin:/Library/Frameworks/Python.framework/Versions/3.10/bin:$PATH"
export PATH

ACTION="${1:-}"
SELF_SCRIPT="$(cd "$(dirname "$0")" && pwd)/$(basename "$0")"

run_crg() {
  if command -v code-review-graph >/dev/null 2>&1; then
    code-review-graph "$@"
  elif command -v uvx >/dev/null 2>&1; then
    uvx code-review-graph "$@"
  else
    return 127
  fi
}

crg_available() {
  command -v code-review-graph >/dev/null 2>&1 || command -v uvx >/dev/null 2>&1
}

if [ "$ACTION" = "sync-now" ]; then
  WORKSPACE_ROOT="${2:-$(pwd)}"
  if [ -n "${CRG_HOOK_TEST_DELAY_SEC:-}" ] && [ "${CRG_HOOK_TEST_DELAY_SEC}" -gt 0 ] 2>/dev/null; then
    sleep "${CRG_HOOK_TEST_DELAY_SEC}"
  fi
  if ! crg_available; then
    exit 0
  fi
  CRG_DIR="$WORKSPACE_ROOT/.code-review-graph"
  mkdir -p "$CRG_DIR"
  if [ ! -f "$CRG_DIR/.gitignore" ]; then
    printf '*\n' > "$CRG_DIR/.gitignore" 2>/dev/null || true
  fi

  if [ ! -f "$CRG_DIR/graph.db" ]; then
    run_crg build --repo "$WORKSPACE_ROOT" >/dev/null 2>&1 || true
    exit 0
  fi

  # 1. Sync working-tree changes against HEAD (for Stop hook edits before commit)
  run_crg update --base HEAD --repo "$WORKSPACE_ROOT" >/dev/null 2>&1 || true

  # 2. Sync committed changes against HEAD~1 (for Git post-commit hook)
  if ! run_crg update --repo "$WORKSPACE_ROOT" >/dev/null 2>&1; then
    run_crg build --repo "$WORKSPACE_ROOT" >/dev/null 2>&1 || true
  fi
  exit 0
fi

# Read JSON payload from stdin via pipe to avoid ARG_MAX (E2BIG) on large file writes
PARSED="$(python3 -c '
import json, os, sys

raw = sys.stdin.read().strip()
data = {}
if raw:
    try:
        obj = json.loads(raw)
        if isinstance(obj, dict):
            data = obj
    except Exception:
        data = {}

ws = ""
wps = data.get("workspacePaths")
if isinstance(wps, list) and wps and isinstance(wps[0], str):
    ws = wps[0].strip()
if not ws:
    ws = data.get("cwd") or data.get("workspaceRoot") or ""
if ws:
    ws = os.path.realpath(ws)

err = data.get("error")
has_err = "1" if (err is not None and err != "" and err is not False) else "0"

fully_idle_val = data.get("fullyIdle")
fully_idle = "false" if fully_idle_val is False else "true"

target_file = ""
tc = data.get("toolCall")
if isinstance(tc, dict):
    args = tc.get("args")
    if isinstance(args, dict):
        for k in ("Path", "TargetFile", "file_path", "path"):
            v = args.get(k)
            if isinstance(v, str) and v.strip():
                target_file = v.strip()
                break
if not target_file:
    ti = data.get("tool_input")
    if isinstance(ti, dict):
        for k in ("file_path", "Path", "TargetFile", "path"):
            v = ti.get(k)
            if isinstance(v, str) and v.strip():
                target_file = v.strip()
                break

if target_file:
    if ws and not os.path.isabs(target_file):
        target_file = os.path.realpath(os.path.join(ws, target_file))
    else:
        target_file = os.path.realpath(target_file)

print(ws)
print(has_err)
print(fully_idle)
print(target_file)
' 2>/dev/null || true)"

WORKSPACE_ROOT="$(printf '%s\n' "$PARSED" | sed -n '1p')"
HAS_ERROR="$(printf '%s\n' "$PARSED" | sed -n '2p')"
FULLY_IDLE="$(printf '%s\n' "$PARSED" | sed -n '3p')"
TARGET_FILE="$(printf '%s\n' "$PARSED" | sed -n '4p')"

if [ -z "$WORKSPACE_ROOT" ]; then
  WORKSPACE_ROOT="$(git rev-parse --show-toplevel 2>/dev/null || pwd)"
fi

CRG_DIR="$WORKSPACE_ROOT/.code-review-graph"
DIRTY_FLAG="$CRG_DIR/.dirty"
LOCK_DIR="$CRG_DIR/update.lock.d"

case "$ACTION" in
  mark-dirty)
    if [ "${HAS_ERROR:-0}" = "1" ]; then
      printf '{}\n'
      exit 0
    fi

    # Zero-pollution guard: only mark dirty in workspaces initialized for CRG
    if [ ! -d "$CRG_DIR" ] && [ ! -f "$WORKSPACE_ROOT/.agents/rules/crg.md" ]; then
      printf '{}\n'
      exit 0
    fi

    # Ignore edits outside WORKSPACE_ROOT (e.g. <ArtifactDir>) or inside .git / .code-review-graph
    if [ -n "$TARGET_FILE" ]; then
      case "$TARGET_FILE" in
        "$WORKSPACE_ROOT/.git/"*|"$WORKSPACE_ROOT/.code-review-graph/"*)
          printf '{}\n'
          exit 0
          ;;
        "$WORKSPACE_ROOT"/*)
          ;;
        *)
          printf '{}\n'
          exit 0
          ;;
      esac
    fi

    mkdir -p "$CRG_DIR" 2>/dev/null || true
    if [ ! -f "$CRG_DIR/.gitignore" ]; then
      printf '*\n' > "$CRG_DIR/.gitignore" 2>/dev/null || true
    fi
    : > "$DIRTY_FLAG" 2>/dev/null || true
    printf '{}\n'
    exit 0
    ;;

  sync-on-stop)
    if [ "${FULLY_IDLE:-true}" = "false" ]; then
      printf '{"decision":"allow"}\n'
      exit 0
    fi

    if [ ! -f "$DIRTY_FLAG" ]; then
      printf '{"decision":"allow"}\n'
      exit 0
    fi

    if ! crg_available; then
      printf '{"decision":"allow"}\n'
      exit 0
    fi

    # Clean stale crash lock older than 10 minutes
    if [ -d "$LOCK_DIR" ]; then
      STALE_LOCK="$(find "$CRG_DIR" -maxdepth 1 -name "update.lock.d" -mmin +10 2>/dev/null || true)"
      if [ -n "$STALE_LOCK" ]; then
        rmdir "$LOCK_DIR" 2>/dev/null || true
      fi
    fi

    # Atomic directory lock + fully detached background execution (closes stdout/stderr/stdin pipes)
    if mkdir "$LOCK_DIR" 2>/dev/null; then
      rm -f "$DIRTY_FLAG" 2>/dev/null || true
      (
        trap 'rmdir "$LOCK_DIR" 2>/dev/null || true' EXIT INT TERM
        "$SELF_SCRIPT" sync-now "$WORKSPACE_ROOT"
      ) >/dev/null 2>&1 </dev/null &
    fi

    printf '{"decision":"allow"}\n'
    exit 0
    ;;

  *)
    printf '{}\n'
    exit 0
    ;;
esac
