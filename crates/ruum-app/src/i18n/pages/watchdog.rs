//! Watchdog dashboard copy and relative check-age formatting.

use crate::i18n::{Locale, pick};

/// Format how recently the background disk check ran.
#[must_use]
pub fn format_checked_age(
    checked_secs: Option<u64>,
    now_secs: u64,
    locale: Locale,
) -> String {
    let Some(checked) = checked_secs else {
        return pick(
            locale,
            "Not checked yet",
            "\u{5c1a}\u{672a}\u{68c0}\u{67e5}",
            "\u{672a}\u{78ba}\u{8a8d}",
            "\u{c544}\u{c9c1} \u{d655}\u{c778} \u{c548} \u{b428}",
        )
        .to_owned();
    };
    let delta = now_secs.saturating_sub(checked);
    if delta < 5 {
        return pick(
            locale,
            "Just now",
            "\u{521a}\u{521a}",
            "\u{305f}\u{3063}\u{305f}\u{4eca}",
            "\u{bc29}\u{ae08}",
        )
        .to_owned();
    }
    if delta < 60 {
        return match locale {
            Locale::En => format!("{delta}s ago"),
            Locale::Zh | Locale::Ja => format!("{delta} \u{79d2}\u{524d}"),
            Locale::Ko => format!("{delta}\u{cd08} \u{c804}"),
        };
    }
    if delta < 3_600 {
        let m = delta / 60;
        return match locale {
            Locale::En => format!("{m}m ago"),
            Locale::Zh => format!("{m} \u{5206}\u{949f}\u{524d}"),
            Locale::Ja => format!("{m} \u{5206}\u{524d}"),
            Locale::Ko => format!("{m}\u{bd84} \u{c804}"),
        };
    }
    if delta < 86_400 {
        let h = delta / 3_600;
        return match locale {
            Locale::En => format!("{h}h ago"),
            Locale::Zh => format!("{h} \u{5c0f}\u{65f6}\u{524d}"),
            Locale::Ja => format!("{h} \u{6642}\u{9593}\u{524d}"),
            Locale::Ko => format!("{h}\u{c2dc}\u{ac04} \u{c804}"),
        };
    }
    let d = delta / 86_400;
    match locale {
        Locale::En => format!("{d}d ago"),
        Locale::Zh => format!("{d} \u{5929}\u{524d}"),
        Locale::Ja => format!("{d} \u{65e5}\u{524d}"),
        Locale::Ko => format!("{d}\u{c77c} \u{c804}"),
    }
}

pub const fn watchdog_title(locale: Locale) -> &'static str {
    pick(
        locale,
        "Local Disk Space Watchdog",
        "\u{672c}\u{673a}\u{78c1}\u{76d8}\u{7a7a}\u{95f4}\u{5b88}\u{62a4}",
        "\u{30ed}\u{30fc}\u{30ab}\u{30eb}\u{30c7}\u{30a3}\u{30b9}\u{30af}\u{5bb9}\u{91cf}\u{30a6}\u{30a9}\u{30c3}\u{30c1}\u{30c9}\u{30c3}\u{30b0}",
        "\u{b85c}\u{ceec} \u{b514}\u{c2a4}\u{d06c} \u{acf5}\u{ac04} \u{c6cc}\u{ce58}\u{b3c5}",
    )
}

