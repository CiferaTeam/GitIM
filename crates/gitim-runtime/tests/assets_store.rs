#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]

use axum::{body::Bytes, http::StatusCode};
use futures::{stream, StreamExt};
use gitim_runtime::assets::{
    checked_dimensions, inspect_bytes, AssetError, AssetLimits, AssetService, AssetSource,
    AssetStore, AssetUsage, HashLock, RequestBudget,
};
use sha2::{Digest, Sha256};
use std::fs::{self, FileTimes, OpenOptions};
use std::io::Write;
use std::path::{Path, PathBuf};
use std::process::{Child, Command, Stdio};
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::sync::{Arc, Barrier};
use std::time::{Duration, Instant, SystemTime};
use tempfile::TempDir;

const PNG_1X1: &[u8] = &[
    0x89, b'P', b'N', b'G', 0x0d, 0x0a, 0x1a, 0x0a, 0, 0, 0, 13, b'I', b'H', b'D', b'R', 0, 0, 0,
    1, 0, 0, 0, 1, 8, 6, 0, 0, 0, 0x1f, 0x15, 0xc4, 0x89,
];
const JPEG_1X1: &[u8] = &[
    0xff, 0xd8, 0xff, 0xe0, 0, 16, b'J', b'F', b'I', b'F', 0, 1, 1, 0, 0, 1, 0, 1, 0, 0, 0xff,
    0xc0, 0, 17, 8, 0, 1, 0, 1, 3, 1, 0x11, 0, 2, 0x11, 0, 3, 0x11, 0, 0xff, 0xd9,
];
const GIF_1X1: &[u8] = b"GIF89a\x01\x00\x01\x00\x80\x00\x00\x00\x00\x00\xff\xff\xff";
const WEBP_1X1: &[u8] = &[
    b'R', b'I', b'F', b'F', 22, 0, 0, 0, b'W', b'E', b'B', b'P', b'V', b'P', b'8', b'X', 10, 0, 0,
    0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0,
];
const RUNTIME_ID_1: &str = "550e8400-e29b-41d4-a716-446655440000";
const RUNTIME_ID_2: &str = "6ba7b810-9dad-41d1-80b4-00c04fd430c8";

fn bmff_box(kind: &[u8; 4], payload: &[u8]) -> Vec<u8> {
    let size = u32::try_from(payload.len() + 8).unwrap();
    let mut bytes = Vec::with_capacity(size as usize);
    bytes.extend_from_slice(&size.to_be_bytes());
    bytes.extend_from_slice(kind);
    bytes.extend_from_slice(payload);
    bytes
}

fn valid_avif(width: u32, height: u32) -> Vec<u8> {
    let mut ftyp = Vec::new();
    ftyp.extend_from_slice(b"avif");
    ftyp.extend_from_slice(&0_u32.to_be_bytes());
    ftyp.extend_from_slice(b"avifmif1");

    let mut ispe = vec![0, 0, 0, 0];
    ispe.extend_from_slice(&width.to_be_bytes());
    ispe.extend_from_slice(&height.to_be_bytes());
    let ispe = bmff_box(b"ispe", &ispe);
    let ipco = bmff_box(b"ipco", &ispe);
    let iprp = bmff_box(b"iprp", &ipco);
    let mut meta = vec![0, 0, 0, 0];
    meta.extend_from_slice(&iprp);

    let mut bytes = bmff_box(b"ftyp", &ftyp);
    bytes.extend_from_slice(&bmff_box(b"meta", &meta));
    bytes
}

fn avif_with_decoy_ispe(width: u32, height: u32) -> Vec<u8> {
    let mut ftyp = Vec::new();
    ftyp.extend_from_slice(b"avif");
    ftyp.extend_from_slice(&0_u32.to_be_bytes());
    ftyp.extend_from_slice(b"avifmif1");
    let mut ispe = vec![0, 0, 0, 0];
    ispe.extend_from_slice(&width.to_be_bytes());
    ispe.extend_from_slice(&height.to_be_bytes());
    let mut bytes = bmff_box(b"ftyp", &ftyp);
    bytes.extend_from_slice(&bmff_box(b"free", &bmff_box(b"ispe", &ispe)));
    bytes
}

fn png_with_dimensions(width: u32, height: u32) -> Vec<u8> {
    let mut bytes = PNG_1X1.to_vec();
    bytes[16..20].copy_from_slice(&width.to_be_bytes());
    bytes[20..24].copy_from_slice(&height.to_be_bytes());
    bytes
}

fn limits(quota: u64) -> AssetLimits {
    AssetLimits {
        workspace_quota_bytes: quota,
        min_free_bytes: 0,
        max_file_bytes: 1024 * 1024,
        max_request_bytes: 4 * 1024 * 1024,
        max_files: 10,
        temp_ttl: Duration::from_secs(24 * 60 * 60),
        upload_slots: 2,
        peer_slots: 4,
    }
}

fn open_store(workspace: &Path, quota: u64) -> AssetStore {
    open_store_result(workspace, quota).unwrap()
}

fn open_store_result(workspace: &Path, quota: u64) -> Result<AssetStore, AssetError> {
    AssetStore::open(workspace, "github:github.com/acme/repo", limits(quota))
}

fn sha256_hex(bytes: &[u8]) -> String {
    format!("{:x}", Sha256::digest(bytes))
}

fn orphaned_asset_trees(workspace: &Path) -> Vec<PathBuf> {
    let path = workspace.join(".gitim-runtime/orphaned-assets");
    let mut entries: Vec<PathBuf> = fs::read_dir(path)
        .map(|entries| entries.flatten().map(|entry| entry.path()).collect())
        .unwrap_or_default();
    entries.sort();
    entries
}

fn write_object_only(store: &AssetStore, bytes: &[u8]) -> String {
    let hash = sha256_hex(bytes);
    let path = store.object_path(&hash).unwrap();
    fs::create_dir_all(path.parent().unwrap()).unwrap();
    fs::write(path, bytes).unwrap();
    hash
}

fn read_sidecar_json(store: &AssetStore, hash: &str) -> serde_json::Value {
    serde_json::from_slice(&fs::read(store.metadata_path(hash).unwrap()).unwrap()).unwrap()
}

fn write_sidecar_json(store: &AssetStore, hash: &str, value: &serde_json::Value) {
    fs::write(
        store.metadata_path(hash).unwrap(),
        serde_json::to_vec(value).unwrap(),
    )
    .unwrap();
}

fn owned_temp_files(store: &AssetStore) -> Vec<PathBuf> {
    let mut paths: Vec<_> = fs::read_dir(store.root().join("tmp"))
        .unwrap()
        .flatten()
        .filter(|entry| {
            let name = entry.file_name();
            let name = name.to_string_lossy();
            name.starts_with("gitim-asset-") && name.ends_with(".tmp")
        })
        .map(|entry| entry.path())
        .collect();
    paths.sort();
    paths
}

fn spawn_hash_lock_child(workspace: &Path, hash: &str, mode: &str, marker: Option<&Path>) -> Child {
    let mut command = Command::new(std::env::current_exe().unwrap());
    command
        .arg("--exact")
        .arg("hash_lock_child")
        .arg("--ignored")
        .arg("--nocapture")
        .env("GITIM_ASSET_LOCK_CHILD", mode)
        .env("GITIM_ASSET_LOCK_WORKSPACE", workspace)
        .env("GITIM_ASSET_LOCK_HASH", hash)
        .stdout(Stdio::piped())
        .stderr(Stdio::piped());
    if let Some(marker) = marker {
        command.env("GITIM_ASSET_LOCK_MARKER", marker);
    }
    command.spawn().unwrap()
}

fn wait_for_marker(child: &mut Child, marker: &Path) {
    let deadline = Instant::now() + Duration::from_secs(5);
    while !marker.exists() {
        assert!(
            child.try_wait().unwrap().is_none(),
            "lock holder exited before publishing its marker"
        );
        assert!(
            Instant::now() < deadline,
            "timed out waiting for lock marker"
        );
        std::thread::sleep(Duration::from_millis(10));
    }
}

async fn wait_for_lock_attempts(store: &AssetStore, minimum: u64) {
    tokio::time::timeout(Duration::from_secs(2), async {
        while store.hash_lock_attempts() < minimum {
            tokio::task::yield_now().await;
        }
    })
    .await
    .unwrap();
}

fn assert_sidecar_mutation_is_rebuilt(mutator: impl FnOnce(&mut serde_json::Value)) {
    let workspace = TempDir::new().unwrap();
    let store = open_store(workspace.path(), 1024);
    let stored = store.put_bytes(PNG_1X1, AssetSource::LocalUpload).unwrap();
    let mut sidecar = read_sidecar_json(&store, &stored.sha256);
    mutator(&mut sidecar);
    write_sidecar_json(&store, &stored.sha256, &sidecar);

    store.recover().unwrap();
    let repaired = store.inspect(&stored.sha256).unwrap();
    assert_eq!(repaired.media_type, "image/png");
    assert_eq!((repaired.width, repaired.height), (Some(1), Some(1)));
}

#[tokio::test]
async fn streaming_staging_hashes_ordered_chunks_and_inspects_only_the_prefix() {
    let workspace = TempDir::new().unwrap();
    let store = open_store(workspace.path(), 1024);
    let mut budget = RequestBudget::default();
    let chunks = stream::iter(vec![
        Ok::<_, std::io::Error>(Bytes::from_static(b"ab")),
        Ok(Bytes::from_static(b"c")),
    ]);

    let staged = store
        .stage_stream("a.txt", chunks, &mut budget)
        .await
        .unwrap();

    assert_eq!(staged.size(), 3);
    assert_eq!(staged.sha256(), sha256_hex(b"abc"));
    assert_eq!(staged.media_type(), "application/octet-stream");
    assert_eq!(budget.bytes(), 3);
    assert_eq!(budget.files(), 1);
    assert_eq!(owned_temp_files(&store).len(), 1);
    drop(staged);
    assert!(owned_temp_files(&store).is_empty());
    assert_eq!(store.reserved_bytes().unwrap(), 0);
}

#[tokio::test]
async fn streaming_limits_stop_before_polling_or_writing_later_chunks() {
    let workspace = TempDir::new().unwrap();
    let mut constrained = limits(1024);
    constrained.max_file_bytes = 3;
    let store = AssetStore::open(workspace.path(), "local:stream", constrained).unwrap();
    let polls = Arc::new(AtomicUsize::new(0));
    let stream_polls = Arc::clone(&polls);
    let chunks = stream::poll_fn(move |_| {
        let poll = stream_polls.fetch_add(1, Ordering::SeqCst);
        match poll {
            0 => std::task::Poll::Ready(Some(Ok::<_, std::io::Error>(Bytes::from_static(
                b"toolong",
            )))),
            1 => panic!("stream was polled after the file limit was known"),
            _ => std::task::Poll::Ready(None),
        }
    });
    let mut budget = RequestBudget::default();

    let error = store
        .stage_stream("large.bin", chunks, &mut budget)
        .await
        .unwrap_err();

    assert!(matches!(error, AssetError::TooLarge { limit: 3 }));
    assert_eq!(polls.load(Ordering::SeqCst), 1);
    assert_eq!((budget.files(), budget.bytes()), (0, 0));
    assert!(owned_temp_files(&store).is_empty());
    assert_eq!(store.reserved_bytes().unwrap(), 0);
}

