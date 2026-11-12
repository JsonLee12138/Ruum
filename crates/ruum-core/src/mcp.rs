//! Headless Model Context Protocol (MCP) JSON-RPC 2.0 server over `stdio`.
//!
//! Exposes disk discovery, scanning, subtree inspection, reclaimable space
//! findings, Git checkout safety checks, and removal guard planning without
//! initializing GPUI or opening any window.

use std::fmt;
use std::fs;
use std::io::{self, BufRead, Read as _, Write};
use std::path::{Component, Path, PathBuf};

use rustc_hash::FxHashMap;
use serde_json::{Value, json};

use crate::auth::{AuthAdapter, AuthFactory, AuthRequest};
use crate::classify::{Category, Reclaim};
use crate::filter::{Keep, filter};
use crate::git::{self, GitState};
use crate::identity::HostIdentity;
use crate::insights::{Finding, worth_a_look};
use crate::remote::{Header, RemoteRoots, RemoteScan, Request};
use crate::remote_trash::{execute_remote_trash, plan_remote};
use crate::removal::{
    Blocked, Plan, RemovalEvent, RemovalMode, Target, TrashBackend,
    detect_trash_backend, normalize, plan, spawn,
};
use crate::scan::{ScanOptions, scan};
use crate::size::human_bytes;
use crate::space::{SpaceInfo, space_info, volume_root_for, volumes};
use crate::tree::{Node, NodeKind};

/// Maximum UTF-8 bytes accepted for a single JSON-RPC request line (`1 MiB`).
pub const MAX_LINE: u64 = 1 << 20;

/// Negotiated MCP protocol version.
pub const PROTOCOL_VERSION: &str = "2024-11-05";

const DEFAULT_MAX_ENTRIES: usize = 25;
const MAX_ENTRIES_LIMIT: usize = 100;
const DEFAULT_RECLAIMABLE_LIMIT: usize = 20;
const MAX_RECLAIMABLE_LIMIT: usize = 100;

/// Cached result of a local or remote directory scan.
#[derive(Clone, Debug)]
pub struct CachedScan {
    pub tree: Node,
    pub space: Option<SpaceInfo>,
    pub home: Option<PathBuf>,
    pub platform: String,
    pub identity: Option<HostIdentity>,
    pub options: ScanOptions,
    pub scan_count: usize,
}

impl CachedScan {
    /// Rebuild the remote scan [`Header`] needed by remote removal guards.
    #[must_use]
    pub fn to_remote_header(&self, root: &Path) -> Header {
        let mut header = Header::new(
            root.to_path_buf(),
            self.home.clone(),
            self.platform.clone(),
            self.identity.clone(),
        );
        header.space = self.space;
        header
    }
}

/// Stateful MCP session server holding the scan cache and OS auth adapter.
pub struct McpServer {
    cache: FxHashMap<(Option<String>, PathBuf), CachedScan>,
    auth: Box<dyn AuthAdapter>,
}

impl fmt::Debug for McpServer {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("McpServer")
            .field("cached_roots", &self.cache.len())
            .field("auth_os", &self.auth.os_kind())
            .finish()
    }
}

impl Default for McpServer {
    fn default() -> Self {
        Self::new(AuthFactory::create())
    }
}

impl McpServer {
    #[must_use]
    pub fn new(auth: Box<dyn AuthAdapter>) -> Self {
        Self {
            cache: FxHashMap::default(),
            auth,
        }
    }

    #[must_use]
    pub const fn cache(
        &self,
    ) -> &FxHashMap<(Option<String>, PathBuf), CachedScan> {
        &self.cache
    }

    pub fn insert_cached(
        &mut self,
        ssh_host: Option<String>,
        root: PathBuf,
        entry: CachedScan,
    ) {
        self.cache.insert((ssh_host, root), entry);
    }

    /// Invalidate any cached entry for `ssh_host` whose root is an ancestor or
    /// descendant of `path` so subsequent calls never read stale sizes.
    pub fn invalidate_overlapping(
        &mut self,
        ssh_host: Option<&str>,
        path: &Path,
    ) {
        let norm = normalize(path);
        self.cache.retain(|(host, root), _| {
            if host.as_deref() != ssh_host {
                return true;
            }
            let root_norm = normalize(root);
            !(root_norm.starts_with(&norm) || norm.starts_with(&root_norm))
        });
    }

    /// Serve newline-delimited JSON-RPC 2.0 messages until `reader` hits EOF.
    pub fn serve(
        &mut self,
        reader: &mut impl BufRead,
        writer: &mut impl Write,
    ) -> io::Result<()> {
        let mut line = String::new();
        loop {
            line.clear();
            let read_bytes =
                reader.by_ref().take(MAX_LINE + 1).read_line(&mut line)?;
            if read_bytes == 0 {
                break;
            }
            if read_bytes as u64 > MAX_LINE {
                return Err(io::Error::new(
                    io::ErrorKind::InvalidData,
                    "oversized JSON-RPC line exceeds 1 MiB limit",
                ));
            }
            let trimmed = line.trim();
            if trimmed.is_empty() {
                continue;
            }
            let message: Value = match serde_json::from_str(trimmed) {
                Ok(val) => val,
                Err(err) => {
                    write_error(
                        writer,
                        &Value::Null,
                        -32700,
                        &format!("Parse error: {err}"),
                    )?;
                    continue;
                }
            };
            self.handle_message(&message, writer)?;
        }
        Ok(())
    }

    fn handle_message(
        &mut self,
        message: &Value,
        writer: &mut impl Write,
    ) -> io::Result<()> {
        let Some(obj) = message.as_object() else {
            return write_error(
                writer,
                &Value::Null,
                -32600,
                "Invalid Request",
            );
        };
        let id = obj.get("id").cloned();
        let Some(method) = obj.get("method").and_then(Value::as_str) else {
            return write_error(
                writer,
                &id.unwrap_or(Value::Null),
                -32600,
                "Invalid Request: missing method",
            );
        };
        // JSON-RPC notifications omit `id` and must not produce a response.
        let Some(request_id) = id else {
            return Ok(());
        };
        let params = obj.get("params").cloned().unwrap_or(Value::Null);
        match method {
            "initialize" => write_result(
                writer,
                &request_id,
                &json!({
                    "protocolVersion": PROTOCOL_VERSION,
                    "capabilities": {
                        "tools": {}
                    },
                    "serverInfo": {
                        "name": "ruum",
                        "version": env!("CARGO_PKG_VERSION"),
                    }
                }),
            ),
            "ping" => write_result(writer, &request_id, &json!({})),
            "tools/list" => write_result(
                writer,
                &request_id,
                &json!({ "tools": tool_definitions() }),
            ),
            "tools/call" => self.handle_tool_call(&request_id, &params, writer),
            _ => write_error(
                writer,
                &request_id,
                -32601,
                &format!("Method not found: {method}"),
            ),
        }
    }

    fn handle_tool_call(
        &mut self,
        request_id: &Value,
        params: &Value,
        writer: &mut impl Write,
    ) -> io::Result<()> {
        let Some(name) = params.get("name").and_then(Value::as_str) else {
            return write_error(
                writer,
                request_id,
                -32602,
                "Invalid params: missing tool name",
            );
        };
        let args = match params.get("arguments") {
            None | Some(Value::Null) => json!({}),
            Some(val) if val.is_object() => val.clone(),
            Some(_) => {
                return write_error(
                    writer,
                    request_id,
                    -32602,
                    "Invalid params: arguments must be an object",
                );
            }
        };
        if !is_known_tool(name) {
            return write_error(
                writer,
                request_id,
                -32601,
                &format!("Unknown tool: {name}"),
            );
        }
        let outcome = self.dispatch_tool(name, &args);
        let body = match outcome {
            Ok(payload) => {
                let text = serde_json::to_string(&payload)
                    .unwrap_or_else(|err| err.to_string());
                json!({
                    "content": [{ "type": "text", "text": text }],
                    "isError": false,
                })
            }
            Err(err_text) => json!({
                "content": [{ "type": "text", "text": err_text }],
                "isError": true,
            }),
        };
        write_result(writer, request_id, &body)
    }

    fn dispatch_tool(
        &mut self,
        name: &str,
        args: &Value,
    ) -> Result<Value, String> {
        match name {
            "list_volumes" => Self::tool_list_volumes(args),
            "scan_directory" => self.tool_scan_directory(args),
            "inspect_subtree" => self.tool_inspect_subtree(args),
            "find_reclaimable" => self.tool_find_reclaimable(args),
            "check_git_safety" => Self::tool_check_git_safety(args),
            "plan_removal" => self.tool_plan_removal(args),
            "trash_paths" => self.tool_trash_paths(args),
            "delete_permanently" => self.tool_delete_permanently(args),
            _ => Err(format!("Unsupported tool: {name}")),
        }
    }

