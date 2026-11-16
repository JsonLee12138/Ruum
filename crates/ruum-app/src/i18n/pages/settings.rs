//! Settings screen and standalone cleanup confirmation copy.

use crate::i18n::{Locale, pick};

pub const fn settings_title(locale: Locale) -> &'static str {
    pick(
        locale,
        "Settings",
        "\u{8bbe}\u{7f6e}",
        "\u{8a2d}\u{5b9a}",
        "\u{c124}\u{c815}",
    )
}

pub const fn settings_subtitle(locale: Locale) -> &'static str {
    pick(
        locale,
        "Configure Watchdog threshold, background daemon service, and AI \
         assistant.",
        "\u{914d}\u{7f6e}\u{7a7a}\u{95f4}\u{5b88}\u{62a4}\u{9608}\u{503c}\u{3001}\u{540e}\u{53f0}\u{5b88}\u{62a4}\u{670d}\u{52a1}\u{4e0e} AI \u{52a9}\u{624b}\u{3002}",
        "\u{30a6}\u{30a9}\u{30c3}\u{30c1}\u{30c9}\u{30c3}\u{30b0}\u{306e}\u{3057}\u{304d}\u{3044}\u{5024}\u{3001}\u{30d0}\u{30c3}\u{30af}\u{30b0}\u{30e9}\u{30a6}\u{30f3}\u{30c9}\u{30b5}\u{30fc}\u{30d3}\u{30b9}\u{3001}AI \u{30a2}\u{30b7}\u{30b9}\u{30bf}\u{30f3}\u{30c8}\u{3092}\u{8a2d}\u{5b9a}\u{3057}\u{307e}\u{3059}\u{3002}",
        "\u{c6cc}\u{ce58}\u{b3c5} \u{c784}\u{acc4}\u{ac12}, \u{bc31}\u{adf8}\u{b77c}\u{c6b4}\u{b4dc} \u{b370}\u{baac} \u{c11c}\u{be44}\u{c2a4} \u{bc0f} AI \u{c5b4}\u{c2dc}\u{c2a4}\u{d134}\u{d2b8}\u{b97c} \u{ad6c}\u{c131}\u{d569}\u{b2c8}\u{b2e4}.",
    )
}

pub const fn watchdog_monitoring_title(locale: Locale) -> &'static str {
    pick(
        locale,
        "Watchdog Monitoring",
        "\u{7a7a}\u{95f4}\u{5b88}\u{62a4}\u{76d1}\u{63a7}",
        "\u{30a6}\u{30a9}\u{30c3}\u{30c1}\u{30c9}\u{30c3}\u{30b0}\u{76e3}\u{8996}",
        "\u{c6cc}\u{ce58}\u{b3c5} \u{baa8}\u{b2c8}\u{d130}\u{b9c1}",
    )
}

pub const fn enable_watchdog(locale: Locale) -> &'static str {
    pick(
        locale,
        "Enable Watchdog Free-Space Monitoring",
        "\u{542f}\u{7528}\u{7a7a}\u{95f4}\u{5b88}\u{62a4}\u{53ef}\u{7528}\u{7a7a}\u{95f4}\u{76d1}\u{63a7}",
        "\u{30a6}\u{30a9}\u{30c3}\u{30c1}\u{30c9}\u{30c3}\u{30b0}\u{306e}\u{7a7a}\u{304d}\u{5bb9}\u{91cf}\u{76e3}\u{8996}\u{3092}\u{6709}\u{52b9}\u{306b}\u{3059}\u{308b}",
        "\u{c6cc}\u{ce58}\u{b3c5} \u{c5ec}\u{c720} \u{acf5}\u{ac04} \u{baa8}\u{b2c8}\u{d130}\u{b9c1} \u{d65c}\u{c131}\u{d654}",
    )
}

