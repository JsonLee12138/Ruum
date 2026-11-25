//! Localized toast notices and status feedback strings for `state.rs`.

use ruum_core::i18n::{
    MAX_AI_FAILURE_SUMMARY_CHARS, clean_ai_failure_reason,
    truncate_preserving_trace,
};
use ruum_core::watchdog::{
    AiFailureKind, classify_ai_failure, format_ai_failure_summary,
};

use crate::i18n::{Locale, path_count, pick};

pub fn failed_save_settings(err: &str, locale: Locale) -> String {
    match locale {
        Locale::En => format!("Failed to save settings: {err}"),
        Locale::Zh => format!("保存设置失败：{err}"),
        Locale::Ja => format!("設定の保存に失敗しました: {err}"),
        Locale::Ko => format!("설정 저장 실패: {err}"),
    }
}

pub const fn invalid_watchdog_number(locale: Locale) -> &'static str {
    pick(
        locale,
        "Enter valid whole numbers for threshold, interval, and cooldown",
        "请为阈值、检查间隔和冷却时间输入有效的整数",
        "しきい値、確認間隔、クールダウンには有効な整数を入力してください",
        "임계값, 확인 간격 및 쿨다운에 올바른 정수를 입력하세요",
    )
}

pub fn cli_installed(path: &str, locale: Locale) -> String {
    match locale {
        Locale::En => format!("CLI command installed at {path}"),
        Locale::Zh => format!("CLI 命令已安装到 {path}"),
        Locale::Ja => format!("CLI コマンドを {path} にインストールしました"),
        Locale::Ko => format!("CLI 명령을 {path}에 설치했습니다"),
    }
}

pub const fn cli_uninstalled(locale: Locale) -> &'static str {
    pick(
        locale,
        "CLI command uninstalled",
        "CLI 命令已卸载",
        "CLI コマンドをアンインストールしました",
        "CLI 명령을 제거했습니다",
    )
}

pub fn cli_install_failed(err: &str, locale: Locale) -> String {
    match locale {
        Locale::En => format!("Failed to install CLI command: {err}"),
        Locale::Zh => format!("安装 CLI 命令失败：{err}"),
        Locale::Ja => {
            format!("CLI コマンドのインストールに失敗しました: {err}")
        }
        Locale::Ko => format!("CLI 명령 설치 실패: {err}"),
    }
}

pub fn cli_uninstall_failed(err: &str, locale: Locale) -> String {
    match locale {
        Locale::En => format!("Failed to uninstall CLI command: {err}"),
        Locale::Zh => format!("卸载 CLI 命令失败：{err}"),
        Locale::Ja => {
            format!("CLI コマンドのアンインストールに失敗しました: {err}")
        }
        Locale::Ko => format!("CLI 명령 제거 실패: {err}"),
    }
}

pub const fn settings_saved(locale: Locale) -> &'static str {
    pick(
        locale,
        "Settings saved",
        "设置已保存",
        "設定を保存しました",
        "설정이 저장되었습니다",
    )
}

pub const fn settings_saved_autostart_requires_approval(
    locale: Locale,
) -> &'static str {
    pick(
        locale,
        "Settings saved; allow Ruum in System Settings \u{2192} Login Items \
         to launch at login.",
        "设置已保存；请在「系统设置 → 通用 → 登录项」中允许 Ruum 开机启动。",
        "設定を保存しました。「システム設定 → ログイン項目」で Ruum を許可してください。",
        "설정이 저장되었습니다. 시스템 설정 \u{2192} 로그인 항목에서 Ruum을 허용해 주세요.",
    )
}

pub fn settings_saved_autostart_error(err: &str, locale: Locale) -> String {
    match locale {
        Locale::En => format!("Settings saved; autostart error: {err}"),
        Locale::Zh => format!("设置已保存；开机自启设置失败：{err}"),
        Locale::Ja => format!("設定を保存しました（自動起動エラー: {err}）"),
        Locale::Ko => format!("설정이 저장되었습니다(자동 실행 오류: {err})"),
    }
}

pub fn testing_cli_connection(provider: &str, locale: Locale) -> String {
    match locale {
        Locale::En => format!("Testing {provider} CLI connection\u{2026}"),
        Locale::Zh => format!("正在测试 {provider} CLI 连接\u{2026}"),
        Locale::Ja => format!("{provider} CLI 接続をテスト中\u{2026}"),
        Locale::Ko => format!("{provider} CLI 연결 테스트 중\u{2026}"),
    }
}

pub fn testing_api_connection(protocol: &str, locale: Locale) -> String {
    match locale {
        Locale::En => format!("Testing {protocol} API connection\u{2026}"),
        Locale::Zh => format!("正在测试 {protocol} API 连接\u{2026}"),
        Locale::Ja => format!("{protocol} API 接続をテスト中\u{2026}"),
        Locale::Ko => format!("{protocol} API 연결 테스트 중\u{2026}"),
    }
}

pub const fn api_key_cleared(locale: Locale) -> &'static str {
    pick(
        locale,
        "Cleared saved API key.",
        "已清除保存的 API 密钥。",
        "保存済みの API キーを削除しました。",
        "저장된 API 키를 지웠습니다.",
    )
}

pub fn fetching_models(provider: &str, locale: Locale) -> String {
    match locale {
        Locale::En => format!("Fetching {provider} models\u{2026}"),
        Locale::Zh => format!("正在获取 {provider} 模型列表\u{2026}"),
        Locale::Ja => format!("{provider} のモデルを取得中\u{2026}"),
        Locale::Ko => format!("{provider} 모델 목록 가져오는 중\u{2026}"),
    }
}

pub fn no_models_discovered(provider: &str, locale: Locale) -> String {
    match locale {
        Locale::En => format!("No models discovered for {provider}"),
        Locale::Zh => format!("未发现 {provider} 可用模型"),
        Locale::Ja => format!("{provider} のモデルが見つかりませんでした"),
        Locale::Ko => {
            format!("{provider}에서 사용 가능한 모델을 찾지 못했습니다")
        }
    }
}

pub fn loaded_models_with_note(
    count: usize,
    note: &str,
    locale: Locale,
) -> String {
    match locale {
        Locale::En => format!("Loaded {count} models ({note})"),
        Locale::Zh => format!("已加载 {count} 个模型（{note}）"),
        Locale::Ja => format!("{count} 個のモデルを読み込みました（{note}）"),
        Locale::Ko => format!("모델 {count}개를 불러왔습니다({note})"),
    }
}

pub fn loaded_models_for_provider(
    count: usize,
    provider: &str,
    locale: Locale,
) -> String {
    match locale {
        Locale::En => format!("Loaded {count} models for {provider}"),
        Locale::Zh => format!("已加载 {provider} 的 {count} 个模型"),
        Locale::Ja => {
            format!("{provider} のモデルを {count} 個読み込みました")
        }
        Locale::Ko => format!("{provider} 모델 {count}개를 불러왔습니다"),
    }
}

