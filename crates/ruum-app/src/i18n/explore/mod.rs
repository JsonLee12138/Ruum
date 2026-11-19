//! Localized copy for the Explore screen, toolbar, scanning panel, find bar,
//! tooltips, and help overlay.

pub mod help;
pub mod panel;
pub mod scanning;

use ruum_core::size::human_count;

use super::{Locale, dir_count, entry_count, file_count, pick};

// ── idle & top bar ──────────────────────────────────────────────────────

pub const fn idle_title(locale: Locale) -> &'static str {
    pick(
        locale,
        "Choose a source. Then choose a root.",
        "先选择来源，再选择扫描根目录。",
        "ソースを選択し、次にルートを選択してください。",
        "소스를 선택한 다음 루트를 선택하세요.",
    )
}

pub const fn idle_subtitle(locale: Locale) -> &'static str {
    pick(
        locale,
        "Local or SSH \u{00b7} one mount at a time \u{00b7} no background scan",
        "本地或 SSH \u{00b7} 每次仅限单个挂载点 \u{00b7} 不进行后台扫描",
        "ローカルまたは SSH \u{00b7} 一度に 1 マウント \u{00b7} \
         バックグラウンドスキャンなし",
        "로컬 또는 SSH \u{00b7} 한 번에 마운트 하나 \u{00b7} 백그라운드 \
         스캔 없음",
    )
}

pub const fn choose_source_button(locale: Locale) -> &'static str {
    pick(
        locale,
        "Choose source\u{2026}",
        "选择来源\u{2026}",
        "ソースを選択\u{2026}",
        "소스 선택\u{2026}",
    )
}

pub const fn logo_tooltip(locale: Locale) -> &'static str {
    pick(
        locale,
        "Choose local or SSH source \u{00b7} v",
        "选择本地或 SSH 来源 \u{00b7} v",
        "ローカルまたは SSH ソースを選択 \u{00b7} v",
        "로컬 또는 SSH 소스 선택 \u{00b7} v",
    )
}

pub const fn host_tab_local(locale: Locale) -> &'static str {
    pick(locale, "Local", "本机", "ローカル", "로컬")
}

pub const fn close_host_tab_tooltip(locale: Locale) -> &'static str {
    pick(
        locale,
        "Close remote host tab",
        "关闭远程主机标签页",
        "リモートホストタブを閉じる",
        "원격 호스트 탭 닫기",
    )
}

pub const fn crumb_widen_tooltip(locale: Locale) -> &'static str {
    pick(
        locale,
        "Scan from here \u{00b7} what is below is reused",
        "从此处开始扫描 \u{00b7} 下层已扫描结果将复用",
        "ここからスキャン \u{00b7} 配下の結果は再利用されます",
        "여기부터 스캔 \u{00b7} 하위 결과는 재사용됩니다",
    )
}

pub const fn crumb_switch_root_tooltip(locale: Locale) -> &'static str {
    pick(
        locale,
        "Select scan directory or mount point \u{00b7} v",
        "选择扫描目录或挂载点 \u{00b7} v",
        "スキャンディレクトリまたはマウントポイントを選択 \u{00b7} v",
        "스캔 디렉터리 또는 마운트 지점 선택 \u{00b7} v",
    )
}

pub const fn switch_directory_tooltip(locale: Locale) -> &'static str {
    pick(
        locale,
        "Switch scan directory or mount \u{00b7} v",
        "切换扫描目录或挂载点 \u{00b7} v",
        "スキャンディレクトリ / マウントを切り替え \u{00b7} v",
        "스캔 디렉터리 / 마운트 전환 \u{00b7} v",
    )
}

pub const fn remote_docker_tooltip(locale: Locale) -> &'static str {
    pick(
        locale,
        "Inspect and clean remote Docker",
        "查看并清理远程 Docker",
        "リモート Docker の確認と整理",
        "원격 Docker 확인 및 정리",
    )
}

pub fn siblings_in(parent_name: &str, locale: Locale) -> String {
    match locale {
        Locale::En => format!("Siblings in {parent_name}"),
        Locale::Zh => format!("{parent_name} 中的同级项"),
        Locale::Ja => format!("{parent_name} 内の兄弟項目"),
        Locale::Ko => format!("{parent_name}의 형제 항목"),
    }
}

pub fn more_smaller(count: usize, locale: Locale) -> String {
    match locale {
        Locale::En => format!("+{count} smaller"),
        Locale::Zh => format!("+{count} 个更小项目"),
        Locale::Ja => format!("+{count} 件のより小さい項目"),
        Locale::Ko => format!("더 작은 항목 +{count}개"),
    }
}

// ── view settings & power menu ──────────────────────────────────────────

pub const fn mode_size(locale: Locale) -> &'static str {
    pick(locale, "Size", "大小", "サイズ", "크기")
}

