use std::{cmp::Ordering, str};

use chrono::{DateTime, FixedOffset, NaiveDateTime, SecondsFormat, TimeZone};
use thiserror::Error;

use crate::ResultOrder;

pub const MAX_TIMESTAMP_PREFIX_BYTES: usize = 128;
pub const MAX_TIMESTAMP_FORMAT_CHARS: usize = 128;

#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord)]
pub struct LogTimestamp(DateTime<FixedOffset>);

impl LogTimestamp {
    #[must_use]
    pub fn unix_timestamp_nanos(&self) -> i128 {
        i128::from(self.0.timestamp()) * 1_000_000_000
            + i128::from(self.0.timestamp_subsec_nanos())
    }

    #[must_use]
    pub fn to_rfc3339(&self) -> String {
        self.0
            .to_rfc3339_opts(SecondsFormat::AutoSi, false)
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum TimestampRule {
    Rfc3339 {
        max_prefix_bytes: usize,
    },
    CustomFixedOffset {
        format: String,
        prefix_bytes: usize,
        offset: FixedOffset,
    },
}

impl TimestampRule {
    pub fn rfc3339(max_prefix_bytes: usize) -> Result<Self, TimestampError> {
        validate_prefix_limit(max_prefix_bytes)?;
        Ok(Self::Rfc3339 { max_prefix_bytes })
    }

    pub fn custom_fixed_offset(
        format: impl Into<String>,
        prefix_bytes: usize,
        utc_offset_seconds: i32,
    ) -> Result<Self, TimestampError> {
        validate_prefix_limit(prefix_bytes)?;
        let format = format.into();
        if format.is_empty() || format.chars().count() > MAX_TIMESTAMP_FORMAT_CHARS {
            return Err(TimestampError::InvalidRule(
                "timestamp format length is outside the server limit",
            ));
        }
        let offset = FixedOffset::east_opt(utc_offset_seconds).ok_or(
            TimestampError::InvalidRule("UTC offset is outside the supported range"),
        )?;

        Ok(Self::CustomFixedOffset {
            format,
            prefix_bytes,
            offset,
        })
    }

    #[must_use]
    pub fn parse_line(&self, line: &[u8]) -> TimestampParse {
        if !looks_like_timestamp_prefix(line) {
            return TimestampParse::NoTimestamp;
        }

        match self {
            Self::Rfc3339 { max_prefix_bytes } => {
                let end = line
                    .iter()
                    .position(u8::is_ascii_whitespace)
                    .unwrap_or(line.len());
                if end == 0 || end > *max_prefix_bytes {
                    return TimestampParse::Malformed;
                }
                let Ok(prefix) = str::from_utf8(&line[..end]) else {
                    return TimestampParse::Malformed;
                };
                DateTime::parse_from_rfc3339(prefix)
                    .map(LogTimestamp)
                    .map_or(TimestampParse::Malformed, TimestampParse::Parsed)
            }
            Self::CustomFixedOffset {
                format,
                prefix_bytes,
                offset,
            } => {
                let Some(prefix) = line.get(..*prefix_bytes) else {
                    return TimestampParse::Malformed;
                };
                let Ok(prefix) = str::from_utf8(prefix) else {
                    return TimestampParse::Malformed;
                };
                let Ok(local) = NaiveDateTime::parse_from_str(prefix, format) else {
                    return TimestampParse::Malformed;
                };
                offset
                    .from_local_datetime(&local)
                    .single()
                    .map(LogTimestamp)
                    .map_or(TimestampParse::Malformed, TimestampParse::Parsed)
            }
        }
    }
}

fn validate_prefix_limit(prefix_bytes: usize) -> Result<(), TimestampError> {
    if prefix_bytes == 0 || prefix_bytes > MAX_TIMESTAMP_PREFIX_BYTES {
        return Err(TimestampError::InvalidRule(
            "timestamp prefix length is outside the server limit",
        ));
    }
    Ok(())
}

fn looks_like_timestamp_prefix(line: &[u8]) -> bool {
    line.len() >= 5
        && line[..4].iter().all(u8::is_ascii_digit)
        && matches!(line[4], b'-' | b'/')
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum TimestampParse {
    Parsed(LogTimestamp),
    NoTimestamp,
    Malformed,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum LineTimestampOrigin {
    Explicit,
    Inherited,
    Missing,
    Malformed,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ObservedLineTimestamp {
    pub timestamp: Option<LogTimestamp>,
    pub origin: LineTimestampOrigin,
}

#[derive(Debug, Clone)]
pub struct TimestampTracker {
    rule: TimestampRule,
    last_timestamp: Option<LogTimestamp>,
}

impl TimestampTracker {
    #[must_use]
    pub fn new(rule: TimestampRule) -> Self {
        Self {
            rule,
            last_timestamp: None,
        }
    }

    #[must_use]
    pub fn observe_line(&mut self, line: &[u8]) -> ObservedLineTimestamp {
        match self.rule.parse_line(line) {
            TimestampParse::Parsed(timestamp) => {
                self.last_timestamp = Some(timestamp.clone());
                ObservedLineTimestamp {
                    timestamp: Some(timestamp),
                    origin: LineTimestampOrigin::Explicit,
                }
            }
            TimestampParse::NoTimestamp => match self.last_timestamp.clone() {
                Some(timestamp) => ObservedLineTimestamp {
                    timestamp: Some(timestamp),
                    origin: LineTimestampOrigin::Inherited,
                },
                None => ObservedLineTimestamp {
                    timestamp: None,
                    origin: LineTimestampOrigin::Missing,
                },
            },
            TimestampParse::Malformed => {
                self.last_timestamp = None;
                ObservedLineTimestamp {
                    timestamp: None,
                    origin: LineTimestampOrigin::Malformed,
                }
            }
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TimeRange {
    pub start: Option<LogTimestamp>,
    pub end: Option<LogTimestamp>,
}

impl TimeRange {
    pub fn parse(start: Option<&str>, end: Option<&str>) -> Result<Self, TimestampError> {
        let start = start.map(parse_request_bound).transpose()?;
        let end = end.map(parse_request_bound).transpose()?;
        if start
            .as_ref()
            .zip(end.as_ref())
            .is_some_and(|(start, end)| start >= end)
        {
            return Err(TimestampError::InvalidRange(
                "start_time must be earlier than end_time",
            ));
        }
        Ok(Self { start, end })
    }

    #[must_use]
    pub fn contains(&self, timestamp: &LogTimestamp) -> bool {
        self.start
            .as_ref()
            .is_none_or(|start| timestamp >= start)
            && self
                .end
                .as_ref()
                .is_none_or(|end| timestamp < end)
    }

    #[must_use]
    pub fn classify(&self, observed: &ObservedLineTimestamp) -> TimeFilterDecision {
        let Some(timestamp) = observed.timestamp.as_ref() else {
            return match observed.origin {
                LineTimestampOrigin::Malformed => TimeFilterDecision::MalformedTimestamp,
                LineTimestampOrigin::Explicit
                | LineTimestampOrigin::Inherited
                | LineTimestampOrigin::Missing => TimeFilterDecision::UnknownTimestamp,
            };
        };

        if self
            .start
            .as_ref()
            .is_some_and(|start| timestamp < start)
        {
            TimeFilterDecision::BeforeRange
        } else if self.end.as_ref().is_some_and(|end| timestamp >= end) {
            TimeFilterDecision::AtOrAfterEnd
        } else {
            TimeFilterDecision::Include
        }
    }
}

fn parse_request_bound(value: &str) -> Result<LogTimestamp, TimestampError> {
    DateTime::parse_from_rfc3339(value)
        .map(LogTimestamp)
        .map_err(|_| TimestampError::InvalidRange("time bounds must use RFC 3339"))
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TimeFilterDecision {
    Include,
    BeforeRange,
    AtOrAfterEnd,
    UnknownTimestamp,
    MalformedTimestamp,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct StableLogPosition {
    pub timestamp: Option<LogTimestamp>,
    pub source_index: usize,
    pub file_index: usize,
    pub line_number: u64,
}

#[must_use]
pub fn compare_log_positions(
    left: &StableLogPosition,
    right: &StableLogPosition,
    order: ResultOrder,
) -> Ordering {
    let timestamp_order = match (&left.timestamp, &right.timestamp) {
        (Some(left), Some(right)) => match order {
            ResultOrder::OldestFirst => left.cmp(right),
            ResultOrder::NewestFirst => right.cmp(left),
        },
        (Some(_), None) => Ordering::Less,
        (None, Some(_)) => Ordering::Greater,
        (None, None) => Ordering::Equal,
    };

    timestamp_order.then_with(|| {
        left.source_index
            .cmp(&right.source_index)
            .then_with(|| left.file_index.cmp(&right.file_index))
            .then_with(|| left.line_number.cmp(&right.line_number))
    })
}

#[derive(Debug, Error)]
pub enum TimestampError {
    #[error("invalid timestamp rule: {0}")]
    InvalidRule(&'static str),

    #[error("invalid time range: {0}")]
    InvalidRange(&'static str),
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_rfc3339_prefix_and_normalizes_equal_instants() {
        let rule = TimestampRule::rfc3339(64).expect("rule should be valid");
        let first = match rule.parse_line(
            b"2026-06-19T14:20:03.125+09:00 ERROR traceId=abc123",
        ) {
            TimestampParse::Parsed(timestamp) => timestamp,
            other => panic!("unexpected parse result: {other:?}"),
        };
        let second = match rule.parse_line(b"2026-06-19T05:20:03.125Z ERROR") {
            TimestampParse::Parsed(timestamp) => timestamp,
            other => panic!("unexpected parse result: {other:?}"),
        };

        assert_eq!(first, second);
        assert_eq!(first.to_rfc3339(), "2026-06-19T14:20:03.125+09:00");
    }

    #[test]
    fn parses_custom_local_timestamp_with_fixed_offset() {
        let rule = TimestampRule::custom_fixed_offset(
            "%Y-%m-%d %H:%M:%S%.f",
            23,
            9 * 60 * 60,
        )
        .expect("rule should be valid");
        let timestamp = match rule.parse_line(b"2026-06-19 14:20:03.125 ERROR payment failed") {
            TimestampParse::Parsed(timestamp) => timestamp,
            other => panic!("unexpected parse result: {other:?}"),
        };

        assert_eq!(timestamp.to_rfc3339(), "2026-06-19T14:20:03.125+09:00");
    }

    #[test]
    fn stack_trace_lines_inherit_previous_timestamp() {
        let rule = TimestampRule::rfc3339(64).expect("rule should be valid");
        let mut tracker = TimestampTracker::new(rule);
        let explicit = tracker.observe_line(b"2026-06-19T14:20:03+09:00 ERROR failure");
        let inherited = tracker.observe_line(b"    at payment::authorize(payment.rs:42)");
        let caused_by = tracker.observe_line(b"Caused by: forbidden");

        assert_eq!(explicit.origin, LineTimestampOrigin::Explicit);
        assert_eq!(inherited.origin, LineTimestampOrigin::Inherited);
        assert_eq!(caused_by.origin, LineTimestampOrigin::Inherited);
        assert_eq!(explicit.timestamp, inherited.timestamp);
        assert_eq!(explicit.timestamp, caused_by.timestamp);
    }

    #[test]
    fn malformed_timestamp_clears_inheritance() {
        let rule = TimestampRule::rfc3339(64).expect("rule should be valid");
        let mut tracker = TimestampTracker::new(rule);
        tracker.observe_line(b"2026-06-19T14:20:03+09:00 ERROR failure");
        let malformed = tracker.observe_line(b"2026-99-99T99:99:99+09:00 ERROR malformed");
        let following = tracker.observe_line(b"    at payment::authorize(payment.rs:42)");

        assert_eq!(malformed.origin, LineTimestampOrigin::Malformed);
        assert_eq!(following.origin, LineTimestampOrigin::Missing);
        assert!(following.timestamp.is_none());
    }

    #[test]
    fn time_range_is_start_inclusive_and_end_exclusive() {
        let range = TimeRange::parse(
            Some("2026-06-19T14:20:00+09:00"),
            Some("2026-06-19T14:21:00+09:00"),
        )
        .expect("range should be valid");
        let start = parse_request_bound("2026-06-19T14:20:00+09:00")
            .expect("bound should parse");
        let inside = parse_request_bound("2026-06-19T14:20:59.999+09:00")
            .expect("bound should parse");
        let end = parse_request_bound("2026-06-19T14:21:00+09:00")
            .expect("bound should parse");

        assert!(range.contains(&start));
        assert!(range.contains(&inside));
        assert!(!range.contains(&end));
    }

    #[test]
    fn unknown_and_malformed_lines_are_not_silently_included() {
        let range = TimeRange::parse(Some("2026-06-19T14:20:00+09:00"), None)
            .expect("range should be valid");

        assert_eq!(
            range.classify(&ObservedLineTimestamp {
                timestamp: None,
                origin: LineTimestampOrigin::Missing,
            }),
            TimeFilterDecision::UnknownTimestamp
        );
        assert_eq!(
            range.classify(&ObservedLineTimestamp {
                timestamp: None,
                origin: LineTimestampOrigin::Malformed,
            }),
            TimeFilterDecision::MalformedTimestamp
        );
    }

    #[test]
    fn cross_service_sort_uses_timestamp_then_stable_position() {
        let timestamp = parse_request_bound("2026-06-19T14:20:00+09:00")
            .expect("bound should parse");
        let mut positions = [
            StableLogPosition {
                timestamp: None,
                source_index: 0,
                file_index: 0,
                line_number: 1,
            },
            StableLogPosition {
                timestamp: Some(timestamp.clone()),
                source_index: 1,
                file_index: 0,
                line_number: 5,
            },
            StableLogPosition {
                timestamp: Some(timestamp),
                source_index: 0,
                file_index: 1,
                line_number: 2,
            },
        ];
        positions.sort_by(|left, right| {
            compare_log_positions(left, right, ResultOrder::OldestFirst)
        });

        assert_eq!(positions[0].source_index, 0);
        assert_eq!(positions[1].source_index, 1);
        assert!(positions[2].timestamp.is_none());
    }

    #[test]
    fn rejects_invalid_rules_and_ranges() {
        assert!(TimestampRule::rfc3339(0).is_err());
        assert!(TimestampRule::custom_fixed_offset("", 23, 0).is_err());
        assert!(TimestampRule::custom_fixed_offset(
            "%Y-%m-%d %H:%M:%S",
            19,
            100_000,
        )
        .is_err());
        assert!(TimeRange::parse(
            Some("2026-06-19T14:21:00+09:00"),
            Some("2026-06-19T14:20:00+09:00"),
        )
        .is_err());
    }
}
