use std::{sync::Arc, time::Instant};

use thiserror::Error;
use tokio::sync::{OwnedSemaphorePermit, Semaphore};
use tokio_util::sync::CancellationToken;

use crate::MAX_CONCURRENT_SCAN_TASKS;

#[derive(Clone, Debug)]
pub struct ContextExecutor {
    permits: Arc<Semaphore>,
}

impl ContextExecutor {
    pub fn new(max_concurrent_tasks: usize) -> Result<Self, ContextTaskError> {
        if max_concurrent_tasks == 0 || max_concurrent_tasks > MAX_CONCURRENT_SCAN_TASKS {
            return Err(ContextTaskError::InvalidConcurrency);
        }
        Ok(Self {
            permits: Arc::new(Semaphore::new(max_concurrent_tasks)),
        })
    }

    pub async fn execute<T, F>(
        &self,
        cancellation: CancellationToken,
        deadline: Option<Instant>,
        task: F,
    ) -> Result<ContextExecution<T>, ContextTaskError>
    where
        T: Send + 'static,
        F: FnOnce() -> T + Send + 'static,
    {
        let permit = match self
            .wait_for_permit(&cancellation, deadline)
            .await?
        {
            PermitWait::Acquired(permit) => permit,
            PermitWait::Cancelled => return Ok(ContextExecution::Cancelled),
            PermitWait::DeadlineExceeded => return Ok(ContextExecution::DeadlineExceeded),
        };

        let mut cancel_on_drop = CancelOnDrop::new(cancellation);
        let handle = tokio::task::spawn_blocking(move || {
            let _permit = permit;
            task()
        });
        let joined = handle.await;
        cancel_on_drop.disarm();
        joined
            .map(ContextExecution::Complete)
            .map_err(ContextTaskError::Join)
    }

    async fn wait_for_permit(
        &self,
        cancellation: &CancellationToken,
        deadline: Option<Instant>,
    ) -> Result<PermitWait, ContextTaskError> {
        if cancellation.is_cancelled() {
            return Ok(PermitWait::Cancelled);
        }
        if deadline.is_some_and(|deadline| Instant::now() >= deadline) {
            return Ok(PermitWait::DeadlineExceeded);
        }

        let acquire = Arc::clone(&self.permits).acquire_owned();
        if let Some(deadline) = deadline {
            let deadline = tokio::time::Instant::from_std(deadline);
            tokio::select! {
                biased;
                _ = cancellation.cancelled() => Ok(PermitWait::Cancelled),
                _ = tokio::time::sleep_until(deadline) => Ok(PermitWait::DeadlineExceeded),
                permit = acquire => permit
                    .map(PermitWait::Acquired)
                    .map_err(|_| ContextTaskError::ExecutorClosed),
            }
        } else {
            tokio::select! {
                biased;
                _ = cancellation.cancelled() => Ok(PermitWait::Cancelled),
                permit = acquire => permit
                    .map(PermitWait::Acquired)
                    .map_err(|_| ContextTaskError::ExecutorClosed),
            }
        }
    }
}

#[derive(Debug)]
enum PermitWait {
    Acquired(OwnedSemaphorePermit),
    Cancelled,
    DeadlineExceeded,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ContextExecution<T> {
    Complete(T),
    Cancelled,
    DeadlineExceeded,
}

#[derive(Debug, Error)]
pub enum ContextTaskError {
    #[error("max concurrent context tasks must be between 1 and 64")]
    InvalidConcurrency,

    #[error("context executor is closed")]
    ExecutorClosed,

    #[error("blocking context task failed")]
    Join(#[source] tokio::task::JoinError),
}

#[derive(Debug)]
struct CancelOnDrop {
    cancellation: Option<CancellationToken>,
}

impl CancelOnDrop {
    const fn new(cancellation: CancellationToken) -> Self {
        Self {
            cancellation: Some(cancellation),
        }
    }

    fn disarm(&mut self) {
        self.cancellation.take();
    }
}

impl Drop for CancelOnDrop {
    fn drop(&mut self) {
        if let Some(cancellation) = self.cancellation.take() {
            cancellation.cancel();
        }
    }
}

#[cfg(test)]
mod tests {
    use std::{
        sync::{
            Arc,
            atomic::{AtomicBool, Ordering},
        },
        thread,
        time::Duration,
    };

    use tokio::time::timeout;

    use super::*;

    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn returns_before_start_for_pre_cancelled_request() {
        let executor = ContextExecutor::new(1).expect("executor should build");
        let cancellation = CancellationToken::new();
        cancellation.cancel();

        let outcome = executor
            .execute(cancellation, None, || 42)
            .await
            .expect("execution should return a stop outcome");
        assert_eq!(outcome, ContextExecution::Cancelled);
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn dropping_future_cancels_running_task_and_releases_permit() {
        let executor = ContextExecutor::new(1).expect("executor should build");
        let cancellation = CancellationToken::new();
        let started = Arc::new(AtomicBool::new(false));
        let task = {
            let executor = executor.clone();
            let cancellation = cancellation.clone();
            let started = Arc::clone(&started);
            tokio::spawn(async move {
                executor
                    .execute(cancellation.clone(), None, move || {
                        started.store(true, Ordering::SeqCst);
                        while !cancellation.is_cancelled() {
                            thread::sleep(Duration::from_millis(1));
                        }
                    })
                    .await
            })
        };

        timeout(Duration::from_secs(1), async {
            while !started.load(Ordering::SeqCst) {
                tokio::task::yield_now().await;
            }
        })
        .await
        .expect("blocking task should start");
        task.abort();
        timeout(Duration::from_secs(1), cancellation.cancelled())
            .await
            .expect("dropping the future should cancel the token");

        let outcome = timeout(
            Duration::from_secs(1),
            executor.execute(CancellationToken::new(), None, || 7),
        )
        .await
        .expect("permit should be released")
        .expect("execution should succeed");
        assert_eq!(outcome, ContextExecution::Complete(7));
    }
}
