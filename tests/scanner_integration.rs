#![cfg(target_os = "linux")]

use std::{fs, path::PathBuf};

use log_query_mcp::{
    AppConfig, CONFIG_VERSION, Encoding, LimitsConfig, LogSourceConfig, ScanExecutor, ScanPosition,
    ScanRequest, ScanStopReason, SourceRegistry,
};
use tempfile::tempdir;

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn scans_a_source_registry_snapshot_through_safe_file_access() {
    let root = tempdir().expect("temporary source root should be created");
    fs::write(
        root.path().join("application.log"),
        "startup\ntraceId=abc123 PaymentAuthException\ncomplete\n",
    )
    .expect("log fixture should be written");

    let config = AppConfig {
        version: CONFIG_VERSION,
        sources: vec![LogSourceConfig {
            source_id: "payment-test".to_owned(),
            name: "Payment test".to_owned(),
            description: String::new(),
            service: "payment-service".to_owned(),
            environment: "test".to_owned(),
            tags: vec!["payment".to_owned()],
            enabled: true,
            encoding: Encoding::Utf8,
            root: root.path().to_path_buf(),
            files: vec![PathBuf::from("application.log")],
            directories: Vec::new(),
            timestamp_rule: None,
        }],
        limits: LimitsConfig::default(),
    };

    let registry = SourceRegistry::from_config(config).expect("registry should build");
    let source = registry.get("payment-test").expect("source should exist");
    let snapshot = source
        .snapshot_files(10)
        .expect("snapshot should succeed")
        .remove(0);
    let file = source
        .open_snapshot_file(&snapshot)
        .expect("snapshot file should open");

    let executor = ScanExecutor::new(registry.limits().max_concurrent_scans)
        .expect("executor should be created");
    let outcome = executor
        .scan(file.into_file(), ScanRequest::new("traceId=abc123"))
        .await
        .expect("scan should succeed");

    assert_eq!(outcome.stop_reason, ScanStopReason::Complete);
    assert_eq!(outcome.results.len(), 1);
    assert_eq!(outcome.results[0].line_number, 2);
    assert_eq!(outcome.results[0].line_start_offset, 8);
    assert_eq!(outcome.results[0].match_byte_offset, 8);
    assert_eq!(outcome.start_position, ScanPosition::default());
    assert_eq!(outcome.next_position, None);
    assert!(outcome.results[0].content.contains("PaymentAuthException"));
}
