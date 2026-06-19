use std::{io::Read, sync::Arc};

use thiserror::Error;
use tokio::sync::{OwnedSemaphorePermit, Semaphore};
use tokio_util::sync::CancellationToken;

use crate::{ScanError, ScanOutcome, ScanRequest, ScanStopReason, scan_reader};

#[derive(Clone)]
pub struct ScanExecutor {
    permits: Arc<Semaphore>,
}

impl ScanExecutor {
    pub fn new(max_concurrent_scans: usize) -> Result<Self, ScanTaskError> {
        if max_concurrent_scans == 0 {
            return Err(ScanTaskError::InvalidConcurrency);
        }

        Ok(Self {
            permits: Arc::new(Semaphore::new(max_concurrent_scans)),
        })
    }

    pub async fn scan<R>(
        &self,
        mut reader: R,
        request: ScanRequest,
    ) -> Result<ScanOutcome, ScanTaskError>
    where
        R: Read + Send + 'static,
    {
        let permit = match self.wait_for_permit(&request).await? {
            PermitWait::Acquired(permit) => permit,
            PermitWait::Stopped(reason) => return Ok(stopped_outcome(reason)),
        };
        let cancellation = request.cancellation().clone();
        let mut cancel_on_drop = CancelOnDrop::new(cancellation);

        let task = tokio::task::spawn_blocking(move || {
            let _permit = permit;
            scan_reader(&mut reader, &request)
        });

        let joined = task.await;
        cancel_on_drop.disarm();
        let scan_result = joined.map_err(ScanTaskError::Join)?;
        scan_result.map_err(ScanTaskError::Scan)
    }

