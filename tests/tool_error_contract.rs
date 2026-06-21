use serde::{Serialize, Serializer, ser::Error as _};
use serde_json::{Value, json};

use log_query_mcp::{
    QueryStateError, SourceRegistryError, StatefulContextError, StatefulQueryError, ToolError,
    ToolErrorCode, serialize_with_limit,
};

#[test]
fn tool_error_serializes_to_compact_frozen_shape() {
    let error = ToolError::new(ToolErrorCode::UnknownSource);
    let json = error.to_json_string().expect("tool error should serialize");

    assert_eq!(
        json,
        r#"{"code":"UNKNOWN_SOURCE","message":"one or more requested log sources are unavailable","retryable":false}"#
    );
    assert!(!json.contains('\n'));
    assert!(!json.contains("details"));
    assert!(!json.contains("path"));
    assert!(!json.contains("cause"));
    assert!(!json.contains("backtrace"));

    let value: Value = serde_json::from_str(&json).expect("tool error json should parse");
    assert_eq!(
        value,
        json!({
            "code": "UNKNOWN_SOURCE",
            "message": "one or more requested log sources are unavailable",
            "retryable": false
        })
    );
    assert_eq!(value.as_object().expect("object").len(), 3);
}

#[test]
fn response_limit_returns_compact_json_or_resource_limit() {
    let value = json!({
        "results": [],
        "truncated": false,
        "next_cursor": null
    });
    let serialized = serialize_with_limit(&value, 128).expect("response should fit");

    assert_eq!(
        serialized,
        r#"{"next_cursor":null,"results":[],"truncated":false}"#
    );
    assert!(!serialized.contains('\n'));

    let error = serialize_with_limit(&value, 12).expect_err("response should be rejected");
    assert_eq!(error.code, ToolErrorCode::ResourceLimit);
    assert_eq!(
        error.message,
        ToolErrorCode::ResourceLimit.default_message()
    );
    assert!(!error.retryable);
}

#[test]
fn response_limit_maps_serialization_failure_to_internal_error() {
    struct FailingSerialize;

    impl Serialize for FailingSerialize {
        fn serialize<S>(&self, _serializer: S) -> Result<S::Ok, S::Error>
        where
            S: Serializer,
        {
            Err(S::Error::custom("secret serializer failure"))
        }
    }

    let error =
        serialize_with_limit(&FailingSerialize, 1024).expect_err("serialization should fail");

    assert_eq!(error.code, ToolErrorCode::InternalError);
    assert_eq!(
        error.message,
        ToolErrorCode::InternalError.default_message()
    );
    assert!(!error.message.contains("secret serializer failure"));
    assert!(error.retryable);
}

#[test]
fn stateful_query_errors_map_to_sanitized_tool_errors() {
    let invalid = ToolError::from(StatefulQueryError::InvalidArgument(
        "bad source /var/log/payment/application.log",
    ));
    assert_eq!(invalid.code, ToolErrorCode::InvalidArgument);
    assert_eq!(
        invalid.message,
        ToolErrorCode::InvalidArgument.default_message()
    );
    assert!(!invalid.message.contains("/var/log"));

    let unknown = ToolError::from(StatefulQueryError::SourceRegistry(
        SourceRegistryError::UnknownSource("missing-source".to_owned()),
    ));
    assert_eq!(unknown.code, ToolErrorCode::UnknownSource);
    assert_eq!(
        unknown.message,
        ToolErrorCode::UnknownSource.default_message()
    );
    assert!(!unknown.message.contains("missing-source"));

    let cursor = ToolError::from(StatefulQueryError::QueryState(
        QueryStateError::UnknownOrExpired,
    ));
    assert_eq!(cursor.code, ToolErrorCode::CursorInvalid);
}

#[test]
fn context_query_state_errors_map_to_match_ref_invalid() {
    let context = ToolError::from(StatefulContextError::QueryState(
        QueryStateError::UnknownOrExpired,
    ));

    assert_eq!(context.code, ToolErrorCode::MatchRefInvalid);
    assert_eq!(
        context.message,
        ToolErrorCode::MatchRefInvalid.default_message()
    );
    assert!(!context.retryable);
}

#[test]
fn resource_and_file_change_errors_use_frozen_codes() {
    let too_many_files = ToolError::from(SourceRegistryError::TooManyFiles {
        source_id: "payment-test".to_owned(),
        limit: 500,
    });
    assert_eq!(too_many_files.code, ToolErrorCode::ResourceLimit);
    assert!(!too_many_files.message.contains("payment-test"));

    let changed = ToolError::from(SourceRegistryError::FileChanged {
        source_id: "payment-test".to_owned(),
        file_id: "file_secret".to_owned(),
    });
    assert_eq!(changed.code, ToolErrorCode::FileChanged);
    assert_eq!(
        changed.message,
        ToolErrorCode::FileChanged.default_message()
    );
    assert!(!changed.message.contains("file_secret"));
}
