//! One replaceable progress snapshot, never a queue of filesystem events.
use std::path::PathBuf;
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

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
        match self {
            Self::Connecting => "Connecting over SSH",
            Self::CheckingHelper => "Checking remote scanner",
            Self::UploadingHelper => "Uploading remote scanner",
            Self::Starting => "Starting remote scanner",
            Self::Indexing => "Indexing remotely",
            Self::Finalizing => "Building the treemap",
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
