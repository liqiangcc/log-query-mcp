from pathlib import Path


def replace_once(path: str, old: str, new: str, label: str) -> None:
    file = Path(path)
    text = file.read_text()
    count = text.count(old)
    if count != 1:
        raise SystemExit(f"{label}: expected one match, got {count}")
    file.write_text(text.replace(old, new))


replace_once(
    "src/backend/mod.rs",
    '''    Remote {\n        reader: PinnedGeneration,\n        identity: FileIdentity,\n        size: u64,\n    },''',
    '''    Remote {\n        reader: Box<PinnedGeneration>,\n        identity: FileIdentity,\n        size: u64,\n    },''',
    "box remote snapshot reader",
)
replace_once(
    "src/backend/mod.rs",
    '''pub(crate) enum SourceBackend {\n    Local(LocalBackend),\n    Remote(RemoteBackend),\n}''',
    '''pub(crate) enum SourceBackend {\n    Local(LocalBackend),\n    Remote(Box<RemoteBackend>),\n}''',
    "box remote backend",
)
replace_once(
    "src/backend/remote.rs",
    '''        Ok(SnapshotFile::Remote {\n            reader,\n            identity,\n            size: size_at_snapshot,\n        })''',
    '''        Ok(SnapshotFile::Remote {\n            reader: Box::new(reader),\n            identity,\n            size: size_at_snapshot,\n        })''',
    "box snapshot reader construction",
)
replace_once(
    "src/backend/remote.rs",
    '''        Ok(SnapshotFile::Remote {\n            reader,\n            identity,\n            size,\n        })''',
    '''        Ok(SnapshotFile::Remote {\n            reader: Box::new(reader),\n            identity,\n            size,\n        })''',
    "box referenced reader construction",
)
replace_once(
    "src/source_registry.rs",
    '''                    SourceBackend::Remote(RemoteBackend::new(\n                        source_config.clone(),\n                        cache.clone(),\n                        sync.clone(),\n                        connections.clone(),\n                        config.limits.max_remote_files_per_source,\n                    )?)''',
    '''                    SourceBackend::Remote(Box::new(RemoteBackend::new(\n                        source_config.clone(),\n                        cache.clone(),\n                        sync.clone(),\n                        connections.clone(),\n                        config.limits.max_remote_files_per_source,\n                    )?))''',
    "box remote backend construction",
)