pub const fn enable_service(locale: Locale) -> &'static str {
    pick(
        locale,
        "Run as OS Background Service at Login (launchd / systemd / Windows)",
        "\u{767b}\u{5f55}\u{65f6}\u{4f5c}\u{4e3a}\u{7cfb}\u{7edf}\u{540e}\u{53f0}\u{670d}\u{52a1}\u{8fd0}\u{884c} (launchd / systemd / Windows)",
        "\u{30ed}\u{30b0}\u{30a4}\u{30f3}\u{6642}\u{306b} OS \u{30d0}\u{30c3}\u{30af}\u{30b0}\u{30e9}\u{30a6}\u{30f3}\u{30c9}\u{30b5}\u{30fc}\u{30d3}\u{30b9}\u{3068}\u{3057}\u{3066}\u{5b9f}\u{884c} (launchd / systemd / Windows)",
        "\u{b85c}\u{adf8}\u{c778} \u{c2dc} OS \u{bc31}\u{adf8}\u{b77c}\u{c6b4}\u{b4dc} \u{c11c}\u{be44}\u{c2a4}\u{b85c} \u{c2e4}\u{d589} (launchd / systemd / Windows)",
    )
}

pub const fn ai_auto_analyze(locale: Locale) -> &'static str {
    pick(
        locale,
        "Automatically Run AI Analysis on Low Disk Space",
        "\u{78c1}\u{76d8}\u{7a7a}\u{95f4}\u{4e0d}\u{8db3}\u{65f6}\u{81ea}\u{52a8}\u{8fd0}\u{884c} AI \u{5206}\u{6790}",
        "\u{7a7a}\u{304d}\u{5bb9}\u{91cf}\u{4e0d}\u{8db3}\u{6642}\u{306b} AI \u{5206}\u{6790}\u{3092}\u{81ea}\u{52d5}\u{5b9f}\u{884c}\u{3059}\u{308b}",
        "\u{b514}\u{c2a4}\u{d06c} \u{acf5}\u{ac04} \u{bd80}\u{c871} \u{c2dc} AI \u{bd84}\u{c11d} \u{c790}\u{b3d9} \u{c2e4}\u{d589}",
    )
}

pub const fn cli_tool_title(locale: Locale) -> &'static str {
    pick(
        locale,
        "Command-Line Tool",
        "\u{547d}\u{4ee4}\u{884c}\u{5de5}\u{5177}",
        "\u{30b3}\u{30de}\u{30f3}\u{30c9}\u{30e9}\u{30a4}\u{30f3}\u{30c4}\u{30fc}\u{30eb}",
        "\u{ba85}\u{b839}\u{c904} \u{b3c4}\u{ad6c}",
    )
}

pub const fn cli_tool_desc(locale: Locale) -> &'static str {
    pick(
        locale,
        "Register the ruum command in your PATH for terminal usage, MCP, and \
         Watchdog.",
        "\u{5c06} ruum \u{547d}\u{4ee4}\u{6ce8}\u{518c}\u{5230} PATH\u{ff0c}\u{4ee5}\u{4fbf}\u{5728}\u{7ec8}\u{7aef}\u{3001}MCP \u{548c}\u{7a7a}\u{95f4}\u{5b88}\u{62a4}\u{4e2d}\u{4f7f}\u{7528}\u{3002}",
        "\u{30bf}\u{30fc}\u{30df}\u{30ca}\u{30eb}\u{3001}MCP\u{3001}\u{30a6}\u{30a9}\u{30c3}\u{30c1}\u{30c9}\u{30c3}\u{30b0}\u{3067}\u{5229}\u{7528}\u{3067}\u{304d}\u{308b}\u{3088}\u{3046} ruum \u{30b3}\u{30de}\u{30f3}\u{30c9}\u{3092} PATH \u{306b}\u{767b}\u{9332}\u{3057}\u{307e}\u{3059}\u{3002}",
        "\u{d130}\u{bbf8}\u{b110}, MCP \u{bc0f} \u{c6cc}\u{ce58}\u{b3c5}\u{c5d0}\u{c11c} \u{c0ac}\u{c6a9}\u{d560} \u{c218} \u{c788}\u{b3c4}\u{b85d} ruum \u{ba85}\u{b839}\u{c744} PATH\u{c5d0} \u{b4f1}\u{b85d}\u{d569}\u{b2c8}\u{b2e4}.",
    )
}

pub fn cli_status_installed(path: &str, locale: Locale) -> String {
    match locale {
        Locale::En => format!("Installed at {path}"),
        Locale::Zh => format!("\u{5df2}\u{5b89}\u{88c5}\u{4e8e} {path}"),
        Locale::Ja => {
            format!(
                "{path} \u{306b}\u{30a4}\u{30f3}\u{30b9}\u{30c8}\u{30fc}\u{30eb}\u{6e08}\u{307f}"
            )
        }
        Locale::Ko => format!("{path}\u{c5d0} \u{c124}\u{ce58}\u{b428}"),
    }
}

