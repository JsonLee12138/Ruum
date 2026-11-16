//! Guard-reason translation and review export/action footer strings.

use crate::i18n::{Locale, item_count, pick};

fn pick_reason(
    reason: &str,
    locale: Locale,
    zh: &str,
    ja: &str,
    ko: &str,
) -> String {
    match locale {
        Locale::En => reason.to_owned(),
        Locale::Zh => zh.to_owned(),
        Locale::Ja => ja.to_owned(),
        Locale::Ko => ko.to_owned(),
    }
}

/// Translate known `ruum_core::removal::Blocked` guard reasons for display
/// without modifying core guard payloads.
#[must_use]
pub fn blocked_reason(reason: &str, locale: Locale) -> String {
    if locale == Locale::En {
        return reason.to_owned();
    }
    match reason {
        "the filesystem root cannot be removed" => pick_reason(
            reason,
            locale,
            "文件系统根目录不可删除",
            "ファイルシステムのルートは削除できません",
            "파일 시스템 루트는 삭제할 수 없습니다",
        ),
        "the scanned root cannot be removed" => pick_reason(
            reason,
            locale,
            "当前扫描根目录不可删除",
            "スキャンルート自体は削除できません",
            "스캔한 루트 디렉터리는 삭제할 수 없습니다",
        ),
        "the home directory cannot be removed" => pick_reason(
            reason,
            locale,
            "主目录不可删除",
            "ホームディレクトリは削除できません",
            "홈 디렉터리는 삭제할 수 없습니다",
        ),
        "it contains the home directory" => pick_reason(
            reason,
            locale,
            "包含用户主目录",
            "ホームディレクトリを含んでいます",
            "홈 디렉터리를 포함하고 있습니다",
        ),
        "a user profile cannot be removed" => pick_reason(
            reason,
            locale,
            "用户配置文件目录不可删除",
            "ユーザープロファイルは削除できません",
            "사용자 프로필은 삭제할 수 없습니다",
        ),
        "outside the scanned root" => pick_reason(
            reason,
            locale,
            "位于扫描根目录之外",
            "スキャンルート外のパスです",
            "스캔 루트 바깥의 경로입니다",
        ),
        "a mount point: removing it would cross onto another filesystem" => {
            pick_reason(
                reason,
                locale,
                "挂载点：删除将跨越到其他文件系统",
                "マウントポイント：削除すると別ファイルシステムに跨がります",
                "마운트 지점: 삭제하면 다른 파일 시스템에 영향을 줍니다",
            )
        }
        _ => translate_dynamic_blocked_reason(reason, locale),
    }
}

fn translate_dynamic_blocked_reason(reason: &str, locale: Locale) -> String {
    if let Some(rest) = reason.strip_prefix("part of the system under ")
        && let Some((system, _)) = rest.split_once(": ")
    {
        return match locale {
            Locale::Zh => {
                format!("系统目录（{system}）：请使用对应包管理工具移除")
            }
            Locale::Ja => format!(
                "システム領域（{system}）：専用ツールで削除してください"
            ),
            Locale::Ko => {
                format!("시스템 경로({system}): 설치한 도구로 제거하세요")
            }
            Locale::En => reason.to_owned(),
        };
    }
    if let Some(rest) = reason.strip_prefix("it contains ")
        && let Some(system) = rest.strip_suffix(", which is part of the system")
    {
        return match locale {
            Locale::Zh => format!("包含系统路径 {system}"),
            Locale::Ja => format!("システム領域 {system} を含んでいます"),
            Locale::Ko => format!("시스템 경로 {system}을(를) 포함합니다"),
            Locale::En => reason.to_owned(),
        };
    }
    if let Some(mount) = reason.strip_suffix(
        " is mounted inside it: removing it would reach into another \
         filesystem",
    ) {
        return match locale {
            Locale::Zh => format!("内部挂载了 {mount}：删除将波及其他文件系统"),
            Locale::Ja => {
                format!("{mount} が内部にマウントされているため削除できません")
            }
            Locale::Ko => {
                format!(
                    "{mount}이(가) 내부에 마운트되어 있어 삭제할 수 없습니다"
                )
            }
            Locale::En => reason.to_owned(),
        };
    }
    if let Some(real) =
        reason.strip_prefix("reached through a link: it is really ")
    {
        return match locale {
            Locale::Zh => format!("通过符号链接指向外部路径：{real}"),
            Locale::Ja => format!("リンク経由のパスです（実体: {real}）"),
            Locale::Ko => {
                format!("링크를 통해 연결된 경로입니다(실제: {real})")
            }
            Locale::En => reason.to_owned(),
        };
    }
    if let Some(rest) = reason.strip_prefix("Windows would read ")
        && let Some(name) =
            rest.strip_suffix(" as a different name, and remove that")
    {
        return match locale {
            Locale::Zh => {
                format!("Windows 会将 {name} 解析为其他名称并误删该目标")
            }
            Locale::Ja => {
                format!(
                    "Windows が {name} を別名として誤削除する恐れがあります"
                )
            }
            Locale::Ko => {
                format!(
                    "Windows가 {name}을(를) 다른 이름으로 잘못 삭제할 수 있습니다"
                )
            }
            Locale::En => reason.to_owned(),
        };
    }
    reason.to_owned()
}

