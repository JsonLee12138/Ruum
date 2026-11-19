//! Localized copy for the dedicated Docker workspace (`Screen::Docker`) and
//! phased execution stages.

use crate::i18n::{Locale, pick};

pub const fn rail_docker(locale: Locale) -> &'static str {
    pick(
        locale,
        "Docker",
        "Docker \u{6e05}\u{7406}",
        "Docker \u{6574}\u{7406}",
        "Docker \u{c815}\u{b9ac}",
    )
}

pub const fn docker_page_title(_locale: Locale) -> &'static str {
    "Docker"
}

pub const fn docker_source_local(locale: Locale) -> &'static str {
    super::this_computer(locale)
}

pub fn docker_source_ssh(alias: &str, locale: Locale) -> String {
    let _ = locale;
    format!("SSH \u{00b7} {alias}")
}

pub const fn docker_refresh_button(locale: Locale) -> &'static str {
    pick(
        locale,
        "Refresh",
        "\u{5237}\u{65b0}",
        "\u{66f4}\u{65b0}",
        "\u{c0c8}\u{b85c}\u{ace0}\u{ce68}",
    )
}

pub const fn docker_refreshing(locale: Locale) -> &'static str {
    pick(
        locale,
        "Refreshing\u{2026}",
        "\u{6b63}\u{5728}\u{5237}\u{65b0}\u{2026}",
        "\u{66f4}\u{65b0}\u{4e2d}\u{2026}",
        "\u{c0c8}\u{b85c}\u{ace0}\u{ce68} \u{c911}\u{2026}",
    )
}

pub const fn docker_close_button(locale: Locale) -> &'static str {
    pick(
        locale,
        "Close",
        "\u{5173}\u{95ed}",
        "\u{9589}\u{3058}\u{308b}",
        "\u{b2eb}\u{ae30}",
    )
}

pub const fn docker_not_installed_title(locale: Locale) -> &'static str {
    pick(
        locale,
        "Docker CLI Not Installed",
        "\u{672a}\u{68c0}\u{6d4b}\u{5230} Docker CLI",
        "Docker CLI \u{304c}\u{30a4}\u{30f3}\u{30b9}\u{30c8}\u{30fc}\u{30eb}\u{3055}\u{308c}\u{3066}\u{3044}\u{307e}\u{305b}\u{3093}",
        "Docker CLI\u{ac00} \u{c124}\u{ce58}\u{b418}\u{c5b4} \u{c788}\u{c9c0} \u{c54a}\u{c2b5}\u{b2c8}\u{b2e4}",
    )
}

pub const fn docker_not_installed_hint(locale: Locale) -> &'static str {
    pick(
        locale,
        "No docker binary was found on PATH or standard desktop locations \
         for this host.",
        "\u{5728}\u{5f53}\u{524d}\u{4e3b}\u{673a}\u{7684} PATH \u{6216}\u{5e38}\u{89c1}\u{5b89}\u{88c5}\u{8def}\u{5f84}\u{4e2d}\u{672a}\u{627e}\u{5230} docker \u{53ef}\u{6267}\u{884c}\u{6587}\u{4ef6}\u{3002}",
        "\u{3053}\u{306e}\u{30db}\u{30b9}\u{30c8}\u{306e} PATH \u{307e}\u{305f}\u{306f}\u{6a19}\u{6e96}\u{306e}\u{5834}\u{6240}\u{306b} docker \u{30d0}\u{30a4}\u{30ca}\u{30ea}\u{304c}\u{898b}\u{3064}\u{304b}\u{308a}\u{307e}\u{305b}\u{3093}\u{3067}\u{3057}\u{305f}\u{3002}",
        "\u{c774} \u{d638}\u{c2a4}\u{d2b8}\u{c758} PATH \u{b610}\u{b294} \u{ae30}\u{bcf8} \u{acbd}\u{b85c}\u{c5d0}\u{c11c} docker \u{c2e4}\u{d589} \u{d30c}\u{c77c}\u{c744} \u{cc3e}\u{c744} \u{c218} \u{c5c6}\u{c2b5}\u{b2c8}\u{b2e4}.",
    )
}

