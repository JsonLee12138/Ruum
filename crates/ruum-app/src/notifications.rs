//! Tri-platform headless and CLI desktop notification runtime.
//!
//! Used by `ruum watchdog` (`RunLoop` and `--once`) and `--notify-wait`
//! click-listener helper processes without initializing GPUI.

use std::ffi::OsString;
use std::io;
#[cfg(any(all(unix, not(target_os = "macos")), windows))]
use std::path::Path;
#[cfg(any(all(unix, not(target_os = "macos")), windows))]
use std::process::{Command, Stdio};

#[cfg(not(windows))]
use ruum_core::watchdog::send_fallback_os_notification;
#[cfg(any(all(unix, not(target_os = "macos")), windows))]
use ruum_core::watchdog::try_ipc_open_window;
#[cfg(any(all(unix, not(target_os = "macos")), windows, test))]
use ruum_core::watchdog::{NOTIF_TAG_AI_ANALYSIS, NOTIF_TAG_LOW_SPACE};
use ruum_core::watchdog::{NotificationAdapter, try_ipc_notification};

pub const RUUM_APP_ID: &str = "io.github.tobi.ruum";
pub const RUUM_APP_NAME: &str = "ruum";

#[cfg(any(all(unix, not(target_os = "macos")), windows))]
const NOTIFY_WAIT_TTL: std::time::Duration = std::time::Duration::from_mins(1);

/// Notification adapter for headless `ruum watchdog` execution.
///
/// Always tries delivering via local IPC to a running Ruum GUI instance first.
/// When no GUI is active, dispatches a native Ruum-attributed clickable
/// notification on macOS (`UNUserNotificationCenter`), Linux (`notify-rust`),
/// and Windows (`WinRT` `ToastNotification`), falling back to platform CLI
/// utilities if native delivery fails or lacks bundle identity.
#[derive(Clone, Copy, Debug, Default)]
pub struct DesktopNotificationAdapter {
    pub detached_once: bool,
}

impl NotificationAdapter for DesktopNotificationAdapter {
    fn notify_tagged(
        &self,
        tag: &str,
        title: &str,
        body: &str,
    ) -> io::Result<()> {
        if try_ipc_notification(tag, title, body) {
            return Ok(());
        }

        #[cfg(target_os = "macos")]
        {
            let _ = self.detached_once;
            if has_native_notification_identity() {
                return post_macos_notification(tag, title, body);
            }
            send_fallback_os_notification(title, body)
        }

        #[cfg(all(unix, not(target_os = "macos")))]
        {
            if post_linux_notification(tag, title, body, self.detached_once)
                .is_err()
            {
                return send_fallback_os_notification(title, body);
            }
            Ok(())
        }

        #[cfg(windows)]
        {
            post_windows_notification(tag, title, body, self.detached_once)
        }

        #[cfg(not(any(target_os = "macos", unix, windows)))]
        {
            let _ = (tag, self.detached_once);
            send_fallback_os_notification(title, body)
        }
    }
}

/// Returns whether the current process has native system notification
/// identity (on macOS, requires running inside an app bundle with a bundle
/// identifier; Linux and Windows always support attributed notifications).
#[must_use]
#[allow(
    clippy::missing_const_for_fn,
    reason = "macOS implementation inspects NSBundle::mainBundle at runtime"
)]
pub fn has_native_notification_identity() -> bool {
    #[cfg(target_os = "macos")]
    {
        objc2_foundation::NSBundle::mainBundle()
            .bundleIdentifier()
            .is_some()
    }
    #[cfg(not(target_os = "macos"))]
    {
        true
    }
}

/// Map a notification tag to the CLI `--open-screen` target value.
#[cfg(any(all(unix, not(target_os = "macos")), windows, test))]
#[must_use]
pub fn target_screen_arg_for_tag(tag: &str) -> Option<&'static str> {
    match tag {
        NOTIF_TAG_LOW_SPACE | NOTIF_TAG_AI_ANALYSIS => Some("watchdog"),
        _ => None,
    }
}

#[cfg(any(all(unix, not(target_os = "macos")), windows, test))]
#[must_use]
fn launch_args_for_tag(tag: &str) -> Vec<&'static str> {
    match target_screen_arg_for_tag(tag) {
        Some(screen) => vec!["--open-screen", screen],
        None => Vec::new(),
    }
}

