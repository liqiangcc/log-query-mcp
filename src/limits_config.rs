use std::{env, ffi::OsString, time::Duration};

use thiserror::Error;

use crate::{
    ContextReadLimits, MAX_CONTEXT_BACKTRACK_BYTES, MAX_CONTEXT_FORWARD_BYTES,
    MAX_LINE_PREVIEW_BYTES, MAX_READ_BUFFER_BYTES, MAX_RETURNED_CONTENT_BYTES, QueryServiceLimits,
};

pub const MAX_CONFIGURED_SCAN_BYTES_PER_PAGE: u64 = 64 * 1024 * 1024 * 1024;
pub const MAX_CONFIGURED_RESPONSE_BYTES: usize = 64 * 1024 * 1024;
pub const MAX_CONFIGURED_QUERY_TIMEOUT_MILLIS: u64 = 10 * 60 * 1000;
pub const MAX_CONFIGURED_CONCURRENT_SCANS: usize = 64;
pub const MAX_CONFIGURED_STATE_CAPACITY: usize = 1_000_000;
pub const MAX_CONFIGURED_STATE_TTL_SECONDS: u64 = 24 * 60 * 60;

pub fn query_service_limits_from_env() -> Result<QueryServiceLimits, LimitConfigError> {
    query_service_limits_from_lookup(env::var_os)
}

fn query_service_limits_from_lookup(
    mut lookup: impl FnMut(&str) -> Option<OsString>,
) -> Result<QueryServiceLimits, LimitConfigError> {
    let defaults = QueryServiceLimits::default();
    let context_defaults = defaults.context;

    let max_scan_bytes_per_page = read_u64(
        &mut lookup,
        "LOG_QUERY_MCP_MAX_SCAN_BYTES_PER_PAGE",
        defaults.max_scan_bytes_per_page,
        MAX_CONFIGURED_SCAN_BYTES_PER_PAGE,
    )?;
    let max_returned_content_bytes = read_usize(
        &mut lookup,
        "LOG_QUERY_MCP_MAX_RETURNED_CONTENT_BYTES",
        defaults.max_returned_content_bytes,
        MAX_RETURNED_CONTENT_BYTES,
    )?;
    let max_response_bytes = read_usize(
        &mut lookup,
        "LOG_QUERY_MCP_MAX_RESPONSE_BYTES",
        defaults.max_response_bytes,
        MAX_CONFIGURED_RESPONSE_BYTES,
    )?;
    let query_timeout_millis = read_u64(
        &mut lookup,
        "LOG_QUERY_MCP_QUERY_TIMEOUT_MILLIS",
        duration_millis(defaults.query_timeout)?,
        MAX_CONFIGURED_QUERY_TIMEOUT_MILLIS,
    )?;
    let max_concurrent_scans = read_usize(
        &mut lookup,
        "LOG_QUERY_MCP_MAX_CONCURRENT_SCANS",
        defaults.max_concurrent_scans,
        MAX_CONFIGURED_CONCURRENT_SCANS,
    )?;
    let match_reference_capacity = read_usize(
        &mut lookup,
        "LOG_QUERY_MCP_MATCH_REFERENCE_CAPACITY",
        defaults.match_reference_capacity,
        MAX_CONFIGURED_STATE_CAPACITY,
    )?;
    let match_reference_ttl_seconds = read_u64(
        &mut lookup,
        "LOG_QUERY_MCP_MATCH_REFERENCE_TTL_SECONDS",
        defaults.match_reference_ttl.as_secs(),
        MAX_CONFIGURED_STATE_TTL_SECONDS,
    )?;
    let cursor_capacity = read_usize(
        &mut lookup,
        "LOG_QUERY_MCP_CURSOR_CAPACITY",
        defaults.cursor_capacity,
        MAX_CONFIGURED_STATE_CAPACITY,
    )?;
    let cursor_ttl_seconds = read_u64(
        &mut lookup,
        "LOG_QUERY_MCP_CURSOR_TTL_SECONDS",
        defaults.cursor_ttl.as_secs(),
        MAX_CONFIGURED_STATE_TTL_SECONDS,
    )?;

    let context = ContextReadLimits {
        max_backtrack_bytes: read_u64(
            &mut lookup,
            "LOG_QUERY_MCP_CONTEXT_MAX_BACKTRACK_BYTES",
            context_defaults.max_backtrack_bytes,
            MAX_CONTEXT_BACKTRACK_BYTES,
        )?,
        max_forward_bytes: read_u64(
            &mut lookup,
            "LOG_QUERY_MCP_CONTEXT_MAX_FORWARD_BYTES",
            context_defaults.max_forward_bytes,
            MAX_CONTEXT_FORWARD_BYTES,
        )?,
        max_line_bytes: read_usize(
            &mut lookup,
            "LOG_QUERY_MCP_CONTEXT_MAX_LINE_BYTES",
            context_defaults.max_line_bytes,
            MAX_LINE_PREVIEW_BYTES,
        )?,
        max_returned_content_bytes: read_usize(
            &mut lookup,
            "LOG_QUERY_MCP_CONTEXT_MAX_RETURNED_CONTENT_BYTES",
            context_defaults.max_returned_content_bytes,
            MAX_RETURNED_CONTENT_BYTES,
        )?,
        read_buffer_bytes: read_usize(
            &mut lookup,
            "LOG_QUERY_MCP_CONTEXT_READ_BUFFER_BYTES",
            context_defaults.read_buffer_bytes,
            MAX_READ_BUFFER_BYTES,
        )?,
    };

    if max_returned_content_bytes >= max_response_bytes {
        return Err(LimitConfigError::InvalidRelationship(
            "LOG_QUERY_MCP_MAX_RETURNED_CONTENT_BYTES must be smaller than LOG_QUERY_MCP_MAX_RESPONSE_BYTES",
        ));
    }
    if context.max_line_bytes > context.max_returned_content_bytes {
        return Err(LimitConfigError::InvalidRelationship(
            "LOG_QUERY_MCP_CONTEXT_MAX_LINE_BYTES must not exceed LOG_QUERY_MCP_CONTEXT_MAX_RETURNED_CONTENT_BYTES",
        ));
    }
    if context.max_returned_content_bytes >= max_response_bytes {
        return Err(LimitConfigError::InvalidRelationship(
            "LOG_QUERY_MCP_CONTEXT_MAX_RETURNED_CONTENT_BYTES must be smaller than LOG_QUERY_MCP_MAX_RESPONSE_BYTES",
        ));
    }

    Ok(QueryServiceLimits {
        max_scan_bytes_per_page,
        max_returned_content_bytes,
        max_response_bytes,
        query_timeout: Duration::from_millis(query_timeout_millis),
        max_concurrent_scans,
        match_reference_capacity,
        match_reference_ttl: Duration::from_secs(match_reference_ttl_seconds),
        cursor_capacity,
        cursor_ttl: Duration::from_secs(cursor_ttl_seconds),
        context,
    })
}

