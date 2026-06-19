use std::{
    env, fs,
    net::SocketAddr,
    path::{Path, PathBuf},
    process::ExitCode,
    sync::Arc,
};

use log_query_mcp::{
    QueryService, QueryServiceLimits, SourceRegistry, query_service_limits_from_env,
};
use serde::Serialize;

const DEFAULT_CONFIG_PATH: &str = "log-query-mcp.json";
const DEFAULT_BIND_ADDRESS: &str = "127.0.0.1:8000";
const MINIMUM_KERNEL_MAJOR: u32 = 5;
const MINIMUM_KERNEL_MINOR: u32 = 6;

#[derive(Debug)]
struct Arguments {
    config_path: PathBuf,
    allow_root: bool,
    compact: bool,
}

#[derive(Debug, Clone, Copy, Serialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
enum CheckStatus {
    Pass,
    Warning,
    Fail,
}

#[derive(Debug, Serialize)]
struct CheckResult {
    name: &'static str,
    status: CheckStatus,
    detail: String,
}

#[derive(Debug, Serialize)]
struct SourceReport {
    source_id: String,
    service: String,
    environment: String,
    file_count: usize,
    timestamp_rule_configured: bool,
}

#[derive(Debug, Serialize)]
struct LimitsReport {
    max_scan_bytes_per_page: u64,
    max_returned_content_bytes: usize,
    max_response_bytes: usize,
    query_timeout_millis: u128,
    max_concurrent_scans: usize,
    match_reference_capacity: usize,
    match_reference_ttl_seconds: u64,
    cursor_capacity: usize,
    cursor_ttl_seconds: u64,
    context_max_backtrack_bytes: u64,
    context_max_forward_bytes: u64,
    context_max_line_bytes: usize,
    context_max_returned_content_bytes: usize,
    context_read_buffer_bytes: usize,
}

impl From<QueryServiceLimits> for LimitsReport {
    fn from(limits: QueryServiceLimits) -> Self {
        Self {
            max_scan_bytes_per_page: limits.max_scan_bytes_per_page,
            max_returned_content_bytes: limits.max_returned_content_bytes,
            max_response_bytes: limits.max_response_bytes,
            query_timeout_millis: limits.query_timeout.as_millis(),
            max_concurrent_scans: limits.max_concurrent_scans,
            match_reference_capacity: limits.match_reference_capacity,
            match_reference_ttl_seconds: limits.match_reference_ttl.as_secs(),
            cursor_capacity: limits.cursor_capacity,
            cursor_ttl_seconds: limits.cursor_ttl.as_secs(),
            context_max_backtrack_bytes: limits.context.max_backtrack_bytes,
            context_max_forward_bytes: limits.context.max_forward_bytes,
            context_max_line_bytes: limits.context.max_line_bytes,
            context_max_returned_content_bytes: limits.context.max_returned_content_bytes,
            context_read_buffer_bytes: limits.context.read_buffer_bytes,
        }
    }
}

#[derive(Debug, Serialize)]
struct DoctorReport {
    ok: bool,
    version: &'static str,
    config_path: String,
    kernel_release: Option<String>,
    effective_uid: Option<u32>,
    bind_address: Option<String>,
    sources: Vec<SourceReport>,
    limits: Option<LimitsReport>,
    checks: Vec<CheckResult>,
}

fn main() -> ExitCode {
    let arguments = match parse_arguments() {
        Ok(arguments) => arguments,
        Err(message) => {
            eprintln!("{message}");
            print_usage();
            return ExitCode::from(2);
        }
    };

    let report = run_checks(&arguments);
    let encoded = if arguments.compact {
        serde_json::to_string(&report)
    } else {
        serde_json::to_string_pretty(&report)
    };
    match encoded {
        Ok(encoded) => println!("{encoded}"),
        Err(error) => {
            eprintln!("failed to serialize preflight report: {error}");
            return ExitCode::FAILURE;
        }
    }

    if report.ok {
        ExitCode::SUCCESS
    } else {
        ExitCode::FAILURE
    }
}

fn parse_arguments() -> Result<Arguments, String> {
    let mut config_path = env::var_os("LOG_QUERY_MCP_CONFIG")
        .map(PathBuf::from)
        .unwrap_or_else(|| PathBuf::from(DEFAULT_CONFIG_PATH));
    let mut allow_root = false;
    let mut compact = false;
    let mut arguments = env::args_os().skip(1);

    while let Some(argument) = arguments.next() {
        match argument.to_str() {
            Some("--config") => {
                config_path = arguments
                    .next()
                    .map(PathBuf::from)
                    .ok_or_else(|| "--config requires a path".to_owned())?;
            }
            Some("--allow-root") => allow_root = true,
            Some("--compact") => compact = true,
            Some("--help" | "-h") => {
                print_usage();
                std::process::exit(0);
            }
            Some(value) => return Err(format!("unknown argument: {value}")),
            None => return Err("arguments must be valid Unicode".to_owned()),
        }
    }

    Ok(Arguments {
        config_path,
        allow_root,
        compact,
    })
}