/// Focus an already-running Ruum GUI via IPC or cold-launch `exe` onto the
/// screen associated with `tag`.
#[cfg(any(all(unix, not(target_os = "macos")), windows))]
pub fn activate_or_launch_ruum(tag: &str, exe: &Path) {
    if try_ipc_open_window(tag) {
        return;
    }
    let mut cmd = Command::new(exe);
    cmd.args(launch_args_for_tag(tag))
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null());
    let _ = cmd.spawn();
}

/// Parsed arguments for the headless `--notify-wait` click-listener subcommand.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct NotifyWaitArgs {
    pub tag: String,
    pub title: String,
    pub body: String,
}

/// Parse `--notify-wait <tag> <title> <body>` from CLI arguments (program name
/// already skipped).
pub fn parse_notify_wait_args<I, S>(args: I) -> Option<NotifyWaitArgs>
where
    I: IntoIterator<Item = S>,
    S: Into<OsString>,
{
    let mut iter = args.into_iter().map(Into::into);
    while let Some(arg) = iter.next() {
        if arg == "--notify-wait" {
            let tag = iter.next()?.into_string().ok()?;
            let title = iter.next()?.into_string().ok()?;
            let body = iter.next()?.into_string().ok()?;
            return Some(NotifyWaitArgs { tag, title, body });
        }
    }
    None
}

/// Run the headless `--notify-wait` click listener (Linux & Windows `--once`
/// helper) up to 60 seconds and exit cleanly.
#[allow(
    clippy::missing_const_for_fn,
    clippy::unnecessary_wraps,
    reason = "Linux and Windows branches perform runtime I/O"
)]
pub fn run_notify_wait_command(args: &NotifyWaitArgs) -> io::Result<()> {
    #[cfg(all(unix, not(target_os = "macos")))]
    {
        run_linux_notify_wait(args)
    }
    #[cfg(windows)]
    {
        run_windows_notify_wait(args)
    }
    #[cfg(not(any(all(unix, not(target_os = "macos")), windows)))]
    {
        let _ = args;
        Ok(())
    }
}

#[cfg(any(all(unix, not(target_os = "macos")), windows))]
fn spawn_detached_notify_wait(
    tag: &str,
    title: &str,
    body: &str,
) -> io::Result<()> {
    let exe = std::env::current_exe()?;
    Command::new(exe)
        .arg("--notify-wait")
        .arg(tag)
        .arg(title)
        .arg(body)
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .spawn()?;
    Ok(())
}

/// Escape XML text nodes for Windows Toast payloads.
#[cfg(any(windows, test))]
#[must_use]
fn escape_xml(input: &str) -> String {
    let mut out = String::with_capacity(input.len());
    for ch in input.chars() {
        match ch {
            '&' => out.push_str("&amp;"),
            '<' => out.push_str("&lt;"),
            '>' => out.push_str("&gt;"),
            '"' => out.push_str("&quot;"),
            '\'' => out.push_str("&apos;"),
            _ => out.push(ch),
        }
    }
    out
}

/// Build a `ToastGeneric` XML document payload for Windows notifications.
#[cfg(any(windows, test))]
#[must_use]
fn build_windows_toast_xml(title: &str, body: &str) -> String {
    format!(
        "<toast><visual><binding template=\"ToastGeneric\">\
         <text>{}</text><text>{}</text>\
         </binding></visual></toast>",
        escape_xml(title),
        escape_xml(body),
    )
}

#[cfg(any(windows, test))]
#[must_use]
fn windows_toast_tag_hash(tag: &str) -> String {
    use std::hash::{Hash as _, Hasher as _};
    let mut hasher = std::collections::hash_map::DefaultHasher::new();
    tag.hash(&mut hasher);
    format!("{:016x}", hasher.finish())
}