pub const fn mode_files(locale: Locale) -> &'static str {
    pick(locale, "Files", "文件数", "ファイル数", "파일 수")
}

pub const fn mode_age(locale: Locale) -> &'static str {
    pick(locale, "Age", "时间", "更新日時", "날짜")
}

pub const fn hidden_files(locale: Locale) -> &'static str {
    pick(
        locale,
        "Hidden files",
        "隐藏文件",
        "隠しファイル",
        "숨김 파일",
    )
}

pub const fn apparent_size(locale: Locale) -> &'static str {
    pick(
        locale,
        "Apparent size",
        "表观大小",
        "見かけのサイズ",
        "겉보기 크기",
    )
}

pub fn depth_label(depth: u32, locale: Locale) -> String {
    match locale {
        Locale::En => format!("Depth {depth}"),
        Locale::Zh => format!("深度 {depth}"),
        Locale::Ja => format!("階層 {depth}"),
        Locale::Ko => format!("깊이 {depth}"),
    }
}

pub const fn depth_tooltip(locale: Locale) -> &'static str {
    pick(
        locale,
        "Levels drawn at once \u{00b7} [ and ]",
        "一次绘制的层级数 \u{00b7} [ 和 ]",
        "一度に表示する階層数 \u{00b7} [ と ]",
        "한 번에 표시할 깊이 \u{00b7} [ 및 ]",
    )
}

pub fn power_accessibility_label(name: &str, locale: Locale) -> String {
    match locale {
        Locale::En => format!("Power Efficiency: {name}"),
        Locale::Zh => format!("电源效率：{name}"),
        Locale::Ja => format!("電源効率: {name}"),
        Locale::Ko => format!("전원 효율: {name}"),
    }
}

const fn power_tradeoff(is_macos: bool, locale: Locale) -> &'static str {
    if is_macos {
        pick(
            locale,
            "Up to about 8 workers a scan finishes sooner: a whole drive took \
             a third less time with 8 than with Balanced. Past that it is no \
             faster, but uses over twice the CPU and runs the fans at full \
             speed.",
            "约 8 个工作线程以内扫描会更快：整盘扫描用 8 线程比“均衡”快约三分\
             之一。超过后速度不再提升，CPU 占用却翻倍以上，风扇也会全速运转。",
            "約 8 ワーカーまではスキャンが高速化します（ドライブ全体で\
             「バランス」より約 3 分の 1 短縮）。それ以上増やしても速くならず\
             、CPU 使用率が 2 倍以上になりファンが全開になります。",
            "약 8개 작업자까지는 스캔이 더 빨라집니다(전체 드라이브 기준 \
             '균형'보다 약 1/3 단축). 그 이상은 더 빨라지지 않고 CPU를 두 배 \
             이상 사용하며 팬이 최대 속도로 돕니다.",
        )
    } else {
        pick(
            locale,
            "More workers finish a scan sooner only up to a point. Past it \
             they use more CPU for no gain, and can run the fans at full \
             speed.",
            "工作线程数仅在一定范围内能加快扫描。超过后只会消耗更多 CPU 而无\
             收益，并可能导致风扇全速运转。",
            "ワーカー数を増やして高速化できるのは一定数までです。それを超える\
             と効果がなく CPU 負荷だけが増え、ファンが全開になる場合があります。",
            "작업자 수를 늘려도 일정 수준까지만 스캔이 빨라집니다. 그 이상은 \
             이득 없이 CPU만 더 사용하고 팬이 최대 속도로 돌 수 있습니다.",
        )
    }
}

pub fn power_tooltip(
    name: &str,
    workers: &str,
    cpus: usize,
    is_macos: bool,
    locale: Locale,
) -> String {
    let tradeoff = power_tradeoff(is_macos, locale);
    match locale {
        Locale::En => format!(
            "Power Efficiency: {name} \u{00b7} {workers} of {cpus} CPUs\n\
             {tradeoff}\nUsed from the next scan, and remembered."
        ),
        Locale::Zh => format!(
            "电源效率：{name} \u{00b7} {workers} / {cpus} 核 CPU\n\
             {tradeoff}\n自下次扫描生效并自动记住。"
        ),
        Locale::Ja => format!(
            "電源効率: {name} \u{00b7} {workers} / {cpus} CPU\n\
             {tradeoff}\n次回のスキャンから適用され、設定は保存されます。"
        ),
        Locale::Ko => format!(
            "전원 효율: {name} \u{00b7} {workers} / {cpus} CPU\n\
             {tradeoff}\n다음 스캔부터 적용되며 기억됩니다."
        ),
    }
}

pub const fn scan_workers_header(locale: Locale) -> &'static str {
    pick(
        locale,
        "Scan workers",
        "扫描工作线程",
        "スキャンワーカー",
        "스캔 작업자",
    )
}

