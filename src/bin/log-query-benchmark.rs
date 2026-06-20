use std::{
    collections::BTreeMap,
    env,
    ffi::OsString,
    fs::File,
    path::{Path, PathBuf},
    sync::{Arc, Barrier},
    thread,
    time::{Duration, Instant},
};

use anyhow::{Context, Result, bail};
use log_query_mcp::{ScanLimits, ScanRequest, scan_reader};
use serde_json::json;

const DEFAULT_ITERATIONS: usize = 3;
const DEFAULT_CONCURRENCY: usize = 1;
const MAX_CONCURRENCY: usize = 64;

fn main() -> Result<()> {
    let mut args = env::args_os().skip(1);
    let path = PathBuf::from(next_arg(&mut args, "missing log file path")?);
    let keyword = next_arg(&mut args, "missing literal search keyword")?
        .into_string()
        .map_err(|_| anyhow::anyhow!("keyword must be valid UTF-8"))?;
    let iterations = parse_positive_argument(
        args.next(),
        DEFAULT_ITERATIONS,
        "iterations must be a positive integer",
    )?;
    let concurrency = parse_positive_argument(
        args.next(),
        DEFAULT_CONCURRENCY,
        "concurrency must be a positive integer",
    )?;
    if concurrency > MAX_CONCURRENCY {
        bail!("concurrency cannot exceed {MAX_CONCURRENCY}");
    }
    if args.next().is_some() {
        bail!(
            "usage: log-query-benchmark <log-file> <keyword> [iterations-per-worker] [concurrency]"
        );
    }

    let file_bytes = std::fs::metadata(&path)
        .with_context(|| format!("cannot read metadata for {}", path.display()))?
        .len();
    let limits = ScanLimits {
        max_scan_bytes: file_bytes.saturating_add(1).max(1),
        ..ScanLimits::default()
    };

    let barrier = Arc::new(Barrier::new(concurrency.saturating_add(1)));
    let mut workers = Vec::with_capacity(concurrency);
    for worker_index in 0..concurrency {
        let worker_path = path.clone();
        let worker_keyword = keyword.clone();
        let worker_barrier = Arc::clone(&barrier);
        workers.push(
            thread::Builder::new()
                .name(format!("log-query-benchmark-{worker_index}"))
                .spawn(move || {
                    run_worker(
                        &worker_path,
                        &worker_keyword,
                        iterations,
                        limits,
                        &worker_barrier,
                    )
                })
                .context("cannot spawn benchmark worker")?,
        );
    }

    let benchmark_start = Instant::now();
    barrier.wait();

    let mut total_bytes_scanned = 0_u64;
    let mut total_results = 0_usize;
    let mut total_worker_elapsed = Duration::ZERO;
    let mut stop_reasons = BTreeMap::<String, usize>::new();
    for worker in workers {
        let report = worker
            .join()
            .map_err(|_| anyhow::anyhow!("benchmark worker panicked"))??;
        total_bytes_scanned = total_bytes_scanned.saturating_add(report.bytes_scanned);
        total_results = total_results.saturating_add(report.results);
        total_worker_elapsed = total_worker_elapsed.saturating_add(report.elapsed);
        for (reason, count) in report.stop_reasons {
            *stop_reasons.entry(reason).or_default() += count;
        }
    }

    let wall_elapsed = benchmark_start.elapsed();
    let wall_seconds = wall_elapsed.as_secs_f64();
    let total_scans = iterations.saturating_mul(concurrency);
    let aggregate_throughput_mib_per_second = if wall_seconds > 0.0 {
        total_bytes_scanned as f64 / (1024.0 * 1024.0) / wall_seconds
    } else {
        0.0
    };
    let mean_scan_milliseconds = if total_scans > 0 {
        total_worker_elapsed.as_secs_f64() * 1000.0 / total_scans as f64
    } else {
        0.0
    };

    let report = json!({
        "file_bytes": file_bytes,
        "iterations_per_worker": iterations,
        "concurrency": concurrency,
        "total_scans": total_scans,
        "keyword_bytes": keyword.len(),
        "total_bytes_scanned": total_bytes_scanned,
        "total_results": total_results,
        "wall_elapsed_milliseconds": wall_elapsed.as_secs_f64() * 1000.0,
        "aggregate_throughput_mib_per_second": aggregate_throughput_mib_per_second,
        "mean_scan_milliseconds": mean_scan_milliseconds,
        "stop_reasons": stop_reasons,
        "note": "Use a non-matching or rare keyword to measure full-file throughput. External tools are still required for peak RSS, CPU and disk metrics."
    });
    println!("{}", serde_json::to_string_pretty(&report)?);

    Ok(())
}

fn run_worker(
    path: &Path,
    keyword: &str,
    iterations: usize,
    limits: ScanLimits,
    barrier: &Barrier,
) -> Result<WorkerReport> {
    barrier.wait();
    let started = Instant::now();
    let mut bytes_scanned = 0_u64;
    let mut results = 0_usize;
    let mut stop_reasons = BTreeMap::<String, usize>::new();

    for _ in 0..iterations {
        let mut file = File::open(path)
            .with_context(|| format!("cannot open benchmark file {}", path.display()))?;
        let request = ScanRequest::new(keyword.to_owned()).with_limits(limits);
        let outcome = scan_reader(&mut file, &request).context("scanner benchmark failed")?;
        bytes_scanned = bytes_scanned.saturating_add(outcome.bytes_scanned);
        results = results.saturating_add(outcome.results.len());
        *stop_reasons
            .entry(format!("{:?}", outcome.stop_reason))
            .or_default() += 1;
    }

    Ok(WorkerReport {
        bytes_scanned,
        results,
        elapsed: started.elapsed(),
        stop_reasons,
    })
}

#[derive(Debug)]
struct WorkerReport {
    bytes_scanned: u64,
    results: usize,
    elapsed: Duration,
    stop_reasons: BTreeMap<String, usize>,
}

fn parse_positive_argument(
    value: Option<OsString>,
    default: usize,
    message: &'static str,
) -> Result<usize> {
    let value = match value {
        Some(value) => value
            .into_string()
            .map_err(|_| anyhow::anyhow!(message))?
            .parse::<usize>()
            .with_context(|| message)?,
        None => default,
    };
    if value == 0 {
        bail!(message);
    }
    Ok(value)
}

fn next_arg(args: &mut impl Iterator<Item = OsString>, message: &'static str) -> Result<OsString> {
    args.next().ok_or_else(|| anyhow::anyhow!(message))
}
