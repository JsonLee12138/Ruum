//! Application UI and message localization (`En`, `Zh`, `Ja`, `Ko`).

pub use ruum_core::i18n::Locale;

/// All supported UI locales in canonical order.
#[cfg(test)]
pub const ALL_LOCALES: [Locale; 4] =
    [Locale::En, Locale::Zh, Locale::Ja, Locale::Ko];

pub mod common;
pub mod explore;
pub mod messages;
pub mod pages;
pub mod review;

/// Pick the localized static string for `locale`.
#[must_use]
pub const fn pick(
    locale: Locale,
    en: &'static str,
    zh: &'static str,
    ja: &'static str,
    ko: &'static str,
) -> &'static str {
    match locale {
        Locale::En => en,
        Locale::Zh => zh,
        Locale::Ja => ja,
        Locale::Ko => ko,
    }
}

/// Format an item count (`"1 item"` / `"N items"` in English).
#[must_use]
pub fn item_count(count: usize, locale: Locale) -> String {
    match locale {
        Locale::En => {
            if count == 1 {
                "1 item".to_owned()
            } else {
                format!("{count} items")
            }
        }
        Locale::Zh => format!("{count} \u{9879}"),
        Locale::Ja => format!("{count} \u{4ef6}"),
        Locale::Ko => format!("{count}\u{ac1c} \u{d56d}\u{baa9}"),
    }
}

/// Format an item count from a `u64`.
#[must_use]
pub fn item_count_u64(count: u64, locale: Locale) -> String {
    match locale {
        Locale::En => {
            if count == 1 {
                "1 item".to_owned()
            } else {
                format!("{count} items")
            }
        }
        Locale::Zh => format!("{count} \u{9879}"),
        Locale::Ja => format!("{count} \u{4ef6}"),
        Locale::Ko => format!("{count}\u{ac1c} \u{d56d}\u{baa9}"),
    }
}

/// Format a file count (`"1 file"` / `"N files"` in English).
#[must_use]
pub fn file_count(count: u64, locale: Locale) -> String {
    match locale {
        Locale::En => {
            if count == 1 {
                "1 file".to_owned()
            } else {
                format!("{count} files")
            }
        }
        Locale::Zh => format!("{count} \u{4e2a}\u{6587}\u{4ef6}"),
        Locale::Ja => {
            format!("{count} \u{500b}\u{306e}\u{30d5}\u{30a1}\u{30a4}\u{30eb}")
        }
        Locale::Ko => format!("\u{d30c}\u{c77c} {count}\u{ac1c}"),
    }
}

/// Format a directory count (`"1 dir"` / `"N dirs"` in English).
#[must_use]
pub fn dir_count(count: u64, locale: Locale) -> String {
    match locale {
        Locale::En => {
            if count == 1 {
                "1 dir".to_owned()
            } else {
                format!("{count} dirs")
            }
        }
        Locale::Zh => format!("{count} \u{4e2a}\u{76ee}\u{5f55}"),
        Locale::Ja => format!(
            "{count} \u{500b}\u{306e}\u{30c7}\u{30a3}\u{30ec}\u{30af}\u{30c8}\u{30ea}"
        ),
        Locale::Ko => {
            format!("\u{b514}\u{b809}\u{d130}\u{b9ac} {count}\u{ac1c}")
        }
    }
}

/// Format an entry count (`"1 entry"` / `"N entries"` in English).
#[must_use]
pub fn entry_count(count: u64, locale: Locale) -> String {
    match locale {
        Locale::En => {
            if count == 1 {
                "1 entry".to_owned()
            } else {
                format!("{count} entries")
            }
        }
        Locale::Zh => format!("{count} \u{6761}\u{76ee}"),
        Locale::Ja => format!("{count} \u{4ef6}"),
        Locale::Ko => format!("{count}\u{ac1c} \u{d56d}\u{baa9}"),
    }
}

