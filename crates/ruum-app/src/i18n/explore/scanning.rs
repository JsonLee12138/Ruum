//! Scanning panel, find bar, hover tooltip, and node card strings.

use ruum_core::size::human_count;

use crate::i18n::{Locale, dir_count, file_count, pick};

pub const fn scan_failed(locale: Locale) -> &'static str {
    pick(
        locale,
        "Scan failed",
        "\u{626b}\u{63cf}\u{5931}\u{8d25}",
        "\u{30b9}\u{30ad}\u{30e3}\u{30f3}\u{5931}\u{6557}",
        "\u{c2a4}\u{ce94} \u{c2e4}\u{d328}",
    )
}

pub fn last_directory(path: &str, locale: Locale) -> String {
    match locale {
        Locale::En => format!("Last directory: {path}"),
        Locale::Zh => format!("\u{6700}\u{540e}\u{76ee}\u{5f55}\u{ff1a}{path}"),
        Locale::Ja => format!(
            "\u{6700}\u{5f8c}\u{306e}\u{30c7}\u{30a3}\u{30ec}\u{30af}\u{30c8}\u{30ea}: {path}"
        ),
        Locale::Ko => format!(
            "\u{b9c8}\u{c9c0}\u{b9c9} \u{b514}\u{b809}\u{d130}\u{b9ac}: {path}"
        ),
    }
}

pub const fn retry_scan(locale: Locale) -> &'static str {
    pick(
        locale,
        "retry scan",
        "\u{91cd}\u{65b0}\u{626b}\u{63cf}",
        "\u{518d}\u{30b9}\u{30ad}\u{30e3}\u{30f3}",
        "\u{b2e4}\u{c2dc} \u{c2a4}\u{ce94}",
    )
}

pub fn stopped_reading(root: &str, locale: Locale) -> String {
    match locale {
        Locale::En => format!("Stopped reading {root}"),
        Locale::Zh => {
            format!("\u{5df2}\u{505c}\u{6b62}\u{8bfb}\u{53d6} {root}")
        }
        Locale::Ja => format!(
            "{root} \u{306e}\u{8aad}\u{307f}\u{53d6}\u{308a}\u{3092}\u{505c}\u{6b62}"
        ),
        Locale::Ko => {
            format!("{root} \u{c77d}\u{ae30} \u{c911}\u{c9c0}\u{b428}")
        }
    }
}

pub fn reading_root(root: &str, locale: Locale) -> String {
    match locale {
        Locale::En => format!("Reading {root}"),
        Locale::Zh => format!("\u{6b63}\u{5728}\u{8bfb}\u{53d6} {root}"),
        Locale::Ja => {
            format!("{root} \u{3092}\u{8aad}\u{307f}\u{53d6}\u{308a}\u{4e2d}")
        }
        Locale::Ko => format!("{root} \u{c77d}\u{b294} \u{c911}"),
    }
}

pub const fn stat_files(locale: Locale) -> &'static str {
    pick(
        locale,
        "files",
        "\u{6587}\u{4ef6}",
        "\u{30d5}\u{30a1}\u{30a4}\u{30eb}",
        "\u{d30c}\u{c77c}",
    )
}

pub const fn stat_directories(locale: Locale) -> &'static str {
    pick(
        locale,
        "directories",
        "\u{76ee}\u{5f55}",
        "\u{30c7}\u{30a3}\u{30ec}\u{30af}\u{30c8}\u{30ea}",
        "\u{b514}\u{b809}\u{d130}\u{b9ac}",
    )
}

pub const fn stat_measured(locale: Locale) -> &'static str {
    pick(
        locale,
        "measured",
        "\u{5df2}\u{7edf}\u{8ba1}",
        "\u{8a08}\u{6e2c}\u{6e08}\u{307f}",
        "\u{ce21}\u{c815}\u{b428}",
    )
}

pub const fn stat_unreadable(locale: Locale) -> &'static str {
    pick(
        locale,
        "unreadable",
        "\u{4e0d}\u{53ef}\u{8bfb}",
        "\u{8aad}\u{307f}\u{53d6}\u{308a}\u{4e0d}\u{53ef}",
        "\u{c77d}\u{ae30} \u{bd88}\u{ac00}",
    )
}

pub fn recent_directory(path: &str, locale: Locale) -> String {
    match locale {
        Locale::En => format!("Recent directory: {path}"),
        Locale::Zh => format!("\u{5f53}\u{524d}\u{76ee}\u{5f55}\u{ff1a}{path}"),
        Locale::Ja => format!(
            "\u{73fe}\u{5728}\u{306e}\u{30c7}\u{30a3}\u{30ec}\u{30af}\u{30c8}\u{30ea}: {path}"
        ),
        Locale::Ko => {
            format!("\u{cd5c}\u{adfc} \u{b514}\u{b809}\u{d130}\u{b9ac}: {path}")
        }
    }
}

