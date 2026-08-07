from pathlib import Path

path = Path("src/backend/local.rs")
text = path.read_text()
old = '''    pub(crate) fn open_configured_file(\n        &self,\n        source_id: &str,\n        relative_path: &Path,\n    ) -> Result<SafeFile, SourceRegistryError> {\n        if !self.path_is_configured(relative_path) {'''
new = '''    pub(crate) fn open_configured_file(\n        &self,\n        source_id: &str,\n        relative_path: &Path,\n    ) -> Result<SafeFile, SourceRegistryError> {\n        // Local current-file access remains local-only; M5 snapshot reads use SnapshotFile.\n        if !self.path_is_configured(relative_path) {'''
if text.count(old) != 1:
    raise SystemExit(f"open_configured_file: expected one match, got {text.count(old)}")
path.write_text(text.replace(old, new))
