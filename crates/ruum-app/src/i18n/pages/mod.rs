//! Localized copy for the Icon Rail, Sources & Drives, Watchdog, Settings, and
//! standalone cleanup confirmation window.

mod settings;
mod watchdog;

pub use settings::*;
pub use watchdog::*;

use super::{Locale, pick};

// ── icon rail tooltips ──────────────────────────────────────────────────

pub const fn rail_explore(locale: Locale) -> &'static str {
    pick(
        locale,
        "Explore treemap",
        "\u{6d4f}\u{89c8}\u{6811}\u{72b6}\u{56fe}",
        "\u{30c4}\u{30ea}\u{30fc}\u{30de}\u{30c3}\u{30d7}\u{3092}\u{63a2}\u{7d22}",
        "\u{d2b8}\u{b9ac}\u{b9f5} \u{d0d0}\u{c0c9}",
    )
}

pub const fn rail_sources(locale: Locale) -> &'static str {
    pick(
        locale,
        "Sources & Drives \u{00b7} v",
        "\u{6765}\u{6e90}\u{4e0e}\u{9a71}\u{52a8}\u{5668} \u{00b7} v",
        "\u{30bd}\u{30fc}\u{30b9}\u{3068}\u{30c9}\u{30e9}\u{30a4}\u{30d6} \u{00b7} v",
        "\u{c18c}\u{c2a4} \u{bc0f} \u{b4dc}\u{b77c}\u{c774}\u{be0c} \u{00b7} v",
    )
}

pub const fn rail_watchdog(locale: Locale) -> &'static str {
    pick(
        locale,
        "Watchdog",
        "\u{7a7a}\u{95f4}\u{5b88}\u{62a4}",
        "\u{30a6}\u{30a9}\u{30c3}\u{30c1}\u{30c9}\u{30c3}\u{30b0}",
        "\u{c6cc}\u{ce58}\u{b3c5}",
    )
}

pub const fn rail_settings(is_macos: bool, locale: Locale) -> &'static str {
    if is_macos {
        pick(
            locale,
            "Settings \u{00b7} \u{2318},",
            "\u{8bbe}\u{7f6e} \u{00b7} \u{2318},",
            "\u{8a2d}\u{5b9a} \u{00b7} \u{2318},",
            "\u{c124}\u{c815} \u{00b7} \u{2318},",
        )
    } else {
        pick(
            locale,
            "Settings \u{00b7} Ctrl+,",
            "\u{8bbe}\u{7f6e} \u{00b7} Ctrl+,",
            "\u{8a2d}\u{5b9a} \u{00b7} Ctrl+,",
            "\u{c124}\u{c815} \u{00b7} Ctrl+,",
        )
    }
}

// ── sources & drives ────────────────────────────────────────────────────

pub const fn sources_title(locale: Locale) -> &'static str {
    pick(
        locale,
        "Sources & Drives",
        "\u{6765}\u{6e90}\u{4e0e}\u{9a71}\u{52a8}\u{5668}",
        "\u{30bd}\u{30fc}\u{30b9}\u{3068}\u{30c9}\u{30e9}\u{30a4}\u{30d6}",
        "\u{c18c}\u{c2a4} \u{bc0f} \u{b4dc}\u{b77c}\u{c774}\u{be0c}",
    )
}

pub const fn sources_subtitle_awaiting(locale: Locale) -> &'static str {
    pick(
        locale,
        "Choose a source. Then choose a root to start scanning.",
        "\u{5148}\u{9009}\u{62e9}\u{6765}\u{6e90}\u{ff0c}\u{518d}\u{9009}\u{62e9}\u{8981}\u{5f00}\u{59cb}\u{626b}\u{63cf}\u{7684}\u{6839}\u{76ee}\u{5f55}\u{3002}",
        "\u{30bd}\u{30fc}\u{30b9}\u{3092}\u{9078}\u{629e}\u{3057}\u{3001}\u{30b9}\u{30ad}\u{30e3}\u{30f3}\u{3092}\u{958b}\u{59cb}\u{3059}\u{308b}\u{30eb}\u{30fc}\u{30c8}\u{3092}\u{9078}\u{629e}\u{3057}\u{3066}\u{304f}\u{3060}\u{3055}\u{3044}\u{3002}",
        "\u{c18c}\u{c2a4}\u{b97c} \u{c120}\u{d0dd}\u{d55c} \u{b2e4}\u{c74c} \u{c2a4}\u{ce94}\u{d560} \u{b8e8}\u{d2b8}\u{b97c} \u{c120}\u{d0dd}\u{d558}\u{c138}\u{c694}.",
    )
}

