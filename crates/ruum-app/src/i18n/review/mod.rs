//! Localized copy for the Review, Delete dialog, Running, Done, and History
//! card UI surfaces.

mod blocked;
mod running;

pub use blocked::*;
pub use running::*;

use super::{Locale, pick};

// ── delete confirmation dialog ──────────────────────────────────────────

pub fn delete_title_single(name: &str, locale: Locale) -> String {
    match locale {
        Locale::En => format!("Delete \u{201c}{name}\u{201d} permanently?"),
        Locale::Zh => format!(
            "\u{6c38}\u{4e45}\u{5220}\u{9664}\u{201c}{name}\u{201d}\u{ff1f}"
        ),
        Locale::Ja => format!(
            "\u{300c}{name}\u{300d}\u{3092}\u{5b8c}\u{5168}\u{306b}\u{524a}\u{9664}\u{3057}\u{307e}\u{3059}\u{304b}\u{ff1f}"
        ),
        Locale::Ko => format!(
            "\u{201c}{name}\u{201d}\u{c744}(\u{b97c}) \u{c601}\u{ad6c} \u{c0ad}\u{c81c}\u{d560}\u{ae4c}\u{c694}?"
        ),
    }
}

pub fn delete_title_many(count: usize, locale: Locale) -> String {
    match locale {
        Locale::En => format!("Delete {count} items permanently?"),
        Locale::Zh => format!(
            "\u{6c38}\u{4e45}\u{5220}\u{9664} {count} \u{4e2a}\u{9879}\u{76ee}\u{ff1f}"
        ),
        Locale::Ja => format!(
            "{count} \u{4ef6}\u{306e}\u{9805}\u{76ee}\u{3092}\u{5b8c}\u{5168}\u{306b}\u{524a}\u{9664}\u{3057}\u{307e}\u{3059}\u{304b}\u{ff1f}"
        ),
        Locale::Ko => format!(
            "{count}\u{ac1c} \u{d56d}\u{baa9}\u{c744} \u{c601}\u{ad6c} \u{c0ad}\u{c81c}\u{d560}\u{ae4c}\u{c694}?"
        ),
    }
}

pub fn delete_body(bytes: &str, locale: Locale) -> String {
    match locale {
        Locale::En => format!(
            "This frees {bytes}. Deleted files can\u{2019}t be recovered; \
             move them to the trash if you might need them again."
        ),
        Locale::Zh => format!(
            "\u{8fd9}\u{5c06}\u{91ca}\u{653e} {bytes}\u{3002}\u{5df2}\u{5220}\u{9664}\u{7684}\u{6587}\u{4ef6}\u{65e0}\u{6cd5}\u{6062}\u{590d}\u{ff1b}\u{5982}\u{679c}\u{65e5}\u{540e}\u{53ef}\u{80fd}\u{8fd8}\u{9700}\u{8981}\u{ff0c}\
             \u{8bf7}\u{6539}\u{4e3a}\u{79fb}\u{81f3}\u{5e9f}\u{7eb8}\u{7bd3}\u{3002}"
        ),
        Locale::Ja => format!(
            "{bytes} \u{304c}\u{89e3}\u{653e}\u{3055}\u{308c}\u{307e}\u{3059}\u{3002}\u{524a}\u{9664}\u{3055}\u{308c}\u{305f}\u{30d5}\u{30a1}\u{30a4}\u{30eb}\u{306f}\u{5fa9}\u{5143}\u{3067}\u{304d}\u{307e}\u{305b}\u{3093}\u{3002}\
             \u{5fc5}\u{8981}\u{306a}\u{5834}\u{5408}\u{306f}\u{30b4}\u{30df}\u{7bb1}\u{3078}\u{79fb}\u{52d5}\u{3057}\u{3066}\u{304f}\u{3060}\u{3055}\u{3044}\u{3002}"
        ),
        Locale::Ko => format!(
            "{bytes}\u{ac00} \u{d655}\u{bcf4}\u{b429}\u{b2c8}\u{b2e4}. \u{c0ad}\u{c81c}\u{b41c} \u{d30c}\u{c77c}\u{c740} \u{bcf5}\u{ad6c}\u{d560} \u{c218} \u{c5c6}\u{c2b5}\u{b2c8}\u{b2e4}. \
             \u{b2e4}\u{c2dc} \u{d544}\u{c694}\u{d560} \u{c218} \u{c788}\u{b2e4}\u{ba74} \u{d734}\u{c9c0}\u{d1b5}\u{c73c}\u{b85c} \u{c774}\u{b3d9}\u{d558}\u{c138}\u{c694}."
        ),
    }
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

pub const fn delete_confirm(locale: Locale) -> &'static str {
    pick(
        locale,
        "Delete",
        "\u{5220}\u{9664}",
        "\u{524a}\u{9664}",
        "\u{c0ad}\u{c81c}",
    )
}