pub const fn scan_cancelled_note(locale: Locale) -> &'static str {
    pick(
        locale,
        "Nothing is shown from a scan that did not finish.",
        "\u{672a}\u{5b8c}\u{6210}\u{7684}\u{626b}\u{63cf}\u{4e0d}\u{4f1a}\u{663e}\u{793a}\u{5c40}\u{90e8}\u{7ed3}\u{679c}\u{3002}",
        "\u{672a}\u{5b8c}\u{4e86}\u{306e}\u{30b9}\u{30ad}\u{30e3}\u{30f3}\u{7d50}\u{679c}\u{306f}\u{8868}\u{793a}\u{3055}\u{308c}\u{307e}\u{305b}\u{3093}\u{3002}",
        "\u{c644}\u{b8cc}\u{b418}\u{c9c0} \u{c54a}\u{c740} \u{c2a4}\u{ce94} \u{acb0}\u{acfc}\u{b294} \u{d45c}\u{c2dc}\u{b418}\u{c9c0} \u{c54a}\u{c2b5}\u{b2c8}\u{b2e4}.",
    )
}

pub const fn scan_running_note(locale: Locale) -> &'static str {
    pick(
        locale,
        "Marking, zooming and the free-space meter all work as soon as it \
         lands.",
        "\u{626b}\u{63cf}\u{5b8c}\u{6210}\u{540e}\u{5373}\u{53ef}\u{4f7f}\u{7528}\u{6807}\u{8bb0}\u{3001}\u{7f29}\u{653e}\u{548c}\u{53ef}\u{7528}\u{7a7a}\u{95f4}\u{8ba1}\u{91cf}\u{3002}",
        "\u{30b9}\u{30ad}\u{30e3}\u{30f3}\u{5b8c}\u{4e86}\u{5f8c}\u{3059}\u{3050}\u{306b}\u{30de}\u{30fc}\u{30af}\u{30fb}\u{30ba}\u{30fc}\u{30e0}\u{30fb}\u{7a7a}\u{304d}\u{5bb9}\u{91cf}\u{8868}\u{793a}\u{304c}\u{4f7f}\u{3048}\u{307e}\u{3059}\u{3002}",
        "\u{c2a4}\u{ce94}\u{c774} \u{c644}\u{b8cc}\u{b418}\u{ba74} \u{d45c}\u{c2dc}, \u{d655}\u{b300}/\u{cd95}\u{c18c}, \u{c5ec}\u{c720} \u{acf5}\u{ac04} \u{ce21}\u{c815}\u{c774} \u{bc14}\u{b85c} \u{c791}\u{b3d9}\u{d569}\u{b2c8}\u{b2e4}.",
    )
}

pub const fn scan_again(locale: Locale) -> &'static str {
    pick(
        locale,
        "Scan again",
        "\u{91cd}\u{65b0}\u{626b}\u{63cf}",
        "\u{518d}\u{30b9}\u{30ad}\u{30e3}\u{30f3}",
        "\u{b2e4}\u{c2dc} \u{c2a4}\u{ce94}",
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

pub const fn find_searching(locale: Locale) -> &'static str {
    pick(
        locale,
        "searching\u{2026}",
        "\u{641c}\u{7d22}\u{4e2d}\u{2026}",
        "\u{691c}\u{7d22}\u{4e2d}\u{2026}",
        "\u{ac80}\u{c0c9} \u{c911}\u{2026}",
    )
}

pub const fn find_type_to_filter(locale: Locale) -> &'static str {
    pick(
        locale,
        "type to filter",
        "\u{8f93}\u{5165}\u{4ee5}\u{8fc7}\u{6ee4}",
        "\u{5165}\u{529b}\u{3057}\u{3066}\u{7d5e}\u{308a}\u{8fbc}\u{307f}",
        "\u{c785}\u{b825}\u{d558}\u{c5ec} \u{d544}\u{d130}\u{b9c1}",
    )
}

pub const fn find_no_matches(locale: Locale) -> &'static str {
    pick(
        locale,
        "no matches",
        "\u{65e0}\u{5339}\u{914d}",
        "\u{4e00}\u{81f4}\u{306a}\u{3057}",
        "\u{c77c}\u{ce58} \u{d56d}\u{baa9} \u{c5c6}\u{c74c}",
    )
}

pub const fn find_esc_clears(locale: Locale) -> &'static str {
    pick(
        locale,
        "esc clears",
        "esc \u{6e05}\u{9664}",
        "esc \u{30af}\u{30ea}\u{30a2}",
        "esc \u{c9c0}\u{c6b0}\u{ae30}",
    )
}

