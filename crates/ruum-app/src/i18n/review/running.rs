//! Running removal, completion summary, and navigation history card strings.

use crate::i18n::{Locale, file_count, item_count_u64, pick};

pub const fn removing_title(locale: Locale) -> &'static str {
    pick(
        locale,
        "Removing",
        "\u{6b63}\u{5728}\u{6e05}\u{7406}",
        "\u{524a}\u{9664}\u{4e2d}",
        "\u{c0ad}\u{c81c} \u{c911}",
    )
}

pub fn removing_subtitle(done: u64, total: usize, locale: Locale) -> String {
    match locale {
        Locale::En => format!("{done} of {total} done"),
        Locale::Zh => format!("\u{5df2}\u{5b8c}\u{6210} {done} / {total}"),
        Locale::Ja => {
            format!("{total} \u{4ef6}\u{4e2d} {done} \u{4ef6}\u{5b8c}\u{4e86}")
        }
        Locale::Ko => {
            format!("{total}\u{ac1c} \u{c911} {done}\u{ac1c} \u{c644}\u{b8cc}")
        }
    }
}

pub const fn progress_label(locale: Locale) -> &'static str {
    pick(
        locale,
        "progress",
        "\u{8fdb}\u{5ea6}",
        "\u{9032}\u{6357}",
        "\u{c9c4}\u{d589}\u{b960}",
    )
}

pub fn progress_value(removed: u64, bytes: &str, locale: Locale) -> String {
    match locale {
        Locale::En => format!("{removed} removed \u{00b7} {bytes} to free"),
        Locale::Zh => format!(
            "\u{5df2}\u{6e05}\u{7406} {removed} \u{9879} \u{00b7} \u{5171} {bytes}"
        ),
        Locale::Ja => {
            format!("{removed} \u{4ef6}\u{524a}\u{9664} \u{00b7} {bytes}")
        }
        Locale::Ko => {
            format!("{removed}\u{ac1c} \u{c0ad}\u{c81c} \u{00b7} {bytes}")
        }
    }
}

pub const fn stop_hint(locale: Locale) -> &'static str {
    pick(
        locale,
        "stop after the current item",
        "\u{5904}\u{7406}\u{5b8c}\u{5f53}\u{524d}\u{9879}\u{76ee}\u{540e}\u{505c}\u{6b62}",
        "\u{73fe}\u{5728}\u{306e}\u{9805}\u{76ee}\u{5b8c}\u{4e86}\u{5f8c}\u{306b}\u{505c}\u{6b62}",
        "\u{d604}\u{c7ac} \u{d56d}\u{baa9} \u{c644}\u{b8cc} \u{d6c4} \u{c911}\u{c9c0}",
    )
}

pub const fn done_title(locale: Locale) -> &'static str {
    pick(
        locale,
        "Done",
        "\u{5b8c}\u{6210}",
        "\u{5b8c}\u{4e86}",
        "\u{c644}\u{b8cc}",
    )
}

pub const fn done_subtitle(locale: Locale) -> &'static str {
    pick(
        locale,
        "removal finished",
        "\u{6e05}\u{7406}\u{5df2}\u{5b8c}\u{6210}",
        "\u{524a}\u{9664}\u{304c}\u{5b8c}\u{4e86}\u{3057}\u{307e}\u{3057}\u{305f}",
        "\u{c0ad}\u{c81c}\u{ac00} \u{c644}\u{b8cc}\u{b418}\u{c5c8}\u{c2b5}\u{b2c8}\u{b2e4}",
    )
}

pub const fn stat_removed(locale: Locale) -> &'static str {
    pick(
        locale,
        "removed",
        "\u{5df2}\u{6e05}\u{7406}",
        "\u{524a}\u{9664}\u{6e08}\u{307f}",
        "\u{c0ad}\u{c81c}\u{b428}",
    )
}

pub fn stat_removed_value(count: u64, locale: Locale) -> String {
    item_count_u64(count, locale)
}

pub const fn stat_bytes_claimed(locale: Locale) -> &'static str {
    pick(
        locale,
        "bytes claimed",
        "\u{6e05}\u{7406}\u{5927}\u{5c0f}",
        "\u{524a}\u{9664}\u{30b5}\u{30a4}\u{30ba}",
        "\u{c0ad}\u{c81c} \u{c6a9}\u{b7c9}",
    )
}

pub const fn stat_failed(locale: Locale) -> &'static str {
    pick(
        locale,
        "failed",
        "\u{5931}\u{8d25}",
        "\u{5931}\u{6557}",
        "\u{c2e4}\u{d328}",
    )
}

pub const fn stat_volume_freed(locale: Locale) -> &'static str {
    pick(
        locale,
        "volume freed",
        "\u{5377}\u{5b9e}\u{9645}\u{91ca}\u{653e}",
        "\u{89e3}\u{653e}\u{3055}\u{308c}\u{305f}\u{5bb9}\u{91cf}",
        "\u{c2e4}\u{c81c} \u{d655}\u{bcf4} \u{c6a9}\u{b7c9}",
    )
}