pub const fn watchdog_subtitle(locale: Locale) -> &'static str {
    pick(
        locale,
        "Monitors this computer only with low-overhead checks and local AI \
         cleanup suggestions.",
        "\u{4ec5}\u{76d1}\u{63a7}\u{672c}\u{673a}\u{78c1}\u{76d8}\u{7a7a}\u{95f4}\u{ff0c}\u{63d0}\u{4f9b}\u{4f4e}\u{5f00}\u{9500}\u{68c0}\u{67e5}\u{4e0e}\u{672c}\u{673a} AI \u{6e05}\u{7406}\u{5efa}\u{8bae}\u{3002}",
        "\u{3053}\u{306e}\u{30b3}\u{30f3}\u{30d4}\u{30e5}\u{30fc}\u{30bf}\u{306e}\u{307f}\u{3092}\u{4f4e}\u{30aa}\u{30fc}\u{30d0}\u{30fc}\u{30d8}\u{30c3}\u{30c9}\u{3067}\u{76e3}\u{8996}\u{3057}\u{3001}\u{30ed}\u{30fc}\u{30ab}\u{30eb} AI \u{30af}\u{30ea}\u{30fc}\u{30f3}\u{30a2}\u{30c3}\u{30d7}\u{63d0}\u{6848}\u{3092}\u{884c}\u{3044}\u{307e}\u{3059}\u{3002}",
        "\u{c774} \u{cef4}\u{d4e8}\u{d130}\u{b9cc} \u{b0ae}\u{c740} \u{c624}\u{bc84}\u{d5e4}\u{b4dc}\u{b85c} \u{baa8}\u{b2c8}\u{d130}\u{b9c1}\u{d558}\u{ba70} \u{b85c}\u{ceec} AI \u{c815}\u{b9ac} \u{c81c}\u{c548}\u{c744} \u{c81c}\u{acf5}\u{d569}\u{b2c8}\u{b2e4}.",
    )
}

pub const fn unknown(locale: Locale) -> &'static str {
    pick(
        locale,
        "Unknown",
        "\u{672a}\u{77e5}",
        "\u{4e0d}\u{660e}",
        "\u{c54c} \u{c218} \u{c5c6}\u{c74c}",
    )
}

pub const fn background_mode_label(locale: Locale) -> &'static str {
    pick(
        locale,
        "Background Monitoring:",
        "\u{540e}\u{53f0}\u{76d1}\u{63a7}\u{ff1a}",
        "\u{30d0}\u{30c3}\u{30af}\u{30b0}\u{30e9}\u{30a6}\u{30f3}\u{30c9}\u{76e3}\u{8996}:",
        "\u{bc31}\u{adf8}\u{b77c}\u{c6b4}\u{b4dc} \u{baa8}\u{b2c8}\u{d130}\u{b9c1}:",
    )
}

pub const fn background_mode_status(
    close_to_background: bool,
    locale: Locale,
) -> &'static str {
    if close_to_background {
        pick(
            locale,
            "Active (System Tray)",
            "\u{540e}\u{53f0}\u{5e38}\u{9a7b}\u{4e2d}\u{ff08}\u{7cfb}\u{7edf}\u{6258}\u{76d8}\u{ff09}",
            "\u{30d0}\u{30c3}\u{30af}\u{30b0}\u{30e9}\u{30a6}\u{30f3}\u{30c9}\u{5e38}\u{99d0}\u{4e2d}\u{ff08}\u{30c8}\u{30ec}\u{30a4}\u{ff09}",
            "\u{bc31}\u{adf8}\u{b77c}\u{c6b4}\u{b4dc} \u{c2e4}\u{d589} \u{c911}(\u{c2dc}\u{c2a4}\u{d15c} \u{d2b8}\u{b808}\u{c774})",
        )
    } else {
        pick(
            locale,
            "Foreground Only (While App Open)",
            "\u{4ec5}\u{524d}\u{53f0}\u{8fd0}\u{884c}\u{ff08}\u{5173}\u{95ed}\u{7a97}\u{53e3}\u{5373}\u{9000}\u{51fa}\u{ff09}",
            "\u{30d5}\u{30a9}\u{30a2}\u{30b0}\u{30e9}\u{30a6}\u{30f3}\u{30c9}\u{306e}\u{307f}\u{ff08}\u{30a6}\u{30a3}\u{30f3}\u{30c9}\u{30a6}\u{3092}\u{9589}\u{3058}\u{308b}\u{3068}\u{7d42}\u{4e86}\u{ff09}",
            "\u{d3ec}\u{adf8}\u{b77c}\u{c6b4}\u{b4dc} \u{c804}\u{c6a9}(\u{cc3d}\u{c744} \u{b2eb}\u{c73c}\u{ba74} \u{c885}\u{b8cc})",
        )
    }
}

