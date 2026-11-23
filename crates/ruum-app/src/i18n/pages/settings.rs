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

pub const fn test_cli_connection(locale: Locale) -> &'static str {
    pick(
        locale,
        "Test CLI Connection",
        "\u{6d4b}\u{8bd5} CLI \u{8fde}\u{63a5}",
        "CLI \u{63a5}\u{7d9a}\u{3092}\u{30c6}\u{30b9}\u{30c8}",
        "CLI \u{c5f0}\u{acb0} \u{d14c}\u{c2a4}\u{d2b8}",
    )
}

pub const fn test_api_connection(locale: Locale) -> &'static str {
    pick(
        locale,
        "Test API Connection",
        "\u{6d4b}\u{8bd5} API \u{8fde}\u{63a5}",
        "API \u{63a5}\u{7d9a}\u{3092}\u{30c6}\u{30b9}\u{30c8}",
        "API \u{c5f0}\u{acb0} \u{d14c}\u{c2a4}\u{d2b8}",
    )
}

pub const fn api_protocol_label(locale: Locale) -> &'static str {
    pick(
        locale,
        "API Protocol:",
        "API \u{534f}\u{8bae}\u{ff1a}",
        "API \u{30d7}\u{30ed}\u{30c8}\u{30b3}\u{30eb}:",
        "API \u{d504}\u{b85c}\u{d1a0}\u{cf5c}:",
    )
}

pub const fn api_base_url_label(locale: Locale) -> &'static str {
    pick(
        locale,
        "Base URL:",
        "\u{63a5}\u{53e3}\u{5730}\u{5740}\u{ff08}Base URL\u{ff09}\u{ff1a}",
        "\u{30d9}\u{30fc}\u{30b9} URL:",
        "\u{ae30}\u{bcf8} URL:",
    )
}

pub const fn api_key_label(locale: Locale) -> &'static str {
    pick(
        locale,
        "API Key:",
        "API \u{5bc6}\u{94a5}\u{ff1a}",
        "API \u{30ad}\u{30fc}:",
        "API \u{d0a4}:",
    )
}

pub const fn clear_api_key(locale: Locale) -> &'static str {
    pick(
        locale,
        "Clear Key",
        "\u{6e05}\u{9664}\u{5bc6}\u{94a5}",
        "\u{30ad}\u{30fc}\u{3092}\u{524a}\u{9664}",
        "\u{d0a4} \u{c9c0}\u{c6b0}\u{ae30}",
    )
}

pub const fn api_key_keep_hint(locale: Locale) -> &'static str {
    pick(
        locale,
        "Leave empty to keep saved API key (or RUUM_AI_API_KEY).",
        "\u{7559}\u{7a7a}\u{5c06}\u{4fdd}\u{7559}\u{5df2}\u{4fdd}\u{5b58}\u{7684} API \u{5bc6}\u{94a5}\u{ff08}\u{6216}\u{4f7f}\u{7528} RUUM_AI_API_KEY\u{ff09}\u{3002}",
        "\u{7a7a}\u{6b04}\u{306e}\u{307e}\u{307e}\u{3067}\u{4fdd}\u{5b58}\u{6e08}\u{307f} API \u{30ad}\u{30fc}\u{ff08}\u{307e}\u{305f}\u{306f} RUUM_AI_API_KEY\u{ff09}\u{3092}\u{7dad}\u{6301}\u{3057}\u{307e}\u{3059}\u{3002}",
        "\u{be44}\u{c6cc} \u{b450}\u{ba74} \u{c800}\u{c7a5}\u{b41c} API \u{d0a4}(\u{b610}\u{b294} RUUM_AI_API_KEY)\u{b97c} \u{c720}\u{c9c0}\u{d569}\u{b2c8}\u{b2e4}.",
    )
}

pub const fn api_key_empty_hint(locale: Locale) -> &'static str {
    pick(
        locale,
        "Stored encrypted in credential store; leave empty to keep existing.",
        "\u{5c06}\u{52a0}\u{5bc6}\u{4fdd}\u{5b58}\u{5728}\u{7cfb}\u{7edf}\u{51ed}\u{636e}\u{5e93}\u{4e2d}\u{ff1b}\u{7559}\u{7a7a}\u{4fdd}\u{5b58}\u{4e0d}\u{4f1a}\u{8986}\u{76d6}\u{5df2}\u{6709}\u{5bc6}\u{94a5}\u{3002}",
        "\u{8a8d}\u{8a3c}\u{60c5}\u{5831}\u{30b9}\u{30c8}\u{30a2}\u{306b}\u{6697}\u{53f7}\u{5316}\u{3057}\u{3066}\u{4fdd}\u{5b58}\u{3055}\u{308c}\u{307e}\u{3059}\u{3002}\u{7a7a}\u{6b04}\u{3067}\u{4fdd}\u{5b58}\u{3059}\u{308b}\u{3068}\u{65e2}\u{5b58}\u{30ad}\u{30fc}\u{3092}\u{7dad}\u{6301}\u{3057}\u{307e}\u{3059}\u{3002}",
        "\u{c790}\u{aca9} \u{c99d}\u{ba85} \u{c800}\u{c7a5}\u{c18c}\u{c5d0} \u{c554}\u{d638}\u{d654}\u{b418}\u{c5b4} \u{c800}\u{c7a5}\u{b418}\u{ba70} \u{be44}\u{c6cc} \u{b450}\u{ba74} \u{ae30}\u{c874} \u{d0a4}\u{b97c} \u{c720}\u{c9c0}\u{d569}\u{b2c8}\u{b2e4}.",
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

// ── general settings (app_settings) ─────────────────────────────────────

pub const fn general_settings_title(locale: Locale) -> &'static str {
    pick(
        locale,
        "General",
        "\u{5e38}\u{89c4}\u{8bbe}\u{7f6e}",
        "\u{4e00}\u{822c}\u{8a2d}\u{5b9a}",
        "\u{c77c}\u{bc18} \u{c124}\u{c815}",
    )
}

pub const fn general_settings_desc(locale: Locale) -> &'static str {
    pick(
        locale,
        "Default scan worker preset, depth, appearance, and SSH lease.",
        "\u{9ed8}\u{8ba4}\u{626b}\u{63cf}\u{5e76}\u{53d1}\u{9884}\u{8bbe}\u{3001}\u{6df1}\u{5ea6}\u{3001}\u{5916}\u{89c2}\u{4e0e} SSH \u{6388}\u{6743}\u{79df}\u{671f}\u{3002}",
        "\u{30c7}\u{30d5}\u{30a9}\u{30eb}\u{30c8}\u{306e}\u{30b9}\u{30ad}\u{30e3}\u{30f3}\u{30d7}\u{30ea}\u{30bb}\u{30c3}\u{30c8}\u{3001}\u{6df1}\u{3055}\u{3001}\u{5916}\u{89b3}\u{3001}SSH \u{8a31}\u{53ef}\u{671f}\u{9593}\u{3092}\u{8a2d}\u{5b9a}\u{3057}\u{307e}\u{3059}\u{3002}",
        "\u{ae30}\u{bcf8} \u{c2a4}\u{ce94} \u{d504}\u{b9ac}\u{c14b}, \u{ae4a}\u{c774}, \u{baa8}\u{c591} \u{bc0f} SSH \u{c138}\u{c158} \u{c720}\u{d6a8} \u{ae30}\u{ac04}\u{c744} \u{c124}\u{c815}\u{d569}\u{b2c8}\u{b2e4}.",
    )
}

pub const fn general_power_preset(locale: Locale) -> &'static str {
    pick(
        locale,
        "Default Worker Preset:",
        "\u{9ed8}\u{8ba4}\u{5e76}\u{53d1}\u{9884}\u{8bbe}\u{ff1a}",
        "\u{30c7}\u{30d5}\u{30a9}\u{30eb}\u{30c8}\u{30ef}\u{30fc}\u{30ab}\u{30fc}\u{30d7}\u{30ea}\u{30bb}\u{30c3}\u{30c8}:",
        "\u{ae30}\u{bcf8} \u{c791}\u{c5c5}\u{c790} \u{d504}\u{b9ac}\u{c14b}:",
    )
}

pub const fn general_default_depth(locale: Locale) -> &'static str {
    pick(
        locale,
        "Default Treemap Depth (1-6):",
        "\u{9ed8}\u{8ba4}\u{6811}\u{56fe}\u{6df1}\u{5ea6}\u{ff08}1-6\u{ff09}\u{ff1a}",
        "\u{30c7}\u{30d5}\u{30a9}\u{30eb}\u{30c8}\u{306e}\u{30c4}\u{30ea}\u{30fc}\u{30de}\u{30c3}\u{30d7}\u{6df1}\u{5ea6} (1-6):",
        "\u{ae30}\u{bcf8} \u{d2b8}\u{b9ac}\u{b9f5} \u{ae4a}\u{c774} (1-6):",
    )
}

pub const fn general_apparent_size(locale: Locale) -> &'static str {
    pick(
        locale,
        "Measure Apparent File Size by Default",
        "\u{9ed8}\u{8ba4}\u{6309}\u{8868}\u{89c2}\u{6587}\u{4ef6}\u{5927}\u{5c0f}\u{7edf}\u{8ba1}",
        "\u{30c7}\u{30d5}\u{30a9}\u{30eb}\u{30c8}\u{3067}\u{898b}\u{304b}\u{3051}\u{306e}\u{30b5}\u{30a4}\u{30ba}\u{3092}\u{8a08}\u{6e2c}\u{3059}\u{308b}",
        "\u{ae30}\u{bcf8}\u{ac12}\u{c73c}\u{b85c} \u{d45c}\u{ba74} \u{d30c}\u{c77c} \u{d06c}\u{ae30} \u{ce21}\u{c815}",
    )
}