// ── review screen ───────────────────────────────────────────────────────

pub const fn review_title(locale: Locale) -> &'static str {
    pick(
        locale,
        "Review",
        "\u{5ba1}\u{6838}\u{6e05}\u{7406}",
        "\u{78ba}\u{8a8d}",
        "\u{ac80}\u{d1a0}",
    )
}

pub fn review_subtitle(count: usize, bytes: &str, locale: Locale) -> String {
    match locale {
        Locale::En => format!("{count} marked \u{00b7} {bytes} to free"),
        Locale::Zh => format!(
            "\u{5df2}\u{6807}\u{8bb0} {count} \u{9879} \u{00b7} \u{5c06}\u{91ca}\u{653e} {bytes}"
        ),
        Locale::Ja => format!(
            "{count} \u{4ef6}\u{30de}\u{30fc}\u{30af}\u{6e08}\u{307f} \u{00b7} {bytes} \u{89e3}\u{653e}\u{4e88}\u{5b9a}"
        ),
        Locale::Ko => format!(
            "{count}\u{ac1c} \u{d45c}\u{c2dc}\u{b428} \u{00b7} {bytes} \u{d655}\u{bcf4} \u{c608}\u{c815}"
        ),
    }
}

pub const fn review_empty(locale: Locale) -> &'static str {
    pick(
        locale,
        "Nothing is marked. Go back and mark what should go.",
        "\u{5c1a}\u{672a}\u{6807}\u{8bb0}\u{4efb}\u{4f55}\u{9879}\u{76ee}\u{3002}\u{8fd4}\u{56de}\u{5e76}\u{6807}\u{8bb0}\u{9700}\u{8981}\u{6e05}\u{7406}\u{7684}\u{5185}\u{5bb9}\u{3002}",
        "\u{30de}\u{30fc}\u{30af}\u{3055}\u{308c}\u{305f}\u{9805}\u{76ee}\u{304c}\u{3042}\u{308a}\u{307e}\u{305b}\u{3093}\u{3002}\u{623b}\u{3063}\u{3066}\u{524a}\u{9664}\u{5bfe}\u{8c61}\u{3092}\u{30de}\u{30fc}\u{30af}\u{3057}\u{3066}\u{304f}\u{3060}\u{3055}\u{3044}\u{3002}",
        "\u{d45c}\u{c2dc}\u{b41c} \u{d56d}\u{baa9}\u{c774} \u{c5c6}\u{c2b5}\u{b2c8}\u{b2e4}. \u{b3cc}\u{c544}\u{ac00}\u{c11c} \u{c0ad}\u{c81c}\u{d560} \u{d56d}\u{baa9}\u{c744} \u{d45c}\u{c2dc}\u{d558}\u{c138}\u{c694}.",
    )
}

pub fn review_list_truncated(extra: usize, locale: Locale) -> String {
    match locale {
        Locale::En => format!(
            "{extra} more are marked and will be removed too. Unmark them in \
             the treemap."
        ),
        Locale::Zh => format!(
            "\u{8fd8}\u{6709} {extra} \u{4e2a}\u{5df2}\u{6807}\u{8bb0}\u{9879}\u{76ee}\u{540c}\u{6837}\u{4f1a}\u{88ab}\u{79fb}\u{9664}\u{3002}\u{5982}\u{9700}\u{4fdd}\u{7559}\u{8bf7}\u{5728}\u{6811}\u{72b6}\u{56fe}\u{4e2d}\
             \u{53d6}\u{6d88}\u{6807}\u{8bb0}\u{3002}"
        ),
        Locale::Ja => format!(
            "\u{307b}\u{304b} {extra} \u{4ef6}\u{3082}\u{30de}\u{30fc}\u{30af}\u{3055}\u{308c}\u{3066}\u{304a}\u{308a}\u{524a}\u{9664}\u{3055}\u{308c}\u{307e}\u{3059}\u{3002}\u{30c4}\u{30ea}\u{30fc}\u{30de}\u{30c3}\u{30d7}\u{3067}\
             \u{30de}\u{30fc}\u{30af}\u{3092}\u{89e3}\u{9664}\u{3057}\u{3066}\u{304f}\u{3060}\u{3055}\u{3044}\u{3002}"
        ),
        Locale::Ko => format!(
            "\u{d45c}\u{c2dc}\u{b41c} \u{d56d}\u{baa9}\u{c774} {extra}\u{ac1c} \u{b354} \u{c788}\u{c73c}\u{ba70} \u{d568}\u{aed8} \u{c0ad}\u{c81c}\u{b429}\u{b2c8}\u{b2e4}. \u{d2b8}\u{b9ac}\u{b9f5}\u{c5d0}\u{c11c} \
             \u{d45c}\u{c2dc}\u{b97c} \u{d574}\u{c81c}\u{d558}\u{c138}\u{c694}."
        ),
    }
}

