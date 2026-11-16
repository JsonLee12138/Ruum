//! Localized CLI help and status messages.

use crate::i18n::Locale;

pub const fn usage(locale: Locale) -> &'static str {
    match locale {
        Locale::En => {
            "\
ruum \u{2014} a treemap of what is using your disk

usage: ruum [OPTIONS] [PATH]
       ruum ssh HOST [OPTIONS] [PATH]
       ruum mcp [--read-only]
       ruum watchdog [--once]
       ruum --confirm-cleanup REQ --response RESP
       ruum extract-scanner ARCH OUTPUT

Linux packaged builds deploy an embedded static scanner over SSH on demand.
Unbundled development builds need ruum-scan on the remote PATH or in
~/.local/bin. ARCH for extraction is x86_64 or aarch64.
SSH uses your config, keys and agent; password prompts use the terminal.
Remote sessions are read-only: mark, export list/prompt/delete script.
Use --check with ssh to deploy and scan without opening a window.

arguments:
  PATH              explicit directory to scan; omitted defaults to ~ (home)

Without PATH, ruum scans the home directory directly, largest first.
Use --choose to select a local mount or SSH source before scanning.
Nested mounts are excluded. Space marks the selected tile, Enter opens it,
c reviews the marked list, ? lists every key.

options:
      --choose          open the source and mount selection dialog before \
scanning
  -a, --apparent-size   measure apparent length instead of allocated blocks
  -l, --follow-links    follow symlinks (local scans only)
  -H, --no-hidden       skip dotfiles and dot-directories
  -D, --disk            scan the whole disk the home directory is on
  -X, --cross-filesystems
                        also measure other disks, network shares and pseudo
                        filesystems mounted below PATH (local only; off by \
default)
  -d, --depth N         how many levels to draw at once (1-6, default 3)
      --power-efficiency PRESET
                        miser, balanced (default), aggressive, drain-my-battery
      --scan-threads N  fixed scan workers, capped by available CPU count
      --adaptive-threads
                        experimental adaptive admission (opt-in)
      --fixed-threads   disable adaptive admission and CPU governor
      --thread-throughput-percent N
                        retain this percent of sampled initial throughput (80)
      --thread-system-cpu-percent N
                        best-effort host CPU budget; 0 disables it (80)
      --metric files    rank by file count instead of bytes
  -h, --help            show this help
"
        }
        Locale::Zh => {
            "\
ruum \u{2014} 直观展示磁盘空间占用的树状图浏览器

用法: ruum [选项] [PATH]
      ruum ssh HOST [选项] [PATH]
      ruum mcp [--read-only]
      ruum watchdog [--once]
      ruum --confirm-cleanup REQ --response RESP
      ruum extract-scanner ARCH OUTPUT

Linux 打包版本会按需通过 SSH 部署内嵌的静态扫描器。
未打包的开发构建需确保远程 PATH 或 ~/.local/bin 中存在 ruum-scan。
extract-scanner 的 ARCH 可选 x86_64 或 aarch64。
SSH 将使用您的配置、密钥与代理；密码提示将在终端中进行。
远程会话为只读模式：可标记项目并导出列表、提示词或删除脚本。
配合 ssh 使用 --check 可在不打开窗口的情况下部署并扫描。

参数:
  PATH              要扫描的目录路径；省略时默认扫描 ~（主目录）

不指定 PATH 时，ruum 将直接按大小从大到小扫描主目录。
使用 --choose 可在扫描前选择本地挂载点或 SSH 来源。
默认排除嵌套挂载点。Space 标记选中区块，Enter 进入目录，
c 查看标记列表，? 查看全部快捷键。

选项:
      --choose          扫描前打开来源与挂载点选择页面
  -a, --apparent-size   统计表观文件长度而非实际占用块大小
  -l, --follow-links    跟随符号链接（仅限本地扫描）
  -H, --no-hidden       跳过以点开头的隐藏文件与目录
  -D, --disk            扫描主目录所在的整块磁盘
  -X, --cross-filesystems
                        同时统计 PATH 下挂载的其他磁盘、网络共享与伪文件系统
                        （仅限本地；默认关闭）
  -d, --depth N         一次绘制的层级深度（1-6，默认 3）
      --power-efficiency PRESET
                        miser、balanced（默认）、aggressive、drain-my-battery
      --scan-threads N  固定扫描工作线程数（受可用 CPU 核数限制）
      --adaptive-threads
                        实验性自适应线程准入（手动启用）
      --fixed-threads   禁用自适应准入与 CPU 限制器
      --thread-throughput-percent N
                        保留初始采样吞吐量的百分比（80）
      --thread-system-cpu-percent N
                        尽力而为的主机 CPU 预算百分比；0 表示禁用（80）
      --metric files    按文件数量而非字节大小排序
  -h, --help            显示此帮助信息
"
        }
        Locale::Ja => {
            "\
ruum \u{2014} ディスク使用量を可視化するツリーマップエクスプローラー

使い方: ruum [オプション] [PATH]
        ruum ssh HOST [オプション] [PATH]
        ruum mcp [--read-only]
        ruum watchdog [--once]
        ruum --confirm-cleanup REQ --response RESP
        ruum extract-scanner ARCH OUTPUT

Linux パッケージ版は必要に応じて SSH 経由で静的スキャナを自動配置します。
非バンドルの開発ビルドでは、リモート PATH または ~/.local/bin に ruum-scan \
が必要です。extract-scanner の ARCH は x86_64 または aarch64 です。
SSH は既存の設定・鍵・エージェントを使用し、パスワード入力は端末で行います。
リモートセッションは読み取り専用です（マーク・リスト／プロンプト／削除スク\
リプトのエクスポートのみ）。--check を指定するとウィンドウを開かずにスキャ\
ンします。

引数:
  PATH              スキャンするディレクトリ（省略時は ~ ホームディレクトリ）

PATH を省略するとホームディレクトリを大きい順にスキャンします。
--choose を指定するとスキャン前にローカルマウントまたは SSH ソースを選択\
できます。ネストしたマウントは除外されます。Space でマーク、Enter で移動、
c でマーク一覧を確認、? で全キーを表示します。

オプション:
      --choose          スキャン前にソースとマウントの選択画面を開く
  -a, --apparent-size   割り当てブロックではなく見かけのサイズを計測
  -l, --follow-links    シンボリックリンクを辿る（ローカルのみ）
  -H, --no-hidden       隠しファイル・ディレクトリをスキップ
  -D, --disk            ホームディレクトリのあるディスク全体をスキャン
  -X, --cross-filesystems
                        PATH 配下の他ディスク・ネットワーク共有も計測\
（ローカルのみ・既定オフ）
  -d, --depth N         一度に描画する階層数（1-6、既定 3）
      --power-efficiency PRESET
                        miser, balanced (既定), aggressive, drain-my-battery
      --scan-threads N  スキャンワーカー数を固定（利用可能 CPU 数が上限）
      --adaptive-threads
                        実験的な適応型スレッド制御を有効化
      --fixed-threads   適応型制御と CPU 上限を無効化
      --thread-throughput-percent N
                        初期スループットの維持目標パーセント (80)
      --thread-system-cpu-percent N
                        ホスト CPU 使用率上限パーセント（0 で無効、既定 80）
      --metric files    バイト数ではなくファイル数で順位付け
  -h, --help            このヘルプを表示
"
        }
        Locale::Ko => {
            "\
ruum \u{2014} 디스크 사용량을 보여주는 트리맵 탐색기

사용법: ruum [옵션] [PATH]
        ruum ssh HOST [옵션] [PATH]
        ruum mcp [--read-only]
        ruum watchdog [--once]
        ruum --confirm-cleanup REQ --response RESP
        ruum extract-scanner ARCH OUTPUT

Linux 패키지 빌드는 필요 시 SSH로 내장 정적 스캐너를 배포합니다.
번들되지 않은 개발 빌드는 원격 PATH 또는 ~/.local/bin에 ruum-scan이 \
필요합니다. extract-scanner의 ARCH는 x86_64 또는 aarch64입니다.
SSH는 기존 설정, 키 및 에이전트를 사용하며 암호 입력은 터미널을 사용합니다.
원격 세션은 읽기 전용입니다(표시, 목록/프롬프트/삭제 스크립트 내보내기).
ssh와 --check를 함께 사용하면 창을 열지 않고 배포 및 스캔합니다.

인수:
  PATH              스캔할 디렉터리 경로(생략 시 ~ 홈 디렉터리)

PATH가 없으면 홈 디렉터리를 큰 항목부터 바로 스캔합니다.
스캔 전 --choose로 로컬 마운트 또는 SSH 소스를 선택할 수 있습니다.
중첩 마운트는 제외됩니다. Space로 표시, Enter로 열기, c로 검토 화면 이동,
?로 모든 단축키를 확인합니다.

옵션:
      --choose          스캔 전 소스 및 마운트 선택 화면 열기
  -a, --apparent-size   할당 블록 대신 겉보기 파일 크기 측정
  -l, --follow-links    심볼릭 링크 따라가기(로컬 스캔 전용)
  -H, --no-hidden       숨김 파일 및 디렉터리 건너뛰기
  -D, --disk            홈 디렉터리가 있는 전체 디스크 스캔
  -X, --cross-filesystems
                        PATH 아래 마운트된 다른 디스크·네트워크 공유도 측정\
(로컬 전용, 기본 꺼짐)
  -d, --depth N         한 번에 그릴 깊이(1-6, 기본값 3)
      --power-efficiency PRESET
                        miser, balanced(기본값), aggressive, drain-my-battery
      --scan-threads N  고정 스캔 작업자 수(사용 가능 CPU 수로 제한)
      --adaptive-threads
                        실험적 적응형 스레드 제어 사용
      --fixed-threads   적응형 제어 및 CPU 제한 비활성화
      --thread-throughput-percent N
                        초기 처리량 유지 비율 백분율(80)
      --thread-system-cpu-percent N
                        호스트 CPU 예산 백분율(0은 비활성화, 기본값 80)
      --metric files    바이트 대신 파일 수 기준으로 정렬
  -h, --help            이 도움말 표시
"
        }
    }
}