pub const fn sources_subtitle_ready(locale: Locale) -> &'static str {
    pick(
        locale,
        "Connect to an SSH server card, or pick a local mount or directory.",
        "\u{8fde}\u{63a5} SSH \u{670d}\u{52a1}\u{5668}\u{5361}\u{7247}\u{ff0c}\u{6216}\u{9009}\u{62e9}\u{672c}\u{673a}\u{6302}\u{8f7d}\u{70b9}\u{4e0e}\u{76ee}\u{5f55}\u{3002}",
        "SSH \u{30b5}\u{30fc}\u{30d0}\u{30fc}\u{30ab}\u{30fc}\u{30c9}\u{306b}\u{63a5}\u{7d9a}\u{3059}\u{308b}\u{304b}\u{3001}\u{30ed}\u{30fc}\u{30ab}\u{30eb}\u{306e}\u{30de}\u{30a6}\u{30f3}\u{30c8}\u{307e}\u{305f}\u{306f}\u{30c7}\u{30a3}\u{30ec}\u{30af}\u{30c8}\u{30ea}\u{3092}\u{9078}\u{629e}\u{3057}\u{307e}\u{3059}\u{3002}",
        "SSH \u{c11c}\u{bc84} \u{ce74}\u{b4dc}\u{c5d0} \u{c5f0}\u{acb0}\u{d558}\u{ac70}\u{b098} \u{b85c}\u{ceec} \u{b9c8}\u{c6b4}\u{d2b8} \u{b610}\u{b294} \u{b514}\u{b809}\u{d130}\u{b9ac}\u{b97c} \u{c120}\u{d0dd}\u{d558}\u{c138}\u{c694}.",
    )
}

pub const fn this_computer(locale: Locale) -> &'static str {
    pick(
        locale,
        "This computer (Local)",
        "\u{672c}\u{673a}",
        "\u{3053}\u{306e}\u{30b3}\u{30f3}\u{30d4}\u{30e5}\u{30fc}\u{30bf}\u{ff08}\u{30ed}\u{30fc}\u{30ab}\u{30eb}\u{ff09}",
        "\u{c774} \u{cef4}\u{d4e8}\u{d130}(\u{b85c}\u{ceec})",
    )
}

pub const fn listing_mounts(locale: Locale) -> &'static str {
    pick(
        locale,
        "Listing local mounts, not scanning files\u{2026}",
        "\u{6b63}\u{5728}\u{5217}\u{51fa}\u{672c}\u{673a}\u{6302}\u{8f7d}\u{70b9}\u{ff0c}\u{5c1a}\u{672a}\u{626b}\u{63cf}\u{6587}\u{4ef6}\u{2026}",
        "\u{30ed}\u{30fc}\u{30ab}\u{30eb}\u{30de}\u{30a6}\u{30f3}\u{30c8}\u{4e00}\u{89a7}\u{3092}\u{53d6}\u{5f97}\u{4e2d}\u{ff08}\u{30d5}\u{30a1}\u{30a4}\u{30eb}\u{306f}\u{672a}\u{30b9}\u{30ad}\u{30e3}\u{30f3}\u{ff09}\u{2026}",
        "\u{b85c}\u{ceec} \u{b9c8}\u{c6b4}\u{d2b8} \u{baa9}\u{b85d} \u{bd88}\u{b7ec}\u{c624}\u{b294} \u{c911}(\u{d30c}\u{c77c}\u{c740} \u{c2a4}\u{ce94}\u{d558}\u{c9c0} \u{c54a}\u{c74c})\u{2026}",
    )
}

pub const fn no_mounts_listed(locale: Locale) -> &'static str {
    pick(
        locale,
        "No local mounts listed. Enter a directory below.",
        "\u{672a}\u{5217}\u{51fa}\u{672c}\u{673a}\u{6302}\u{8f7d}\u{70b9}\u{ff0c}\u{8bf7}\u{5728}\u{4e0b}\u{65b9}\u{8f93}\u{5165}\u{76ee}\u{5f55}\u{8def}\u{5f84}\u{3002}",
        "\u{30ed}\u{30fc}\u{30ab}\u{30eb}\u{30de}\u{30a6}\u{30f3}\u{30c8}\u{304c}\u{898b}\u{3064}\u{304b}\u{308a}\u{307e}\u{305b}\u{3093}\u{3002}\u{4e0b}\u{306b}\u{30c7}\u{30a3}\u{30ec}\u{30af}\u{30c8}\u{30ea}\u{3092}\u{5165}\u{529b}\u{3057}\u{3066}\u{304f}\u{3060}\u{3055}\u{3044}\u{3002}",
        "\u{d45c}\u{c2dc}\u{b41c} \u{b85c}\u{ceec} \u{b9c8}\u{c6b4}\u{d2b8}\u{ac00} \u{c5c6}\u{c2b5}\u{b2c8}\u{b2e4}. \u{c544}\u{b798}\u{c5d0} \u{b514}\u{b809}\u{d130}\u{b9ac}\u{b97c} \u{c785}\u{b825}\u{d558}\u{c138}\u{c694}.",
    )
}