pub const fn inside_marked_dir(locale: Locale) -> &'static str {
    pick(
        locale,
        "Inside a marked directory",
        "\u{4f4d}\u{4e8e}\u{5df2}\u{6807}\u{8bb0}\u{76ee}\u{5f55}\u{5185}",
        "\u{30de}\u{30fc}\u{30af}\u{6e08}\u{307f}\u{30c7}\u{30a3}\u{30ec}\u{30af}\u{30c8}\u{30ea}\u{5185}",
        "\u{d45c}\u{c2dc}\u{b41c} \u{b514}\u{b809}\u{d130}\u{b9ac} \u{b0b4}\u{bd80}",
    )
}

pub const fn unmark(locale: Locale) -> &'static str {
    pick(
        locale,
        "Unmark",
        "\u{53d6}\u{6d88}\u{6807}\u{8bb0}",
        "\u{30de}\u{30fc}\u{30af}\u{89e3}\u{9664}",
        "\u{d45c}\u{c2dc} \u{d574}\u{c81c}",
    )
}

pub const fn not_counted_label(locale: Locale) -> &'static str {
    pick(
        locale,
        "Not counted here",
        "\u{672a}\u{8ba1}\u{5165}\u{6b64}\u{5377}",
        "\u{3053}\u{306e}\u{30dc}\u{30ea}\u{30e5}\u{30fc}\u{30e0}\u{306b}\u{542b}\u{307e}\u{306a}\u{3044}",
        "\u{c5ec}\u{ae30} \u{bbf8}\u{d3ec}\u{d568}",
    )
}

pub const fn mode_trash(locale: Locale) -> &'static str {
    pick(
        locale,
        "Move to trash",
        "\u{79fb}\u{81f3}\u{5e9f}\u{7eb8}\u{7bd3}",
        "\u{30b4}\u{30df}\u{7bb1}\u{3078}\u{79fb}\u{52d5}",
        "\u{d734}\u{c9c0}\u{d1b5}\u{c73c}\u{b85c} \u{c774}\u{b3d9}",
    )
}

pub const fn mode_permanent(locale: Locale) -> &'static str {
    pick(
        locale,
        "Delete permanently",
        "\u{6c38}\u{4e45}\u{5220}\u{9664}",
        "\u{5b8c}\u{5168}\u{306b}\u{524a}\u{9664}",
        "\u{c601}\u{ad6c} \u{c0ad}\u{c81c}",
    )
}

