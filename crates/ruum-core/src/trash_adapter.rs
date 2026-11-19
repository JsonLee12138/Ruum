//! Tri-platform Trash & Recycle Bin classification and empty-in-place
//! execution.
//!
//! Mirrors [`crate::auth`]'s Factory + Adapter pattern so `removal.rs`
//! and cross-OS unit tests share one authoritative set of
//! rules for macOS, Linux (`FreeDesktop` XDG Trash), and Windows (`$Recycle.Bin`).

use std::ffi::OsStr;
use std::fs;
use std::io;
use std::path::{Component, Path, PathBuf};

use crate::removal::{
    guard_key_for, is_dir_link, remove_permanently, summarize_drain_errors,
};

/// Operating system family for trash/recycle-bin semantics.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum TrashOs {
    MacOs,
    Linux,
    Windows,
}

/// Environment context needed to locate a user's trash directories.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct TrashEnv {
    pub home: Option<PathBuf>,
    pub xdg_data_home: Option<PathBuf>,
    pub uid: Option<u32>,
}

impl TrashEnv {
    /// Capture the current process environment once per planning or worker run.
    #[must_use]
    pub fn current() -> Self {
        let home = std::env::home_dir();
        let xdg_data_home = std::env::var_os("XDG_DATA_HOME")
            .map(PathBuf::from)
            .filter(|dir| !dir.as_os_str().is_empty() && dir.is_absolute());
        #[cfg(unix)]
        let uid = Some(rustix::process::getuid().as_raw());
        #[cfg(not(unix))]
        let uid = None;
        Self {
            home,
            xdg_data_home,
            uid,
        }
    }
}

/// Guard reason when a target is already inside a trash/recycle-bin
/// container and the caller is in `RemovalMode::Trash`.
pub const ALREADY_IN_TRASH_REASON: &str =
    "already in the trash; switch to permanent deletion to empty it";

/// Guard reason when a target belongs to another user's trash directory.
pub const OTHER_USER_TRASH_REASON: &str =
    "another user's Trash directory cannot be removed";

/// Guard reason when a target is structural Recycle Bin metadata
/// (`desktop.ini`).
pub const RECYCLE_BIN_METADATA_REASON: &str =
    "part of the Recycle Bin structure; empty the Recycle Bin instead";

/// Structural subdirectories inside a `FreeDesktop` XDG Trash root.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum XdgTrashSubdir {
    Files,
    Info,
    Expunged,
}

/// Kind of trash container whose directory skeleton must be preserved when
/// emptied in-place.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum TrashContainer {
    DirKeepSelf,
    MacOsVolumeTrashesRoot,
    XdgRoot,
    LinuxVolumeTrashRoot,
    XdgSubdir(XdgTrashSubdir),
    WindowsRecycleRoot { volume_root: PathBuf },
    WindowsRecycleSidDir,
}

/// Classification of a candidate target path relative to OS trash layouts.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum TrashDisposition {
    NotTrash,
    BlockedAlways(&'static str),
    Container(TrashContainer),
    Entry { paired_metadata: Option<PathBuf> },
}

impl TrashDisposition {
    /// Whether this disposition denotes a trash container or an item inside it.
    #[must_use]
    pub const fn is_trash(&self) -> bool {
        matches!(self, Self::Container(_) | Self::Entry { .. })
    }
}

/// Adapter interface encapsulating OS-specific trash classification and
/// permanent empty-in-place execution.
pub trait TrashAdapter: Send + Sync + std::fmt::Debug {
    fn os_kind(&self) -> TrashOs;
    fn classify(
        &self,
        path: &Path,
        env: &TrashEnv,
        mounts: &[PathBuf],
    ) -> TrashDisposition;
    fn remove_permanent(
        &self,
        path: &Path,
        root: &Path,
        disposition: &TrashDisposition,
    ) -> io::Result<()>;
}

/// macOS Trash adapter (`~/.Trash` and `<mount>/.Trashes/<uid>`).
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct MacOsTrashAdapter;

impl TrashAdapter for MacOsTrashAdapter {
    fn os_kind(&self) -> TrashOs {
        TrashOs::MacOs
    }

    fn classify(
        &self,
        path: &Path,
        env: &TrashEnv,
        mounts: &[PathBuf],
    ) -> TrashDisposition {
        let key = guard_key_for(path, true);
        if let Some(home) = env.home.as_deref() {
            let home_trash = guard_key_for(&home.join(".Trash"), true);
            if key == home_trash {
                return TrashDisposition::Container(
                    TrashContainer::DirKeepSelf,
                );
            }
            if key.starts_with(&home_trash) {
                return TrashDisposition::Entry {
                    paired_metadata: None,
                };
            }
        }

        for mount in mounts {
            let mount_key = guard_key_for(mount, true);
            let trashes = mount_key.join(".trashes");
            if key == trashes {
                return TrashDisposition::Container(
                    TrashContainer::MacOsVolumeTrashesRoot,
                );
            }
            let Ok(rel) = key.strip_prefix(&trashes) else {
                continue;
            };
            let mut comps = rel.components();
            let Some(Component::Normal(first)) = comps.next() else {
                continue;
            };
            if is_ascii_uid(first) {
                if matches_uid(first, env.uid) {
                    return if comps.next().is_none() {
                        TrashDisposition::Container(TrashContainer::DirKeepSelf)
                    } else {
                        TrashDisposition::Entry {
                            paired_metadata: None,
                        }
                    };
                }
                return TrashDisposition::BlockedAlways(
                    OTHER_USER_TRASH_REASON,
                );
            }
        }
        TrashDisposition::NotTrash
    }

    fn remove_permanent(
        &self,
        path: &Path,
        root: &Path,
        disposition: &TrashDisposition,
    ) -> io::Result<()> {
        match disposition {
            TrashDisposition::NotTrash | TrashDisposition::Entry { .. } => {
                remove_permanently(path, root)
            }
            TrashDisposition::BlockedAlways(reason) => {
                Err(io::Error::other(*reason))
            }
            TrashDisposition::Container(TrashContainer::DirKeepSelf) => {
                unix_empty_dir_keep_self(path, root)
            }
            TrashDisposition::Container(
                TrashContainer::MacOsVolumeTrashesRoot,
            ) => unix_empty_trashes_root(path, root),
            TrashDisposition::Container(_) => Err(io::Error::new(
                io::ErrorKind::InvalidInput,
                "unsupported trash container for macOS adapter",
            )),
        }
    }
}

/// Linux `FreeDesktop` XDG Trash adapter.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct LinuxTrashAdapter;

impl TrashAdapter for LinuxTrashAdapter {
    fn os_kind(&self) -> TrashOs {
        TrashOs::Linux
    }

    fn classify(
        &self,
        path: &Path,
        env: &TrashEnv,
        mounts: &[PathBuf],
    ) -> TrashDisposition {
        let key = guard_key_for(path, false);

        if let Some(xdg_root) = linux_home_trash_root(env)
            && let Some(disp) = classify_xdg_root(&key, &xdg_root)
        {
            return disp;
        }

        for mount in mounts {
            let mount_key = guard_key_for(mount, false);
            let vol_trash = mount_key.join(".Trash");
            if key == vol_trash {
                return TrashDisposition::Container(
                    TrashContainer::LinuxVolumeTrashRoot,
                );
            }
            if let Ok(rel) = key.strip_prefix(&vol_trash)
                && let Some(Component::Normal(first)) = rel.components().next()
                && is_ascii_uid(first)
            {
                if matches_uid(first, env.uid) {
                    let xdg_root = vol_trash.join(first);
                    if let Some(disp) = classify_xdg_root(&key, &xdg_root) {
                        return disp;
                    }
                } else {
                    return TrashDisposition::BlockedAlways(
                        OTHER_USER_TRASH_REASON,
                    );
                }
            }

            if let Ok(rel) = key.strip_prefix(&mount_key)
                && let Some(Component::Normal(first)) = rel.components().next()
                && let Some(uid_str) = first
                    .to_str()
                    .and_then(|name| name.strip_prefix(".Trash-"))
                    .filter(|uid| {
                        !uid.is_empty()
                            && uid.bytes().all(|b| b.is_ascii_digit())
                    })
            {
                if env.uid.is_some_and(|u| uid_str == u.to_string()) {
                    let xdg_root = mount_key.join(first);
                    if let Some(disp) = classify_xdg_root(&key, &xdg_root) {
                        return disp;
                    }
                } else {
                    return TrashDisposition::BlockedAlways(
                        OTHER_USER_TRASH_REASON,
                    );
                }
            }
        }

        TrashDisposition::NotTrash
    }