pub const fn cli_status_not_installed(locale: Locale) -> &'static str {
    pick(
        locale,
        "Not installed",
        "\u{672a}\u{5b89}\u{88c5}",
        "\u{672a}\u{30a4}\u{30f3}\u{30b9}\u{30c8}\u{30fc}\u{30eb}",
        "\u{c124}\u{ce58}\u{b418}\u{c9c0} \u{c54a}\u{c74c}",
    )
}

pub fn cli_not_in_path_hint(dir: &str, locale: Locale) -> String {
    match locale {
        Locale::En => format!("Directory {dir} is not currently in your PATH."),
        Locale::Zh => format!(
            "\u{76ee}\u{5f55} {dir} \u{5f53}\u{524d}\u{672a}\u{5305}\u{542b}\u{5728}\u{60a8}\u{7684} PATH \u{4e2d}\u{3002}"
        ),
        Locale::Ja => format!(
            "\u{30c7}\u{30a3}\u{30ec}\u{30af}\u{30c8}\u{30ea} {dir} \u{306f}\u{73fe}\u{5728} PATH \u{306b}\u{542b}\u{307e}\u{308c}\u{3066}\u{3044}\u{307e}\u{305b}\u{3093}\u{3002}"
        ),
        Locale::Ko => format!(
            "\u{b514}\u{b809}\u{d130}\u{b9ac} {dir}\u{c774}(\u{ac00}) \u{d604}\u{c7ac} PATH\u{c5d0} \u{c5c6}\u{c2b5}\u{b2c8}\u{b2e4}."
        ),
    }
}

pub const fn cli_install_btn(locale: Locale) -> &'static str {
    pick(
        locale,
        "Install CLI Command",
        "\u{5b89}\u{88c5} CLI \u{547d}\u{4ee4}",
        "CLI \u{30b3}\u{30de}\u{30f3}\u{30c9}\u{3092}\u{30a4}\u{30f3}\u{30b9}\u{30c8}\u{30fc}\u{30eb}",
        "CLI \u{ba85}\u{b839} \u{c124}\u{ce58}",
    )
}

pub const fn cli_uninstall_btn(locale: Locale) -> &'static str {
    pick(
        locale,
        "Uninstall CLI Command",
        "\u{5378}\u{8f7d} CLI \u{547d}\u{4ee4}",
        "CLI \u{30b3}\u{30de}\u{30f3}\u{30c9}\u{3092}\u{30a2}\u{30f3}\u{30a4}\u{30f3}\u{30b9}\u{30c8}\u{30fc}\u{30eb}",
        "CLI \u{ba85}\u{b839} \u{c81c}\u{ac70}",
    )
}

pub const fn cli_updating_btn(locale: Locale) -> &'static str {
    pick(
        locale,
        "Updating\u{2026}",
        "\u{66f4}\u{65b0}\u{4e2d}\u{2026}",
        "\u{66f4}\u{65b0}\u{4e2d}\u{2026}",
        "\u{c5c5}\u{b370}\u{c774}\u{d2b8} \u{c911}\u{2026}",
    )
}

pub const fn min_free_threshold(locale: Locale) -> &'static str {
    pick(
        locale,
        "Minimum Free Space Threshold (GB):",
        "\u{6700}\u{4f4e}\u{53ef}\u{7528}\u{7a7a}\u{95f4}\u{9608}\u{503c} (GB)\u{ff1a}",
        "\u{6700}\u{5c0f}\u{7a7a}\u{304d}\u{5bb9}\u{91cf}\u{3057}\u{304d}\u{3044}\u{5024} (GB):",
        "\u{cd5c}\u{c18c} \u{c5ec}\u{c720} \u{acf5}\u{ac04} \u{c784}\u{acc4}\u{ac12} (GB):",
    )
}

pub const fn poll_interval_minutes(locale: Locale) -> &'static str {
    pick(
        locale,
        "Check Interval (minutes):",
        "\u{68c0}\u{6d4b}\u{95f4}\u{9694}\u{ff08}\u{5206}\u{949f}\u{ff09}\u{ff1a}",
        "\u{30c1}\u{30a7}\u{30c3}\u{30af}\u{9593}\u{9694}\u{ff08}\u{5206}\u{ff09}:",
        "\u{d655}\u{c778} \u{ac04}\u{aca9}(\u{bd84}):",
    )
}

