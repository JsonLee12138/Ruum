//! Shared localized strings for menus, tray, power presets, age buckets, and
//! shared widgets.

use ruum_core::size::human_bytes;

use super::{Locale, pick};
use crate::power::PowerEfficiency;

// ── app menu ────────────────────────────────────────────────────────────

pub const fn menu_services(locale: Locale) -> &'static str {
    pick(locale, "Services", "服务", "サービス", "서비스")
}

pub const fn menu_settings(locale: Locale) -> &'static str {
    pick(
        locale,
        "Settings\u{2026}",
        "设置\u{2026}",
        "設定\u{2026}",
        "설정\u{2026}",
    )
}

pub const fn menu_install_cli(locale: Locale) -> &'static str {
    pick(
        locale,
        "Install CLI Command\u{2026}",
        "安装命令行工具\u{2026}",
        "CLI コマンドをインストール\u{2026}",
        "CLI 명령 설치\u{2026}",
    )
}

pub const fn menu_check_updates(locale: Locale) -> &'static str {
    pick(
        locale,
        "Check for Updates\u{2026}",
        "检查更新\u{2026}",
        "アップデートを確認\u{2026}",
        "업데이트 확인\u{2026}",
    )
}

pub const fn menu_hide_app(locale: Locale) -> &'static str {
    pick(
        locale,
        "Hide ruum",
        "隐藏 ruum",
        "ruum を隠す",
        "ruum 가리기",
    )
}

pub const fn menu_hide_others(locale: Locale) -> &'static str {
    pick(
        locale,
        "Hide Others",
        "隐藏其他",
        "ほかを隠す",
        "기타 가리기",
    )
}

pub const fn menu_show_all(locale: Locale) -> &'static str {
    pick(locale, "Show All", "全部显示", "すべて表示", "모두 보기")
}

pub const fn menu_quit_app(locale: Locale) -> &'static str {
    pick(locale, "Quit ruum", "退出 ruum", "ruum を終了", "ruum 종료")
}

pub const fn menu_file(locale: Locale) -> &'static str {
    pick(locale, "File", "文件", "ファイル", "파일")
}

pub const fn menu_open_folder(locale: Locale) -> &'static str {
    pick(
        locale,
        "Open Folder\u{2026}",
        "打开文件夹\u{2026}",
        "フォルダを開く\u{2026}",
        "폴더 열기\u{2026}",
    )
}

pub const fn menu_show_in_finder(locale: Locale) -> &'static str {
    pick(
        locale,
        "Show in Finder",
        "在访达中显示",
        "Finder で表示",
        "Finder에서 보기",
    )
}

pub const fn menu_rescan(locale: Locale) -> &'static str {
    pick(locale, "Rescan", "重新扫描", "再スキャン", "다시 스캔")
}

pub const fn menu_close_window(locale: Locale) -> &'static str {
    pick(
        locale,
        "Close Window",
        "关闭窗口",
        "ウィンドウを閉じる",
        "창 닫기",
    )
}

pub const fn menu_view(locale: Locale) -> &'static str {
    pick(locale, "View", "视图", "表示", "보기")
}

pub const fn menu_explore(locale: Locale) -> &'static str {
    pick(locale, "Explore", "探索", "探索", "탐색")
}

pub const fn menu_docker(locale: Locale) -> &'static str {
    pick(locale, "Docker", "Docker", "Docker", "Docker")
}

pub const fn menu_remote(locale: Locale) -> &'static str {
    pick(locale, "Remote", "远程", "リモート", "원격")
}

pub const fn menu_watchdog(locale: Locale) -> &'static str {
    pick(locale, "Watchdog", "空间守护", "ウォッチドッグ", "워치독")
}

pub const fn menu_show_main_window(locale: Locale) -> &'static str {
    pick(
        locale,
        "Show Main Window",
        "显示主窗口",
        "メインウィンドウを表示",
        "기본 창 보기",
    )
}

pub const fn menu_go(locale: Locale) -> &'static str {
    pick(locale, "Go", "前往", "移動", "이동")
}

pub const fn menu_back(locale: Locale) -> &'static str {
    pick(locale, "Back", "后退", "戻る", "뒤로")
}

pub const fn menu_forward(locale: Locale) -> &'static str {
    pick(locale, "Forward", "前进", "進む", "앞으로")
}

// ── tray menu ───────────────────────────────────────────────────────────

pub const fn tray_open_ruum(locale: Locale) -> &'static str {
    pick(locale, "Open Ruum", "打开 Ruum", "Ruum を開く", "Ruum 열기")
}

/// Tray label for the Remote SSH servers page.
pub const fn tray_remote(locale: Locale) -> &'static str {
    menu_remote(locale)
}

pub const fn tray_watchdog(locale: Locale) -> &'static str {
    menu_watchdog(locale)
}

