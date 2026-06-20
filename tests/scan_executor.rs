use std::{
    io::{Cursor, Read},
    sync::{
        Arc, Barrier,
        atomic::{AtomicBool, AtomicUsize, Ordering},
    },
    thread,
    time::{Duration, Instant},
};

use log_query_mcp::{ScanExecutor, ScanLimits, ScanRequest, ScanStopReason, ScanTaskError};
use tokio::time::timeout;
use tokio_util::sync::CancellationToken;

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
    assert_eq!(executor.available_permits(), 2);
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn running_scan_observes_cancellation() {
    let executor = ScanExecutor::new(1).expect("executor should be created");
    let cancellation = CancellationToken::new();
    let request = ScanRequest::new("never")
        .with_limits(ScanLimits {
            max_scan_bytes: 64 * 1024 * 1024,
            read_buffer_bytes: 1024,
            ..ScanLimits::default()
        })
        .with_cancellation(cancellation.clone());
    let task = tokio::spawn(async move {
        executor
            .scan(SlowReader { reads_left: 10_000 }, request)
            .await
    });

    tokio::time::sleep(Duration::from_millis(30)).await;
    cancellation.cancel();
    let outcome = timeout(Duration::from_millis(500), task)
        .await
        .expect("cancelled scan should finish")
        .expect("task should not panic")
        .expect("scan should return an outcome");

    assert_eq!(outcome.stop_reason, ScanStopReason::Cancelled);
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn queued_cancellation_never_starts_reader() {
    let executor = ScanExecutor::new(1).expect("executor should be created");
    let first_cancel = CancellationToken::new();
    let first_started = Arc::new(AtomicBool::new(false));
    let first = {
        let executor = executor.clone();
        let started = first_started.clone();
        let reader_cancel = first_cancel.clone();
        let request = ScanRequest::new("never").with_cancellation(first_cancel.clone());
        tokio::spawn(async move {
            executor
                .scan(
                    BlockingUntilCancelledReader {
                        started,
                        cancellation: reader_cancel,
                    },
                    request,
                )
                .await
        })
    };
    wait_until_true(&first_started).await;

    let second_cancel = CancellationToken::new();
    let reads = Arc::new(AtomicUsize::new(0));
    let second = {
        let executor = executor.clone();
        let reads = reads.clone();
        let request = ScanRequest::new("MATCH").with_cancellation(second_cancel.clone());
        tokio::spawn(async move { executor.scan(CountingReader { reads }, request).await })
    };

    tokio::time::sleep(Duration::from_millis(20)).await;
    second_cancel.cancel();
    let outcome = timeout(Duration::from_millis(500), second)
        .await
        .expect("queued cancellation should finish")
        .expect("task should not panic")
        .expect("scan should return an outcome");
    assert_eq!(outcome.stop_reason, ScanStopReason::Cancelled);
    assert_eq!(reads.load(Ordering::SeqCst), 0);

    first_cancel.cancel();
    finish_first(first).await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn queued_deadline_never_starts_reader() {
    let executor = ScanExecutor::new(1).expect("executor should be created");
    let first_cancel = CancellationToken::new();
    let first_started = Arc::new(AtomicBool::new(false));
    let first = {
        let executor = executor.clone();
        let started = first_started.clone();
        let reader_cancel = first_cancel.clone();
        let request = ScanRequest::new("never").with_cancellation(first_cancel.clone());
        tokio::spawn(async move {
            executor
                .scan(
                    BlockingUntilCancelledReader {
                        started,
                        cancellation: reader_cancel,
                    },
                    request,
                )
                .await
        })
    };
    wait_until_true(&first_started).await;

    let reads = Arc::new(AtomicUsize::new(0));
    let outcome = timeout(
        Duration::from_millis(500),
        executor.scan(
            CountingReader {
                reads: reads.clone(),
            },
            ScanRequest::new("MATCH")
                .with_deadline(Instant::now() + Duration::from_millis(30)),
        ),
    )
    .await
    .expect("queued deadline should finish")
    .expect("scan should return an outcome");
    assert_eq!(outcome.stop_reason, ScanStopReason::DeadlineExceeded);
    assert_eq!(reads.load(Ordering::SeqCst), 0);

    first_cancel.cancel();
    finish_first(first).await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn aborting_future_cancels_blocking_task_and_releases_permit() {
    let executor = ScanExecutor::new(1).expect("executor should be created");
    let cancellation = CancellationToken::new();
    let started = Arc::new(AtomicBool::new(false));
    let task = {
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

    task.abort();
    timeout(Duration::from_millis(500), cancellation.cancelled())
        .await
        .expect("dropping future should cancel the token");

    let outcome = timeout(
        Duration::from_secs(1),
        executor.scan(
            Cursor::new(b"MATCH\n".to_vec()),
            ScanRequest::new("MATCH"),
        ),
    )
    .await
    .expect("permit should be released")
    .expect("second scan should succeed");
    assert_eq!(outcome.results.len(), 1);
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn semaphore_limits_concurrent_scans() {
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
            .expect("task should not panic")
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

async fn finish_first(
    task: tokio::task::JoinHandle<Result<log_query_mcp::ScanOutcome, ScanTaskError>>,
) {
    timeout(Duration::from_millis(500), task)
        .await
        .expect("first scan should stop")
        .expect("task should not panic")
        .expect("scan should return an outcome");
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
        let count = buffer.len().min(1024);
        buffer[..count].fill(b'x');
        self.reads_left -= 1;
        Ok(count)
    }
}

struct SignallingSlowReader {
    started: Arc<AtomicBool>,
    reads_left: usize,
}

impl Read for SignallingSlowReader {
    fn read(&mut self, buffer: &mut [u8]) -> std::io::Result<usize> {
        self.started.store(true, Ordering::SeqCst);
        SlowReader {
            reads_left: self.reads_left,
        }
        .read(buffer)
        .inspect(|_| self.reads_left = self.reads_left.saturating_sub(1))
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
