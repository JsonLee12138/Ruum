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
        "Connect, then select a mount or directory. No new scan starts until \
         you choose Scan.",
        "\u{8fde}\u{63a5}\u{540e}\u{9009}\u{62e9}\u{4e00}\u{4e2a}\u{6302}\u{8f7d}\u{70b9}\u{6216}\u{76ee}\u{5f55}\u{3002}\u{5728}\u{70b9}\u{51fb}\u{201c}\u{626b}\u{63cf}\u{201d}\u{524d}\u{4e0d}\u{4f1a}\u{542f}\u{52a8}\u{65b0}\u{626b}\u{63cf}\u{3002}",
        "\u{63a5}\u{7d9a}\u{5f8c}\u{3001}\u{30de}\u{30a6}\u{30f3}\u{30c8}\u{307e}\u{305f}\u{306f}\u{30c7}\u{30a3}\u{30ec}\u{30af}\u{30c8}\u{30ea}\u{3092}\u{9078}\u{629e}\u{3057}\u{3066}\u{304f}\u{3060}\u{3055}\u{3044}\u{3002}\u{30b9}\u{30ad}\u{30e3}\u{30f3}\u{3092}\u{9078}\u{629e}\u{3059}\u{308b}\u{307e}\u{3067}\u{958b}\u{59cb}\u{3055}\u{308c}\u{307e}\u{305b}\u{3093}\u{3002}",
        "\u{c5f0}\u{acb0}\u{d55c} \u{d6c4} \u{b9c8}\u{c6b4}\u{d2b8} \u{b610}\u{b294} \u{b514}\u{b809}\u{d130}\u{b9ac}\u{b97c} \u{c120}\u{d0dd}\u{d558}\u{c138}\u{c694}. \u{c2a4}\u{ce94}\u{c744} \u{c120}\u{d0dd}\u{d558}\u{ae30} \u{c804}\u{c5d0}\u{b294} \u{c0c8} \u{c2a4}\u{ce94}\u{c774} \u{c2dc}\u{c791}\u{b418}\u{c9c0} \u{c54a}\u{c2b5}\u{b2c8}\u{b2e4}.",
    )
}

pub const fn this_computer(locale: Locale) -> &'static str {
    pick(
        locale,
        "This computer",
        "\u{672c}\u{673a}",
        "\u{3053}\u{306e}\u{30b3}\u{30f3}\u{30d4}\u{30e5}\u{30fc}\u{30bf}",
        "\u{c774} \u{cef4}\u{d4e8}\u{d130}",
    )
}

pub fn ssh_read_only(host: &str, locale: Locale) -> String {
    match locale {
        Locale::En => format!("{host} \u{00b7} SSH \u{00b7} read-only"),
        Locale::Zh => format!("{host} \u{00b7} SSH \u{00b7} \u{53ea}\u{8bfb}"),
        Locale::Ja => format!(
            "{host} \u{00b7} SSH \u{00b7} \u{8aad}\u{307f}\u{53d6}\u{308a}\u{5c02}\u{7528}"
        ),
        Locale::Ko => format!(
            "{host} \u{00b7} SSH \u{00b7} \u{c77d}\u{ae30} \u{c804}\u{c6a9}"
        ),
    }
}

pub const fn listing_mounts(locale: Locale) -> &'static str {
    pick(
        locale,
        "Listing mounts, not scanning files\u{2026}",
        "\u{6b63}\u{5728}\u{5217}\u{51fa}\u{6302}\u{8f7d}\u{70b9}\u{ff0c}\u{5c1a}\u{672a}\u{626b}\u{63cf}\u{6587}\u{4ef6}\u{2026}",
        "\u{30de}\u{30a6}\u{30f3}\u{30c8}\u{4e00}\u{89a7}\u{3092}\u{53d6}\u{5f97}\u{4e2d}\u{ff08}\u{30d5}\u{30a1}\u{30a4}\u{30eb}\u{306f}\u{672a}\u{30b9}\u{30ad}\u{30e3}\u{30f3}\u{ff09}\u{2026}",
        "\u{b9c8}\u{c6b4}\u{d2b8} \u{baa9}\u{b85d} \u{bd88}\u{b7ec}\u{c624}\u{b294} \u{c911}(\u{d30c}\u{c77c}\u{c740} \u{c2a4}\u{ce94}\u{d558}\u{c9c0} \u{c54a}\u{c74c})\u{2026}",
    )
}