pub const fn docker_daemon_stopped_title(locale: Locale) -> &'static str {
    pick(
        locale,
        "Docker Daemon Is Not Running",
        "Docker \u{5b88}\u{62a4}\u{8fdb}\u{7a0b}\u{672a}\u{8fd0}\u{884c}",
        "Docker \u{30c7}\u{30fc}\u{30e2}\u{30f3}\u{304c}\u{505c}\u{6b62}\u{3057}\u{3066}\u{3044}\u{307e}\u{3059}",
        "Docker \u{b370}\u{baac}\u{c774} \u{c2e4}\u{d589} \u{c911}\u{c774} \u{c544}\u{b2d9}\u{b2c8}\u{b2e4}",
    )
}

pub const fn docker_daemon_stopped_hint(locale: Locale) -> &'static str {
    pick(
        locale,
        "Start Docker Desktop, OrbStack, or the dockerd daemon, then \
         click Refresh.",
        "\u{8bf7}\u{542f}\u{52a8} Docker Desktop\u{3001}OrbStack \u{6216} dockerd \u{670d}\u{52a1}\u{540e}\u{70b9}\u{51fb}\u{201c}\u{5237}\u{65b0}\u{201d}\u{3002}",
        "Docker Desktop\u{3001}OrbStack\u{3001}\u{307e}\u{305f}\u{306f} dockerd \u{3092}\u{8d77}\u{52d5}\u{3057}\u{3066}\u{304b}\u{3089}\u{300c}\u{66f4}\u{65b0}\u{300d}\u{3092}\u{30af}\u{30ea}\u{30c3}\u{30af}\u{3057}\u{3066}\u{304f}\u{3060}\u{3055}\u{3044}\u{3002}",
        "Docker Desktop, OrbStack \u{b610}\u{b294} dockerd\u{b97c} \u{c2dc}\u{c791}\u{d55c} \u{d6c4} \u{c0c8}\u{b85c}\u{ace0}\u{ce68}\u{c744} \u{b204}\u{b974}\u{c138}\u{c694}.",
    )
}

pub const fn docker_permission_denied_title(locale: Locale) -> &'static str {
    pick(
        locale,
        "Docker Socket Permission Denied",
        "Docker Socket \u{6743}\u{9650}\u{4e0d}\u{8db3}",
        "Docker \u{30bd}\u{30b1}\u{30c3}\u{30c8}\u{306e}\u{6a29}\u{9650}\u{304c}\u{3042}\u{308a}\u{307e}\u{305b}\u{3093}",
        "Docker \u{c18c}\u{cf13} \u{ad8c}\u{d55c} \u{ac70}\u{bd80}",
    )
}

pub const fn docker_overview_title(locale: Locale) -> &'static str {
    pick(
        locale,
        "Disk Allocation Overview",
        "\u{7a7a}\u{95f4}\u{5360}\u{7528}\u{603b}\u{89c8}",
        "\u{30c7}\u{30a3}\u{30b9}\u{30af}\u{4f7f}\u{7528}\u{91cf}\u{6982}\u{8981}",
        "\u{b514}\u{c2a4}\u{d06c} \u{c0ac}\u{c6a9}\u{b7c9} \u{ac1c}\u{c694}",
    )
}

pub fn docker_overview_reclaimable(bytes: &str, locale: Locale) -> String {
    match locale {
        Locale::En => format!("Safe reclaimable: {bytes}"),
        Locale::Zh => {
            format!("\u{5b89}\u{5168}\u{53ef}\u{91ca}\u{653e}\u{ff1a}{bytes}")
        }
        Locale::Ja => format!(
            "\u{5b89}\u{5168}\u{306b}\u{89e3}\u{653e}\u{53ef}\u{80fd}: {bytes}"
        ),
        Locale::Ko => format!(
            "\u{c548}\u{c804} \u{d68c}\u{c218} \u{ac00}\u{b2a5}: {bytes}"
        ),
    }
}

