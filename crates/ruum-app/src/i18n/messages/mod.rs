//! Localized messages, window titles, placeholders, picker errors,
//! OS notifications, and CLI help strings.

mod notices;

pub use notices::*;
use ruum_core::size::human_bytes;

use crate::i18n::{Locale, item_count, pick};

/// Title for the source-chooser window.
#[must_use]
pub const fn title_choose_source(locale: Locale) -> &'static str {
    pick(
        locale,
        "ruum \u{00b7} Choose a source",
        "ruum \u{00b7} \u{9009}\u{62e9}\u{6765}\u{6e90}",
        "ruum \u{00b7} \u{30bd}\u{30fc}\u{30b9}\u{3092}\u{9078}\u{629e}",
        "ruum \u{00b7} \u{c18c}\u{c2a4} \u{c120}\u{d0dd}",
    )
}

/// Title suffix for a remote read-only SSH session.
#[must_use]
pub fn title_remote_suffix(host: &str, locale: Locale) -> String {
    match locale {
        Locale::En => format!(" \u{00b7} {host} \u{00b7} read-only"),
        Locale::Zh => format!(" \u{00b7} {host} \u{00b7} 只读"),
        Locale::Ja => format!(" \u{00b7} {host} \u{00b7} 読み取り専用"),
        Locale::Ko => format!(" \u{00b7} {host} \u{00b7} 읽기 전용"),
    }
}

/// Device label for a remote volume whose space comes from scan totals.
#[must_use]
pub fn device_remote_read_only(host: &str, locale: Locale) -> String {
    match locale {
        Locale::En => {
            format!("{host} \u{00b7} read-only \u{00b7} space at scan")
        }
        Locale::Zh => format!("{host} \u{00b7} 只读 \u{00b7} 扫描时容量"),
        Locale::Ja => {
            format!("{host} \u{00b7} 読み取り専用 \u{00b7} スキャン時容量")
        }
        Locale::Ko => {
            format!("{host} \u{00b7} 읽기 전용 \u{00b7} 스캔 시 용량")
        }
    }
}

/// Button label in the native folder-picker prompt.
#[must_use]
pub const fn prompt_scan(locale: Locale) -> &'static str {
    pick(locale, "Scan", "扫描", "スキャン", "스캔")
}

/// Title for the standalone cleanup confirmation window.
#[must_use]
pub const fn title_confirm_cleanup(locale: Locale) -> &'static str {
    pick(
        locale,
        "ruum \u{00b7} Confirm Cleanup",
        "ruum \u{00b7} \u{786e}\u{8ba4}\u{6e05}\u{7406}",
        "ruum \u{00b7} \u{30af}\u{30ea}\u{30fc}\u{30f3}\u{30a2}\u{30c3}\u{30d7}\u{306e}\u{78ba}\u{8a8d}",
        "ruum \u{00b7} \u{c815}\u{b9ac} \u{d655}\u{c778}",
    )
}

/// Title for an AI cleanup proposal modal.
#[must_use]
pub const fn ai_proposal_title(locale: Locale) -> &'static str {
    pick(
        locale,
        "Local AI Cleanup Proposal",
        "\u{672c}\u{673a} AI \u{6e05}\u{7406}\u{5efa}\u{8bae}",
        "\u{30ed}\u{30fc}\u{30ab}\u{30eb} AI \u{30af}\u{30ea}\u{30fc}\u{30f3}\u{30a2}\u{30c3}\u{30d7}\u{63d0}\u{6848}",
        "\u{b85c}\u{ceec} AI \u{c815}\u{b9ac} \u{c81c}\u{c548}",
    )
}

/// Title for a remote SSH AI cleanup proposal modal.
#[must_use]
pub fn ai_proposal_remote_title(alias: &str, locale: Locale) -> String {
    match locale {
        Locale::En => format!("Remote AI Cleanup Proposal \u{00b7} {alias}"),
        Locale::Zh => format!(
            "\u{8fdc}\u{7a0b} AI \u{6e05}\u{7406}\u{5efa}\u{8bae} \u{00b7} {alias}"
        ),
        Locale::Ja => format!(
            "\u{30ea}\u{30e2}\u{30fc}\u{30c8} AI \u{30af}\u{30ea}\u{30fc}\u{30f3}\u{30a2}\u{30c3}\u{30d7}\u{63d0}\u{6848} \u{00b7} {alias}"
        ),
        Locale::Ko => format!(
            "\u{c6d0}\u{aca9} AI \u{c815}\u{b9ac} \u{c81c}\u{c548} \u{00b7} {alias}"
        ),
    }
}