pub const fn cooldown_minutes(locale: Locale) -> &'static str {
    pick(
        locale,
        "Re-Alert Cooldown (min 5 minutes):",
        "\u{91cd}\u{590d}\u{63d0}\u{793a}\u{51b7}\u{5374}\u{65f6}\u{95f4}\u{ff08}\u{6700}\u{4f4e} 5 \u{5206}\u{949f}\u{ff09}\u{ff1a}",
        "\u{518d}\u{901a}\u{77e5}\u{30af}\u{30fc}\u{30eb}\u{30c0}\u{30a6}\u{30f3}\u{ff08}\u{6700}\u{5c0f} 5 \u{5206}\u{ff09}:",
        "\u{c7ac}\u{c54c}\u{b9bc} \u{cfe8}\u{b2e4}\u{c6b4}(\u{cd5c}\u{c18c} 5\u{bd84}):",
    )
}

pub const fn ai_assistant_cli_title(locale: Locale) -> &'static str {
    pick(
        locale,
        "AI Assistant CLI",
        "AI \u{52a9}\u{624b} CLI",
        "AI \u{30a2}\u{30b7}\u{30b9}\u{30bf}\u{30f3}\u{30c8} CLI",
        "AI \u{c5b4}\u{c2dc}\u{c2a4}\u{d134}\u{d2b8} CLI",
    )
}

pub const fn ai_assistant_cli_desc(locale: Locale) -> &'static str {
    pick(
        locale,
        "Select which AI CLI to invoke when free space drops below threshold.",
        "\u{9009}\u{62e9}\u{5f53}\u{53ef}\u{7528}\u{7a7a}\u{95f4}\u{4f4e}\u{4e8e}\u{9608}\u{503c}\u{65f6}\u{8c03}\u{7528}\u{7684} AI CLI\u{3002}",
        "\u{7a7a}\u{304d}\u{5bb9}\u{91cf}\u{304c}\u{3057}\u{304d}\u{3044}\u{5024}\u{3092}\u{4e0b}\u{56de}\u{3063}\u{305f}\u{3068}\u{304d}\u{306b}\u{547c}\u{3073}\u{51fa}\u{3059} AI CLI \u{3092}\u{9078}\u{629e}\u{3057}\u{307e}\u{3059}\u{3002}",
        "\u{c5ec}\u{c720} \u{acf5}\u{ac04}\u{c774} \u{c784}\u{acc4}\u{ac12} \u{bbf8}\u{b9cc}\u{c73c}\u{b85c} \u{b5a8}\u{c5b4}\u{c9c8} \u{b54c} \u{d638}\u{cd9c}\u{d560} AI CLI\u{b97c} \u{c120}\u{d0dd}\u{d558}\u{c138}\u{c694}.",
    )
}

pub const fn custom_executable(locale: Locale) -> &'static str {
    pick(
        locale,
        "Custom Executable Command / Path:",
        "\u{81ea}\u{5b9a}\u{4e49}\u{53ef}\u{6267}\u{884c}\u{547d}\u{4ee4} / \u{8def}\u{5f84}\u{ff1a}",
        "\u{30ab}\u{30b9}\u{30bf}\u{30e0}\u{5b9f}\u{884c}\u{30b3}\u{30de}\u{30f3}\u{30c9} / \u{30d1}\u{30b9}:",
        "\u{c0ac}\u{c6a9}\u{c790} \u{c9c0}\u{c815} \u{c2e4}\u{d589} \u{ba85}\u{b839} / \u{acbd}\u{b85c}:",
    )
}

pub const fn custom_extra_args(locale: Locale) -> &'static str {
    pick(
        locale,
        "Custom Extra Arguments:",
        "\u{81ea}\u{5b9a}\u{4e49}\u{9644}\u{52a0}\u{53c2}\u{6570}\u{ff1a}",
        "\u{30ab}\u{30b9}\u{30bf}\u{30e0}\u{8ffd}\u{52a0}\u{5f15}\u{6570}:",
        "\u{c0ac}\u{c6a9}\u{c790} \u{c9c0}\u{c815} \u{cd94}\u{ac00} \u{c778}\u{c218}:",
    )
}

