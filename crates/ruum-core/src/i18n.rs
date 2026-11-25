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
        || lower.contains("请求超时")
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

/// Normalize whitespace to a single line and cap at `max_chars` while
/// preserving a trailing `[Trace: <id>]` suffix when it fits.
#[must_use]
pub fn truncate_preserving_trace(text: &str, max_chars: usize) -> String {
    let single = text.split_whitespace().collect::<Vec<_>>().join(" ");
    if single.chars().count() <= max_chars {
        return single;
    }
    if let Some(pos) = single.rfind("[Trace:") {
        let trace_suffix = &single[pos..];
        let trace_chars = trace_suffix.chars().count();
        if trace_suffix.ends_with(']')
            && trace_chars.saturating_add(2) <= max_chars
        {
            let prefix = single[..pos].trim_end();
            let avail = max_chars - trace_chars - 2;
            let mut out: String = prefix.chars().take(avail).collect();
            out.push('\u{2026}');
            out.push(' ');
            out.push_str(trace_suffix);
            return out;
        }
    }
    truncate_single_line(&single, max_chars)
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
    truncate_preserving_trace(&msg, MAX_AI_FAILURE_SUMMARY_CHARS)
}

/// Format a Ruum Cloud gateway or network error into a localized message with a
/// trailing `[Trace: <trace_id>]` tag when available.
#[must_use]
pub fn format_cloud_error_message(
    code: Option<&str>,
    http_status: Option<u16>,
    fallback_message: &str,
    trace_id: Option<&str>,
    is_timeout: bool,
    locale: Locale,
) -> String {
    let norm_code = code.map(str::trim).filter(|s| !s.is_empty());
    let zh = matches!(locale, Locale::Zh);
    let base: String = if is_timeout
        || matches!(norm_code, Some("COMMON_UPSTREAM_TIMEOUT"))
        || matches!(http_status, Some(408 | 504))
    {
        if zh {
            "云端 AI 请求超时，请稍后重试或切换本地 CLI / 自带 API Key。"
        } else {
            "Cloud AI request timed out. Retry later or switch to Local CLI / Custom API."
        }
        .to_owned()
    } else {
        match norm_code {
            Some("ENTITLEMENT_PAYMENT_REQUIRED") => {
                if zh {
                    "免费体验次数已用完或云端会员已到期，请开通云端会员或使用兑换码。"
                } else {
                    "Free trial exhausted or Cloud Pro expired. Upgrade to Cloud Pro or redeem a code."
                }
                .to_owned()
            }
            Some("ENTITLEMENT_DENIED") => {
                if zh {
                    "当前设备主体权益被拒绝访问。"
                } else {
                    "Cloud AI access was denied for this device."
                }
                .to_owned()
            }
            Some("INFERENCE_MODEL_NOT_ALLOWED") => {
                if zh {
                    "当前权益层级无权使用所选云端模型，请开通云端会员或切换可用模型。"
                } else {
                    "Selected cloud model requires Cloud Pro. Upgrade or choose an available model."
                }
                .to_owned()
            }
            Some("INFERENCE_FUP_LIMIT_EXCEEDED") => match locale {
                Locale::En => {
                    "Daily or monthly usage limit reached. Retry tomorrow or switch to Local CLI / Custom API."
                }
                Locale::Zh => {
                    "已达到今日或本月用量上限，请次日重试或切换本地 CLI / 自带 API Key。"
                }
                Locale::Ja => {
                    "本日または今月の利用上限に達しました。翌日再試行するか、ローカル CLI / カスタム API に切り替えてください。"
                }
                Locale::Ko => {
                    "일일 또는 월간 사용 한도에 도달했습니다. 내일 다시 시도하거나 로컬 CLI / 사용자 API로 전환하세요."
                }
            }
            .to_owned(),
            Some("INFERENCE_CONCURRENCY_LIMITED") => {
                if zh {
                    "当前设备已有云端分析任务正在执行，请等待完成后重试。"
                } else {
                    "Another cloud analysis is already running on this device. Wait and retry."
                }
                .to_owned()
            }
            Some("INFERENCE_SESSION_LIMIT_EXCEEDED") => {
                if zh {
                    "单次云端分析轮数已达上限。"
                } else {
                    "Cloud analysis session turn limit exceeded."
                }
                .to_owned()
            }
            Some("COMMON_RATE_LIMITED" | "INFERENCE_RATE_LIMITED") => {
                if zh {
                    "请求过于频繁，请稍后重试。"
                } else {
                    "Too many requests. Please try again shortly."
                }
                .to_owned()
            }
            Some("AUTH_DEVICE_REVOKED") => {
                if zh {
                    "当前设备已被吊销云端访问权限。"
                } else {
                    "This device's cloud access has been revoked."
                }
                .to_owned()
            }
            Some("BILLING_UNAVAILABLE") => match locale {
                Locale::En => {
                    "Online payment is unavailable. You can use a redemption code."
                }
                Locale::Zh => "在线收款尚未开启或暂不可用，可使用兑换码。",
                Locale::Ja => {
                    "オンライン決済は現在利用できません。引換コードをご利用いただけます。"
                }
                Locale::Ko => {
                    "온라인 결제를 현재 사용할 수 없습니다. 교환 코드를 사용할 수 있습니다."
                }
            }
            .to_owned(),
            Some("BILLING_PRODUCT_DISABLED") => match locale {
                Locale::En => {
                    "Billing product is currently disabled. You can use a redemption code."
                }
                Locale::Zh => "当前收款商品暂未开启购买，可使用兑换码。",
                Locale::Ja => {
                    "現在このプランの購入は無効です。引換コードをご利用いただけます。"
                }
                Locale::Ko => {
                    "현재 결제 상품 구매가 비활성화되어 있습니다. 교환 코드를 사용할 수 있습니다."
                }
            }
            .to_owned(),
            Some("REDEMPTION_CODE_INVALID") => match locale {
                Locale::En => "Redemption code is invalid or does not exist.",
                Locale::Zh => "兑换码不存在或格式无效。",
                Locale::Ja => "引換コードが無効か、存在しません。",
                Locale::Ko => "교환 코드가 잘못되었거나 존재하지 않습니다.",
            }
            .to_owned(),
            Some("REDEMPTION_CODE_ALREADY_USED") => match locale {
                Locale::En => {
                    "This one-time redemption code has already been used and is permanently invalidated."
                }
                Locale::Zh => {
                    "该兑换码已被使用并永久失效（仅限一次性核销，不可重复使用）。"
                }
                Locale::Ja => {
                    "この使い切り引換コードはすでに使用され、永久に無効化されています。"
                }
                Locale::Ko => {
                    "이 일회용 교환 코드는 이미 사용되어 영구적으로 무효화되었습니다."
                }
            }
            .to_owned(),
            Some("REDEMPTION_CODE_EXPIRED") => match locale {
                Locale::En => "This redemption code has expired.",
                Locale::Zh => "该兑换码已过期。",
                Locale::Ja => "この引換コードは有効期限が切れています。",
                Locale::Ko => "이 교환 코드는 만료되었습니다.",
            }
            .to_owned(),
            Some("REDEMPTION_CODE_REVOKED") => match locale {
                Locale::En => "This redemption code has been revoked.",
                Locale::Zh => "该兑换码已作废。",
                Locale::Ja => "この引換コードは取り消されています。",
                Locale::Ko => "이 교환 코드는 폐기되었습니다.",
            }
            .to_owned(),
            Some("REDEMPTION_RATE_LIMITED") => match locale {
                Locale::En => {
                    "Too many redemption attempts. Please try again later."
                }
                Locale::Zh => "兑换尝试过于频繁，请稍后再试。",
                Locale::Ja => {
                    "引換の試行回数が多すぎます。しばらくしてからもう一度お試しください。"
                }
                Locale::Ko => {
                    "교환 시도가 너무 많습니다. 잠시 후 다시 시도하세요."
                }
            }
            .to_owned(),
            Some(
                "INFERENCE_GLOBAL_BUDGET_EXCEEDED"
                | "INFERENCE_NO_AVAILABLE_UPSTREAM"
                | "COMMON_SERVICE_UNAVAILABLE"
                | "COMMON_UPSTREAM_PROTOCOL_ERROR"
                | "COMMON_INTERNAL_ERROR",
            ) => {
                if zh {
                    "云端 AI 服务暂不可用，请稍后重试或切换本地 CLI / 自带 API Key。"
                } else {
                    "Cloud AI service is temporarily unavailable. Retry later or switch to Local CLI / Custom API."
                }
                .to_owned()
            }
            _ if http_status == Some(402) => {
                if zh {
                    "免费体验次数已用完或云端会员已到期，请开通云端会员或使用兑换码。"
                } else {
                    "Free trial exhausted or Cloud Pro expired. Upgrade to Cloud Pro or redeem a code."
                }
                .to_owned()
            }
            _ if http_status.is_some_and(|s| s >= 500) => {
                if zh {
                    "云端 AI 服务暂不可用，请稍后重试或切换本地 CLI / 自带 API Key。"
                } else {
                    "Cloud AI service is temporarily unavailable. Retry later or switch to Local CLI / Custom API."
                }
                .to_owned()
            }
            _ => {
                let cleaned = clean_ai_failure_reason(fallback_message);
                if cleaned.is_empty() {
                    if zh {
                        "云端 AI 请求失败，请稍后重试。"
                    } else {
                        "Cloud AI request failed. Please try again later."
                    }
                    .to_owned()
                } else {
                    cleaned
                }
            }
        }
    };

    let clean_trace = trace_id.map(str::trim).filter(|s| !s.is_empty());
    match clean_trace {
        Some(tid) => format!("{base} [Trace: {tid}]"),
        None => base,
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

    #[test]
    fn cloud_error_messages_localize_all_codes_and_preserve_trace_id() {
        let codes = [
            "ENTITLEMENT_PAYMENT_REQUIRED",
            "INFERENCE_FUP_LIMIT_EXCEEDED",
            "INFERENCE_CONCURRENCY_LIMITED",
            "COMMON_RATE_LIMITED",
            "AUTH_DEVICE_REVOKED",
            "BILLING_UNAVAILABLE",
            "BILLING_PRODUCT_DISABLED",
            "REDEMPTION_CODE_INVALID",
            "REDEMPTION_CODE_ALREADY_USED",
            "REDEMPTION_CODE_EXPIRED",
            "REDEMPTION_CODE_REVOKED",
            "REDEMPTION_RATE_LIMITED",
        ];
        for code in codes {
            let en = format_cloud_error_message(
                Some(code),
                None,
                "raw",
                Some("tr-123"),
                false,
                Locale::En,
            );
            let zh = format_cloud_error_message(
                Some(code),
                None,
                "raw",
                Some("tr-123"),
                false,
                Locale::Zh,
            );
            assert!(en.ends_with("[Trace: tr-123]"), "code={code} en={en}");
            assert!(zh.ends_with("[Trace: tr-123]"), "code={code} zh={zh}");
            assert_ne!(en, zh, "code={code}");
        }

        let timeout_zh = format_cloud_error_message(
            None,
            None,
            "timed out",
            Some("tr-to"),
            true,
            Locale::Zh,
        );
        assert!(timeout_zh.contains("超时"));
        assert!(timeout_zh.ends_with("[Trace: tr-to]"));
        assert_eq!(classify_ai_failure(&timeout_zh), AiFailureKind::TimedOut);

        let err_503 = format_cloud_error_message(
            None,
            Some(503),
            "unavailable",
            Some("tr-503"),
            false,
            Locale::En,
        );
        assert!(err_503.contains("unavailable"));
        assert!(err_503.ends_with("[Trace: tr-503]"));
    }

    #[test]
    fn format_ai_failure_summary_preserves_trace_id_under_cap() {
        let long_raw = "Cloud gateway returned a very verbose diagnostic \
             message describing an upstream model failure that exceeds one \
             hundred and sixty chars by a wide margin \
             [Trace: trace-503-9f8e7d6c]";
        let summary = format_ai_failure_summary(long_raw, Locale::En);
        assert!(
            summary.chars().count() <= MAX_AI_FAILURE_SUMMARY_CHARS,
            "summary too long: {}",
            summary.chars().count()
        );
        assert!(
            summary.ends_with("[Trace: trace-503-9f8e7d6c]"),
            "trace id lost in summary: {summary}"
        );
    }

    #[test]
    fn redemption_error_codes_localize_distinctly_across_all_locales_with_trace()
     {
        let codes = [
            "REDEMPTION_CODE_INVALID",
            "REDEMPTION_CODE_ALREADY_USED",
            "REDEMPTION_CODE_EXPIRED",
            "REDEMPTION_CODE_REVOKED",
            "REDEMPTION_RATE_LIMITED",
        ];
        let locales = [Locale::En, Locale::Zh, Locale::Ja, Locale::Ko];
        for locale in locales {
            let mut seen = std::collections::BTreeSet::new();
            for code in codes {
                let msg = format_cloud_error_message(
                    Some(code),
                    None,
                    "raw fallback",
                    Some("tr-red-1"),
                    false,
                    locale,
                );
                assert!(msg.ends_with("[Trace: tr-red-1]"), "{code}: {msg}");
                assert!(
                    seen.insert(msg),
                    "duplicate msg for {code} in {locale:?}"
                );
            }
        }

        let used_en = format_cloud_error_message(
            Some("REDEMPTION_CODE_ALREADY_USED"),
            Some(409),
            "used",
            Some("tr-used"),
            false,
            Locale::En,
        );
        assert!(used_en.to_ascii_lowercase().contains("one-time"));
        assert!(used_en.to_ascii_lowercase().contains("permanently"));

        let used_zh = format_cloud_error_message(
            Some("REDEMPTION_CODE_ALREADY_USED"),
            Some(409),
            "used",
            Some("tr-used"),
            false,
            Locale::Zh,
        );
        assert!(used_zh.contains("永久失效"));
        assert!(used_zh.contains("一次"));
    }

    #[test]
    fn cloud_usage_limit_and_billing_errors_no_fup_or_supporter() {
        let locales = [Locale::En, Locale::Zh, Locale::Ja, Locale::Ko];

        let mut fup_msgs = std::collections::BTreeSet::new();
        for locale in locales {
            let msg = format_cloud_error_message(
                Some("INFERENCE_FUP_LIMIT_EXCEEDED"),
                Some(429),
                "raw",
                Some("tr-limit"),
                false,
                locale,
            );
            let lower = msg.to_ascii_lowercase();
            assert!(!lower.contains("fup"), "locale={locale:?} msg={msg}");
            assert!(!lower.contains("fair-use"), "locale={locale:?} msg={msg}");
            assert!(!msg.contains("公平使用"), "locale={locale:?} msg={msg}");
            assert!(msg.ends_with("[Trace: tr-limit]"));
            assert!(fup_msgs.insert(msg));
        }

        let expected_redeem_terms = [
            (Locale::En, "redemption code"),
            (Locale::Zh, "兑换码"),
            (Locale::Ja, "引換コード"),
            (Locale::Ko, "교환 코드"),
        ];
        for code in ["BILLING_UNAVAILABLE", "BILLING_PRODUCT_DISABLED"] {
            let mut seen = std::collections::BTreeSet::new();
            for (locale, expected_term) in expected_redeem_terms {
                let msg = format_cloud_error_message(
                    Some(code),
                    Some(503),
                    "raw",
                    Some("tr-bill"),
                    false,
                    locale,
                );
                let lower = msg.to_ascii_lowercase();
                assert!(
                    !lower.contains("supporter"),
                    "code={code} locale={locale:?} msg={msg}"
                );
                assert!(
                    !msg.contains("打赏兑换码"),
                    "code={code} locale={locale:?} msg={msg}"
                );
                assert!(
                    msg.contains(expected_term),
                    "code={code} locale={locale:?} msg={msg}"
                );
                assert!(msg.ends_with("[Trace: tr-bill]"));
                assert!(seen.insert(msg));
            }
        }
    }

    #[test]
    fn cloud_membership_errors_use_cloud_pro_terminology() {
        for (code, status) in [
            (Some("ENTITLEMENT_PAYMENT_REQUIRED"), Some(402)),
            (Some("INFERENCE_MODEL_NOT_ALLOWED"), Some(403)),
            (None, Some(402)),
        ] {
            let zh = format_cloud_error_message(
                code,
                status,
                "raw",
                Some("tr-pro"),
                false,
                Locale::Zh,
            );
            assert!(zh.contains("云端会员"), "code={code:?} zh={zh}");
            assert!(
                !zh.contains("\u{65f6}\u{957f}\u{5305}"),
                "code={code:?} zh={zh}"
            );
            assert!(zh.ends_with("[Trace: tr-pro]"));

            let en = format_cloud_error_message(
                code,
                status,
                "raw",
                Some("tr-pro"),
                false,
                Locale::En,
            );
            assert!(en.contains("Cloud Pro"), "code={code:?} en={en}");
            assert!(en.ends_with("[Trace: tr-pro]"));
        }
    }
}