#[tokio::test]
async fn streaming_enforces_aggregate_bytes_and_file_count_incrementally() {
    let workspace = TempDir::new().unwrap();
    let mut constrained = limits(1024);
    constrained.max_request_bytes = 5;
    constrained.max_files = 2;
    let store = AssetStore::open(workspace.path(), "local:batch-limits", constrained).unwrap();
    let mut budget = RequestBudget::default();
    let first = store
        .stage_stream(
            "one.bin",
            stream::iter([Ok::<_, std::io::Error>(Bytes::from_static(b"abc"))]),
            &mut budget,
        )
        .await
        .unwrap();
    let aggregate_error = store
        .stage_stream(
            "two.bin",
            stream::iter([Ok::<_, std::io::Error>(Bytes::from_static(b"def"))]),
            &mut budget,
        )
        .await
        .unwrap_err();
    assert!(matches!(
        aggregate_error,
        AssetError::RequestTooLarge { limit: 5 }
    ));
    assert_eq!((budget.files(), budget.bytes()), (1, 3));

    let second = store
        .stage_stream(
            "two.bin",
            stream::iter([Ok::<_, std::io::Error>(Bytes::from_static(b"de"))]),
            &mut budget,
        )
        .await
        .unwrap();
    let count_error = store
        .stage_stream(
            "three.bin",
            stream::empty::<Result<Bytes, std::io::Error>>(),
            &mut budget,
        )
        .await
        .unwrap_err();
    assert!(matches!(count_error, AssetError::TooMany { limit: 2 }));
    assert_eq!((budget.files(), budget.bytes()), (2, 5));
    drop((first, second));
    assert!(owned_temp_files(&store).is_empty());
    assert_eq!(store.reserved_bytes().unwrap(), 0);
}

#[tokio::test]
async fn streaming_transport_error_releases_request_quota_and_tempfile() {
    let workspace = TempDir::new().unwrap();
    let store = open_store(workspace.path(), 1024);
    let mut budget = RequestBudget::default();
    let chunks = stream::iter(vec![
        Ok(Bytes::from_static(b"partial")),
        Err(std::io::Error::new(
            std::io::ErrorKind::ConnectionReset,
            "stream reset",
        )),
    ]);

    let error = store
        .stage_stream("reset.bin", chunks, &mut budget)
        .await
        .unwrap_err();

    assert_eq!(error.error_code(), "asset_store_failed");
    assert_eq!((budget.files(), budget.bytes()), (0, 0));
    assert_eq!(store.reserved_bytes().unwrap(), 0);
    assert!(owned_temp_files(&store).is_empty());
}

#[tokio::test]
async fn cancelling_pending_stream_leaves_request_budget_unchanged_and_reusable() {
    let workspace = TempDir::new().unwrap();
    let store = open_store(workspace.path(), 1024);
    let mut budget = RequestBudget::default();
    let pending = stream::iter([Ok::<_, std::io::Error>(Bytes::from_static(b"partial"))])
        .chain(stream::pending());

    assert!(tokio::time::timeout(
        Duration::from_millis(50),
        store.stage_stream("pending.bin", pending, &mut budget),
    )
    .await
    .is_err());
    assert_eq!((budget.files(), budget.bytes()), (0, 0));
    assert_eq!(store.reserved_bytes().unwrap(), 0);
    assert!(owned_temp_files(&store).is_empty());

    let staged = store
        .stage_stream(
            "reuse.bin",
            stream::iter([Ok::<_, std::io::Error>(Bytes::from_static(b"ok"))]),
            &mut budget,
        )
        .await
        .unwrap();
    assert_eq!((budget.files(), budget.bytes()), (1, 2));
    drop(staged);
}

