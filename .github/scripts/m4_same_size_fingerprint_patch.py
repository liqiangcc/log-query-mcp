from pathlib import Path


def replace_once(text: str, old: str, new: str, label: str) -> str:
    count = text.count(old)
    if count != 1:
        raise SystemExit(f"{label}: expected exactly one match, got {count}")
    return text.replace(old, new)


path = Path("src/cache/sync.rs")
text = path.read_text()

old = '''    if remote.size == current.remote_size {
        if remote.mtime_millis == current.remote_mtime_millis {
            return Ok(outcome_from_record(
                SyncAction::Unchanged,
                &current,
                budget.used(),
                0,
            ));
        }
        return bootstrap_generation(
            cache,
            target,
            reader,
            &remote,
            SyncGenerationReason::MetadataChangedWithoutGrowth,
            &mut budget,
        )
        .await;
    }

    let Some(expected_fingerprint) = current
        .continuity_fingerprint
        .as_deref()
        .and_then(ContinuityFingerprint::parse)
    else {
        return bootstrap_generation(
            cache,
            target,
            reader,
            &remote,
            SyncGenerationReason::ContinuityUnavailable,
            &mut budget,
        )
        .await;
    };
    if expected_fingerprint.end != current.remote_size {
        return bootstrap_generation(
            cache,
            target,
            reader,
            &remote,
            SyncGenerationReason::ContinuityUnavailable,
            &mut budget,
        )
        .await;
    }

    let observed = fingerprint_remote_window(
        reader,
        &target.remote_path,
        expected_fingerprint.start,
        expected_fingerprint.end,
        &mut budget,
    )
    .await?;
    if observed != expected_fingerprint {
        return bootstrap_generation(
            cache,
            target,
            reader,
            &remote,
            SyncGenerationReason::ContinuityMismatch,
            &mut budget,
        )
        .await;
    }

    append_generation(cache, target, reader, &remote, &current, &mut budget).await
'''

new = '''    let Some(expected_fingerprint) = current
        .continuity_fingerprint
        .as_deref()
        .and_then(ContinuityFingerprint::parse)
    else {
        return bootstrap_generation(
            cache,
            target,
            reader,
            &remote,
            SyncGenerationReason::ContinuityUnavailable,
            &mut budget,
        )
        .await;
    };
    if expected_fingerprint.end != current.remote_size {
        return bootstrap_generation(
            cache,
            target,
            reader,
            &remote,
            SyncGenerationReason::ContinuityUnavailable,
            &mut budget,
        )
        .await;
    }

    let observed = fingerprint_remote_window(
        reader,
        &target.remote_path,
        expected_fingerprint.start,
        expected_fingerprint.end,
        &mut budget,
    )
    .await?;
    if observed != expected_fingerprint {
        return bootstrap_generation(
            cache,
            target,
            reader,
            &remote,
            SyncGenerationReason::ContinuityMismatch,
            &mut budget,
        )
        .await;
    }

    if remote.size == current.remote_size {
        return Ok(outcome_from_record(
            SyncAction::Unchanged,
            &current,
            budget.used(),
            0,
        ));
    }

    append_generation(cache, target, reader, &remote, &current, &mut budget).await
'''
text = replace_once(text, old, new, "sync continuity ordering")

old = '''    #[tokio::test]
    async fn unchanged_metadata_does_not_download_ranges() {
        let temp = TempDir::new().expect("temp");
        let cache = cache(&temp);
        let target = target(BootstrapType::Full, None);
        run(&cache, &target, &FakeReader::new(b"abcdef", 1), 1024)
            .await
            .expect("bootstrap");

        let reader = FakeReader::new(b"abcdef", 1);
        let outcome = run(&cache, &target, &reader, 1024).await.expect("sync");
        assert_eq!(outcome.action, SyncAction::Unchanged);
        assert!(reader.reads().is_empty());
    }
'''
new = '''    #[tokio::test]
    async fn unchanged_file_verifies_continuity_without_downloading_payload() {
        let temp = TempDir::new().expect("temp");
        let cache = cache(&temp);
        let target = target(BootstrapType::Full, None);
        run(&cache, &target, &FakeReader::new(b"abcdef", 1), 1024)
            .await
            .expect("bootstrap");

        let reader = FakeReader::new(b"abcdef", 1);
        let outcome = run(&cache, &target, &reader, 1024).await.expect("sync");
        assert_eq!(outcome.action, SyncAction::Unchanged);
        assert_eq!(outcome.cached_bytes_written, 0);
        assert_eq!(reader.reads(), vec![(0, 6)]);
    }
'''
text = replace_once(text, old, new, "unchanged test")

old = '''    #[tokio::test]
    async fn same_size_mtime_change_is_treated_as_replacement() {
        let temp = TempDir::new().expect("temp");
        let cache = cache(&temp);
        let target = target(BootstrapType::Full, None);
        let first = run(&cache, &target, &FakeReader::new(b"abcdef", 1), 1024)
            .await
            .expect("bootstrap");
        let second = run(&cache, &target, &FakeReader::new(b"uvwxyz", 2), 1024)
            .await
            .expect("replacement");
        assert_eq!(
            second.action,
            SyncAction::NewGeneration(SyncGenerationReason::MetadataChangedWithoutGrowth)
        );
        assert_ne!(second.generation, first.generation);
    }
'''
new = '''    #[tokio::test]
    async fn same_size_replacement_is_caught_even_when_mtime_is_unchanged() {
        let temp = TempDir::new().expect("temp");
        let cache = cache(&temp);
        let target = target(BootstrapType::Full, None);
        let first = run(&cache, &target, &FakeReader::new(b"abcdef", 1), 1024)
            .await
            .expect("bootstrap");
        let second = run(&cache, &target, &FakeReader::new(b"uvwxyz", 1), 1024)
            .await
            .expect("replacement");
        assert_eq!(
            second.action,
            SyncAction::NewGeneration(SyncGenerationReason::ContinuityMismatch)
        );
        assert_ne!(second.generation, first.generation);
    }

    #[tokio::test]
    async fn mtime_change_with_same_content_does_not_rotate_generation() {
        let temp = TempDir::new().expect("temp");
        let cache = cache(&temp);
        let target = target(BootstrapType::Full, None);
        let first = run(&cache, &target, &FakeReader::new(b"abcdef", 1), 1024)
            .await
            .expect("bootstrap");
        let second = run(&cache, &target, &FakeReader::new(b"abcdef", 2), 1024)
            .await
            .expect("unchanged content");
        assert_eq!(second.action, SyncAction::Unchanged);
        assert_eq!(second.generation, first.generation);
    }
'''
text = replace_once(text, old, new, "same-size replacement test")

path.write_text(text)