#[cfg(target_os = "macos")]
fn post_macos_notification(
    tag: &str,
    title: &str,
    body: &str,
) -> io::Result<()> {
    use std::sync::mpsc;
    use std::time::Duration;

    use block2::RcBlock;
    use objc2_foundation::{NSError, NSString};
    use objc2_user_notifications::{
        UNAuthorizationOptions, UNMutableNotificationContent,
        UNNotificationRequest, UNUserNotificationCenter,
    };

    let center = UNUserNotificationCenter::currentNotificationCenter();

    let (auth_tx, auth_rx) = mpsc::sync_channel(1);
    let auth_block = RcBlock::new(
        move |granted: objc2::runtime::Bool, err: *mut NSError| {
            let ok = err.is_null() && granted.as_bool();
            let _ = auth_tx.try_send(ok);
        },
    );
    center.requestAuthorizationWithOptions_completionHandler(
        UNAuthorizationOptions::Alert | UNAuthorizationOptions::Sound,
        &auth_block,
    );
    if !matches!(auth_rx.recv_timeout(Duration::from_secs(5)), Ok(true)) {
        return Ok(());
    }

    let content = UNMutableNotificationContent::new();
    content.setTitle(&NSString::from_str(title));
    content.setBody(&NSString::from_str(body));

    let request = UNNotificationRequest::requestWithIdentifier_content_trigger(
        &NSString::from_str(tag),
        &content,
        None,
    );

    let (send_tx, send_rx) = mpsc::sync_channel(1);
    let send_block = RcBlock::new(move |err: *mut NSError| {
        let _ = send_tx.try_send(err.is_null());
    });
    center.addNotificationRequest_withCompletionHandler(
        &request,
        Some(&send_block),
    );
    if matches!(send_rx.recv_timeout(Duration::from_secs(2)), Ok(false)) {
        return Err(io::Error::other(
            "UNUserNotificationCenter delivery failed",
        ));
    }
    Ok(())
}

#[cfg(all(unix, not(target_os = "macos")))]
fn build_linux_notification(
    title: &str,
    body: &str,
) -> notify_rust::Notification {
    let mut n = notify_rust::Notification::new();
    n.appname(RUUM_APP_NAME)
        .icon(RUUM_APP_NAME)
        .hint(notify_rust::Hint::DesktopEntry(RUUM_APP_NAME.to_owned()))
        .summary(title)
        .body(body)
        .action("default", "default");
    n.finalize()
}

#[cfg(all(unix, not(target_os = "macos")))]
fn post_linux_notification(
    tag: &str,
    title: &str,
    body: &str,
    detached_once: bool,
) -> io::Result<()> {
    if detached_once {
        return spawn_detached_notify_wait(tag, title, body);
    }
    let built = build_linux_notification(title, body);
    let handle = built.show().map_err(io::Error::other)?;
    let tag_owned = tag.to_owned();
    let _ = std::thread::Builder::new()
        .name("ruum-notify-wait".to_string())
        .spawn(move || {
            handle.wait_for_action(|action| {
                if action != "__closed"
                    && let Ok(exe) = std::env::current_exe()
                {
                    activate_or_launch_ruum(&tag_owned, &exe);
                }
            });
        });
    Ok(())
}

#[cfg(all(unix, not(target_os = "macos")))]
fn run_linux_notify_wait(args: &NotifyWaitArgs) -> io::Result<()> {
    use std::sync::mpsc;

    let built = build_linux_notification(&args.title, &args.body);
    let Ok(handle) = built.show() else {
        return send_fallback_os_notification(&args.title, &args.body);
    };
    let (tx, rx) = mpsc::sync_channel(1);
    let _ = std::thread::Builder::new()
        .name("ruum-notify-wait-once".to_string())
        .spawn(move || {
            handle.wait_for_action(|action| {
                let _ = tx.try_send(action != "__closed");
            });
        });

    if rx.recv_timeout(NOTIFY_WAIT_TTL) == Ok(true)
        && let Ok(exe) = std::env::current_exe()
    {
        activate_or_launch_ruum(&args.tag, &exe);
    }
    Ok(())
}

#[cfg(windows)]
mod win_toast {
    use std::collections::HashMap;
    use std::io;
    use std::sync::mpsc;
    use std::sync::{Mutex, OnceLock};

    use windows::Data::Xml::Dom::XmlDocument;
    use windows::Foundation::TypedEventHandler;
    use windows::UI::Notifications::{
        ToastDismissalReason, ToastDismissedEventArgs, ToastFailedEventArgs,
        ToastNotification, ToastNotificationManager, ToastNotifier,
    };
    use windows::core::{HSTRING, IInspectable};

