//! Application state and every mutation the UI can perform.
//!
//! The screens in [`crate::views`] are pure functions of this state; all the
//! decisions — what is selected, what a mark means, what a key does, when to
//! re-scan — live here so they can be reasoned about in one place.

use std::cell::Cell;
use std::path::{Path, PathBuf};
use std::rc::Rc;
use std::sync::{
    Arc, Mutex,
    atomic::{AtomicBool, AtomicU64, Ordering},
};
use std::time::{Duration, Instant};

use gpui_kit::base::input::{InputEvent, InputState};
use gpui_kit::{
    App, AppContext as _, Context, Entity, FocusHandle, Focusable as _,
    KeyDownEvent, MouseButton, MouseDownEvent, MouseMoveEvent,
    NavigationDirection, Pixels, Point, Render, ScrollDelta, ScrollWheelEvent,
    Size, Window, px, size,
};
use gpui_omarchy::Status;
use rustc_hash::{FxHashMap, FxHashSet};
use ruum_core::access::file_table_readable;
use ruum_core::cli_install::{
    CliInstallEnv, CliInstallFactory, CliInstallStatus,
};
use ruum_core::cloud_client::{
    BillingPlansSnapshot, ChargingOption, CloudApiError, CloudClient,
    CloudVirtualModel, EntitlementSnapshot, PurchaseOrderSnapshot,
    normalize_redemption_code_input,
};
use ruum_core::confirm::{
    CleanupConfirmDecision, CleanupConfirmItem, CleanupConfirmRequest,
    DockerConfirmItem, RemoteConnectConfirmDecision,
    RemoteConnectConfirmRequest,
};
use ruum_core::credentials::LazyCredentialStore;
use ruum_core::docker_adapter::{
    DockerCleanOutcome, DockerCleanTarget, DockerEnv, DockerFactory,
    DockerProbeStatus, validate_targets_against_summary,
};
use ruum_core::export::{
    RemoteCleanupScriptSpec, build_remote_cleanup_script,
    write_cleanup_script_atomic,
};
use ruum_core::filter::{Keep, Matches, filter};
use ruum_core::insights::{Candidate, worth_a_look};
use ruum_core::removal::{
    Plan, RemovalEvent, RemovalHandle, RemovalMode, Target, TrashBackend,
    detect_trash_backend, plan_for_mode,
};
use ruum_core::scan::{Known, ScanHandle, ScanOptions, ScanSnapshot};
use ruum_core::space::{SpaceInfo, device_for, space_info, volume_root_for};
use ruum_core::ssh_session::SshSession;
use ruum_core::storage::{
    AiConfig, AppSettings, AuthMode, CloudState, SshProfile, StorageDb,
    UpdateChannel, UpdateSettings, WebhookConfig, WebhookEndpoint,
};
use ruum_core::terminal_launcher::{TerminalEnv, TerminalFactory};
#[cfg(test)]
use ruum_core::terminal_launcher::{TerminalLaunchPlan, TerminalOs};
use ruum_core::tree::{Metric, Node, path_of};
use ruum_core::treemap::{
    LayoutOptions, Rect, Tile, TileKind, hit, layout_filtered,
};
use ruum_core::updater::{
    ApplyOutcome, DEFAULT_GITHUB_REPO, InstallKind, UpdateArch,
    UpdateCheckRequest, UpdateCheckResult, UpdateEnv, UpdateFactory,
    check_for_updates, download_and_verify_asset,
    resolve_effective_public_key_for_url, resolve_update_base_url,
    validate_update_url,
};
use ruum_core::watchdog::ServiceStatus;
use ruum_core::watchdog_config::{AiCliProvider, ApiProtocol, CleanupProposal};
use ruum_watchdog::{MIN_COOLDOWN_SECS, WatchdogConfig, WatchdogState};
use std::collections::{BTreeSet, HashSet, VecDeque};

use crate::git::GitState;

use crate::marks::{Marks, display_path, is_hidden};
use crate::treemap_view::{Mosaic, TileDeco};

/// What a tile's colour says.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum ColorMode {
    /// The kind of data it is.
    #[default]
    Kind,
    /// How long since anything in it was written.
    Age,
}

/// A trail crumb's sibling menu, open.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct CrumbMenu {
    /// Crumbs of the directory whose children are listed.
    pub parent: Vec<usize>,
    /// The listed child the crumb stands for.
    pub current: usize,
    /// The row the arrow keys are on, as an index into [`Ruum::siblings`].
    pub highlighted: usize,
}

/// One row of a sibling menu.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Sibling {
    /// Index among the parent's children.
    pub index: usize,
    pub name: String,
    pub value: u64,
    pub category: ruum_core::classify::Category,
    pub is_dir: bool,
}

/// Rows a sibling menu lists; the rest are counted.
pub const SIBLING_ROWS: usize = 24;

/// One step of the trail.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Crumb {
    /// A directory above the scanned root: going there scans only what is
    /// new at that level.
    Above(PathBuf),
    /// A directory in the tree, by crumbs from the scanned root.
    Tree(Vec<usize>),
}

/// The side panel's width in rem: default and limits. In rem, so interface
/// zoom scales it with everything it holds.
pub const PANEL_REMS: f32 = 23.0;
pub const PANEL_MIN_REMS: f32 = 17.0;
pub const PANEL_MAX_REMS: f32 = 44.0;

/// The panel width, in rem, for its left edge at `pointer_x` in a window
/// `viewport` pixels wide: between its limits, and never so wide that the
/// mosaic is squeezed below the panel's own minimum.
pub fn panel_width(pointer_x: f32, viewport: f32, rem: f32) -> f32 {
    let width = (viewport - pointer_x) / rem;
    let max =
        (viewport / rem - PANEL_MIN_REMS).clamp(PANEL_MIN_REMS, PANEL_MAX_REMS);
    width.clamp(PANEL_MIN_REMS, max)
}

/// How many "worth a look" findings the panel lists.
const INSIGHT_LIMIT: usize = 6;

/// Which screen the app is showing.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum Screen {
    /// Walk the treemap and mark what should go.
    #[default]
    Explore,
    /// Dedicated Docker workspace for inspecting and cleaning images,
    /// containers, and build cache.
    Docker,
    /// Manage saved remote SSH server configurations.
    Remote,
    /// Volume free space monitor and AI cleanup proposals.
    Watchdog,
    /// Application preferences and watchdog configuration.
    Settings,
    /// Review every marked path and choose how to remove them.
    Review,
    /// A removal is running.
    Running,
    /// The removal finished; show what happened.
    Done,
}

/// The treemap view transform: `screen = (base - origin) * scale`.
///
/// All viewport geometry stays in the base space of an unzoomed layout, so pan
/// and zoom never require a re-layout.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct View {
    pub scale: f32,
    pub origin_x: f32,
    pub origin_y: f32,
}

impl Default for View {
    fn default() -> Self {
        Self::IDENTITY
    }
}

impl View {
    pub const IDENTITY: Self = Self {
        scale: 1.0,
        origin_x: 0.0,
        origin_y: 0.0,
    };
    pub const MIN_SCALE: f32 = 1.0;
    /// Past this, zooming again descends into whatever is under the cursor
    /// instead of magnifying further: the continuous "keep zooming and you are
    /// inside" gesture, with a breadcrumb to walk back out.
    pub const MAX_SCALE: f32 = 5.0;

    pub fn project(&self, rect: Rect) -> Rect {
        Rect::new(
            (rect.x - self.origin_x) * self.scale,
            (rect.y - self.origin_y) * self.scale,
            rect.w * self.scale,
            rect.h * self.scale,
        )
    }

    /// Viewport point to base-space point.
    pub fn unproject(&self, x: f32, y: f32) -> (f32, f32) {
        (
            x / self.scale + self.origin_x,
            y / self.scale + self.origin_y,
        )
    }

    /// Zoom by `factor` toward `(x, y)` and no further than `ceiling`.
    ///
    /// The base point under `(x, y)` stays put, which is what makes a wheel
    /// zoom feel pointed at the tile rather than at the window.
    pub fn zoomed_at(&self, x: f32, y: f32, factor: f32, ceiling: f32) -> Self {
        let scale = (self.scale * factor).clamp(Self::MIN_SCALE, ceiling);
        let (base_x, base_y) = self.unproject(x, y);
        Self {
            scale,
            origin_x: base_x - x / scale,
            origin_y: base_y - y / scale,
        }
    }

    /// The scale at which `rect` exactly fits the viewport.
    pub fn fit_scale(rect: Rect, area: Size<Pixels>) -> f32 {
        let width = area.width.as_f32().max(1.0);
        let height = area.height.as_f32().max(1.0);
        if rect.w <= 0.0 || rect.h <= 0.0 {
            return Self::MIN_SCALE;
        }
        (width / rect.w).min(height / rect.h)
    }

    /// The region of the base layout the viewport currently shows.
    pub fn visible_base(&self, area: Size<Pixels>) -> Rect {
        let width = area.width.as_f32().max(1.0);
        let height = area.height.as_f32().max(1.0);
        Rect::new(
            self.origin_x,
            self.origin_y,
            width / self.scale,
            height / self.scale,
        )
    }

    /// Keep the viewport inside the layout: no empty margins, ever.
    pub fn clamped(self, area: Size<Pixels>) -> Self {
        let width = area.width.as_f32();
        let height = area.height.as_f32();
        let max_x = (width - width / self.scale).max(0.0);
        let max_y = (height - height / self.scale).max(0.0);
        Self {
            scale: self.scale,
            origin_x: self.origin_x.clamp(0.0, max_x),
            origin_y: self.origin_y.clamp(0.0, max_y),
        }
    }
}

/// A layout transition: the region the user is looking at, and where that same
/// region lands in the layout being switched to.
///
/// Every descending or ascending move is one region of the tree changing
/// address. Rather than moving a camera, each tile is drawn from where that
/// region *was* to where it *is*, so the tiles inside the directory that is
/// being entered grow into place and the ones being left slide out. That is
/// what makes "zoom in and descend" read as one motion: the newly visible
/// level is always larger than it was a moment ago, never smaller.
#[derive(Clone, Copy, Debug)]
pub struct LayoutTransition {
    /// The region in the old frame, in viewport coordinates.
    src: Rect,
    /// The same region in the new frame, in viewport coordinates.
    dst: Rect,
    started: Instant,
    duration: Duration,
}

impl LayoutTransition {
    fn new(src: Rect, dst: Rect) -> Self {
        // Longer pulls take a little longer, but never enough to feel slow.
        let ratio = if dst.w > 1.0 { src.w / dst.w } else { 1.0 };
        let magnitude = ratio.abs().max(1.0).log2().clamp(0.0, 4.0);
        Self {
            src,
            dst,
            started: Instant::now(),
            duration: Duration::from_millis(130 + (magnitude * 45.0) as u64),
        }
    }

    /// Where a rectangle in the new layout was, before the switch.
    fn origin_of(&self, rect: Rect) -> Rect {
        let scale = if self.dst.w > 0.0 {
            self.src.w / self.dst.w
        } else {
            1.0
        };
        Rect::new(
            (rect.x - self.dst.x).mul_add(scale, self.src.x),
            (rect.y - self.dst.y).mul_add(scale, self.src.y),
            rect.w * scale,
            rect.h * scale,
        )
    }

    /// The rectangle to draw at this instant, and whether the transition is
    /// still running.
    fn sample(&self, rect: Rect) -> (Rect, bool) {
        let elapsed =
            self.started.elapsed().as_secs_f32() / self.duration.as_secs_f32();
        if elapsed >= 1.0 {
            return (rect, false);
        }
        let eased = 1.0 - (1.0 - elapsed).powi(3);
        let from = self.origin_of(rect);
        let lerp = |a: f32, b: f32| (b - a).mul_add(eased, a);
        (
            Rect::new(
                lerp(from.x, rect.x),
                lerp(from.y, rect.y),
                lerp(from.w, rect.w),
                lerp(from.h, rect.h),
            ),
            true,
        )
    }
}

/// Layout for one (crumbs, area, options) combination.
struct LayoutCache {
    key: LayoutKey,
    tiles: Vec<Tile>,
}

#[derive(Clone, PartialEq)]
struct LayoutKey {
    crumbs: Vec<usize>,
    width: f32,
    height: f32,
    options: LayoutOptions,
    /// Bumped whenever the applied filter changes.
    filter: u64,
}

/// Directories left behind and come back from, for `<` and `>`.
///
/// Kept as absolute paths, not crumbs: a re-scan or a widening renumbers the
/// tree, and crumbs would then silently point at other directories.
#[derive(Clone, Debug, Default)]
pub struct History {
    pub back: Vec<PathBuf>,
    pub forward: Vec<PathBuf>,
}

/// How many directories back is remembered. Far more than anyone clicks
/// through, and small enough that pruning after a scan costs nothing.
const HISTORY_DEPTH: usize = 100;

/// Result of the removal run, summarised for the final screen.
#[derive(Clone, Debug, Default)]
pub struct RunSummary {
    pub total: usize,
    pub removed: u64,
    pub bytes: u64,
    pub failed: usize,
}

/// Category grouping for Docker cleanup checkboxes.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum DockerGroupKind {
    BuildCache,
    Images,
    Containers,
}

/// Interactive dialog state shown after generating a deterministic remote
/// cleanup script.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct GeneratedScriptDialog {
    pub profile_id: String,
    pub alias: String,
    pub script_path: PathBuf,
    pub item_count: usize,
    pub reclaim_bytes: u64,
}

/// Interactive modal state holding proposed items for user review and confirmation.
#[derive(Debug)]
pub struct CleanupConfirmModal {
    pub request: CleanupConfirmRequest,
    pub checked_paths: BTreeSet<PathBuf>,
    pub checked_docker_targets: HashSet<DockerCleanTarget>,
    pub responder: Option<std::sync::mpsc::Sender<CleanupConfirmDecision>>,
    pub response_file: Option<PathBuf>,
    pub remote_profile_id: Option<String>,
    pub decided: bool,
}

impl CleanupConfirmModal {
    /// Create a new modal with candidate paths and default-checked Docker items
    /// selected.
    #[must_use]
    pub fn new(
        request: CleanupConfirmRequest,
        responder: Option<std::sync::mpsc::Sender<CleanupConfirmDecision>>,
        response_file: Option<PathBuf>,
    ) -> Self {
        let mut checked_paths = BTreeSet::new();
        for item in &request.items {
            checked_paths.insert(item.path.clone());
        }
        let mut checked_docker_targets = HashSet::new();
        for item in &request.docker_items {
            if item.default_checked {
                checked_docker_targets.insert(item.target.clone());
            }
        }
        Self {
            request,
            checked_paths,
            checked_docker_targets,
            responder,
            response_file,
            remote_profile_id: None,
            decided: false,
        }
    }

    /// Tag this modal as targeting a remote SSH profile rather than local disk.
    #[must_use]
    pub fn with_remote_profile(mut self, profile_id: String) -> Self {
        self.remote_profile_id = Some(profile_id);
        self
    }

    #[must_use]
    pub fn is_checked(&self, path: &Path) -> bool {
        self.checked_paths.contains(path)
    }

    pub fn toggle_path(&mut self, path: &Path) {
        if self.checked_paths.contains(path) {
            self.checked_paths.remove(path);
        } else {
            self.checked_paths.insert(path.to_path_buf());
        }
    }

    #[must_use]
    pub fn is_docker_checked(&self, target: &DockerCleanTarget) -> bool {
        self.checked_docker_targets.contains(target)
    }

    pub fn toggle_docker_target(&mut self, target: &DockerCleanTarget) {
        if self.checked_docker_targets.contains(target) {
            self.checked_docker_targets.remove(target);
        } else {
            self.checked_docker_targets.insert(target.clone());
        }
    }

    pub fn toggle_docker_group(&mut self, group: DockerGroupKind) {
        let targets: Vec<DockerCleanTarget> = self
            .request
            .docker_items
            .iter()
            .filter(|i| docker_target_matches_group(&i.target, group))
            .map(|i| i.target.clone())
            .collect();
        if targets.is_empty() {
            return;
        }
        let all_checked = targets
            .iter()
            .all(|t| self.checked_docker_targets.contains(t));
        if all_checked {
            for t in &targets {
                self.checked_docker_targets.remove(t);
            }
        } else {
            for t in targets {
                self.checked_docker_targets.insert(t);
            }
        }
    }

    pub fn select_all(&mut self) {
        for item in &self.request.items {
            self.checked_paths.insert(item.path.clone());
        }
        for item in &self.request.docker_items {
            self.checked_docker_targets.insert(item.target.clone());
        }
    }

    pub fn select_none(&mut self) {
        self.checked_paths.clear();
        self.checked_docker_targets.clear();
    }

    #[must_use]
    pub const fn total_count(&self) -> usize {
        self.request.items.len() + self.request.docker_items.len()
    }

    #[must_use]
    pub fn selected_count(&self) -> usize {
        self.checked_paths.len() + self.checked_docker_targets.len()
    }

    #[must_use]
    pub fn selected_bytes(&self) -> u64 {
        let file_bytes: u64 = self
            .request
            .items
            .iter()
            .filter(|i| self.checked_paths.contains(&i.path))
            .map(|i| i.bytes)
            .sum();
        let docker_bytes: u64 = self
            .request
            .docker_items
            .iter()
            .filter(|i| self.checked_docker_targets.contains(&i.target))
            .map(|i| i.reclaimable_bytes)
            .sum();
        file_bytes.saturating_add(docker_bytes)
    }

    pub fn confirm(&mut self) -> CleanupConfirmDecision {
        self.decided = true;
        let selected_paths: Vec<PathBuf> = self
            .request
            .items
            .iter()
            .filter(|i| self.checked_paths.contains(&i.path))
            .map(|i| i.path.clone())
            .collect();
        let selected_docker_targets: Vec<DockerCleanTarget> = self
            .request
            .docker_items
            .iter()
            .filter(|i| self.checked_docker_targets.contains(&i.target))
            .map(|i| i.target.clone())
            .collect();
        let decision = CleanupConfirmDecision::Confirmed {
            selected_paths,
            selected_docker_targets,
        };
        if let Some(resp) = self.responder.take() {
            let _ = resp.send(decision.clone());
        }
        if let Some(ref path) = self.response_file {
            let _ = write_decision_file(path, &decision);
        }
        decision
    }

    pub fn cancel(&mut self) -> CleanupConfirmDecision {
        self.decided = true;
        let decision = CleanupConfirmDecision::Cancelled;
        if let Some(resp) = self.responder.take() {
            let _ = resp.send(decision.clone());
        }
        if let Some(ref path) = self.response_file {
            let _ = write_decision_file(path, &decision);
        }
        decision
    }
}

impl Drop for CleanupConfirmModal {
    fn drop(&mut self) {
        if !self.decided {
            let decision = CleanupConfirmDecision::Cancelled;
            if let Some(resp) = self.responder.take() {
                let _ = resp.send(decision.clone());
            }
            if let Some(ref path) = self.response_file {
                let _ = write_decision_file(path, &decision);
            }
        }
    }
}

#[must_use]
pub const fn docker_target_matches_group(
    target: &DockerCleanTarget,
    group: DockerGroupKind,
) -> bool {
    matches!(
        (target, group),
        (DockerCleanTarget::BuildCache, DockerGroupKind::BuildCache)
            | (DockerCleanTarget::Image { .. }, DockerGroupKind::Images)
            | (
                DockerCleanTarget::Container { .. },
                DockerGroupKind::Containers
            )
    )
}

#[must_use]
pub fn docker_short_id(id: &str) -> String {
    id.strip_prefix("sha256:")
        .unwrap_or(id)
        .chars()
        .take(12)
        .collect()
}

fn sync_docker_defaults_for(
    docker: &DockerProbeStatus,
    checked: &mut HashSet<DockerCleanTarget>,
    initialized: &mut bool,
    locale: crate::i18n::Locale,
) {
    let (items, _) = build_docker_confirm_items(docker, &[], locale);
    checked.clear();
    for item in items {
        if item.default_checked {
            checked.insert(item.target);
        }
    }
    *initialized = true;
}

fn toggle_docker_target_in(
    checked: &mut HashSet<DockerCleanTarget>,
    target: &DockerCleanTarget,
) {
    if checked.contains(target) {
        checked.remove(target);
    } else {
        checked.insert(target.clone());
    }
}

fn select_all_docker_in(
    docker: &DockerProbeStatus,
    checked: &mut HashSet<DockerCleanTarget>,
    locale: crate::i18n::Locale,
) {
    let (items, _) = build_docker_confirm_items(docker, &[], locale);
    for item in items {
        checked.insert(item.target);
    }
}

fn toggle_docker_group_in(
    docker: &DockerProbeStatus,
    checked: &mut HashSet<DockerCleanTarget>,
    group: DockerGroupKind,
    locale: crate::i18n::Locale,
) {
    let (items, _) = build_docker_confirm_items(docker, &[], locale);
    let targets: Vec<DockerCleanTarget> = items
        .into_iter()
        .filter(|i| docker_target_matches_group(&i.target, group))
        .map(|i| i.target)
        .collect();
    if targets.is_empty() {
        return;
    }
    let all_checked = targets.iter().all(|t| checked.contains(t));
    if all_checked {
        for t in &targets {
            checked.remove(t);
        }
    } else {
        for t in targets {
            checked.insert(t);
        }
    }
}

fn selected_docker_targets_in(
    docker: &DockerProbeStatus,
    checked: &HashSet<DockerCleanTarget>,
) -> Vec<DockerCleanTarget> {
    let DockerProbeStatus::Available(summary) = docker else {
        return Vec::new();
    };
    let requested: Vec<DockerCleanTarget> = checked.iter().cloned().collect();
    validate_targets_against_summary(&requested, summary)
}

fn selected_docker_bytes_in(
    docker: &DockerProbeStatus,
    checked: &HashSet<DockerCleanTarget>,
    locale: crate::i18n::Locale,
) -> u64 {
    let (items, _) = build_docker_confirm_items(docker, &[], locale);
    items
        .into_iter()
        .filter(|i| checked.contains(&i.target))
        .map(|i| i.reclaimable_bytes)
        .sum()
}

/// Build item-level Docker confirmation checkboxes and optional permission hint
/// from a [`DockerProbeStatus`].
#[must_use]
pub fn build_docker_confirm_items(
    docker: &DockerProbeStatus,
    ai_recommended: &[DockerCleanTarget],
    locale: crate::i18n::Locale,
) -> (Vec<DockerConfirmItem>, Option<String>) {
    let summary = match docker {
        DockerProbeStatus::Available(summary) => summary,
        DockerProbeStatus::PermissionDenied { socket_hint } => {
            return (Vec::new(), Some(socket_hint.clone()));
        }
        DockerProbeStatus::NotInstalled | DockerProbeStatus::DaemonStopped => {
            return (Vec::new(), None);
        }
    };

    let mut items = Vec::new();
    let ai_reason = crate::i18n::review::docker_ai_recommended(locale);

    let standalone_bc = summary
        .build_cache_reclaimable_bytes
        .max(summary.build_cache.reclaimable_bytes);
    if standalone_bc > 0 || summary.build_cache_shared_bytes > 0 {
        let target = DockerCleanTarget::BuildCache;
        let is_ai = ai_recommended.contains(&target);
        let reclaim_str = ruum_core::size::human_bytes(standalone_bc);
        let shared_str = (summary.build_cache_shared_bytes > 0).then(|| {
            ruum_core::size::human_bytes(summary.build_cache_shared_bytes)
        });
        items.push(DockerConfirmItem {
            target,
            label: crate::i18n::review::docker_build_cache_label(locale)
                .to_owned(),
            detail: Some(crate::i18n::pages::docker_build_cache_detail(
                &reclaim_str,
                shared_str.as_deref(),
                locale,
            )),
            reclaimable_bytes: standalone_bc,
            default_checked: standalone_bc > 0 || is_ai,
            reason: is_ai.then(|| ai_reason.to_owned()),
        });
    }

    for img in &summary.removable_images {
        let reference = (!img.dangling
            && !img.repository.is_empty()
            && img.repository != "<none>"
            && !img.tag.is_empty()
            && img.tag != "<none>")
            .then(|| format!("{}:{}", img.repository, img.tag));
        let label = reference
            .clone()
            .unwrap_or_else(|| "<none>:<none>".to_owned());
        let short_id = docker_short_id(&img.id);
        let detail = if img.created_since.is_empty() {
            Some(short_id)
        } else {
            Some(format!("{short_id} \u{00b7} {}", img.created_since))
        };
        let reclaimable_bytes = if img.unique_bytes > 0 {
            img.unique_bytes
        } else {
            img.size_bytes
        };
        let is_ai = ai_recommended.iter().any(|t| match t {
            DockerCleanTarget::Image {
                id,
                reference: target_ref,
                ..
            } => {
                id == &img.id
                    && (target_ref.is_none()
                        || target_ref.as_deref() == reference.as_deref())
            }
            _ => false,
        });
        items.push(DockerConfirmItem {
            target: DockerCleanTarget::Image {
                id: img.id.clone(),
                reference,
                dangling: img.dangling,
            },
            label,
            detail,
            reclaimable_bytes,
            default_checked: img.dangling || is_ai,
            reason: is_ai.then(|| ai_reason.to_owned()),
        });
    }

    for ctr in &summary.stopped_containers {
        let short_id = docker_short_id(&ctr.id);
        let label = if ctr.name.is_empty() {
            short_id
        } else {
            ctr.name.clone()
        };
        let status_part = if ctr.status.is_empty() {
            ctr.state.as_str()
        } else {
            ctr.status.as_str()
        };
        let detail = match (ctr.image.is_empty(), status_part.is_empty()) {
            (false, false) => {
                Some(format!("{} \u{00b7} {status_part}", ctr.image))
            }
            (false, true) => Some(ctr.image.clone()),
            (true, false) => Some(status_part.to_owned()),
            (true, true) => None,
        };
        let is_ai = ai_recommended.iter().any(|t| {
            matches!(t, DockerCleanTarget::Container { id, .. } if id == &ctr.id)
        });
        items.push(DockerConfirmItem {
            target: DockerCleanTarget::Container {
                id: ctr.id.clone(),
                name: ctr.name.clone(),
            },
            label,
            detail,
            reclaimable_bytes: ctr.size_bytes,
            default_checked: is_ai,
            reason: is_ai.then(|| ai_reason.to_owned()),
        });
    }

    (items, None)
}

/// Build a standalone confirmation request from a saved AI cleanup proposal.
#[cfg(test)]
pub fn proposal_confirm_request(
    root: PathBuf,
    proposal: &ruum_core::watchdog_config::CleanupProposal,
    locale: crate::i18n::Locale,
) -> CleanupConfirmRequest {
    proposal_confirm_request_with_docker(
        root,
        proposal,
        &DockerProbeStatus::NotInstalled,
        locale,
    )
}

/// Build a standalone confirmation request from a saved local AI cleanup
/// proposal and live [`DockerProbeStatus`].
pub fn proposal_confirm_request_with_docker(
    root: PathBuf,
    proposal: &ruum_core::watchdog_config::CleanupProposal,
    docker: &DockerProbeStatus,
    locale: crate::i18n::Locale,
) -> CleanupConfirmRequest {
    let items = proposal
        .items
        .iter()
        .map(|item| CleanupConfirmItem {
            path: item.path.clone(),
            bytes: item.bytes,
            mode: item.mode,
            reason: item.reason.clone(),
            git_warning: item.git_warning.clone(),
        })
        .collect();
    let (docker_items, docker_permission_hint) =
        build_docker_confirm_items(docker, &proposal.docker_targets, locale);
    CleanupConfirmRequest {
        root,
        title: crate::i18n::messages::ai_proposal_title(locale).to_string(),
        summary: Some(proposal.summary.clone()),
        items,
        docker_items,
        docker_permission_hint,
    }
}

/// Build a standalone confirmation request from a remote SSH AI proposal.
#[cfg(test)]
pub fn remote_proposal_confirm_request(
    root: PathBuf,
    alias: &str,
    proposal: &ruum_core::watchdog_config::CleanupProposal,
    locale: crate::i18n::Locale,
) -> CleanupConfirmRequest {
    remote_proposal_confirm_request_with_docker(
        root,
        alias,
        proposal,
        &DockerProbeStatus::NotInstalled,
        locale,
    )
}

/// Build a standalone confirmation request from a remote SSH AI proposal and
/// snapshot [`DockerProbeStatus`].
pub fn remote_proposal_confirm_request_with_docker(
    root: PathBuf,
    alias: &str,
    proposal: &ruum_core::watchdog_config::CleanupProposal,
    docker: &DockerProbeStatus,
    locale: crate::i18n::Locale,
) -> CleanupConfirmRequest {
    let items = proposal
        .items
        .iter()
        .map(|item| CleanupConfirmItem {
            path: item.path.clone(),
            bytes: item.bytes,
            mode: item.mode,
            reason: item.reason.clone(),
            git_warning: item.git_warning.clone(),
        })
        .collect();
    let (docker_items, docker_permission_hint) =
        build_docker_confirm_items(docker, &proposal.docker_targets, locale);
    CleanupConfirmRequest {
        root,
        title: crate::i18n::messages::ai_proposal_remote_title(alias, locale),
        summary: Some(proposal.summary.clone()),
        items,
        docker_items,
        docker_permission_hint,
    }
}

fn write_decision_file(
    path: &Path,
    decision: &CleanupConfirmDecision,
) -> std::io::Result<()> {
    let json = serde_json::to_string_pretty(decision)
        .map_err(|e| std::io::Error::new(std::io::ErrorKind::InvalidData, e))?;
    std::fs::write(path, json)
}

fn write_remote_connect_decision_file(
    path: &Path,
    decision: RemoteConnectConfirmDecision,
) -> std::io::Result<()> {
    let json = serde_json::to_string_pretty(&decision)
        .map_err(|e| std::io::Error::new(std::io::ErrorKind::InvalidData, e))?;
    std::fs::write(path, json)
}

/// Interactive modal state holding an MCP remote SSH connection request.
#[derive(Debug)]
pub struct RemoteConnectConfirmModal {
    pub request: RemoteConnectConfirmRequest,
    pub responder:
        Option<std::sync::mpsc::Sender<RemoteConnectConfirmDecision>>,
    pub response_file: Option<PathBuf>,
    pub decided: bool,
}

impl RemoteConnectConfirmModal {
    /// Create a new modal for an incoming remote SSH connection request.
    #[must_use]
    pub const fn new(
        request: RemoteConnectConfirmRequest,
        responder: Option<
            std::sync::mpsc::Sender<RemoteConnectConfirmDecision>,
        >,
        response_file: Option<PathBuf>,
    ) -> Self {
        Self {
            request,
            responder,
            response_file,
            decided: false,
        }
    }

    pub fn approve(&mut self) -> RemoteConnectConfirmDecision {
        self.finish(RemoteConnectConfirmDecision::Approved)
    }

    pub fn deny(&mut self) -> RemoteConnectConfirmDecision {
        self.finish(RemoteConnectConfirmDecision::Denied)
    }

    fn finish(
        &mut self,
        decision: RemoteConnectConfirmDecision,
    ) -> RemoteConnectConfirmDecision {
        self.decided = true;
        if let Some(resp) = self.responder.take() {
            let _ = resp.send(decision);
        }
        if let Some(ref path) = self.response_file {
            let _ = write_remote_connect_decision_file(path, decision);
        }
        decision
    }
}

impl Drop for RemoteConnectConfirmModal {
    fn drop(&mut self) {
        if !self.decided {
            let decision = RemoteConnectConfirmDecision::Denied;
            if let Some(resp) = self.responder.take() {
                let _ = resp.send(decision);
            }
            if let Some(ref path) = self.response_file {
                let _ = write_remote_connect_decision_file(path, decision);
            }
        }
    }
}

/// Category tab shown in the Zed-style left sidebar of `Screen::Settings`.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum SettingsCategory {
    #[default]
    General,
    Watchdog,
    Webhook,
    Ai,
    Cli,
    Updates,
}

/// UI state machine for in-app update checks, downloads, and installation.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub enum UpdateUiState {
    #[default]
    Idle,
    Checking,
    UpToDate {
        checked_at_secs: u64,
    },
    Available {
        info: Arc<UpdateCheckResult>,
    },
    Downloading {
        info: Arc<UpdateCheckResult>,
        downloaded: u64,
        total: u64,
    },
    ReadyToInstall {
        info: Arc<UpdateCheckResult>,
        artifact_path: PathBuf,
    },
    Installing {
        info: Arc<UpdateCheckResult>,
    },
    Error {
        message: String,
        info: Option<Arc<UpdateCheckResult>>,
    },
}

impl UpdateUiState {
    /// Return the associated [`UpdateCheckResult`] metadata when present.
    #[must_use]
    pub const fn info(&self) -> Option<&Arc<UpdateCheckResult>> {
        match self {
            Self::Available { info }
            | Self::Downloading { info, .. }
            | Self::ReadyToInstall { info, .. }
            | Self::Installing { info } => Some(info),
            Self::Error { info, .. } => info.as_ref(),
            Self::Idle | Self::Checking | Self::UpToDate { .. } => None,
        }
    }

    /// Return whether an available update version was explicitly skipped.
    #[must_use]
    pub fn is_skipped(&self, settings: &UpdateSettings) -> bool {
        match self {
            Self::Available { info } => {
                settings.skipped_version.as_deref()
                    == Some(info.latest_version.as_str())
            }
            Self::Idle
            | Self::Checking
            | Self::UpToDate { .. }
            | Self::Downloading { .. }
            | Self::ReadyToInstall { .. }
            | Self::Installing { .. }
            | Self::Error { .. } => false,
        }
    }

    /// Whether the left icon rail should reveal the dedicated update button.
    #[must_use]
    pub fn should_show_rail_entry(&self, settings: &UpdateSettings) -> bool {
        match self {
            Self::Available { .. } => !self.is_skipped(settings),
            Self::Downloading { .. }
            | Self::ReadyToInstall { .. }
            | Self::Installing { .. } => true,
            Self::Idle
            | Self::Checking
            | Self::UpToDate { .. }
            | Self::Error { .. } => false,
        }
    }

    /// Return the active unskipped version string for tray menu updates.
    #[must_use]
    pub fn available_version_if_unskipped(
        &self,
        settings: &UpdateSettings,
    ) -> Option<&str> {
        if self.should_show_rail_entry(settings) {
            self.info().map(|i| i.latest_version.as_str())
        } else {
            None
        }
    }

    /// Whether the first-seen-per-version non-blocking banner should display.
    #[must_use]
    pub fn should_show_banner(
        &self,
        settings: &UpdateSettings,
        dismissed_version: Option<&str>,
    ) -> bool {
        match self {
            Self::Available { info } if !self.is_skipped(settings) => {
                dismissed_version != Some(info.latest_version.as_str())
            }
            Self::ReadyToInstall { info, .. } => {
                dismissed_version != Some(info.latest_version.as_str())
            }
            _ => false,
        }
    }

    /// Whether opening `Check for Updates` should show the update modal
    /// immediately instead of jumping to `Settings::Updates` to re-check.
    #[must_use]
    pub fn should_open_modal_directly(
        &self,
        settings: &UpdateSettings,
    ) -> bool {
        match self {
            Self::Available { .. } => !self.is_skipped(settings),
            Self::Downloading { .. }
            | Self::ReadyToInstall { .. }
            | Self::Installing { .. }
            | Self::Error { info: Some(_), .. } => true,
            Self::Idle
            | Self::Checking
            | Self::UpToDate { .. }
            | Self::Error { info: None, .. } => false,
        }
    }
}

/// Choice presented in the first-run AI setup modal.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum AiSetupChoice {
    RuumCloud,
    CustomApi,
    LocalCli,
}

/// Deferred user action paused until first-run AI onboarding finishes.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum PendingAiSetupAction {
    LocalAnalysis,
    SshAnalysis { profile_id: String },
    EnableWatchdogAutoAnalyze,
}

/// State for the first-run 3-choice AI setup onboarding modal.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct AiSetupChoiceModalState {
    pub pending_action: PendingAiSetupAction,
}

/// Polling cadence and ceiling for Alipay QR checkout status verification.
const BILLING_POLL_INTERVAL: Duration = Duration::from_secs(3);
const BILLING_POLL_MAX_DURATION: Duration = Duration::from_mins(5);
const CLOUD_REFRESH_DEBOUNCE: Duration = Duration::from_secs(2);

/// Active step inside `CloudBillingModal`.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum BillingModalStep {
    #[default]
    SelectPlan,
    Checkout,
}

/// State for the Ruum Cloud pass billing & Alipay QR checkout modal.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct BillingModalState {
    pub step: BillingModalStep,
    pub plans: Option<BillingPlansSnapshot>,
    pub loading_plans: bool,
    pub plans_unavailable: bool,
    pub selected_sku_id: Option<String>,
    pub pending_out_request_no: Option<String>,
    pub active_order: Option<PurchaseOrderSnapshot>,
    pub active_sku: Option<ChargingOption>,
    pub qr_matrix: Option<Arc<crate::qr::QrMatrix>>,
    pub creating_order: bool,
    pub refreshing_status: bool,
    pub polling_timed_out: bool,
    pub error: Option<String>,
    pub poll_epoch: u64,
}

fn is_alipay_qr_wrapper_page(raw: &str) -> bool {
    let Ok(parsed) = url::Url::parse(raw.trim()) else {
        return false;
    };
    parsed
        .host_str()
        .is_some_and(|h| h.eq_ignore_ascii_case("mobilecodec.alipay.com"))
        && parsed.path().contains("/show.htm")
}

/// Select the direct scannable QR payload from `[purchase_url, purchase_qr_code]`.
///
/// Prefers `purchase_url` when it uses `alipays://` (case-insensitive), when
/// `purchase_qr_code` is empty, or when `purchase_qr_code` is Alipay's hosted
/// `mobilecodec.alipay.com/show.htm` wrapper page (preventing nested QR scans).
#[must_use]
pub fn select_billing_qr_payload(
    purchase_url: Option<&str>,
    purchase_qr_code: Option<&str>,
) -> Option<String> {
    let url = purchase_url.map(str::trim).filter(|s| !s.is_empty());
    let qr = purchase_qr_code.map(str::trim).filter(|s| !s.is_empty());
    if let Some(u) = url {
        let is_alipays = u
            .get(..10)
            .is_some_and(|p| p.eq_ignore_ascii_case("alipays://"));
        if is_alipays
            || qr.is_none()
            || qr.is_some_and(is_alipay_qr_wrapper_page)
        {
            return Some(u.to_owned());
        }
    }
    if let Some(q) = qr {
        return Some(q.to_owned());
    }
    url.map(ToOwned::to_owned)
}

/// Official Ruum support / donation anchor URL for `locale`.
#[must_use]
pub const fn ruum_support_url(locale: crate::i18n::Locale) -> &'static str {
    match locale {
        crate::i18n::Locale::En => "https://ruum.jsonlee12138.com/#support",
        crate::i18n::Locale::Zh => "https://ruum.jsonlee12138.com/zh/#support",
        crate::i18n::Locale::Ja => "https://ruum.jsonlee12138.com/ja/#support",
        crate::i18n::Locale::Ko => "https://ruum.jsonlee12138.com/ko/#support",
    }
}

/// Select the first safe browser URL from `[purchase_url, purchase_qr_code]`.
///
/// Only `https://` URLs with a host are accepted in production (skipping mobile
/// deep links like `alipays://` or unsafe schemes such as `javascript:` /
/// `file:`). Under `#[cfg(test)]`, loopback `http://` (`127.0.0.1`,
/// `localhost`, `[::1]`) is also allowed for offline test stubs.
#[must_use]
pub fn select_safe_billing_browser_url(
    purchase_url: Option<&str>,
    purchase_qr_code: Option<&str>,
) -> Option<String> {
    for raw in [purchase_url, purchase_qr_code].into_iter().flatten() {
        let trimmed = raw.trim();
        if trimmed.is_empty() {
            continue;
        }
        let Ok(parsed) = url::Url::parse(trimmed) else {
            continue;
        };
        if parsed.scheme().eq_ignore_ascii_case("https") && parsed.has_host() {
            return Some(parsed.into());
        }
        #[cfg(test)]
        if parsed.scheme().eq_ignore_ascii_case("http")
            && is_loopback_host(parsed.host().as_ref())
        {
            return Some(parsed.into());
        }
    }
    None
}

#[cfg(test)]
const fn is_loopback_host(host: Option<&url::Host<&str>>) -> bool {
    match host {
        Some(url::Host::Domain(domain)) => {
            domain.eq_ignore_ascii_case("localhost")
        }
        Some(url::Host::Ipv4(ip)) => ip.is_loopback(),
        Some(url::Host::Ipv6(ip)) => ip.is_loopback(),
        None => false,
    }
}

/// Format a UTC Unix timestamp in seconds as `YYYY-MM-DD`.
#[must_use]
pub fn format_unix_date_ymd(secs: u64) -> String {
    let days = i64::try_from(secs / 86_400).unwrap_or(0);
    // Howard Hinnant's civil-from-days conversion (proleptic Gregorian).
    let z = days + 719_468;
    let era = if z >= 0 { z } else { z - 146_096 } / 146_097;
    let doe = z - era * 146_097;
    let yoe = (doe - doe / 1_460 + doe / 36_524 - doe / 146_096) / 365;
    let y = yoe + era * 400;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let d = doy - (153 * mp + 2) / 5 + 1;
    let m = if mp < 10 { mp + 3 } else { mp - 9 };
    let year = y + i64::from(m <= 2);
    format!("{year:04}-{m:02}-{d:02}")
}

/// Detect whether a formatted error indicates missing/exhausted cloud payment.
#[must_use]
pub fn is_cloud_payment_required_message(err: &str) -> bool {
    err.contains("ENTITLEMENT_PAYMENT_REQUIRED")
        || err.contains("INFERENCE_MODEL_NOT_ALLOWED")
        || err.contains("Free trial exhausted or Cloud Pro expired")
        || err.contains("Selected cloud model requires Cloud Pro")
        || err.contains(
            "\u{514d}\u{8d39}\u{4f53}\u{9a8c}\u{6b21}\u{6570}\u{5df2}\u{7528}\u{5b8c}\u{6216}\u{4e91}\u{7aef}\u{4f1a}\u{5458}\u{5df2}\u{5230}\u{671f}",
        )
        || err.contains(
            "\u{5f53}\u{524d}\u{6743}\u{76ca}\u{5c42}\u{7ea7}\u{65e0}\u{6743}\u{4f7f}\u{7528}\u{6240}\u{9009}\u{4e91}\u{7aef}\u{6a21}\u{578b}",
        )
}

/// Unique key identifying a host tab in the Explore top bar.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub enum HostTabKey {
    #[default]
    Local,
    Ssh(String),
}

/// Per-tab Explore session snapshot preserved across host tab switches.
pub struct ExploreSessionSnapshot {
    pub root_path: PathBuf,
    pub scan_root: PathBuf,
    pub tree: Option<Arc<Node>>,
    pub scan: Option<ScanHandle>,
    pub remote_scan: Option<ruum_core::remote::RemoteScan>,
    pub remote_disk: bool,
    pub remote_progress: ruum_core::remote_progress::Progress,
    pub scan_preview: Option<Arc<ruum_core::scan_preview::Preview>>,
    pub preview_updated: Option<Instant>,
    pub progress: ScanSnapshot,
    pub scan_error: Option<String>,
    pub scan_started: Option<Instant>,
    pub scan_elapsed: Option<Duration>,
    pub scanned_at: i64,
    pub scan_epoch: u64,
    pub crumbs: Vec<usize>,
    pub selected: Option<Vec<usize>>,
    pub hovered: Option<Vec<usize>>,
    pub history: History,
    pub view: View,
    pub transition: Option<LayoutTransition>,
    cache: Option<LayoutCache>,
    pub marks: Marks,
    pub space: Option<SpaceInfo>,
    pub space_baseline: Option<SpaceInfo>,
    pub disk_root: Option<PathBuf>,
    pub device: Option<String>,
    pub home: Option<PathBuf>,
    pub file_table: bool,
    pub full_disk_access: Option<bool>,
    pub remote: Option<String>,
    pub remote_platform: String,
    pub remote_identity: Option<ruum_core::identity::HostIdentity>,
    pub remote_header: Option<ruum_core::remote::Header>,
    pub docker: DockerProbeStatus,
    pub review_docker_checked: HashSet<DockerCleanTarget>,
    pub review_docker_initialized: bool,
    pub insights: Vec<Candidate>,
    pub git: FxHashMap<PathBuf, Option<GitState>>,
    git_pending: FxHashSet<PathBuf>,
    pub find: String,
    pub find_open: bool,
    pub matches: Option<Arc<Matches>>,
    pub filter_applied: bool,
    filter_epoch: u64,
    find_epoch: u64,
    pub finding: bool,
    apply_pending: bool,
    pub crumb_menu: Option<CrumbMenu>,
    pub awaiting_root: bool,
    pub active_ssh_profile_id: Option<String>,
    pub active_ssh_session: Option<Arc<SshSession>>,
}

impl Default for ExploreSessionSnapshot {
    fn default() -> Self {
        Self {
            root_path: PathBuf::new(),
            scan_root: PathBuf::new(),
            tree: None,
            scan: None,
            remote_scan: None,
            remote_disk: false,
            remote_progress: ruum_core::remote_progress::Progress::default(),
            scan_preview: None,
            preview_updated: None,
            progress: ScanSnapshot::default(),
            scan_error: None,
            scan_started: None,
            scan_elapsed: None,
            scanned_at: 0,
            scan_epoch: 0,
            crumbs: Vec::new(),
            selected: None,
            hovered: None,
            history: History::default(),
            view: View::default(),
            transition: None,
            cache: None,
            marks: Marks::default(),
            space: None,
            space_baseline: None,
            disk_root: None,
            device: None,
            home: None,
            file_table: false,
            full_disk_access: None,
            remote: None,
            remote_platform: String::new(),
            remote_identity: None,
            remote_header: None,
            docker: DockerProbeStatus::default(),
            review_docker_checked: HashSet::new(),
            review_docker_initialized: false,
            insights: Vec::new(),
            git: FxHashMap::default(),
            git_pending: FxHashSet::default(),
            find: String::new(),
            find_open: false,
            matches: None,
            filter_applied: false,
            filter_epoch: 0,
            find_epoch: 0,
            finding: false,
            apply_pending: false,
            crumb_menu: None,
            awaiting_root: false,
            active_ssh_profile_id: None,
            active_ssh_session: None,
        }
    }
}

/// A host tab entry in the Explore top bar.
pub struct ExploreHostTab {
    pub key: HostTabKey,
    pub label: String,
    pub session: Option<ExploreSessionSnapshot>,
}

/// Draft form state for the Settings page, isolated from committed config
/// until the user explicitly saves.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct WatchdogSettingsDraft {
    pub power_preset: crate::power::PowerEfficiency,
    pub apparent_size: bool,
    pub include_hidden: bool,
    pub follow_system_appearance: bool,
    pub close_to_background: bool,
    pub launch_at_login: bool,
    pub tray_show_disk_space: bool,
    pub enabled: bool,
    pub background_service: bool,
    pub ai_auto_analyze: bool,
    pub ai_provider: AiCliProvider,
    pub api_protocol: ApiProtocol,
    pub model: Option<String>,
}

impl Default for WatchdogSettingsDraft {
    fn default() -> Self {
        Self::from_configs(
            &AppSettings::default(),
            &WatchdogConfig::default(),
            &AiConfig::default(),
        )
    }
}

impl WatchdogSettingsDraft {
    #[must_use]
    pub fn from_configs(
        app: &AppSettings,
        wd: &WatchdogConfig,
        ai: &AiConfig,
    ) -> Self {
        Self {
            power_preset: crate::power::PowerEfficiency::parse(
                &app.power_preset,
            )
            .unwrap_or_default(),
            apparent_size: app.apparent_size,
            include_hidden: app.include_hidden,
            follow_system_appearance: app.follow_system_appearance,
            close_to_background: app.close_to_background,
            launch_at_login: app.launch_at_login,
            tray_show_disk_space: app.tray_show_disk_space,
            enabled: wd.enabled,
            background_service: wd.background_service,
            ai_auto_analyze: ai.auto_analyze_on_alert,
            ai_provider: ai.provider,
            api_protocol: ai.api_protocol,
            model: ai.model.clone(),
        }
    }
}

impl From<&WatchdogConfig> for WatchdogSettingsDraft {
    fn from(cfg: &WatchdogConfig) -> Self {
        Self {
            enabled: cfg.enabled,
            background_service: cfg.background_service,
            ..Self::default()
        }
    }
}

/// Everything the app knows and everything it can do.
pub struct Ruum {
    /// Active UI locale (`En`, `Zh`, `Ja`, `Ko`).
    pub locale: crate::i18n::Locale,
    /// The directory the tree was scanned from.
    pub root_path: PathBuf,
    pub remote: Option<String>,
    remote_disk: bool,
    pub remote_scan: Option<ruum_core::remote::RemoteScan>,
    pub remote_progress: ruum_core::remote_progress::Progress,
    pub scan_preview: Option<Arc<ruum_core::scan_preview::Preview>>,
    preview_updated: Option<Instant>,
    remote_platform: String,
    pub remote_identity: Option<ruum_core::identity::HostIdentity>,
    pub remote_header: Option<ruum_core::remote::Header>,
    pub docker: DockerProbeStatus,
    pub docker_probing: bool,
    pub review_docker_checked: HashSet<DockerCleanTarget>,
    pub review_docker_initialized: bool,
    pub local_docker: DockerProbeStatus,
    pub local_docker_checked: HashSet<DockerCleanTarget>,
    pub local_docker_initialized: bool,
    pub local_docker_probing: bool,
    pub remote_docker_modal_open: bool,
    pub pending_open_docker: bool,
    pub generated_script_dialog: Option<GeneratedScriptDialog>,
    pub done_return_screen: Screen,
    pub home: Option<PathBuf>,
    pub options: ScanOptions,
    pub power_choice: Option<crate::power::PowerEfficiency>,
    /// Injectable path to unified `SQLite` `ruum.db` (`None` under `cfg!(test)`).
    pub db_path: Option<PathBuf>,
    /// Lazy keychain-backed credential store (`None` under `cfg!(test)`).
    pub cred_store: Option<LazyCredentialStore>,
    /// Application-wide settings persisted in `app_settings`.
    pub app_settings: AppSettings,
    /// Saved SSH server cards loaded from `ssh_profiles` (metadata only).
    pub ssh_profiles: Vec<SshProfile>,
    /// Active SSH profile ID when exploring a saved SSH card.
    pub active_ssh_profile_id: Option<String>,
    /// Active in-memory SSH session (`SSH_AUTH_SOCK` or `SSH_ASKPASS`).
    pub active_ssh_session: Option<Arc<SshSession>>,
    /// Whether the inline SSH Add/Edit form is open on `Screen::Remote`.
    pub ssh_form_open: bool,
    /// Profile ID being edited, or `None` when adding a new SSH server.
    pub ssh_form_editing_id: Option<String>,
    /// Auth mode selected in the SSH Add/Edit form.
    pub ssh_form_auth_mode: AuthMode,
    /// Inline validation or storage error on the SSH Add/Edit form.
    pub ssh_form_error: Option<String>,
    /// When true, `views::root` syncs SSH form inputs from the pending state.
    pub ssh_form_needs_sync: bool,
    /// SSH form inputs.
    pub ssh_input_alias: Option<Entity<InputState>>,
    pub ssh_input_host: Option<Entity<InputState>>,
    pub ssh_input_port: Option<Entity<InputState>>,
    pub ssh_input_username: Option<Entity<InputState>>,
    pub ssh_input_default_path: Option<Entity<InputState>>,
    pub ssh_input_password: Option<Entity<InputState>>,
    pub ssh_input_private_key: Option<Entity<InputState>>,
    pub ssh_input_passphrase: Option<Entity<InputState>>,
    /// Active MCP remote SSH connection confirmation modal, if any.
    pub remote_connect_confirm: Option<RemoteConnectConfirmModal>,
    /// Queued MCP remote SSH connection requests waiting for user approval.
    pub remote_connect_confirm_queue: VecDeque<RemoteConnectConfirmModal>,
    /// Focus owner for the remote SSH connection confirmation modal.
    pub remote_connect_focus: FocusHandle,
    /// The Power Efficiency menu, when open: the row the arrow keys are on,
    /// as an index into [`crate::power::PowerEfficiency::ALL`].
    pub power_menu: Option<usize>,
    /// The pointer is on the menu's button. A press there is the button's
    /// to toggle, not a press outside the menu that closes it first.
    pub power_hover: bool,
    pub cpu_threads: usize,
    pub tree: Option<Arc<Node>>,
    pub scan: Option<ScanHandle>,
    pub scan_epoch: u64,
    pub progress: ScanSnapshot,
    pub scan_error: Option<String>,

    pub screen: Screen,
    /// Path from the scanned root to the directory currently drawn.
    pub crumbs: Vec<usize>,
    pub selected: Option<Vec<usize>>,
    pub hovered: Option<Vec<usize>>,
    pub history: History,
    /// The history button under the pointer, `true` for `<`: its card is
    /// drawn below it for as long as it is.
    pub history_hover: Option<bool>,
    /// The pointer moved more recently than the keyboard navigated. Then the
    /// tile under the pointer is what Space, X and Enter act on; after an
    /// arrow or Tab it is the keyboard selection again.
    pub pointer_active: bool,
    pub view: View,
    /// The in-flight layout transition, if a level was just entered or left.
    pub transition: Option<LayoutTransition>,
    pub layout_options: LayoutOptions,
    cache: Option<LayoutCache>,

    /// Mouse position in treemap-local pixels, for hit-testing and the tooltip.
    pub pointer: Option<Point<Pixels>>,
    /// The treemap area in window space, recorded while painting.
    pub treemap_origin: Rc<Cell<Point<Pixels>>>,
    pub treemap_size: Rc<Cell<Size<Pixels>>>,

    pub marks: Marks,
    pub removal_mode: RemovalMode,
    pub trash_backend: TrashBackend,
    #[cfg(test)]
    pub trash_env: Option<ruum_core::trash_adapter::TrashEnv>,
    #[cfg(test)]
    pub trash_os: Option<ruum_core::trash_adapter::TrashOs>,
    /// The permanent-deletion alert dialog is open. Trash needs no dialog: it
    /// is reversible, so it commits directly.
    pub confirm_open: bool,
    /// Focus owner for the alert dialog while it is open.
    pub confirm_focus: FocusHandle,
    /// Focus to move on the next occasion a window is in hand. Key handling
    /// has no window, and opening or closing the dialog must move focus.
    pub focus_request: Option<FocusTarget>,
    /// What the titlebar says, so it is only set when it changes.
    window_title: String,
    /// The window's `rem` in pixels, read each frame. The mosaic is laid out
    /// in pixels, so its header band and label thresholds are scaled by this
    /// to follow interface zoom like the rest of the interface.
    pub rem: f32,
    pub run: Option<RemovalHandle>,
    pub run_epoch: u64,
    pub run_stage: Option<String>,
    pub run_summary: RunSummary,
    pub run_log: Vec<(PathBuf, Result<(), String>)>,
    pub space: Option<SpaceInfo>,
    /// Free space before the removal, for the honest "what did it actually
    /// free" number rather than the sum of what was marked.
    pub space_baseline: Option<SpaceInfo>,
    pub notice: Option<(String, Status)>,

    pub find: String,
    pub find_open: bool,
    /// What the find text matches in the directory on screen, recomputed
    /// on every keystroke. While typing it only dims what does not match.
    pub matches: Option<Arc<Matches>>,
    /// Enter was pressed: the mosaic lays out only the matches.
    pub filter_applied: bool,
    filter_epoch: u64,
    /// Bumped per keystroke; only the newest search's result is kept.
    find_epoch: u64,
    /// A search is running off the UI thread.
    pub finding: bool,
    /// Enter was pressed before the search finished: apply on arrival.
    apply_pending: bool,
    pub show_help: bool,
    pub show_selection: bool,
    pub focus: FocusHandle,
    /// A trail crumb's sibling menu, when open.
    pub crumb_menu: Option<CrumbMenu>,

    pub color_mode: ColorMode,
    /// The largest things worth clearing, recomputed when a scan lands.
    pub insights: Vec<Candidate>,
    /// What git knows about each checkout that has been selected; `None`
    /// once asked and found not to be one.
    pub git: FxHashMap<PathBuf, Option<GitState>>,
    git_pending: FxHashSet<PathBuf>,
    /// The device the scanned volume is mounted from.
    pub device: Option<String>,
    /// Whether macOS lets this process read everything: asked once, since a
    /// grant only takes effect after a relaunch. `None` off macOS.
    pub full_disk_access: Option<bool>,
    /// Whether ruum runs as an administrator, which lets a whole drive
    /// be read from its file table instead of walked. `None` off Windows.
    pub administrator: Option<bool>,
    /// Whether the root is a whole NTFS drive, which an administrator reads
    /// from its file table. Follows the root; `false` off Windows.
    pub file_table: bool,
    /// A restart as administrator was asked for and not yet declined, so a
    /// second click does not raise a second prompt.
    pub restarting: bool,
    /// The top of the disk the scanned root lives on: what "Whole disk"
    /// scans. Follows the root when a folder is opened.
    pub disk_root: Option<PathBuf>,
    /// The volume picker: open, what it listed, and which row the keys are
    /// on. `None` when closed; empty when no volume besides the current one
    /// could be read.
    pub volumes_open: bool,
    pub volumes_loading: bool,
    /// When `Some(profile_id)`, the volume/directory modal targets that remote
    /// SSH profile (`RemoteRoots::discover_with_session`); when `None`, local.
    pub volume_picker_remote_profile_id: Option<String>,
    pub awaiting_root: bool,
    pub volumes_error: Option<String>,
    pub picker_error: Option<String>,
    pub picker_path: Option<Entity<InputState>>,
    volume_epoch: u64,
    pub volumes: Vec<ruum_core::space::Volume>,
    pub volume_highlight: usize,
    /// Open Explore host tabs (always starts with unclosable `HostTabKey::Local`).
    pub host_tabs: Vec<ExploreHostTab>,
    /// Currently active host tab key in Explore.
    pub active_host_tab: HostTabKey,
    /// SSH profile IDs currently running a disk-space preflight check.
    pub ssh_checking_profiles: FxHashSet<String>,
    /// SSH profile IDs currently running a remote AI analysis.
    pub ssh_analyzing_profiles: FxHashSet<String>,
    /// Active left-sidebar category on `Screen::Settings`.
    pub settings_category: SettingsCategory,
    /// Configured webhook endpoints loaded from `webhook_endpoints`.
    pub webhooks: Vec<WebhookEndpoint>,
    /// Whether the inline Add/Edit webhook form is open on Settings -> Webhooks.
    pub webhook_form_open: bool,
    /// Endpoint ID being edited, or `None` when adding a new webhook.
    pub webhook_form_editing_id: Option<String>,
    /// Enabled checkbox draft state for the webhook form.
    pub webhook_form_enabled: bool,
    /// Validation or persistence error shown inside the webhook form.
    pub webhook_form_error: Option<String>,
    /// When true, `views::root` syncs webhook form inputs from state.
    pub webhook_form_needs_sync: bool,
    /// Whether the Webhook Setup Tutorial & Payload Guide card is expanded.
    pub webhook_guide_open: bool,
    /// Webhook form input: human-readable endpoint name.
    pub webhook_input_name: Option<Entity<InputState>>,
    /// Endpoint IDs currently executing a live test POST.
    pub webhook_testing_ids: FxHashSet<String>,
    /// The side panel's width, in rem; dragged from its left edge.
    pub panel_rems: f32,
    pub scan_started: Option<Instant>,
    /// Where the scan in flight is rooted. Differs from `root_path` while
    /// widening, when the old tree stays on screen until the wider one lands.
    pub scan_root: PathBuf,
    pub scan_elapsed: Option<Duration>,
    /// Unix seconds when the tree landed: the "now" ages are measured from.
    pub scanned_at: i64,
    /// Watchdog monitoring configuration (`ruum_watchdog` table).
    pub watchdog_config: WatchdogConfig,
    /// Watchdog persistent runtime state (`ruum_watchdog` table).
    pub watchdog_state: WatchdogState,
    /// Generic HTTP webhook alert configuration (`webhook_config` table).
    pub webhook_config: WebhookConfig,
    /// Independent AI analysis configuration (`ai_config` table).
    pub ai_config: AiConfig,
    /// Latest AI cleanup proposal (`ai_state` table).
    pub ai_proposal: Option<CleanupProposal>,
    /// Latest AI analysis failure summary (`ai_state.last_error`).
    pub ai_last_error: Option<String>,
    /// Unix seconds when `ai_last_error` was recorded.
    pub ai_last_error_at_secs: Option<u64>,
    /// Cached Ruum Cloud registration, entitlement, and virtual model snapshot.
    pub cloud_state: CloudState,
    /// True while an explicit cloud entitlement/model refresh is in flight.
    pub cloud_refreshing: bool,
    /// Generation counter for discarding stale cloud entitlement refreshes.
    pub cloud_refresh_epoch: u64,
    /// True while an explicit Cloud virtual model list fetch is in flight.
    pub cloud_fetching_models: bool,
    /// Generation counter for discarding stale Cloud model fetch responses.
    pub cloud_fetch_models_epoch: u64,
    /// Shared mutex serializing `?refresh=1` calls against the 2s server debounce.
    pub cloud_refresh_lock: Arc<Mutex<()>>,
    /// Timestamp of the last `?refresh=1` request for 2s debounce coordination.
    pub cloud_last_refresh_at: Arc<Mutex<Option<Instant>>>,
    /// True after a 402 payment-required or 0-available-models condition.
    pub cloud_payment_required: bool,
    /// Active first-run 3-choice AI onboarding modal, if open.
    pub ai_setup_choice_modal: Option<AiSetupChoiceModalState>,
    /// Focus owner for `AiSetupChoiceModal`.
    pub ai_setup_choice_focus: FocusHandle,
    /// Active Ruum Cloud billing / pass upgrade modal, if open.
    pub billing_modal: Option<BillingModalState>,
    /// Epoch counter invalidating stale billing plan loads and poll loops.
    pub billing_modal_epoch: u64,
    /// Focus owner for `CloudBillingModal`.
    pub billing_focus: FocusHandle,
    /// Settings `RedeemCodeControl` text input.
    pub cloud_redeem_input_settings: Option<Entity<InputState>>,
    /// When true, `views::root` clears the redemption code input.
    pub cloud_redeem_inputs_need_clear: bool,
    /// True while `POST /v1/entitlement/redeem` is in flight (double-submit guard).
    pub redeem_submitting: bool,
    /// Epoch counter invalidating stale redemption responses.
    pub redeem_epoch: u64,
    /// Inline feedback `(message, is_error)` rendered inside `RedeemCodeControl`.
    pub cloud_redeem_feedback: Option<(String, bool)>,
    /// Unsaved toggle/provider/model selections on the Settings page.
    pub watchdog_draft: WatchdogSettingsDraft,
    /// When true, `views::root` syncs Settings text inputs from committed DB state.
    pub watchdog_inputs_need_sync: bool,
    /// Settings input: default treemap depth (`1..=6`).
    pub setting_input_default_depth: Option<Entity<InputState>>,
    /// Settings input: SSH session / MCP lease duration in minutes.
    pub setting_input_ssh_duration: Option<Entity<InputState>>,
    /// Settings input: threshold minimum free gigabytes.
    pub watchdog_input_min_free: Option<Entity<InputState>>,
    /// Settings input: watchdog check interval in minutes.
    pub watchdog_input_poll_interval: Option<Entity<InputState>>,
    /// Settings input: watchdog re-alert cooldown in minutes.
    pub watchdog_input_cooldown: Option<Entity<InputState>>,
    /// Settings input: generic webhook URL.
    pub setting_input_webhook_url: Option<Entity<InputState>>,
    /// Settings input: generic webhook timeout in seconds.
    pub setting_input_webhook_timeout: Option<Entity<InputState>>,
    /// Settings input: generic webhook headers JSON object.
    pub setting_input_webhook_headers: Option<Entity<InputState>>,
    /// Settings input: custom AI CLI command.
    pub watchdog_input_custom_cmd: Option<Entity<InputState>>,
    /// Settings input: custom AI CLI extra arguments.
    pub watchdog_input_extra_args: Option<Entity<InputState>>,
    /// Settings input: optional AI model override.
    pub watchdog_input_model: Option<Entity<InputState>>,
    /// Settings input: direct AI API base URL.
    pub watchdog_input_api_base_url: Option<Entity<InputState>>,
    /// Settings input: masked direct AI API key (write-only draft).
    pub watchdog_input_api_key: Option<Entity<InputState>>,
    /// Session-local knowledge of whether a direct API key is saved
    /// (`None` until a user action checks/writes/clears it, avoiding startup
    /// keychain prompts).
    pub api_key_session_known: Option<bool>,
    /// Monotonic generation counter for auto-dismissing toast notices.
    pub toast_epoch: u64,
    /// True while testing AI CLI connectivity.
    pub watchdog_testing: bool,
    /// Generation counter for discarding stale AI connection tests.
    pub watchdog_test_epoch: u64,
    /// True while fetching available models from the selected AI CLI.
    pub watchdog_fetching_models: bool,
    /// Generation counter for discarding stale model list fetches.
    pub watchdog_fetch_epoch: u64,
    /// Models discovered for the active AI CLI provider.
    pub watchdog_models: Vec<ruum_core::ai_cli::DiscoveredModel>,
    /// Model IDs currently running a speed benchmark (or `(default)`).
    pub watchdog_benchmarking_models: FxHashSet<String>,
    /// Generation counter for discarding stale model speed probes on switch.
    pub watchdog_benchmark_epoch: u64,
    /// Latest per-model speed test results in milliseconds.
    pub watchdog_speed_results: Vec<ruum_core::ai_cli::ModelSpeedResult>,
    /// True while saving settings / updating OS background service.
    pub watchdog_saving: bool,
    /// True while a manual Watchdog AI analysis runs in the background.
    pub watchdog_analyzing: bool,
    /// Generation counter for Watchdog AI analysis jobs.
    pub watchdog_analysis_epoch: u64,
    /// Unix seconds when disk space was last checked on Watchdog page.
    pub watchdog_space_checked_at: Option<u64>,
    /// Free space on the home directory's volume for the Watchdog dashboard.
    pub watchdog_home_space: Option<SpaceInfo>,
    /// Background daemon OS service status.
    pub watchdog_service_status: Option<ServiceStatus>,
    /// Self-CLI PATH registration status.
    pub cli_install_status: Option<CliInstallStatus>,
    /// True while installing or uninstalling the `ruum` CLI command.
    pub cli_updating: bool,
    /// Current software update state machine.
    pub update_state: UpdateUiState,
    /// Whether the centered software update card modal is open.
    pub update_modal_open: bool,
    /// Version whose non-blocking bottom-right update banner was dismissed.
    pub update_banner_dismissed_version: Option<String>,
    /// Persisted update check preferences (`update_settings` table).
    pub update_settings: UpdateSettings,
    /// Detected OS installation form for self-update or command guidance.
    pub update_install_kind: InstallKind,
    /// Shared cancellation token for an in-flight background asset download.
    pub update_download_cancel: Option<Arc<AtomicBool>>,
    /// Most recent update check/download/install error message.
    pub update_last_error: Option<String>,
    /// Focus handle for the centered update modal dialog.
    pub update_focus: FocusHandle,
    #[cfg(test)]
    pub cli_install_env: Option<CliInstallEnv>,
    #[cfg(test)]
    pub cli_install_os: Option<ruum_core::cli_install::CliInstallOs>,
    #[cfg(test)]
    pub cli_install_exe: Option<PathBuf>,
    #[cfg(test)]
    pub docker_runner:
        Option<Arc<dyn ruum_core::docker_adapter::DockerCommandRunner>>,
    #[cfg(test)]
    pub terminal_env_override: Option<TerminalEnv>,
    #[cfg(test)]
    pub terminal_os_override: Option<TerminalOs>,
    #[cfg(test)]
    pub terminal_launch_log: Vec<(String, TerminalLaunchPlan)>,
    #[cfg(test)]
    pub cloud_base_url_override: Option<String>,
}

fn open_db_for_window(this: &Ruum) -> Option<StorageDb> {
    if this.db_path.is_some() {
        this.open_db().ok()
    } else if !cfg!(test) {
        StorageDb::open_default().ok()
    } else {
        None
    }
}

fn open_db_for_background(db_path: Option<&Path>) -> Option<StorageDb> {
    if let Some(path) = db_path {
        StorageDb::open(path).ok()
    } else if !cfg!(test) {
        StorageDb::open_default().ok()
    } else {
        None
    }
}

impl Ruum {
    #[cfg(test)]
    pub fn new(
        root_path: PathBuf,
        options: ScanOptions,
        depth: u32,
        cx: &mut Context<'_, Self>,
    ) -> Self {
        Self::open(root_path, options, depth, None, false, cx)
    }

    #[cfg(test)]
    pub fn open(
        root_path: PathBuf,
        options: ScanOptions,
        depth: u32,
        remote: Option<String>,
        remote_disk: bool,
        cx: &mut Context<'_, Self>,
    ) -> Self {
        Self::open_with_cred_store(
            root_path,
            options,
            depth,
            remote,
            remote_disk,
            None,
            cx,
        )
    }

    pub(crate) fn open_with_cred_store(
        root_path: PathBuf,
        options: ScanOptions,
        depth: u32,
        remote: Option<String>,
        remote_disk: bool,
        cred_store: Option<LazyCredentialStore>,
        cx: &mut Context<'_, Self>,
    ) -> Self {
        Self::build(
            root_path,
            options,
            depth,
            remote,
            remote_disk,
            true,
            cred_store,
            cx,
        )
    }

    /// Startup without a scan: discovering roots is not consent to walk one.
    #[cfg(test)]
    pub fn choose(
        options: ScanOptions,
        depth: u32,
        remote: Option<String>,
        cx: &mut Context<'_, Self>,
    ) -> Self {
        Self::choose_with_cred_store(options, depth, remote, None, cx)
    }

    pub(crate) fn choose_with_cred_store(
        options: ScanOptions,
        depth: u32,
        remote: Option<String>,
        cred_store: Option<LazyCredentialStore>,
        cx: &mut Context<'_, Self>,
    ) -> Self {
        Self::build(
            PathBuf::new(),
            options,
            depth,
            remote,
            false,
            false,
            cred_store,
            cx,
        )
    }

    #[allow(
        clippy::too_many_arguments,
        reason = "internal window bootstrap passes launch flags and shared store"
    )]
    fn build(
        root_path: PathBuf,
        options: ScanOptions,
        depth: u32,
        remote: Option<String>,
        remote_disk: bool,
        start: bool,
        shared_cred_store: Option<LazyCredentialStore>,
        cx: &mut Context<'_, Self>,
    ) -> Self {
        let local = remote.is_none();
        let home = if local { std::env::home_dir() } else { None };
        let space = if local && start {
            space_info(&root_path).ok()
        } else {
            None
        };
        let trash_backend = detect_trash_backend();
        let (
            db_path,
            cred_store,
            app_settings,
            watchdog_config,
            watchdog_state,
            webhook_config,
            webhooks,
            ai_config,
            ai_state,
            ssh_profiles,
            watchdog_home_space,
            update_settings,
            update_install_kind,
        ) = if cfg!(test) {
            (
                None,
                shared_cred_store,
                AppSettings::default(),
                WatchdogConfig::default(),
                WatchdogState::default(),
                WebhookConfig::default(),
                Vec::new(),
                AiConfig::default(),
                ruum_core::storage::AiState::default(),
                Vec::new(),
                None,
                UpdateSettings::default(),
                InstallKind::Development,
            )
        } else {
            let db_path = ruum_watchdog::ruum_db_path();
            let cred_store = shared_cred_store
                .or_else(|| LazyCredentialStore::open_default().ok());
            let (
                app_s,
                wd_c,
                wd_s,
                wh_c,
                wh_list,
                ai_c,
                ai_st,
                profiles,
                upd_s,
            ) = db_path
                .as_deref()
                .and_then(|p| StorageDb::open(p).ok())
                .map_or_else(
                    || {
                        (
                            AppSettings::default(),
                            WatchdogConfig::default(),
                            WatchdogState::default(),
                            WebhookConfig::default(),
                            Vec::new(),
                            AiConfig::default(),
                            ruum_core::storage::AiState::default(),
                            Vec::new(),
                            UpdateSettings::default(),
                        )
                    },
                    |db| {
                        let wd = db.watchdog().ok();
                        (
                            db.load_app_settings().unwrap_or_default(),
                            wd.as_ref()
                                .and_then(|w| w.load_config().ok())
                                .unwrap_or_default(),
                            wd.as_ref()
                                .and_then(|w| w.load_state().ok())
                                .unwrap_or_default(),
                            db.load_webhook_config().unwrap_or_default(),
                            db.list_webhooks().unwrap_or_default(),
                            db.load_ai_config().unwrap_or_default(),
                            db.load_ai_state().ok().unwrap_or_default(),
                            db.list_ssh_profiles().unwrap_or_default(),
                            db.load_update_settings().unwrap_or_default(),
                        )
                    },
                );
            let home_space = home.as_deref().and_then(|h| space_info(h).ok());
            let install_kind = UpdateFactory::create()
                .detect_install_kind(&UpdateEnv::current());
            (
                db_path,
                cred_store,
                app_s,
                wd_c,
                wd_s,
                wh_c,
                wh_list,
                ai_c,
                ai_st,
                profiles,
                home_space,
                upd_s,
                install_kind,
            )
        };
        let cloud_state = if cfg!(test) {
            CloudState::default()
        } else {
            db_path
                .as_deref()
                .and_then(|p| StorageDb::open(p).ok())
                .and_then(|db| db.load_cloud_state().ok())
                .unwrap_or_default()
        };
        let ai_proposal = ai_state.parse_proposal().ok().flatten();
        let ai_last_error = ai_state.last_error;
        let ai_last_error_at_secs = ai_state.last_error_at_secs;
        let power_choice = if cfg!(test) {
            None
        } else {
            crate::power::PowerEfficiency::parse(&app_settings.power_preset)
        };
        let watchdog_space_checked_at = watchdog_state.last_checked_secs;
        let watchdog_draft = WatchdogSettingsDraft::from_configs(
            &app_settings,
            &watchdog_config,
            &ai_config,
        );

        let mut tree = Self {
            root_path,
            remote,
            remote_disk,
            remote_scan: None,
            scan_preview: None,
            preview_updated: None,
            remote_progress: ruum_core::remote_progress::Progress::default(),
            remote_platform: String::new(),
            remote_identity: None,
            remote_header: None,
            docker: DockerProbeStatus::default(),
            docker_probing: false,
            review_docker_checked: HashSet::new(),
            review_docker_initialized: false,
            local_docker: DockerProbeStatus::default(),
            local_docker_checked: HashSet::new(),
            local_docker_initialized: false,
            local_docker_probing: false,
            remote_docker_modal_open: false,
            pending_open_docker: false,
            generated_script_dialog: None,
            done_return_screen: Screen::Explore,
            home,
            options,
            power_choice,
            db_path,
            cred_store,
            app_settings,
            ssh_profiles,
            active_ssh_profile_id: None,
            active_ssh_session: None,
            ssh_form_open: false,
            ssh_form_editing_id: None,
            ssh_form_auth_mode: AuthMode::PrivateKey,
            ssh_form_error: None,
            ssh_form_needs_sync: false,
            ssh_input_alias: None,
            ssh_input_host: None,
            ssh_input_port: None,
            ssh_input_username: None,
            ssh_input_default_path: None,
            ssh_input_password: None,
            ssh_input_private_key: None,
            ssh_input_passphrase: None,
            remote_connect_confirm: None,
            remote_connect_confirm_queue: VecDeque::new(),
            remote_connect_focus: cx.focus_handle(),
            power_menu: None,
            power_hover: false,
            cpu_threads: crate::power::cpu_threads(),
            tree: None,
            scan: None,
            scan_epoch: 0,
            progress: ScanSnapshot::default(),
            scan_error: None,
            screen: Screen::Explore,
            crumbs: Vec::new(),
            selected: None,
            hovered: None,
            history: History::default(),
            history_hover: None,
            pointer_active: false,
            view: View::default(),
            transition: None,
            layout_options: LayoutOptions {
                max_depth: depth.clamp(1, 6),
                ..LayoutOptions::default()
            },
            cache: None,
            pointer: None,
            treemap_origin: Rc::new(Cell::new(Point::new(px(0.), px(0.)))),
            treemap_size: Rc::new(Cell::new(size(px(0.), px(0.)))),
            marks: Marks::default(),
            // Reversible by default whenever this machine has a trash: the
            // permanent path stays one choice away, behind a dialog.
            removal_mode: if trash_backend.is_available() {
                RemovalMode::Trash
            } else {
                RemovalMode::Permanent
            },
            trash_backend,
            #[cfg(test)]
            trash_env: None,
            #[cfg(test)]
            trash_os: None,
            confirm_open: false,
            confirm_focus: cx.focus_handle(),
            focus_request: None,
            window_title: String::new(),
            rem: crate::ui::BASE_REM,
            run: None,
            run_epoch: 0,
            run_stage: None,
            run_summary: RunSummary::default(),
            run_log: Vec::new(),
            space,
            space_baseline: None,
            notice: None,
            find: String::new(),
            find_open: false,
            matches: None,
            filter_applied: false,
            filter_epoch: 0,
            find_epoch: 0,
            finding: false,
            apply_pending: false,
            show_help: false,
            show_selection: true,
            focus: cx.focus_handle(),
            crumb_menu: None,
            color_mode: ColorMode::Kind,
            insights: Vec::new(),
            git: FxHashMap::default(),
            git_pending: FxHashSet::default(),
            device: None,
            full_disk_access: None,
            administrator: ruum_core::access::administrator(),
            file_table: false,
            restarting: false,
            disk_root: None,
            volumes_open: false,
            volumes_loading: false,
            volume_picker_remote_profile_id: None,
            awaiting_root: !start,
            volumes_error: None,
            picker_error: None,
            picker_path: None,
            volume_epoch: 0,
            volumes: Vec::new(),
            volume_highlight: 0,
            host_tabs: vec![ExploreHostTab {
                key: HostTabKey::Local,
                label: String::new(),
                session: None,
            }],
            active_host_tab: HostTabKey::Local,
            ssh_checking_profiles: FxHashSet::default(),
            ssh_analyzing_profiles: FxHashSet::default(),
            settings_category: SettingsCategory::General,
            webhooks,
            webhook_form_open: false,
            webhook_form_editing_id: None,
            webhook_form_enabled: true,
            webhook_form_error: None,
            webhook_form_needs_sync: false,
            webhook_guide_open: true,
            webhook_input_name: None,
            webhook_testing_ids: FxHashSet::default(),
            panel_rems: PANEL_REMS,
            scan_started: None,
            scan_root: PathBuf::new(),
            scan_elapsed: None,
            scanned_at: now_seconds(),
            watchdog_config,
            watchdog_state,
            webhook_config,
            ai_config,
            ai_proposal,
            ai_last_error,
            ai_last_error_at_secs,
            cloud_state,
            cloud_refreshing: false,
            cloud_refresh_epoch: 0,
            cloud_fetching_models: false,
            cloud_fetch_models_epoch: 0,
            cloud_refresh_lock: Arc::new(Mutex::new(())),
            cloud_last_refresh_at: Arc::new(Mutex::new(None)),
            cloud_payment_required: false,
            ai_setup_choice_modal: None,
            ai_setup_choice_focus: cx.focus_handle(),
            billing_modal: None,
            billing_modal_epoch: 0,
            billing_focus: cx.focus_handle(),
            cloud_redeem_input_settings: None,
            cloud_redeem_inputs_need_clear: false,
            redeem_submitting: false,
            redeem_epoch: 0,
            cloud_redeem_feedback: None,
            watchdog_draft,
            watchdog_inputs_need_sync: false,
            setting_input_default_depth: None,
            setting_input_ssh_duration: None,
            watchdog_input_min_free: None,
            watchdog_input_poll_interval: None,
            watchdog_input_cooldown: None,
            setting_input_webhook_url: None,
            setting_input_webhook_timeout: None,
            setting_input_webhook_headers: None,
            watchdog_input_custom_cmd: None,
            watchdog_input_extra_args: None,
            watchdog_input_model: None,
            watchdog_input_api_base_url: None,
            watchdog_input_api_key: None,
            api_key_session_known: None,
            toast_epoch: 0,
            watchdog_testing: false,
            watchdog_test_epoch: 0,
            watchdog_fetching_models: false,
            watchdog_fetch_epoch: 0,
            watchdog_models: Vec::new(),
            watchdog_benchmarking_models: FxHashSet::default(),
            watchdog_benchmark_epoch: 0,
            watchdog_speed_results: Vec::new(),
            watchdog_saving: false,
            watchdog_analyzing: false,
            watchdog_analysis_epoch: 0,
            watchdog_space_checked_at,
            watchdog_home_space,
            watchdog_service_status: None,
            cli_install_status: None,
            cli_updating: false,
            update_state: UpdateUiState::Idle,
            update_modal_open: false,
            update_banner_dismissed_version: None,
            update_settings,
            update_install_kind,
            update_download_cancel: None,
            update_last_error: None,
            update_focus: cx.focus_handle(),
            #[cfg(test)]
            cli_install_env: None,
            #[cfg(test)]
            cli_install_os: None,
            #[cfg(test)]
            cli_install_exe: None,
            #[cfg(test)]
            docker_runner: None,
            #[cfg(test)]
            terminal_env_override: None,
            #[cfg(test)]
            terminal_os_override: None,
            #[cfg(test)]
            terminal_launch_log: Vec::new(),
            #[cfg(test)]
            cloud_base_url_override: None,
            locale: crate::i18n::Locale::current(),
        };
        if tree.watchdog_draft.ai_provider == AiCliProvider::RuumCloud {
            tree.sync_selected_cloud_model();
        }
        if !cfg!(test) {
            Self::probe_system_statuses(cx);
            tree.local_docker_probing = true;
            Self::refresh_local_docker(cx);
        }
        if local && start {
            tree.device = device_for(&tree.root_path);
            tree.full_disk_access = tree
                .home
                .as_deref()
                .and_then(ruum_core::access::full_disk_access);
            // The disk of what is on screen, as `set_root` keeps it: `ruum
            // /Volumes/Ext` then `g` measures that drive, like opening it with ⌘O.
            tree.disk_root = volume_root_for(&tree.root_path);
            tree.file_table = file_table_readable(&tree.root_path);
            ruum_core::removal::prime_mount_points();
        }
        if start {
            tree.start_scan(cx);
        } else {
            tree.open_volumes(cx);
        }
        Self::start_space_ticker(cx);
        tree
    }

    /// Build a view over a tree that is already known.
    ///
    /// Mirrors [`Self::new`] without starting a walk, so a test can hand the
    /// screens a tree it already has instead of waiting on a real one. A
    /// feature that opens an archived scan would use this too.
    #[cfg(test)]
    pub fn with_tree(
        root_path: PathBuf,
        tree: Node,
        options: ScanOptions,
        depth: u32,
        cx: &mut Context<'_, Self>,
    ) -> Self {
        let mut app = Self::new(root_path, options, depth, cx);
        // `new` started a walk; this tree stands in for its result.
        if let Some(scan) = app.scan.take() {
            scan.cancel();
        }
        app.scan_epoch += 1;
        app.marks.refresh(&app.root_path, &tree, app.options.metric);
        app.tree = Some(Arc::new(tree));
        app.cache = None;
        app.refresh_insights();
        app.select_largest(cx);
        app
    }

    /// Scan a different root from scratch. Marks are kept; a mark outside
    /// the new root is shown as kept back, never removed.
    pub fn set_root(&mut self, root: PathBuf, cx: &mut Context<'_, Self>) {
        if self.remote.is_some() {
            self.root_path = root;
            self.remote_disk = false;
            self.start_scan(cx);
            return;
        }
        self.space = space_info(&root).ok();
        self.device = device_for(&root);
        // "Whole disk" means the disk of what is on screen, which a new root
        // can change: an external drive has its own.
        self.disk_root =
            volume_root_for(&root).or_else(|| self.disk_root.take());
        self.file_table = file_table_readable(&root);
        self.root_path = root;
        self.screen = Screen::Explore;
        self.start_scan(cx);
    }

    /// Go up to `above`, a directory containing the scanned root.
    ///
    /// Memoized: the tree already measured is handed to the walk and reused
    /// where it is reached, so only what is new at the wider level is read,
    /// and the current view stays on screen until the wider tree lands.
    pub fn widen_to(&mut self, above: PathBuf, cx: &mut Context<'_, Self>) {
        if self.remote.is_some() {
            self.set_root(above, cx);
            return;
        }
        let Some(tree) = self.tree.clone() else {
            self.set_root(above, cx);
            return;
        };
        if !self.root_path.starts_with(&above) || self.root_path == above {
            return;
        }
        if let Some(scan) = &self.scan {
            scan.cancel();
        }
        self.scan_epoch += 1;
        let epoch = self.scan_epoch;
        self.progress = ScanSnapshot::default();
        self.scan_error = None;
        self.scan_started = Some(Instant::now());
        self.scan_elapsed = None;
        self.scan_root.clone_from(&above);
        self.remember();
        let known = Known {
            path: self.root_path.clone(),
            tree,
        };
        self.scan = Some(ScanHandle::spawn_with(
            above,
            self.options.clone(),
            Some(known),
        ));
        Self::poll_scan(epoch, cx);
        cx.notify();
    }

    /// Open the configured `ruum.db` database, creating it if needed.
    pub(crate) fn open_db(&self) -> std::io::Result<StorageDb> {
        let path = self.db_path.as_deref().ok_or_else(|| {
            std::io::Error::new(
                std::io::ErrorKind::NotFound,
                "no database path configured",
            )
        })?;
        StorageDb::open(path)
    }

    /// Open `ruum.db` only when it already exists on disk (read-only pages).
    fn open_existing_db(&self) -> Option<StorageDb> {
        let path = self.db_path.as_deref().filter(|p| p.is_file())?;
        StorageDb::open(path).ok()
    }

    pub(crate) fn ensure_cred_store(
        &mut self,
    ) -> Result<LazyCredentialStore, String> {
        if let Some(ref store) = self.cred_store {
            return Ok(store.clone());
        }
        if let Some(ref db_path) = self.db_path
            && let Some(dir) = db_path.parent()
            && !dir.as_os_str().is_empty()
        {
            let store = LazyCredentialStore::platform(dir);
            self.cred_store = Some(store.clone());
            return Ok(store);
        }
        Err("no credential store configured".to_owned())
    }

    /// Reload saved SSH server cards from `ruum.db` (metadata only, zero keychain).
    pub fn reload_ssh_profiles(&mut self) {
        if let Some(db) = self.open_existing_db()
            && let Ok(profiles) = db.list_ssh_profiles()
        {
            self.ssh_profiles = profiles;
        }
    }

    /// Enqueue an MCP remote SSH connection confirmation request.
    pub fn enqueue_remote_connect_confirm(
        &mut self,
        request: RemoteConnectConfirmRequest,
        responder: Option<
            std::sync::mpsc::Sender<RemoteConnectConfirmDecision>,
        >,
        response_file: Option<PathBuf>,
        cx: &mut Context<'_, Self>,
    ) {
        let modal =
            RemoteConnectConfirmModal::new(request, responder, response_file);
        if self.remote_connect_confirm.is_none() {
            self.remote_connect_confirm = Some(modal);
            self.focus_request = Some(FocusTarget::RemoteConnectDialog);
        } else {
            self.remote_connect_confirm_queue.push_back(modal);
        }
        cx.notify();
    }

    /// Approve the active remote SSH connection modal and promote any queued next request.
    pub fn approve_remote_connect(&mut self, cx: &mut Context<'_, Self>) {
        if let Some(mut modal) = self.remote_connect_confirm.take() {
            let _ = modal.approve();
        }
        self.advance_remote_connect_queue(cx);
    }

    /// Deny the active remote SSH connection modal and promote any queued next request.
    pub fn deny_remote_connect(&mut self, cx: &mut Context<'_, Self>) {
        if let Some(mut modal) = self.remote_connect_confirm.take() {
            let _ = modal.deny();
        }
        self.advance_remote_connect_queue(cx);
    }

    fn advance_remote_connect_queue(&mut self, cx: &mut Context<'_, Self>) {
        if let Some(next) = self.remote_connect_confirm_queue.pop_front() {
            self.remote_connect_confirm = Some(next);
            self.focus_request = Some(FocusTarget::RemoteConnectDialog);
        } else {
            self.focus_request = Some(FocusTarget::Root);
        }
        cx.notify();
    }

    /// Switch primary screen from the icon rail or app menu shortcuts.
    pub fn navigate_rail(
        &mut self,
        target: Screen,
        cx: &mut Context<'_, Self>,
    ) {
        if self.run.is_some()
            || matches!(self.screen, Screen::Review | Screen::Running)
        {
            return;
        }
        if target == Screen::Docker && !self.has_local_docker_daemon() {
            return;
        }
        if target != Screen::Docker {
            self.pending_open_docker = false;
        }
        if self.screen != target {
            self.notice = None;
            self.toast_epoch += 1;
        }
        self.crumb_menu = None;
        self.power_menu = None;
        self.show_help = false;
        self.remote_docker_modal_open = false;
        if target == Screen::Remote {
            self.volume_epoch += 1;
            self.volumes_loading = false;
            self.volumes_open = false;
            self.volume_picker_remote_profile_id = None;
            self.picker_error = None;
            self.screen = Screen::Remote;
            self.reload_ssh_profiles();
            self.focus_request = Some(FocusTarget::Root);
            cx.notify();
            return;
        }
        if self.volumes_open || self.screen == Screen::Remote {
            self.volume_epoch += 1;
            self.volumes_loading = false;
            self.volumes_open = false;
            self.volume_picker_remote_profile_id = None;
            self.picker_error = None;
            self.ssh_form_open = false;
            self.ssh_form_error = None;
        }
        self.screen = target;
        if target == Screen::Docker {
            self.ensure_local_docker_initialized();
            if !cfg!(test) && !self.local_docker_probing {
                self.local_docker_probing = true;
                Self::refresh_local_docker(cx);
            }
        } else if target == Screen::Settings {
            self.reload_watchdog_config_and_prepare_settings();
            if !cfg!(test) {
                Self::probe_system_statuses(cx);
            }
        } else if target == Screen::Watchdog {
            self.reload_watchdog_state();
            self.refresh_watchdog_home_space_silent();
            if !cfg!(test) {
                Self::probe_system_statuses(cx);
            }
        }
        self.focus_request = Some(FocusTarget::Root);
        cx.notify();
    }

    /// Apply an external screen-open request (`--open-screen`, menu, tray, or
    /// notification), deferring `Screen::Docker` until a running local probe
    /// resolves as `Available`.
    pub(crate) fn apply_target_screen(
        &mut self,
        screen: Screen,
        cx: &mut Context<'_, Self>,
    ) {
        if screen == Screen::Docker && !self.has_local_docker_daemon() {
            if self.local_docker_probing {
                self.pending_open_docker = true;
            }
            return;
        }
        self.navigate_rail(screen, cx);
        if screen == Screen::Watchdog {
            self.reload_watchdog_state();
        }
    }

    /// `V` / breadcrumb root click: open the directory & mount picker modal
    /// in the current window aligned with the active host tab (local or SSH).
    pub fn open_volumes(&mut self, cx: &mut Context<'_, Self>) {
        if self.run.is_some()
            || matches!(self.screen, Screen::Review | Screen::Running)
        {
            return;
        }
        self.remote_docker_modal_open = false;
        match self.active_host_tab.clone() {
            HostTabKey::Local => {
                self.crumb_menu = None;
                self.power_menu = None;
                self.show_help = false;
                self.volumes_open = true;
                self.volume_picker_remote_profile_id = None;
                self.picker_path = None;
                self.picker_error = None;
                self.focus_request = Some(FocusTarget::Dialog);
                self.load_local_volumes(cx);
            }
            HostTabKey::Ssh(id) => {
                self.open_remote_volumes_modal(&id, cx);
            }
        }
    }

    /// Open the directory & mount picker modal for a remote SSH profile using
    /// read-only `RemoteRoots::discover_with_session` (without probing/scanning).
    pub fn open_remote_volumes_modal(
        &mut self,
        id: &str,
        cx: &mut Context<'_, Self>,
    ) {
        if self.run.is_some()
            || matches!(self.screen, Screen::Review | Screen::Running)
        {
            return;
        }
        let Some(profile) =
            self.ssh_profiles.iter().find(|p| p.id == id).cloned()
        else {
            return;
        };
        self.crumb_menu = None;
        self.power_menu = None;
        self.show_help = false;
        self.remote_docker_modal_open = false;
        self.volumes_open = true;
        self.volume_picker_remote_profile_id = Some(profile.id.clone());
        self.picker_path = None;
        self.picker_error = None;
        self.volumes.clear();
        self.volume_highlight = 0;
        self.volumes_error = None;
        self.volumes_loading = true;
        self.volume_epoch += 1;
        let epoch = self.volume_epoch;
        self.focus_request = Some(FocusTarget::Dialog);
        cx.notify();

        let store = match self.ensure_cred_store() {
            Ok(store) => store,
            Err(err) => {
                self.volumes_loading = false;
                self.volumes_error = Some(err);
                cx.notify();
                return;
            }
        };
        let existing_session = self.find_live_ssh_session(id);
        let duration_minutes = self.app_settings.ssh_session_duration_minutes;
        let profile_id = profile.id.clone();

        let task = cx.background_executor().spawn(async move {
            let session = match existing_session {
                Some(s) if !s.is_closed() && !s.is_expired() => s,
                _ => Arc::new(start_ssh_session_with_store(
                    &profile,
                    &store,
                    duration_minutes,
                )?),
            };
            let roots =
                ruum_core::remote::RemoteRoots::discover_with_session(&session)
                    .map_err(|err| err.to_string())?
                    .wait()
                    .map_err(|err| err.to_string())?;
            Ok::<_, String>((session, roots))
        });

        cx.spawn(async move |this, cx| {
            let res = task.await;
            let _ = this.update(cx, |this, cx| {
                if !this.volumes_open
                    || this.volume_epoch != epoch
                    || this.volume_picker_remote_profile_id.as_deref()
                        != Some(profile_id.as_str())
                {
                    return;
                }
                match res {
                    Ok((session, roots)) => {
                        this.store_live_ssh_session(&profile_id, &session);
                        this.finish_volume_listing(
                            Ok((roots.volumes, roots.home)),
                            cx,
                        );
                    }
                    Err(err) => {
                        this.volumes_loading = false;
                        this.volumes_error = Some(err);
                        cx.notify();
                    }
                }
            });
        })
        .detach();
    }

    /// Discover local mounts and volumes without probing or depending on any
    /// active remote SSH session.
    pub fn load_local_volumes(&mut self, cx: &mut Context<'_, Self>) {
        if !self.volumes_open {
            return;
        }
        self.volume_epoch += 1;
        let epoch = self.volume_epoch;
        self.volumes.clear();
        self.volume_highlight = 0;
        self.volumes_error = None;
        self.picker_error = None;
        self.volumes_loading = true;
        let task = cx.background_executor().spawn(async {
            ruum_core::space::volume_roots()
                .map(|volumes| (volumes, std::env::home_dir()))
        });
        cx.spawn(async move |this, cx| {
            let result = task.await;
            let _ = this.update(cx, |this, cx| {
                if this.volumes_open && this.volume_epoch == epoch {
                    this.finish_volume_listing(result, cx);
                }
            });
        })
        .detach();
        cx.notify();
    }

    pub(crate) fn finish_volume_listing(
        &mut self,
        result: std::io::Result<(
            Vec<ruum_core::space::Volume>,
            Option<PathBuf>,
        )>,
        cx: &mut Context<'_, Self>,
    ) {
        self.volumes_loading = false;
        let locale = self.locale;
        match result {
            Ok((mut volumes, home)) => {
                if let Some(home) = home
                    && !volumes.iter().any(|volume| volume.point == home)
                {
                    volumes.insert(
                        0,
                        ruum_core::space::Volume {
                            point: home,
                            device: Some(
                                crate::i18n::messages::home_directory(locale)
                                    .into(),
                            ),
                            space: None,
                        },
                    );
                }
                self.volumes = volumes;
            }
            Err(error) => self.volumes_error = Some(error.to_string()),
        }
        cx.notify();
    }

    pub fn prepare_picker_inputs(
        &mut self,
        window: &mut Window,
        cx: &mut Context<'_, Self>,
    ) {
        let locale = self.locale;
        if self.picker_path.is_none() {
            let path = cx.new(|cx| {
                InputState::new(window, cx).placeholder(
                    crate::i18n::messages::placeholder_path_or_tilde(locale),
                )
            });
            cx.subscribe(&path, |this, _, event, cx| {
                if matches!(event, InputEvent::PressEnter { .. }) {
                    this.choose_picker_path(cx);
                }
            })
            .detach();
            self.picker_path = Some(path);
        }
    }

    /// Whether any text input currently owns keyboard focus.
    pub fn is_editing(&self, window: &Window, cx: &App) -> bool {
        let inputs = [
            &self.picker_path,
            &self.ssh_input_alias,
            &self.ssh_input_host,
            &self.ssh_input_port,
            &self.ssh_input_username,
            &self.ssh_input_default_path,
            &self.ssh_input_password,
            &self.ssh_input_private_key,
            &self.ssh_input_passphrase,
            &self.setting_input_default_depth,
            &self.setting_input_ssh_duration,
            &self.watchdog_input_min_free,
            &self.watchdog_input_poll_interval,
            &self.watchdog_input_cooldown,
            &self.webhook_input_name,
            &self.setting_input_webhook_url,
            &self.setting_input_webhook_timeout,
            &self.setting_input_webhook_headers,
            &self.watchdog_input_custom_cmd,
            &self.watchdog_input_extra_args,
            &self.watchdog_input_model,
            &self.watchdog_input_api_base_url,
            &self.watchdog_input_api_key,
            &self.cloud_redeem_input_settings,
        ];
        inputs
            .into_iter()
            .flatten()
            .any(|input| input.read(cx).focus_handle(cx).is_focused(window))
    }

    /// Probe OS background service and CLI registration statuses off the UI
    /// thread. Local Docker is probed concurrently via [`Self::refresh_local_docker`].
    pub fn probe_system_statuses(cx: &Context<'_, Self>) {
        if cfg!(test) {
            return;
        }
        let task = cx.background_executor().spawn(async {
            let runner = ruum_core::ai_cli::TimedCommandRunner::new(
                Duration::from_secs(5),
            );
            let service_status =
                ruum_core::watchdog::service_status(&runner).ok();
            let cli_status = std::env::current_exe().ok().and_then(|exe| {
                ruum_core::cli_install::cli_status(&runner, &exe).ok()
            });
            (service_status, cli_status)
        });
        cx.spawn(async move |this, cx| {
            let (service_status, cli_status) = task.await;
            let _ = this.update(cx, |this, cx| {
                this.watchdog_service_status = service_status;
                this.cli_install_status = cli_status;
                cx.notify();
            });
        })
        .detach();
    }

    /// Refresh local Docker daemon & usage status off the UI thread.
    pub fn refresh_local_docker(cx: &Context<'_, Self>) {
        if cfg!(test) {
            return;
        }
        let task = cx.background_executor().spawn(async {
            DockerFactory::create().probe(&DockerEnv::current())
        });
        cx.spawn(async move |this, cx| {
            let status = task.await;
            let _ = this.update(cx, |this, cx| {
                this.local_docker_probing = false;
                this.apply_local_docker_status(status);
                cx.notify();
            });
        })
        .detach();
    }

    /// Refresh local Docker status from `Screen::Docker` (uses mock runner
    /// under tests).
    pub fn refresh_local_docker_now(&mut self, cx: &mut Context<'_, Self>) {
        if self.local_docker_probing {
            return;
        }
        self.local_docker_probing = true;
        cx.notify();
        #[cfg(test)]
        let runner = self.docker_runner.clone();
        let task = cx.background_executor().spawn(async move {
            #[cfg(test)]
            {
                runner.map(|mock| {
                    DockerFactory::create()
                        .probe_binary_with(Path::new("docker"), mock.as_ref())
                })
            }
            #[cfg(not(test))]
            {
                Some(DockerFactory::create().probe(&DockerEnv::current()))
            }
        });
        cx.spawn(async move |this, cx| {
            let fresh = task.await;
            let _ = this.update(cx, |this, cx| {
                this.local_docker_probing = false;
                if let Some(status) = fresh {
                    this.apply_local_docker_status(status);
                }
                cx.notify();
            });
        })
        .detach();
    }

    /// Refresh Docker metrics for the active remote SSH tab via a remote scan.
    pub fn refresh_remote_docker(&mut self, cx: &mut Context<'_, Self>) {
        if self.remote.is_none() || self.docker_probing {
            return;
        }
        self.docker_probing = true;
        self.start_scan(cx);
        if self.remote_scan.is_none() {
            self.docker_probing = false;
            if let Some(err) = self.scan_error.clone() {
                self.push_toast(err, Status::Error, cx);
            }
        }
        cx.notify();
    }

    /// Return whether the local Docker daemon is currently running and probed
    /// as [`DockerProbeStatus::Available`].
    #[must_use]
    pub const fn has_local_docker_daemon(&self) -> bool {
        matches!(self.local_docker, DockerProbeStatus::Available(_))
    }

    pub fn apply_local_docker_status(&mut self, status: DockerProbeStatus) {
        self.local_docker = status;
        self.sync_local_docker_defaults();
        if self.has_local_docker_daemon() {
            if std::mem::take(&mut self.pending_open_docker)
                && self.run.is_none()
                && !matches!(self.screen, Screen::Review | Screen::Running)
            {
                self.crumb_menu = None;
                self.power_menu = None;
                self.show_help = false;
                self.remote_docker_modal_open = false;
                self.volume_epoch += 1;
                self.volumes_loading = false;
                self.volumes_open = false;
                self.volume_picker_remote_profile_id = None;
                self.picker_error = None;
                self.ssh_form_open = false;
                self.ssh_form_error = None;
                self.screen = Screen::Docker;
                self.focus_request = Some(FocusTarget::Root);
            }
        } else {
            self.pending_open_docker = false;
            if self.screen == Screen::Docker {
                self.screen = Screen::Explore;
                self.focus_request = Some(FocusTarget::Root);
            }
        }
    }

    #[must_use]
    pub fn local_docker_status(&self) -> DockerProbeStatus {
        self.local_docker.clone()
    }

    /// Open the centered remote Docker inspection & script modal for the active
    /// remote SSH tab.
    pub fn open_remote_docker_modal(&mut self, cx: &mut Context<'_, Self>) {
        if self.remote.is_none()
            || !matches!(self.docker, DockerProbeStatus::Available(_))
            || self.run.is_some()
            || self.volumes_open
            || matches!(self.screen, Screen::Review | Screen::Running)
        {
            return;
        }
        self.crumb_menu = None;
        self.power_menu = None;
        self.show_help = false;
        self.ensure_review_docker_initialized();
        self.remote_docker_modal_open = true;
        self.focus_request = Some(FocusTarget::Dialog);
        cx.notify();
    }

    /// Close the remote Docker modal and return focus to the root view.
    pub fn close_remote_docker_modal(&mut self, cx: &mut Context<'_, Self>) {
        if !self.remote_docker_modal_open {
            return;
        }
        self.remote_docker_modal_open = false;
        self.focus_request = Some(FocusTarget::Root);
        cx.notify();
    }

    #[must_use]
    pub fn local_docker_items(
        &self,
    ) -> (Vec<DockerConfirmItem>, Option<String>) {
        build_docker_confirm_items(&self.local_docker, &[], self.locale)
    }

    pub fn sync_local_docker_defaults(&mut self) {
        sync_docker_defaults_for(
            &self.local_docker,
            &mut self.local_docker_checked,
            &mut self.local_docker_initialized,
            self.locale,
        );
    }

    pub fn ensure_local_docker_initialized(&mut self) {
        if !self.local_docker_initialized {
            self.sync_local_docker_defaults();
        }
    }

    pub fn toggle_local_docker_target(&mut self, target: &DockerCleanTarget) {
        self.ensure_local_docker_initialized();
        toggle_docker_target_in(&mut self.local_docker_checked, target);
    }

    pub fn select_all_local_docker(&mut self) {
        self.ensure_local_docker_initialized();
        select_all_docker_in(
            &self.local_docker,
            &mut self.local_docker_checked,
            self.locale,
        );
    }

    pub fn select_none_local_docker(&mut self) {
        self.local_docker_checked.clear();
        self.local_docker_initialized = true;
    }

    pub fn toggle_local_docker_group(&mut self, group: DockerGroupKind) {
        self.ensure_local_docker_initialized();
        toggle_docker_group_in(
            &self.local_docker,
            &mut self.local_docker_checked,
            group,
            self.locale,
        );
    }

    #[must_use]
    pub fn selected_local_docker_targets(&self) -> Vec<DockerCleanTarget> {
        selected_docker_targets_in(
            &self.local_docker,
            &self.local_docker_checked,
        )
    }

    #[must_use]
    pub fn selected_local_docker_bytes(&self) -> u64 {
        selected_docker_bytes_in(
            &self.local_docker,
            &self.local_docker_checked,
            self.locale,
        )
    }

    #[must_use]
    pub fn review_docker_items(
        &self,
    ) -> (Vec<DockerConfirmItem>, Option<String>) {
        build_docker_confirm_items(&self.docker, &[], self.locale)
    }

    pub fn sync_review_docker_defaults(&mut self) {
        sync_docker_defaults_for(
            &self.docker,
            &mut self.review_docker_checked,
            &mut self.review_docker_initialized,
            self.locale,
        );
    }

    pub fn ensure_review_docker_initialized(&mut self) {
        if !self.review_docker_initialized {
            self.sync_review_docker_defaults();
        }
    }

    pub fn toggle_review_docker_target(&mut self, target: &DockerCleanTarget) {
        self.ensure_review_docker_initialized();
        toggle_docker_target_in(&mut self.review_docker_checked, target);
    }

    pub fn select_all_review_docker(&mut self) {
        self.ensure_review_docker_initialized();
        select_all_docker_in(
            &self.docker,
            &mut self.review_docker_checked,
            self.locale,
        );
    }

    pub fn select_none_review_docker(&mut self) {
        self.review_docker_checked.clear();
        self.review_docker_initialized = true;
    }

    pub fn toggle_review_docker_group(&mut self, group: DockerGroupKind) {
        self.ensure_review_docker_initialized();
        toggle_docker_group_in(
            &self.docker,
            &mut self.review_docker_checked,
            group,
            self.locale,
        );
    }

    #[must_use]
    pub fn selected_review_docker_targets(&self) -> Vec<DockerCleanTarget> {
        selected_docker_targets_in(&self.docker, &self.review_docker_checked)
    }

    #[must_use]
    pub fn selected_review_docker_bytes(&self) -> u64 {
        selected_docker_bytes_in(
            &self.docker,
            &self.review_docker_checked,
            self.locale,
        )
    }

    /// Re-read watchdog persistent state and latest AI proposal from `ruum.db`.
    pub fn reload_watchdog_state(&mut self) {
        if let Some(db) = self.open_existing_db() {
            if let Ok(wd) = db.watchdog()
                && let Ok(st) = wd.load_state()
            {
                self.watchdog_space_checked_at = match (
                    self.watchdog_space_checked_at,
                    st.last_checked_secs,
                ) {
                    (Some(cur), Some(db_ts)) => Some(cur.max(db_ts)),
                    (cur, db_ts) => cur.or(db_ts),
                };
                self.watchdog_state = st;
                self.watchdog_state.last_checked_secs =
                    self.watchdog_space_checked_at;
            }
            if let Ok(ai_st) = db.load_ai_state() {
                self.ai_proposal = ai_st.parse_proposal().ok().flatten();
                self.ai_last_error = ai_st.last_error;
                self.ai_last_error_at_secs = ai_st.last_error_at_secs;
            }
            if let Ok(cloud_st) = db.load_cloud_state() {
                self.cloud_state = cloud_st;
                if self.watchdog_draft.ai_provider == AiCliProvider::RuumCloud {
                    self.sync_selected_cloud_model();
                }
            }
        }
    }

    /// Switch the active left-sidebar category on the Settings page without
    /// discarding unsaved draft edits.
    pub fn set_settings_category(
        &mut self,
        category: SettingsCategory,
        cx: &mut Context<'_, Self>,
    ) {
        self.settings_category = category;
        cx.notify();
    }

    /// Toggle the Webhook Setup Tutorial & JSON Payload Guide card.
    pub fn toggle_webhook_guide(&mut self, cx: &mut Context<'_, Self>) {
        self.webhook_guide_open = !self.webhook_guide_open;
        cx.notify();
    }

    /// Reload saved webhook endpoints from `ruum.db`.
    pub fn reload_webhooks(&mut self) {
        if let Some(db) = self.open_existing_db() {
            if let Ok(list) = db.list_webhooks() {
                self.webhooks = list;
            }
            if let Ok(wh) = db.load_webhook_config() {
                self.webhook_config = wh;
            }
        }
    }

    /// Open the inline webhook form to add a new endpoint.
    pub fn open_add_webhook_form(&mut self, cx: &mut Context<'_, Self>) {
        self.webhook_form_open = true;
        self.webhook_form_editing_id = None;
        self.webhook_form_enabled = true;
        self.webhook_form_error = None;
        self.webhook_form_needs_sync = true;
        cx.notify();
    }

    /// Open the inline webhook form populated from an existing endpoint.
    pub fn open_edit_webhook_form(
        &mut self,
        id: &str,
        cx: &mut Context<'_, Self>,
    ) {
        let Some(ep) = self.webhooks.iter().find(|w| w.id == id).cloned()
        else {
            return;
        };
        self.webhook_form_open = true;
        self.webhook_form_editing_id = Some(ep.id);
        self.webhook_form_enabled = ep.enabled;
        self.webhook_form_error = None;
        self.webhook_form_needs_sync = true;
        cx.notify();
    }

    /// Close the inline webhook form without saving.
    pub fn close_webhook_form(&mut self, cx: &mut Context<'_, Self>) {
        self.webhook_form_open = false;
        self.webhook_form_editing_id = None;
        self.webhook_form_error = None;
        self.webhook_form_needs_sync = false;
        cx.notify();
    }

    /// Sync inline webhook form text inputs from the editing endpoint or defaults.
    pub fn sync_webhook_form_inputs(
        &self,
        window: &mut Window,
        cx: &mut Context<'_, Self>,
    ) {
        let ep = self
            .webhook_form_editing_id
            .as_deref()
            .and_then(|id| self.webhooks.iter().find(|w| w.id == id));
        let (name, url, timeout, headers) = if let Some(ep) = ep {
            (
                ep.name.clone(),
                ep.url.clone(),
                ep.timeout_secs.to_string(),
                ep.headers_json.clone(),
            )
        } else {
            (
                String::new(),
                String::new(),
                ruum_core::storage::DEFAULT_WEBHOOK_TIMEOUT_SECS.to_string(),
                "{}".to_owned(),
            )
        };
        let pairs = [
            (&self.webhook_input_name, name),
            (&self.setting_input_webhook_url, url),
            (&self.setting_input_webhook_timeout, timeout),
            (&self.setting_input_webhook_headers, headers),
        ];
        for (slot, val) in pairs {
            if let Some(input) = slot {
                input.update(cx, |inp, cx| inp.set_value(val, window, cx));
            }
        }
    }

    /// Validate and save the inline webhook endpoint form to `ruum.db`.
    pub fn save_webhook_form(&mut self, cx: &mut Context<'_, Self>) {
        let name = self
            .webhook_input_name
            .as_ref()
            .map_or_else(String::new, |input| {
                input.read(cx).value().trim().to_owned()
            });
        let url = self
            .setting_input_webhook_url
            .as_ref()
            .map_or_else(String::new, |input| {
                input.read(cx).value().trim().to_owned()
            });
        let timeout_raw =
            self.setting_input_webhook_timeout.as_ref().map_or_else(
                || ruum_core::storage::DEFAULT_WEBHOOK_TIMEOUT_SECS.to_string(),
                |input| input.read(cx).value().trim().to_owned(),
            );
        let Ok(timeout_secs) = timeout_raw.parse::<u64>() else {
            self.webhook_form_error =
                Some(crate::i18n::messages::invalid_webhook_config(
                    "timeout must be a positive integer",
                    self.locale,
                ));
            cx.notify();
            return;
        };
        if timeout_secs == 0 {
            self.webhook_form_error =
                Some(crate::i18n::messages::invalid_webhook_config(
                    "timeout must be a positive integer",
                    self.locale,
                ));
            cx.notify();
            return;
        }
        let headers_json =
            self.setting_input_webhook_headers.as_ref().map_or_else(
                || "{}".to_owned(),
                |input| {
                    let text = input.read(cx).value().trim().to_owned();
                    if text.is_empty() {
                        "{}".to_owned()
                    } else {
                        text
                    }
                },
            );
        let now_secs = u64::try_from(now_seconds()).unwrap_or(0);
        let editing_id = self.webhook_form_editing_id.clone();
        let existing = editing_id
            .as_deref()
            .and_then(|id| self.webhooks.iter().find(|w| w.id == id))
            .cloned();
        let candidate = WebhookEndpoint {
            id: existing.as_ref().map_or_else(String::new, |w| w.id.clone()),
            name,
            enabled: self.webhook_form_enabled,
            url,
            headers_json,
            timeout_secs,
            created_at_secs: existing
                .as_ref()
                .map_or(now_secs, |w| w.created_at_secs),
            updated_at_secs: now_secs,
        };
        let validated = match candidate.validated() {
            Ok(v) => v,
            Err(err) => {
                self.webhook_form_error =
                    Some(crate::i18n::messages::invalid_webhook_config(
                        &err.to_string(),
                        self.locale,
                    ));
                cx.notify();
                return;
            }
        };

        let saved_name = validated.name.clone();
        if self.db_path.is_some() {
            let res = self.open_db().and_then(|db| {
                if existing.is_some() {
                    db.update_webhook(&validated)
                } else {
                    db.add_webhook(&validated)
                }
            });
            match res {
                Ok(_) => self.reload_webhooks(),
                Err(err) => {
                    self.webhook_form_error =
                        Some(crate::i18n::messages::webhook_save_failed(
                            &err.to_string(),
                            self.locale,
                        ));
                    cx.notify();
                    return;
                }
            }
        } else if let Some(slot) = editing_id
            .as_deref()
            .and_then(|id| self.webhooks.iter_mut().find(|w| w.id == id))
        {
            *slot = validated;
        } else {
            let mut inserted = validated;
            inserted.id = format!("wh-{}", self.webhooks.len() + 1);
            self.webhooks.push(inserted);
        }

        self.webhook_form_open = false;
        self.webhook_form_editing_id = None;
        self.webhook_form_error = None;
        self.webhook_form_needs_sync = false;
        self.push_toast(
            crate::i18n::messages::webhook_saved(&saved_name, self.locale),
            Status::Success,
            cx,
        );
    }

    /// Toggle whether a saved webhook endpoint is enabled.
    pub fn toggle_webhook_enabled(
        &mut self,
        id: &str,
        cx: &mut Context<'_, Self>,
    ) {
        let Some(mut ep) = self.webhooks.iter().find(|w| w.id == id).cloned()
        else {
            return;
        };
        ep.enabled = !ep.enabled;
        ep.updated_at_secs = u64::try_from(now_seconds()).unwrap_or(0);
        if self.db_path.is_some() {
            match self.open_db().and_then(|db| db.update_webhook(&ep)) {
                Ok(_) => self.reload_webhooks(),
                Err(err) => {
                    self.push_toast(
                        crate::i18n::messages::webhook_save_failed(
                            &err.to_string(),
                            self.locale,
                        ),
                        Status::Error,
                        cx,
                    );
                    return;
                }
            }
        } else if let Some(slot) = self.webhooks.iter_mut().find(|w| w.id == id)
        {
            *slot = ep;
        }
        cx.notify();
    }

    /// Delete a saved webhook endpoint from `ruum.db`.
    pub fn delete_webhook_endpoint(
        &mut self,
        id: &str,
        cx: &mut Context<'_, Self>,
    ) {
        let name = self
            .webhooks
            .iter()
            .find(|w| w.id == id)
            .map_or_else(|| id.to_owned(), |w| w.name.clone());
        if self.db_path.is_some() {
            match self.open_db().and_then(|db| db.delete_webhook(id)) {
                Ok(_) => self.reload_webhooks(),
                Err(err) => {
                    self.push_toast(
                        crate::i18n::messages::webhook_save_failed(
                            &err.to_string(),
                            self.locale,
                        ),
                        Status::Error,
                        cx,
                    );
                    return;
                }
            }
        } else {
            self.webhooks.retain(|w| w.id != id);
        }
        if self.webhook_form_editing_id.as_deref() == Some(id) {
            self.close_webhook_form(cx);
        }
        self.push_toast(
            crate::i18n::messages::webhook_deleted(&name, self.locale),
            Status::Success,
            cx,
        );
    }

    /// Send a test POST payload to a configured webhook endpoint off UI thread.
    pub fn test_webhook_endpoint_now(
        &mut self,
        id: &str,
        cx: &mut Context<'_, Self>,
    ) {
        let Some(ep) = self.webhooks.iter().find(|w| w.id == id).cloned()
        else {
            return;
        };
        if self.webhook_testing_ids.contains(&ep.id) {
            return;
        }
        let ep_id = ep.id.clone();
        let ep_name = ep.name.clone();
        self.webhook_testing_ids.insert(ep_id.clone());
        cx.notify();

        let task = cx.background_executor().spawn(async move {
            ruum_core::webhook::test_webhook_endpoint(&ep)
        });
        cx.spawn(async move |this, cx| {
            let res = task.await;
            let _ = this.update(cx, |this, cx| {
                this.webhook_testing_ids.remove(&ep_id);
                let locale = this.locale;
                match res {
                    Ok(()) => {
                        this.push_toast(
                            crate::i18n::messages::webhook_test_succeeded(
                                &ep_name, locale,
                            ),
                            Status::Success,
                            cx,
                        );
                    }
                    Err(err) => {
                        this.push_toast(
                            crate::i18n::messages::webhook_test_failed(
                                &ep_name,
                                &err.to_string(),
                                locale,
                            ),
                            Status::Error,
                            cx,
                        );
                    }
                }
            });
        })
        .detach();
    }

    /// Reload all Settings sections (`app_settings`, `watchdog_config`,
    /// `webhook_endpoints`, `ai_config`) from `ruum.db`, reset draft, and
    /// schedule text input synchronization.
    pub fn reload_watchdog_config_and_prepare_settings(&mut self) {
        if let Some(db) = self.open_existing_db() {
            if let Ok(app_s) = db.load_app_settings() {
                self.app_settings = app_s;
            }
            if let Ok(wd) = db.watchdog()
                && let Ok(cfg) = wd.load_config()
            {
                self.watchdog_config = cfg;
            }
            if let Ok(wh) = db.load_webhook_config() {
                self.webhook_config = wh;
            }
            if let Ok(list) = db.list_webhooks() {
                self.webhooks = list;
            }
            if let Ok(ai) = db.load_ai_config() {
                self.ai_config = ai;
            }
            if let Ok(cloud) = db.load_cloud_state() {
                self.cloud_state = cloud;
            }
        }
        self.watchdog_draft = WatchdogSettingsDraft::from_configs(
            &self.app_settings,
            &self.watchdog_config,
            &self.ai_config,
        );
        if self.watchdog_draft.ai_provider == AiCliProvider::RuumCloud {
            self.sync_selected_cloud_model();
        }
        self.watchdog_inputs_need_sync = true;
    }

    /// Write all Settings text inputs from committed config structs.
    pub fn sync_watchdog_inputs_from_config(
        &self,
        window: &mut Window,
        cx: &mut Context<'_, Self>,
    ) {
        let default_depth = self.app_settings.default_depth.to_string();
        let ssh_duration =
            self.app_settings.ssh_session_duration_minutes.to_string();
        let min_free = self.watchdog_config.min_free_gb.to_string();
        let poll_interval = self
            .watchdog_config
            .poll_interval_secs
            .div_ceil(60)
            .max(1)
            .to_string();
        let min_cooldown_mins = MIN_COOLDOWN_SECS / 60;
        let cooldown = self
            .watchdog_config
            .cooldown_secs
            .div_ceil(60)
            .max(min_cooldown_mins)
            .to_string();
        let custom_cmd = self.ai_config.custom_command.clone();
        let extra_args = self.ai_config.extra_args.clone();
        let model = self.ai_config.model.clone().unwrap_or_default();
        let api_base_url = self.ai_config.api_base_url.clone();
        let api_placeholder = self.ai_config.api_protocol.default_base_url();

        let pairs = [
            (&self.setting_input_default_depth, default_depth),
            (&self.setting_input_ssh_duration, ssh_duration),
            (&self.watchdog_input_min_free, min_free),
            (&self.watchdog_input_poll_interval, poll_interval),
            (&self.watchdog_input_cooldown, cooldown),
            (&self.watchdog_input_custom_cmd, custom_cmd),
            (&self.watchdog_input_extra_args, extra_args),
            (&self.watchdog_input_model, model),
            (&self.watchdog_input_api_base_url, api_base_url),
            (&self.watchdog_input_api_key, String::new()),
        ];
        for (slot, val) in pairs {
            if let Some(input) = slot
                && input.read(cx).value().as_str() != val
            {
                input.update(cx, |inp, cx| {
                    inp.set_value(val, window, cx);
                });
            }
        }
        if let Some(ref input) = self.watchdog_input_api_base_url
            && input.read(cx).presentation().placeholder().as_str()
                != api_placeholder
        {
            input.update(cx, |inp, cx| {
                inp.set_placeholder(api_placeholder, window, cx);
            });
        }
    }

    /// Initialize text input entities for the Settings page if absent.
    pub fn ensure_watchdog_inputs(
        &mut self,
        window: &mut Window,
        cx: &mut Context<'_, Self>,
    ) {
        let locale = self.locale;
        if self.setting_input_default_depth.is_none() {
            let val = self.app_settings.default_depth.to_string();
            let input = cx.new(|cx| {
                InputState::new(window, cx)
                    .placeholder("3")
                    .default_value(val)
            });
            self.setting_input_default_depth = Some(input);
        }
        if self.setting_input_ssh_duration.is_none() {
            let val =
                self.app_settings.ssh_session_duration_minutes.to_string();
            let input = cx.new(|cx| {
                InputState::new(window, cx)
                    .placeholder("15")
                    .default_value(val)
            });
            self.setting_input_ssh_duration = Some(input);
        }
        if self.watchdog_input_min_free.is_none() {
            let val = self.watchdog_config.min_free_gb.to_string();
            let input = cx.new(|cx| {
                InputState::new(window, cx)
                    .placeholder("20")
                    .default_value(val)
            });
            self.watchdog_input_min_free = Some(input);
        }
        if self.watchdog_input_poll_interval.is_none() {
            let val = self
                .watchdog_config
                .poll_interval_secs
                .div_ceil(60)
                .max(1)
                .to_string();
            let input = cx.new(|cx| {
                InputState::new(window, cx)
                    .placeholder("5")
                    .default_value(val)
            });
            self.watchdog_input_poll_interval = Some(input);
        }
        if self.watchdog_input_cooldown.is_none() {
            let min_cooldown_mins = MIN_COOLDOWN_SECS / 60;
            let val = self
                .watchdog_config
                .cooldown_secs
                .div_ceil(60)
                .max(min_cooldown_mins)
                .to_string();
            let input = cx.new(|cx| {
                InputState::new(window, cx)
                    .placeholder("360")
                    .default_value(val)
            });
            self.watchdog_input_cooldown = Some(input);
        }
        if self.webhook_input_name.is_none() {
            let input = cx.new(|cx| {
                InputState::new(window, cx).placeholder(
                    crate::i18n::pages::placeholder_webhook_name(locale),
                )
            });
            self.webhook_input_name = Some(input);
        }
        if self.setting_input_webhook_url.is_none() {
            let input = cx.new(|cx| {
                InputState::new(window, cx).placeholder(
                    crate::i18n::pages::placeholder_webhook_url(locale),
                )
            });
            self.setting_input_webhook_url = Some(input);
        }
        if self.setting_input_webhook_timeout.is_none() {
            let val =
                ruum_core::storage::DEFAULT_WEBHOOK_TIMEOUT_SECS.to_string();
            let input = cx.new(|cx| {
                InputState::new(window, cx)
                    .placeholder("10")
                    .default_value(val)
            });
            self.setting_input_webhook_timeout = Some(input);
        }
        if self.setting_input_webhook_headers.is_none() {
            let input = cx.new(|cx| {
                InputState::new(window, cx)
                    .placeholder(
                        crate::i18n::pages::placeholder_webhook_headers(locale),
                    )
                    .default_value("{}")
            });
            self.setting_input_webhook_headers = Some(input);
        }
        if self.watchdog_input_custom_cmd.is_none() {
            let val = self.ai_config.custom_command.clone();
            let input = cx.new(|cx| {
                InputState::new(window, cx)
                    .placeholder(crate::i18n::messages::placeholder_custom_cmd(
                        locale,
                    ))
                    .default_value(val)
            });
            self.watchdog_input_custom_cmd = Some(input);
        }
        if self.watchdog_input_extra_args.is_none() {
            let val = self.ai_config.extra_args.clone();
            let input = cx.new(|cx| {
                InputState::new(window, cx)
                    .placeholder(crate::i18n::messages::placeholder_extra_args(
                        locale,
                    ))
                    .default_value(val)
            });
            self.watchdog_input_extra_args = Some(input);
        }
        if self.watchdog_input_model.is_none() {
            let val = self.ai_config.model.clone().unwrap_or_default();
            let input = cx.new(|cx| {
                InputState::new(window, cx)
                    .placeholder(
                        crate::i18n::messages::placeholder_model_override(
                            locale,
                        ),
                    )
                    .default_value(val)
            });
            self.watchdog_input_model = Some(input);
        }
        if self.watchdog_input_api_base_url.is_none() {
            let val = self.ai_config.api_base_url.clone();
            let placeholder =
                self.watchdog_draft.api_protocol.default_base_url();
            let input = cx.new(|cx| {
                InputState::new(window, cx)
                    .placeholder(placeholder)
                    .default_value(val)
            });
            self.watchdog_input_api_base_url = Some(input);
        }
        if self.watchdog_input_api_key.is_none() {
            let input = cx.new(|cx| {
                InputState::new(window, cx).masked(true).placeholder(
                    crate::i18n::messages::placeholder_api_key(locale),
                )
            });
            self.watchdog_input_api_key = Some(input);
        }
        self.ensure_cloud_settings_redeem_input(window, cx);
    }

    fn create_redeem_input_entity(
        window: &mut Window,
        cx: &mut Context<'_, Self>,
    ) -> Entity<InputState> {
        let input = cx.new(|cx| {
            InputState::new(window, cx)
                .placeholder(crate::i18n::pages::CLOUD_REDEEM_PLACEHOLDER)
        });
        cx.subscribe(&input, move |this, _, event, cx| {
            if matches!(event, InputEvent::PressEnter { .. }) {
                this.submit_redeem_code(cx);
            }
        })
        .detach();
        input
    }

    /// Initialize the Settings `RedeemCodeControl` input entity if absent.
    pub fn ensure_cloud_settings_redeem_input(
        &mut self,
        window: &mut Window,
        cx: &mut Context<'_, Self>,
    ) {
        if self.cloud_redeem_input_settings.is_none() {
            self.cloud_redeem_input_settings =
                Some(Self::create_redeem_input_entity(window, cx));
        }
    }

    /// Clear the redemption code input after a successful redemption.
    pub fn clear_cloud_redeem_inputs(
        &self,
        window: &mut Window,
        cx: &mut Context<'_, Self>,
    ) {
        if let Some(input) = &self.cloud_redeem_input_settings
            && !input.read(cx).value().is_empty()
        {
            input.update(cx, |inp, cx| {
                inp.set_value(String::new(), window, cx);
            });
        }
    }

    /// Read current AI settings including any unsaved draft/input values.
    fn snapshot_ai_config(&self, cx: &App) -> AiConfig {
        let mut cfg = self.ai_config.clone();
        cfg.auto_analyze_on_alert = self.watchdog_draft.ai_auto_analyze;
        cfg.provider = self.watchdog_draft.ai_provider;
        cfg.api_protocol = self.watchdog_draft.api_protocol;
        cfg.model.clone_from(&self.watchdog_draft.model);

        if let Some(ref input) = self.watchdog_input_custom_cmd {
            cfg.custom_command = input.read(cx).value().trim().to_string();
        }
        if let Some(ref input) = self.watchdog_input_extra_args {
            cfg.extra_args = input.read(cx).value().trim().to_string();
        }
        if let Some(ref input) = self.watchdog_input_api_base_url {
            cfg.api_base_url = input.read(cx).value().trim().to_string();
        }
        if cfg.provider != AiCliProvider::RuumCloud
            && let Some(ref input) = self.watchdog_input_model
        {
            let text = input.read(cx).value().trim().to_string();
            cfg.model = (!text.is_empty()).then_some(text);
        }
        cfg.normalized()
    }

    /// Build an execution context carrying any unsaved draft API key or the
    /// lazy credential store (without accessing the keychain on UI threads).
    fn snapshot_ai_execution_context(
        &self,
        cx: &App,
    ) -> ruum_core::ai_cli::AiExecutionContext {
        let mut ctx = ruum_core::ai_cli::AiExecutionContext::default();
        if let Some(ref db_path) = self.db_path {
            ctx = ctx.with_db_path(db_path.clone());
        }
        #[cfg(test)]
        if let Some(ref base_url) = self.cloud_base_url_override {
            ctx = ctx.with_cloud_base_url(base_url.clone());
        }
        if self.watchdog_draft.ai_provider == AiCliProvider::RuumCloud {
            return self.attach_lazy_credential_store(ctx);
        }
        if self.watchdog_draft.ai_provider != AiCliProvider::Api {
            return ctx;
        }
        if let Some(ref input) = self.watchdog_input_api_key {
            let key = input.read(cx).value().trim().to_string();
            if !key.is_empty() {
                return ctx.with_api_key(key);
            }
        }
        self.attach_lazy_credential_store(ctx)
    }

    fn snapshot_cloud_execution_context(
        &self,
    ) -> ruum_core::ai_cli::AiExecutionContext {
        let mut ctx = ruum_core::ai_cli::AiExecutionContext::default();
        if let Some(ref db_path) = self.db_path {
            ctx = ctx.with_db_path(db_path.clone());
        }
        #[cfg(test)]
        if let Some(ref base_url) = self.cloud_base_url_override {
            ctx = ctx.with_cloud_base_url(base_url.clone());
        }
        self.attach_lazy_credential_store(ctx)
    }

    fn attach_lazy_credential_store(
        &self,
        mut ctx: ruum_core::ai_cli::AiExecutionContext,
    ) -> ruum_core::ai_cli::AiExecutionContext {
        if let Some(ref store) = self.cred_store {
            ctx = ctx.with_credential_store(store.clone());
        } else if let Some(ref db_path) = self.db_path
            && let Some(dir) = db_path.parent()
            && !dir.as_os_str().is_empty()
        {
            ctx = ctx.with_credential_store(LazyCredentialStore::platform(dir));
        } else if cfg!(test) && !ruum_core::ai_cli::env_api_key_present() {
            ctx = ctx.with_credential_store(LazyCredentialStore::unavailable(
                "no credential store configured in test",
            ));
        }
        ctx
    }

    /// Switch the active UI locale in tests.
    #[cfg(test)]
    pub const fn set_locale(&mut self, locale: crate::i18n::Locale) {
        self.locale = locale;
    }

    /// Current resolved window title.
    pub fn window_title(&self) -> &str {
        &self.window_title
    }

    /// Show a floating toast notification that auto-dismisses after 4 seconds.
    pub fn push_toast(
        &mut self,
        message: impl Into<String>,
        status: Status,
        cx: &mut Context<'_, Self>,
    ) {
        self.toast_epoch = self.toast_epoch.wrapping_add(1);
        let epoch = self.toast_epoch;
        self.notice = Some((message.into(), status));
        cx.notify();

        if cfg!(test) {
            return;
        }

        cx.spawn(async move |this, cx| {
            cx.background_executor().timer(Duration::from_secs(4)).await;
            let _ = this.update(cx, |this, cx| {
                if this.toast_epoch == epoch {
                    this.notice = None;
                    cx.notify();
                }
            });
        })
        .detach();
    }

    /// Dismiss the active floating toast notification immediately.
    pub fn dismiss_toast(&mut self, cx: &mut Context<'_, Self>) {
        self.toast_epoch = self.toast_epoch.wrapping_add(1);
        if self.notice.take().is_some() {
            cx.notify();
        }
    }

    /// Switch the configured AI CLI provider and reset stale model/speed state.
    pub fn set_watchdog_provider(
        &mut self,
        provider: ruum_core::watchdog_config::AiCliProvider,
        cx: &mut Context<'_, Self>,
    ) {
        let changed = self.watchdog_draft.ai_provider != provider;
        if changed {
            self.watchdog_draft.ai_provider = provider;
            self.watchdog_test_epoch += 1;
            self.watchdog_fetch_epoch += 1;
            self.watchdog_benchmark_epoch += 1;
            self.cloud_refresh_epoch += 1;
            self.cloud_fetch_models_epoch += 1;
            self.redeem_epoch = self.redeem_epoch.wrapping_add(1);
            self.watchdog_testing = false;
            self.watchdog_fetching_models = false;
            self.cloud_refreshing = false;
            self.cloud_fetching_models = false;
            self.redeem_submitting = false;
            self.cloud_redeem_feedback = None;
            self.watchdog_benchmarking_models.clear();
            self.watchdog_models.clear();
            self.watchdog_speed_results.clear();
        }
        if provider == AiCliProvider::RuumCloud {
            if !self.ai_config.setup_completed {
                self.ai_config.setup_completed = true;
                if let Some(db) = open_db_for_window(self) {
                    let mut persisted = db
                        .load_ai_config()
                        .unwrap_or_else(|_| self.ai_config.clone());
                    persisted.setup_completed = true;
                    persisted.updated_at_secs =
                        u64::try_from(now_seconds()).unwrap_or(0);
                    let _ = db.save_ai_config(&persisted);
                }
            }
            self.reload_cloud_state();
            self.sync_selected_cloud_model();
        }
        if changed || provider == AiCliProvider::RuumCloud {
            cx.notify();
        }
    }

    /// Parse cached Ruum Cloud entitlement from `cloud_state.entitlement_json`.
    #[must_use]
    pub fn cached_cloud_entitlement(&self) -> Option<EntitlementSnapshot> {
        let raw = self.cloud_state.entitlement_json.as_deref()?;
        serde_json::from_str::<EntitlementSnapshot>(raw).ok()
    }

    /// Parse cached Ruum Cloud billing plans from `cloud_state.plans_json`.
    #[must_use]
    pub fn cached_cloud_plans(&self) -> Option<BillingPlansSnapshot> {
        let raw = self.cloud_state.plans_json.as_deref()?;
        serde_json::from_str::<BillingPlansSnapshot>(raw).ok()
    }

    /// Parse cached Ruum Cloud virtual models from `cloud_state.models_json`.
    #[must_use]
    pub fn cached_cloud_models(&self) -> Vec<CloudVirtualModel> {
        let Some(raw) = self.cloud_state.models_json.as_deref() else {
            return Vec::new();
        };
        if let Ok(list) = serde_json::from_str::<Vec<CloudVirtualModel>>(raw) {
            return list;
        }
        serde_json::from_str::<serde_json::Value>(raw)
            .ok()
            .and_then(|val| val.get("data").cloned())
            .and_then(|data| {
                serde_json::from_value::<Vec<CloudVirtualModel>>(data).ok()
            })
            .unwrap_or_default()
    }

    /// Reload cached `cloud_state` from `SQLite` without touching keychain/network.
    pub fn reload_cloud_state(&mut self) {
        if let Some(db) = self.open_existing_db()
            && let Ok(cloud) = db.load_cloud_state()
        {
            self.cloud_state = cloud;
        }
    }

    /// Validate cached Cloud model selection without overwriting `None` (`Auto`).
    ///
    /// Only clears a selected `Some(id)` back to `None` when `models` is
    /// non-empty and no available model matches `id`.
    pub fn sync_selected_cloud_model(&mut self) {
        let models = self.cached_cloud_models();
        if models.is_empty() {
            return;
        }
        self.cloud_payment_required = models.iter().all(|m| !m.available);
        let preferred = self
            .watchdog_draft
            .model
            .as_deref()
            .or(self.cloud_state.selected_model_id.as_deref())
            .map(str::trim)
            .filter(|s| !s.is_empty());
        let valid = preferred.and_then(|id| {
            models
                .iter()
                .any(|m| m.id == id && m.available)
                .then(|| id.to_owned())
        });
        self.cloud_state.selected_model_id.clone_from(&valid);
        self.watchdog_draft.model = valid;
    }

    fn persist_cloud_model_selection(
        &mut self,
        chosen: Option<String>,
        cx: &mut Context<'_, Self>,
    ) {
        let now_secs = u64::try_from(now_seconds()).unwrap_or(0);
        self.watchdog_draft.model.clone_from(&chosen);
        self.ai_config.model.clone_from(&chosen);
        self.cloud_state.selected_model_id.clone_from(&chosen);
        if let Some(db) = open_db_for_window(self) {
            if let Ok(mut state) = db.load_cloud_state() {
                state.selected_model_id.clone_from(&chosen);
                state.updated_at_secs = now_secs;
                let _ = db.save_cloud_state(&state);
                self.cloud_state = state;
            }
            let mut ai_cfg = db
                .load_ai_config()
                .unwrap_or_else(|_| self.ai_config.clone());
            ai_cfg.model = chosen;
            ai_cfg.updated_at_secs = now_secs;
            let _ = db.save_ai_config(&ai_cfg);
            self.ai_config = ai_cfg;
        }
        cx.notify();
    }

    /// Select `Auto (Recommended)` (`None`) for Ruum Cloud and persist to
    /// both `cloud_state.selected_model_id` and `ai_config.model`.
    pub fn select_cloud_model_auto(&mut self, cx: &mut Context<'_, Self>) {
        self.persist_cloud_model_selection(None, cx);
    }

    /// Select an available Ruum Cloud virtual model and persist to both
    /// `cloud_state.selected_model_id` and `ai_config.model`.
    pub fn select_cloud_model(
        &mut self,
        model_id: &str,
        cx: &mut Context<'_, Self>,
    ) {
        let trimmed = model_id.trim();
        let models = self.cached_cloud_models();
        if !models.iter().any(|m| m.id == trimmed && m.available) {
            return;
        }
        self.persist_cloud_model_selection(Some(trimmed.to_owned()), cx);
    }

    /// Fetch Ruum Cloud virtual models off the UI thread, reload `cloud_state`,
    /// validate current selection (preserving `Auto`), and toast the outcome.
    pub fn fetch_cloud_models(&mut self, cx: &mut Context<'_, Self>) {
        if self.cloud_fetching_models {
            return;
        }
        let ai_ctx = self.snapshot_cloud_execution_context();
        let locale = self.locale;
        let provider_label = AiCliProvider::RuumCloud.label_in(locale);
        self.cloud_fetching_models = true;
        self.cloud_fetch_models_epoch =
            self.cloud_fetch_models_epoch.wrapping_add(1);
        let epoch = self.cloud_fetch_models_epoch;
        self.push_toast(
            crate::i18n::messages::fetching_models(provider_label, locale),
            Status::Neutral,
            cx,
        );

        let task = cx.background_executor().spawn(async move {
            let client = build_cloud_client(&ai_ctx)?;
            client.fetch_virtual_models()
        });

        cx.spawn(async move |this, cx| {
            let res = task.await;
            let _ = this.update(cx, |this, cx| {
                if this.cloud_fetch_models_epoch != epoch {
                    return;
                }
                this.cloud_fetching_models = false;
                this.reload_cloud_state();
                this.sync_selected_cloud_model();
                let active_locale = this.locale;
                let active_provider_label =
                    AiCliProvider::RuumCloud.label_in(active_locale);
                match res {
                    Ok(models) => {
                        let (msg, status) = if models.is_empty() {
                            (
                                crate::i18n::messages::no_models_discovered(
                                    active_provider_label,
                                    active_locale,
                                ),
                                Status::Warning,
                            )
                        } else {
                            (
                                crate::i18n::messages::loaded_models_for_provider(
                                    models.len(),
                                    active_provider_label,
                                    active_locale,
                                ),
                                Status::Success,
                            )
                        };
                        this.push_toast(msg, status, cx);
                    }
                    Err(err) => {
                        this.push_toast(
                            err.format_localized(active_locale),
                            Status::Error,
                            cx,
                        );
                    }
                }
            });
        })
        .detach();
    }

    /// Set Watchdog auto-analyze, gating first-run onboarding on `!setup_completed`.
    pub fn set_watchdog_ai_auto_analyze(
        &mut self,
        enabled: bool,
        cx: &mut Context<'_, Self>,
    ) {
        if enabled && !self.ai_config.setup_completed {
            self.ai_setup_choice_modal = Some(AiSetupChoiceModalState {
                pending_action: PendingAiSetupAction::EnableWatchdogAutoAnalyze,
            });
            self.focus_request = Some(FocusTarget::AiSetupChoiceDialog);
            cx.notify();
            return;
        }
        if self.watchdog_draft.ai_auto_analyze != enabled {
            self.watchdog_draft.ai_auto_analyze = enabled;
            cx.notify();
        }
    }

    /// Toggle Watchdog auto-analyze from the Settings checkbox.
    pub fn toggle_watchdog_ai_auto_analyze(
        &mut self,
        cx: &mut Context<'_, Self>,
    ) {
        let next = !self.watchdog_draft.ai_auto_analyze;
        self.set_watchdog_ai_auto_analyze(next, cx);
    }

    /// Dismiss the first-run AI setup modal without marking `setup_completed`
    /// or touching keychain/network.
    pub fn cancel_ai_setup_choice(&mut self, cx: &mut Context<'_, Self>) {
        if self.ai_setup_choice_modal.take().is_some() {
            self.focus_request = Some(FocusTarget::Root);
            cx.notify();
        }
    }

    /// Confirm an onboarding backend choice, persist `setup_completed = true`,
    /// and resume the deferred action.
    pub fn confirm_ai_setup_choice(
        &mut self,
        choice: AiSetupChoice,
        cx: &mut Context<'_, Self>,
    ) {
        let Some(modal) = self.ai_setup_choice_modal.take() else {
            return;
        };
        let provider = match choice {
            AiSetupChoice::RuumCloud => AiCliProvider::RuumCloud,
            AiSetupChoice::CustomApi => AiCliProvider::Api,
            AiSetupChoice::LocalCli => match self.ai_config.provider {
                AiCliProvider::Pi
                | AiCliProvider::Agy
                | AiCliProvider::Codex
                | AiCliProvider::Custom => self.ai_config.provider,
                AiCliProvider::RuumCloud | AiCliProvider::Api => {
                    AiCliProvider::Agy
                }
            },
        };
        let now_secs = u64::try_from(now_seconds()).unwrap_or(0);
        self.ai_config.provider = provider;
        self.ai_config.setup_completed = true;
        self.ai_config.updated_at_secs = now_secs;
        self.watchdog_draft.ai_provider = provider;
        if matches!(
            modal.pending_action,
            PendingAiSetupAction::EnableWatchdogAutoAnalyze
        ) {
            self.watchdog_draft.ai_auto_analyze = true;
            self.ai_config.auto_analyze_on_alert = true;
        }
        if let Some(db) = open_db_for_window(self) {
            let _ = db.save_ai_config(&self.ai_config);
        }
        if provider == AiCliProvider::RuumCloud {
            self.reload_cloud_state();
            self.sync_selected_cloud_model();
        }
        self.focus_request = Some(FocusTarget::Root);
        cx.notify();

        match (choice, modal.pending_action) {
            (AiSetupChoice::RuumCloud, PendingAiSetupAction::LocalAnalysis) => {
                self.run_ai_analysis_now(cx);
            }
            (
                AiSetupChoice::RuumCloud,
                PendingAiSetupAction::SshAnalysis { profile_id },
            ) => {
                self.analyze_ssh_profile(&profile_id, cx);
            }
            (
                AiSetupChoice::CustomApi | AiSetupChoice::LocalCli,
                PendingAiSetupAction::LocalAnalysis
                | PendingAiSetupAction::SshAnalysis { .. },
            ) => {
                self.screen = Screen::Settings;
                self.settings_category = SettingsCategory::Ai;
                self.watchdog_inputs_need_sync = true;
                cx.notify();
            }
            (_, PendingAiSetupAction::EnableWatchdogAutoAnalyze) => {}
        }
    }

    /// Open the localized official support / donation section in the browser.
    pub fn open_support_website(&self, cx: &App) {
        cx.open_url(ruum_support_url(self.locale));
    }

    /// Open the Ruum Cloud pass billing modal, hydrating cached plans at 0ms
    /// when available while silently refreshing plans in the background.
    pub fn open_billing_modal(&mut self, cx: &mut Context<'_, Self>) {
        let now_secs = current_unix_secs();
        if let Some(ent) = self.cached_cloud_entitlement()
            && ent.is_alipay_active_at(now_secs)
        {
            let date = ent
                .alipay_valid_until_secs
                .or(ent.valid_until_secs)
                .map_or_else(|| "-".to_owned(), format_unix_date_ymd);
            self.push_toast(
                crate::i18n::pages::cloud_status_paid_until(
                    &date,
                    &ent.source,
                    self.locale,
                ),
                Status::Neutral,
                cx,
            );
            return;
        }
        self.crumb_menu = None;
        self.power_menu = None;
        self.show_help = false;
        self.cloud_redeem_feedback = None;
        self.billing_modal_epoch = self.billing_modal_epoch.wrapping_add(1);
        let epoch = self.billing_modal_epoch;
        let cached = self
            .cached_cloud_plans()
            .filter(BillingPlansSnapshot::is_available);
        self.billing_modal = Some(match cached {
            Some(plans) => {
                let first_sku =
                    plans.charging_options.first().map(|o| o.sku_id.clone());
                BillingModalState {
                    step: BillingModalStep::SelectPlan,
                    loading_plans: false,
                    plans: Some(plans),
                    selected_sku_id: first_sku,
                    poll_epoch: epoch,
                    ..BillingModalState::default()
                }
            }
            None => BillingModalState {
                step: BillingModalStep::SelectPlan,
                loading_plans: true,
                poll_epoch: epoch,
                ..BillingModalState::default()
            },
        });
        self.focus_request = Some(FocusTarget::BillingDialog);
        cx.notify();
        self.load_billing_plans_for_epoch(epoch, cx);
    }

    /// Retry loading Ruum Cloud billing plans for the active modal.
    pub fn retry_billing_plans(&mut self, cx: &mut Context<'_, Self>) {
        let Some(modal) = self.billing_modal.as_mut() else {
            return;
        };
        if modal.loading_plans {
            return;
        }
        modal.step = BillingModalStep::SelectPlan;
        modal.loading_plans = true;
        modal.plans_unavailable = false;
        modal.error = None;
        self.focus_request = Some(FocusTarget::BillingDialog);
        let epoch = self.billing_modal_epoch;
        cx.notify();
        self.load_billing_plans_for_epoch(epoch, cx);
    }

    fn load_billing_plans_for_epoch(&self, epoch: u64, cx: &Context<'_, Self>) {
        let ai_ctx = self.snapshot_cloud_execution_context();
        let locale = self.locale;
        let task = cx.background_executor().spawn(async move {
            let client = build_cloud_client(&ai_ctx)?;
            client.fetch_billing_plans()
        });

        cx.spawn(async move |this, cx| {
            let res = task.await;
            let _ = this.update(cx, |this, cx| {
                if this.billing_modal_epoch != epoch {
                    return;
                }
                if res.is_ok() {
                    this.reload_cloud_state();
                }
                let Some(modal) = this.billing_modal.as_mut() else {
                    return;
                };
                modal.loading_plans = false;
                match res {
                    Ok(plans) => {
                        if plans.is_available() {
                            modal.plans_unavailable = false;
                            let keep_existing = modal
                                .selected_sku_id
                                .as_deref()
                                .is_some_and(|sel| {
                                    plans
                                        .charging_options
                                        .iter()
                                        .any(|o| o.sku_id == sel)
                                });
                            if !keep_existing {
                                modal.selected_sku_id = plans
                                    .charging_options
                                    .first()
                                    .map(|o| o.sku_id.clone());
                            }
                        } else {
                            modal.plans_unavailable = true;
                            modal.selected_sku_id = None;
                        }
                        modal.plans = Some(plans);
                        modal.error = None;
                    }
                    Err(err) => {
                        if modal.plans.is_some() {
                            modal.plans_unavailable = false;
                            modal.error = None;
                        } else if err.is_billing_unavailable() {
                            modal.plans_unavailable = true;
                            modal.error = None;
                        } else {
                            modal.error = Some(err.format_localized(locale));
                        }
                    }
                }
                cx.notify();
            });
        })
        .detach();
    }

    /// Close the Ruum Cloud billing modal and invalidate any active poll loop.
    pub fn close_billing_modal(&mut self, cx: &mut Context<'_, Self>) {
        self.billing_modal_epoch = self.billing_modal_epoch.wrapping_add(1);
        if self.billing_modal.take().is_some() {
            self.focus_request = Some(FocusTarget::Root);
            cx.notify();
        }
    }

    /// Select a SKU card in Step 1 (`SelectPlan`) without triggering any network call.
    pub fn select_billing_sku(
        &mut self,
        sku_id: String,
        cx: &mut Context<'_, Self>,
    ) {
        let Some(modal) = self.billing_modal.as_mut() else {
            return;
        };
        if modal.creating_order || modal.step != BillingModalStep::SelectPlan {
            return;
        }
        if modal.selected_sku_id.as_deref() != Some(sku_id.as_str()) {
            modal.selected_sku_id = Some(sku_id);
            modal.pending_out_request_no = None;
            cx.notify();
        }
    }

    /// Confirm the selected SKU in Step 1 (`SelectPlan`) and advance to Step 2
    /// (`Checkout`), creating the purchase order.
    pub fn confirm_billing_plan(&mut self, cx: &mut Context<'_, Self>) {
        let Some(sku_id) = self
            .billing_modal
            .as_ref()
            .filter(|m| {
                m.step == BillingModalStep::SelectPlan
                    && !m.creating_order
                    && !m.loading_plans
                    && !m.plans_unavailable
            })
            .and_then(|m| m.selected_sku_id.clone())
        else {
            return;
        };
        self.focus_request = Some(FocusTarget::BillingDialog);
        self.purchase_billing_sku(sku_id, cx);
    }

    /// Return from QR checkout view (`Checkout`) to SKU plan selection (`SelectPlan`).
    pub fn back_to_billing_plans(&mut self, cx: &mut Context<'_, Self>) {
        self.billing_modal_epoch = self.billing_modal_epoch.wrapping_add(1);
        let epoch = self.billing_modal_epoch;
        if let Some(modal) = self.billing_modal.as_mut() {
            modal.step = BillingModalStep::SelectPlan;
            modal.active_order = None;
            modal.active_sku = None;
            modal.qr_matrix = None;
            modal.creating_order = false;
            modal.refreshing_status = false;
            modal.polling_timed_out = false;
            modal.error = None;
            modal.pending_out_request_no = None;
            modal.poll_epoch = epoch;
            self.focus_request = Some(FocusTarget::BillingDialog);
            cx.notify();
        }
    }

    /// Create a purchase order for `sku_id`, either unlocking immediately on
    /// `ALLOW` or entering QR checkout + 3s non-blocking polling.
    pub fn purchase_billing_sku(
        &mut self,
        sku_id: String,
        cx: &mut Context<'_, Self>,
    ) {
        let Some(modal) = self.billing_modal.as_mut() else {
            return;
        };
        if modal.creating_order {
            return;
        }
        if modal.selected_sku_id.as_deref() != Some(sku_id.as_str()) {
            modal.pending_out_request_no = None;
        }
        let out_request_no = modal
            .pending_out_request_no
            .get_or_insert_with(
                ruum_core::cloud_client::generate_purchase_request_no,
            )
            .clone();
        modal.step = BillingModalStep::Checkout;
        modal.creating_order = true;
        modal.selected_sku_id = Some(sku_id.clone());
        modal.error = None;
        let active_sku = modal.plans.as_ref().and_then(|plans| {
            plans
                .charging_options
                .iter()
                .find(|opt| opt.sku_id == sku_id)
                .cloned()
        });
        modal.active_sku.clone_from(&active_sku);
        self.focus_request = Some(FocusTarget::BillingDialog);
        let epoch = self.billing_modal_epoch;
        let ai_ctx = self.snapshot_cloud_execution_context();
        let lock = Arc::clone(&self.cloud_refresh_lock);
        let last_refresh_at = Arc::clone(&self.cloud_last_refresh_at);
        cx.notify();

        let task = cx.background_executor().spawn(async move {
            let client = build_cloud_client(&ai_ctx)?;
            let order = client
                .create_purchase_with_request_no(&sku_id, &out_request_no)?;
            if order.decision.eq_ignore_ascii_case("ALLOW") {
                let _ = sync_entitlement_and_models_after_allow(
                    &client,
                    &lock,
                    &last_refresh_at,
                )?;
                return Ok::<_, CloudApiError>(PurchaseOutcome::Allowed);
            }
            let qr_payload = select_billing_qr_payload(
                order.purchase_url.as_deref(),
                order.purchase_qr_code.as_deref(),
            );
            let qr_matrix = qr_payload
                .as_deref()
                .map(crate::qr::QrMatrix::encode)
                .transpose()
                .map_err(|e| {
                    CloudApiError::new(None, Some("CLOUD_QR_ERROR"), e, None)
                })?
                .map(Arc::new);
            Ok::<_, CloudApiError>(PurchaseOutcome::PendingQr {
                order,
                qr_matrix,
            })
        });

        cx.spawn(async move |this, cx| {
            let res = task.await;
            let _ = this.update(cx, |this, cx| {
                if this.billing_modal_epoch != epoch {
                    return;
                }
                match res {
                    Ok(PurchaseOutcome::Allowed) => {
                        let _ = this.mark_cloud_paid_unlocked(cx);
                    }
                    Ok(PurchaseOutcome::PendingQr { order, qr_matrix }) => {
                        let matched_sku = active_sku.or_else(|| {
                            order.charging_options.first().cloned()
                        });
                        if let Some(modal) = this.billing_modal.as_mut() {
                            modal.creating_order = false;
                            modal.active_order = Some(order);
                            modal.active_sku = matched_sku;
                            modal.qr_matrix = qr_matrix;
                            modal.polling_timed_out = false;
                            modal.error = None;
                            modal.poll_epoch = epoch;
                        }
                        cx.notify();
                        Self::start_billing_poll_loop(epoch, cx);
                    }
                    Err(err) => {
                        let active_locale = this.locale;
                        if let Some(modal) = this.billing_modal.as_mut() {
                            modal.creating_order = false;
                            modal.error =
                                Some(err.format_localized(active_locale));
                        }
                        cx.notify();
                    }
                }
            });
        })
        .detach();
    }

    /// Open the validated HTTPS billing checkout URL in the system browser.
    pub fn open_billing_browser_url(&self, cx: &App) {
        let Some(order) = self
            .billing_modal
            .as_ref()
            .and_then(|m| m.active_order.as_ref())
        else {
            return;
        };
        if let Some(url) = select_safe_billing_browser_url(
            order.purchase_url.as_deref(),
            order.purchase_qr_code.as_deref(),
        ) {
            cx.open_url(&url);
        }
    }

    /// Manually check `?refresh=1` from the QR checkout view using the shared
    /// refresh lock so it does not collide with automatic 3s polling.
    pub fn refresh_billing_order_status(&mut self, cx: &mut Context<'_, Self>) {
        if self.cloud_refreshing {
            return;
        }
        let Some(modal) = self.billing_modal.as_mut() else {
            return;
        };
        if modal.refreshing_status || modal.active_order.is_none() {
            return;
        }
        modal.refreshing_status = true;
        modal.error = None;
        self.cloud_refreshing = true;
        let epoch = self.billing_modal_epoch;
        let ai_ctx = self.snapshot_cloud_execution_context();
        let lock = Arc::clone(&self.cloud_refresh_lock);
        let last_refresh_at = Arc::clone(&self.cloud_last_refresh_at);
        cx.notify();

        let task = cx.background_executor().spawn(async move {
            poll_billing_entitlement_once(&ai_ctx, &lock, &last_refresh_at)
        });

        cx.spawn(async move |this, cx| {
            let res = task.await;
            cx.update(|cx| {
                let mut paid_notif: Option<(crate::i18n::Locale, String)> = None;
                let _ = this.update(cx, |this, cx| {
                    this.cloud_refreshing = false;
                    if this.billing_modal_epoch != epoch {
                        return;
                    }
                    this.reload_cloud_state();
                    this.sync_selected_cloud_model();
                    let active_locale = this.locale;
                    match res {
                        Ok(ent) if ent.is_paid => {
                            let msg = this.mark_cloud_paid_unlocked(cx);
                            paid_notif = Some((active_locale, msg));
                        }
                        Ok(_) => {
                            if let Some(modal) = this.billing_modal.as_mut() {
                                modal.refreshing_status = false;
                            }
                            this.push_toast(
                                crate::i18n::messages::cloud_payment_still_pending(
                                    active_locale,
                                ),
                                Status::Neutral,
                                cx,
                            );
                        }
                        Err(err) => {
                            if let Some(modal) = this.billing_modal.as_mut() {
                                modal.refreshing_status = false;
                                modal.error =
                                    Some(err.format_localized(active_locale));
                            }
                            cx.notify();
                        }
                    }
                });
                if let Some((notif_locale, msg)) = paid_notif {
                    crate::post_app_notification(
                        ruum_core::watchdog::NOTIF_TAG_AI_ANALYSIS,
                        crate::i18n::messages::cloud_payment_succeeded_notif_title(
                            notif_locale,
                        ),
                        &msg,
                        cx,
                    );
                }
            });
        })
        .detach();
    }

    fn start_billing_poll_loop(epoch: u64, cx: &Context<'_, Self>) {
        cx.spawn(async move |this, cx| {
            let mut elapsed = Duration::ZERO;
            loop {
                cx.background_executor().timer(BILLING_POLL_INTERVAL).await;
                elapsed += BILLING_POLL_INTERVAL;

                let action = this
                    .update(cx, |this, cx| {
                        if this.billing_modal_epoch != epoch {
                            return BillingPollStep::Stop;
                        }
                        let Some(modal) = this.billing_modal.as_mut() else {
                            return BillingPollStep::Stop;
                        };
                        if modal.active_order.is_none()
                            || modal.poll_epoch != epoch
                        {
                            return BillingPollStep::Stop;
                        }
                        if elapsed > BILLING_POLL_MAX_DURATION {
                            modal.polling_timed_out = true;
                            cx.notify();
                            return BillingPollStep::Stop;
                        }
                        if this.cloud_refreshing || modal.refreshing_status {
                            return BillingPollStep::Skip;
                        }
                        if !cfg!(test)
                            && let Some(last) = *this
                                .cloud_last_refresh_at
                                .lock()
                                .unwrap_or_else(
                                    std::sync::PoisonError::into_inner,
                                )
                            && last.elapsed() < CLOUD_REFRESH_DEBOUNCE
                        {
                            return BillingPollStep::Skip;
                        }
                        this.cloud_refreshing = true;
                        modal.refreshing_status = true;
                        cx.notify();
                        BillingPollStep::Poll {
                            ai_ctx: this.snapshot_cloud_execution_context(),
                            lock: Arc::clone(&this.cloud_refresh_lock),
                            last_refresh_at: Arc::clone(
                                &this.cloud_last_refresh_at,
                            ),
                        }
                    })
                    .unwrap_or(BillingPollStep::Stop);

                let (ai_ctx, lock, last_refresh_at) = match action {
                    BillingPollStep::Stop => break,
                    BillingPollStep::Skip => continue,
                    BillingPollStep::Poll {
                        ai_ctx,
                        lock,
                        last_refresh_at,
                    } => (ai_ctx, lock, last_refresh_at),
                };

                let res = cx
                    .background_executor()
                    .spawn(async move {
                        poll_billing_entitlement_once(
                            &ai_ctx,
                            &lock,
                            &last_refresh_at,
                        )
                    })
                    .await;

                let aftermath = cx.update(|cx| {
                    let mut paid_notif: Option<(crate::i18n::Locale, String)> =
                        None;
                    let step = this
                        .update(cx, |this, cx| {
                            this.cloud_refreshing = false;
                            if this.billing_modal_epoch != epoch {
                                return BillingPollAftermath::Stop;
                            }
                            this.reload_cloud_state();
                            this.sync_selected_cloud_model();
                            let active_locale = this.locale;
                            match res {
                                Ok(ent) if ent.is_paid => {
                                    let msg = this.mark_cloud_paid_unlocked(cx);
                                    paid_notif = Some((active_locale, msg));
                                    BillingPollAftermath::Stop
                                }
                                Ok(_) => {
                                    if let Some(modal) =
                                        this.billing_modal.as_mut()
                                    {
                                        modal.refreshing_status = false;
                                    }
                                    cx.notify();
                                    BillingPollAftermath::Continue
                                }
                                Err(err) => {
                                    if let Some(modal) =
                                        this.billing_modal.as_mut()
                                    {
                                        modal.refreshing_status = false;
                                        modal.error = Some(
                                            err.format_localized(active_locale),
                                        );
                                    }
                                    cx.notify();
                                    BillingPollAftermath::Continue
                                }
                            }
                        })
                        .unwrap_or(BillingPollAftermath::Stop);

                    if let Some((notif_locale, msg)) = paid_notif {
                        crate::post_app_notification(
                            ruum_core::watchdog::NOTIF_TAG_AI_ANALYSIS,
                            crate::i18n::messages::cloud_payment_succeeded_notif_title(
                                notif_locale,
                            ),
                            &msg,
                            cx,
                        );
                    }
                    step
                });

                if aftermath == BillingPollAftermath::Stop {
                    break;
                }
            }
        })
        .detach();
    }

    fn mark_cloud_paid_unlocked(
        &mut self,
        cx: &mut Context<'_, Self>,
    ) -> String {
        let done_secs = u64::try_from(now_seconds()).unwrap_or(0);
        self.reload_cloud_state();
        self.sync_selected_cloud_model();
        self.cloud_payment_required = false;
        self.ai_last_error = None;
        self.ai_last_error_at_secs = None;
        if let Some(db) = open_db_for_window(self) {
            let _ = db.clear_ai_error(done_secs);
        }
        self.close_billing_modal(cx);
        let msg = crate::i18n::messages::cloud_payment_succeeded(self.locale)
            .to_owned();
        self.push_toast(msg.clone(), Status::Success, cx);
        msg
    }

    /// Submit a redemption code from AI Settings, guarding against empty
    /// submissions and concurrent double-submits, then refreshing `/v1/models`
    /// and syncing `cloud_state` on success.
    pub fn submit_redeem_code(&mut self, cx: &mut Context<'_, Self>) {
        if self.redeem_submitting {
            return;
        }
        let raw = self
            .cloud_redeem_input_settings
            .as_ref()
            .map_or_else(String::new, |inp| inp.read(cx).value().to_string());

        let clean_code = normalize_redemption_code_input(&raw);
        if clean_code.is_empty() {
            let hint = crate::i18n::pages::cloud_redeem_empty_hint(self.locale)
                .to_owned();
            self.cloud_redeem_feedback = Some((hint.clone(), false));
            self.push_toast(hint, Status::Neutral, cx);
            cx.notify();
            return;
        }

        self.redeem_submitting = true;
        self.redeem_epoch = self.redeem_epoch.wrapping_add(1);
        let epoch = self.redeem_epoch;
        self.cloud_redeem_feedback = None;
        if let Some(modal) = self.billing_modal.as_mut() {
            modal.error = None;
        }

        let ai_ctx = self.snapshot_cloud_execution_context();
        let lock = Arc::clone(&self.cloud_refresh_lock);
        let last_refresh_at = Arc::clone(&self.cloud_last_refresh_at);
        cx.notify();

        let task = cx.background_executor().spawn(async move {
            let client = build_cloud_client(&ai_ctx)?;
            let res = client.redeem_code(&clean_code)?;
            {
                let _guard = lock
                    .lock()
                    .unwrap_or_else(std::sync::PoisonError::into_inner);
                *last_refresh_at
                    .lock()
                    .unwrap_or_else(std::sync::PoisonError::into_inner) =
                    Some(Instant::now());
                let _ = client.fetch_virtual_models();
            }
            Ok::<_, CloudApiError>(res)
        });

        cx.spawn(async move |this, cx| {
            let res = task.await;
            let done_secs = u64::try_from(now_seconds()).unwrap_or(0);
            cx.update(|cx| {
                let mut success_notif: Option<(crate::i18n::Locale, String)> =
                    None;
                let _ = this.update(cx, |this, cx| {
                    if this.redeem_epoch != epoch {
                        return;
                    }
                    this.redeem_submitting = false;
                    let active_locale = this.locale;
                    match res {
                        Ok(redeemed) => {
                            this.reload_cloud_state();
                            this.sync_selected_cloud_model();
                            this.cloud_payment_required = false;
                            this.ai_last_error = None;
                            this.ai_last_error_at_secs = None;
                            this.cloud_redeem_feedback = None;
                            this.cloud_redeem_inputs_need_clear = true;
                            if let Some(db) = open_db_for_window(this) {
                                let _ = db.clear_ai_error(done_secs);
                            }
                            this.close_billing_modal(cx);
                            let valid_until_secs = redeemed
                                .entitlement
                                .valid_until_secs
                                .unwrap_or(redeemed.new_valid_until_secs);
                            let date_ymd =
                                format_unix_date_ymd(valid_until_secs);
                            let msg =
                                crate::i18n::pages::cloud_redeem_succeeded(
                                    &date_ymd,
                                    active_locale,
                                );
                            this.push_toast(msg.clone(), Status::Success, cx);
                            success_notif = Some((active_locale, msg));
                        }
                        Err(err) => {
                            let msg = err.format_localized(active_locale);
                            this.cloud_redeem_feedback =
                                Some((msg.clone(), true));
                            this.push_toast(msg, Status::Error, cx);
                            cx.notify();
                        }
                    }
                });
                if let Some((notif_locale, msg)) = success_notif {
                    crate::post_app_notification(
                        ruum_core::watchdog::NOTIF_TAG_AI_ANALYSIS,
                        crate::i18n::pages::cloud_redeem_succeeded_notif_title(
                            notif_locale,
                        ),
                        &msg,
                        cx,
                    );
                }
            });
        })
        .detach();
    }

    /// Refresh Ruum Cloud entitlement (`?refresh=1`) and virtual models off the
    /// UI thread using an epoch guard and shared mutex lock.
    pub fn refresh_cloud_entitlement(&mut self, cx: &mut Context<'_, Self>) {
        if self.cloud_refreshing {
            return;
        }
        let ai_ctx = self.snapshot_cloud_execution_context();
        let db_path = self.db_path.clone();
        let locale = self.locale;
        let lock = Arc::clone(&self.cloud_refresh_lock);
        let last_refresh_at = Arc::clone(&self.cloud_last_refresh_at);
        self.cloud_refreshing = true;
        self.cloud_refresh_epoch += 1;
        let epoch = self.cloud_refresh_epoch;
        self.push_toast(
            crate::i18n::messages::refreshing_cloud_entitlement(locale),
            Status::Neutral,
            cx,
        );

        let task = cx.background_executor().spawn(async move {
            let client = build_cloud_client(&ai_ctx)
                .map_err(|e| e.format_localized(locale))?;
            refresh_entitlement_and_models_with_client(
                &client,
                &lock,
                &last_refresh_at,
            )
            .map_err(|e| e.format_localized(locale))
        });

        cx.spawn(async move |this, cx| {
            let res = task.await;
            let done_secs = u64::try_from(now_seconds()).unwrap_or(0);
            cx.update(|cx| {
                let mut notif: Option<(
                    ruum_core::watchdog::AiFailureKind,
                    crate::i18n::Locale,
                    String,
                )> = None;
                let updated = this.update(cx, |this, cx| {
                    if epoch != this.cloud_refresh_epoch {
                        return;
                    }
                    this.cloud_refreshing = false;
                    this.reload_cloud_state();
                    this.sync_selected_cloud_model();
                    let active_locale = this.locale;
                    match res {
                        Ok((ent, models)) => {
                            let all_locked = !models.is_empty()
                                && models.iter().all(|m| !m.available);
                            this.cloud_payment_required = all_locked
                                || (!ent.is_paid && ent.trial_remaining == 0);
                            if !this.cloud_payment_required {
                                this.ai_last_error = None;
                                this.ai_last_error_at_secs = None;
                                if let Some(db) = open_db_for_window(this) {
                                    let _ = db.clear_ai_error(done_secs);
                                }
                            }
                            this.push_toast(
                                crate::i18n::messages::cloud_entitlement_refreshed(
                                    active_locale,
                                ),
                                Status::Success,
                                cx,
                            );
                        }
                        Err(err) => {
                            let kind =
                                ruum_core::watchdog::classify_ai_failure(&err);
                            let summary =
                                crate::i18n::messages::ai_analysis_error(
                                    &err,
                                    active_locale,
                                );
                            this.ai_last_error = Some(summary.clone());
                            this.ai_last_error_at_secs = Some(done_secs);
                            this.cloud_payment_required =
                                is_cloud_payment_required_message(&summary)
                                    || is_cloud_payment_required_message(&err);
                            if let Some(db) = open_db_for_window(this) {
                                let _ = db.record_ai_error(&summary, done_secs);
                            }
                            this.push_toast(summary.clone(), Status::Error, cx);
                            notif = Some((kind, active_locale, summary));
                        }
                    }
                    cx.notify();
                });

                if let Some((kind, notif_locale, summary)) = notif {
                    crate::post_app_notification(
                        ruum_core::watchdog::NOTIF_TAG_AI_ANALYSIS,
                        crate::i18n::messages::ai_analysis_failure_notif_title(
                            kind,
                            notif_locale,
                        ),
                        &summary,
                        cx,
                    );
                } else if updated.is_err()
                    && let Some(db) = open_db_for_background(db_path.as_deref())
                {
                    let _ = db.load_cloud_state();
                }
            });
        })
        .detach();
    }

    /// Switch the direct API wire protocol and update uncustomized default URLs
    /// and placeholders without overwriting user-entered custom URLs.
    pub fn set_watchdog_api_protocol(
        &mut self,
        protocol: ApiProtocol,
        window: &mut Window,
        cx: &mut Context<'_, Self>,
    ) {
        let changed = self.watchdog_draft.api_protocol != protocol;
        if changed {
            self.watchdog_draft.api_protocol = protocol;
            self.watchdog_test_epoch += 1;
            self.watchdog_fetch_epoch += 1;
            self.watchdog_benchmark_epoch += 1;
            self.watchdog_testing = false;
            self.watchdog_fetching_models = false;
            self.watchdog_benchmarking_models.clear();
            self.watchdog_models.clear();
            self.watchdog_speed_results.clear();
        }
        if let Some(ref input) = self.watchdog_input_api_base_url {
            let other_default = match protocol {
                ApiProtocol::OpenAi => {
                    ApiProtocol::Anthropic.default_base_url()
                }
                ApiProtocol::Anthropic => {
                    ApiProtocol::OpenAi.default_base_url()
                }
            };
            input.update(cx, |inp, cx| {
                inp.set_placeholder(protocol.default_base_url(), window, cx);
                let cur = inp.value().trim().to_owned();
                if cur.is_empty()
                    || cur.trim_end_matches('/')
                        == other_default.trim_end_matches('/')
                {
                    inp.set_value(
                        protocol.default_base_url().to_owned(),
                        window,
                        cx,
                    );
                }
            });
        }
        if changed {
            cx.notify();
        }
    }

    /// Clear the draft API key input and delete any stored direct API key in
    /// `LazyCredentialStore` (zero keychain calls when no encrypted file exists).
    pub fn clear_watchdog_api_key(
        &mut self,
        window: &mut Window,
        cx: &mut Context<'_, Self>,
    ) {
        if let Some(ref input) = self.watchdog_input_api_key
            && !input.read(cx).value().is_empty()
        {
            input.update(cx, |inp, cx| {
                inp.set_value(String::new(), window, cx);
            });
        }
        if self.watchdog_draft.ai_provider == AiCliProvider::Api
            && (self.cred_store.is_some() || self.db_path.is_some())
        {
            match self.ensure_cred_store() {
                Ok(store) => {
                    if let Err(err) = store.delete_ai_api_key() {
                        self.push_toast(
                            crate::i18n::messages::failed_save_settings(
                                &err,
                                self.locale,
                            ),
                            Status::Error,
                            cx,
                        );
                        return;
                    }
                }
                Err(err) => {
                    self.push_toast(
                        crate::i18n::messages::failed_save_settings(
                            &err,
                            self.locale,
                        ),
                        Status::Error,
                        cx,
                    );
                    return;
                }
            }
        }
        self.api_key_session_known = Some(false);
        self.push_toast(
            crate::i18n::messages::api_key_cleared(self.locale),
            Status::Success,
            cx,
        );
    }

    /// Populate the model override input from a discovered model choice.
    pub fn select_watchdog_model(
        &mut self,
        model_id: &str,
        window: &mut Window,
        cx: &mut Context<'_, Self>,
    ) {
        let trimmed = model_id.trim();
        self.watchdog_draft.model =
            (!trimmed.is_empty()).then(|| trimmed.to_string());
        if let Some(ref input) = self.watchdog_input_model {
            input.update(cx, |inp, cx| {
                inp.set_value(trimmed.to_string(), window, cx);
            });
        }
        cx.notify();
    }

    fn parse_strict_u64_input(
        slot: Option<&Entity<InputState>>,
        fallback: u64,
        cx: &App,
    ) -> Result<u64, ()> {
        match slot {
            Some(input) => {
                input.read(cx).value().trim().parse::<u64>().map_err(|_| ())
            }
            None => Ok(fallback),
        }
    }

    /// Save all Settings sections (General, Watchdog, Webhook, AI) to `ruum.db`,
    /// echo normalized values, update runtime cache, and sync OS service.
    pub fn save_watchdog_settings(
        &mut self,
        window: &mut Window,
        cx: &mut Context<'_, Self>,
    ) {
        let Ok(default_depth_raw) = Self::parse_strict_u64_input(
            self.setting_input_default_depth.as_ref(),
            u64::from(self.app_settings.default_depth),
            cx,
        ) else {
            self.push_toast(
                crate::i18n::messages::invalid_general_number(self.locale),
                Status::Error,
                cx,
            );
            return;
        };
        let Ok(ssh_duration_mins) = Self::parse_strict_u64_input(
            self.setting_input_ssh_duration.as_ref(),
            self.app_settings.ssh_session_duration_minutes,
            cx,
        ) else {
            self.push_toast(
                crate::i18n::messages::invalid_general_number(self.locale),
                Status::Error,
                cx,
            );
            return;
        };

        let fallback_poll_mins =
            self.watchdog_config.poll_interval_secs.div_ceil(60).max(1);
        let fallback_cool_mins = self
            .watchdog_config
            .cooldown_secs
            .div_ceil(60)
            .max(MIN_COOLDOWN_SECS / 60);

        let Ok(min_free_gb) = Self::parse_strict_u64_input(
            self.watchdog_input_min_free.as_ref(),
            self.watchdog_config.min_free_gb,
            cx,
        ) else {
            self.push_toast(
                crate::i18n::messages::invalid_watchdog_number(self.locale),
                Status::Error,
                cx,
            );
            return;
        };
        let Ok(poll_mins) = Self::parse_strict_u64_input(
            self.watchdog_input_poll_interval.as_ref(),
            fallback_poll_mins,
            cx,
        ) else {
            self.push_toast(
                crate::i18n::messages::invalid_watchdog_number(self.locale),
                Status::Error,
                cx,
            );
            return;
        };
        let Ok(cooldown_mins) = Self::parse_strict_u64_input(
            self.watchdog_input_cooldown.as_ref(),
            fallback_cool_mins,
            cx,
        ) else {
            self.push_toast(
                crate::i18n::messages::invalid_watchdog_number(self.locale),
                Status::Error,
                cx,
            );
            return;
        };

        let now_secs = u64::try_from(now_seconds()).unwrap_or(0);

        let custom_command =
            self.watchdog_input_custom_cmd.as_ref().map_or_else(
                || self.ai_config.custom_command.clone(),
                |input| input.read(cx).value().trim().to_string(),
            );
        let extra_args = self.watchdog_input_extra_args.as_ref().map_or_else(
            || self.ai_config.extra_args.clone(),
            |input| input.read(cx).value().trim().to_string(),
        );
        let api_base_url =
            self.watchdog_input_api_base_url.as_ref().map_or_else(
                || self.ai_config.api_base_url.clone(),
                |input| input.read(cx).value().trim().to_string(),
            );
        let api_key_draft = self
            .watchdog_input_api_key
            .as_ref()
            .map(|input| input.read(cx).value().trim().to_string())
            .filter(|val| !val.is_empty());
        let model =
            if self.watchdog_draft.ai_provider == AiCliProvider::RuumCloud {
                self.watchdog_draft.model.clone()
            } else {
                self.watchdog_input_model.as_ref().map_or_else(
                    || self.watchdog_draft.model.clone(),
                    |input| {
                        let text = input.read(cx).value().trim().to_string();
                        (!text.is_empty()).then_some(text)
                    },
                )
            };

        let next_app = AppSettings {
            power_preset: self.watchdog_draft.power_preset.key().to_owned(),
            default_depth: u32::try_from(default_depth_raw)
                .unwrap_or(ruum_core::storage::MAX_DEPTH),
            apparent_size: self.watchdog_draft.apparent_size,
            include_hidden: self.watchdog_draft.include_hidden,
            follow_system_appearance: self
                .watchdog_draft
                .follow_system_appearance,
            ssh_session_duration_minutes: ssh_duration_mins,
            close_to_background: self.watchdog_draft.close_to_background,
            launch_at_login: self.watchdog_draft.launch_at_login,
            tray_show_disk_space: self.watchdog_draft.tray_show_disk_space,
            updated_at_secs: now_secs,
        }
        .normalized();

        let min_cooldown_mins = MIN_COOLDOWN_SECS / 60;
        let next_cfg = WatchdogConfig {
            enabled: self.watchdog_draft.enabled,
            background_service: self.watchdog_draft.background_service,
            min_free_gb,
            poll_interval_secs: poll_mins.max(1).saturating_mul(60),
            cooldown_secs: cooldown_mins
                .max(min_cooldown_mins)
                .saturating_mul(60),
            updated_at_secs: now_secs,
        }
        .normalized();

        let next_ai = AiConfig {
            auto_analyze_on_alert: self.watchdog_draft.ai_auto_analyze,
            provider: self.watchdog_draft.ai_provider,
            custom_command,
            extra_args,
            model,
            api_protocol: self.watchdog_draft.api_protocol,
            api_base_url,
            setup_completed: self.ai_config.setup_completed,
            updated_at_secs: now_secs,
        }
        .normalized();

        if next_ai.provider == AiCliProvider::Api
            && let Some(ref new_key) = api_key_draft
        {
            let store = match self.ensure_cred_store() {
                Ok(store) => store,
                Err(err) => {
                    self.push_toast(
                        crate::i18n::messages::failed_save_settings(
                            &err,
                            self.locale,
                        ),
                        Status::Error,
                        cx,
                    );
                    return;
                }
            };
            if let Err(err) = store.set_ai_api_key(new_key) {
                self.push_toast(
                    crate::i18n::messages::failed_save_settings(
                        &err,
                        self.locale,
                    ),
                    Status::Error,
                    cx,
                );
                return;
            }
            self.api_key_session_known = Some(true);
        }

        if self.db_path.is_some()
            && let Err(err) = (|| -> std::io::Result<()> {
                let db = self.open_db()?;
                db.save_app_settings(&next_app)?;
                db.watchdog()?.save_config(&next_cfg)?;
                db.save_ai_config(&next_ai)?;
                Ok(())
            })()
        {
            self.push_toast(
                crate::i18n::messages::failed_save_settings(
                    &err.to_string(),
                    self.locale,
                ),
                Status::Error,
                cx,
            );
            return;
        }

        let prev_cfg =
            std::mem::replace(&mut self.watchdog_config, next_cfg.clone());
        let prev_app =
            std::mem::replace(&mut self.app_settings, next_app.clone());
        self.ai_config = next_ai.clone();
        if let Some(preset) =
            crate::power::PowerEfficiency::parse(&next_app.power_preset)
        {
            self.power_choice = Some(preset);
            self.options.threads = preset.policy(self.cpu_threads);
        }
        self.options.apparent_size = next_app.apparent_size;
        self.options.include_hidden = next_app.include_hidden;
        self.layout_options.max_depth = next_app.default_depth;
        self.cache = None;

        self.watchdog_draft =
            WatchdogSettingsDraft::from_configs(&next_app, &next_cfg, &next_ai);
        self.watchdog_inputs_need_sync = false;
        self.sync_watchdog_inputs_from_config(window, cx);

        crate::app_menu::set_close_to_background(
            cx,
            next_app.close_to_background,
        );

        let threshold_changed = prev_cfg.min_free_gb != next_cfg.min_free_gb;
        let re_enabled = !prev_cfg.enabled && next_cfg.enabled;
        if threshold_changed || re_enabled {
            self.watchdog_state.last_triggered_secs = None;
            self.watchdog_state.last_triggered_threshold_gb = None;
            if let Ok(db) = self.open_db()
                && let Ok(wd) = db.watchdog()
            {
                let _ = wd.save_state(&self.watchdog_state);
            }
        }

        let reset_poll = threshold_changed
            || re_enabled
            || prev_cfg.poll_interval_secs != next_cfg.poll_interval_secs;
        crate::sync_runtime_app_settings(&next_app, cx);
        crate::notify_watchdog_config_saved(&next_cfg, reset_poll, cx);

        if cfg!(test) {
            self.push_toast(
                crate::i18n::messages::settings_saved(self.locale),
                Status::Success,
                cx,
            );
            return;
        }

        let launch_at_login = next_app.launch_at_login;
        let needs_autostart_sync =
            launch_at_login || prev_app.launch_at_login != launch_at_login;
        if !needs_autostart_sync || self.watchdog_saving {
            self.push_toast(
                crate::i18n::messages::settings_saved(self.locale),
                Status::Success,
                cx,
            );
            return;
        }

        self.watchdog_saving = true;
        let task = cx.background_executor().spawn(async move {
            use ruum_core::autostart::AutostartStatus;

            let runner = ruum_core::ai_cli::TimedCommandRunner::new(
                Duration::from_secs(5),
            );
            let env = crate::background::current_autostart_env();
            let adapter = crate::background::autostart_adapter();
            let autostart_res = if launch_at_login {
                std::env::current_exe()
                    .and_then(|gui_exe| adapter.enable(&runner, &env, &gui_exe))
            } else {
                adapter
                    .disable(&runner, &env)
                    .map(|()| AutostartStatus::Disabled)
            };
            let _ = ruum_core::watchdog::uninstall_legacy_service_if_present(
                Duration::from_secs(2),
            );
            let status = ruum_core::watchdog::service_status(&runner).ok();
            (autostart_res, status)
        });

        cx.spawn(async move |this, cx| {
            use ruum_core::autostart::AutostartStatus;

            let (autostart_res, status) = task.await;
            let _ = this.update(cx, |this, cx| {
                this.watchdog_saving = false;
                this.watchdog_service_status = status;
                let locale = this.locale;
                match autostart_res {
                    Ok(AutostartStatus::Enabled | AutostartStatus::Disabled) => {
                        this.push_toast(
                            crate::i18n::messages::settings_saved(locale),
                            Status::Success,
                            cx,
                        );
                    }
                    Ok(AutostartStatus::RequiresApproval) => {
                        this.push_toast(
                            crate::i18n::messages::settings_saved_autostart_requires_approval(locale),
                            Status::Warning,
                            cx,
                        );
                    }
                    Err(err) => {
                        this.push_toast(
                            crate::i18n::messages::settings_saved_autostart_error(
                                &err.to_string(),
                                locale,
                            ),
                            Status::Warning,
                            cx,
                        );
                    }
                }
            });
        })
        .detach();
    }

    #[cfg(test)]
    fn resolve_cli_target(
        &self,
    ) -> std::io::Result<(
        ruum_core::cli_install::CliInstallOs,
        CliInstallEnv,
        PathBuf,
    )> {
        let env = self.cli_install_env.clone().ok_or_else(|| {
            std::io::Error::other(
                "CLI registration disabled in tests without injected env",
            )
        })?;
        let os = self
            .cli_install_os
            .unwrap_or_else(CliInstallFactory::current_os);
        let exe = self
            .cli_install_exe
            .clone()
            .unwrap_or_else(|| PathBuf::from("/tmp/ruum-test"));
        Ok((os, env, exe))
    }

    #[cfg(not(test))]
    fn resolve_cli_target() -> std::io::Result<(
        ruum_core::cli_install::CliInstallOs,
        CliInstallEnv,
        PathBuf,
    )> {
        let exe = std::env::current_exe()?;
        Ok((
            CliInstallFactory::current_os(),
            CliInstallEnv::current(),
            exe,
        ))
    }

    /// Install or update the `ruum` CLI command in `PATH` off the UI thread.
    pub fn install_or_update_cli(&mut self, cx: &mut Context<'_, Self>) {
        if self.cli_updating {
            return;
        }
        #[cfg(test)]
        let target = self.resolve_cli_target();
        #[cfg(not(test))]
        let target = Self::resolve_cli_target();
        self.cli_updating = true;
        cx.notify();

        let task = cx.background_executor().spawn(async move {
            let (os, env, exe) = target?;
            let adapter = CliInstallFactory::create_for(os);
            let runner = ruum_core::ai_cli::DefaultCommandRunner;
            let installed_path = adapter.install(&runner, &env, &exe)?;
            let status =
                adapter.status(&runner, &env, &exe).unwrap_or_else(|_| {
                    CliInstallStatus::Installed {
                        path: installed_path.clone(),
                        in_path: true,
                    }
                });
            Ok::<_, std::io::Error>((installed_path, status))
        });

        cx.spawn(async move |this, cx| {
            let res = task.await;
            let _ = this.update(cx, |this, cx| {
                this.cli_updating = false;
                let locale = this.locale;
                match res {
                    Ok((path, status)) => {
                        this.cli_install_status = Some(status);
                        this.push_toast(
                            crate::i18n::messages::cli_installed(
                                &path.display().to_string(),
                                locale,
                            ),
                            Status::Success,
                            cx,
                        );
                    }
                    Err(err) => {
                        this.push_toast(
                            crate::i18n::messages::cli_install_failed(
                                &err.to_string(),
                                locale,
                            ),
                            Status::Error,
                            cx,
                        );
                    }
                }
            });
        })
        .detach();
    }

    /// Uninstall the `ruum` CLI command from `PATH` off the UI thread.
    pub fn uninstall_ruum_cli(&mut self, cx: &mut Context<'_, Self>) {
        if self.cli_updating {
            return;
        }
        #[cfg(test)]
        let target = self.resolve_cli_target();
        #[cfg(not(test))]
        let target = Self::resolve_cli_target();
        self.cli_updating = true;
        cx.notify();

        let task = cx.background_executor().spawn(async move {
            let (os, env, exe) = target?;
            let adapter = CliInstallFactory::create_for(os);
            let runner = ruum_core::ai_cli::DefaultCommandRunner;
            adapter.uninstall(&runner, &env, &exe)?;
            let status = adapter
                .status(&runner, &env, &exe)
                .unwrap_or(CliInstallStatus::NotInstalled);
            Ok::<_, std::io::Error>(status)
        });

        cx.spawn(async move |this, cx| {
            let res = task.await;
            let _ = this.update(cx, |this, cx| {
                this.cli_updating = false;
                let locale = this.locale;
                match res {
                    Ok(status) => {
                        this.cli_install_status = Some(status);
                        this.push_toast(
                            crate::i18n::messages::cli_uninstalled(locale),
                            Status::Success,
                            cx,
                        );
                    }
                    Err(err) => {
                        this.push_toast(
                            crate::i18n::messages::cli_uninstall_failed(
                                &err.to_string(),
                                locale,
                            ),
                            Status::Error,
                            cx,
                        );
                    }
                }
            });
        })
        .detach();
    }

    // ── software updates ────────────────────────────────────────────────

    fn sync_update_runtime(&self, cx: &mut App) {
        crate::sync_runtime_update_state(
            self.update_state.clone(),
            self.update_settings.clone(),
            self.update_install_kind,
            self.update_banner_dismissed_version.clone(),
            self.update_download_cancel.clone(),
            self.update_last_error.clone(),
            cx,
        );
    }

    fn persist_update_settings(&mut self) {
        self.update_settings.updated_at_secs =
            u64::try_from(now_seconds()).unwrap_or(0);
        if self.db_path.is_none() {
            return;
        }
        if let Ok(db) = self.open_db() {
            let _ = db.save_update_settings(&self.update_settings);
        }
    }

    /// Open the centered software update card modal.
    pub fn open_update_modal(&mut self, cx: &mut Context<'_, Self>) {
        self.crumb_menu = None;
        self.power_menu = None;
        self.show_help = false;
        self.update_modal_open = true;
        self.focus_request = Some(FocusTarget::UpdateDialog);
        cx.notify();
    }

    /// Close the centered software update card modal without cancelling an
    /// active background download.
    pub fn close_update_modal(&mut self, cx: &mut Context<'_, Self>) {
        if self.update_modal_open {
            self.update_modal_open = false;
            self.focus_request = Some(FocusTarget::Root);
            cx.notify();
        }
    }

    /// Dismiss the first-seen-per-version non-blocking banner for the current
    /// update version.
    pub fn dismiss_update_banner(&mut self, cx: &mut Context<'_, Self>) {
        if let Some(info) = self.update_state.info() {
            self.update_banner_dismissed_version =
                Some(info.latest_version.clone());
            self.sync_update_runtime(cx);
            cx.notify();
        }
    }

    /// Apply a completed update check result onto `Ruum` and `AppRuntime`.
    pub fn apply_update_check_outcome(
        &mut self,
        outcome: Result<(UpdateSettings, UpdateCheckResult), String>,
        cx: &mut Context<'_, Self>,
    ) {
        match outcome {
            Ok((settings, result)) => {
                self.update_settings = settings;
                self.update_install_kind = result.install_kind;
                self.update_last_error = None;
                if result.update_available {
                    self.update_state = UpdateUiState::Available {
                        info: Arc::new(result),
                    };
                } else {
                    self.update_state = UpdateUiState::UpToDate {
                        checked_at_secs: self
                            .update_settings
                            .last_checked_at_secs,
                    };
                }
            }
            Err(err) => {
                let prev_info = self.update_state.info().cloned();
                self.update_last_error = Some(err.clone());
                self.update_state = UpdateUiState::Error {
                    message: err,
                    info: prev_info,
                };
            }
        }
        self.sync_update_runtime(cx);
        cx.notify();
    }

    /// Trigger an immediate update check off the UI thread.
    pub fn check_for_updates_now(&mut self, cx: &mut Context<'_, Self>) {
        if matches!(
            self.update_state,
            UpdateUiState::Checking
                | UpdateUiState::Downloading { .. }
                | UpdateUiState::Installing { .. }
        ) {
            return;
        }
        self.update_state = UpdateUiState::Checking;
        self.update_last_error = None;
        self.sync_update_runtime(cx);
        cx.notify();

        let db_path = self.db_path.clone();
        let settings = self.update_settings.clone();
        let install_kind = self.update_install_kind;
        let lang = locale_code(self.locale);

        let task = cx.background_executor().spawn(async move {
            run_update_check_blocking(
                db_path.as_deref(),
                &settings,
                install_kind,
                lang,
            )
        });
        cx.spawn(async move |this, cx| {
            let outcome = task.await;
            cx.update(|cx| {
                let fallback_outcome = outcome.clone();
                if this
                    .update(cx, |app, cx| {
                        app.apply_update_check_outcome(outcome, cx);
                    })
                    .is_err()
                {
                    let is_open = crate::update_open_main_window_checked(
                        cx,
                        |app, cx| {
                            app.apply_update_check_outcome(
                                fallback_outcome.clone(),
                                cx,
                            );
                        },
                    );
                    if !is_open {
                        apply_update_check_to_runtime(fallback_outcome, cx);
                    }
                }
            });
        })
        .detach();
    }

    /// Start downloading and verifying the selected update asset after explicit
    /// user action (`Download & Update`).
    pub fn start_update_download(&mut self, cx: &mut Context<'_, Self>) {
        let Some(info) = self.update_state.info().cloned() else {
            return;
        };
        if matches!(
            self.update_state,
            UpdateUiState::Downloading { .. }
                | UpdateUiState::Installing { .. }
        ) {
            return;
        }
        if !info.can_self_update {
            return;
        }
        let Some(asset) = info.selected_asset.clone() else {
            let msg = "No release package matches this platform".to_owned();
            self.update_last_error = Some(msg.clone());
            self.update_state = UpdateUiState::Error {
                message: msg,
                info: Some(info),
            };
            self.sync_update_runtime(cx);
            cx.notify();
            return;
        };

        let cancel = Arc::new(AtomicBool::new(false));
        let quitting = crate::runtime_update_quitting(cx);
        self.update_download_cancel = Some(Arc::clone(&cancel));
        self.update_last_error = None;
        self.update_state = UpdateUiState::Downloading {
            info: Arc::clone(&info),
            downloaded: 0,
            total: asset.size,
        };
        self.sync_update_runtime(cx);
        cx.notify();

        let base_url = resolve_update_base_url();
        let dest_dir = std::env::temp_dir().join("ruum-updates");
        let progress_downloaded = Arc::new(AtomicU64::new(0));
        let progress_total = Arc::new(AtomicU64::new(asset.size));
        let done_slot: Arc<Mutex<Option<Result<PathBuf, String>>>> =
            Arc::new(Mutex::new(None));
        let signal = Arc::new(crate::AsyncSignal::default());

        let dl_atomic = Arc::clone(&progress_downloaded);
        let tot_atomic = Arc::clone(&progress_total);
        let dl_done = Arc::clone(&done_slot);
        let dl_signal = Arc::clone(&signal);
        let dl_cancel = Arc::clone(&cancel);

        let spawn_res = std::thread::Builder::new()
            .name("ruum-update-dl".to_owned())
            .spawn(move || {
                let mut last_wake = Instant::now();
                let res = download_and_verify_asset(
                    &base_url,
                    &asset,
                    &dest_dir,
                    &dl_cancel,
                    |dl, total| {
                        if let Some(ref q) = quitting
                            && q.load(Ordering::Relaxed)
                        {
                            dl_cancel.store(true, Ordering::Release);
                        }
                        dl_atomic.store(dl, Ordering::Relaxed);
                        tot_atomic.store(total, Ordering::Relaxed);
                        if last_wake.elapsed() >= Duration::from_millis(80)
                            || (total > 0 && dl >= total)
                        {
                            last_wake = Instant::now();
                            dl_signal.notify();
                        }
                    },
                )
                .map_err(|e| e.to_string());
                if let Ok(mut guard) = dl_done.lock() {
                    *guard = Some(res);
                }
                dl_signal.notify();
            });

        if let Err(err) = spawn_res {
            let msg = err.to_string();
            self.update_download_cancel = None;
            self.update_last_error = Some(msg.clone());
            self.update_state = UpdateUiState::Error {
                message: msg,
                info: Some(info),
            };
            self.sync_update_runtime(cx);
            cx.notify();
            return;
        }

        cx.spawn(async move |this, cx| {
            loop {
                signal.wait().await;
                let done = done_slot.lock().ok().and_then(|mut g| g.take());
                let dl = progress_downloaded.load(Ordering::Relaxed);
                let total = progress_total.load(Ordering::Relaxed);
                let cancelled = cancel.load(Ordering::Relaxed);

                let finished = done.is_some();
                cx.update(|cx| {
                    let apply = |app: &mut Self, cx: &mut Context<'_, Self>| {
                        apply_download_progress_or_result(
                            app,
                            &info,
                            dl,
                            total,
                            cancelled,
                            done.clone(),
                            cx,
                        );
                    };
                    if this.update(cx, apply).is_err() {
                        let updated =
                            crate::update_open_main_window_checked(cx, apply);
                        if !updated {
                            apply_download_to_runtime(
                                &info, dl, total, cancelled, done, cx,
                            );
                        }
                    }
                });
                if finished {
                    break;
                }
            }
        })
        .detach();
    }

    /// Cancel an active update download and return to `Available`.
    pub fn cancel_update_download(&mut self, cx: &mut Context<'_, Self>) {
        if let Some(cancel) = self.update_download_cancel.take() {
            cancel.store(true, Ordering::Release);
        }
        if let UpdateUiState::Downloading { info, .. } = &self.update_state {
            self.update_state = UpdateUiState::Available {
                info: Arc::clone(info),
            };
        }
        self.sync_update_runtime(cx);
        cx.notify();
    }

    /// Apply a verified downloaded update package and quit when restart is
    /// required.
    pub fn apply_ready_update(&mut self, cx: &mut Context<'_, Self>) {
        let UpdateUiState::ReadyToInstall {
            info,
            artifact_path,
        } = &self.update_state
        else {
            return;
        };
        let info = Arc::clone(info);
        let Some(manifest) = info.manifest.clone() else {
            let msg = "No release manifest available".to_owned();
            self.update_last_error = Some(msg.clone());
            self.update_state = UpdateUiState::Error {
                message: msg,
                info: Some(info),
            };
            self.sync_update_runtime(cx);
            cx.notify();
            return;
        };
        let artifact_path = artifact_path.clone();
        let kind = self.update_install_kind;

        self.update_state = UpdateUiState::Installing {
            info: Arc::clone(&info),
        };
        self.sync_update_runtime(cx);
        cx.notify();

        let task = cx.background_executor().spawn(async move {
            let env = UpdateEnv::current();
            let adapter = UpdateFactory::create();
            let runner = ruum_core::ai_cli::DefaultCommandRunner;
            adapter
                .apply_update(&env, kind, &artifact_path, &manifest, &runner)
                .map_err(|e| e.to_string())
        });

        cx.spawn(async move |this, cx| {
            let res = task.await;
            cx.update(|cx| match res {
                Ok(
                    ApplyOutcome::RestartRequired { .. }
                    | ApplyOutcome::InstallerLaunched { .. },
                ) => {
                    crate::app_menu::request_quit(cx);
                }
                Ok(ApplyOutcome::ManualActionRequired { .. }) => {
                    let _ = this.update(cx, |app, cx| {
                        app.update_state = UpdateUiState::Available {
                            info: Arc::clone(&info),
                        };
                        app.sync_update_runtime(cx);
                        cx.notify();
                    });
                }
                Err(err) => {
                    let _ = this.update(cx, |app, cx| {
                        app.update_last_error = Some(err.clone());
                        app.update_state = UpdateUiState::Error {
                            message: err,
                            info: Some(Arc::clone(&info)),
                        };
                        app.sync_update_runtime(cx);
                        cx.notify();
                    });
                }
            });
        })
        .detach();
    }

    /// Skip the currently offered update version and persist to `ruum.db`.
    pub fn skip_update_version(&mut self, cx: &mut Context<'_, Self>) {
        let Some(version) =
            self.update_state.info().map(|i| i.latest_version.clone())
        else {
            self.close_update_modal(cx);
            return;
        };
        self.update_settings.skipped_version = Some(version.clone());
        self.update_banner_dismissed_version = Some(version);
        self.update_modal_open = false;
        self.focus_request = Some(FocusTarget::Root);
        self.persist_update_settings();
        self.sync_update_runtime(cx);
        cx.notify();
    }

    /// Switch the preferred update channel (`Stable` / `Beta`) and persist.
    pub fn set_update_channel(
        &mut self,
        channel: UpdateChannel,
        cx: &mut Context<'_, Self>,
    ) {
        if self.update_settings.channel == channel {
            return;
        }
        self.update_settings.channel = channel;
        self.persist_update_settings();
        self.sync_update_runtime(cx);
        cx.notify();
    }

    /// Toggle automatic background update checking and persist.
    pub fn set_update_auto_check(
        &mut self,
        auto_check: bool,
        cx: &mut Context<'_, Self>,
    ) {
        if self.update_settings.auto_check == auto_check {
            return;
        }
        self.update_settings.auto_check = auto_check;
        self.persist_update_settings();
        self.sync_update_runtime(cx);
        cx.notify();
    }

    /// Toggle sending the anonymous `UUIDv4` install ID and persist.
    pub fn set_update_send_client_id(
        &mut self,
        send_client_id: bool,
        cx: &mut Context<'_, Self>,
    ) {
        if self.update_settings.send_client_id == send_client_id {
            return;
        }
        self.update_settings.send_client_id = send_client_id;
        self.persist_update_settings();
        self.sync_update_runtime(cx);
        cx.notify();
    }

    /// Copy the recommended CLI or package-manager upgrade command.
    pub fn copy_update_command(&mut self, cx: &mut Context<'_, Self>) {
        let Some(cmd) =
            self.update_state.info().map(|i| i.upgrade_command.clone())
        else {
            return;
        };
        cx.write_to_clipboard(gpui_kit::ClipboardItem::new_string(cmd));
        self.push_toast(
            crate::i18n::messages::update_command_copied(self.locale),
            Status::Success,
            cx,
        );
    }

    /// Resolve the release page URL for the current update state, falling
    /// back to the repository releases index when no manifest is present.
    #[must_use]
    pub fn resolved_update_release_url(&self) -> String {
        let fallback =
            format!("https://github.com/{DEFAULT_GITHUB_REPO}/releases");
        self.update_state
            .info()
            .and_then(|i| i.manifest.as_ref())
            .map_or(fallback, |m| m.release_url.clone())
    }

    /// Open the validated GitHub Release page in the system browser.
    pub fn open_update_release_page(&self, cx: &App) {
        let raw_url = self.resolved_update_release_url();
        if let Ok(parsed) = validate_update_url(&raw_url) {
            cx.open_url(parsed.as_str());
        }
    }

    /// Test AI CLI connectivity off the UI thread (separate from model fetch).
    pub fn test_watchdog_ai_cli(&mut self, cx: &mut Context<'_, Self>) {
        if self.watchdog_testing {
            return;
        }
        let temp_cfg = self.snapshot_ai_config(cx);
        let ai_ctx = self.snapshot_ai_execution_context(cx);
        let provider_label = temp_cfg.provider.label_in(self.locale);
        self.watchdog_testing = true;
        self.watchdog_test_epoch += 1;
        let epoch = self.watchdog_test_epoch;
        let toast_msg = match temp_cfg.provider {
            AiCliProvider::Api => {
                crate::i18n::messages::testing_api_connection(
                    temp_cfg.api_protocol.label_in(self.locale),
                    self.locale,
                )
            }
            AiCliProvider::RuumCloud => {
                crate::i18n::messages::testing_cloud_connection(self.locale)
                    .to_owned()
            }
            _ => crate::i18n::messages::testing_cli_connection(
                provider_label,
                self.locale,
            ),
        };
        self.push_toast(toast_msg, Status::Neutral, cx);

        let is_cloud = temp_cfg.provider == AiCliProvider::RuumCloud;
        let task = cx.background_executor().spawn(async move {
            let runner = ruum_core::ai_cli::TimedCommandRunner::new(
                Duration::from_secs(
                    ruum_core::ai_cli::SHORT_COMMAND_TIMEOUT_SECS,
                ),
            );
            ruum_core::ai_cli::test_connection_for_config_with_context(
                &temp_cfg, &runner, &ai_ctx,
            )
        });

        cx.spawn(async move |this, cx| {
            let res = task.await;
            let _ = this.update(cx, |this, cx| {
                if epoch != this.watchdog_test_epoch {
                    return;
                }
                this.watchdog_testing = false;
                if is_cloud {
                    this.reload_cloud_state();
                    this.sync_selected_cloud_model();
                }
                match res {
                    Ok(version_msg) => {
                        if is_cloud {
                            this.cloud_payment_required = this
                                .cached_cloud_models()
                                .iter()
                                .all(|m| !m.available)
                                && !this.cached_cloud_models().is_empty();
                        }
                        this.push_toast(version_msg, Status::Success, cx);
                    }
                    Err(err) => {
                        if is_cloud && is_cloud_payment_required_message(&err) {
                            this.cloud_payment_required = true;
                        }
                        this.push_toast(err, Status::Error, cx);
                    }
                }
            });
        })
        .detach();
    }

    /// Discover available models from the selected AI CLI provider.
    pub fn fetch_watchdog_models(&mut self, cx: &mut Context<'_, Self>) {
        if self.watchdog_fetching_models {
            return;
        }
        let temp_cfg = self.snapshot_ai_config(cx);
        let ai_ctx = self.snapshot_ai_execution_context(cx);
        let provider_label = temp_cfg.provider.label_in(self.locale);
        let is_cloud = temp_cfg.provider == AiCliProvider::RuumCloud;
        self.watchdog_fetching_models = true;
        self.watchdog_fetch_epoch += 1;
        let epoch = self.watchdog_fetch_epoch;
        self.push_toast(
            crate::i18n::messages::fetching_models(provider_label, self.locale),
            Status::Neutral,
            cx,
        );

        let task = cx.background_executor().spawn(async move {
            let runner = ruum_core::ai_cli::TimedCommandRunner::new(
                Duration::from_secs(
                    ruum_core::ai_cli::SHORT_COMMAND_TIMEOUT_SECS,
                ),
            );
            ruum_core::ai_cli::discover_models_for_config_with_context(
                &temp_cfg, &runner, &ai_ctx,
            )
        });

        cx.spawn(async move |this, cx| {
            let res = task.await;
            let _ = this.update(cx, |this, cx| {
                if epoch != this.watchdog_fetch_epoch {
                    return;
                }
                this.watchdog_fetching_models = false;
                if is_cloud {
                    this.reload_cloud_state();
                    this.sync_selected_cloud_model();
                }
                let locale = this.locale;
                match res {
                    Ok((models, note)) => {
                        let count = models.len();
                        this.watchdog_models = models;
                        let (msg, status) = match (count, note) {
                            (0, Some(n)) => (n, Status::Warning),
                            (0, None) => (
                                crate::i18n::messages::no_models_discovered(
                                    provider_label,
                                    locale,
                                ),
                                Status::Warning,
                            ),
                            (n, Some(n_note)) => (
                                crate::i18n::messages::loaded_models_with_note(
                                    n,
                                    &n_note,
                                    locale,
                                ),
                                Status::Success,
                            ),
                            (n, None) => (
                                crate::i18n::messages::loaded_models_for_provider(
                                    n,
                                    provider_label,
                                    locale,
                                ),
                                Status::Success,
                            ),
                        };
                        this.push_toast(msg, status, cx);
                    }
                    Err(err) => {
                        this.watchdog_models.clear();
                        this.push_toast(err, Status::Error, cx);
                    }
                }
            });
        })
        .detach();
    }

    fn upsert_speed_result(
        &mut self,
        result: ruum_core::ai_cli::ModelSpeedResult,
    ) {
        if let Some(existing) = self
            .watchdog_speed_results
            .iter_mut()
            .find(|item| item.model == result.model)
        {
            *existing = result;
        } else {
            self.watchdog_speed_results.insert(0, result);
        }
    }

    /// Measure response latency in milliseconds for a chosen or configured model.
    pub fn benchmark_watchdog_model(
        &mut self,
        model_override: Option<String>,
        cx: &mut Context<'_, Self>,
    ) {
        let temp_cfg = self.snapshot_ai_config(cx);
        let ai_ctx = self.snapshot_ai_execution_context(cx);
        let target_key = model_override
            .as_deref()
            .map(str::trim)
            .filter(|s| !s.is_empty())
            .map(ToString::to_string)
            .or_else(|| temp_cfg.model.clone())
            .unwrap_or_else(|| "(default)".to_string());
        if self.watchdog_benchmarking_models.contains(&target_key) {
            return;
        }
        self.watchdog_benchmarking_models.insert(target_key.clone());
        let epoch = self.watchdog_benchmark_epoch;
        cx.notify();

        let task = cx.background_executor().spawn(async move {
            let runner = ruum_core::ai_cli::TimedCommandRunner::new(
                Duration::from_secs(
                    ruum_core::ai_cli::SHORT_COMMAND_TIMEOUT_SECS,
                ),
            );
            ruum_core::ai_cli::benchmark_model_for_config_with_context(
                &temp_cfg,
                model_override.as_deref(),
                &runner,
                &ai_ctx,
            )
        });

        cx.spawn(async move |this, cx| {
            let result = task.await;
            let _ = this.update(cx, |this, cx| {
                this.watchdog_benchmarking_models.remove(&target_key);
                if epoch != this.watchdog_benchmark_epoch {
                    cx.notify();
                    return;
                }
                let failed = (!result.ok).then(|| {
                    crate::i18n::messages::model_speed_failed(
                        &result.model,
                        &result.detail,
                        this.locale,
                    )
                });
                this.upsert_speed_result(result);
                if let Some(err) = failed {
                    this.push_toast(err, Status::Error, cx);
                } else {
                    cx.notify();
                }
            });
        })
        .detach();
    }

    /// Benchmark latency sequentially for every discovered model.
    pub fn benchmark_all_watchdog_models(
        &mut self,
        cx: &mut Context<'_, Self>,
    ) {
        let temp_cfg = self.snapshot_ai_config(cx);
        let ai_ctx = self.snapshot_ai_execution_context(cx);
        let models_to_test: Vec<String> = self
            .watchdog_models
            .iter()
            .filter(|m| !self.watchdog_benchmarking_models.contains(&m.id))
            .map(|m| m.id.clone())
            .collect();
        if models_to_test.is_empty() {
            if self.watchdog_models.is_empty() {
                self.push_toast(
                    crate::i18n::messages::fetch_models_first(self.locale),
                    Status::Warning,
                    cx,
                );
            }
            return;
        }
        for model_id in &models_to_test {
            self.watchdog_benchmarking_models.insert(model_id.clone());
        }
        let epoch = self.watchdog_benchmark_epoch;
        let total = models_to_test.len();
        self.push_toast(
            crate::i18n::messages::benchmarking_models(total, self.locale),
            Status::Neutral,
            cx,
        );

        cx.spawn(async move |this, cx| {
            let mut ok_count = 0usize;
            let mut fail_count = 0usize;
            for model_id in models_to_test {
                let cfg_clone = temp_cfg.clone();
                let ctx_clone = ai_ctx.clone();
                let id_for_task = model_id.clone();
                let task = cx.background_executor().spawn(async move {
                    let runner = ruum_core::ai_cli::TimedCommandRunner::new(
                        Duration::from_secs(
                            ruum_core::ai_cli::SHORT_COMMAND_TIMEOUT_SECS,
                        ),
                    );
                    ruum_core::ai_cli::benchmark_model_for_config_with_context(
                        &cfg_clone,
                        Some(&id_for_task),
                        &runner,
                        &ctx_clone,
                    )
                });
                let result = task.await;
                let keep_going = this.update(cx, |this, cx| {
                    this.watchdog_benchmarking_models.remove(&model_id);
                    if epoch != this.watchdog_benchmark_epoch {
                        cx.notify();
                        return false;
                    }
                    if result.ok {
                        ok_count += 1;
                    } else {
                        fail_count += 1;
                    }
                    this.upsert_speed_result(result);
                    cx.notify();
                    true
                });
                if !matches!(keep_going, Ok(true)) {
                    return;
                }
            }
            let _ = this.update(cx, |this, cx| {
                if epoch != this.watchdog_benchmark_epoch {
                    return;
                }
                let status = if fail_count == 0 {
                    Status::Success
                } else if ok_count == 0 {
                    Status::Error
                } else {
                    Status::Warning
                };
                this.push_toast(
                    crate::i18n::messages::speed_test_finished(
                        ok_count,
                        total,
                        this.locale,
                    ),
                    status,
                    cx,
                );
            });
        })
        .detach();
    }

    /// Silently refresh `watchdog_home_space` and `watchdog_space_checked_at`
    /// without emitting user-facing toast notices.
    pub(crate) fn refresh_watchdog_home_space_silent(&mut self) {
        let target =
            std::env::home_dir().filter(|p| p.exists()).or_else(|| {
                (self.remote.is_none() && self.root_path.exists())
                    .then(|| self.root_path.clone())
            });
        let Some(target) = target else {
            return;
        };
        if let Ok(info) = space_info(&target) {
            let now_secs = u64::try_from(now_seconds()).unwrap_or(0);
            self.watchdog_home_space = Some(info);
            if self.remote.is_none() && self.root_path == target {
                self.space = Some(info);
            }
            self.watchdog_space_checked_at = Some(now_secs);
            self.watchdog_state.last_checked_secs = Some(now_secs);
        }
    }

    /// Block Ruum Cloud analysis before network when cached virtual models
    /// exist and every model has `available == false`.
    fn abort_if_cloud_models_locked(
        &mut self,
        ai_cfg: &AiConfig,
        ssh_alias: Option<&str>,
        cx: &mut Context<'_, Self>,
    ) -> bool {
        if ai_cfg.provider != AiCliProvider::RuumCloud {
            return false;
        }
        let cached = self.cached_cloud_models();
        if cached.is_empty() || cached.iter().any(|m| m.available) {
            return false;
        }
        let now_secs = u64::try_from(now_seconds()).unwrap_or(0);
        let raw = crate::i18n::messages::cloud_no_available_models_blocked(
            self.locale,
        );
        let summary = match ssh_alias {
            Some(alias) => crate::i18n::messages::ssh_ai_analyze_failed(
                alias,
                raw,
                self.locale,
            ),
            None => crate::i18n::messages::ai_analysis_error(raw, self.locale),
        };
        self.cloud_payment_required = true;
        self.ai_last_error = Some(summary.clone());
        self.ai_last_error_at_secs = Some(now_secs);
        if let Some(db) = open_db_for_window(self) {
            let _ = db.record_ai_error(&summary, now_secs);
        }
        self.push_toast(summary.clone(), Status::Error, cx);
        crate::post_app_notification(
            ruum_core::watchdog::NOTIF_TAG_AI_ANALYSIS,
            crate::i18n::messages::ai_analysis_failure_notif_title(
                ruum_core::watchdog::AiFailureKind::Failed,
                self.locale,
            ),
            &summary,
            cx,
        );
        cx.notify();
        true
    }

    /// Trigger manual AI analysis off the UI thread and open confirmation modal.
    pub fn run_ai_analysis_now(&mut self, cx: &mut Context<'_, Self>) {
        if self.watchdog_analyzing {
            return;
        }
        if !self.ai_config.setup_completed {
            self.ai_setup_choice_modal = Some(AiSetupChoiceModalState {
                pending_action: PendingAiSetupAction::LocalAnalysis,
            });
            self.focus_request = Some(FocusTarget::AiSetupChoiceDialog);
            cx.notify();
            return;
        }
        let Some(home) = std::env::home_dir() else {
            self.push_toast(
                crate::i18n::messages::cannot_find_home_dir(self.locale),
                Status::Error,
                cx,
            );
            return;
        };
        let ai_cfg = self.snapshot_ai_config(cx);
        if self.abort_if_cloud_models_locked(&ai_cfg, None, cx) {
            return;
        }
        let is_cloud = ai_cfg.provider == AiCliProvider::RuumCloud;
        let ai_ctx = self.snapshot_ai_execution_context(cx);
        let provider_label = ai_cfg.provider.label_in(self.locale);
        let cli_exe = crate::resolve_cli_exe();
        let now_secs = u64::try_from(now_seconds()).unwrap_or(0);
        let existing_tree = if self.active_host_tab == HostTabKey::Local {
            (self.remote.is_none() && self.root_path == home)
                .then(|| self.tree.clone())
                .flatten()
        } else {
            self.host_tabs
                .iter()
                .find(|tab| tab.key == HostTabKey::Local)
                .and_then(|tab| tab.session.as_ref())
                .and_then(|snap| {
                    (snap.remote.is_none() && snap.root_path == home)
                        .then(|| snap.tree.clone())
                        .flatten()
                })
        };

        let db_path = self.db_path.clone();
        let locale = self.locale;

        self.watchdog_analyzing = true;
        self.watchdog_analysis_epoch += 1;
        let epoch = self.watchdog_analysis_epoch;
        self.push_toast(
            crate::i18n::messages::running_ai_analysis(
                provider_label,
                self.locale,
            ),
            Status::Neutral,
            cx,
        );

        let task = cx.background_executor().spawn(async move {
            let runner = ruum_core::ai_cli::DefaultCommandRunner;
            let res =
                ruum_core::ai_cli::run_analysis_with_tree_for_config_with_context(
                    &ai_cfg,
                    &home,
                    &cli_exe,
                    now_secs,
                    existing_tree.as_deref(),
                    &runner,
                    &ai_ctx,
                );
            (home, res)
        });

        cx.spawn(async move |this, cx| {
            let (home, res) = task.await;
            let err_raw = res.as_ref().err().cloned();
            cx.update(|cx| {
                let done_secs =
                    u64::try_from(now_seconds()).unwrap_or(now_secs);
                let mut notif: Option<(
                    ruum_core::watchdog::AiFailureKind,
                    crate::i18n::Locale,
                    String,
                )> = None;
                let updated = this.update(cx, |this, cx| {
                    if epoch != this.watchdog_analysis_epoch {
                        return false;
                    }
                    this.watchdog_analyzing = false;
                    if is_cloud {
                        this.reload_cloud_state();
                        this.sync_selected_cloud_model();
                    }
                    let active_locale = this.locale;
                    match res {
                        Ok(proposal) => {
                            this.ai_proposal = Some(proposal.clone());
                            this.ai_last_error = None;
                            this.ai_last_error_at_secs = None;
                            if is_cloud {
                                this.cloud_payment_required = false;
                            }
                            let ai_state =
                                ruum_core::storage::AiState::from_proposal(
                                    &proposal, done_secs,
                                )
                                .unwrap_or_default();
                            if let Some(db) = open_db_for_window(this) {
                                let _ = db.save_ai_state(&ai_state);
                            }
                            if proposal.items.is_empty()
                                && proposal.docker_targets.is_empty()
                                && build_docker_confirm_items(
                                    &this.local_docker_status(),
                                    &[],
                                    active_locale,
                                )
                                .0
                                .is_empty()
                            {
                                this.push_toast(
                                    crate::i18n::messages::ai_found_no_files(
                                        active_locale,
                                    ),
                                    Status::Success,
                                    cx,
                                );
                            } else {
                                let local_docker = this.local_docker_status();
                                let request =
                                    proposal_confirm_request_with_docker(
                                        home,
                                        &proposal,
                                        &local_docker,
                                        active_locale,
                                    );
                                let count = request.items.len()
                                    + request.docker_items.len();
                                crate::open_local_confirm_window(
                                    request,
                                    this.db_path.clone(),
                                    local_docker,
                                    cx,
                                );
                                crate::refresh_tray_status(cx);
                                this.push_toast(
                                    crate::i18n::messages::ai_proposed_items(
                                        count,
                                        active_locale,
                                    ),
                                    Status::Success,
                                    cx,
                                );
                            }
                        }
                        Err(err) => {
                            let kind =
                                ruum_core::watchdog::classify_ai_failure(&err);
                            let summary =
                                crate::i18n::messages::ai_analysis_error(
                                    &err,
                                    active_locale,
                                );
                            this.ai_last_error = Some(summary.clone());
                            this.ai_last_error_at_secs = Some(done_secs);
                            if is_cloud
                                && (is_cloud_payment_required_message(&summary)
                                    || is_cloud_payment_required_message(&err))
                            {
                                this.cloud_payment_required = true;
                            }
                            if let Some(db) = open_db_for_window(this) {
                                let _ = db.record_ai_error(&summary, done_secs);
                            }
                            this.push_toast(
                                summary.clone(),
                                Status::Error,
                                cx,
                            );
                            notif = Some((kind, active_locale, summary));
                        }
                    }
                    cx.notify();
                    true
                });

                match updated {
                    Ok(false) => {}
                    Ok(true) => {
                        if let Some((kind, notif_locale, summary)) = notif {
                            crate::post_app_notification(
                                ruum_core::watchdog::NOTIF_TAG_AI_ANALYSIS,
                                crate::i18n::messages::ai_analysis_failure_notif_title(
                                    kind,
                                    notif_locale,
                                ),
                                &summary,
                                cx,
                            );
                        }
                    }
                    Err(_) => {
                        if let Some(err) = err_raw {
                            let kind =
                                ruum_core::watchdog::classify_ai_failure(&err);
                            let summary =
                                crate::i18n::messages::ai_analysis_error(
                                    &err, locale,
                                );
                            if let Some(db) =
                                open_db_for_background(db_path.as_deref())
                            {
                                let _ = db.record_ai_error(&summary, done_secs);
                            }
                            crate::post_app_notification(
                                ruum_core::watchdog::NOTIF_TAG_AI_ANALYSIS,
                                crate::i18n::messages::ai_analysis_failure_notif_title(
                                    kind, locale,
                                ),
                                &summary,
                                cx,
                            );
                        }
                    }
                }
            });
        })
        .detach();
    }

    /// Open confirmation window for the most recently saved local proposal.
    pub fn open_last_proposal(&self, cx: &mut Context<'_, Self>) {
        let Some(proposal) = &self.ai_proposal else {
            return;
        };
        let Some(home) = std::env::home_dir() else {
            return;
        };
        let local_docker = self.local_docker_status();
        let request = proposal_confirm_request_with_docker(
            home,
            proposal,
            &local_docker,
            self.locale,
        );
        crate::open_local_confirm_window(
            request,
            self.db_path.clone(),
            local_docker,
            cx,
        );
        cx.notify();
    }

    /// Remove confirmed/cleaned paths from the local Watchdog AI proposal.
    pub fn prune_local_ai_proposal(&mut self, cleaned_paths: &[PathBuf]) {
        if let Some(proposal) = &mut self.ai_proposal {
            proposal
                .items
                .retain(|item| !cleaned_paths.contains(&item.path));
            if proposal.items.is_empty() && proposal.docker_targets.is_empty() {
                self.ai_proposal = None;
            }
        }
    }

    // ── SSH Server Cards & Credentials ────────────────────────────────────

    /// Open the inline SSH server form to add a new profile.
    pub fn open_add_ssh_form(&mut self, cx: &mut Context<'_, Self>) {
        self.ssh_form_open = true;
        self.ssh_form_editing_id = None;
        self.ssh_form_auth_mode = ruum_core::storage::AuthMode::PrivateKey;
        self.ssh_form_error = None;
        self.ssh_form_needs_sync = true;
        cx.notify();
    }

    /// Open the inline SSH server form populated with an existing profile.
    /// Does not read the OS keychain.
    pub fn open_edit_ssh_form(&mut self, id: &str, cx: &mut Context<'_, Self>) {
        let Some(profile) =
            self.ssh_profiles.iter().find(|p| p.id == id).cloned()
        else {
            return;
        };
        self.ssh_form_open = true;
        self.ssh_form_editing_id = Some(profile.id);
        self.ssh_form_auth_mode = profile.auth_mode;
        self.ssh_form_error = None;
        self.ssh_form_needs_sync = true;
        cx.notify();
    }

    /// Close the inline SSH server form without saving.
    pub fn close_ssh_form(&mut self, cx: &mut Context<'_, Self>) {
        self.ssh_form_open = false;
        self.ssh_form_editing_id = None;
        self.ssh_form_error = None;
        self.ssh_form_needs_sync = false;
        if self.volumes_open {
            self.focus_request = Some(FocusTarget::Dialog);
        } else if self.screen == Screen::Remote {
            self.focus_request = Some(FocusTarget::Root);
        }
        cx.notify();
    }

    /// Switch authentication mode inside the SSH server form.
    pub fn set_ssh_form_auth_mode(
        &mut self,
        mode: ruum_core::storage::AuthMode,
        cx: &mut Context<'_, Self>,
    ) {
        self.ssh_form_auth_mode = mode;
        self.ssh_form_error = None;
        cx.notify();
    }

    /// Ensure the SSH form `InputState` entities exist and sync if dirty.
    pub fn ensure_ssh_form_inputs(
        &mut self,
        window: &mut Window,
        cx: &mut Context<'_, Self>,
    ) {
        let locale = self.locale;
        if self.ssh_input_alias.is_none() {
            self.ssh_input_alias = Some(cx.new(|cx| {
                InputState::new(window, cx).placeholder(
                    crate::i18n::pages::placeholder_ssh_alias(locale),
                )
            }));
        }
        if self.ssh_input_host.is_none() {
            self.ssh_input_host = Some(cx.new(|cx| {
                InputState::new(window, cx).placeholder(
                    crate::i18n::pages::placeholder_ssh_host_addr(locale),
                )
            }));
        }
        if self.ssh_input_port.is_none() {
            self.ssh_input_port = Some(cx.new(|cx| {
                InputState::new(window, cx)
                    .placeholder("22")
                    .default_value("22")
            }));
        }
        if self.ssh_input_username.is_none() {
            self.ssh_input_username = Some(cx.new(|cx| {
                InputState::new(window, cx).placeholder(
                    crate::i18n::pages::placeholder_ssh_username(locale),
                )
            }));
        }
        if self.ssh_input_default_path.is_none() {
            self.ssh_input_default_path = Some(cx.new(|cx| {
                InputState::new(window, cx).placeholder(
                    crate::i18n::pages::placeholder_ssh_default_path(locale),
                )
            }));
        }
        if self.ssh_input_password.is_none() {
            self.ssh_input_password = Some(cx.new(|cx| {
                InputState::new(window, cx).masked(true).placeholder(
                    crate::i18n::pages::placeholder_ssh_password(locale),
                )
            }));
        }
        if self.ssh_input_private_key.is_none() {
            self.ssh_input_private_key = Some(cx.new(|cx| {
                InputState::new(window, cx).placeholder(
                    crate::i18n::pages::placeholder_ssh_private_key(locale),
                )
            }));
        }
        if self.ssh_input_passphrase.is_none() {
            self.ssh_input_passphrase = Some(cx.new(|cx| {
                InputState::new(window, cx).masked(true).placeholder(
                    crate::i18n::pages::placeholder_ssh_passphrase(locale),
                )
            }));
        }
        if self.ssh_form_needs_sync {
            self.sync_ssh_form_inputs(window, cx);
            self.ssh_form_needs_sync = false;
        }
    }

    fn sync_ssh_form_inputs(
        &self,
        window: &mut Window,
        cx: &mut Context<'_, Self>,
    ) {
        let profile = self
            .ssh_form_editing_id
            .as_deref()
            .and_then(|id| self.ssh_profiles.iter().find(|p| p.id == id));
        let mut set_val = |slot: &Option<Entity<InputState>>, val: &str| {
            if let Some(input) = slot {
                input.update(cx, |state, cx| state.set_value(val, window, cx));
            }
        };
        if let Some(p) = profile {
            set_val(&self.ssh_input_alias, &p.alias);
            set_val(&self.ssh_input_host, &p.host);
            set_val(&self.ssh_input_port, &p.port.to_string());
            set_val(&self.ssh_input_username, &p.username);
            set_val(
                &self.ssh_input_default_path,
                p.default_path
                    .as_deref()
                    .filter(|s| !s.trim().is_empty())
                    .unwrap_or("~"),
            );
        } else {
            set_val(&self.ssh_input_alias, "");
            set_val(&self.ssh_input_host, "");
            set_val(&self.ssh_input_port, "22");
            set_val(&self.ssh_input_username, "");
            set_val(&self.ssh_input_default_path, "~");
        }
        // Secrets stay blank unless the user explicitly enters a new value.
        set_val(&self.ssh_input_password, "");
        set_val(&self.ssh_input_private_key, "");
        set_val(&self.ssh_input_passphrase, "");
    }

    fn read_ssh_input(
        slot: Option<&Entity<InputState>>,
        cx: &Context<'_, Self>,
    ) -> String {
        slot.map(|input| input.read(cx).value().trim().to_owned())
            .unwrap_or_default()
    }

    fn resolve_ssh_form_secret(
        &self,
        store: &ruum_core::credentials::LazyCredentialStore,
        existing_id: Option<&str>,
        cx: &Context<'_, Self>,
    ) -> Result<Option<ruum_core::credentials::SshCredentialSecret>, String>
    {
        let password_raw =
            Self::read_ssh_input(self.ssh_input_password.as_ref(), cx);
        let key_raw =
            Self::read_ssh_input(self.ssh_input_private_key.as_ref(), cx);
        let passphrase_raw =
            Self::read_ssh_input(self.ssh_input_passphrase.as_ref(), cx);

        match self.ssh_form_auth_mode {
            ruum_core::storage::AuthMode::Password => {
                if password_raw.is_empty() {
                    if let Some(id) = existing_id
                        && matches!(
                            store.get_ssh_secret(id),
                            Ok(Some(
                                ruum_core::credentials::SshCredentialSecret::Password {
                                    ..
                                }
                            ))
                        )
                    {
                        return Ok(None);
                    }
                    return Err(crate::i18n::messages::ssh_secret_required(
                        self.locale,
                    )
                    .to_owned());
                }
                Ok(Some(
                    ruum_core::credentials::SshCredentialSecret::Password {
                        password: password_raw,
                    },
                ))
            }
            ruum_core::storage::AuthMode::PrivateKey => {
                if key_raw.is_empty() {
                    if let Some(id) = existing_id
                        && matches!(
                            store.get_ssh_secret(id),
                            Ok(Some(
                                ruum_core::credentials::SshCredentialSecret::PrivateKey {
                                    ..
                                }
                            ))
                        )
                    {
                        return Ok(None);
                    }
                    return Err(crate::i18n::messages::ssh_secret_required(
                        self.locale,
                    )
                    .to_owned());
                }
                let private_key_pem = if key_raw.contains("BEGIN ")
                    && key_raw.contains("PRIVATE KEY")
                {
                    key_raw
                } else {
                    std::fs::read_to_string(&key_raw).map_err(|err| {
                        crate::i18n::messages::ssh_invalid_input(
                            &format!("cannot read private key file: {err}"),
                            self.locale,
                        )
                    })?
                };
                Ok(Some(
                    ruum_core::credentials::SshCredentialSecret::PrivateKey {
                        private_key_pem,
                        passphrase: (!passphrase_raw.is_empty())
                            .then_some(passphrase_raw),
                    },
                ))
            }
        }
    }

    /// Validate and save the SSH server profile to `SQLite` + encrypted store.
    pub fn save_ssh_profile_form(&mut self, cx: &mut Context<'_, Self>) {
        let alias = Self::read_ssh_input(self.ssh_input_alias.as_ref(), cx);
        let host = Self::read_ssh_input(self.ssh_input_host.as_ref(), cx);
        let port_raw = Self::read_ssh_input(self.ssh_input_port.as_ref(), cx);
        let username =
            Self::read_ssh_input(self.ssh_input_username.as_ref(), cx);
        let default_path_raw =
            Self::read_ssh_input(self.ssh_input_default_path.as_ref(), cx);

        if alias.is_empty() || host.is_empty() || username.is_empty() {
            self.ssh_form_error =
                Some(crate::i18n::messages::ssh_invalid_input(
                    "alias, host, and username are required",
                    self.locale,
                ));
            cx.notify();
            return;
        }
        if let Err(err) = ruum_core::remote::validate_host(&host) {
            self.ssh_form_error =
                Some(crate::i18n::messages::ssh_invalid_input(
                    &err.to_string(),
                    self.locale,
                ));
            cx.notify();
            return;
        }
        let port: u16 = match if port_raw.is_empty() {
            Ok(22)
        } else {
            port_raw.parse::<u16>().map_err(|e| e.to_string())
        } {
            Ok(p) if p > 0 => p,
            Ok(_) | Err(_) => {
                self.ssh_form_error = Some(
                    crate::i18n::messages::ssh_invalid_port(self.locale)
                        .to_owned(),
                );
                cx.notify();
                return;
            }
        };
        let default_path = if default_path_raw.is_empty() {
            Some("~".to_owned())
        } else {
            Some(default_path_raw)
        };
        let db = match self.open_db() {
            Ok(db) => db,
            Err(err) => {
                self.ssh_form_error = Some(err.to_string());
                cx.notify();
                return;
            }
        };
        let store = match self.ensure_cred_store() {
            Ok(s) => s,
            Err(err) => {
                self.ssh_form_error = Some(err);
                cx.notify();
                return;
            }
        };

        let editing_id = self.ssh_form_editing_id.clone();
        let secret_opt = match self.resolve_ssh_form_secret(
            &store,
            editing_id.as_deref(),
            cx,
        ) {
            Ok(s) => s,
            Err(msg) => {
                self.ssh_form_error = Some(msg);
                cx.notify();
                return;
            }
        };

        let now_secs = u64::try_from(now_seconds()).unwrap_or(0);
        let existing = editing_id
            .as_deref()
            .and_then(|id| self.ssh_profiles.iter().find(|p| p.id == id))
            .cloned();
        let profile = ruum_core::storage::SshProfile {
            id: existing.as_ref().map_or_else(String::new, |p| p.id.clone()),
            alias: alias.clone(),
            host,
            port,
            username,
            auth_mode: self.ssh_form_auth_mode,
            default_path,
            created_at_secs: existing
                .as_ref()
                .map_or(now_secs, |p| p.created_at_secs),
            updated_at_secs: now_secs,
            last_total_bytes: existing
                .as_ref()
                .and_then(|p| p.last_total_bytes),
            last_available_bytes: existing
                .as_ref()
                .and_then(|p| p.last_available_bytes),
            last_scanned_secs: existing
                .as_ref()
                .and_then(|p| p.last_scanned_secs),
            pending_script_path: existing
                .as_ref()
                .and_then(|p| p.pending_script_path.clone()),
            pending_script_items: existing
                .as_ref()
                .and_then(|p| p.pending_script_items),
            pending_reclaim_bytes: existing
                .as_ref()
                .and_then(|p| p.pending_reclaim_bytes),
            pending_created_secs: existing
                .as_ref()
                .and_then(|p| p.pending_created_secs),
        };

        let saved_profile = match if existing.is_some() {
            db.update_ssh_profile(&profile)
        } else {
            db.add_ssh_profile(&profile)
        } {
            Ok(p) => p,
            Err(err) => {
                self.ssh_form_error =
                    Some(crate::i18n::messages::ssh_profile_save_failed(
                        &err.to_string(),
                        self.locale,
                    ));
                cx.notify();
                return;
            }
        };
        let profile_id = saved_profile.id;
        if let Some(secret) = secret_opt
            && let Err(err) = store.set_ssh_secret(&profile_id, &secret)
        {
            if let Some(prev) = existing {
                let _ = db.update_ssh_profile(&prev);
            } else {
                let _ = db.delete_ssh_profile(&profile_id);
            }
            self.ssh_form_error =
                Some(crate::i18n::messages::ssh_profile_save_failed(
                    &err,
                    self.locale,
                ));
            cx.notify();
            return;
        }

        self.reload_ssh_profiles();
        for tab in &mut self.host_tabs {
            if tab.key == HostTabKey::Ssh(profile_id.clone()) {
                tab.label.clone_from(&alias);
            }
        }
        if self.active_ssh_profile_id.as_deref() == Some(profile_id.as_str()) {
            self.remote = Some(alias.clone());
        }
        self.ssh_form_open = false;
        self.ssh_form_editing_id = None;
        self.ssh_form_error = None;
        self.push_toast(
            crate::i18n::messages::ssh_profile_saved(&alias, self.locale),
            Status::Success,
            cx,
        );
        cx.notify();
    }

    /// Delete an SSH server profile (credential first, then `SQLite` row).
    pub fn delete_ssh_profile_card(
        &mut self,
        id: &str,
        cx: &mut Context<'_, Self>,
    ) {
        let alias = self
            .ssh_profiles
            .iter()
            .find(|p| p.id == id)
            .map_or_else(|| id.to_owned(), |p| p.alias.clone());
        let db = match self.open_db() {
            Ok(db) => db,
            Err(err) => {
                self.push_toast(
                    crate::i18n::messages::ssh_profile_delete_failed(
                        &err.to_string(),
                        self.locale,
                    ),
                    Status::Error,
                    cx,
                );
                return;
            }
        };
        let store = match self.ensure_cred_store() {
            Ok(s) => s,
            Err(err) => {
                self.push_toast(
                    crate::i18n::messages::ssh_profile_delete_failed(
                        &err,
                        self.locale,
                    ),
                    Status::Error,
                    cx,
                );
                return;
            }
        };
        if let Err(err) =
            ruum_core::credentials::delete_ssh_profile_with_credentials(
                &db, &store, id,
            )
        {
            self.push_toast(
                crate::i18n::messages::ssh_profile_delete_failed(
                    &err.to_string(),
                    self.locale,
                ),
                Status::Error,
                cx,
            );
            return;
        }
        self.close_host_tab(&HostTabKey::Ssh(id.to_owned()), cx);
        if self.ssh_form_editing_id.as_deref() == Some(id) {
            self.close_ssh_form(cx);
        }
        self.reload_ssh_profiles();
        self.push_toast(
            crate::i18n::messages::ssh_profile_deleted(&alias, self.locale),
            Status::Success,
            cx,
        );
        cx.notify();
    }

    fn snapshot_active_session(&mut self) -> ExploreSessionSnapshot {
        ExploreSessionSnapshot {
            root_path: std::mem::take(&mut self.root_path),
            scan_root: std::mem::take(&mut self.scan_root),
            tree: self.tree.take(),
            scan: self.scan.take(),
            remote_scan: self.remote_scan.take(),
            remote_disk: std::mem::take(&mut self.remote_disk),
            remote_progress: std::mem::take(&mut self.remote_progress),
            scan_preview: self.scan_preview.take(),
            preview_updated: self.preview_updated.take(),
            progress: std::mem::take(&mut self.progress),
            scan_error: self.scan_error.take(),
            scan_started: self.scan_started.take(),
            scan_elapsed: self.scan_elapsed.take(),
            scanned_at: std::mem::replace(&mut self.scanned_at, 0),
            scan_epoch: self.scan_epoch,
            crumbs: std::mem::take(&mut self.crumbs),
            selected: self.selected.take(),
            hovered: self.hovered.take(),
            history: std::mem::take(&mut self.history),
            view: std::mem::take(&mut self.view),
            transition: self.transition.take(),
            cache: self.cache.take(),
            marks: std::mem::take(&mut self.marks),
            space: self.space.take(),
            space_baseline: self.space_baseline.take(),
            disk_root: self.disk_root.take(),
            device: self.device.take(),
            home: self.home.take(),
            file_table: std::mem::take(&mut self.file_table),
            full_disk_access: self.full_disk_access.take(),
            remote: self.remote.take(),
            remote_platform: std::mem::take(&mut self.remote_platform),
            remote_identity: self.remote_identity.take(),
            remote_header: self.remote_header.take(),
            docker: std::mem::take(&mut self.docker),
            review_docker_checked: std::mem::take(
                &mut self.review_docker_checked,
            ),
            review_docker_initialized: std::mem::take(
                &mut self.review_docker_initialized,
            ),
            insights: std::mem::take(&mut self.insights),
            git: std::mem::take(&mut self.git),
            git_pending: std::mem::take(&mut self.git_pending),
            find: std::mem::take(&mut self.find),
            find_open: std::mem::take(&mut self.find_open),
            matches: self.matches.take(),
            filter_applied: std::mem::take(&mut self.filter_applied),
            filter_epoch: self.filter_epoch,
            find_epoch: self.find_epoch,
            finding: std::mem::take(&mut self.finding),
            apply_pending: std::mem::take(&mut self.apply_pending),
            crumb_menu: self.crumb_menu.take(),
            awaiting_root: std::mem::take(&mut self.awaiting_root),
            active_ssh_profile_id: self.active_ssh_profile_id.take(),
            active_ssh_session: self.active_ssh_session.take(),
        }
    }

    fn reset_active_explore_session(&mut self) {
        self.root_path = PathBuf::new();
        self.scan_root = PathBuf::new();
        self.tree = None;
        self.scan = None;
        self.remote_scan = None;
        self.remote_disk = false;
        self.remote_progress = ruum_core::remote_progress::Progress::default();
        self.scan_preview = None;
        self.preview_updated = None;
        self.progress = ScanSnapshot::default();
        self.scan_error = None;
        self.scan_started = None;
        self.scan_elapsed = None;
        self.scanned_at = 0;
        self.crumbs.clear();
        self.selected = None;
        self.hovered = None;
        self.history = History::default();
        self.view = View::default();
        self.transition = None;
        self.cache = None;
        self.marks.clear();
        self.space = None;
        self.space_baseline = None;
        self.disk_root = None;
        self.device = None;
        self.home = None;
        self.file_table = false;
        self.full_disk_access = None;
        self.remote = None;
        self.remote_platform.clear();
        self.remote_identity = None;
        self.remote_header = None;
        self.docker = DockerProbeStatus::default();
        self.docker_probing = false;
        self.review_docker_checked.clear();
        self.review_docker_initialized = false;
        self.insights.clear();
        self.git.clear();
        self.git_pending.clear();
        self.find.clear();
        self.find_open = false;
        self.matches = None;
        self.filter_applied = false;
        self.finding = false;
        self.apply_pending = false;
        self.crumb_menu = None;
        self.awaiting_root = false;
        self.active_ssh_profile_id = None;
        self.active_ssh_session = None;
        self.scan_epoch += 1;
    }

    fn restore_session(
        &mut self,
        snap: ExploreSessionSnapshot,
        cx: &Context<'_, Self>,
    ) {
        self.root_path = snap.root_path;
        self.scan_root = snap.scan_root;
        self.tree = snap.tree;
        self.scan = snap.scan;
        self.remote_scan = snap.remote_scan;
        self.remote_disk = snap.remote_disk;
        self.remote_progress = snap.remote_progress;
        self.scan_preview = snap.scan_preview;
        self.preview_updated = snap.preview_updated;
        self.progress = snap.progress;
        self.scan_error = snap.scan_error;
        self.scan_started = snap.scan_started;
        self.scan_elapsed = snap.scan_elapsed;
        self.scanned_at = snap.scanned_at;
        self.crumbs = snap.crumbs;
        self.selected = snap.selected;
        self.hovered = snap.hovered;
        self.history = snap.history;
        self.view = snap.view;
        self.transition = snap.transition;
        self.cache = snap.cache;
        self.marks = snap.marks;
        self.space = snap.space;
        self.space_baseline = snap.space_baseline;
        self.disk_root = snap.disk_root;
        self.device = snap.device;
        self.home = snap.home;
        self.file_table = snap.file_table;
        self.full_disk_access = snap.full_disk_access;
        self.remote = snap.remote;
        self.remote_platform = snap.remote_platform;
        self.remote_identity = snap.remote_identity;
        self.remote_header = snap.remote_header;
        self.docker = snap.docker;
        self.docker_probing = false;
        self.review_docker_checked = snap.review_docker_checked;
        self.review_docker_initialized = snap.review_docker_initialized;
        self.insights = snap.insights;
        self.git = snap.git;
        self.git_pending = snap.git_pending;
        self.find = snap.find;
        self.find_open = snap.find_open;
        self.matches = snap.matches;
        self.filter_applied = snap.filter_applied;
        self.filter_epoch = snap.filter_epoch;
        self.find_epoch = snap.find_epoch;
        self.finding = snap.finding;
        self.apply_pending = snap.apply_pending;
        self.crumb_menu = snap.crumb_menu;
        self.awaiting_root = snap.awaiting_root;
        self.active_ssh_profile_id = snap.active_ssh_profile_id;
        self.active_ssh_session = snap.active_ssh_session;

        self.scan_epoch = self.scan_epoch.max(snap.scan_epoch) + 1;
        if self.scan.is_some() || self.remote_scan.is_some() {
            Self::poll_scan(self.scan_epoch, cx);
        }
    }

    fn save_active_tab_snapshot(&mut self) {
        let snap = self.snapshot_active_session();
        let active_key = self.active_host_tab.clone();
        if let Some(tab) =
            self.host_tabs.iter_mut().find(|t| t.key == active_key)
        {
            tab.session = Some(snap);
        }
    }

    /// Activate an existing Explore host tab (`Local` or `Ssh(id)`), preserving
    /// every other tab's scanned tree and session state.
    pub fn switch_host_tab(
        &mut self,
        key: HostTabKey,
        cx: &mut Context<'_, Self>,
    ) {
        let Some(target_idx) = self.host_tabs.iter().position(|t| t.key == key)
        else {
            return;
        };
        self.screen = Screen::Explore;
        self.remote_docker_modal_open = false;
        self.volumes_open = false;
        self.volume_picker_remote_profile_id = None;
        self.ssh_form_open = false;
        self.ssh_form_error = None;
        if self.active_host_tab != key {
            let is_local = key == HostTabKey::Local;
            self.save_active_tab_snapshot();
            let snap = self.host_tabs[target_idx].session.take();
            self.active_host_tab = key;
            if let Some(snap) = snap {
                self.restore_session(snap, cx);
            } else {
                self.reset_active_explore_session();
                if is_local {
                    self.home = std::env::home_dir();
                    self.awaiting_root = true;
                }
            }
        }
        self.focus_request = Some(FocusTarget::Root);
        cx.notify();
    }

    /// Close a remote SSH host tab, cancelling any in-flight scan and
    /// dropping its `SshSession`. `HostTabKey::Local` can never be closed.
    pub fn close_host_tab(
        &mut self,
        key: &HostTabKey,
        cx: &mut Context<'_, Self>,
    ) {
        if *key == HostTabKey::Local {
            return;
        }
        let Some(pos) = self.host_tabs.iter().position(|t| &t.key == key)
        else {
            return;
        };
        self.remote_docker_modal_open = false;
        if self.active_host_tab == *key {
            if let Some(scan) = self.scan.take() {
                scan.cancel();
            }
            self.remote_scan = None;
            self.active_ssh_session = None;
            self.active_ssh_profile_id = None;
            self.host_tabs.remove(pos);
            let next_idx = pos.min(self.host_tabs.len().saturating_sub(1));
            let next_key = self.host_tabs[next_idx].key.clone();
            let snap = self.host_tabs[next_idx].session.take();
            self.active_host_tab = next_key.clone();
            if let Some(snap) = snap {
                self.restore_session(snap, cx);
            } else {
                self.reset_active_explore_session();
                if next_key == HostTabKey::Local {
                    self.home = std::env::home_dir();
                    self.awaiting_root = true;
                }
            }
        } else {
            let mut tab = self.host_tabs.remove(pos);
            if let Some(mut snap) = tab.session.take() {
                if let Some(scan) = snap.scan.take() {
                    scan.cancel();
                }
                snap.remote_scan = None;
                snap.active_ssh_session = None;
            }
        }
        cx.notify();
    }

    /// Report whether an SSH profile currently has an active scan in its tab.
    #[must_use]
    pub fn is_ssh_profile_scanning(&self, id: &str) -> bool {
        let key = HostTabKey::Ssh(id.to_owned());
        if self.active_host_tab == key {
            return self.remote_scan.is_some();
        }
        self.host_tabs
            .iter()
            .find(|t| t.key == key)
            .and_then(|t| t.session.as_ref())
            .is_some_and(|s| s.remote_scan.is_some())
    }

    /// Report whether an SSH tab has both a known profile and a live session.
    #[must_use]
    pub(crate) fn has_connected_ssh_tab(&self, id: &str) -> bool {
        if !self.ssh_profiles.iter().any(|p| p.id == id) {
            return false;
        }
        let active_live = self.active_host_tab
            == HostTabKey::Ssh(id.to_owned())
            && self
                .active_ssh_session
                .as_ref()
                .is_some_and(|s| !s.is_closed() && !s.is_expired());
        active_live || self.find_live_ssh_session(id).is_some()
    }

    fn find_live_ssh_session(&self, id: &str) -> Option<Arc<SshSession>> {
        if self.active_ssh_profile_id.as_deref() == Some(id)
            && let Some(session) = &self.active_ssh_session
            && !session.is_closed()
            && !session.is_expired()
        {
            return Some(Arc::clone(session));
        }
        let key = HostTabKey::Ssh(id.to_owned());
        self.host_tabs
            .iter()
            .find(|t| t.key == key)
            .and_then(|t| t.session.as_ref())
            .and_then(|snap| snap.active_ssh_session.as_ref())
            .filter(|s| !s.is_closed() && !s.is_expired())
            .map(Arc::clone)
    }

    fn store_live_ssh_session(&mut self, id: &str, session: &Arc<SshSession>) {
        if self.active_ssh_profile_id.as_deref() == Some(id) {
            self.active_ssh_session = Some(Arc::clone(session));
            return;
        }
        let key = HostTabKey::Ssh(id.to_owned());
        if let Some(tab) = self.host_tabs.iter_mut().find(|t| t.key == key)
            && let Some(snap) = &mut tab.session
        {
            snap.active_ssh_session = Some(Arc::clone(session));
        }
    }

    fn ensure_active_ssh_session(
        &mut self,
        profile_id: &str,
    ) -> Result<Arc<SshSession>, String> {
        if let Some(session) = &self.active_ssh_session
            && !session.is_closed()
            && !session.is_expired()
        {
            return Ok(Arc::clone(session));
        }
        let profile = self
            .ssh_profiles
            .iter()
            .find(|p| p.id == profile_id)
            .cloned()
            .ok_or_else(|| format!("SSH profile `{profile_id}` not found"))?;
        let session = Arc::new(self.start_profile_ssh_session(&profile)?);
        self.active_ssh_session = Some(Arc::clone(&session));
        Ok(session)
    }

    fn start_profile_ssh_session(
        &mut self,
        profile: &ruum_core::storage::SshProfile,
    ) -> Result<ruum_core::ssh_session::SshSession, String> {
        let store = self.ensure_cred_store()?;
        start_ssh_session_with_store(
            profile,
            &store,
            self.app_settings.ssh_session_duration_minutes,
        )
    }

    /// Connect to a saved SSH profile on-demand (or activate its existing tab
    /// if already connected), preserving other tabs' session state.
    pub fn connect_ssh_profile(
        &mut self,
        id: &str,
        cx: &mut Context<'_, Self>,
    ) {
        self.connect_ssh_profile_at(id, None, cx);
    }

    /// Connect to a saved SSH profile at `override_root` (or the profile's
    /// configured `default_path`, defaulting to `~`), switching to
    /// `Screen::Explore` and starting the scan.
    pub fn connect_ssh_profile_at(
        &mut self,
        id: &str,
        override_root: Option<PathBuf>,
        cx: &mut Context<'_, Self>,
    ) {
        let Some(profile) =
            self.ssh_profiles.iter().find(|p| p.id == id).cloned()
        else {
            return;
        };
        let target_key = HostTabKey::Ssh(profile.id.clone());
        if override_root.is_none()
            && let Some(existing_idx) =
                self.host_tabs.iter().position(|t| t.key == target_key)
        {
            let has_live_state =
                if self.active_host_tab == target_key {
                    self.tree.is_some() || self.remote_scan.is_some()
                } else {
                    self.host_tabs[existing_idx].session.as_ref().is_some_and(
                        |s| s.tree.is_some() || s.remote_scan.is_some(),
                    )
                };
            if has_live_state {
                self.switch_host_tab(target_key, cx);
                return;
            }
        }

        let session = if let Some(live) = self.find_live_ssh_session(id) {
            live
        } else {
            match self.start_profile_ssh_session(&profile) {
                Ok(session) => Arc::new(session),
                Err(err) => {
                    self.push_toast(
                        crate::i18n::messages::ssh_connect_failed(
                            &profile.alias,
                            &err,
                            self.locale,
                        ),
                        Status::Error,
                        cx,
                    );
                    return;
                }
            }
        };

        let root_path = override_root
            .filter(|p| !p.as_os_str().is_empty())
            .unwrap_or_else(|| {
                profile
                    .default_path
                    .as_deref()
                    .map(str::trim)
                    .filter(|s| !s.is_empty())
                    .map_or_else(|| PathBuf::from("~"), PathBuf::from)
            });

        if self.active_host_tab == target_key {
            self.marks.clear();
            self.history = History::default();
            self.remote_identity = None;
            self.remote_platform.clear();
            self.docker = DockerProbeStatus::default();
            self.git.clear();
            self.git_pending.clear();
            self.space = None;
            self.space_baseline = None;
            self.disk_root = None;
            self.device = None;
            self.full_disk_access = None;
            self.file_table = false;
            self.home = None;
        } else {
            self.save_active_tab_snapshot();
            if let Some(idx) =
                self.host_tabs.iter().position(|t| t.key == target_key)
            {
                let _ = self.host_tabs[idx].session.take();
                self.host_tabs[idx].label.clone_from(&profile.alias);
            } else {
                self.host_tabs.push(ExploreHostTab {
                    key: target_key.clone(),
                    label: profile.alias.clone(),
                    session: None,
                });
            }
            self.active_host_tab = target_key;
            self.reset_active_explore_session();
        }

        self.screen = Screen::Explore;
        self.remote_docker_modal_open = false;
        self.remote = Some(profile.alias);
        self.active_ssh_profile_id = Some(profile.id);
        self.active_ssh_session = Some(session);
        self.options.follow_links = false;
        self.options.one_filesystem = true;
        self.awaiting_root = false;
        self.close_volumes(cx);
        self.set_root(root_path, cx);
    }

    /// Pre-flight check remote disk space for a saved SSH profile off the UI
    /// thread and update its cached summary in `ruum.db`.
    pub fn check_ssh_profile_space(
        &mut self,
        id: &str,
        cx: &mut Context<'_, Self>,
    ) {
        if self.ssh_checking_profiles.contains(id) {
            return;
        }
        let Some(profile) =
            self.ssh_profiles.iter().find(|p| p.id == id).cloned()
        else {
            return;
        };
        let store = match self.ensure_cred_store() {
            Ok(store) => store,
            Err(err) => {
                self.push_toast(
                    crate::i18n::messages::ssh_check_space_failed(
                        &profile.alias,
                        &err,
                        self.locale,
                    ),
                    Status::Error,
                    cx,
                );
                return;
            }
        };
        let existing_session = self.find_live_ssh_session(id);
        let duration_minutes = self.app_settings.ssh_session_duration_minutes;
        let profile_id = profile.id.clone();
        let alias = profile.alias.clone();
        self.ssh_checking_profiles.insert(profile_id.clone());
        cx.notify();

        let task = cx.background_executor().spawn(async move {
            let session = match existing_session {
                Some(s) if !s.is_closed() && !s.is_expired() => s,
                _ => Arc::new(start_ssh_session_with_store(
                    &profile,
                    &store,
                    duration_minutes,
                )?),
            };
            let target_path = profile
                .default_path
                .as_deref()
                .map(str::trim)
                .filter(|s| !s.is_empty());
            let report = ruum_core::remote::RemoteSpace::check_with_session(
                &session,
                target_path.map(Path::new),
            )
            .map_err(|err| err.to_string())?;
            Ok::<_, String>((session, report.space))
        });

        cx.spawn(async move |this, cx| {
            let res = task.await;
            let _ = this.update(cx, |this, cx| {
                this.ssh_checking_profiles.remove(&profile_id);
                let locale = this.locale;
                match res {
                    Ok((session, info)) => {
                        this.store_live_ssh_session(&profile_id, &session);
                        let now_secs =
                            u64::try_from(now_seconds()).unwrap_or(0);
                        if this.db_path.is_some()
                            && let Ok(db) = this.open_db()
                        {
                            let _ = db.update_ssh_profile_scan_summary(
                                &profile_id,
                                info.total,
                                info.available,
                                now_secs,
                            );
                            this.reload_ssh_profiles();
                        }
                        let thresh_bytes = this
                            .watchdog_config
                            .min_free_gb
                            .saturating_mul(1024 * 1024 * 1024);
                        let breached = info.available < thresh_bytes;
                        let free = ruum_core::size::human_bytes(info.available);
                        let total = ruum_core::size::human_bytes(info.total);
                        let msg = if breached {
                            crate::i18n::messages::ssh_space_checked_below(
                                &alias,
                                &free,
                                &total,
                                this.watchdog_config.min_free_gb,
                                locale,
                            )
                        } else {
                            crate::i18n::messages::ssh_space_checked_healthy(
                                &alias,
                                &free,
                                &total,
                                this.watchdog_config.min_free_gb,
                                locale,
                            )
                        };
                        let status = if breached {
                            Status::Warning
                        } else {
                            Status::Success
                        };
                        this.push_toast(msg, status, cx);
                    }
                    Err(err) => {
                        this.push_toast(
                            crate::i18n::messages::ssh_check_space_failed(
                                &alias, &err, locale,
                            ),
                            Status::Error,
                            cx,
                        );
                    }
                }
                cx.notify();
            });
        })
        .detach();
    }

    /// Run remote AI analysis by pulling an in-memory [`Snapshot`] via
    /// `RemoteScan::spawn_with_session` and running local AI CLI + pure-memory
    /// tree validation (zero remote AI CLI requirement).
    pub fn analyze_ssh_profile(
        &mut self,
        id: &str,
        cx: &mut Context<'_, Self>,
    ) {
        if self.ssh_analyzing_profiles.contains(id) {
            return;
        }
        let Some(profile) =
            self.ssh_profiles.iter().find(|p| p.id == id).cloned()
        else {
            return;
        };
        if !self.ai_config.setup_completed {
            self.ai_setup_choice_modal = Some(AiSetupChoiceModalState {
                pending_action: PendingAiSetupAction::SshAnalysis {
                    profile_id: profile.id,
                },
            });
            self.focus_request = Some(FocusTarget::AiSetupChoiceDialog);
            cx.notify();
            return;
        }
        let ai_cfg = self.snapshot_ai_config(cx);
        if self.abort_if_cloud_models_locked(&ai_cfg, Some(&profile.alias), cx)
        {
            return;
        }
        let is_cloud = ai_cfg.provider == AiCliProvider::RuumCloud;
        let store = match self.ensure_cred_store() {
            Ok(store) => store,
            Err(err) => {
                let now_secs = u64::try_from(now_seconds()).unwrap_or(0);
                let kind = ruum_core::watchdog::classify_ai_failure(&err);
                let summary = crate::i18n::messages::ssh_ai_analyze_failed(
                    &profile.alias,
                    &err,
                    self.locale,
                );
                self.ai_last_error = Some(summary.clone());
                self.ai_last_error_at_secs = Some(now_secs);
                if let Some(db) = open_db_for_window(self) {
                    let _ = db.record_ai_error(&summary, now_secs);
                }
                self.push_toast(summary.clone(), Status::Error, cx);
                crate::post_app_notification(
                    ruum_core::watchdog::NOTIF_TAG_AI_ANALYSIS,
                    crate::i18n::messages::ai_analysis_failure_notif_title(
                        kind,
                        self.locale,
                    ),
                    &summary,
                    cx,
                );
                return;
            }
        };

        let existing_session = self.find_live_ssh_session(id);
        let duration_minutes = self.app_settings.ssh_session_duration_minutes;
        let ai_ctx = self.snapshot_ai_execution_context(cx);
        let provider_label = ai_cfg.provider.label_in(self.locale);
        let options = self.options.clone();
        let now_secs = u64::try_from(now_seconds()).unwrap_or(0);
        let profile_id = profile.id.clone();
        let alias = profile.alias.clone();
        let db_path = self.db_path.clone();
        let locale = self.locale;

        self.ssh_analyzing_profiles.insert(profile_id.clone());
        self.push_toast(
            crate::i18n::messages::running_ai_analysis(
                provider_label,
                self.locale,
            ),
            Status::Neutral,
            cx,
        );
        cx.notify();

        let task = cx.background_executor().spawn(async move {
            let session = match existing_session {
                Some(s) if !s.is_closed() && !s.is_expired() => s,
                _ => Arc::new(start_ssh_session_with_store(
                    &profile,
                    &store,
                    duration_minutes,
                )?),
            };
            let request = ruum_core::remote::Request {
                root: profile
                    .default_path
                    .as_deref()
                    .map(str::trim)
                    .filter(|s| !s.is_empty())
                    .map(PathBuf::from),
                disk: false,
                options,
            };
            let scan = ruum_core::remote::RemoteScan::spawn_with_session(
                &session, &request,
            )
            .map_err(|err| err.to_string())?;
            let snapshot = scan.wait().map_err(|err| err.to_string())?;
            let runner = ruum_core::ai_cli::DefaultCommandRunner;
            let proposal = analyze_remote_snapshot_with_context(
                &ai_cfg, &snapshot, now_secs, &runner, &ai_ctx,
            )?;
            Ok::<_, String>((session, snapshot.header, proposal))
        });

        cx.spawn(async move |this, cx| {
            let res = task.await;
            let err_raw = res.as_ref().err().cloned();
            cx.update(|cx| {
                let done_secs =
                    u64::try_from(now_seconds()).unwrap_or(now_secs);
                let mut notif: Option<(
                    ruum_core::watchdog::AiFailureKind,
                    crate::i18n::Locale,
                    String,
                )> = None;
                let updated = this.update(cx, |this, cx| {
                    this.ssh_analyzing_profiles.remove(&profile_id);
                    if is_cloud {
                        this.reload_cloud_state();
                        this.sync_selected_cloud_model();
                    }
                    let active_locale = this.locale;
                    match res {
                        Ok((session, header, proposal)) => {
                            this.store_live_ssh_session(&profile_id, &session);
                            this.ai_last_error = None;
                            this.ai_last_error_at_secs = None;
                            if is_cloud {
                                this.cloud_payment_required = false;
                            }
                            if let Some(db) = open_db_for_window(this) {
                                let _ = db.clear_ai_error(done_secs);
                                if let Some(info) = header.space {
                                    let _ = db.update_ssh_profile_scan_summary(
                                        &profile_id,
                                        info.total,
                                        info.available,
                                        done_secs,
                                    );
                                    this.reload_ssh_profiles();
                                }
                            }
                            if proposal.items.is_empty()
                                && proposal.docker_targets.is_empty()
                                && build_docker_confirm_items(
                                    &header.docker,
                                    &[],
                                    active_locale,
                                )
                                .0
                                .is_empty()
                            {
                                this.push_toast(
                                    crate::i18n::messages::ai_found_no_files(
                                        active_locale,
                                    ),
                                    Status::Success,
                                    cx,
                                );
                            } else {
                                let request =
                                    remote_proposal_confirm_request_with_docker(
                                        header.root.clone(),
                                        &alias,
                                        &proposal,
                                        &header.docker,
                                        active_locale,
                                    );
                                let count = request.items.len()
                                    + request.docker_items.len();
                                crate::open_remote_confirm_window(
                                    request,
                                    profile_id,
                                    session,
                                    header,
                                    this.db_path.clone(),
                                    cx,
                                );
                                this.push_toast(
                                    crate::i18n::messages::ai_proposed_items(
                                        count,
                                        active_locale,
                                    ),
                                    Status::Success,
                                    cx,
                                );
                            }
                        }
                        Err(err) => {
                            let kind =
                                ruum_core::watchdog::classify_ai_failure(&err);
                            let summary =
                                crate::i18n::messages::ssh_ai_analyze_failed(
                                    &alias,
                                    &err,
                                    active_locale,
                                );
                            this.ai_last_error = Some(summary.clone());
                            this.ai_last_error_at_secs = Some(done_secs);
                            if is_cloud
                                && (is_cloud_payment_required_message(&summary)
                                    || is_cloud_payment_required_message(&err))
                            {
                                this.cloud_payment_required = true;
                            }
                            if let Some(db) = open_db_for_window(this) {
                                let _ = db.record_ai_error(&summary, done_secs);
                            }
                            this.push_toast(summary.clone(), Status::Error, cx);
                            notif = Some((kind, active_locale, summary));
                        }
                    }
                    cx.notify();
                });

                if let Some((kind, notif_locale, summary)) = notif {
                    crate::post_app_notification(
                        ruum_core::watchdog::NOTIF_TAG_AI_ANALYSIS,
                        crate::i18n::messages::ai_analysis_failure_notif_title(
                            kind,
                            notif_locale,
                        ),
                        &summary,
                        cx,
                    );
                } else if updated.is_err()
                    && let Some(err) = err_raw
                {
                    let kind = ruum_core::watchdog::classify_ai_failure(&err);
                    let summary = crate::i18n::messages::ssh_ai_analyze_failed(
                        &alias, &err, locale,
                    );
                    if let Some(db) = open_db_for_background(db_path.as_deref())
                    {
                        let _ = db.record_ai_error(&summary, done_secs);
                    }
                    crate::post_app_notification(
                        ruum_core::watchdog::NOTIF_TAG_AI_ANALYSIS,
                        crate::i18n::messages::ai_analysis_failure_notif_title(
                            kind, locale,
                        ),
                        &summary,
                        cx,
                    );
                }
            });
        })
        .detach();
    }

    pub fn choose_picker_path(&mut self, cx: &mut Context<'_, Self>) {
        let raw = self
            .picker_path
            .as_ref()
            .map(|input| input.read(cx).value().as_str().trim().to_owned());
        let Some(raw) = raw else {
            return;
        };
        if let Some(profile_id) = self.volume_picker_remote_profile_id.clone() {
            self.choose_remote_root(&profile_id, Path::new(&raw), cx);
            return;
        }
        let expanded = expand_local_tilde_path(&raw);
        self.choose_source_root(expanded, cx);
    }

    /// Pick a remote directory or mount for an SSH profile and start scanning.
    pub fn choose_remote_root(
        &mut self,
        profile_id: &str,
        path: &Path,
        cx: &mut Context<'_, Self>,
    ) {
        if !self.volumes_open || self.volumes_loading {
            return;
        }
        let raw = path.to_string_lossy().trim().to_owned();
        let valid_tilde =
            raw == "~" || raw.starts_with("~/") || raw.starts_with("~\\");
        if raw.is_empty()
            || (!valid_tilde
                && !ruum_core::remote::is_remote_absolute(Path::new(&raw)))
        {
            self.picker_error = Some(
                crate::i18n::messages::picker_remote_abs_path(self.locale)
                    .into(),
            );
            cx.notify();
            return;
        }
        self.picker_error = None;
        self.connect_ssh_profile_at(profile_id, Some(PathBuf::from(raw)), cx);
    }

    /// Pick a local directory or mount and activate the Local Explore tab.
    pub fn choose_source_root(
        &mut self,
        mut path: PathBuf,
        cx: &mut Context<'_, Self>,
    ) {
        if !self.volumes_open
            || self.volumes_loading
            || self.volumes_error.is_some()
        {
            return;
        }
        if !path.is_absolute() {
            self.picker_error = Some(
                crate::i18n::messages::picker_abs_path(self.locale).into(),
            );
            cx.notify();
            return;
        }
        match dunce::canonicalize(&path) {
            Ok(root) if root.is_dir() => path = root,
            _ => {
                self.picker_error = Some(
                    crate::i18n::messages::picker_cannot_open_dir(self.locale)
                        .into(),
                );
                cx.notify();
                return;
            }
        }
        self.picker_error = None;
        if self.active_host_tab != HostTabKey::Local {
            self.switch_host_tab(HostTabKey::Local, cx);
        }
        if self.remote.is_some() || self.remote_identity.is_some() {
            // Absolute paths only identify marks within one machine.
            self.marks.clear();
            self.history = History::default();
            self.remote_identity = None;
            self.remote_platform.clear();
            self.docker = DockerProbeStatus::default();
            self.git.clear();
            self.git_pending.clear();
            self.space = None;
            self.space_baseline = None;
            self.disk_root = None;
            self.device = None;
            self.full_disk_access = None;
            self.file_table = false;
        }
        self.screen = Screen::Explore;
        self.remote_docker_modal_open = false;
        self.remote = None;
        self.active_ssh_profile_id = None;
        self.active_ssh_session = None;
        self.home = std::env::home_dir();
        self.awaiting_root = false;
        self.close_volumes(cx);
        self.set_root(path, cx);
    }

    /// Closing cancels discovery, not the scan already on screen.
    pub fn close_volumes(&mut self, cx: &mut Context<'_, Self>) {
        self.volume_epoch += 1;
        self.volumes_loading = false;
        self.volumes_open = false;
        self.volume_picker_remote_profile_id = None;
        self.picker_error = None;
        self.ssh_form_open = false;
        self.ssh_form_error = None;
        self.focus_request = Some(FocusTarget::Root);
        cx.notify();
    }

    pub fn choose_volume(&mut self, cx: &mut Context<'_, Self>) {
        if let Some(point) = self
            .volumes
            .get(self.volume_highlight)
            .map(|volume| volume.point.clone())
        {
            if let Some(profile_id) =
                self.volume_picker_remote_profile_id.clone()
            {
                self.choose_remote_root(&profile_id, &point, cx);
            } else {
                self.choose_source_root(point, cx);
            }
        }
    }

    /// Move the highlight in the open picker, wrapping at the ends.
    pub fn move_volume_highlight(
        &mut self,
        step: i32,
        cx: &mut Context<'_, Self>,
    ) {
        if self.volumes.is_empty() {
            return;
        }
        let len = self.volumes.len();
        let at = self.volume_highlight;
        // Picker steps are ±1, but wrap either way without casting the length
        // down to a narrower type.
        let next = match step.signum() {
            1 => (at + 1) % len,
            -1 => at.checked_sub(1).unwrap_or(len - 1),
            _ => at,
        };
        self.volume_highlight = next;
        cx.notify();
    }

    /// `g`: the whole disk. Widens when the disk is above the scanned root,
    /// and goes to its top when it already is the root.
    pub fn go_to_disk(&mut self, cx: &mut Context<'_, Self>) {
        let Some(disk) = self.disk_root.clone() else {
            return;
        };
        if disk == self.root_path {
            self.go_to(Vec::new(), cx);
        } else {
            self.widen_to(disk, cx);
        }
    }

    /// Recompute "worth a look" from the tree on screen.
    fn refresh_insights(&mut self) {
        self.scanned_at = now_seconds();
        self.insights = self.tree.as_deref().map_or_else(Vec::new, |tree| {
            worth_a_look(tree, self.scanned_at, INSIGHT_LIMIT)
        });
    }

    /// Ask git about `path` once, off the UI thread, if it is a checkout.
    pub fn ensure_git(&mut self, path: &Path, cx: &Context<'_, Self>) {
        if self.remote.is_some()
            || self.git.contains_key(path)
            || self.git_pending.contains(path)
        {
            return;
        }
        let path = path.to_path_buf();
        self.git_pending.insert(path.clone());
        let epoch = self.scan_epoch;
        let task = cx.background_executor().spawn({
            let path = path.clone();
            async move { crate::git::state(&path) }
        });
        cx.spawn(async move |this, cx| {
            let state = task.await;
            let _ = this.update(cx, |this, cx| {
                if this.scan_epoch != epoch || this.remote.is_some() {
                    return;
                }
                this.git_pending.remove(&path);
                this.git.insert(path, state);
                cx.notify();
            });
        })
        .detach();
    }

    /// Select the largest entry of the current root, so the selection line, the
    /// tooltip and the mark key all have something to act on from the first
    /// frame. The largest entry is also the answer to "what is eating my disk"
    /// most of the time.
    fn select_largest(&mut self, cx: &mut Context<'_, Self>) {
        if self.selected.is_some() {
            return;
        }
        if let Some(index) = self.current().and_then(Node::largest_child) {
            self.selected = Some(vec![index]);
            cx.notify();
        }
    }

    // ── scanning ────────────────────────────────────────────────────────

    /// Stop the walk in progress. A partial tree is never shown as if it
    /// were the whole one: a first scan leaves the panel saying it stopped,
    /// and a widening scan leaves the tree it started from on screen.
    pub fn cancel_scan(&mut self, cx: &mut Context<'_, Self>) {
        self.scan_preview = None;
        if self.remote_scan.take().is_some() {
            self.scan_epoch += 1;
            self.progress.cancelled = true;
            self.progress.finished = true;
            self.scan_elapsed = self.scan_started.map(|t| t.elapsed());
            cx.notify();
            return;
        }
        let Some(scan) = self.scan.take() else {
            return;
        };
        scan.cancel();
        // The poller stops at its next tick instead of taking the result.
        self.scan_epoch += 1;
        self.progress = scan.progress.snapshot();
        self.scan_elapsed = self.scan_started.map(|started| started.elapsed());
        self.scan_root.clone_from(&self.root_path);
        cx.notify();
    }

    /// Changing resource policy leaves the current scan and result intact;
    /// rescan, navigation to a new root and restart use the new fixed budget.
    pub fn set_power_efficiency(
        &mut self,
        preset: crate::power::PowerEfficiency,
        cx: &mut Context<'_, Self>,
    ) {
        self.power_menu = None;
        // Not offered here: it would run the same workers as the preset
        // below it, under a name that promises more.
        if !preset.available(self.cpu_threads) {
            cx.notify();
            return;
        }
        self.options.threads = preset.policy(self.cpu_threads);
        self.power_choice = Some(preset);
        preset.key().clone_into(&mut self.app_settings.power_preset);
        self.app_settings.updated_at_secs =
            u64::try_from(now_seconds()).unwrap_or(0);
        self.watchdog_draft.power_preset = preset;
        let label = preset.label_in(self.cpu_threads, self.locale);
        self.notice = Some(if self.db_path.is_some() {
            match self
                .open_db()
                .and_then(|db| db.save_app_settings(&self.app_settings))
            {
                Ok(()) => (
                    crate::i18n::messages::power_saved(&label, self.locale),
                    Status::Success,
                ),
                Err(error) => (
                    crate::i18n::messages::power_save_failed(
                        &label,
                        &error.to_string(),
                        self.locale,
                    ),
                    Status::Warning,
                ),
            }
        } else {
            (
                crate::i18n::messages::power_no_dir(&label, self.locale),
                Status::Warning,
            )
        });
        cx.notify();
    }

    /// Open the Power Efficiency menu on the current preset, or close it.
    pub fn toggle_power_menu(&mut self, cx: &mut Context<'_, Self>) {
        use crate::power::PowerEfficiency;
        self.power_menu = if self.power_menu.is_some() {
            None
        } else {
            // On the current preset, or the nearest one below it that this
            // machine offers: a custom count starts from Balanced.
            let current = self.power_choice.unwrap_or_default() as usize;
            (0..=current)
                .rev()
                .find(|&index| {
                    PowerEfficiency::ALL[index].available(self.cpu_threads)
                })
                .or(Some(0))
        };
        cx.notify();
    }

    /// Keys while the Power Efficiency menu is open, as the sibling menu
    /// takes them. The arrows step over presets this machine cannot offer.
    fn on_power_key(&mut self, key: &str, cx: &mut Context<'_, Self>) -> bool {
        use crate::power::PowerEfficiency;
        let Some(highlighted) = self.power_menu else {
            return false;
        };
        let cpus = self.cpu_threads;
        let offered: Vec<usize> = (0..PowerEfficiency::ALL.len())
            .filter(|&index| PowerEfficiency::ALL[index].available(cpus))
            .collect();
        let step = |forward: bool| {
            let next = if forward {
                offered.iter().find(|&&index| index > highlighted)
            } else {
                offered.iter().rev().find(|&&index| index < highlighted)
            };
            next.copied().unwrap_or(highlighted)
        };
        match key {
            "down" | "j" => self.power_menu = Some(step(true)),
            "up" | "k" => self.power_menu = Some(step(false)),
            "home" => self.power_menu = offered.first().copied(),
            "end" => self.power_menu = offered.last().copied(),
            "enter" | "space" => {
                self.set_power_efficiency(
                    PowerEfficiency::ALL[highlighted],
                    cx,
                );
            }
            "escape" => self.power_menu = None,
            _ => {
                self.power_menu = None;
                cx.notify();
                return false;
            }
        }
        cx.notify();
        true
    }

    /// Start a fresh scan, abandoning any walk still in progress.
    pub fn start_scan(&mut self, cx: &mut Context<'_, Self>) {
        if self.awaiting_root {
            self.open_volumes(cx);
            return;
        }
        if let Some(scan) = self.scan.take() {
            scan.cancel();
        }
        self.scan_epoch += 1;
        let epoch = self.scan_epoch;
        self.progress = ScanSnapshot::default();
        self.scan_error = None;
        self.remote_progress = ruum_core::remote_progress::Progress::default();
        self.scan_preview = None;
        self.preview_updated = None;
        // A new scan starts at its root; where it was is one `<` away.
        if !self.crumbs.is_empty() {
            self.remember();
        }
        discard(self.tree.take());
        self.crumbs.clear();
        self.selected = None;
        self.hovered = None;
        self.view = View::default();
        self.cache = None;
        self.insights.clear();
        self.git.clear();
        self.git_pending.clear();
        self.clear_filter();
        self.scan_started = Some(Instant::now());
        self.scan_elapsed = None;
        self.scan_root.clone_from(&self.root_path);
        self.remote_scan = None;
        if let Some(host) = &self.remote {
            let request = ruum_core::remote::Request {
                root: (!self.root_path.as_os_str().is_empty())
                    .then(|| self.root_path.clone()),
                disk: self.remote_disk,
                options: self.options.clone(),
            };
            let spawn_res =
                if let Some(profile_id) = self.active_ssh_profile_id.clone() {
                    match self.ensure_active_ssh_session(&profile_id) {
                        Ok(session) => {
                            ruum_core::remote::RemoteScan::spawn_with_session(
                                &session, &request,
                            )
                        }
                        Err(err) => Err(std::io::Error::other(err)),
                    }
                } else {
                    ruum_core::remote::RemoteScan::spawn(host, &request)
                };
            match spawn_res {
                Ok(scan) => {
                    self.remote_scan = Some(scan);
                    Self::poll_scan(epoch, cx);
                }
                Err(error) => self.scan_error = Some(error.to_string()),
            }
            cx.notify();
            return;
        }
        self.scan = Some(ScanHandle::spawn(
            self.root_path.clone(),
            self.options.clone(),
        ));
        if !cfg!(test) && !self.local_docker_probing {
            self.local_docker_probing = true;
            Self::refresh_local_docker(cx);
        }
        Self::poll_scan(epoch, cx);
        cx.notify();
    }

    fn poll_scan(epoch: u64, cx: &Context<'_, Self>) {
        cx.spawn(async move |this, cx| {
            loop {
                cx.background_executor()
                    .timer(Duration::from_millis(110))
                    .await;
                let keep_going = this
                    .update(cx, |this, cx| this.poll_scan_once(epoch, cx))
                    .unwrap_or(false);
                if !keep_going {
                    break;
                }
            }
        })
        .detach();
    }

    pub(crate) fn update_remote_identity(
        &mut self,
        identity: Option<ruum_core::identity::HostIdentity>,
    ) {
        // A stable SSH alias can be repointed to a different machine.
        // Old marks must not acquire that machine's freshly captured guard.
        if self.remote_identity != identity && !self.marks.is_empty() {
            self.marks.clear();
            self.notice = Some((
                crate::i18n::messages::remote_identity_changed(self.locale)
                    .into(),
                Status::Warning,
            ));
        }
        self.remote_identity = identity;
    }

    /// Drain the scan channel once. Returns whether the poller should keep
    /// ticking. Driven by the poll task above, and by the tests, which cannot
    /// wait on the test clock while a real worker thread walks a directory.
    pub(crate) fn poll_scan_once(
        &mut self,
        epoch: u64,
        cx: &mut Context<'_, Self>,
    ) -> bool {
        if epoch != self.scan_epoch {
            return false;
        }
        if let Some(scan) = &self.remote_scan {
            let outcome = scan.poll();
            self.remote_progress = scan.progress();
            self.scan_preview.clone_from(&self.remote_progress.preview);
            self.progress.files = self.remote_progress.files;
            self.progress.dirs = self.remote_progress.dirs;
            self.progress.bytes = self.remote_progress.bytes;
            self.progress.errors = self.remote_progress.errors;
            let Some(outcome) = outcome else {
                cx.notify();
                return true;
            };
            match outcome {
                Ok(snapshot) => {
                    self.root_path.clone_from(&snapshot.header.root);
                    self.home.clone_from(&snapshot.header.home);
                    self.disk_root.clone_from(&snapshot.header.disk_root);
                    self.space = snapshot.header.space;
                    self.remote_platform.clone_from(&snapshot.header.platform);
                    self.update_remote_identity(
                        snapshot.header.identity.clone(),
                    );
                    self.docker = snapshot.docker.clone();
                    self.remote_header = Some(snapshot.header);
                    self.sync_review_docker_defaults();
                    self.docker_probing = false;
                    self.device = self.remote.as_ref().map(|host| {
                        crate::i18n::messages::device_remote_read_only(
                            host,
                            self.locale,
                        )
                    });
                    if let (Some(profile_id), Some(space)) =
                        (self.active_ssh_profile_id.clone(), self.space)
                        && self.db_path.is_some()
                        && let Ok(db) = self.open_db()
                    {
                        let now_secs =
                            u64::try_from(now_seconds()).unwrap_or(0);
                        let _ = db.update_ssh_profile_scan_summary(
                            &profile_id,
                            space.total,
                            space.available,
                            now_secs,
                        );
                        self.reload_ssh_profiles();
                    }
                    let node = snapshot.tree;
                    self.marks.refresh(
                        &self.root_path,
                        &node,
                        self.options.metric,
                    );
                    self.tree = Some(Arc::new(node));
                    self.refresh_insights();
                    self.select_largest(cx);
                    self.scan_elapsed = self.scan_started.map(|t| t.elapsed());
                }
                Err(error) => {
                    let err_msg = error.to_string();
                    self.scan_error = Some(err_msg.clone());
                    self.docker_probing = false;
                    if self.remote_docker_modal_open {
                        self.push_toast(err_msg, Status::Error, cx);
                    }
                }
            }
            self.remote_scan = None;
            self.scan_preview = None;
            self.progress.finished = true;
            cx.notify();
            return false;
        }
        let Some(scan) = &self.scan else {
            return false;
        };
        self.progress = scan.progress.snapshot();
        if self
            .preview_updated
            .is_none_or(|at| at.elapsed().as_secs() >= 1)
        {
            let preview = scan.progress.preview();
            if !preview.entries.is_empty() {
                self.scan_preview = Some(Arc::new(preview));
                self.preview_updated = Some(Instant::now());
            }
        }
        let Some(outcome) = scan.poll() else {
            cx.notify();
            return true;
        };
        match outcome {
            Ok(node) => {
                // A widening scan lands on a new root: move the view up to it,
                // with the directory it came from selected.
                let came_from = (self.scan_root != self.root_path)
                    .then(|| self.root_path.clone());
                if came_from.is_some() {
                    self.clear_filter();
                    self.root_path.clone_from(&self.scan_root);
                    self.space = space_info(&self.root_path).ok();
                    self.device = device_for(&self.root_path);
                    self.file_table = file_table_readable(&self.root_path);
                } else if self.home.as_deref() == Some(self.root_path.as_path())
                {
                    self.space = space_info(&self.root_path).ok();
                }
                let metric = self.options.metric;
                self.marks.refresh(&self.root_path, &node, metric);
                discard(self.tree.replace(Arc::new(node)));
                self.cache = None;
                self.refresh_insights();
                self.scan_elapsed =
                    self.scan_started.map(|started| started.elapsed());
                if let Some(from) = came_from {
                    let crumbs = self.crumbs_for_path(&from);
                    self.crumbs.clear();
                    self.view = View::default();
                    self.transition = None;
                    self.forget_hover();
                    self.selected = crumbs.and_then(|crumbs| {
                        crumbs.first().map(|&top| vec![top])
                    });
                }
                self.keep_selection_valid();
                self.prune_history();
                self.select_largest(cx);
                let home_space = (self.home.as_deref()
                    == Some(self.root_path.as_path()))
                .then_some(self.space)
                .flatten();
                crate::refresh_tray_status_with_space(home_space, cx);
            }
            Err(error) => self.scan_error = Some(error.to_string()),
        }
        self.scan = None;
        self.scan_preview = None;
        self.progress.finished = true;
        cx.notify();
        false
    }

    fn keep_selection_valid(&mut self) {
        let Some(tree) = &self.tree else {
            return;
        };
        if tree.resolve(&self.crumbs).is_none() {
            self.crumbs.clear();
        }
        if let Some(selected) = self.selected.clone()
            && tree.resolve(&selected).is_none()
        {
            self.selected = None;
        }
    }

    /// Space changes under us all the time; poll it so the meter is live
    /// without repainting when nothing moved.
    fn start_space_ticker(cx: &Context<'_, Self>) {
        cx.spawn(async move |this, cx| {
            loop {
                let interval = this
                    .update(cx, |this, _| {
                        if this.screen == Screen::Running {
                            Duration::from_millis(150)
                        } else {
                            Duration::from_millis(1200)
                        }
                    })
                    .unwrap_or(Duration::from_millis(1200));
                cx.background_executor().timer(interval).await;
                let ok = this
                    .update(cx, |this, cx| {
                        if this.remote.is_some()
                            || this.awaiting_root
                            || this.tree.is_none()
                        {
                            return;
                        }
                        let path = this.root_path.clone();
                        let fresh = space_info(&path).ok();
                        if fresh != this.space {
                            this.space = fresh;
                            if this.home.as_deref() == Some(path.as_path()) {
                                crate::refresh_tray_status_with_space(
                                    fresh, cx,
                                );
                            }
                            cx.notify();
                        }
                    })
                    .is_ok();
                if !ok {
                    break;
                }
            }
        })
        .detach();
    }

    // ── navigation ──────────────────────────────────────────────────────

    pub const fn is_indexing(&self) -> bool {
        self.scan.is_some() || self.remote_scan.is_some()
    }

    pub fn tree(&self) -> Option<&Node> {
        self.tree.as_deref()
    }

    /// The node the treemap is currently rooted at.
    pub fn current(&self) -> Option<&Node> {
        self.tree
            .as_ref()
            .and_then(|tree| tree.resolve(&self.crumbs))
    }

    pub fn node_at(&self, crumbs: &[usize]) -> Option<&Node> {
        self.tree.as_ref().and_then(|tree| tree.resolve(crumbs))
    }

    pub fn path_at(&self, crumbs: &[usize]) -> Option<PathBuf> {
        let tree = self.tree.as_ref()?;
        Some(path_of(&self.root_path, tree, crumbs))
    }

    /// Path of the directory currently drawn.
    pub fn current_path(&self) -> PathBuf {
        self.path_at(&self.crumbs)
            .unwrap_or_else(|| self.root_path.clone())
    }

    /// Breadcrumb labels from the scanned root to the current directory.
    /// The trail, from `/`: the directories above the scanned root, which
    /// widen the scan, then the root and the path into the tree.
    pub fn breadcrumbs(&self) -> Vec<(String, Crumb)> {
        let mut trail: Vec<(String, Crumb)> = self
            .root_path
            .ancestors()
            .skip(1)
            .collect::<Vec<_>>()
            .into_iter()
            .rev()
            .map(|path| (crumb_label(path), Crumb::Above(path.to_path_buf())))
            .collect();
        trail.push((crumb_label(&self.root_path), Crumb::Tree(Vec::new())));
        let Some(tree) = &self.tree else {
            return trail;
        };
        let mut crumbs: Vec<usize> = Vec::new();
        for &index in &self.crumbs {
            let Some(node) =
                tree.resolve(&crumbs).and_then(|node| node.child(index))
            else {
                break;
            };
            crumbs.push(index);
            trail.push((node.name.to_string(), Crumb::Tree(crumbs.clone())));
        }
        trail
    }

    /// The children of `parent`, largest first, for a sibling menu, and how
    /// many more there are beyond [`SIBLING_ROWS`].
    pub fn siblings(&self, parent: &[usize]) -> (Vec<Sibling>, usize) {
        let metric = self.options.metric;
        let Some(node) = self.node_at(parent) else {
            return (Vec::new(), 0);
        };
        let mut rows: Vec<Sibling> = node
            .children
            .iter()
            .enumerate()
            .map(|(index, child)| Sibling {
                index,
                name: child.name.to_string(),
                value: child.value(metric),
                category: child.category,
                is_dir: child.is_dir(),
            })
            .collect();
        rows.sort_by_key(|row| std::cmp::Reverse(row.value));
        let more = rows.len().saturating_sub(SIBLING_ROWS);
        rows.truncate(SIBLING_ROWS);
        (rows, more)
    }

    /// Open the sibling menu for the crumb at `crumbs` (not the root).
    pub fn open_crumb_menu(
        &mut self,
        crumbs: &[usize],
        cx: &mut Context<'_, Self>,
    ) {
        let Some((&current, parent)) = crumbs.split_last() else {
            return;
        };
        let (rows, _) = self.siblings(parent);
        let highlighted = rows
            .iter()
            .position(|row| row.index == current)
            .unwrap_or(0);
        self.crumb_menu = Some(CrumbMenu {
            parent: parent.to_vec(),
            current,
            highlighted,
        });
        cx.notify();
    }

    /// Go to a sibling: into it when it is a directory, beside it (selected)
    /// when it is a file.
    pub fn choose_sibling(
        &mut self,
        parent: &[usize],
        index: usize,
        cx: &mut Context<'_, Self>,
    ) {
        self.crumb_menu = None;
        let mut crumbs = parent.to_vec();
        crumbs.push(index);
        if self.node_at(&crumbs).is_some_and(Node::is_dir) {
            self.go_to(crumbs, cx);
        } else {
            self.reveal(crumbs, cx);
        }
    }

    /// Keys while a sibling menu is open: it owns the arrows, Enter and
    /// Escape, and any other key closes it before acting.
    fn on_menu_key(&mut self, key: &str, cx: &mut Context<'_, Self>) -> bool {
        let Some(menu) = self.crumb_menu.clone() else {
            return false;
        };
        let (rows, _) = self.siblings(&menu.parent);
        let last = rows.len().saturating_sub(1);
        match key {
            "down" | "j" => {
                self.set_highlight((menu.highlighted + 1).min(last));
            }
            "up" | "k" => {
                self.set_highlight(menu.highlighted.saturating_sub(1));
            }
            "home" => self.set_highlight(0),
            "end" => self.set_highlight(last),
            "enter" | "space" | "right" | "l" => {
                if let Some(row) = rows.get(menu.highlighted) {
                    self.choose_sibling(&menu.parent, row.index, cx);
                }
            }
            "escape" | "left" | "h" => self.crumb_menu = None,
            _ => {
                self.crumb_menu = None;
                cx.notify();
                return false;
            }
        }
        cx.notify();
        true
    }

    const fn set_highlight(&mut self, row: usize) {
        if let Some(menu) = &mut self.crumb_menu {
            menu.highlighted = row;
        }
    }

    /// Descend into the selected tile, or into the largest child of the
    /// current root when nothing is selected.
    pub fn descend(&mut self, cx: &mut Context<'_, Self>) {
        let target = match self.selected.clone() {
            // Enter what is selected, however deep: a directory opens itself,
            // a file opens the directory holding it.
            Some(selected) if selected.len() > self.crumbs.len() => {
                if self.node_at(&selected).is_some_and(Node::is_dir) {
                    selected
                } else {
                    selected[..selected.len() - 1].to_vec()
                }
            }
            // Nothing below the root is selected: the largest entry.
            _ => match self.current().and_then(Node::largest_child) {
                Some(index) => {
                    let mut crumbs = self.crumbs.clone();
                    crumbs.push(index);
                    crumbs
                }
                None => return,
            },
        };
        let from = self.tile_body(&target).map(|rect| self.view.project(rect));
        self.enter(target, from, cx);
    }

    /// Go inside one child of the current root, so that it fills the viewport.
    ///
    /// `from` is where the child was on screen, when the caller knows it.
    /// Make `target` — any directory below the current root — the root.
    ///
    /// `from` is where that directory's contents were on screen, so the
    /// transition grows them from exactly there into the full viewport.
    fn enter(
        &mut self,
        target: Vec<usize>,
        from: Option<Rect>,
        cx: &mut Context<'_, Self>,
    ) {
        if target.len() <= self.crumbs.len()
            || !target.starts_with(&self.crumbs)
        {
            return;
        }
        let Some(node) = self.node_at(&target) else {
            return;
        };
        if !node.is_dir() || node.children.is_empty() {
            return;
        }
        self.remember();
        self.selected = Some(target.clone());
        self.crumbs = target;
        self.forget_hover();
        self.cache = None;
        let area = self.treemap_size.get();
        let src = from.unwrap_or_else(|| self.view.visible_base(area));
        self.view = View::IDENTITY;
        let dst = Rect::new(
            0.0,
            0.0,
            area.width.as_f32().max(1.0),
            area.height.as_f32().max(1.0),
        );
        self.transition = Some(LayoutTransition::new(src, dst));
        cx.notify();
    }

    /// Where a drawn directory's contents sit: its tile below the name band.
    /// This, not the whole tile, is the region its children occupy, so it is
    /// what a transition into or out of it has to map.
    pub fn tile_body(&mut self, crumbs: &[usize]) -> Option<Rect> {
        let tile =
            self.layout()?.iter().find(|tile| tile.crumbs() == crumbs)?;
        Some(match tile.header {
            Some(header) => Rect::new(
                tile.rect.x,
                header.bottom(),
                tile.rect.w,
                tile.rect.bottom() - header.bottom(),
            ),
            None => tile.rect,
        })
    }

    /// The deepest drawn directory under a viewport point that has contents
    /// to show: what zooming at that point is zooming into.
    fn zoom_target(&mut self, x: f32, y: f32) -> Option<Vec<usize>> {
        let hovered = self.tile_at(x, y)?;
        let root = self.crumbs.len();
        (root + 1..=hovered.len())
            .rev()
            .map(|length| hovered[..length].to_vec())
            .find(|crumbs| {
                self.node_at(crumbs).is_some_and(|node| {
                    node.is_dir() && !node.children.is_empty()
                }) && self.tile_body(crumbs).is_some()
            })
    }

    /// Ascend to the parent directory, keeping the directory we came from in
    /// view so the motion reads as zooming out.
    pub fn ascend(&mut self, cx: &mut Context<'_, Self>) {
        if let Some(parent_crumbs) = self.parent_crumbs() {
            self.ascend_to(parent_crumbs, cx);
        }
    }

    /// Make `ancestor`, a directory above the current root, the root.
    ///
    /// The directory we leave shrinks back into its tile when that tile is
    /// drawn at the new level; from further up it is too small to track, and
    /// the new level simply lands.
    fn ascend_to(&mut self, ancestor: Vec<usize>, cx: &mut Context<'_, Self>) {
        self.remember();
        // The region we are looking at now, and where it sits in the layout
        // we are going back to.
        let area = self.treemap_size.get();
        let child_crumbs = self.crumbs.clone();
        let src = self.view.visible_base(area);
        self.crumbs.clone_from(&ancestor);
        self.forget_hover();
        self.cache = None;
        self.selected = Some(ancestor);
        self.view = View::IDENTITY;
        self.transition = self
            .tile_body(&child_crumbs)
            .filter(|rect| rect.w > 1.0 && rect.h > 1.0)
            .map(|dst| LayoutTransition::new(src, dst));
        cx.notify();
    }

    /// The crumbs of the parent of the current root, if any.
    pub fn parent_crumbs(&self) -> Option<Vec<usize>> {
        if self.crumbs.is_empty() {
            None
        } else {
            Some(self.crumbs[..self.crumbs.len() - 1].to_vec())
        }
    }

    /// Jump straight to a crumb from the breadcrumb bar.
    ///
    /// A jump can skip several levels, so there is no single region to move:
    /// it lands immediately, the way selecting a folder does.
    pub fn go_to(&mut self, crumbs: Vec<usize>, cx: &mut Context<'_, Self>) {
        self.crumb_menu = None;
        // A reveal in the directory on screen goes nowhere.
        if crumbs != self.crumbs {
            self.remember();
        }
        self.crumbs.clone_from(&crumbs);
        self.selected = Some(crumbs);
        self.forget_hover();
        self.view = View::IDENTITY;
        self.transition = None;
        self.cache = None;
        cx.notify();
    }

    /// Note the directory on screen as one to come back to, before leaving
    /// it. A new departure ends whatever was ahead, as in a browser.
    fn remember(&mut self) {
        if self.tree.is_none() {
            return;
        }
        let here = self.current_path();
        if self.history.back.last() != Some(&here) {
            self.history.back.push(here);
        }
        if self.history.back.len() > HISTORY_DEPTH {
            self.history.back.remove(0);
        }
        self.history.forward.clear();
    }

    /// Drop what a new tree no longer holds: a removed directory, or one
    /// outside a new root. What is left always resolves, so `<` and `>` are
    /// enabled exactly when they would go somewhere.
    fn prune_history(&mut self) {
        let mut history = std::mem::take(&mut self.history);
        history
            .back
            .retain(|path| self.crumbs_for_path(path).is_some());
        history
            .forward
            .retain(|path| self.crumbs_for_path(path).is_some());
        self.history = history;
    }

    /// Whether `<` would go anywhere: what drives its button's disabled state.
    pub fn can_go_back(&self) -> bool {
        self.history_target(true).is_some()
    }

    /// Whether `>` would go anywhere.
    pub fn can_go_forward(&self) -> bool {
        self.history_target(false).is_some()
    }

    /// `<`: the directory on screen before this one.
    pub fn go_back(&mut self, cx: &mut Context<'_, Self>) {
        self.step_history(true, cx);
    }

    /// `>`: undo a `<`.
    pub fn go_forward(&mut self, cx: &mut Context<'_, Self>) {
        self.step_history(false, cx);
    }

    /// Where `<` (or `>`) would go, and its place on that stack: the newest
    /// entry that is somewhere else. An entry can be where we already are,
    /// after a widening that was superseded; it is skipped rather than spend
    /// a click on nothing. Every other entry resolves, being pruned whenever
    /// a tree lands.
    pub fn history_target(&self, back: bool) -> Option<(usize, Vec<usize>)> {
        self.tree.as_ref()?;
        let stack = if back {
            &self.history.back
        } else {
            &self.history.forward
        };
        stack.iter().enumerate().rev().find_map(|(at, path)| {
            self.crumbs_for_path(path)
                .filter(|crumbs| *crumbs != self.crumbs)
                .map(|crumbs| (at, crumbs))
        })
    }

    fn step_history(&mut self, back: bool, cx: &mut Context<'_, Self>) {
        let Some((at, target)) = self.history_target(back) else {
            return;
        };
        let here = self.current_path();
        let mut history = std::mem::take(&mut self.history);
        let (from, to) = if back {
            (&mut history.back, &mut history.forward)
        } else {
            (&mut history.forward, &mut history.back)
        };
        from.truncate(at);
        to.push(here);
        // The move is an ordinary one, animation and all; it records itself
        // like any other, so the stacks are put back afterwards.
        self.travel(target, cx);
        self.history = history;
        cx.notify();
    }

    /// Go to `target` with the motion that fits: grow into a directory
    /// below, shrink back out to one above, or land on one beside.
    fn travel(&mut self, target: Vec<usize>, cx: &mut Context<'_, Self>) {
        let enterable = self
            .node_at(&target)
            .is_some_and(|node| node.is_dir() && !node.children.is_empty());
        if target.len() > self.crumbs.len()
            && target.starts_with(&self.crumbs)
            && enterable
        {
            let from =
                self.tile_body(&target).map(|rect| self.view.project(rect));
            self.enter(target, from, cx);
        } else if target.len() < self.crumbs.len()
            && self.crumbs.starts_with(&target)
        {
            self.ascend_to(target, cx);
        } else {
            self.go_to(target, cx);
        }
    }

    /// Show `crumbs` in its directory, selected: what a "worth a look" row
    /// or a found name does.
    pub fn reveal(&mut self, crumbs: Vec<usize>, cx: &mut Context<'_, Self>) {
        let parent = crumbs[..crumbs.len().saturating_sub(1)].to_vec();
        self.go_to(parent, cx);
        self.selected = Some(crumbs);
        self.pointer_active = false;
        cx.notify();
    }

    /// Select the tile at `crumbs` without changing the root.
    pub fn select(
        &mut self,
        crumbs: Option<Vec<usize>>,
        cx: &mut Context<'_, Self>,
    ) {
        self.selected = crumbs;
        cx.notify();
    }

    /// Move the selection geometrically, falling back to the parent at an edge.
    /// The tile a key acts on: under the pointer if the pointer moved last,
    /// otherwise the keyboard selection.
    pub fn action_target(&self) -> Option<Vec<usize>> {
        if self.pointer_active {
            self.hovered.clone().or_else(|| self.selected.clone())
        } else {
            self.selected.clone()
        }
    }

    /// Make the pointed-at tile the selection before a key acts, so marking,
    /// opening and arrow movement all start from what the user is looking at.
    fn adopt_pointer_target(&mut self) {
        if self.pointer_active
            && let Some(hovered) = self.hovered.clone()
        {
            self.selected = Some(hovered);
        }
    }

    /// After the layout changes under a still pointer, its hover is stale
    /// until the pointer moves again.
    fn forget_hover(&mut self) {
        self.hovered = None;
        self.pointer_active = false;
    }

    pub fn move_selection(
        &mut self,
        direction: Direction,
        cx: &mut Context<'_, Self>,
    ) {
        self.pointer_active = false;
        let area = self.treemap_size.get();
        let _ = area;
        let Some(tiles) = self.layout().map(<[Tile]>::to_vec) else {
            return;
        };
        let Some(current) = self.selected.clone() else {
            if let Some(first) = tiles.first() {
                let crumbs = first.crumbs().to_vec();
                self.select(Some(crumbs), cx);
            }
            return;
        };
        let Some(from) = tiles
            .iter()
            .find(|tile| tile.crumbs() == current.as_slice())
            .map(|tile| self.view.project(tile.rect))
        else {
            return;
        };

        // Same-depth neighbours only: stepping into a child by arrow key would
        // make the depth of the selection impossible to predict.
        let depth = current.len();
        let mut best: Option<(f32, Vec<usize>)> = None;
        for tile in &tiles {
            let crumbs = tile.crumbs();
            if crumbs.len() != depth || crumbs == current.as_slice() {
                continue;
            }
            let rect = self.view.project(tile.rect);
            let Some(gap) = direction.gap(&from, &rect) else {
                continue;
            };
            let offset = direction.offset(&from, &rect);
            let score = offset.mul_add(2.5, gap);
            if best.as_ref().is_none_or(|(existing, _)| score < *existing) {
                best = Some((score, crumbs.to_vec()));
            }
        }

        if let Some((_, crumbs)) = best {
            self.select(Some(crumbs), cx);
        } else if direction.is_backwards()
            && let Some(parent) = self.parent_crumbs()
        {
            self.select(Some(parent), cx);
        }
    }

    /// Select the next sibling by rank, which is next-largest by the active
    /// metric. Scanning a directory for space is exactly this walk.
    pub fn cycle_sibling(&mut self, step: isize, cx: &mut Context<'_, Self>) {
        self.pointer_active = false;
        let siblings = self.ranked_siblings();
        if siblings.is_empty() {
            return;
        }
        let current = self.selected.as_ref().and_then(|selected| {
            siblings
                .iter()
                .position(|crumbs| crumbs.as_slice() == selected.as_slice())
        });
        let next = match current {
            Some(index) => {
                let len = siblings.len().cast_signed();
                (((index.cast_signed() + step) % len + len) % len) as usize
            }
            None if step >= 0 => 0,
            None => siblings.len() - 1,
        };
        self.select(Some(siblings[next].clone()), cx);
    }

    fn ranked_siblings(&self) -> Vec<Vec<usize>> {
        let parent = self.selected.as_ref().map_or_else(
            || self.crumbs.clone(),
            |selected| {
                if selected.len() <= self.crumbs.len() {
                    self.crumbs.clone()
                } else {
                    selected[..selected.len() - 1].to_vec()
                }
            },
        );
        let Some(node) = self.node_at(&parent) else {
            return Vec::new();
        };
        (0..node.children.len())
            .map(|index| {
                let mut crumbs = parent.clone();
                crumbs.push(index);
                crumbs
            })
            .collect()
    }

    /// Mark or unmark the current selection.
    pub fn toggle_mark_selected(&mut self, cx: &mut Context<'_, Self>) {
        let Some(crumbs) = self.selected.clone() else {
            return;
        };
        if crumbs.is_empty() {
            self.notice = Some((
                crate::i18n::messages::cannot_remove_root(self.locale).into(),
                Status::Warning,
            ));
            cx.notify();
            return;
        }
        self.toggle_mark(&crumbs, cx);
    }

    /// Mark or unmark the node at `crumbs`.
    ///
    /// A mark covers everything beneath it, since removing a directory takes
    /// its contents with it: marking a directory absorbs the marks already
    /// inside it, and a path inside a marked directory cannot be marked or
    /// kept on its own — it says which mark it goes with instead.
    pub fn toggle_mark(
        &mut self,
        crumbs: &[usize],
        cx: &mut Context<'_, Self>,
    ) {
        let Some(target) = self.target_at(crumbs) else {
            return;
        };
        self.notice = None;
        if !self.marks.contains(&target.path)
            && let Some(ancestor) = self.marked_ancestor(&target.path)
        {
            let target_disp = display_path(&target.path, self.home.as_deref());
            let ancestor_disp = display_path(&ancestor, self.home.as_deref());
            self.notice = Some((
                crate::i18n::messages::goes_with_marked(
                    &target_disp,
                    &ancestor_disp,
                    self.locale,
                ),
                Status::Warning,
            ));
            cx.notify();
            return;
        }
        let path = target.path.clone();
        if self.marks.toggle(target) {
            self.space_baseline = self.space_baseline.or(self.space);
            let inside: Vec<PathBuf> = self
                .marks
                .items()
                .iter()
                .filter(|item| {
                    item.path != path && item.path.starts_with(&path)
                })
                .map(|item| item.path.clone())
                .collect();
            for inner in &inside {
                self.marks.remove(inner);
            }
            if !inside.is_empty() {
                let path_disp = display_path(&path, self.home.as_deref());
                self.notice = Some((
                    crate::i18n::messages::covers_marks_inside(
                        &path_disp,
                        inside.len(),
                        self.locale,
                    ),
                    Status::Neutral,
                ));
            }
        }
        cx.notify();
    }

    /// The marked directory `path` is inside, if any; never `path` itself.
    pub fn marked_ancestor(&self, path: &Path) -> Option<PathBuf> {
        self.marks
            .items()
            .iter()
            .filter(|item| item.path != path && path.starts_with(&item.path))
            .map(|item| item.path.clone())
            .min_by_key(|ancestor| ancestor.as_os_str().len())
    }

    pub fn target_at(&self, crumbs: &[usize]) -> Option<Target> {
        let node = self.node_at(crumbs)?;
        let path = self.path_at(crumbs)?;
        Some(Target {
            hidden: is_hidden(&path),
            path,
            bytes: node.bytes,
            is_dir: node.is_dir(),
        })
    }

    pub fn unmark(&mut self, path: &Path, cx: &mut Context<'_, Self>) {
        self.marks.remove(path);
        cx.notify();
    }

    pub fn clear_marks(&mut self, cx: &mut Context<'_, Self>) {
        self.marks.clear();
        self.notice = None;
        cx.notify();
    }

    fn plan_in_mode(&self, mode: RemovalMode) -> Plan {
        #[cfg(test)]
        if let Some(env) = &self.trash_env {
            let os = self.trash_os.unwrap_or_else(
                ruum_core::trash_adapter::TrashAdapterFactory::current_os,
            );
            let adapter =
                ruum_core::trash_adapter::TrashAdapterFactory::create_for(os);
            return ruum_core::removal::plan_with_trash_adapter(
                self.marks.items(),
                &self.root_path,
                mode,
                adapter.as_ref(),
                env,
            );
        }
        plan_for_mode(self.marks.items(), &self.root_path, mode)
    }

    /// Enter the Review screen, automatically choosing Permanent mode when
    /// every marked local path is blocked in Trash mode (such as items already
    /// in the trash) yet valid for Permanent cleanup.
    pub fn open_review(&mut self) {
        if self.remote.is_none()
            && self.plan_in_mode(RemovalMode::Trash).is_empty()
            && !self.plan_in_mode(RemovalMode::Permanent).is_empty()
        {
            self.removal_mode = RemovalMode::Permanent;
        }
        self.screen = Screen::Review;
    }

    /// The plan the review screen shows and the removal runs.
    pub fn plan(&self) -> Plan {
        if self.remote.is_some() {
            // Never judge remote paths against this machine's filesystem.
            // This is an export selection, not an executable removal plan.
            let mut selection = Plan::default();
            selection.targets = self
                .marks
                .items()
                .iter()
                .filter(|target| {
                    target.path != self.root_path
                        && target.path.starts_with(&self.root_path)
                })
                .cloned()
                .collect();
            selection.root.clone_from(&self.root_path);
            // Remote volume attribution is unknown; never infer it from
            // this computer's mounts or project an unmeasured saving.
            return selection;
        }
        self.plan_in_mode(self.removal_mode)
    }

    // ── layout and hit-testing ──────────────────────────────────────────

    /// Resolve everything the mosaic needs for this frame.
    ///
    /// Runs once per render, before the paint callbacks: painting then only has
    /// to draw, and the marked/hidden/selected state of every tile is decided
    /// here, where the tree and the marks are both at hand.
    pub fn prepare(&mut self) -> Mosaic {
        let metric = self.options.metric;
        let view = self.view;
        let hovered = self.hovered.clone();
        let selected = self.selected.clone();

        // Marks are paths; the mosaic thinks in crumbs. Resolve once per frame
        // rather than building a path for every tile.
        let mut marked: FxHashSet<Vec<usize>> = FxHashSet::default();
        for item in self.marks.items() {
            if let Some(crumbs) = self.crumbs_for_path(&item.path) {
                marked.insert(crumbs);
            }
        }

        let Some(tiles) = self.layout().map(<[Tile]>::to_vec) else {
            return Mosaic {
                view,
                ..Mosaic::default()
            };
        };

        // Hue comes from the node's kind; lightness from its depth in this
        // view, so the first level always reads as the first level.
        let age = self.color_mode == ColorMode::Age;
        let now = self.scanned_at;
        let mut decorations = Vec::with_capacity(tiles.len());
        let mut labels = Vec::new();
        for tile in &tiles {
            let crumbs = tile.crumbs();
            let node = match &tile.kind {
                TileKind::Node { crumbs } => self.node_at(crumbs),
                TileKind::Others { .. } => None,
            };
            let category = node.map_or_else(Default::default, |n| n.category);
            let age_bucket = (age && node.is_some_and(|n| n.modified > 0))
                .then(|| {
                    let days =
                        (now - node.map_or(now, |n| n.modified)) / 86_400;
                    crate::palette::age_bucket(days)
                });
            let filtered = match self.matches.as_deref().map(|m| m.keep(crumbs))
            {
                None | Some(Some(Keep::Whole)) => Filtered::Shown,
                Some(Some(Keep::Partial { .. })) => Filtered::Holds,
                Some(None) => Filtered::Out,
            };
            let is_marked = marked.contains(crumbs);
            // Everything inside a marked directory goes with it, so it is
            // drawn marked too.
            let is_covered = !marked.is_empty()
                && (1..crumbs.len())
                    .any(|length| marked.contains(&crumbs[..length]));
            decorations.push(TileDeco {
                rect: self.animated_rect(tile.rect),
                depth: tile.depth,
                category,
                age_bucket,
                reclaimable: node.is_some_and(|n| n.reclaim.is_some()),
                filtered,
                unreadable: node.is_some_and(|n| n.read_error),
                marked: is_marked,
                covered: is_covered,
                hovered: hovered.as_deref() == Some(crumbs),
                selected: selected.as_deref() == Some(crumbs),
            });

            // Labels are chosen in screen space: zooming in makes room for more
            // of them, which is the point of zooming in.
            let drawn = self.animated_rect(tile.rect);
            let screen = view.project(
                tile.header
                    .map_or(drawn, |header| self.animated_rect(header)),
            );
            if screen.w < LABEL_MIN_W_REMS * self.rem
                || screen.h < LABEL_MIN_H_REMS * self.rem
            {
                continue;
            }
            match &tile.kind {
                TileKind::Node { crumbs } => {
                    let Some(node) = self.node_at(crumbs) else {
                        continue;
                    };
                    labels.push(Label {
                        text: node.name.to_string(),
                        rect: self.animated_rect(tile.rect),
                        header: tile
                            .header
                            .map(|header| self.animated_rect(header)),
                        depth: tile.depth,
                        dim: filtered == Filtered::Out,
                        marked: is_marked || is_covered,
                        size_text: crate::widgets::short_value(node, metric),
                    });
                }
                TileKind::Others { count, .. } => labels.push(Label {
                    text: crate::i18n::messages::plus_more(*count, self.locale),
                    rect: self.animated_rect(tile.rect),
                    header: None,
                    depth: tile.depth,
                    dim: self.matches.is_some(),
                    marked: false,
                    size_text: String::new(),
                }),
            }
        }

        labels.sort_by(|left, right| {
            let left_area = left.rect.w * left.rect.h;
            let right_area = right.rect.w * right.rect.h;
            right_area.total_cmp(&left_area)
        });
        labels.truncate(MAX_LABELS);

        Mosaic {
            tiles: decorations,
            labels,
            view,
        }
    }

    /// Crumbs for an absolute path, if the tree still contains it.
    #[allow(
        clippy::unnecessary_wraps,
        reason = "a marked path may have been removed from the tree already"
    )]
    pub fn crumbs_for_path(&self, path: &Path) -> Option<Vec<usize>> {
        // The path is relative to the scanned root, so the walk starts there,
        // not at the directory currently drawn.
        let relative = path.strip_prefix(&self.root_path).ok()?;
        let tree = self.tree.clone()?;
        let mut node: &Node = &tree;
        let mut crumbs = Vec::new();
        for component in relative.components() {
            let name = component.as_os_str().to_string_lossy();
            let index = node
                .children
                .iter()
                .position(|child| child.name.as_ref() == name)?;
            crumbs.push(index);
            node = node.child(index)?;
        }
        Some(crumbs)
    }

    /// Tiles for the current root and viewport size, computed once per change.
    pub fn layout(&mut self) -> Option<&[Tile]> {
        // A 17 px band at the default rem, scaled so zoom keeps the band's
        // relationship to the label inside it.
        self.layout_options.header = HEADER_REMS * self.rem;
        self.layout_options.header_inner = HEADER_INNER_REMS * self.rem;
        // A filter is about the directory it was typed in: above it, it
        // would hide everything beside that directory, so it lapses.
        if self
            .matches
            .as_ref()
            .is_some_and(|matches| !self.crumbs.starts_with(&matches.base))
        {
            self.clear_filter();
        }
        let area = self.treemap_size.get();
        let key = LayoutKey {
            crumbs: self.crumbs.clone(),
            width: area.width.as_f32().round(),
            height: area.height.as_f32().round(),
            options: self.layout_options.clone(),
            filter: self.filter_epoch,
        };
        if key.width < 1.0 || key.height < 1.0 {
            return None;
        }
        let stale = self.cache.as_ref().is_none_or(|cache| cache.key != key);
        if stale {
            let tree = self.tree.clone()?;
            let node = tree.resolve(&self.crumbs)?;
            let rect = Rect::new(0.0, 0.0, key.width, key.height);
            let filter =
                self.matches.as_deref().filter(|_| self.filter_applied);
            let tiles = layout_filtered(
                node,
                &key.crumbs,
                rect,
                self.options.metric,
                &key.options,
                filter,
            );
            self.cache = Some(LayoutCache { key, tiles });
        }
        self.cache.as_ref().map(|cache| cache.tiles.as_slice())
    }

    /// Base-space rect of the tile at `crumbs`, if it is currently drawn.
    #[cfg(test)]
    pub fn tile_rect(&mut self, crumbs: &[usize]) -> Option<Rect> {
        self.layout()?
            .iter()
            .find(|tile| tile.crumbs() == crumbs)
            .map(|tile| tile.rect)
    }

    /// The deepest tile under a viewport point.
    pub fn tile_at(&mut self, x: f32, y: f32) -> Option<Vec<usize>> {
        let (base_x, base_y) = self.view.unproject(x, y);
        let tiles = self.layout()?;
        hit(tiles, base_x, base_y).map(|tile| tile.crumbs().to_vec())
    }

    // ── view ────────────────────────────────────────────────────────────

    /// Zoom toward a point.
    ///
    /// When `descend` is set the wheel stops magnifying at the point where the
    /// directory under the pointer exactly fits the viewport, and the next
    /// notch goes inside it. That ceiling is what keeps the two gestures
    /// continuous: at the moment the level changes, the tiles inside the
    /// directory are already as large as they will be, so entering makes them
    /// grow rather than shrink.
    pub fn zoom_at(
        &mut self,
        x: f32,
        y: f32,
        factor: f32,
        descend: bool,
        cx: &mut Context<'_, Self>,
    ) {
        let area = self.treemap_size.get();

        // At the bottom of the zoom, zooming out goes up a level.
        if factor < 1.0 && self.view.scale <= View::MIN_SCALE + f32::EPSILON {
            if descend {
                self.ascend(cx);
            }
            return;
        }

        // One directory decides both how far the wheel magnifies and where it
        // then goes: the deepest one under the pointer. At the ceiling its
        // contents fill the view, so going inside continues the same motion.
        let target = if descend {
            self.zoom_target(x, y)
        } else {
            None
        };
        let body = target.as_ref().and_then(|crumbs| self.tile_body(crumbs));
        let ceiling = body
            .map_or(View::MAX_SCALE, |rect| View::fit_scale(rect, area))
            .clamp(View::MIN_SCALE, View::MAX_SCALE);

        if factor > 1.0
            && self.view.scale >= ceiling - f32::EPSILON
            && let Some(target) = target
        {
            let from = body.map(|rect| self.view.project(rect));
            self.enter(target, from, cx);
            return;
        }

        self.view = self.view.zoomed_at(x, y, factor, ceiling).clamped(area);
        cx.notify();
    }

    pub fn reset_view(&mut self, cx: &mut Context<'_, Self>) {
        self.view = View::default();
        cx.notify();
    }

    /// Change how many levels are drawn, which is the other meaning of zoom in
    /// a treemap: seeing further in without changing what is on screen.
    pub fn adjust_depth(&mut self, step: i32, cx: &mut Context<'_, Self>) {
        let depth = (self.layout_options.max_depth.cast_signed() + step)
            .clamp(1, 6) as u32;
        self.layout_options.max_depth = depth;
        self.cache = None;
        cx.notify();
    }

    pub fn toggle_metric(&mut self, cx: &mut Context<'_, Self>) {
        // Children are ordered by the metric, so every crumb moves. Keep the
        // paths and find them again in the reordered tree.
        let directory = self.current_path();
        let selected = self.selected.as_deref().and_then(|c| self.path_at(c));
        self.options.metric = self.options.metric.toggled();
        if let Some(tree) = &self.tree {
            let mut tree = (**tree).clone();
            ruum_core::tree::aggregate(&mut tree, self.options.metric);
            discard(self.tree.replace(Arc::new(tree)));
            let metric = self.options.metric;
            self.marks.refresh(
                &self.root_path,
                self.tree.as_ref().unwrap(),
                metric,
            );
        }
        self.crumbs = self.crumbs_for_path(&directory).unwrap_or_default();
        self.selected = selected.and_then(|path| self.crumbs_for_path(&path));
        self.forget_hover();
        self.crumb_menu = None;
        self.transition = None;
        self.refresh_insights();
        self.clear_filter();
        self.cache = None;
        cx.notify();
    }

    /// The Size | Files | Age choice: what areas measure, and what colour
    /// says. Age keeps areas by size, since age has no area of its own.
    pub fn set_mode(&mut self, index: usize, cx: &mut Context<'_, Self>) {
        let (metric, color) = match index {
            1 => (Metric::Files, ColorMode::Kind),
            2 => (Metric::Bytes, ColorMode::Age),
            _ => (Metric::Bytes, ColorMode::Kind),
        };
        self.color_mode = color;
        if self.options.metric != metric {
            self.toggle_metric(cx);
        }
        cx.notify();
    }

    /// The Size | Files | Age choice currently on.
    pub const fn mode_index(&self) -> usize {
        match (self.color_mode, self.options.metric) {
            (ColorMode::Age, _) => 2,
            (ColorMode::Kind, Metric::Files) => 1,
            (ColorMode::Kind, Metric::Bytes) => 0,
        }
    }

    // ── removal ─────────────────────────────────────────────────────────

    /// The review screen's commit: move to the trash at once, or ask first for
    /// a permanent deletion, which cannot be undone. On remote tabs, generates
    /// a deterministic `.sh` script for marked file targets.
    pub fn commit(&mut self, cx: &mut Context<'_, Self>) {
        if self.remote.is_some() {
            self.generate_remote_cleanup_script(cx);
            return;
        }
        let plan = self.plan();
        if plan.is_empty() {
            return;
        }
        if self.removal_mode == RemovalMode::Permanent {
            self.confirm_open = true;
            self.focus_request = Some(FocusTarget::Dialog);
            cx.notify();
        } else {
            self.begin_removal(cx);
        }
    }

    /// The alert dialog's `Delete`.
    pub fn confirm_delete(&mut self, cx: &mut Context<'_, Self>) {
        self.confirm_open = false;
        self.focus_request = Some(FocusTarget::Root);
        self.begin_removal(cx);
    }

    /// The alert dialog's `Cancel`, or Escape.
    pub fn cancel_delete(&mut self, cx: &mut Context<'_, Self>) {
        self.confirm_open = false;
        self.focus_request = Some(FocusTarget::Root);
        cx.notify();
    }

    /// Move focus if something asked for it. Called wherever a window is in
    /// hand: the key listener and the click listeners.
    pub fn apply_focus(
        &mut self,
        window: &mut Window,
        cx: &mut Context<'_, Self>,
    ) {
        match self.focus_request.take() {
            Some(FocusTarget::Root) => window.focus(&self.focus, cx),
            Some(FocusTarget::Dialog) => window.focus(&self.confirm_focus, cx),
            Some(FocusTarget::RemoteConnectDialog) => {
                window.focus(&self.remote_connect_focus, cx);
            }
            Some(FocusTarget::UpdateDialog) => {
                window.focus(&self.update_focus, cx);
            }
            Some(FocusTarget::AiSetupChoiceDialog) => {
                window.focus(&self.ai_setup_choice_focus, cx);
            }
            Some(FocusTarget::BillingDialog) => {
                window.focus(&self.billing_focus, cx);
            }
            None => {}
        }
    }

    /// `ctrl =`, `ctrl -` and `ctrl 0`: interface zoom. Changes the window's
    /// `rem`, which every size in this app is expressed in, so hierarchy and
    /// spacing keep their proportions at every step.
    pub fn zoom_interface(
        &mut self,
        event: &KeyDownEvent,
        window: &mut Window,
    ) -> bool {
        let keystroke = &event.keystroke;
        if !(keystroke.modifiers.control || keystroke.modifiers.platform) {
            return false;
        }
        let steps = crate::ui::ZOOM_STEPS;
        let current = window.rem_size().as_f32() / crate::ui::BASE_REM;
        let index = steps
            .iter()
            .position(|step| (step - current).abs() < 0.01)
            .unwrap_or(2);
        let next = match keystroke.key.as_str() {
            "=" | "+" => (index + 1).min(steps.len() - 1),
            "-" => index.saturating_sub(1),
            "0" => 2,
            _ => return false,
        };
        window.set_rem_size(px(crate::ui::BASE_REM * steps[next]));
        self.cache = None;
        true
    }

    fn spawn_removal(&self, plan: Plan) -> RemovalHandle {
        #[cfg(test)]
        if let Some(env) = &self.trash_env {
            let os = self.trash_os.unwrap_or_else(
                ruum_core::trash_adapter::TrashAdapterFactory::current_os,
            );
            let adapter =
                ruum_core::trash_adapter::TrashAdapterFactory::create_for(os);
            return ruum_core::removal::spawn_with_trash_adapter(
                plan,
                self.removal_mode,
                adapter,
                env.clone(),
            );
        }
        ruum_core::removal::spawn(plan, self.removal_mode)
    }

    pub fn begin_removal(&mut self, cx: &mut Context<'_, Self>) {
        if self.remote.is_some() {
            return;
        }
        let plan = self.plan();
        if plan.is_empty() {
            self.notice = Some((
                crate::i18n::messages::nothing_is_marked(self.locale).into(),
                Status::Warning,
            ));
            cx.notify();
            return;
        }
        // This window quits once the prompt is answered yes, and quitting
        // would stop a permanent delete halfway.
        if self.restarting {
            self.notice = Some((
                crate::i18n::messages::not_while_restarting(self.locale).into(),
                Status::Warning,
            ));
            cx.notify();
            return;
        }
        self.space_baseline = self.space.or(self.space_baseline);
        self.run_epoch += 1;
        let epoch = self.run_epoch;
        self.run_stage = None;
        self.run_summary = RunSummary {
            total: plan.targets.len(),
            ..RunSummary::default()
        };
        self.run_log.clear();
        self.done_return_screen = Screen::Explore;
        self.focus_request = Some(FocusTarget::Root);
        self.screen = Screen::Running;
        self.run = Some(self.spawn_removal(plan));
        Self::poll_removal(epoch, cx);
        cx.notify();
    }

    /// Execute selected local Docker cleanup targets from `Screen::Docker`.
    pub fn commit_docker_cleanup(&mut self, cx: &mut Context<'_, Self>) {
        let targets = self.selected_local_docker_targets();
        if targets.is_empty() {
            return;
        }
        if self.restarting {
            self.notice = Some((
                crate::i18n::messages::not_while_restarting(self.locale).into(),
                Status::Warning,
            ));
            cx.notify();
            return;
        }
        let local_space = if self.active_host_tab == HostTabKey::Local {
            self.space
        } else {
            self.host_tabs
                .iter()
                .find(|t| t.key == HostTabKey::Local)
                .and_then(|t| t.session.as_ref())
                .and_then(|s| s.space)
        };
        self.space_baseline = local_space.or(self.space_baseline);
        self.run_epoch += 1;
        let epoch = self.run_epoch;
        self.run_stage = None;
        self.run_summary = RunSummary {
            total: targets.len(),
            ..RunSummary::default()
        };
        self.run_log.clear();
        self.done_return_screen = Screen::Docker;
        self.focus_request = Some(FocusTarget::Root);
        self.screen = Screen::Running;
        self.begin_docker_cleanup(epoch, targets, cx);
        cx.notify();
    }

    fn begin_docker_cleanup(
        &mut self,
        epoch: u64,
        targets: Vec<DockerCleanTarget>,
        cx: &mut Context<'_, Self>,
    ) {
        let DockerProbeStatus::Available(summary) = self.local_docker.clone()
        else {
            self.finish_removal_run(false, cx);
            return;
        };
        let (docker_items, _) = self.local_docker_items();
        let estimated_map: Vec<(DockerCleanTarget, u64)> = docker_items
            .into_iter()
            .map(|i| (i.target, i.reclaimable_bytes))
            .collect();

        let mut containers = Vec::new();
        let mut images = Vec::new();
        let mut build_cache = Vec::new();
        for target in targets {
            match target {
                DockerCleanTarget::Container { .. } => containers.push(target),
                DockerCleanTarget::Image { .. } => images.push(target),
                DockerCleanTarget::BuildCache => build_cache.push(target),
            }
        }
        let mut phases: Vec<(&'static str, Vec<DockerCleanTarget>)> =
            Vec::new();
        if !containers.is_empty() {
            phases.push((
                crate::i18n::pages::docker_stage_containers(self.locale),
                containers,
            ));
        }
        if !images.is_empty() {
            phases.push((
                crate::i18n::pages::docker_stage_images(self.locale),
                images,
            ));
        }
        if !build_cache.is_empty() {
            phases.push((
                crate::i18n::pages::docker_stage_build_cache(self.locale),
                build_cache,
            ));
        }

        let local_root = if self.active_host_tab == HostTabKey::Local {
            self.root_path.clone()
        } else {
            self.host_tabs
                .iter()
                .find(|t| t.key == HostTabKey::Local)
                .and_then(|t| t.session.as_ref())
                .map(|s| s.root_path.clone())
                .or_else(std::env::home_dir)
                .unwrap_or_default()
        };
        let refreshing_stage =
            crate::i18n::pages::docker_stage_refreshing(self.locale);
        #[cfg(test)]
        let test_runner = self.docker_runner.clone();

        cx.spawn(async move |this, cx| {
            for (stage_label, phase_targets) in phases {
                let still_active = this
                    .update(cx, |this, cx| {
                        if this.run_epoch != epoch {
                            return false;
                        }
                        this.run_stage = Some(stage_label.to_owned());
                        cx.notify();
                        true
                    })
                    .unwrap_or(false);
                if !still_active {
                    return;
                }

                let summary_for_phase = summary.clone();
                #[cfg(test)]
                let runner_for_phase = test_runner.clone();
                let phase_outcomes = cx
                    .background_executor()
                    .spawn(async move {
                        let adapter = DockerFactory::create();
                        #[cfg(test)]
                        {
                            if let Some(runner) = runner_for_phase {
                                adapter.run_clean_targets_with(
                                    Path::new("docker"),
                                    &summary_for_phase,
                                    &phase_targets,
                                    runner.as_ref(),
                                )
                            } else {
                                phase_targets
                                    .into_iter()
                                    .map(|target| DockerCleanOutcome {
                                        target,
                                        result: Err(
                                            "no mock docker runner configured"
                                                .to_owned(),
                                        ),
                                    })
                                    .collect()
                            }
                        }
                        #[cfg(not(test))]
                        {
                            let env = DockerEnv::current();
                            adapter.run_clean_targets(
                                &env,
                                &summary_for_phase,
                                &phase_targets,
                            )
                        }
                    })
                    .await;

                let ok = this
                    .update(cx, |this, cx| {
                        if this.run_epoch != epoch {
                            return false;
                        }
                        this.record_docker_clean_outcomes(
                            phase_outcomes,
                            &estimated_map,
                        );
                        cx.notify();
                        true
                    })
                    .unwrap_or(false);
                if !ok {
                    return;
                }
            }

            let _ = this.update(cx, |this, cx| {
                if this.run_epoch == epoch {
                    this.run_stage = Some(refreshing_stage.to_owned());
                    cx.notify();
                }
            });

            #[cfg(test)]
            let runner_for_probe = test_runner;
            let (refreshed_docker, refreshed_space) = cx
                .background_executor()
                .spawn(async move {
                    let adapter = DockerFactory::create();
                    #[cfg(test)]
                    let fresh_docker = runner_for_probe.map(|runner| {
                        adapter.probe_binary_with(
                            Path::new("docker"),
                            runner.as_ref(),
                        )
                    });
                    #[cfg(not(test))]
                    let fresh_docker =
                        Some(adapter.probe(&DockerEnv::current()));
                    let fresh_space = if local_root.as_os_str().is_empty() {
                        None
                    } else {
                        space_info(&local_root).ok()
                    };
                    (fresh_docker, fresh_space)
                })
                .await;

            let _ = this.update(cx, |this, cx| {
                if this.run_epoch != epoch {
                    return;
                }
                if let Some(fresh_docker) = refreshed_docker {
                    this.apply_local_docker_status(fresh_docker);
                }
                if let Some(sp) = refreshed_space {
                    if this.active_host_tab == HostTabKey::Local {
                        this.space = Some(sp);
                    } else if let Some(local_tab) = this
                        .host_tabs
                        .iter_mut()
                        .find(|t| t.key == HostTabKey::Local)
                        && let Some(snap) = &mut local_tab.session
                    {
                        snap.space = Some(sp);
                    }
                }
                this.finish_removal_run(false, cx);
            });
        })
        .detach();
    }

    fn record_docker_clean_outcomes(
        &mut self,
        outcomes: Vec<DockerCleanOutcome>,
        estimated_map: &[(DockerCleanTarget, u64)],
    ) {
        for outcome in outcomes {
            let label = match &outcome.target {
                DockerCleanTarget::BuildCache => {
                    "docker: build cache".to_owned()
                }
                DockerCleanTarget::Image { id, reference, .. } => {
                    let name = reference.as_deref().unwrap_or(id.as_str());
                    format!("docker: image {name}")
                }
                DockerCleanTarget::Container { name, id } => {
                    let label = if name.is_empty() { id } else { name };
                    format!("docker: container {label}")
                }
            };
            let bytes = estimated_map
                .iter()
                .find(|(t, _)| *t == outcome.target)
                .map_or(0, |(_, b)| *b);
            if outcome.result.is_ok() {
                self.run_summary.removed += 1;
                self.run_summary.bytes =
                    self.run_summary.bytes.saturating_add(bytes);
            } else {
                self.run_summary.failed += 1;
            }
            self.run_log.push((PathBuf::from(label), outcome.result));
        }
    }

    fn finish_removal_run(
        &mut self,
        had_files: bool,
        cx: &mut Context<'_, Self>,
    ) {
        self.run = None;
        self.run_stage = None;
        if had_files {
            self.marks.clear();
            if self.remote.is_none() {
                self.space = space_info(&self.root_path).ok();
            }
        }
        let home_space = (self.remote.is_none()
            && self.home.as_deref() == Some(self.root_path.as_path()))
        .then_some(self.space)
        .flatten();
        crate::refresh_tray_status_with_space(home_space, cx);
        self.focus_request = Some(FocusTarget::Root);
        self.screen = Screen::Done;
        if had_files {
            // The tree on screen is now wrong; start over rather than leaving
            // numbers that include what was just removed.
            self.start_scan(cx);
        }
        cx.notify();
    }

    /// Leave [`Screen::Done`] back to its originating screen and reset the
    /// return target to [`Screen::Explore`].
    pub fn finish_done_screen(&mut self, cx: &mut Context<'_, Self>) {
        let mut target = self.done_return_screen;
        if target == Screen::Docker && !self.has_local_docker_daemon() {
            target = Screen::Explore;
        }
        self.done_return_screen = Screen::Explore;
        self.run_stage = None;
        self.screen = target;
        self.focus_request = Some(FocusTarget::Root);
        cx.notify();
    }

    fn poll_removal(epoch: u64, cx: &Context<'_, Self>) {
        cx.spawn(async move |this, cx| {
            loop {
                cx.background_executor()
                    .timer(Duration::from_millis(60))
                    .await;
                let keep_going = this
                    .update(cx, |this, cx| this.poll_removal_once(epoch, cx))
                    .unwrap_or(false);
                if !keep_going {
                    break;
                }
            }
        })
        .detach();
    }

    /// Drain the removal channel once. Returns whether the run should keep
    /// being polled. Driven by the poll task above, and by the tests, which
    /// cannot wait on the test clock while a real worker thread runs.
    pub(crate) fn poll_removal_once(
        &mut self,
        epoch: u64,
        cx: &mut Context<'_, Self>,
    ) -> bool {
        if epoch != self.run_epoch {
            return false;
        }
        let Some(run) = &self.run else {
            return false;
        };
        let mut finished = false;
        while let Some(event) = run.poll() {
            match event {
                RemovalEvent::Start { total } => self.run_summary.total = total,
                RemovalEvent::Item {
                    path,
                    bytes,
                    outcome,
                } => {
                    if outcome.is_ok() {
                        self.run_summary.removed += 1;
                        self.run_summary.bytes += bytes;
                    } else {
                        self.run_summary.failed += 1;
                    }
                    self.run_log.push((path, outcome));
                }
                RemovalEvent::Done {
                    removed,
                    bytes,
                    failed,
                } => {
                    self.run_summary.removed = removed;
                    self.run_summary.bytes = bytes;
                    self.run_summary.failed = failed;
                    finished = true;
                }
            }
        }
        if finished {
            self.run = None;
            self.finish_removal_run(true, cx);
            return false;
        }
        cx.notify();
        !finished
    }

    fn current_remote_header(&self) -> ruum_core::remote::Header {
        if let Some(header) = &self.remote_header {
            let mut merged = header.clone();
            merged.docker = self.docker.clone();
            return merged;
        }
        let platform = if self.remote_platform.is_empty() {
            "linux"
        } else {
            self.remote_platform.as_str()
        };
        let mut header = ruum_core::remote::Header::new(
            self.root_path.clone(),
            self.home.clone(),
            platform,
            self.remote_identity.clone(),
        );
        header.disk_root.clone_from(&self.disk_root);
        header.space = self.space;
        header.docker = self.docker.clone();
        header
    }

    pub fn app_data_dir_for_scripts(&self) -> Option<PathBuf> {
        self.db_path
            .as_deref()
            .and_then(|p| p.parent().filter(|d| !d.as_os_str().is_empty()))
            .map(Path::to_path_buf)
            .or_else(ruum_watchdog::config::config_dir)
    }

    /// Generate and atomically write a deterministic `.sh` cleanup script from
    /// the current remote Review file selection, persist pending script metadata
    /// on the SSH card, and show the generated-script action dialog.
    pub fn generate_remote_cleanup_script(
        &mut self,
        cx: &mut Context<'_, Self>,
    ) {
        let plan = self.plan();
        if plan.is_empty() {
            return;
        }
        let reclaim_bytes = plan.bytes();
        self.generate_remote_script_for_targets(
            plan.targets,
            reclaim_bytes,
            Vec::new(),
            cx,
        );
    }

    /// Generate and atomically write a deterministic `.sh` cleanup script for
    /// selected Docker items on the active SSH server from the remote Docker
    /// modal.
    pub fn generate_remote_docker_script(
        &mut self,
        cx: &mut Context<'_, Self>,
    ) {
        let docker_targets = self.selected_review_docker_targets();
        if docker_targets.is_empty() {
            return;
        }
        let reclaim_bytes = self.selected_review_docker_bytes();
        self.remote_docker_modal_open = false;
        self.generate_remote_script_for_targets(
            Vec::new(),
            reclaim_bytes,
            docker_targets,
            cx,
        );
    }

    fn generate_remote_script_for_targets(
        &mut self,
        targets: Vec<Target>,
        reclaim_bytes: u64,
        docker_targets: Vec<DockerCleanTarget>,
        cx: &mut Context<'_, Self>,
    ) {
        if self.remote.is_none()
            || (targets.is_empty() && docker_targets.is_empty())
        {
            return;
        }
        let locale = self.locale;
        let Some(profile_id) = self.active_ssh_profile_id.clone() else {
            self.push_toast(
                crate::i18n::messages::script_generate_failed(
                    "missing active SSH profile",
                    locale,
                ),
                Status::Error,
                cx,
            );
            return;
        };
        let Some(profile) = self
            .ssh_profiles
            .iter()
            .find(|p| p.id == profile_id)
            .cloned()
        else {
            self.push_toast(
                crate::i18n::messages::script_generate_failed(
                    "SSH profile not found in storage",
                    locale,
                ),
                Status::Error,
                cx,
            );
            return;
        };

        let header = self.current_remote_header();
        let item_count = targets.len() + docker_targets.len();
        let spec = RemoteCleanupScriptSpec {
            profile_alias: profile.alias.clone(),
            host: profile.host,
            port: profile.port,
            username: profile.username,
            header,
            targets,
            docker_targets,
        };
        let script_content = match build_remote_cleanup_script(&spec) {
            Ok(content) => content,
            Err(err) => {
                self.push_toast(
                    crate::i18n::messages::script_generate_failed(&err, locale),
                    Status::Error,
                    cx,
                );
                return;
            }
        };
        let Some(app_data_dir) = self.app_data_dir_for_scripts() else {
            self.push_toast(
                crate::i18n::messages::script_generate_failed(
                    "cannot resolve application data directory",
                    locale,
                ),
                Status::Error,
                cx,
            );
            return;
        };
        let now_secs = u64::try_from(now_seconds()).unwrap_or(0);
        let script_path = match write_cleanup_script_atomic(
            &app_data_dir,
            &profile.alias,
            now_secs,
            &script_content,
        ) {
            Ok(path) => path,
            Err(err) => {
                self.push_toast(
                    crate::i18n::messages::script_generate_failed(
                        &err.to_string(),
                        locale,
                    ),
                    Status::Error,
                    cx,
                );
                return;
            }
        };

        if let Ok(db) = self.open_db() {
            let _ = db.set_ssh_profile_pending_script(
                &profile.id,
                &script_path,
                item_count as u64,
                reclaim_bytes,
                now_secs,
            );
        }
        self.present_generated_remote_script(
            profile.id,
            profile.alias,
            script_path,
            item_count,
            reclaim_bytes,
            cx,
        );
    }

    pub fn present_generated_remote_script(
        &mut self,
        profile_id: String,
        alias: String,
        script_path: PathBuf,
        item_count: usize,
        reclaim_bytes: u64,
        cx: &mut Context<'_, Self>,
    ) {
        self.reload_ssh_profiles();
        self.generated_script_dialog = Some(GeneratedScriptDialog {
            profile_id,
            alias,
            script_path,
            item_count,
            reclaim_bytes,
        });
        self.focus_request = Some(FocusTarget::Dialog);
        cx.notify();
    }

    pub fn close_generated_script_dialog(
        &mut self,
        cx: &mut Context<'_, Self>,
    ) {
        self.generated_script_dialog = None;
        self.focus_request = Some(FocusTarget::Root);
        cx.notify();
    }

    #[cfg_attr(
        not(test),
        allow(
            clippy::unused_self,
            reason = "reads terminal override fields on Ruum under cfg(test)"
        )
    )]
    fn terminal_adapter_and_env(
        &self,
    ) -> (
        Box<dyn ruum_core::terminal_launcher::TerminalAdapter>,
        TerminalEnv,
    ) {
        #[cfg(test)]
        let adapter = TerminalFactory::create_for(
            self.terminal_os_override
                .unwrap_or_else(TerminalFactory::current_os),
        );
        #[cfg(not(test))]
        let adapter = TerminalFactory::create();

        #[cfg(test)]
        let env = self
            .terminal_env_override
            .clone()
            .unwrap_or_else(TerminalEnv::current);
        #[cfg(not(test))]
        let env = TerminalEnv::current();

        (adapter, env)
    }

    #[cfg_attr(
        not(test),
        allow(
            clippy::needless_pass_by_ref_mut,
            reason = "cfg(test) synchronously updates toast via &mut Context"
        )
    )]
    pub fn execute_cleanup_script_in_terminal(
        &mut self,
        profile_id: &str,
        script_path: &Path,
        cx: &mut Context<'_, Self>,
    ) {
        #[cfg(test)]
        let locale = self.locale;
        let alias = self
            .ssh_profiles
            .iter()
            .find(|p| p.id == profile_id)
            .map_or_else(|| profile_id.to_owned(), |p| p.alias.clone());
        self.generated_script_dialog = None;
        self.focus_request = Some(FocusTarget::Root);

        let (adapter, env) = self.terminal_adapter_and_env();
        #[cfg(test)]
        {
            match adapter.build_run_script_plan(
                &env,
                script_path,
                Some(profile_id),
                true,
            ) {
                Ok(plan) => {
                    self.terminal_launch_log
                        .push(("run-script".to_owned(), plan));
                    self.push_toast(
                        crate::i18n::messages::script_launched_in_terminal(
                            &alias, locale,
                        ),
                        Status::Success,
                        cx,
                    );
                }
                Err(err) => {
                    self.push_toast(
                        crate::i18n::messages::script_launch_failed(
                            &err.to_string(),
                            locale,
                        ),
                        Status::Error,
                        cx,
                    );
                }
            }
            cx.notify();
        }
        #[cfg(not(test))]
        {
            let script = script_path.to_path_buf();
            let pid = profile_id.to_owned();
            let task = cx.background_executor().spawn(async move {
                adapter.launch_run_script(&env, &script, Some(&pid), true)
            });
            cx.spawn(async move |this, cx| {
                let res = task.await;
                let _ = this.update(cx, |this, cx| {
                    match res {
                        Ok(_) => this.push_toast(
                            crate::i18n::messages::script_launched_in_terminal(
                                &alias,
                                this.locale,
                            ),
                            Status::Success,
                            cx,
                        ),
                        Err(err) => this.push_toast(
                            crate::i18n::messages::script_launch_failed(
                                &err.to_string(),
                                this.locale,
                            ),
                            Status::Error,
                            cx,
                        ),
                    }
                    cx.notify();
                });
            })
            .detach();
        }
    }

    #[cfg_attr(
        not(test),
        allow(
            clippy::needless_pass_by_ref_mut,
            reason = "cfg(test) records terminal_launch_log on &mut self"
        )
    )]
    pub fn reveal_cleanup_script(
        &mut self,
        script_path: &Path,
        cx: &mut Context<'_, Self>,
    ) {
        let locale = self.locale;
        let (adapter, env) = self.terminal_adapter_and_env();
        #[cfg(test)]
        {
            match adapter.build_reveal_plan(&env, script_path) {
                Ok(plan) => {
                    self.terminal_launch_log.push(("reveal".to_owned(), plan));
                }
                Err(err) => {
                    self.push_toast(
                        crate::i18n::messages::script_reveal_failed(
                            &err.to_string(),
                            locale,
                        ),
                        Status::Error,
                        cx,
                    );
                }
            }
            cx.notify();
        }
        #[cfg(not(test))]
        {
            let script = script_path.to_path_buf();
            let task = cx
                .background_executor()
                .spawn(async move { adapter.reveal_script(&env, &script) });
            cx.spawn(async move |this, cx| {
                let res = task.await;
                let _ = this.update(cx, |this, cx| {
                    if let Err(err) = res {
                        this.push_toast(
                            crate::i18n::messages::script_reveal_failed(
                                &err.to_string(),
                                locale,
                            ),
                            Status::Error,
                            cx,
                        );
                    }
                    cx.notify();
                });
            })
            .detach();
        }
    }

    pub fn discard_ssh_pending_script(
        &mut self,
        profile_id: &str,
        cx: &mut Context<'_, Self>,
    ) {
        let locale = self.locale;
        let profile = self
            .ssh_profiles
            .iter()
            .find(|p| p.id == profile_id)
            .cloned();
        let alias = profile
            .as_ref()
            .map_or_else(|| profile_id.to_owned(), |p| p.alias.clone());
        if let Some(ref prof) = profile
            && let Some(ref script_str) = prof.pending_script_path
        {
            let p = Path::new(script_str);
            if p.parent().is_some_and(|dir| {
                dir.file_name().is_some_and(|n| n == "cleanup-scripts")
            }) {
                let _ = std::fs::remove_file(p);
                let _ = std::fs::remove_file(p.with_extension("command"));
            }
        }
        if let Ok(db) = self.open_db() {
            let _ = db.clear_ssh_profile_pending_script(profile_id);
        }
        if self
            .generated_script_dialog
            .as_ref()
            .is_some_and(|d| d.profile_id == profile_id)
        {
            self.generated_script_dialog = None;
        }
        self.reload_ssh_profiles();
        self.push_toast(
            crate::i18n::messages::script_discarded(&alias, locale),
            Status::Neutral,
            cx,
        );
        cx.notify();
    }

    /// Descend to a path found by the search field.
    /// Recompute what the find text matches in the directory on screen.
    ///
    /// Off the UI thread: a whole disk is millions of names, a few hundred
    /// milliseconds to search, and typing must not wait for it. Each
    /// keystroke supersedes the search before it.
    pub fn refresh_matches(&mut self, cx: &Context<'_, Self>) {
        self.find_epoch += 1;
        let epoch = self.find_epoch;
        let needle = self.find.clone();
        let Some(tree) =
            self.tree.clone().filter(|_| !needle.trim().is_empty())
        else {
            self.matches = None;
            self.filter_applied = false;
            self.finding = false;
            self.filter_epoch += 1;
            return;
        };
        self.finding = true;
        let base = self.crumbs.clone();
        let task = cx.background_executor().spawn(async move {
            tree.resolve(&base)
                .and_then(|node| filter(node, &base, &needle))
        });
        cx.spawn(async move |this, cx| {
            let found = task.await;
            let _ = this.update(cx, |this, cx| {
                if epoch != this.find_epoch {
                    return;
                }
                this.finding = false;
                this.matches = found.map(Arc::new);
                this.filter_epoch += 1;
                if std::mem::take(&mut this.apply_pending) {
                    this.apply_filter(cx);
                }
                cx.notify();
            });
        })
        .detach();
    }

    /// Enter in the find field: lay out only the matches, with the largest
    /// selected so Space marks it.
    pub fn apply_filter(&mut self, cx: &mut Context<'_, Self>) {
        self.find_open = false;
        if self.finding {
            self.apply_pending = true;
            return;
        }
        let Some(matches) = self.matches.clone() else {
            return;
        };
        if matches.count == 0 {
            self.notice = Some((
                crate::i18n::messages::nothing_matches(
                    &matches.needle,
                    self.locale,
                ),
                Status::Warning,
            ));
            cx.notify();
            return;
        }
        self.filter_applied = true;
        self.filter_epoch += 1;
        self.view = View::IDENTITY;
        self.transition = None;
        self.forget_hover();
        self.pointer_active = false;
        let tree = self.tree.clone();
        self.selected = matches
            .keep
            .iter()
            .filter(|(_, keep)| **keep == Keep::Whole)
            .filter_map(|(crumbs, _)| {
                let node = tree.as_deref()?.resolve(crumbs)?;
                Some((node.bytes, crumbs))
            })
            .max_by_key(|(bytes, _)| *bytes)
            .map(|(_, crumbs)| crumbs.clone());
        self.notice = None;
        cx.notify();
    }

    /// Drop the find text and the filter with it.
    pub fn clear_filter(&mut self) {
        self.find_epoch += 1;
        self.finding = false;
        self.apply_pending = false;
        self.find.clear();
        self.find_open = false;
        self.matches = None;
        self.filter_applied = false;
        self.filter_epoch += 1;
    }

    // ── input ───────────────────────────────────────────────────────────

    /// Handle a key press.
    pub fn on_key_down(
        &mut self,
        event: &KeyDownEvent,
        cx: &mut Context<'_, Self>,
    ) {
        self.dispatch_key(event, cx);
    }

    /// Whether a menu command may replace the tree: where `r` would, not
    /// behind the confirmation, and not under the review list or a removal
    /// that is still running.
    pub fn can_start_over(&self) -> bool {
        self.screen == Screen::Explore
            && !self.confirm_open
            && !self.volumes_open
            && !self.remote_docker_modal_open
    }

    /// Ask for a directory and scan it: an app opened from the Dock has no
    /// command line to name one.
    pub fn open_folder(&self, cx: &Context<'_, Self>) {
        if self.remote.is_some() {
            return;
        }
        let chosen = cx.prompt_for_paths(gpui_kit::PathPromptOptions {
            files: false,
            directories: true,
            multiple: false,
            prompt: Some(
                crate::i18n::messages::prompt_scan(self.locale).into(),
            ),
        });
        cx.spawn(async move |this, cx| {
            let Ok(Ok(Some(paths))) = chosen.await else {
                return;
            };
            let Some(path) = paths.into_iter().next() else {
                return;
            };
            // Canonical, like a root from the command line, so a later
            // widening recognises this tree in the wider walk.
            let path = path.canonicalize().unwrap_or(path);
            // The panel does not block the window: the review list or a
            // removal may have started while it was open.
            let _ = this.update(cx, |this, cx| {
                if this.can_start_over() {
                    this.set_root(path, cx);
                }
            });
        })
        .detach();
    }

    /// Save what the plan would act on as a list of paths, one per line,
    /// wherever the user chooses: for a script, or for later.
    pub fn save_delete_list(&mut self, cx: &mut Context<'_, Self>) {
        self.save_export(false, cx);
    }

    pub fn save_delete_script(&mut self, cx: &mut Context<'_, Self>) {
        self.save_export(true, cx);
    }

    fn save_export(&mut self, script: bool, cx: &mut Context<'_, Self>) {
        let plan = self.plan();
        if plan.is_empty() {
            self.notice = Some((
                crate::i18n::messages::nothing_to_save(self.locale).into(),
                Status::Warning,
            ));
            cx.notify();
            return;
        }
        let list = if script {
            self.delete_script(&plan.targets)
        } else {
            ruum_core::export::delete_list(&plan.targets)
        };
        let count = plan.targets.len();
        let directory =
            std::env::home_dir().unwrap_or_else(|| PathBuf::from("."));
        let filename = if script {
            "ruum-delete.sh"
        } else {
            "ruum-delete-list.txt"
        };
        let locale = self.locale;
        let chosen = cx.prompt_for_new_path(&directory, Some(filename));
        cx.spawn(async move |this, cx| {
            let Ok(Ok(Some(path))) = chosen.await else {
                return;
            };
            let path_disp = path.display().to_string();
            let notice = match std::fs::write(&path, list) {
                Ok(()) => (
                    crate::i18n::messages::saved_paths_to(
                        count, &path_disp, locale,
                    ),
                    Status::Success,
                ),
                Err(error) => (
                    crate::i18n::messages::could_not_save_path(
                        &path_disp,
                        &error.to_string(),
                        locale,
                    ),
                    Status::Error,
                ),
            };
            let _ = this.update(cx, |this, cx| {
                this.notice = Some(notice);
                cx.notify();
            });
        })
        .detach();
    }

    fn delete_script(&self, targets: &[Target]) -> String {
        let (identity, platform) = if self.remote.is_some() {
            let platform = if self.remote_platform.is_empty() {
                "linux"
            } else {
                self.remote_platform.as_str()
            };
            (self.remote_identity.clone(), platform)
        } else {
            let local_platform = if cfg!(target_os = "macos") {
                "macos"
            } else {
                "linux"
            };
            (ruum_core::identity::HostIdentity::current(), local_platform)
        };
        ruum_core::export::delete_script_for_platform(
            targets,
            &self.root_path,
            self.remote.as_deref(),
            platform,
            identity.as_ref(),
        )
    }

    pub fn copy_delete_script(&mut self, cx: &mut Context<'_, Self>) {
        let plan = self.plan();
        if plan.is_empty() {
            self.notice = Some((
                crate::i18n::messages::nothing_to_copy(self.locale).into(),
                Status::Warning,
            ));
        } else {
            let script = self.delete_script(&plan.targets);
            cx.write_to_clipboard(gpui_kit::ClipboardItem::new_string(script));
            self.notice = Some((
                crate::i18n::messages::copied_delete_script(self.locale).into(),
                Status::Success,
            ));
        }
        cx.notify();
    }

    /// Copy instructions for a coding agent to free the space by removing
    /// what the plan would act on, after checking each path.
    pub fn copy_agent_prompt(&mut self, cx: &mut Context<'_, Self>) {
        let plan = self.plan();
        if plan.is_empty() {
            self.notice = Some((
                crate::i18n::messages::nothing_to_copy(self.locale).into(),
                Status::Warning,
            ));
            cx.notify();
            return;
        }
        let mut prompt = if self.remote.is_some() {
            ruum_core::export::agent_prompt_for(
                &plan.targets,
                &self.root_path,
                self.space,
                &self.remote_platform,
                self.remote_identity.as_ref(),
            )
        } else {
            ruum_core::export::agent_prompt(
                &plan.targets,
                &self.root_path,
                self.space,
            )
        };
        if let Some(host) = &self.remote {
            prompt = format!(
                "The scan is from remote SSH host {host} ({}), NOT this \
                 computer. All paths below belong to that server. Connect \
                 via SSH; verify the host and paths before acting. \
                 Free space is a snapshot, not live.\n\n{prompt}",
                self.remote_platform
            );
        }
        cx.write_to_clipboard(gpui_kit::ClipboardItem::new_string(prompt));
        let count = plan.targets.len();
        let bytes = ruum_core::size::human_bytes(plan.bytes());
        self.notice = Some((
            crate::i18n::messages::copied_agent_prompt(
                count,
                &bytes,
                self.locale,
            ),
            Status::Success,
        ));
        cx.notify();
    }

    /// Show the tile a key acts on in Finder (or the file manager), selected.
    pub fn reveal_target(&mut self, cx: &mut Context<'_, Self>) {
        if self.remote.is_some() {
            return;
        }
        let crumbs =
            self.action_target().unwrap_or_else(|| self.crumbs.clone());
        let Some(path) = self.path_at(&crumbs) else {
            return;
        };
        // GPUI's reveal cannot report failure, so check what it can't.
        if std::fs::symlink_metadata(&path).is_err() {
            let disp = crate::marks::display_path(&path, self.home.as_deref());
            self.notice = Some((
                crate::i18n::messages::no_longer_on_disk(&disp, self.locale),
                Status::Warning,
            ));
            cx.notify();
            return;
        }
        cx.reveal_path(&path);
    }

    /// Every binding, in reading order of the hint bar, so the keys and the
    /// documented list cannot drift apart.
    fn dispatch_key(
        &mut self,
        event: &KeyDownEvent,
        cx: &mut Context<'_, Self>,
    ) {
        let key = event.keystroke.key.as_str();
        let control = event.keystroke.modifiers.control;
        let shift = event.keystroke.modifiers.shift;
        let alt = event.keystroke.modifiers.alt;

        // The alert dialog owns Enter and Escape while it is open; a key that
        // bubbles up to here must not also act on the screen behind it.
        if self.confirm_open {
            return;
        }
        if self.generated_script_dialog.is_some() {
            if key == "escape" {
                self.close_generated_script_dialog(cx);
            }
            return;
        }
        if self.remote_connect_confirm.is_some() {
            match key {
                "enter" => self.approve_remote_connect(cx),
                "escape" => self.deny_remote_connect(cx),
                _ => {}
            }
            return;
        }
        if self.update_modal_open {
            if key == "escape" {
                self.close_update_modal(cx);
            }
            return;
        }
        if self.ai_setup_choice_modal.is_some() {
            if key == "escape" {
                self.cancel_ai_setup_choice(cx);
            }
            return;
        }
        if self.billing_modal.is_some() {
            if key == "escape" {
                self.close_billing_modal(cx);
            }
            return;
        }

        // The volume picker modal owns its keys while open: arrows move, Enter
        // picks, Escape closes, and nothing behind acts.
        if self.volumes_open {
            match key {
                "escape" => self.close_volumes(cx),
                "enter" => self.choose_volume(cx),
                "up" | "k" if !control => {
                    self.move_volume_highlight(-1, cx);
                }
                "down" | "j" if !control => {
                    self.move_volume_highlight(1, cx);
                }
                _ => {}
            }
            return;
        }
        if self.remote_docker_modal_open {
            if key == "escape" {
                self.close_remote_docker_modal(cx);
            }
            return;
        }
        if self.screen == Screen::Remote {
            if self.ssh_form_open {
                if key == "escape" {
                    self.close_ssh_form(cx);
                }
                return;
            }
            if key == "escape" {
                self.screen = Screen::Explore;
                self.focus_request = Some(FocusTarget::Root);
                cx.notify();
            }
            return;
        }

        // Escape on Docker, Watchdog, or Settings returns to Explore:
        if matches!(
            self.screen,
            Screen::Docker | Screen::Watchdog | Screen::Settings
        ) && key == "escape"
        {
            self.screen = Screen::Explore;
            cx.notify();
            return;
        }

        // ⌘ chords belong to the menu bar (⌘Q, ⌘W, ⌘R) or to the system.
        // Read as plain letters they would act twice or by surprise: ⌘D
        // would re-scan with apparent sizes, ⌘H would hide *and* toggle.
        // Zoom (⌘= ⌘- ⌘0) is handled before this, with a window in hand.
        if event.keystroke.modifiers.platform {
            return;
        }

        if self.crumb_menu.is_some() && self.on_menu_key(key, cx) {
            return;
        }
        if self.power_menu.is_some() && self.on_power_key(key, cx) {
            return;
        }

        if self.show_help {
            if matches!(key, "escape" | "?" | "/" | "q") {
                self.show_help = false;
                cx.notify();
            }
            return;
        }

        // The search field takes the keyboard while it is open. It is a field
        // of three keys on purpose: no editor state to keep in sync with the
        // tree, and Escape always means "give the keyboard back".
        if self.screen == Screen::Explore && self.find_open {
            match key {
                "escape" => self.clear_filter(),
                "enter" => self.apply_filter(cx),
                "backspace" => {
                    self.find.pop();
                    self.refresh_matches(cx);
                }
                _ => {
                    // Some platforms report only `key` for a character and
                    // leave `key_char` empty; a one-character key is a
                    // character either way.
                    let typed = event
                        .keystroke
                        .key_char
                        .as_deref()
                        .unwrap_or(event.keystroke.key.as_str());
                    if !control && typed.chars().count() == 1 {
                        self.find.push_str(typed);
                        self.refresh_matches(cx);
                    }
                }
            }
            cx.notify();
            return;
        }

        match self.screen {
            // Alt-arrows are history in every browser and file manager.
            Screen::Explore if alt && key == "left" => self.go_back(cx),
            Screen::Explore if alt && key == "right" => self.go_forward(cx),
            Screen::Explore => self.on_explore_key(key, control, shift, cx),
            Screen::Docker | Screen::Watchdog | Screen::Settings
                if key == "escape" =>
            {
                self.screen = Screen::Explore;
                cx.notify();
            }
            Screen::Docker
            | Screen::Remote
            | Screen::Watchdog
            | Screen::Settings => {}
            Screen::Review => self.on_review_key(key, cx),
            Screen::Running => {
                if key == "escape"
                    && let Some(run) = &self.run
                {
                    run.cancel();
                    self.notice = Some((
                        crate::i18n::messages::stopping_after_current(
                            self.locale,
                        )
                        .into(),
                        Status::Warning,
                    ));
                    cx.notify();
                }
            }
            Screen::Done => {
                if matches!(key, "escape" | "enter") {
                    self.finish_done_screen(cx);
                }
            }
        }
    }

    fn on_explore_key(
        &mut self,
        key: &str,
        control: bool,
        shift: bool,
        cx: &mut Context<'_, Self>,
    ) {
        // Keys that act on "the current tile" start from the pointer when it
        // moved last.
        if matches!(
            key,
            "space"
                | "x"
                | "enter"
                | "tab"
                | "left"
                | "right"
                | "up"
                | "down"
                | "h"
                | "j"
                | "k"
                | "l"
        ) {
            self.adopt_pointer_target();
        }
        match key {
            "/" | "s" if !control => {
                self.find_open = true;
                cx.notify();
            }
            "enter" => self.descend(cx),
            "right" | "l" if !control => {
                self.move_selection(Direction::Right, cx);
            }
            "left" | "h" if !control => {
                self.move_selection(Direction::Left, cx);
            }
            "up" | "k" if !control => self.move_selection(Direction::Up, cx),
            "down" | "j" if !control => {
                self.move_selection(Direction::Down, cx);
            }
            "backspace" | "u" if !control => self.ascend(cx),
            // A filter is the first thing Escape takes away.
            "escape" if self.matches.is_some() => self.clear_filter(),
            "escape" if self.scan.is_some() || self.remote_scan.is_some() => {
                self.cancel_scan(cx);
            }
            "escape" => {
                if self.selected.is_some() {
                    self.selected = None;
                } else {
                    self.ascend(cx);
                }
                cx.notify();
            }
            "space" => self.toggle_mark_selected(cx),
            "x" if !control => self.toggle_mark_selected(cx),
            "tab" => self.cycle_sibling(if shift { -1 } else { 1 }, cx),
            "c" if !control => {
                if self.marks.is_empty() {
                    self.notice = Some((
                        crate::i18n::messages::mark_something_first(
                            self.locale,
                        )
                        .into(),
                        Status::Warning,
                    ));
                } else {
                    self.open_review();
                }
                cx.notify();
            }
            "[" => self.adjust_depth(-1, cx),
            "]" => self.adjust_depth(1, cx),
            "-" => {
                self.zoom_at(
                    self.half_width(),
                    self.half_height(),
                    1.0 / 1.25,
                    false,
                    cx,
                );
            }
            "=" | "+" => {
                self.zoom_at(
                    self.half_width(),
                    self.half_height(),
                    1.25,
                    false,
                    cx,
                );
            }
            "0" => self.reset_view(cx),
            "t" if !control => {
                self.set_mode((self.mode_index() + 1) % 3, cx);
            }
            "r" if !control => self.start_scan(cx),
            "g" if !control => self.go_to_disk(cx),
            "v" if !control && !shift => self.open_volumes(cx),
            "i" if !control => {
                self.options.include_hidden = !self.options.include_hidden;
                self.start_scan(cx);
            }
            "d" if !control => {
                self.options.apparent_size = !self.options.apparent_size;
                self.start_scan(cx);
            }
            "p" if !control => {
                self.show_selection = !self.show_selection;
                cx.notify();
            }
            "o" if !control => self.reveal_target(cx),
            "?" => {
                self.show_help = true;
                cx.notify();
            }
            "q" if !control => crate::app_menu::request_quit(cx),
            _ => {}
        }
    }

    fn on_review_key(&mut self, key: &str, cx: &mut Context<'_, Self>) {
        if self.remote.is_some() && matches!(key, "m" | "p") {
            return;
        }
        if key == "e" {
            self.save_delete_script(cx);
            return;
        }
        match key {
            "escape" => {
                self.screen = Screen::Explore;
                cx.notify();
            }
            "enter" => self.commit(cx),
            "!" => self.clear_marks(cx),
            "p" => {
                self.removal_mode = RemovalMode::Permanent;
                cx.notify();
            }
            "m" => {
                self.removal_mode = RemovalMode::Trash;
                cx.notify();
            }
            "s" => self.save_delete_list(cx),
            "a" => self.copy_agent_prompt(cx),
            "y" => self.copy_delete_script(cx),
            "?" => {
                self.show_help = true;
                cx.notify();
            }
            _ => {}
        }
    }

    fn half_width(&self) -> f32 {
        self.treemap_size.get().width.as_f32() / 2.0
    }

    fn half_height(&self) -> f32 {
        self.treemap_size.get().height.as_f32() / 2.0
    }

    /// Mouse moved over the treemap.
    pub fn on_mouse_move(
        &mut self,
        event: &MouseMoveEvent,
        cx: &mut Context<'_, Self>,
    ) {
        let origin = self.treemap_origin.get();
        let local = Point::new(
            event.position.x - origin.x,
            event.position.y - origin.y,
        );
        // Moves are delivered here even when the pointer is elsewhere in the
        // window. Outside the mosaic there is nothing to hover, and a stale
        // tooltip would cover whatever the pointer went to — the panel's
        // resize handle, for one.
        let area = self.treemap_size.get();
        let inside = local.x >= px(0.)
            && local.y >= px(0.)
            && local.x < area.width
            && local.y < area.height;
        if !inside {
            if self.pointer.is_some() || self.hovered.is_some() {
                self.on_mouse_leave(cx);
            }
            return;
        }
        self.pointer = Some(local);
        self.pointer_active = true;
        let previous = self.hovered.clone();
        self.hovered = self.tile_at(local.x.as_f32(), local.y.as_f32());
        // The cursor tooltip is positioned from `pointer`, so a move *within*
        // one tile still has to repaint — otherwise the tooltip sticks where
        // the tile was first entered until the hover target changes.
        if self.hovered != previous || self.hovered.is_some() {
            cx.notify();
        }
    }

    pub fn on_mouse_leave(&mut self, cx: &mut Context<'_, Self>) {
        self.pointer = None;
        self.pointer_active = false;
        if self.hovered.take().is_some() {
            cx.notify();
        }
    }

    /// Click: select, then act on a repeat click, like a file manager.
    pub fn on_mouse_down(
        &mut self,
        event: &MouseDownEvent,
        cx: &mut Context<'_, Self>,
    ) {
        let origin = self.treemap_origin.get();
        let x = (event.position.x - origin.x).as_f32();
        let y = (event.position.y - origin.y).as_f32();
        let crumbs = self.tile_at(x, y);

        match event.button {
            MouseButton::Left if event.click_count >= 2 => {
                if let Some(crumbs) = crumbs {
                    self.select(Some(crumbs), cx);
                    self.descend(cx);
                }
            }
            MouseButton::Left
                if event.modifiers.control || event.modifiers.platform =>
            {
                if let Some(crumbs) = crumbs {
                    self.toggle_mark(&crumbs, cx);
                }
            }
            MouseButton::Left => {
                let activate = crumbs.as_ref().is_some_and(|crumbs| {
                    self.selected.as_ref() == Some(crumbs)
                        && self.node_at(crumbs).is_some_and(Node::is_dir)
                });
                if activate {
                    // A second click on the selection opens it, like a file
                    // manager, without needing a double click.
                    self.descend(cx);
                    return;
                }
                self.select(crumbs, cx);
            }
            MouseButton::Middle => {
                if let Some(crumbs) = crumbs {
                    self.toggle_mark(&crumbs, cx);
                }
            }
            // What `o` does, for the tile under the pointer: select it, then
            // show it in Finder, File Explorer or the file manager.
            MouseButton::Right => {
                if let Some(crumbs) = crumbs {
                    self.select(Some(crumbs), cx);
                    self.reveal_target(cx);
                }
            }
            // Buttons 8 and 9. gpui-pre maps them on X11, Wayland and
            // Windows; a mouse with no side buttons never sends them, and
            // then the header `<` / `>` and alt-arrows are the whole story.
            MouseButton::Navigate(NavigationDirection::Back)
                if self.screen == Screen::Explore =>
            {
                self.go_back(cx);
            }
            MouseButton::Navigate(NavigationDirection::Forward)
                if self.screen == Screen::Explore =>
            {
                self.go_forward(cx);
            }
            // Side buttons off the explore screen.
            MouseButton::Navigate(_) => {}
        }
    }

    pub fn on_scroll_wheel(
        &mut self,
        event: &ScrollWheelEvent,
        cx: &mut Context<'_, Self>,
    ) {
        let origin = self.treemap_origin.get();
        let x = (event.position.x - origin.x).as_f32();
        let y = (event.position.y - origin.y).as_f32();
        let lines = match event.delta {
            ScrollDelta::Lines(delta) => delta.y,
            ScrollDelta::Pixels(delta) => delta.y.as_f32() / 24.0,
        };
        if lines.abs() < f32::EPSILON {
            return;
        }
        if event.modifiers.shift {
            // Pan instead of zoom, for looking around a magnified view.
            self.view.origin_y =
                (self.view.origin_y - lines * 40.0 / self.view.scale).max(0.0);
            cx.notify();
            return;
        }
        let factor = if lines > 0.0 { 1.15 } else { 1.0 / 1.15 };
        self.zoom_at(x, y, factor, true, cx);
    }

    /// Advance the layout transition, if one is running.
    pub fn tick_transition(&mut self, window: &Window) {
        if let Some(transition) = self.transition {
            let (_, running) = transition.sample(Rect::default());
            if running {
                window.request_animation_frame();
            } else {
                self.transition = None;
            }
        }
    }

    /// A tile's rectangle for this frame: mid-transition it is on its way from
    /// where it was to where it is.
    fn animated_rect(&self, rect: Rect) -> Rect {
        match self.transition {
            Some(transition) => transition.sample(rect).0,
            None => rect,
        }
    }
}

/// A direction for geometric selection movement.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Direction {
    Left,
    Right,
    Up,
    Down,
}

impl Direction {
    const fn is_backwards(self) -> bool {
        matches!(self, Self::Left | Self::Up)
    }

    /// Distance from `from` to `rect` along the axis, if `rect` lies that way.
    fn gap(self, from: &Rect, rect: &Rect) -> Option<f32> {
        let epsilon = 0.5;
        match self {
            Self::Right => {
                let gap = rect.x - from.right();
                (gap >= -epsilon).then_some(gap.max(0.0))
            }
            Self::Left => {
                let gap = from.x - rect.right();
                (gap >= -epsilon).then_some(gap.max(0.0))
            }
            Self::Down => {
                let gap = rect.y - from.bottom();
                (gap >= -epsilon).then_some(gap.max(0.0))
            }
            Self::Up => {
                let gap = from.y - rect.bottom();
                (gap >= -epsilon).then_some(gap.max(0.0))
            }
        }
    }

    /// How far off the direction's axis `rect` sits, so the nearest tile in the
    /// direction wins rather than any tile in that half-plane.
    fn offset(self, from: &Rect, rect: &Rect) -> f32 {
        let overlap =
            |a_start: f32, a_end: f32, b_start: f32, b_end: f32| -> f32 {
                (a_end.min(b_end) - a_start.max(b_start)).max(0.0)
            };
        match self {
            Self::Left | Self::Right => {
                let shared =
                    overlap(from.y, from.bottom(), rect.y, rect.bottom());
                (from.h.min(rect.h) - shared).max(0.0)
            }
            Self::Up | Self::Down => {
                let shared =
                    overlap(from.x, from.right(), rect.x, rect.right());
                (from.w.min(rect.w) - shared).max(0.0)
            }
        }
    }
}

/// A tile's label, resolved for painting.
#[derive(Clone, Debug)]
pub struct Label {
    pub text: String,
    /// Base-space rectangle; the view transform is applied while painting.
    pub rect: Rect,
    /// The band this label belongs in, when its tile reserved one. A parent's
    /// name goes in its own band, never over its children.
    pub header: Option<Rect>,
    /// Nesting depth in this view: the first level is set in bold.
    pub depth: u32,
    /// Filtered out while typing: drawn quietly.
    pub dim: bool,
    pub marked: bool,
    pub size_text: String,
}

/// How many labels one frame will shape.
const MAX_LABELS: usize = 150;

/// Height of a top-level directory's name band, in rem.
const HEADER_REMS: f32 = 1.375;

/// Height of the slim label row a deeper open directory keeps, in rem.
const HEADER_INNER_REMS: f32 = 1.0;

/// A trail step's label: the directory's own name, or `/` for the root.
fn crumb_label(path: &Path) -> String {
    path.file_name().map_or_else(
        || path.display().to_string(),
        |name| name.to_string_lossy().into_owned(),
    )
}

impl Ruum {
    /// Start again as an administrator on the root this window is headed
    /// for, with the same options, and close this window once the prompt is
    /// accepted.
    /// Declined, nothing changes. Not while a removal runs: quitting would
    /// stop a permanent delete halfway.
    pub fn restart_as_administrator(&mut self, cx: &mut Context<'_, Self>) {
        if self.remote.is_some() || self.restarting || self.run.is_some() {
            return;
        }
        self.restarting = true;
        cx.notify();
        // While a widening scan runs, the wider root is where this window
        // ends up, and the drive the offer is about: reopening the folder
        // still on screen would walk it again. `scan_root` equals
        // `root_path` for any other scan, and is stale once none runs.
        let root = if self.scan.is_some() {
            &self.scan_root
        } else {
            &self.root_path
        };
        let args = restart_args(root);
        // Off the UI thread: the prompt runs its own message loop, which
        // would re-enter this window while it is still being handled.
        let restart = cx.background_executor().spawn(async move {
            ruum_core::access::restart_as_administrator(&args)
        });
        cx.spawn(async move |this, cx| {
            let outcome = restart.await;
            let _ = this.update(cx, |this, cx| match outcome {
                Ok(()) => cx.quit(),
                Err(error) => {
                    this.restarting = false;
                    this.notice = Some((
                        crate::i18n::messages::still_not_admin(
                            &error.to_string(),
                            this.locale,
                        ),
                        Status::Warning,
                    ));
                    cx.notify();
                }
            });
        })
        .detach();
    }
}

/// The command line that reopens `root` in `ruum-gui` after elevation.
pub fn restart_args(root: &Path) -> Vec<std::ffi::OsString> {
    vec![root.as_os_str().to_owned()]
}

/// Now, in unsigned Unix seconds.
pub fn current_unix_secs() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map_or(0, |since| since.as_secs())
}

/// Now, in Unix seconds.
pub fn now_seconds() -> i64 {
    i64::try_from(current_unix_secs()).unwrap_or(i64::MAX)
}

/// Let go of a tree off the UI thread: freeing a disk's millions of nodes
/// takes a good part of a second, and nothing waits on it.
fn discard(tree: Option<Arc<Node>>) {
    if let Some(tree) = tree {
        // When no thread can be made, the closure is dropped here instead,
        // and the tree is freed in place rather than taking the window down.
        let _ = std::thread::Builder::new().spawn(move || drop(tree));
    }
}

/// Smallest tile, in rem, that gets a label: below this a name cannot be read.
const LABEL_MIN_W_REMS: f32 = 3.375;
const LABEL_MIN_H_REMS: f32 = 0.9375;

/// How a tile stands against the find text.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Filtered {
    /// It matches, is inside a match, or nothing is being found.
    Shown,
    /// It holds matches: its fill steps back, its name stays readable.
    Holds,
    /// Nothing in it matches.
    Out,
}

/// Where keyboard focus should go once a window is available.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum FocusTarget {
    /// The treemap and screens, which own every key binding.
    Root,
    /// The permanent-deletion alert dialog.
    Dialog,
    /// The remote SSH connect confirmation dialog.
    RemoteConnectDialog,
    /// The centered software update card modal.
    UpdateDialog,
    /// The first-run 3-choice AI onboarding modal.
    AiSetupChoiceDialog,
    /// The Ruum Cloud pass billing modal.
    BillingDialog,
}

impl Render for Ruum {
    fn render(
        &mut self,
        window: &mut Window,
        cx: &mut Context<'_, Self>,
    ) -> impl gpui_kit::IntoElement {
        self.rem = window.rem_size().as_f32();
        self.tick_transition(window);
        // The titlebar names the directory on screen, however it got there:
        // a key, a click, a rescan or a folder chosen from the menu.
        let title = if self.awaiting_root {
            crate::i18n::messages::title_choose_source(self.locale).into()
        } else {
            format!(
                "ruum{} · {}",
                self.remote.as_ref().map_or_else(String::new, |host| {
                    crate::i18n::messages::title_remote_suffix(
                        host,
                        self.locale,
                    )
                }),
                crate::marks::display_path(
                    &self.current_path(),
                    self.home.as_deref()
                )
            )
        };
        if title != self.window_title {
            window.set_window_title(&title);
            self.window_title = title;
        }
        crate::views::root(self, window, cx)
    }
}

/// Canonical lowercase language tag sent to the update service.
#[must_use]
pub const fn locale_code(locale: crate::i18n::Locale) -> &'static str {
    match locale {
        crate::i18n::Locale::En => "en",
        crate::i18n::Locale::Zh => "zh",
        crate::i18n::Locale::Ja => "ja",
        crate::i18n::Locale::Ko => "ko",
    }
}

fn is_loopback_base_url(raw: &str) -> bool {
    let trimmed = raw.trim();
    trimmed.starts_with("http://127.0.0.1")
        || trimmed.starts_with("http://localhost")
        || trimmed.starts_with("http://[::1]")
}

/// Execute a blocking signed manifest update check and persist `last_checked_at_secs`.
pub fn run_update_check_blocking(
    db_path: Option<&Path>,
    settings: &UpdateSettings,
    install_kind: InstallKind,
    lang: &str,
) -> Result<(UpdateSettings, UpdateCheckResult), String> {
    let base_url = resolve_update_base_url();
    if cfg!(test) && !is_loopback_base_url(&base_url) {
        return Err("update checks disabled in test builds".to_owned());
    }
    let pubkey_b64 = resolve_effective_public_key_for_url(&base_url);
    let db = if let Some(path) = db_path {
        StorageDb::open(path).ok()
    } else if !cfg!(test) {
        StorageDb::open_default().ok()
    } else {
        None
    };

    let client_id = if settings.send_client_id {
        if let Some(ref d) = db {
            d.ensure_update_client_id().ok()
        } else if !settings.client_id.trim().is_empty() {
            Some(settings.client_id.clone())
        } else {
            None
        }
    } else {
        None
    };

    let env = UpdateEnv::current();
    let os = env.os.unwrap_or_else(UpdateFactory::current_os);
    let arch = env.arch.unwrap_or_else(UpdateArch::current);

    let result = check_for_updates(&UpdateCheckRequest {
        channel: settings.channel,
        current_version: env!("CARGO_PKG_VERSION"),
        os,
        arch,
        install_kind,
        lang,
        entry: "gui",
        client_id: client_id.as_deref(),
        base_url: &base_url,
        public_key_b64: &pubkey_b64,
        github_repo: DEFAULT_GITHUB_REPO,
        github_fallback_base: None,
    })
    .map_err(|e| e.to_string())?;

    let now = u64::try_from(now_seconds()).unwrap_or(0);
    let mut updated_settings = settings.clone();
    if let Some(cid) = client_id {
        updated_settings.client_id = cid;
    }
    updated_settings.last_checked_at_secs = now;
    updated_settings.updated_at_secs = now;
    if let Some(ref d) = db {
        let _ = d.save_update_settings(&updated_settings);
    }

    Ok((updated_settings, result))
}

/// Apply an update check outcome directly to `AppRuntime` when no window is open.
pub fn apply_update_check_to_runtime(
    outcome: Result<(UpdateSettings, UpdateCheckResult), String>,
    cx: &mut App,
) {
    crate::with_runtime_mut(cx, |rt| match outcome {
        Ok((settings, result)) => {
            rt.update_settings = settings;
            rt.update_install_kind = result.install_kind;
            rt.update_last_error = None;
            rt.update_state = if result.update_available {
                UpdateUiState::Available {
                    info: Arc::new(result),
                }
            } else {
                UpdateUiState::UpToDate {
                    checked_at_secs: rt.update_settings.last_checked_at_secs,
                }
            };
        }
        Err(err) => {
            let prev_info = rt.update_state.info().cloned();
            rt.update_last_error = Some(err.clone());
            rt.update_state = UpdateUiState::Error {
                message: err,
                info: prev_info,
            };
        }
    });
    crate::refresh_tray_update_status(cx);
}

enum PurchaseOutcome {
    Allowed,
    PendingQr {
        order: PurchaseOrderSnapshot,
        qr_matrix: Option<Arc<crate::qr::QrMatrix>>,
    },
}

enum BillingPollStep {
    Stop,
    Skip,
    Poll {
        ai_ctx: ruum_core::ai_cli::AiExecutionContext,
        lock: Arc<Mutex<()>>,
        last_refresh_at: Arc<Mutex<Option<Instant>>>,
    },
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum BillingPollAftermath {
    Continue,
    Stop,
}

fn build_cloud_client(
    ai_ctx: &ruum_core::ai_cli::AiExecutionContext,
) -> Result<CloudClient, CloudApiError> {
    let db = match ai_ctx.db_path.as_deref() {
        Some(path) => StorageDb::open(path),
        None => StorageDb::open_default(),
    }
    .map_err(|e| {
        CloudApiError::new(
            None,
            Some("CLOUD_DB_ERROR"),
            format!("failed to open Ruum database: {e}"),
            None,
        )
    })?;
    let store = if let Some(ref s) = ai_ctx.credential_store {
        s.clone()
    } else if let Some(dir) = db.db_path().parent()
        && !dir.as_os_str().is_empty()
    {
        LazyCredentialStore::platform(dir)
    } else {
        LazyCredentialStore::open_default().map_err(|e| {
            CloudApiError::new(None, Some("CLOUD_KEYCHAIN_ERROR"), e, None)
        })?
    };
    let mut client = CloudClient::new(Arc::new(db), store);
    if let Some(ref base_url) = ai_ctx.cloud_base_url {
        client = client.with_base_url(base_url)?;
    }
    Ok(client)
}

fn sync_entitlement_and_models_inner(
    client: &CloudClient,
    lock: &Mutex<()>,
    last_refresh_at: &Mutex<Option<Instant>>,
    force_refresh: bool,
) -> Result<(EntitlementSnapshot, Vec<CloudVirtualModel>), CloudApiError> {
    let _guard = lock
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner);
    *last_refresh_at
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner) =
        Some(Instant::now());
    let ent = client.fetch_entitlement(force_refresh)?;
    let models = client.fetch_virtual_models()?;
    Ok((ent, models))
}

fn sync_entitlement_and_models_after_allow(
    client: &CloudClient,
    lock: &Mutex<()>,
    last_refresh_at: &Mutex<Option<Instant>>,
) -> Result<(EntitlementSnapshot, Vec<CloudVirtualModel>), CloudApiError> {
    sync_entitlement_and_models_inner(client, lock, last_refresh_at, false)
}

fn refresh_entitlement_and_models_with_client(
    client: &CloudClient,
    lock: &Mutex<()>,
    last_refresh_at: &Mutex<Option<Instant>>,
) -> Result<(EntitlementSnapshot, Vec<CloudVirtualModel>), CloudApiError> {
    let (ent, models) =
        sync_entitlement_and_models_inner(client, lock, last_refresh_at, true)?;
    let now_secs = current_unix_secs();
    if !ent.is_alipay_active_at(now_secs)
        && client.cached_plans_need_refresh(now_secs, 3600)
    {
        let _ = client.fetch_billing_plans();
    }
    Ok((ent, models))
}

fn poll_billing_entitlement_once(
    ai_ctx: &ruum_core::ai_cli::AiExecutionContext,
    lock: &Mutex<()>,
    last_refresh_at: &Mutex<Option<Instant>>,
) -> Result<EntitlementSnapshot, CloudApiError> {
    let client = build_cloud_client(ai_ctx)?;
    let _guard = lock
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner);
    *last_refresh_at
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner) =
        Some(Instant::now());
    let ent = client.fetch_entitlement(true)?;
    if ent.is_paid {
        let _ = client.fetch_virtual_models();
    }
    Ok(ent)
}

enum DownloadTransition {
    Finished {
        state: UpdateUiState,
        last_error: Option<String>,
    },
    Progress {
        downloaded: u64,
        total: u64,
    },
}

fn classify_download_update(
    info: &Arc<UpdateCheckResult>,
    downloaded: u64,
    total: u64,
    cancelled: bool,
    done: Option<Result<PathBuf, String>>,
) -> DownloadTransition {
    match done {
        Some(Ok(artifact_path)) => DownloadTransition::Finished {
            state: UpdateUiState::ReadyToInstall {
                info: Arc::clone(info),
                artifact_path,
            },
            last_error: None,
        },
        Some(Err(err)) => {
            if cancelled {
                DownloadTransition::Finished {
                    state: UpdateUiState::Available {
                        info: Arc::clone(info),
                    },
                    last_error: None,
                }
            } else {
                DownloadTransition::Finished {
                    state: UpdateUiState::Error {
                        message: err.clone(),
                        info: Some(Arc::clone(info)),
                    },
                    last_error: Some(err),
                }
            }
        }
        None => DownloadTransition::Progress { downloaded, total },
    }
}

fn apply_download_transition_fields(
    update_state: &mut UpdateUiState,
    update_download_cancel: &mut Option<Arc<AtomicBool>>,
    update_last_error: &mut Option<String>,
    info: &Arc<UpdateCheckResult>,
    transition: DownloadTransition,
) {
    match transition {
        DownloadTransition::Finished { state, last_error } => {
            *update_download_cancel = None;
            if let Some(err) = last_error {
                *update_last_error = Some(err);
            } else {
                *update_last_error = None;
            }
            *update_state = state;
        }
        DownloadTransition::Progress { downloaded, total } => {
            if matches!(update_state, UpdateUiState::Downloading { .. }) {
                *update_state = UpdateUiState::Downloading {
                    info: Arc::clone(info),
                    downloaded,
                    total,
                };
            }
        }
    }
}

fn apply_download_progress_or_result(
    app: &mut Ruum,
    info: &Arc<UpdateCheckResult>,
    downloaded: u64,
    total: u64,
    cancelled: bool,
    done: Option<Result<PathBuf, String>>,
    cx: &mut Context<'_, Ruum>,
) {
    let transition =
        classify_download_update(info, downloaded, total, cancelled, done);
    apply_download_transition_fields(
        &mut app.update_state,
        &mut app.update_download_cancel,
        &mut app.update_last_error,
        info,
        transition,
    );
    app.sync_update_runtime(cx);
    cx.notify();
}

fn apply_download_to_runtime(
    info: &Arc<UpdateCheckResult>,
    downloaded: u64,
    total: u64,
    cancelled: bool,
    done: Option<Result<PathBuf, String>>,
    cx: &mut App,
) {
    let transition =
        classify_download_update(info, downloaded, total, cancelled, done);
    crate::with_runtime_mut(cx, |rt| {
        apply_download_transition_fields(
            &mut rt.update_state,
            &mut rt.update_download_cancel,
            &mut rt.update_last_error,
            info,
            transition,
        );
    });
    crate::refresh_tray_update_status(cx);
}

/// Run local AI analysis on an already-fetched remote [`Snapshot`] using pure
/// in-memory tree + Docker whitelist validation.
#[cfg(test)]
pub fn analyze_remote_snapshot_with_runner(
    ai_cfg: &ruum_core::storage::AiConfig,
    snapshot: &ruum_core::remote::Snapshot,
    now_secs: u64,
    runner: &dyn ruum_core::ai_cli::CommandRunner,
) -> Result<CleanupProposal, String> {
    analyze_remote_snapshot_with_context(
        ai_cfg,
        snapshot,
        now_secs,
        runner,
        &ruum_core::ai_cli::AiExecutionContext::default(),
    )
}

/// Run local AI analysis on an already-fetched remote [`Snapshot`] with an
/// explicit [`ruum_core::ai_cli::AiExecutionContext`].
pub fn analyze_remote_snapshot_with_context(
    ai_cfg: &ruum_core::storage::AiConfig,
    snapshot: &ruum_core::remote::Snapshot,
    now_secs: u64,
    runner: &dyn ruum_core::ai_cli::CommandRunner,
    ctx: &ruum_core::ai_cli::AiExecutionContext,
) -> Result<CleanupProposal, String> {
    let target_os =
        ruum_core::removal::trash_os_for_platform(&snapshot.header.platform);
    ruum_core::ai_cli::run_tree_analysis_for_config_with_context(
        ai_cfg,
        &snapshot.header.root,
        snapshot.header.home.as_deref(),
        &snapshot.tree,
        &snapshot.docker,
        target_os,
        now_secs,
        runner,
        ctx,
    )
}

fn start_ssh_session_with_store(
    profile: &ruum_core::storage::SshProfile,
    store: &ruum_core::credentials::LazyCredentialStore,
    duration_minutes: u64,
) -> Result<ruum_core::ssh_session::SshSession, String> {
    let endpoint = ruum_core::ssh_session::SshEndpoint::from_profile(profile)?;
    let auth = ruum_core::ssh_session::SshAuthMaterial::from_profile_and_store(
        profile, store,
    )?;
    ruum_core::ssh_session::SshSession::start(endpoint, auth, duration_minutes)
}

fn expand_local_tilde_path(raw: &str) -> PathBuf {
    let trimmed = raw.trim();
    let Some(home) = std::env::home_dir() else {
        return PathBuf::from(trimmed);
    };
    if trimmed == "~" {
        return home;
    }
    if let Some(rest) = trimmed.strip_prefix("~/") {
        return if rest.is_empty() {
            home
        } else {
            home.join(rest)
        };
    }
    if cfg!(windows)
        && let Some(rest) = trimmed.strip_prefix("~\\")
    {
        return if rest.is_empty() {
            home
        } else {
            home.join(rest)
        };
    }
    PathBuf::from(trimmed)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn view() -> View {
        View {
            scale: 2.0,
            origin_x: 100.0,
            origin_y: 50.0,
        }
    }

    #[test]
    fn projecting_and_unprojecting_are_inverses() {
        let view = view();
        let (x, y) = view.unproject(400.0, 300.0);
        let rect = Rect::new(x, y, 10.0, 10.0);
        let screen = view.project(rect);
        assert!((screen.x - 400.0).abs() < 0.001);
        assert!((screen.y - 300.0).abs() < 0.001);
        assert!((screen.w - 20.0).abs() < 0.001);
    }

    #[test]
    fn zooming_keeps_the_point_under_the_cursor_still() {
        let view = View::IDENTITY;
        let (before_x, before_y) = view.unproject(300.0, 200.0);
        let zoomed = view.zoomed_at(300.0, 200.0, 2.0, View::MAX_SCALE);
        let rect = Rect::new(before_x, before_y, 1.0, 1.0);
        let screen = zoomed.project(rect);
        assert!((screen.x - 300.0).abs() < 0.01, "{screen:?}");
        assert!((screen.y - 200.0).abs() < 0.01, "{screen:?}");
    }

    #[test]
    fn clamping_never_leaves_a_margin() {
        let area = size(px(800.), px(600.));
        let clamped = View {
            scale: 2.0,
            origin_x: -500.0,
            origin_y: 900.0,
        }
        .clamped(area);
        assert!(clamped.origin_x.abs() < f32::EPSILON);
        assert!((clamped.origin_y - 300.0).abs() < f32::EPSILON);

        let identity = View {
            scale: 1.0,
            origin_x: 40.0,
            origin_y: 40.0,
        }
        .clamped(area);
        assert!(identity.origin_x.abs() < f32::EPSILON);
        assert!(identity.origin_y.abs() < f32::EPSILON);
    }

    #[test]
    fn a_transition_starts_where_the_region_was_and_ends_where_it_is() {
        // A region that occupied the whole viewport, landing in the top-left
        // quarter of the new layout: everything inside it must start twice as
        // large and centred where it was.
        let transition = LayoutTransition::new(
            Rect::new(0.0, 0.0, 800.0, 600.0),
            Rect::new(0.0, 0.0, 400.0, 300.0),
        );
        // A tile inside the destination, at the far corner.
        let tile = Rect::new(200.0, 150.0, 200.0, 150.0);
        let (_, running) = transition.sample(tile);
        assert!(running, "the transition is in flight");
        let origin = transition.origin_of(tile);
        assert!((origin.x - 400.0).abs() < 0.001, "{origin:?}");
        assert!((origin.y - 300.0).abs() < 0.001, "{origin:?}");
        assert!((origin.w - 400.0).abs() < 0.001, "{origin:?}");
        assert!((origin.h - 300.0).abs() < 0.001, "{origin:?}");

        let finished = LayoutTransition {
            started: Instant::now()
                .checked_sub(Duration::from_millis(500))
                .expect("a moment on a running machine"),
            ..transition
        };
        let (end, running) = finished.sample(tile);
        assert!(!running);
        assert_eq!(end, tile, "it ends exactly at the real layout");
    }

    #[test]
    fn a_transition_grows_a_tile_that_is_being_entered() {
        // Entering a child: the child's region becomes the viewport, so
        // everything inside it gets bigger, never smaller.
        let transition = LayoutTransition::new(
            Rect::new(100.0, 100.0, 200.0, 200.0),
            Rect::new(0.0, 0.0, 800.0, 600.0),
        );
        let tile = Rect::new(400.0, 300.0, 100.0, 100.0);
        let origin = transition.origin_of(tile);
        assert!(origin.w < tile.w, "it starts smaller: {origin:?}");
        assert!(
            origin.x > 100.0 && origin.x < 300.0,
            "and where the child was: {origin:?}"
        );
    }

    #[test]
    fn directions_only_see_gaps_on_their_own_side() {
        let from = Rect::new(100.0, 100.0, 50.0, 50.0);
        let right = Rect::new(200.0, 100.0, 50.0, 50.0);
        let left = Rect::new(0.0, 100.0, 50.0, 50.0);
        assert!(Direction::Right.gap(&from, &right).is_some());
        assert!(Direction::Right.gap(&from, &left).is_none());
        assert!(Direction::Left.gap(&from, &left).is_some());
        assert!(Direction::Down.gap(&from, &right).is_none());
        assert_eq!(Direction::Right.gap(&from, &right), Some(50.0));
        // Aligned neighbours have no perpendicular offset; stacked ones do.
        assert!(Direction::Right.offset(&from, &right).abs() < f32::EPSILON);
        assert!(
            Direction::Right
                .offset(&from, &Rect::new(200.0, 160.0, 20.0, 20.0))
                > 0.0
        );
    }

    #[test]
    fn live_alipay_pair_selects_alipays_for_qr_and_https_for_browser() {
        let purchase_url = "alipays://platformapi/startapp?\
            appId=2021006180624128&page=pages/order/index?\
            purchaseLinkId=abc";
        let purchase_qr_code = "https://mobilecodec.alipay.com/show.htm?code=6w711231jgnqlyjvhbr7v58";

        assert_eq!(
            select_billing_qr_payload(
                Some(purchase_url),
                Some(purchase_qr_code),
            ),
            Some(purchase_url.to_owned()),
        );
        assert_eq!(
            select_safe_billing_browser_url(
                Some(purchase_url),
                Some(purchase_qr_code),
            ),
            Some(purchase_qr_code.to_owned()),
        );
    }
}