/// Merged small-items tile label (`"+N more"`).
#[must_use]
pub fn plus_more(count: usize, locale: Locale) -> String {
    match locale {
        Locale::En => format!("+{count} more"),
        Locale::Zh => format!("+{count} \u{9879}\u{66f4}\u{591a}"),
        Locale::Ja => format!("\u{4ed6} +{count} \u{4ef6}"),
        Locale::Ko => format!("+{count}\u{ac1c} \u{b354} \u{c788}\u{c74c}"),
    }
}

/// Label for the synthetic home-directory volume row in Sources & Drives.
#[must_use]
pub const fn home_directory(locale: Locale) -> &'static str {
    pick(
        locale,
        "Home directory",
        "\u{4e3b}\u{76ee}\u{5f55}",
        "\u{30db}\u{30fc}\u{30e0}\u{30c7}\u{30a3}\u{30ec}\u{30af}\u{30c8}\u{30ea}",
        "\u{d648} \u{b514}\u{b809}\u{d130}\u{b9ac}",
    )
}

/// Placeholder for the directory path input in Sources & Drives.
#[must_use]
pub const fn placeholder_abs_path(locale: Locale) -> &'static str {
    pick(
        locale,
        "/absolute/path",
        "/\u{7edd}\u{5bf9}/\u{8def}\u{5f84}",
        "/\u{7d76}\u{5bfe}/\u{30d1}\u{30b9}",
        "/\u{c808}\u{b300}/\u{acbd}\u{b85c}",
    )
}

/// Placeholder for custom AI command path input in Watchdog settings.
#[must_use]
pub const fn placeholder_custom_cmd(locale: Locale) -> &'static str {
    pick(
        locale,
        "e.g. /usr/local/bin/my-ai",
        "\u{4f8b}\u{5982} /usr/local/bin/my-ai",
        "\u{4f8b}: /usr/local/bin/my-ai",
        "\u{c608}: /usr/local/bin/my-ai",
    )
}

/// Placeholder for custom AI extra arguments input in Watchdog settings.
#[must_use]
pub const fn placeholder_extra_args(locale: Locale) -> &'static str {
    pick(
        locale,
        "e.g. --timeout 60",
        "\u{4f8b}\u{5982} --timeout 60",
        "\u{4f8b}: --timeout 60",
        "\u{c608}: --timeout 60",
    )
}

/// Placeholder for optional AI model override input in Watchdog settings.
#[must_use]
pub const fn placeholder_model_override(locale: Locale) -> &'static str {
    pick(
        locale,
        "optional model override",
        "\u{53ef}\u{9009}\u{6a21}\u{578b}\u{8986}\u{76d6}",
        "\u{4efb}\u{610f}\u{306e}\u{30e2}\u{30c7}\u{30eb}\u{4e0a}\u{66f8}\u{304d}",
        "\u{c120}\u{d0dd}\u{c801} \u{baa8}\u{b378} \u{c7ac}\u{c815}\u{c758}",
    )
}

/// Picker error shown when a relative path is submitted.
#[must_use]
pub const fn picker_abs_path(locale: Locale) -> &'static str {
    pick(
        locale,
        "Enter an absolute directory path.",
        "\u{8bf7}\u{8f93}\u{5165}\u{7edd}\u{5bf9}\u{76ee}\u{5f55}\u{8def}\u{5f84}\u{3002}",
        "\u{7d76}\u{5bfe}\u{30c7}\u{30a3}\u{30ec}\u{30af}\u{30c8}\u{30ea}\u{30d1}\u{30b9}\u{3092}\u{5165}\u{529b}\u{3057}\u{3066}\u{304f}\u{3060}\u{3055}\u{3044}\u{3002}",
        "\u{c808}\u{b300} \u{b514}\u{b809}\u{d130}\u{b9ac} \u{acbd}\u{b85c}\u{b97c} \u{c785}\u{b825}\u{d558}\u{c138}\u{c694}.",
    )
}