pub const fn mounted_volume(locale: Locale) -> &'static str {
    pick(
        locale,
        "Mounted volume",
        "\u{5df2}\u{6302}\u{8f7d}\u{5377}",
        "\u{30de}\u{30a6}\u{30f3}\u{30c8}\u{6e08}\u{307f}\u{30dc}\u{30ea}\u{30e5}\u{30fc}\u{30e0}",
        "\u{b9c8}\u{c6b4}\u{d2b8}\u{b41c} \u{bcfc}\u{b968}",
    )
}

pub const fn card_mounts_title(locale: Locale) -> &'static str {
    pick(
        locale,
        "Local Mounts & Root Directory",
        "\u{672c}\u{673a}\u{6302}\u{8f7d}\u{70b9}\u{4e0e}\u{6839}\u{76ee}\u{5f55}",
        "\u{30ed}\u{30fc}\u{30ab}\u{30eb}\u{30de}\u{30a6}\u{30f3}\u{30c8}\u{3068}\u{30eb}\u{30fc}\u{30c8}\u{30c7}\u{30a3}\u{30ec}\u{30af}\u{30c8}\u{30ea}",
        "\u{b85c}\u{ceec} \u{b9c8}\u{c6b4}\u{d2b8} \u{bc0f} \u{b8e8}\u{d2b8} \u{b514}\u{b809}\u{d130}\u{b9ac}",
    )
}

pub const fn card_mounts_desc(locale: Locale) -> &'static str {
    pick(
        locale,
        "Select a local mount or enter an absolute local directory path. \
         Opens in the Local Explore tab.",
        "\u{9009}\u{62e9}\u{4e00}\u{4e2a}\u{672c}\u{673a}\u{6302}\u{8f7d}\u{70b9}\u{6216}\u{8f93}\u{5165}\u{672c}\u{5730}\u{7edd}\u{5bf9}\u{8def}\u{5f84}\u{ff0c}\u{5c06}\u{5728}\u{201c}\u{672c}\u{673a}\u{201d}\u{6807}\u{7b7e}\u{9875}\u{626b}\u{63cf}\u{5e76}\u{6253}\u{5f00}\u{3002}",
        "\u{30ed}\u{30fc}\u{30ab}\u{30eb}\u{30de}\u{30a6}\u{30f3}\u{30c8}\u{307e}\u{305f}\u{306f}\u{7d76}\u{5bfe}\u{30d1}\u{30b9}\u{3092}\u{9078}\u{629e}\u{3057}\u{307e}\u{3059}\u{3002}\u{30ed}\u{30fc}\u{30ab}\u{30eb}\u{30bf}\u{30d6}\u{3067}\u{30b9}\u{30ad}\u{30e3}\u{30f3}\u{3092}\u{958b}\u{59cb}\u{3057}\u{307e}\u{3059}\u{3002}",
        "\u{b85c}\u{ceec} \u{b9c8}\u{c6b4}\u{d2b8} \u{b610}\u{b294} \u{c808}\u{b300} \u{b85c}\u{ceec} \u{acbd}\u{b85c}\u{b97c} \u{c120}\u{d0dd}\u{d558}\u{ba74} \u{b85c}\u{ceec} \u{d0d0}\u{c0c9} \u{d0ed}\u{c5d0}\u{c11c} \u{c2a4}\u{ce94}\u{c744} \u{c2dc}\u{c791}\u{d569}\u{b2c8}\u{b2e4}.",
    )
}

pub const fn scan_directory(locale: Locale) -> &'static str {
    pick(
        locale,
        "Scan directory",
        "\u{626b}\u{63cf}\u{76ee}\u{5f55}",
        "\u{30c7}\u{30a3}\u{30ec}\u{30af}\u{30c8}\u{30ea}\u{3092}\u{30b9}\u{30ad}\u{30e3}\u{30f3}",
        "\u{b514}\u{b809}\u{d130}\u{b9ac} \u{c2a4}\u{ce94}",
    )
}

pub const fn scan_selected_root(locale: Locale) -> &'static str {
    pick(
        locale,
        "Scan selected root",
        "\u{626b}\u{63cf}\u{6240}\u{9009}\u{6839}\u{76ee}\u{5f55}",
        "\u{9078}\u{629e}\u{3057}\u{305f}\u{30eb}\u{30fc}\u{30c8}\u{3092}\u{30b9}\u{30ad}\u{30e3}\u{30f3}",
        "\u{c120}\u{d0dd}\u{d55c} \u{b8e8}\u{d2b8} \u{c2a4}\u{ce94}",
    )
}

// ── SSH server cards & CRUD form ─────────────────────────────────────────

