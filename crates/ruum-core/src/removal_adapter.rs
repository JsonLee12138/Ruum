//! Tri-platform removal execution, Windows extended-length path handling,
//! and Trash error classification.
//!
//! Mirrors [`crate::trash_adapter`]'s Factory + Adapter pattern so
//! `removal.rs`, `scan.rs`, `trash_adapter.rs`, and cross-OS unit tests share
//! one authoritative implementation across macOS, Linux, and Windows.

use std::fs;
use std::io;
use std::path::{Component, Path, PathBuf};
#[cfg(windows)]
use std::time::Duration;

use crate::removal::{
    guard_key, is_dir_link, normalize, summarize_drain_errors,
};

/// Win32 `MAX_PATH` threshold (in UTF-16 code units, including NUL).
const WIN_MAX_PATH: usize = 260;

/// Operating system family for permanent removal and path normalization.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum RemovalOs {
    MacOs,
    Linux,
    Windows,
}

/// Environment context for removal operations.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct RemovalEnv {
    pub home: Option<PathBuf>,
    pub current_dir: Option<PathBuf>,
}

impl RemovalEnv {
    /// Capture the current process environment once per removal run.
    #[must_use]
    pub fn current() -> Self {
        Self {
            home: std::env::home_dir(),
            current_dir: std::env::current_dir().ok(),
        }
    }
}

/// High-level classification of a desktop Trash / Recycle Bin failure.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum TrashErrorKind {
    LongPathOrRecycleLimit,
    LockedOrDenied,
    Other,
}

/// Adapter interface encapsulating OS-specific permanent removal, long-path
/// syscall adaptation, and trash failure guidance.
pub trait RemovalAdapter: Send + Sync + std::fmt::Debug {
    fn os_kind(&self) -> RemovalOs;
    fn syscall_path(&self, path: &Path) -> PathBuf;
    fn strip_syscall_prefix(&self, path: &Path) -> PathBuf;
    fn remove_permanently(&self, path: &Path, root: &Path) -> io::Result<()>;
    fn remove_entry_best_effort(&self, path: &Path) -> io::Result<()>;
    fn format_trash_error(&self, kind: TrashErrorKind, raw: &str) -> String;
}

/// macOS removal adapter.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct MacOsRemovalAdapter;

impl RemovalAdapter for MacOsRemovalAdapter {
    fn os_kind(&self) -> RemovalOs {
        RemovalOs::MacOs
    }

    fn syscall_path(&self, path: &Path) -> PathBuf {
        path.to_path_buf()
    }

    fn strip_syscall_prefix(&self, path: &Path) -> PathBuf {
        path.to_path_buf()
    }

    fn remove_permanently(&self, path: &Path, root: &Path) -> io::Result<()> {
        remove_permanently_unix_or_fallback(path, root)
    }

    fn remove_entry_best_effort(&self, path: &Path) -> io::Result<()> {
        remove_tree_iterative(path, true)
    }

    fn format_trash_error(&self, _kind: TrashErrorKind, raw: &str) -> String {
        format!(
            "could not move to the Trash ({raw}); network and read-only \
             disks have none, so use permanent deletion there"
        )
    }
}

/// Linux removal adapter.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct LinuxRemovalAdapter;

impl RemovalAdapter for LinuxRemovalAdapter {
    fn os_kind(&self) -> RemovalOs {
        RemovalOs::Linux
    }

    fn syscall_path(&self, path: &Path) -> PathBuf {
        path.to_path_buf()
    }

    fn strip_syscall_prefix(&self, path: &Path) -> PathBuf {
        path.to_path_buf()
    }

    fn remove_permanently(&self, path: &Path, root: &Path) -> io::Result<()> {
        remove_permanently_unix_or_fallback(path, root)
    }

    fn remove_entry_best_effort(&self, path: &Path) -> io::Result<()> {
        remove_tree_iterative(path, true)
    }

    fn format_trash_error(&self, _kind: TrashErrorKind, raw: &str) -> String {
        format!(
            "could not move to the trash ({raw}); use permanent deletion \
             instead"
        )
    }
}

/// Windows removal adapter with `\\?\` extended-length paths, mount-point
/// guards, read-only attribute clearing, and bounded transient-lock retries.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct WindowsRemovalAdapter;

impl RemovalAdapter for WindowsRemovalAdapter {
    fn os_kind(&self) -> RemovalOs {
        RemovalOs::Windows
    }

    fn syscall_path(&self, path: &Path) -> PathBuf {
        to_windows_extended_path(path)
    }

    fn strip_syscall_prefix(&self, path: &Path) -> PathBuf {
        strip_windows_extended_prefix_path(path)
    }