/// Picker error shown when a local path cannot be opened as a directory.
#[must_use]
pub const fn picker_cannot_open_dir(locale: Locale) -> &'static str {
    pick(
        locale,
        "Cannot open that local directory.",
        "\u{65e0}\u{6cd5}\u{6253}\u{5f00}\u{8be5}\u{672c}\u{5730}\u{76ee}\u{5f55}\u{3002}",
        "\u{305d}\u{306e}\u{30ed}\u{30fc}\u{30ab}\u{30eb}\u{30c7}\u{30a3}\u{30ec}\u{30af}\u{30c8}\u{30ea}\u{3092}\u{958b}\u{3051}\u{307e}\u{305b}\u{3093}\u{3002}",
        "\u{d574}\u{b2f9} \u{b85c}\u{ceec} \u{b514}\u{b809}\u{d130}\u{b9ac}\u{b97c} \u{c5f4} \u{c218} \u{c5c6}\u{c2b5}\u{b2c8}\u{b2e4}.",
    )
}

/// Cleanup summary message after approving an AI proposal.
#[must_use]
pub fn cleaned_items_summary(
    count: usize,
    bytes: u64,
    locale: Locale,
) -> String {
    let items = item_count(count, locale);
    let freed = human_bytes(bytes);
    match locale {
        Locale::En => format!("Cleaned {items}, freeing {freed}"),
        Locale::Zh => format!(
            "\u{5df2}\u{6e05}\u{7406} {items}\u{ff0c}\u{91ca}\u{653e} {freed}"
        ),
        Locale::Ja => format!(
            "{items} \u{3092}\u{30af}\u{30ea}\u{30fc}\u{30f3}\u{30a2}\u{30c3}\u{30d7}\u{3057}\u{3001}{freed} \u{3092}\u{89e3}\u{653e}\u{3057}\u{307e}\u{3057}\u{305f}"
        ),
        Locale::Ko => format!(
            "{items} \u{c815}\u{b9ac} \u{c644}\u{b8cc}, {freed} \u{d655}\u{bcf4}"
        ),
    }
}

/// OS notification title when background cleanup begins after confirmation.
#[must_use]
pub const fn cleanup_started_notif_title(locale: Locale) -> &'static str {
    pick(
        locale,
        "ruum Cleaning in Background",
        "ruum \u{6b63}\u{5728}\u{540e}\u{53f0}\u{6e05}\u{7406}",
        "ruum \u{30d0}\u{30c3}\u{30af}\u{30b0}\u{30e9}\u{30a6}\u{30f3}\u{30c9}\u{3067}\u{30af}\u{30ea}\u{30fc}\u{30f3}\u{30a2}\u{30c3}\u{30d7}\u{4e2d}",
        "ruum \u{bc31}\u{adf8}\u{b77c}\u{c6b4}\u{b4dc} \u{c815}\u{b9ac} \u{c911}",
    )
}

/// OS notification and toast body when background cleanup begins.
#[must_use]
pub fn cleanup_started_notif_body(
    count: usize,
    bytes: u64,
    locale: Locale,
) -> String {
    let items = item_count(count, locale);
    let size = human_bytes(bytes);
    match locale {
        Locale::En => format!(
            "Cleaning {items} ({size}) in background, please wait\u{2026}"
        ),
        Locale::Zh => format!(
            "\u{6b63}\u{5728}\u{540e}\u{53f0}\u{6e05}\u{7406} {items}\u{ff08}{size}\u{ff09}\u{ff0c}\u{8bf7}\u{7a0d}\u{7b49}\u{2026}"
        ),
        Locale::Ja => format!(
            "{items}\u{ff08}{size}\u{ff09}\u{3092}\u{30d0}\u{30c3}\u{30af}\u{30b0}\u{30e9}\u{30a6}\u{30f3}\u{30c9}\u{3067}\u{30af}\u{30ea}\u{30fc}\u{30f3}\u{30a2}\u{30c3}\u{30d7}\u{4e2d}\u{3067}\u{3059}\u{3002}\u{3057}\u{3070}\u{3089}\u{304f}\u{304a}\u{5f85}\u{3061}\u{304f}\u{3060}\u{3055}\u{3044}\u{2026}"
        ),
        Locale::Ko => format!(
            "{items} ({size}) \u{bc31}\u{adf8}\u{b77c}\u{c6b4}\u{b4dc}\u{c5d0}\u{c11c} \u{c815}\u{b9ac} \u{c911}\u{c785}\u{b2c8}\u{b2e4}. \u{c7a0}\u{c2dc}\u{b9cc} \u{ae30}\u{b2e4}\u{b824} \u{c8fc}\u{c138}\u{c694}\u{2026}"
        ),
    }
}