pub const fn save_list(locale: Locale) -> &'static str {
    pick(
        locale,
        "Save list\u{2026}",
        "保存列表\u{2026}",
        "リストを保存\u{2026}",
        "목록 저장\u{2026}",
    )
}

pub const fn copy_prompt(locale: Locale) -> &'static str {
    pick(
        locale,
        "Copy as prompt",
        "复制为提示词",
        "プロンプトとしてコピー",
        "프롬프트로 복사",
    )
}

pub const fn save_script(locale: Locale) -> &'static str {
    pick(
        locale,
        "Save delete script\u{2026}",
        "保存删除脚本\u{2026}",
        "削除スクリプトを保存\u{2026}",
        "삭제 스크립트 저장\u{2026}",
    )
}

pub const fn copy_script(locale: Locale) -> &'static str {
    pick(
        locale,
        "Copy delete script",
        "复制删除脚本",
        "削除スクリプトをコピー",
        "삭제 스크립트 복사",
    )
}

pub const fn back(locale: Locale) -> &'static str {
    pick(locale, "Back", "返回", "戻る", "뒤로")
}

pub fn move_to_trash_button(count: usize, locale: Locale) -> String {
    let items = item_count(count, locale);
    match locale {
        Locale::En => format!("Move {items} to trash"),
        Locale::Zh => format!("将 {items} 移至废纸篓"),
        Locale::Ja => format!("{items} をゴミ箱へ移動"),
        Locale::Ko => format!("{items} 휴지통으로 이동"),
    }
}

pub fn delete_button(count: usize, locale: Locale) -> String {
    let items = item_count(count, locale);
    match locale {
        Locale::En => format!("Delete {items}\u{2026}"),
        Locale::Zh => format!("删除 {items}\u{2026}"),
        Locale::Ja => format!("{items} を削除\u{2026}"),
        Locale::Ko => format!("{items} 삭제\u{2026}"),
    }
}

pub const fn commit_trash_hint(locale: Locale) -> &'static str {
    pick(
        locale,
        "move to trash",
        "移至废纸篓",
        "ゴミ箱へ移動",
        "휴지통으로 이동",
    )
}

pub const fn commit_delete_hint(locale: Locale) -> &'static str {
    pick(
        locale,
        "delete\u{2026}",
        "删除\u{2026}",
        "削除\u{2026}",
        "삭제\u{2026}",
    )
}

pub const fn trash_hint(locale: Locale) -> &'static str {
    pick(locale, "trash", "废纸篓", "ゴミ箱", "휴지통")
}

pub const fn permanent_hint(locale: Locale) -> &'static str {
    pick(locale, "permanent", "永久删除", "完全削除", "영구 삭제")
}

pub const fn save_script_hint(locale: Locale) -> &'static str {
    pick(
        locale,
        "save script",
        "保存脚本",
        "スクリプト保存",
        "스크립트 저장",
    )
}

pub const fn copy_script_hint(locale: Locale) -> &'static str {
    pick(
        locale,
        "copy script",
        "复制脚本",
        "スクリプトコピー",
        "스크립트 복사",
    )
}

pub const fn unmark_all_hint(locale: Locale) -> &'static str {
    pick(locale, "unmark all", "全部取消", "全解除", "모두 해제")
}

pub const fn save_list_hint(locale: Locale) -> &'static str {
    pick(locale, "save list", "保存列表", "リスト保存", "목록 저장")
}

pub const fn copy_prompt_hint(locale: Locale) -> &'static str {
    pick(
        locale,
        "copy as prompt",
        "复制提示词",
        "プロンプトコピー",
        "프롬프트 복사",
    )
}

pub const fn back_hint(locale: Locale) -> &'static str {
    pick(locale, "back", "返回", "戻る", "뒤로")
}

pub fn available_footer(avail: &str, locale: Locale) -> String {
    match locale {
        Locale::En => format!("{avail} available"),
        Locale::Zh => format!("可用 {avail}"),
        Locale::Ja => format!("空き {avail}"),
        Locale::Ko => format!("{avail} 사용 가능"),
    }
}