pub fn cpus_count(cpus: usize, locale: Locale) -> String {
    match locale {
        Locale::En => format!("{cpus} CPUs"),
        Locale::Zh => format!("{cpus} 核 CPU"),
        Locale::Ja => format!("{cpus} CPU"),
        Locale::Ko => format!("CPU {cpus}개"),
    }
}

pub fn same_as_preset(below: &str, cpus: usize, locale: Locale) -> String {
    match locale {
        Locale::En => format!("Same as {below} with {cpus} CPUs"),
        Locale::Zh => format!("在 {cpus} 核 CPU 上与“{below}”相同"),
        Locale::Ja => format!("{cpus} CPU 環境では「{below}」と同じ"),
        Locale::Ko => format!("{cpus} CPU 환경에서는 {below}와(과) 동일"),
    }
}

// ── scan totals & legend ────────────────────────────────────────────────

pub fn scan_totals_counts(files: u64, dirs: u64, locale: Locale) -> String {
    let f = file_count(files, locale);
    let d = dir_count(dirs, locale);
    format!("\u{00b7} {f} \u{00b7} {d}")
}

pub fn scan_totals_unreadable(errors: u64, locale: Locale) -> String {
    let count = human_count(errors);
    match locale {
        Locale::En => format!("\u{00b7} {count} unreadable"),
        Locale::Zh => format!("\u{00b7} {count} 项不可读"),
        Locale::Ja => format!("\u{00b7} {count} 件読み取り不可"),
        Locale::Ko => format!("\u{00b7} {count}개 읽기 불가"),
    }
}

pub const fn legend_reclaimable(locale: Locale) -> &'static str {
    pick(locale, "Reclaimable", "可清理", "削除可能", "정리 가능")
}

// ── key bar ─────────────────────────────────────────────────────────────

pub const fn key_hints(locale: Locale) -> [(&'static str, &'static str); 11] {
    [
        ("space", pick(locale, "mark", "标记", "マーク", "표시")),
        ("enter", pick(locale, "open", "打开", "開く", "열기")),
        ("\u{232b}", pick(locale, "up", "上级", "上へ", "상위")),
        ("c", pick(locale, "review", "审核", "確認", "검토")),
        ("hjkl", pick(locale, "move", "移动", "移動", "이동")),
        ("/", pick(locale, "filter", "过滤", "絞り込み", "필터")),
        ("[ ]", pick(locale, "depth", "深度", "階層", "깊이")),
        ("t", pick(locale, "mode", "模式", "モード", "모드")),
        ("0", pick(locale, "reset", "重置", "リセット", "초기화")),
        ("v", pick(locale, "volumes", "来源", "ソース", "볼륨")),
        ("r", pick(locale, "rescan", "重扫", "再スキャン", "재스캔")),
    ]
}

pub const fn all_keys_hint(locale: Locale) -> &'static str {
    pick(locale, "all keys", "全部快捷键", "全キー", "모든 키")
}

pub fn scan_status_scanning(files: u64, bytes: &str, locale: Locale) -> String {
    let entries = entry_count(files, locale);
    match locale {
        Locale::En => format!("scanning \u{00b7} {entries} \u{00b7} {bytes}"),
        Locale::Zh => format!("扫描中 \u{00b7} {entries} \u{00b7} {bytes}"),
        Locale::Ja => {
            format!("スキャン中 \u{00b7} {entries} \u{00b7} {bytes}")
        }
        Locale::Ko => format!("스캔 중 \u{00b7} {entries} \u{00b7} {bytes}"),
    }
}

pub fn scan_status_cancelled(files: u64, locale: Locale) -> String {
    let entries = entry_count(files, locale);
    match locale {
        Locale::En => {
            format!("scan cancelled \u{00b7} {entries} \u{00b7} r scans again")
        }
        Locale::Zh => {
            format!("扫描已取消 \u{00b7} {entries} \u{00b7} 按 r 重新扫描")
        }
        Locale::Ja => {
            format!("スキャン中止 \u{00b7} {entries} \u{00b7} r で再スキャン")
        }
        Locale::Ko => {
            format!("스캔 취소됨 \u{00b7} {entries} \u{00b7} r 다시 스캔")
        }
    }
}

pub fn scan_status_done(
    files: u64,
    elapsed_secs: Option<f32>,
    locale: Locale,
) -> String {
    let entries = entry_count(files, locale);
    let elapsed = elapsed_secs.map_or_else(String::new, |secs| match locale {
        Locale::En | Locale::Ja => format!(" \u{00b7} {secs:.1} s"),
        Locale::Zh => format!(" \u{00b7} {secs:.1} 秒"),
        Locale::Ko => format!(" \u{00b7} {secs:.1}초"),
    });
    match locale {
        Locale::En => format!("scan {entries}{elapsed}"),
        Locale::Zh => format!("已扫描 {entries}{elapsed}"),
        Locale::Ja => format!("スキャン {entries}{elapsed}"),
        Locale::Ko => format!("스캔 {entries}{elapsed}"),
    }
}
