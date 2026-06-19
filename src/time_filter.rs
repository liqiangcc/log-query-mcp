use std::{cmp::Ordering, path::PathBuf, time::SystemTime};

use chrono::{
    DateTime, FixedOffset, NaiveDate, NaiveDateTime, TimeZone, Utc, format::ParseErrorKind,
};
use thiserror::Error;

use crate::ResultOrder;

pub const MAX_TIMESTAMP_PREFIX_BYTES: usize = 256;
pub const MAX_TIMESTAMP_FORMAT_CHARS: usize = 128;
pub const MAX_ROTATION_COMPONENT_CHARS: usize = 128;

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum TimestampRule {
    /// Parse the first whitespace-delimited RFC 3339 token within this maximum
    /// prefix size. This supports timestamps with or without fractional seconds.
    Rfc3339 { prefix_bytes: usize },
    /// Parse an exact fixed-width prefix with a configured chrono format.
    Custom {
        prefix_bytes: usize,
        format: String,
        default_offset_seconds: Option<i32>,
    },
}

impl TimestampRule {
    pub fn validate(&self) -> Result<(), TimeFilterError> {
        let prefix_bytes = match self {
            Self::Rfc3339 { prefix_bytes } | Self::Custom { prefix_bytes, .. } => *prefix_bytes,
        };
        if prefix_bytes == 0 || prefix_bytes > MAX_TIMESTAMP_PREFIX_BYTES {
            return Err(TimeFilterError::InvalidConfiguration(
                "timestamp prefix length is outside the server limit",
            ));
        }

        if let Self::Custom {
            format,
            default_offset_seconds,
            ..
        } = self
        {
            if format.is_empty() || format.chars().count() > MAX_TIMESTAMP_FORMAT_CHARS {
                return Err(TimeFilterError::InvalidConfiguration(
                    "timestamp format length is outside the server limit",
                ));
            }
            validate_offset(*default_offset_seconds)?;
        }

        Ok(())
    }