pub fn model_speed_failed(model: &str, detail: &str, locale: Locale) -> String {
    match locale {
        Locale::En => format!("Model {model} speed test failed: {detail}"),
        Locale::Zh => format!("模型 {model} 测速失败：{detail}"),
        Locale::Ja => format!("モデル {model} の速度テスト失敗: {detail}"),
        Locale::Ko => format!("모델 {model} 속도 테스트 실패: {detail}"),
    }
}

pub const fn fetch_models_first(locale: Locale) -> &'static str {
    pick(
        locale,
        "Fetch models first before testing all models",
        "请先获取模型列表，再测试全部模型",
        "全モデルをテストする前にモデル一覧を取得してください",
        "모든 모델을 테스트하기 전에 먼저 모델 목록을 가져오세요",
    )
}

pub fn benchmarking_models(total: usize, locale: Locale) -> String {
    match locale {
        Locale::En => format!("Benchmarking {total} models\u{2026}"),
        Locale::Zh => format!("正在测试 {total} 个模型速度\u{2026}"),
        Locale::Ja => format!("{total} 個のモデルをベンチマーク中\u{2026}"),
        Locale::Ko => format!("모델 {total}개 벤치마크 중\u{2026}"),
    }
}

pub fn speed_test_finished(
    ok_count: usize,
    total: usize,
    locale: Locale,
) -> String {
    match locale {
        Locale::En => {
            format!("Speed test finished: {ok_count}/{total} succeeded")
        }
        Locale::Zh => format!("测速完成：{ok_count}/{total} 成功"),
        Locale::Ja => format!("速度テスト完了: {ok_count}/{total} 成功"),
        Locale::Ko => format!("속도 테스트 완료: {ok_count}/{total} 성공"),
    }
}

pub const fn cannot_find_home_dir(locale: Locale) -> &'static str {
    pick(
        locale,
        "Cannot find home directory",
        "无法找到用户主目录",
        "ホームディレクトリが見つかりません",
        "홈 디렉터리를 찾을 수 없습니다",
    )
}

pub fn running_ai_analysis(provider: &str, locale: Locale) -> String {
    match locale {
        Locale::En => format!("Running AI analysis via {provider}\u{2026}"),
        Locale::Zh => format!("正在通过 {provider} 运行 AI 分析\u{2026}"),
        Locale::Ja => format!("{provider} で AI 分析を実行中\u{2026}"),
        Locale::Ko => format!("{provider}(으)로 AI 분석 실행 중\u{2026}"),
    }
}

pub const fn ai_found_no_files(locale: Locale) -> &'static str {
    pick(
        locale,
        "AI analysis found no reclaimable files",
        "AI 分析未发现可清理文件",
        "AI 分析では削除可能なファイルが見つかりませんでした",
        "AI 분석에서 정리할 파일을 찾지 못했습니다",
    )
}

pub fn ai_proposed_items(count: usize, locale: Locale) -> String {
    match locale {
        Locale::En => format!("AI proposed {count} candidate items for review"),
        Locale::Zh => format!("AI 提出了 {count} 个待审核清理项"),
        Locale::Ja => {
            format!("AI が {count} 件のクリーンアップ候補を提案しました")
        }
        Locale::Ko => format!("AI가 검토할 후보 항목 {count}개를 제안했습니다"),
    }
}

pub fn ai_analysis_error(err: &str, locale: Locale) -> String {
    format_ai_failure_summary(err, locale)
}

pub fn power_saved(label: &str, locale: Locale) -> String {
    match locale {
        Locale::En => format!("{label} from the next scan; r rescans now"),
        Locale::Zh => format!("自下次扫描生效：{label}；按 r 立即重扫"),
        Locale::Ja => {
            format!("次のスキャンから {label}（r で今すぐ再スキャン）")
        }
        Locale::Ko => format!("다음 스캔부터 {label}(r로 지금 다시 스캔)"),
    }
}

pub fn power_save_failed(label: &str, err: &str, locale: Locale) -> String {
    match locale {
        Locale::En => {
            format!(
                "{label} from the next scan, but it could not be saved: {err}"
            )
        }
        Locale::Zh => format!("自下次扫描生效：{label}，但无法保存：{err}"),
        Locale::Ja => {
            format!("次のスキャンから {label}（保存できませんでした: {err}）")
        }
        Locale::Ko => format!("다음 스캔부터 {label}(저장 실패: {err})"),
    }
}

pub fn power_no_dir(label: &str, locale: Locale) -> String {
    match locale {
        Locale::En => {
            format!(
                "{label} from the next scan; no settings directory to save it in"
            )
        }
        Locale::Zh => format!("自下次扫描生效：{label}；无设置目录可供保存"),
        Locale::Ja => {
            format!("次のスキャンから {label}（保存先の設定ディレクトリなし）")
        }
        Locale::Ko => {
            format!("다음 스캔부터 {label}(저장할 설정 디렉터리 없음)")
        }
    }
}

pub const fn title_confirm_remote_connect(locale: Locale) -> &'static str {
    pick(
        locale,
        "ruum \u{00b7} Confirm Remote SSH Connection",
        "ruum \u{00b7} \u{786e}\u{8ba4}\u{8fdc}\u{7a0b} SSH \u{8fde}\u{63a5}",
        "ruum \u{00b7} \u{30ea}\u{30e2}\u{30fc}\u{30c8} SSH \u{63a5}\u{7d9a}\u{306e}\u{78ba}\u{8a8d}",
        "ruum \u{00b7} \u{c6d0}\u{aca9} SSH \u{c5f0}\u{acb0} \u{d655}\u{c778}",
    )
}

pub const fn invalid_general_number(locale: Locale) -> &'static str {
    pick(
        locale,
        "Enter valid positive integers for default depth and SSH lease.",
        "\u{8bf7}\u{4e3a}\u{9ed8}\u{8ba4}\u{6df1}\u{5ea6}\u{548c} SSH \u{79df}\u{671f}\u{8f93}\u{5165}\u{6709}\u{6548}\u{7684}\u{6b63}\u{6574}\u{6570}\u{3002}",
        "\u{30c7}\u{30d5}\u{30a9}\u{30eb}\u{30c8}\u{6df1}\u{5ea6}\u{3068} SSH \u{671f}\u{9593}\u{306b}\u{6709}\u{52b9}\u{306a}\u{6b63}\u{306e}\u{6574}\u{6570}\u{3092}\u{5165}\u{529b}\u{3057}\u{3066}\u{304f}\u{3060}\u{3055}\u{3044}\u{3002}",
        "\u{ae30}\u{bcf8} \u{ae4a}\u{c774}\u{c640} SSH \u{c138}\u{c158} \u{ae30}\u{ac04}\u{c5d0} \u{c62c}\u{bc14}\u{b978} \u{c591}\u{c758} \u{c815}\u{c218}\u{b97c} \u{c785}\u{b825}\u{d558}\u{c138}\u{c694}.",
    )
}