    fn tool_list_volumes(args: &Value) -> Result<Value, String> {
        let ssh_host = optional_host(args);
        if let Some(host) = ssh_host {
            let roots = RemoteRoots::discover(&host)
                .and_then(RemoteRoots::wait)
                .map_err(|err| err.to_string())?;
            let list: Vec<Value> =
                roots.volumes.iter().map(format_volume).collect();
            return Ok(json!({
                "ssh_host": host,
                "platform": roots.platform,
                "home": roots.home.map(|p| p.display().to_string()),
                "identity": roots.identity.map(|id| json!({
                    "hostname": id.hostname,
                    "machine_id": id.machine_id,
                })),
                "volumes": list,
            }));
        }
        let list: Vec<Value> = volumes().iter().map(format_volume).collect();
        Ok(json!({
            "ssh_host": Value::Null,
            "platform": std::env::consts::OS,
            "home": std::env::home_dir().map(|p| p.display().to_string()),
            "volumes": list,
        }))
    }

    fn tool_scan_directory(&mut self, args: &Value) -> Result<Value, String> {
        let ssh_host = optional_host(args);
        let path_arg = args.get("path").and_then(Value::as_str);
        let disk = args.get("disk").and_then(Value::as_bool).unwrap_or(false);
        let apparent_size = args
            .get("apparent_size")
            .and_then(Value::as_bool)
            .unwrap_or(false);
        let include_hidden = args
            .get("include_hidden")
            .and_then(Value::as_bool)
            .unwrap_or(true);
        let rescan =
            args.get("rescan").and_then(Value::as_bool).unwrap_or(false);
        let max_entries = parse_limit(
            args.get("max_entries"),
            DEFAULT_MAX_ENTRIES,
            MAX_ENTRIES_LIMIT,
        );
        let options = ScanOptions {
            apparent_size,
            include_hidden,
            ..ScanOptions::default()
        };
        let (root_path, cached_hit) = self.ensure_scanned(
            ssh_host.as_deref(),
            path_arg,
            disk,
            &options,
            rescan,
        )?;
        let key = (ssh_host.clone(), root_path.clone());
        let entry = self
            .cache
            .get(&key)
            .ok_or_else(|| "scan cache missing entry".to_owned())?;
        Ok(format_scan_summary(
            ssh_host.as_deref(),
            &root_path,
            entry,
            cached_hit,
            max_entries,
        ))
    }

    fn tool_inspect_subtree(&mut self, args: &Value) -> Result<Value, String> {
        let ssh_host = optional_host(args);
        let root_str = required_str(args, "root")?;
        let subpath = args.get("subpath").and_then(Value::as_str);
        let filter_query = args
            .get("filter")
            .and_then(Value::as_str)
            .map(str::trim)
            .filter(|s| !s.is_empty());
        let max_entries = parse_limit(
            args.get("max_entries"),
            DEFAULT_MAX_ENTRIES,
            MAX_ENTRIES_LIMIT,
        );
        let (root_path, _) =
            self.ensure_cached_or_default_scan(ssh_host.as_deref(), root_str)?;
        let key = (ssh_host, root_path.clone());
        let entry = self
            .cache
            .get(&key)
            .ok_or_else(|| "scan cache missing entry".to_owned())?;
        let (crumbs, target_path, target_node) =
            resolve_subpath(&root_path, &entry.tree, subpath)?;
        if let Some(needle) = filter_query {
            let matches =
                filter(target_node, &crumbs, needle).unwrap_or_default();
            let mut matched_nodes: Vec<(PathBuf, &Node)> = matches
                .keep
                .iter()
                .filter_map(|(match_crumbs, keep)| {
                    (*keep == Keep::Whole).then(|| {
                        let node = entry.tree.resolve(match_crumbs)?;
                        let path = path_for_crumbs(
                            &root_path,
                            &entry.tree,
                            match_crumbs,
                        );
                        Some((path, node))
                    })?
                })
                .collect();
            matched_nodes.sort_by(|left, right| {
                right
                    .1
                    .bytes
                    .cmp(&left.1.bytes)
                    .then_with(|| left.0.cmp(&right.0))
            });
            let items: Vec<Value> = matched_nodes
                .into_iter()
                .take(max_entries)
                .map(|(path, node)| format_node_entry(&path, node))
                .collect();
            return Ok(json!({
                "root": root_path.display().to_string(),
                "path": target_path.display().to_string(),
                "filter": needle,
                "match_count": matches.count,
                "matched_bytes": matches.bytes,
                "matched_files": matches.files,
                "human_size": human_bytes(matches.bytes),
                "entries": items,
            }));
        }
        let children =
            format_top_children(&target_path, target_node, max_entries);
        Ok(json!({
            "root": root_path.display().to_string(),
            "path": target_path.display().to_string(),
            "name": &*target_node.name,
            "kind": kind_label(target_node.kind),
            "bytes": target_node.bytes,
            "files": target_node.files,
            "dirs": target_node.dirs,
            "category": target_node.category.label(),
            "reclaim": target_node.reclaim.map(Reclaim::label),
            "human_size": human_bytes(target_node.bytes),
            "entries": children,
        }))
    }

    fn tool_find_reclaimable(&mut self, args: &Value) -> Result<Value, String> {
        let ssh_host = optional_host(args);
        let root_str = required_str(args, "root")?;
        let limit = parse_limit(
            args.get("limit"),
            DEFAULT_RECLAIMABLE_LIMIT,
            MAX_RECLAIMABLE_LIMIT,
        );
        let (root_path, _) =
            self.ensure_cached_or_default_scan(ssh_host.as_deref(), root_str)?;
        let key = (ssh_host, root_path.clone());
        let entry = self
            .cache
            .get(&key)
            .ok_or_else(|| "scan cache missing entry".to_owned())?;
        let now = chrono::Utc::now().timestamp();
        let candidates = worth_a_look(&entry.tree, now, limit);
        let mut total_reclaimable = 0_u64;
        let items: Vec<Value> = candidates
            .into_iter()
            .filter_map(|candidate| {
                let node = entry.tree.resolve(&candidate.crumbs)?;
                let path =
                    path_for_crumbs(&root_path, &entry.tree, &candidate.crumbs);
                total_reclaimable =
                    total_reclaimable.saturating_add(candidate.bytes);
                Some(json!({
                    "name": &*node.name,
                    "path": path.display().to_string(),
                    "bytes": candidate.bytes,
                    "human_size": human_bytes(candidate.bytes),
                    "category": node.category.label(),
                    "reclaim": node.reclaim.map(Reclaim::label),
                    "finding": format_finding(&candidate.finding),
                }))
            })
            .collect();
        Ok(json!({
            "root": root_path.display().to_string(),
            "total_reclaimable_bytes": total_reclaimable,
            "human_total_reclaimable": human_bytes(total_reclaimable),
            "candidates": items,
        }))
    }

    fn tool_check_git_safety(args: &Value) -> Result<Value, String> {
        if args.get("ssh_host").is_some_and(|v| !v.is_null()) {
            return Err(
                "Git safety inspection is local-only; ssh_host is not \
                 supported."
                    .to_owned(),
            );
        }
        let paths = required_string_array(args, "paths")?;
        let results: Vec<Value> = paths
            .into_iter()
            .map(|raw| {
                let path = Path::new(&raw);
                let checkout = git::is_checkout(path);
                let state = git::state(path);
                json!({
                    "path": raw,
                    "is_checkout": checkout,
                    "is_clean": state.as_ref().map(GitState::is_clean),
                    "changed": state.as_ref().and_then(|s| s.changed),
                    "stashes": state.as_ref().map(|s| s.stashes),
                    "unpushed": state.as_ref().and_then(|s| s.unpushed),
                    "summary": state.as_ref().map_or_else(
                        || "not a git checkout".to_owned(),
                        GitState::summary,
                    ),
                })
            })
            .collect();
        Ok(json!({ "results": results }))
    }

