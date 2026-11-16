//! Locale detection and localized core messages.
//!
//! Supports English (`en`), Simplified Chinese (`zh`), Japanese (`ja`), and
//! Korean (`ko`). System locales resolve once at startup via `sys-locale` and
//! fall back to English for unsupported or POSIX placeholder tags.

use std::sync::OnceLock;

use crate::size::human_bytes;

/// Supported display languages.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash)]
pub enum Locale {
    /// English (default and fallback).
    #[default]
    En,
    /// Simplified Chinese.
    Zh,
    /// Japanese.
    Ja,
    /// Korean.
    Ko,
}

impl Locale {
    /// Map a BCP-47 or POSIX locale tag to a supported [`Locale`].
    ///
    /// Returns `None` for empty strings and POSIX placeholders (`C`, `POSIX`)
    /// so detection can skip them and inspect subsequent preferred locales.
    /// Unsupported language tags (such as `fr-FR` or `de`) map to
    /// `Some(Locale::En)` so fallback stops immediately at the user's primary
    /// language rather than leaking down to a lower-priority entry.
    #[must_use]
    pub fn from_tag(tag: &str) -> Option<Self> {
        let trimmed = tag.trim();
        let primary = trimmed
            .split(['-', '_', '.', '@'])
            .next()
            .unwrap_or_default()
            .to_ascii_lowercase();
        match primary.as_str() {
            "" | "c" | "posix" => None,
            "zh" => Some(Self::Zh),
            "ja" => Some(Self::Ja),
            "ko" => Some(Self::Ko),
            _ => Some(Self::En),
        }
    }

    /// Resolve the first meaningful locale from an ordered preference list.
    #[must_use]
    pub fn resolve_locales<'a>(
        tags: impl IntoIterator<Item = &'a str>,
    ) -> Self {
        tags.into_iter()
            .find_map(Self::from_tag)
            .unwrap_or_default()
    }

    /// Detect the operating system's preferred locale.
    #[must_use]
    pub fn detect() -> Self {
        let locales: Vec<String> = sys_locale::get_locales().collect();
        Self::resolve_locales(locales.iter().map(String::as_str))
    }

    /// Process-wide cached locale detected at first access.
    #[must_use]
    pub fn current() -> Self {
        static CURRENT: OnceLock<Locale> = OnceLock::new();
        *CURRENT.get_or_init(Self::detect)
    }
}

/// Title for a low disk space OS notification.
#[must_use]
pub const fn low_disk_space_title(locale: Locale) -> &'static str {
    match locale {
        Locale::En => "Low Disk Space",
        Locale::Zh => "磁盘空间不足",
        Locale::Ja => "ディスク容量不足",
        Locale::Ko => "디스크 공간 부족",
    }
}

/// Body for a low disk space OS notification.
#[must_use]
pub fn low_disk_space_body(bytes: u64, min_gb: u64, locale: Locale) -> String {
    let avail = human_bytes(bytes);
    match locale {
        Locale::En => format!(
            "Only {avail} remaining (threshold: {min_gb} GiB). \
             Analyzing reclaimable files..."
        ),
        Locale::Zh => format!(
            "仅剩 {avail} 可用空间（阈值：{min_gb} GiB），\
             正在分析可清理文件…"
        ),
        Locale::Ja => format!(
            "残り空き容量は {avail} です（しきい値: {min_gb} GiB）。\
             削除可能なファイルを分析しています…"
        ),
        Locale::Ko => format!(
            "남은 여유 공간이 {avail}입니다(임계값: {min_gb} GiB). \
             정리 가능한 파일을 분석하는 중…"
        ),
    }
}

/// Title for a cleanup completion OS notification.
#[must_use]
pub const fn cleanup_complete_title(locale: Locale) -> &'static str {
    match locale {
        Locale::En => "Cleanup Complete",
        Locale::Zh => "清理完成",
        Locale::Ja => "クリーンアップ完了",
        Locale::Ko => "정리 완료",
    }
}

