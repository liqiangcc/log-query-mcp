use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

use crate::{
    ContextLine as ServiceContextLine, RegisteredQueryMatch, SourceDescriptor,
    StatefulContextResult, StatefulQueryPage,
};

#[derive(Debug, Clone, Default, PartialEq, Eq, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct ListLogSourcesRequest {}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct LogSource {
    pub source_id: String,
    pub name: String,
    pub description: String,
    pub service: String,
    pub environment: String,
    pub tags: Vec<String>,
}

impl From<SourceDescriptor> for LogSource {
    fn from(source: SourceDescriptor) -> Self {
        Self {
            source_id: source.source_id,
            name: source.name,
            description: source.description,
            service: source.service,
            environment: source.environment,
            tags: source.tags,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct ListLogSourcesResponse {
    pub sources: Vec<LogSource>,
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum SearchOrder {
    #[default]
    OldestFirst,
}

#[derive(Debug, Clone, PartialEq, Eq, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct SearchLogsRequest {
    pub source_ids: Vec<String>,
    pub keyword: String,
    #[serde(default)]
    pub case_sensitive: bool,
    #[serde(default)]
    pub start_time: Option<String>,
    #[serde(default)]
    pub end_time: Option<String>,
    #[serde(default)]
    pub order: SearchOrder,
    #[serde(default)]
    pub max_results: Option<usize>,
    #[serde(default)]
    pub cursor: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct LogMatch {
    pub match_ref: String,
    pub source_id: String,
    pub file_id: String,
    pub file_name: String,
    pub line_number: u64,
    pub timestamp: Option<String>,
    pub content: String,
    pub content_truncated: bool,
}

impl From<RegisteredQueryMatch> for LogMatch {
    fn from(value: RegisteredQueryMatch) -> Self {
        let timestamp = value.timestamp_rfc3339();
        Self {
            match_ref: value.match_ref,
            source_id: value.source_id,
            file_id: value.file_id,
            file_name: value.file_name,
            line_number: value.line_number,
            timestamp,
            content: value.content,
            content_truncated: value.content_truncated,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct SearchLogsResponse {
    pub results: Vec<LogMatch>,
    pub truncated: bool,
    pub next_cursor: Option<String>,
}

impl From<StatefulQueryPage> for SearchLogsResponse {
    fn from(page: StatefulQueryPage) -> Self {
        Self {
            results: page.results.into_iter().map(LogMatch::from).collect(),
            truncated: page.truncated,
            next_cursor: page.next_cursor,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct GetLogContextRequest {
    pub match_ref: String,
    #[serde(default)]
    pub before_lines: usize,
    #[serde(default)]
    pub after_lines: usize,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct ContextLine {
    pub line_number: u64,
    pub content: String,
}

impl From<ServiceContextLine> for ContextLine {
    fn from(line: ServiceContextLine) -> Self {
        Self {
            line_number: line.line_number,
            content: line.content,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct GetLogContextResponse {
    pub source_id: String,
    pub file_id: String,
    pub file_name: String,
    pub start_line: u64,
    pub end_line: u64,
    pub lines: Vec<ContextLine>,
    pub truncated: bool,
}

impl From<StatefulContextResult> for GetLogContextResponse {
    fn from(value: StatefulContextResult) -> Self {
        Self {
            source_id: value.source_id,
            file_id: value.file_id,
            file_name: value.file_name,
            start_line: value.start_line,
            end_line: value.end_line,
            lines: value.lines.into_iter().map(ContextLine::from).collect(),
            truncated: value.truncated,
        }
    }
}