pub const fn no_mounts_listed(locale: Locale) -> &'static str {
    pick(
        locale,
        "No mounts listed. Enter a directory below.",
        "\u{672a}\u{5217}\u{51fa}\u{6302}\u{8f7d}\u{70b9}\u{ff0c}\u{8bf7}\u{5728}\u{4e0b}\u{65b9}\u{8f93}\u{5165}\u{76ee}\u{5f55}\u{8def}\u{5f84}\u{3002}",
        "\u{30de}\u{30a6}\u{30f3}\u{30c8}\u{304c}\u{898b}\u{3064}\u{304b}\u{308a}\u{307e}\u{305b}\u{3093}\u{3002}\u{4e0b}\u{306b}\u{30c7}\u{30a3}\u{30ec}\u{30af}\u{30c8}\u{30ea}\u{3092}\u{5165}\u{529b}\u{3057}\u{3066}\u{304f}\u{3060}\u{3055}\u{3044}\u{3002}",
        "\u{d45c}\u{c2dc}\u{b41c} \u{b9c8}\u{c6b4}\u{d2b8}\u{ac00} \u{c5c6}\u{c2b5}\u{b2c8}\u{b2e4}. \u{c544}\u{b798}\u{c5d0} \u{b514}\u{b809}\u{d130}\u{b9ac}\u{b97c} \u{c785}\u{b825}\u{d558}\u{c138}\u{c694}.",
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

pub const fn card_source_title(locale: Locale) -> &'static str {
    pick(
        locale,
        "Source",
        "\u{6765}\u{6e90}",
        "\u{30bd}\u{30fc}\u{30b9}",
        "\u{c18c}\u{c2a4}",
    )
}

pub const fn card_source_desc(locale: Locale) -> &'static str {
    pick(
        locale,
        "Choose this computer or connect to a remote host over SSH.",
        "\u{9009}\u{62e9}\u{672c}\u{673a}\u{ff0c}\u{6216}\u{901a}\u{8fc7} SSH \u{8fde}\u{63a5}\u{5230}\u{8fdc}\u{7a0b}\u{4e3b}\u{673a}\u{3002}",
        "\u{3053}\u{306e}\u{30b3}\u{30f3}\u{30d4}\u{30e5}\u{30fc}\u{30bf}\u{3092}\u{9078}\u{629e}\u{3059}\u{308b}\u{304b}\u{3001}SSH \u{7d4c}\u{7531}\u{3067}\u{30ea}\u{30e2}\u{30fc}\u{30c8}\u{30db}\u{30b9}\u{30c8}\u{306b}\u{63a5}\u{7d9a}\u{3057}\u{307e}\u{3059}\u{3002}",
        "\u{c774} \u{cef4}\u{d4e8}\u{d130}\u{b97c} \u{c120}\u{d0dd}\u{d558}\u{ac70}\u{b098} SSH\u{b85c} \u{c6d0}\u{aca9} \u{d638}\u{c2a4}\u{d2b8}\u{c5d0} \u{c5f0}\u{acb0}\u{d558}\u{c138}\u{c694}.",
    )
}

pub const fn connect_ssh(locale: Locale) -> &'static str {
    pick(
        locale,
        "Connect SSH",
        "\u{8fde}\u{63a5} SSH",
        "SSH \u{63a5}\u{7d9a}",
        "SSH \u{c5f0}\u{acb0}",
    )
}