pub const fn volume_status_title(locale: Locale) -> &'static str {
    pick(
        locale,
        "Local Volume Status",
        "\u{672c}\u{673a}\u{5377}\u{72b6}\u{6001}",
        "\u{30ed}\u{30fc}\u{30ab}\u{30eb}\u{30dc}\u{30ea}\u{30e5}\u{30fc}\u{30e0}\u{72b6}\u{614b}",
        "\u{b85c}\u{ceec} \u{bcfc}\u{b968} \u{c0c1}\u{d0dc}",
    )
}

pub const fn monitored_root(locale: Locale) -> &'static str {
    pick(
        locale,
        "Local Monitored Root:",
        "\u{672c}\u{673a}\u{76d1}\u{63a7}\u{6839}\u{76ee}\u{5f55}\u{ff1a}",
        "\u{30ed}\u{30fc}\u{30ab}\u{30eb}\u{76e3}\u{8996}\u{30eb}\u{30fc}\u{30c8}:",
        "\u{b85c}\u{ceec} \u{baa8}\u{b2c8}\u{d130}\u{b9c1} \u{b8e8}\u{d2b8}:",
    )
}

pub const fn free_space_label(locale: Locale) -> &'static str {
    pick(
        locale,
        "Free Space:",
        "\u{53ef}\u{7528}\u{7a7a}\u{95f4}\u{ff1a}",
        "\u{7a7a}\u{304d}\u{5bb9}\u{91cf}:",
        "\u{c5ec}\u{c720} \u{acf5}\u{ac04}:",
    )
}

pub fn free_of_total(free: &str, total: &str, locale: Locale) -> String {
    match locale {
        Locale::En => format!("{free} of {total}"),
        Locale::Zh => format!("{free} / \u{5171} {total}"),
        Locale::Ja | Locale::Ko => format!("{free} / {total}"),
    }
}

pub const fn alert_threshold_label(locale: Locale) -> &'static str {
    pick(
        locale,
        "Alert Threshold:",
        "\u{544a}\u{8b66}\u{9608}\u{503c}\u{ff1a}",
        "\u{8b66}\u{544a}\u{3057}\u{304d}\u{3044}\u{5024}:",
        "\u{acbd}\u{ace0} \u{c784}\u{acc4}\u{ac12}:",
    )
}

pub const fn status_label(locale: Locale) -> &'static str {
    pick(
        locale,
        "Status:",
        "\u{72b6}\u{6001}\u{ff1a}",
        "\u{72b6}\u{614b}:",
        "\u{c0c1}\u{d0dc}:",
    )
}

pub const fn status_low_disk(locale: Locale) -> &'static str {
    pick(
        locale,
        "Low Disk Space (Below threshold)",
        "\u{78c1}\u{76d8}\u{7a7a}\u{95f4}\u{4e0d}\u{8db3}\u{ff08}\u{4f4e}\u{4e8e}\u{9608}\u{503c}\u{ff09}",
        "\u{30c7}\u{30a3}\u{30b9}\u{30af}\u{5bb9}\u{91cf}\u{4e0d}\u{8db3}\u{ff08}\u{3057}\u{304d}\u{3044}\u{5024}\u{672a}\u{6e80}\u{ff09}",
        "\u{b514}\u{c2a4}\u{d06c} \u{acf5}\u{ac04} \u{bd80}\u{c871}(\u{c784}\u{acc4}\u{ac12} \u{bbf8}\u{b9cc})",
    )
}

pub const fn status_healthy(locale: Locale) -> &'static str {
    pick(
        locale,
        "Healthy (Above threshold)",
        "\u{5065}\u{5eb7}\u{ff08}\u{9ad8}\u{4e8e}\u{9608}\u{503c}\u{ff09}",
        "\u{6b63}\u{5e38}\u{ff08}\u{3057}\u{304d}\u{3044}\u{5024}\u{4ee5}\u{4e0a}\u{ff09}",
        "\u{c815}\u{c0c1}(\u{c784}\u{acc4}\u{ac12} \u{c774}\u{c0c1})",
    )
}

