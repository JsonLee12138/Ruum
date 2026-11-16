//! Localized side-panel copy for the Explore screen.

use ruum_core::size::human_count;

use crate::i18n::{Locale, pick};

pub const fn selection_eyebrow(locale: Locale) -> &'static str {
    pick(locale, "Selection", "当前选中", "選択中", "선택 항목")
}

pub const fn selection_empty(locale: Locale) -> &'static str {
    pick(
        locale,
        "Point at a tile or select one with the arrows",
        "将指针移到区块上，或使用方向键选择",
        "タイルにカーソルを合わせるか矢印キーで選択",
        "타일을 가리키거나 방향키로 선택하세요",
    )
}

pub const fn unit_files(locale: Locale) -> &'static str {
    pick(locale, "files", "个文件", "ファイル", "개 파일")
}

pub const fn git_eyebrow(locale: Locale) -> &'static str {
    let _ = locale;
    "Git"
}

pub const fn git_not_readable(locale: Locale) -> &'static str {
    pick(
        locale,
        "not readable",
        "不可读",
        "読み取り不可",
        "읽을 수 없음",
    )
}

pub const fn git_asking(locale: Locale) -> &'static str {
    pick(
        locale,
        "asking\u{2026}",
        "查询中\u{2026}",
        "確認中\u{2026}",
        "확인 중\u{2026}",
    )
}

pub const fn kind_eyebrow(locale: Locale) -> &'static str {
    pick(locale, "Kind", "类型", "種類", "종류")
}

pub const fn of_scan_eyebrow(locale: Locale) -> &'static str {
    pick(locale, "Of scan", "占比", "割合", "비율")
}

pub const fn files_eyebrow(locale: Locale) -> &'static str {
    pick(locale, "Files", "文件数", "ファイル数", "파일 수")
}

pub const fn last_write_eyebrow(locale: Locale) -> &'static str {
    pick(locale, "Last write", "最近修改", "最終更新", "마지막 수정")
}

pub const fn badge_marked(locale: Locale) -> &'static str {
    pick(locale, "Marked", "已标记", "マーク済み", "표시됨")
}

pub fn badge_goes_with(ancestor: &str, locale: Locale) -> String {
    match locale {
        Locale::En => format!("Goes with {ancestor}"),
        Locale::Zh => format!("随 {ancestor} 一同清理"),
        Locale::Ja => format!("{ancestor} と一緒に削除"),
        Locale::Ko => format!("{ancestor}와(과) 함께 삭제"),
    }
}

pub const fn badge_partly_unreadable(locale: Locale) -> &'static str {
    pick(
        locale,
        "Partly unreadable",
        "部分不可读",
        "一部読み取り不可",
        "일부 읽기 불가",
    )
}

pub const fn action_open(locale: Locale) -> &'static str {
    pick(locale, "Open", "打开", "開く", "열기")
}

pub fn action_unmark_ancestor(name: &str, locale: Locale) -> String {
    match locale {
        Locale::En => format!("Unmark {name}"),
        Locale::Zh => format!("取消标记 {name}"),
        Locale::Ja => format!("{name} のマークを解除"),
        Locale::Ko => format!("{name} 표시 해제"),
    }
}

pub const fn action_unmark(locale: Locale) -> &'static str {
    pick(locale, "Unmark", "取消标记", "マーク解除", "표시 해제")
}

pub const fn action_mark_for_removal(locale: Locale) -> &'static str {
    pick(
        locale,
        "Mark for removal",
        "标记为待清理",
        "削除対象にマーク",
        "삭제 대상으로 표시",
    )
}

pub const fn worth_eyebrow(locale: Locale) -> &'static str {
    pick(
        locale,
        "Worth a look",
        "值得关注",
        "要チェック",
        "확인 권장",
    )
}

pub const fn worth_empty_done(locale: Locale) -> &'static str {
    pick(
        locale,
        "Nothing obviously disposable",
        "未发现明显可清理项",
        "明らかに不要な項目はありません",
        "명확히 정리할 항목이 없습니다",
    )
}

pub const fn worth_empty_waiting(locale: Locale) -> &'static str {
    pick(
        locale,
        "Waiting for the scan",
        "等待扫描完成",
        "スキャンを待機中",
        "스캔 대기 중",
    )
}

pub fn insight_worktrees(
    count: usize,
    oldest_days: u64,
    locale: Locale,
) -> String {
    match locale {
        Locale::En => {
            let s = if count == 1 { "" } else { "s" };
            format!("{count} worktree{s} \u{00b7} oldest {oldest_days} d")
        }
        Locale::Zh => {
            format!("{count} 个工作树 \u{00b7} 最旧 {oldest_days} 天")
        }
        Locale::Ja => {
            format!("{count} 件のワークツリー \u{00b7} 最古 {oldest_days} 日")
        }
        Locale::Ko => {
            format!("워크트리 {count}개 \u{00b7} 최장 {oldest_days}일")
        }
    }
}

