from pathlib import Path

path = Path("src/backend/mod.rs")
text = path.read_text()
old = '''    pub fn size(&self) -> u64 {\n        match self {\n            Self::Local(file) => file.size(),\n            Self::Remote { size, .. } => *size,\n        }\n    }\n}\n\nimpl Read for SnapshotFile {'''
new = '''    pub fn size(&self) -> u64 {\n        match self {\n            Self::Local(file) => file.size(),\n            Self::Remote { size, .. } => *size,\n        }\n    }\n\n    #[must_use]\n    pub fn into_file(self) -> Self {\n        self\n    }\n}\n\nimpl Read for SnapshotFile {'''
if text.count(old) != 1:
    raise SystemExit(f"SnapshotFile compatibility: expected one match, got {text.count(old)}")
path.write_text(text.replace(old, new))

path = Path("src/context_reader.rs")
text = path.read_text()
old = '''    let mut file = source.open_referenced_file(\n        &reference.relative_path,\n        reference.file_identity,\n        reference.file_size_at_match,\n        &reference.file_id,\n    )?;'''
new = '''    let mut file = source\n        .open_referenced_file(\n            &reference.relative_path,\n            reference.file_identity,\n            reference.file_size_at_match,\n            &reference.file_id,\n        )\n        .map_err(|error| match error {\n            SourceRegistryError::FileChanged { .. } => ContextReadError::FileChanged,\n            other => ContextReadError::SourceRegistry(other),\n        })?;'''
if text.count(old) != 1:
    raise SystemExit(f"context FileChanged mapping: expected one match, got {text.count(old)}")
path.write_text(text.replace(old, new))

path = Path("tests/v2_m1_config_backend.rs")
text = path.read_text()
text = text.replace(
    "use log_query_mcp::{ConfigDocument, SourceRegistry, SourceRegistryError};",
    "use log_query_mcp::{ConfigDocument, SourceRegistry};",
)
old = '''#[test]\nfn v2_ssh_document_is_valid_but_m1_rejects_unimplemented_backend() {\n    let document =\n        ConfigDocument::from_json_str(include_str!("contracts/v2/valid/ssh-password-tail.json"))\n            .expect("valid v2 SSH configuration should parse");\n\n    assert!(matches!(\n        SourceRegistry::from_document(document),\n        Err(SourceRegistryError::BackendUnavailable { backend: "ssh", .. })\n    ));\n}\n'''
new = '''#[test]\nfn v2_ssh_document_builds_remote_registry_without_connecting_at_startup() {\n    let cache = tempdir().expect("cache root should be created");\n    let mut config: serde_json::Value = serde_json::from_str(include_str!(\n        "contracts/v2/valid/ssh-password-tail.json"\n    ))\n    .expect("valid v2 SSH fixture should be JSON");\n    config["cache"]["root"] = serde_json::json!(cache.path());\n    let document = ConfigDocument::from_json_str(&config.to_string())\n        .expect("valid v2 SSH configuration should parse");\n\n    let registry = SourceRegistry::from_document(document)\n        .expect("M5 remote registry should build without opening SSH at startup");\n    let source = registry\n        .get("remote-order-test")\n        .expect("remote source should be registered");\n    assert_eq!(source.descriptor().service, "order-service");\n}\n'''
if text.count(old) != 1:
    raise SystemExit(f"obsolete SSH rejection test: expected one match, got {text.count(old)}")
path.write_text(text.replace(old, new))