    fn remove_permanent(
        &self,
        path: &Path,
        root: &Path,
        disposition: &TrashDisposition,
    ) -> io::Result<()> {
        match disposition {
            TrashDisposition::NotTrash => remove_permanently(path, root),
            TrashDisposition::BlockedAlways(reason) => {
                Err(io::Error::other(*reason))
            }
            TrashDisposition::Entry { paired_metadata } => {
                remove_permanently(path, root)?;
                if let Some(info_path) = paired_metadata {
                    unix_remove_paired_trashinfo(info_path)?;
                }
                Ok(())
            }
            TrashDisposition::Container(TrashContainer::XdgRoot) => {
                unix_empty_xdg_root(path, root)
            }
            TrashDisposition::Container(
                TrashContainer::LinuxVolumeTrashRoot,
            ) => unix_empty_linux_volume_trash_root(path, root),
            TrashDisposition::Container(TrashContainer::XdgSubdir(subdir)) => {
                unix_empty_xdg_subdir(path, root, *subdir)
            }
            TrashDisposition::Container(_) => Err(io::Error::new(
                io::ErrorKind::InvalidInput,
                "unsupported trash container for Linux adapter",
            )),
        }
    }
}

/// Windows `$Recycle.Bin` adapter.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct WindowsTrashAdapter;

impl TrashAdapter for WindowsTrashAdapter {
    fn os_kind(&self) -> TrashOs {
        TrashOs::Windows
    }

    fn classify(
        &self,
        path: &Path,
        _env: &TrashEnv,
        mounts: &[PathBuf],
    ) -> TrashDisposition {
        let Some(wp) = parse_win_path(path) else {
            return TrashDisposition::NotTrash;
        };

        // Any `<drive>:\` or `\\server\share\` is always a candidate volume
        // root even when `mounts` is empty or does not list a secondary drive.
        let drive_fallback =
            (wp.root_display != "/").then(|| WinVolumeCandidate {
                root_folded: wp.root_folded.clone(),
                prefix: Vec::new(),
                volume_root: wp.root_display.clone(),
            });

        // Longest prefix match wins so a volume mounted inside a folder beats
        // the enclosing drive root.
        let best = mounts
            .iter()
            .filter_map(|mount| {
                let parsed = parse_win_path(mount)?;
                let volume_root = parsed.to_volume_root_string();
                let prefix = parsed
                    .segments
                    .iter()
                    .map(|seg| seg.to_lowercase())
                    .collect();
                Some(WinVolumeCandidate {
                    root_folded: parsed.root_folded,
                    prefix,
                    volume_root,
                })
            })
            .chain(drive_fallback)
            .filter(|cand| {
                cand.root_folded == wp.root_folded
                    && wp.segments.get(cand.prefix.len()).is_some_and(|seg| {
                        seg.eq_ignore_ascii_case("$Recycle.Bin")
                    })
                    && wp.segments[..cand.prefix.len()]
                        .iter()
                        .zip(&cand.prefix)
                        .all(|(left, right)| left.to_lowercase() == *right)
            })
            .max_by_key(|cand| cand.prefix.len());

        let Some(cand) = best else {
            return TrashDisposition::NotTrash;
        };

        let bin_idx = cand.prefix.len();
        let after_bin = &wp.segments[bin_idx + 1..];
        match after_bin {
            [] => TrashDisposition::Container(
                TrashContainer::WindowsRecycleRoot {
                    volume_root: PathBuf::from(cand.volume_root),
                },
            ),
            [child] => {
                if child.eq_ignore_ascii_case("desktop.ini") {
                    TrashDisposition::BlockedAlways(RECYCLE_BIN_METADATA_REASON)
                } else {
                    TrashDisposition::Container(
                        TrashContainer::WindowsRecycleSidDir,
                    )
                }
            }
            [sid, item] => {
                if sid.eq_ignore_ascii_case("desktop.ini")
                    || item.eq_ignore_ascii_case("desktop.ini")
                {
                    return TrashDisposition::BlockedAlways(
                        RECYCLE_BIN_METADATA_REASON,
                    );
                }
                let paired_metadata = strip_prefix_ascii_ci(item, "$R")
                    .filter(|id| !id.is_empty())
                    .map(|id| {
                        let sid_prefix = wp.join_segments(bin_idx + 2);
                        let sep = wp.sep;
                        PathBuf::from(format!("{sid_prefix}{sep}$I{id}"))
                    });
                TrashDisposition::Entry { paired_metadata }
            }
            [sid, ..] => {
                if sid.eq_ignore_ascii_case("desktop.ini") {
                    TrashDisposition::BlockedAlways(RECYCLE_BIN_METADATA_REASON)
                } else {
                    TrashDisposition::Entry {
                        paired_metadata: None,
                    }
                }
            }
        }
    }

    fn remove_permanent(
        &self,
        path: &Path,
        root: &Path,
        disposition: &TrashDisposition,
    ) -> io::Result<()> {
        match disposition {
            TrashDisposition::NotTrash => remove_permanently(path, root),
            TrashDisposition::BlockedAlways(reason) => {
                Err(io::Error::other(*reason))
            }
            TrashDisposition::Entry { paired_metadata } => {
                remove_permanently(path, root)?;
                if let Some(info_path) = paired_metadata {
                    remove_windows_paired_metadata(info_path)?;
                }
                Ok(())
            }
            TrashDisposition::Container(
                TrashContainer::WindowsRecycleRoot { volume_root },
            ) => empty_windows_recycle_root(path, volume_root),
            TrashDisposition::Container(
                TrashContainer::WindowsRecycleSidDir,
            ) => {
                ensure_real_win_dir(path)?;
                drain_windows_sid_dir(path)
            }
            TrashDisposition::Container(_) => Err(io::Error::new(
                io::ErrorKind::InvalidInput,
                "unsupported trash container for Windows adapter",
            )),
        }
    }
}

/// Sanitize a Windows volume root for `SHEmptyRecycleBinW`: strips `\\?\`,
/// normalizes separators to `\`, ensures a trailing `\`, and rejects empty
/// or rootless values that would empty every drive on the machine.
pub(crate) fn sanitize_windows_volume_root(path: &Path) -> io::Result<String> {
    let parsed = parse_win_path(path).ok_or_else(|| {
        io::Error::new(
            io::ErrorKind::InvalidInput,
            "volume root must be a valid drive or volume path",
        )
    })?;
    if parsed.root_display == "/" {
        return Err(io::Error::new(
            io::ErrorKind::InvalidInput,
            "Windows volume root cannot be a POSIX root",
        ));
    }
    let clean = parsed.to_volume_root_string();
    if clean.is_empty() || !clean.ends_with('\\') {
        return Err(io::Error::new(
            io::ErrorKind::InvalidInput,
            "volume root cannot be empty",
        ));
    }
    Ok(clean)
}

