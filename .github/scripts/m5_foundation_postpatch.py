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