pub const fn docker_card_images(locale: Locale) -> &'static str {
    pick(
        locale,
        "Images",
        "\u{955c}\u{50cf}",
        "\u{30a4}\u{30e1}\u{30fc}\u{30b8}",
        "\u{c774}\u{bbf8}\u{c9c0}",
    )
}

pub const fn docker_card_containers(locale: Locale) -> &'static str {
    pick(
        locale,
        "Containers",
        "\u{5bb9}\u{5668}",
        "\u{30b3}\u{30f3}\u{30c6}\u{30ca}",
        "\u{cee8}\u{d14c}\u{c774}\u{b108}",
    )
}

pub const fn docker_card_build_cache(locale: Locale) -> &'static str {
    pick(
        locale,
        "Build Cache",
        "\u{6784}\u{5efa}\u{7f13}\u{5b58}",
        "\u{30d3}\u{30eb}\u{30c9}\u{30ad}\u{30e3}\u{30c3}\u{30b7}\u{30e5}",
        "\u{be4c}\u{b4dc} \u{ce90}\u{c2dc}",
    )
}

pub const fn docker_card_local_volumes(locale: Locale) -> &'static str {
    pick(
        locale,
        "Local Volumes",
        "\u{672c}\u{5730}\u{5377}",
        "\u{30ed}\u{30fc}\u{30ab}\u{30eb}\u{30dc}\u{30ea}\u{30e5}\u{30fc}\u{30e0}",
        "\u{b85c}\u{ceec} \u{bcfc}\u{b968}",
    )
}

pub const fn docker_card_protected_badge(locale: Locale) -> &'static str {
    pick(
        locale,
        "Protected \u{00b7} Read-only",
        "\u{53d7}\u{4fdd}\u{62a4} \u{00b7} \u{53ea}\u{8bfb}",
        "\u{4fdd}\u{8b77}\u{6e08}\u{307f} \u{00b7} \u{8aad}\u{307f}\u{53d6}\u{308a}\u{5c02}\u{7528}",
        "\u{bcf4}\u{d638}\u{b428} \u{00b7} \u{c77d}\u{ae30} \u{c804}\u{c6a9}",
    )
}

pub fn docker_card_meta(
    total: u64,
    active: u64,
    reclaim: &str,
    locale: Locale,
) -> String {
    match locale {
        Locale::En => {
            format!(
                "{total} total \u{00b7} {active} active \u{00b7} {reclaim} reclaimable"
            )
        }
        Locale::Zh => format!(
            "\u{5171} {total} \u{9879} \u{00b7} {active} \u{8fd0}\u{884c}\u{4e2d} \u{00b7} \u{53ef}\u{91ca}\u{653e} {reclaim}"
        ),
        Locale::Ja => format!(
            "\u{8a08} {total} \u{00b7} \u{7a3c}\u{50cd} {active} \u{00b7} \u{89e3}\u{653e}\u{53ef}\u{80fd} {reclaim}"
        ),
        Locale::Ko => format!(
            "\u{cd1d} {total} \u{00b7} \u{d65c}\u{c131} {active} \u{00b7} \u{d68c}\u{c218} \u{ac00}\u{b2a5} {reclaim}"
        ),
    }
}

pub fn docker_card_shared_note(bytes: &str, locale: Locale) -> String {
    match locale {
        Locale::En => format!("{bytes} shared with images"),
        Locale::Zh => {
            format!("\u{4e0e}\u{955c}\u{50cf}\u{5171}\u{4eab} {bytes}")
        }
        Locale::Ja => {
            format!(
                "\u{30a4}\u{30e1}\u{30fc}\u{30b8}\u{3068} {bytes} \u{5171}\u{6709}"
            )
        }
        Locale::Ko => {
            format!("\u{c774}\u{bbf8}\u{c9c0}\u{c640} {bytes} \u{acf5}\u{c720}")
        }
    }
}

