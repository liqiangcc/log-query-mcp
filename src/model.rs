use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

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

#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum ResultOrder {
    OldestFirst,
    NewestFirst,
}

#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema, PartialEq, Eq)]
pub struct SearchLogsRequest {
    /// One or more configured source identifiers. Server paths are not accepted.
    pub source_ids: Vec<String>,
    /// Literal UTF-8 substring to search for. It is not treated as a regex or shell expression.
    pub keyword: String,
    /// Whether matching is case-sensitive. Defaults to false.
    pub case_sensitive: Option<bool>,
    /// Optional RFC 3339 lower time bound. The POC accepts the field but does not filter by it yet.
    pub start_time: Option<String>,
    /// Optional RFC 3339 upper time bound. The POC accepts the field but does not filter by it yet.
    pub end_time: Option<String>,
    /// Result order. Defaults to oldest_first.
    pub order: Option<ResultOrder>,
    /// Maximum number of results requested. The server hard limit is 200.
    pub max_results: Option<usize>,
    /// Opaque pagination cursor. Pagination is intentionally not implemented in this first POC.
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
pub struct GetLogContextRequest {
    /// Opaque reference previously returned by search_logs.
    pub match_ref: String,
    /// Number of lines before the match. The POC hard limit is 50.
    pub before_lines: usize,
    /// Number of lines after the match. The POC hard limit is 50.
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