#[tokio::test]
async fn materialized_temp_bytes_are_not_double_counted_by_free_space_checks() {
    let workspace = TempDir::new().unwrap();
    let store = open_store(workspace.path(), 100);
    store.inject_free_space(4, 0);
    let first = store.stage_bytes("first.bin", b"aaaa").await.unwrap();
    let second = store.stage_bytes("second.bin", b"bbbb").await.unwrap();

    assert_eq!(store.reserved_bytes().unwrap(), 8);
    drop((first, second));
    assert_eq!(store.reserved_bytes().unwrap(), 0);
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn stale_available_space_sample_cannot_outlive_a_materialized_claim() {
    let workspace = TempDir::new().unwrap();
    let store = Arc::new(open_store(workspace.path(), 100));
    store.inject_free_space(8, 0);
    let materialize_reached = Arc::new(Barrier::new(2));
    let materialize_resume = Arc::new(Barrier::new(2));
    let materialized = Arc::new(AtomicBool::new(false));
    store.inject_materialize_pause(
        Arc::clone(&materialize_reached),
        Arc::clone(&materialize_resume),
        Arc::clone(&materialized),
    );
    let first_store = Arc::clone(&store);
    let first = tokio::spawn(async move { first_store.stage_bytes("first.bin", b"aaaa").await });
    materialize_reached.wait();

    let sampled = Arc::new(Barrier::new(2));
    store.inject_after_free_space_sample_wait(Arc::clone(&sampled), Arc::clone(&materialized));
    let second_store = Arc::clone(&store);
    let second =
        tokio::spawn(async move { second_store.stage_bytes("second.bin", b"bbbbb").await });
    sampled.wait();
    materialize_resume.wait();

    let second_error = second.await.unwrap().unwrap_err();
    assert!(matches!(second_error, AssetError::QuotaExceeded { .. }));
    let first = first.await.unwrap().unwrap();
    assert!(materialized.load(Ordering::Acquire));
    drop(first);
    assert_eq!(store.reserved_bytes().unwrap(), 0);
}

#[tokio::test]
async fn batch_validates_every_reference_before_persisting_any_object() {
    let workspace = TempDir::new().unwrap();
    let store = open_store(workspace.path(), 1024);
    let mut budget = RequestBudget::default();
    let valid = store
        .stage_stream(
            "ok.txt",
            stream::iter([Ok::<_, std::io::Error>(Bytes::from_static(b"ok"))]),
            &mut budget,
        )
        .await
        .unwrap();
    let invalid = store
        .stage_stream(
            &"界".repeat(100),
            stream::iter([Ok::<_, std::io::Error>(Bytes::from_static(b"later"))]),
            &mut budget,
        )
        .await
        .unwrap();

    let error = store
        .persist_batch(RUNTIME_ID_1, vec![valid, invalid])
        .await
        .unwrap_err();

    assert_eq!(error.error_code(), "invalid_asset");
    assert_eq!(store.usage().unwrap(), AssetUsage::default());
    assert_eq!(store.reserved_bytes().unwrap(), 0);
    assert!(owned_temp_files(&store).is_empty());
}

#[tokio::test]
async fn batch_rechecks_aggregate_size_for_independently_staged_files() {
    let workspace = TempDir::new().unwrap();
    let mut constrained = limits(1024);
    constrained.max_request_bytes = 5;
    let store = AssetStore::open(workspace.path(), "local:batch-total", constrained).unwrap();
    let first = store.stage_bytes("one.bin", b"abc").await.unwrap();
    let second = store.stage_bytes("two.bin", b"def").await.unwrap();

    let error = store
        .persist_batch(RUNTIME_ID_1, vec![first, second])
        .await
        .unwrap_err();

    assert!(matches!(error, AssetError::RequestTooLarge { limit: 5 }));
    assert_eq!(store.usage().unwrap(), AssetUsage::default());
    assert_eq!(store.reserved_bytes().unwrap(), 0);
    assert!(owned_temp_files(&store).is_empty());
}

#[tokio::test]
async fn batch_persistence_error_returns_no_refs_and_retry_converges() {
    let workspace = TempDir::new().unwrap();
    let store = open_store(workspace.path(), 1024);
    let staged = store.stage_bytes("retry.txt", b"durable").await.unwrap();
    let hash = staged.sha256().to_string();
    store.inject_sidecar_write_failure_once();

    let error = store
        .persist_batch(RUNTIME_ID_1, vec![staged])
        .await
        .unwrap_err();

    assert_eq!(error.error_code(), "asset_store_failed");
    assert_eq!(store.read(&hash).unwrap(), b"durable");
    assert_eq!(
        store.usage().unwrap(),
        AssetUsage {
            bytes: 7,
            objects: 1,
        }
    );

    let retry = store.stage_bytes("retry.txt", b"durable").await.unwrap();
    let refs = store
        .persist_batch(RUNTIME_ID_1, vec![retry])
        .await
        .unwrap();
    assert_eq!(refs.len(), 1);
    assert_eq!(refs[0].sha256, hash);
    assert_eq!(store.reserved_bytes().unwrap(), 0);
    assert!(owned_temp_files(&store).is_empty());
}

#[tokio::test]
async fn failure_after_no_clobber_publish_registers_the_object_before_returning() {
    let workspace = TempDir::new().unwrap();
    let store = open_store(workspace.path(), 8);
    let staged = store.stage_bytes("published.txt", b"four").await.unwrap();
    let hash = staged.sha256().to_string();
    store.inject_after_publish_failure_once();

    let error = store
        .persist_staged(staged, AssetSource::LocalUpload)
        .await
        .unwrap_err();

    assert_eq!(error.error_code(), "asset_store_failed");
    assert_eq!(store.read(&hash).unwrap(), b"four");
    assert_eq!(
        store.usage().unwrap(),
        AssetUsage {
            bytes: 4,
            objects: 1,
        }
    );
    assert_eq!(store.reserved_bytes().unwrap(), 0);
    assert!(matches!(
        store.put_bytes(b"12345", AssetSource::LocalUpload),
        Err(AssetError::QuotaExceeded { .. })
    ));
}

#[tokio::test]
async fn concurrent_staged_dedupe_publishes_one_object_and_counts_it_once() {
    let workspace = TempDir::new().unwrap();
    let store = Arc::new(open_store(workspace.path(), 1024));
    let staged_a = store.stage_bytes("a.txt", b"same").await.unwrap();
    let staged_b = store.stage_bytes("b.txt", b"same").await.unwrap();
    let first_store = Arc::clone(&store);
    let second_store = Arc::clone(&store);
    let first = tokio::spawn(async move {
        first_store
            .persist_staged(staged_a, AssetSource::LocalUpload)
            .await
    });
    let second = tokio::spawn(async move {
        second_store
            .persist_staged(staged_b, AssetSource::LocalUpload)
            .await
    });

    let (first, second) = tokio::join!(first, second);
    let first = first.unwrap().unwrap();
    let second = second.unwrap().unwrap();
    assert_eq!(first.sha256, second.sha256);
    assert_eq!(store.read(&first.sha256).unwrap(), b"same");
    assert_eq!(
        store.usage().unwrap(),
        AssetUsage {
            bytes: 4,
            objects: 1,
        }
    );
    assert!(owned_temp_files(&store).is_empty());
}

#[cfg(unix)]
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn no_clobber_publish_keeps_a_valid_object_that_wins_the_link_race() {
    use std::os::unix::fs::{MetadataExt, PermissionsExt};

    let workspace = TempDir::new().unwrap();
    let store = Arc::new(open_store(workspace.path(), 1024));
    let staged = store.stage_bytes("race.txt", b"winner").await.unwrap();
    let hash = staged.sha256().to_string();
    let object = store.object_path(&hash).unwrap();
    let reached = Arc::new(Barrier::new(2));
    let resume = Arc::new(Barrier::new(2));
    store.inject_before_publish_pause(Arc::clone(&reached), Arc::clone(&resume));
    let persist_store = Arc::clone(&store);
    let persist = tokio::spawn(async move {
        persist_store
            .persist_staged(staged, AssetSource::LocalUpload)
            .await
    });

    reached.wait();
    fs::create_dir_all(object.parent().unwrap()).unwrap();
    fs::write(&object, b"winner").unwrap();
    fs::set_permissions(&object, fs::Permissions::from_mode(0o644)).unwrap();
    let winning_inode = fs::metadata(&object).unwrap().ino();
    resume.wait();

    let stored = persist.await.unwrap().unwrap();
    assert_eq!(stored.sha256, hash);
    assert_eq!(fs::metadata(&object).unwrap().ino(), winning_inode);
    assert_eq!(
        fs::metadata(&object).unwrap().permissions().mode() & 0o777,
        0o600
    );
    assert_eq!(
        fs::metadata(store.metadata_path(&hash).unwrap())
            .unwrap()
            .permissions()
            .mode()
            & 0o777,
        0o600
    );
    assert_eq!(store.read(&hash).unwrap(), b"winner");
    assert_eq!(
        store.usage().unwrap(),
        AssetUsage {
            bytes: 6,
            objects: 1,
        }
    );
    assert_eq!(store.reserved_bytes().unwrap(), 0);
}

#[tokio::test]
async fn staged_dedupe_repairs_same_length_corruption_before_discarding_known_good_bytes() {
    let workspace = TempDir::new().unwrap();
    let store = open_store(workspace.path(), 1024);
    let original = store.put_bytes(b"good", AssetSource::LocalUpload).unwrap();
    fs::write(store.object_path(&original.sha256).unwrap(), b"evil").unwrap();
    let staged = store.stage_bytes("repair.txt", b"good").await.unwrap();

    let repaired = store
        .persist_staged(staged, AssetSource::LocalUpload)
        .await
        .unwrap();

    assert_eq!(repaired.sha256, original.sha256);
    assert_eq!(store.read(&original.sha256).unwrap(), b"good");
    assert_eq!(
        store.usage().unwrap(),
        AssetUsage {
            bytes: 4,
            objects: 1,
        }
    );
}

#[tokio::test]
async fn staged_persistence_quarantines_a_directory_at_the_object_path_without_looping() {
    let workspace = TempDir::new().unwrap();
    let store = open_store(workspace.path(), 1024);
    let staged = store
        .stage_bytes("directory.bin", b"directory")
        .await
        .unwrap();
    let hash = staged.sha256().to_string();
    let object = store.object_path(&hash).unwrap();
    fs::create_dir_all(&object).unwrap();

    tokio::time::timeout(
        Duration::from_secs(1),
        store.persist_staged(staged, AssetSource::LocalUpload),
    )
    .await
    .unwrap()
    .unwrap();
    assert_eq!(store.read(&hash).unwrap(), b"directory");
}

#[cfg(unix)]
#[tokio::test]
async fn staged_persistence_never_blocks_opening_a_fifo_at_the_object_path() {
    use std::os::unix::ffi::OsStrExt;

    let workspace = TempDir::new().unwrap();
    let store = open_store(workspace.path(), 1024);
    let staged = store.stage_bytes("fifo.bin", b"fifo-bytes").await.unwrap();
    let hash = staged.sha256().to_string();
    let object = store.object_path(&hash).unwrap();
    fs::create_dir_all(object.parent().unwrap()).unwrap();
    let path = std::ffi::CString::new(object.as_os_str().as_bytes()).unwrap();
    assert_eq!(unsafe { libc::mkfifo(path.as_ptr(), 0o600) }, 0);

    tokio::time::timeout(
        Duration::from_secs(1),
        store.persist_staged(staged, AssetSource::LocalUpload),
    )
    .await
    .unwrap()
    .unwrap();
    assert_eq!(store.read(&hash).unwrap(), b"fifo-bytes");
}

#[cfg(unix)]
#[tokio::test]
async fn staged_persistence_quarantines_an_object_symlink_without_touching_its_target() {
    use std::os::unix::fs::symlink;

    let workspace = TempDir::new().unwrap();
    let store = open_store(workspace.path(), 1024);
    let staged = store
        .stage_bytes("symlink.bin", b"symlink-bytes")
        .await
        .unwrap();
    let hash = staged.sha256().to_string();
    let object = store.object_path(&hash).unwrap();
    fs::create_dir_all(object.parent().unwrap()).unwrap();
    let outside = workspace.path().join("outside-object-target");
    fs::write(&outside, b"outside").unwrap();
    symlink(&outside, &object).unwrap();

    tokio::time::timeout(
        Duration::from_secs(1),
        store.persist_staged(staged, AssetSource::LocalUpload),
    )
    .await
    .unwrap()
    .unwrap();
    assert_eq!(fs::read(outside).unwrap(), b"outside");
    assert_eq!(store.read(&hash).unwrap(), b"symlink-bytes");
}

#[cfg(unix)]
#[tokio::test]
async fn verified_dedupe_restores_private_object_and_sidecar_modes() {
    use std::os::unix::fs::PermissionsExt;

    let workspace = TempDir::new().unwrap();
    let store = open_store(workspace.path(), 1024);
    let stored = store
        .put_bytes(b"mode-repair", AssetSource::LocalUpload)
        .unwrap();
    let object = store.object_path(&stored.sha256).unwrap();
    let sidecar = store.metadata_path(&stored.sha256).unwrap();
    fs::set_permissions(&object, fs::Permissions::from_mode(0o644)).unwrap();
    fs::set_permissions(&sidecar, fs::Permissions::from_mode(0o644)).unwrap();
    let staged = store.stage_bytes("mode.bin", b"mode-repair").await.unwrap();

    store
        .persist_staged(staged, AssetSource::LocalUpload)
        .await
        .unwrap();

    assert_eq!(
        fs::metadata(object).unwrap().permissions().mode() & 0o777,
        0o600
    );
    assert_eq!(
        fs::metadata(sidecar).unwrap().permissions().mode() & 0o777,
        0o600
    );
}

#[cfg(unix)]
#[tokio::test]
async fn dedupe_replaces_a_sidecar_symlink_without_touching_its_target() {
    use std::os::unix::fs::symlink;

    let workspace = TempDir::new().unwrap();
    let store = open_store(workspace.path(), 1024);
    let stored = store
        .put_bytes(b"sidecar-symlink", AssetSource::LocalUpload)
        .unwrap();
    let sidecar = store.metadata_path(&stored.sha256).unwrap();
    fs::remove_file(&sidecar).unwrap();
    let outside = workspace.path().join("outside-sidecar-target");
    fs::write(&outside, b"outside-sidecar").unwrap();
    symlink(&outside, &sidecar).unwrap();
    let staged = store
        .stage_bytes("sidecar.bin", b"sidecar-symlink")
        .await
        .unwrap();

    store
        .persist_staged(staged, AssetSource::LocalUpload)
        .await
        .unwrap();

    assert_eq!(fs::read(outside).unwrap(), b"outside-sidecar");
    assert!(fs::symlink_metadata(sidecar).unwrap().file_type().is_file());
}

#[tokio::test]
async fn same_process_hash_lock_blocks_until_the_holder_drops() {
    let workspace = TempDir::new().unwrap();
    let store = Arc::new(open_store(workspace.path(), 1024));
    let hash = sha256_hex(b"locked");
    let first = HashLock::acquire(&store, &hash).await.unwrap();
    let second_store = Arc::clone(&store);
    let second_hash = hash.clone();
    let attempts = store.hash_lock_attempts();
    let second = tokio::spawn(async move { HashLock::acquire(&second_store, &second_hash).await });

    wait_for_lock_attempts(&store, attempts + 1).await;
    assert!(!second.is_finished());
    drop(first);
    tokio::time::timeout(Duration::from_secs(2), second)
        .await
        .unwrap()
        .unwrap()
        .unwrap();
}

#[tokio::test]
async fn cancelled_hash_lock_waiter_cannot_become_a_zombie_holder() {
    let workspace = TempDir::new().unwrap();
    let store = Arc::new(open_store(workspace.path(), 1024));
    let hash = sha256_hex(b"cancel-lock");
    let holder = HashLock::acquire(&store, &hash).await.unwrap();
    let attempts = store.hash_lock_attempts();
    let waiter_store = Arc::clone(&store);
    let waiter_hash = hash.clone();
    let waiter = tokio::spawn(async move { HashLock::acquire(&waiter_store, &waiter_hash).await });
    wait_for_lock_attempts(&store, attempts + 1).await;

    waiter.abort();
    assert!(waiter.await.unwrap_err().is_cancelled());
    let stopped_at = store.hash_lock_attempts();
    tokio::time::sleep(Duration::from_millis(100)).await;
    assert_eq!(store.hash_lock_attempts(), stopped_at);
    drop(holder);

    tokio::time::timeout(Duration::from_secs(1), HashLock::acquire(&store, &hash))
        .await
        .unwrap()
        .unwrap();
}

#[tokio::test]
async fn hash_lock_is_released_when_a_task_returns_an_error() {
    let workspace = TempDir::new().unwrap();
    let store = open_store(workspace.path(), 1024);
    let hash = sha256_hex(b"release");
    let result: Result<(), AssetError> = async {
        let _lock = HashLock::acquire(&store, &hash).await?;
        Err(AssetError::Invalid("injected task failure".to_string()))
    }
    .await;
    assert!(result.is_err());

    tokio::time::timeout(Duration::from_secs(2), HashLock::acquire(&store, &hash))
        .await
        .unwrap()
        .unwrap();
}

#[tokio::test]
async fn simultaneous_processes_share_one_filesystem_hash_lock() {
    let workspace = TempDir::new().unwrap();
    let store = open_store(workspace.path(), 1024);
    let hash = sha256_hex(b"cross-process-lock");
    let marker = workspace.path().join("holder.marker");
    let mut holder = spawn_hash_lock_child(workspace.path(), &hash, "hold", Some(&marker));
    wait_for_marker(&mut holder, &marker);

    let contender = spawn_hash_lock_child(workspace.path(), &hash, "measure", None);
    let output = contender.wait_with_output().unwrap();
    assert!(
        output.status.success(),
        "contender failed: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    let stdout = String::from_utf8(output.stdout).unwrap();
    let blocked_ms = stdout
        .lines()
        .find_map(|line| line.strip_prefix("blocked_ms="))
        .unwrap_or_else(|| panic!("missing elapsed marker in {stdout:?}"))
        .parse::<u128>()
        .unwrap();
    assert!(
        blocked_ms >= 250,
        "contender only blocked for {blocked_ms}ms"
    );
    assert!(holder.wait().unwrap().success());
    drop(store);
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn direct_put_uses_the_same_cross_process_hash_lock() {
    let workspace = TempDir::new().unwrap();
    let store = Arc::new(open_store(workspace.path(), 1024));
    let bytes = b"direct-put-lock";
    let hash = sha256_hex(bytes);
    let marker = workspace.path().join("direct-put-holder.marker");
    let mut holder = spawn_hash_lock_child(workspace.path(), &hash, "hold", Some(&marker));
    wait_for_marker(&mut holder, &marker);
    let started = Instant::now();
    let put_store = Arc::clone(&store);
    let put =
        tokio::task::spawn_blocking(move || put_store.put_bytes(bytes, AssetSource::LocalUpload));

    let stored = put.await.unwrap().unwrap();

    assert!(started.elapsed() >= Duration::from_millis(250));
    assert_eq!(stored.sha256, hash);
    assert!(holder.wait().unwrap().success());
}

#[tokio::test]
async fn staged_and_lock_capabilities_become_stale_after_namespace_rebind() {
    let workspace = TempDir::new().unwrap();
    let old = open_store(workspace.path(), 1024);
    let staged = old.stage_bytes("old.txt", b"old").await.unwrap();
    let hash = staged.sha256().to_string();
    let staged_temp = owned_temp_files(&old).pop().unwrap();
    let staged_basename = staged_temp.file_name().unwrap().to_owned();
    let lock = HashLock::acquire(&old, &hash).await.unwrap();
    let current = AssetStore::open(workspace.path(), "local:new", limits(1024)).unwrap();
    let sentinel = current.root().join("tmp").join(staged_basename);
    fs::write(&sentinel, b"new-generation-sentinel").unwrap();
    let untouched_hash = sha256_hex(b"stale-lock-must-not-create");
    let current_lock_path = current.lock_path(&untouched_hash).unwrap();

    assert_eq!(
        lock.ensure_current().unwrap_err().error_code(),
        "asset_store_stale"
    );
    assert_eq!(
        HashLock::acquire(&old, &untouched_hash)
            .await
            .unwrap_err()
            .error_code(),
        "asset_store_stale"
    );
    assert!(!current_lock_path.exists());
    assert_eq!(
        old.persist_staged(staged, AssetSource::LocalUpload)
            .await
            .unwrap_err()
            .error_code(),
        "asset_store_stale"
    );
    assert_eq!(fs::read(&sentinel).unwrap(), b"new-generation-sentinel");
    assert_eq!(current.usage().unwrap(), AssetUsage::default());
    assert_eq!(current.reserved_bytes().unwrap(), 0);
}

#[tokio::test]
async fn unfinished_stager_drop_cannot_unlink_a_new_generation_sentinel() {
    let workspace = TempDir::new().unwrap();
    let old = Arc::new(open_store(workspace.path(), 1024));
    let old_for_stage = Arc::clone(&old);
    let staging = tokio::spawn(async move {
        let mut budget = RequestBudget::default();
        let chunks = stream::iter([Ok::<_, std::io::Error>(Bytes::from_static(b"partial"))])
            .chain(stream::pending());
        old_for_stage
            .stage_stream("pending.bin", chunks, &mut budget)
            .await
    });
    let old_temp = tokio::time::timeout(Duration::from_secs(2), async {
        loop {
            if let Some(path) = owned_temp_files(&old).pop() {
                break path;
            }
            tokio::task::yield_now().await;
        }
    })
    .await
    .unwrap();
    let basename = old_temp.file_name().unwrap().to_owned();
    let current = AssetStore::open(workspace.path(), "local:new-stager", limits(1024)).unwrap();
    let sentinel = current.root().join("tmp").join(basename);
    fs::write(&sentinel, b"new-stager-sentinel").unwrap();

    staging.abort();
    assert!(staging.await.unwrap_err().is_cancelled());
    assert_eq!(fs::read(&sentinel).unwrap(), b"new-stager-sentinel");
}

#[cfg(unix)]
#[tokio::test]
async fn hash_lock_rejects_symlinks_and_nonregular_paths() {
    use std::os::unix::fs::symlink;

    let workspace = TempDir::new().unwrap();
    let store = open_store(workspace.path(), 1024);
    let symlink_hash = sha256_hex(b"symlink-lock");
    let symlink_path = store.lock_path(&symlink_hash).unwrap();
    fs::create_dir_all(symlink_path.parent().unwrap()).unwrap();
    let outside = workspace.path().join("outside-lock");
    fs::write(&outside, b"outside").unwrap();
    symlink(&outside, &symlink_path).unwrap();
    let symlink_error = HashLock::acquire(&store, &symlink_hash).await.unwrap_err();
    assert_eq!(symlink_error.error_code(), "asset_store_failed");
    assert_eq!(
        symlink_error.status_code(),
        StatusCode::INTERNAL_SERVER_ERROR
    );
    assert_eq!(fs::read(&outside).unwrap(), b"outside");

    let directory_hash = sha256_hex(b"directory-lock");
    let directory_path = store.lock_path(&directory_hash).unwrap();
    fs::create_dir_all(&directory_path).unwrap();
    let directory_error = HashLock::acquire(&store, &directory_hash)
        .await
        .unwrap_err();
    assert_eq!(directory_error.error_code(), "asset_store_failed");
    assert_eq!(
        directory_error.status_code(),
        StatusCode::INTERNAL_SERVER_ERROR
    );
}

#[cfg(unix)]
#[tokio::test]
async fn hash_lock_files_and_shards_are_private() {
    use std::os::unix::fs::PermissionsExt;

    let workspace = TempDir::new().unwrap();
    let store = open_store(workspace.path(), 1024);
    let hash = sha256_hex(b"private-lock");
    let lock_path = store.lock_path(&hash).unwrap();
    HashLock::acquire(&store, &hash).await.unwrap();

    assert_eq!(
        fs::metadata(lock_path.parent().unwrap())
            .unwrap()
            .permissions()
            .mode()
            & 0o777,
        0o700
    );
    assert_eq!(
        fs::metadata(lock_path).unwrap().permissions().mode() & 0o777,
        0o600
    );
}

#[test]
#[ignore = "child-process helper"]
fn hash_lock_child() {
    let Ok(mode) = std::env::var("GITIM_ASSET_LOCK_CHILD") else {
        return;
    };
    let workspace = PathBuf::from(std::env::var_os("GITIM_ASSET_LOCK_WORKSPACE").unwrap());
    let hash = std::env::var("GITIM_ASSET_LOCK_HASH").unwrap();
    let store = open_store(&workspace, 1024);
    let runtime = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .unwrap();
    let started = Instant::now();
    let lock = runtime.block_on(HashLock::acquire(&store, &hash)).unwrap();
    match mode.as_str() {
        "hold" => {
            let marker = PathBuf::from(std::env::var_os("GITIM_ASSET_LOCK_MARKER").unwrap());
            fs::write(marker, b"locked").unwrap();
            std::thread::sleep(Duration::from_millis(600));
        }
        "measure" => {
            let mut stdout = std::io::stdout().lock();
            writeln!(&mut stdout, "blocked_ms={}", started.elapsed().as_millis()).unwrap();
        }
        other => panic!("unknown lock child mode: {other}"),
    }
    drop(lock);
}

#[test]
fn default_limits_are_bounded() {
    let defaults = AssetLimits::from_environment(|_| None);
    assert_eq!(defaults.workspace_quota_bytes, 20 * 1024 * 1024 * 1024);
    assert_eq!(defaults.min_free_bytes, 2 * 1024 * 1024 * 1024);
    assert_eq!(defaults.max_file_bytes, 50 * 1024 * 1024);
    assert_eq!(defaults.max_request_bytes, 200 * 1024 * 1024);
    assert_eq!(defaults.max_files, 10);
    assert_eq!(defaults.temp_ttl, Duration::from_secs(24 * 60 * 60));
    assert_eq!(defaults.upload_slots, 2);
    assert_eq!(defaults.peer_slots, 4);

    let invalid = AssetLimits::from_environment(|name| match name {
        "GITIM_ASSET_WORKSPACE_QUOTA_BYTES" => Some("0".to_string()),
        "GITIM_ASSET_MIN_FREE_BYTES" => Some("invalid".to_string()),
        _ => None,
    });
    assert_eq!(
        invalid.workspace_quota_bytes,
        defaults.workspace_quota_bytes
    );
    assert_eq!(invalid.min_free_bytes, defaults.min_free_bytes);
}

#[test]
fn hash_paths_reject_traversal_uppercase_and_wrong_length() {
    let workspace = TempDir::new().unwrap();
    let store = open_store(workspace.path(), 1024);
    for hash in [
        "../object",
        &"A".repeat(64),
        &"a".repeat(63),
        &"g".repeat(64),
    ] {
        assert!(matches!(
            store.object_path(hash),
            Err(AssetError::Invalid(_))
        ));
        assert!(matches!(
            store.metadata_path(hash),
            Err(AssetError::Invalid(_))
        ));
    }
}

#[test]
fn same_binding_reuses_objects_in_place() {
    let workspace = TempDir::new().unwrap();
    let first = open_store(workspace.path(), 1024);
    let stored = first.put_bytes(b"one", AssetSource::LocalUpload).unwrap();
    let root = first.root().to_path_buf();

    let second = open_store(workspace.path(), 1024);
    assert_eq!(second.root(), root);
    assert_eq!(
        second.usage().unwrap(),
        AssetUsage {
            bytes: 3,
            objects: 1
        }
    );
    assert_eq!(second.read(&stored.sha256).unwrap(), b"one");
    assert!(orphaned_asset_trees(workspace.path()).is_empty());
}

#[test]
fn binding_mismatch_quarantines_old_namespace() {
    let workspace = TempDir::new().unwrap();
    let first = open_store(workspace.path(), 1024);
    first.put_bytes(b"one", AssetSource::LocalUpload).unwrap();

    let second = AssetStore::open(
        workspace.path(),
        "github:github.com/acme/other",
        limits(1024),
    )
    .unwrap();
    assert_eq!(second.usage().unwrap(), AssetUsage::default());
    let quarantined = orphaned_asset_trees(workspace.path());
    assert_eq!(quarantined.len(), 1);
    assert!(quarantined[0].join("objects").exists());
}

#[test]
fn missing_invalid_and_partial_manifests_with_data_are_quarantined() {
    for replacement in [
        None,
        Some(b"{".as_slice()),
        Some(br#"{"schema_version":1}"#),
    ] {
        let workspace = TempDir::new().unwrap();
        let store = open_store(workspace.path(), 1024);
        store.put_bytes(b"kept", AssetSource::LocalUpload).unwrap();
        let manifest = store.root().join("store.json");
        match replacement {
            Some(bytes) => fs::write(manifest, bytes).unwrap(),
            None => fs::remove_file(manifest).unwrap(),
        }

        let reopened = open_store(workspace.path(), 1024);
        assert_eq!(reopened.usage().unwrap(), AssetUsage::default());
        assert_eq!(orphaned_asset_trees(workspace.path()).len(), 1);
    }
}

#[test]
fn empty_first_use_tree_initializes_without_quarantine() {
    let workspace = TempDir::new().unwrap();
    fs::create_dir_all(workspace.path().join(".gitim-runtime/assets/v1")).unwrap();
    let store = open_store(workspace.path(), 1024);
    assert_eq!(store.usage().unwrap(), AssetUsage::default());
    assert!(store.root().join("store.json").exists());
    assert!(orphaned_asset_trees(workspace.path()).is_empty());
}

#[test]
fn quarantine_names_are_unique() {
    let workspace = TempDir::new().unwrap();
    let first = open_store(workspace.path(), 1024);
    first.put_bytes(b"one", AssetSource::LocalUpload).unwrap();
    fs::write(first.root().join("store.json"), b"bad").unwrap();
    let second = open_store(workspace.path(), 1024);
    second.put_bytes(b"two", AssetSource::LocalUpload).unwrap();
    fs::write(second.root().join("store.json"), b"bad").unwrap();
    open_store(workspace.path(), 1024);

    let quarantined = orphaned_asset_trees(workspace.path());
    assert_eq!(quarantined.len(), 2);
    assert_ne!(quarantined[0].file_name(), quarantined[1].file_name());
}

#[test]
fn every_read_and_write_rejects_a_changed_binding_until_reopen() {
    let workspace = TempDir::new().unwrap();
    let store = open_store(workspace.path(), 1024);
    let stored = store
        .put_bytes(b"secret", AssetSource::LocalUpload)
        .unwrap();
    fs::write(
        store.root().join("store.json"),
        br#"{"schema_version":1,"namespace":"other"}"#,
    )
    .unwrap();

    assert!(matches!(
        store.read(&stored.sha256),
        Err(AssetError::StaleBinding)
    ));
    assert!(matches!(store.reserve(1), Err(AssetError::StaleBinding)));
    let reopened = open_store(workspace.path(), 1024);
    assert_eq!(reopened.usage().unwrap(), AssetUsage::default());
    assert_eq!(orphaned_asset_trees(workspace.path()).len(), 1);
    reopened
        .put_bytes(b"new", AssetSource::LocalUpload)
        .unwrap();
}

#[test]
fn inspection_uses_magic_and_requires_valid_bounded_dimensions() {
    for (bytes, mime) in [
        (PNG_1X1, "image/png"),
        (JPEG_1X1, "image/jpeg"),
        (GIF_1X1, "image/gif"),
        (WEBP_1X1, "image/webp"),
    ] {
        let inspected = inspect_bytes(bytes, "spoofed.html").unwrap();
        assert_eq!(inspected.media_type, mime);
        assert_eq!((inspected.width, inspected.height), (Some(1), Some(1)));
        assert!(inspected.inline_safe, "{mime}");
    }
    let avif = inspect_bytes(&valid_avif(1, 1), "spoofed.html").unwrap();
    assert_eq!(avif.media_type, "image/avif");
    assert_eq!((avif.width, avif.height), (Some(1), Some(1)));
    assert!(avif.inline_safe);

    for (bytes, mime) in [
        (
            b"<svg xmlns='http://www.w3.org/2000/svg'/>".as_slice(),
            "image/svg+xml",
        ),
        (b"<!doctype html><html></html>".as_slice(), "text/html"),
        (b"not an image".as_slice(), "application/octet-stream"),
        (b"".as_slice(), "application/octet-stream"),
    ] {
        let inspected = inspect_bytes(bytes, "fake.png").unwrap();
        assert_eq!(inspected.media_type, mime);
        assert_eq!((inspected.width, inspected.height), (None, None));
        assert!(!inspected.inline_safe);
    }

    let malformed = inspect_bytes(b"\x89PNG\r\n\x1a\n", "broken.png").unwrap();
    assert_eq!(malformed.media_type, "image/png");
    assert_eq!((malformed.width, malformed.height), (None, None));
    assert!(!malformed.inline_safe);
}

#[test]
fn inline_images_enforce_axis_and_pixel_limits_without_losing_hints() {
    for (width, height, inline_safe) in [
        (32_768, 1, true),
        (32_769, 1, false),
        (10_000, 10_000, true),
        (10_000, 10_001, false),
    ] {
        let inspected = inspect_bytes(&png_with_dimensions(width, height), "large.png").unwrap();
        assert_eq!(inspected.media_type, "image/png");
        assert_eq!(
            (inspected.width, inspected.height),
            (Some(width), Some(height))
        );
        assert_eq!(inspected.inline_safe, inline_safe, "{width}x{height}");
    }
}

#[test]
fn avif_dimensions_require_the_bmff_property_hierarchy() {
    let structured = inspect_bytes(&valid_avif(123, 45), "image.avif").unwrap();
    assert_eq!(structured.media_type, "image/avif");
    assert_eq!((structured.width, structured.height), (Some(123), Some(45)));
    assert!(structured.inline_safe);

    let decoy = inspect_bytes(&avif_with_decoy_ispe(123, 45), "image.avif").unwrap();
    assert_eq!(decoy.media_type, "image/avif");
    assert_eq!((decoy.width, decoy.height), (None, None));
    assert!(!decoy.inline_safe);
}

#[test]
fn dimension_conversion_rejects_values_outside_u32() {
    assert_eq!(
        checked_dimensions(1, u32::MAX as usize),
        Some((1, u32::MAX))
    );
    if usize::BITS > 32 {
        assert_eq!(checked_dimensions(u32::MAX as usize + 1, 1), None);
    }
    assert_eq!(checked_dimensions(0, 1), None);
}

#[test]
fn quota_accepts_exact_limit_rejects_overage_without_tree_changes() {
    let workspace = TempDir::new().unwrap();
    let store = open_store(workspace.path(), 6);
    store
        .put_bytes(b"abcdef", AssetSource::LocalUpload)
        .unwrap();
    assert_eq!(
        store.usage().unwrap(),
        AssetUsage {
            bytes: 6,
            objects: 1
        }
    );
    let before = fs::read_dir(store.root().join("objects/sha256"))
        .unwrap()
        .count();
    let error = store.put_bytes(b"g", AssetSource::LocalUpload).unwrap_err();
    assert!(matches!(
        error,
        AssetError::QuotaExceeded { used: 6, quota: 6 }
    ));
    assert_eq!(
        store.usage().unwrap(),
        AssetUsage {
            bytes: 6,
            objects: 1
        }
    );
    assert_eq!(
        fs::read_dir(store.root().join("objects/sha256"))
            .unwrap()
            .count(),
        before
    );
}

#[test]
fn dedupe_counts_once_across_local_and_replica_sources() {
    let workspace = TempDir::new().unwrap();
    let store = open_store(workspace.path(), 6);
    let local = store.put_bytes(b"abc", AssetSource::LocalUpload).unwrap();
    let replica = store
        .put_bytes(
            b"abc",
            AssetSource::FleetReplica {
                origin_runtime_id: RUNTIME_ID_1.to_string(),
            },
        )
        .unwrap();
    assert_eq!(local.sha256, replica.sha256);
    assert!(matches!(replica.source, AssetSource::LocalUpload));
    assert_eq!(
        store.usage().unwrap(),
        AssetUsage {
            bytes: 3,
            objects: 1
        }
    );
    assert!(matches!(
        store.put_bytes(b"defg", AssetSource::LocalUpload),
        Err(AssetError::QuotaExceeded { .. })
    ));
}

#[test]
fn object_only_fleet_dedupe_records_the_requested_origin_source() {
    let workspace = TempDir::new().unwrap();
    let store = open_store(workspace.path(), 1024);
    let hash = write_object_only(&store, PNG_1X1);
    let metadata = store
        .put_bytes(
            PNG_1X1,
            AssetSource::FleetReplica {
                origin_runtime_id: RUNTIME_ID_1.to_string(),
            },
        )
        .unwrap();
    assert_eq!(metadata.sha256, hash);
    assert!(matches!(
        metadata.source,
        AssetSource::FleetReplica { origin_runtime_id } if origin_runtime_id == RUNTIME_ID_1
    ));
}

#[test]
fn dedupe_force_hash_repairs_same_size_same_mtime_corruption() {
    let workspace = TempDir::new().unwrap();
    let store = open_store(workspace.path(), 1024);
    let stored = store.put_bytes(b"good", AssetSource::LocalUpload).unwrap();
    let object = store.object_path(&stored.sha256).unwrap();
    let modified = fs::metadata(&object).unwrap().modified().unwrap();
    fs::write(&object, b"evil").unwrap();
    OpenOptions::new()
        .write(true)
        .open(&object)
        .unwrap()
        .set_times(FileTimes::new().set_modified(modified))
        .unwrap();

    store.put_bytes(b"good", AssetSource::LocalUpload).unwrap();
    assert_eq!(store.read(&stored.sha256).unwrap(), b"good");
    assert_eq!(
        store.usage().unwrap(),
        AssetUsage {
            bytes: 4,
            objects: 1
        }
    );
}

#[test]
fn sidecar_write_failure_cannot_leave_an_untracked_quota_bypass() {
    let workspace = TempDir::new().unwrap();
    let store = open_store(workspace.path(), 6);
    let hash = sha256_hex(b"abc");
    let object = store.object_path(&hash).unwrap();
    store.inject_sidecar_write_failure_once();

    assert!(store.put_bytes(b"abc", AssetSource::LocalUpload).is_err());
    let expected_after_failure = if object.exists() {
        AssetUsage {
            bytes: 3,
            objects: 1,
        }
    } else {
        AssetUsage::default()
    };
    assert_eq!(store.usage().unwrap(), expected_after_failure);

    store.put_bytes(b"abc", AssetSource::LocalUpload).unwrap();
    assert_eq!(
        store.usage().unwrap(),
        AssetUsage {
            bytes: 3,
            objects: 1
        }
    );
    assert!(matches!(
        store.put_bytes(b"defg", AssetSource::LocalUpload),
        Err(AssetError::QuotaExceeded { .. })
    ));
}

#[test]
fn late_dedupe_sidecar_failure_registers_the_verified_object_once() {
    let workspace = TempDir::new().unwrap();
    let store = open_store(workspace.path(), 6);
    let hash = write_object_only(&store, b"abc");
    store.inject_sidecar_write_failure_once();

    assert!(store.put_bytes(b"abc", AssetSource::LocalUpload).is_err());
    assert_eq!(
        store.usage().unwrap(),
        AssetUsage {
            bytes: 3,
            objects: 1
        }
    );
    store.put_bytes(b"abc", AssetSource::LocalUpload).unwrap();
    assert_eq!(store.read(&hash).unwrap(), b"abc");
    assert_eq!(
        store.usage().unwrap(),
        AssetUsage {
            bytes: 3,
            objects: 1
        }
    );
    assert!(matches!(
        store.put_bytes(b"defg", AssetSource::LocalUpload),
        Err(AssetError::QuotaExceeded { .. })
    ));
}

#[test]
fn reservations_use_checked_aggregate_quota_and_release_on_drop() {
    let workspace = TempDir::new().unwrap();
    let store = open_store(workspace.path(), 10);
    let reservation = store.reserve(7).unwrap();
    assert_eq!(store.reserved_bytes().unwrap(), 7);
    assert!(matches!(
        store.reserve(4),
        Err(AssetError::QuotaExceeded { .. })
    ));
    drop(reservation);
    assert_eq!(store.reserved_bytes().unwrap(), 0);
    assert!(store.reserve(10).is_ok());

    let huge_workspace = TempDir::new().unwrap();
    let huge = open_store(huge_workspace.path(), u64::MAX);
    assert!(matches!(
        huge.reserve(u64::MAX),
        Err(AssetError::QuotaExceeded { .. })
    ));
}

#[test]
fn asset_service_shares_reservations_across_store_handles() {
    let workspace = TempDir::new().unwrap();
    let service = AssetService::new(limits(10));
    let first = service
        .open_store(workspace.path(), "github:github.com/acme/repo")
        .unwrap();
    let second = service
        .open_store(workspace.path(), "github:github.com/acme/repo")
        .unwrap();

    let reservation = first.reserve(7).unwrap();
    assert_eq!(second.reserved_bytes().unwrap(), 7);
    assert!(matches!(
        second.reserve(4),
        Err(AssetError::QuotaExceeded { .. })
    ));
    drop(reservation);
    assert_eq!(second.reserved_bytes().unwrap(), 0);
}

#[test]
fn direct_store_handles_share_one_quota_state() {
    let workspace = TempDir::new().unwrap();
    let first = open_store(workspace.path(), 10);
    let second = open_store(workspace.path(), 10);

    let reservation = first.reserve(7).unwrap();
    assert_eq!(second.reserved_bytes().unwrap(), 7);
    assert!(matches!(
        second.reserve(4),
        Err(AssetError::QuotaExceeded { .. })
    ));
    drop(reservation);
}

#[cfg(unix)]
#[test]
fn workspace_aliases_share_accounting_and_cached_usage() {
    use std::os::unix::fs::symlink;

    let parent = TempDir::new().unwrap();
    let workspace = parent.path().join("workspace");
    fs::create_dir(&workspace).unwrap();
    let alias = parent.path().join("workspace-alias");
    symlink(&workspace, &alias).unwrap();
    let service = AssetService::new(limits(10));
    let first = AssetStore::open(&workspace, "github:github.com/acme/repo", limits(10)).unwrap();
    assert_eq!(service.cached_usage(&alias), Some(AssetUsage::default()));
    let second = service
        .open_store(&alias, "github:github.com/acme/repo")
        .unwrap();

    let reservation = first.reserve(7).unwrap();
    assert_eq!(second.reserved_bytes().unwrap(), 7);
    assert!(matches!(
        second.reserve(4),
        Err(AssetError::QuotaExceeded { .. })
    ));
    assert_eq!(
        service.cached_usage(&alias),
        service.cached_usage(&workspace)
    );
    drop(reservation);
}

#[test]
fn repeated_open_uses_cached_initialization_until_explicit_recovery() {
    let workspace = TempDir::new().unwrap();
    let first = open_store(workspace.path(), 1024);
    let hash = write_object_only(&first, b"late-object");
    assert!(!first.metadata_path(&hash).unwrap().exists());

    let second = open_store(workspace.path(), 1024);
    assert_eq!(second.usage().unwrap(), AssetUsage::default());
    assert!(!second.metadata_path(&hash).unwrap().exists());

    first.recover().unwrap();
    assert_eq!(
        second.usage().unwrap(),
        AssetUsage {
            bytes: 11,
            objects: 1
        }
    );
}

#[test]
fn live_workspace_state_rejects_incompatible_limits() {
    let workspace = TempDir::new().unwrap();
    let _first = open_store(workspace.path(), 10);
    let error = match open_store_result(workspace.path(), 11) {
        Ok(_) => panic!("incompatible limits must fail"),
        Err(error) => error,
    };
    assert!(matches!(error, AssetError::Invalid(_)));
}

#[test]
fn concurrent_direct_puts_cannot_exceed_workspace_quota() {
    let workspace = TempDir::new().unwrap();
    let first = open_store(workspace.path(), 10);
    let second = open_store(workspace.path(), 10);
    let barrier = Arc::new(Barrier::new(3));
    let first_barrier = Arc::clone(&barrier);
    let first_thread = std::thread::spawn(move || {
        first_barrier.wait();
        first.put_bytes(b"aaaaaa", AssetSource::LocalUpload)
    });
    let second_barrier = Arc::clone(&barrier);
    let second_thread = std::thread::spawn(move || {
        second_barrier.wait();
        second.put_bytes(b"bbbbbb", AssetSource::LocalUpload)
    });
    barrier.wait();

    let results = [first_thread.join().unwrap(), second_thread.join().unwrap()];
    assert_eq!(results.iter().filter(|result| result.is_ok()).count(), 1);
    assert_eq!(results.iter().filter(|result| result.is_err()).count(), 1);
    let reopened = open_store(workspace.path(), 10);
    assert_eq!(
        reopened.usage().unwrap(),
        AssetUsage {
            bytes: 6,
            objects: 1
        }
    );
}

#[test]
fn concurrent_recovery_cannot_overwrite_a_committed_put() {
    let workspace = TempDir::new().unwrap();
    let put_store = open_store(workspace.path(), 10);
    let recover_store = open_store(workspace.path(), 10);
    let barrier = Arc::new(Barrier::new(3));
    let put_barrier = Arc::clone(&barrier);
    let put_thread = std::thread::spawn(move || {
        put_barrier.wait();
        put_store.put_bytes(b"new", AssetSource::LocalUpload)
    });
    let recover_barrier = Arc::clone(&barrier);
    let recover_thread = std::thread::spawn(move || {
        recover_barrier.wait();
        recover_store.recover()
    });
    barrier.wait();

    put_thread.join().unwrap().unwrap();
    recover_thread.join().unwrap().unwrap();
    let reopened = open_store(workspace.path(), 10);
    assert_eq!(
        reopened.usage().unwrap(),
        AssetUsage {
            bytes: 3,
            objects: 1
        }
    );
}

#[test]
fn successful_put_atomically_transfers_its_reservation_to_committed_usage() {
    let workspace = TempDir::new().unwrap();
    let store = open_store(workspace.path(), 10);
    store.put_bytes(b"abc", AssetSource::LocalUpload).unwrap();
    assert_eq!(store.reserved_bytes().unwrap(), 0);
    assert_eq!(
        store.usage().unwrap(),
        AssetUsage {
            bytes: 3,
            objects: 1
        }
    );
}

#[test]
fn namespace_rebinding_does_not_release_live_reservations() {
    let workspace = TempDir::new().unwrap();
    let store = open_store(workspace.path(), 10);
    let old_reservation = store.reserve(7).unwrap();
    let rebound = AssetStore::open(workspace.path(), "local:other", limits(10)).unwrap();
    assert_eq!(rebound.reserved_bytes().unwrap(), 0);
    let new_reservation = rebound.reserve(3).unwrap();
    drop(old_reservation);
    assert_eq!(rebound.reserved_bytes().unwrap(), 3);
    drop(new_reservation);
    assert_eq!(rebound.reserved_bytes().unwrap(), 0);
}

#[test]
fn old_binding_handles_cannot_reclaim_the_active_namespace() {
    let workspace = TempDir::new().unwrap();
    let old = open_store(workspace.path(), 1024);
    let old_object = old.put_bytes(b"old", AssetSource::LocalUpload).unwrap();
    let current =
        AssetStore::open(workspace.path(), "github:github.com/acme/new", limits(1024)).unwrap();
    let current_object = current
        .put_bytes(b"current", AssetSource::LocalUpload)
        .unwrap();

    let stale = match old.reserve(1) {
        Ok(_) => panic!("stale reservation must fail"),
        Err(error) => error,
    };
    assert_eq!(stale.error_code(), "asset_store_stale");
    assert_eq!(
        old.put_bytes(b"stale", AssetSource::LocalUpload)
            .unwrap_err()
            .error_code(),
        "asset_store_stale"
    );
    assert_eq!(
        old.read(&old_object.sha256).unwrap_err().error_code(),
        "asset_store_stale"
    );
    assert_eq!(
        old.inspect(&old_object.sha256).unwrap_err().error_code(),
        "asset_store_stale"
    );
    assert_eq!(
        old.object_path(&old_object.sha256)
            .unwrap_err()
            .error_code(),
        "asset_store_stale"
    );
    assert_eq!(
        old.metadata_path(&old_object.sha256)
            .unwrap_err()
            .error_code(),
        "asset_store_stale"
    );
    assert_eq!(
        old.lock_path(&old_object.sha256).unwrap_err().error_code(),
        "asset_store_stale"
    );
    assert_eq!(
        old.create_owned_temp().unwrap_err().error_code(),
        "asset_store_stale"
    );
    assert_eq!(old.recover().unwrap_err().error_code(), "asset_store_stale");
    assert_eq!(old.usage().unwrap_err().error_code(), "asset_store_stale");
    assert_eq!(
        old.reserved_bytes().unwrap_err().error_code(),
        "asset_store_stale"
    );

    assert_eq!(current.read(&current_object.sha256).unwrap(), b"current");
    assert_eq!(
        current.usage().unwrap(),
        AssetUsage {
            bytes: 7,
            objects: 1
        }
    );
    assert_eq!(orphaned_asset_trees(workspace.path()).len(), 1);
}

#[test]
fn old_generation_reservation_drop_does_not_touch_current_accounting() {
    let workspace = TempDir::new().unwrap();
    let old = open_store(workspace.path(), 10);
    let old_reservation = old.reserve(7).unwrap();
    let current = AssetStore::open(workspace.path(), "local:new", limits(10)).unwrap();
    let current_reservation = current.reserve(3).unwrap();

    drop(old_reservation);
    assert_eq!(current.reserved_bytes().unwrap(), 3);
    drop(current_reservation);
    assert_eq!(current.reserved_bytes().unwrap(), 0);
}

#[test]
fn free_space_reserve_can_reject_before_writing() {
    let workspace = TempDir::new().unwrap();
    let mut constrained = limits(1024);
    constrained.min_free_bytes = u64::MAX;
    let store = AssetStore::open(workspace.path(), "local:one", constrained).unwrap();
    let error = store.put_bytes(b"x", AssetSource::LocalUpload).unwrap_err();
    assert!(matches!(error, AssetError::QuotaExceeded { .. }));
    assert_eq!(store.usage().unwrap(), AssetUsage::default());
}

#[test]
fn recovery_reconstructs_object_only_and_invalid_sidecars() {
    let workspace = TempDir::new().unwrap();
    let store = open_store(workspace.path(), 1024);
    let object_only = write_object_only(&store, PNG_1X1);
    store.recover().unwrap();
    let metadata = store.inspect(&object_only).unwrap();
    assert_eq!(metadata.media_type, "image/png");
    assert_eq!(metadata.width, Some(1));
    assert!(store.metadata_path(&object_only).unwrap().exists());

    fs::write(store.metadata_path(&object_only).unwrap(), b"invalid").unwrap();
    store.recover().unwrap();
    let repaired: serde_json::Value =
        serde_json::from_slice(&fs::read(store.metadata_path(&object_only).unwrap()).unwrap())
            .unwrap();
    assert_eq!(repaired["schema_version"], 1);
    assert_eq!(repaired["sha256"], object_only);

    let mut semantically_invalid = repaired;
    semantically_invalid["width"] = 0.into();
    fs::write(
        store.metadata_path(&object_only).unwrap(),
        serde_json::to_vec(&semantically_invalid).unwrap(),
    )
    .unwrap();
    store.recover().unwrap();
    let repaired = store.inspect(&object_only).unwrap();
    assert_eq!((repaired.width, repaired.height), (Some(1), Some(1)));
}

#[test]
fn recovery_removes_sidecar_without_object() {
    let workspace = TempDir::new().unwrap();
    let store = open_store(workspace.path(), 1024);
    let stored = store.put_bytes(b"gone", AssetSource::LocalUpload).unwrap();
    fs::remove_file(store.object_path(&stored.sha256).unwrap()).unwrap();
    store.recover().unwrap();
    assert_eq!(store.usage().unwrap(), AssetUsage::default());
    assert!(!store.metadata_path(&stored.sha256).unwrap().exists());
}

#[test]
fn hot_read_rehashes_when_object_metadata_changes() {
    let workspace = TempDir::new().unwrap();
    let store = open_store(workspace.path(), 1024);
    let stored = store.put_bytes(b"good", AssetSource::LocalUpload).unwrap();
    let path = store.object_path(&stored.sha256).unwrap();
    let modified = fs::metadata(&path).unwrap().modified().unwrap();
    fs::write(&path, b"evil").unwrap();
    OpenOptions::new()
        .write(true)
        .open(&path)
        .unwrap()
        .set_times(FileTimes::new().set_modified(modified + Duration::from_secs(1)))
        .unwrap();

    assert!(matches!(
        store.read(&stored.sha256),
        Err(AssetError::LocalCorruption)
    ));
    assert!(!path.exists());
    assert_eq!(store.usage().unwrap(), AssetUsage::default());
}

#[test]
fn quarantine_recounts_usage_after_counted_object_length_changes() {
    for corrupt in [b"x".as_slice(), b"corrupt-and-longer".as_slice()] {
        let workspace = TempDir::new().unwrap();
        let store = open_store(workspace.path(), 1024);
        let stored = store.put_bytes(b"good", AssetSource::LocalUpload).unwrap();
        let path = store.object_path(&stored.sha256).unwrap();
        let modified = fs::metadata(&path).unwrap().modified().unwrap();
        fs::write(&path, corrupt).unwrap();
        OpenOptions::new()
            .write(true)
            .open(&path)
            .unwrap()
            .set_times(FileTimes::new().set_modified(modified + Duration::from_secs(1)))
            .unwrap();

        assert!(matches!(
            store.inspect(&stored.sha256),
            Err(AssetError::LocalCorruption)
        ));
        assert_eq!(store.usage().unwrap(), AssetUsage::default());
        assert!(!path.exists());
    }
}

#[test]
fn quarantine_recount_preserves_remaining_regular_object_usage() {
    let workspace = TempDir::new().unwrap();
    let store = open_store(workspace.path(), 1024);
    let kept = store.put_bytes(b"kept", AssetSource::LocalUpload).unwrap();
    let corrupt = store
        .put_bytes(b"doomed", AssetSource::LocalUpload)
        .unwrap();
    let corrupt_path = store.object_path(&corrupt.sha256).unwrap();
    let modified = fs::metadata(&corrupt_path).unwrap().modified().unwrap();
    fs::write(&corrupt_path, b"x").unwrap();
    OpenOptions::new()
        .write(true)
        .open(&corrupt_path)
        .unwrap()
        .set_times(FileTimes::new().set_modified(modified + Duration::from_secs(1)))
        .unwrap();

    assert!(matches!(
        store.read(&corrupt.sha256),
        Err(AssetError::LocalCorruption)
    ));
    assert_eq!(
        store.usage().unwrap(),
        AssetUsage {
            bytes: 4,
            objects: 1
        }
    );
    assert_eq!(store.read(&kept.sha256).unwrap(), b"kept");
}

#[test]
fn recovery_quarantines_corrupt_object_and_does_not_count_it() {
    let workspace = TempDir::new().unwrap();
    let store = open_store(workspace.path(), 1024);
    let expected_hash = sha256_hex(b"expected");
    let object = store.object_path(&expected_hash).unwrap();
    fs::create_dir_all(object.parent().unwrap()).unwrap();
    fs::write(&object, b"corrupt").unwrap();
    store.recover().unwrap();

    assert!(!object.exists());
    assert_eq!(store.usage().unwrap(), AssetUsage::default());
    assert!(matches!(
        store.read(&expected_hash),
        Err(AssetError::Missing)
    ));
}

#[test]
fn recovery_quarantines_objects_over_the_configured_file_limit() {
    let workspace = TempDir::new().unwrap();
    let mut constrained = limits(1024);
    constrained.max_file_bytes = 3;
    let store = AssetStore::open(workspace.path(), "local:bounded", constrained).unwrap();
    let hash = write_object_only(&store, b"four");

    store.recover().unwrap();
    assert_eq!(store.usage().unwrap(), AssetUsage::default());
    assert!(!store.object_path(&hash).unwrap().exists());
}

#[test]
fn oversized_local_objects_are_reported_as_corruption() {
    let workspace = TempDir::new().unwrap();
    let mut constrained = limits(1024);
    constrained.max_file_bytes = 3;
    let store = AssetStore::open(workspace.path(), "local:bounded-read", constrained).unwrap();
    let expected_hash = sha256_hex(b"abc");
    let object = store.object_path(&expected_hash).unwrap();
    fs::create_dir_all(object.parent().unwrap()).unwrap();
    fs::write(&object, b"oversized").unwrap();

    let error = store.inspect(&expected_hash).unwrap_err();
    assert!(matches!(error, AssetError::LocalCorruption));
    assert_eq!(error.status_code(), StatusCode::INTERNAL_SERVER_ERROR);
    assert!(!object.exists());
}

#[test]
fn put_replaces_an_oversized_object_at_the_expected_hash() {
    let workspace = TempDir::new().unwrap();
    let mut constrained = limits(1024);
    constrained.max_file_bytes = 3;
    let store = AssetStore::open(workspace.path(), "local:bounded-put", constrained).unwrap();
    let expected_hash = sha256_hex(b"abc");
    let object = store.object_path(&expected_hash).unwrap();
    fs::create_dir_all(object.parent().unwrap()).unwrap();
    fs::write(&object, b"oversized").unwrap();

    store.put_bytes(b"abc", AssetSource::LocalUpload).unwrap();
    assert_eq!(store.read(&expected_hash).unwrap(), b"abc");
    assert_eq!(
        store.usage().unwrap(),
        AssetUsage {
            bytes: 3,
            objects: 1
        }
    );
}

#[test]
fn sidecar_schema_records_source_without_filename() {
    let workspace = TempDir::new().unwrap();
    let store = open_store(workspace.path(), 1024);
    let stored = store
        .put_bytes(
            PNG_1X1,
            AssetSource::FleetReplica {
                origin_runtime_id: RUNTIME_ID_2.to_string(),
            },
        )
        .unwrap();
    let json: serde_json::Value =
        serde_json::from_slice(&fs::read(store.metadata_path(&stored.sha256).unwrap()).unwrap())
            .unwrap();
    assert_eq!(json["schema_version"], 1);
    assert_eq!(json["source"]["kind"], "fleet_replica");
    assert_eq!(json["source"]["origin_runtime_id"], RUNTIME_ID_2);
    assert!(json.get("filename").is_none());
}

#[test]
fn unknown_sidecar_fields_are_removed_by_atomic_rebuild() {
    let workspace = TempDir::new().unwrap();
    let store = open_store(workspace.path(), 1024);
    let stored = store
        .put_bytes(
            PNG_1X1,
            AssetSource::FleetReplica {
                origin_runtime_id: RUNTIME_ID_1.to_string(),
            },
        )
        .unwrap();
    let mut sidecar = read_sidecar_json(&store, &stored.sha256);
    sidecar["filename"] = "must-not-survive.png".into();
    write_sidecar_json(&store, &stored.sha256, &sidecar);

    store.recover().unwrap();
    let rebuilt = read_sidecar_json(&store, &stored.sha256);
    assert!(rebuilt.get("filename").is_none());
    assert_eq!(rebuilt["source"]["kind"], "fleet_replica");
    assert_eq!(rebuilt["source"]["origin_runtime_id"], RUNTIME_ID_1);

    let second_workspace = TempDir::new().unwrap();
    let second = open_store(second_workspace.path(), 1024);
    let stored = second.put_bytes(PNG_1X1, AssetSource::LocalUpload).unwrap();
    let mut sidecar = read_sidecar_json(&second, &stored.sha256);
    sidecar["source"]["unexpected"] = true.into();
    write_sidecar_json(&second, &stored.sha256, &sidecar);
    second.recover().unwrap();
    let rebuilt = read_sidecar_json(&second, &stored.sha256);
    assert!(rebuilt["source"].get("unexpected").is_none());
}

#[test]
fn put_rejects_noncanonical_replica_runtime_ids_even_on_dedupe() {
    let workspace = TempDir::new().unwrap();
    let store = open_store(workspace.path(), 1024);
    store.put_bytes(b"same", AssetSource::LocalUpload).unwrap();

    for invalid in [
        "",
        "not-a-uuid",
        "550E8400-E29B-41D4-A716-446655440000",
        "550e8400e29b41d4a716446655440000",
    ] {
        assert!(matches!(
            store.put_bytes(
                b"same",
                AssetSource::FleetReplica {
                    origin_runtime_id: invalid.to_string(),
                },
            ),
            Err(AssetError::Invalid(_))
        ));
    }
}

#[test]
fn invalid_replica_sources_rebuild_once_as_local_upload() {
    for invalid in ["", "not-a-uuid", "550E8400-E29B-41D4-A716-446655440000"] {
        let workspace = TempDir::new().unwrap();
        let store = open_store(workspace.path(), 1024);
        let stored = store.put_bytes(PNG_1X1, AssetSource::LocalUpload).unwrap();
        let mut sidecar = read_sidecar_json(&store, &stored.sha256);
        sidecar["source"] = serde_json::json!({
            "kind": "fleet_replica",
            "origin_runtime_id": invalid,
        });
        write_sidecar_json(&store, &stored.sha256, &sidecar);

        store.recover().unwrap();
        let first_rebuild = fs::read(store.metadata_path(&stored.sha256).unwrap()).unwrap();
        assert!(matches!(
            store.inspect(&stored.sha256).unwrap().source,
            AssetSource::LocalUpload
        ));
        store.recover().unwrap();
        let second_recovery = fs::read(store.metadata_path(&stored.sha256).unwrap()).unwrap();
        assert_eq!(second_recovery, first_rebuild);
    }
}

#[test]
fn sidecar_mime_and_dimension_fields_require_canonical_semantics() {
    assert_sidecar_mutation_is_rebuilt(|sidecar| {
        sidecar["media_type"] = "IMAGE/PNG".into();
    });
    assert_sidecar_mutation_is_rebuilt(|sidecar| {
        sidecar["media_type"] = "image/png; charset=utf-8".into();
    });
    assert_sidecar_mutation_is_rebuilt(|sidecar| {
        sidecar["media_type"] = format!("application/{}", "a".repeat(116)).into();
    });
    assert_sidecar_mutation_is_rebuilt(|sidecar| {
        sidecar["height"] = serde_json::Value::Null;
    });
    assert_sidecar_mutation_is_rebuilt(|sidecar| {
        sidecar["media_type"] = "text/plain".into();
    });
    for wildcard in ["*/*", "image/*", "*/png"] {
        assert_sidecar_mutation_is_rebuilt(|sidecar| {
            sidecar["media_type"] = wildcard.into();
            sidecar["width"] = serde_json::Value::Null;
            sidecar["height"] = serde_json::Value::Null;
        });
    }
}

#[test]
fn hot_path_trusts_schema_valid_sidecar_with_matching_length_and_mtime() {
    let workspace = TempDir::new().unwrap();
    let store = open_store(workspace.path(), 1024);
    let stored = store.put_bytes(PNG_1X1, AssetSource::LocalUpload).unwrap();
    let mut sidecar = read_sidecar_json(&store, &stored.sha256);
    sidecar["media_type"] = "image/jpeg".into();
    sidecar["width"] = serde_json::Value::Null;
    sidecar["height"] = serde_json::Value::Null;
    write_sidecar_json(&store, &stored.sha256, &sidecar);
    let before = fs::read(store.metadata_path(&stored.sha256).unwrap()).unwrap();

    let trusted = store.inspect(&stored.sha256).unwrap();
    assert_eq!(trusted.media_type, "image/jpeg");
    assert_eq!((trusted.width, trusted.height), (None, None));
    assert_eq!(
        fs::read(store.metadata_path(&stored.sha256).unwrap()).unwrap(),
        before
    );
}

#[test]
fn temp_cleanup_removes_only_stale_owned_files() {
    let workspace = TempDir::new().unwrap();
    let store = open_store(workspace.path(), 1024);
    let recent = store.create_owned_temp().unwrap();
    let stale = store.create_owned_temp().unwrap();
    let unrelated = store.root().join("tmp/other-client.tmp");
    fs::write(&unrelated, b"keep").unwrap();
    let old = SystemTime::now() - Duration::from_secs(25 * 60 * 60);
    for path in [&stale, &unrelated] {
        OpenOptions::new()
            .write(true)
            .open(path)
            .unwrap()
            .set_times(FileTimes::new().set_modified(old))
            .unwrap();
    }

    store.recover().unwrap();
    assert!(recent.exists());
    assert!(!stale.exists());
    assert!(unrelated.exists());
}

#[test]
fn recovery_removes_only_stale_owned_atomic_write_temps() {
    let workspace = TempDir::new().unwrap();
    let store = open_store(workspace.path(), 1024);
    let hash = "a".repeat(64);
    let object_shard = store
        .object_path(&hash)
        .unwrap()
        .parent()
        .unwrap()
        .to_path_buf();
    let metadata_shard = store
        .metadata_path(&hash)
        .unwrap()
        .parent()
        .unwrap()
        .to_path_buf();
    fs::create_dir_all(&object_shard).unwrap();
    fs::create_dir_all(&metadata_shard).unwrap();
    let old_root_temp = store.root().join("gitim-atomic-root.tmp");
    let old_object_temp = object_shard.join("gitim-atomic-old.tmp");
    let old_metadata_temp = metadata_shard.join("gitim-atomic-old.tmp");
    let recent_owned = object_shard.join("gitim-atomic-recent.tmp");
    let unrelated = metadata_shard.join("other-client.tmp");
    for path in [
        &old_root_temp,
        &old_object_temp,
        &old_metadata_temp,
        &recent_owned,
        &unrelated,
    ] {
        fs::write(path, b"temporary").unwrap();
    }
    let old = SystemTime::now() - Duration::from_secs(25 * 60 * 60);
    for path in [&old_root_temp, &old_object_temp, &old_metadata_temp] {
        OpenOptions::new()
            .write(true)
            .open(path)
            .unwrap()
            .set_times(FileTimes::new().set_modified(old))
            .unwrap();
    }

    store.recover().unwrap();
    assert!(!old_root_temp.exists());
    assert!(!old_object_temp.exists());
    assert!(!old_metadata_temp.exists());
    assert!(recent_owned.exists());
    assert!(unrelated.exists());
    assert_eq!(store.usage().unwrap(), AssetUsage::default());
}

#[cfg(unix)]
#[test]
fn store_files_and_directories_have_private_modes() {
    use std::os::unix::fs::PermissionsExt;

    let workspace = TempDir::new().unwrap();
    let store = open_store(workspace.path(), 1024);
    let stored = store
        .put_bytes(b"private", AssetSource::LocalUpload)
        .unwrap();
    let temp = store.create_owned_temp().unwrap();
    for directory in [
        store.root().parent().unwrap().to_path_buf(),
        store.root().to_path_buf(),
        store.root().join("objects"),
        store
            .object_path(&stored.sha256)
            .unwrap()
            .parent()
            .unwrap()
            .to_path_buf(),
        store.root().join("tmp"),
    ] {
        assert_eq!(
            fs::metadata(directory).unwrap().permissions().mode() & 0o777,
            0o700
        );
    }
    for file in [
        store.root().join("store.json"),
        store.object_path(&stored.sha256).unwrap(),
        store.metadata_path(&stored.sha256).unwrap(),
        temp,
    ] {
        assert_eq!(
            fs::metadata(file).unwrap().permissions().mode() & 0o777,
            0o600
        );
    }
}

#[cfg(unix)]
#[test]
fn recovery_restores_private_modes_for_existing_store_files() {
    use std::os::unix::fs::PermissionsExt;

    let workspace = TempDir::new().unwrap();
    let store = open_store(workspace.path(), 1024);
    let stored = store
        .put_bytes(b"private", AssetSource::LocalUpload)
        .unwrap();
    let temp = store.create_owned_temp().unwrap();
    let object = store.object_path(&stored.sha256).unwrap();
    let sidecar = store.metadata_path(&stored.sha256).unwrap();
    let object_shard = object.parent().unwrap().to_path_buf();

    for directory in [store.root().to_path_buf(), object_shard.clone()] {
        fs::set_permissions(directory, fs::Permissions::from_mode(0o755)).unwrap();
    }
    for file in [
        store.root().join("store.json"),
        object.clone(),
        sidecar.clone(),
        temp.clone(),
    ] {
        fs::set_permissions(file, fs::Permissions::from_mode(0o400)).unwrap();
    }

    store.recover().unwrap();
    let reopened = store;
    assert_eq!(
        fs::metadata(reopened.root()).unwrap().permissions().mode() & 0o777,
        0o700
    );
    assert_eq!(
        fs::metadata(object_shard).unwrap().permissions().mode() & 0o777,
        0o700
    );
    for file in [reopened.root().join("store.json"), object, sidecar, temp] {
        assert_eq!(
            fs::metadata(file).unwrap().permissions().mode() & 0o777,
            0o600
        );
    }
}

#[cfg(unix)]
#[test]
fn object_symlinks_are_not_followed() {
    use std::os::unix::fs::symlink;

    let workspace = TempDir::new().unwrap();
    let store = open_store(workspace.path(), 1024);
    let outside = workspace.path().join("outside-secret");
    fs::write(&outside, b"secret").unwrap();
    let hash = sha256_hex(b"secret");
    let object = store.object_path(&hash).unwrap();
    fs::create_dir_all(object.parent().unwrap()).unwrap();
    symlink(&outside, &object).unwrap();

    store.recover().unwrap();
    assert_eq!(store.usage().unwrap(), AssetUsage::default());
    assert!(outside.exists());
    assert!(matches!(store.read(&hash), Err(AssetError::Missing)));
}

#[cfg(unix)]
#[test]
fn store_layout_does_not_follow_internal_directory_symlinks() {
    use std::os::unix::fs::symlink;

    let workspace = TempDir::new().unwrap();
    let store = open_store(workspace.path(), 1024);
    fs::remove_dir_all(store.root().join("objects")).unwrap();
    let outside = workspace.path().join("outside-directory");
    fs::create_dir(&outside).unwrap();
    symlink(&outside, store.root().join("objects")).unwrap();

    assert!(matches!(
        open_store_result(workspace.path(), 1024),
        Err(AssetError::Store(_))
    ));
    assert!(fs::read_dir(outside).unwrap().next().is_none());
}

#[cfg(unix)]
#[test]
fn open_store_rejects_internal_directory_symlinks_added_after_open() {
    use std::os::unix::fs::symlink;

    let workspace = TempDir::new().unwrap();
    let store = open_store(workspace.path(), 1024);
    fs::remove_dir_all(store.root().join("objects")).unwrap();

    let bytes = b"outside-object";
    let hash = sha256_hex(bytes);
    let outside = workspace.path().join("outside-objects");
    let outside_object = outside.join("sha256").join(&hash[..2]).join(&hash);
    fs::create_dir_all(outside_object.parent().unwrap()).unwrap();
    fs::write(&outside_object, bytes).unwrap();
    symlink(&outside, store.root().join("objects")).unwrap();

    assert!(matches!(store.read(&hash), Err(AssetError::Store(_))));
    assert_eq!(fs::read(&outside_object).unwrap(), bytes);
}

#[cfg(unix)]
#[test]
fn open_store_rejects_object_shard_symlinks_added_after_open() {
    use std::os::unix::fs::symlink;

    let workspace = TempDir::new().unwrap();
    let store = open_store(workspace.path(), 1024);
    let bytes = b"outside-shard-object";
    let hash = sha256_hex(bytes);
    let outside = workspace.path().join("outside-shard");
    fs::create_dir(&outside).unwrap();
    fs::write(outside.join(&hash), bytes).unwrap();
    symlink(
        &outside,
        store.root().join("objects/sha256").join(&hash[..2]),
    )
    .unwrap();

    assert!(matches!(store.read(&hash), Err(AssetError::Store(_))));
    assert_eq!(fs::read(outside.join(&hash)).unwrap(), bytes);
}

#[test]
fn asset_service_owns_limits_slots_usage_cache_and_metrics() {
    let workspace = TempDir::new().unwrap();
    let service = AssetService::new(limits(1024));
    assert_eq!(service.available_upload_permits(), 2);
    assert_eq!(service.available_peer_permits(), 4);
    let store = service
        .open_store(workspace.path(), "github:github.com/acme/repo")
        .unwrap();
    store
        .put_bytes(b"cached", AssetSource::LocalUpload)
        .unwrap();
    assert_eq!(
        service.cached_usage(workspace.path()),
        Some(AssetUsage {
            bytes: 6,
            objects: 1
        })
    );
    service.store_failures.fetch_add(1, Ordering::Relaxed);
    service.hash_mismatches.fetch_add(2, Ordering::Relaxed);
    service.fleet_fetch_failures.fetch_add(3, Ordering::Relaxed);
    assert_eq!(service.store_failures.load(Ordering::Relaxed), 1);
    assert_eq!(service.hash_mismatches.load(Ordering::Relaxed), 2);
    assert_eq!(service.fleet_fetch_failures.load(Ordering::Relaxed), 3);
}

#[test]
fn asset_service_eviction_releases_workspace_state_for_disk_reinitialization() {
    let workspace = TempDir::new().unwrap();
    let service = AssetService::new(limits(1024));
    let store = service
        .open_store(workspace.path(), "github:github.com/acme/repo")
        .unwrap();
    let stored = store
        .put_bytes(b"cached", AssetSource::LocalUpload)
        .unwrap();
    let metadata_path = store.metadata_path(&stored.sha256).unwrap();
    drop(store);

    assert!(service.evict_workspace(workspace.path()).unwrap());
    assert_eq!(service.cached_usage(workspace.path()), None);
    fs::remove_file(&metadata_path).unwrap();

    let reopened = service
        .open_store(workspace.path(), "github:github.com/acme/repo")
        .unwrap();
    assert!(metadata_path.exists());
    assert_eq!(
        reopened.usage().unwrap(),
        AssetUsage {
            bytes: 6,
            objects: 1,
        }
    );
}

#[test]
fn cached_usage_rejects_a_service_entry_staled_by_direct_rebind() {
    let workspace = TempDir::new().unwrap();
    let service = AssetService::new(limits(1024));
    let old = service
        .open_store(workspace.path(), "github:github.com/acme/old")
        .unwrap();
    old.put_bytes(b"old", AssetSource::LocalUpload).unwrap();

    let current = AssetStore::open(
        workspace.path(),
        "github:github.com/acme/current",
        limits(1024),
    )
    .unwrap();
    current
        .put_bytes(b"current", AssetSource::LocalUpload)
        .unwrap();

    assert_eq!(service.cached_usage(workspace.path()), None);
}

#[test]
fn runtime_state_default_has_one_asset_service() {
    let state = gitim_runtime::http::RuntimeState::default();
    assert_eq!(state.assets.available_upload_permits(), 2);
    assert_eq!(state.assets.available_peer_permits(), 4);
}

#[tokio::test]
async fn asset_service_exposes_owned_permits_without_mutating_semaphore_capacity() {
    let service = AssetService::new(limits(1024));
    let upload = service.acquire_upload().await.unwrap();
    let peer = service.acquire_peer().await.unwrap();
    assert_eq!(service.available_upload_permits(), 1);
    assert_eq!(service.available_peer_permits(), 3);
    drop(upload);
    drop(peer);
    assert_eq!(service.available_upload_permits(), 2);
    assert_eq!(service.available_peer_permits(), 4);
}

#[test]
fn asset_error_status_and_code_table_is_stable() {
    let cases = [
        (
            AssetError::Invalid("bad".into()),
            StatusCode::BAD_REQUEST,
            "invalid_asset",
        ),
        (
            AssetError::TooLarge { limit: 1 },
            StatusCode::PAYLOAD_TOO_LARGE,
            "asset_too_large",
        ),
        (
            AssetError::RequestTooLarge { limit: 1 },
            StatusCode::PAYLOAD_TOO_LARGE,
            "asset_request_too_large",
        ),
        (
            AssetError::TooMany { limit: 10 },
            StatusCode::UNPROCESSABLE_ENTITY,
            "too_many_assets",
        ),
        (
            AssetError::QuotaExceeded { used: 1, quota: 1 },
            StatusCode::INSUFFICIENT_STORAGE,
            "asset_quota_exceeded",
        ),
        (
            AssetError::Store(std::io::Error::other("disk")),
            StatusCode::INTERNAL_SERVER_ERROR,
            "asset_store_failed",
        ),
        (
            AssetError::StaleBinding,
            StatusCode::CONFLICT,
            "asset_store_stale",
        ),
        (
            AssetError::Invariant("accounting"),
            StatusCode::INTERNAL_SERVER_ERROR,
            "asset_store_failed",
        ),
        (AssetError::Missing, StatusCode::NOT_FOUND, "asset_missing"),
        (
            AssetError::LocalCorruption,
            StatusCode::INTERNAL_SERVER_ERROR,
            "asset_local_corruption",
        ),
        (
            AssetError::OriginUnavailable,
            StatusCode::SERVICE_UNAVAILABLE,
            "asset_origin_unavailable",
        ),
        (
            AssetError::HashMismatch,
            StatusCode::BAD_GATEWAY,
            "asset_hash_mismatch",
        ),
        (
            AssetError::PeerInvalid("bad".into()),
            StatusCode::BAD_GATEWAY,
            "asset_peer_invalid",
        ),
        (
            AssetError::ForbiddenOrigin,
            StatusCode::FORBIDDEN,
            "asset_origin_forbidden",
        ),
    ];
    for (error, status, code) in cases {
        assert_eq!(error.status_code(), status);
        assert_eq!(error.error_code(), code);
    }

    let internal = AssetError::Store(std::io::Error::other("/private/workspace/secret"));
    assert_eq!(internal.to_string(), "asset store operation failed");
    assert!(!internal.to_string().contains("/private"));
}