pub fn invalid_webhook_config(err: &str, locale: Locale) -> String {
    match locale {
        Locale::En => format!("Invalid webhook settings: {err}"),
        Locale::Zh => {
            format!("Webhook \u{8bbe}\u{7f6e}\u{65e0}\u{6548}\u{ff1a}{err}")
        }
        Locale::Ja => format!(
            "Webhook \u{8a2d}\u{5b9a}\u{304c}\u{7121}\u{52b9}\u{3067}\u{3059}: {err}"
        ),
        Locale::Ko => {
            format!("\u{c798}\u{babb}\u{b41c} Webhook \u{c124}\u{c815}: {err}")
        }
    }
}

pub fn ssh_profile_saved(alias: &str, locale: Locale) -> String {
    match locale {
        Locale::En => format!("Saved SSH server `{alias}`"),
        Locale::Zh => format!(
            "\u{5df2}\u{4fdd}\u{5b58} SSH \u{670d}\u{52a1}\u{5668}\u{300c}{alias}\u{300d}"
        ),
        Locale::Ja => format!(
            "SSH \u{30b5}\u{30fc}\u{30d0}\u{30fc}\u{300c}{alias}\u{300d}\u{3092}\u{4fdd}\u{5b58}\u{3057}\u{307e}\u{3057}\u{305f}"
        ),
        Locale::Ko => format!(
            "SSH \u{c11c}\u{bc84} `{alias}`\u{b97c} \u{c800}\u{c7a5}\u{d588}\u{c2b5}\u{b2c8}\u{b2e4}"
        ),
    }
}

pub fn ssh_profile_deleted(alias: &str, locale: Locale) -> String {
    match locale {
        Locale::En => format!("Deleted SSH server `{alias}`"),
        Locale::Zh => format!(
            "\u{5df2}\u{5220}\u{9664} SSH \u{670d}\u{52a1}\u{5668}\u{300c}{alias}\u{300d}"
        ),
        Locale::Ja => format!(
            "SSH \u{30b5}\u{30fc}\u{30d0}\u{30fc}\u{300c}{alias}\u{300d}\u{3092}\u{524a}\u{9664}\u{3057}\u{307e}\u{3057}\u{305f}"
        ),
        Locale::Ko => format!(
            "SSH \u{c11c}\u{bc84} `{alias}`\u{b97c} \u{c0ad}\u{c81c}\u{d588}\u{c2b5}\u{b2c8}\u{b2e4}"
        ),
    }
}

pub fn ssh_profile_save_failed(err: &str, locale: Locale) -> String {
    match locale {
        Locale::En => format!("Failed to save SSH server: {err}"),
        Locale::Zh => format!(
            "\u{4fdd}\u{5b58} SSH \u{670d}\u{52a1}\u{5668}\u{5931}\u{8d25}\u{ff1a}{err}"
        ),
        Locale::Ja => format!(
            "SSH \u{30b5}\u{30fc}\u{30d0}\u{30fc}\u{306e}\u{4fdd}\u{5b58}\u{306b}\u{5931}\u{6557}\u{3057}\u{307e}\u{3057}\u{305f}: {err}"
        ),
        Locale::Ko => format!(
            "SSH \u{c11c}\u{bc84} \u{c800}\u{c7a5} \u{c2e4}\u{d328}: {err}"
        ),
    }
}

pub fn ssh_profile_delete_failed(err: &str, locale: Locale) -> String {
    match locale {
        Locale::En => format!("Failed to delete SSH server: {err}"),
        Locale::Zh => format!(
            "\u{5220}\u{9664} SSH \u{670d}\u{52a1}\u{5668}\u{5931}\u{8d25}\u{ff1a}{err}"
        ),
        Locale::Ja => format!(
            "SSH \u{30b5}\u{30fc}\u{30d0}\u{30fc}\u{306e}\u{524a}\u{9664}\u{306b}\u{5931}\u{6557}\u{3057}\u{307e}\u{3057}\u{305f}: {err}"
        ),
        Locale::Ko => format!(
            "SSH \u{c11c}\u{bc84} \u{c0ad}\u{c81c} \u{c2e4}\u{d328}: {err}"
        ),
    }
}

pub fn ssh_connect_failed(alias: &str, err: &str, locale: Locale) -> String {
    match locale {
        Locale::En => format!("Failed to connect to `{alias}`: {err}"),
        Locale::Zh => format!(
            "\u{8fde}\u{63a5}\u{300c}{alias}\u{300d}\u{5931}\u{8d25}\u{ff1a}{err}"
        ),
        Locale::Ja => format!(
            "\u{300c}{alias}\u{300d}\u{3078}\u{306e}\u{63a5}\u{7d9a}\u{306b}\u{5931}\u{6557}\u{3057}\u{307e}\u{3057}\u{305f}: {err}"
        ),
        Locale::Ko => {
            format!("`{alias}` \u{c5f0}\u{acb0} \u{c2e4}\u{d328}: {err}")
        }
    }
}

pub const fn ssh_secret_required(locale: Locale) -> &'static str {
    pick(
        locale,
        "Enter a password or private key for this authentication mode.",
        "\u{8bf7}\u{4e3a}\u{5f53}\u{524d}\u{8ba4}\u{8bc1}\u{65b9}\u{5f0f}\u{8f93}\u{5165}\u{5bc6}\u{7801}\u{6216}\u{79c1}\u{94a5}\u{3002}",
        "\u{3053}\u{306e}\u{8a8d}\u{8a3c}\u{65b9}\u{5f0f}\u{306e}\u{30d1}\u{30b9}\u{30ef}\u{30fc}\u{30c9}\u{307e}\u{305f}\u{306f}\u{79d8}\u{5bc6}\u{9375}\u{3092}\u{5165}\u{529b}\u{3057}\u{3066}\u{304f}\u{3060}\u{3055}\u{3044}\u{3002}",
        "\u{c774} \u{c778}\u{c99d} \u{bc29}\u{c2dd}\u{c5d0} \u{d544}\u{c694}\u{d55c} \u{be44}\u{bc00}\u{bc88}\u{d638} \u{b610}\u{b294} \u{ac1c}\u{c778} \u{d0a4}\u{b97c} \u{c785}\u{b825}\u{d558}\u{c138}\u{c694}.",
    )
}