pub const fn explanation_remote(locale: Locale) -> &'static str {
    pick(
        locale,
        "Read-only SSH session. Export the selection; nothing is deleted \
         here. Free space is sampled at scan time.",
        "\u{53ea}\u{8bfb} SSH \u{4f1a}\u{8bdd}\u{3002}\u{53ef}\u{5bfc}\u{51fa}\u{5df2}\u{9009}\u{5217}\u{8868}\u{ff1b}\u{6b64}\u{5904}\u{4e0d}\u{4f1a}\u{5220}\u{9664}\u{4efb}\u{4f55}\u{6587}\u{4ef6}\u{3002}\u{53ef}\u{7528}\u{7a7a}\u{95f4}\u{4e3a}\
         \u{626b}\u{63cf}\u{65f6}\u{7684}\u{5feb}\u{7167}\u{3002}",
        "\u{8aad}\u{307f}\u{53d6}\u{308a}\u{5c02}\u{7528} SSH \u{30bb}\u{30c3}\u{30b7}\u{30e7}\u{30f3}\u{3067}\u{3059}\u{3002}\u{9078}\u{629e}\u{9805}\u{76ee}\u{3092}\u{30a8}\u{30af}\u{30b9}\u{30dd}\u{30fc}\u{30c8}\u{3067}\u{304d}\u{3001}\
         \u{3053}\u{3053}\u{3067}\u{306f}\u{524a}\u{9664}\u{3055}\u{308c}\u{307e}\u{305b}\u{3093}\u{3002}\u{7a7a}\u{304d}\u{5bb9}\u{91cf}\u{306f}\u{30b9}\u{30ad}\u{30e3}\u{30f3}\u{6642}\u{70b9}\u{306e}\u{5024}\u{3067}\u{3059}\u{3002}",
        "\u{c77d}\u{ae30} \u{c804}\u{c6a9} SSH \u{c138}\u{c158}\u{c785}\u{b2c8}\u{b2e4}. \u{c120}\u{d0dd} \u{d56d}\u{baa9}\u{c744} \u{b0b4}\u{bcf4}\u{b0bc} \u{c218} \u{c788}\u{c73c}\u{ba70} \u{c5ec}\u{ae30}\u{c11c}\u{b294} \
         \u{c0ad}\u{c81c}\u{b418}\u{c9c0} \u{c54a}\u{c2b5}\u{b2c8}\u{b2e4}. \u{c5ec}\u{c720} \u{acf5}\u{ac04}\u{c740} \u{c2a4}\u{ce94} \u{c2dc}\u{c810} \u{ae30}\u{c900}\u{c785}\u{b2c8}\u{b2e4}.",
    )
}

pub fn explanation_trash(backend_label: &str, locale: Locale) -> String {
    match locale {
        Locale::En => format!(
            "Recoverable from the trash until it is emptied. Uses \
             {backend_label}."
        ),
        Locale::Zh => format!(
            "\u{5728}\u{6e05}\u{7a7a}\u{524d}\u{53ef}\u{4ece}\u{5e9f}\u{7eb8}\u{7bd3}\u{6062}\u{590d}\u{3002}\u{4f7f}\u{7528} {backend_label}\u{3002}"
        ),
        Locale::Ja => format!(
            "\u{7a7a}\u{306b}\u{3059}\u{308b}\u{307e}\u{3067}\u{30b4}\u{30df}\u{7bb1}\u{304b}\u{3089}\u{5fa9}\u{5143}\u{3067}\u{304d}\u{307e}\u{3059}\u{3002}{backend_label} \u{3092}\u{4f7f}\u{7528}\u{3057}\u{307e}\u{3059}\u{3002}"
        ),
        Locale::Ko => format!(
            "\u{d734}\u{c9c0}\u{d1b5}\u{c744} \u{be44}\u{c6b0}\u{ae30} \u{c804}\u{ae4c}\u{c9c0} \u{bcf5}\u{ad6c}\u{d560} \u{c218} \u{c788}\u{c2b5}\u{b2c8}\u{b2e4}. {backend_label} \u{c0ac}\u{c6a9}."
        ),
    }
}

pub const fn explanation_permanent(locale: Locale) -> &'static str {
    pick(
        locale,
        "Deleted at once, like rm -rf. Nothing is recoverable.",
        "\u{7acb}\u{5373}\u{5220}\u{9664}\u{ff0c}\u{7b49}\u{540c}\u{4e8e} rm -rf\u{ff0c}\u{65e0}\u{6cd5}\u{6062}\u{590d}\u{3002}",
        "rm -rf \u{3068}\u{540c}\u{69d8}\u{306b}\u{5373}\u{6642}\u{524a}\u{9664}\u{3055}\u{308c}\u{3001}\u{5fa9}\u{5143}\u{3067}\u{304d}\u{307e}\u{305b}\u{3093}\u{3002}",
        "rm -rf\u{cc98}\u{b7fc} \u{c989}\u{c2dc} \u{c0ad}\u{c81c}\u{b418}\u{ba70} \u{bcf5}\u{ad6c}\u{d560} \u{c218} \u{c5c6}\u{c2b5}\u{b2c8}\u{b2e4}.",
    )
}

pub const fn section_what_happens(locale: Locale) -> &'static str {
    pick(
        locale,
        "What happens",
        "\u{6e05}\u{7406}\u{65b9}\u{5f0f}",
        "\u{5b9f}\u{884c}\u{5185}\u{5bb9}",
        "\u{cc98}\u{b9ac} \u{bc29}\u{c2dd}",
    )
}

pub const fn section_totals(locale: Locale) -> &'static str {
    pick(
        locale,
        "Totals",
        "\u{6c47}\u{603b}",
        "\u{5408}\u{8a08}",
        "\u{d569}\u{acc4}",
    )
}

