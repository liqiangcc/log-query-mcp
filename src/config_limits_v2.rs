use serde::{Deserialize, Serialize};

use crate::LimitsConfig;

const HARD_MAX_CONCURRENT_SSH_CONNECTIONS: usize = 64;
const HARD_MAX_SYNC_BYTES_PER_QUERY: u64 = 64 * 1024 * 1024 * 1024;
const HARD_MAX_REMOTE_FILES_PER_SOURCE: usize = 10_000;

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(default, deny_unknown_fields)]
pub struct LimitsConfigV2 {
    pub max_sources_per_query: usize,
    pub max_scan_files_per_query: usize,
    pub max_scan_bytes_per_page: u64,
    pub query_timeout_millis: u64,
    pub default_results_per_page: usize,
    pub max_results_per_page: usize,
    pub max_line_bytes: usize,
    pub max_returned_content_bytes: usize,
    pub max_response_bytes: usize,
    pub max_context_lines_per_side: usize,
    pub max_concurrent_scans: usize,
    pub match_reference_capacity: usize,
    pub match_reference_ttl_seconds: u64,
    pub cursor_capacity: usize,
    pub cursor_ttl_seconds: u64,
    pub max_concurrent_ssh_connections: usize,
    pub max_sync_bytes_per_query: u64,
    pub max_remote_files_per_source: usize,
}

impl Default for LimitsConfigV2 {
    fn default() -> Self {
        let local = LimitsConfig::default();
        Self {
            max_sources_per_query: local.max_sources_per_query,
            max_scan_files_per_query: local.max_scan_files_per_query,
            max_scan_bytes_per_page: local.max_scan_bytes_per_page,
            query_timeout_millis: local.query_timeout_millis,
            default_results_per_page: local.default_results_per_page,
            max_results_per_page: local.max_results_per_page,
            max_line_bytes: local.max_line_bytes,
            max_returned_content_bytes: local.max_returned_content_bytes,
            max_response_bytes: local.max_response_bytes,
            max_context_lines_per_side: local.max_context_lines_per_side,
            max_concurrent_scans: local.max_concurrent_scans,
            match_reference_capacity: local.match_reference_capacity,
            match_reference_ttl_seconds: local.match_reference_ttl_seconds,
            cursor_capacity: local.cursor_capacity,
            cursor_ttl_seconds: local.cursor_ttl_seconds,
            max_concurrent_ssh_connections: 4,
            max_sync_bytes_per_query: 512 * 1024 * 1024,
            max_remote_files_per_source: 500,
        }
    }
}

impl LimitsConfigV2 {
    #[must_use]
    pub fn local_limits(&self) -> LimitsConfig {
        LimitsConfig {
            max_sources_per_query: self.max_sources_per_query,
            max_scan_files_per_query: self.max_scan_files_per_query,
            max_scan_bytes_per_page: self.max_scan_bytes_per_page,
            query_timeout_millis: self.query_timeout_millis,
            default_results_per_page: self.default_results_per_page,
            max_results_per_page: self.max_results_per_page,
            max_line_bytes: self.max_line_bytes,
            max_returned_content_bytes: self.max_returned_content_bytes,
            max_response_bytes: self.max_response_bytes,
            max_context_lines_per_side: self.max_context_lines_per_side,
            max_concurrent_scans: self.max_concurrent_scans,
            match_reference_capacity: self.match_reference_capacity,
            match_reference_ttl_seconds: self.match_reference_ttl_seconds,
            cursor_capacity: self.cursor_capacity,
            cursor_ttl_seconds: self.cursor_ttl_seconds,
        }
    }

    pub(crate) fn validate_remote(&self) -> Result<(), LimitsConfigV2ValidationError> {
        if self.max_concurrent_ssh_connections == 0
            || self.max_concurrent_ssh_connections > HARD_MAX_CONCURRENT_SSH_CONNECTIONS
        {
            return Err(LimitsConfigV2ValidationError::ConcurrentSshConnections);
        }
        if self.max_sync_bytes_per_query == 0
            || self.max_sync_bytes_per_query > HARD_MAX_SYNC_BYTES_PER_QUERY
        {
            return Err(LimitsConfigV2ValidationError::SyncBytesPerQuery);
        }
        if self.max_remote_files_per_source == 0
            || self.max_remote_files_per_source > HARD_MAX_REMOTE_FILES_PER_SOURCE
        {
            return Err(LimitsConfigV2ValidationError::RemoteFilesPerSource);
        }
        Ok(())
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum LimitsConfigV2ValidationError {
    ConcurrentSshConnections,
    SyncBytesPerQuery,
    RemoteFilesPerSource,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn defaults_preserve_v1_query_limits() {
        let v2 = LimitsConfigV2::default();
        assert_eq!(v2.local_limits(), LimitsConfig::default());
    }

    #[test]
    fn rejects_remote_limit_above_hard_cap() {
        let limits = LimitsConfigV2 {
            max_concurrent_ssh_connections: HARD_MAX_CONCURRENT_SSH_CONNECTIONS + 1,
            ..LimitsConfigV2::default()
        };
        assert_eq!(
            limits.validate_remote(),
            Err(LimitsConfigV2ValidationError::ConcurrentSshConnections)
        );
    }
}
