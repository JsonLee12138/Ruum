//! Localized toast notices and status feedback strings for `state.rs`.

use ruum_core::watchdog::ServiceStatus;

use crate::i18n::{Locale, path_count, pick};

pub fn failed_save_settings(err: &str, locale: Locale) -> String {
    match locale {
        Locale::En => format!("Failed to save settings: {err}"),
        Locale::Zh => format!("保存设置失败：{err}"),
        Locale::Ja => format!("設定の保存に失敗しました: {err}"),
        Locale::Ko => format!("설정 저장 실패: {err}"),
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

pub const fn settings_saved_updating_service(locale: Locale) -> &'static str {
    pick(
        locale,
        "Settings saved; updating background service\u{2026}",
        "设置已保存；正在更新后台服务\u{2026}",
        "設定を保存しました。バックグラウンドサービスを更新中\u{2026}",
        "설정이 저장되었습니다. 백그라운드 서비스를 업데이트하는 중\u{2026}",
    )
}

pub const fn service_note(
    status: Option<ServiceStatus>,
    locale: Locale,
) -> &'static str {
    match status {
        Some(ServiceStatus::Active) => pick(
            locale,
            "Service: Active",
            "服务：运行中",
            "サービス: 有効",
            "서비스: 활성",
        ),
        Some(ServiceStatus::Inactive) => pick(
            locale,
            "Service: Inactive",
            "服务：已停止",
            "サービス: 停止",
            "서비스: 비활성",
        ),
        Some(ServiceStatus::NotInstalled) | None => pick(
            locale,
            "Service: Off",
            "服务：关闭",
            "サービス: オフ",
            "서비스: 꺼짐",
        ),
        Some(ServiceStatus::Unsupported) => pick(
            locale,
            "Service: Unsupported",
            "服务：不支持",
            "サービス: 非対応",
            "서비스: 지원 안 됨",
        ),
    }
}

pub fn settings_saved_with_service(note: &str, locale: Locale) -> String {
    match locale {
        Locale::En => format!("Settings saved ({note})"),
        Locale::Zh => format!("设置已保存（{note}）"),
        Locale::Ja => format!("設定を保存しました（{note}）"),
        Locale::Ko => format!("설정이 저장되었습니다({note})"),
    }
}

pub fn settings_saved_service_error(err: &str, locale: Locale) -> String {
    match locale {
        Locale::En => format!("Settings saved; service error: {err}"),
        Locale::Zh => format!("设置已保存；后台服务错误：{err}"),
        Locale::Ja => format!("設定を保存しました（サービスエラー: {err}）"),
        Locale::Ko => format!("설정이 저장되었습니다(서비스 오류: {err})"),
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

pub fn disk_checked_below(
    free: &str,
    total: &str,
    min_gb: u64,
    locale: Locale,
) -> String {
    match locale {
        Locale::En => format!(
            "Checked: {free} free of {total} \u{2014} below {min_gb} GB \
             threshold"
        ),
        Locale::Zh => format!(
            "检查完成：可用 {free} / 共 {total} \u{2014} 低于 {min_gb} GB 阈值"
        ),
        Locale::Ja => format!(
            "確認済み: 空き {free} / {total} \u{2014} {min_gb} GB しきい値未満"
        ),
        Locale::Ko => format!(
            "확인됨: 여유 {free} / {total} \u{2014} {min_gb} GB 임계값 미만"
        ),
    }
}

pub fn disk_checked_healthy(
    free: &str,
    total: &str,
    min_gb: u64,
    locale: Locale,
) -> String {
    match locale {
        Locale::En => format!(
            "Checked: {free} free of {total} \u{2014} healthy (above \
             {min_gb} GB threshold)"
        ),
        Locale::Zh => format!(
            "检查完成：可用 {free} / 共 {total} \u{2014} 健康（高于 \
             {min_gb} GB 阈值）"
        ),
        Locale::Ja => format!(
            "確認済み: 空き {free} / {total} \u{2014} 正常（{min_gb} GB \
             しきい値以上）"
        ),
        Locale::Ko => format!(
            "확인됨: 여유 {free} / {total} \u{2014} 정상({min_gb} GB \
             임계값 이상)"
        ),
    }
}

pub fn failed_check_disk_space(err: &str, locale: Locale) -> String {
    match locale {
        Locale::En => format!("Failed to check disk space: {err}"),
        Locale::Zh => format!("检查磁盘空间失败：{err}"),
        Locale::Ja => format!("ディスク空き容量の確認に失敗しました: {err}"),
        Locale::Ko => format!("디스크 공간 확인 실패: {err}"),
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
    match locale {
        Locale::En => format!("AI analysis error: {err}"),
        Locale::Zh => format!("AI 分析出错：{err}"),
        Locale::Ja => format!("AI 分析エラー: {err}"),
        Locale::Ko => format!("AI 분석 오류: {err}"),
    }
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
