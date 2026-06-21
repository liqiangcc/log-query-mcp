use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

pub const DEFAULT_SEARCH_RESULTS: usize = 50;
pub const MAX_MCP_SOURCE_IDS: usize = 10;
pub const MAX_MCP_RESULTS: usize = 200;
pub const MAX_MCP_CONTEXT_LINES_PER_SIDE: usize = 50;
pub const MAX_MCP_TOKEN_CHARS: usize = 512;

fn default_search_results() -> usize {
    DEFAULT_SEARCH_RESULTS
}

fn default_order() -> String {
    "oldest_first".to_owned()
}

#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct LogSource {
    #[schemars(length(min = 1, max = 128))]
    pub source_id: String,
    #[schemars(length(min = 1, max = 256))]
    pub name: String,
    #[schemars(length(max = 1024))]
    pub description: String,
    #[schemars(length(min = 1, max = 256))]
    pub service: String,
    #[schemars(length(min = 1, max = 128))]
    pub environment: String,
    #[schemars(length(max = 32), inner(length(min = 1, max = 64)))]
    pub tags: Vec<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct ListLogSourcesResponse {
    pub sources: Vec<LogSource>,
}

#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct SearchLogsRequest {
    /// One to ten configured source identifiers. Server paths are not accepted.
    #[schemars(length(min = 1, max = 10), inner(length(min = 1, max = 128)))]
    pub source_ids: Vec<String>,

    /// Literal UTF-8 substring. It is not a regex, glob, query language or shell expression.
    #[schemars(length(min = 1, max = 256), example = &"traceId=abc123")]
    pub keyword: String,

    /// Whether matching is case-sensitive. False performs ASCII-only case folding.
    #[serde(default)]
    pub case_sensitive: bool,

    /// Optional inclusive RFC 3339 lower bound.
    #[schemars(length(min = 1, max = 64), extend("format" = "date-time"))]
    pub start_time: Option<String>,

    /// Optional exclusive RFC 3339 upper bound.
    #[schemars(length(min = 1, max = 64), extend("format" = "date-time"))]
    pub end_time: Option<String>,

    /// v1 accepts only oldest_first. Other values produce INVALID_ARGUMENT.
    #[serde(default = "default_order")]
    #[schemars(length(min = 1, max = 32), extend("enum" = ["oldest_first"]))]
    pub order: String,

    /// Maximum page size. The server may apply a smaller configured limit.
    #[serde(default = "default_search_results")]
    #[schemars(range(min = 1, max = 200))]
    pub max_results: usize,

    /// Opaque cursor returned by a previous call with identical query conditions.
    #[schemars(length(min = 1, max = 512))]
    pub cursor: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct LogMatch {
    #[schemars(length(min = 1, max = 512))]
    pub match_ref: String,
    #[schemars(length(min = 1, max = 128))]
    pub source_id: String,
    #[schemars(length(min = 1, max = 256))]
    pub file_id: String,
    #[schemars(length(min = 1, max = 1024))]
    pub file_name: String,
    #[schemars(range(min = 1))]
    pub line_number: u64,
    #[schemars(extend("format" = "date-time"))]
    pub timestamp: Option<String>,
    pub content: String,
    pub content_truncated: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct SearchLogsResponse {
    #[schemars(length(max = 200))]
    pub results: Vec<LogMatch>,
    pub truncated: bool,
    #[schemars(length(min = 1, max = 512))]
    pub next_cursor: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct GetLogContextRequest {
    /// Opaque reference returned by search_logs. Paths and arbitrary line numbers are not accepted.
    #[schemars(length(min = 1, max = 512))]
    pub match_ref: String,

    /// Number of preceding lines, from zero through fifty.
    #[serde(default)]
    #[schemars(range(min = 0, max = 50))]
    pub before_lines: usize,

    /// Number of following lines, from zero through fifty.
    #[serde(default)]
    #[schemars(range(min = 0, max = 50))]
    pub after_lines: usize,
}

#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct ContextLineResponse {
    #[schemars(range(min = 1))]
    pub line_number: u64,
    pub content: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct GetLogContextResponse {
    #[schemars(length(min = 1, max = 128))]
    pub source_id: String,
    #[schemars(length(min = 1, max = 256))]
    pub file_id: String,
    #[schemars(length(min = 1, max = 1024))]
    pub file_name: String,
    #[schemars(range(min = 1))]
    pub start_line: u64,
    #[schemars(range(min = 1))]
    pub end_line: u64,
    #[schemars(length(max = 101))]
    pub lines: Vec<ContextLineResponse>,
    pub truncated: bool,
}

#[cfg(test)]
mod tests {
    use serde_json::Value;

    use super::*;

    fn schema_value<T: JsonSchema>() -> Value {
        serde_json::to_value(schemars::schema_for!(T)).expect("schema should serialize")
    }

    #[test]
    fn search_request_defaults_are_stable() {
        let request: SearchLogsRequest = serde_json::from_value(serde_json::json!({
            "source_ids": ["payment-test"],
            "keyword": "traceId=abc123",
            "start_time": null,
            "end_time": null,
            "cursor": null
        }))
        .expect("request should deserialize");

        assert!(!request.case_sensitive);
        assert_eq!(request.order, "oldest_first");
        assert_eq!(request.max_results, DEFAULT_SEARCH_RESULTS);
    }

    #[test]
    fn request_schemas_expose_v1_limits_and_reject_unknown_fields() {
        let search = schema_value::<SearchLogsRequest>();
        assert_eq!(search["additionalProperties"], false);
        assert_eq!(search["properties"]["source_ids"]["minItems"], 1);
        assert_eq!(search["properties"]["source_ids"]["maxItems"], 10);
        assert_eq!(search["properties"]["keyword"]["maxLength"], 256);
        assert_eq!(search["properties"]["max_results"]["maximum"], 200);
        assert_eq!(search["properties"]["max_results"]["default"], 50);

        let context = schema_value::<GetLogContextRequest>();
        assert_eq!(context["additionalProperties"], false);
        assert_eq!(context["properties"]["before_lines"]["maximum"], 50);
        assert_eq!(context["properties"]["after_lines"]["maximum"], 50);
    }

    #[test]
    fn deserialization_accepts_newest_first_for_stable_tool_error_mapping() {
        let request: SearchLogsRequest = serde_json::from_value(serde_json::json!({
            "source_ids": ["payment-test"],
            "keyword": "abc123",
            "order": "newest_first",
            "start_time": null,
            "end_time": null,
            "cursor": null
        }))
        .expect("unsupported order is validated by the tool layer");

        assert_eq!(request.order, "newest_first");
    }
}