pub const fn general_include_hidden(locale: Locale) -> &'static str {
    pick(
        locale,
        "Include Hidden Entries by Default",
        "\u{9ed8}\u{8ba4}\u{5305}\u{542b}\u{9690}\u{85cf}\u{6587}\u{4ef6}\u{4e0e}\u{76ee}\u{5f55}",
        "\u{30c7}\u{30d5}\u{30a9}\u{30eb}\u{30c8}\u{3067}\u{96a0}\u{3057}\u{30d5}\u{30a1}\u{30a4}\u{30eb}\u{3092}\u{542b}\u{3081}\u{308b}",
        "\u{ae30}\u{bcf8}\u{ac12}\u{c73c}\u{b85c} \u{c228}\u{ae40} \u{d56d}\u{baa9} \u{d3ec}\u{d568}",
    )
}

pub const fn general_follow_appearance(locale: Locale) -> &'static str {
    pick(
        locale,
        "Follow System Light/Dark Appearance",
        "\u{8ddf}\u{968f}\u{7cfb}\u{7edf}\u{6d45}\u{8272}/\u{6df1}\u{8272}\u{5916}\u{89c2}",
        "\u{30b7}\u{30b9}\u{30c6}\u{30e0}\u{306e}\u{30e9}\u{30a4}\u{30c8}/\u{30c0}\u{30fc}\u{30af}\u{5916}\u{89b3}\u{306b}\u{5f93}\u{3046}",
        "\u{c2dc}\u{c2a4}\u{d15c} \u{b77c}\u{c774}\u{d2b8}/\u{b2e4}\u{d06c} \u{baa8}\u{c591} \u{b530}\u{b974}\u{ae30}",
    )
}

pub const fn general_ssh_lease_minutes(locale: Locale) -> &'static str {
    pick(
        locale,
        "SSH Session & MCP Approval Lease (minutes):",
        "SSH \u{4f1a}\u{8bdd}\u{4e0e} MCP \u{6388}\u{6743}\u{6709}\u{6548}\u{671f}\u{ff08}\u{5206}\u{949f}\u{ff09}\u{ff1a}",
        "SSH \u{30bb}\u{30c3}\u{30b7}\u{30e7}\u{30f3}\u{304a}\u{3088}\u{3073} MCP \u{8a31}\u{53ef}\u{671f}\u{9593}\u{ff08}\u{5206}\u{ff09}:",
        "SSH \u{c138}\u{c158} \u{bc0f} MCP \u{c2b9}\u{c778} \u{c720}\u{d6a8} \u{ae30}\u{ac04}(\u{bd84}):",
    )
}

// ── settings left-category sidebar ──────────────────────────────────────

pub const fn settings_tab_general(locale: Locale) -> &'static str {
    pick(
        locale,
        "General",
        "\u{5e38}\u{89c4}",
        "\u{4e00}\u{822c}",
        "\u{c77c}\u{bc18}",
    )
}

pub const fn settings_tab_watchdog(locale: Locale) -> &'static str {
    pick(
        locale,
        "Watchdog",
        "\u{7a7a}\u{95f4}\u{5b88}\u{62a4}",
        "\u{30a6}\u{30a9}\u{30c3}\u{30c1}\u{30c9}\u{30c3}\u{30b0}",
        "\u{c6cc}\u{ce58}\u{b3c5}",
    )
}

pub const fn settings_tab_webhook(locale: Locale) -> &'static str {
    pick(
        locale,
        "Webhooks",
        "Webhooks",
        "Webhooks",
        "\u{c6f9}\u{d6c5}",
    )
}

pub const fn settings_tab_ai(locale: Locale) -> &'static str {
    pick(locale, "AI", "AI \u{52a9}\u{624b}", "AI", "AI")
}

pub const fn settings_tab_cli(locale: Locale) -> &'static str {
    pick(
        locale,
        "CLI",
        "\u{547d}\u{4ee4}\u{884c}\u{5de5}\u{5177}",
        "CLI",
        "CLI",
    )
}

pub const fn settings_tab_updates(locale: Locale) -> &'static str {
    pick(
        locale,
        "Updates",
        "\u{66f4}\u{65b0}",
        "\u{30a2}\u{30c3}\u{30d7}\u{30c7}\u{30fc}\u{30c8}",
        "\u{c5c5}\u{b370}\u{c774}\u{d2b8}",
    )
}

// ── software updates settings & modal ───────────────────────────────────

pub const fn updates_title(locale: Locale) -> &'static str {
    pick(
        locale,
        "Software Updates",
        "\u{8f6f}\u{4ef6}\u{66f4}\u{65b0}",
        "\u{30bd}\u{30d5}\u{30c8}\u{30a6}\u{30a7}\u{30a2}\u{30a2}\u{30c3}\u{30d7}\u{30c7}\u{30fc}\u{30c8}",
        "\u{c18c}\u{d504}\u{d2b8}\u{c6e8}\u{c5b4} \u{c5c5}\u{b370}\u{c774}\u{d2b8}",
    )
}

pub const fn updates_desc(locale: Locale) -> &'static str {
    pick(
        locale,
        "Check for new versions, choose an update channel, and manage update \
         preferences.",
        "\u{68c0}\u{67e5}\u{65b0}\u{7248}\u{672c}\u{3001}\u{5207}\u{6362}\
         \u{66f4}\u{65b0}\u{901a}\u{9053}\u{5e76}\u{7ba1}\u{7406}\u{66f4}\
         \u{65b0}\u{504f}\u{597d}\u{8bbe}\u{7f6e}\u{3002}",
        "\u{65b0}\u{3057}\u{3044}\u{30d0}\u{30fc}\u{30b8}\u{30e7}\u{30f3}\
         \u{306e}\u{78ba}\u{8a8d}\u{3001}\u{30c1}\u{30e3}\u{30f3}\u{30cd}\
         \u{30eb}\u{5207}\u{308a}\u{66ff}\u{3048}\u{3001}\u{66f4}\u{65b0}\
         \u{8a2d}\u{5b9a}\u{3092}\u{7ba1}\u{7406}\u{3057}\u{307e}\u{3059}\
         \u{3002}",
        "\u{c0c8} \u{bc84}\u{c804} \u{d655}\u{c778}, \u{cc44}\u{b110} \
         \u{c804}\u{d658} \u{bc0f} \u{c5c5}\u{b370}\u{c774}\u{d2b8} \
         \u{ae30}\u{bcf8} \u{c124}\u{c815}\u{c744} \u{ad00}\u{b9ac}\u{d569}\
         \u{b2c8}\u{b2e4}.",
    )
}

pub const fn updates_current_version_label(locale: Locale) -> &'static str {
    pick(
        locale,
        "Current Version:",
        "\u{5f53}\u{524d}\u{7248}\u{672c}\u{ff1a}",
        "\u{73fe}\u{5728}\u{306e}\u{30d0}\u{30fc}\u{30b8}\u{30e7}\u{30f3}:",
        "\u{d604}\u{c7ac} \u{bc84}\u{c804}:",
    )
}

pub const fn updates_install_kind_label(locale: Locale) -> &'static str {
    pick(
        locale,
        "Installation:",
        "\u{5b89}\u{88c5}\u{65b9}\u{5f0f}\u{ff1a}",
        "\u{30a4}\u{30f3}\u{30b9}\u{30c8}\u{30fc}\u{30eb}\u{5f62}\u{5f0f}:",
        "\u{c124}\u{ce58} \u{bc29}\u{c2dd}:",
    )
}

pub const fn updates_last_checked_label(locale: Locale) -> &'static str {
    pick(
        locale,
        "Last Checked:",
        "\u{4e0a}\u{6b21}\u{68c0}\u{67e5}\u{ff1a}",
        "\u{6700}\u{7d42}\u{78ba}\u{8a8d}:",
        "\u{b9c8}\u{c9c0}\u{b9c9} \u{d655}\u{c778}:",
    )
}

pub const fn updates_last_checked_never(locale: Locale) -> &'static str {
    pick(
        locale,
        "Never",
        "\u{4ece}\u{672a}\u{68c0}\u{67e5}",
        "\u{672a}\u{78ba}\u{8a8d}",
        "\u{d655}\u{c778} \u{ae30}\u{b85d} \u{c5c6}\u{c74c}",
    )
}

pub const fn install_kind_label(
    kind: ruum_core::updater::InstallKind,
    locale: Locale,
) -> &'static str {
    use ruum_core::updater::InstallKind;

    match kind {
        InstallKind::MacAppBundle => pick(
            locale,
            "macOS App Bundle (.app)",
            "macOS \u{5e94}\u{7528}\u{5305} (.app)",
            "macOS \u{30a2}\u{30d7}\u{30ea}\u{30d0}\u{30f3}\u{30c9}\u{30eb} (.app)",
            "macOS \u{c571} \u{bc88}\u{b4e4} (.app)",
        ),
        InstallKind::WindowsInno => pick(
            locale,
            "Windows Installer (Inno)",
            "Windows \u{5b89}\u{88c5}\u{7a0b}\u{5e8f} (Inno)",
            "Windows \u{30a4}\u{30f3}\u{30b9}\u{30c8}\u{30fc}\u{30e9} (Inno)",
            "Windows \u{c124}\u{ce58} \u{d504}\u{b85c}\u{adf8}\u{b7a8} (Inno)",
        ),
        InstallKind::WindowsMsi => pick(
            locale,
            "Windows MSI Package",
            "Windows MSI \u{5b89}\u{88c5}\u{5305}",
            "Windows MSI \u{30d1}\u{30c3}\u{30b1}\u{30fc}\u{30b8}",
            "Windows MSI \u{d328}\u{d0a4}\u{c9c0}",
        ),
        InstallKind::WindowsPortable => pick(
            locale,
            "Windows Portable (.zip)",
            "Windows \u{4fbf}\u{643a}\u{7248} (.zip)",
            "Windows \u{30dd}\u{30fc}\u{30bf}\u{30d6}\u{30eb} (.zip)",
            "Windows \u{d3ec}\u{d130}\u{be14} (.zip)",
        ),
        InstallKind::LinuxAppImage => pick(
            locale,
            "Linux AppImage",
            "Linux AppImage",
            "Linux AppImage",
            "Linux AppImage",
        ),
        InstallKind::LinuxTarball => pick(
            locale,
            "Linux Portable Tarball",
            "Linux \u{4fbf}\u{643a}\u{538b}\u{7f29}\u{5305}",
            "Linux \u{30dd}\u{30fc}\u{30bf}\u{30d6}\u{30eb} Tarball",
            "Linux \u{d3ec}\u{d130}\u{be14} Tarball",
        ),
        InstallKind::LinuxSystemPackage => pick(
            locale,
            "Linux System Package",
            "Linux \u{7cfb}\u{7edf}\u{8f6f}\u{4ef6}\u{5305}",
            "Linux \u{30b7}\u{30b9}\u{30c6}\u{30e0}\u{30d1}\u{30c3}\u{30b1}\u{30fc}\u{30b8}",
            "Linux \u{c2dc}\u{c2a4}\u{d15c} \u{d328}\u{d0a4}\u{c9c0}",
        ),
        InstallKind::CliStandalone => pick(
            locale,
            "Standalone CLI",
            "\u{72ec}\u{7acb} CLI",
            "\u{30b9}\u{30bf}\u{30f3}\u{30c9}\u{30a2}\u{30ed}\u{30f3} CLI",
            "\u{b3c5}\u{b9bd}\u{d615} CLI",
        ),
        InstallKind::Development => pick(
            locale,
            "Development Build",
            "\u{5f00}\u{53d1}\u{6784}\u{5efa}",
            "\u{958b}\u{767a}\u{30d3}\u{30eb}\u{30c9}",
            "\u{ac1c}\u{bc1c} \u{be4c}\u{b4dc}",
        ),
    }
}