pub const fn ssh_servers_title(locale: Locale) -> &'static str {
    pick(
        locale,
        "Saved SSH Servers",
        "\u{5df2}\u{4fdd}\u{5b58}\u{7684} SSH \u{670d}\u{52a1}\u{5668}",
        "\u{4fdd}\u{5b58}\u{6e08}\u{307f} SSH \u{30b5}\u{30fc}\u{30d0}\u{30fc}",
        "\u{c800}\u{c7a5}\u{b41c} SSH \u{c11c}\u{bc84}",
    )
}

pub const fn ssh_servers_desc(locale: Locale) -> &'static str {
    pick(
        locale,
        "Metadata lives in SQLite; secrets stay encrypted until you connect.",
        "\u{5143}\u{6570}\u{636e}\u{4fdd}\u{5b58}\u{5728} SQLite \u{4e2d}\u{ff0c}\u{5bc6}\u{94a5}\u{4ec5}\u{5728}\u{8fde}\u{63a5}\u{6216}\u{4fdd}\u{5b58}\u{65f6}\u{6309}\u{9700}\u{8bbf}\u{95ee}\u{94a5}\u{5319}\u{4e32}\u{3002}",
        "\u{30e1}\u{30bf}\u{30c7}\u{30fc}\u{30bf}\u{306f} SQLite \u{306b}\u{4fdd}\u{5b58}\u{3055}\u{308c}\u{3001}\u{8a8d}\u{8a3c}\u{60c5}\u{5831}\u{306f}\u{63a5}\u{7d9a}\u{6642}\u{306b}\u{306e}\u{307f}\u{5fa9}\u{53f7}\u{3055}\u{308c}\u{307e}\u{3059}\u{3002}",
        "\u{ba54}\u{d0c0}\u{b370}\u{c774}\u{d130}\u{b294} SQLite\u{c5d0} \u{c800}\u{c7a5}\u{b418}\u{ba70}, \u{c790}\u{aca9} \u{c99d}\u{ba85}\u{c740} \u{c5f0}\u{acb0} \u{c2dc}\u{c5d0}\u{b9cc} \u{bcf5}\u{d638}\u{d654}\u{b429}\u{b2c8}\u{b2e4}.",
    )
}

pub const fn add_ssh_server(locale: Locale) -> &'static str {
    pick(
        locale,
        "Add Server",
        "\u{65b0}\u{589e}\u{670d}\u{52a1}\u{5668}",
        "\u{30b5}\u{30fc}\u{30d0}\u{30fc}\u{3092}\u{8ffd}\u{52a0}",
        "\u{c11c}\u{bc84} \u{cd94}\u{ac00}",
    )
}

pub const fn edit_ssh_server(locale: Locale) -> &'static str {
    pick(
        locale,
        "Edit",
        "\u{7f16}\u{8f91}",
        "\u{7de8}\u{96c6}",
        "\u{d3b8}\u{c9d1}",
    )
}

pub const fn delete_ssh_server(locale: Locale) -> &'static str {
    pick(
        locale,
        "Delete",
        "\u{5220}\u{9664}",
        "\u{524a}\u{9664}",
        "\u{c0ad}\u{c81c}",
    )
}

pub const fn connect_and_scan(locale: Locale) -> &'static str {
    pick(
        locale,
        "Connect & Scan",
        "\u{8fde}\u{63a5}\u{5e76}\u{626b}\u{63cf}",
        "\u{63a5}\u{7d9a}\u{3057}\u{3066}\u{30b9}\u{30ad}\u{30e3}\u{30f3}",
        "\u{c5f0}\u{acb0} \u{bc0f} \u{c2a4}\u{ce94}",
    )
}

pub const fn ssh_check_space_tooltip(locale: Locale) -> &'static str {
    pick(
        locale,
        "Check remote disk space",
        "\u{68c0}\u{67e5}\u{8fdc}\u{7a0b}\u{78c1}\u{76d8}\u{7a7a}\u{95f4}",
        "\u{30ea}\u{30e2}\u{30fc}\u{30c8}\u{30c7}\u{30a3}\u{30b9}\u{30af}\u{7a7a}\u{304d}\u{5bb9}\u{91cf}\u{3092}\u{78ba}\u{8a8d}",
        "\u{c6d0}\u{aca9} \u{b514}\u{c2a4}\u{d06c} \u{acf5}\u{ac04} \u{d655}\u{c778}",
    )
}

pub const fn ssh_ai_analyze_tooltip(locale: Locale) -> &'static str {
    pick(
        locale,
        "Run remote AI cleanup analysis",
        "\u{8fd0}\u{884c}\u{8fdc}\u{7a0b} AI \u{6e05}\u{7406}\u{5206}\u{6790}",
        "\u{30ea}\u{30e2}\u{30fc}\u{30c8} AI \u{30af}\u{30ea}\u{30fc}\u{30f3}\u{30a2}\u{30c3}\u{30d7}\u{5206}\u{6790}\u{3092}\u{5b9f}\u{884c}",
        "\u{c6d0}\u{aca9} AI \u{c815}\u{b9ac} \u{bd84}\u{c11d} \u{c2e4}\u{d589}",
    )
}

