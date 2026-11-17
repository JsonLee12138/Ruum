//! App notification identity helpers for `ruum-gui`.
//!
//! Background / headless OS notification delivery lives in
//! `ruum_core::notify`; this module retains only the bundle identity probe
//! needed by GPUI window notifications.

pub const RUUM_APP_ID: &str = "io.github.tobi.ruum";
pub const RUUM_APP_NAME: &str = "ruum";

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

#[cfg(test)]
mod tests {
    use super::*;

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