pub const fn updates_check_now(locale: Locale) -> &'static str {
    pick(
        locale,
        "Check Now",
        "\u{7acb}\u{5373}\u{68c0}\u{67e5}",
        "\u{4eca}\u{3059}\u{3050}\u{78ba}\u{8a8d}",
        "\u{c9c0}\u{ae08} \u{d655}\u{c778}",
    )
}

pub const fn updates_checking(locale: Locale) -> &'static str {
    pick(
        locale,
        "Checking for updates\u{2026}",
        "\u{6b63}\u{5728}\u{68c0}\u{67e5}\u{66f4}\u{65b0}\u{2026}",
        "\u{30a2}\u{30c3}\u{30d7}\u{30c7}\u{30fc}\u{30c8}\u{3092}\u{78ba}\u{8a8d}\u{4e2d}\u{2026}",
        "\u{c5c5}\u{b370}\u{c774}\u{d2b8} \u{d655}\u{c778} \u{c911}\u{2026}",
    )
}

pub const fn updates_up_to_date(locale: Locale) -> &'static str {
    pick(
        locale,
        "You are on the latest version.",
        "\u{5f53}\u{524d}\u{5df2}\u{662f}\u{6700}\u{65b0}\u{7248}\u{672c}\u{3002}",
        "\u{6700}\u{65b0}\u{30d0}\u{30fc}\u{30b8}\u{30e7}\u{30f3}\u{3092}\u{304a}\u{4f7f}\u{3044}\u{3067}\u{3059}\u{3002}",
        "\u{cd5c}\u{c2e0} \u{bc84}\u{c804}\u{c744} \u{c0ac}\u{c6a9} \u{c911}\u{c785}\u{b2c8}\u{b2e4}.",
    )
}

pub const fn updates_auto_check(locale: Locale) -> &'static str {
    pick(
        locale,
        "Automatically check for updates",
        "\u{81ea}\u{52a8}\u{68c0}\u{67e5}\u{66f4}\u{65b0}",
        "\u{30a2}\u{30c3}\u{30d7}\u{30c7}\u{30fc}\u{30c8}\u{3092}\u{81ea}\u{52d5}\u{78ba}\u{8a8d}",
        "\u{c790}\u{b3d9}\u{c73c}\u{b85c} \u{c5c5}\u{b370}\u{c774}\u{d2b8} \u{d655}\u{c778}",
    )
}

pub const fn updates_channel_label(locale: Locale) -> &'static str {
    pick(
        locale,
        "Update Channel:",
        "\u{66f4}\u{65b0}\u{901a}\u{9053}\u{ff1a}",
        "\u{30a2}\u{30c3}\u{30d7}\u{30c7}\u{30fc}\u{30c8}\u{30c1}\u{30e3}\u{30f3}\u{30cd}\u{30eb}:",
        "\u{c5c5}\u{b370}\u{c774}\u{d2b8} \u{cc44}\u{b110}:",
    )
}

pub const fn updates_channel_stable(locale: Locale) -> &'static str {
    pick(
        locale,
        "Stable",
        "\u{7a33}\u{5b9a}\u{7248}",
        "\u{5b89}\u{5b9a}\u{7248}",
        "\u{c548}\u{c815} \u{bc84}\u{c804}",
    )
}

pub const fn updates_channel_beta(locale: Locale) -> &'static str {
    pick(
        locale,
        "Beta",
        "\u{6d4b}\u{8bd5}\u{7248}",
        "\u{30d9}\u{30fc}\u{30bf}\u{7248}",
        "\u{bca0}\u{d0c0}",
    )
}

pub const fn updates_send_client_id(locale: Locale) -> &'static str {
    pick(
        locale,
        "Share anonymous usage statistics",
        "\u{53d1}\u{9001}\u{533f}\u{540d}\u{4f7f}\u{7528}\u{7edf}\u{8ba1}",
        "\u{533f}\u{540d}\u{306e}\u{5229}\u{7528}\u{7d71}\u{8a08}\u{3092}\
         \u{9001}\u{4fe1}\u{3059}\u{308b}",
        "\u{c775}\u{ba85} \u{c0ac}\u{c6a9} \u{d1b5}\u{acc4} \u{acf5}\u{c720}",
    )
}

pub fn update_banner_available(version: &str, locale: Locale) -> String {
    let ver = version.trim().trim_start_matches('v');
    match locale {
        Locale::En => format!("v{ver} available"),
        Locale::Zh => {
            format!("\u{53d1}\u{73b0}\u{65b0}\u{7248}\u{672c} v{ver}")
        }
        Locale::Ja => {
            format!(
                "v{ver} \u{304c}\u{5229}\u{7528}\u{53ef}\u{80fd}\u{3067}\u{3059}"
            )
        }
        Locale::Ko => {
            format!("v{ver} \u{c5c5}\u{b370}\u{c774}\u{d2b8} \u{ac00}\u{b2a5}")
        }
    }
}

pub fn update_banner_ready(version: &str, locale: Locale) -> String {
    let ver = version.trim().trim_start_matches('v');
    match locale {
        Locale::En => format!("v{ver} ready to install"),
        Locale::Zh => {
            format!("v{ver} \u{5df2}\u{5c31}\u{7eea}\u{53ef}\u{5b89}\u{88c5}")
        }
        Locale::Ja => format!(
            "v{ver} \u{306e}\u{30a4}\u{30f3}\u{30b9}\u{30c8}\u{30fc}\u{30eb}\u{6e96}\u{5099}\u{5b8c}\u{4e86}"
        ),
        Locale::Ko => {
            format!("v{ver} \u{c124}\u{ce58} \u{c900}\u{be44} \u{c644}\u{b8cc}")
        }
    }
}

pub const fn update_banner_view(locale: Locale) -> &'static str {
    pick(
        locale,
        "View",
        "\u{67e5}\u{770b}",
        "\u{8868}\u{793a}",
        "\u{bcf4}\u{ae30}",
    )
}

pub const fn update_modal_title(locale: Locale) -> &'static str {
    pick(
        locale,
        "Software Update",
        "\u{8f6f}\u{4ef6}\u{66f4}\u{65b0}",
        "\u{30bd}\u{30d5}\u{30c8}\u{30a6}\u{30a7}\u{30a2}\u{30a2}\u{30c3}\u{30d7}\u{30c7}\u{30fc}\u{30c8}",
        "\u{c18c}\u{d504}\u{d2b8}\u{c6e8}\u{c5b4} \u{c5c5}\u{b370}\u{c774}\u{d2b8}",
    )
}

pub fn update_published_at(date: &str, locale: Locale) -> String {
    let short = date.split('T').next().unwrap_or(date);
    match locale {
        Locale::En => format!("Published {short}"),
        Locale::Zh => format!("\u{53d1}\u{5e03}\u{4e8e} {short}"),
        Locale::Ja => format!("\u{516c}\u{958b}\u{65e5}: {short}"),
        Locale::Ko => format!("\u{bc30}\u{d3ec}\u{c77c}: {short}"),
    }
}

pub const fn update_changelog_empty(locale: Locale) -> &'static str {
    pick(
        locale,
        "No release notes provided.",
        "\u{6682}\u{65e0}\u{66f4}\u{65b0}\u{8bf4}\u{660e}\u{3002}",
        "\u{30ea}\u{30ea}\u{30fc}\u{30b9}\u{30ce}\u{30fc}\u{30c8}\u{306f}\u{3042}\u{308a}\u{307e}\u{305b}\u{3093}\u{3002}",
        "\u{b9b4}\u{b9ac}\u{c2a4} \u{b178}\u{d2b8}\u{ac00} \u{c5c6}\u{c2b5}\u{b2c8}\u{b2e4}.",
    )
}

pub const fn update_download_and_update(locale: Locale) -> &'static str {
    pick(
        locale,
        "Download & Update",
        "\u{4e0b}\u{8f7d}\u{5e76}\u{66f4}\u{65b0}",
        "\u{30c0}\u{30a6}\u{30f3}\u{30ed}\u{30fc}\u{30c9}\u{3057}\u{3066}\u{66f4}\u{65b0}",
        "\u{b2e4}\u{c6b4}\u{b85c}\u{b4dc} \u{bc0f} \u{c5c5}\u{b370}\u{c774}\u{d2b8}",
    )
}

pub const fn update_later(locale: Locale) -> &'static str {
    pick(
        locale,
        "Later",
        "\u{7a0d}\u{540e}\u{63d0}\u{9192}",
        "\u{5f8c}\u{3067}",
        "\u{b098}\u{c911}\u{c5d0}",
    )
}