/// Factory that constructs the appropriate [`TrashAdapter`].
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct TrashAdapterFactory;

impl TrashAdapterFactory {
    #[must_use]
    pub const fn current_os() -> TrashOs {
        if cfg!(target_os = "macos") {
            TrashOs::MacOs
        } else if cfg!(windows) {
            TrashOs::Windows
        } else {
            TrashOs::Linux
        }
    }

    #[must_use]
    pub fn create_for(os: TrashOs) -> Box<dyn TrashAdapter> {
        match os {
            TrashOs::MacOs => Box::new(MacOsTrashAdapter),
            TrashOs::Linux => Box::new(LinuxTrashAdapter),
            TrashOs::Windows => Box::new(WindowsTrashAdapter),
        }
    }

    #[must_use]
    pub fn create() -> Box<dyn TrashAdapter> {
        Self::create_for(Self::current_os())
    }
}

fn is_ascii_uid(part: &OsStr) -> bool {
    part.to_str()
        .is_some_and(|s| !s.is_empty() && s.bytes().all(|b| b.is_ascii_digit()))
}

fn matches_uid(part: &OsStr, uid: Option<u32>) -> bool {
    uid.is_some_and(|id| part.to_str() == Some(id.to_string().as_str()))
}

fn linux_home_trash_root(env: &TrashEnv) -> Option<PathBuf> {
    let valid_xdg = env.xdg_data_home.as_deref().filter(|dir| {
        !dir.as_os_str().is_empty()
            && matches!(dir.components().next(), Some(Component::RootDir))
    });
    let data_home = match valid_xdg {
        Some(dir) => guard_key_for(dir, false),
        None => guard_key_for(env.home.as_deref()?, false)
            .join(".local")
            .join("share"),
    };
    Some(data_home.join("Trash"))
}

fn classify_xdg_root(key: &Path, xdg_root: &Path) -> Option<TrashDisposition> {
    if key == xdg_root {
        return Some(TrashDisposition::Container(TrashContainer::XdgRoot));
    }
    let rel = key.strip_prefix(xdg_root).ok()?;
    let mut comps = rel.components();
    let Some(Component::Normal(first)) = comps.next() else {
        return None;
    };
    let second = comps.next();
    let third = comps.next();
    if first == OsStr::new("files") {
        return match (second, third) {
            (None, _) => Some(TrashDisposition::Container(
                TrashContainer::XdgSubdir(XdgTrashSubdir::Files),
            )),
            (Some(Component::Normal(item)), None) => {
                let mut info_name = item.to_os_string();
                info_name.push(".trashinfo");
                Some(TrashDisposition::Entry {
                    paired_metadata: Some(
                        xdg_root.join("info").join(info_name),
                    ),
                })
            }
            _ => Some(TrashDisposition::Entry {
                paired_metadata: None,
            }),
        };
    }
    if first == OsStr::new("info") && second.is_none() {
        return Some(TrashDisposition::Container(TrashContainer::XdgSubdir(
            XdgTrashSubdir::Info,
        )));
    }
    if first == OsStr::new("expunged") && second.is_none() {
        return Some(TrashDisposition::Container(TrashContainer::XdgSubdir(
            XdgTrashSubdir::Expunged,
        )));
    }
    Some(TrashDisposition::Entry {
        paired_metadata: None,
    })
}

fn strip_prefix_ascii_ci<'a>(value: &'a str, prefix: &str) -> Option<&'a str> {
    value
        .get(..prefix.len())
        .filter(|head| head.eq_ignore_ascii_case(prefix))
        .map(|_| &value[prefix.len()..])
}

#[derive(Clone, Debug)]
struct WinVolumeCandidate {
    root_folded: String,
    prefix: Vec<String>,
    volume_root: String,
}

#[derive(Clone, Debug)]
struct WinPath {
    root_display: String,
    root_folded: String,
    segments: Vec<String>,
    sep: char,
}

impl WinPath {
    fn to_volume_root_string(&self) -> String {
        if self.segments.is_empty() {
            self.root_display.clone()
        } else {
            format!(
                "{}{}{}",
                self.root_display,
                self.segments.join(&self.sep.to_string()),
                self.sep
            )
        }
    }

    fn join_segments(&self, count: usize) -> String {
        let mut out = self.root_display.clone();
        for (idx, seg) in self.segments.iter().take(count).enumerate() {
            if idx > 0 {
                out.push(self.sep);
            }
            out.push_str(seg);
        }
        out
    }
}