    fn tool_plan_removal(&self, args: &Value) -> Result<Value, String> {
        let ssh_host = optional_host(args);
        let root_str = required_str(args, "root")?;
        let raw_targets = required_string_array(args, "targets")?;
        if let Some(host) = ssh_host {
            let root_path = normalize(Path::new(root_str));
            let key = (Some(host.clone()), root_path.clone());
            let cached = self.cache.get(&key).ok_or_else(|| {
                format!(
                    "No cached remote scan for {host}:{}; run scan_directory \
                     first.",
                    root_path.display()
                )
            })?;
            let targets = build_targets(
                &raw_targets,
                &root_path,
                Some(&cached.tree),
                true,
            );
            let header = cached.to_remote_header(&root_path);
            let remote_plan = plan_remote(&targets, &header);
            return Ok(json!({
                "ssh_host": host,
                "root": remote_plan.root.display().to_string(),
                "targets": format_targets(&remote_plan.targets),
                "covered": format_targets(&remote_plan.covered),
                "blocked": format_blocked(&remote_plan.blocked),
                "total_bytes": remote_plan.bytes(),
                "reclaim_bytes": remote_plan.reclaim(),
                "foreign_bytes": 0,
                "human_reclaim": human_bytes(remote_plan.reclaim()),
                "trash_available": true,
                "allowed_modes": ["trash"],
            }));
        }
        let root_path = resolve_local_path(root_str)?;
        let cached_tree =
            self.cache.get(&(None, root_path.clone())).map(|c| &c.tree);
        let targets =
            build_targets(&raw_targets, &root_path, cached_tree, false);
        let local_plan = plan(&targets, &root_path);
        let backend = detect_trash_backend();
        let trash_available = !matches!(backend, TrashBackend::Unavailable);
        let allowed_modes = if trash_available {
            vec!["trash", "permanent"]
        } else {
            vec!["permanent"]
        };
        Ok(json!({
            "root": local_plan.root.display().to_string(),
            "targets": format_targets(&local_plan.targets),
            "covered": format_targets(&local_plan.covered),
            "blocked": format_blocked(&local_plan.blocked),
            "total_bytes": local_plan.bytes(),
            "reclaim_bytes": local_plan.reclaim(),
            "foreign_bytes": local_plan.foreign(),
            "human_reclaim": human_bytes(local_plan.reclaim()),
            "trash_available": trash_available,
            "trash_backend": backend.label(),
            "allowed_modes": allowed_modes,
        }))
    }

    fn tool_trash_paths(&mut self, args: &Value) -> Result<Value, String> {
        let ssh_host = optional_host(args);
        let root_str = required_str(args, "root")?;
        let raw_targets = required_string_array(args, "targets")?;
        if let Some(host) = ssh_host {
            let root_path = normalize(Path::new(root_str));
            let key = (Some(host.clone()), root_path.clone());
            let cached = self.cache.get(&key).ok_or_else(|| {
                format!(
                    "No cached remote scan for {host}:{}; run scan_directory \
                     first.",
                    root_path.display()
                )
            })?;
            let targets = build_targets(
                &raw_targets,
                &root_path,
                Some(&cached.tree),
                true,
            );
            let header = cached.to_remote_header(&root_path);
            let remote_plan = plan_remote(&targets, &header);
            if remote_plan.targets.is_empty() || !remote_plan.blocked.is_empty()
            {
                return Err(format_plan_rejection(
                    "trash_paths",
                    &remote_plan.root,
                    &remote_plan.blocked,
                ));
            }
            execute_remote_trash(&host, &remote_plan.targets, &header)
                .map_err(|err| err.to_string())?;
            self.invalidate_overlapping(Some(&host), &root_path);
            return Ok(json!({
                "mode": "trash",
                "target": host,
                "ssh_host": host,
                "root": remote_plan.root.display().to_string(),
                "removed_count": remote_plan.targets.len(),
                "reclaim_bytes": remote_plan.reclaim(),
                "human_reclaim": human_bytes(remote_plan.reclaim()),
            }));
        }

        let root_path = resolve_local_path(root_str)?;
        let cached_tree =
            self.cache.get(&(None, root_path.clone())).map(|c| &c.tree);
        let targets =
            build_targets(&raw_targets, &root_path, cached_tree, false);
        let local_plan = plan(&targets, &root_path);
        if local_plan.targets.is_empty() || !local_plan.blocked.is_empty() {
            return Err(format_plan_rejection(
                "trash_paths",
                &local_plan.root,
                &local_plan.blocked,
            ));
        }
        let backend = detect_trash_backend();
        if matches!(backend, TrashBackend::Unavailable) {
            return Err(
                "trash backend is unavailable on this system".to_owned()
            );
        }
        let space_before = space_info(&root_path).ok();
        let summary = run_local_removal(local_plan, RemovalMode::Trash);
        self.invalidate_overlapping(None, &root_path);
        let space_after = space_info(&root_path).ok();
        if !summary.errors.is_empty() || summary.failed_count > 0 {
            return Err(format_removal_failure(
                "trash",
                &root_path,
                &summary,
                space_before,
                space_after,
            ));
        }
        Ok(json!({
            "mode": "trash",
            "root": root_path.display().to_string(),
            "trash_backend": backend.label(),
            "removed_count": summary.removed_count,
            "removed_bytes": summary.removed_bytes,
            "human_removed": human_bytes(summary.removed_bytes),
            "errors": summary.errors,
            "space_before": space_before.map(format_space),
            "space_after": space_after.map(format_space),
        }))
    }

    fn tool_delete_permanently(
        &mut self,
        args: &Value,
    ) -> Result<Value, String> {
        if args.get("ssh_host").is_some_and(|v| !v.is_null()) {
            return Err(
                "Permanent deletion over SSH is forbidden; use trash_paths \
                 instead."
                    .to_owned(),
            );
        }
        let root_str = required_str(args, "root")?;
        let raw_targets = required_string_array(args, "targets")?;
        let caller_reason = args.get("reason").and_then(Value::as_str);
        let root_path = resolve_local_path(root_str)?;
        let cached_tree =
            self.cache.get(&(None, root_path.clone())).map(|c| &c.tree);
        let targets =
            build_targets(&raw_targets, &root_path, cached_tree, false);
        let local_plan = plan(&targets, &root_path);
        if local_plan.targets.is_empty() || !local_plan.blocked.is_empty() {
            return Err(format_plan_rejection(
                "delete_permanently",
                &local_plan.root,
                &local_plan.blocked,
            ));
        }
        let request = AuthRequest::for_permanent_delete_with_reason(
            &local_plan,
            caller_reason,
        );
        match self.auth.authenticate(&request) {
            Ok(true) => {}
            Ok(false) => {
                return Err("permanent deletion cancelled by user".to_owned());
            }
            Err(err) => {
                return Err(format!("OS authentication failed closed: {err}"));
            }
        }
        let space_before = space_info(&root_path).ok();
        let summary = run_local_removal(local_plan, RemovalMode::Permanent);
        self.invalidate_overlapping(None, &root_path);
        let space_after = space_info(&root_path).ok();
        if !summary.errors.is_empty() || summary.failed_count > 0 {
            return Err(format_removal_failure(
                "permanent",
                &root_path,
                &summary,
                space_before,
                space_after,
            ));
        }
        Ok(json!({
            "mode": "permanent",
            "root": root_path.display().to_string(),
            "removed_count": summary.removed_count,
            "removed_bytes": summary.removed_bytes,
            "human_removed": human_bytes(summary.removed_bytes),
            "errors": summary.errors,
            "space_before": space_before.map(format_space),
            "space_after": space_after.map(format_space),
        }))
    }

    fn ensure_cached_or_default_scan(
        &mut self,
        ssh_host: Option<&str>,
        root_str: &str,
    ) -> Result<(PathBuf, bool), String> {
        let candidate_root = if ssh_host.is_some() {
            normalize(Path::new(root_str))
        } else {
            resolve_local_path(root_str)?
        };
        let key = (ssh_host.map(str::to_owned), candidate_root.clone());
        if self.cache.contains_key(&key) {
            return Ok((candidate_root, true));
        }
        let default_options = ScanOptions::default();
        self.ensure_scanned(
            ssh_host,
            Some(root_str),
            false,
            &default_options,
            false,
        )
    }