pub const fn mcp_help(locale: Locale) -> &'static str {
    match locale {
        Locale::En => {
            "ruum mcp \u{2014} Model Context Protocol (MCP) server over \
             stdio\n\nusage: ruum mcp [--read-only]"
        }
        Locale::Zh => {
            "ruum mcp \u{2014} 基于 stdio 的 Model Context Protocol (MCP) \
             服务器\n\n用法: ruum mcp [--read-only]"
        }
        Locale::Ja => {
            "ruum mcp \u{2014} stdio 経由の Model Context Protocol (MCP) \
             サーバー\n\n使い方: ruum mcp [--read-only]"
        }
        Locale::Ko => {
            "ruum mcp \u{2014} stdio 기반 Model Context Protocol (MCP) \
             서버\n\n사용법: ruum mcp [--read-only]"
        }
    }
}

pub const fn watchdog_help(locale: Locale) -> &'static str {
    match locale {
        Locale::En => {
            "ruum watchdog \u{2014} background volume monitor and AI cleanup \
             poller\n\nusage: ruum watchdog [--once]"
        }
        Locale::Zh => {
            "ruum watchdog \u{2014} 后台卷空间监控与 AI 清理轮询器\n\n\
             用法: ruum watchdog [--once]"
        }
        Locale::Ja => {
            "ruum watchdog \u{2014} バックグラウンド空き容量監視・AI \
             クリーンアップポーラー\n\n使い方: ruum watchdog [--once]"
        }
        Locale::Ko => {
            "ruum watchdog \u{2014} 백그라운드 볼륨 모니터 및 AI 정리 \
             폴러\n\n사용법: ruum watchdog [--once]"
        }
    }
}