pub fn update_skip_version(version: &str, locale: Locale) -> String {
    let ver = version.trim().trim_start_matches('v');
    match locale {
        Locale::En => format!("Skip v{ver}"),
        Locale::Zh => format!("\u{8df3}\u{8fc7} v{ver}"),
        Locale::Ja => {
            format!("v{ver} \u{3092}\u{30b9}\u{30ad}\u{30c3}\u{30d7}")
        }
        Locale::Ko => format!("v{ver} \u{ac74}\u{b108}\u{b6f0}\u{ae30}"),
    }
}

pub const fn update_copy_command(locale: Locale) -> &'static str {
    pick(
        locale,
        "Copy Command",
        "\u{590d}\u{5236}\u{5347}\u{7ea7}\u{547d}\u{4ee4}",
        "\u{30b3}\u{30de}\u{30f3}\u{30c9}\u{3092}\u{30b3}\u{30d4}\u{30fc}",
        "\u{ba85}\u{b839} \u{bcf5}\u{c0ac}",
    )
}

pub const fn update_open_release(locale: Locale) -> &'static str {
    pick(
        locale,
        "Open Release",
        "\u{6253}\u{5f00}\u{53d1}\u{5e03}\u{9875}",
        "\u{30ea}\u{30ea}\u{30fc}\u{30b9}\u{30da}\u{30fc}\u{30b8}\u{3092}\u{958b}\u{304f}",
        "\u{b9b4}\u{b9ac}\u{c2a4} \u{c5f4}\u{ae30}",
    )
}

pub const fn update_open_github_release(locale: Locale) -> &'static str {
    pick(
        locale,
        "Open GitHub Release",
        "\u{6253}\u{5f00} GitHub Release",
        "GitHub Release \u{3092}\u{958b}\u{304f}",
        "GitHub Release \u{c5f4}\u{ae30}",
    )
}

pub const fn update_hide(locale: Locale) -> &'static str {
    pick(
        locale,
        "Hide",
        "\u{6536}\u{8d77}",
        "\u{96a0}\u{3059}",
        "\u{c228}\u{ae30}\u{ae30}",
    )
}

pub const fn update_cancel(locale: Locale) -> &'static str {
    cancel(locale)
}

pub const fn update_restart_and_install(locale: Locale) -> &'static str {
    pick(
        locale,
        "Restart & Install",
        "\u{91cd}\u{542f}\u{5e76}\u{5b89}\u{88c5}",
        "\u{518d}\u{8d77}\u{52d5}\u{3057}\u{3066}\u{30a4}\u{30f3}\u{30b9}\u{30c8}\u{30fc}\u{30eb}",
        "\u{c7ac}\u{c2dc}\u{c791} \u{bc0f} \u{c124}\u{ce58}",
    )
}

pub const fn update_retry(locale: Locale) -> &'static str {
    pick(
        locale,
        "Retry",
        "\u{91cd}\u{8bd5}",
        "\u{518d}\u{8a66}\u{884c}",
        "\u{c7ac}\u{c2dc}\u{b3c4}",
    )
}

pub const fn update_installing_status(locale: Locale) -> &'static str {
    pick(
        locale,
        "Installing update and preparing restart\u{2026}",
        "\u{6b63}\u{5728}\u{5b89}\u{88c5}\u{66f4}\u{65b0}\u{5e76}\u{51c6}\u{5907}\u{91cd}\u{542f}\u{2026}",
        "\u{30a2}\u{30c3}\u{30d7}\u{30c7}\u{30fc}\u{30c8}\u{3092}\u{30a4}\u{30f3}\u{30b9}\u{30c8}\u{30fc}\u{30eb}\u{3057}\u{3066}\u{3044}\u{307e}\u{3059}\u{2026}",
        "\u{c5c5}\u{b370}\u{c774}\u{d2b8}\u{b97c} \u{c124}\u{ce58}\u{d558}\u{ace0} \u{c7ac}\u{c2dc}\u{c791}\u{c744} \u{c900}\u{be44}\u{d558}\u{b294} \u{c911}\u{2026}",
    )
}

pub const fn update_verified_badge(locale: Locale) -> &'static str {
    pick(
        locale,
        "Update ready to install. Restart to apply.",
        "\u{66f4}\u{65b0}\u{5df2}\u{4e0b}\u{8f7d}\u{5c31}\u{7eea}\u{ff0c}\
         \u{91cd}\u{542f}\u{540e}\u{5373}\u{53ef}\u{5b8c}\u{6210}\u{5b89}\
         \u{88c5}\u{3002}",
        "\u{30a2}\u{30c3}\u{30d7}\u{30c7}\u{30fc}\u{30c8}\u{306e}\u{6e96}\
         \u{5099}\u{304c}\u{3067}\u{304d}\u{307e}\u{3057}\u{305f}\u{3002}\
         \u{518d}\u{8d77}\u{52d5}\u{3057}\u{3066}\u{9069}\u{7528}\u{3057}\
         \u{307e}\u{3059}\u{3002}",
        "\u{c5c5}\u{b370}\u{c774}\u{d2b8} \u{c124}\u{ce58} \u{c900}\u{be44}\
         \u{ac00} \u{c644}\u{b8cc}\u{b418}\u{c5c8}\u{c2b5}\u{b2c8}\u{b2e4}. \
         \u{c7ac}\u{c2dc}\u{c791}\u{d558}\u{c5ec} \u{c801}\u{c6a9}\u{d558}\
         \u{c138}\u{c694}.",
    )
}

pub const fn update_restart_warning_scan(locale: Locale) -> &'static str {
    pick(
        locale,
        "A scan is currently in progress. Restarting will interrupt it.",
        "\u{5f53}\u{524d}\u{6709}\u{6b63}\u{5728}\u{8fdb}\u{884c}\u{7684}\u{626b}\u{63cf}\u{ff0c}\u{91cd}\u{542f}\u{5c06}\u{4e2d}\u{65ad}\u{8be5}\u{626b}\u{63cf}\u{3002}",
        "\u{30b9}\u{30ad}\u{30e3}\u{30f3}\u{304c}\u{5b9f}\u{884c}\u{4e2d}\u{3067}\u{3059}\u{3002}\u{518d}\u{8d77}\u{52d5}\u{3059}\u{308b}\u{3068}\u{4e2d}\u{65ad}\u{3055}\u{308c}\u{307e}\u{3059}\u{3002}",
        "\u{d604}\u{c7ac} \u{c2a4}\u{ce94}\u{c774} \u{c9c4}\u{d589} \u{c911}\u{c785}\u{b2c8}\u{b2e4}. \u{c7ac}\u{c2dc}\u{c791}\u{d558}\u{ba74} \u{c2a4}\u{ce94}\u{c774} \u{c911}\u{b2e8}\u{b429}\u{b2c8}\u{b2e4}.",
    )
}

pub fn update_restart_warning_marks(count: usize, locale: Locale) -> String {
    match locale {
        Locale::En => format!(
            "{count} marked item(s) have not been committed yet. Restarting \
             will discard uncommitted marks."
        ),
        Locale::Zh => format!(
            "\u{5f53}\u{524d}\u{6709} {count} \u{4e2a}\u{672a}\u{63d0}\u{4ea4}\u{7684}\u{5df2}\u{6807}\u{8bb0}\u{9879}\u{ff0c}\u{91cd}\u{542f}\u{5c06}\u{6e05}\u{7a7a}\u{8fd9}\u{4e9b}\u{6807}\u{8bb0}\u{3002}"
        ),
        Locale::Ja => format!(
            "\u{672a}\u{78ba}\u{5b9a}\u{306e}\u{30de}\u{30fc}\u{30af}\u{304c} {count} \u{4ef6}\u{3042}\u{308a}\u{307e}\u{3059}\u{3002}\u{518d}\u{8d77}\u{52d5}\u{3059}\u{308b}\u{3068}\u{30de}\u{30fc}\u{30af}\u{306f}\u{7834}\u{68c4}\u{3055}\u{308c}\u{307e}\u{3059}\u{3002}"
        ),
        Locale::Ko => format!(
            "\u{c544}\u{c9c1} \u{c801}\u{c6a9}\u{d558}\u{c9c0} \u{c54a}\u{c740} \u{d45c}\u{c2dc} \u{d56d}\u{baa9}\u{c774} {count}\u{ac1c} \u{c788}\u{c2b5}\u{b2c8}\u{b2e4}. \u{c7ac}\u{c2dc}\u{c791}\u{d558}\u{ba74} \u{d45c}\u{c2dc}\u{ac00} \u{cd08}\u{ae30}\u{d654}\u{b429}\u{b2c8}\u{b2e4}."
        ),
    }
}

pub const fn update_manual_command_hint(locale: Locale) -> &'static str {
    pick(
        locale,
        "Run this command to upgrade with your current package manager:",
        "\u{8bf7}\u{4f7f}\u{7528}\u{5f53}\u{524d}\u{5b89}\u{88c5}\u{65b9}\u{5f0f}\u{5bf9}\u{5e94}\u{7684}\u{547d}\u{4ee4}\u{5347}\u{7ea7}\u{ff1a}",
        "\u{73fe}\u{5728}\u{306e}\u{30d1}\u{30c3}\u{30b1}\u{30fc}\u{30b8}\u{30de}\u{30cd}\u{30fc}\u{30b8}\u{30e3}\u{3067}\u{6b21}\u{306e}\u{30b3}\u{30de}\u{30f3}\u{30c9}\u{3092}\u{5b9f}\u{884c}\u{3057}\u{3066}\u{66f4}\u{65b0}\u{3057}\u{3066}\u{304f}\u{3060}\u{3055}\u{3044}:",
        "\u{d604}\u{c7ac} \u{d328}\u{d0a4}\u{c9c0} \u{ad00}\u{b9ac}\u{c790}\u{b85c} \u{c5c5}\u{adf8}\u{b808}\u{c774}\u{b4dc}\u{d558}\u{b824}\u{ba74} \u{b2e4}\u{c74c} \u{ba85}\u{b839}\u{c744} \u{c2e4}\u{d589}\u{d558}\u{c138}\u{c694}:",
    )
}