pub const fn ssh_invalid_port(locale: Locale) -> &'static str {
    pick(
        locale,
        "SSH port must be a number between 1 and 65535.",
        "SSH \u{7aef}\u{53e3}\u{5fc5}\u{987b}\u{4e3a} 1 \u{5230} 65535 \u{4e4b}\u{95f4}\u{7684}\u{6574}\u{6570}\u{3002}",
        "SSH \u{30dd}\u{30fc}\u{30c8}\u{306f} 1 \u{304b}\u{3089} 65535 \u{306e}\u{9593}\u{306e}\u{6570}\u{5024}\u{3067}\u{3042}\u{308b}\u{5fc5}\u{8981}\u{304c}\u{3042}\u{308a}\u{307e}\u{3059}\u{3002}",
        "SSH \u{d3ec}\u{d2b8}\u{b294} 1\u{c5d0}\u{c11c} 65535 \u{c0ac}\u{c774}\u{c758} \u{c22b}\u{c790}\u{c5ec}\u{c57c} \u{d569}\u{b2c8}\u{b2e4}.",
    )
}

pub fn ssh_invalid_input(err: &str, locale: Locale) -> String {
    match locale {
        Locale::En => format!("Invalid SSH server settings: {err}"),
        Locale::Zh => {
            format!(
                "SSH \u{670d}\u{52a1}\u{5668}\u{8bbe}\u{7f6e}\u{65e0}\u{6548}\u{ff1a}{err}"
            )
        }
        Locale::Ja => format!(
            "SSH \u{30b5}\u{30fc}\u{30d0}\u{30fc}\u{8a2d}\u{5b9a}\u{304c}\u{7121}\u{52b9}\u{3067}\u{3059}: {err}"
        ),
        Locale::Ko => {
            format!(
                "\u{c798}\u{babb}\u{b41c} SSH \u{c11c}\u{bc84} \u{c124}\u{c815}: {err}"
            )
        }
    }
}

pub const fn remote_identity_changed(locale: Locale) -> &'static str {
    pick(
        locale,
        "Remote identity changed; previous marks were cleared.",
        "远程主机标识已变化，先前的标记已清除。",
        "リモートホストの識別子が変更されたため、以前のマークをクリアしました。",
        "원격 호스트 식별자가 변경되어 이전 표시를 지웠습니다.",
    )
}

pub const fn cannot_remove_root(locale: Locale) -> &'static str {
    pick(
        locale,
        "the scanned root cannot be removed; open a directory first",
        "无法删除当前扫描根目录；请先进入子目录",
        "スキャンルート自体は削除できません。先にディレクトリを開いてください",
        "스캔한 루트는 삭제할 수 없습니다. 먼저 디렉터리를 여세요",
    )
}

pub fn goes_with_marked(
    target: &str,
    ancestor: &str,
    locale: Locale,
) -> String {
    match locale {
        Locale::En => format!(
            "{target} goes with the marked {ancestor}; unmark that to keep it"
        ),
        Locale::Zh => {
            format!(
                "{target} 已包含在标记项 {ancestor} 中；如需保留请先取消标记它"
            )
        }
        Locale::Ja => format!(
            "{target} はマーク済みの {ancestor} に含まれます。残す場合は \
             {ancestor} のマークを解除してください"
        ),
        Locale::Ko => format!(
            "{target}은(는) 표시된 {ancestor}에 포함됩니다. 유지하려면 \
             {ancestor}의 표시를 해제하세요"
        ),
    }
}

pub fn covers_marks_inside(path: &str, count: usize, locale: Locale) -> String {
    match locale {
        Locale::En => {
            let s = if count == 1 { "" } else { "s" };
            format!("{path} now covers {count} mark{s} inside it")
        }
        Locale::Zh => format!("{path} 已覆盖其内部 {count} 项标记"),
        Locale::Ja => {
            format!("{path} が配下の {count} 件のマークを統合しました")
        }
        Locale::Ko => {
            format!("{path}이(가) 내부의 {count}개 표시를 포함합니다")
        }
    }
}

pub const fn nothing_is_marked(locale: Locale) -> &'static str {
    pick(
        locale,
        "nothing is marked",
        "尚未标记任何项目",
        "何もマークされていません",
        "표시된 항목이 없습니다",
    )
}

pub const fn not_while_restarting(locale: Locale) -> &'static str {
    pick(
        locale,
        "not while restarting as administrator",
        "正在以管理员身份重启，暂时无法执行",
        "管理者として再起動中は実行できません",
        "관리자 권한으로 다시 시작하는 동안에는 실행할 수 없습니다",
    )
}

pub fn nothing_matches(needle: &str, locale: Locale) -> String {
    match locale {
        Locale::En => format!("nothing here matches {needle}"),
        Locale::Zh => format!("当前目录中没有匹配 {needle} 的项目"),
        Locale::Ja => format!("{needle} に一致する項目がありません"),
        Locale::Ko => format!("{needle}와(과) 일치하는 항목이 없습니다"),
    }
}

pub const fn nothing_to_save(locale: Locale) -> &'static str {
    pick(
        locale,
        "nothing to save",
        "没有可保存的内容",
        "保存する項目がありません",
        "저장할 항목이 없습니다",
    )
}

pub fn saved_paths_to(count: usize, path: &str, locale: Locale) -> String {
    let paths = path_count(count, locale);
    match locale {
        Locale::En => format!("saved {paths} to {path}"),
        Locale::Zh => format!("已保存 {paths} 到 {path}"),
        Locale::Ja => format!("{paths} を {path} に保存しました"),
        Locale::Ko => format!("{paths}를 {path}에 저장했습니다"),
    }
}

pub fn could_not_save_path(path: &str, err: &str, locale: Locale) -> String {
    match locale {
        Locale::En => format!("could not save {path}: {err}"),
        Locale::Zh => format!("无法保存 {path}：{err}"),
        Locale::Ja => format!("{path} を保存できませんでした: {err}"),
        Locale::Ko => format!("{path} 저장 실패: {err}"),
    }
}

pub const fn nothing_to_copy(locale: Locale) -> &'static str {
    pick(
        locale,
        "nothing to copy",
        "没有可复制的内容",
        "コピーする項目がありません",
        "복사할 항목이 없습니다",
    )
}

pub const fn copied_delete_script(locale: Locale) -> &'static str {
    pick(
        locale,
        "copied delete script; previews by default, checks host before deletion",
        "已复制删除脚本；默认仅预览，并在删除前校验主机",
        "削除スクリプトをコピーしました（既定でプレビュー、削除前にホストを確認）",
        "삭제 스크립트를 복사했습니다(기본 미리보기, 삭제 전 호스트 확인)",
    )
}

pub fn copied_agent_prompt(
    count: usize,
    bytes: &str,
    locale: Locale,
) -> String {
    let paths = path_count(count, locale);
    match locale {
        Locale::En => {
            format!("copied a prompt for your agent: {paths}, {bytes}")
        }
        Locale::Zh => format!("已复制给代理的提示词：{paths}，{bytes}"),
        Locale::Ja => format!(
            "エージェント用プロンプトをコピーしました: {paths}、{bytes}"
        ),
        Locale::Ko => {
            format!("에이전트용 프롬프트를 복사했습니다: {paths}, {bytes}")
        }
    }
}