pub fn docker_selected_summary(
    count: usize,
    bytes: &str,
    locale: Locale,
) -> String {
    match locale {
        Locale::En => format!("{count} selected \u{00b7} est. {bytes}"),
        Locale::Zh => format!(
            "\u{5df2}\u{9009} {count} \u{9879} \u{00b7} \u{9884}\u{8ba1}\u{91ca}\u{653e} {bytes}"
        ),
        Locale::Ja => {
            format!(
                "{count} \u{4ef6}\u{9078}\u{629e} \u{00b7} \u{7d04} {bytes}"
            )
        }
        Locale::Ko => {
            format!(
                "{count}\u{ac1c} \u{c120}\u{d0dd} \u{00b7} \u{c608}\u{c0c1} {bytes}"
            )
        }
    }
}

pub const fn docker_no_candidates_hint(
    detailed: bool,
    locale: Locale,
) -> &'static str {
    if detailed {
        pick(
            locale,
            "No unused images, stopped containers, or reclaimable build \
             cache were found.",
            "\u{672a}\u{53d1}\u{73b0}\u{672a}\u{4f7f}\u{7528}\u{955c}\u{50cf}\u{3001}\u{5df2}\u{505c}\u{6b62}\u{5bb9}\u{5668}\u{6216}\u{53ef}\u{56de}\u{6536}\u{6784}\u{5efa}\u{7f13}\u{5b58}\u{3002}",
            "\u{672a}\u{4f7f}\u{7528}\u{30a4}\u{30e1}\u{30fc}\u{30b8}\u{3001}\u{505c}\u{6b62}\u{6e08}\u{307f}\u{30b3}\u{30f3}\u{30c6}\u{30ca}\u{3001}\u{56de}\u{53ce}\u{53ef}\u{80fd}\u{306a}\u{30ad}\u{30e3}\u{30c3}\u{30b7}\u{30e5}\u{306f}\u{898b}\u{3064}\u{304b}\u{308a}\u{307e}\u{305b}\u{3093}\u{3067}\u{3057}\u{305f}\u{3002}",
            "\u{bbf8}\u{c0ac}\u{c6a9} \u{c774}\u{bbf8}\u{c9c0}, \u{c911}\u{c9c0}\u{b41c} \u{cee8}\u{d14c}\u{c774}\u{b108} \u{b610}\u{b294} \u{d68c}\u{c218} \u{ac00}\u{b2a5}\u{d55c} \u{be4c}\u{b4dc} \u{ce90}\u{c2dc}\u{ac00} \u{c5c6}\u{c2b5}\u{b2c8}\u{b2e4}.",
        )
    } else {
        pick(
            locale,
            "Summary-only probe active; only Build Cache pruning is available \
             when reclaimable bytes exist.",
            "\u{5f53}\u{524d}\u{4e3a}\u{6458}\u{8981}\u{63a2}\u{6d4b}\u{6a21}\u{5f0f}\u{ff0c}\u{4ec5}\u{53ef}\u{5728}\u{6709}\u{53ef}\u{56de}\u{6536}\u{5b57}\u{8282}\u{65f6}\u{6e05}\u{7406}\u{6784}\u{5efa}\u{7f13}\u{5b58}\u{3002}",
            "\u{30b5}\u{30de}\u{30ea}\u{30fc}\u{30e2}\u{30fc}\u{30c9}\u{306e}\u{305f}\u{3081}\u{3001}\u{30d3}\u{30eb}\u{30c9}\u{30ad}\u{30e3}\u{30c3}\u{30b7}\u{30e5}\u{306e}\u{524a}\u{9664}\u{306e}\u{307f}\u{5229}\u{7528}\u{3067}\u{304d}\u{307e}\u{3059}\u{3002}",
            "\u{c694}\u{c57d} \u{d504}\u{b85c}\u{be0c} \u{baa8}\u{b4dc}\u{c774}\u{bbc0}\u{b85c} \u{d68c}\u{c218} \u{ac00}\u{b2a5}\u{d55c} \u{be4c}\u{b4dc} \u{ce90}\u{c2dc} \u{c815}\u{b9ac}\u{b9cc} \u{c9c0}\u{c6d0}\u{b429}\u{b2c8}\u{b2e4}.",
        )
    }
}