fn contains_any(haystack: &str, needles: &[&str]) -> bool {
    needles.iter().any(|needle| haystack.contains(needle))
}

/// Map low-level update error strings to localized, actionable user copy.
pub fn friendly_update_error(raw: &str, locale: Locale) -> &'static str {
    let lower = raw.to_ascii_lowercase();

    if contains_any(&lower, &["timed out", "timeout", "deadline"]) {
        return pick(
            locale,
            "The update request timed out. Check your internet connection and \
             try again.",
            "\u{68c0}\u{67e5}\u{6216}\u{4e0b}\u{8f7d}\u{66f4}\u{65b0}\u{8d85}\
             \u{65f6}\u{ff0c}\u{8bf7}\u{68c0}\u{67e5}\u{7f51}\u{7edc}\u{8fde}\
             \u{63a5}\u{540e}\u{91cd}\u{8bd5}\u{3002}",
            "\u{66f4}\u{65b0}\u{306e}\u{78ba}\u{8a8d}\u{307e}\u{305f}\u{306f}\
             \u{30c0}\u{30a6}\u{30f3}\u{30ed}\u{30fc}\u{30c9}\u{304c}\u{30bf}\
             \u{30a4}\u{30e0}\u{30a2}\u{30a6}\u{30c8}\u{3057}\u{307e}\u{3057}\
             \u{305f}\u{3002}\u{30a4}\u{30f3}\u{30bf}\u{30fc}\u{30cd}\u{30c3}\
             \u{30c8}\u{63a5}\u{7d9a}\u{3092}\u{78ba}\u{8a8d}\u{3057}\u{3066}\
             \u{518d}\u{8a66}\u{884c}\u{3057}\u{3066}\u{304f}\u{3060}\u{3055}\
             \u{3044}\u{3002}",
            "\u{c5c5}\u{b370}\u{c774}\u{d2b8} \u{d655}\u{c778} \u{b610}\u{b294} \
             \u{b2e4}\u{c6b4}\u{b85c}\u{b4dc} \u{c2dc}\u{ac04}\u{c774} \
             \u{cd08}\u{acfc}\u{b418}\u{c5c8}\u{c2b5}\u{b2c8}\u{b2e4}. \
             \u{c778}\u{d130}\u{b137} \u{c5f0}\u{acb0}\u{c744} \u{d655}\u{c778}\
             \u{d55c} \u{d6c4} \u{b2e4}\u{c2dc} \u{c2dc}\u{b3c4}\u{d558}\
             \u{c138}\u{c694}.",
        );
    }

    if contains_any(
        &lower,
        &[
            "ed25519",
            "signature",
            "x-ruum-signature",
            "sha-256",
            "sha256",
            "checksum",
            "size mismatch",
            "invalid manifest",
            "schema_version",
            "public key",
        ],
    ) {
        return pick(
            locale,
            "The update could not be verified and was discarded for your \
             safety. Please try again or download it from the release page.",
            "\u{66f4}\u{65b0}\u{6587}\u{4ef6}\u{6821}\u{9a8c}\u{672a}\u{901a}\
             \u{8fc7}\u{ff0c}\u{4e3a}\u{4fdd}\u{969c}\u{5b89}\u{5168}\u{5df2}\
             \u{653e}\u{5f03}\u{672c}\u{6b21}\u{66f4}\u{65b0}\u{3002}\u{8bf7}\
             \u{91cd}\u{8bd5}\u{6216}\u{524d}\u{5f80}\u{53d1}\u{5e03}\u{9875}\
             \u{4e0b}\u{8f7d}\u{3002}",
            "\u{66f4}\u{65b0}\u{30d5}\u{30a1}\u{30a4}\u{30eb}\u{306e}\u{691c}\
             \u{8a3c}\u{306b}\u{5931}\u{6557}\u{3057}\u{305f}\u{305f}\u{3081}\
             \u{3001}\u{5b89}\u{5168}\u{306e}\u{305f}\u{3081}\u{4e2d}\u{65ad}\
             \u{3057}\u{307e}\u{3057}\u{305f}\u{3002}\u{518d}\u{8a66}\u{884c}\
             \u{3059}\u{308b}\u{304b}\u{3001}\u{30ea}\u{30ea}\u{30fc}\u{30b9}\
             \u{30da}\u{30fc}\u{30b8}\u{304b}\u{3089}\u{30c0}\u{30a6}\u{30f3}\
             \u{30ed}\u{30fc}\u{30c9}\u{3057}\u{3066}\u{304f}\u{3060}\u{3055}\
             \u{3044}\u{3002}",
            "\u{c5c5}\u{b370}\u{c774}\u{d2b8} \u{d30c}\u{c77c} \u{ac80}\u{c99d}\
             \u{c5d0} \u{c2e4}\u{d328}\u{d558}\u{c5ec} \u{c548}\u{c804}\u{c744} \
             \u{c704}\u{d574} \u{c911}\u{b2e8}\u{b418}\u{c5c8}\u{c2b5}\u{b2c8}\
             \u{b2e4}. \u{b2e4}\u{c2dc} \u{c2dc}\u{b3c4}\u{d558}\u{ac70}\u{b098} \
             \u{b9b4}\u{b9ac}\u{c2a4} \u{d398}\u{c774}\u{c9c0}\u{c5d0}\u{c11c} \
             \u{b2e4}\u{c6b4}\u{b85c}\u{b4dc}\u{d558}\u{c138}\u{c694}.",
        );
    }

    if contains_any(
        &lower,
        &[
            "permission denied",
            "access is denied",
            "read-only",
            "readonly",
            "not writable",
            "os error 5",
            "os error 13",
        ],
    ) {
        return pick(
            locale,
            "Ruum cannot write to the application folder. Please download the \
             installer from the release page to update manually.",
            "\u{6ca1}\u{6709}\u{6743}\u{9650}\u{5199}\u{5165}\u{5f53}\u{524d}\
             \u{5e94}\u{7528}\u{5b89}\u{88c5}\u{76ee}\u{5f55}\u{ff0c}\u{8bf7}\
             \u{524d}\u{5f80}\u{53d1}\u{5e03}\u{9875}\u{624b}\u{52a8}\u{4e0b}\
             \u{8f7d}\u{5b89}\u{88c5}\u{5305}\u{66f4}\u{65b0}\u{3002}",
            "\u{30a2}\u{30d7}\u{30ea}\u{30b1}\u{30fc}\u{30b7}\u{30e7}\u{30f3}\
             \u{30d5}\u{30a9}\u{30eb}\u{30c0}\u{3078}\u{306e}\u{66f8}\u{304d}\
             \u{8fbc}\u{307f}\u{6a29}\u{9650}\u{304c}\u{3042}\u{308a}\u{307e}\
             \u{305b}\u{3093}\u{3002}\u{30ea}\u{30ea}\u{30fc}\u{30b9}\u{30da}\
             \u{30fc}\u{30b8}\u{304b}\u{3089}\u{30a4}\u{30f3}\u{30b9}\u{30c8}\
             \u{30fc}\u{30e9}\u{30fc}\u{3092}\u{30c0}\u{30a6}\u{30f3}\u{30ed}\
             \u{30fc}\u{30c9}\u{3057}\u{3066}\u{624b}\u{52d5}\u{3067}\u{66f4}\
             \u{65b0}\u{3057}\u{3066}\u{304f}\u{3060}\u{3055}\u{3044}\u{3002}",
            "\u{c560}\u{d50c}\u{b9ac}\u{cf00}\u{c774}\u{c158} \u{d3f4}\u{b354}\
             \u{c5d0} \u{c4f8} \u{ad8c}\u{d55c}\u{c774} \u{c5c6}\u{c2b5}\u{b2c8}\
             \u{b2e4}. \u{b9b4}\u{b9ac}\u{c2a4} \u{d398}\u{c774}\u{c9c0}\u{c5d0}\
             \u{c11c} \u{c124}\u{ce58} \u{d30c}\u{c77c}\u{c744} \u{b2e4}\u{c6b4}\
             \u{b85c}\u{b4dc}\u{d558}\u{c5ec} \u{c218}\u{b3d9}\u{c73c}\u{b85c} \
             \u{c5c5}\u{b370}\u{c774}\u{d2b8}\u{d558}\u{c138}\u{c694}.",
        );
    }

    if contains_any(
        &lower,
        &["no space", "disk full", "os error 28", "os error 112"],
    ) {
        return pick(
            locale,
            "Not enough free disk space to download or install the update. \
             Free up some space and try again.",
            "\u{78c1}\u{76d8}\u{53ef}\u{7528}\u{7a7a}\u{95f4}\u{4e0d}\u{8db3}\
             \u{ff0c}\u{65e0}\u{6cd5}\u{4e0b}\u{8f7d}\u{6216}\u{5b89}\u{88c5}\
             \u{66f4}\u{65b0}\u{3002}\u{8bf7}\u{6e05}\u{7406}\u{90e8}\u{5206}\
             \u{7a7a}\u{95f4}\u{540e}\u{91cd}\u{8bd5}\u{3002}",
            "\u{66f4}\u{65b0}\u{306e}\u{30c0}\u{30a6}\u{30f3}\u{30ed}\u{30fc}\
             \u{30c9}\u{307e}\u{305f}\u{306f}\u{30a4}\u{30f3}\u{30b9}\u{30c8}\
             \u{30fc}\u{30eb}\u{306b}\u{5fc5}\u{8981}\u{306a}\u{7a7a}\u{304d}\
             \u{5bb9}\u{91cf}\u{304c}\u{4e0d}\u{8db3}\u{3057}\u{3066}\u{3044}\
             \u{307e}\u{3059}\u{3002}\u{7a7a}\u{304d}\u{5bb9}\u{91cf}\u{3092}\
             \u{78ba}\u{4fdd}\u{3057}\u{3066}\u{518d}\u{8a66}\u{884c}\u{3057}\
             \u{3066}\u{304f}\u{3060}\u{3055}\u{3044}\u{3002}",
            "\u{c5c5}\u{b370}\u{c774}\u{d2b8}\u{b97c} \u{b2e4}\u{c6b4}\u{b85c}\
             \u{b4dc}\u{d558}\u{ac70}\u{b098} \u{c124}\u{ce58}\u{d560} \
             \u{b514}\u{c2a4}\u{d06c} \u{acf5}\u{ac04}\u{c774} \u{bd80}\u{c871}\
             \u{d569}\u{b2c8}\u{b2e4}. \u{acf5}\u{ac04}\u{c744} \u{d655}\u{bcf4}\
             \u{d55c} \u{d6c4} \u{b2e4}\u{c2dc} \u{c2dc}\u{b3c4}\u{d558}\
             \u{c138}\u{c694}.",
        );
    }

    if contains_any(&lower, &["no release package", "no release manifest"]) {
        return pick(
            locale,
            "No compatible update package was found for your system. Please \
             download it from the release page.",
            "\u{672a}\u{627e}\u{5230}\u{9002}\u{7528}\u{4e8e}\u{5f53}\u{524d}\
             \u{7cfb}\u{7edf}\u{7684}\u{66f4}\u{65b0}\u{5b89}\u{88c5}\u{5305}\
             \u{ff0c}\u{8bf7}\u{524d}\u{5f80}\u{53d1}\u{5e03}\u{9875}\u{4e0b}\
             \u{8f7d}\u{3002}",
            "\u{304a}\u{4f7f}\u{3044}\u{306e}\u{30b7}\u{30b9}\u{30c6}\u{30e0}\
             \u{306b}\u{5bfe}\u{5fdc}\u{3059}\u{308b}\u{66f4}\u{65b0}\u{30d1}\
             \u{30c3}\u{30b1}\u{30fc}\u{30b8}\u{304c}\u{898b}\u{3064}\u{304b}\
             \u{308a}\u{307e}\u{305b}\u{3093}\u{3067}\u{3057}\u{305f}\u{3002}\
             \u{30ea}\u{30ea}\u{30fc}\u{30b9}\u{30da}\u{30fc}\u{30b8}\u{304b}\
             \u{3089}\u{30c0}\u{30a6}\u{30f3}\u{30ed}\u{30fc}\u{30c9}\u{3057}\
             \u{3066}\u{304f}\u{3060}\u{3055}\u{3044}\u{3002}",
            "\u{d604}\u{c7ac} \u{c2dc}\u{c2a4}\u{d15c}\u{c5d0} \u{b9de}\u{b294} \
             \u{c5c5}\u{b370}\u{c774}\u{d2b8} \u{d328}\u{d0a4}\u{c9c0}\u{b97c} \
             \u{cc3e}\u{c744} \u{c218} \u{c5c6}\u{c2b5}\u{b2c8}\u{b2e4}. \
             \u{b9b4}\u{b9ac}\u{c2a4} \u{d398}\u{c774}\u{c9c0}\u{c5d0}\u{c11c} \
             \u{b2e4}\u{c6b4}\u{b85c}\u{b4dc}\u{d558}\u{c138}\u{c694}.",
        );
    }

    if contains_any(
        &lower,
        &[
            "http 500",
            "http 502",
            "http 503",
            "http 504",
            "returned http 5",
        ],
    ) {
        return pick(
            locale,
            "The update server is temporarily unavailable. Please try again \
             in a few minutes.",
            "\u{66f4}\u{65b0}\u{670d}\u{52a1}\u{5668}\u{6682}\u{65f6}\u{4e0d}\
             \u{53ef}\u{7528}\u{ff0c}\u{8bf7}\u{7a0d}\u{540e}\u{518d}\u{8bd5}\
             \u{3002}",
            "\u{66f4}\u{65b0}\u{30b5}\u{30fc}\u{30d0}\u{30fc}\u{304c}\u{4e00}\
             \u{6642}\u{7684}\u{306b}\u{5229}\u{7528}\u{3067}\u{304d}\u{307e}\
             \u{305b}\u{3093}\u{3002}\u{3057}\u{3070}\u{3089}\u{304f}\u{3057}\
             \u{3066}\u{304b}\u{3089}\u{518d}\u{8a66}\u{884c}\u{3057}\u{3066}\
             \u{304f}\u{3060}\u{3055}\u{3044}\u{3002}",
            "\u{c5c5}\u{b370}\u{c774}\u{d2b8} \u{c11c}\u{bc84}\u{b97c} \
             \u{c77c}\u{c2dc}\u{c801}\u{c73c}\u{b85c} \u{c0ac}\u{c6a9}\u{d560} \
             \u{c218} \u{c5c6}\u{c2b5}\u{b2c8}\u{b2e4}. \u{c7a0}\u{c2dc} \
             \u{d6c4} \u{b2e4}\u{c2dc} \u{c2dc}\u{b3c4}\u{d558}\u{c138}\u{c694}.",
        );
    }

    if contains_any(&lower, &["returned http", "status code", "http 4"]) {
        return pick(
            locale,
            "The update service returned an unexpected response. Please try \
             again later or open the release page.",
            "\u{66f4}\u{65b0}\u{670d}\u{52a1}\u{8fd4}\u{56de}\u{5f02}\u{5e38}\
             \u{54cd}\u{5e94}\u{ff0c}\u{8bf7}\u{7a0d}\u{540e}\u{91cd}\u{8bd5}\
             \u{6216}\u{6253}\u{5f00}\u{53d1}\u{5e03}\u{9875}\u{3002}",
            "\u{66f4}\u{65b0}\u{30b5}\u{30fc}\u{30d3}\u{30b9}\u{304b}\u{3089}\
             \u{4e88}\u{671f}\u{3057}\u{306a}\u{3044}\u{5fdc}\u{7b54}\u{304c}\
             \u{3042}\u{308a}\u{307e}\u{3057}\u{305f}\u{3002}\u{5f8c}\u{3067}\
             \u{518d}\u{8a66}\u{884c}\u{3059}\u{308b}\u{304b}\u{3001}\u{30ea}\
             \u{30ea}\u{30fc}\u{30b9}\u{30da}\u{30fc}\u{30b8}\u{3092}\u{958b}\
             \u{3044}\u{3066}\u{304f}\u{3060}\u{3055}\u{3044}\u{3002}",
            "\u{c5c5}\u{b370}\u{c774}\u{d2b8} \u{c11c}\u{be44}\u{c2a4}\u{c5d0}\
             \u{c11c} \u{c608}\u{ae30}\u{ce58} \u{c54a}\u{c740} \u{c751}\u{b2f5}\
             \u{c744} \u{bc1b}\u{c558}\u{c2b5}\u{b2c8}\u{b2e4}. \u{b098}\u{c911}\
             \u{c5d0} \u{b2e4}\u{c2dc} \u{c2dc}\u{b3c4}\u{d558}\u{ac70}\u{b098} \
             \u{b9b4}\u{b9ac}\u{c2a4} \u{d398}\u{c774}\u{c9c0}\u{b97c} \
             \u{c5ec}\u{c138}\u{c694}.",
        );
    }

    if contains_any(
        &lower,
        &[
            "dns",
            "connect",
            "network",
            "tls",
            "rustls",
            "certificate",
            "broken pipe",
            "eof",
            "reset",
            "unreachable",
            "unavailable",
            "request failed",
            "fetch failed",
            "read failed",
            "ureq",
        ],
    ) {
        return pick(
            locale,
            "Unable to reach the update server. Check your internet \
             connection or download from the release page.",
            "\u{65e0}\u{6cd5}\u{8fde}\u{63a5}\u{5230}\u{66f4}\u{65b0}\u{670d}\
             \u{52a1}\u{5668}\u{ff0c}\u{8bf7}\u{68c0}\u{67e5}\u{7f51}\u{7edc}\
             \u{8fde}\u{63a5}\u{6216}\u{524d}\u{5f80}\u{53d1}\u{5e03}\u{9875}\
             \u{624b}\u{52a8}\u{4e0b}\u{8f7d}\u{3002}",
            "\u{66f4}\u{65b0}\u{30b5}\u{30fc}\u{30d0}\u{30fc}\u{306b}\u{63a5}\
             \u{7d9a}\u{3067}\u{304d}\u{307e}\u{305b}\u{3093}\u{3067}\u{3057}\
             \u{305f}\u{3002}\u{30a4}\u{30f3}\u{30bf}\u{30fc}\u{30cd}\u{30c3}\
             \u{30c8}\u{63a5}\u{7d9a}\u{3092}\u{78ba}\u{8a8d}\u{3059}\u{308b}\
             \u{304b}\u{3001}\u{30ea}\u{30ea}\u{30fc}\u{30b9}\u{30da}\u{30fc}\
             \u{30b8}\u{304b}\u{3089}\u{30c0}\u{30a6}\u{30f3}\u{30ed}\u{30fc}\
             \u{30c9}\u{3057}\u{3066}\u{304f}\u{3060}\u{3055}\u{3044}\u{3002}",
            "\u{c5c5}\u{b370}\u{c774}\u{d2b8} \u{c11c}\u{bc84}\u{c5d0} \
             \u{c5f0}\u{acb0}\u{d560} \u{c218} \u{c5c6}\u{c2b5}\u{b2c8}\u{b2e4}. \
             \u{c778}\u{d130}\u{b137} \u{c5f0}\u{acb0}\u{c744} \u{d655}\u{c778}\
             \u{d558}\u{ac70}\u{b098} \u{b9b4}\u{b9ac}\u{c2a4} \u{d398}\u{c774}\
             \u{c9c0}\u{c5d0}\u{c11c} \u{b2e4}\u{c6b4}\u{b85c}\u{b4dc}\u{d558}\
             \u{c138}\u{c694}.",
        );
    }

    if contains_any(
        &lower,
        &[
            "extract",
            "unzip",
            "installer",
            "jxa",
            "appimage",
            "replace",
            "rename",
            "spawn",
        ],
    ) {
        return pick(
            locale,
            "Could not install the downloaded update. Please try again or \
             download the latest version from the release page.",
            "\u{65e0}\u{6cd5}\u{5b8c}\u{6210}\u{66f4}\u{65b0}\u{5b89}\u{88c5}\
             \u{ff0c}\u{8bf7}\u{91cd}\u{8bd5}\u{6216}\u{524d}\u{5f80}\u{53d1}\
             \u{5e03}\u{9875}\u{4e0b}\u{8f7d}\u{6700}\u{65b0}\u{7248}\u{672c}\
             \u{3002}",
            "\u{66f4}\u{65b0}\u{3092}\u{30a4}\u{30f3}\u{30b9}\u{30c8}\u{30fc}\
             \u{30eb}\u{3067}\u{304d}\u{307e}\u{305b}\u{3093}\u{3067}\u{3057}\
             \u{305f}\u{3002}\u{518d}\u{8a66}\u{884c}\u{3059}\u{308b}\u{304b}\
             \u{3001}\u{30ea}\u{30ea}\u{30fc}\u{30b9}\u{30da}\u{30fc}\u{30b8}\
             \u{304b}\u{3089}\u{6700}\u{65b0}\u{7248}\u{3092}\u{30c0}\u{30a6}\
             \u{30f3}\u{30ed}\u{30fc}\u{30c9}\u{3057}\u{3066}\u{304f}\u{3060}\
             \u{3055}\u{3044}\u{3002}",
            "\u{c5c5}\u{b370}\u{c774}\u{d2b8}\u{b97c} \u{c124}\u{ce58}\u{d560} \
             \u{c218} \u{c5c6}\u{c2b5}\u{b2c8}\u{b2e4}. \u{b2e4}\u{c2dc} \
             \u{c2dc}\u{b3c4}\u{d558}\u{ac70}\u{b098} \u{b9b4}\u{b9ac}\u{c2a4} \
             \u{d398}\u{c774}\u{c9c0}\u{c5d0}\u{c11c} \u{cd5c}\u{c2e0} \
             \u{bc84}\u{c804}\u{c744} \u{b2e4}\u{c6b4}\u{b85c}\u{b4dc}\u{d558}\
             \u{c138}\u{c694}.",
        );
    }

    pick(
        locale,
        "Could not complete the update. Please try again or open the release \
         page.",
        "\u{6682}\u{65f6}\u{65e0}\u{6cd5}\u{5b8c}\u{6210}\u{66f4}\u{65b0}\
         \u{ff0c}\u{8bf7}\u{7a0d}\u{540e}\u{91cd}\u{8bd5}\u{6216}\u{6253}\
         \u{5f00}\u{53d1}\u{5e03}\u{9875}\u{3002}",
        "\u{66f4}\u{65b0}\u{3092}\u{5b8c}\u{4e86}\u{3067}\u{304d}\u{307e}\
         \u{305b}\u{3093}\u{3067}\u{3057}\u{305f}\u{3002}\u{5f8c}\u{3067}\
         \u{518d}\u{8a66}\u{884c}\u{3059}\u{308b}\u{304b}\u{3001}\u{30ea}\
         \u{30ea}\u{30fc}\u{30b9}\u{30da}\u{30fc}\u{30b8}\u{3092}\u{958b}\
         \u{3044}\u{3066}\u{304f}\u{3060}\u{3055}\u{3044}\u{3002}",
        "\u{c5c5}\u{b370}\u{c774}\u{d2b8}\u{b97c} \u{c644}\u{b8cc}\u{d560} \
         \u{c218} \u{c5c6}\u{c2b5}\u{b2c8}\u{b2e4}. \u{b098}\u{c911}\u{c5d0} \
         \u{b2e4}\u{c2dc} \u{c2dc}\u{b3c4}\u{d558}\u{ac70}\u{b098} \
         \u{b9b4}\u{b9ac}\u{c2a4} \u{d398}\u{c774}\u{c9c0}\u{b97c} \u{c5ec}\
         \u{c138}\u{c694}.",
    )
}