pub fn no_longer_on_disk(path: &str, locale: Locale) -> String {
    match locale {
        Locale::En => format!("{path} is no longer on disk"),
        Locale::Zh => format!("{path} 已不在磁盘上"),
        Locale::Ja => format!("{path} はすでにディスク上にありません"),
        Locale::Ko => format!("{path}이(가) 더 이상 디스크에 없습니다"),
    }
}

pub const fn stopping_after_current(locale: Locale) -> &'static str {
    pick(
        locale,
        "stopping after the current item",
        "将在当前项目完成后停止",
        "現在の項目完了後に停止します",
        "현재 항목 완료 후 중지합니다",
    )
}

pub const fn mark_something_first(locale: Locale) -> &'static str {
    pick(
        locale,
        "mark something first: space marks the selected tile",
        "请先标记项目：按 Space 标记选中区块",
        "先にマークしてください: Space で選択タイルをマークします",
        "먼저 표시하세요: Space로 선택한 타일을 표시합니다",
    )
}

pub fn still_not_admin(err: &str, locale: Locale) -> String {
    match locale {
        Locale::En => format!("still not an administrator: {err}"),
        Locale::Zh => format!("仍未获得管理员权限：{err}"),
        Locale::Ja => format!("管理者権限を取得できませんでした: {err}"),
        Locale::Ko => format!("관리자 권한을 얻지 못했습니다: {err}"),
    }
}

pub fn ssh_space_checked_healthy(
    alias: &str,
    free: &str,
    total: &str,
    min_gb: u64,
    locale: Locale,
) -> String {
    match locale {
        Locale::En => format!(
            "{alias}: {free} free of {total} \u{2014} healthy (above {min_gb} GB)"
        ),
        Locale::Zh => format!(
            "{alias}\u{ff1a}\u{53ef}\u{7528} {free} / \u{5171} {total} \u{2014} \u{5065}\u{5eb7}\u{ff08}\u{9ad8}\u{4e8e} {min_gb} GB\u{ff09}"
        ),
        Locale::Ja => format!(
            "{alias}: \u{7a7a}\u{304d} {free} / {total} \u{2014} \u{6b63}\u{5e38}\u{ff08}{min_gb} GB \u{4ee5}\u{4e0a}\u{ff09}"
        ),
        Locale::Ko => format!(
            "{alias}: \u{c5ec}\u{c720} {free} / {total} \u{2014} \u{c815}\u{c0c1}({min_gb} GB \u{c774}\u{c0c1})"
        ),
    }
}

pub fn ssh_space_checked_below(
    alias: &str,
    free: &str,
    total: &str,
    min_gb: u64,
    locale: Locale,
) -> String {
    match locale {
        Locale::En => format!(
            "{alias}: {free} free of {total} \u{2014} below {min_gb} GB threshold"
        ),
        Locale::Zh => format!(
            "{alias}\u{ff1a}\u{53ef}\u{7528} {free} / \u{5171} {total} \u{2014} \u{4f4e}\u{4e8e} {min_gb} GB \u{9608}\u{503c}"
        ),
        Locale::Ja => format!(
            "{alias}: \u{7a7a}\u{304d} {free} / {total} \u{2014} {min_gb} GB \u{3057}\u{304d}\u{3044}\u{5024}\u{672a}\u{6e80}"
        ),
        Locale::Ko => format!(
            "{alias}: \u{c5ec}\u{c720} {free} / {total} \u{2014} {min_gb} GB \u{c784}\u{acc4}\u{ac12} \u{bbf8}\u{b9cc}"
        ),
    }
}

pub fn ssh_check_space_failed(
    alias: &str,
    err: &str,
    locale: Locale,
) -> String {
    match locale {
        Locale::En => format!("Remote space check failed for `{alias}`: {err}"),
        Locale::Zh => format!(
            "\u{68c0}\u{67e5}\u{8fdc}\u{7a0b}\u{4e3b}\u{673a}\u{300c}{alias}\u{300d}\u{7a7a}\u{95f4}\u{5931}\u{8d25}\u{ff1a}{err}"
        ),
        Locale::Ja => format!(
            "\u{30ea}\u{30e2}\u{30fc}\u{30c8} `{alias}` \u{306e}\u{7a7a}\u{304d}\u{5bb9}\u{91cf}\u{78ba}\u{8a8d}\u{306b}\u{5931}\u{6557}\u{3057}\u{307e}\u{3057}\u{305f}: {err}"
        ),
        Locale::Ko => format!(
            "\u{c6d0}\u{aca9} `{alias}` \u{acf5}\u{ac04} \u{d655}\u{c778} \u{c2e4}\u{d328}: {err}"
        ),
    }
}

