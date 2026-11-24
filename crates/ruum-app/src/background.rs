//! Tri-platform background presentation switching and macOS `SMAppService`
//! integration.
//!
//! Encapsulates OS-specific foreground/background lifecycle behind
//! [`BackgroundPresentationAdapter`] and [`BackgroundPresentationFactory`]:
//! - macOS: switches `NSApplicationActivationPolicy` between `Accessory` (no
//!   Dock / Cmd+Tab entry) and `Regular`, detects `SMAppService` login launches
//!   via `NSAppleEventManager`, and bridges `SMAppService::mainAppService()`
//! - Linux / Windows: OS naturally hides taskbar placeholders when all windows
//!   close while keeping the system tray (`ksni` / Win32) resident

use std::fmt;
use std::io;
use std::sync::Arc;

use gpui_kit::App;
use ruum_core::autostart::{
    AutostartAdapter, AutostartEnv, AutostartFactory, AutostartStatus,
    MacOsLoginItemBridge,
};

#[cfg(target_os = "macos")]
mod sm_app_service {
    #![allow(
        unsafe_code,
        reason = "objc2-service-management 0.3.2 marks zero-pointer \
                  SMAppService selectors as unsafe"
    )]

    use std::io;

    use objc2_service_management::{
        SMAppService, SMAppServiceStatus, kSMErrorAlreadyRegistered,
        kSMErrorJobNotFound, kSMErrorLaunchDeniedByUser,
    };
    use ruum_core::autostart::AutostartStatus;

    const fn map_status(status: SMAppServiceStatus) -> AutostartStatus {
        if status.0 == SMAppServiceStatus::Enabled.0 {
            AutostartStatus::Enabled
        } else if status.0 == SMAppServiceStatus::RequiresApproval.0 {
            AutostartStatus::RequiresApproval
        } else {
            AutostartStatus::Disabled
        }
    }

    pub(super) fn register_main_app() -> io::Result<AutostartStatus> {
        // SAFETY: `mainAppService` takes no pointer arguments and returns the
        // process-wide `SMAppService` singleton for the main app bundle.
        let service = unsafe { SMAppService::mainAppService() };
        // SAFETY: `registerAndReturnError` operates on the valid `SMAppService`
        // singleton and bridges Objective-C `NSError**` via `objc2`.
        match unsafe { service.registerAndReturnError() } {
            Ok(()) => Ok(status_main_app()),
            Err(err) => {
                let code = err.code();
                if code
                    == isize::try_from(kSMErrorAlreadyRegistered).unwrap_or(12)
                {
                    Ok(status_main_app())
                } else if code
                    == isize::try_from(kSMErrorLaunchDeniedByUser).unwrap_or(11)
                {
                    Ok(AutostartStatus::RequiresApproval)
                } else {
                    let msg = err.localizedDescription().to_string();
                    Err(io::Error::other(format!(
                        "SMAppService register failed ({code}): {msg}"
                    )))
                }
            }
        }
    }

    pub(super) fn unregister_main_app() -> io::Result<()> {
        // SAFETY: `mainAppService` takes no pointer arguments.
        let service = unsafe { SMAppService::mainAppService() };
        // SAFETY: `unregisterAndReturnError` operates on the valid
        // `SMAppService` singleton and bridges Objective-C `NSError**` safely.
        match unsafe { service.unregisterAndReturnError() } {
            Ok(()) => Ok(()),
            Err(err)
                if err.code()
                    == isize::try_from(kSMErrorJobNotFound).unwrap_or(6) =>
            {
                Ok(())
            }
            Err(err) => {
                let code = err.code();
                let msg = err.localizedDescription().to_string();
                Err(io::Error::other(format!(
                    "SMAppService unregister failed ({code}): {msg}"
                )))
            }
        }
    }

    pub(super) fn status_main_app() -> AutostartStatus {
        // SAFETY: `mainAppService` takes no pointer arguments.
        let service = unsafe { SMAppService::mainAppService() };
        // SAFETY: `status` is a read-only Objective-C enum property getter on
        // the `SMAppService` singleton.
        let raw = unsafe { service.status() };
        map_status(raw)
    }
}