pub const fn find_enter_shows_only(locale: Locale) -> &'static str {
    pick(
        locale,
        "enter shows only these",
        "enter \u{4ec5}\u{663e}\u{793a}\u{5339}\u{914d}\u{9879}",
        "enter \u{4e00}\u{81f4}\u{306e}\u{307f}\u{8868}\u{793a}",
        "enter \u{c77c}\u{ce58} \u{d56d}\u{baa9}\u{b9cc} \u{d45c}\u{c2dc}",
    )
}

pub fn find_matches(count: usize, bytes: &str, locale: Locale) -> String {
    let formatted = human_count(count as u64);
    match locale {
        Locale::En => {
            let es = if count == 1 { "" } else { "es" };
            format!("{formatted} match{es} \u{00b7} {bytes}")
        }
        Locale::Zh => {
            format!("{formatted} \u{4e2a}\u{5339}\u{914d} \u{00b7} {bytes}")
        }
        Locale::Ja => {
            format!("{formatted} \u{4ef6}\u{4e00}\u{81f4} \u{00b7} {bytes}")
        }
        Locale::Ko => {
            format!("{formatted}\u{ac1c} \u{c77c}\u{ce58} \u{00b7} {bytes}")
        }
    }
}

pub const fn find_placeholder(locale: Locale) -> &'static str {
    pick(
        locale,
        "Filter by name",
        "\u{6309}\u{540d}\u{79f0}\u{8fc7}\u{6ee4}",
        "\u{540d}\u{524d}\u{3067}\u{7d5e}\u{308a}\u{8fbc}\u{307f}",
        "\u{c774}\u{b984}\u{c73c}\u{b85c} \u{d544}\u{d130}",
    )
}

pub const fn hover_keys(is_dir: bool, locale: Locale) -> &'static str {
    if is_dir {
        pick(
            locale,
            "space mark \u{00b7} enter open",
            "space \u{6807}\u{8bb0} \u{00b7} enter \u{6253}\u{5f00}",
            "space \u{30de}\u{30fc}\u{30af} \u{00b7} enter \u{958b}\u{304f}",
            "space \u{d45c}\u{c2dc} \u{00b7} enter \u{c5f4}\u{ae30}",
        )
    } else {
        pick(
            locale,
            "space mark",
            "space \u{6807}\u{8bb0}",
            "space \u{30de}\u{30fc}\u{30af}",
            "space \u{d45c}\u{c2dc}",
        )
    }
}

pub fn node_card_counts(
    files: u64,
    dirs: u64,
    direct_bytes: &str,
    locale: Locale,
) -> String {
    let f = file_count(files, locale);
    let d = dir_count(dirs, locale);
    match locale {
        Locale::En => {
            format!("{f} \u{00b7} {d} \u{00b7} {direct_bytes} direct")
        }
        Locale::Zh => {
            format!("{f} \u{00b7} {d} \u{00b7} \u{76f4}\u{5c5e} {direct_bytes}")
        }
        Locale::Ja => {
            format!("{f} \u{00b7} {d} \u{00b7} \u{76f4}\u{4e0b} {direct_bytes}")
        }
        Locale::Ko => {
            format!("{f} \u{00b7} {d} \u{00b7} \u{c9c1}\u{c811} {direct_bytes}")
        }
    }
}

pub const fn badge_hidden(locale: Locale) -> &'static str {
    pick(
        locale,
        "Hidden",
        "\u{9690}\u{85cf}",
        "\u{96a0}\u{3057}",
        "\u{c228}\u{ae40}",
    )
}

pub const fn badge_marked_for_removal(locale: Locale) -> &'static str {
    pick(
        locale,
        "Marked for removal",
        "\u{5df2}\u{6807}\u{8bb0}\u{5f85}\u{6e05}\u{7406}",
        "\u{524a}\u{9664}\u{5bfe}\u{8c61}\u{306b}\u{30de}\u{30fc}\u{30af}",
        "\u{c0ad}\u{c81c} \u{b300}\u{c0c1}\u{c73c}\u{b85c} \u{d45c}\u{c2dc}\u{b428}",
    )
}

pub fn badge_inside_marked(ancestor: &str, locale: Locale) -> String {
    match locale {
        Locale::En => format!("Inside marked {ancestor}"),
        Locale::Zh => format!(
            "\u{4f4d}\u{4e8e}\u{5df2}\u{6807}\u{8bb0}\u{7684} {ancestor} \u{5185}"
        ),
        Locale::Ja => format!(
            "\u{30de}\u{30fc}\u{30af}\u{6e08}\u{307f}\u{306e} {ancestor} \u{5185}"
        ),
        Locale::Ko => {
            format!("\u{d45c}\u{c2dc}\u{b41c} {ancestor} \u{b0b4}\u{bd80}")
        }
    }
}