pub const fn model_override(locale: Locale) -> &'static str {
    pick(
        locale,
        "Model Override (optional):",
        "\u{6307}\u{5b9a}\u{6a21}\u{578b}\u{ff08}\u{53ef}\u{9009}\u{ff09}\u{ff1a}",
        "\u{30e2}\u{30c7}\u{30eb}\u{6307}\u{5b9a}\u{ff08}\u{4efb}\u{610f}\u{ff09}:",
        "\u{baa8}\u{b378} \u{c7ac}\u{c815}\u{c758}(\u{c120}\u{d0dd} \u{c0ac}\u{d56d}):",
    )
}

pub const fn testing(locale: Locale) -> &'static str {
    pick(
        locale,
        "Testing\u{2026}",
        "\u{6d4b}\u{8bd5}\u{4e2d}\u{2026}",
        "\u{30c6}\u{30b9}\u{30c8}\u{4e2d}\u{2026}",
        "\u{d14c}\u{c2a4}\u{d2b8} \u{c911}\u{2026}",
    )
}

pub const fn test_cli_connection(locale: Locale) -> &'static str {
    pick(
        locale,
        "Test CLI Connection",
        "\u{6d4b}\u{8bd5} CLI \u{8fde}\u{63a5}",
        "CLI \u{63a5}\u{7d9a}\u{3092}\u{30c6}\u{30b9}\u{30c8}",
        "CLI \u{c5f0}\u{acb0} \u{d14c}\u{c2a4}\u{d2b8}",
    )
}

pub const fn fetching(locale: Locale) -> &'static str {
    pick(
        locale,
        "Fetching\u{2026}",
        "\u{83b7}\u{53d6}\u{4e2d}\u{2026}",
        "\u{53d6}\u{5f97}\u{4e2d}\u{2026}",
        "\u{ac00}\u{c838}\u{c624}\u{b294} \u{c911}\u{2026}",
    )
}

pub const fn fetch_models(locale: Locale) -> &'static str {
    pick(
        locale,
        "Fetch Models",
        "\u{83b7}\u{53d6}\u{6a21}\u{578b}\u{5217}\u{8868}",
        "\u{30e2}\u{30c7}\u{30eb}\u{4e00}\u{89a7}\u{3092}\u{53d6}\u{5f97}",
        "\u{baa8}\u{b378} \u{baa9}\u{b85d} \u{ac00}\u{c838}\u{c624}\u{ae30}",
    )
}

pub const fn benchmarking_speed_tooltip(locale: Locale) -> &'static str {
    pick(
        locale,
        "Benchmarking model speed\u{2026}",
        "\u{6b63}\u{5728}\u{6d4b}\u{8bd5}\u{6a21}\u{578b}\u{901f}\u{5ea6}\u{2026}",
        "\u{30e2}\u{30c7}\u{30eb}\u{901f}\u{5ea6}\u{3092}\u{8a08}\u{6e2c}\u{4e2d}\u{2026}",
        "\u{baa8}\u{b378} \u{c18d}\u{b3c4} \u{ce21}\u{c815} \u{c911}\u{2026}",
    )
}

pub const fn retest_speed_tooltip(locale: Locale) -> &'static str {
    pick(
        locale,
        "Click to re-test model speed",
        "\u{70b9}\u{51fb}\u{91cd}\u{65b0}\u{6d4b}\u{8bd5}\u{6a21}\u{578b}\u{901f}\u{5ea6}",
        "\u{30af}\u{30ea}\u{30c3}\u{30af}\u{3057}\u{3066}\u{901f}\u{5ea6}\u{3092}\u{518d}\u{30c6}\u{30b9}\u{30c8}",
        "\u{d074}\u{b9ad}\u{d558}\u{c5ec} \u{baa8}\u{b378} \u{c18d}\u{b3c4} \u{c7ac}\u{d14c}\u{c2a4}\u{d2b8}",
    )
}

pub const fn test_speed_tooltip(locale: Locale) -> &'static str {
    pick(
        locale,
        "Test model speed",
        "\u{6d4b}\u{8bd5}\u{6a21}\u{578b}\u{901f}\u{5ea6}",
        "\u{30e2}\u{30c7}\u{30eb}\u{901f}\u{5ea6}\u{3092}\u{30c6}\u{30b9}\u{30c8}",
        "\u{baa8}\u{b378} \u{c18d}\u{b3c4} \u{d14c}\u{c2a4}\u{d2b8}",
    )
}