/// Operating system family for foreground/background presentation switching.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum BackgroundOs {
    MacOs,
    Linux,
    Windows,
}

/// Adapter interface encapsulating OS-specific background presentation state.
pub trait BackgroundPresentationAdapter: Send + Sync + fmt::Debug {
    #[cfg(test)]
    fn os_kind(&self) -> BackgroundOs;
    fn enter_background(&self, cx: &mut App);
    fn enter_foreground(&self, cx: &mut App);
    fn launched_as_login_item(&self) -> bool;
}

/// macOS `NSApplicationActivationPolicy` + `NSAppleEventManager` adapter.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct MacOsBackgroundAdapter;

impl BackgroundPresentationAdapter for MacOsBackgroundAdapter {
    #[cfg(test)]
    fn os_kind(&self) -> BackgroundOs {
        BackgroundOs::MacOs
    }

    fn enter_background(&self, _cx: &mut App) {
        #[cfg(target_os = "macos")]
        if !cfg!(test)
            && let Some(mtm) = objc2::MainThreadMarker::new()
        {
            let app = objc2_app_kit::NSApplication::sharedApplication(mtm);
            let _ = app.setActivationPolicy(
                objc2_app_kit::NSApplicationActivationPolicy::Accessory,
            );
        }
    }

    fn enter_foreground(&self, cx: &mut App) {
        #[cfg(target_os = "macos")]
        if !cfg!(test)
            && let Some(mtm) = objc2::MainThreadMarker::new()
        {
            let app = objc2_app_kit::NSApplication::sharedApplication(mtm);
            let _ = app.setActivationPolicy(
                objc2_app_kit::NSApplicationActivationPolicy::Regular,
            );
        }
        cx.activate(true);
    }

    fn launched_as_login_item(&self) -> bool {
        #[cfg(target_os = "macos")]
        {
            // `keyAELaunchedAsLogInItem` ('lgit') from AppleEvents AERegistry.
            const KEY_AE_LAUNCHED_AS_LOGIN_ITEM: u32 = 0x6c67_6974;
            if cfg!(test) {
                return false;
            }
            let manager =
                objc2_foundation::NSAppleEventManager::sharedAppleEventManager(
                );
            let Some(event) = manager.currentAppleEvent() else {
                return false;
            };
            let Some(desc) =
                event.paramDescriptorForKeyword(KEY_AE_LAUNCHED_AS_LOGIN_ITEM)
            else {
                return false;
            };
            desc.booleanValue() != 0
        }
        #[cfg(not(target_os = "macos"))]
        {
            false
        }
    }
}

/// Linux tray-resident presentation adapter.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct LinuxBackgroundAdapter;

impl BackgroundPresentationAdapter for LinuxBackgroundAdapter {
    #[cfg(test)]
    fn os_kind(&self) -> BackgroundOs {
        BackgroundOs::Linux
    }

    fn enter_background(&self, _cx: &mut App) {}

    fn enter_foreground(&self, cx: &mut App) {
        cx.activate(true);
    }

    fn launched_as_login_item(&self) -> bool {
        false
    }
}

/// Windows tray-resident presentation adapter.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct WindowsBackgroundAdapter;

impl BackgroundPresentationAdapter for WindowsBackgroundAdapter {
    #[cfg(test)]
    fn os_kind(&self) -> BackgroundOs {
        BackgroundOs::Windows
    }

    fn enter_background(&self, _cx: &mut App) {}

    fn enter_foreground(&self, cx: &mut App) {
        cx.activate(true);
    }

    fn launched_as_login_item(&self) -> bool {
        false
    }
}

/// Factory selecting the [`BackgroundPresentationAdapter`] for the host or a
/// requested target OS.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct BackgroundPresentationFactory;

impl BackgroundPresentationFactory {
    /// Detect the current operating system family.
    #[must_use]
    pub const fn current_os() -> BackgroundOs {
        if cfg!(target_os = "macos") {
            BackgroundOs::MacOs
        } else if cfg!(windows) {
            BackgroundOs::Windows
        } else {
            BackgroundOs::Linux
        }
    }