pub const fn row_marked(locale: Locale) -> &'static str {
    pick(
        locale,
        "Marked",
        "\u{5df2}\u{6807}\u{8bb0}",
        "\u{30de}\u{30fc}\u{30af}\u{6e08}\u{307f}",
        "\u{d45c}\u{c2dc}\u{b428}",
    )
}

pub const fn row_acted_on(locale: Locale) -> &'static str {
    pick(
        locale,
        "Acted on",
        "\u{5c06}\u{6267}\u{884c}",
        "\u{5b9f}\u{884c}\u{5bfe}\u{8c61}",
        "\u{c2e4}\u{d589} \u{b300}\u{c0c1}",
    )
}

pub const fn row_nested(locale: Locale) -> &'static str {
    pick(
        locale,
        "Nested, go with a parent",
        "\u{5d4c}\u{5957}\u{ff0c}\u{968f}\u{7236}\u{76ee}\u{5f55}\u{6e05}\u{7406}",
        "\u{89aa}\u{30c7}\u{30a3}\u{30ec}\u{30af}\u{30c8}\u{30ea}\u{3068}\u{4e00}\u{62ec}\u{524a}\u{9664}",
        "\u{d558}\u{c704} \u{d56d}\u{baa9}(\u{c0c1}\u{c704}\u{c640} \u{d568}\u{aed8} \u{c0ad}\u{c81c})",
    )
}

pub const fn row_kept_back(locale: Locale) -> &'static str {
    pick(
        locale,
        "Kept back",
        "\u{53d7}\u{4fdd}\u{62a4}\u{4fdd}\u{7559}",
        "\u{4fdd}\u{8b77}\u{306b}\u{3088}\u{308a}\u{9664}\u{5916}",
        "\u{bcf4}\u{d638}\u{b418}\u{c5b4} \u{c81c}\u{c678}\u{b428}",
    )
}

pub const fn row_space_freed(locale: Locale) -> &'static str {
    pick(
        locale,
        "Space freed",
        "\u{91ca}\u{653e}\u{7a7a}\u{95f4}",
        "\u{89e3}\u{653e}\u{5bb9}\u{91cf}",
        "\u{d655}\u{bcf4} \u{acf5}\u{ac04}",
    )
}

pub const fn section_volume(locale: Locale) -> &'static str {
    pick(
        locale,
        "Volume",
        "\u{5377}\u{7a7a}\u{95f4}",
        "\u{30dc}\u{30ea}\u{30e5}\u{30fc}\u{30e0}",
        "\u{bcfc}\u{b968}",
    )
}

pub const fn section_kept_back(locale: Locale) -> &'static str {
    pick(
        locale,
        "Kept back",
        "\u{53d7}\u{4fdd}\u{62a4}\u{4fdd}\u{7559}",
        "\u{4fdd}\u{8b77}\u{306b}\u{3088}\u{308a}\u{9664}\u{5916}",
        "\u{bcf4}\u{d638}\u{b41c} \u{d56d}\u{baa9}",
    )
}

pub const fn docker_cleanup_section(locale: Locale) -> &'static str {
    pick(
        locale,
        "Docker Cleanup",
        "Docker \u{6e05}\u{7406}",
        "Docker \u{30af}\u{30ea}\u{30fc}\u{30f3}\u{30a2}\u{30c3}\u{30d7}",
        "Docker \u{c815}\u{b9ac}",
    )
}

pub const fn docker_group_build_cache(locale: Locale) -> &'static str {
    pick(
        locale,
        "Build Cache",
        "\u{6784}\u{5efa}\u{7f13}\u{5b58} (Build Cache)",
        "\u{30d3}\u{30eb}\u{30c9}\u{30ad}\u{30e3}\u{30c3}\u{30b7}\u{30e5}",
        "\u{be4c}\u{b4dc} \u{ce90}\u{c2dc}",
    )
}

pub const fn docker_group_unused_images(locale: Locale) -> &'static str {
    pick(
        locale,
        "Unused Images",
        "\u{672a}\u{4f7f}\u{7528}\u{955c}\u{50cf} (Unused Images)",
        "\u{672a}\u{4f7f}\u{7528}\u{30a4}\u{30e1}\u{30fc}\u{30b8}",
        "\u{bbf8}\u{c0ac}\u{c6a9} \u{c774}\u{bbf8}\u{c9c0}",
    )
}

