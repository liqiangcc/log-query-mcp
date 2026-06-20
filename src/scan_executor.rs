use std::{io::Read, sync::Arc};

use thiserror::Error;
use tokio::sync::{OwnedSemaphorePermit, Semaphore};
use tokio_util::sync::CancellationToken;

use crate::{ScanError, ScanOutcome, ScanRequest, ScanStopReason, scan_reader};

#[derive(Clone, Debug)]
pub struct ScanExecutor {
    permits: Arc<Semaphore>,
    max_concurrent_scans: usize,
}

impl ScanExecutor {
    pub fn new(max_concurrent_scans: usize) -> Result<Self, ScanTaskError> {
        if max_concurrent_scans == 0 {
            return Err(ScanTaskError::InvalidConcurrency);
        }
        Ok(Self {
            permits: Arc::new(Semaphore::new(max_concurrent_scans)),
            max_concurrent_scans,
        })
    }

    #[must_use]
    pub const fn max_concurrent_scans(&self) -> usize {
        self.max_concurrent_scans
    }

    #[must_use]
    pub fn available_permits(&self) -> usize {
        self.permits.available_permits()
    }

    pub async fn scan<R>(
        &self,
        mut reader: R,
        request: ScanRequest,
    ) -> Result<ScanOutcome, ScanTaskError>
    where
        R: Read + Send + 'static,
    {
        request.validate()?;
        let cancellation = request.cancellation().clone();
        let mut cancel_on_drop = CancelOnDrop::new(cancellation);
        let permit = match self.wait_for_permit(&request).await? {
            PermitWait::Acquired(permit) => permit,
            PermitWait::Stopped(reason) => {
                cancel_on_drop.disarm();
                return Ok(stopped_outcome(reason));
            }
        };

        let task = tokio::task::spawn_blocking(move || {
            let _permit = permit;
            scan_reader(&mut reader, &request)
        });
        let joined = task.await;
        cancel_on_drop.disarm();
        joined.map_err(ScanTaskError::Join)?.map_err(ScanTaskError::Scan)
    }

    async fn wait_for_permit(&self, request: &ScanRequest) -> Result<PermitWait, ScanTaskError> {
        if let Some(reason) = request_stop_reason(request) {
            return Ok(PermitWait::Stopped(reason));
        }

        let cancellation = request.cancellation().clone();
        let acquire = self.permits.clone().acquire_owned();
        if let Some(deadline) = request.deadline {
            tokio::select! {
                biased;
                _ = cancellation.cancelled() => Ok(PermitWait::Stopped(ScanStopReason::Cancelled)),
                _ = tokio::time::sleep_until(tokio::time::Instant::from_std(deadline)) => {
                    Ok(PermitWait::Stopped(ScanStopReason::DeadlineExceeded))
                }
                permit = acquire => permit
                    .map(PermitWait::Acquired)
                    .map_err(|_| ScanTaskError::ExecutorClosed),
            }
        } else {
            tokio::select! {
                biased;
                _ = cancellation.cancelled() => Ok(PermitWait::Stopped(ScanStopReason::Cancelled)),
                permit = acquire => permit
                    .map(PermitWait::Acquired)
                    .map_err(|_| ScanTaskError::ExecutorClosed),
            }
        }
    }
}

#[derive(Debug)]
enum PermitWait {
    Acquired(OwnedSemaphorePermit),
    Stopped(ScanStopReason),
}

fn request_stop_reason(request: &ScanRequest) -> Option<ScanStopReason> {
    if request.cancellation().is_cancelled() {
        Some(ScanStopReason::Cancelled)
    } else if request
        .deadline
        .is_some_and(|deadline| std::time::Instant::now() >= deadline)
    {
        Some(ScanStopReason::DeadlineExceeded)
    } else {
        None
    }
}

fn stopped_outcome(stop_reason: ScanStopReason) -> ScanOutcome {
    ScanOutcome {
        results: Vec::new(),
        bytes_scanned: 0,
        lines_scanned: 0,
        returned_content_bytes: 0,
        stop_reason,
    }
}

#[derive(Debug, Error)]
pub enum ScanTaskError {
    #[error("max_concurrent_scans must be greater than zero")]
    InvalidConcurrency,
    #[error("scan executor is closed")]
    ExecutorClosed,
    #[error("blocking scan task failed")]
    Join(#[source] tokio::task::JoinError),
    #[error(transparent)]
    Scan(#[from] ScanError),
}

#[derive(Debug)]
struct CancelOnDrop {
    cancellation: Option<CancellationToken>,
}

impl CancelOnDrop {
    fn new(cancellation: CancellationToken) -> Self {
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