    /// Instantiate the adapter for a specific target OS.
    #[must_use]
    pub fn create_for(
        os: BackgroundOs,
    ) -> Box<dyn BackgroundPresentationAdapter> {
        match os {
            BackgroundOs::MacOs => Box::new(MacOsBackgroundAdapter),
            BackgroundOs::Linux => Box::new(LinuxBackgroundAdapter),
            BackgroundOs::Windows => Box::new(WindowsBackgroundAdapter),
        }
    }

    /// Instantiate the adapter for the current host OS.
    #[must_use]
    pub fn create() -> Box<dyn BackgroundPresentationAdapter> {
        Self::create_for(Self::current_os())
    }
}

/// Production macOS `SMAppService` bridge injected into
/// [`ruum_core::autostart::MacOsAutostartAdapter`].
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct MacOsSmAppServiceBridge;

impl MacOsLoginItemBridge for MacOsSmAppServiceBridge {
    fn register(&self) -> io::Result<AutostartStatus> {
        #[cfg(target_os = "macos")]
        {
            sm_app_service::register_main_app()
        }
        #[cfg(not(target_os = "macos"))]
        {
            Err(io::Error::new(
                io::ErrorKind::Unsupported,
                "SMAppService is only available on macOS",
            ))
        }
    }

    fn unregister(&self) -> io::Result<()> {
        #[cfg(target_os = "macos")]
        {
            sm_app_service::unregister_main_app()
        }
        #[cfg(not(target_os = "macos"))]
        {
            Ok(())
        }
    }

    fn status(&self) -> io::Result<AutostartStatus> {
        #[cfg(target_os = "macos")]
        {
            Ok(sm_app_service::status_main_app())
        }
        #[cfg(not(target_os = "macos"))]
        {
            Ok(AutostartStatus::Disabled)
        }
    }
}

/// Capture the host [`AutostartEnv`] enriched with macOS bundle and OS major
/// version metadata from `NSBundle` / `NSProcessInfo`.
#[must_use]
pub fn current_autostart_env() -> AutostartEnv {
    #[allow(
        unused_mut,
        reason = "mutated only on macOS to populate NSProcessInfo & NSBundle"
    )]
    let mut env = AutostartEnv::current();
    #[cfg(target_os = "macos")]
    {
        let ver = objc2_foundation::NSProcessInfo::processInfo()
            .operatingSystemVersion();
        env.macos_major_version = u32::try_from(ver.majorVersion).ok();
        env.is_macos_app_bundle = objc2_foundation::NSBundle::mainBundle()
            .bundleIdentifier()
            .is_some();
    }
    env
}

/// Build the host [`AutostartAdapter`] with [`MacOsSmAppServiceBridge`] wired.
#[must_use]
pub fn autostart_adapter() -> Box<dyn AutostartAdapter> {
    AutostartFactory::create_with_macos_bridge(Some(Arc::new(
        MacOsSmAppServiceBridge,
    )))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn factory_creates_all_three_adapters_and_detects_non_bundle_test_env() {
        for (os, expected) in [
            (BackgroundOs::MacOs, BackgroundOs::MacOs),
            (BackgroundOs::Linux, BackgroundOs::Linux),
            (BackgroundOs::Windows, BackgroundOs::Windows),
        ] {
            let adapter = BackgroundPresentationFactory::create_for(os);
            assert_eq!(adapter.os_kind(), expected);
            assert!(!adapter.launched_as_login_item());
        }

        let env = current_autostart_env();
        assert!(!env.is_macos_app_bundle);
        if cfg!(target_os = "macos") {
            assert!(env.macos_major_version.is_some_and(|v| v >= 10));
        } else {
            assert_eq!(env.macos_major_version, None);
        }
    }

    #[gpui_kit::test]
    #[allow(
        clippy::needless_pass_by_ref_mut,
        reason = "gpui_kit::test passes &mut TestAppContext"
    )]
    fn all_background_adapters_switch_cleanly_in_gpui_context(
        cx: &mut gpui_kit::TestAppContext,
    ) {
        cx.update(|cx| {
            for os in [
                BackgroundOs::MacOs,
                BackgroundOs::Linux,
                BackgroundOs::Windows,
            ] {
                let adapter = BackgroundPresentationFactory::create_for(os);
                adapter.enter_background(cx);
                adapter.enter_foreground(cx);
            }
        });
    }
}