pub const fn last_checked_label(locale: Locale) -> &'static str {
    pick(
        locale,
        "Last Checked:",
        "\u{4e0a}\u{6b21}\u{68c0}\u{67e5}\u{ff1a}",
        "\u{6700}\u{7d42}\u{78ba}\u{8a8d}:",
        "\u{b9c8}\u{c9c0}\u{b9c9} \u{d655}\u{c778}:",
    )
}

pub const fn latest_proposal_title(locale: Locale) -> &'static str {
    pick(
        locale,
        "Latest Local AI Cleanup Proposal",
        "\u{6700}\u{65b0}\u{672c}\u{673a} AI \u{6e05}\u{7406}\u{5efa}\u{8bae}",
        "\u{6700}\u{65b0}\u{306e}\u{30ed}\u{30fc}\u{30ab}\u{30eb} AI \u{30af}\u{30ea}\u{30fc}\u{30f3}\u{30a2}\u{30c3}\u{30d7}\u{63d0}\u{6848}",
        "\u{cd5c}\u{c2e0} \u{b85c}\u{ceec} AI \u{c815}\u{b9ac} \u{c81c}\u{c548}",
    )
}

pub fn candidate_items_summary(
    count: usize,
    bytes: &str,
    locale: Locale,
) -> String {
    match locale {
        Locale::En => format!("{count} candidate items \u{00b7} {bytes}"),
        Locale::Zh => format!(
            "{count} \u{4e2a}\u{5f85}\u{6e05}\u{7406}\u{9879} \u{00b7} {bytes}"
        ),
        Locale::Ja => {
            format!("{count} \u{4ef6}\u{306e}\u{5019}\u{88dc} \u{00b7} {bytes}")
        }
        Locale::Ko => {
            format!("\u{d6c4}\u{bcf4} {count}\u{ac1c} \u{00b7} {bytes}")
        }
    }
}

pub const fn review_and_confirm(locale: Locale) -> &'static str {
    pick(
        locale,
        "Review & Confirm\u{2026}",
        "\u{67e5}\u{770b}\u{5e76}\u{786e}\u{8ba4}\u{2026}",
        "\u{78ba}\u{8a8d}\u{3057}\u{3066}\u{627f}\u{8a8d}\u{2026}",
        "\u{ac80}\u{d1a0} \u{bc0f} \u{d655}\u{c778}\u{2026}",
    )
}

pub const fn no_proposals_yet(locale: Locale) -> &'static str {
    pick(
        locale,
        "No proposals recorded yet. Run AI Analysis or wait for watchdog \
         trigger.",
        "\u{6682}\u{65e0}\u{6e05}\u{7406}\u{5efa}\u{8bae}\u{8bb0}\u{5f55}\u{3002}\u{53ef}\u{624b}\u{52a8}\u{8fd0}\u{884c} AI \u{5206}\u{6790}\u{6216}\u{7b49}\u{5f85}\u{7a7a}\u{95f4}\u{5b88}\u{62a4}\u{89e6}\u{53d1}\u{3002}",
        "\u{63d0}\u{6848}\u{306f}\u{307e}\u{3060}\u{3042}\u{308a}\u{307e}\u{305b}\u{3093}\u{3002}AI \u{5206}\u{6790}\u{3092}\u{5b9f}\u{884c}\u{3059}\u{308b}\u{304b}\u{3001}\u{30a6}\u{30a9}\u{30c3}\u{30c1}\u{30c9}\u{30c3}\u{30b0}\u{306e}\u{767a}\u{52d5}\u{3092}\u{304a}\u{5f85}\u{3061}\u{304f}\u{3060}\u{3055}\u{3044}\u{3002}",
        "\u{c544}\u{c9c1} \u{ae30}\u{b85d}\u{b41c} \u{c81c}\u{c548}\u{c774} \u{c5c6}\u{c2b5}\u{b2c8}\u{b2e4}. AI \u{bd84}\u{c11d}\u{c744} \u{c2e4}\u{d589}\u{d558}\u{ac70}\u{b098} \u{c6cc}\u{ce58}\u{b3c5} \u{d2b8}\u{b9ac}\u{ac70}\u{b97c} \u{ae30}\u{b2e4}\u{b9ac}\u{c138}\u{c694}.",
    )
}