/// Format a path count (`"1 path"` / `"N paths"` in English).
#[must_use]
pub fn path_count(count: usize, locale: Locale) -> String {
    match locale {
        Locale::En => {
            if count == 1 {
                "1 path".to_owned()
            } else {
                format!("{count} paths")
            }
        }
        Locale::Zh => format!("{count} \u{4e2a}\u{8def}\u{5f84}"),
        Locale::Ja => format!("{count} \u{4ef6}\u{306e}\u{30d1}\u{30b9}"),
        Locale::Ko => format!("\u{acbd}\u{b85c} {count}\u{ac1c}"),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::power::PowerEfficiency;
    use ruum_core::removal::Blocked;
    use std::path::PathBuf;

    #[test]
    fn plural_helpers_cover_singular_and_plural_across_locales() {
        assert_eq!(item_count(1, Locale::En), "1 item");
        assert_eq!(item_count(3, Locale::En), "3 items");
        assert_eq!(file_count(1, Locale::En), "1 file");
        assert_eq!(file_count(2, Locale::En), "2 files");
        assert_eq!(dir_count(1, Locale::En), "1 dir");
        assert_eq!(dir_count(4, Locale::En), "4 dirs");
        assert_eq!(entry_count(1, Locale::En), "1 entry");
        assert_eq!(entry_count(5, Locale::En), "5 entries");
        assert_eq!(path_count(1, Locale::En), "1 path");
        assert_eq!(path_count(2, Locale::En), "2 paths");

        for locale in [Locale::Zh, Locale::Ja, Locale::Ko] {
            assert!(!item_count(2, locale).is_empty());
            assert!(!item_count_u64(5, locale).is_empty());
            assert!(!file_count(3, locale).is_empty());
            assert!(!dir_count(2, locale).is_empty());
            assert!(!entry_count(9, locale).is_empty());
            assert!(!path_count(4, locale).is_empty());
            assert_ne!(item_count(2, locale), item_count(2, Locale::En));
        }
    }

    #[test]
    fn common_strings_differ_across_locales() {
        assert_eq!(
            common::tray_sources_and_drives(Locale::En),
            "Sources && Drives\u{2026}"
        );
        for locale in ALL_LOCALES {
            assert!(!common::menu_settings(locale).is_empty());
            assert!(!common::menu_quit_app(locale).is_empty());
            assert!(!common::tray_open_ruum(locale).is_empty());
            assert!(
                !common::power_name(PowerEfficiency::Miser, locale).is_empty()
            );
            assert!(!common::age_bucket_label(0, locale).is_empty());
            assert!(!common::ago(1000, 900, locale).is_empty());
        }
        assert_ne!(
            common::tray_sources_and_drives(Locale::Zh),
            common::tray_sources_and_drives(Locale::En)
        );
        assert_ne!(
            common::tray_sources_and_drives(Locale::Ja),
            common::tray_sources_and_drives(Locale::En)
        );
        assert_ne!(
            common::tray_sources_and_drives(Locale::Ko),
            common::tray_sources_and_drives(Locale::En)
        );
    }

    #[test]
    fn explore_review_pages_and_messages_are_localized() {
        let blocked = Blocked {
            path: PathBuf::from("/tmp/example"),
            reason: "the home directory cannot be removed".to_owned(),
        };
        for locale in ALL_LOCALES {
            assert!(!explore::idle_title(locale).is_empty());
            assert!(!explore::panel::selection_eyebrow(locale).is_empty());
            assert!(!explore::help::help_title(locale).is_empty());
            assert_eq!(explore::help::help_rows(locale).len(), 28);
            assert!(!review::review_title(locale).is_empty());
            assert!(
                !review::blocked_reason(&blocked.reason, locale).is_empty()
            );
            assert!(!pages::sources_title(locale).is_empty());
            assert!(!pages::watchdog_title(locale).is_empty());
            assert!(!pages::settings_title(locale).is_empty());
            assert!(!messages::title_choose_source(locale).is_empty());
            assert!(!messages::plus_more(3, locale).is_empty());
            assert!(!messages::nothing_to_copy(locale).is_empty());
            assert!(!common::menu_install_cli(locale).is_empty());
            assert!(!pages::cli_tool_title(locale).is_empty());
            assert!(!pages::cli_tool_desc(locale).is_empty());
            assert!(
                !pages::cli_status_installed("/usr/local/bin/ruum", locale)
                    .is_empty()
            );
            assert!(!pages::cli_status_not_installed(locale).is_empty());
            assert!(
                !pages::cli_not_in_path_hint("/opt/bin", locale).is_empty()
            );
            assert!(!pages::cli_install_btn(locale).is_empty());
            assert!(!pages::cli_uninstall_btn(locale).is_empty());
            assert!(!explore::host_tab_local(locale).is_empty());
            assert!(!explore::close_host_tab_tooltip(locale).is_empty());
            assert!(!pages::ssh_check_space_tooltip(locale).is_empty());
            assert!(!pages::ssh_ai_analyze_tooltip(locale).is_empty());
            assert!(!pages::settings_tab_general(locale).is_empty());
            assert!(!pages::settings_tab_watchdog(locale).is_empty());
            assert!(!pages::settings_tab_webhook(locale).is_empty());
            assert!(!pages::settings_tab_ai(locale).is_empty());
            assert!(!pages::settings_tab_cli(locale).is_empty());
            assert!(!pages::webhook_guide_title(locale).is_empty());
            assert!(!pages::webhook_guide_flow(locale).is_empty());
            assert!(
                !messages::ai_proposal_remote_title("prod", locale).is_empty()
            );
            assert!(!messages::webhook_saved("ops", locale).is_empty());
            assert!(!messages::webhook_deleted("ops", locale).is_empty());
            assert!(
                !messages::webhook_test_succeeded("ops", locale).is_empty()
            );
            assert!(
                !messages::ssh_check_space_failed("prod", "err", locale)
                    .is_empty()
            );
            assert!(
                !messages::ssh_ai_analyze_failed("prod", "err", locale)
                    .is_empty()
            );
            assert!(!pages::ai_auto_analyze(locale).is_empty());
            assert!(pages::enable_service(locale).contains("Windows"));
            assert!(!messages::invalid_watchdog_number(locale).is_empty());
            assert!(
                !messages::cli_installed("/usr/local/bin/ruum", locale)
                    .is_empty()
            );
            assert!(!messages::cli_uninstalled(locale).is_empty());
            assert!(!messages::cli_install_failed("err", locale).is_empty());
            assert!(!messages::cli_uninstall_failed("err", locale).is_empty());
            assert!(!pages::ssh_servers_title(locale).is_empty());
            assert!(!pages::add_ssh_server(locale).is_empty());
            assert!(!pages::connect_and_scan(locale).is_empty());
            assert!(!pages::auth_badge_private_key(locale).is_empty());
            assert!(!pages::auth_badge_password(locale).is_empty());
            assert!(!pages::remote_connect_title(locale).is_empty());
            assert!(
                !pages::remote_connect_desc("scan_directory", locale)
                    .is_empty()
            );
            assert!(!pages::remote_connect_approve(locale).is_empty());
            assert!(!pages::remote_connect_deny(locale).is_empty());
            assert!(!pages::general_settings_title(locale).is_empty());
            assert!(!pages::webhook_title(locale).is_empty());
            assert!(!messages::ssh_invalid_input("err", locale).is_empty());
            assert!(!pages::enable_webhook(locale).is_empty());
            assert!(!messages::title_confirm_remote_connect(locale).is_empty());
            assert!(!messages::cleanup_started_notif_title(locale).is_empty());
            assert!(
                !messages::cleanup_started_notif_body(2, 1_500_000, locale)
                    .is_empty()
            );
            assert!(!messages::ssh_profile_saved("prod", locale).is_empty());
            assert!(!messages::ssh_profile_deleted("prod", locale).is_empty());
            assert!(
                !messages::invalid_webhook_config("bad", locale).is_empty()
            );
        }
        for locale in [Locale::Zh, Locale::Ja, Locale::Ko] {
            assert_ne!(
                pages::ssh_servers_title(locale),
                pages::ssh_servers_title(Locale::En)
            );
            assert_ne!(
                pages::remote_connect_title(locale),
                pages::remote_connect_title(Locale::En)
            );
            assert_ne!(
                pages::webhook_title(locale),
                pages::webhook_title(Locale::En)
            );
            assert_ne!(
                pages::general_settings_title(locale),
                pages::general_settings_title(Locale::En)
            );
            assert_ne!(
                messages::title_confirm_remote_connect(locale),
                messages::title_confirm_remote_connect(Locale::En)
            );
            assert_ne!(
                common::menu_install_cli(locale),
                common::menu_install_cli(Locale::En)
            );
            assert_ne!(
                pages::cli_tool_title(locale),
                pages::cli_tool_title(Locale::En)
            );
            assert_ne!(
                pages::ai_auto_analyze(locale),
                pages::ai_auto_analyze(Locale::En)
            );
            assert_ne!(
                messages::invalid_watchdog_number(locale),
                messages::invalid_watchdog_number(Locale::En)
            );
            assert_ne!(
                explore::panel::selection_eyebrow(locale),
                explore::panel::selection_eyebrow(Locale::En)
            );
            assert_ne!(
                review::blocked_reason(&blocked.reason, locale),
                review::blocked_reason(&blocked.reason, Locale::En)
            );
            assert_ne!(
                pages::sources_title(locale),
                pages::sources_title(Locale::En)
            );
            assert_ne!(
                messages::plus_more(3, locale),
                messages::plus_more(3, Locale::En)
            );
            assert_ne!(
                messages::cleanup_started_notif_title(locale),
                messages::cleanup_started_notif_title(Locale::En)
            );
            assert_ne!(
                messages::cleanup_started_notif_body(2, 1_500_000, locale),
                messages::cleanup_started_notif_body(2, 1_500_000, Locale::En)
            );
        }
        assert!(
            messages::cleanup_started_notif_body(2, 1_500_000, Locale::Zh)
                .contains("正在后台清理")
        );
        assert!(
            messages::cleanup_started_notif_body(2, 1_500_000, Locale::Zh)
                .contains("请稍等")
        );
    }
}