    fn remove_permanently(&self, path: &Path, root: &Path) -> io::Result<()> {
        verify_below_root_windows(path, root)?;
        remove_tree_iterative(path, false)
    }

    fn remove_entry_best_effort(&self, path: &Path) -> io::Result<()> {
        remove_tree_iterative(path, true)
    }

    fn format_trash_error(&self, kind: TrashErrorKind, raw: &str) -> String {
        match kind {
            TrashErrorKind::LongPathOrRecycleLimit => format!(
                "could not move to the Recycle Bin ({raw}); Windows Recycle \
                 Bin cannot accept paths over 260 characters (common in \
                 node_modules) or items exceeding Recycle Bin capacity — \
                 switch to permanent deletion"
            ),
            TrashErrorKind::LockedOrDenied => format!(
                "could not move to the Recycle Bin ({raw}); files may be in \
                 use by another process (such as Node, IDE, or antivirus) — \
                 close locking programs and retry, or use permanent deletion"
            ),
            TrashErrorKind::Other => format!(
                "could not move to the Recycle Bin ({raw}); if this is a \
                 deep or read-only directory such as node_modules, try \
                 permanent deletion"
            ),
        }
    }
}

/// Factory returning the removal adapter for the current or a requested OS.
#[derive(Clone, Copy, Debug, Default)]
pub struct RemovalAdapterFactory;

impl RemovalAdapterFactory {
    /// Detect the compile-time OS family.
    #[must_use]
    pub const fn current_os() -> RemovalOs {
        if cfg!(target_os = "macos") {
            RemovalOs::MacOs
        } else if cfg!(windows) {
            RemovalOs::Windows
        } else {
            RemovalOs::Linux
        }
    }

    /// Construct the adapter for an explicit [`RemovalOs`] (used by tests).
    #[must_use]
    pub fn create_for(os: RemovalOs) -> Box<dyn RemovalAdapter> {
        match os {
            RemovalOs::MacOs => Box::new(MacOsRemovalAdapter),
            RemovalOs::Linux => Box::new(LinuxRemovalAdapter),
            RemovalOs::Windows => Box::new(WindowsRemovalAdapter),
        }
    }

    /// Construct the adapter for the host operating system.
    #[must_use]
    pub fn create() -> Box<dyn RemovalAdapter> {
        Self::create_for(Self::current_os())
    }
}

fn remove_permanently_unix_or_fallback(
    path: &Path,
    root: &Path,
) -> io::Result<()> {
    #[cfg(unix)]
    {
        crate::removal::remove_permanently_unix(path, root)
    }
    #[cfg(not(unix))]
    {
        WindowsRemovalAdapter.remove_permanently(path, root)
    }
}

