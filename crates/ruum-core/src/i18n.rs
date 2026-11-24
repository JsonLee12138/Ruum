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

/// Maximum character count for single-line AI failure notification summaries.
pub const MAX_AI_FAILURE_SUMMARY_CHARS: usize = 160;

/// Broad category for an AI analysis failure.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum AiFailureKind {
    /// Execution or API request exceeded its allowed timeout.
    TimedOut,
    /// Execution, network, credential, or proposal parsing error.
    Failed,
}

fn contains_http_timeout_status(lower: &str) -> bool {
    const PATTERNS: [&str; 16] = [
        "http 408",
        "http 504",
        "http/1.1 408",
        "http/1.1 504",
        "status 408",
        "status 504",
        "status: 408",
        "status: 504",
        "status code 408",
        "status code 504",
        "code 408",
        "code 504",
        "code: 408",
        "code: 504",
        "(408)",
        "(504)",
    ];
    PATTERNS.iter().any(|p| lower.contains(p))
        || lower.contains("408 request")
        || lower.contains("504 gateway")
}

/// Classify a raw AI analysis error string into timeout vs general failure.
#[must_use]
pub fn classify_ai_failure(err: &str) -> AiFailureKind {
    let lower = err.to_ascii_lowercase();
    if lower.contains("timed out")
        || lower.contains("timedout")
        || lower.contains("time out")
        || lower.contains("time-out")
        || lower.contains("timeout")
        || lower.contains("deadline has elapsed")
        || contains_http_timeout_status(&lower)
    {
        AiFailureKind::TimedOut
    } else {
        AiFailureKind::Failed
    }
}

/// Strip ANSI escape sequences and control characters while turning line breaks
/// into spaces so multi-line errors flatten cleanly.
fn clean_terminal_sequences(text: &str) -> String {
    let mut out = String::with_capacity(text.len());
    let mut chars = text.chars().peekable();
    while let Some(ch) = chars.next() {
        if ch == '\x1b' {
            match chars.peek().copied() {
                Some('[') => {
                    chars.next();
                    for seq_ch in chars.by_ref() {
                        if ('@'..='~').contains(&seq_ch) {
                            break;
                        }
                    }
                }
                Some(']') => {
                    chars.next();
                    for seq_ch in chars.by_ref() {
                        if seq_ch == '\x07' {
                            break;
                        }
                    }
                }
                _ => {}
            }
        } else if ch == '\r' || ch == '\n' || ch == '\t' {
            out.push(' ');
        } else if !ch.is_control() {
            out.push(ch);
        }
    }
    out
}

fn is_sk_secret_token(word: &str) -> bool {
    let trimmed = word.trim_matches(|c: char| {
        matches!(c, '"' | '\'' | '`' | ',' | ';' | '.' | ')' | '(')
    });
    trimmed.strip_prefix("sk-").is_some_and(|rest| {
        rest.len() >= 16
            && rest
                .chars()
                .all(|c| c.is_ascii_alphanumeric() || c == '-' || c == '_')
    })
}

fn redact_single_kv(segment: &str) -> Option<String> {
    let (key, val) = segment.split_once('=')?;
    let key_lc = key.to_ascii_lowercase();
    let sensitive = ["api_key", "apikey", "access_token", "authorization"]
        .iter()
        .any(|needle| key_lc.ends_with(needle));
    if !sensitive || val.is_empty() {
        return None;
    }
    Some(format!("{key}=[REDACTED]"))
}

fn redact_query_or_kv_secret(word: &str) -> Option<String> {
    if word.contains('&') {
        let mut changed = false;
        let parts: Vec<String> = word
            .split('&')
            .map(|seg| match redact_single_kv(seg) {
                Some(r) => {
                    changed = true;
                    r
                }
                None => seg.to_owned(),
            })
            .collect();
        return changed.then(|| parts.join("&"));
    }
    redact_single_kv(word)
}

