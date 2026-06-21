use schemars::JsonSchema;
use serde::Serialize;

use crate::{
    ContextReadError, ContextTaskError, QueryStateError, ScanError, ScanTaskError,
    SourceRegistryError, StatefulContextError, StatefulQueryError,
};

#[derive(Debug, Clone, Copy, Serialize, JsonSchema, PartialEq, Eq)]
#[serde(rename_all = "SCREAMING_SNAKE_CASE")]
pub enum ToolErrorCode {
    InvalidArgument,
    UnknownSource,
    SourceUnavailable,
    DeadlineExceeded,
    QueryCancelled,
    ResourceLimit,
    CursorInvalid,
    MatchRefInvalid,
    FileChanged,
    InternalError,
}

#[derive(Debug, Clone, Serialize, JsonSchema, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct ToolErrorBody {
    pub code: ToolErrorCode,
    pub message: &'static str,
    pub retryable: bool,
}

impl ToolErrorBody {
    #[must_use]
    pub const fn new(code: ToolErrorCode, message: &'static str) -> Self {
        Self {
            code,
            message,
            retryable: retryable(code),
        }
    }

    #[must_use]
    pub fn to_json_text(&self) -> String {
        serde_json::to_string(self).unwrap_or_else(|_| {
            r#"{"code":"INTERNAL_ERROR","message":"an internal tool error occurred","retryable":true}"#
                .to_owned()
        })
    }

    #[must_use]
    pub const fn response_limit() -> Self {
        Self::new(
            ToolErrorCode::ResourceLimit,
            "the serialized tool response exceeds the service limit",
        )
    }

    #[must_use]
    pub const fn serialization() -> Self {
        Self::new(
            ToolErrorCode::InternalError,
            "the tool response could not be serialized",
        )
    }
}

impl From<StatefulQueryError> for ToolErrorBody {
    fn from(error: StatefulQueryError) -> Self {
        match error {
            StatefulQueryError::InvalidArgument(_) | StatefulQueryError::TimeFilter(_) => {
                Self::new(
                    ToolErrorCode::InvalidArgument,
                    "the search request is outside the v1 contract",
                )
            }
            StatefulQueryError::Cancelled => Self::new(
                ToolErrorCode::QueryCancelled,
                "the log query was cancelled",
            ),
            StatefulQueryError::DeadlineExceeded => Self::new(
                ToolErrorCode::DeadlineExceeded,
                "the log query exceeded its deadline",
            ),
            StatefulQueryError::FileLimitExceeded
            | StatefulQueryError::UnsafeContinuation => Self::new(
                ToolErrorCode::ResourceLimit,
                "the log query reached a service resource limit",
            ),
            StatefulQueryError::InvalidCursorState => cursor_invalid(),
            StatefulQueryError::InvalidScanPosition
            | StatefulQueryError::ScanPositionNotLineBoundary => file_changed(),
            StatefulQueryError::ResourceCounterOverflow
            | StatefulQueryError::DeadlineOverflow => internal_error(),
            StatefulQueryError::QueryState(error) => map_query_state_for_search(error),
            StatefulQueryError::SourceRegistry(error) => map_source_registry_for_search(error),
            StatefulQueryError::ScanTask(error) => map_scan_task(error),
            StatefulQueryError::Io(_) => source_unavailable(),
        }
    }
}

impl From<StatefulContextError> for ToolErrorBody {
    fn from(error: StatefulContextError) -> Self {
        match error {
            StatefulContextError::InvalidArgument(_) => Self::new(
                ToolErrorCode::InvalidArgument,
                "the context request is outside the v1 contract",
            ),
            StatefulContextError::Cancelled => Self::new(
                ToolErrorCode::QueryCancelled,
                "the context request was cancelled",
            ),
            StatefulContextError::DeadlineExceeded => Self::new(
                ToolErrorCode::DeadlineExceeded,
                "the context request exceeded its deadline",
            ),
            StatefulContextError::DeadlineOverflow => internal_error(),
            StatefulContextError::QueryState(error) => map_query_state_for_context(error),
            StatefulContextError::SourceRegistry(error) => {
                map_source_registry_for_context(error)
            }
            StatefulContextError::ContextRead(error) => map_context_read(error),
            StatefulContextError::ContextTask(error) => map_context_task(error),
        }
    }
}