    fn ensure_scanned(
        &mut self,
        ssh_host: Option<&str>,
        path_arg: Option<&str>,
        disk: bool,
        options: &ScanOptions,
        rescan: bool,
    ) -> Result<(PathBuf, bool), String> {
        if let Some(host) = ssh_host {
            if !rescan && let Some(raw) = path_arg {
                let norm = normalize(Path::new(raw));
                let key = (Some(host.to_owned()), norm.clone());
                if let Some(existing) = self.cache.get(&key)
                    && same_scan_options(&existing.options, options)
                {
                    return Ok((norm, true));
                }
            }
            let request = Request {
                root: path_arg.map(PathBuf::from),
                disk,
                options: options.clone(),
            };
            let snapshot = RemoteScan::spawn(host, &request)
                .and_then(RemoteScan::wait)
                .map_err(|err| err.to_string())?;
            let root_path = normalize(&snapshot.header.root);
            let key = (Some(host.to_owned()), root_path.clone());
            let prev_count = self.cache.get(&key).map_or(0, |c| c.scan_count);
            self.cache.insert(
                key,
                CachedScan {
                    tree: snapshot.tree,
                    space: snapshot.header.space,
                    home: snapshot.header.home,
                    platform: snapshot.header.platform,
                    identity: snapshot.header.identity,
                    options: options.clone(),
                    scan_count: prev_count + 1,
                },
            );
            return Ok((root_path, false));
        }

        let root_path = resolve_local_scan_root(path_arg, disk)?;
        let key = (None, root_path.clone());
        if !rescan
            && let Some(existing) = self.cache.get(&key)
            && same_scan_options(&existing.options, options)
        {
            return Ok((root_path, true));
        }
        let tree =
            scan(&root_path, options.clone()).map_err(|err| err.to_string())?;
        let prev_count = self.cache.get(&key).map_or(0, |c| c.scan_count);
        let entry = CachedScan {
            tree,
            space: space_info(&root_path).ok(),
            home: std::env::home_dir(),
            platform: std::env::consts::OS.to_owned(),
            identity: crate::identity::HostIdentity::current(),
            options: options.clone(),
            scan_count: prev_count + 1,
        };
        self.cache.insert(key, entry);
        Ok((root_path, false))
    }
}

/// Serve MCP requests over an arbitrary reader/writer pair.
pub fn serve(
    mut reader: impl BufRead,
    mut writer: impl Write,
    auth: Box<dyn AuthAdapter>,
) -> io::Result<()> {
    let mut server = McpServer::new(auth);
    server.serve(&mut reader, &mut writer)
}

/// Serve MCP requests over process `stdin` and `stdout` using the host OS
/// authentication adapter.
pub fn serve_stdio() -> io::Result<()> {
    let stdin = io::stdin();
    let stdout = io::stdout();
    let mut reader = stdin.lock();
    let mut writer = stdout.lock();
    let mut server = McpServer::default();
    server.serve(&mut reader, &mut writer)
}

fn write_result(
    writer: &mut impl Write,
    id: &Value,
    result: &Value,
) -> io::Result<()> {
    let payload = json!({
        "jsonrpc": "2.0",
        "id": id,
        "result": result,
    });
    serde_json::to_writer(&mut *writer, &payload)
        .map_err(|err| io::Error::new(io::ErrorKind::InvalidData, err))?;
    writer.write_all(b"\n")?;
    writer.flush()
}

fn write_error(
    writer: &mut impl Write,
    id: &Value,
    code: i64,
    message: &str,
) -> io::Result<()> {
    let payload = json!({
        "jsonrpc": "2.0",
        "id": id,
        "error": {
            "code": code,
            "message": message,
        }
    });
    serde_json::to_writer(&mut *writer, &payload)
        .map_err(|err| io::Error::new(io::ErrorKind::InvalidData, err))?;
    writer.write_all(b"\n")?;
    writer.flush()
}

const fn same_scan_options(left: &ScanOptions, right: &ScanOptions) -> bool {
    left.apparent_size == right.apparent_size
        && left.follow_links == right.follow_links
        && left.include_hidden == right.include_hidden
        && left.one_filesystem == right.one_filesystem
        && matches!(
            (left.max_depth, right.max_depth),
            (None, None) | (Some(_), Some(_))
        )
}

fn optional_host(args: &Value) -> Option<String> {
    args.get("ssh_host")
        .and_then(Value::as_str)
        .map(str::trim)
        .filter(|s| !s.is_empty())
        .map(str::to_owned)
}

fn required_str<'a>(args: &'a Value, field: &str) -> Result<&'a str, String> {
    args.get(field)
        .and_then(Value::as_str)
        .map(str::trim)
        .filter(|s| !s.is_empty())
        .ok_or_else(|| format!("Missing or empty required parameter: {field}"))
}

fn required_string_array(
    args: &Value,
    field: &str,
) -> Result<Vec<String>, String> {
    let Some(arr) = args.get(field).and_then(Value::as_array) else {
        return Err(format!("Missing or invalid array parameter: {field}"));
    };
    let mut out = Vec::with_capacity(arr.len());
    for item in arr {
        let Some(text) = item.as_str() else {
            return Err(format!("Every entry in '{field}' must be a string"));
        };
        out.push(text.to_owned());
    }
    Ok(out)
}

fn parse_limit(
    raw: Option<&Value>,
    default_val: usize,
    max_val: usize,
) -> usize {
    raw.and_then(Value::as_u64)
        .map_or(default_val, |v| (v as usize).clamp(1, max_val))
}

fn resolve_local_path(raw: &str) -> Result<PathBuf, String> {
    let path = Path::new(raw);
    let abs = if path.is_absolute() {
        path.to_path_buf()
    } else {
        std::env::current_dir()
            .map_err(|err| err.to_string())?
            .join(path)
    };
    Ok(normalize(&abs))
}

fn resolve_local_scan_root(
    path_arg: Option<&str>,
    disk: bool,
) -> Result<PathBuf, String> {
    let base = match path_arg.map(str::trim).filter(|s| !s.is_empty()) {
        Some(raw) => resolve_local_path(raw)?,
        None => std::env::home_dir()
            .ok_or_else(|| "Could not determine home directory".to_owned())?,
    };
    let root = if disk {
        volume_root_for(&base).unwrap_or(base)
    } else {
        base
    };
    let norm = normalize(&root);
    let meta = fs::metadata(&norm)
        .map_err(|err| format!("{}: {err}", norm.display()))?;
    if !meta.is_dir() {
        return Err(format!("{} is not a directory", norm.display()));
    }
    Ok(norm)
}

fn resolve_subpath<'a>(
    root_path: &Path,
    root_node: &'a Node,
    subpath: Option<&str>,
) -> Result<(Vec<usize>, PathBuf, &'a Node), String> {
    let Some(raw) = subpath.map(str::trim).filter(|s| !s.is_empty()) else {
        return Ok((Vec::new(), root_path.to_path_buf(), root_node));
    };
    let sub = Path::new(raw);
    let rel = if sub.is_absolute() {
        let norm_sub = normalize(sub);
        norm_sub
            .strip_prefix(root_path)
            .map_err(|_| {
                format!(
                    "subpath {} is outside scanned root {}",
                    norm_sub.display(),
                    root_path.display()
                )
            })?
            .to_path_buf()
    } else {
        sub.to_path_buf()
    };
    let mut crumbs = Vec::new();
    let mut current_node = root_node;
    let mut current_path = root_path.to_path_buf();
    for component in rel.components() {
        match component {
            Component::CurDir | Component::RootDir => {}
            Component::ParentDir => {
                return Err("subpath must not contain parent ('..') traversal"
                    .to_owned());
            }
            Component::Prefix(_) => {
                return Err("invalid path prefix in subpath".to_owned());
            }
            Component::Normal(os_name) => {
                let name = os_name.to_string_lossy();
                let Some((idx, child)) = current_node
                    .children
                    .iter()
                    .enumerate()
                    .find(|(_, c)| &*c.name == name.as_ref())
                else {
                    return Err(format!(
                        "entry '{}' not found under {}",
                        name,
                        current_path.display()
                    ));
                };
                crumbs.push(idx);
                current_path.push(&*child.name);
                current_node = child;
            }
        }
    }
    Ok((crumbs, current_path, current_node))
}

fn path_for_crumbs(
    root_path: &Path,
    root_node: &Node,
    crumbs: &[usize],
) -> PathBuf {
    let mut path = root_path.to_path_buf();
    let mut node = root_node;
    for &idx in crumbs {
        let Some(child) = node.children.get(idx) else {
            break;
        };
        path.push(&*child.name);
        node = child;
    }
    path
}

fn build_targets(
    raw_targets: &[String],
    root_path: &Path,
    cached_tree: Option<&Node>,
    remote: bool,
) -> Vec<Target> {
    raw_targets
        .iter()
        .map(|raw| {
            let path = PathBuf::from(raw);
            if let Some(tree) = cached_tree
                && let Ok(rel) = normalize(&path).strip_prefix(root_path)
                && let Some(node) = find_relative_node(tree, rel)
            {
                return Target {
                    path,
                    bytes: node.bytes,
                    is_dir: node.is_dir(),
                    hidden: node.name.starts_with('.'),
                };
            }
            if !remote && let Ok(meta) = fs::symlink_metadata(&path) {
                let hidden = path
                    .file_name()
                    .is_some_and(|n| n.to_string_lossy().starts_with('.'));
                return Target {
                    path,
                    bytes: meta.len(),
                    is_dir: meta.is_dir(),
                    hidden,
                };
            }
            Target {
                path,
                bytes: 0,
                is_dir: false,
                hidden: false,
            }
        })
        .collect()
}