pub const fn no_ssh_servers(locale: Locale) -> &'static str {
    pick(
        locale,
        "No saved SSH servers yet. Click Add Server to store one.",
        "\u{6682}\u{65e0}\u{5df2}\u{4fdd}\u{5b58}\u{7684} SSH \u{670d}\u{52a1}\u{5668}\u{3002}\u{70b9}\u{51fb}\u{201c}\u{65b0}\u{589e}\u{670d}\u{52a1}\u{5668}\u{201d}\u{6dfb}\u{52a0}\u{914d}\u{7f6e}\u{3002}",
        "\u{4fdd}\u{5b58}\u{6e08}\u{307f}\u{306e} SSH \u{30b5}\u{30fc}\u{30d0}\u{30fc}\u{306f}\u{307e}\u{3060}\u{3042}\u{308a}\u{307e}\u{305b}\u{3093}\u{3002}\u{300c}\u{30b5}\u{30fc}\u{30d0}\u{30fc}\u{3092}\u{8ffd}\u{52a0}\u{300d}\u{304b}\u{3089}\u{767b}\u{9332}\u{3067}\u{304d}\u{307e}\u{3059}\u{3002}",
        "\u{c800}\u{c7a5}\u{b41c} SSH \u{c11c}\u{bc84}\u{ac00} \u{c5c6}\u{c2b5}\u{b2c8}\u{b2e4}. \u{c11c}\u{bc84} \u{cd94}\u{ac00}\u{b97c} \u{b20c}\u{b7ec} \u{b4f1}\u{b85d}\u{d558}\u{c138}\u{c694}.",
    )
}

pub const fn auth_badge_private_key(locale: Locale) -> &'static str {
    pick(
        locale,
        "Private Key",
        "\u{79c1}\u{94a5}",
        "\u{79d8}\u{5bc6}\u{9375}",
        "\u{ac1c}\u{c778} \u{d0a4}",
    )
}

pub const fn auth_badge_password(locale: Locale) -> &'static str {
    pick(
        locale,
        "Password",
        "\u{5bc6}\u{7801}",
        "\u{30d1}\u{30b9}\u{30ef}\u{30fc}\u{30c9}",
        "\u{be44}\u{bc00}\u{bc88}\u{d638}",
    )
}

pub fn ssh_default_path_display(path: &str, locale: Locale) -> String {
    match locale {
        Locale::En => format!("Default path: {path}"),
        Locale::Zh => format!("\u{9ed8}\u{8ba4}\u{8def}\u{5f84}\u{ff1a}{path}"),
        Locale::Ja => format!(
            "\u{30c7}\u{30d5}\u{30a9}\u{30eb}\u{30c8}\u{30d1}\u{30b9}: {path}"
        ),
        Locale::Ko => format!("\u{ae30}\u{bcf8} \u{acbd}\u{b85c}: {path}"),
    }
}

pub fn ssh_cached_space(
    available: &str,
    total: &str,
    age: &str,
    locale: Locale,
) -> String {
    match locale {
        Locale::En => {
            format!("Last scan: {available} free of {total} ({age})")
        }
        Locale::Zh => format!(
            "\u{4e0a}\u{6b21}\u{626b}\u{63cf}\u{ff1a}\u{53ef}\u{7528} {available} / \u{5171} {total}\u{ff08}{age}\u{ff09}"
        ),
        Locale::Ja => format!(
            "\u{524d}\u{56de}\u{30b9}\u{30ad}\u{30e3}\u{30f3}: \u{7a7a}\u{304d} {available} / {total} ({age})"
        ),
        Locale::Ko => format!(
            "\u{cd5c}\u{adfc} \u{c2a4}\u{ce94}: \u{c5ec}\u{c720} {available} / {total} ({age})"
        ),
    }
}

pub const fn ssh_never_scanned(locale: Locale) -> &'static str {
    pick(
        locale,
        "Not scanned yet",
        "\u{5c1a}\u{672a}\u{626b}\u{63cf}",
        "\u{672a}\u{30b9}\u{30ad}\u{30e3}\u{30f3}",
        "\u{c544}\u{c9c1} \u{c2a4}\u{ce94}\u{b418}\u{c9c0} \u{c54a}\u{c74c}",
    )
}

pub const fn ssh_form_add_title(locale: Locale) -> &'static str {
    pick(
        locale,
        "Add SSH Server",
        "\u{65b0}\u{589e} SSH \u{670d}\u{52a1}\u{5668}",
        "SSH \u{30b5}\u{30fc}\u{30d0}\u{30fc}\u{3092}\u{8ffd}\u{52a0}",
        "SSH \u{c11c}\u{bc84} \u{cd94}\u{ac00}",
    )
}

