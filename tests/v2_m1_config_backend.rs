#![cfg(target_os = "linux")]

use std::fs;

use log_query_mcp::{ConfigDocument, SourceRegistry};
use tempfile::tempdir;

#[test]
fn v2_local_document_builds_registry_through_local_backend() {
    let root = tempdir().expect("source root should be created");
    fs::write(root.path().join("application.log"), "hello\n").expect("fixture should be written");

    let config = serde_json::json!({
        "version": 2,
        "sources": [{
            "source_id": "local-test",
            "name": "Local test",
            "service": "local",
            "environment": "test",
            "backend": {"type": "local"},
            "root": root.path(),
            "files": ["application.log"]
        }]
    });
    let document = ConfigDocument::from_json_str(&config.to_string())
        .expect("v2 local configuration should parse");
    let registry = SourceRegistry::from_document(document).expect("v2 local registry should build");

    let source = registry.get("local-test").expect("source should exist");
    let snapshots = source.snapshot_files(10).expect("snapshot should build");
    assert_eq!(snapshots.len(), 1);
    assert_eq!(snapshots[0].display_name(), "application.log");
}

#[test]
fn v2_ssh_document_builds_remote_registry_without_connecting_at_startup() {
    let cache = tempdir().expect("cache root should be created");
    let mut config: serde_json::Value =
        serde_json::from_str(include_str!("contracts/v2/valid/ssh-password-tail.json"))
            .expect("valid v2 SSH fixture should be JSON");
    config["cache"]["root"] = serde_json::json!(cache.path());
    let document = ConfigDocument::from_json_str(&config.to_string())
        .expect("valid v2 SSH configuration should parse");

    let registry = SourceRegistry::from_document(document)
        .expect("M5 remote registry should build without opening SSH at startup");
    let source = registry
        .get("remote-order-test")
        .expect("remote source should be registered");
    assert_eq!(source.descriptor().service, "order-service");
}
