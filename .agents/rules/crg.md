# Code Review Graph (CRG) Workspace Protocol

This repository is indexed by **`code-review-graph`** (`.code-review-graph/graph.db`). Follow these rules on every code exploration, debugging, refactoring, or review task:

## 1. Graph-First Retrieval (No Blind Directory Scans or Greps)

When locating functions, classes, components, call chains (Callers / Callees / Imports), or impacted tests, **do NOT start with** `read_file` / `view_file`, `find_file` / `find_by_name`, `list_directory` / `list_dir`, `search_directory` / `grep_search`, or `run_command` (`grep`, `rg`, `find`).

Query the **`agy-code-graph`** (`code-review-graph`) MCP server first:
1. `get_minimal_context_tool` — Primary entry point (~100 tokens) to orient on task-relevant files, communities, and suggested next tool calls.
2. `semantic_search_nodes_tool` (pass `include_source=true`) — Locate classes, functions, or types by name/keyword with inline source snippets and exact line numbers.
3. `query_graph_tool` — Traverse `callers_of`, `callees_of`, `imports_of`, `importers_of`, `children_of`, `tests_for`, `inheritors_of`, or `file_summary`.
4. `get_impact_radius_tool` / `get_affected_flows_tool` — Compute blast radius before editing shared symbols.
5. `detect_changes_tool` / `get_review_context_tool` — Risk-scored diff and review context for uncommitted or branch changes.
6. `get_flow_tool` / `get_architecture_overview_tool` — Trace execution flows and module boundaries.

## 2. Explicit `repo_root` Routing

Because the plugin MCP server runs as a shared global process across workspaces, **always pass `repo_root="<active_workspace_absolute_path>"` explicitly** in every `code-review-graph` MCP tool call.

## 3. Zero Double-Read Policy

- When `semantic_search_nodes_tool(include_source=true)` or `get_review_context_tool` already returns the relevant source snippet and line range, **never call `read_file` / `view_file` to re-read that same code**.
- When surrounding context is needed, call `read_file` / `view_file` using the exact `line_start`..`line_end` window reported by the graph rather than reading the entire file.
- Fall back to `search_directory` / `grep_search` or `find_file` **only** when dealing with dynamic dispatch (`getattr`, reflection, string-concatenated event names), non-AST config/markdown assets, or after confirming the graph returned zero matches.

## 4. Hook-Managed Incremental Sync

- Routine file edits (`edit_file`, `write_file`, etc.) are automatically tracked via `PostToolUse` (`.code-review-graph/.dirty`) and incrementally indexed in the background on `Stop` (`fullyIdle`) as well as on every `git commit` (`post-commit` hook).
- Do **not** manually invoke `build_or_update_graph_tool` after ordinary edits; reserve `build_or_update_graph_tool` for explicit `/agy-code-graph:build-graph` requests or branch-wide `/agy-code-graph:review-pr` rebuilds.