pub fn insight_stale_title(
    tail: &str,
    stale_days: u64,
    locale: Locale,
) -> String {
    match locale {
        Locale::En => format!("{tail} > {stale_days} days"),
        Locale::Zh => format!("{tail} > {stale_days} 天"),
        Locale::Ja => format!("{tail} > {stale_days} 日"),
        Locale::Ko => format!("{tail} > {stale_days}일"),
    }
}

pub fn insight_stale_detail(count: usize, locale: Locale) -> String {
    match locale {
        Locale::En => {
            let s = if count == 1 { "" } else { "s" };
            format!("{count} experiment{s} untouched")
        }
        Locale::Zh => format!("{count} 个实验目录长期未动"),
        Locale::Ja => format!("{count} 件の実験ディレクトリが未更新"),
        Locale::Ko => format!("실험 디렉터리 {count}개 미사용"),
    }
}

pub fn marked_header(count: usize, locale: Locale) -> String {
    if count == 0 {
        return pick(locale, "Marked", "已标记", "マーク済み", "표시됨")
            .to_owned();
    }
    match locale {
        Locale::En => format!("Marked \u{00b7} {count}"),
        Locale::Zh => format!("已标记 \u{00b7} {count}"),
        Locale::Ja => format!("マーク済み \u{00b7} {count}"),
        Locale::Ko => format!("표시됨 \u{00b7} {count}"),
    }
}

pub const fn marked_empty(locale: Locale) -> &'static str {
    pick(
        locale,
        "Space marks the tile you point at",
        "按 Space 标记当前指向的区块",
        "Space キーで指しているタイルをマーク",
        "Space 키로 가리킨 타일을 표시하세요",
    )
}

pub fn marked_more_on_review(extra: usize, locale: Locale) -> String {
    match locale {
        Locale::En => format!("+{extra} more on the review screen"),
        Locale::Zh => format!("还有 {extra} 项，请在审核页面查看"),
        Locale::Ja => format!("ほか {extra} 件は確認画面に表示"),
        Locale::Ko => format!("검토 화면에 {extra}개 더 있음"),
    }
}

pub fn marked_nested_blocked(
    covered: usize,
    blocked: usize,
    locale: Locale,
) -> String {
    match locale {
        Locale::En => format!("{covered} nested \u{00b7} {blocked} kept back"),
        Locale::Zh => {
            format!("{covered} 项被包含 \u{00b7} {blocked} 项受保护保留")
        }
        Locale::Ja => format!("{covered} 件内包 \u{00b7} {blocked} 件保護"),
        Locale::Ko => format!("{covered}개 포함 \u{00b7} {blocked}개 보호됨"),
    }
}

pub fn privacy_message(errors: u64, locale: Locale) -> String {
    let count = human_count(errors);
    match locale {
        Locale::En => {
            let noun = if errors == 1 { "item" } else { "items" };
            format!(
                "macOS kept {count} {noun} unreadable. Give ruum Full Disk \
                 Access, or your terminal if you started it there, then \
                 reopen it."
            )
        }
        Locale::Zh => format!(
            "macOS 限制了 {count} 个项目不可读。请为 ruum（若从终端启动则为\
             终端）授予“完全磁盘访问权限”，然后重新打开。"
        ),
        Locale::Ja => format!(
            "macOS により {count} 件が読み取り不可でした。ruum（ターミナル\
             から起動した場合はターミナル）にフルディスクアクセスを許可して\
             から再起動してください。"
        ),
        Locale::Ko => format!(
            "macOS에서 {count}개 항목을 읽을 수 없습니다. ruum(또는 실행한 \
             터미널)에 전체 디스크 접근 권한을 부여한 뒤 다시 여세요."
        ),
    }
}

pub const fn privacy_button(locale: Locale) -> &'static str {
    pick(
        locale,
        "Open Privacy Settings",
        "打开隐私设置",
        "プライバシー設定を開く",
        "개인정보 보호 설정 열기",
    )
}

pub fn admin_unreadable(
    errors: u64,
    follow_links: bool,
    locale: Locale,
) -> String {
    let count = human_count(errors);
    match locale {
        Locale::En => {
            let noun = if errors == 1 { "item" } else { "items" };
            let faster = if follow_links {
                ""
            } else {
                ", and a whole drive several times faster"
            };
            format!(
                "Windows kept {count} {noun} unreadable. Run as administrator \
                 to read them{faster}."
            )
        }
        Locale::Zh => {
            let faster = if follow_links {
                ""
            } else {
                "，并使整盘扫描速度提升数倍"
            };
            format!(
                "Windows 限制了 {count} 个项目不可读。以管理员身份运行可读取\
                 它们{faster}。"
            )
        }
        Locale::Ja => {
            let faster = if follow_links {
                ""
            } else {
                "、ドライブ全体のスキャンも数倍高速になります"
            };
            format!(
                "Windows により {count} 件が読み取り不可でした。管理者として\
                 実行するとこれらを読み取れます{faster}。"
            )
        }
        Locale::Ko => {
            let faster = if follow_links {
                ""
            } else {
                ", 전체 드라이브도 몇 배 빠르게 스캔합니다"
            };
            format!(
                "Windows에서 {count}개 항목을 읽을 수 없습니다. 관리자 권한으로 \
                 실행하면 읽을 수 있습니다{faster}."
            )
        }
    }
}

