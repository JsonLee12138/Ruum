//! Localized keyboard and mouse help overlay copy.

use crate::i18n::{Locale, pick};

const MODIFIER_CLICK: &str = if cfg!(target_os = "macos") {
    "\u{2318}-click"
} else {
    "ctrl-click"
};

const MODIFIER_ZOOM: &str = if cfg!(target_os = "macos") {
    "\u{2318} = / - / 0"
} else {
    "ctrl = / - / 0"
};

const MODIFIER_OPEN: &str = if cfg!(target_os = "macos") {
    "\u{2318}O"
} else {
    "ctrl-o"
};

pub const fn help_title(locale: Locale) -> &'static str {
    pick(
        locale,
        "Keyboard and mouse",
        "键盘与鼠标",
        "キーボードとマウス",
        "키보드 및 마우스",
    )
}

pub fn help_footer(root: &str, metric: &str, locale: Locale) -> String {
    match locale {
        Locale::En => {
            format!("? or esc closes \u{00b7} {root} \u{00b7} {metric}")
        }
        Locale::Zh => {
            format!("? 或 esc 关闭 \u{00b7} {root} \u{00b7} {metric}")
        }
        Locale::Ja => {
            format!("? または esc で閉じる \u{00b7} {root} \u{00b7} {metric}")
        }
        Locale::Ko => {
            format!("? 또는 esc로 닫기 \u{00b7} {root} \u{00b7} {metric}")
        }
    }
}