pub const fn update_view_btn(locale: Locale) -> &'static str {
    update_banner_view(locale)
}

// ── webhook settings (multi-endpoint CRUD & tutorial) ───────────────────

pub const fn webhook_title(locale: Locale) -> &'static str {
    pick(
        locale,
        "Webhook Endpoints",
        "Webhook \u{7aef}\u{70b9}",
        "Webhook \u{30a8}\u{30f3}\u{30c9}\u{30dd}\u{30a4}\u{30f3}\u{30c8}",
        "Webhook \u{c5d4}\u{b4dc}\u{d3ec}\u{c778}\u{d2b8}",
    )
}

pub const fn webhook_desc(locale: Locale) -> &'static str {
    pick(
        locale,
        "Deliver generic JSON POST alerts to one or more endpoints when local \
         disk space falls below threshold.",
        "\u{5f53}\u{672c}\u{673a}\u{78c1}\u{76d8}\u{7a7a}\u{95f4}\u{4f4e}\u{4e8e}\u{9608}\u{503c}\u{65f6}\u{ff0c}\u{5411}\u{4e00}\u{4e2a}\u{6216}\u{591a}\u{4e2a}\u{7aef}\u{70b9}\u{53d1}\u{9001}\u{901a}\u{7528} JSON POST \u{544a}\u{8b66}\u{3002}",
        "\u{30ed}\u{30fc}\u{30ab}\u{30eb}\u{30c7}\u{30a3}\u{30b9}\u{30af}\u{5bb9}\u{91cf}\u{304c}\u{3057}\u{304d}\u{3044}\u{5024}\u{3092}\u{4e0b}\u{56de}\u{308b}\u{3068}\u{3001}\u{6709}\u{52b9}\u{306a}\u{3059}\u{3079}\u{3066}\u{306e}\u{30a8}\u{30f3}\u{30c9}\u{30dd}\u{30a4}\u{30f3}\u{30c8}\u{306b} JSON POST \u{901a}\u{77e5}\u{3092}\u{9001}\u{4fe1}\u{3057}\u{307e}\u{3059}\u{3002}",
        "\u{b85c}\u{ceec} \u{b514}\u{c2a4}\u{d06c} \u{acf5}\u{ac04}\u{c774} \u{c784}\u{acc4}\u{ac12} \u{bbf8}\u{b9cc}\u{c774} \u{b418}\u{ba74} \u{d65c}\u{c131}\u{d654}\u{b41c} \u{baa8}\u{b4e0} \u{c5d4}\u{b4dc}\u{d3ec}\u{c778}\u{d2b8}\u{c5d0} \u{c77c}\u{bc18} JSON POST \u{acbd}\u{ace0}\u{b97c} \u{bcf4}\u{b0c5}\u{b2c8}\u{b2e4}.",
    )
}