fn read_u64(
    lookup: &mut impl FnMut(&str) -> Option<OsString>,
    name: &'static str,
    default: u64,
    maximum: u64,
) -> Result<u64, LimitConfigError> {
    let Some(raw) = lookup(name) else {
        return Ok(default);
    };
    let text = raw
        .into_string()
        .map_err(|_| LimitConfigError::NotUnicode { name })?;
    let value = text
        .parse::<u64>()
        .map_err(|_| LimitConfigError::InvalidInteger { name })?;
    if value == 0 {
        return Err(LimitConfigError::Zero { name });
    }
    if value > maximum {
        return Err(LimitConfigError::ExceedsMaximum { name, maximum });
    }
    Ok(value)
}

fn read_usize(
    lookup: &mut impl FnMut(&str) -> Option<OsString>,
    name: &'static str,
    default: usize,
    maximum: usize,
) -> Result<usize, LimitConfigError> {
    let maximum_u64 = u64::try_from(maximum).map_err(|_| LimitConfigError::PlatformRange { name })?;
    let default_u64 = u64::try_from(default).map_err(|_| LimitConfigError::PlatformRange { name })?;
    let value = read_u64(lookup, name, default_u64, maximum_u64)?;
    usize::try_from(value).map_err(|_| LimitConfigError::PlatformRange { name })
}

fn duration_millis(duration: Duration) -> Result<u64, LimitConfigError> {
    u64::try_from(duration.as_millis()).map_err(|_| LimitConfigError::PlatformRange {
        name: "LOG_QUERY_MCP_QUERY_TIMEOUT_MILLIS",
    })
}