pub fn ssh_ai_analyze_failed(alias: &str, err: &str, locale: Locale) -> String {
    let kind = classify_ai_failure(err);
    let detail = clean_ai_failure_reason(err);
    let msg = if detail.is_empty() {
        match (kind, locale) {
            (AiFailureKind::TimedOut, Locale::En) => {
                format!("Remote AI analysis timed out for `{alias}`.")
            }
            (AiFailureKind::TimedOut, Locale::Zh) => {
                format!(
                    "\u{8fdc}\u{7a0b}\u{4e3b}\u{673a}\u{300c}{alias}\u{300d} AI \u{5206}\u{6790}\u{8d85}\u{65f6}\u{3002}"
                )
            }
            (AiFailureKind::TimedOut, Locale::Ja) => {
                format!(
                    "\u{30ea}\u{30e2}\u{30fc}\u{30c8} `{alias}` \u{306e} AI \u{5206}\u{6790}\u{304c}\u{30bf}\u{30a4}\u{30e0}\u{30a2}\u{30a6}\u{30c8}\u{3057}\u{307e}\u{3057}\u{305f}\u{3002}"
                )
            }
            (AiFailureKind::TimedOut, Locale::Ko) => {
                format!(
                    "\u{c6d0}\u{aca9} `{alias}` AI \u{bd84}\u{c11d} \u{c2dc}\u{ac04}\u{c774} \u{cd08}\u{acfc}\u{b418}\u{c5c8}\u{c2b5}\u{b2c8}\u{b2e4}."
                )
            }
            (AiFailureKind::Failed, Locale::En) => {
                format!("Remote AI analysis failed for `{alias}`.")
            }
            (AiFailureKind::Failed, Locale::Zh) => {
                format!(
                    "\u{8fdc}\u{7a0b}\u{4e3b}\u{673a}\u{300c}{alias}\u{300d} AI \u{5206}\u{6790}\u{5931}\u{8d25}\u{3002}"
                )
            }
            (AiFailureKind::Failed, Locale::Ja) => {
                format!(
                    "\u{30ea}\u{30e2}\u{30fc}\u{30c8} `{alias}` \u{306e} AI \u{5206}\u{6790}\u{306b}\u{5931}\u{6557}\u{3057}\u{307e}\u{3057}\u{305f}\u{3002}"
                )
            }
            (AiFailureKind::Failed, Locale::Ko) => {
                format!(
                    "\u{c6d0}\u{aca9} `{alias}` AI \u{bd84}\u{c11d} \u{c2e4}\u{d328}."
                )
            }
        }
    } else {
        match (kind, locale) {
            (AiFailureKind::TimedOut, Locale::En) => {
                format!("Remote AI analysis timed out for `{alias}`: {detail}")
            }
            (AiFailureKind::TimedOut, Locale::Zh) => format!(
                "\u{8fdc}\u{7a0b}\u{4e3b}\u{673a}\u{300c}{alias}\u{300d} AI \u{5206}\u{6790}\u{8d85}\u{65f6}\u{ff1a}{detail}"
            ),
            (AiFailureKind::TimedOut, Locale::Ja) => format!(
                "\u{30ea}\u{30e2}\u{30fc}\u{30c8} `{alias}` \u{306e} AI \u{5206}\u{6790}\u{304c}\u{30bf}\u{30a4}\u{30e0}\u{30a2}\u{30a6}\u{30c8}\u{3057}\u{307e}\u{3057}\u{305f}: {detail}"
            ),
            (AiFailureKind::TimedOut, Locale::Ko) => format!(
                "\u{c6d0}\u{aca9} `{alias}` AI \u{bd84}\u{c11d} \u{c2dc}\u{ac04} \u{cd08}\u{acfc}: {detail}"
            ),
            (AiFailureKind::Failed, Locale::En) => {
                format!("Remote AI analysis failed for `{alias}`: {detail}")
            }
            (AiFailureKind::Failed, Locale::Zh) => format!(
                "\u{8fdc}\u{7a0b}\u{4e3b}\u{673a}\u{300c}{alias}\u{300d} AI \u{5206}\u{6790}\u{5931}\u{8d25}\u{ff1a}{detail}"
            ),
            (AiFailureKind::Failed, Locale::Ja) => format!(
                "\u{30ea}\u{30e2}\u{30fc}\u{30c8} `{alias}` \u{306e} AI \u{5206}\u{6790}\u{306b}\u{5931}\u{6557}\u{3057}\u{307e}\u{3057}\u{305f}: {detail}"
            ),
            (AiFailureKind::Failed, Locale::Ko) => format!(
                "\u{c6d0}\u{aca9} `{alias}` AI \u{bd84}\u{c11d} \u{c2e4}\u{d328}: {detail}"
            ),
        }
    };
    truncate_preserving_trace(&msg, MAX_AI_FAILURE_SUMMARY_CHARS)
}

pub fn webhook_saved(name: &str, locale: Locale) -> String {
    match locale {
        Locale::En => format!("Saved webhook `{name}`"),
        Locale::Zh => {
            format!("\u{5df2}\u{4fdd}\u{5b58} Webhook\u{300c}{name}\u{300d}")
        }
        Locale::Ja => format!(
            "Webhook \u{300c}{name}\u{300d}\u{3092}\u{4fdd}\u{5b58}\u{3057}\u{307e}\u{3057}\u{305f}"
        ),
        Locale::Ko => format!("Webhook `{name}` \u{c800}\u{c7a5}\u{b428}"),
    }
}

pub fn webhook_deleted(name: &str, locale: Locale) -> String {
    match locale {
        Locale::En => format!("Deleted webhook `{name}`"),
        Locale::Zh => {
            format!("\u{5df2}\u{5220}\u{9664} Webhook\u{300c}{name}\u{300d}")
        }
        Locale::Ja => format!(
            "Webhook \u{300c}{name}\u{300d}\u{3092}\u{524a}\u{9664}\u{3057}\u{307e}\u{3057}\u{305f}"
        ),
        Locale::Ko => format!("Webhook `{name}` \u{c0ad}\u{c81c}\u{b428}"),
    }
}

pub fn webhook_test_succeeded(name: &str, locale: Locale) -> String {
    match locale {
        Locale::En => format!("Test webhook sent to `{name}`"),
        Locale::Zh => format!(
            "\u{5df2}\u{5411}\u{300c}{name}\u{300d}\u{53d1}\u{9001}\u{6d4b}\u{8bd5} Webhook"
        ),
        Locale::Ja => format!(
            "\u{300c}{name}\u{300d}\u{3078}\u{30c6}\u{30b9}\u{30c8} Webhook \u{3092}\u{9001}\u{4fe1}\u{3057}\u{307e}\u{3057}\u{305f}"
        ),
        Locale::Ko => format!(
            "`{name}`\u{c5d0} \u{d14c}\u{c2a4}\u{d2b8} Webhook \u{c804}\u{c1a1} \u{c644}\u{b8cc}"
        ),
    }
}

pub fn webhook_test_failed(name: &str, err: &str, locale: Locale) -> String {
    match locale {
        Locale::En => format!("Webhook test failed for `{name}`: {err}"),
        Locale::Zh => format!(
            "\u{300c}{name}\u{300d}Webhook \u{6d4b}\u{8bd5}\u{5931}\u{8d25}\u{ff1a}{err}"
        ),
        Locale::Ja => format!(
            "\u{300c}{name}\u{300d}\u{306e} Webhook \u{30c6}\u{30b9}\u{30c8}\u{5931}\u{6557}: {err}"
        ),
        Locale::Ko => format!(
            "`{name}` Webhook \u{d14c}\u{c2a4}\u{d2b8} \u{c2e4}\u{d328}: {err}"
        ),
    }
}

pub fn webhook_save_failed(err: &str, locale: Locale) -> String {
    match locale {
        Locale::En => format!("Failed to save webhook: {err}"),
        Locale::Zh => {
            format!("\u{4fdd}\u{5b58} Webhook \u{5931}\u{8d25}\u{ff1a}{err}")
        }
        Locale::Ja => format!(
            "Webhook \u{306e}\u{4fdd}\u{5b58}\u{306b}\u{5931}\u{6557}\u{3057}\u{307e}\u{3057}\u{305f}: {err}"
        ),
        Locale::Ko => {
            format!("Webhook \u{c800}\u{c7a5} \u{c2e4}\u{d328}: {err}")
        }
    }
}

pub fn script_generate_failed(err: &str, locale: Locale) -> String {
    match locale {
        Locale::En => format!("Failed to generate cleanup script: {err}"),
        Locale::Zh => {
            format!(
                "\u{751f}\u{6210}\u{6e05}\u{7406}\u{811a}\u{672c}\u{5931}\u{8d25}\u{ff1a}{err}"
            )
        }
        Locale::Ja => format!(
            "\u{30af}\u{30ea}\u{30fc}\u{30f3}\u{30a2}\u{30c3}\u{30d7}\u{30b9}\u{30af}\u{30ea}\u{30d7}\u{30c8}\u{306e}\u{751f}\u{6210}\u{306b}\u{5931}\u{6557}\u{3057}\u{307e}\u{3057}\u{305f}: {err}"
        ),
        Locale::Ko => format!(
            "\u{c815}\u{b9ac} \u{c2a4}\u{d06c}\u{b9bd}\u{d2b8} \u{c0dd}\u{c131} \u{c2e4}\u{d328}: {err}"
        ),
    }
}