pub const fn ssh_form_edit_title(locale: Locale) -> &'static str {
    pick(
        locale,
        "Edit SSH Server",
        "\u{7f16}\u{8f91} SSH \u{670d}\u{52a1}\u{5668}",
        "SSH \u{30b5}\u{30fc}\u{30d0}\u{30fc}\u{3092}\u{7de8}\u{96c6}",
        "SSH \u{c11c}\u{bc84} \u{d3b8}\u{c9d1}",
    )
}

pub const fn ssh_form_alias(locale: Locale) -> &'static str {
    pick(
        locale,
        "Display Alias:",
        "\u{663e}\u{793a}\u{522b}\u{540d}\u{ff1a}",
        "\u{8868}\u{793a}\u{540d}\u{ff08}\u{30a8}\u{30a4}\u{30ea}\u{30a2}\u{30b9}\u{ff09}:",
        "\u{d45c}\u{c2dc} \u{bcc4}\u{ce6d}:",
    )
}

pub const fn ssh_form_host(locale: Locale) -> &'static str {
    pick(
        locale,
        "Host / IP:",
        "\u{4e3b}\u{673a} / IP\u{ff1a}",
        "\u{30db}\u{30b9}\u{30c8} / IP:",
        "\u{d638}\u{c2a4}\u{d2b8} / IP:",
    )
}

pub const fn ssh_form_port(locale: Locale) -> &'static str {
    pick(
        locale,
        "Port:",
        "\u{7aef}\u{53e3}\u{ff1a}",
        "\u{30dd}\u{30fc}\u{30c8}:",
        "\u{d3ec}\u{d2b8}:",
    )
}

pub const fn ssh_form_username(locale: Locale) -> &'static str {
    pick(
        locale,
        "Username:",
        "\u{7528}\u{6237}\u{540d}\u{ff1a}",
        "\u{30e6}\u{30fc}\u{30b6}\u{30fc}\u{540d}:",
        "\u{c0ac}\u{c6a9}\u{c790} \u{c774}\u{b984}:",
    )
}

pub const fn ssh_form_default_path(locale: Locale) -> &'static str {
    pick(
        locale,
        "Default Remote Path (optional):",
        "\u{9ed8}\u{8ba4}\u{8fdc}\u{7a0b}\u{8def}\u{5f84}\u{ff08}\u{53ef}\u{9009}\u{ff09}\u{ff1a}",
        "\u{30c7}\u{30d5}\u{30a9}\u{30eb}\u{30c8}\u{306e}\u{30ea}\u{30e2}\u{30fc}\u{30c8}\u{30d1}\u{30b9}\u{ff08}\u{4efb}\u{610f}\u{ff09}:",
        "\u{ae30}\u{bcf8} \u{c6d0}\u{aca9} \u{acbd}\u{b85c}(\u{c120}\u{d0dd} \u{c0ac}\u{d56d}):",
    )
}

pub const fn ssh_form_auth_mode(locale: Locale) -> &'static str {
    pick(
        locale,
        "Authentication Mode:",
        "\u{8ba4}\u{8bc1}\u{65b9}\u{5f0f}\u{ff1a}",
        "\u{8a8d}\u{8a3c}\u{65b9}\u{5f0f}:",
        "\u{c778}\u{c99d} \u{bc29}\u{c2dd}:",
    )
}

pub const fn ssh_form_password(locale: Locale) -> &'static str {
    pick(
        locale,
        "Password:",
        "\u{5bc6}\u{7801}\u{ff1a}",
        "\u{30d1}\u{30b9}\u{30ef}\u{30fc}\u{30c9}:",
        "\u{be44}\u{bc00}\u{bc88}\u{d638}:",
    )
}

pub const fn ssh_form_private_key(locale: Locale) -> &'static str {
    pick(
        locale,
        "Private Key (file path or PEM):",
        "\u{79c1}\u{94a5}\u{ff08}\u{6587}\u{4ef6}\u{8def}\u{5f84}\u{6216} PEM \u{5185}\u{5bb9}\u{ff09}\u{ff1a}",
        "\u{79d8}\u{5bc6}\u{9375}\u{ff08}\u{30d5}\u{30a1}\u{30a4}\u{30eb}\u{30d1}\u{30b9}\u{307e}\u{305f}\u{306f} PEM\u{ff09}:",
        "\u{ac1c}\u{c778} \u{d0a4}(\u{d30c}\u{c77c} \u{acbd}\u{b85c} \u{b610}\u{b294} PEM):",
    )
}

pub const fn ssh_form_passphrase(locale: Locale) -> &'static str {
    pick(
        locale,
        "Key Passphrase (optional):",
        "\u{79c1}\u{94a5}\u{53e3}\u{4ee4}\u{ff08}\u{53ef}\u{9009}\u{ff09}\u{ff1a}",
        "\u{9375}\u{30d1}\u{30b9}\u{30d5}\u{30ec}\u{30fc}\u{30ba}\u{ff08}\u{4efb}\u{610f}\u{ff09}:",
        "\u{d0a4} \u{c554}\u{d638} \u{bb38}\u{ad6c}(\u{c120}\u{d0dd} \u{c0ac}\u{d56d}):",
    )
}

