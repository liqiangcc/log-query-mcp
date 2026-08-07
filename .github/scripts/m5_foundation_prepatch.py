from pathlib import Path

path = Path("src/backend/local.rs")
text = path.read_text()
old = '''    pub(crate) fn open_configured_file(\n        &self,\n        source_id: &str,\n        relative_path: &Path,\n    ) -> Result<SafeFile, SourceRegistryError> {\n        if !self.path_is_configured(relative_path) {'''
new = '''    pub(crate) fn open_configured_file(\n        &self,\n        source_id: &str,\n        relative_path: &Path,\n    ) -> Result<SafeFile, SourceRegistryError> {\n        // Local current-file access remains local-only; M5 snapshot reads use SnapshotFile.\n        if !self.path_is_configured(relative_path) {'''
if text.count(old) != 1:
    raise SystemExit(f"open_configured_file: expected one match, got {text.count(old)}")
path.write_text(text.replace(old, new))

path = Path("src/stateful_query.rs")
text = path.read_text()
old = '''    let mut prefix_file = if timestamp_parser.is_some() {\n        Some(source.open_snapshot_file(&candidate.snapshot)?.into_file())\n    } else {'''
new = '''    let mut prefix_file = if timestamp_parser.is_some() {\n        Some(\n            source\n                .open_snapshot_file(&candidate.snapshot)?\n                .into_file(),\n        )\n    } else {'''
if text.count(old) != 1:
    raise SystemExit(f"stateful prefix reader: expected one match, got {text.count(old)}")
path.write_text(text.replace(old, new))