fn find_relative_node<'a>(root: &'a Node, rel: &Path) -> Option<&'a Node> {
    let mut current = root;
    for component in rel.components() {
        match component {
            Component::CurDir => {}
            Component::Normal(name) => {
                let name_str = name.to_str()?;
                current = current.child_named(name_str)?;
            }
            _ => return None,
        }
    }
    Some(current)
}

fn format_scan_summary(
    ssh_host: Option<&str>,
    root_path: &Path,
    entry: &CachedScan,
    cached: bool,
    max_entries: usize,
) -> Value {
    let errors = count_read_errors(&entry.tree);
    let categories = summarize_categories(&entry.tree);
    let children = format_top_children(root_path, &entry.tree, max_entries);
    json!({
        "ssh_host": ssh_host,
        "root": root_path.display().to_string(),
        "platform": entry.platform,
        "cached": cached,
        "scan_count": entry.scan_count,
        "space": entry.space.map(format_space),
        "totals": {
            "bytes": entry.tree.bytes,
            "files": entry.tree.files,
            "dirs": entry.tree.dirs,
            "errors": errors,
            "human_size": human_bytes(entry.tree.bytes),
        },
        "categories": categories,
        "entries": children,
    })
}

fn count_read_errors(node: &Node) -> u64 {
    let own = u64::from(node.read_error);
    node.children
        .iter()
        .fold(own, |acc, child| acc + count_read_errors(child))
}

fn summarize_categories(root: &Node) -> Vec<Value> {
    let mut map: FxHashMap<Category, (u64, u64)> = FxHashMap::default();
    accumulate_categories(root, &mut map);
    let mut list: Vec<(Category, u64, u64)> = map
        .into_iter()
        .filter(|(_, (bytes, files))| *bytes > 0 || *files > 0)
        .map(|(cat, (bytes, files))| (cat, bytes, files))
        .collect();
    list.sort_by_key(|right| std::cmp::Reverse(right.1));
    list.into_iter()
        .map(|(cat, bytes, files)| {
            json!({
                "category": cat.label(),
                "bytes": bytes,
                "files": files,
                "human_size": human_bytes(bytes),
            })
        })
        .collect()
}

fn accumulate_categories(
    node: &Node,
    totals: &mut FxHashMap<Category, (u64, u64)>,
) {
    if !node.is_dir() {
        let entry = totals.entry(node.category).or_insert((0, 0));
        entry.0 = entry.0.saturating_add(node.bytes);
        entry.1 = entry.1.saturating_add(node.files);
        return;
    }
    for child in &node.children {
        accumulate_categories(child, totals);
    }
}

fn format_top_children(
    base_path: &Path,
    node: &Node,
    max_entries: usize,
) -> Vec<Value> {
    node.children
        .iter()
        .take(max_entries)
        .map(|child| {
            let path = base_path.join(&*child.name);
            format_node_entry(&path, child)
        })
        .collect()
}

fn format_node_entry(path: &Path, node: &Node) -> Value {
    json!({
        "name": &*node.name,
        "path": path.display().to_string(),
        "kind": kind_label(node.kind),
        "bytes": node.bytes,
        "files": node.files,
        "dirs": node.dirs,
        "category": node.category.label(),
        "reclaim": node.reclaim.map(Reclaim::label),
        "human_size": human_bytes(node.bytes),
    })
}

const fn kind_label(kind: NodeKind) -> &'static str {
    match kind {
        NodeKind::Directory => "directory",
        NodeKind::File => "file",
        NodeKind::Symlink => "symlink",
        NodeKind::Other => "other",
    }
}

fn format_finding(finding: &Finding) -> Value {
    match finding {
        Finding::Reclaimable(reclaim) => json!({
            "kind": "reclaimable",
            "reason": reclaim.label(),
        }),
        Finding::Worktrees { count, oldest_days } => json!({
            "kind": "worktrees",
            "count": count,
            "oldest_days": oldest_days,
        }),
        Finding::StaleExperiments { count } => json!({
            "kind": "stale_experiments",
            "count": count,
        }),
    }
}

fn format_volume(volume: &crate::space::Volume) -> Value {
    json!({
        "point": volume.point.display().to_string(),
        "device": volume.device,
        "space": volume.space.map(format_space),
    })
}

fn format_space(space: SpaceInfo) -> Value {
    json!({
        "total": space.total,
        "free": space.free,
        "available": space.available,
        "used": space.used(),
        "human_total": human_bytes(space.total),
        "human_free": human_bytes(space.free),
        "human_available": human_bytes(space.available),
    })
}

fn format_targets(targets: &[Target]) -> Vec<Value> {
    targets
        .iter()
        .map(|target| {
            json!({
                "path": target.path.display().to_string(),
                "bytes": target.bytes,
                "human_size": human_bytes(target.bytes),
                "is_dir": target.is_dir,
            })
        })
        .collect()
}

fn format_blocked(blocked: &[Blocked]) -> Vec<Value> {
    blocked
        .iter()
        .map(|item| {
            json!({
                "path": item.path.display().to_string(),
                "reason": item.reason,
            })
        })
        .collect()
}

struct RemovalSummary {
    removed_count: u64,
    removed_bytes: u64,
    failed_count: usize,
    errors: Vec<Value>,
}

fn run_local_removal(plan: Plan, mode: RemovalMode) -> RemovalSummary {
    let handle = spawn(plan, mode);
    let mut errors = Vec::new();
    loop {
        let mut progressed = false;
        while let Some(event) = handle.poll() {
            progressed = true;
            match event {
                RemovalEvent::Start { .. } => {}
                RemovalEvent::Item {
                    path,
                    bytes: _,
                    outcome,
                } => {
                    if let Err(reason) = outcome {
                        let spawn_failed = path.as_os_str().is_empty();
                        errors.push(json!({
                            "path": path.display().to_string(),
                            "error": reason,
                        }));
                        if spawn_failed {
                            return RemovalSummary {
                                removed_count: 0,
                                removed_bytes: 0,
                                failed_count: 1,
                                errors,
                            };
                        }
                    }
                }
                RemovalEvent::Done {
                    removed,
                    bytes,
                    failed,
                } => {
                    return RemovalSummary {
                        removed_count: removed,
                        removed_bytes: bytes,
                        failed_count: failed,
                        errors,
                    };
                }
            }
        }
        if !progressed {
            std::thread::sleep(std::time::Duration::from_millis(1));
        }
    }
}

fn format_plan_rejection(
    action: &str,
    root: &Path,
    blocked: &[Blocked],
) -> String {
    let message = if blocked.is_empty() {
        format!(
            "{action} aborted: no valid targets under {}",
            root.display()
        )
    } else {
        format!(
            "{action} aborted: {} target(s) blocked by safety guards",
            blocked.len()
        )
    };
    serde_json::to_string(&json!({
        "error": message,
        "root": root.display().to_string(),
        "blocked": format_blocked(blocked),
    }))
    .unwrap_or(message)
}

fn format_removal_failure(
    mode: &str,
    root: &Path,
    summary: &RemovalSummary,
    space_before: Option<SpaceInfo>,
    space_after: Option<SpaceInfo>,
) -> String {
    serde_json::to_string(&json!({
        "mode": mode,
        "root": root.display().to_string(),
        "removed_count": summary.removed_count,
        "removed_bytes": summary.removed_bytes,
        "failed_count": summary.failed_count,
        "errors": summary.errors,
        "space_before": space_before.map(format_space),
        "space_after": space_after.map(format_space),
    }))
    .unwrap_or_else(|_| format!("{mode} removal encountered errors"))
}

fn is_known_tool(name: &str) -> bool {
    matches!(
        name,
        "list_volumes"
            | "scan_directory"
            | "inspect_subtree"
            | "find_reclaimable"
            | "check_git_safety"
            | "plan_removal"
            | "trash_paths"
            | "delete_permanently"
    )
}