pub const fn help_rows(locale: Locale) -> [(&'static str, &'static str); 28] {
    let reveal_label = if cfg!(target_os = "macos") {
        pick(
            locale,
            "Show it in Finder",
            "在访达中显示",
            "Finder で表示",
            "Finder에서 보기",
        )
    } else if cfg!(windows) {
        pick(
            locale,
            "Show it in File Explorer",
            "在文件资源管理器中显示",
            "エクスプローラーで表示",
            "파일 탐색기에서 보기",
        )
    } else {
        pick(
            locale,
            "Show it in the file manager",
            "在文件管理器中显示",
            "ファイルマネージャーで表示",
            "파일 관리자에서 보기",
        )
    };

    [
        (
            "space / x",
            pick(
                locale,
                "Mark or unmark the tile you point at",
                "标记或取消标记当前指向的区块",
                "指しているタイルをマーク／解除",
                "가리킨 타일을 표시하거나 해제",
            ),
        ),
        (
            MODIFIER_CLICK,
            pick(
                locale,
                "Mark without moving the selection",
                "直接标记而不移动当前选中项",
                "選択を移動せずにマーク",
                "선택을 옮기지 않고 표시",
            ),
        ),
        (
            "enter",
            pick(
                locale,
                "Open that directory, at any depth",
                "进入该目录（支持任意层级）",
                "任意の階層のディレクトリを開く",
                "어느 깊이에서나 해당 디렉터리 열기",
            ),
        ),
        (
            "\u{232b} / esc",
            pick(
                locale,
                "Go up one directory",
                "返回上一级目录",
                "1 つ上のディレクトリへ戻る",
                "상위 디렉터리로 이동",
            ),
        ),
        (
            "alt \u{2190} / \u{2192}",
            pick(
                locale,
                "Back or forward, as do the mouse's side buttons",
                "后退或前进（也可用鼠标侧键）",
                "戻る／進む（マウスのサイドボタンでも可）",
                "뒤로 또는 앞으로(마우스 측면 버튼 포함)",
            ),
        ),
        (
            "\u{2190} \u{2191} \u{2193} \u{2192}",
            pick(
                locale,
                "Move between tiles at this level",
                "在当前层级的区块间移动选择",
                "この階層のタイル間を移動",
                "현재 수준의 타일 간 이동",
            ),
        ),
        (
            "tab",
            pick(
                locale,
                "Next largest sibling",
                "下一个较大的同级项目",
                "次に大きい兄弟項目へ移動",
                "다음으로 큰 형제 항목",
            ),
        ),
        (
            "scroll",
            pick(
                locale,
                "Zoom toward a directory, then go into it",
                "向目录缩放并进入其中",
                "ディレクトリに向けて拡大し、中へ移動",
                "디렉터리 방향으로 확대 후 진입",
            ),
        ),
        (
            "shift-scroll",
            pick(
                locale,
                "Pan the magnified view",
                "平移已放大的视图",
                "拡大表示をパン移動",
                "확대된 보기 이동",
            ),
        ),
        (
            "[ / ]",
            pick(
                locale,
                "Draw fewer or more levels at once",
                "减少或增加一次绘制的层级数",
                "一度に表示する階層数を増減",
                "한 번에 표시할 깊이 줄이기/늘리기",
            ),
        ),
        (
            "- / = / 0",
            pick(
                locale,
                "Magnify, shrink, or reset the view",
                "放大、缩小或重置视图",
                "表示を拡大・縮小・リセット",
                "보기 확대, 축소 또는 초기화",
            ),
        ),
        (
            MODIFIER_ZOOM,
            pick(
                locale,
                "Interface zoom",
                "界面缩放",
                "インターフェース拡大率",
                "인터페이스 확대/축소",
            ),
        ),
        (
            "/",
            pick(
                locale,
                "Filter by name: only matches keep their colour; enter shows \
                 only them",
                "按名称过滤：仅匹配项保留色彩；按 enter 仅显示匹配项",
                "名前で絞り込み: 一致項目のみ色を維持、enter で一致のみ表示",
                "이름으로 필터: 일치 항목만 색상 유지, enter로 일치만 표시",
            ),
        ),
        (
            "c",
            pick(
                locale,
                "Review the marked list",
                "审核已标记列表",
                "マーク済みリストを確認",
                "표시된 목록 검토",
            ),
        ),
        (
            "t",
            pick(
                locale,
                "Size, files or age: what areas and colours say",
                "切换大小、文件数或修改时间视图",
                "サイズ・ファイル数・更新日時を切り替え",
                "크기, 파일 수 또는 날짜 기준 전환",
            ),
        ),
        (
            "r",
            pick(
                locale,
                "Scan again from the same root",
                "从当前根目录重新扫描",
                "同じルートから再スキャン",
                "같은 루트에서 다시 스캔",
            ),
        ),
        (
            "esc",
            pick(
                locale,
                "Stop a scan in progress",
                "停止正在进行的扫描",
                "実行中のスキャンを停止",
                "진행 중인 스캔 중지",
            ),
        ),
        (
            "v",
            pick(
                locale,
                "Scan another volume",
                "扫描其他卷或来源",
                "別のボリュームをスキャン",
                "다른 볼륨 스캔",
            ),
        ),
        (
            MODIFIER_OPEN,
            pick(
                locale,
                "Choose another directory to scan",
                "选择其他目录进行扫描",
                "別のディレクトリを選択してスキャン",
                "스캔할 다른 디렉터리 선택",
            ),
        ),
        (
            "g",
            pick(
                locale,
                "The whole disk; click any directory above to widen",
                "扫描整块磁盘；点击上方任意路径可扩大范围",
                "ディスク全体を表示（上のパンくずをクリックで拡張）",
                "전체 디스크 보기(위 경로를 클릭해 범위 확장)",
            ),
        ),
        (
            "d",
            pick(
                locale,
                "Disk usage or apparent size",
                "切换磁盘占用或表观大小",
                "ディスク使用量／見かけのサイズを切り替え",
                "디스크 사용량 또는 겉보기 크기 전환",
            ),
        ),
        (
            "i",
            pick(
                locale,
                "Include or skip hidden entries",
                "包含或跳过隐藏项目",
                "隠し項目を含める／除外する",
                "숨김 항목 포함 또는 건너뛰기",
            ),
        ),
        (
            "p",
            pick(
                locale,
                "Show or hide the selection line",
                "显示或隐藏侧边详情栏",
                "選択詳細パネルを表示／非表示",
                "선택 정보 패널 표시/숨기기",
            ),
        ),
        ("o", reveal_label),
        ("q", pick(locale, "Quit", "退出", "終了", "종료")),
        (
            pick(locale, "Review screen", "审核页面", "確認画面", "검토 화면"),
            pick(
                locale,
                "m trash \u{00b7} p permanent \u{00b7} ! unmark all \u{00b7} \
                 s save list \u{00b7} a copy as prompt",
                "m 移至废纸篓 \u{00b7} p 永久删除 \u{00b7} ! 取消全部 \u{00b7} \
                 s 保存列表 \u{00b7} a 复制为提示词",
                "m ゴミ箱 \u{00b7} p 完全削除 \u{00b7} ! 全解除 \u{00b7} \
                 s リスト保存 \u{00b7} a プロンプトコピー",
                "m 휴지통 \u{00b7} p 영구 삭제 \u{00b7} ! 모두 해제 \u{00b7} \
                 s 목록 저장 \u{00b7} a 프롬프트 복사",
            ),
        ),
        (
            "",
            pick(
                locale,
                "enter commits \u{00b7} esc goes back",
                "enter 执行清理 \u{00b7} esc 返回",
                "enter で実行 \u{00b7} esc で戻る",
                "enter 실행 \u{00b7} esc 뒤로",
            ),
        ),
        (
            "",
            pick(
                locale,
                "A permanent deletion always asks first",
                "永久删除前始终会弹出确认窗口",
                "完全削除の前には必ず確認が表示されます",
                "영구 삭제 전에는 항상 먼저 확인합니다",
            ),
        ),
    ]
}
