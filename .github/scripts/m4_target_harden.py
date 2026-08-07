from pathlib import Path


def replace_once(text: str, old: str, new: str, label: str) -> str:
    count = text.count(old)
    if count != 1:
        raise SystemExit(f"{label}: expected one match, found {count}")
    return text.replace(old, new)


path = Path("src/cache/sync.rs")
text = path.read_text()
text = replace_once(
    text,
    '''use std::{
    fmt,
    fmt::Write as FmtWrite,
    io::{Read, Seek, SeekFrom, Write},
};''',
    '''use std::{
    fmt,
    fmt::Write as FmtWrite,
    io::{Read, Seek, SeekFrom, Write},
    path::Path,
};''',
    "import Path",
)
text = replace_once(
    text,
    '''    pub fn from_source(
        source: &LogSourceConfigV2,
        remote_identifier: impl Into<String>,
        remote_path: impl Into<String>,
    ) -> Result<Self, SyncError> {
        if source.backend.backend_type != BackendType::Ssh {
            return Err(SyncError::InvalidTarget);
        }
        let connection_id = source
            .backend
            .connection_id
            .clone()
            .ok_or(SyncError::InvalidTarget)?;
        let sync = source.sync.clone().ok_or(SyncError::InvalidTarget)?;
        if sync.allow_stale_on_error {
            return Err(SyncError::InvalidTarget);
        }
        let remote_identifier = remote_identifier.into();
        let remote_path = remote_path.into();
        if remote_identifier.is_empty() || remote_path.is_empty() {
            return Err(SyncError::InvalidTarget);
        }

        Ok(Self {
            connection_id,
            source_identifier: source.source_id.clone(),
            remote_identifier,
            remote_path,
            bootstrap: sync.bootstrap,
        })
    }''',
    '''    pub fn from_source(
        source: &LogSourceConfigV2,
        remote_identifier: impl Into<String>,
    ) -> Result<Self, SyncError> {
        if source.backend.backend_type != BackendType::Ssh {
            return Err(SyncError::InvalidTarget);
        }
        let connection_id = source
            .backend
            .connection_id
            .clone()
            .ok_or(SyncError::InvalidTarget)?;
        let sync = source.sync.clone().ok_or(SyncError::InvalidTarget)?;
        if sync.allow_stale_on_error {
            return Err(SyncError::InvalidTarget);
        }
        let remote_identifier = remote_identifier.into();
        let remote_path = build_remote_path(&source.root, &remote_identifier)?;

        Ok(Self {
            connection_id,
            source_identifier: source.source_id.clone(),
            remote_identifier,
            remote_path,
            bootstrap: sync.bootstrap,
        })
    }''',
    "derive remote path from source",
)
anchor = '''#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SyncAction {'''
helper = '''fn build_remote_path(root: &Path, remote_identifier: &str) -> Result<String, SyncError> {
    let root = root.to_str().ok_or(SyncError::InvalidTarget)?;
    if root.is_empty()
        || !root.starts_with('/')
        || root.len() > 4096
        || root.chars().any(char::is_control)
    {
        return Err(SyncError::InvalidTarget);
    }
    if remote_identifier.is_empty()
        || remote_identifier.len() > 4096
        || remote_identifier.starts_with('/')
        || remote_identifier.contains('\\\\')
        || remote_identifier.chars().any(char::is_control)
        || remote_identifier
            .split('/')
            .any(|component| component.is_empty() || component == "." || component == "..")
    {
        return Err(SyncError::InvalidTarget);
    }

    let root = if root == "/" {
        ""
    } else {
        root.trim_end_matches('/')
    };
    Ok(format!("{root}/{remote_identifier}"))
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SyncAction {'''
text = replace_once(text, anchor, helper, "insert remote path builder")

test_anchor = '''    async fn run(
        cache: &CacheStore,
        target: &RemoteSyncTarget,
        reader: &FakeReader,
        limit: u64,
    ) -> Result<SyncOutcome, SyncError> {
        sync_with_reader(cache, target, limit, reader).await
    }

    #[tokio::test]
    async fn full_bootstrap_publishes_stable_generation() {'''
test_replacement = '''    async fn run(
        cache: &CacheStore,
        target: &RemoteSyncTarget,
        reader: &FakeReader,
        limit: u64,
    ) -> Result<SyncOutcome, SyncError> {
        sync_with_reader(cache, target, limit, reader).await
    }

    #[test]
    fn configured_target_derives_remote_path_and_rejects_escape() {
        const CONFIG: &str =
            include_str!("../../tests/contracts/v2/valid/ssh-password-tail.json");
        let config = AppConfigV2::from_json_str(CONFIG).expect("config");
        let source = &config.sources[0];
        let target = RemoteSyncTarget::from_source(source, "application.log").expect("target");
        assert_eq!(target.remote_path, "/data/log/order-service/application.log");
        assert!(matches!(
            RemoteSyncTarget::from_source(source, "../secret.log"),
            Err(SyncError::InvalidTarget)
        ));
        assert!(matches!(
            RemoteSyncTarget::from_source(source, "/etc/passwd"),
            Err(SyncError::InvalidTarget)
        ));
    }

    #[tokio::test]
    async fn full_bootstrap_publishes_stable_generation() {'''
text = replace_once(text, test_anchor, test_replacement, "add target hardening test")
path.write_text(text)