fn tool_definitions() -> Vec<Value> {
    vec![
        json!({
            "name": "list_volumes",
            "description": "List mounted volumes and free space on the local machine or a remote SSH host.",
            "inputSchema": {
                "type": "object",
                "properties": {
                    "ssh_host": { "type": "string", "description": "Optional SSH destination (e.g. user@host)." }
                }
            }
        }),
        json!({
            "name": "scan_directory",
            "description": "Scan a directory tree (or its containing volume), returning totals, category breakdown, and largest entries.",
            "inputSchema": {
                "type": "object",
                "properties": {
                    "path": { "type": "string", "description": "Directory path to scan (defaults to home directory)." },
                    "disk": { "type": "boolean", "description": "Scan the whole containing volume instead of just path." },
                    "ssh_host": { "type": "string", "description": "Optional SSH destination." },
                    "apparent_size": { "type": "boolean", "description": "Measure apparent file length instead of allocated blocks." },
                    "include_hidden": { "type": "boolean", "description": "Include dotfiles and hidden directories (default true)." },
                    "max_entries": { "type": "integer", "description": "Maximum direct children to return (default 25, max 100)." },
                    "rescan": { "type": "boolean", "description": "Force a fresh scan instead of reusing the session cache." }
                }
            }
        }),
        json!({
            "name": "inspect_subtree",
            "description": "Inspect a subdirectory or search by substring filter within a scanned root.",
            "inputSchema": {
                "type": "object",
                "required": ["root"],
                "properties": {
                    "root": { "type": "string", "description": "Scanned root directory path." },
                    "subpath": { "type": "string", "description": "Optional relative or absolute path inside root." },
                    "filter": { "type": "string", "description": "Optional case-insensitive substring filter." },
                    "ssh_host": { "type": "string", "description": "Optional SSH destination." },
                    "max_entries": { "type": "integer", "description": "Maximum entries to return (default 25, max 100)." }
                }
            }
        }),
        json!({
            "name": "find_reclaimable",
            "description": "Find the largest reclaimable caches, build outputs, worktrees, and stale experiments under a scanned root.",
            "inputSchema": {
                "type": "object",
                "required": ["root"],
                "properties": {
                    "root": { "type": "string", "description": "Scanned root directory path." },
                    "ssh_host": { "type": "string", "description": "Optional SSH destination." },
                    "limit": { "type": "integer", "description": "Maximum candidates to return (default 20, max 100)." }
                }
            }
        }),
        json!({
            "name": "check_git_safety",
            "description": "Check whether local paths are Git checkouts with uncommitted changes, stashes, or unpushed commits.",
            "inputSchema": {
                "type": "object",
                "required": ["paths"],
                "properties": {
                    "paths": {
                        "type": "array",
                        "items": { "type": "string" },
                        "description": "Local filesystem paths to inspect."
                    },
                    "ssh_host": { "type": "string", "description": "Unsupported for git checks; rejected if set." }
                }
            }
        }),
        json!({
            "name": "plan_removal",
            "description": "Validate candidate paths against Ruum's removal safety guards without deleting anything.",
            "inputSchema": {
                "type": "object",
                "required": ["root", "targets"],
                "properties": {
                    "root": { "type": "string", "description": "Scanned root directory path." },
                    "targets": {
                        "type": "array",
                        "items": { "type": "string" },
                        "description": "Target paths under root to evaluate."
                    },
                    "ssh_host": { "type": "string", "description": "Optional SSH destination." }
                }
            }
        }),
        json!({
            "name": "trash_paths",
            "description": "Move validated paths under a scanned root to the system Trash (local or guarded remote SSH).",
            "inputSchema": {
                "type": "object",
                "required": ["root", "targets"],
                "properties": {
                    "root": { "type": "string", "description": "Scanned root directory path." },
                    "targets": {
                        "type": "array",
                        "items": { "type": "string" },
                        "description": "Target paths under root to move to Trash."
                    },
                    "ssh_host": { "type": "string", "description": "Optional SSH destination." }
                }
            }
        }),
        json!({
            "name": "delete_permanently",
            "description": "Permanently delete local paths under a scanned root after out-of-band OS native authentication.",
            "inputSchema": {
                "type": "object",
                "required": ["root", "targets"],
                "properties": {
                    "root": { "type": "string", "description": "Local scanned root directory path." },
                    "targets": {
                        "type": "array",
                        "items": { "type": "string" },
                        "description": "Target paths under root to permanently delete."
                    },
                    "reason": { "type": "string", "description": "Reason for permanent deletion shown in the OS authentication prompt (defaults to '清理回收站')." },
                    "ssh_host": { "type": "string", "description": "Forbidden for permanent deletion." }
                }
            }
        }),
    ]
}

#[cfg(test)]
mod tests {
    use std::io::Cursor;
    use std::process::Command;

    use super::*;
    use crate::auth::AuthOs;
    use crate::classify::classify;
    use crate::tree::{Metric, aggregate};

    struct DenyAuth;

    impl AuthAdapter for DenyAuth {
        fn os_kind(&self) -> AuthOs {
            AuthOs::Linux
        }

        fn authenticate(&self, _request: &AuthRequest) -> io::Result<bool> {
            Ok(false)
        }
    }

    fn run_session(
        server: &mut McpServer,
        requests: &[Value],
    ) -> io::Result<Vec<Value>> {
        let mut input = String::new();
        for req in requests {
            input.push_str(&serde_json::to_string(req).expect("serialize req"));
            input.push('\n');
        }
        let mut reader = Cursor::new(input.into_bytes());
        let mut output = Vec::new();
        server.serve(&mut reader, &mut output)?;
        let text = String::from_utf8(output).expect("utf8 output");
        Ok(text
            .lines()
            .map(|line| serde_json::from_str(line).expect("valid json line"))
            .collect())
    }

    fn parse_tool_payload(response: &Value) -> (bool, Value) {
        let result = &response["result"];
        let is_error = result["isError"].as_bool().unwrap_or(false);
        let text = result["content"][0]["text"]
            .as_str()
            .expect("tool content text");
        let parsed = serde_json::from_str(text).unwrap_or_else(|_| json!(text));
        (is_error, parsed)
    }

    #[test]
    fn lifecycle_initialize_notifications_ping_tools_list_and_eof() {
        let mut server = McpServer::new(Box::new(DenyAuth));
        let responses = run_session(
            &mut server,
            &[
                json!({
                    "jsonrpc": "2.0",
                    "id": 1,
                    "method": "initialize",
                    "params": { "protocolVersion": "2024-11-05" }
                }),
                json!({
                    "jsonrpc": "2.0",
                    "method": "notifications/initialized"
                }),
                json!({
                    "jsonrpc": "2.0",
                    "id": 2,
                    "method": "ping"
                }),
                json!({
                    "jsonrpc": "2.0",
                    "id": 3,
                    "method": "tools/list"
                }),
                json!({
                    "jsonrpc": "2.0",
                    "id": 4,
                    "method": "tools/call",
                    "params": { "name": "nonexistent_tool", "arguments": {} }
                }),
            ],
        )
        .expect("serve succeeds");

        // Notification produces no frame, so 4 responses for 5 messages.
        assert_eq!(responses.len(), 4);
        assert_eq!(responses[0]["id"], 1);
        assert_eq!(responses[0]["result"]["protocolVersion"], PROTOCOL_VERSION);
        assert_eq!(responses[0]["result"]["serverInfo"]["name"], "ruum");

        assert_eq!(responses[1]["id"], 2);
        assert_eq!(responses[1]["result"], json!({}));

        assert_eq!(responses[2]["id"], 3);
        let tools = responses[2]["result"]["tools"]
            .as_array()
            .expect("tools array");
        assert_eq!(tools.len(), 8);

        assert_eq!(responses[3]["id"], 4);
        assert_eq!(responses[3]["error"]["code"], -32601);
    }

    #[test]
    fn scan_cache_reuse_rescan_and_subtree_inspection() {
        let dir = tempfile::tempdir().expect("tempdir");
        let root = dir.path();
        fs::create_dir_all(root.join(".cache/pip")).expect("mkdir .cache");
        fs::create_dir_all(root.join("src/app")).expect("mkdir src/app");
        fs::write(root.join(".cache/pip/wheel.bin"), vec![b'a'; 8192])
            .expect("write wheel");
        fs::write(root.join("src/app/main.rs"), vec![b'b'; 2048])
            .expect("write main.rs");

        let root_str = root.to_str().expect("utf8 root");
        let mut server = McpServer::new(Box::new(DenyAuth));

        let responses = run_session(
            &mut server,
            &[
                json!({
                    "jsonrpc": "2.0",
                    "id": 1,
                    "method": "tools/call",
                    "params": {
                        "name": "scan_directory",
                        "arguments": { "path": root_str, "apparent_size": true }
                    }
                }),
                json!({
                    "jsonrpc": "2.0",
                    "id": 2,
                    "method": "tools/call",
                    "params": {
                        "name": "scan_directory",
                        "arguments": { "path": root_str, "apparent_size": true }
                    }
                }),
                json!({
                    "jsonrpc": "2.0",
                    "id": 3,
                    "method": "tools/call",
                    "params": {
                        "name": "scan_directory",
                        "arguments": {
                            "path": root_str,
                            "apparent_size": true,
                            "rescan": true
                        }
                    }
                }),
                json!({
                    "jsonrpc": "2.0",
                    "id": 4,
                    "method": "tools/call",
                    "params": {
                        "name": "inspect_subtree",
                        "arguments": { "root": root_str, "subpath": "src/app" }
                    }
                }),
                json!({
                    "jsonrpc": "2.0",
                    "id": 5,
                    "method": "tools/call",
                    "params": {
                        "name": "inspect_subtree",
                        "arguments": { "root": root_str, "filter": "wheel" }
                    }
                }),
            ],
        )
        .expect("serve succeeds");

        let (err1, scan1) = parse_tool_payload(&responses[0]);
        assert!(!err1);
        assert_eq!(scan1["cached"], false);
        assert_eq!(scan1["scan_count"], 1);
        assert_eq!(scan1["totals"]["bytes"], 8192 + 2048);

        let (err2, scan2) = parse_tool_payload(&responses[1]);
        assert!(!err2);
        assert_eq!(scan2["cached"], true);
        assert_eq!(scan2["scan_count"], 1);

        let (err3, scan3) = parse_tool_payload(&responses[2]);
        assert!(!err3);
        assert_eq!(scan3["cached"], false);
        assert_eq!(scan3["scan_count"], 2);

        let (err4, sub) = parse_tool_payload(&responses[3]);
        assert!(!err4);
        assert_eq!(sub["entries"][0]["name"], "main.rs");
        assert_eq!(sub["entries"][0]["bytes"], 2048);

        let (err5, filtered) = parse_tool_payload(&responses[4]);
        assert!(!err5);
        assert_eq!(filtered["match_count"], 1);
        assert_eq!(filtered["entries"][0]["name"], "wheel.bin");
    }