pub const fn docker_group_stopped_containers(locale: Locale) -> &'static str {
    pick(
        locale,
        "Stopped Containers",
        "\u{5df2}\u{505c}\u{6b62}\u{5bb9}\u{5668} (Stopped Containers)",
        "\u{505c}\u{6b62}\u{6e08}\u{307f}\u{30b3}\u{30f3}\u{30c6}\u{30ca}",
        "\u{c911}\u{c9c0}\u{b41c} \u{cee8}\u{d14c}\u{c774}\u{b108}",
    )
}

pub const fn docker_build_cache_label(locale: Locale) -> &'static str {
    pick(
        locale,
        "Docker Build Cache",
        "Docker \u{6784}\u{5efa}\u{7f13}\u{5b58}",
        "Docker \u{30d3}\u{30eb}\u{30c9}\u{30ad}\u{30e3}\u{30c3}\u{30b7}\u{30e5}",
        "Docker \u{be4c}\u{b4dc} \u{ce90}\u{c2dc}",
    )
}

pub const fn docker_ai_recommended(locale: Locale) -> &'static str {
    pick(
        locale,
        "Recommended by AI",
        "AI \u{63a8}\u{8350}\u{6e05}\u{7406}",
        "AI \u{304a}\u{3059}\u{3059}\u{3081}",
        "AI \u{cd94}\u{cc9c}",
    )
}

pub fn docker_permission_denied_hint(socket: &str, locale: Locale) -> String {
    match locale {
        Locale::En => format!(
            "Docker read-only: permission denied on {socket} (cleanup skipped)."
        ),
        Locale::Zh => format!(
            "Docker \u{53ea}\u{8bfb}\u{63d0}\u{793a}\u{ff1a}\u{65e0}\u{6743}\u{8bbf}\u{95ee} {socket}\u{ff0c}\u{5df2}\u{8df3}\u{8fc7}\u{6e05}\u{7406}\u{3002}"
        ),
        Locale::Ja => format!(
            "Docker \u{8aad}\u{307f}\u{53d6}\u{308a}\u{5c02}\u{7528}: {socket} \u{3078}\u{306e}\u{6a29}\u{9650}\u{304c}\u{306a}\u{3044}\u{305f}\u{3081}\u{30b9}\u{30ad}\u{30c3}\u{30d7}\u{3057}\u{307e}\u{3059}\u{3002}"
        ),
        Locale::Ko => format!(
            "Docker \u{c77d}\u{ae30} \u{c804}\u{c6a9}: {socket} \u{ad8c}\u{d55c} \u{ac70}\u{bd80}\u{b85c} \u{c815}\u{b9ac}\u{b97c} \u{ac74}\u{b108}\u{b701}\u{b2c8}\u{b2e4}."
        ),
    }
}

pub const fn docker_select_all(locale: Locale) -> &'static str {
    pick(
        locale,
        "Select All",
        "\u{5168}\u{9009}",
        "\u{3059}\u{3079}\u{3066}\u{9078}\u{629e}",
        "\u{baa8}\u{b450} \u{c120}\u{d0dd}",
    )
}

pub const fn docker_select_none(locale: Locale) -> &'static str {
    pick(
        locale,
        "Select None",
        "\u{5168}\u{4e0d}\u{9009}",
        "\u{9078}\u{629e}\u{89e3}\u{9664}",
        "\u{c120}\u{d0dd} \u{d574}\u{c81c}",
    )
}

pub const fn row_docker_items(locale: Locale) -> &'static str {
    pick(
        locale,
        "Docker items",
        "Docker \u{6e05}\u{7406}\u{9879}",
        "Docker \u{9805}\u{76ee}",
        "Docker \u{d56d}\u{baa9}",
    )
}

pub fn generate_remote_script_button(count: usize, locale: Locale) -> String {
    match locale {
        Locale::En => format!("Generate Cleanup Script ({count})"),
        Locale::Zh => {
            format!(
                "\u{751f}\u{6210}\u{6e05}\u{7406}\u{811a}\u{672c} ({count})"
            )
        }
        Locale::Ja => format!(
            "\u{30af}\u{30ea}\u{30fc}\u{30f3}\u{30a2}\u{30c3}\u{30d7}\u{30b9}\u{30af}\u{30ea}\u{30d7}\u{30c8}\u{3092}\u{751f}\u{6210} ({count})"
        ),
        Locale::Ko => {
            format!(
                "\u{c815}\u{b9ac} \u{c2a4}\u{d06c}\u{b9bd}\u{d2b8} \u{c0dd}\u{c131} ({count})"
            )
        }
    }
}