pub fn cannot_load_power(error: &str, locale: Locale) -> String {
    match locale {
        Locale::En => {
            format!("Cannot load Power Efficiency: {error}; using Balanced")
        }
        Locale::Zh => {
            format!("无法加载电源效率设置：{error}；将使用“均衡”模式")
        }
        Locale::Ja => format!(
            "電源効率設定を読み込めません: {error}（「バランス」を使用します）"
        ),
        Locale::Ko => {
            format!("전원 효율 설정을 불러올 수 없습니다: {error}('균형' 사용)")
        }
    }
}

pub fn check_summary(
    host: &str,
    root: &str,
    bytes: u64,
    files: u64,
    dirs: u32,
    locale: Locale,
) -> String {
    match locale {
        Locale::En => format!(
            "{host}:{root} \u{2014} {bytes} bytes, {files} files, {dirs} \
             directories"
        ),
        Locale::Zh => format!(
            "{host}:{root} \u{2014} {bytes} 字节，{files} 个文件，{dirs} 个目录"
        ),
        Locale::Ja => format!(
            "{host}:{root} \u{2014} {bytes} バイト、{files} ファイル、{dirs} \
             ディレクトリ"
        ),
        Locale::Ko => format!(
            "{host}:{root} \u{2014} {bytes}바이트, 파일 {files}개, 디렉터리 \
             {dirs}개"
        ),
    }
}

pub fn extracted_scanner(arch: &str, path: &str, locale: Locale) -> String {
    match locale {
        Locale::En => format!("Extracted Linux {arch} scanner to {path}"),
        Locale::Zh => format!("已将 Linux {arch} 扫描器提取至 {path}"),
        Locale::Ja => {
            format!("Linux {arch} スキャナを {path} に展開しました")
        }
        Locale::Ko => format!("Linux {arch} 스캐너를 {path}에 추출했습니다"),
    }
}