/// Sanitize an AI error reason into a clean, redacted single-line string.
#[must_use]
pub fn clean_ai_failure_reason(err: &str) -> String {
    let cleaned = clean_terminal_sequences(err);
    let mut words = Vec::new();
    let mut redact_next = false;
    for raw_word in cleaned.split_whitespace() {
        if redact_next {
            words.push("[REDACTED]".to_owned());
            redact_next = false;
            continue;
        }
        if raw_word.eq_ignore_ascii_case("bearer") {
            words.push(raw_word.to_owned());
            redact_next = true;
            continue;
        }
        if is_sk_secret_token(raw_word) {
            words.push("[REDACTED]".to_owned());
            continue;
        }
        if let Some(redacted) = redact_query_or_kv_secret(raw_word) {
            words.push(redacted);
            continue;
        }
        words.push(raw_word.to_owned());
    }
    words.join(" ")
}

/// Normalize whitespace to a single line and cap at `max_chars` scalar chars.
#[must_use]
pub fn truncate_single_line(text: &str, max_chars: usize) -> String {
    let single = text.split_whitespace().collect::<Vec<_>>().join(" ");
    if single.chars().count() <= max_chars {
        return single;
    }
    if max_chars == 0 {
        return String::new();
    }
    let mut out: String = single.chars().take(max_chars - 1).collect();
    out.push('\u{2026}');
    out
}

/// Core notification title for a general AI analysis failure.
#[must_use]
pub const fn ai_analysis_failed_title(locale: Locale) -> &'static str {
    match locale {
        Locale::En => "AI Analysis Failed",
        Locale::Zh => "AI 分析失败",
        Locale::Ja => "AI 分析失敗",
        Locale::Ko => "AI 분석 실패",
    }
}

/// Core notification title for an AI analysis timeout.
#[must_use]
pub const fn ai_analysis_timed_out_title(locale: Locale) -> &'static str {
    match locale {
        Locale::En => "AI Analysis Timed Out",
        Locale::Zh => "AI 分析超时",
        Locale::Ja => "AI 分析タイムアウト",
        Locale::Ko => "AI 분석 시간 초과",
    }
}

/// Select the localized core notification title matching `kind`.
#[must_use]
pub const fn ai_analysis_failure_title(
    kind: AiFailureKind,
    locale: Locale,
) -> &'static str {
    match kind {
        AiFailureKind::TimedOut => ai_analysis_timed_out_title(locale),
        AiFailureKind::Failed => ai_analysis_failed_title(locale),
    }
}