    async fn wait_for_permit(&self, request: &ScanRequest) -> Result<PermitWait, ScanTaskError> {
        if let Some(reason) = request_stop_reason(request) {
            return Ok(PermitWait::Stopped(reason));
        }

        let cancellation = request.cancellation().clone();
        let acquire = self.permits.clone().acquire_owned();

        if let Some(deadline) = request.deadline {
            let deadline = tokio::time::Instant::from_std(deadline);
            tokio::select! {
                biased;
                _ = cancellation.cancelled() => Ok(PermitWait::Stopped(ScanStopReason::Cancelled)),
                _ = tokio::time::sleep_until(deadline) => {
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
        return Some(ScanStopReason::Cancelled);
    }
    if request
        .deadline
        .is_some_and(|deadline| std::time::Instant::now() >= deadline)
    {
        return Some(ScanStopReason::DeadlineExceeded);
    }
    None
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

#[cfg(test)]
mod tests {
    use std::{
        io::{Cursor, Read},
        sync::{
            Arc, Barrier,
            atomic::{AtomicBool, AtomicUsize, Ordering},
        },
        thread,
        time::{Duration, Instant},
    };

    use tokio::time::timeout;

    use crate::{ScanLimits, ScanStopReason};

    use super::*;

    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn scans_on_blocking_executor() {
        let executor = ScanExecutor::new(2).expect("executor should be created");
        let outcome = executor
            .scan(
                Cursor::new(b"traceId=abc123\n".to_vec()),
                ScanRequest::new("abc123"),
            )
            .await
            .expect("scan should succeed");

        assert_eq!(outcome.results.len(), 1);
        assert_eq!(outcome.stop_reason, ScanStopReason::Complete);
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn cancellation_interrupts_cooperative_scan() {
        let executor = ScanExecutor::new(1).expect("executor should be created");
        let cancellation = CancellationToken::new();
        let limits = ScanLimits {
            max_scan_bytes: 64 * 1024 * 1024,
            read_buffer_bytes: 1024,
            ..ScanLimits::default()
        };
        let request = ScanRequest::new("never")
            .with_limits(limits)
            .with_cancellation(cancellation.clone());
        let scan_task = tokio::spawn(async move {
            executor
                .scan(SlowReader { reads_left: 10_000 }, request)
                .await
        });

        tokio::time::sleep(Duration::from_millis(30)).await;
        cancellation.cancel();

        let outcome = timeout(Duration::from_millis(500), scan_task)
            .await
            .expect("cancelled scan should finish promptly")
            .expect("scan task should not panic")
            .expect("scan should return an outcome");
        assert_eq!(outcome.stop_reason, ScanStopReason::Cancelled);
        assert!(outcome.bytes_scanned < limits.max_scan_bytes);
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn queued_cancellation_returns_without_starting_reader() {
        let executor = ScanExecutor::new(1).expect("executor should be created");
        let first_cancellation = CancellationToken::new();
        let first_started = Arc::new(AtomicBool::new(false));
        let first_task = {
            let executor = executor.clone();
            let first_started = first_started.clone();
            let first_cancellation_for_reader = first_cancellation.clone();
            let request = ScanRequest::new("never").with_cancellation(first_cancellation.clone());
            tokio::spawn(async move {
                executor
                    .scan(
                        BlockingUntilCancelledReader {
                            started: first_started,
                            cancellation: first_cancellation_for_reader,
                        },
                        request,
                    )
                    .await
            })
        };
        wait_until_true(&first_started).await;

        let second_cancellation = CancellationToken::new();
        let second_reads = Arc::new(AtomicUsize::new(0));
        let second_task = {
            let executor = executor.clone();
            let second_reads = second_reads.clone();
            let request = ScanRequest::new("MATCH").with_cancellation(second_cancellation.clone());
            tokio::spawn(async move {
                executor
                    .scan(
                        CountingReader {
                            reads: second_reads,
                        },
                        request,
                    )
                    .await
            })
        };

        tokio::time::sleep(Duration::from_millis(20)).await;
        second_cancellation.cancel();
        let outcome = timeout(Duration::from_millis(500), second_task)
            .await
            .expect("queued cancellation should return promptly")
            .expect("queued scan task should not panic")
            .expect("queued scan should return an outcome");
        assert_eq!(outcome.stop_reason, ScanStopReason::Cancelled);
        assert_eq!(second_reads.load(Ordering::SeqCst), 0);

        first_cancellation.cancel();
        timeout(Duration::from_millis(500), first_task)
            .await
            .expect("first scan should stop")
            .expect("first scan task should not panic")
            .expect("first scan should return an outcome");
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn queued_deadline_expires_without_starting_reader() {
        let executor = ScanExecutor::new(1).expect("executor should be created");
        let first_cancellation = CancellationToken::new();
        let first_started = Arc::new(AtomicBool::new(false));
        let first_task = {
            let executor = executor.clone();
            let first_started = first_started.clone();
            let first_cancellation_for_reader = first_cancellation.clone();
            let request = ScanRequest::new("never").with_cancellation(first_cancellation.clone());
            tokio::spawn(async move {
                executor
                    .scan(
                        BlockingUntilCancelledReader {
                            started: first_started,
                            cancellation: first_cancellation_for_reader,
                        },
                        request,
                    )
                    .await
            })
        };
        wait_until_true(&first_started).await;

        let second_reads = Arc::new(AtomicUsize::new(0));
        let request =
            ScanRequest::new("MATCH").with_deadline(Instant::now() + Duration::from_millis(30));
        let outcome = timeout(
            Duration::from_millis(500),
            executor.scan(
                CountingReader {
                    reads: second_reads.clone(),
                },
                request,
            ),
        )
        .await
        .expect("queued deadline should return promptly")
        .expect("queued scan should return an outcome");

        assert_eq!(outcome.stop_reason, ScanStopReason::DeadlineExceeded);
        assert_eq!(second_reads.load(Ordering::SeqCst), 0);

        first_cancellation.cancel();
        timeout(Duration::from_millis(500), first_task)
            .await
            .expect("first scan should stop")
            .expect("first scan task should not panic")
            .expect("first scan should return an outcome");
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn aborting_async_scan_cancels_blocking_task_and_releases_permit() {
        let executor = ScanExecutor::new(1).expect("executor should be created");
        let cancellation = CancellationToken::new();
        let started = Arc::new(AtomicBool::new(false));
        let scan_task = {
            let executor = executor.clone();
            let started = started.clone();
            let request = ScanRequest::new("never").with_cancellation(cancellation.clone());
            tokio::spawn(async move {
                executor
                    .scan(
                        SignallingSlowReader {
                            started,
                            reads_left: 10_000,
                        },
                        request,
                    )
                    .await
            })
        };
        wait_until_true(&started).await;

        scan_task.abort();
        timeout(Duration::from_millis(500), cancellation.cancelled())
            .await
            .expect("dropping scan future should cancel its token");

        let outcome = timeout(
            Duration::from_secs(1),
            executor.scan(Cursor::new(b"MATCH\n".to_vec()), ScanRequest::new("MATCH")),
        )
        .await
        .expect("permit should be released after cooperative cancellation")
        .expect("second scan should succeed");
        assert_eq!(outcome.results.len(), 1);
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 4)]
    async fn concurrency_is_limited_by_semaphore() {
        let executor = ScanExecutor::new(2).expect("executor should be created");
        let active = Arc::new(AtomicUsize::new(0));
        let maximum = Arc::new(AtomicUsize::new(0));
        let barrier = Arc::new(Barrier::new(2));
        let mut tasks = Vec::new();

        for _ in 0..4 {
            let reader = TrackingReader {
                cursor: Cursor::new(b"MATCH\n".to_vec()),
                active: active.clone(),
                maximum: maximum.clone(),
                barrier: barrier.clone(),
                started: false,
            };
            let executor = executor.clone();
            tasks.push(tokio::spawn(async move {
                executor.scan(reader, ScanRequest::new("MATCH")).await
            }));
        }

        for task in tasks {
            task.await
                .expect("scan task should not panic")
                .expect("scan should succeed");
        }

        assert_eq!(maximum.load(Ordering::SeqCst), 2);
        assert_eq!(active.load(Ordering::SeqCst), 0);
    }

    #[test]
    fn rejects_zero_concurrency() {
        assert!(matches!(
            ScanExecutor::new(0),
            Err(ScanTaskError::InvalidConcurrency)
        ));
    }

    async fn wait_until_true(value: &AtomicBool) {
        timeout(Duration::from_millis(500), async {
            while !value.load(Ordering::SeqCst) {
                tokio::time::sleep(Duration::from_millis(5)).await;
            }
        })
        .await
        .expect("reader should start");
    }

    struct SlowReader {
        reads_left: usize,
    }

    impl Read for SlowReader {
        fn read(&mut self, buffer: &mut [u8]) -> std::io::Result<usize> {
            if self.reads_left == 0 || buffer.is_empty() {
                return Ok(0);
            }

            thread::sleep(Duration::from_millis(10));
            let bytes_read = buffer.len().min(1024);
            buffer[..bytes_read].fill(b'x');
            self.reads_left -= 1;
            Ok(bytes_read)
        }
    }

    struct SignallingSlowReader {
        started: Arc<AtomicBool>,
        reads_left: usize,
    }

    impl Read for SignallingSlowReader {
        fn read(&mut self, buffer: &mut [u8]) -> std::io::Result<usize> {
            self.started.store(true, Ordering::SeqCst);
            if self.reads_left == 0 || buffer.is_empty() {
                return Ok(0);
            }

            thread::sleep(Duration::from_millis(10));
            let bytes_read = buffer.len().min(1024);
            buffer[..bytes_read].fill(b'x');
            self.reads_left -= 1;
            Ok(bytes_read)
        }
    }

    struct BlockingUntilCancelledReader {
        started: Arc<AtomicBool>,
        cancellation: CancellationToken,
    }

    impl Read for BlockingUntilCancelledReader {
        fn read(&mut self, _buffer: &mut [u8]) -> std::io::Result<usize> {
            self.started.store(true, Ordering::SeqCst);
            while !self.cancellation.is_cancelled() {
                thread::sleep(Duration::from_millis(5));
            }
            Err(std::io::Error::from(std::io::ErrorKind::Interrupted))
        }
    }

    struct CountingReader {
        reads: Arc<AtomicUsize>,
    }

    impl Read for CountingReader {
        fn read(&mut self, _buffer: &mut [u8]) -> std::io::Result<usize> {
            self.reads.fetch_add(1, Ordering::SeqCst);
            Ok(0)
        }
    }

    struct TrackingReader {
        cursor: Cursor<Vec<u8>>,
        active: Arc<AtomicUsize>,
        maximum: Arc<AtomicUsize>,
        barrier: Arc<Barrier>,
        started: bool,
    }

    impl Read for TrackingReader {
        fn read(&mut self, buffer: &mut [u8]) -> std::io::Result<usize> {
            if !self.started {
                self.started = true;
                let active = self.active.fetch_add(1, Ordering::SeqCst) + 1;
                self.maximum.fetch_max(active, Ordering::SeqCst);
                self.barrier.wait();
                thread::sleep(Duration::from_millis(20));
            }

            self.cursor.read(buffer)
        }
    }

    impl Drop for TrackingReader {
        fn drop(&mut self) {
            if self.started {
                self.active.fetch_sub(1, Ordering::SeqCst);
            }
        }
    }
}
