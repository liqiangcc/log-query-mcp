use chrono::{DateTime, FixedOffset, NaiveDate, NaiveDateTime, TimeZone, format::ParseErrorKind};
use thiserror::Error;

use crate::TimestampRule;

pub const MAX_TIMESTAMP_PREFIX_BYTES: usize = 256;
pub const MAX_TIMESTAMP_FORMAT_CHARS: usize = 128;

#[derive(Debug, Clone)]
pub struct TimestampParser {
    rule: TimestampRule,
}

impl TimestampParser {
    pub fn new(rule: &TimestampRule) -> Result<Self, TimeFilterError> {
        validate_rule(rule)?;
        Ok(Self { rule: rule.clone() })
    }

    #[must_use]
    pub const fn prefix_bytes(&self) -> usize {
        match &self.rule {
            TimestampRule::Rfc3339 { prefix_bytes }
            | TimestampRule::Custom { prefix_bytes, .. } => *prefix_bytes,
        }
    }

    #[must_use]
    pub fn observe(&self, line_prefix: &[u8]) -> TimestampObservation {
        let timestamp = match &self.rule {
            TimestampRule::Rfc3339 { prefix_bytes } => {
                parse_rfc3339_prefix(line_prefix, *prefix_bytes)
            }
            TimestampRule::Custom {
                prefix_bytes,
                format,
                default_offset_seconds,
            } => parse_custom_prefix(line_prefix, *prefix_bytes, format, *default_offset_seconds),
        };

        TimestampObservation {
            timestamp,
            malformed: timestamp.is_none() && looks_like_timestamp_prefix(line_prefix),
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TimestampObservation {
    pub timestamp: Option<DateTime<FixedOffset>>,
    pub malformed: bool,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TimeRange {
    pub start: Option<DateTime<FixedOffset>>,
    pub end: Option<DateTime<FixedOffset>>,
}

impl TimeRange {
    pub fn from_rfc3339(start: Option<&str>, end: Option<&str>) -> Result<Self, TimeFilterError> {
        let start = start
            .map(DateTime::parse_from_rfc3339)
            .transpose()
            .map_err(|_| TimeFilterError::InvalidRange("start_time is not valid RFC 3339"))?;
        let end = end
            .map(DateTime::parse_from_rfc3339)
            .transpose()
            .map_err(|_| TimeFilterError::InvalidRange("end_time is not valid RFC 3339"))?;
        let range = Self { start, end };
        range.validate()?;
        Ok(range)
    }

    pub fn validate(&self) -> Result<(), TimeFilterError> {
        if self
            .start
            .as_ref()
            .zip(self.end.as_ref())
            .is_some_and(|(start, end)| start > end)
        {
            return Err(TimeFilterError::InvalidRange(
                "start_time must not be later than end_time",
            ));
        }
        Ok(())
    }

    #[must_use]
    pub fn contains(&self, timestamp: &DateTime<FixedOffset>) -> bool {
        self.start.as_ref().is_none_or(|start| timestamp >= start)
            && self.end.as_ref().is_none_or(|end| timestamp < end)
    }

    #[must_use]
    pub fn classify(&self, observation: &TimestampObservation) -> TimeFilterDecision {
        match observation.timestamp.as_ref() {
            Some(timestamp) if self.contains(timestamp) => TimeFilterDecision::InRange,
            Some(_) => TimeFilterDecision::OutOfRange,
            None if observation.malformed => TimeFilterDecision::MalformedTimestamp,
            None => TimeFilterDecision::UnknownTimestamp,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TimeFilterDecision {
    InRange,
    OutOfRange,
    UnknownTimestamp,
    MalformedTimestamp,
}

fn validate_rule(rule: &TimestampRule) -> Result<(), TimeFilterError> {
    let prefix_bytes = match rule {
        TimestampRule::Rfc3339 { prefix_bytes } | TimestampRule::Custom { prefix_bytes, .. } => {
            *prefix_bytes
        }
    };
    if prefix_bytes == 0 || prefix_bytes > MAX_TIMESTAMP_PREFIX_BYTES {
        return Err(TimeFilterError::InvalidConfiguration(
            "timestamp prefix length is outside the service limit",
        ));
    }

    if let TimestampRule::Custom {
        format,
        default_offset_seconds,
        ..
    } = rule
    {
        if format.is_empty() || format.chars().count() > MAX_TIMESTAMP_FORMAT_CHARS {
            return Err(TimeFilterError::InvalidConfiguration(
                "timestamp format length is outside the service limit",
            ));
        }
        if default_offset_seconds.is_some_and(|seconds| FixedOffset::east_opt(seconds).is_none()) {
            return Err(TimeFilterError::InvalidConfiguration(
                "default timestamp offset is outside the supported range",
            ));
        }
    }
    Ok(())
}

fn parse_rfc3339_prefix(
    line_prefix: &[u8],
    maximum_prefix_bytes: usize,
) -> Option<DateTime<FixedOffset>> {
    let search_len = line_prefix.len().min(maximum_prefix_bytes);
    let candidate = &line_prefix[..search_len];
    let end = candidate
        .iter()
        .position(u8::is_ascii_whitespace)
        .unwrap_or(candidate.len());
    let token = std::str::from_utf8(&candidate[..end]).ok()?;
    DateTime::parse_from_rfc3339(token).ok()
}

fn parse_custom_prefix(
    line_prefix: &[u8],
    prefix_bytes: usize,
    format: &str,
    default_offset_seconds: Option<i32>,
) -> Option<DateTime<FixedOffset>> {
    let prefix = std::str::from_utf8(line_prefix.get(..prefix_bytes)?).ok()?;
    parse_custom_timestamp(prefix, format, default_offset_seconds)
}

fn parse_custom_timestamp(
    value: &str,
    format: &str,
    default_offset_seconds: Option<i32>,
) -> Option<DateTime<FixedOffset>> {
    if let Some(offset_seconds) = default_offset_seconds {
        let offset = FixedOffset::east_opt(offset_seconds)?;
        if let Ok(naive) = NaiveDateTime::parse_from_str(value, format) {
            return offset.from_local_datetime(&naive).single();
        }
        if let Ok(date) = NaiveDate::parse_from_str(value, format) {
            return offset
                .from_local_datetime(&date.and_hms_opt(0, 0, 0)?)
                .single();
        }
        return None;
    }

    match DateTime::parse_from_str(value, format) {
        Ok(timestamp) => Some(timestamp),
        Err(error) if error.kind() == ParseErrorKind::NotEnough => None,
        Err(_) => None,
    }
}

fn looks_like_timestamp_prefix(line: &[u8]) -> bool {
    line.len() >= 5 && line[..4].iter().all(u8::is_ascii_digit) && matches!(line[4], b'-' | b'/')
}

#[derive(Debug, Error)]
pub enum TimeFilterError {
    #[error("invalid timestamp configuration: {0}")]
    InvalidConfiguration(&'static str),

    #[error("invalid time range: {0}")]
    InvalidRange(&'static str),
}

#[cfg(test)]
mod tests {
    use chrono::Timelike;

    use super::*;

    #[test]
    fn parses_rfc3339_token_with_variable_precision() {
        let parser = TimestampParser::new(&TimestampRule::Rfc3339 { prefix_bytes: 64 })
            .expect("parser should be valid");
        let milliseconds = parser.observe(b"2026-06-19T14:20:03.125+09:00 ERROR payment failed");
        let seconds = parser.observe(b"2026-06-19T05:20:03Z INFO payment succeeded");

        assert_eq!(
            milliseconds
                .timestamp
                .expect("millisecond timestamp should parse")
                .offset()
                .local_minus_utc(),
            9 * 3600
        );
        assert_eq!(
            seconds
                .timestamp
                .expect("second timestamp should parse")
                .hour(),
            5
        );
    }

    #[test]
    fn parses_custom_prefix_with_default_offset() {
        let parser = TimestampParser::new(&TimestampRule::Custom {
            prefix_bytes: 23,
            format: "%Y-%m-%d %H:%M:%S%.3f".to_owned(),
            default_offset_seconds: Some(9 * 3600),
        })
        .expect("parser should be valid");
        let observation = parser.observe(b"2026-06-19 14:20:03.125 ERROR payment failed");
        let timestamp = observation.timestamp.expect("timestamp should parse");

        assert_eq!(timestamp.hour(), 14);
        assert_eq!(timestamp.offset().local_minus_utc(), 9 * 3600);
        assert!(!observation.malformed);
    }

    #[test]
    fn marks_date_like_parse_failures_as_malformed() {
        let parser = TimestampParser::new(&TimestampRule::Rfc3339 { prefix_bytes: 64 })
            .expect("parser should be valid");

        let malformed = parser.observe(b"2026-99-99T99:99:99Z ERROR broken timestamp");
        let unknown = parser.observe(b"    at payment::authorize(payment.rs:42)");

        assert!(malformed.timestamp.is_none());
        assert!(malformed.malformed);
        assert!(unknown.timestamp.is_none());
        assert!(!unknown.malformed);
    }

    #[test]
    fn applies_start_inclusive_end_exclusive_range() {
        let range = TimeRange::from_rfc3339(
            Some("2026-06-19T14:00:00+09:00"),
            Some("2026-06-19T15:00:00+09:00"),
        )
        .expect("range should be valid");
        let start = TimestampObservation {
            timestamp: Some(
                DateTime::parse_from_rfc3339("2026-06-19T14:00:00+09:00")
                    .expect("timestamp should parse"),
            ),
            malformed: false,
        };
        let end = TimestampObservation {
            timestamp: Some(
                DateTime::parse_from_rfc3339("2026-06-19T15:00:00+09:00")
                    .expect("timestamp should parse"),
            ),
            malformed: false,
        };

        assert_eq!(range.classify(&start), TimeFilterDecision::InRange);
        assert_eq!(range.classify(&end), TimeFilterDecision::OutOfRange);
    }

    #[test]
    fn rejects_reverse_range() {
        assert!(matches!(
            TimeRange::from_rfc3339(
                Some("2026-06-19T15:00:01+09:00"),
                Some("2026-06-19T15:00:00+09:00")
            ),
            Err(TimeFilterError::InvalidRange(_))
        ));
    }
}