pub const fn tray_settings(locale: Locale) -> &'static str {
    menu_settings(locale)
}

pub const fn tray_check_updates(locale: Locale) -> &'static str {
    menu_check_updates(locale)
}

pub fn tray_update_available(version: &str, locale: Locale) -> String {
    let ver = version.trim().trim_start_matches('v');
    match locale {
        Locale::En => format!("Update Available (v{ver})\u{2026}"),
        Locale::Zh => format!("发现新版本 (v{ver})\u{2026}"),
        Locale::Ja => format!("アップデートあり (v{ver})\u{2026}"),
        Locale::Ko => format!("업데이트 가능 (v{ver})\u{2026}"),
    }
}

pub const fn tray_run_ai_analysis(locale: Locale) -> &'static str {
    pick(
        locale,
        "Run AI Analysis Now",
        "立即运行 AI 分析",
        "今すぐ AI 分析を実行",
        "지금 AI 분석 실행",
    )
}

pub const fn tray_review_latest_proposal(locale: Locale) -> &'static str {
    pick(
        locale,
        "Review Latest Proposal\u{2026}",
        "查看最新建议\u{2026}",
        "最新の提案を確認\u{2026}",
        "최신 제안 검토\u{2026}",
    )
}

pub fn tray_cleanup_progress_active(
    completed: usize,
    total: usize,
    cancelling: bool,
    locale: Locale,
) -> String {
    if cancelling {
        match locale {
            Locale::En => format!(
                "Stopping Cleanup ({completed}/{total})\u{2026} \u{00b7} View Progress"
            ),
            Locale::Zh => format!(
                "正在停止清理 ({completed}/{total})\u{2026} \u{00b7} 查看进度"
            ),
            Locale::Ja => format!(
                "クリーンアップを停止中 ({completed}/{total})\u{2026} \u{00b7} 進捗を表示"
            ),
            Locale::Ko => format!(
                "정리 중지 중 ({completed}/{total})\u{2026} \u{00b7} 진행 상태 보기"
            ),
        }
    } else {
        match locale {
            Locale::En => format!(
                "Cleaning ({completed}/{total})\u{2026} \u{00b7} View Progress"
            ),
            Locale::Zh => format!(
                "正在清理 ({completed}/{total})\u{2026} \u{00b7} 查看进度"
            ),
            Locale::Ja => format!(
                "クリーンアップ中 ({completed}/{total})\u{2026} \u{00b7} 進捗を表示"
            ),
            Locale::Ko => format!(
                "정리 중 ({completed}/{total})\u{2026} \u{00b7} 진행 상태 보기"
            ),
        }
    }
}

pub const fn tray_cleanup_progress_idle(locale: Locale) -> &'static str {
    pick(
        locale,
        "No Active Cleanup",
        "暂无进行中的清理",
        "実行中のクリーンアップはありません",
        "진행 중인 정리 작업 없음",
    )
}

pub const fn tray_quit_ruum(locale: Locale) -> &'static str {
    pick(locale, "Quit Ruum", "退出 Ruum", "Ruum を終了", "Ruum 종료")
}

pub fn tray_status_label(free_bytes: Option<u64>, locale: Locale) -> String {
    let free = free_bytes.map_or_else(|| "\u{2014}".to_owned(), human_bytes);
    let watchdog = pick(
        locale,
        "Watchdog Active",
        "\u{7a7a}\u{95f4}\u{5b88}\u{62a4}\u{4e2d}",
        "\u{76e3}\u{8996}\u{4e2d}",
        "\u{c6cc}\u{ce58}\u{b3c5} \u{d65c}\u{c131}",
    );
    match locale {
        Locale::En => format!("Free: {free} \u{00b7} {watchdog}"),
        Locale::Zh => format!("可用：{free} \u{00b7} {watchdog}"),
        Locale::Ja => format!("空き: {free} \u{00b7} {watchdog}"),
        Locale::Ko => format!("여유: {free} \u{00b7} {watchdog}"),
    }
}

// ── power efficiency ────────────────────────────────────────────────────

pub const fn power_name(
    preset: PowerEfficiency,
    locale: Locale,
) -> &'static str {
    match preset {
        PowerEfficiency::Miser => {
            pick(locale, "Miser", "节能", "省電力", "절전")
        }
        PowerEfficiency::Balanced => {
            pick(locale, "Balanced", "均衡", "バランス", "균형")
        }
        PowerEfficiency::Aggressive => {
            pick(locale, "Aggressive", "性能", "高パフォーマンス", "고성능")
        }
        PowerEfficiency::DrainMyBattery => pick(
            locale,
            "Drain My Battery",
            "全速狂飙",
            "フルパワー",
            "최대 성능",
        ),
    }
}

pub const fn power_custom_name(locale: Locale) -> &'static str {
    pick(locale, "Custom", "自定义", "カスタム", "사용자 지정")
}