    pub fn parse_line(&self, line: &str) -> Option<DateTime<FixedOffset>> {
        self.validate().ok()?;

        match self {
            Self::Rfc3339 { prefix_bytes } => {
                let bytes = line.as_bytes();
                let search_len = bytes.len().min(*prefix_bytes);
                let end = bytes[..search_len]
                    .iter()
                    .position(u8::is_ascii_whitespace)
                    .unwrap_or(search_len);
                let prefix = line.get(..end)?;
                DateTime::parse_from_rfc3339(prefix).ok()
            }
            Self::Custom {
                prefix_bytes,
                format,
                default_offset_seconds,
            } => {
                let prefix = line.get(..*prefix_bytes)?;
                parse_custom_timestamp(prefix, format, *default_offset_seconds)
            }
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RotationTimestampRule {
    pub prefix: String,
    pub suffix: String,
    pub format: String,
    pub default_offset_seconds: i32,
}

impl RotationTimestampRule {
    pub fn validate(&self) -> Result<(), TimeFilterError> {
        if self.prefix.chars().count() > MAX_ROTATION_COMPONENT_CHARS
            || self.suffix.chars().count() > MAX_ROTATION_COMPONENT_CHARS
        {
            return Err(TimeFilterError::InvalidConfiguration(
                "rotation filename prefix or suffix is too long",
            ));
        }
        if self.format.is_empty() || self.format.chars().count() > MAX_TIMESTAMP_FORMAT_CHARS {
            return Err(TimeFilterError::InvalidConfiguration(
                "rotation timestamp format length is outside the server limit",
            ));
        }
        validate_offset(Some(self.default_offset_seconds))
    }

    pub fn parse_path(&self, relative_path: &std::path::Path) -> Option<DateTime<FixedOffset>> {
        self.validate().ok()?;
        let filename = relative_path.file_name()?.to_str()?;
        let fragment = filename
            .strip_prefix(&self.prefix)?
            .strip_suffix(&self.suffix)?;
        parse_custom_timestamp(fragment, &self.format, Some(self.default_offset_seconds))
    }
}

/// A start-inclusive, end-exclusive query time range.
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
            .is_some_and(|(start, end)| start >= end)
        {
            return Err(TimeFilterError::InvalidRange(
                "start_time must be earlier than end_time",
            ));
        }
        Ok(())
    }

    #[must_use]
    pub fn contains(&self, timestamp: &DateTime<FixedOffset>) -> bool {
        let after_start = self.start.as_ref().is_none_or(|start| timestamp >= start);
        let before_end = self.end.as_ref().is_none_or(|end| timestamp < end);
        after_start && before_end
    }

    #[must_use]
    pub fn classify(&self, timestamp: Option<&DateTime<FixedOffset>>) -> TimeFilterDecision {
        match timestamp {
            Some(timestamp) if self.contains(timestamp) => TimeFilterDecision::InRange,
            Some(_) => TimeFilterDecision::OutOfRange,
            None => TimeFilterDecision::UnknownTimestamp,
        }
    }

    #[must_use]
    pub fn classify_line(&self, observed: &LineTimestamp) -> TimeFilterDecision {
        if observed.malformed {
            TimeFilterDecision::MalformedTimestamp
        } else {
            self.classify(observed.timestamp.as_ref())
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

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LineTimestamp {
    pub timestamp: Option<DateTime<FixedOffset>>,
    pub inherited: bool,
    pub malformed: bool,
}

#[derive(Debug, Clone)]
pub struct TimestampTracker {
    rule: TimestampRule,
    last_timestamp: Option<DateTime<FixedOffset>>,
}

impl TimestampTracker {
    pub fn new(rule: TimestampRule) -> Result<Self, TimeFilterError> {
        rule.validate()?;
        Ok(Self {
            rule,
            last_timestamp: None,
        })
    }

    pub fn observe(&mut self, line: &str) -> LineTimestamp {
        if let Some(timestamp) = self.rule.parse_line(line) {
            self.last_timestamp = Some(timestamp);
            return LineTimestamp {
                timestamp: self.last_timestamp,
                inherited: false,
                malformed: false,
            };
        }

        if looks_like_timestamp_prefix(line.as_bytes()) {
            self.last_timestamp = None;
            return LineTimestamp {
                timestamp: None,
                inherited: false,
                malformed: true,
            };
        }

        LineTimestamp {
            timestamp: self.last_timestamp,
            inherited: self.last_timestamp.is_some(),
            malformed: false,
        }
    }

    pub fn reset(&mut self) {
        self.last_timestamp = None;
    }
}

fn looks_like_timestamp_prefix(line: &[u8]) -> bool {
    line.len() >= 5
        && line[..4].iter().all(u8::is_ascii_digit)
        && matches!(line[4], b'-' | b'/')
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TimedLogResult<T> {
    pub timestamp: Option<DateTime<FixedOffset>>,
    pub source_index: usize,
    pub file_index: usize,
    pub line_number: u64,
    pub value: T,
}

pub fn sort_timed_results<T>(results: &mut [TimedLogResult<T>], order: ResultOrder) {
    results.sort_by(|left, right| {
        let timestamp_order = match (&left.timestamp, &right.timestamp) {
            (Some(left), Some(right)) => match order {
                ResultOrder::OldestFirst => left.cmp(right),
                ResultOrder::NewestFirst => right.cmp(left),
            },
            (Some(_), None) => Ordering::Less,
            (None, Some(_)) => Ordering::Greater,
            (None, None) => Ordering::Equal,
        };

        timestamp_order
            .then_with(|| left.source_index.cmp(&right.source_index))
            .then_with(|| left.file_index.cmp(&right.file_index))
            .then_with(|| left.line_number.cmp(&right.line_number))
    });
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct OrderedFileCandidate {
    pub source_index: usize,
    pub relative_path: PathBuf,
    pub timestamp_hint: Option<DateTime<FixedOffset>>,
    pub modified_at: SystemTime,
}

pub fn sort_file_candidates(candidates: &mut [OrderedFileCandidate], order: ResultOrder) {
    candidates.sort_by(|left, right| {
        let left_time = candidate_time(left);
        let right_time = candidate_time(right);
        let time_order = match order {
            ResultOrder::OldestFirst => left_time.cmp(&right_time),
            ResultOrder::NewestFirst => right_time.cmp(&left_time),
        };

        time_order
            .then_with(|| left.source_index.cmp(&right.source_index))
            .then_with(|| left.relative_path.cmp(&right.relative_path))
    });
}

fn candidate_time(candidate: &OrderedFileCandidate) -> DateTime<Utc> {
    candidate
        .timestamp_hint
        .as_ref()
        .map(|timestamp| timestamp.with_timezone(&Utc))
        .unwrap_or_else(|| DateTime::<Utc>::from(candidate.modified_at))
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
            let naive = date.and_hms_opt(0, 0, 0)?;
            return offset.from_local_datetime(&naive).single();
        }
        return None;
    }

    match DateTime::parse_from_str(value, format) {
        Ok(timestamp) => Some(timestamp),
        Err(error) if error.kind() == ParseErrorKind::NotEnough => None,
        Err(_) => None,
    }
}

fn validate_offset(offset_seconds: Option<i32>) -> Result<(), TimeFilterError> {
    if offset_seconds.is_some_and(|seconds| FixedOffset::east_opt(seconds).is_none()) {
        return Err(TimeFilterError::InvalidConfiguration(
            "default timestamp offset is outside the supported range",
        ));
    }
    Ok(())
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
    use std::time::{Duration, UNIX_EPOCH};

    use chrono::Timelike;

    use super::*;

    #[test]
    fn parses_rfc3339_prefix_with_variable_precision() {
        let rule = TimestampRule::Rfc3339 { prefix_bytes: 64 };
        let milliseconds = rule
            .parse_line("2026-06-19T14:20:03.125+09:00 ERROR payment failed")
            .expect("timestamp with milliseconds should parse");
        let seconds = rule
            .parse_line("2026-06-19T05:20:03Z INFO payment succeeded")
            .expect("timestamp without fractional seconds should parse");

        assert_eq!(milliseconds.offset().local_minus_utc(), 9 * 3600);
        assert_eq!(milliseconds.hour(), 14);
        assert_eq!(milliseconds.timestamp(), seconds.timestamp());
    }

    #[test]
    fn parses_custom_java_timestamp_with_default_timezone() {
        let rule = TimestampRule::Custom {
            prefix_bytes: 23,
            format: "%Y-%m-%d %H:%M:%S%.3f".to_owned(),
            default_offset_seconds: Some(9 * 3600),
        };
        let timestamp = rule
            .parse_line("2026-06-19 14:20:03.125 ERROR payment failed")
            .expect("timestamp should parse");

        assert_eq!(timestamp.offset().local_minus_utc(), 9 * 3600);
        assert_eq!(timestamp.hour(), 14);
    }

    #[test]
    fn stack_trace_lines_inherit_previous_event_timestamp() {
        let rule = TimestampRule::Rfc3339 { prefix_bytes: 64 };
        let mut tracker = TimestampTracker::new(rule).expect("tracker should be created");
        let event = tracker.observe("2026-06-19T14:20:03.125+09:00 ERROR failure");
        let stack = tracker.observe("    at payment::authorize(payment.rs:42)");

        assert!(!event.inherited);
        assert!(!event.malformed);
        assert!(stack.inherited);
        assert!(!stack.malformed);
        assert_eq!(event.timestamp, stack.timestamp);
    }

    #[test]
    fn malformed_event_timestamp_clears_inheritance() {
        let rule = TimestampRule::Rfc3339 { prefix_bytes: 64 };
        let mut tracker = TimestampTracker::new(rule).expect("tracker should be created");
        tracker.observe("2026-06-19T14:20:03+09:00 ERROR first event");
        let malformed = tracker.observe("2026-99-99T99:99:99+09:00 ERROR malformed");
        let following = tracker.observe("    at payment::authorize(payment.rs:42)");

        assert!(malformed.malformed);
        assert_eq!(malformed.timestamp, None);
        assert!(!following.inherited);
        assert_eq!(following.timestamp, None);
    }

    #[test]
    fn line_before_first_timestamp_remains_unknown() {
        let rule = TimestampRule::Rfc3339 { prefix_bytes: 64 };
        let mut tracker = TimestampTracker::new(rule).expect("tracker should be created");
        let timestamp = tracker.observe("startup banner without timestamp");

        assert_eq!(timestamp.timestamp, None);
        assert!(!timestamp.inherited);
        assert!(!timestamp.malformed);
    }

    #[test]
    fn time_range_is_start_inclusive_and_end_exclusive() {
        let range = TimeRange::from_rfc3339(
            Some("2026-06-19T14:00:00+09:00"),
            Some("2026-06-19T15:00:00+09:00"),
        )
        .expect("range should parse");
        let start = DateTime::parse_from_rfc3339("2026-06-19T14:00:00+09:00")
            .expect("timestamp should parse");
        let before_end = DateTime::parse_from_rfc3339("2026-06-19T14:59:59+09:00")
            .expect("timestamp should parse");
        let end = DateTime::parse_from_rfc3339("2026-06-19T15:00:00+09:00")
            .expect("timestamp should parse");

        assert_eq!(range.classify(Some(&start)), TimeFilterDecision::InRange);
        assert_eq!(
            range.classify(Some(&before_end)),
            TimeFilterDecision::InRange
        );
        assert_eq!(range.classify(Some(&end)), TimeFilterDecision::OutOfRange);
        assert_eq!(range.classify(None), TimeFilterDecision::UnknownTimestamp);
    }

    #[test]
    fn classifies_malformed_line_separately() {
        let range = TimeRange::from_rfc3339(None, None).expect("range should be valid");
        let observed = LineTimestamp {
            timestamp: None,
            inherited: false,
            malformed: true,
        };

        assert_eq!(
            range.classify_line(&observed),
            TimeFilterDecision::MalformedTimestamp
        );
    }

    #[test]
    fn rejects_reversed_equal_and_invalid_ranges() {
        assert!(matches!(
            TimeRange::from_rfc3339(
                Some("2026-06-19T15:00:00+09:00"),
                Some("2026-06-19T14:00:00+09:00")
            ),
            Err(TimeFilterError::InvalidRange(_))
        ));
        assert!(matches!(
            TimeRange::from_rfc3339(
                Some("2026-06-19T15:00:00+09:00"),
                Some("2026-06-19T15:00:00+09:00")
            ),
            Err(TimeFilterError::InvalidRange(_))
        ));
        assert!(matches!(
            TimeRange::from_rfc3339(Some("not-a-time"), None),
            Err(TimeFilterError::InvalidRange(_))
        ));
    }

    #[test]
    fn parses_rotation_filename_timestamp() {
        let rule = RotationTimestampRule {
            prefix: "application-".to_owned(),
            suffix: ".log".to_owned(),
            format: "%Y-%m-%d".to_owned(),
            default_offset_seconds: 9 * 3600,
        };
        let timestamp = rule
            .parse_path(std::path::Path::new("archive/application-2026-06-19.log"))
            .expect("rotation timestamp should parse");

        assert_eq!(timestamp.date_naive().to_string(), "2026-06-19");
        assert_eq!(timestamp.offset().local_minus_utc(), 9 * 3600);
    }

    #[test]
    fn sorts_cross_service_results_with_unknowns_last() {
        let early = DateTime::parse_from_rfc3339("2026-06-19T14:00:00+09:00")
            .expect("timestamp should parse");
        let late = DateTime::parse_from_rfc3339("2026-06-19T14:00:02+09:00")
            .expect("timestamp should parse");
        let mut results = vec![
            TimedLogResult {
                timestamp: None,
                source_index: 0,
                file_index: 0,
                line_number: 1,
                value: "unknown",
            },
            TimedLogResult {
                timestamp: Some(late),
                source_index: 1,
                file_index: 0,
                line_number: 1,
                value: "late",
            },
            TimedLogResult {
                timestamp: Some(early),
                source_index: 0,
                file_index: 0,
                line_number: 2,
                value: "early",
            },
        ];

        sort_timed_results(&mut results, ResultOrder::OldestFirst);
        assert_eq!(
            results
                .iter()
                .map(|result| result.value)
                .collect::<Vec<_>>(),
            vec!["early", "late", "unknown"]
        );

        sort_timed_results(&mut results, ResultOrder::NewestFirst);
        assert_eq!(
            results
                .iter()
                .map(|result| result.value)
                .collect::<Vec<_>>(),
            vec!["late", "early", "unknown"]
        );
    }

    #[test]
    fn orders_rotation_and_current_files_on_one_timeline() {
        let rotation_rule = RotationTimestampRule {
            prefix: "application-".to_owned(),
            suffix: ".log".to_owned(),
            format: "%Y-%m-%d".to_owned(),
            default_offset_seconds: 9 * 3600,
        };
        let old_path = PathBuf::from("application-2026-06-18.log");
        let new_path = PathBuf::from("application-2026-06-19.log");
        let current_path = PathBuf::from("application.log");
        let current_time = DateTime::parse_from_rfc3339("2026-06-20T10:00:00+09:00")
            .expect("timestamp should parse")
            .with_timezone(&Utc);
        let current_mtime = UNIX_EPOCH
            + Duration::from_secs(
                u64::try_from(current_time.timestamp()).expect("timestamp should be positive"),
            );
        let mut candidates = vec![
            OrderedFileCandidate {
                source_index: 0,
                relative_path: current_path,
                timestamp_hint: None,
                modified_at: current_mtime,
            },
            OrderedFileCandidate {
                source_index: 0,
                timestamp_hint: rotation_rule.parse_path(&new_path),
                relative_path: new_path,
                modified_at: UNIX_EPOCH,
            },
            OrderedFileCandidate {
                source_index: 0,
                timestamp_hint: rotation_rule.parse_path(&old_path),
                relative_path: old_path,
                modified_at: UNIX_EPOCH,
            },
        ];

        sort_file_candidates(&mut candidates, ResultOrder::OldestFirst);
        assert_eq!(
            candidates
                .iter()
                .map(|candidate| candidate.relative_path.to_string_lossy().into_owned())
                .collect::<Vec<_>>(),
            vec![
                "application-2026-06-18.log",
                "application-2026-06-19.log",
                "application.log"
            ]
        );

        sort_file_candidates(&mut candidates, ResultOrder::NewestFirst);
        assert_eq!(
            candidates
                .iter()
                .map(|candidate| candidate.relative_path.to_string_lossy().into_owned())
                .collect::<Vec<_>>(),
            vec![
                "application.log",
                "application-2026-06-19.log",
                "application-2026-06-18.log"
            ]
        );
    }
}