pub const fn ssh_form_save(locale: Locale) -> &'static str {
    pick(
        locale,
        "Save Server",
        "\u{4fdd}\u{5b58}\u{670d}\u{52a1}\u{5668}",
        "\u{30b5}\u{30fc}\u{30d0}\u{30fc}\u{3092}\u{4fdd}\u{5b58}",
        "\u{c11c}\u{bc84} \u{c800}\u{c7a5}",
    )
}

pub const fn placeholder_ssh_alias(locale: Locale) -> &'static str {
    pick(locale, "prod-web", "prod-web", "prod-web", "prod-web")
}

pub const fn placeholder_ssh_host_addr(locale: Locale) -> &'static str {
    pick(
        locale,
        "10.0.0.12 or host.example.com",
        "10.0.0.12 \u{6216} host.example.com",
        "10.0.0.12 \u{307e}\u{305f}\u{306f} host.example.com",
        "10.0.0.12 \u{b610}\u{b294} host.example.com",
    )
}

pub const fn placeholder_ssh_username(locale: Locale) -> &'static str {
    pick(locale, "ubuntu", "ubuntu", "ubuntu", "ubuntu")
}

pub const fn placeholder_ssh_default_path(locale: Locale) -> &'static str {
    pick(locale, "~", "~", "~", "~")
}

pub const fn placeholder_ssh_password(locale: Locale) -> &'static str {
    pick(
        locale,
        "SSH password",
        "\u{8f93}\u{5165} SSH \u{5bc6}\u{7801}",
        "SSH \u{30d1}\u{30b9}\u{30ef}\u{30fc}\u{30c9}",
        "SSH \u{be44}\u{bc00}\u{bc88}\u{d638}",
    )
}

pub const fn placeholder_ssh_private_key(locale: Locale) -> &'static str {
    pick(
        locale,
        "/path/to/id_ed25519 or -----BEGIN OPENSSH PRIVATE KEY-----",
        "/path/to/id_ed25519 \u{6216} -----BEGIN OPENSSH PRIVATE KEY-----",
        "/path/to/id_ed25519 \u{307e}\u{305f}\u{306f} -----BEGIN OPENSSH PRIVATE KEY-----",
        "/path/to/id_ed25519 \u{b610}\u{b294} -----BEGIN OPENSSH PRIVATE KEY-----",
    )
}

pub const fn placeholder_ssh_passphrase(locale: Locale) -> &'static str {
    pick(
        locale,
        "Optional passphrase",
        "\u{53ef}\u{9009}\u{53e3}\u{4ee4}",
        "\u{4efb}\u{610f}\u{306e}\u{30d1}\u{30b9}\u{30d5}\u{30ec}\u{30fc}\u{30ba}",
        "\u{c120}\u{d0dd} \u{c554}\u{d638} \u{bb38}\u{ad6c}",
    )
}

pub const fn placeholder_ssh_secret_keep(locale: Locale) -> &'static str {
    pick(
        locale,
        "Leave empty to keep saved credential",
        "\u{7559}\u{7a7a}\u{5219}\u{4fdd}\u{7559}\u{5df2}\u{5b58}\u{51ed}\u{636e}",
        "\u{7a7a}\u{6b04}\u{306e}\u{307e}\u{307e}\u{3067}\u{4fdd}\u{5b58}\u{6e08}\u{307f}\u{8a8d}\u{8a3c}\u{60c5}\u{5831}\u{3092}\u{7dad}\u{6301}",
        "\u{be44}\u{c6cc} \u{b450}\u{ba74} \u{c800}\u{c7a5}\u{b41c} \u{c790}\u{aca9} \u{c99d}\u{ba85}\u{c744} \u{c720}\u{c9c0}\u{d569}\u{b2c8}\u{b2e4}",
    )
}

// ── remote connect confirmation modal ───────────────────────────────────

pub const fn remote_connect_title(locale: Locale) -> &'static str {
    pick(
        locale,
        "Approve Remote SSH Connection?",
        "\u{662f}\u{5426}\u{5141}\u{8bb8}\u{8fde}\u{63a5}\u{8fdc}\u{7a0b} SSH \u{670d}\u{52a1}\u{5668}\u{ff1f}",
        "\u{30ea}\u{30e2}\u{30fc}\u{30c8} SSH \u{63a5}\u{7d9a}\u{3092}\u{8a31}\u{53ef}\u{3057}\u{307e}\u{3059}\u{304b}\u{ff1f}",
        "\u{c6d0}\u{aca9} SSH \u{c5f0}\u{acb0}\u{c744} \u{c2b9}\u{c778}\u{d558}\u{c2dc}\u{aca0}\u{c2b5}\u{b2c8}\u{ae4c}?",
    )
}