const fn retryable(code: ToolErrorCode) -> bool {
    matches!(
        code,
        ToolErrorCode::SourceUnavailable
            | ToolErrorCode::DeadlineExceeded
            | ToolErrorCode::QueryCancelled
            | ToolErrorCode::FileChanged
            | ToolErrorCode::InternalError
    )
}

const fn cursor_invalid() -> ToolErrorBody {
    ToolErrorBody::new(
        ToolErrorCode::CursorInvalid,
        "the search cursor is invalid or expired; run the search again",
    )
}

const fn match_ref_invalid() -> ToolErrorBody {
    ToolErrorBody::new(
        ToolErrorCode::MatchRefInvalid,
        "the match reference is invalid or expired; run the search again",
    )
}

const fn file_changed() -> ToolErrorBody {
    ToolErrorBody::new(
        ToolErrorCode::FileChanged,
        "the referenced log file changed; run the search again",
    )
}

const fn source_unavailable() -> ToolErrorBody {
    ToolErrorBody::new(
        ToolErrorCode::SourceUnavailable,
        "one or more configured log files are temporarily unavailable",
    )
}

const fn internal_error() -> ToolErrorBody {
    ToolErrorBody::new(
        ToolErrorCode::InternalError,
        "an internal tool error occurred",
    )
}

fn map_query_state_for_search(error: QueryStateError) -> ToolErrorBody {
    match error {
        QueryStateError::UnknownOrExpired
        | QueryStateError::QueryMismatch
        | QueryStateError::Busy
        | QueryStateError::LeaseLost
        | QueryStateError::InvalidContinuation(_) => cursor_invalid(),
        QueryStateError::CumulativeLimit | QueryStateError::CapacityBusy => ToolErrorBody::new(
            ToolErrorCode::ResourceLimit,
            "the search cursor reached a service resource limit",
        ),
        QueryStateError::InvalidCapacity
        | QueryStateError::InvalidTtl
        | QueryStateError::ExpirationOverflow
        | QueryStateError::InvalidData(_)
        | QueryStateError::CounterOverflow => internal_error(),
    }
}

fn map_query_state_for_context(error: QueryStateError) -> ToolErrorBody {
    match error {
        QueryStateError::UnknownOrExpired => match_ref_invalid(),
        QueryStateError::CumulativeLimit | QueryStateError::CapacityBusy => ToolErrorBody::new(
            ToolErrorCode::ResourceLimit,
            "the context state reached a service resource limit",
        ),
        QueryStateError::InvalidCapacity
        | QueryStateError::InvalidTtl
        | QueryStateError::ExpirationOverflow
        | QueryStateError::InvalidData(_)
        | QueryStateError::CounterOverflow
        | QueryStateError::QueryMismatch
        | QueryStateError::Busy
        | QueryStateError::LeaseLost
        | QueryStateError::InvalidContinuation(_) => internal_error(),
    }
}

fn map_source_registry_for_search(error: SourceRegistryError) -> ToolErrorBody {
    match error {
        SourceRegistryError::UnknownSource(_) => ToolErrorBody::new(
            ToolErrorCode::UnknownSource,
            "one or more requested log sources are unavailable",
        ),
        SourceRegistryError::TooManyFiles { .. } => ToolErrorBody::new(
            ToolErrorCode::ResourceLimit,
            "the selected sources contain more files than the query limit",
        ),
        SourceRegistryError::FileChanged { .. }
        | SourceRegistryError::SnapshotSourceMismatch
        | SourceRegistryError::PathNotConfigured => file_changed(),
        SourceRegistryError::RootUnavailable { .. }
        | SourceRegistryError::ExplicitFileUnavailable { .. }
        | SourceRegistryError::DirectoryRuleInvalid { .. }
        | SourceRegistryError::DiscoveryFailed { .. }
        | SourceRegistryError::FileUnavailable { .. } => source_unavailable(),
        SourceRegistryError::InvalidConfiguration(_) => internal_error(),
    }
}

