use std::{io::Read, sync::Arc};

use thiserror::Error;
use tokio::sync::Semaphore;
use tokio_util::sync::CancellationToken;

use crate::{ScanError, ScanOutcome, ScanRequest, scan_reader};

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
        let permit = self
            .permits
            .clone()
            .acquire_owned()
            .await
            .map_err(|_| ScanTaskError::ExecutorClosed)?;
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
            atomic::{AtomicUsize, Ordering},
        },
        thread,
        time::Duration,
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

        let outcome = timeout(Duration::from_secs(1), scan_task)
            .await
            .expect("cancelled scan should finish promptly")
            .expect("scan task should not panic")
            .expect("scan should return an outcome");
        assert_eq!(outcome.stop_reason, ScanStopReason::Cancelled);
        assert!(outcome.bytes_scanned < limits.max_scan_bytes);
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