pub const fn webhook_add_tooltip(locale: Locale) -> &'static str {
    pick(
        locale,
        "Add webhook endpoint",
        "\u{65b0}\u{589e} Webhook \u{7aef}\u{70b9}",
        "Webhook \u{30a8}\u{30f3}\u{30c9}\u{30dd}\u{30a4}\u{30f3}\u{30c8}\u{3092}\u{8ffd}\u{52a0}",
        "Webhook \u{c5d4}\u{b4dc}\u{d3ec}\u{c778}\u{d2b8} \u{cd94}\u{ac00}",
    )
}

pub const fn webhook_none(locale: Locale) -> &'static str {
    pick(
        locale,
        "No webhook endpoints configured yet. Click + to add one.",
        "\u{6682}\u{672a}\u{914d}\u{7f6e} Webhook \u{7aef}\u{70b9}\u{3002}\u{70b9}\u{51fb}\u{53f3}\u{4e0a}\u{89d2} + \u{6dfb}\u{52a0}\u{3002}",
        "Webhook \u{30a8}\u{30f3}\u{30c9}\u{30dd}\u{30a4}\u{30f3}\u{30c8}\u{306f}\u{307e}\u{3060}\u{3042}\u{308a}\u{307e}\u{305b}\u{3093}\u{3002}+ \u{3092}\u{30af}\u{30ea}\u{30c3}\u{30af}\u{3057}\u{3066}\u{8ffd}\u{52a0}\u{3057}\u{3066}\u{304f}\u{3060}\u{3055}\u{3044}\u{3002}",
        "\u{c544}\u{c9c1} \u{ad6c}\u{c131}\u{b41c} Webhook \u{c5d4}\u{b4dc}\u{d3ec}\u{c778}\u{d2b8}\u{ac00} \u{c5c6}\u{c2b5}\u{b2c8}\u{b2e4}. + \u{bc84}\u{d2bc}\u{c744} \u{b20c}\u{b7ec} \u{cd94}\u{ac00}\u{d558}\u{c138}\u{c694}.",
    )
}

pub const fn webhook_form_add_title(locale: Locale) -> &'static str {
    pick(
        locale,
        "Add Webhook Endpoint",
        "\u{65b0}\u{589e} Webhook \u{7aef}\u{70b9}",
        "Webhook \u{30a8}\u{30f3}\u{30c9}\u{30dd}\u{30a4}\u{30f3}\u{30c8}\u{3092}\u{8ffd}\u{52a0}",
        "Webhook \u{c5d4}\u{b4dc}\u{d3ec}\u{c778}\u{d2b8} \u{cd94}\u{ac00}",
    )
}

pub const fn webhook_form_edit_title(locale: Locale) -> &'static str {
    pick(
        locale,
        "Edit Webhook Endpoint",
        "\u{7f16}\u{8f91} Webhook \u{7aef}\u{70b9}",
        "Webhook \u{30a8}\u{30f3}\u{30c9}\u{30dd}\u{30a4}\u{30f3}\u{30c8}\u{3092}\u{7de8}\u{96c6}",
        "Webhook \u{c5d4}\u{b4dc}\u{d3ec}\u{c778}\u{d2b8} \u{d3b8}\u{c9d1}",
    )
}