fn map_source_registry_for_context(error: SourceRegistryError) -> ToolErrorBody {
    match error {
        SourceRegistryError::UnknownSource(_)
        | SourceRegistryError::SnapshotSourceMismatch
        | SourceRegistryError::PathNotConfigured => match_ref_invalid(),
        SourceRegistryError::FileChanged { .. } => file_changed(),
        SourceRegistryError::TooManyFiles { .. } => ToolErrorBody::new(
            ToolErrorCode::ResourceLimit,
            "the context request reached a service resource limit",
        ),
        SourceRegistryError::RootUnavailable { .. }
        | SourceRegistryError::ExplicitFileUnavailable { .. }
        | SourceRegistryError::DirectoryRuleInvalid { .. }
        | SourceRegistryError::DiscoveryFailed { .. }
        | SourceRegistryError::FileUnavailable { .. } => source_unavailable(),
        SourceRegistryError::InvalidConfiguration(_) => internal_error(),
    }
}

fn map_scan_task(error: ScanTaskError) -> ToolErrorBody {
    match error {
        ScanTaskError::Scan(error) => match error {
            ScanError::InvalidKeyword
            | ScanError::InvalidStartPosition
            | ScanError::InvalidLimits(_) => ToolErrorBody::new(
                ToolErrorCode::InvalidArgument,
                "the search request is outside the v1 contract",
            ),
            ScanError::Io(_) => source_unavailable(),
            ScanError::PositionOverflow => internal_error(),
        },
        ScanTaskError::InvalidConcurrency
        | ScanTaskError::ExecutorClosed
        | ScanTaskError::Join(_) => internal_error(),
    }
}

fn map_context_read(error: ContextReadError) -> ToolErrorBody {
    match error {
        ContextReadError::InvalidRequest(_) => ToolErrorBody::new(
            ToolErrorCode::InvalidArgument,
            "the context request is outside the v1 contract",
        ),
        ContextReadError::InvalidLimits(_) | ContextReadError::CounterOverflow => internal_error(),
        ContextReadError::FileChanged => file_changed(),
        ContextReadError::MatchOutsideScanBudget | ContextReadError::MatchLineScanLimit => {
            ToolErrorBody::new(
                ToolErrorCode::ResourceLimit,
                "the requested context exceeds the bounded scan limit",
            )
        }
        ContextReadError::Cancelled => ToolErrorBody::new(
            ToolErrorCode::QueryCancelled,
            "the context request was cancelled",
        ),
        ContextReadError::DeadlineExceeded => ToolErrorBody::new(
            ToolErrorCode::DeadlineExceeded,
            "the context request exceeded its deadline",
        ),
        ContextReadError::InvalidReference(error) => map_query_state_for_context(error),
        ContextReadError::SourceRegistry(error) => map_source_registry_for_context(error),
        ContextReadError::Io(_) => source_unavailable(),
    }
}

fn map_context_task(error: ContextTaskError) -> ToolErrorBody {
    match error {
        ContextTaskError::InvalidConcurrency
        | ContextTaskError::ExecutorClosed
        | ContextTaskError::Join(_) => internal_error(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn error_json_uses_frozen_wire_codes() {
        let body = cursor_invalid();
        let value = serde_json::to_value(&body).expect("error should serialize");

        assert_eq!(value["code"], "CURSOR_INVALID");
        assert_eq!(value["retryable"], false);
        assert!(value.get("details").is_none());
    }

    #[test]
    fn messages_do_not_contain_server_paths() {
        let bodies = [
            source_unavailable(),
            file_changed(),
            match_ref_invalid(),
            internal_error(),
            ToolErrorBody::response_limit(),
        ];
        for body in bodies {
            assert!(!body.message.contains('/'));
            assert!(!body.to_json_text().contains("/var/"));
        }
    }

    #[test]
    fn retryable_semantics_match_v1_contract() {
        assert!(source_unavailable().retryable);
        assert!(file_changed().retryable);
        assert!(internal_error().retryable);
        assert!(!cursor_invalid().retryable);
        assert!(!match_ref_invalid().retryable);
        assert!(!ToolErrorBody::response_limit().retryable);
    }
}