    #[test]
    fn find_reclaimable_and_overlap_invalidation() {
        let mut root = Node::directory("root");
        let mut cache_dir = Node::directory(".cache");
        cache_dir.children.push(Node::entry(
            "blob.bin",
            NodeKind::File,
            128 * 1024 * 1024,
        ));
        root.children.push(cache_dir);
        aggregate(&mut root, Metric::Bytes);
        classify(&mut root);

        let mut server = McpServer::new(Box::new(DenyAuth));
        let root_path = PathBuf::from("/tmp/synth-work");
        server.insert_cached(
            None,
            root_path,
            CachedScan {
                tree: root,
                space: None,
                home: None,
                platform: "linux".to_owned(),
                identity: None,
                options: ScanOptions::default(),
                scan_count: 1,
            },
        );

        let responses = run_session(
            &mut server,
            &[json!({
                "jsonrpc": "2.0",
                "id": 1,
                "method": "tools/call",
                "params": {
                    "name": "find_reclaimable",
                    "arguments": { "root": "/tmp/synth-work" }
                }
            })],
        )
        .expect("serve succeeds");

        let (is_err, payload) = parse_tool_payload(&responses[0]);
        assert!(!is_err);
        let candidates = payload["candidates"].as_array().expect("candidates");
        assert_eq!(candidates.len(), 1);
        assert_eq!(candidates[0]["path"], "/tmp/synth-work/.cache");
        assert_eq!(candidates[0]["finding"]["kind"], "reclaimable");

        server
            .invalidate_overlapping(None, Path::new("/tmp/synth-work/.cache"));
        assert!(server.cache().is_empty());
    }

    #[test]
    fn check_git_safety_detects_dirty_checkout_and_rejects_ssh_host() {
        let dir = tempfile::tempdir().expect("tempdir");
        let status = Command::new("git")
            .arg("init")
            .arg(dir.path())
            .output()
            .expect("git init");
        assert!(status.status.success());
        fs::write(dir.path().join("dirty.txt"), b"uncommitted").expect("write");

        let mut server = McpServer::new(Box::new(DenyAuth));
        let path_str = dir.path().to_str().expect("utf8");
        let responses = run_session(
            &mut server,
            &[
                json!({
                    "jsonrpc": "2.0",
                    "id": 1,
                    "method": "tools/call",
                    "params": {
                        "name": "check_git_safety",
                        "arguments": { "paths": [path_str] }
                    }
                }),
                json!({
                    "jsonrpc": "2.0",
                    "id": 2,
                    "method": "tools/call",
                    "params": {
                        "name": "check_git_safety",
                        "arguments": {
                            "paths": [path_str],
                            "ssh_host": "user@remote"
                        }
                    }
                }),
            ],
        )
        .expect("serve succeeds");

        let (err1, res1) = parse_tool_payload(&responses[0]);
        assert!(!err1);
        assert_eq!(res1["results"][0]["is_checkout"], true);
        assert_eq!(res1["results"][0]["is_clean"], false);
        assert_eq!(res1["results"][0]["changed"], 1);

        let (err2, _res2) = parse_tool_payload(&responses[1]);
        assert!(err2, "ssh_host must be rejected for check_git_safety");
    }

    #[test]
    fn plan_removal_local_and_cached_remote_enforce_guards() {
        let dir = tempfile::tempdir().expect("tempdir");
        let root = dir.path();
        let cache_dir = root.join("cache");
        let nested = cache_dir.join("nested");
        fs::create_dir_all(&nested).expect("mkdir nested");
        fs::write(nested.join("item.bin"), vec![0_u8; 4096]).expect("write");

        let mut server = McpServer::new(Box::new(DenyAuth));
        let root_str = root.to_str().expect("utf8");

        // Seed a remote scan entry for remote plan_removal testing.
        let mut remote_tree = Node::directory("me");
        remote_tree.children.push(Node::entry(
            "cache",
            NodeKind::Directory,
            2048,
        ));
        aggregate(&mut remote_tree, Metric::Bytes);
        server.insert_cached(
            Some("box".to_owned()),
            PathBuf::from("/home/me"),
            CachedScan {
                tree: remote_tree,
                space: None,
                home: Some(PathBuf::from("/home/me")),
                platform: "linux".to_owned(),
                identity: Some(HostIdentity {
                    hostname: "box".to_owned(),
                    machine_id: Some(
                        "0123456789abcdef0123456789abcdef".to_owned(),
                    ),
                }),
                options: ScanOptions::default(),
                scan_count: 1,
            },
        );

        let responses = run_session(
            &mut server,
            &[
                json!({
                    "jsonrpc": "2.0",
                    "id": 1,
                    "method": "tools/call",
                    "params": {
                        "name": "plan_removal",
                        "arguments": {
                            "root": root_str,
                            "targets": [
                                cache_dir.to_str().expect("utf8"),
                                nested.to_str().expect("utf8"),
                                root_str,
                                "/usr"
                            ]
                        }
                    }
                }),
                json!({
                    "jsonrpc": "2.0",
                    "id": 2,
                    "method": "tools/call",
                    "params": {
                        "name": "plan_removal",
                        "arguments": {
                            "root": "/home/me",
                            "ssh_host": "box",
                            "targets": ["/home/me/cache", "/home/me", "/etc"]
                        }
                    }
                }),
            ],
        )
        .expect("serve succeeds");

        let (err1, local_plan) = parse_tool_payload(&responses[0]);
        assert!(!err1);
        assert_eq!(local_plan["targets"].as_array().expect("targets").len(), 1);
        assert_eq!(local_plan["covered"].as_array().expect("covered").len(), 1);
        assert_eq!(local_plan["blocked"].as_array().expect("blocked").len(), 2);

        let (err2, rem_plan) = parse_tool_payload(&responses[1]);
        assert!(!err2);
        assert_eq!(rem_plan["targets"].as_array().expect("targets").len(), 1);
        assert_eq!(rem_plan["blocked"].as_array().expect("blocked").len(), 2);
        assert_eq!(rem_plan["allowed_modes"], json!(["trash"]));
    }

    struct CountingAuth {
        calls: std::sync::Arc<std::sync::atomic::AtomicUsize>,
        last_reason: std::sync::Arc<std::sync::Mutex<Option<String>>>,
        decision: Result<bool, &'static str>,
    }

    impl AuthAdapter for CountingAuth {
        fn os_kind(&self) -> AuthOs {
            AuthOs::Linux
        }

        fn authenticate(&self, request: &AuthRequest) -> io::Result<bool> {
            self.calls.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
            *self.last_reason.lock().expect("lock reason") =
                Some(request.reason.clone());
            self.decision.map_err(io::Error::other)
        }
    }