pub const fn card_mounts_title(locale: Locale) -> &'static str {
    pick(
        locale,
        "Mounts & Root Directory",
        "\u{6302}\u{8f7d}\u{70b9}\u{4e0e}\u{6839}\u{76ee}\u{5f55}",
        "\u{30de}\u{30a6}\u{30f3}\u{30c8}\u{3068}\u{30eb}\u{30fc}\u{30c8}\u{30c7}\u{30a3}\u{30ec}\u{30af}\u{30c8}\u{30ea}",
        "\u{b9c8}\u{c6b4}\u{d2b8} \u{bc0f} \u{b8e8}\u{d2b8} \u{b514}\u{b809}\u{d130}\u{b9ac}",
    )
}

pub const fn card_mounts_desc(locale: Locale) -> &'static str {
    pick(
        locale,
        "Select a mount or enter an absolute directory path. Nested mounts \
         and symlinks are not traversed; space is measured only after \
         selection.",
        "\u{9009}\u{62e9}\u{4e00}\u{4e2a}\u{6302}\u{8f7d}\u{70b9}\u{6216}\u{8f93}\u{5165}\u{7edd}\u{5bf9}\u{76ee}\u{5f55}\u{8def}\u{5f84}\u{3002}\u{4e0d}\u{4f1a}\u{8de8}\u{8d8a}\u{5d4c}\u{5957}\u{6302}\u{8f7d}\u{70b9}\u{6216}\u{7b26}\u{53f7}\u{94fe}\u{63a5}\u{ff1b}\u{4ec5}\u{5728}\u{9009}\u{62e9}\u{540e}\u{7edf}\u{8ba1}\u{7a7a}\u{95f4}\u{3002}",
        "\u{30de}\u{30a6}\u{30f3}\u{30c8}\u{3092}\u{9078}\u{629e}\u{3059}\u{308b}\u{304b}\u{3001}\u{7d76}\u{5bfe}\u{30c7}\u{30a3}\u{30ec}\u{30af}\u{30c8}\u{30ea}\u{30d1}\u{30b9}\u{3092}\u{5165}\u{529b}\u{3057}\u{3066}\u{304f}\u{3060}\u{3055}\u{3044}\u{3002}\u{30cd}\u{30b9}\u{30c8}\u{3057}\u{305f}\u{30de}\u{30a6}\u{30f3}\u{30c8}\u{3084}\u{30b7}\u{30f3}\u{30dc}\u{30ea}\u{30c3}\u{30af}\u{30ea}\u{30f3}\u{30af}\u{306f}\u{8fbf}\u{3089}\u{305a}\u{3001}\u{9078}\u{629e}\u{5f8c}\u{306b}\u{306e}\u{307f}\u{5bb9}\u{91cf}\u{3092}\u{8a08}\u{6e2c}\u{3057}\u{307e}\u{3059}\u{3002}",
        "\u{b9c8}\u{c6b4}\u{d2b8}\u{b97c} \u{c120}\u{d0dd}\u{d558}\u{ac70}\u{b098} \u{c808}\u{b300} \u{b514}\u{b809}\u{d130}\u{b9ac} \u{acbd}\u{b85c}\u{b97c} \u{c785}\u{b825}\u{d558}\u{c138}\u{c694}. \u{c911}\u{cca9} \u{b9c8}\u{c6b4}\u{d2b8}\u{c640} \u{c2ec}\u{bcfc}\u{b9ad} \u{b9c1}\u{d06c}\u{b294} \u{b530}\u{b77c}\u{ac00}\u{c9c0} \u{c54a}\u{c73c}\u{ba70} \u{c120}\u{d0dd} \u{d6c4}\u{c5d0}\u{b9cc} \u{c6a9}\u{b7c9}\u{c744} \u{ce21}\u{c815}\u{d569}\u{b2c8}\u{b2e4}.",
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