pub const fn webhook_name_label(locale: Locale) -> &'static str {
    pick(
        locale,
        "Endpoint Name:",
        "\u{7aef}\u{70b9}\u{540d}\u{79f0}\u{ff1a}",
        "\u{30a8}\u{30f3}\u{30c9}\u{30dd}\u{30a4}\u{30f3}\u{30c8}\u{540d}:",
        "\u{c5d4}\u{b4dc}\u{d3ec}\u{c778}\u{d2b8} \u{c774}\u{b984}:",
    )
}

pub const fn placeholder_webhook_name(locale: Locale) -> &'static str {
    pick(
        locale,
        "Ops Alert / Slack Relay",
        "\u{8fd0}\u{7ef4}\u{544a}\u{8b66} / \u{98de}\u{4e66} / \u{9489}\u{9489}",
        "\u{904b}\u{7528}\u{30a2}\u{30e9}\u{30fc}\u{30c8}",
        "\u{c6b4}\u{c601} \u{c54c}\u{b9bc}",
    )
}

pub const fn webhook_save(locale: Locale) -> &'static str {
    pick(
        locale,
        "Save Webhook",
        "\u{4fdd}\u{5b58} Webhook",
        "Webhook \u{3092}\u{4fdd}\u{5b58}",
        "Webhook \u{c800}\u{c7a5}",
    )
}

pub const fn webhook_enabled_badge(locale: Locale) -> &'static str {
    pick(
        locale,
        "Enabled",
        "\u{5df2}\u{542f}\u{7528}",
        "\u{6709}\u{52b9}",
        "\u{d65c}\u{c131}\u{d654}\u{b428}",
    )
}

pub const fn webhook_disabled_badge(locale: Locale) -> &'static str {
    pick(
        locale,
        "Paused",
        "\u{5df2}\u{6682}\u{505c}",
        "\u{505c}\u{6b62}\u{4e2d}",
        "\u{c77c}\u{c2dc} \u{c911}\u{c9c0}\u{b428}",
    )
}

pub const fn webhook_test_tooltip(locale: Locale) -> &'static str {
    pick(
        locale,
        "Send test POST payload",
        "\u{53d1}\u{9001}\u{6d4b}\u{8bd5} POST \u{8bf7}\u{6c42}",
        "\u{30c6}\u{30b9}\u{30c8} POST \u{3092}\u{9001}\u{4fe1}",
        "\u{d14c}\u{c2a4}\u{d2b8} POST \u{c804}\u{c1a1}",
    )
}

pub fn webhook_timeout_summary(secs: u64, locale: Locale) -> String {
    match locale {
        Locale::En => format!("{secs}s timeout"),
        Locale::Zh => format!("{secs} \u{79d2}\u{8d85}\u{65f6}"),
        Locale::Ja => format!(
            "{secs}\u{79d2} \u{30bf}\u{30a4}\u{30e0}\u{30a2}\u{30a6}\u{30c8}"
        ),
        Locale::Ko => {
            format!("{secs}\u{cd08} \u{c81c}\u{d55c} \u{c2dc}\u{ac04}")
        }
    }
}

pub const fn enable_webhook(locale: Locale) -> &'static str {
    pick(
        locale,
        "Enable this webhook endpoint",
        "\u{542f}\u{7528}\u{6b64} Webhook \u{7aef}\u{70b9}",
        "\u{3053}\u{306e} Webhook \u{30a8}\u{30f3}\u{30c9}\u{30dd}\u{30a4}\u{30f3}\u{30c8}\u{3092}\u{6709}\u{52b9}\u{5316}",
        "\u{c774} Webhook \u{c5d4}\u{b4dc}\u{d3ec}\u{c778}\u{d2b8} \u{d65c}\u{c131}\u{d654}",
    )
}

pub const fn webhook_url(locale: Locale) -> &'static str {
    pick(
        locale,
        "Webhook URL (http:// or https://):",
        "Webhook URL\u{ff08}http:// \u{6216} https://\u{ff09}\u{ff1a}",
        "Webhook URL (http:// \u{307e}\u{305f}\u{306f} https://):",
        "Webhook URL (http:// \u{b610}\u{b294} https://):",
    )
}

pub const fn webhook_timeout_secs(locale: Locale) -> &'static str {
    pick(
        locale,
        "Request Timeout (seconds):",
        "\u{8bf7}\u{6c42}\u{8d85}\u{65f6}\u{ff08}\u{79d2}\u{ff09}\u{ff1a}",
        "\u{30bf}\u{30a4}\u{30e0}\u{30a2}\u{30a6}\u{30c8}\u{ff08}\u{79d2}\u{ff09}:",
        "\u{c694}\u{ccad} \u{c81c}\u{d55c} \u{c2dc}\u{ac04}(\u{cd08}):",
    )
}

pub const fn webhook_headers_json(locale: Locale) -> &'static str {
    pick(
        locale,
        "Custom HTTP Headers (JSON object):",
        "\u{81ea}\u{5b9a}\u{4e49} HTTP \u{8bf7}\u{6c42}\u{5934}\u{ff08}JSON \u{5bf9}\u{8c61}\u{ff09}\u{ff1a}",
        "\u{30ab}\u{30b9}\u{30bf}\u{30e0} HTTP \u{30d8}\u{30c3}\u{30c0}\u{30fc} (JSON \u{30aa}\u{30d6}\u{30b8}\u{30a7}\u{30af}\u{30c8}):",
        "\u{c0ac}\u{c6a9}\u{c790} \u{c9c0}\u{c815} HTTP \u{d5e4}\u{b354} (JSON \u{ac1d}\u{ccb4}):",
    )
}

pub const fn placeholder_webhook_url(locale: Locale) -> &'static str {
    pick(
        locale,
        "https://hooks.example.com/ruum",
        "https://hooks.example.com/ruum",
        "https://hooks.example.com/ruum",
        "https://hooks.example.com/ruum",
    )
}

pub const fn placeholder_webhook_headers(locale: Locale) -> &'static str {
    pick(
        locale,
        "{\"Authorization\": \"Bearer ...\"}",
        "{\"Authorization\": \"Bearer ...\"}",
        "{\"Authorization\": \"Bearer ...\"}",
        "{\"Authorization\": \"Bearer ...\"}",
    )
}

pub const fn webhook_guide_title(locale: Locale) -> &'static str {
    pick(
        locale,
        "Webhook Setup Tutorial & JSON Payload Guide",
        "Webhook \u{914d}\u{7f6e}\u{6559}\u{7a0b}\u{4e0e} JSON \u{8f7d}\u{8377}\u{683c}\u{5f0f}",
        "Webhook \u{8a2d}\u{5b9a}\u{30ac}\u{30a4}\u{30c9}\u{3068} JSON \u{30da}\u{30a4}\u{30ed}\u{30fc}\u{30c9}",
        "Webhook \u{c124}\u{c815} \u{ac00}\u{c774}\u{b4dc} \u{bc0f} JSON \u{d398}\u{c774}\u{b85c}\u{b4dc}",
    )
}

pub const fn webhook_guide_flow(locale: Locale) -> &'static str {
    pick(
        locale,
        "Trigger Flow: ruum watchdog (local or headless server) -> HTTP POST \
         JSON to every enabled endpoint",
        "\u{89e6}\u{53d1}\u{6d41}\u{7a0b}\u{ff1a}ruum watchdog\u{ff08}\u{672c}\u{673a}\u{6216}\u{8fdc}\u{7a0b}\u{65e0}\u{754c}\u{9762}\u{670d}\u{52a1}\u{5668}\u{ff09}-> HTTP POST JSON \u{81f3}\u{6240}\u{6709}\u{5df2}\u{542f}\u{7528}\u{7aef}\u{70b9}",
        "\u{30c8}\u{30ea}\u{30ac}\u{30fc}\u{30d5}\u{30ed}\u{30fc}: ruum watchdog -> \u{6709}\u{52b9}\u{306a}\u{3059}\u{3079}\u{3066}\u{306e}\u{30a8}\u{30f3}\u{30c9}\u{30dd}\u{30a4}\u{30f3}\u{30c8}\u{3078} HTTP POST JSON",
        "\u{d2b8}\u{b9ac}\u{ac70} \u{d750}\u{b984}: ruum watchdog -> \u{d65c}\u{c131}\u{d654}\u{b41c} \u{baa8}\u{b4e0} \u{c5d4}\u{b4dc}\u{d3ec}\u{c778}\u{d2b8}\u{b85c} HTTP POST JSON",
    )
}

pub const fn webhook_guide_headers_label(locale: Locale) -> &'static str {
    pick(
        locale,
        "Example Headers JSON:",
        "\u{793a}\u{4f8b} Headers JSON\u{ff1a}",
        "Headers JSON \u{4f8b}:",
        "\u{c608}\u{c2dc} Headers JSON:",
    )
}

pub const fn webhook_guide_payload_label(locale: Locale) -> &'static str {
    pick(
        locale,
        "Payload Schema (event = ruum.disk.low_space):",
        "\u{8f7d}\u{8377} JSON \u{7ed3}\u{6784}\u{ff08}event = ruum.disk.low_space\u{ff09}\u{ff1a}",
        "\u{30da}\u{30a4}\u{30ed}\u{30fc}\u{30c9}\u{4f8b} (event = ruum.disk.low_space):",
        "\u{d398}\u{c774}\u{b85c}\u{b4dc} \u{c2a4}\u{d0a4}\u{b9c8} (event = ruum.disk.low_space):",
    )
}

pub const fn webhook_guide_cli_label(locale: Locale) -> &'static str {
    pick(
        locale,
        "Headless Server CLI Example:",
        "\u{8fdc}\u{7a0b}\u{670d}\u{52a1}\u{5668} CLI \u{914d}\u{7f6e}\u{793a}\u{4f8b}\u{ff1a}",
        "\u{30b5}\u{30fc}\u{30d0}\u{30fc} CLI \u{8a2d}\u{5b9a}\u{4f8b}:",
        "\u{c11c}\u{bc84} CLI \u{c124}\u{c815} \u{c608}\u{c2dc}:",
    )
}