/// Normalize a Windows path string to an extended-length `\\?\` path without
/// touching the filesystem or following symbolic links.
///
/// Preserves existing `\\?\` / `\\.\` prefixes, converts `\\server\share\...`
/// to `\\?\UNC\server\share\...`, converts `<drive>:\...` to
/// `\\?\<drive>:\...`, replaces `/` with `\`, and lexically resolves `.`/`..`.
/// Relative or drive-relative strings are returned with separators normalized.
#[must_use]
pub fn format_windows_extended_path(path: &str) -> String {
    if path.starts_with(r"\\?\") || path.starts_with(r"\\.\") {
        return path.to_owned();
    }
    let normalized = path.replace('/', r"\");
    if let Some(unc_rest) = normalized.strip_prefix(r"\\") {
        let mut parts =
            unc_rest.split('\\').filter(|segment| !segment.is_empty());
        let Some(server) = parts.next() else {
            return normalized;
        };
        let Some(share) = parts.next() else {
            return normalized;
        };
        let resolved = resolve_lexical_segments(parts);
        return if resolved.is_empty() {
            format!(r"\\?\UNC\{server}\{share}")
        } else {
            format!(r"\\?\UNC\{server}\{share}\{}", resolved.join(r"\"))
        };
    }
    if let Some((drive, rest)) = split_drive_absolute(&normalized) {
        let resolved = resolve_lexical_segments(
            rest.split('\\').filter(|segment| !segment.is_empty()),
        );
        return if resolved.is_empty() {
            format!(r"\\?\{drive}:\")
        } else {
            format!(r"\\?\{drive}:\{}", resolved.join(r"\"))
        };
    }
    normalized
}

fn split_drive_absolute(path: &str) -> Option<(char, &str)> {
    let bytes = path.as_bytes();
    let drive = *bytes.first()?;
    if !drive.is_ascii_alphabetic() {
        return None;
    }
    if bytes.get(1) != Some(&b':') || bytes.get(2) != Some(&b'\\') {
        return None;
    }
    Some((char::from(drive), &path[3..]))
}

fn resolve_lexical_segments<'a>(
    segments: impl Iterator<Item = &'a str>,
) -> Vec<&'a str> {
    let mut out = Vec::new();
    for segment in segments {
        match segment {
            "" | "." => {}
            ".." => {
                out.pop();
            }
            part => out.push(part),
        }
    }
    out
}

/// Strip `\\?\` / `\\?\UNC\` from a Windows path string when the resulting
/// path fits under `MAX_PATH` (260 UTF-16 code units).
///
/// Volume GUID paths (`\\?\Volume{...}\`) and paths >= 260 UTF-16 code units
/// are preserved untouched.
#[must_use]
pub fn strip_windows_extended_prefix(path: &str) -> String {
    let Some(after_prefix) = path.strip_prefix(r"\\?\") else {
        return path.to_owned();
    };
    if starts_with_ascii_ci(after_prefix, "volume{") {
        return path.to_owned();
    }
    let candidate =
        if let Some(unc_rest) = strip_prefix_ascii_ci(after_prefix, r"unc\") {
            format!(r"\\{unc_rest}")
        } else if split_drive_absolute(after_prefix).is_some() {
            after_prefix.to_owned()
        } else {
            return path.to_owned();
        };
    if candidate.encode_utf16().count() < WIN_MAX_PATH {
        candidate
    } else {
        path.to_owned()
    }
}

fn starts_with_ascii_ci(haystack: &str, prefix: &str) -> bool {
    haystack
        .get(..prefix.len())
        .is_some_and(|head| head.eq_ignore_ascii_case(prefix))
}

fn strip_prefix_ascii_ci<'a>(
    haystack: &'a str,
    prefix: &str,
) -> Option<&'a str> {
    starts_with_ascii_ci(haystack, prefix).then(|| &haystack[prefix.len()..])
}

/// Strip `\\?\` / `\\?\UNC\` from a [`Path`] when safe to expose to UI/CLI.
#[must_use]
pub fn strip_windows_extended_prefix_path(path: &Path) -> PathBuf {
    if let Some(text) = path.to_str() {
        return PathBuf::from(strip_windows_extended_prefix(text));
    }
    #[cfg(windows)]
    {
        strip_windows_extended_prefix_wide(path)
    }
    #[cfg(not(windows))]
    {
        path.to_path_buf()
    }
}

#[cfg(windows)]
fn eq_ascii_ci_wide(slice: &[u16], ascii_lower: &[u8]) -> bool {
    slice.len() >= ascii_lower.len()
        && slice[..ascii_lower.len()]
            .iter()
            .zip(ascii_lower)
            .all(|(&u, &b)| {
                u8::try_from(u).is_ok_and(|byte| byte.to_ascii_lowercase() == b)
            })
}

#[cfg(windows)]
fn wide_is_ascii_alpha(unit: u16) -> bool {
    u8::try_from(unit).is_ok_and(|byte| byte.is_ascii_alphabetic())
}

#[cfg(windows)]
fn strip_windows_extended_prefix_wide(path: &Path) -> PathBuf {
    use std::ffi::OsString;
    use std::os::windows::ffi::{OsStrExt as _, OsStringExt as _};

    let units: Vec<u16> = path.as_os_str().encode_wide().collect();
    let verbatim: [u16; 4] = [92, 92, 63, 92]; // \\?\
    if !units.starts_with(&verbatim) {
        return path.to_path_buf();
    }
    let rest = &units[4..];
    if eq_ascii_ci_wide(rest, b"volume{") {
        return path.to_path_buf();
    }
    let candidate = if eq_ascii_ci_wide(rest, br"unc\") {
        let mut out = vec![92_u16, 92];
        out.extend_from_slice(&rest[4..]);
        out
    } else if rest.len() >= 3
        && wide_is_ascii_alpha(rest[0])
        && rest[1] == u16::from(b':')
        && rest[2] == u16::from(b'\\')
    {
        rest.to_vec()
    } else {
        return path.to_path_buf();
    };
    if candidate.len() < WIN_MAX_PATH {
        PathBuf::from(OsString::from_wide(&candidate))
    } else {
        path.to_path_buf()
    }
}

/// Convert `path` into a Windows `\\?\` extended-length path for syscalls.
///
/// Avoids `fs::canonicalize` so symlinks are never followed, and avoids
/// `std::path::absolute` when `path` is already fully qualified so long paths
/// are never truncated in un-manifested processes.
#[must_use]
pub fn to_windows_extended_path(path: &Path) -> PathBuf {
    #[cfg(windows)]
    {
        let resolved = if is_fully_qualified_windows_path(path) {
            path.to_path_buf()
        } else {
            std::path::absolute(path).unwrap_or_else(|_| path.to_path_buf())
        };
        if let Some(text) = resolved.to_str() {
            return PathBuf::from(format_windows_extended_path(text));
        }
        to_windows_extended_path_wide(&resolved)
    }
    #[cfg(not(windows))]
    {
        if let Some(text) = path.to_str() {
            PathBuf::from(format_windows_extended_path(text))
        } else {
            path.to_path_buf()
        }
    }
}

#[cfg(windows)]
fn is_fully_qualified_windows_path(path: &Path) -> bool {
    use std::os::windows::ffi::OsStrExt as _;
    let mut it = path.as_os_str().encode_wide();
    let Some(u0) = it.next() else {
        return false;
    };
    let Some(u1) = it.next() else {
        return false;
    };
    let Some(u2) = it.next() else {
        return false;
    };
    let slash = u16::from(b'\\');
    let fwd = u16::from(b'/');
    let is_sep = |u: u16| u == slash || u == fwd;
    // \\server\share or \\?\... or \\.\...
    if is_sep(u0) && is_sep(u1) {
        return true;
    }
    // <drive>:\... or <drive>:/...
    wide_is_ascii_alpha(u0) && u16::from(b':') == u1 && is_sep(u2)
}

#[cfg(windows)]
fn to_windows_extended_path_wide(path: &Path) -> PathBuf {
    use std::ffi::OsString;
    use std::os::windows::ffi::{OsStrExt as _, OsStringExt as _};

    let slash = u16::from(b'\\');
    let fwd = u16::from(b'/');
    let units: Vec<u16> = path
        .as_os_str()
        .encode_wide()
        .map(|u| if u == fwd { slash } else { u })
        .collect();
    let verbatim: [u16; 4] = [92, 92, 63, 92]; // \\?\
    let device: [u16; 4] = [92, 92, 46, 92]; // \\.\
    if units.starts_with(&verbatim) || units.starts_with(&device) {
        return PathBuf::from(OsString::from_wide(&units));
    }
    let mut out = if units.starts_with(&[slash, slash]) {
        // \\server\share... -> \\?\UNC\server\share...
        let mut unc = vec![92, 92, 63, 92, 85, 78, 67, 92]; // \\?\UNC\
        unc.extend_from_slice(&units[2..]);
        unc
    } else if units.len() >= 3
        && wide_is_ascii_alpha(units[0])
        && units[1] == u16::from(b':')
        && units[2] == slash
    {
        let mut drive = vec![92, 92, 63, 92]; // \\?\
        drive.extend_from_slice(&units);
        drive
    } else {
        units
    };
    while out.len() > 7 && out.last() == Some(&slash) {
        out.pop();
    }
    PathBuf::from(OsString::from_wide(&out))
}

/// Classify a Windows `HRESULT` returned by `trash::delete` (`IFileOperation`)
/// using documented `WinError.h` and `ShErrors.h` codes.
#[must_use]
pub const fn classify_windows_trash_hresult(code: u32) -> TrashErrorKind {
    match code {
        // ERROR_FILENAME_EXCED_RANGE, COPYENGINE_E_PATH_TOO_DEEP_SRC,
        // COPYENGINE_E_RECYCLE_{UNKNOWN_ERROR,FORCE_NUKE,SIZE_TOO_BIG,PATH_TOO_LONG}
        0x8007_00CE | 0x8027_001D | 0x8027_0035 | 0x8027_0036 | 0x8027_0037
        | 0x8027_0038 => TrashErrorKind::LongPathOrRecycleLimit,
        // ERROR_SHARING_VIOLATION, ERROR_LOCK_VIOLATION, E_ACCESSDENIED,
        // COPYENGINE_E_SHARING_VIOLATION_{SRC,DEST}
        0x8007_0020 | 0x8007_0021 | 0x8007_0005 | 0x8027_0027 | 0x8027_0028 => {
            TrashErrorKind::LockedOrDenied
        }
        _ => TrashErrorKind::Other,
    }
}

fn syscall_if_windows(path: &Path) -> PathBuf {
    if cfg!(windows) {
        to_windows_extended_path(path)
    } else {
        path.to_path_buf()
    }
}

/// Verify that `path` is strictly inside `root` and that every intermediate
/// directory strictly below `root` up to `path.parent()` is a real directory
/// (never a symlink, junction, or mount point).
///
/// `root` itself and its ancestors are not rejected if they are links, matching
/// Unix `open_parent` (`permanent_removal_follows_links_above_the_root`).
fn verify_below_root_windows(path: &Path, root: &Path) -> io::Result<()> {
    let norm_path = normalize(path);
    let norm_root = normalize(root);
    if norm_path.file_name().is_none() {
        return Err(io::Error::other(format!(
            "{} is not an absolute, normalized path",
            path.display()
        )));
    }
    let path_key = guard_key(&norm_path);
    let root_key = guard_key(&norm_root);
    if path_key == root_key || !path_key.starts_with(&root_key) {
        return Err(io::Error::other(format!(
            "{} is not inside the scanned root {}",
            path.display(),
            root.display()
        )));
    }
    let Some(parent) = norm_path.parent() else {
        return Err(io::Error::other(format!(
            "{} is not an absolute, normalized path",
            path.display()
        )));
    };
    let root_count = norm_root.components().count();
    let mut step = norm_root;
    for component in parent.components().skip(root_count) {
        match component {
            Component::Normal(part) => {
                step.push(part);
                let sys_step = syscall_if_windows(&step);
                let meta = fs::symlink_metadata(&sys_step)?;
                if meta.file_type().is_symlink()
                    || is_dir_link(&meta)
                    || !meta.is_dir()
                {
                    return Err(io::Error::other(format!(
                        "{} changed since the scan: {} is no longer a \
                         directory, so nothing was removed",
                        path.display(),
                        part.display()
                    )));
                }
                #[cfg(windows)]
                if crate::windows::is_mount_point(&sys_step) {
                    return Err(io::Error::other(format!(
                        "stopped at {}: another volume is mounted there, \
                         and nothing on it was touched",
                        step.display()
                    )));
                }
            }
            _ => {
                return Err(io::Error::other(format!(
                    "{} is not an absolute, normalized path",
                    path.display()
                )));
            }
        }
    }
    Ok(())
}

/// Remove a file or directory tree iteratively (avoiding thread-stack
/// overflow on 32,767-char deep paths).
fn remove_tree_iterative(target: &Path, best_effort: bool) -> io::Result<()> {
    enum Frame {
        Enter { path: PathBuf, is_root: bool },
        RemoveDir(PathBuf),
    }

    let start = syscall_if_windows(target);
    let mut stack = vec![Frame::Enter {
        path: start,
        is_root: true,
    }];
    let mut failures = 0_usize;
    let mut first_error: Option<io::Error> = None;

    let mut record = |err: io::Error| {
        failures += 1;
        if first_error.is_none() {
            first_error = Some(err);
        }
    };

    while let Some(frame) = stack.pop() {
        match frame {
            Frame::Enter { path, is_root } => {
                let meta = match fs::symlink_metadata(&path) {
                    Ok(meta) => meta,
                    Err(err)
                        if !is_root
                            && err.kind() == io::ErrorKind::NotFound =>
                    {
                        continue;
                    }
                    Err(err) if best_effort && !is_root => {
                        record(err);
                        continue;
                    }
                    Err(err) => return Err(err),
                };

                #[cfg(windows)]
                {
                    use std::os::windows::fs::MetadataExt as _;
                    // Must match ordinary dirs, junctions, dir symlinks, AND
                    // volume mount points BEFORE `is_dir_link` can unlink one.
                    const FILE_ATTRIBUTE_DIRECTORY: u32 = 0x10;
                    if meta.file_attributes() & FILE_ATTRIBUTE_DIRECTORY != 0
                        && crate::windows::is_mount_point(&path)
                    {
                        let clean = strip_windows_extended_prefix_path(&path);
                        let err = io::Error::other(format!(
                            "stopped at {}: another volume is mounted there, \
                             and nothing on it was touched",
                            clean.display()
                        ));
                        if best_effort && !is_root {
                            record(err);
                            continue;
                        }
                        return Err(err);
                    }
                }

                // Surrogate links (symlinks & junctions) only: unlink the link
                // itself without touching its target or clearing attributes.
                if meta.file_type().is_symlink() || is_dir_link(&meta) {
                    let res = if is_dir_link(&meta) {
                        remove_dir_with_retry(&path, false, best_effort)
                    } else {
                        remove_file_with_retry(&path, false, best_effort)
                    };
                    if let Err(err) = res {
                        if best_effort && !is_root {
                            record(err);
                        } else {
                            return Err(err);
                        }
                    }
                    continue;
                }

                if !meta.is_dir() {
                    if let Err(err) =
                        remove_file_with_retry(&path, true, best_effort)
                    {
                        if best_effort && !is_root {
                            record(err);
                        } else {
                            return Err(err);
                        }
                    }
                    continue;
                }

                // Collect child paths and drop `ReadDir` BEFORE removing any
                // entries or parent directory so no handle stays open.
                let mut children = Vec::new();
                let mut dir_failed = false;
                let entries = match fs::read_dir(&path) {
                    Ok(entries) => entries,
                    Err(err)
                        if !is_root
                            && err.kind() == io::ErrorKind::NotFound =>
                    {
                        continue;
                    }
                    Err(err) if best_effort && !is_root => {
                        record(err);
                        continue;
                    }
                    Err(err) => return Err(err),
                };
                for entry in entries {
                    match entry {
                        Ok(child) => children.push(child.path()),
                        Err(err) if best_effort => {
                            dir_failed = true;
                            record(err);
                        }
                        Err(err) => return Err(err),
                    }
                }

                if !dir_failed || !best_effort {
                    stack.push(Frame::RemoveDir(path));
                }
                for child in children.into_iter().rev() {
                    stack.push(Frame::Enter {
                        path: child,
                        is_root: false,
                    });
                }
            }
            Frame::RemoveDir(path) => {
                if let Err(err) =
                    remove_dir_with_retry(&path, true, best_effort)
                {
                    if best_effort {
                        record(err);
                    } else {
                        return Err(err);
                    }
                }
            }
        }
    }

    if failures == 0 {
        Ok(())
    } else {
        Err(summarize_drain_errors(failures, first_error))
    }
}

fn remove_file_with_retry(
    path: &Path,
    allow_clear_readonly: bool,
    best_effort: bool,
) -> io::Result<()> {
    remove_with_retry(path, allow_clear_readonly, best_effort, || {
        fs::remove_file(path)
    })
}

fn remove_dir_with_retry(
    path: &Path,
    allow_clear_readonly: bool,
    best_effort: bool,
) -> io::Result<()> {
    remove_with_retry(path, allow_clear_readonly, best_effort, || {
        fs::remove_dir(path)
    })
}

#[cfg(windows)]
fn remove_with_retry<F>(
    path: &Path,
    allow_clear_readonly: bool,
    best_effort: bool,
    mut op: F,
) -> io::Result<()>
where
    F: FnMut() -> io::Result<()>,
{
    const BACKOFFS_MS: [u64; 3] = [15, 40, 100];

    let mut err = match op() {
        Ok(()) => return Ok(()),
        Err(err) => err,
    };

    if allow_clear_readonly
        && err.kind() == io::ErrorKind::PermissionDenied
        && crate::windows::clear_readonly_if_not_reparse(path)
    {
        match op() {
            Ok(()) => return Ok(()),
            Err(next) => err = next,
        }
    }

    if !is_retryable_windows_error(&err, best_effort) {
        return Err(err);
    }

    for ms in BACKOFFS_MS {
        std::thread::sleep(Duration::from_millis(ms));
        match op() {
            Ok(()) => return Ok(()),
            Err(next) if next.kind() == io::ErrorKind::NotFound => {
                return Ok(());
            }
            Err(next) => {
                err = next;
                if !is_retryable_windows_error(&err, best_effort) {
                    return Err(err);
                }
            }
        }
    }
    Err(err)
}

#[cfg(windows)]
fn is_retryable_windows_error(err: &io::Error, best_effort: bool) -> bool {
    const ERROR_ACCESS_DENIED: i32 = 5;
    const ERROR_SHARING_VIOLATION: i32 = 32;
    const ERROR_LOCK_VIOLATION: i32 = 33;
    const ERROR_DIR_NOT_EMPTY: i32 = 145;
    const ERROR_DELETE_PENDING: i32 = 303;

    let Some(code) = err.raw_os_error() else {
        return false;
    };
    matches!(
        code,
        ERROR_SHARING_VIOLATION
            | ERROR_LOCK_VIOLATION
            | ERROR_DIR_NOT_EMPTY
            | ERROR_DELETE_PENDING
    ) || (!best_effort && code == ERROR_ACCESS_DENIED)
}

#[cfg(not(windows))]
fn remove_with_retry<F>(
    _path: &Path,
    _allow_clear_readonly: bool,
    _best_effort: bool,
    mut op: F,
) -> io::Result<()>
where
    F: FnMut() -> io::Result<()>,
{
    op()
}

#[cfg(test)]
mod tests {
    use tempfile::TempDir;

    use super::*;

    #[test]
    fn factory_builds_adapter_for_each_os() {
        let mac = RemovalAdapterFactory::create_for(RemovalOs::MacOs);
        assert_eq!(mac.os_kind(), RemovalOs::MacOs);

        let linux = RemovalAdapterFactory::create_for(RemovalOs::Linux);
        assert_eq!(linux.os_kind(), RemovalOs::Linux);

        let windows = RemovalAdapterFactory::create_for(RemovalOs::Windows);
        assert_eq!(windows.os_kind(), RemovalOs::Windows);

        let host = RemovalAdapterFactory::create();
        assert_eq!(host.os_kind(), RemovalAdapterFactory::current_os());

        let env = RemovalEnv::current();
        assert!(env.current_dir.is_some());
    }

    #[test]
    fn format_windows_extended_path_handles_drives_unc_slashes_and_long_paths()
    {
        assert_eq!(format_windows_extended_path(r"C:\"), r"\\?\C:\");
        assert_eq!(format_windows_extended_path("C:/"), r"\\?\C:\");
        assert_eq!(
            format_windows_extended_path(r"C:\Users\tobi\repo\node_modules\"),
            r"\\?\C:\Users\tobi\repo\node_modules"
        );
        assert_eq!(
            format_windows_extended_path("C:/a/./b/../c/"),
            r"\\?\C:\a\c"
        );
        assert_eq!(
            format_windows_extended_path(r"\\server\share"),
            r"\\?\UNC\server\share"
        );
        assert_eq!(
            format_windows_extended_path("//server/share/dir/./sub/../leaf/"),
            r"\\?\UNC\server\share\dir\leaf"
        );
        assert_eq!(
            format_windows_extended_path(r"\\?\C:\already\here"),
            r"\\?\C:\already\here"
        );
        assert_eq!(
            format_windows_extended_path(r"\\?\UNC\srv\sh\x"),
            r"\\?\UNC\srv\sh\x"
        );
        assert_eq!(
            format_windows_extended_path(r"\\.\pipe\ruum"),
            r"\\.\pipe\ruum"
        );
        assert_eq!(
            format_windows_extended_path("relative/path"),
            r"relative\path"
        );

        let deep = format!(
            r"C:\work\proj\node_modules\{}",
            vec!["nested_package_segment"; 20].join(r"\")
        );
        assert!(deep.len() > 300);
        let formatted = format_windows_extended_path(&deep);
        assert!(formatted.starts_with(r"\\?\C:\work\proj\node_modules\"));
        assert_eq!(formatted.len(), deep.len() + 4);
    }

    #[test]
    fn strip_windows_extended_prefix_preserves_volume_guid_and_long_paths() {
        assert_eq!(strip_windows_extended_prefix(r"\\?\C:\"), r"C:\");
        assert_eq!(
            strip_windows_extended_prefix(r"\\?\C:\Users\tobi"),
            r"C:\Users\tobi"
        );
        assert_eq!(
            strip_windows_extended_prefix(r"\\?\UNC\srv\share\a"),
            r"\\srv\share\a"
        );
        assert_eq!(
            strip_windows_extended_prefix(
                r"\\?\Volume{12345678-1234-1234-1234-1234567890ab}\"
            ),
            r"\\?\Volume{12345678-1234-1234-1234-1234567890ab}\"
        );

        let long_inner = format!(r"C:\{}", "a".repeat(265));
        let long_verbatim = format!(r"\\?\{long_inner}");
        assert_eq!(
            strip_windows_extended_prefix(&long_verbatim),
            long_verbatim
        );
    }

    #[test]
    fn classify_windows_trash_hresult_and_messages() {
        for code in [
            0x8007_00CE,
            0x8027_001D,
            0x8027_0035,
            0x8027_0036,
            0x8027_0037,
            0x8027_0038,
        ] {
            assert_eq!(
                classify_windows_trash_hresult(code),
                TrashErrorKind::LongPathOrRecycleLimit,
                "code {code:#010x}"
            );
        }
        for code in [
            0x8007_0020,
            0x8007_0021,
            0x8007_0005,
            0x8027_0027,
            0x8027_0028,
        ] {
            assert_eq!(
                classify_windows_trash_hresult(code),
                TrashErrorKind::LockedOrDenied,
                "code {code:#010x}"
            );
        }
        assert_eq!(
            classify_windows_trash_hresult(0x8000_4005),
            TrashErrorKind::Other
        );

        let win = RemovalAdapterFactory::create_for(RemovalOs::Windows);
        assert!(
            win.format_trash_error(
                TrashErrorKind::LongPathOrRecycleLimit,
                "0x80270038"
            )
            .contains("260")
        );
        assert!(
            win.format_trash_error(TrashErrorKind::LockedOrDenied, "locked")
                .contains("another process")
        );
        assert!(
            win.format_trash_error(TrashErrorKind::Other, "err")
                .contains("permanent deletion")
        );

        let mac = RemovalAdapterFactory::create_for(RemovalOs::MacOs);
        assert!(
            mac.format_trash_error(TrashErrorKind::Other, "err")
                .contains("Trash")
        );

        let linux = RemovalAdapterFactory::create_for(RemovalOs::Linux);
        assert!(
            linux
                .format_trash_error(TrashErrorKind::Other, "err")
                .contains("permanent deletion")
        );
    }

    #[test]
    fn windows_adapter_removes_nested_tree_and_enforces_root_and_symlink_guards()
     {
        let adapter = WindowsRemovalAdapter;
        let temp = TempDir::new().expect("tempdir");
        let root = temp.path().join("root");
        let nm = root.join("proj/node_modules/pkg/dist");
        fs::create_dir_all(&nm).expect("mkdir");
        fs::write(nm.join("index.js"), b"module.exports = 1;").expect("write");

        let outside_dir = temp.path().join("outside");
        fs::create_dir_all(&outside_dir).expect("mkdir outside");
        let outside_file = outside_dir.join("keep.txt");
        fs::write(&outside_file, b"keep").expect("write outside");

        // Refuses targets outside `root`.
        let err = adapter
            .remove_permanently(&outside_file, &root)
            .expect_err("outside root refused");
        assert!(err.to_string().contains("not inside"), "{err}");
        assert!(outside_file.exists());

        // Missing target reports NotFound.
        let missing = adapter
            .remove_permanently(&root.join("missing"), &root)
            .expect_err("missing");
        assert_eq!(missing.kind(), io::ErrorKind::NotFound);

        // Removes nested tree safely.
        adapter
            .remove_permanently(&root.join("proj/node_modules"), &root)
            .expect("remove node_modules");
        assert!(!root.join("proj/node_modules").exists());
        assert!(root.join("proj").exists());

        // Symlink inside tree is unlinked without touching its target; an
        // intermediate symlink directory strictly below `root` is refused.
        #[cfg(unix)]
        {
            use std::os::unix::fs::symlink;

            let link_in_tree = root.join("proj/link_out");
            symlink(&outside_dir, &link_in_tree).expect("symlink");
            adapter
                .remove_permanently(&link_in_tree, &root)
                .expect("unlink symlink only");
            assert!(fs::symlink_metadata(&link_in_tree).is_err());
            assert!(outside_file.exists());

            let mid_link = root.join("mid_link");
            symlink(&outside_dir, &mid_link).expect("mid symlink");
            let err = adapter
                .remove_permanently(&mid_link.join("keep.txt"), &root)
                .expect_err("intermediate link refused");
            assert!(err.to_string().contains("no longer a directory"), "{err}");
            assert!(outside_file.exists());

            // Link above `root` is allowed (follows user's chosen root alias).
            let alias_root = temp.path().join("alias_root");
            symlink(&root, &alias_root).expect("alias root");
            let victim = root.join("proj/victim.txt");
            fs::write(&victim, b"bye").expect("write victim");
            adapter
                .remove_permanently(
                    &alias_root.join("proj/victim.txt"),
                    &alias_root,
                )
                .expect("remove through link above root");
            assert!(!victim.exists());
        }
    }

    #[cfg(windows)]
    #[test]
    fn windows_adapter_removes_over_300_char_readonly_node_modules_and_preserves_link_target()
     {
        let adapter = WindowsRemovalAdapter;
        let temp = TempDir::new().expect("tempdir");
        let root = temp.path();

        let mut deep = root.join("node_modules");
        for i in 0..16 {
            deep.push(format!("very_long_package_segment_{i:02}"));
        }
        let deep_ext = to_windows_extended_path(&deep);
        fs::create_dir_all(&deep_ext).expect("create deep dir");
        let deep_file = deep_ext.join("readonly_bundle_artifact.js");
        fs::write(&deep_file, b"export default 42;").expect("write deep file");
        let mut perms = fs::metadata(&deep_file).expect("meta").permissions();
        perms.set_readonly(true);
        fs::set_permissions(&deep_file, perms).expect("set readonly");

        assert!(deep.as_os_str().to_string_lossy().len() > 300);
        adapter
            .remove_permanently(&root.join("node_modules"), root)
            .expect("remove deep readonly node_modules");
        assert!(fs::symlink_metadata(&deep_ext).is_err());

        // Junction inside marked dir: junction is unlinked and target stays
        // intact including its read-only flag.
        let target_dir = temp.path().join("target_keep");
        fs::create_dir_all(&target_dir).expect("mkdir target");
        let target_file = target_dir.join("keep.txt");
        fs::write(&target_file, b"keep").expect("write target");
        let mut tperms = fs::metadata(&target_file)
            .expect("target meta")
            .permissions();
        tperms.set_readonly(true);
        fs::set_permissions(&target_file, tperms).expect("target readonly");

        let marked = temp.path().join("marked");
        fs::create_dir_all(&marked).expect("mkdir marked");
        let junction = marked.join("link");
        if crate::windows::make_junction(&junction, &target_dir) {
            adapter
                .remove_permanently(&marked, temp.path())
                .expect("remove marked with junction");
            assert!(!marked.exists());
            assert!(target_file.exists());
            assert!(
                fs::metadata(&target_file)
                    .expect("still there")
                    .permissions()
                    .readonly()
            );
        }
    }
}