pub const fn latest_ai_error_title(locale: Locale) -> &'static str {
    pick(
        locale,
        "Latest AI Analysis Failure",
        "\u{6700}\u{8fd1}\u{4e00}\u{6b21} AI \u{5206}\u{6790}\u{5931}\u{8d25}",
        "\u{6700}\u{65b0}\u{306e} AI \u{5206}\u{6790}\u{30a8}\u{30e9}\u{30fc}",
        "\u{cd5c}\u{adfc} AI \u{bd84}\u{c11d} \u{c2e4}\u{d328}",
    )
}

pub const fn cleanup_progress_heading(
    cancelling: bool,
    locale: Locale,
) -> &'static str {
    if cancelling {
        pick(
            locale,
            "Stopping Cleanup\u{2026}",
            "\u{6b63}\u{5728}\u{505c}\u{6b62}\u{6e05}\u{7406}\u{2026}",
            "\u{30af}\u{30ea}\u{30fc}\u{30f3}\u{30a2}\u{30c3}\u{30d7}\u{3092}\u{505c}\u{6b62}\u{4e2d}\u{2026}",
            "\u{c815}\u{b9ac} \u{c911}\u{c9c0} \u{c911}\u{2026}",
        )
    } else {
        pick(
            locale,
            "Cleaning Selected Items\u{2026}",
            "\u{6b63}\u{5728}\u{6e05}\u{7406}\u{9009}\u{5b9a}\u{9879}\u{76ee}\u{2026}",
            "\u{9078}\u{629e}\u{3057}\u{305f}\u{9805}\u{76ee}\u{3092}\u{30af}\u{30ea}\u{30fc}\u{30f3}\u{30a2}\u{30c3}\u{30d7}\u{4e2d}\u{2026}",
            "\u{c120}\u{d0dd}\u{d55c} \u{d56d}\u{baa9} \u{c815}\u{b9ac} \u{c911}\u{2026}",
        )
    }
}

