use time::{
    OffsetDateTime, PrimitiveDateTime, UtcOffset,
    format_description::well_known::Rfc3339,
    macros::format_description,
};

const LOCAL_MILLIS_FORMAT: &[time::format_description::BorrowedFormatItem<'static>] =
    format_description!(
        "[year]-[month]-[day] [hour]:[minute]:[second].[subsecond digits:3]"
    );

pub const MAX_TIMESTAMP_PREFIX_BYTES: usize = 128;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TimestampFormat {
    Rfc3339,
    YearMonthDayMillis,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct TimestampRule {
    pub format: TimestampFormat,
    pub prefix_bytes: usize,
    pub local_offset_seconds: Option<i32>,
    pub inherit_previous: bool,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ResolvedTimestamp {
    pub timestamp: OffsetDateTime,
    pub inherited: bool,
}

#[derive(Debug, Clone)]
pub struct TimestampParser {
    rule: TimestampRule,
    local_offset: Option<UtcOffset>,
}

impl TimestampParser {
    pub fn new(rule: TimestampRule) -> Result<Self, TimestampConfigError> {
        if rule.prefix_bytes == 0 || rule.prefix_bytes > MAX_TIMESTAMP_PREFIX_BYTES {
            return Err(TimestampConfigError::InvalidPrefixLength);
        }

        let local_offset = match rule.format {
            TimestampFormat::Rfc3339 => {
                if rule.local_offset_seconds.is_some() {
                    return Err(TimestampConfigError::UnexpectedLocalOffset);
                }
                None
            }
            TimestampFormat::YearMonthDayMillis => {
                let seconds = rule
                    .local_offset_seconds
                    .ok_or(TimestampConfigError::MissingLocalOffset)?;
                Some(
                    UtcOffset::from_whole_seconds(seconds)
                        .map_err(|_| TimestampConfigError::InvalidLocalOffset)?,
                )
            }
        };

        Ok(Self { rule, local_offset })
    }

    #[must_use]
    pub const fn rule(&self) -> TimestampRule {
        self.rule
    }

    #[must_use]
    pub fn parse_line(&self, line: &[u8]) -> Option<OffsetDateTime> {
        if line.len() < self.rule.prefix_bytes {
            return None;
        }

        let prefix = line.get(..self.rule.prefix_bytes)?;
        let text = std::str::from_utf8(prefix).ok()?;

        match self.rule.format {
            TimestampFormat::Rfc3339 => OffsetDateTime::parse(text, &Rfc3339).ok(),
            TimestampFormat::YearMonthDayMillis => {
                let local = PrimitiveDateTime::parse(text, LOCAL_MILLIS_FORMAT).ok()?;
                Some(local.assume_offset(
                    self.local_offset
                        .expect("local timestamp parser always has a configured offset"),
                ))
            }
        }
    }
}

#[derive(Debug, Clone)]
pub struct TimestampTracker {
    parser: TimestampParser,
    last_timestamp: Option<OffsetDateTime>,
}

impl TimestampTracker {
    #[must_use]
    pub const fn new(parser: TimestampParser) -> Self {
        Self {
            parser,
            last_timestamp: None,
        }
    }

    pub fn resolve_line(&mut self, line: &[u8]) -> Option<ResolvedTimestamp> {
        if let Some(timestamp) = self.parser.parse_line(line) {
            self.last_timestamp = Some(timestamp);
            return Some(ResolvedTimestamp {
                timestamp,
                inherited: false,
            });
        }

        if self.parser.rule.inherit_previous {
            self.last_timestamp.map(|timestamp| ResolvedTimestamp {
                timestamp,
                inherited: true,
            })
        } else {
            None
        }
    }

    pub fn reset(&mut self) {
        self.last_timestamp = None;
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct TimeRange {
    pub start: Option<OffsetDateTime>,
    pub end: Option<OffsetDateTime>,
}

impl TimeRange {
    pub fn new(
        start: Option<OffsetDateTime>,
        end: Option<OffsetDateTime>,
    ) -> Result<Self, TimestampConfigError> {
        if start.zip(end).is_some_and(|(start, end)| start > end) {
            return Err(TimestampConfigError::InvalidRange);
        }

        Ok(Self { start, end })
    }

    #[must_use]
    pub fn contains(self, timestamp: OffsetDateTime) -> bool {
        self.start.is_none_or(|start| timestamp >= start)
            && self.end.is_none_or(|end| timestamp <= end)
    }
}

#[derive(Debug, thiserror::Error)]
pub enum TimestampConfigError {
    #[error("timestamp prefix length must be between 1 and 128 bytes")]
    InvalidPrefixLength,

    #[error("RFC 3339 timestamps already contain an offset")]
    UnexpectedLocalOffset,

    #[error("local timestamp format requires a configured UTC offset")]
    MissingLocalOffset,

    #[error("configured UTC offset is invalid")]
    InvalidLocalOffset,

    #[error("time range start must not be later than end")]
    InvalidRange,
}

#[cfg(test)]
mod tests {
    use time::Month;

    use super::*;

    #[test]
    fn parses_rfc3339_prefix() {
        let parser = TimestampParser::new(TimestampRule {
            format: TimestampFormat::Rfc3339,
            prefix_bytes: 29,
            local_offset_seconds: None,
            inherit_previous: false,
        })
        .expect("parser should be created");

        let timestamp = parser
            .parse_line(b"2026-06-19T14:20:03.125+09:00 ERROR payment failed")
            .expect("timestamp should parse");

        assert_eq!(timestamp.year(), 2026);
        assert_eq!(timestamp.month(), Month::June);
        assert_eq!(timestamp.day(), 19);
        assert_eq!(timestamp.offset().whole_hours(), 9);
    }

    #[test]
    fn parses_local_millis_with_source_offset() {
        let parser = TimestampParser::new(TimestampRule {
            format: TimestampFormat::YearMonthDayMillis,
            prefix_bytes: 23,
            local_offset_seconds: Some(9 * 60 * 60),
            inherit_previous: false,
        })
        .expect("parser should be created");

        let timestamp = parser
            .parse_line(b"2026-06-19 14:20:03.125 ERROR payment failed")
            .expect("timestamp should parse");

        assert_eq!(timestamp.hour(), 14);
        assert_eq!(timestamp.millisecond(), 125);
        assert_eq!(timestamp.offset().whole_hours(), 9);
    }

    #[test]
    fn inherits_timestamp_for_stack_trace_lines() {
        let parser = TimestampParser::new(TimestampRule {
            format: TimestampFormat::YearMonthDayMillis,
            prefix_bytes: 23,
            local_offset_seconds: Some(9 * 60 * 60),
            inherit_previous: true,
        })
        .expect("parser should be created");
        let mut tracker = TimestampTracker::new(parser);

        let event = tracker
            .resolve_line(b"2026-06-19 14:20:03.125 ERROR PaymentAuthException")
            .expect("event timestamp should resolve");
        let stack = tracker
            .resolve_line(b"    at payment::authorize(payment.rs:42)")
            .expect("stack line should inherit timestamp");

        assert!(!event.inherited);
        assert!(stack.inherited);
        assert_eq!(event.timestamp, stack.timestamp);
    }

    #[test]
    fn does_not_inherit_when_disabled() {
        let parser = TimestampParser::new(TimestampRule {
            format: TimestampFormat::Rfc3339,
            prefix_bytes: 29,
            local_offset_seconds: None,
            inherit_previous: false,
        })
        .expect("parser should be created");
        let mut tracker = TimestampTracker::new(parser);

        assert!(tracker.resolve_line(b"stack continuation").is_none());
    }

    #[test]
    fn range_is_inclusive() {
        let start = OffsetDateTime::parse("2026-06-19T14:00:00+09:00", &Rfc3339)
            .expect("start should parse");
        let end = OffsetDateTime::parse("2026-06-19T15:00:00+09:00", &Rfc3339)
            .expect("end should parse");
        let range = TimeRange::new(Some(start), Some(end)).expect("range should be valid");

        assert!(range.contains(start));
        assert!(range.contains(end));
        assert!(!range.contains(start - time::Duration::nanoseconds(1)));
        assert!(!range.contains(end + time::Duration::nanoseconds(1)));
    }

    #[test]
    fn rejects_invalid_configuration() {
        assert!(matches!(
            TimestampParser::new(TimestampRule {
                format: TimestampFormat::Rfc3339,
                prefix_bytes: 0,
                local_offset_seconds: None,
                inherit_previous: false,
            }),
            Err(TimestampConfigError::InvalidPrefixLength)
        ));
        assert!(matches!(
            TimestampParser::new(TimestampRule {
                format: TimestampFormat::YearMonthDayMillis,
                prefix_bytes: 23,
                local_offset_seconds: None,
                inherit_previous: false,
            }),
            Err(TimestampConfigError::MissingLocalOffset)
        ));
        assert!(matches!(
            TimestampParser::new(TimestampRule {
                format: TimestampFormat::Rfc3339,
                prefix_bytes: 29,
                local_offset_seconds: Some(0),
                inherit_previous: false,
            }),
            Err(TimestampConfigError::UnexpectedLocalOffset)
        ));
    }

    #[test]
    fn rejects_reverse_range() {
        let start = OffsetDateTime::parse("2026-06-19T15:00:00+09:00", &Rfc3339)
            .expect("start should parse");
        let end = OffsetDateTime::parse("2026-06-19T14:00:00+09:00", &Rfc3339)
            .expect("end should parse");

        assert!(matches!(
            TimeRange::new(Some(start), Some(end)),
            Err(TimestampConfigError::InvalidRange)
        ));
    }
}