pub fn speed_retry(ms: u128, locale: Locale) -> String {
    match locale {
        Locale::En => format!("retry ({ms} ms)"),
        Locale::Zh => format!("\u{91cd}\u{8bd5} ({ms} ms)"),
        Locale::Ja => format!("\u{518d}\u{8a66}\u{884c} ({ms} ms)"),
        Locale::Ko => format!("\u{c7ac}\u{c2dc}\u{b3c4} ({ms} ms)"),
    }
}

pub const fn selected(locale: Locale) -> &'static str {
    pick(
        locale,
        "Selected",
        "\u{5df2}\u{9009}\u{62e9}",
        "\u{9078}\u{629e}\u{4e2d}",
        "\u{c120}\u{d0dd}\u{b428}",
    )
}

pub const fn select(locale: Locale) -> &'static str {
    pick(
        locale,
        "Select",
        "\u{9009}\u{62e9}",
        "\u{9078}\u{629e}",
        "\u{c120}\u{d0dd}",
    )
}

pub fn available_models(count: usize, locale: Locale) -> String {
    match locale {
        Locale::En => format!("Available Models ({count}):"),
        Locale::Zh => {
            format!("\u{53ef}\u{7528}\u{6a21}\u{578b} ({count})\u{ff1a}")
        }
        Locale::Ja => format!(
            "\u{5229}\u{7528}\u{53ef}\u{80fd}\u{306a}\u{30e2}\u{30c7}\u{30eb} ({count}):"
        ),
        Locale::Ko => format!(
            "\u{c0ac}\u{c6a9} \u{ac00}\u{b2a5}\u{d55c} \u{baa8}\u{b378} ({count}):"
        ),
    }
}

pub const fn test_all_models_tooltip(locale: Locale) -> &'static str {
    pick(
        locale,
        "Test speed of all models",
        "\u{6d4b}\u{8bd5}\u{6240}\u{6709}\u{6a21}\u{578b}\u{901f}\u{5ea6}",
        "\u{3059}\u{3079}\u{3066}\u{306e}\u{30e2}\u{30c7}\u{30eb}\u{306e}\u{901f}\u{5ea6}\u{3092}\u{30c6}\u{30b9}\u{30c8}",
        "\u{baa8}\u{b4e0} \u{baa8}\u{b378} \u{c18d}\u{b3c4} \u{d14c}\u{c2a4}\u{d2b8}",
    )
}

pub const fn saving(locale: Locale) -> &'static str {
    pick(
        locale,
        "Saving\u{2026}",
        "\u{4fdd}\u{5b58}\u{4e2d}\u{2026}",
        "\u{4fdd}\u{5b58}\u{4e2d}\u{2026}",
        "\u{c800}\u{c7a5} \u{c911}\u{2026}",
    )
}

pub const fn save_settings(locale: Locale) -> &'static str {
    pick(
        locale,
        "Save Settings",
        "\u{4fdd}\u{5b58}\u{8bbe}\u{7f6e}",
        "\u{8a2d}\u{5b9a}\u{3092}\u{4fdd}\u{5b58}",
        "\u{c124}\u{c815} \u{c800}\u{c7a5}",
    )
}

pub const fn no_candidate_items(locale: Locale) -> &'static str {
    pick(
        locale,
        "No candidate items proposed for cleanup.",
        "\u{6ca1}\u{6709}\u{5efa}\u{8bae}\u{6e05}\u{7406}\u{7684}\u{5019}\u{9009}\u{9879}\u{3002}",
        "\u{30af}\u{30ea}\u{30fc}\u{30f3}\u{30a2}\u{30c3}\u{30d7}\u{5019}\u{88dc}\u{306e}\u{9805}\u{76ee}\u{306f}\u{3042}\u{308a}\u{307e}\u{305b}\u{3093}\u{3002}",
        "\u{c815}\u{b9ac} \u{d6c4}\u{bcf4}\u{b85c} \u{c81c}\u{c548}\u{b41c} \u{d56d}\u{baa9}\u{c774} \u{c5c6}\u{c2b5}\u{b2c8}\u{b2e4}.",
    )
}

