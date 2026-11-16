//! One replaceable progress snapshot, never a queue of filesystem events.
use std::path::PathBuf;
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use crate::i18n::Locale;

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum Stage {
    #[default]
    Connecting,
    CheckingHelper,
    UploadingHelper,
    Starting,
    Indexing,
    Finalizing,
}

impl Stage {
    pub const fn label(self) -> &'static str {
        self.label_in(Locale::En)
    }

    /// Localized progress stage label.
    #[must_use]
    pub const fn label_in(self, locale: Locale) -> &'static str {
        match (self, locale) {
            (Self::Connecting, Locale::En) => "Connecting over SSH",
            (Self::Connecting, Locale::Zh) => "正在通过 SSH 连接",
            (Self::Connecting, Locale::Ja) => "SSH で接続中",
            (Self::Connecting, Locale::Ko) => "SSH로 연결 중",
            (Self::CheckingHelper, Locale::En) => "Checking remote scanner",
            (Self::CheckingHelper, Locale::Zh) => "正在检查远程扫描器",
            (Self::CheckingHelper, Locale::Ja) => "リモートスキャナを確認中",
            (Self::CheckingHelper, Locale::Ko) => "원격 스캐너 확인 중",
            (Self::UploadingHelper, Locale::En) => "Uploading remote scanner",
            (Self::UploadingHelper, Locale::Zh) => "正在上传远程扫描器",
            (Self::UploadingHelper, Locale::Ja) => "リモートスキャナを送信中",
            (Self::UploadingHelper, Locale::Ko) => "원격 스캐너 업로드 중",
            (Self::Starting, Locale::En) => "Starting remote scanner",
            (Self::Starting, Locale::Zh) => "正在启动远程扫描器",
            (Self::Starting, Locale::Ja) => "リモートスキャナを起動中",
            (Self::Starting, Locale::Ko) => "원격 스캐너 시작 중",
            (Self::Indexing, Locale::En) => "Indexing remotely",
            (Self::Indexing, Locale::Zh) => "正在远程建立索引",
            (Self::Indexing, Locale::Ja) => "リモートでインデックス作成中",
            (Self::Indexing, Locale::Ko) => "원격 인덱싱 중",
            (Self::Finalizing, Locale::En) => "Building the treemap",
            (Self::Finalizing, Locale::Zh) => "正在构建矩形树图",
            (Self::Finalizing, Locale::Ja) => "ツリーマップを構築中",
            (Self::Finalizing, Locale::Ko) => "트리맵 생성 중",
        }
    }
}

#[derive(Clone, Debug, Default)]
pub struct Progress {
    pub stage: Stage,
    pub files: u64,
    pub dirs: u64,
    pub bytes: u64,
    pub errors: u64,
    pub directory: Option<PathBuf>,
    pub updated: Option<Instant>,
    pub preview: Option<Arc<crate::scan_preview::Preview>>,
}

/// Cap both wire heartbeats and UI publications at five per second.
pub(crate) const INTERVAL: Duration = Duration::from_millis(200);
pub(crate) type Shared = Arc<Mutex<Progress>>;

pub(crate) fn stage(shared: &Shared, value: Stage) {
    let mut progress = shared.lock().expect("remote progress lock");
    progress.stage = value;
    progress.updated = Some(Instant::now());
}