fn print_usage() {
    eprintln!(
        "usage: log-query-doctor [--config PATH] [--allow-root] [--compact]\n\
         Run as the same non-root user that will run log-query-mcp."
    );
}

fn run_checks(arguments: &Arguments) -> DoctorReport {
    let mut checks = Vec::new();

    let kernel_release = read_trimmed("/proc/sys/kernel/osrelease").ok();
    match kernel_release.as_deref().and_then(parse_kernel_release) {
        Some((major, minor)) if kernel_is_supported(major, minor) => checks.push(CheckResult {
            name: "linux_kernel",
            status: CheckStatus::Pass,
            detail: format!(
                "kernel {major}.{minor} satisfies the Linux {MINIMUM_KERNEL_MAJOR}.{MINIMUM_KERNEL_MINOR}+ openat2 baseline"
            ),
        }),
        Some((major, minor)) => checks.push(CheckResult {
            name: "linux_kernel",
            status: CheckStatus::Fail,
            detail: format!(
                "kernel {major}.{minor} is older than the required Linux {MINIMUM_KERNEL_MAJOR}.{MINIMUM_KERNEL_MINOR} baseline"
            ),
        }),
        None => checks.push(CheckResult {
            name: "linux_kernel",
            status: CheckStatus::Fail,
            detail: "cannot determine the Linux kernel release".to_owned(),
        }),
    }

    let effective_uid = read_effective_uid().ok();
    match effective_uid {
        Some(0) if arguments.allow_root => checks.push(CheckResult {
            name: "non_root_user",
            status: CheckStatus::Warning,
            detail: "doctor is running as root because --allow-root was supplied; repeat as the service user before deployment".to_owned(),
        }),
        Some(0) => checks.push(CheckResult {
            name: "non_root_user",
            status: CheckStatus::Fail,
            detail: "service must run as a dedicated non-root user; use --allow-root only for administrative inspection".to_owned(),
        }),
        Some(uid) => checks.push(CheckResult {
            name: "non_root_user",
            status: CheckStatus::Pass,
            detail: format!("effective UID {uid} is non-root"),
        }),
        None => checks.push(CheckResult {
            name: "non_root_user",
            status: CheckStatus::Fail,
            detail: "cannot determine the effective UID from /proc/self/status".to_owned(),
        }),
    }

    let bind_text =
        env::var("LOG_QUERY_MCP_BIND").unwrap_or_else(|_| DEFAULT_BIND_ADDRESS.to_owned());
    let bind_address = match bind_text.parse::<SocketAddr>() {
        Ok(address) if address.ip().is_loopback() => {
            checks.push(CheckResult {
                name: "bind_address",
                status: CheckStatus::Pass,
                detail: format!("{address} is loopback-only"),
            });
            Some(address.to_string())
        }
        Ok(address) if address.ip().is_unspecified() => {
            checks.push(CheckResult {
                name: "bind_address",
                status: CheckStatus::Warning,
                detail: format!(
                    "{address} listens on all interfaces; verify firewall or reverse-proxy restrictions"
                ),
            });
            Some(address.to_string())
        }
        Ok(address) => {
            checks.push(CheckResult {
                name: "bind_address",
                status: CheckStatus::Warning,
                detail: format!(
                    "{address} is non-loopback; verify that it is reachable only from the controlled internal network"
                ),
            });
            Some(address.to_string())
        }
        Err(error) => {
            checks.push(CheckResult {
                name: "bind_address",
                status: CheckStatus::Fail,
                detail: format!("LOG_QUERY_MCP_BIND is not a socket address: {error}"),
            });
            None
        }
    };

    let limits = match query_service_limits_from_env() {
        Ok(limits) => {
            checks.push(CheckResult {
                name: "query_limits",
                status: CheckStatus::Pass,
                detail: "query resource limits are internally consistent".to_owned(),
            });
            Some(limits)
        }
        Err(error) => {
            checks.push(CheckResult {
                name: "query_limits",
                status: CheckStatus::Fail,
                detail: error.to_string(),
            });
            None
        }
    };

    let mut sources = Vec::new();
    let registry = match SourceRegistry::from_config_path(&arguments.config_path) {
        Ok(registry) => {
            let public_sources = registry.list();
            let mut total_files = 0_usize;
            let mut empty_source = false;
            for public in public_sources {
                let Some(configured) = registry.get(&public.source_id) else {
                    empty_source = true;
                    continue;
                };
                let file_count = configured.files().len();
                total_files = total_files.saturating_add(file_count);
                empty_source |= file_count == 0;
                sources.push(SourceReport {
                    source_id: public.source_id,
                    service: public.service,
                    environment: public.environment,
                    file_count,
                    timestamp_rule_configured: configured.timestamp_rule().is_some(),
                });
            }

            if empty_source || sources.is_empty() || total_files == 0 {
                checks.push(CheckResult {
                    name: "source_configuration",
                    status: CheckStatus::Fail,
                    detail: "configuration loaded but at least one source resolved to no readable regular log files".to_owned(),
                });
            } else {
                checks.push(CheckResult {
                    name: "source_configuration",
                    status: CheckStatus::Pass,
                    detail: format!(
                        "loaded {} sources and safely opened {total_files} regular log files",
                        sources.len()
                    ),
                });
            }
            Some(registry)
        }
        Err(error) => {
            checks.push(CheckResult {
                name: "source_configuration",
                status: CheckStatus::Fail,
                detail: error.to_string(),
            });
            None
        }
    };

    if let (Some(registry), Some(limits)) = (registry, limits) {
        match QueryService::new(Arc::new(registry), limits) {
            Ok(_) => checks.push(CheckResult {
                name: "query_service_initialization",
                status: CheckStatus::Pass,
                detail: "query executor, match reference store, and cursor store initialized"
                    .to_owned(),
            }),
            Err(error) => checks.push(CheckResult {
                name: "query_service_initialization",
                status: CheckStatus::Fail,
                detail: error.to_string(),
            }),
        }
    } else {
        checks.push(CheckResult {
            name: "query_service_initialization",
            status: CheckStatus::Fail,
            detail: "query service was not initialized because an earlier required check failed"
                .to_owned(),
        });
    }

    let ok = checks.iter().all(|check| check.status != CheckStatus::Fail);
    DoctorReport {
        ok,
        version: env!("CARGO_PKG_VERSION"),
        config_path: display_path(&arguments.config_path),
        kernel_release,
        effective_uid,
        bind_address,
        sources,
        limits: limits.map(LimitsReport::from),
        checks,
    }
}