/// OS notification title when standalone cleanup completes.
#[must_use]
pub const fn cleanup_complete_notif_title(locale: Locale) -> &'static str {
    pick(
        locale,
        "ruum Cleanup Complete",
        "ruum \u{6e05}\u{7406}\u{5b8c}\u{6210}",
        "ruum \u{30af}\u{30ea}\u{30fc}\u{30f3}\u{30a2}\u{30c3}\u{30d7}\u{5b8c}\u{4e86}",
        "ruum \u{c815}\u{b9ac} \u{c644}\u{b8cc}",
    )
}

/// Toast shown after triggering a tray disk-space check.
#[must_use]
pub fn disk_check_available(avail: u64, locale: Locale) -> String {
    let bytes = human_bytes(avail);
    match locale {
        Locale::En => format!("Disk check: {bytes} available"),
        Locale::Zh => format!(
            "\u{78c1}\u{76d8}\u{68c0}\u{67e5}\u{ff1a}\u{53ef}\u{7528} {bytes}"
        ),
        Locale::Ja => format!(
            "\u{30c7}\u{30a3}\u{30b9}\u{30af}\u{78ba}\u{8a8d}: {bytes} \u{5229}\u{7528}\u{53ef}\u{80fd}"
        ),
        Locale::Ko => format!(
            "\u{b514}\u{c2a4}\u{d06c} \u{d655}\u{c778}: {bytes} \u{c0ac}\u{c6a9} \u{ac00}\u{b2a5}"
        ),
    }
}

/// OS notification title when background AI analysis finishes with zero items.
#[must_use]
pub const fn ai_analysis_notif_title(locale: Locale) -> &'static str {
    pick(
        locale,
        "ruum AI Analysis",
        "ruum AI \u{5206}\u{6790}",
        "ruum AI \u{5206}\u{6790}",
        "ruum AI \u{bd84}\u{c11d}",
    )
}

/// OS notification body when background AI analysis finds nothing reclaimable.
#[must_use]
pub const fn ai_analysis_no_reclaimable(locale: Locale) -> &'static str {
    pick(
        locale,
        "No reclaimable files found.",
        "\u{672a}\u{53d1}\u{73b0}\u{53ef}\u{56de}\u{6536}\u{6587}\u{4ef6}\u{3002}",
        "\u{56de}\u{53ce}\u{53ef}\u{80fd}\u{306a}\u{30d5}\u{30a1}\u{30a4}\u{30eb}\u{306f}\u{898b}\u{3064}\u{304b}\u{308a}\u{307e}\u{305b}\u{3093}\u{3067}\u{3057}\u{305f}\u{3002}",
        "\u{d68c}\u{c218} \u{ac00}\u{b2a5}\u{d55c} \u{d30c}\u{c77c}\u{c744} \u{cc3e}\u{c9c0} \u{babb}\u{d588}\u{c2b5}\u{b2c8}\u{b2e4}.",
    )
}

/// OS notification title when background AI analysis fails.
#[must_use]
pub const fn ai_analysis_failed_notif_title(locale: Locale) -> &'static str {
    pick(
        locale,
        "ruum AI Analysis Failed",
        "ruum AI \u{5206}\u{6790}\u{5931}\u{8d25}",
        "ruum AI \u{5206}\u{6790}\u{5931}\u{6557}",
        "ruum AI \u{bd84}\u{c11d} \u{c2e4}\u{d328}",
    )
}