pub fn workers(count: usize, locale: Locale) -> String {
    match locale {
        Locale::En => {
            let unit = if count == 1 { "worker" } else { "workers" };
            format!("{count} {unit}")
        }
        Locale::Zh => format!("{count} 个工作线程"),
        Locale::Ja => format!("{count} ワーカー"),
        Locale::Ko => format!("작업자 {count}개"),
    }
}

pub fn power_label(name: &str, workers: &str, locale: Locale) -> String {
    match locale {
        Locale::Zh => format!("{name}（{workers}）"),
        Locale::En | Locale::Ja | Locale::Ko => format!("{name} ({workers})"),
    }
}

// ── age buckets ─────────────────────────────────────────────────────────

pub const fn age_bucket_label(index: usize, locale: Locale) -> &'static str {
    match index {
        0 => pick(locale, "This week", "本周", "今週", "이번 주"),
        1 => pick(locale, "This month", "本月", "今月", "이번 달"),
        2 => pick(locale, "Six months", "半年内", "6か月以内", "6개월 이내"),
        3 => pick(locale, "This year", "今年", "今年", "올해"),
        _ => pick(locale, "Older", "更早", "それ以前", "이전"),
    }
}

// ── shared widgets ──────────────────────────────────────────────────────

pub fn space_meter_reclaiming(
    available: &str,
    after: &str,
    reclaiming: &str,
    locale: Locale,
) -> String {
    match locale {
        Locale::En => format!(
            "{available} free \u{00b7} {after} after removing {reclaiming}"
        ),
        Locale::Zh => {
            format!(
                "可用 {available} \u{00b7} 清理 {reclaiming} 后可用 {after}"
            )
        }
        Locale::Ja => {
            format!("空き {available} \u{00b7} {reclaiming} 削除後は {after}")
        }
        Locale::Ko => {
            format!("여유 {available} \u{00b7} {reclaiming} 삭제 후 {after}")
        }
    }
}

pub fn space_meter_total(
    available: &str,
    total: &str,
    locale: Locale,
) -> String {
    match locale {
        Locale::En => format!("{available} free of {total}"),
        Locale::Zh => format!("可用 {available} / 共 {total}"),
        Locale::Ja => format!("空き {available} / 合計 {total}"),
        Locale::Ko => format!("여유 {available} / 전체 {total}"),
    }
}

pub fn ago(now: i64, then: i64, locale: Locale) -> String {
    if then <= 0 {
        return pick(locale, "unknown", "未知", "不明", "알 수 없음")
            .to_owned();
    }
    let seconds = (now - then).max(0);
    match seconds {
        0..60 => {
            pick(locale, "just now", "刚刚", "たった今", "방금").to_owned()
        }
        60..3_600 => ago_unit(seconds / 60, AgoUnit::Minute, locale),
        3_600..86_400 => ago_unit(seconds / 3_600, AgoUnit::Hour, locale),
        86_400..5_184_000 => ago_unit(seconds / 86_400, AgoUnit::Day, locale),
        5_184_000..63_072_000 => {
            ago_unit(seconds / 2_592_000, AgoUnit::Month, locale)
        }
        _ => ago_unit(seconds / 31_536_000, AgoUnit::Year, locale),
    }
}

#[derive(Clone, Copy)]
enum AgoUnit {
    Minute,
    Hour,
    Day,
    Month,
    Year,
}

fn ago_unit(count: i64, unit: AgoUnit, locale: Locale) -> String {
    match locale {
        Locale::En => {
            let name = match unit {
                AgoUnit::Minute => "minute",
                AgoUnit::Hour => "hour",
                AgoUnit::Day => "day",
                AgoUnit::Month => "month",
                AgoUnit::Year => "year",
            };
            if count == 1 {
                format!("1 {name} ago")
            } else {
                format!("{count} {name}s ago")
            }
        }
        Locale::Zh => {
            let name = match unit {
                AgoUnit::Minute => "分钟前",
                AgoUnit::Hour => "小时前",
                AgoUnit::Day => "天前",
                AgoUnit::Month => "个月前",
                AgoUnit::Year => "年前",
            };
            format!("{count} {name}")
        }
        Locale::Ja => {
            let name = match unit {
                AgoUnit::Minute => "分前",
                AgoUnit::Hour => "時間前",
                AgoUnit::Day => "日前",
                AgoUnit::Month => "か月前",
                AgoUnit::Year => "年前",
            };
            format!("{count} {name}")
        }
        Locale::Ko => {
            let name = match unit {
                AgoUnit::Minute => "분 전",
                AgoUnit::Hour => "시간 전",
                AgoUnit::Day => "일 전",
                AgoUnit::Month => "개월 전",
                AgoUnit::Year => "년 전",
            };
            format!("{count}{name}")
        }
    }
}