pub const fn admin_file_table(locale: Locale) -> &'static str {
    pick(
        locale,
        "Run as administrator to read the whole drive from its file table: \
         several times faster than this walk.",
        "以管理员身份运行可直接从文件表读取整块磁盘，比当前遍历快数倍。",
        "管理者として実行するとファイルテーブルからドライブ全体を読み取れ、\
         通常の走査より数倍高速になります。",
        "관리자 권한으로 실행하면 파일 테이블에서 전체 드라이브를 읽어 훨씬 \
         빠르게 스캔합니다.",
    )
}

pub fn admin_marks_warning(message: &str, locale: Locale) -> String {
    match locale {
        Locale::En => format!("{message} Restarting drops the marks."),
        Locale::Zh => format!("{message} 重启将清空当前标记。"),
        Locale::Ja => format!("{message} 再起動するとマークはクリアされます。"),
        Locale::Ko => format!("{message} 다시 시작하면 표시가 초기화됩니다."),
    }
}

pub const fn admin_button(locale: Locale) -> &'static str {
    pick(
        locale,
        "Restart as Administrator",
        "以管理员身份重启",
        "管理者として再起動",
        "관리자 권한으로 다시 시작",
    )
}

pub const fn disk_eyebrow(locale: Locale) -> &'static str {
    pick(locale, "Disk", "磁盘", "ディスク", "디스크")
}

pub const fn source_button(locale: Locale) -> &'static str {
    pick(
        locale,
        "Source\u{2026}",
        "来源\u{2026}",
        "ソース\u{2026}",
        "소스\u{2026}",
    )
}

pub const fn free_space_unavailable(locale: Locale) -> &'static str {
    pick(
        locale,
        "Free space is not available here",
        "此处无法获取可用空间",
        "ここでは空き容量を取得できません",
        "여기서는 여유 공간을 확인할 수 없습니다",
    )
}

pub fn unit_free(unit: &str, locale: Locale) -> String {
    match locale {
        Locale::En => format!("{unit} free"),
        Locale::Zh => format!("{unit} 可用"),
        Locale::Ja => format!("{unit} 空き"),
        Locale::Ko => format!("{unit} 여유"),
    }
}

pub fn arrow_free(available: &str, locale: Locale) -> String {
    match locale {
        Locale::En => format!("\u{2192} {available} free"),
        Locale::Zh => format!("\u{2192} {available} 可用"),
        Locale::Ja => format!("\u{2192} 空き {available}"),
        Locale::Ko => format!("\u{2192} {available} 여유"),
    }
}

pub fn not_counted_here(left_out: &str, locale: Locale) -> String {
    match locale {
        Locale::En => format!("not counted here: {left_out}"),
        Locale::Zh => format!("未计入此卷：{left_out}"),
        Locale::Ja => format!("このボリュームには含まれません: {left_out}"),
        Locale::Ko => format!("여기에 포함되지 않음: {left_out}"),
    }
}

pub fn other_volumes(bytes: &str, locale: Locale) -> String {
    match locale {
        Locale::En => format!("{bytes} on other volumes"),
        Locale::Zh => format!("{bytes} 位于其他卷"),
        Locale::Ja => format!("{bytes} は別ボリューム"),
        Locale::Ko => format!("다른 볼륨 {bytes}"),
    }
}

pub fn unattributed_bytes(bytes: &str, locale: Locale) -> String {
    match locale {
        Locale::En => format!("{bytes} could not be placed"),
        Locale::Zh => format!("{bytes} 无法归属卷"),
        Locale::Ja => format!("{bytes} はボリューム特定不可"),
        Locale::Ko => format!("{bytes} 위치 확인 불가"),
    }
}

pub fn used_amount(bytes: &str, locale: Locale) -> String {
    match locale {
        Locale::En => format!("{bytes} used"),
        Locale::Zh => format!("已用 {bytes}"),
        Locale::Ja => format!("使用済み {bytes}"),
        Locale::Ko => format!("{bytes} 사용"),
    }
}

pub fn total_amount(bytes: &str, locale: Locale) -> String {
    match locale {
        Locale::En => format!("{bytes} total"),
        Locale::Zh => format!("共 {bytes}"),
        Locale::Ja => format!("合計 {bytes}"),
        Locale::Ko => format!("전체 {bytes}"),
    }
}

pub fn review_marked_button(
    count: usize,
    reclaiming: &str,
    locale: Locale,
) -> String {
    match locale {
        Locale::En => {
            format!("Review {count} marked \u{00b7} frees {reclaiming}\u{2026}")
        }
        Locale::Zh => {
            format!("审核 {count} 项标记 \u{00b7} 释放 {reclaiming}\u{2026}")
        }
        Locale::Ja => format!(
            "{count} 件のマークを確認 \u{00b7} {reclaiming} 解放\u{2026}"
        ),
        Locale::Ko => {
            format!("표시된 {count}개 검토 \u{00b7} {reclaiming} 확보\u{2026}")
        }
    }
}