#[derive(Debug, Error, PartialEq, Eq)]
pub enum LimitConfigError {
    #[error("environment variable {name} is not valid Unicode")]
    NotUnicode { name: &'static str },

    #[error("environment variable {name} must be a positive base-10 integer")]
    InvalidInteger { name: &'static str },

    #[error("environment variable {name} must be greater than zero")]
    Zero { name: &'static str },

    #[error("environment variable {name} exceeds the hard maximum {maximum}")]
    ExceedsMaximum { name: &'static str, maximum: u64 },

    #[error("environment variable {name} cannot be represented on this platform")]
    PlatformRange { name: &'static str },

    #[error("query limit configuration is inconsistent: {0}")]
    InvalidRelationship(&'static str),
}

#[cfg(test)]
mod tests {
    use std::collections::HashMap;

    use super::*;

    fn load(values: &[(&str, &str)]) -> Result<QueryServiceLimits, LimitConfigError> {
        let values: HashMap<&str, OsString> = values
            .iter()
            .map(|(name, value)| (*name, OsString::from(value)))
            .collect();
        query_service_limits_from_lookup(|name| values.get(name).cloned())
    }

    #[test]
    fn keeps_defaults_when_environment_is_empty() {
        let observed = load(&[]).expect("default limits should load");
        assert_eq!(observed, QueryServiceLimits::default());
    }

    #[test]
    fn applies_bounded_overrides() {
        let observed = load(&[
            ("LOG_QUERY_MCP_MAX_SCAN_BYTES_PER_PAGE", "1048576"),
            ("LOG_QUERY_MCP_MAX_RETURNED_CONTENT_BYTES", "65536"),
            ("LOG_QUERY_MCP_MAX_RESPONSE_BYTES", "131072"),
            ("LOG_QUERY_MCP_QUERY_TIMEOUT_MILLIS", "2500"),
            ("LOG_QUERY_MCP_MAX_CONCURRENT_SCANS", "8"),
            ("LOG_QUERY_MCP_MATCH_REFERENCE_CAPACITY", "20000"),
            ("LOG_QUERY_MCP_MATCH_REFERENCE_TTL_SECONDS", "900"),
            ("LOG_QUERY_MCP_CURSOR_CAPACITY", "2000"),
            ("LOG_QUERY_MCP_CURSOR_TTL_SECONDS", "600"),
            ("LOG_QUERY_MCP_CONTEXT_MAX_BACKTRACK_BYTES", "1048576"),
            ("LOG_QUERY_MCP_CONTEXT_MAX_FORWARD_BYTES", "1048576"),
            ("LOG_QUERY_MCP_CONTEXT_MAX_LINE_BYTES", "4096"),
            ("LOG_QUERY_MCP_CONTEXT_MAX_RETURNED_CONTENT_BYTES", "32768"),
            ("LOG_QUERY_MCP_CONTEXT_READ_BUFFER_BYTES", "8192"),
        ])
        .expect("valid overrides should load");

        assert_eq!(observed.max_scan_bytes_per_page, 1_048_576);
        assert_eq!(observed.max_returned_content_bytes, 65_536);
        assert_eq!(observed.max_response_bytes, 131_072);
        assert_eq!(observed.query_timeout, Duration::from_millis(2_500));
        assert_eq!(observed.max_concurrent_scans, 8);
        assert_eq!(observed.match_reference_capacity, 20_000);
        assert_eq!(observed.match_reference_ttl, Duration::from_secs(900));
        assert_eq!(observed.cursor_capacity, 2_000);
        assert_eq!(observed.cursor_ttl, Duration::from_secs(600));
        assert_eq!(observed.context.max_line_bytes, 4_096);
        assert_eq!(observed.context.max_returned_content_bytes, 32_768);
        assert_eq!(observed.context.read_buffer_bytes, 8_192);
    }

    #[test]
    fn rejects_zero_invalid_and_excessive_values() {
        assert!(matches!(
            load(&[("LOG_QUERY_MCP_MAX_CONCURRENT_SCANS", "0")]),
            Err(LimitConfigError::Zero { .. })
        ));
        assert!(matches!(
            load(&[("LOG_QUERY_MCP_QUERY_TIMEOUT_MILLIS", "fast")]),
            Err(LimitConfigError::InvalidInteger { .. })
        ));
        assert!(matches!(
            load(&[("LOG_QUERY_MCP_MAX_CONCURRENT_SCANS", "65")]),
            Err(LimitConfigError::ExceedsMaximum { .. })
        ));
    }

    #[test]
    fn rejects_inconsistent_response_budgets() {
        assert!(matches!(
            load(&[
                ("LOG_QUERY_MCP_MAX_RETURNED_CONTENT_BYTES", "131072"),
                ("LOG_QUERY_MCP_MAX_RESPONSE_BYTES", "131072"),
            ]),
            Err(LimitConfigError::InvalidRelationship(_))
        ));
        assert!(matches!(
            load(&[
                ("LOG_QUERY_MCP_MAX_RESPONSE_BYTES", "131072"),
                ("LOG_QUERY_MCP_CONTEXT_MAX_LINE_BYTES", "65536"),
                ("LOG_QUERY_MCP_CONTEXT_MAX_RETURNED_CONTENT_BYTES", "32768"),
            ]),
            Err(LimitConfigError::InvalidRelationship(_))
        ));
    }
}