pub fn remote_connect_desc(tool_name: &str, locale: Locale) -> String {
    match locale {
        Locale::En => format!(
            "An MCP tool ({tool_name}) requested access to open an SSH \
             session. Credentials will only be decrypted if you approve."
        ),
        Locale::Zh => format!(
            "MCP \u{5de5}\u{5177}\u{ff08}{tool_name}\u{ff09}\u{8bf7}\u{6c42}\u{5efa}\u{7acb}\u{8fdc}\u{7a0b} SSH \u{4f1a}\u{8bdd}\u{3002}\u{4ec5}\u{5728}\u{60a8}\u{5141}\u{8bb8}\u{540e}\u{624d}\u{4f1a}\u{89e3}\u{5bc6}\u{51ed}\u{636e}\u{5e76}\u{53d1}\u{8d77}\u{8fde}\u{63a5}\u{3002}"
        ),
        Locale::Ja => format!(
            "MCP \u{30c4}\u{30fc}\u{30eb}\u{ff08}{tool_name}\u{ff09}\u{304c} SSH \u{30bb}\u{30c3}\u{30b7}\u{30e7}\u{30f3}\u{306e}\u{958b}\u{59cb}\u{3092}\u{8981}\u{6c42}\u{3057}\u{3066}\u{3044}\u{307e}\u{3059}\u{3002}\u{8a31}\u{53ef}\u{3057}\u{305f}\u{5834}\u{5408}\u{306e}\u{307f}\u{8a8d}\u{8a3c}\u{60c5}\u{5831}\u{304c}\u{5fa9}\u{53f7}\u{3055}\u{308c}\u{307e}\u{3059}\u{3002}"
        ),
        Locale::Ko => format!(
            "MCP \u{b3c4}\u{ad6c}({tool_name})\u{ac00} SSH \u{c138}\u{c158} \u{c5f0}\u{acb0}\u{c744} \u{c694}\u{ccad}\u{d588}\u{c2b5}\u{b2c8}\u{b2e4}. \u{c2b9}\u{c778}\u{d55c} \u{acbd}\u{c6b0}\u{c5d0}\u{b9cc} \u{c790}\u{aca9} \u{c99d}\u{ba85}\u{c774} \u{bcf5}\u{d638}\u{d654}\u{b429}\u{b2c8}\u{b2e4}."
        ),
    }
}

pub const fn remote_connect_target_label(locale: Locale) -> &'static str {
    pick(
        locale,
        "Target:",
        "\u{76ee}\u{6807}\u{522b}\u{540d}\u{ff1a}",
        "\u{63a5}\u{7d9a}\u{5148}:",
        "\u{b300}\u{c0c1}:",
    )
}

pub const fn remote_connect_endpoint_label(locale: Locale) -> &'static str {
    pick(
        locale,
        "Endpoint:",
        "\u{7ec8}\u{7aef}\u{5730}\u{5740}\u{ff1a}",
        "\u{30a8}\u{30f3}\u{30c9}\u{30dd}\u{30a4}\u{30f3}\u{30c8}:",
        "\u{c5d4}\u{b4dc}\u{d3ec}\u{c778}\u{d2b8}:",
    )
}

pub const fn remote_connect_auth_label(locale: Locale) -> &'static str {
    pick(
        locale,
        "Auth Mode:",
        "\u{8ba4}\u{8bc1}\u{65b9}\u{5f0f}\u{ff1a}",
        "\u{8a8d}\u{8a3c}\u{65b9}\u{5f0f}:",
        "\u{c778}\u{c99d} \u{bc29}\u{c2dd}:",
    )
}

pub const fn remote_connect_tool_label(locale: Locale) -> &'static str {
    pick(
        locale,
        "Requested by Tool:",
        "\u{89e6}\u{53d1}\u{5de5}\u{5177}\u{ff1a}",
        "\u{8981}\u{6c42}\u{5143}\u{30c4}\u{30fc}\u{30eb}:",
        "\u{c694}\u{ccad} \u{b3c4}\u{ad6c}:",
    )
}

pub const fn remote_connect_approve(locale: Locale) -> &'static str {
    pick(
        locale,
        "Allow Connection",
        "\u{5141}\u{8bb8}\u{8fde}\u{63a5}",
        "\u{63a5}\u{7d9a}\u{3092}\u{8a31}\u{53ef}",
        "\u{c5f0}\u{acb0} \u{d5c8}\u{c6a9}",
    )
}

pub const fn remote_connect_deny(locale: Locale) -> &'static str {
    pick(
        locale,
        "Deny",
        "\u{62d2}\u{7edd}",
        "\u{62d2}\u{5426}",
        "\u{ac70}\u{bd80}",
    )
}