pub fn script_launched_in_terminal(alias: &str, locale: Locale) -> String {
    match locale {
        Locale::En => {
            format!("Opened terminal to run cleanup script for `{alias}`")
        }
        Locale::Zh => format!(
            "\u{5df2}\u{5728}\u{7ec8}\u{7aef}\u{4e2d}\u{6253}\u{5f00}\u{300c}{alias}\u{300d}\u{7684}\u{6e05}\u{7406}\u{811a}\u{672c}"
        ),
        Locale::Ja => format!(
            "`{alias}` \u{306e}\u{30af}\u{30ea}\u{30fc}\u{30f3}\u{30a2}\u{30c3}\u{30d7}\u{30b9}\u{30af}\u{30ea}\u{30d7}\u{30c8}\u{3092}\u{7aef}\u{672b}\u{3067}\u{958b}\u{304d}\u{307e}\u{3057}\u{305f}"
        ),
        Locale::Ko => format!(
            "`{alias}` \u{c815}\u{b9ac} \u{c2a4}\u{d06c}\u{b9bd}\u{d2b8}\u{b97c} \u{d130}\u{bbf8}\u{b110}\u{c5d0}\u{c11c} \u{c5f4}\u{c5c8}\u{c2b5}\u{b2c8}\u{b2e4}"
        ),
    }
}

pub fn script_launch_failed(err: &str, locale: Locale) -> String {
    match locale {
        Locale::En => format!("Failed to open terminal for script: {err}"),
        Locale::Zh => {
            format!(
                "\u{6253}\u{5f00}\u{7ec8}\u{7aef}\u{6267}\u{884c}\u{811a}\u{672c}\u{5931}\u{8d25}\u{ff1a}{err}"
            )
        }
        Locale::Ja => format!(
            "\u{7aef}\u{672b}\u{3067}\u{30b9}\u{30af}\u{30ea}\u{30d7}\u{30c8}\u{3092}\u{958b}\u{3051}\u{307e}\u{305b}\u{3093}\u{3067}\u{3057}\u{305f}: {err}"
        ),
        Locale::Ko => format!(
            "\u{d130}\u{bbf8}\u{b110}\u{c5d0}\u{c11c} \u{c2a4}\u{d06c}\u{b9bd}\u{d2b8} \u{c2e4}\u{d589} \u{c2e4}\u{d328}: {err}"
        ),
    }
}

pub fn script_reveal_failed(err: &str, locale: Locale) -> String {
    match locale {
        Locale::En => format!("Failed to reveal script file: {err}"),
        Locale::Zh => {
            format!(
                "\u{6253}\u{5f00}\u{811a}\u{672c}\u{6240}\u{5728}\u{76ee}\u{5f55}\u{5931}\u{8d25}\u{ff1a}{err}"
            )
        }
        Locale::Ja => format!(
            "\u{30b9}\u{30af}\u{30ea}\u{30d7}\u{30c8}\u{30d5}\u{30a1}\u{30a4}\u{30eb}\u{3092}\u{8868}\u{793a}\u{3067}\u{304d}\u{307e}\u{305b}\u{3093}\u{3067}\u{3057}\u{305f}: {err}"
        ),
        Locale::Ko => format!(
            "\u{c2a4}\u{d06c}\u{b9bd}\u{d2b8} \u{d30c}\u{c77c} \u{c5f4}\u{ae30} \u{c2e4}\u{d328}: {err}"
        ),
    }
}

pub fn script_discarded(alias: &str, locale: Locale) -> String {
    match locale {
        Locale::En => format!("Discarded pending cleanup script for `{alias}`"),
        Locale::Zh => format!(
            "\u{5df2}\u{4f5c}\u{5e9f}\u{300c}{alias}\u{300d}\u{7684}\u{5f85}\u{6267}\u{884c}\u{6e05}\u{7406}\u{811a}\u{672c}"
        ),
        Locale::Ja => format!(
            "`{alias}` \u{306e}\u{672a}\u{5b9f}\u{884c}\u{30b9}\u{30af}\u{30ea}\u{30d7}\u{30c8}\u{3092}\u{7834}\u{68c4}\u{3057}\u{307e}\u{3057}\u{305f}"
        ),
        Locale::Ko => format!(
            "`{alias}`\u{c758} \u{b300}\u{ae30} \u{c815}\u{b9ac} \u{c2a4}\u{d06c}\u{b9bd}\u{d2b8}\u{b97c} \u{d3d0}\u{ae30}\u{d588}\u{c2b5}\u{b2c8}\u{b2e4}"
        ),
    }
}

pub const fn update_command_copied(locale: Locale) -> &'static str {
    pick(
        locale,
        "Upgrade command copied to clipboard",
        "\u{5347}\u{7ea7}\u{547d}\u{4ee4}\u{5df2}\u{590d}\u{5236}\u{5230}\u{526a}\u{8d34}\u{677f}",
        "\u{30a2}\u{30c3}\u{30d7}\u{30b0}\u{30ec}\u{30fc}\u{30c9}\u{30b3}\u{30de}\u{30f3}\u{30c9}\u{3092}\u{30b3}\u{30d4}\u{30fc}\u{3057}\u{307e}\u{3057}\u{305f}",
        "\u{c5c5}\u{adf8}\u{b808}\u{c774}\u{b4dc} \u{ba85}\u{b839}\u{c744} \u{d074}\u{b9bd}\u{bcf4}\u{b4dc}\u{c5d0} \u{bcf5}\u{c0ac}\u{d588}\u{c2b5}\u{b2c8}\u{b2e4}",
    )
}

pub const fn refreshing_cloud_entitlement(locale: Locale) -> &'static str {
    pick(
        locale,
        "Refreshing Ruum Cloud entitlement\u{2026}",
        "\u{6b63}\u{5728}\u{5237}\u{65b0} Ruum \u{4e91}\u{7aef}\u{6743}\u{76ca}\u{2026}",
        "Ruum Cloud \u{306e}\u{5229}\u{7528}\u{6a29}\u{9650}\u{3092}\u{66f4}\u{65b0}\u{4e2d}\u{2026}",
        "Ruum Cloud \u{ad8c}\u{d55c} \u{c0c1}\u{d0dc}\u{b97c} \u{c0c8}\u{b85c} \u{ace0}\u{ce58}\u{b294} \u{c911}\u{2026}",
    )
}

