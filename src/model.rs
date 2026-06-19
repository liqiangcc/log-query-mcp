use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

pub const MAX_SOURCES: usize = 10;
pub const MAX_SOURCE_ID_CHARS: usize = 128;
pub const MAX_KEYWORD_CHARS: usize = 256;
pub const DEFAULT_MAX_RESULTS: usize = 50;
pub const MAX_RESULTS: usize = 200;
pub const MAX_CONTEXT_LINES_PER_SIDE: usize = 50;
pub const MAX_REFERENCE_CHARS: usize = 512;

const fn default_max_results() -> usize {
    DEFAULT_MAX_RESULTS
}

#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema, PartialEq, Eq)]
pub struct LogSource {
    pub source_id: String,
    pub name: String,
    pub description: String,
    pub service: String,
    pub environment: String,
    pub tags: Vec<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema, PartialEq, Eq)]
pub struct ListLogSourcesResponse {
    pub sources: Vec<LogSource>,
}

#[derive(Debug, Clone, Copy, Default, Serialize, Deserialize, JsonSchema, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum ResultOrder {
    #[default]
    OldestFirst,
    NewestFirst,
}

#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct SearchLogsRequest {
    /// One to ten configured source identifiers. Server paths are not accepted.
    #[schemars(length(min = 1, max = 10), inner(length(min = 1, max = 128)))]
    pub source_ids: Vec<String>,

    /// Literal UTF-8 substring to search for. It is not treated as a regex, glob, query language or shell expression.
    #[schemars(length(min = 1, max = 256), example = &"traceId=abc123")]
    pub keyword: String,

    /// Whether matching is case-sensitive. Defaults to false.
    #[serde(default)]
    pub case_sensitive: bool,

    /// Optional RFC 3339 lower time bound. The current POC validates the schema but does not apply time filtering yet.
    #[schemars(length(min = 1, max = 64), extend("format" = "date-time"))]
    pub start_time: Option<String>,

    /// Optional RFC 3339 upper time bound. The current POC validates the schema but does not apply time filtering yet.
    #[schemars(length(min = 1, max = 64), extend("format" = "date-time"))]
    pub end_time: Option<String>,

    /// Result order. Defaults to oldest_first.
    #[serde(default)]
    pub order: ResultOrder,

    /// Maximum number of results requested. Defaults to 50; the server hard limit is 200.
    #[serde(default = "default_max_results")]
    #[schemars(range(min = 1, max = 200))]
    pub max_results: usize,

    /// Opaque pagination cursor. It cannot be combined with changed query conditions.
    #[schemars(length(min = 1, max = 512))]
    pub cursor: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema, PartialEq, Eq)]
pub struct LogMatch {
    pub match_ref: String,
    pub source_id: String,
    pub file_id: String,
    pub file_name: String,
    pub line_number: usize,
    pub timestamp: Option<String>,
    pub content: String,
    pub content_truncated: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema, PartialEq, Eq)]
pub struct SearchLogsResponse {
    pub results: Vec<LogMatch>,
    pub truncated: bool,
    pub next_cursor: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct GetLogContextRequest {
    /// Opaque reference previously returned by search_logs. File paths and arbitrary line numbers are not accepted.
    #[schemars(length(min = 1, max = 512))]
    pub match_ref: String,

    /// Number of lines before the match. Defaults to zero; the server hard limit is 50.
    #[serde(default)]
    #[schemars(range(min = 0, max = 50))]
    pub before_lines: usize,

    /// Number of lines after the match. Defaults to zero; the server hard limit is 50.
    #[serde(default)]
    #[schemars(range(min = 0, max = 50))]
    pub after_lines: usize,
}

#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema, PartialEq, Eq)]
pub struct ContextLine {
    pub line_number: usize,
    pub content: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema, PartialEq, Eq)]
pub struct GetLogContextResponse {
    pub source_id: String,
    pub file_id: String,
    pub file_name: String,
    pub start_line: usize,
    pub end_line: usize,
    pub lines: Vec<ContextLine>,
    pub truncated: bool,
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::Value;

    fn schema_value<T: JsonSchema>() -> Value {
        serde_json::to_value(schemars::schema_for!(T)).expect("schema should serialize")
    }

    #[test]
    fn search_schema_exposes_hard_limits() {
        let schema = schema_value::<SearchLogsRequest>();
        let properties = &schema["properties"];

        assert_eq!(schema["additionalProperties"], false);
        assert_eq!(properties["source_ids"]["minItems"], 1);
        assert_eq!(properties["source_ids"]["maxItems"], 10);
        assert_eq!(properties["source_ids"]["items"]["minLength"], 1);
        assert_eq!(properties["source_ids"]["items"]["maxLength"], 128);
        assert_eq!(properties["keyword"]["minLength"], 1);
        assert_eq!(properties["keyword"]["maxLength"], 256);
        assert_eq!(properties["max_results"]["minimum"], 1);
        assert_eq!(properties["max_results"]["maximum"], 200);
        assert_eq!(properties["max_results"]["default"], 50);
    }

    #[test]
    fn context_schema_exposes_line_limits() {
        let schema = schema_value::<GetLogContextRequest>();
        let properties = &schema["properties"];

        assert_eq!(schema["additionalProperties"], false);
        assert_eq!(properties["before_lines"]["minimum"], 0);
        assert_eq!(properties["before_lines"]["maximum"], 50);
        assert_eq!(properties["after_lines"]["minimum"], 0);
        assert_eq!(properties["after_lines"]["maximum"], 50);
        assert_eq!(properties["match_ref"]["minLength"], 1);
        assert_eq!(properties["match_ref"]["maxLength"], 512);
    }

    #[test]
    fn request_defaults_are_stable() {
        let request: SearchLogsRequest = serde_json::from_value(serde_json::json!({
            "source_ids": ["payment-test"],
            "keyword": "abc123",
            "start_time": null,
            "end_time": null,
            "cursor": null
        }))
        .expect("request should deserialize");

        assert!(!request.case_sensitive);
        assert_eq!(request.order, ResultOrder::OldestFirst);
        assert_eq!(request.max_results, DEFAULT_MAX_RESULTS);
    }
}