pub const fn done_rescan_note(locale: Locale) -> &'static str {
    pick(
        locale,
        "The treemap is being re-scanned so the numbers on screen match the \
         disk again.",
        "\u{6b63}\u{5728}\u{91cd}\u{65b0}\u{626b}\u{63cf}\u{6811}\u{72b6}\u{56fe}\u{ff0c}\u{4f7f}\u{5c4f}\u{5e55}\u{6570}\u{636e}\u{4e0e}\u{78c1}\u{76d8}\u{4fdd}\u{6301}\u{4e00}\u{81f4}\u{3002}",
        "\u{753b}\u{9762}\u{306e}\u{6570}\u{5024}\u{3068}\u{30c7}\u{30a3}\u{30b9}\u{30af}\u{3092}\u{4e00}\u{81f4}\u{3055}\u{305b}\u{308b}\u{305f}\u{3081}\u{3001}\u{30c4}\u{30ea}\u{30fc}\u{30de}\u{30c3}\u{30d7}\u{3092}\u{518d}\u{30b9}\u{30ad}\u{30e3}\u{30f3}\u{3057}\u{3066}\u{3044}\u{307e}\u{3059}\u{3002}",
        "\u{d654}\u{ba74} \u{c218}\u{ce58}\u{ac00} \u{b514}\u{c2a4}\u{d06c}\u{c640} \u{b2e4}\u{c2dc} \u{c77c}\u{ce58}\u{d558}\u{b3c4}\u{b85d} \u{d2b8}\u{b9ac}\u{b9f5}\u{c744} \u{c7ac}\u{c2a4}\u{ce94}\u{d558}\u{ace0} \u{c788}\u{c2b5}\u{b2c8}\u{b2e4}.",
    )
}

pub const fn rescanning_label(locale: Locale) -> &'static str {
    pick(
        locale,
        "re-scanning",
        "\u{6b63}\u{5728}\u{91cd}\u{65b0}\u{626b}\u{63cf}",
        "\u{518d}\u{30b9}\u{30ad}\u{30e3}\u{30f3}\u{4e2d}",
        "\u{c7ac}\u{c2a4}\u{ce94} \u{c911}",
    )
}

pub fn rescanning_files(files: u64, locale: Locale) -> String {
    file_count(files, locale)
}

pub const fn what_could_not_be_removed(locale: Locale) -> &'static str {
    pick(
        locale,
        "what could not be removed",
        "\u{672a}\u{80fd}\u{6e05}\u{7406}\u{7684}\u{9879}\u{76ee}",
        "\u{524a}\u{9664}\u{3067}\u{304d}\u{306a}\u{304b}\u{3063}\u{305f}\u{9805}\u{76ee}",
        "\u{c0ad}\u{c81c}\u{d558}\u{c9c0} \u{babb}\u{d55c} \u{d56d}\u{baa9}",
    )
}

pub const fn continue_hint(locale: Locale) -> &'static str {
    pick(
        locale,
        "continue",
        "\u{7ee7}\u{7eed}",
        "\u{7d9a}\u{884c}",
        "\u{acc4}\u{c18d}",
    )
}

pub const fn history_back_label(locale: Locale) -> &'static str {
    pick(
        locale,
        "Back",
        "\u{540e}\u{9000}",
        "\u{623b}\u{308b}",
        "\u{b4a4}\u{b85c}",
    )
}

pub const fn history_back_keys(locale: Locale) -> &'static str {
    pick(
        locale,
        "alt \u{2190} \u{00b7} side button",
        "alt \u{2190} \u{00b7} \u{9f20}\u{6807}\u{4fa7}\u{952e}",
        "alt \u{2190} \u{00b7} \u{30b5}\u{30a4}\u{30c9}\u{30dc}\u{30bf}\u{30f3}",
        "alt \u{2190} \u{00b7} \u{ce21}\u{ba74} \u{bc84}\u{d2bc}",
    )
}

pub const fn history_forward_label(locale: Locale) -> &'static str {
    pick(
        locale,
        "Forward",
        "\u{524d}\u{8fdb}",
        "\u{9032}\u{3080}",
        "\u{c55e}\u{c73c}\u{b85c}",
    )
}

pub const fn history_forward_keys(locale: Locale) -> &'static str {
    pick(
        locale,
        "alt \u{2192} \u{00b7} side button",
        "alt \u{2192} \u{00b7} \u{9f20}\u{6807}\u{4fa7}\u{952e}",
        "alt \u{2192} \u{00b7} \u{30b5}\u{30a4}\u{30c9}\u{30dc}\u{30bf}\u{30f3}",
        "alt \u{2192} \u{00b7} \u{ce21}\u{ba74} \u{bc84}\u{d2bc}",
    )
}