fn read_trimmed(path: impl AsRef<Path>) -> Result<String, std::io::Error> {
    fs::read_to_string(path).map(|value| value.trim().to_owned())
}

fn parse_kernel_release(release: &str) -> Option<(u32, u32)> {
    let mut parts = release.split('.');
    let major = parts.next()?.parse().ok()?;
    let minor_digits: String = parts
        .next()?
        .chars()
        .take_while(char::is_ascii_digit)
        .collect();
    let minor = minor_digits.parse().ok()?;
    Some((major, minor))
}

const fn kernel_is_supported(major: u32, minor: u32) -> bool {
    major > MINIMUM_KERNEL_MAJOR || (major == MINIMUM_KERNEL_MAJOR && minor >= MINIMUM_KERNEL_MINOR)
}

fn read_effective_uid() -> Result<u32, String> {
    let status = fs::read_to_string("/proc/self/status").map_err(|error| error.to_string())?;
    let uid_line = status
        .lines()
        .find_map(|line| line.strip_prefix("Uid:"))
        .ok_or_else(|| "Uid field is absent".to_owned())?;
    let mut values = uid_line.split_whitespace();
    let _real = values
        .next()
        .ok_or_else(|| "real UID is absent".to_owned())?;
    values
        .next()
        .ok_or_else(|| "effective UID is absent".to_owned())?
        .parse()
        .map_err(|error| format!("effective UID is invalid: {error}"))
}

fn display_path(path: &Path) -> String {
    path.to_string_lossy().into_owned()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_common_kernel_release_strings() {
        assert_eq!(parse_kernel_release("6.8.0-31-generic"), Some((6, 8)));
        assert_eq!(parse_kernel_release("5.15.0-1068-azure"), Some((5, 15)));
        assert_eq!(parse_kernel_release("5.6"), Some((5, 6)));
        assert_eq!(parse_kernel_release("not-a-kernel"), None);
    }

    #[test]
    fn enforces_openat2_kernel_baseline() {
        assert!(kernel_is_supported(5, 6));
        assert!(kernel_is_supported(6, 0));
        assert!(!kernel_is_supported(5, 5));
        assert!(!kernel_is_supported(4, 19));
    }

    #[test]
    fn limit_report_preserves_runtime_values() {
        let limits = QueryServiceLimits::default();
        let report = LimitsReport::from(limits);
        assert_eq!(
            report.max_scan_bytes_per_page,
            limits.max_scan_bytes_per_page
        );
        assert_eq!(report.max_concurrent_scans, limits.max_concurrent_scans);
        assert_eq!(report.context_max_line_bytes, limits.context.max_line_bytes);
    }
}