pub const fn cloud_entitlement_refreshed(locale: Locale) -> &'static str {
    pick(
        locale,
        "Ruum Cloud entitlement and models updated",
        "\u{5df2}\u{5237}\u{65b0} Ruum \u{4e91}\u{7aef}\u{6743}\u{76ca}\u{4e0e}\u{53ef}\u{7528}\u{6a21}\u{578b}",
        "Ruum Cloud \u{306e}\u{5229}\u{7528}\u{6a29}\u{9650}\u{3068}\u{30e2}\u{30c7}\u{30eb}\u{3092}\u{66f4}\u{65b0}\u{3057}\u{307e}\u{3057}\u{305f}",
        "Ruum Cloud \u{ad8c}\u{d55c} \u{bc0f} \u{baa8}\u{b378} \u{baa9}\u{b85d}\u{c744} \u{c5c5}\u{b370}\u{c774}\u{d2b8}\u{d588}\u{c2b5}\u{b2c8}\u{b2e4}",
    )
}

pub const fn cloud_no_available_models_blocked(locale: Locale) -> &'static str {
    pick(
        locale,
        "Current models require Cloud Pro or a redemption code.",
        "\u{5f53}\u{524d}\u{6a21}\u{578b}\u{9700}\u{5f00}\u{901a}\u{4e91}\u{7aef}\u{4f1a}\u{5458}\u{6216}\u{4f7f}\u{7528}\u{5151}\u{6362}\u{7801}\u{89e3}\u{9501}\u{3002}",
        "\u{73fe}\u{5728}\u{306e}\u{30e2}\u{30c7}\u{30eb}\u{3092}\u{5229}\u{7528}\u{3059}\u{308b}\u{306b}\u{306f}\u{30af}\u{30e9}\u{30a6}\u{30c9}\u{4f1a}\u{54e1}\u{307e}\u{305f}\u{306f}\u{5f15}\u{63db}\u{30b3}\u{30fc}\u{30c9}\u{304c}\u{5fc5}\u{8981}\u{3067}\u{3059}\u{3002}",
        "\u{d604}\u{c7ac} \u{baa8}\u{b378}\u{c744} \u{c0ac}\u{c6a9}\u{d558}\u{b824}\u{ba74} \u{d074}\u{b77c}\u{c6b0}\u{b4dc} \u{d504}\u{b85c} \u{b610}\u{b294} \u{ad50}\u{d658} \u{cf54}\u{b4dc}\u{ac00} \u{d544}\u{c694}\u{d569}\u{b2c8}\u{b2e4}.",
    )
}

pub const fn testing_cloud_connection(locale: Locale) -> &'static str {
    pick(
        locale,
        "Checking Ruum Cloud entitlement and models\u{2026}",
        "\u{6b63}\u{5728}\u{68c0}\u{67e5} Ruum \u{4e91}\u{7aef}\u{6743}\u{76ca}\u{4e0e}\u{6a21}\u{578b}\u{2026}",
        "Ruum Cloud \u{306e}\u{63a5}\u{7d9a}\u{3068}\u{30e2}\u{30c7}\u{30eb}\u{3092}\u{78ba}\u{8a8d}\u{4e2d}\u{2026}",
        "Ruum Cloud \u{c5f0}\u{acb0} \u{bc0f} \u{baa8}\u{b378} \u{d655}\u{c778} \u{c911}\u{2026}",
    )
}

pub const fn cloud_payment_succeeded(locale: Locale) -> &'static str {
    pick(
        locale,
        "Ruum Cloud Pro activated! Available models refreshed.",
        "Ruum \u{4e91}\u{7aef}\u{4f1a}\u{5458}\u{5df2}\u{5f00}\u{901a}\u{ff01}\u{5df2}\u{5237}\u{65b0}\u{53ef}\u{7528}\u{6a21}\u{578b}\u{3002}",
        "Ruum Cloud Pro \u{304c}\u{6709}\u{52b9}\u{5316}\u{3055}\u{308c}\u{307e}\u{3057}\u{305f}\u{ff01}\u{5229}\u{7528}\u{53ef}\u{80fd}\u{306a}\u{30e2}\u{30c7}\u{30eb}\u{3092}\u{66f4}\u{65b0}\u{3057}\u{307e}\u{3057}\u{305f}\u{3002}",
        "Ruum Cloud Pro\u{ac00} \u{d65c}\u{c131}\u{d654}\u{b418}\u{c5c8}\u{c2b5}\u{b2c8}\u{b2e4}! \u{c0ac}\u{c6a9} \u{ac00}\u{b2a5}\u{d55c} \u{baa8}\u{b378}\u{c744} \u{c0c8}\u{b85c}\u{ace0}\u{ce68}\u{d588}\u{c2b5}\u{b2c8}\u{b2e4}.",
    )
}

pub const fn cloud_payment_succeeded_notif_title(
    locale: Locale,
) -> &'static str {
    pick(
        locale,
        "Ruum Cloud Pro Activated",
        "Ruum \u{4e91}\u{7aef}\u{4f1a}\u{5458}\u{5df2}\u{5f00}\u{901a}",
        "Ruum Cloud Pro \u{6709}\u{52b9}\u{5316}\u{5b8c}\u{4e86}",
        "Ruum Cloud Pro \u{d65c}\u{c131}\u{d654}\u{b428}",
    )
}

pub const fn cloud_payment_still_pending(locale: Locale) -> &'static str {
    pick(
        locale,
        "Payment not confirmed yet. Complete payment and try again.",
        "\u{6682}\u{672a}\u{68c0}\u{6d4b}\u{5230}\u{652f}\u{4ed8}\u{5230}\u{8d26}\u{ff0c}\u{8bf7}\u{5b8c}\u{6210}\u{652f}\u{4ed8}\u{540e}\u{518d}\u{8bd5}\u{3002}",
        "\u{6c7a}\u{6e08}\u{304c}\u{307e}\u{3060}\u{78ba}\u{8a8d}\u{3055}\u{308c}\u{3066}\u{3044}\u{307e}\u{305b}\u{3093}\u{3002}\u{652f}\u{6255}\u{3044}\u{5b8c}\u{4e86}\u{5f8c}\u{306b}\u{518d}\u{5ea6}\u{304a}\u{8a66}\u{3057}\u{304f}\u{3060}\u{3055}\u{3044}\u{3002}",
        "\u{c544}\u{c9c1} \u{acb0}\u{c81c}\u{ac00} \u{d655}\u{c778}\u{b418}\u{c9c0} \u{c54a}\u{c558}\u{c2b5}\u{b2c8}\u{b2e4}. \u{acb0}\u{c81c}\u{b97c} \u{c644}\u{b8cc}\u{d55c} \u{d6c4} \u{b2e4}\u{c2dc} \u{c2dc}\u{b3c4}\u{d558}\u{c138}\u{c694}.",
    )
}