fn parse_win_path(path: &Path) -> Option<WinPath> {
    let raw = path.to_string_lossy();
    let rest = if strip_prefix_ascii_ci(&raw, r"\\?\UNC\").is_some()
        || strip_prefix_ascii_ci(&raw, "//?/UNC/").is_some()
    {
        return parse_unc_tail(&raw[8..]);
    } else if raw.starts_with(r"\\?\") || raw.starts_with("//?/") {
        &raw[4..]
    } else {
        raw.as_ref()
    };

    if let Some(tail) =
        rest.strip_prefix(r"\\").or_else(|| rest.strip_prefix("//"))
    {
        return parse_unc_tail(tail);
    }

    let bytes = rest.as_bytes();
    if bytes.len() >= 2
        && bytes[0].is_ascii_alphabetic()
        && bytes[1] == b':'
        && (bytes.len() == 2 || bytes[2] == b'\\' || bytes[2] == b'/')
    {
        let letter = bytes[0].to_ascii_uppercase() as char;
        let root_display = format!("{letter}:\\");
        let root_folded = format!("{}:\\", letter.to_ascii_lowercase());
        let segments = normalize_win_segments(&rest[2..]);
        return Some(WinPath {
            root_display,
            root_folded,
            segments,
            sep: '\\',
        });
    }

    if let Some(tail) = rest.strip_prefix('/') {
        let segments = normalize_win_segments(tail);
        return Some(WinPath {
            root_display: "/".to_owned(),
            root_folded: "/".to_owned(),
            segments,
            sep: '/',
        });
    }

    None
}

fn parse_unc_tail(tail: &str) -> Option<WinPath> {
    let mut parts = tail
        .split(['\\', '/'])
        .filter(|part| !part.is_empty() && *part != ".");
    let server = parts.next()?;
    let share = parts.next()?;
    let root_display = format!("\\\\{server}\\{share}\\");
    let root_folded = root_display.to_lowercase();
    let mut segments = Vec::new();
    for part in parts {
        if part == ".." {
            segments.pop();
        } else {
            segments.push(part.to_owned());
        }
    }
    Some(WinPath {
        root_display,
        root_folded,
        segments,
        sep: '\\',
    })
}

fn normalize_win_segments(tail: &str) -> Vec<String> {
    let mut segments = Vec::new();
    for part in tail.split(['\\', '/']) {
        match part {
            "" | "." => {}
            ".." => {
                segments.pop();
            }
            other => segments.push(other.to_owned()),
        }
    }
    segments
}

#[cfg(unix)]
fn unix_empty_dir_keep_self(path: &Path, root: &Path) -> io::Result<()> {
    let (parent, name) = crate::removal::open_parent(path, root)?;
    let dir = crate::removal::open_dir(&parent, name)?;
    let vol = crate::removal::Volume::of(&dir)?;
    crate::removal::drain_dir_best_effort(&dir, &vol, path)
}

#[cfg(not(unix))]
fn unix_empty_dir_keep_self(_path: &Path, _root: &Path) -> io::Result<()> {
    Err(io::Error::new(
        io::ErrorKind::Unsupported,
        "Unix trash container emptying requires a Unix host",
    ))
}

#[cfg(unix)]
fn unix_empty_trashes_root(path: &Path, root: &Path) -> io::Result<()> {
    use rustix::io::Errno;

    let (parent, name) = crate::removal::open_parent(path, root)?;
    let trashes = crate::removal::open_dir(&parent, name)?;
    let vol = crate::removal::Volume::of(&trashes)?;
    let uid_str = rustix::process::getuid().as_raw().to_string();
    let uid_dir = match crate::removal::open_dir(&trashes, &uid_str) {
        Ok(dir) => dir,
        Err(Errno::NOENT) => return Ok(()),
        Err(err) => return Err(err.into()),
    };
    if !vol.contains(&crate::removal::Volume::of(&uid_dir)?) {
        return Err(io::Error::other(format!(
            "stopped at {}: another filesystem is mounted there, and nothing \
             on it was touched",
            path.join(&uid_str).display()
        )));
    }
    crate::removal::drain_dir_best_effort(&uid_dir, &vol, &path.join(&uid_str))
}

#[cfg(not(unix))]
fn unix_empty_trashes_root(_path: &Path, _root: &Path) -> io::Result<()> {
    Err(io::Error::new(
        io::ErrorKind::Unsupported,
        "macOS .Trashes emptying requires a Unix host",
    ))
}

#[cfg(unix)]
fn unix_empty_xdg_root(path: &Path, root: &Path) -> io::Result<()> {
    let (parent, name) = crate::removal::open_parent(path, root)?;
    let xdg_dir = crate::removal::open_dir(&parent, name)?;
    let xdg_vol = crate::removal::Volume::of(&xdg_dir)?;
    drain_xdg_root_fd(&xdg_dir, &xdg_vol, path)
}

#[cfg(not(unix))]
fn unix_empty_xdg_root(_path: &Path, _root: &Path) -> io::Result<()> {
    Err(io::Error::new(
        io::ErrorKind::Unsupported,
        "Linux XDG trash emptying requires a Unix host",
    ))
}

#[cfg(unix)]
fn unix_empty_linux_volume_trash_root(
    path: &Path,
    root: &Path,
) -> io::Result<()> {
    use rustix::io::Errno;

    let (parent, name) = crate::removal::open_parent(path, root)?;
    let trash_root = crate::removal::open_dir(&parent, name)?;
    let vol = crate::removal::Volume::of(&trash_root)?;
    let uid_str = rustix::process::getuid().as_raw().to_string();
    let uid_dir = match crate::removal::open_dir(&trash_root, &uid_str) {
        Ok(dir) => dir,
        Err(Errno::NOENT) => return Ok(()),
        Err(err) => return Err(err.into()),
    };
    if !vol.contains(&crate::removal::Volume::of(&uid_dir)?) {
        return Err(io::Error::other(format!(
            "stopped at {}: another filesystem is mounted there, and nothing \
             on it was touched",
            path.join(&uid_str).display()
        )));
    }
    drain_xdg_root_fd(&uid_dir, &vol, &path.join(&uid_str))
}

#[cfg(not(unix))]
fn unix_empty_linux_volume_trash_root(
    _path: &Path,
    _root: &Path,
) -> io::Result<()> {
    Err(io::Error::new(
        io::ErrorKind::Unsupported,
        "Linux volume .Trash emptying requires a Unix host",
    ))
}

#[cfg(unix)]
fn drain_xdg_root_fd(
    xdg_dir: &std::os::fd::OwnedFd,
    xdg_vol: &crate::removal::Volume,
    xdg_path: &Path,
) -> io::Result<()> {
    use rustix::io::Errno;

    let mut failures = 0_usize;
    let mut first_error: Option<io::Error> = None;

    for sub in ["files", "info", "expunged"] {
        let sub_path = xdg_path.join(sub);
        let sub_fd = match crate::removal::open_dir(xdg_dir, sub) {
            Ok(fd) => fd,
            Err(Errno::NOENT) => continue,
            Err(Errno::LOOP) => {
                failures += 1;
                if first_error.is_none() {
                    first_error = Some(io::Error::other(format!(
                        "refused to follow symlink at {}",
                        sub_path.display()
                    )));
                }
                continue;
            }
            Err(err) => {
                failures += 1;
                if first_error.is_none() {
                    first_error = Some(err.into());
                }
                continue;
            }
        };
        let sub_vol = match crate::removal::Volume::of(&sub_fd) {
            Ok(vol) => vol,
            Err(err) => {
                failures += 1;
                if first_error.is_none() {
                    first_error = Some(err);
                }
                continue;
            }
        };
        if !xdg_vol.contains(&sub_vol) {
            failures += 1;
            if first_error.is_none() {
                first_error = Some(io::Error::other(format!(
                    "stopped at {}: another filesystem is mounted there, and \
                     nothing on it was touched",
                    sub_path.display()
                )));
            }
            continue;
        }
        if let Err(err) =
            crate::removal::drain_dir_best_effort(&sub_fd, xdg_vol, &sub_path)
        {
            failures += 1;
            if first_error.is_none() {
                first_error = Some(err);
            }
        }
    }

    if failures == 0 {
        Ok(())
    } else {
        Err(summarize_drain_errors(failures, first_error))
    }
}

#[cfg(unix)]
fn unix_empty_xdg_subdir(
    path: &Path,
    root: &Path,
    subdir: XdgTrashSubdir,
) -> io::Result<()> {
    use rustix::fs::{AtFlags, FileType, statat};
    use rustix::io::Errno;

    let (parent, name) = crate::removal::open_parent(path, root)?;
    let sub_fd = crate::removal::open_dir(&parent, name)?;
    let sub_vol = crate::removal::Volume::of(&sub_fd)?;

    if subdir != XdgTrashSubdir::Files {
        return crate::removal::drain_dir_best_effort(&sub_fd, &sub_vol, path);
    }

    let info_fd = match statat(&parent, "info", AtFlags::SYMLINK_NOFOLLOW) {
        Ok(stat) => {
            let kind = FileType::from_raw_mode(stat.st_mode);
            if kind == FileType::Symlink {
                return Err(io::Error::other(
                    "refused to follow symlink at XDG info directory",
                ));
            }
            if kind != FileType::Directory {
                return Err(io::Error::other(
                    "XDG info path is not a directory",
                ));
            }
            match crate::removal::open_dir(&parent, "info") {
                Ok(fd) => {
                    if !sub_vol.contains(&crate::removal::Volume::of(&fd)?) {
                        return Err(io::Error::other(
                            "stopped at info: another filesystem is mounted \
                             there, and nothing on it was touched",
                        ));
                    }
                    Some(fd)
                }
                Err(Errno::NOENT) => None,
                Err(Errno::LOOP | Errno::NOTDIR) => {
                    return Err(io::Error::other(
                        "refused to follow symlink at XDG info directory",
                    ));
                }
                Err(err) => return Err(err.into()),
            }
        }
        Err(Errno::NOENT) => None,
        Err(err) => return Err(err.into()),
    };

    crate::removal::drain_xdg_files_best_effort(
        &sub_fd,
        &sub_vol,
        path,
        info_fd.as_ref(),
    )
}

#[cfg(not(unix))]
fn unix_empty_xdg_subdir(
    _path: &Path,
    _root: &Path,
    _subdir: XdgTrashSubdir,
) -> io::Result<()> {
    Err(io::Error::new(
        io::ErrorKind::Unsupported,
        "Linux XDG subdir emptying requires a Unix host",
    ))
}

#[cfg(unix)]
fn unix_remove_paired_trashinfo(info_path: &Path) -> io::Result<()> {
    crate::removal::remove_paired_trashinfo(info_path)
}

#[cfg(not(unix))]
#[allow(clippy::unnecessary_wraps, reason = "matches unix signature")]
const fn unix_remove_paired_trashinfo(_info_path: &Path) -> io::Result<()> {
    Ok(())
}

fn ensure_real_win_dir(path: &Path) -> io::Result<()> {
    let meta = fs::symlink_metadata(path)?;
    if meta.file_type().is_symlink() || is_dir_link(&meta) || !meta.is_dir() {
        return Err(io::Error::other(format!(
            "refused to empty non-directory or link at {}",
            path.display()
        )));
    }
    #[cfg(windows)]
    if crate::windows::is_mount_point(path) {
        return Err(io::Error::other(format!(
            "stopped at {}: another volume is mounted there",
            path.display()
        )));
    }
    Ok(())
}

fn remove_windows_paired_metadata(info_path: &Path) -> io::Result<()> {
    let meta = match fs::symlink_metadata(info_path) {
        Ok(meta) => meta,
        Err(err) if err.kind() == io::ErrorKind::NotFound => return Ok(()),
        Err(err) => return Err(err),
    };
    if meta.file_type().is_symlink() || is_dir_link(&meta) || !meta.is_file() {
        return Err(io::Error::other(format!(
            "refused to remove non-file or link at {}",
            info_path.display()
        )));
    }
    match fs::remove_file(info_path) {
        Ok(()) => Ok(()),
        Err(err) if err.kind() == io::ErrorKind::NotFound => Ok(()),
        Err(err) => Err(err),
    }
}

fn empty_windows_recycle_root(
    recycle_bin_path: &Path,
    volume_root: &Path,
) -> io::Result<()> {
    ensure_real_win_dir(recycle_bin_path)?;

    #[cfg(windows)]
    crate::windows::empty_recycle_bin(volume_root)?;
    #[cfg(not(windows))]
    let _ = sanitize_windows_volume_root(volume_root)?;

    let entries = match fs::read_dir(recycle_bin_path) {
        Ok(entries) => entries,
        Err(err) if err.kind() == io::ErrorKind::NotFound => return Ok(()),
        Err(err) => return Err(err),
    };

    let mut failures = 0_usize;
    let mut first_error: Option<io::Error> = None;
    for entry in entries {
        let entry = match entry {
            Ok(entry) => entry,
            Err(err) => {
                if err.kind() != io::ErrorKind::PermissionDenied {
                    failures += 1;
                    if first_error.is_none() {
                        first_error = Some(err);
                    }
                }
                continue;
            }
        };
        let name = entry.file_name();
        let name_lossy = name.to_string_lossy();
        if strip_prefix_ascii_ci(&name_lossy, "S-1-").is_none() {
            continue;
        }
        let sid_path = entry.path();
        let meta = match fs::symlink_metadata(&sid_path) {
            Ok(meta) => meta,
            Err(err)
                if matches!(
                    err.kind(),
                    io::ErrorKind::PermissionDenied | io::ErrorKind::NotFound
                ) =>
            {
                continue;
            }
            Err(err) => {
                failures += 1;
                if first_error.is_none() {
                    first_error = Some(err);
                }
                continue;
            }
        };
        if meta.file_type().is_symlink() || is_dir_link(&meta) || !meta.is_dir()
        {
            continue;
        }
        #[cfg(windows)]
        if crate::windows::is_mount_point(&sid_path) {
            continue;
        }
        let sid_entries = match fs::read_dir(&sid_path) {
            Ok(entries) => entries,
            Err(err)
                if matches!(
                    err.kind(),
                    io::ErrorKind::PermissionDenied | io::ErrorKind::NotFound
                ) =>
            {
                continue;
            }
            Err(err) => {
                failures += 1;
                if first_error.is_none() {
                    first_error = Some(err);
                }
                continue;
            }
        };
        if let Err(err) = drain_windows_sid_entries(sid_entries) {
            failures += 1;
            if first_error.is_none() {
                first_error = Some(err);
            }
        }
    }

    if failures == 0 {
        Ok(())
    } else {
        Err(summarize_drain_errors(failures, first_error))
    }
}

fn drain_windows_sid_dir(sid_dir: &Path) -> io::Result<()> {
    drain_windows_sid_entries(fs::read_dir(sid_dir)?)
}

fn drain_windows_sid_entries(entries: fs::ReadDir) -> io::Result<()> {
    let mut failures = 0_usize;
    let mut first_error: Option<io::Error> = None;

    for entry in entries {
        let entry = match entry {
            Ok(entry) => entry,
            Err(err) => {
                failures += 1;
                if first_error.is_none() {
                    first_error = Some(err);
                }
                continue;
            }
        };
        if entry
            .file_name()
            .to_string_lossy()
            .eq_ignore_ascii_case("desktop.ini")
        {
            continue;
        }
        if let Err(err) = remove_win_entry_best_effort(&entry.path()) {
            failures += 1;
            if first_error.is_none() {
                first_error = Some(err);
            }
        }
    }

    if failures == 0 {
        Ok(())
    } else {
        Err(summarize_drain_errors(failures, first_error))
    }
}

fn remove_win_entry_best_effort(path: &Path) -> io::Result<()> {
    let meta = fs::symlink_metadata(path)?;
    if meta.file_type().is_symlink() {
        if is_dir_link(&meta) {
            return fs::remove_dir(path);
        }
        return fs::remove_file(path);
    }
    if is_dir_link(&meta) {
        return fs::remove_dir(path);
    }
    if !meta.is_dir() {
        return fs::remove_file(path);
    }
    #[cfg(windows)]
    if crate::windows::is_mount_point(path) {
        return Err(io::Error::other(format!(
            "stopped at {}: another volume is mounted there",
            path.display()
        )));
    }
    let mut failures = 0_usize;
    let mut first_error: Option<io::Error> = None;
    for entry in fs::read_dir(path)? {
        match entry {
            Ok(child) => {
                if let Err(err) = remove_win_entry_best_effort(&child.path()) {
                    failures += 1;
                    if first_error.is_none() {
                        first_error = Some(err);
                    }
                }
            }
            Err(err) => {
                failures += 1;
                if first_error.is_none() {
                    first_error = Some(err);
                }
            }
        }
    }
    if failures > 0 {
        return Err(summarize_drain_errors(failures, first_error));
    }
    fs::remove_dir(path)
}

#[cfg(test)]
mod tests {
    use tempfile::TempDir;

    use super::*;

    #[test]
    fn factory_builds_adapter_for_each_os() {
        let mac = TrashAdapterFactory::create_for(TrashOs::MacOs);
        assert_eq!(mac.os_kind(), TrashOs::MacOs);

        let linux = TrashAdapterFactory::create_for(TrashOs::Linux);
        assert_eq!(linux.os_kind(), TrashOs::Linux);

        let windows = TrashAdapterFactory::create_for(TrashOs::Windows);
        assert_eq!(windows.os_kind(), TrashOs::Windows);

        let host = TrashAdapterFactory::create();
        assert_eq!(host.os_kind(), TrashAdapterFactory::current_os());
    }

    #[test]
    fn macos_classifies_home_trash_data_alias_and_volume_trashes() {
        let adapter = TrashAdapterFactory::create_for(TrashOs::MacOs);
        let env = TrashEnv {
            home: Some(PathBuf::from("/Users/tobi")),
            xdg_data_home: None,
            uid: Some(501),
        };
        let mounts = [PathBuf::from("/"), PathBuf::from("/Volumes/Backup")];

        assert_eq!(
            adapter.classify(Path::new("/Users/tobi/.Trash"), &env, &mounts),
            TrashDisposition::Container(TrashContainer::DirKeepSelf)
        );
        assert_eq!(
            adapter.classify(
                Path::new("/System/Volumes/Data/Users/tobi/.trash"),
                &env,
                &mounts
            ),
            TrashDisposition::Container(TrashContainer::DirKeepSelf)
        );
        assert_eq!(
            adapter.classify(
                Path::new("/Users/tobi/.Trash/old.zip"),
                &env,
                &mounts
            ),
            TrashDisposition::Entry {
                paired_metadata: None
            }
        );
        assert_eq!(
            adapter.classify(
                Path::new("/Volumes/Backup/.Trashes"),
                &env,
                &mounts
            ),
            TrashDisposition::Container(TrashContainer::MacOsVolumeTrashesRoot)
        );
        assert_eq!(
            adapter.classify(
                Path::new("/Volumes/Backup/.Trashes/501"),
                &env,
                &mounts
            ),
            TrashDisposition::Container(TrashContainer::DirKeepSelf)
        );
        assert_eq!(
            adapter.classify(
                Path::new("/Volumes/Backup/.Trashes/501/app.dmg"),
                &env,
                &mounts
            ),
            TrashDisposition::Entry {
                paired_metadata: None
            }
        );
        assert_eq!(
            adapter.classify(
                Path::new("/Volumes/Backup/.Trashes/502"),
                &env,
                &mounts
            ),
            TrashDisposition::BlockedAlways(OTHER_USER_TRASH_REASON)
        );
        assert_eq!(
            adapter.classify(
                Path::new("/Volumes/Backup/.Trashes/502/secret"),
                &env,
                &mounts
            ),
            TrashDisposition::BlockedAlways(OTHER_USER_TRASH_REASON)
        );
        assert_eq!(
            adapter.classify(Path::new("/Users/tobi/Documents"), &env, &mounts),
            TrashDisposition::NotTrash
        );
    }

    #[test]
    fn linux_classifies_xdg_roots_subdirs_paired_trashinfo_and_mounts() {
        let adapter = TrashAdapterFactory::create_for(TrashOs::Linux);
        let default_env = TrashEnv {
            home: Some(PathBuf::from("/home/tobi")),
            xdg_data_home: None,
            uid: Some(1000),
        };
        let mounts = [PathBuf::from("/"), PathBuf::from("/mnt/usb")];

        assert_eq!(
            adapter.classify(
                Path::new("/home/tobi/.local/share/Trash"),
                &default_env,
                &mounts
            ),
            TrashDisposition::Container(TrashContainer::XdgRoot)
        );
        // Empty or relative XDG_DATA_HOME falls back to ~/.local/share.
        for bad_xdg in ["", "relative/data", "./local"] {
            let env = TrashEnv {
                xdg_data_home: Some(PathBuf::from(bad_xdg)),
                ..default_env.clone()
            };
            assert_eq!(
                adapter.classify(
                    Path::new("/home/tobi/.local/share/Trash"),
                    &env,
                    &mounts
                ),
                TrashDisposition::Container(TrashContainer::XdgRoot),
                "fallback for {bad_xdg:?}"
            );
        }

        let custom_env = TrashEnv {
            xdg_data_home: Some(PathBuf::from("/var/tmp/xdg")),
            ..default_env.clone()
        };
        assert_eq!(
            adapter.classify(
                Path::new("/var/tmp/xdg/Trash"),
                &custom_env,
                &mounts
            ),
            TrashDisposition::Container(TrashContainer::XdgRoot)
        );
        assert_eq!(
            adapter.classify(
                Path::new("/home/tobi/.local/share/Trash"),
                &custom_env,
                &mounts
            ),
            TrashDisposition::NotTrash
        );

        assert_eq!(
            adapter.classify(
                Path::new("/home/tobi/.local/share/Trash/files"),
                &default_env,
                &mounts
            ),
            TrashDisposition::Container(TrashContainer::XdgSubdir(
                XdgTrashSubdir::Files
            ))
        );
        assert_eq!(
            adapter.classify(
                Path::new("/home/tobi/.local/share/Trash/info"),
                &default_env,
                &mounts
            ),
            TrashDisposition::Container(TrashContainer::XdgSubdir(
                XdgTrashSubdir::Info
            ))
        );
        assert_eq!(
            adapter.classify(
                Path::new("/home/tobi/.local/share/Trash/expunged"),
                &default_env,
                &mounts
            ),
            TrashDisposition::Container(TrashContainer::XdgSubdir(
                XdgTrashSubdir::Expunged
            ))
        );
        assert_eq!(
            adapter.classify(
                Path::new("/home/tobi/.local/share/Trash/files/doc.pdf"),
                &default_env,
                &mounts
            ),
            TrashDisposition::Entry {
                paired_metadata: Some(PathBuf::from(
                    "/home/tobi/.local/share/Trash/info/doc.pdf.trashinfo"
                )),
            }
        );
        assert_eq!(
            adapter.classify(
                Path::new("/home/tobi/.local/share/Trash/files/dir/nested"),
                &default_env,
                &mounts
            ),
            TrashDisposition::Entry {
                paired_metadata: None
            }
        );
        assert_eq!(
            adapter.classify(
                Path::new(
                    "/home/tobi/.local/share/Trash/info/doc.pdf.trashinfo"
                ),
                &default_env,
                &mounts
            ),
            TrashDisposition::Entry {
                paired_metadata: None
            }
        );

        // Per-mount .Trash/<uid> and .Trash-<uid>
        assert_eq!(
            adapter.classify(
                Path::new("/mnt/usb/.Trash"),
                &default_env,
                &mounts
            ),
            TrashDisposition::Container(TrashContainer::LinuxVolumeTrashRoot)
        );
        assert_eq!(
            adapter.classify(
                Path::new("/mnt/usb/.Trash/1000"),
                &default_env,
                &mounts
            ),
            TrashDisposition::Container(TrashContainer::XdgRoot)
        );
        assert_eq!(
            adapter.classify(
                Path::new("/mnt/usb/.Trash/1000/files/a"),
                &default_env,
                &mounts
            ),
            TrashDisposition::Entry {
                paired_metadata: Some(PathBuf::from(
                    "/mnt/usb/.Trash/1000/info/a.trashinfo"
                )),
            }
        );
        assert_eq!(
            adapter.classify(
                Path::new("/mnt/usb/.Trash/1001"),
                &default_env,
                &mounts
            ),
            TrashDisposition::BlockedAlways(OTHER_USER_TRASH_REASON)
        );
        assert_eq!(
            adapter.classify(
                Path::new("/mnt/usb/.Trash-1000"),
                &default_env,
                &mounts
            ),
            TrashDisposition::Container(TrashContainer::XdgRoot)
        );
        assert_eq!(
            adapter.classify(
                Path::new("/mnt/usb/.Trash-1000/files/b"),
                &default_env,
                &mounts
            ),
            TrashDisposition::Entry {
                paired_metadata: Some(PathBuf::from(
                    "/mnt/usb/.Trash-1000/info/b.trashinfo"
                )),
            }
        );
        assert_eq!(
            adapter.classify(
                Path::new("/mnt/usb/.Trash-1001"),
                &default_env,
                &mounts
            ),
            TrashDisposition::BlockedAlways(OTHER_USER_TRASH_REASON)
        );
    }

    #[cfg(unix)]
    #[test]
    fn linux_paired_trashinfo_preserves_non_utf8_bytes() {
        use std::os::unix::ffi::OsStrExt as _;

        let adapter = TrashAdapterFactory::create_for(TrashOs::Linux);
        let env = TrashEnv {
            home: Some(PathBuf::from("/home/tobi")),
            xdg_data_home: None,
            uid: Some(1000),
        };
        let raw_name = OsStr::from_bytes(b"raw-\xff-item");
        let path =
            Path::new("/home/tobi/.local/share/Trash/files").join(raw_name);
        let expected_info = Path::new("/home/tobi/.local/share/Trash/info")
            .join(OsStr::from_bytes(b"raw-\xff-item.trashinfo"));

        assert_eq!(
            adapter.classify(&path, &env, &[]),
            TrashDisposition::Entry {
                paired_metadata: Some(expected_info)
            }
        );
    }

    #[test]
    fn windows_classifies_all_drives_desktop_ini_and_r_i_pairing() {
        let adapter = TrashAdapterFactory::create_for(TrashOs::Windows);
        let env = TrashEnv::default();
        let mounts = [PathBuf::from(r"C:\"), PathBuf::from(r"D:\")];

        assert_eq!(
            adapter.classify(Path::new(r"C:\$Recycle.Bin"), &env, &mounts),
            TrashDisposition::Container(TrashContainer::WindowsRecycleRoot {
                volume_root: PathBuf::from(r"C:\")
            })
        );
        assert_eq!(
            adapter.classify(Path::new(r"\\?\d:/$recycle.bin"), &env, &mounts),
            TrashDisposition::Container(TrashContainer::WindowsRecycleRoot {
                volume_root: PathBuf::from(r"D:\")
            })
        );
        assert_eq!(
            adapter.classify(
                Path::new(r"D:\$Recycle.Bin\S-1-5-21-100"),
                &env,
                &mounts
            ),
            TrashDisposition::Container(TrashContainer::WindowsRecycleSidDir)
        );
        assert_eq!(
            adapter.classify(
                Path::new(r"D:\$Recycle.Bin\desktop.ini"),
                &env,
                &mounts
            ),
            TrashDisposition::BlockedAlways(RECYCLE_BIN_METADATA_REASON)
        );
        assert_eq!(
            adapter.classify(
                Path::new(r"D:\$Recycle.Bin\S-1-5-21-100\DESKTOP.INI"),
                &env,
                &mounts
            ),
            TrashDisposition::BlockedAlways(RECYCLE_BIN_METADATA_REASON)
        );
        assert_eq!(
            adapter.classify(
                Path::new(r"D:\$Recycle.Bin\S-1-5-21-100\$RABC123.txt"),
                &env,
                &mounts
            ),
            TrashDisposition::Entry {
                paired_metadata: Some(PathBuf::from(
                    r"D:\$Recycle.Bin\S-1-5-21-100\$IABC123.txt"
                )),
            }
        );
        assert_eq!(
            adapter.classify(
                Path::new(r"D:\$Recycle.Bin\S-1-5-21-100\$IABC123.txt"),
                &env,
                &mounts
            ),
            TrashDisposition::Entry {
                paired_metadata: None
            }
        );
        assert_eq!(
            adapter.classify(
                Path::new(r"D:\$Recycle.Bin\S-1-5-21-100\nested\item.txt"),
                &env,
                &mounts
            ),
            TrashDisposition::Entry {
                paired_metadata: None
            }
        );
        assert_eq!(
            adapter.classify(
                Path::new(r"C:\Users\tobi\$Recycle.Bin"),
                &env,
                &mounts
            ),
            TrashDisposition::NotTrash
        );

        // Volume root sanitizer safety checks.
        assert_eq!(
            sanitize_windows_volume_root(Path::new(r"\\?\c:")).unwrap(),
            r"C:\"
        );
        assert_eq!(
            sanitize_windows_volume_root(Path::new(r"\\?\D:\")).unwrap(),
            r"D:\"
        );
        assert!(sanitize_windows_volume_root(Path::new("")).is_err());
        assert!(sanitize_windows_volume_root(Path::new(r"\\?\")).is_err());
    }

    #[cfg(unix)]
    #[test]
    fn macos_empty_in_place_preserves_container_inode() {
        use std::os::unix::fs::MetadataExt as _;

        let temp = TempDir::new().expect("tempdir");
        let trash = temp.path().join(".Trash");
        fs::create_dir_all(trash.join("folder/sub")).expect("mkdir");
        fs::write(trash.join("top.txt"), b"top").expect("write");
        fs::write(trash.join("folder/sub/deep.txt"), b"deep").expect("write");
        let before_ino = fs::symlink_metadata(&trash).expect("stat").ino();

        let adapter = TrashAdapterFactory::create_for(TrashOs::MacOs);
        let disp = TrashDisposition::Container(TrashContainer::DirKeepSelf);
        adapter
            .remove_permanent(&trash, temp.path(), &disp)
            .expect("empty trash");

        assert!(trash.is_dir(), ".Trash container must remain");
        assert_eq!(
            fs::symlink_metadata(&trash).expect("stat").ino(),
            before_ino,
            ".Trash inode must be preserved"
        );
        assert_eq!(
            fs::read_dir(&trash).expect("readdir").count(),
            0,
            ".Trash contents must be emptied"
        );
    }

    #[cfg(unix)]
    #[test]
    fn linux_xdg_empty_in_place_preserves_skeleton_and_paired_metadata() {
        use std::os::unix::fs::MetadataExt as _;

        let temp = TempDir::new().expect("tempdir");
        let xdg = temp.path().join("Trash");
        let files = xdg.join("files");
        let info = xdg.join("info");
        let expunged = xdg.join("expunged");
        fs::create_dir_all(files.join("nested")).expect("mkdir");
        fs::create_dir_all(&info).expect("mkdir");
        fs::create_dir_all(&expunged).expect("mkdir");
        fs::write(files.join("nested/a.txt"), b"a").expect("write");
        fs::write(info.join("nested.trashinfo"), b"[Trash Info]\n")
            .expect("write");
        fs::write(expunged.join("old"), b"old").expect("write");

        let root_ino = fs::symlink_metadata(&xdg).expect("stat").ino();
        let files_ino = fs::symlink_metadata(&files).expect("stat").ino();
        let info_ino = fs::symlink_metadata(&info).expect("stat").ino();

        let adapter = TrashAdapterFactory::create_for(TrashOs::Linux);
        adapter
            .remove_permanent(
                &xdg,
                temp.path(),
                &TrashDisposition::Container(TrashContainer::XdgRoot),
            )
            .expect("empty XdgRoot");

        assert_eq!(fs::symlink_metadata(&xdg).unwrap().ino(), root_ino);
        assert_eq!(fs::symlink_metadata(&files).unwrap().ino(), files_ino);
        assert_eq!(fs::symlink_metadata(&info).unwrap().ino(), info_ino);
        assert_eq!(fs::read_dir(&files).unwrap().count(), 0);
        assert_eq!(fs::read_dir(&info).unwrap().count(), 0);
        assert_eq!(fs::read_dir(&expunged).unwrap().count(), 0);

        // XdgSubdir(Files) removes paired info/*.trashinfo only when files/*
        // succeeds, and Entry removes paired info/*.trashinfo even when root
        // is files/.
        fs::write(files.join("one.bin"), b"1").expect("write");
        fs::write(info.join("one.bin.trashinfo"), b"info1").expect("write");
        fs::write(files.join("two.bin"), b"2").expect("write");
        fs::write(info.join("two.bin.trashinfo"), b"info2").expect("write");
        fs::write(info.join("unrelated.trashinfo"), b"keep").expect("write");

        let entry_path = files.join("one.bin");
        let entry_disp = TrashDisposition::Entry {
            paired_metadata: Some(info.join("one.bin.trashinfo")),
        };
        adapter
            .remove_permanent(&entry_path, &files, &entry_disp)
            .expect("remove single entry with root=files/");
        assert!(!entry_path.exists());
        assert!(!info.join("one.bin.trashinfo").exists());
        assert!(info.join("two.bin.trashinfo").exists());

        adapter
            .remove_permanent(
                &files,
                &xdg,
                &TrashDisposition::Container(TrashContainer::XdgSubdir(
                    XdgTrashSubdir::Files,
                )),
            )
            .expect("empty files subdir");
        assert!(files.is_dir());
        assert!(!files.join("two.bin").exists());
        assert!(!info.join("two.bin.trashinfo").exists());
        assert!(info.join("unrelated.trashinfo").exists());
    }

    #[cfg(unix)]
    #[test]
    fn linux_xdg_refuses_symlinked_info_dir_and_tolerates_missing_trashinfo() {
        let temp = TempDir::new().expect("tempdir");
        let xdg = temp.path().join("Trash");
        let files = xdg.join("files");
        let outside = temp.path().join("outside");
        fs::create_dir_all(&files).expect("mkdir");
        fs::create_dir_all(&outside).expect("mkdir");
        fs::write(outside.join("secret.trashinfo"), b"keep").expect("write");
        fs::write(files.join("item"), b"1").expect("write");

        // Missing info/ and missing .trashinfo are tolerated.
        let adapter = TrashAdapterFactory::create_for(TrashOs::Linux);
        adapter
            .remove_permanent(
                &files.join("item"),
                &files,
                &TrashDisposition::Entry {
                    paired_metadata: Some(xdg.join("info/item.trashinfo")),
                },
            )
            .expect("missing trashinfo tolerated");
        assert!(!files.join("item").exists());

        // Symlinked info/ is refused without deleting outside target.
        std::os::unix::fs::symlink(&outside, xdg.join("info"))
            .expect("symlink");
        fs::write(files.join("secret"), b"2").expect("write");
        let err = adapter
            .remove_permanent(
                &files.join("secret"),
                &files,
                &TrashDisposition::Entry {
                    paired_metadata: Some(xdg.join("info/secret.trashinfo")),
                },
            )
            .expect_err("symlinked info must be refused");
        assert!(err.to_string().contains("symlink"), "{err}");
        assert!(outside.join("secret.trashinfo").exists());
    }

    #[cfg(unix)]
    #[test]
    fn best_effort_drain_removes_other_entries_when_one_is_locked() {
        use std::os::unix::fs::PermissionsExt as _;

        // Root bypasses directory write permissions on Unix.
        if rustix::process::getuid().is_root() {
            return;
        }

        let temp = TempDir::new().expect("tempdir");
        let trash = temp.path().join(".Trash");
        fs::create_dir_all(&trash).expect("mkdir");
        fs::write(trash.join("01_ok.txt"), b"ok1").expect("write");
        let locked = trash.join("02_locked");
        fs::create_dir(&locked).expect("mkdir");
        fs::write(locked.join("pinned.txt"), b"pinned").expect("write");
        fs::set_permissions(&locked, fs::Permissions::from_mode(0o555))
            .expect("chmod 555");
        fs::write(trash.join("03_ok.txt"), b"ok3").expect("write");

        let adapter = TrashAdapterFactory::create_for(TrashOs::MacOs);
        let err = adapter
            .remove_permanent(
                &trash,
                temp.path(),
                &TrashDisposition::Container(TrashContainer::DirKeepSelf),
            )
            .expect_err("locked entry must report summarized error");

        // Restore permissions before assertions/cleanup so TempDir drops cleanly.
        let _ = fs::set_permissions(&locked, fs::Permissions::from_mode(0o755));

        assert!(!trash.join("01_ok.txt").exists(), "01_ok should be drained");
        assert!(!trash.join("03_ok.txt").exists(), "03_ok should be drained");
        assert!(locked.exists(), "locked directory remains");
        assert!(
            err.to_string().contains('1'),
            "error should count failure: {err}"
        );
    }

    #[test]
    fn windows_sid_dir_drain_preserves_desktop_ini_and_tolerates_missing_i() {
        let temp = TempDir::new().expect("tempdir");
        let bin = temp.path().join("$Recycle.Bin");
        let sid = bin.join("S-1-5-21-100");
        fs::create_dir_all(sid.join("folder")).expect("mkdir");
        fs::write(sid.join("desktop.ini"), b"[.ShellClassInfo]").expect("ini");
        fs::write(sid.join("$R1.txt"), b"data").expect("r1");
        fs::write(sid.join("$I1.txt"), b"meta").expect("i1");
        fs::write(sid.join("folder/inner.txt"), b"inner").expect("inner");

        let adapter = TrashAdapterFactory::create_for(TrashOs::Windows);
        // Removing $R2 when $I2 is already gone tolerates NotFound.
        fs::write(sid.join("$R2.txt"), b"data2").expect("r2");
        adapter
            .remove_permanent(
                &sid.join("$R2.txt"),
                temp.path(),
                &TrashDisposition::Entry {
                    paired_metadata: Some(sid.join("$I2.txt")),
                },
            )
            .expect("missing $I2 tolerated");
        assert!(!sid.join("$R2.txt").exists());

        // Draining SID dir removes everything except desktop.ini and keeps SID.
        adapter
            .remove_permanent(
                &sid,
                temp.path(),
                &TrashDisposition::Container(
                    TrashContainer::WindowsRecycleSidDir,
                ),
            )
            .expect("drain SID dir");
        assert!(sid.is_dir());
        assert!(sid.join("desktop.ini").exists());
        assert!(!sid.join("$R1.txt").exists());
        assert!(!sid.join("$I1.txt").exists());
        assert!(!sid.join("folder").exists());
    }

    #[cfg(unix)]
    #[test]
    fn windows_recycle_root_skips_unreadable_sid_but_reports_inner_denied() {
        use std::os::unix::fs::PermissionsExt as _;

        if rustix::process::getuid().is_root() {
            return;
        }

        let temp = TempDir::new().expect("tempdir");
        let bin = temp.path().join("$Recycle.Bin");
        let other_sid = bin.join("S-1-5-21-999");
        let own_sid = bin.join("S-1-5-21-1000");
        fs::create_dir_all(&other_sid).expect("mkdir other");
        fs::write(other_sid.join("$R9.txt"), b"other").expect("write other");
        fs::set_permissions(&other_sid, fs::Permissions::from_mode(0o000))
            .expect("chmod 000");

        fs::create_dir_all(&own_sid).expect("mkdir own");
        fs::write(own_sid.join("desktop.ini"), b"ini").expect("write ini");
        fs::write(own_sid.join("$R1.txt"), b"ok").expect("write ok");

        let adapter = TrashAdapterFactory::create_for(TrashOs::Windows);
        let disp =
            TrashDisposition::Container(TrashContainer::WindowsRecycleRoot {
                volume_root: PathBuf::from(r"C:\"),
            });

        // Inaccessible other-user SID folder is skipped without error.
        adapter
            .remove_permanent(&bin, temp.path(), &disp)
            .expect("unreadable SID skipped");
        assert!(own_sid.join("desktop.ini").exists());
        assert!(!own_sid.join("$R1.txt").exists());

        // Inner PermissionDenied inside an accessible SID is NOT swallowed.
        let locked_sub = own_sid.join("locked");
        fs::create_dir(&locked_sub).expect("mkdir locked");
        fs::write(locked_sub.join("pinned.txt"), b"pinned").expect("write");
        fs::set_permissions(&locked_sub, fs::Permissions::from_mode(0o555))
            .expect("chmod 555");

        let err = adapter
            .remove_permanent(&bin, temp.path(), &disp)
            .expect_err("inner PermissionDenied must be reported");

        let _ =
            fs::set_permissions(&other_sid, fs::Permissions::from_mode(0o755));
        let _ =
            fs::set_permissions(&locked_sub, fs::Permissions::from_mode(0o755));

        assert_eq!(err.kind(), io::ErrorKind::PermissionDenied);
    }
}