pub fn docker_build_cache_detail(
    reclaim: &str,
    shared: Option<&str>,
    locale: Locale,
) -> String {
    match (shared, locale) {
        (Some(sh), Locale::En) => format!(
            "docker builder prune -f ({reclaim} standalone; {sh} shared with \
             images)"
        ),
        (Some(sh), Locale::Zh) => format!(
            "docker builder prune -f\u{ff08}\u{72ec}\u{7acb}\u{53ef}\u{91ca}\u{653e} {reclaim}\u{ff1b}\u{4e0e}\u{955c}\u{50cf}\u{5171}\u{4eab} {sh}\u{ff09}"
        ),
        (Some(sh), Locale::Ja) => format!(
            "docker builder prune -f\u{ff08}\u{5358}\u{72ec} {reclaim}\u{3001}\u{30a4}\u{30e1}\u{30fc}\u{30b8}\u{5171}\u{6709} {sh}\u{ff09}"
        ),
        (Some(sh), Locale::Ko) => format!(
            "docker builder prune -f (\u{b2e8}\u{b3c5} {reclaim}, \u{c774}\u{bbf8}\u{c9c0} \u{acf5}\u{c720} {sh})"
        ),
        (None, Locale::En) => {
            format!("docker builder prune -f ({reclaim} reclaimable)")
        }
        (None, Locale::Zh) => {
            format!(
                "docker builder prune -f\u{ff08}\u{53ef}\u{91ca}\u{653e} {reclaim}\u{ff09}"
            )
        }
        (None, Locale::Ja) => {
            format!(
                "docker builder prune -f\u{ff08}\u{89e3}\u{653e}\u{53ef}\u{80fd} {reclaim}\u{ff09}"
            )
        }
        (None, Locale::Ko) => {
            format!(
                "docker builder prune -f (\u{d68c}\u{c218} \u{ac00}\u{b2a5} {reclaim})"
            )
        }
    }
}

pub const fn docker_volumes_notice_title(locale: Locale) -> &'static str {
    pick(
        locale,
        "Local Volumes Are Never Deleted Automatically",
        "\u{672c}\u{5730}\u{5377}\u{53d7}\u{4fdd}\u{62a4}\u{ff0c}\u{4e0d}\u{4f1a}\u{81ea}\u{52a8}\u{6e05}\u{7406}",
        "\u{30ed}\u{30fc}\u{30ab}\u{30eb}\u{30dc}\u{30ea}\u{30e5}\u{30fc}\u{30e0}\u{306f}\u{81ea}\u{52d5}\u{524a}\u{9664}\u{3055}\u{308c}\u{307e}\u{305b}\u{3093}",
        "\u{b85c}\u{ceec} \u{bcfc}\u{b968}\u{c740} \u{c790}\u{b3d9} \u{c0ad}\u{c81c}\u{b418}\u{c9c0} \u{c54a}\u{c2b5}\u{b2c8}\u{b2e4}",
    )
}

pub fn docker_volumes_notice_detail(
    count: u64,
    size: &str,
    locale: Locale,
) -> String {
    match locale {
        Locale::En => format!(
            "{count} local volumes ({size}) hold persistent data and remain \
             read-only."
        ),
        Locale::Zh => format!(
            "\u{5171} {count} \u{4e2a}\u{672c}\u{5730}\u{5377}\u{ff08}{size}\u{ff09}\u{4fdd}\u{5b58}\u{6301}\u{4e45}\u{5316}\u{6570}\u{636e}\u{ff0c}\u{59cb}\u{7ec8}\u{4fdd}\u{6301}\u{53ea}\u{8bfb}\u{3002}"
        ),
        Locale::Ja => format!(
            "{count} \u{500b}\u{306e}\u{30dc}\u{30ea}\u{30e5}\u{30fc}\u{30e0}\u{ff08}{size}\u{ff09}\u{306f}\u{6c38}\u{7d9a}\u{30c7}\u{30fc}\u{30bf}\u{306e}\u{305f}\u{3081}\u{8aad}\u{307f}\u{53d6}\u{308a}\u{5c02}\u{7528}\u{3067}\u{3059}\u{3002}"
        ),
        Locale::Ko => format!(
            "\u{b85c}\u{ceec} \u{bcfc}\u{b968} {count}\u{ac1c}({size})\u{b294} \u{c601}\u{ad6c} \u{b370}\u{c774}\u{d130} \u{bcf4}\u{d638}\u{b97c} \u{c704}\u{d574} \u{c77d}\u{ae30} \u{c804}\u{c6a9}\u{c73c}\u{b85c} \u{c720}\u{c9c0}\u{b429}\u{b2c8}\u{b2e4}."
        ),
    }
}