/// Format a raw AI analysis failure into a localized, redacted, single-line
/// summary capped at 160 characters.
#[must_use]
pub fn format_ai_failure_summary(err: &str, locale: Locale) -> String {
    let kind = classify_ai_failure(err);
    let detail = clean_ai_failure_reason(err);
    let msg = if detail.is_empty() {
        match (kind, locale) {
            (AiFailureKind::TimedOut, Locale::En) => {
                "AI analysis timed out.".to_owned()
            }
            (AiFailureKind::TimedOut, Locale::Zh) => "AI 分析超时。".to_owned(),
            (AiFailureKind::TimedOut, Locale::Ja) => {
                "AI 分析がタイムアウトしました。".to_owned()
            }
            (AiFailureKind::TimedOut, Locale::Ko) => {
                "AI 분석 시간이 초과되었습니다.".to_owned()
            }
            (AiFailureKind::Failed, Locale::En) => {
                "AI analysis failed.".to_owned()
            }
            (AiFailureKind::Failed, Locale::Zh) => "AI 分析失败。".to_owned(),
            (AiFailureKind::Failed, Locale::Ja) => {
                "AI 分析に失敗しました。".to_owned()
            }
            (AiFailureKind::Failed, Locale::Ko) => {
                "AI 분석에 실패했습니다.".to_owned()
            }
        }
    } else {
        match (kind, locale) {
            (AiFailureKind::TimedOut, Locale::En) => {
                format!("AI analysis timed out: {detail}")
            }
            (AiFailureKind::TimedOut, Locale::Zh) => {
                format!("AI 分析超时：{detail}")
            }
            (AiFailureKind::TimedOut, Locale::Ja) => {
                format!("AI 分析がタイムアウトしました: {detail}")
            }
            (AiFailureKind::TimedOut, Locale::Ko) => {
                format!("AI 분석 시간이 초과되었습니다: {detail}")
            }
            (AiFailureKind::Failed, Locale::En) => {
                format!("AI analysis failed: {detail}")
            }
            (AiFailureKind::Failed, Locale::Zh) => {
                format!("AI 分析失败：{detail}")
            }
            (AiFailureKind::Failed, Locale::Ja) => {
                format!("AI 分析に失敗しました: {detail}")
            }
            (AiFailureKind::Failed, Locale::Ko) => {
                format!("AI 분석에 실패했습니다: {detail}")
            }
        }
    };
    truncate_single_line(&msg, MAX_AI_FAILURE_SUMMARY_CHARS)
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
        #[cfg(feature = "service")]
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

        #[cfg(feature = "service")]
        {
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
        }

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

    #[test]
    fn ai_failure_classification_formatting_redaction_and_bounds() {
        for raw in [
            "command `/usr/bin/agy` timed out after 180s",
            "request timed out after 120s",
            "TimedOut",
            "API request timeout",
            "HTTP 504 upstream gateway",
            "HTTP 408 Request",
            "HTTP error (status 504): upstream failure",
            "status: 408 while waiting",
            "504 Gateway Time-out",
        ] {
            assert_eq!(classify_ai_failure(raw), AiFailureKind::TimedOut);
        }
        for raw in [
            "AI CLI exited with code 1: cli crashed",
            "invalid JSON proposal",
            "connection refused",
            "connect 127.0.0.1:50412 failed",
            "freed 4080 bytes",
        ] {
            assert_eq!(classify_ai_failure(raw), AiFailureKind::Failed);
        }

        let long_cjk = format!("错误详情：\n{}\n{}", "甲".repeat(200), "乙");
        let secret_err = concat!(
            "\x1b[31mHTTP 401 Bearer sk-proj-1234567890abcdef1234\x1b[0m\n",
            "GET https://api.example.com/v1?api_key=raw-secret-1&access_token=tok-secret-2\n",
            "retry failed"
        );
        let locales = [Locale::En, Locale::Zh, Locale::Ja, Locale::Ko];

        let mut timed_out_titles = Vec::new();
        let mut failed_titles = Vec::new();
        for locale in locales {
            let t_title = ai_analysis_timed_out_title(locale);
            let f_title = ai_analysis_failed_title(locale);
            assert!(!t_title.is_empty());
            assert!(!f_title.is_empty());
            assert_ne!(t_title, f_title);
            timed_out_titles.push(t_title);
            failed_titles.push(f_title);

            let timeout_summary = format_ai_failure_summary(
                "command timed out after 30s",
                locale,
            );
            assert!(timeout_summary.chars().count() <= 160);
            assert!(!timeout_summary.contains('\n'));
            assert!(!timeout_summary.contains('\r'));

            let long_summary = format_ai_failure_summary(&long_cjk, locale);
            assert!(
                long_summary.chars().count() <= 160,
                "summary exceeded 160 chars: {}",
                long_summary.chars().count()
            );
            assert!(!long_summary.contains('\n'));

            let redacted = format_ai_failure_summary(secret_err, locale);
            assert!(!redacted.contains("sk-proj-1234567890abcdef1234"));
            assert!(!redacted.contains("raw-secret-1"));
            assert!(!redacted.contains("tok-secret-2"));
            assert!(redacted.contains("[REDACTED]"));
            assert!(!redacted.contains('\x1b'));
            assert!(!redacted.contains('\n'));
        }
        for i in 1..locales.len() {
            assert_ne!(timed_out_titles[i], timed_out_titles[0]);
            assert_ne!(failed_titles[i], failed_titles[0]);
        }
    }
}