    use super::{
        NOTIFY_WAIT_TTL, NotifyWaitArgs, RUUM_APP_ID, RUUM_APP_NAME,
        activate_or_launch_ruum, build_windows_toast_xml,
        spawn_detached_notify_wait, windows_toast_tag_hash,
    };

    const MAX_ACTIVE_TOASTS: usize = 16;

    static ACTIVE_TOASTS: OnceLock<Mutex<HashMap<String, ToastNotification>>> =
        OnceLock::new();

    #[derive(Clone, Copy)]
    enum ToastWaitEvent {
        Activated,
        Closed,
    }

    fn register_aumid() {
        let _ = windows_registry::CURRENT_USER
            .create(format!(r"Software\Classes\AppUserModelId\{RUUM_APP_ID}"))
            .and_then(|key| key.set_string("DisplayName", RUUM_APP_NAME));
    }

    fn create_toast(
        tag: &str,
        title: &str,
        body: &str,
    ) -> windows::core::Result<(ToastNotifier, ToastNotification)> {
        register_aumid();
        let xml = build_windows_toast_xml(title, body);
        let doc = XmlDocument::new()?;
        doc.LoadXml(&HSTRING::from(xml))?;
        let toast = ToastNotification::CreateToastNotification(&doc)?;
        toast.SetTag(&HSTRING::from(windows_toast_tag_hash(tag)))?;
        let notifier = ToastNotificationManager::CreateToastNotifierWithId(
            &HSTRING::from(RUUM_APP_ID),
        )?;
        Ok((notifier, toast))
    }

    pub(super) fn post(
        tag: &str,
        title: &str,
        body: &str,
        detached_once: bool,
    ) -> io::Result<()> {
        if detached_once {
            return spawn_detached_notify_wait(tag, title, body);
        }
        post_retained(tag, title, body).map_err(io::Error::other)
    }

    fn post_retained(
        tag: &str,
        title: &str,
        body: &str,
    ) -> windows::core::Result<()> {
        let (notifier, toast) = create_toast(tag, title, body)?;
        let tag_owned = tag.to_owned();
        toast.Activated(
            &TypedEventHandler::<ToastNotification, IInspectable>::new(
                move |_sender, _args| {
                    if let Ok(exe) = std::env::current_exe() {
                        activate_or_launch_ruum(&tag_owned, &exe);
                    }
                    Ok(())
                },
            ),
        )?;

        let previous = {
            let map_lock =
                ACTIVE_TOASTS.get_or_init(|| Mutex::new(HashMap::new()));
            let mut map = map_lock
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner);
            if map.len() >= MAX_ACTIVE_TOASTS
                && !map.contains_key(tag)
                && let Some(oldest_key) = map.keys().next().cloned()
            {
                map.remove(&oldest_key);
            }
            map.insert(tag.to_owned(), toast.clone())
        };
        if let Some(prev) = previous {
            let _ = notifier.Hide(&prev);
        }
        notifier.Show(&toast)
    }

    pub(super) fn run_wait(args: &NotifyWaitArgs) -> io::Result<()> {
        run_wait_inner(args).map_err(io::Error::other)
    }

    fn run_wait_inner(args: &NotifyWaitArgs) -> windows::core::Result<()> {
        let (notifier, toast) =
            create_toast(&args.tag, &args.title, &args.body)?;
        let (tx, rx) = mpsc::sync_channel(4);

        let tx_act = tx.clone();
        toast.Activated(
            &TypedEventHandler::<ToastNotification, IInspectable>::new(
                move |_sender, _args| {
                    let _ = tx_act.try_send(ToastWaitEvent::Activated);
                    Ok(())
                },
            ),
        )?;

        let tx_dis = tx.clone();
        toast.Dismissed(&TypedEventHandler::<
            ToastNotification,
            ToastDismissedEventArgs,
        >::new(
            move |_sender,
                  event_args: windows::core::Ref<
                '_,
                ToastDismissedEventArgs,
            >| {
                let timed_out = event_args
                    .as_ref()
                    .and_then(|a| a.Reason().ok())
                    .is_some_and(|r| r == ToastDismissalReason::TimedOut);
                if !timed_out {
                    let _ = tx_dis.try_send(ToastWaitEvent::Closed);
                }
                Ok(())
            },
        ))?;

        toast.Failed(&TypedEventHandler::<
            ToastNotification,
            ToastFailedEventArgs,
        >::new(move |_sender, _args| {
            let _ = tx.try_send(ToastWaitEvent::Closed);
            Ok(())
        }))?;

        notifier.Show(&toast)?;

        if matches!(
            rx.recv_timeout(NOTIFY_WAIT_TTL),
            Ok(ToastWaitEvent::Activated)
        ) && let Ok(exe) = std::env::current_exe()
        {
            activate_or_launch_ruum(&args.tag, &exe);
        }
        drop(toast);
        Ok(())
    }
}

