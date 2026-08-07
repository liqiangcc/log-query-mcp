use std::{
    fs::{self, OpenOptions},
    path::{Path, PathBuf},
    time::Duration,
};

use log_query_mcp::{
    ByteRange, CacheCoverage, CacheStore, CacheStoreError, CacheStoreLimits, GenerationMetadata,
    SourceRegistryError, ToolError, ToolErrorCode,
};
use tempfile::TempDir;

const SOURCE_ID: &str = "m6-cache-source";
const REMOTE_ID: &str = "application.log";

#[test]
fn externally_deleted_active_generation_fails_closed_and_recovery_rejects_store() {
    let temp = TempDir::new().expect("cache tempdir");
    let store = cache(&temp);
    publish_generation(&store, b"stable-generation\n");

    let generation_path = only_generation_file(temp.path());
    fs::remove_file(&generation_path).expect("delete active generation externally");

    let error = store
        .pin_current_generation(SOURCE_ID, REMOTE_ID)
        .expect_err("missing active generation must not be treated as a cache miss");
    assert!(matches!(error, CacheStoreError::Io(_)));
    assert_cache_corrupted_wire_error(error, temp.path());

    let manifest = store
        .load_manifest(SOURCE_ID, REMOTE_ID)
        .expect("manifest remains readable")
        .expect("manifest remains published");
    assert!(manifest.current_generation.is_some());

    let restart_error = CacheStore::open(temp.path(), limits())
        .expect_err("restart recovery must reject a manifest that references missing data");
    assert!(matches!(restart_error, CacheStoreError::Io(_)));
    assert!(!restart_error.to_string().contains(&temp.path().to_string_lossy().into_owned()));
}

#[test]
fn externally_truncated_active_generation_fails_closed_as_cache_corruption() {
    let temp = TempDir::new().expect("cache tempdir");
    let store = cache(&temp);
    publish_generation(&store, b"stable-generation\n");

    let generation_path = only_generation_file(temp.path());
    OpenOptions::new()
        .write(true)
        .open(&generation_path)
        .expect("open generation for external truncation")
        .set_len(1)
        .expect("truncate active generation externally");

    let error = store
        .pin_current_generation(SOURCE_ID, REMOTE_ID)
        .expect_err("truncated active generation must not be served");
    assert!(matches!(
        error,
        CacheStoreError::GenerationLengthMismatch { .. }
    ));
    assert_cache_corrupted_wire_error(error, temp.path());

    let restart_error = CacheStore::open(temp.path(), limits())
        .expect_err("restart recovery must reject truncated generation data");
    assert!(matches!(
        restart_error,
        CacheStoreError::GenerationLengthMismatch { .. }
    ));
}

fn cache(temp: &TempDir) -> CacheStore {
    CacheStore::open(temp.path(), limits()).expect("cache store")
}

fn limits() -> CacheStoreLimits {
    CacheStoreLimits {
        max_bytes: 4 * 1024 * 1024,
        max_bytes_per_source: 2 * 1024 * 1024,
        retention: Duration::from_secs(3600),
        max_generations_per_file: 4,
    }
}

fn publish_generation(store: &CacheStore, bytes: &[u8]) {
    use std::io::Write;

    let mut staged = store
        .begin_generation(SOURCE_ID, REMOTE_ID)
        .expect("stage generation");
    staged.write_all(bytes).expect("write generation");
    let len = u64::try_from(bytes.len()).expect("generation length");
    staged
        .commit(GenerationMetadata {
            remote_size: len,
            cached_range: ByteRange::new(0, len).expect("cached range"),
            remote_mtime_millis: Some(1),
            continuity_fingerprint: Some("sha256-v1:0:0:0000000000000000000000000000000000000000000000000000000000000000".to_owned()),
            coverage: CacheCoverage::Full,
        })
        .expect("commit generation");
}

fn only_generation_file(root: &Path) -> PathBuf {
    let files = all_paths(root)
        .into_iter()
        .filter(|path| path.extension().is_some_and(|extension| extension == "log"))
        .collect::<Vec<_>>();
    assert_eq!(files.len(), 1, "expected exactly one generation data file");
    files.into_iter().next().expect("generation file")
}

fn all_paths(root: &Path) -> Vec<PathBuf> {
    let mut output = Vec::new();
    let mut stack = vec![root.to_path_buf()];
    while let Some(directory) = stack.pop() {
        for entry in fs::read_dir(directory).expect("read cache directory") {
            let path = entry.expect("cache entry").path();
            let metadata = fs::symlink_metadata(&path).expect("cache entry metadata");
            output.push(path.clone());
            if metadata.is_dir() {
                stack.push(path);
            }
        }
    }
    output
}

fn assert_cache_corrupted_wire_error(error: CacheStoreError, root: &Path) {
    let internal_source = "internal-source-id";
    let internal_file = "internal-file-id";
    let tool = ToolError::from(SourceRegistryError::CachedGenerationUnavailable {
        source_id: internal_source.to_owned(),
        file_id: internal_file.to_owned(),
        source: error,
    });
    assert_eq!(tool.code, ToolErrorCode::CacheCorrupted);
    let json = tool.to_json_string().expect("serialize tool error");
    assert!(!json.contains(internal_source));
    assert!(!json.contains(internal_file));
    assert!(!json.contains(REMOTE_ID));
    assert!(!json.contains(&root.to_string_lossy().into_owned()));
    assert!(!json.contains("backtrace"));
}