pub const fn cleanup_progress_subtitle(
    cancelling: bool,
    locale: Locale,
) -> &'static str {
    if cancelling {
        pick(
            locale,
            "Stopping, waiting for current item to finish safely\u{2026}",
            "\u{6b63}\u{5728}\u{505c}\u{6b62}\u{ff0c}\u{7b49}\u{5f85}\u{5f53}\u{524d}\u{9879}\u{5b89}\u{5168}\u{5b8c}\u{6210}\u{2026}",
            "\u{505c}\u{6b62}\u{4e2d}\u{3067}\u{3059}\u{3002}\u{73fe}\u{5728}\u{306e}\u{9805}\u{76ee}\u{304c}\u{5b89}\u{5168}\u{306b}\u{5b8c}\u{4e86}\u{3059}\u{308b}\u{306e}\u{3092}\u{5f85}\u{3063}\u{3066}\u{3044}\u{307e}\u{3059}\u{2026}",
            "\u{c911}\u{c9c0} \u{c911}\u{c785}\u{b2c8}\u{b2e4}. \u{d604}\u{c7ac} \u{d56d}\u{baa9}\u{c774} \u{c548}\u{c804}\u{d558}\u{ac8c} \u{c644}\u{b8cc}\u{b420} \u{b54c}\u{ae4c}\u{c9c0} \u{ae30}\u{b2e4}\u{b9ac}\u{b294} \u{c911}\u{2026}",
        )
    } else {
        pick(
            locale,
            "Removing confirmed items safely. Closing this window continues cleanup in the background.",
            "\u{6b63}\u{5728}\u{5b89}\u{5168}\u{6e05}\u{7406}\u{5df2}\u{786e}\u{8ba4}\u{9879}\u{76ee}\u{3002}\u{5173}\u{95ed}\u{6b64}\u{7a97}\u{53e3}\u{540e}\u{5c06}\u{5728}\u{540e}\u{53f0}\u{7ee7}\u{7eed}\u{6e05}\u{7406}\u{3002}",
            "\u{78ba}\u{8a8d}\u{6e08}\u{307f}\u{306e}\u{9805}\u{76ee}\u{3092}\u{5b89}\u{5168}\u{306b}\u{524a}\u{9664}\u{3057}\u{3066}\u{3044}\u{307e}\u{3059}\u{3002}\u{3053}\u{306e}\u{30a6}\u{30a3}\u{30f3}\u{30c9}\u{30a6}\u{3092}\u{9589}\u{3058}\u{3066}\u{3082}\u{30d0}\u{30c3}\u{30af}\u{30b0}\u{30e9}\u{30a6}\u{30f3}\u{30c9}\u{3067}\u{7d9a}\u{884c}\u{3055}\u{308c}\u{307e}\u{3059}\u{3002}",
            "\u{d655}\u{c778}\u{b41c} \u{d56d}\u{baa9}\u{c744} \u{c548}\u{c804}\u{d558}\u{ac8c} \u{c815}\u{b9ac}\u{d558}\u{ace0} \u{c788}\u{c2b5}\u{b2c8}\u{b2e4}. \u{c774} \u{cc3d}\u{c744} \u{b2eb}\u{c544}\u{b3c4} \u{bc31}\u{adf8}\u{b77c}\u{c6b4}\u{b4dc}\u{c5d0}\u{c11c} \u{acc4}\u{c18d} \u{c9c4}\u{d589}\u{b429}\u{b2c8}\u{b2e4}.",
        )
    }
}

pub const fn cleanup_progress_preparing(locale: Locale) -> &'static str {
    pick(
        locale,
        "Preparing cleanup\u{2026}",
        "\u{6b63}\u{5728}\u{51c6}\u{5907}\u{6e05}\u{7406}\u{2026}",
        "\u{30af}\u{30ea}\u{30fc}\u{30f3}\u{30a2}\u{30c3}\u{30d7}\u{3092}\u{6e96}\u{5099}\u{4e2d}\u{2026}",
        "\u{c815}\u{b9ac} \u{c900}\u{be44} \u{c911}\u{2026}",
    )
}

pub fn cleanup_reclaimed_so_far(
    reclaimed: &str,
    total: Option<&str>,
    locale: Locale,
) -> String {
    match total {
        Some(tot) => match locale {
            Locale::En => format!("Freed {reclaimed} / {tot}"),
            Locale::Zh => {
                format!("\u{5df2}\u{91ca}\u{653e} {reclaimed} / {tot}")
            }
            Locale::Ja => format!("{reclaimed} / {tot} \u{89e3}\u{653e}"),
            Locale::Ko => format!("{reclaimed} / {tot} \u{d655}\u{bcf4}"),
        },
        None => match locale {
            Locale::En => format!("Freed {reclaimed}"),
            Locale::Zh => format!("\u{5df2}\u{91ca}\u{653e} {reclaimed}"),
            Locale::Ja => format!("{reclaimed} \u{89e3}\u{653e}"),
            Locale::Ko => format!("{reclaimed} \u{d655}\u{bcf4}"),
        },
    }
}

pub const fn cleanup_continue_in_background(locale: Locale) -> &'static str {
    pick(
        locale,
        "Continue in Background",
        "\u{540e}\u{53f0}\u{7ee7}\u{7eed}",
        "\u{30d0}\u{30c3}\u{30af}\u{30b0}\u{30e9}\u{30a6}\u{30f3}\u{30c9}\u{3067}\u{7d9a}\u{884c}",
        "\u{bc31}\u{adf8}\u{b77c}\u{c6b4}\u{b4dc}\u{c5d0}\u{c11c} \u{acc4}\u{c18d}",
    )
}