pub fn docker_commit_button(count: usize, locale: Locale) -> String {
    match locale {
        Locale::En => format!("Clean Selected ({count})"),
        Locale::Zh => {
            format!("\u{6e05}\u{7406}\u{5df2}\u{9009}\u{9879} ({count})")
        }
        Locale::Ja => format!(
            "\u{9078}\u{629e}\u{9805}\u{76ee}\u{3092}\u{524a}\u{9664} ({count})"
        ),
        Locale::Ko => {
            format!(
                "\u{c120}\u{d0dd} \u{d56d}\u{baa9} \u{c815}\u{b9ac} ({count})"
            )
        }
    }
}

pub const fn docker_stage_containers(locale: Locale) -> &'static str {
    pick(
        locale,
        "Removing stopped containers\u{2026}",
        "\u{6b63}\u{5728}\u{6e05}\u{7406}\u{5df2}\u{505c}\u{6b62}\u{5bb9}\u{5668}\u{2026}",
        "\u{505c}\u{6b62}\u{6e08}\u{307f}\u{30b3}\u{30f3}\u{30c6}\u{30ca}\u{3092}\u{524a}\u{9664}\u{4e2d}\u{2026}",
        "\u{c911}\u{c9c0}\u{b41c} \u{cee8}\u{d14c}\u{c774}\u{b108} \u{c815}\u{b9ac} \u{c911}\u{2026}",
    )
}

pub const fn docker_stage_images(locale: Locale) -> &'static str {
    pick(
        locale,
        "Removing unused images\u{2026}",
        "\u{6b63}\u{5728}\u{6e05}\u{7406}\u{672a}\u{4f7f}\u{7528}\u{955c}\u{50cf}\u{2026}",
        "\u{672a}\u{4f7f}\u{7528}\u{30a4}\u{30e1}\u{30fc}\u{30b8}\u{3092}\u{524a}\u{9664}\u{4e2d}\u{2026}",
        "\u{bbf8}\u{c0ac}\u{c6a9} \u{c774}\u{bbf8}\u{c9c0} \u{c815}\u{b9ac} \u{c911}\u{2026}",
    )
}

pub const fn docker_stage_build_cache(locale: Locale) -> &'static str {
    pick(
        locale,
        "Pruning Docker build cache\u{2026}",
        "\u{6b63}\u{5728}\u{6e05}\u{7406}\u{6784}\u{5efa}\u{7f13}\u{5b58}\u{2026}",
        "\u{30d3}\u{30eb}\u{30c9}\u{30ad}\u{30e3}\u{30c3}\u{30b7}\u{30e5}\u{3092}\u{524a}\u{9664}\u{4e2d}\u{2026}",
        "\u{be4c}\u{b4dc} \u{ce90}\u{c2dc} \u{c815}\u{b9ac} \u{c911}\u{2026}",
    )
}

pub const fn docker_stage_refreshing(locale: Locale) -> &'static str {
    pick(
        locale,
        "Refreshing Docker usage summary\u{2026}",
        "\u{6b63}\u{5728}\u{5237}\u{65b0} Docker \u{7a7a}\u{95f4}\u{7edf}\u{8ba1}\u{2026}",
        "Docker \u{4f7f}\u{7528}\u{72b6}\u{6cc1}\u{3092}\u{66f4}\u{65b0}\u{4e2d}\u{2026}",
        "Docker \u{c0ac}\u{c6a9}\u{b7c9} \u{c694}\u{c57d} \u{c0c8}\u{b85c}\u{ace0}\u{ce68} \u{c911}\u{2026}",
    )
}