#[cfg(windows)]
fn post_windows_notification(
    tag: &str,
    title: &str,
    body: &str,
    detached_once: bool,
) -> io::Result<()> {
    win_toast::post(tag, title, body, detached_once)
}

#[cfg(windows)]
fn run_windows_notify_wait(args: &NotifyWaitArgs) -> io::Result<()> {
    win_toast::run_wait(args)
}

#[cfg(test)]
mod tests {
    use super::*;
    use ruum_core::watchdog::NOTIF_TAG_CLEANUP_DONE;

    #[test]
    fn target_screen_arg_maps_watchdog_and_ai_tags() {
        assert_eq!(
            target_screen_arg_for_tag(NOTIF_TAG_LOW_SPACE),
            Some("watchdog")
        );
        assert_eq!(
            target_screen_arg_for_tag(NOTIF_TAG_AI_ANALYSIS),
            Some("watchdog")
        );
        assert_eq!(target_screen_arg_for_tag(NOTIF_TAG_CLEANUP_DONE), None);
        assert_eq!(target_screen_arg_for_tag("ruum.unknown"), None);

        assert_eq!(
            launch_args_for_tag(NOTIF_TAG_LOW_SPACE),
            vec!["--open-screen", "watchdog"]
        );
        assert!(launch_args_for_tag(NOTIF_TAG_CLEANUP_DONE).is_empty());
    }

    #[test]
    fn windows_toast_xml_escapes_special_characters() {
        let xml = build_windows_toast_xml(
            "Disk < 5% & \"Low\" 'Alert'",
            "Free: 1.2GB > 0 & <critical>",
        );
        assert!(xml.contains("template=\"ToastGeneric\""));
        assert!(xml.contains(
            "<text>Disk &lt; 5% &amp; &quot;Low&quot; &apos;Alert&apos;</text>"
        ));
        assert!(xml.contains(
            "<text>Free: 1.2GB &gt; 0 &amp; &lt;critical&gt;</text>"
        ));
    }

    #[test]
    fn windows_toast_tag_hash_stays_within_limit() {
        let long_tag = "ruum.very_long_notification_tag_".repeat(4);
        let hashed = windows_toast_tag_hash(&long_tag);
        assert_eq!(hashed.len(), 16);
        assert!(hashed.chars().all(|c| c.is_ascii_hexdigit()));
    }

    #[test]
    fn parse_notify_wait_args_handles_valid_and_incomplete_inputs() {
        let parsed = parse_notify_wait_args([
            "--notify-wait",
            "ruum.low_disk_space",
            "Low Disk Space",
            "Only 2 GB remaining",
        ])
        .unwrap();
        assert_eq!(
            parsed,
            NotifyWaitArgs {
                tag: "ruum.low_disk_space".to_string(),
                title: "Low Disk Space".to_string(),
                body: "Only 2 GB remaining".to_string(),
            }
        );

        assert_eq!(
            parse_notify_wait_args(["--notify-wait", "tag", "title"]),
            None
        );
        assert_eq!(parse_notify_wait_args(["--open-screen", "watchdog"]), None);
    }

    #[test]
    fn native_notification_identity_matches_bundle_presence() {
        let has_id = has_native_notification_identity();
        if cfg!(target_os = "macos") {
            // Bare `cargo test` binary has no Info.plist CFBundleIdentifier.
            assert!(!has_id);
        } else {
            assert!(has_id);
        }
    }
}