pub const fn cleanup_cancel_button(
    cancelling: bool,
    locale: Locale,
) -> &'static str {
    if cancelling {
        pick(
            locale,
            "Stopping\u{2026}",
            "\u{6b63}\u{5728}\u{505c}\u{6b62}\u{2026}",
            "\u{505c}\u{6b62}\u{4e2d}\u{2026}",
            "\u{c911}\u{c9c0} \u{c911}\u{2026}",
        )
    } else {
        pick(
            locale,
            "Cancel Cleanup",
            "\u{53d6}\u{6d88}\u{6e05}\u{7406}",
            "\u{30af}\u{30ea}\u{30fc}\u{30f3}\u{30a2}\u{30c3}\u{30d7}\u{3092}\u{4e2d}\u{6b62}",
            "\u{c815}\u{b9ac} \u{cde8}\u{c18c}",
        )
    }
}

pub const fn cleanup_progress_done_heading(
    cancelled: bool,
    failed: bool,
    locale: Locale,
) -> &'static str {
    if cancelled {
        pick(
            locale,
            "Cleanup Cancelled",
            "\u{6e05}\u{7406}\u{5df2}\u{53d6}\u{6d88}",
            "\u{30af}\u{30ea}\u{30fc}\u{30f3}\u{30a2}\u{30c3}\u{30d7}\u{3092}\u{4e2d}\u{6b62}\u{3057}\u{307e}\u{3057}\u{305f}",
            "\u{c815}\u{b9ac}\u{ac00} \u{cde8}\u{c18c}\u{b428}",
        )
    } else if failed {
        pick(
            locale,
            "Cleanup Failed",
            "\u{6e05}\u{7406}\u{5931}\u{8d25}",
            "\u{30af}\u{30ea}\u{30fc}\u{30f3}\u{30a2}\u{30c3}\u{30d7}\u{306b}\u{5931}\u{6557}\u{3057}\u{307e}\u{3057}\u{305f}",
            "\u{c815}\u{b9ac} \u{c2e4}\u{d328}",
        )
    } else {
        pick(
            locale,
            "Cleanup Complete",
            "\u{6e05}\u{7406}\u{5b8c}\u{6210}",
            "\u{30af}\u{30ea}\u{30fc}\u{30f3}\u{30a2}\u{30c3}\u{30d7}\u{5b8c}\u{4e86}",
            "\u{c815}\u{b9ac} \u{c644}\u{b8cc}",
        )
    }
}

pub fn cleanup_progress_auto_closing(seconds: u64, locale: Locale) -> String {
    match locale {
        Locale::En => format!("Closing automatically in {seconds}s\u{2026}"),
        Locale::Zh => format!(
            "\u{6b64}\u{7a97}\u{53e3}\u{5c06}\u{5728} {seconds} \u{79d2}\u{540e}\u{81ea}\u{52a8}\u{5173}\u{95ed}\u{2026}"
        ),
        Locale::Ja => format!(
            "\u{3053}\u{306e}\u{30a6}\u{30a3}\u{30f3}\u{30c9}\u{30a6}\u{306f} {seconds} \u{79d2}\u{5f8c}\u{306b}\u{81ea}\u{52d5}\u{3067}\u{9589}\u{3058}\u{307e}\u{3059}\u{2026}"
        ),
        Locale::Ko => format!(
            "\u{c774} \u{cc3d}\u{c740} {seconds}\u{cd08} \u{d6c4} \u{c790}\u{b3d9}\u{c73c}\u{b85c} \u{b2eb}\u{d799}\u{b2c8}\u{b2e4}\u{2026}"
        ),
    }
}

pub const fn cleanup_progress_close_button(locale: Locale) -> &'static str {
    pick(
        locale,
        "Close",
        "\u{5173}\u{95ed}",
        "\u{9589}\u{3058}\u{308b}",
        "\u{b2eb}\u{ae30}",
    )
}
