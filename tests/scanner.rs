use std::{
    io::Cursor,
    time::{Duration, Instant},
};

use log_query_mcp::{
    LimitsConfig, ScanError, ScanLimits, ScanOutcome, ScanRequest, ScanStopReason, scan_reader,
};
use tokio_util::sync::CancellationToken;

fn scan(data: &[u8], request: ScanRequest) -> Result<ScanOutcome, ScanError> {
    scan_reader(&mut Cursor::new(data), &request)
}

#[test]
fn derives_limits_from_service_configuration() {
    let service = LimitsConfig {
        max_scan_bytes_per_page: 1024,
        max_results_per_page: 25,
        max_line_bytes: 128,
        max_returned_content_bytes: 512,
        ..LimitsConfig::default()
    };
    let limits = ScanLimits::from_service_limits(&service, 10).expect("limits should derive");

    assert_eq!(limits.max_scan_bytes, 1024);
    assert_eq!(limits.max_results, 10);
    assert_eq!(limits.max_line_bytes, 128);
    assert!(ScanLimits::from_service_limits(&service, 26).is_err());
}

#[test]
fn matches_across_buffers_and_records_position() {
    let limits = ScanLimits {
        read_buffer_bytes: 2,
        ..ScanLimits::default()
    };
    let outcome = scan(
        b"first\nsecond MATCH value\n",
        ScanRequest::new("MATCH").with_limits(limits),
    )
    .expect("scan should succeed");
    let position = outcome.results[0].position;

    assert_eq!(position.line_number, 2);
    assert_eq!(position.line_start_offset, 6);
    assert_eq!(position.match_byte_offset, 13);
    assert_eq!(position.original_line_bytes, 18);
    assert_eq!(outcome.stop_reason, ScanStopReason::Complete);
}

#[test]
fn supports_utf8_and_ascii_case_folding() {
    let utf8 = scan(
        "请求失败 orderId=10001\n".as_bytes(),
        ScanRequest::new("失败"),
    )
    .expect("UTF-8 scan should succeed");
    assert_eq!(utf8.results.len(), 1);

    let folded = scan(
        b"PaymentAuthException\n",
        ScanRequest::new("paymentauthexception"),
    )
    .expect("ASCII folded scan should succeed");
    assert_eq!(folded.results.len(), 1);

    let exact = scan(
        b"PaymentAuthException\n",
        ScanRequest::new("paymentauthexception").with_case_sensitive(true),
    )
    .expect("case-sensitive scan should succeed");
    assert!(exact.results.is_empty());
}

#[test]
fn never_matches_across_lines() {
    let outcome = scan(b"ab\nc\n", ScanRequest::new("abc")).expect("scan should succeed");

    assert!(outcome.results.is_empty());
    assert_eq!(outcome.lines_scanned, 2);
}

#[test]
fn bounds_long_line_preview_around_match() {
    let limits = ScanLimits {
        max_line_bytes: 16,
        ..ScanLimits::default()
    };
    let outcome = scan(
        b"aaaaaaaaaaaaaaaaaaaaMATCHbbbbbbbbbbbbbbbb\n",
        ScanRequest::new("MATCH").with_limits(limits),
    )
    .expect("scan should succeed");
    let result = &outcome.results[0];

    assert!(result.content.contains("MATCH"));
    assert!(result.content.len() <= 16);
    assert!(result.content_truncated);
    assert_eq!(result.position.original_line_bytes, 41);
}

#[test]
fn handles_invalid_utf8_and_crlf() {
    let invalid = [0xff, b' ', b'M', b'A', b'T', b'C', b'H', b'\n'];
    let outcome = scan(&invalid, ScanRequest::new("MATCH")).expect("scan should succeed");
    assert!(outcome.results[0].content_lossy);
    assert!(outcome.results[0].content.contains("MATCH"));

    let crlf =
        scan(b"traceId=abc123\r\n", ScanRequest::new("abc123")).expect("CRLF scan should succeed");
    assert_eq!(crlf.results[0].content, "traceId=abc123");
}

#[test]
fn enforces_result_scan_and_content_limits() {
    let results = scan(
        b"MATCH one\nMATCH two\nMATCH three\n",
        ScanRequest::new("MATCH").with_limits(ScanLimits {
            max_results: 2,
            ..ScanLimits::default()
        }),
    )
    .expect("result-limited scan should succeed");
    assert_eq!(results.results.len(), 2);
    assert_eq!(results.stop_reason, ScanStopReason::ResultLimit);
    assert!(results.stopped_by_limit());

    let bytes = scan(
        b"abcdef\n",
        ScanRequest::new("def").with_limits(ScanLimits {
            max_scan_bytes: 4,
            ..ScanLimits::default()
        }),
    )
    .expect("byte-limited scan should succeed");
    assert!(bytes.results.is_empty());
    assert_eq!(bytes.bytes_scanned, 4);
    assert_eq!(bytes.stop_reason, ScanStopReason::ScanByteLimit);

    let content = scan(
        b"MATCH\n",
        ScanRequest::new("MATCH").with_limits(ScanLimits {
            max_line_bytes: 8,
            max_returned_content_bytes: 3,
            ..ScanLimits::default()
        }),
    )
    .expect("content-limited scan should succeed");
    assert!(content.results.is_empty());
    assert_eq!(
        content.stop_reason,
        ScanStopReason::ReturnedContentByteLimit
    );
}

#[test]
fn honors_cancellation_and_deadline_before_reading() {
    let cancellation = CancellationToken::new();
    cancellation.cancel();
    let cancelled = scan(
        b"MATCH\n",
        ScanRequest::new("MATCH").with_cancellation(cancellation),
    )
    .expect("cancelled scan should return an outcome");
    assert_eq!(cancelled.bytes_scanned, 0);
    assert_eq!(cancelled.stop_reason, ScanStopReason::Cancelled);

    let expired = scan(
        b"MATCH\n",
        ScanRequest::new("MATCH").with_deadline(Instant::now() - Duration::from_millis(1)),
    )
    .expect("expired scan should return an outcome");
    assert_eq!(expired.bytes_scanned, 0);
    assert_eq!(expired.stop_reason, ScanStopReason::DeadlineExceeded);
}

#[test]
fn rejects_invalid_keyword_and_limits() {
    assert!(matches!(
        scan(b"data", ScanRequest::new("")),
        Err(ScanError::InvalidKeyword)
    ));
    assert!(matches!(
        scan(b"data", ScanRequest::new("a\nb")),
        Err(ScanError::InvalidKeyword)
    ));
    assert!(matches!(
        scan(
            b"MATCH",
            ScanRequest::new("MATCH").with_limits(ScanLimits {
                max_line_bytes: 2,
                ..ScanLimits::default()
            })
        ),
        Err(ScanError::InvalidLimits(_))
    ));
}