/// Body for a cleanup completion OS notification.
#[must_use]
pub fn cleanup_complete_body(
    count: usize,
    bytes: u64,
    locale: Locale,
) -> String {
    let size = human_bytes(bytes);
    match locale {
        Locale::En => format!("Cleaned up {count} items ({size})"),
        Locale::Zh => format!("已清理 {count} 个项目（释放 {size}）"),
        Locale::Ja => format!("{count} 件の項目を削除しました（{size}）"),
        Locale::Ko => format!("{count}개 항목을 정리했습니다({size})"),
    }
}

/// Default window/dialog title for watchdog cleanup confirmation requests.
#[must_use]
pub const fn recommended_cleanup_title(locale: Locale) -> &'static str {
    match locale {
        Locale::En => "Ruum Watchdog: Recommended Cleanup",
        Locale::Zh => "Ruum 空间守护：建议清理",
        Locale::Ja => "Ruum ウォッチドッグ: 推奨クリーンアップ",
        Locale::Ko => "Ruum 워치독: 권장 정리",
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn from_tag_maps_supported_and_skips_posix_placeholders() {
        assert_eq!(Locale::from_tag("zh-Hans-CN"), Some(Locale::Zh));
        assert_eq!(Locale::from_tag("zh_TW.UTF-8"), Some(Locale::Zh));
        assert_eq!(Locale::from_tag("ja-JP"), Some(Locale::Ja));
        assert_eq!(Locale::from_tag("ko_KR"), Some(Locale::Ko));
        assert_eq!(Locale::from_tag("en-US"), Some(Locale::En));
        assert_eq!(Locale::from_tag("fr-FR"), Some(Locale::En));
        assert_eq!(Locale::from_tag("de"), Some(Locale::En));
        assert_eq!(Locale::from_tag("es-419"), Some(Locale::En));
        assert_eq!(Locale::from_tag("  C.UTF-8 "), None);
        assert_eq!(Locale::from_tag("POSIX"), None);
        assert_eq!(Locale::from_tag("c@euro"), None);
        assert_eq!(Locale::from_tag(""), None);
        assert_eq!(Locale::from_tag("   "), None);
    }

    #[test]
    fn resolve_locales_respects_priority_and_fallbacks() {
        assert_eq!(Locale::resolve_locales(["zh-Hans-CN"]), Locale::Zh);
        assert_eq!(Locale::resolve_locales(["zh-TW"]), Locale::Zh);
        assert_eq!(Locale::resolve_locales(["ja-JP"]), Locale::Ja);
        assert_eq!(Locale::resolve_locales(["ko-KR"]), Locale::Ko);
        assert_eq!(
            Locale::resolve_locales(["C.UTF-8", "zh-Hans-CN"]),
            Locale::Zh
        );
        assert_eq!(
            Locale::resolve_locales(["POSIX", "", "ja_JP.UTF-8"]),
            Locale::Ja
        );
        // Unsupported primary language stops at English instead of falling
        // through to a secondary language.
        assert_eq!(Locale::resolve_locales(["fr-FR", "ja-JP"]), Locale::En);
        assert_eq!(Locale::resolve_locales(Vec::<&str>::new()), Locale::En);
        assert_eq!(Locale::resolve_locales(["C", "POSIX", ""]), Locale::En);
    }

    #[test]
    fn watchdog_notifications_are_localized_across_all_locales() {
        let gib = 1024 * 1024 * 1024;
        for locale in [Locale::En, Locale::Zh, Locale::Ja, Locale::Ko] {
            assert!(!low_disk_space_title(locale).is_empty());
            let low_body = low_disk_space_body(5 * gib, 20, locale);
            assert!(low_body.contains("5.0 GiB"));
            assert!(low_body.contains("20 GiB"));

            assert!(!cleanup_complete_title(locale).is_empty());
            let done_body = cleanup_complete_body(3, 2 * gib, locale);
            assert!(done_body.contains('3'));
            assert!(done_body.contains("2.0 GiB"));

            assert!(!recommended_cleanup_title(locale).is_empty());
        }

        assert_eq!(low_disk_space_title(Locale::En), "Low Disk Space");
        assert_eq!(low_disk_space_title(Locale::Zh), "磁盘空间不足");
        assert_eq!(low_disk_space_title(Locale::Ja), "ディスク容量不足");
        assert_eq!(low_disk_space_title(Locale::Ko), "디스크 공간 부족");

        assert_eq!(cleanup_complete_title(Locale::En), "Cleanup Complete");
        assert_eq!(cleanup_complete_title(Locale::Zh), "清理完成");
        assert_eq!(cleanup_complete_title(Locale::Ja), "クリーンアップ完了");
        assert_eq!(cleanup_complete_title(Locale::Ko), "정리 완료");
    }

    #[test]
    fn core_types_expose_localized_labels_for_all_locales() {
        use crate::classify::{Category, Reclaim};
        use crate::git::GitState;
        use crate::remote_progress::Stage;
        use crate::removal::{RemovalMode, TrashBackend};
        use crate::tree::Metric;
        use crate::watchdog_config::AiCliProvider;

        let locales = [Locale::En, Locale::Zh, Locale::Ja, Locale::Ko];

        for category in Category::LEGEND
            .into_iter()
            .chain(std::iter::once(Category::Other))
        {
            assert_eq!(category.label_in(Locale::En), category.label());
            for locale in locales {
                assert!(!category.label_in(locale).is_empty());
            }
        }

        for reclaim in [
            Reclaim::Regenerable,
            Reclaim::SyncHistory,
            Reclaim::PackageStore,
            Reclaim::BuildOutput,
            Reclaim::Reinstallable,
            Reclaim::SandboxLayers,
            Reclaim::Snapshots,
            Reclaim::Trash,
            Reclaim::Temporary,
        ] {
            assert_eq!(reclaim.label_in(Locale::En), reclaim.label());
            for locale in locales {
                assert!(!reclaim.label_in(locale).is_empty());
            }
        }

        for metric in [Metric::Bytes, Metric::Files] {
            assert_eq!(metric.label_in(Locale::En), metric.label());
            for locale in locales {
                assert!(!metric.label_in(locale).is_empty());
            }
        }

        for mode in [RemovalMode::Permanent, RemovalMode::Trash] {
            assert_eq!(mode.label_in(Locale::En), mode.label());
            assert_eq!(mode.detail_in(Locale::En), mode.detail());
            for locale in locales {
                assert!(!mode.label_in(locale).is_empty());
                assert!(!mode.detail_in(locale).is_empty());
            }
        }

        for backend in [
            TrashBackend::MacOs,
            TrashBackend::TrashPut,
            TrashBackend::Gio,
            TrashBackend::XdgHome,
            TrashBackend::RecycleBin,
            TrashBackend::Unavailable,
        ] {
            assert_eq!(backend.label_in(Locale::En), backend.label());
            assert_eq!(backend.detail_in(Locale::En), backend.detail());
            for locale in locales {
                assert!(!backend.label_in(locale).is_empty());
                assert!(!backend.detail_in(locale).is_empty());
            }
        }

        for stage in [
            Stage::Connecting,
            Stage::CheckingHelper,
            Stage::UploadingHelper,
            Stage::Starting,
            Stage::Indexing,
            Stage::Finalizing,
        ] {
            assert_eq!(stage.label_in(Locale::En), stage.label());
            for locale in locales {
                assert!(!stage.label_in(locale).is_empty());
            }
        }

        for provider in AiCliProvider::ALL {
            assert_eq!(provider.label_in(Locale::En), provider.label());
            for locale in locales {
                assert!(!provider.label_in(locale).is_empty());
            }
        }
        assert_ne!(
            AiCliProvider::Custom.label_in(Locale::Zh),
            AiCliProvider::Custom.label_in(Locale::En)
        );

        let clean = GitState {
            changed: Some(0),
            stashes: 0,
            unpushed: Some(0),
        };
        let dirty = GitState {
            changed: Some(3),
            stashes: 1,
            unpushed: Some(2),
        };
        let unknown = GitState {
            changed: None,
            stashes: 2,
            unpushed: None,
        };
        for state in [&clean, &dirty, &unknown] {
            assert_eq!(state.summary_in(Locale::En), state.summary());
            for locale in locales {
                assert!(!state.summary_in(locale).is_empty());
            }
        }
        assert_eq!(clean.summary_in(Locale::En), "clean, no stash");
        assert_eq!(
            dirty.summary_in(Locale::En),
            "3 changed, 1 stash, 2 unpushed"
        );
    }
}
