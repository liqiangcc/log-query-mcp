from pathlib import Path

path = Path("src/backend/mod.rs")
text = path.read_text()
old = '''    pub fn size(&self) -> u64 {\n        match self {\n            Self::Local(file) => file.size(),\n            Self::Remote { size, .. } => *size,\n        }\n    }\n}\n\nimpl Read for SnapshotFile {'''
new = '''    pub fn size(&self) -> u64 {\n        match self {\n            Self::Local(file) => file.size(),\n            Self::Remote { size, .. } => *size,\n        }\n    }\n\n    #[must_use]\n    pub fn into_file(self) -> Self {\n        self\n    }\n}\n\nimpl Read for SnapshotFile {'''
if text.count(old) != 1:
    raise SystemExit(f"SnapshotFile compatibility: expected one match, got {text.count(old)}")
path.write_text(text.replace(old, new))
