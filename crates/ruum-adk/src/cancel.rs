//! Cooperative cancellation token shared across async HTTP and sync loops.

use std::future::Future;
use std::pin::pin;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};

use futures::future::{Either, select};
use tokio::sync::Notify;

/// Cooperative cancellation handle waking both Tokio futures and sync loops.
#[derive(Clone, Debug, Default)]
pub struct CancelToken {
    cancelled: Arc<AtomicBool>,
    notify: Arc<Notify>,
}

impl CancelToken {
    /// Creates an active, uncancelled token.
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    /// Marks the token cancelled and wakes all awaiting futures.
    pub fn cancel(&self) {
        self.cancelled.store(true, Ordering::Release);
        self.notify.notify_waiters();
    }

    /// Returns `true` once [`Self::cancel`] has been called.
    #[must_use]
    pub fn is_cancelled(&self) -> bool {
        self.cancelled.load(Ordering::Acquire)
    }

    /// Returns the underlying atomic flag for synchronous polling loops.
    #[must_use]
    pub fn flag(&self) -> Arc<AtomicBool> {
        Arc::clone(&self.cancelled)
    }

    /// Completes once cancellation is requested without lost-wakeup races.
    pub async fn cancelled(&self) {
        let notified = self.notify.notified();
        let mut pinned = pin!(notified);
        // Enable waiter registration before inspecting the atomic flag so a
        // concurrent `cancel()` between check and poll cannot be missed.
        pinned.as_mut().enable();
        if self.is_cancelled() {
            return;
        }
        pinned.await;
    }
}

/// Races `fut` against an optional [`CancelToken`], returning `Err(())` on abort.
pub async fn race_cancel<F, T>(
    cancel: Option<&CancelToken>,
    fut: F,
) -> Result<T, ()>
where
    F: Future<Output = T>,
{
    let Some(token) = cancel else {
        return Ok(fut.await);
    };
    if token.is_cancelled() {
        return Err(());
    }
    let cancel_fut = pin!(token.cancelled());
    let work_fut = pin!(fut);
    match select(cancel_fut, work_fut).await {
        Either::Left(((), _)) => Err(()),
        Either::Right((value, _)) => Ok(value),
    }
}
