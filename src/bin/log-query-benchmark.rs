use std::{env, ffi::OsString, fs::File, path::PathBuf, time::Instant};

use anyhow::{Context, Result, bail};
use log_query_mcp::{ScanLimits, ScanRequest, scan_reader};
use serde_json::json;

const DEFAULT_ITERATIONS: usize = 3;

fn main() -> Result<()> {
    let mut args = env::args_os().skip(1);
    let path = PathBuf::from(next_arg(&mut args, "missing log file path")?);
    let keyword = next_arg(&mut args, "missing literal search keyword")?
        .into_string()
        .map_err(|_| anyhow::anyhow!("keyword must be valid UTF-8"))?;
    let iterations = match args.next() {
        Some(value) => value
            .into_string()
            .map_err(|_| anyhow::anyhow!("iterations must be valid UTF-8"))?
            .parse::<usize>()
            .context("iterations must be a positive integer")?,
        None => DEFAULT_ITERATIONS,
    };
    if iterations == 0 {
        bail!("iterations must be greater than zero");
    }
    if args.next().is_some() {
        bail!("usage: log-query-benchmark <log-file> <keyword> [iterations]");
    }

    let file_bytes = std::fs::metadata(&path)
        .with_context(|| format!("cannot read metadata for {}", path.display()))?
        .len();
    let limits = ScanLimits {
        max_scan_bytes: file_bytes.saturating_add(1).max(1),
        ..ScanLimits::default()
    };

    let benchmark_start = Instant::now();
    let mut total_bytes_scanned = 0_u64;
    let mut total_results = 0_usize;
    let mut last_stop_reason = String::new();

    for _ in 0..iterations {
        let mut file = File::open(&path)
            .with_context(|| format!("cannot open benchmark file {}", path.display()))?;
        let request = ScanRequest::new(keyword.clone()).with_limits(limits);
        let outcome = scan_reader(&mut file, &request).context("scanner benchmark failed")?;
        total_bytes_scanned = total_bytes_scanned.saturating_add(outcome.bytes_scanned);
        total_results = total_results.saturating_add(outcome.results.len());
        last_stop_reason = format!("{:?}", outcome.stop_reason);
    }

    let elapsed = benchmark_start.elapsed();
    let elapsed_seconds = elapsed.as_secs_f64();
    let throughput_mib_per_second = if elapsed_seconds > 0.0 {
        total_bytes_scanned as f64 / (1024.0 * 1024.0) / elapsed_seconds
    } else {
        0.0
    };

    let report = json!({
        "file_bytes": file_bytes,
        "iterations": iterations,
        "keyword_bytes": keyword.len(),
        "total_bytes_scanned": total_bytes_scanned,
        "total_results": total_results,
        "elapsed_milliseconds": elapsed.as_secs_f64() * 1000.0,
        "throughput_mib_per_second": throughput_mib_per_second,
        "last_stop_reason": last_stop_reason,
        "note": "Use a non-matching or rare keyword to measure full-file throughput."
    });
    println!("{}", serde_json::to_string_pretty(&report)?);

    Ok(())
}

fn next_arg(args: &mut impl Iterator<Item = OsString>, message: &'static str) -> Result<OsString> {
    args.next().ok_or_else(|| anyhow::anyhow!(message))
}
