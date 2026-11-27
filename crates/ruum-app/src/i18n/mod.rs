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
        assert_eq!(common::menu_remote(Locale::En), "Remote");
        assert_eq!(common::tray_remote(Locale::En), "Remote");
        for locale in ALL_LOCALES {
            assert!(!common::menu_remote(locale).is_empty());
            assert!(!common::tray_remote(locale).is_empty());
            assert!(!common::menu_settings(locale).is_empty());
            assert!(!common::menu_quit_app(locale).is_empty());
            assert!(!common::tray_open_ruum(locale).is_empty());
            assert!(
                !common::power_name(PowerEfficiency::Miser, locale).is_empty()
            );
            assert!(!common::age_bucket_label(0, locale).is_empty());
            assert!(!common::ago(1000, 900, locale).is_empty());
        }
        for locale in [Locale::Zh, Locale::Ja, Locale::Ko] {
            assert_ne!(
                common::menu_remote(locale),
                common::menu_remote(Locale::En)
            );
            assert_ne!(
                common::tray_remote(locale),
                common::tray_remote(Locale::En)
            );
        }
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
            assert!(!pages::rail_remote(locale).is_empty());
            assert!(!pages::remote_title(locale).is_empty());
            assert!(!pages::remote_subtitle(locale).is_empty());
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
            assert!(
                !messages::ssh_ai_analyze_failed(
                    "prod",
                    "request timed out",
                    locale,
                )
                .is_empty()
            );
            assert!(!messages::ai_analysis_error("err", locale).is_empty());
            assert!(
                !messages::ai_analysis_failed_notif_title(locale).is_empty()
            );
            assert!(
                !messages::ai_analysis_timed_out_notif_title(locale).is_empty()
            );
            assert!(!pages::latest_ai_error_title(locale).is_empty());
            assert!(!pages::ai_auto_analyze(locale).is_empty());
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
            assert!(!pages::general_close_to_background(locale).is_empty());
            assert!(!pages::general_launch_at_login(locale).is_empty());
            assert!(!pages::general_tray_show_disk_space(locale).is_empty());
            assert!(!common::tray_toggle_watchdog(false, locale).is_empty());
            assert!(!common::tray_toggle_watchdog(true, locale).is_empty());
            assert_ne!(
                common::tray_toggle_watchdog(false, locale),
                common::tray_toggle_watchdog(true, locale)
            );
            assert!(
                !messages::settings_saved_autostart_requires_approval(locale)
                    .is_empty()
            );
            assert!(
                !messages::settings_saved_autostart_error("err", locale)
                    .is_empty()
            );
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
            assert!(!explore::crumb_switch_root_tooltip(locale).is_empty());
            assert!(!explore::switch_directory_tooltip(locale).is_empty());
            assert!(!explore::remote_docker_tooltip(locale).is_empty());
            assert!(!explore::ai_analyze_local_tooltip(locale).is_empty());
            assert!(!explore::ai_analyze_remote_tooltip(locale).is_empty());
            assert!(!explore::ai_analyze_running_tooltip(locale).is_empty());
            assert!(
                !explore::ai_analyze_disconnected_tooltip(locale).is_empty()
            );
            for stage in [
                ruum_core::AiProgressStage::Scanning,
                ruum_core::AiProgressStage::Queued,
                ruum_core::AiProgressStage::Connecting,
                ruum_core::AiProgressStage::Thinking,
                ruum_core::AiProgressStage::Generating,
            ] {
                assert!(
                    !explore::ai_stage_compact_label(stage, locale).is_empty()
                );
            }
            assert!(
                !explore::ai_stage_tooltip(
                    Some(ruum_core::AiProgressStage::Thinking),
                    locale
                )
                .is_empty()
            );
            assert!(!pages::docker_close_button(locale).is_empty());
            assert!(!pages::ssh_browse_dirs_tooltip(locale).is_empty());
            assert!(!pages::modal_mounts_local_title(locale).is_empty());
            assert!(!pages::modal_mounts_local_desc(locale).is_empty());
            assert!(
                !pages::modal_mounts_remote_title("prod", locale).is_empty()
            );
            assert!(!pages::modal_mounts_remote_desc(locale).is_empty());
            assert!(!pages::listing_remote_mounts(locale).is_empty());
            assert!(!pages::no_remote_mounts_listed(locale).is_empty());
            assert!(!messages::picker_remote_abs_path(locale).is_empty());
            assert!(!messages::placeholder_path_or_tilde(locale).is_empty());
            assert!(!review::docker_cleanup_section(locale).is_empty());
            assert!(!review::docker_group_build_cache(locale).is_empty());
            assert!(!review::docker_group_unused_images(locale).is_empty());
            assert!(
                !review::docker_group_stopped_containers(locale).is_empty()
            );
            assert!(!review::docker_build_cache_label(locale).is_empty());
            assert!(!review::docker_ai_recommended(locale).is_empty());
            assert!(
                !review::docker_permission_denied_hint(
                    "/var/run/docker.sock",
                    locale,
                )
                .is_empty()
            );
            assert!(!review::docker_select_all(locale).is_empty());
            assert!(!review::docker_select_none(locale).is_empty());
            assert!(!review::row_docker_items(locale).is_empty());
            assert!(
                !review::generate_remote_script_button(3, locale).is_empty()
            );
            assert!(!pages::script_generated_title(locale).is_empty());
            assert!(
                !pages::script_generated_desc(
                    "prod",
                    2,
                    "1.2 GB",
                    "/tmp/s.sh",
                    locale,
                )
                .is_empty()
            );
            assert!(!pages::script_run_in_terminal(locale).is_empty());
            assert!(!pages::script_reveal_file(locale).is_empty());
            assert!(!pages::script_run_later(locale).is_empty());
            assert!(
                !pages::ssh_pending_script_banner(3, "500 MB", locale)
                    .is_empty()
            );
            assert!(!pages::ssh_pending_run(locale).is_empty());
            assert!(!pages::ssh_pending_view(locale).is_empty());
            assert!(!pages::ssh_pending_discard(locale).is_empty());
            assert!(
                !messages::script_generate_failed("err", locale).is_empty()
            );
            assert!(
                !messages::script_launched_in_terminal("prod", locale)
                    .is_empty()
            );
            assert!(!messages::script_launch_failed("err", locale).is_empty());
            assert!(!messages::script_reveal_failed("err", locale).is_empty());
            assert!(!messages::script_discarded("prod", locale).is_empty());
            assert!(!pages::api_protocol_label(locale).is_empty());
            assert!(!pages::api_base_url_label(locale).is_empty());
            assert!(!pages::api_key_label(locale).is_empty());
            assert!(!pages::clear_api_key(locale).is_empty());
            assert!(!pages::api_key_keep_hint(locale).is_empty());
            assert!(!pages::api_key_empty_hint(locale).is_empty());
            assert!(!pages::test_api_connection(locale).is_empty());
            assert!(!messages::placeholder_api_key(locale).is_empty());
            assert!(
                !messages::testing_api_connection("OpenAI", locale).is_empty()
            );
            assert!(!messages::api_key_cleared(locale).is_empty());
            assert_eq!(
                ruum_core::watchdog_config::AiCliProvider::Api.label_in(locale),
                "API"
            );
            assert_eq!(
                ruum_core::watchdog_config::ApiProtocol::OpenAi
                    .label_in(locale),
                "OpenAI"
            );
            assert_eq!(
                ruum_core::watchdog_config::ApiProtocol::Anthropic
                    .label_in(locale),
                "Claude"
            );
        }
        for locale in [Locale::Zh, Locale::Ja, Locale::Ko] {
            assert_ne!(
                pages::latest_ai_error_title(locale),
                pages::latest_ai_error_title(Locale::En)
            );
            assert_ne!(
                messages::ai_analysis_timed_out_notif_title(locale),
                messages::ai_analysis_timed_out_notif_title(Locale::En)
            );
            assert_ne!(
                messages::ai_analysis_failed_notif_title(locale),
                messages::ai_analysis_failed_notif_title(Locale::En)
            );
            assert!(
                messages::ssh_ai_analyze_failed(
                    "prod",
                    &"x".repeat(220),
                    locale
                )
                .chars()
                .count()
                    <= 160
            );
            assert_ne!(
                pages::api_protocol_label(locale),
                pages::api_protocol_label(Locale::En)
            );
            assert_ne!(
                pages::api_base_url_label(locale),
                pages::api_base_url_label(Locale::En)
            );
            assert_ne!(
                pages::api_key_label(locale),
                pages::api_key_label(Locale::En)
            );
            assert_ne!(
                pages::clear_api_key(locale),
                pages::clear_api_key(Locale::En)
            );
            assert_ne!(
                pages::api_key_keep_hint(locale),
                pages::api_key_keep_hint(Locale::En)
            );
            assert_ne!(
                pages::test_api_connection(locale),
                pages::test_api_connection(Locale::En)
            );
            assert_ne!(
                messages::placeholder_api_key(locale),
                messages::placeholder_api_key(Locale::En)
            );
            assert_ne!(
                messages::testing_api_connection("OpenAI", locale),
                messages::testing_api_connection("OpenAI", Locale::En)
            );
            assert_ne!(
                messages::api_key_cleared(locale),
                messages::api_key_cleared(Locale::En)
            );
            assert_ne!(
                pages::script_generated_title(locale),
                pages::script_generated_title(Locale::En)
            );
            assert_ne!(
                pages::script_run_in_terminal(locale),
                pages::script_run_in_terminal(Locale::En)
            );
            assert_ne!(
                pages::ssh_pending_run(locale),
                pages::ssh_pending_run(Locale::En)
            );
            assert_ne!(
                review::docker_cleanup_section(locale),
                review::docker_cleanup_section(Locale::En)
            );
            assert_ne!(
                review::generate_remote_script_button(2, locale),
                review::generate_remote_script_button(2, Locale::En)
            );
            assert_ne!(
                pages::modal_mounts_local_title(locale),
                pages::modal_mounts_local_title(Locale::En)
            );
            assert_ne!(
                pages::modal_mounts_remote_title("prod", locale),
                pages::modal_mounts_remote_title("prod", Locale::En)
            );
            assert_ne!(
                explore::crumb_switch_root_tooltip(locale),
                explore::crumb_switch_root_tooltip(Locale::En)
            );
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
                pages::general_tray_show_disk_space(locale),
                pages::general_tray_show_disk_space(Locale::En)
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
                pages::rail_remote(locale),
                pages::rail_remote(Locale::En)
            );
            assert_ne!(
                pages::remote_title(locale),
                pages::remote_title(Locale::En)
            );
            assert_ne!(
                pages::remote_subtitle(locale),
                pages::remote_subtitle(Locale::En)
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
            assert_ne!(
                pages::rail_docker(locale),
                pages::rail_docker(Locale::En)
            );
            assert_ne!(
                pages::docker_refresh_button(locale),
                pages::docker_refresh_button(Locale::En)
            );
            assert_ne!(
                pages::docker_close_button(locale),
                pages::docker_close_button(Locale::En)
            );
            assert_ne!(
                explore::remote_docker_tooltip(locale),
                explore::remote_docker_tooltip(Locale::En)
            );
            assert_ne!(
                explore::ai_analyze_local_tooltip(locale),
                explore::ai_analyze_local_tooltip(Locale::En)
            );
            assert_ne!(
                explore::ai_analyze_remote_tooltip(locale),
                explore::ai_analyze_remote_tooltip(Locale::En)
            );
            assert_ne!(
                explore::ai_analyze_running_tooltip(locale),
                explore::ai_analyze_running_tooltip(Locale::En)
            );
            assert_ne!(
                explore::ai_analyze_disconnected_tooltip(locale),
                explore::ai_analyze_disconnected_tooltip(Locale::En)
            );
            for stage in [
                ruum_core::AiProgressStage::Scanning,
                ruum_core::AiProgressStage::Queued,
                ruum_core::AiProgressStage::Connecting,
                ruum_core::AiProgressStage::Thinking,
                ruum_core::AiProgressStage::Generating,
            ] {
                assert_ne!(
                    explore::ai_stage_compact_label(stage, locale),
                    explore::ai_stage_compact_label(stage, Locale::En)
                );
            }
            assert_ne!(
                pages::docker_not_installed_title(locale),
                pages::docker_not_installed_title(Locale::En)
            );
            assert_ne!(
                pages::docker_daemon_stopped_title(locale),
                pages::docker_daemon_stopped_title(Locale::En)
            );
            assert_ne!(
                pages::docker_permission_denied_title(locale),
                pages::docker_permission_denied_title(Locale::En)
            );
            assert_ne!(
                pages::docker_overview_title(locale),
                pages::docker_overview_title(Locale::En)
            );
            assert_ne!(
                pages::docker_card_images(locale),
                pages::docker_card_images(Locale::En)
            );
            assert_ne!(
                pages::docker_card_protected_badge(locale),
                pages::docker_card_protected_badge(Locale::En)
            );
            assert_ne!(
                pages::docker_card_shared_note("2 GB", locale),
                pages::docker_card_shared_note("2 GB", Locale::En)
            );
            assert_ne!(
                pages::docker_build_cache_detail("1 GB", Some("2 GB"), locale),
                pages::docker_build_cache_detail(
                    "1 GB",
                    Some("2 GB"),
                    Locale::En
                )
            );
            assert_ne!(
                pages::docker_volumes_notice_title(locale),
                pages::docker_volumes_notice_title(Locale::En)
            );
            assert_ne!(
                pages::docker_commit_button(2, locale),
                pages::docker_commit_button(2, Locale::En)
            );
            assert_ne!(
                pages::docker_stage_images(locale),
                pages::docker_stage_images(Locale::En)
            );
            assert!(!common::menu_docker(locale).is_empty());
            assert!(!pages::docker_page_title(locale).is_empty());
            assert!(!pages::docker_source_ssh("prod", locale).is_empty());
            assert!(!common::tray_check_updates(locale).is_empty());
            assert!(!common::tray_update_available("0.9.0", locale).is_empty());
            assert!(!pages::rail_update(locale).is_empty());
            assert!(!pages::rail_update_available("0.9.0", locale).is_empty());
            assert!(!pages::rail_update_downloading(locale).is_empty());
            assert!(!pages::rail_update_ready("0.9.0", locale).is_empty());
            assert!(!pages::settings_tab_updates(locale).is_empty());
            assert!(!pages::updates_title(locale).is_empty());
            assert!(!pages::updates_desc(locale).is_empty());
            assert!(!pages::updates_install_kind_label(locale).is_empty());
            assert!(!pages::updates_check_now(locale).is_empty());
            assert!(!pages::updates_auto_check(locale).is_empty());
            assert!(!pages::updates_channel_stable(locale).is_empty());
            assert!(!pages::updates_channel_beta(locale).is_empty());
            assert!(!pages::updates_send_client_id(locale).is_empty());
            assert!(!pages::update_verified_badge(locale).is_empty());

            assert!(!pages::updates_desc(locale).contains("signed"));
            assert!(!pages::updates_desc(locale).contains("\u{7b7e}\u{540d}"));
            assert!(!pages::updates_desc(locale).contains("\u{7f72}\u{540d}"));
            assert!(!pages::updates_desc(locale).contains("\u{c11c}\u{ba85}"));
            assert!(
                !pages::updates_install_kind_label(locale)
                    .contains("Install Form")
            );
            assert!(
                !pages::updates_install_kind_label(locale)
                    .contains("\u{5f62}\u{6001}")
            );
            assert!(
                !pages::updates_install_kind_label(locale)
                    .contains("\u{d615}\u{d0dc}")
            );
            assert!(!pages::updates_send_client_id(locale).contains("ID"));
            assert!(!pages::update_verified_badge(locale).contains("Ed25519"));
            assert!(!pages::update_verified_badge(locale).contains("SHA-256"));
            assert!(
                !pages::update_banner_available("0.9.0", locale).is_empty()
            );
            assert!(!pages::update_banner_ready("0.9.0", locale).is_empty());
            assert!(!pages::update_banner_view(locale).is_empty());
            assert!(!pages::update_modal_title(locale).is_empty());
            assert!(!pages::update_download_and_update(locale).is_empty());
            assert!(!pages::update_later(locale).is_empty());
            assert!(!pages::update_skip_version("0.9.0", locale).is_empty());
            assert!(!pages::update_copy_command(locale).is_empty());
            assert!(!pages::update_run_command(locale).is_empty());
            assert!(!pages::update_open_release(locale).is_empty());
            assert!(!pages::update_open_github_release(locale).is_empty());
            assert!(!pages::update_hide(locale).is_empty());
            assert!(!pages::update_cancel(locale).is_empty());
            assert!(!pages::update_restart_and_install(locale).is_empty());
            assert!(!pages::update_retry(locale).is_empty());
            assert!(!pages::update_restart_warning_marks(2, locale).is_empty());
            assert!(!pages::update_restart_warning_scan(locale).is_empty());
            assert!(!messages::update_command_copied(locale).is_empty());
            assert!(!messages::update_command_launched(locale).is_empty());
            assert!(
                !messages::update_command_launch_failed_copied(
                    "not found",
                    locale,
                )
                .is_empty()
            );
            assert!(!messages::refreshing_cloud_entitlement(locale).is_empty());
            assert!(!messages::cloud_entitlement_refreshed(locale).is_empty());
            assert!(
                !messages::cloud_no_available_models_blocked(locale).is_empty()
            );
            assert!(!messages::testing_cloud_connection(locale).is_empty());
            assert!(!messages::cloud_payment_succeeded(locale).is_empty());
            assert!(
                !messages::cloud_payment_succeeded_notif_title(locale)
                    .is_empty()
            );
            assert!(!messages::cloud_payment_still_pending(locale).is_empty());
            assert!(!pages::ai_setup_choice_title(locale).is_empty());
            assert!(!pages::ai_setup_choice_desc(locale).is_empty());
            assert!(!pages::ai_setup_choice_recommended(locale).is_empty());
            assert!(!pages::ai_setup_choice_cloud_title(locale).is_empty());
            assert!(!pages::ai_setup_choice_cloud_desc(locale).is_empty());
            assert!(!pages::ai_setup_choice_api_title(locale).is_empty());
            assert!(!pages::ai_setup_choice_api_desc(locale).is_empty());
            assert!(!pages::ai_setup_choice_cli_title(locale).is_empty());
            assert!(!pages::ai_setup_choice_cli_desc(locale).is_empty());
            assert!(!pages::cloud_card_title(locale).is_empty());
            assert!(!pages::cloud_card_desc(locale).is_empty());
            assert!(!pages::cloud_status_not_activated(locale).is_empty());
            assert!(!pages::cloud_status_trial(2, locale).is_empty());
            assert!(
                !pages::cloud_status_paid_until("2026-12-31", "alipay", locale)
                    .is_empty()
            );
            assert!(!pages::cloud_status_expired(locale).is_empty());
            for zone in ["green", "yellow", "red"] {
                let label = pages::cloud_fup_label(zone, locale);
                assert!(!label.is_empty());
                assert!(!label.to_ascii_lowercase().contains("fup"));
            }
            assert!(!pages::cloud_upgrade_pass(locale).is_empty());
            assert!(!pages::cloud_refresh_entitlement(locale).is_empty());
            assert!(!pages::cloud_test_connection(locale).is_empty());
            assert!(!pages::cloud_fetch_models_btn(locale).is_empty());
            assert!(!pages::cloud_model_auto_title(locale).is_empty());
            assert!(!pages::cloud_model_auto_desc(locale).is_empty());
            let models_title = pages::cloud_virtual_models_title(2, locale);
            assert!(!models_title.is_empty());
            assert!(!models_title.contains("Virtual Models"));
            assert!(!models_title.contains("\u{865a}\u{62df}\u{6a21}\u{578b}"));
            assert!(!pages::cloud_virtual_models_empty(locale).is_empty());
            assert!(!pages::cloud_model_locked_badge(locale).is_empty());
            assert!(!pages::cloud_billing_modal_title(locale).is_empty());
            assert!(!pages::cloud_billing_modal_desc(locale).is_empty());
            assert!(!pages::cloud_billing_loading_plans(locale).is_empty());
            assert!(
                !pages::cloud_billing_unavailable_notice(locale).is_empty()
            );
            assert!(!pages::cloud_billing_confirm(locale).is_empty());
            let shared_usage =
                pages::cloud_billing_usage_shared(25, 180, 70, locale);
            assert!(shared_usage.contains("25"));
            assert!(shared_usage.contains("180"));
            assert!(shared_usage.contains("70"));
            let plan_usage =
                pages::cloud_billing_usage_per_plan(40, 300, 75, locale);
            assert!(plan_usage.contains("40"));
            assert!(plan_usage.contains("300"));
            assert!(plan_usage.contains("75"));
            assert!(!pages::cloud_billing_support_btn(locale).is_empty());
            assert!(
                pages::cloud_billing_support_contact_hint(locale)
                    .contains("lijunsong2@gmail.com")
            );
            assert!(!pages::cloud_billing_select_plan(locale).is_empty());
            let creating = pages::cloud_billing_creating_order(locale);
            assert!(!creating.is_empty());
            assert!(creating.contains("10"));
            assert!(creating.contains("30"));
            for membership_copy in [
                pages::ai_setup_choice_cloud_desc(locale),
                pages::cloud_source_label("other", locale),
                pages::cloud_upgrade_pass(locale),
                pages::cloud_model_locked_badge(locale),
                pages::cloud_billing_modal_title(locale),
                pages::cloud_billing_modal_desc(locale),
                pages::cloud_billing_loading_plans(locale),
                pages::cloud_billing_select_plan(locale),
                messages::cloud_no_available_models_blocked(locale),
                messages::cloud_payment_succeeded(locale),
                messages::cloud_payment_succeeded_notif_title(locale),
            ] {
                assert!(!membership_copy.contains("\u{65f6}\u{957f}\u{5305}"));
                assert!(
                    !membership_copy
                        .to_ascii_lowercase()
                        .contains(concat!("duration ", "pass"))
                );
            }
            assert!(
                !pages::cloud_redeem_succeeded("2030-01-01", locale)
                    .contains("\u{65f6}\u{957f}\u{5305}")
            );
            assert!(!pages::cloud_billing_scan_qr_hint(locale).is_empty());
            assert!(
                !pages::cloud_billing_expiry_minutes(15, locale).is_empty()
            );
            assert!(!pages::cloud_billing_polling_status(locale).is_empty());
            assert!(!pages::cloud_billing_poll_timed_out(locale).is_empty());
            assert!(!pages::cloud_billing_mobile_only_hint(locale).is_empty());
            assert!(!pages::cloud_billing_open_browser(locale).is_empty());
            assert!(!pages::cloud_billing_refresh_now(locale).is_empty());
            assert!(!pages::cloud_billing_back_to_plans(locale).is_empty());
            assert!(!pages::cloud_billing_retry(locale).is_empty());
            assert!(
                !pages::cloud_billing_duration_label("month", locale)
                    .is_empty()
            );
            assert!(!pages::cloud_redeem_title(locale).is_empty());
            for text in [
                pages::cloud_redeem_title(locale),
                pages::cloud_billing_unavailable_notice(locale),
                pages::cloud_billing_modal_desc(locale),
            ] {
                assert!(
                    !text.contains("\u{6253}\u{8d4f}\u{5151}\u{6362}\u{7801}")
                );
                assert!(!text.to_ascii_lowercase().contains("supporter"));
            }
            assert!(!pages::cloud_redeem_submit(locale).is_empty());
            assert!(!pages::cloud_redeem_empty_hint(locale).is_empty());
            assert!(
                pages::cloud_redeem_succeeded("2030-01-01", locale)
                    .contains("2030-01-01")
            );
            assert!(
                !pages::cloud_redeem_succeeded_notif_title(locale).is_empty()
            );
            assert!(!messages::title_cleanup_progress(locale).is_empty());
            assert!(
                !messages::cleanup_cancelled_notif_title(locale).is_empty()
            );
            assert!(
                !messages::cleanup_cancelled_summary(2, 4096, locale)
                    .is_empty()
            );
            assert!(
                !messages::cleanup_docker_stage("containers", locale)
                    .is_empty()
            );
            assert!(
                !messages::cleanup_docker_stage("images", locale).is_empty()
            );
            assert!(
                !messages::cleanup_docker_stage("build_cache", locale)
                    .is_empty()
            );
            assert!(!common::tray_cleanup_progress_idle(locale).is_empty());
            assert!(
                !common::tray_cleanup_progress_active(3, 12, false, locale)
                    .is_empty()
            );
            assert!(
                !common::tray_cleanup_progress_active(3, 12, true, locale)
                    .is_empty()
            );
            assert!(!pages::cleanup_progress_heading(false, locale).is_empty());
            assert!(!pages::cleanup_progress_heading(true, locale).is_empty());
            assert!(
                !pages::cleanup_progress_subtitle(false, locale).is_empty()
            );
            assert!(!pages::cleanup_progress_subtitle(true, locale).is_empty());
            assert!(!pages::cleanup_progress_preparing(locale).is_empty());
            assert!(
                !pages::cleanup_reclaimed_so_far(
                    "1.2 GiB",
                    Some("4.0 GiB"),
                    locale
                )
                .is_empty()
            );
            assert!(!pages::cleanup_continue_in_background(locale).is_empty());
            assert!(!pages::cleanup_cancel_button(false, locale).is_empty());
            assert!(!pages::cleanup_cancel_button(true, locale).is_empty());
            assert!(
                !pages::cleanup_progress_done_heading(false, false, locale)
                    .is_empty()
            );
            assert!(
                !pages::cleanup_progress_done_heading(true, false, locale)
                    .is_empty()
            );
            assert!(
                !pages::cleanup_progress_done_heading(false, true, locale)
                    .is_empty()
            );
            assert!(
                !pages::cleanup_progress_auto_closing(3, locale).is_empty()
            );
            assert!(!pages::cleanup_progress_close_button(locale).is_empty());

            assert_ne!(
                pages::settings_tab_updates(locale),
                pages::settings_tab_updates(Locale::En)
            );
            assert_ne!(
                pages::update_modal_title(locale),
                pages::update_modal_title(Locale::En)
            );
            assert_ne!(
                pages::update_download_and_update(locale),
                pages::update_download_and_update(Locale::En)
            );
            assert_ne!(
                pages::update_restart_and_install(locale),
                pages::update_restart_and_install(Locale::En)
            );
            assert_ne!(
                pages::update_banner_view(locale),
                pages::update_banner_view(Locale::En)
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
        for locale in [Locale::Zh, Locale::Ja, Locale::Ko] {
            assert_ne!(
                messages::title_cleanup_progress(locale),
                messages::title_cleanup_progress(Locale::En)
            );
            assert_ne!(
                messages::cleanup_cancelled_notif_title(locale),
                messages::cleanup_cancelled_notif_title(Locale::En)
            );
            assert_ne!(
                messages::cleanup_cancelled_summary(1, 512, locale),
                messages::cleanup_cancelled_summary(1, 512, Locale::En)
            );
            assert_ne!(
                common::tray_cleanup_progress_idle(locale),
                common::tray_cleanup_progress_idle(Locale::En)
            );
            assert_ne!(
                common::tray_cleanup_progress_active(3, 12, false, locale),
                common::tray_cleanup_progress_active(3, 12, false, Locale::En)
            );
            assert_ne!(
                pages::cleanup_continue_in_background(locale),
                pages::cleanup_continue_in_background(Locale::En)
            );
            assert_ne!(
                pages::cleanup_cancel_button(false, locale),
                pages::cleanup_cancel_button(false, Locale::En)
            );
            assert_ne!(
                pages::cleanup_cancel_button(true, locale),
                pages::cleanup_cancel_button(true, Locale::En)
            );
            assert_ne!(
                pages::cleanup_progress_done_heading(false, false, locale),
                pages::cleanup_progress_done_heading(false, false, Locale::En)
            );
            assert_ne!(
                pages::cleanup_progress_done_heading(true, false, locale),
                pages::cleanup_progress_done_heading(true, false, Locale::En)
            );
            assert_ne!(
                pages::cleanup_progress_done_heading(false, true, locale),
                pages::cleanup_progress_done_heading(false, true, Locale::En)
            );
            assert_ne!(
                pages::cleanup_progress_auto_closing(3, locale),
                pages::cleanup_progress_auto_closing(3, Locale::En)
            );
            assert_ne!(
                pages::cleanup_progress_close_button(locale),
                pages::cleanup_progress_close_button(Locale::En)
            );
        }
    }

    #[test]
    fn friendly_update_error_localizes_categories_without_raw_codes() {
        let raw_samples = [
            "worker request timed out: Global",
            "SHA-256 mismatch: expected aaa, got bbb",
            "manifest Ed25519 signature verification failed: bad sig",
            "Permission denied (os error 13)",
            "No space left on device (os error 28)",
            "No release package matches this platform",
            "worker returned HTTP 503",
            "worker returned HTTP 404",
            "primary update service unavailable for beta channel (dns error)",
            "failed to extract zip archive",
            "unexpected internal error",
        ];

        for raw in raw_samples {
            let en = pages::friendly_update_error(raw, Locale::En);
            for locale in [Locale::En, Locale::Zh, Locale::Ja, Locale::Ko] {
                let text = pages::friendly_update_error(raw, locale);
                assert!(!text.is_empty());
                assert!(!text.contains("HTTP 503"));
                assert!(!text.contains("HTTP 404"));
                assert!(!text.contains("Ed25519"));
                assert!(!text.contains("SHA-256"));
                assert!(!text.contains("os error"));
                assert!(!text.contains("ureq"));
                if locale != Locale::En {
                    assert_ne!(text, en);
                }
            }
        }
    }

    #[test]
    fn cloud_and_settings_copy_avoids_fup_and_supporter_jargon() {
        assert!(!pages::ai_assistant_cli_desc(Locale::Zh).contains("AI CLI"));
        assert_eq!(
            pages::ai_assistant_cli_desc(Locale::Zh),
            "\u{9009}\u{62e9}\u{5f53}\u{53ef}\u{7528}\u{7a7a}\u{95f4}\u{4f4e}\u{4e8e}\u{9608}\u{503c}\u{65f6}\u{8c03}\u{7528}\u{7684} AI \u{5206}\u{6790}\u{3002}"
        );
        assert_eq!(
            pages::cloud_card_desc(Locale::Zh),
            "\u{5e73}\u{53f0}\u{81ea}\u{5e26}\u{4e91}\u{7aef}AI\u{7f51}\u{5173}"
        );
        assert_eq!(
            pages::cloud_fup_label("green", Locale::Zh),
            "\u{7528}\u{91cf}\u{6b63}\u{5e38}"
        );
        assert_eq!(
            pages::cloud_fup_label("yellow", Locale::Zh),
            "\u{7528}\u{91cf}\u{504f}\u{9ad8}\u{ff08}\u{8f7b}\u{91cf}\u{6a21}\u{5f0f}\u{ff09}"
        );
        assert_eq!(
            pages::cloud_fup_label("red", Locale::Zh),
            "\u{7528}\u{91cf}\u{5df2}\u{8fbe}\u{4e0a}\u{9650}"
        );
        for locale in [Locale::En, Locale::Zh, Locale::Ja, Locale::Ko] {
            assert!(!pages::ai_assistant_cli_desc(locale).contains("AI CLI"));
            assert!(!pages::cloud_card_desc(locale).is_empty());
            assert!(!pages::cloud_billing_confirm(locale).is_empty());
            assert!(!pages::cloud_billing_support_btn(locale).is_empty());
            assert!(
                pages::cloud_billing_support_contact_hint(locale)
                    .contains("lijunsong2@gmail.com")
            );
            assert!(!pages::cloud_fetch_models_btn(locale).is_empty());
            assert!(!pages::cloud_model_auto_title(locale).is_empty());
            assert!(!pages::cloud_model_auto_desc(locale).is_empty());
        }
    }
}