    #[test]
    fn delete_permanently_rejects_ssh_and_blocked_without_calling_auth() {
        let dir = tempfile::tempdir().expect("tempdir");
        let root = dir.path();
        let valid_target = root.join("build");
        fs::create_dir_all(&valid_target).expect("mkdir build");
        fs::write(valid_target.join("artifact.o"), b"bytes").expect("write");

        let calls = std::sync::Arc::new(std::sync::atomic::AtomicUsize::new(0));
        let mut server = McpServer::new(Box::new(CountingAuth {
            calls: std::sync::Arc::clone(&calls),
            last_reason: std::sync::Arc::new(std::sync::Mutex::new(None)),
            decision: Ok(true),
        }));
        let root_str = root.to_str().expect("utf8");
        let valid_str = valid_target.to_str().expect("utf8");

        let responses = run_session(
            &mut server,
            &[
                json!({
                    "jsonrpc": "2.0",
                    "id": 1,
                    "method": "tools/call",
                    "params": {
                        "name": "delete_permanently",
                        "arguments": {
                            "root": root_str,
                            "targets": [valid_str],
                            "ssh_host": "user@remote"
                        }
                    }
                }),
                json!({
                    "jsonrpc": "2.0",
                    "id": 2,
                    "method": "tools/call",
                    "params": {
                        "name": "delete_permanently",
                        "arguments": {
                            "root": root_str,
                            "targets": [valid_str, root_str]
                        }
                    }
                }),
            ],
        )
        .expect("serve succeeds");

        let (err1, msg1) = parse_tool_payload(&responses[0]);
        assert!(err1, "remote permanent delete must fail");
        assert!(
            msg1.as_str()
                .unwrap_or_default()
                .contains("Permanent deletion over SSH is forbidden")
        );

        let (err2, msg2) = parse_tool_payload(&responses[1]);
        assert!(err2, "batch with blocked target must abort");
        assert_eq!(msg2["blocked"].as_array().expect("blocked").len(), 1);
        assert_eq!(
            calls.load(std::sync::atomic::Ordering::SeqCst),
            0,
            "auth must not be invoked when ssh_host is set or targets are blocked"
        );
        assert!(
            valid_target.exists(),
            "sibling target in aborted batch must stay untouched"
        );
    }

    #[test]
    fn delete_permanently_fails_closed_on_deny_or_error_and_deletes_on_approve()
    {
        let dir = tempfile::tempdir().expect("tempdir");
        let root = dir.path();
        let doomed = root.join("cache-dir");
        let keep = root.join("keep-dir");
        fs::create_dir_all(&doomed).expect("mkdir doomed");
        fs::create_dir_all(&keep).expect("mkdir keep");
        fs::write(doomed.join("temp.bin"), vec![b'x'; 1024]).expect("write");
        fs::write(keep.join(" precious.txt"), b"keep me").expect("write");

        let root_str = root.to_str().expect("utf8");
        let doomed_str = doomed.to_str().expect("utf8");

        // 1. User cancels OS auth prompt (Ok(false)).
        let deny_calls =
            std::sync::Arc::new(std::sync::atomic::AtomicUsize::new(0));
        let deny_reason = std::sync::Arc::new(std::sync::Mutex::new(None));
        let mut deny_server = McpServer::new(Box::new(CountingAuth {
            calls: std::sync::Arc::clone(&deny_calls),
            last_reason: std::sync::Arc::clone(&deny_reason),
            decision: Ok(false),
        }));
        let resp_deny = run_session(
            &mut deny_server,
            &[json!({
                "jsonrpc": "2.0",
                "id": 1,
                "method": "tools/call",
                "params": {
                    "name": "delete_permanently",
                    "arguments": { "root": root_str, "targets": [doomed_str] }
                }
            })],
        )
        .expect("serve succeeds");
        let (is_err_deny, _) = parse_tool_payload(&resp_deny[0]);
        assert!(is_err_deny);
        assert_eq!(deny_calls.load(std::sync::atomic::Ordering::SeqCst), 1);
        assert!(
            deny_reason
                .lock()
                .expect("lock")
                .as_deref()
                .unwrap_or_default()
                .starts_with("清理回收站 — Permanently delete"),
            "default reason must start with 清理回收站"
        );
        assert!(doomed.exists(), "denied auth must leave target untouched");

        // 2. OS auth prompt errors (Err(...)).
        let err_calls =
            std::sync::Arc::new(std::sync::atomic::AtomicUsize::new(0));
        let mut err_server = McpServer::new(Box::new(CountingAuth {
            calls: std::sync::Arc::clone(&err_calls),
            last_reason: std::sync::Arc::new(std::sync::Mutex::new(None)),
            decision: Err("no display server available"),
        }));
        let resp_err = run_session(
            &mut err_server,
            &[json!({
                "jsonrpc": "2.0",
                "id": 1,
                "method": "tools/call",
                "params": {
                    "name": "delete_permanently",
                    "arguments": { "root": root_str, "targets": [doomed_str] }
                }
            })],
        )
        .expect("serve succeeds");
        let (is_err_auth, _) = parse_tool_payload(&resp_err[0]);
        assert!(is_err_auth);
        assert_eq!(err_calls.load(std::sync::atomic::Ordering::SeqCst), 1);
        assert!(doomed.exists(), "errored auth must fail closed");

        // 3. OS auth prompt approves (Ok(true)) with explicit caller reason.
        let ok_calls =
            std::sync::Arc::new(std::sync::atomic::AtomicUsize::new(0));
        let ok_reason = std::sync::Arc::new(std::sync::Mutex::new(None));
        let mut ok_server = McpServer::new(Box::new(CountingAuth {
            calls: std::sync::Arc::clone(&ok_calls),
            last_reason: std::sync::Arc::clone(&ok_reason),
            decision: Ok(true),
        }));
        let resp_ok = run_session(
            &mut ok_server,
            &[
                json!({
                    "jsonrpc": "2.0",
                    "id": 1,
                    "method": "tools/call",
                    "params": {
                        "name": "scan_directory",
                        "arguments": { "path": root_str }
                    }
                }),
                json!({
                    "jsonrpc": "2.0",
                    "id": 2,
                    "method": "tools/call",
                    "params": {
                        "name": "delete_permanently",
                        "arguments": {
                            "root": root_str,
                            "targets": [doomed_str],
                            "reason": "清理回收站"
                        }
                    }
                }),
            ],
        )
        .expect("serve succeeds");
        let (is_err_scan, _) = parse_tool_payload(&resp_ok[0]);
        assert!(!is_err_scan);
        let (is_err_del, del_payload) = parse_tool_payload(&resp_ok[1]);
        assert!(
            !is_err_del,
            "approved deletion should succeed: {del_payload}"
        );
        assert_eq!(ok_calls.load(std::sync::atomic::Ordering::SeqCst), 1);
        assert!(
            ok_reason
                .lock()
                .expect("lock")
                .as_deref()
                .unwrap_or_default()
                .starts_with("清理回收站 — Permanently delete")
        );
        assert_eq!(del_payload["mode"], "permanent");
        assert_eq!(del_payload["removed_count"], 1);
        assert!(!doomed.exists(), "approved target must be removed");
        assert!(keep.exists(), "unmarked sibling must remain intact");
        assert!(
            ok_server.cache().is_empty(),
            "overlapping session cache entry must be invalidated after delete"
        );
    }

    #[test]
    fn trash_paths_aborts_when_blocked_or_remote_host_uncached() {
        let dir = tempfile::tempdir().expect("tempdir");
        let root = dir.path();
        let valid_item = root.join("logs");
        fs::create_dir_all(&valid_item).expect("mkdir logs");
        fs::write(valid_item.join("app.log"), b"log").expect("write log");

        let mut server = McpServer::new(Box::new(DenyAuth));
        let root_str = root.to_str().expect("utf8");
        let valid_str = valid_item.to_str().expect("utf8");

        let responses = run_session(
            &mut server,
            &[
                json!({
                    "jsonrpc": "2.0",
                    "id": 1,
                    "method": "tools/call",
                    "params": {
                        "name": "trash_paths",
                        "arguments": {
                            "root": root_str,
                            "targets": [valid_str, root_str]
                        }
                    }
                }),
                json!({
                    "jsonrpc": "2.0",
                    "id": 2,
                    "method": "tools/call",
                    "params": {
                        "name": "trash_paths",
                        "arguments": {
                            "root": "/home/me",
                            "targets": ["/home/me/cache"],
                            "ssh_host": "uncached-box"
                        }
                    }
                }),
            ],
        )
        .expect("serve succeeds");

        let (err1, payload1) = parse_tool_payload(&responses[0]);
        assert!(err1, "trash_paths must abort when any target is blocked");
        assert_eq!(payload1["blocked"].as_array().expect("blocked").len(), 1);
        assert!(
            valid_item.exists(),
            "valid target must not be trashed when batch has a blocked target"
        );

        let (err2, payload2) = parse_tool_payload(&responses[1]);
        assert!(err2, "trash_paths on uncached remote host must fail");
        assert!(
            payload2
                .as_str()
                .unwrap_or_default()
                .contains("No cached remote scan")
        );
    }
}