pub const fn mode_trash_short(locale: Locale) -> &'static str {
    pick(
        locale,
        "Trash",
        "\u{5e9f}\u{7eb8}\u{7bd3}",
        "\u{30b4}\u{30df}\u{7bb1}",
        "\u{d734}\u{c9c0}\u{d1b5}",
    )
}

pub const fn mode_permanent_short(locale: Locale) -> &'static str {
    pick(
        locale,
        "Permanent",
        "\u{6c38}\u{4e45}\u{5220}\u{9664}",
        "\u{5b8c}\u{5168}\u{524a}\u{9664}",
        "\u{c601}\u{ad6c} \u{c0ad}\u{c81c}",
    )
}

pub fn confirm_summary(
    selected: usize,
    total: usize,
    bytes: &str,
    locale: Locale,
) -> String {
    match locale {
        Locale::En => {
            format!("Selected {selected} of {total} items \u{00b7} {bytes}")
        }
        Locale::Zh => format!(
            "\u{5df2}\u{9009} {selected} / {total} \u{9879} \u{00b7} {bytes}"
        ),
        Locale::Ja => {
            format!(
                "{total} \u{4ef6}\u{4e2d} {selected} \u{4ef6}\u{9078}\u{629e} \u{00b7} {bytes}"
            )
        }
        Locale::Ko => {
            format!(
                "{total}\u{ac1c} \u{c911} {selected}\u{ac1c} \u{c120}\u{d0dd} \u{00b7} {bytes}"
            )
        }
    }
}

pub const fn confirm_default_desc(locale: Locale) -> &'static str {
    pick(
        locale,
        "Review and uncheck any items you wish to keep.",
        "\u{8bf7}\u{5ba1}\u{6838}\u{5e76}\u{53d6}\u{6d88}\u{52fe}\u{9009}\u{60a8}\u{5e0c}\u{671b}\u{4fdd}\u{7559}\u{7684}\u{9879}\u{76ee}\u{3002}",
        "\u{6b8b}\u{3057}\u{305f}\u{3044}\u{9805}\u{76ee}\u{306e}\u{30c1}\u{30a7}\u{30c3}\u{30af}\u{3092}\u{5916}\u{3057}\u{3066}\u{78ba}\u{8a8d}\u{3057}\u{3066}\u{304f}\u{3060}\u{3055}\u{3044}\u{3002}",
        "\u{c720}\u{c9c0}\u{d558}\u{b824}\u{b294} \u{d56d}\u{baa9}\u{c758} \u{c120}\u{d0dd}\u{c744} \u{d574}\u{c81c}\u{d558}\u{c5ec} \u{ac80}\u{d1a0}\u{d558}\u{c138}\u{c694}.",
    )
}

pub const fn select_all(locale: Locale) -> &'static str {
    pick(
        locale,
        "Select All",
        "\u{5168}\u{9009}",
        "\u{3059}\u{3079}\u{3066}\u{9078}\u{629e}",
        "\u{baa8}\u{b450} \u{c120}\u{d0dd}",
    )
}

pub const fn select_none(locale: Locale) -> &'static str {
    pick(
        locale,
        "Select None",
        "\u{5168}\u{4e0d}\u{9009}",
        "\u{9078}\u{629e}\u{89e3}\u{9664}",
        "\u{c120}\u{d0dd} \u{d574}\u{c81c}",
    )
}

pub const fn cancel(locale: Locale) -> &'static str {
    pick(
        locale,
        "Cancel",
        "\u{53d6}\u{6d88}",
        "\u{30ad}\u{30e3}\u{30f3}\u{30bb}\u{30eb}",
        "\u{cde8}\u{c18c}",
    )
}

pub fn confirm_and_clean(selected: usize, locale: Locale) -> String {
    match locale {
        Locale::En => format!("Confirm & Clean ({selected})"),
        Locale::Zh => format!("\u{786e}\u{8ba4}\u{6e05}\u{7406} ({selected})"),
        Locale::Ja => format!(
            "\u{78ba}\u{8a8d}\u{3057}\u{3066}\u{524a}\u{9664} ({selected})"
        ),
        Locale::Ko => {
            format!("\u{d655}\u{c778} \u{bc0f} \u{c815}\u{b9ac} ({selected})")
        }
    }
}
