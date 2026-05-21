#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]
//! Parse + validate tests for `gitim.epoch.yaml`.
//!
//! Fixture YAML mirrors the design at `docs/plans/2026-05-06-git-history-snapshot-pack.md`
//! ("元数据文件" section). The active and redirected shapes are the wire contract
//! both the daemon (read-only in Phase A) and a future pack coordinator (writer
//! in Phase B) must agree on.

use std::io::Write;

use gitim_core::epoch::{EpochError, EpochFile, EpochStatus};
use tempfile::NamedTempFile;

fn write_yaml(contents: &str) -> NamedTempFile {
    let mut f = NamedTempFile::new().expect("create temp file");
    f.write_all(contents.as_bytes()).expect("write yaml");
    f.flush().expect("flush yaml");
    f
}

#[test]
fn active_round_trip() {
    let yaml = r#"
schema_version: 1
status: active
epoch: 2
branch: main-epoch-2
snapshot:
  source_branch: main
  source_commit: aabbccddeeff00112233445566778899aabbccdd
  commit: 1122334455667788990011223344556677889900
  created_at: "2026-05-06T00:00:00Z"
archive:
  tag: archive/epoch-1/aabbccdd
  bundle_sha256: 0123456789abcdef0123456789abcdef0123456789abcdef0123456789abcdef
"#;
    let f = write_yaml(yaml);
    let epoch = EpochFile::load_from_path(f.path())
        .expect("load active epoch file")
        .expect("active fixture is present on disk");

    assert_eq!(epoch.schema_version, 1);
    assert_eq!(epoch.status, EpochStatus::Active);
    assert_eq!(epoch.epoch, 2);
    assert_eq!(epoch.branch, "main-epoch-2");

    let snap = epoch.snapshot.as_ref().expect("active must carry snapshot");
    assert_eq!(snap.source_branch, "main");
    assert_eq!(
        snap.source_commit,
        "aabbccddeeff00112233445566778899aabbccdd"
    );
    assert_eq!(snap.commit, "1122334455667788990011223344556677889900");
    assert_eq!(snap.created_at, "2026-05-06T00:00:00Z");

    assert!(
        epoch.redirect.is_none(),
        "active epoch must not carry redirect"
    );

    let archive = epoch
        .archive
        .as_ref()
        .expect("fixture YAML carries archive");
    assert_eq!(archive.tag, "archive/epoch-1/aabbccdd");
    assert_eq!(
        archive.bundle_sha256,
        "0123456789abcdef0123456789abcdef0123456789abcdef0123456789abcdef"
    );
}

#[test]
fn redirected_round_trip() {
    let yaml = r#"
schema_version: 1
status: redirected
epoch: 1
branch: main
redirect:
  target_epoch: 2
  target_branch: main-epoch-2
  target_commit: 1122334455667788990011223344556677889900
  snapshot_of: aabbccddeeff00112233445566778899aabbccdd
  created_at: "2026-05-06T00:00:00Z"
archive:
  tag: archive/epoch-1/aabbccdd
  bundle_sha256: 0123456789abcdef0123456789abcdef0123456789abcdef0123456789abcdef
"#;
    let f = write_yaml(yaml);
    let epoch = EpochFile::load_from_path(f.path())
        .expect("load redirected epoch file")
        .expect("redirected fixture is present on disk");

    assert_eq!(epoch.schema_version, 1);
    assert_eq!(epoch.status, EpochStatus::Redirected);
    assert_eq!(epoch.epoch, 1);
    assert_eq!(epoch.branch, "main");

    let redir = epoch
        .redirect
        .as_ref()
        .expect("redirected must carry redirect");
    assert_eq!(redir.target_epoch, 2);
    assert_eq!(redir.target_branch, "main-epoch-2");
    assert_eq!(
        redir.target_commit,
        "1122334455667788990011223344556677889900"
    );
    assert_eq!(
        redir.snapshot_of,
        "aabbccddeeff00112233445566778899aabbccdd"
    );
    assert_eq!(redir.created_at, "2026-05-06T00:00:00Z");

    assert!(
        epoch.snapshot.is_none(),
        "redirected epoch must not carry snapshot"
    );

    let archive = epoch
        .archive
        .as_ref()
        .expect("fixture YAML carries archive");
    assert_eq!(archive.tag, "archive/epoch-1/aabbccdd");
}

#[test]
fn rejects_unsupported_schema_version_999() {
    let yaml = r#"
schema_version: 999
status: active
epoch: 2
branch: main-epoch-2
snapshot:
  source_branch: main
  source_commit: aabbccddeeff00112233445566778899aabbccdd
  commit: 1122334455667788990011223344556677889900
  created_at: "2026-05-06T00:00:00Z"
archive:
  tag: archive/epoch-1/aabbccdd
  bundle_sha256: 0123456789abcdef0123456789abcdef0123456789abcdef0123456789abcdef
"#;
    let f = write_yaml(yaml);
    let err = EpochFile::load_from_path(f.path()).expect_err("schema_version 999 must reject");
    assert!(
        matches!(err, EpochError::UnsupportedSchemaVersion(999)),
        "expected UnsupportedSchemaVersion(999), got {err:?}",
    );
}

#[test]
fn rejects_unsupported_schema_version_zero() {
    let yaml = r#"
schema_version: 0
status: active
epoch: 2
branch: main-epoch-2
snapshot:
  source_branch: main
  source_commit: aabbccddeeff00112233445566778899aabbccdd
  commit: 1122334455667788990011223344556677889900
  created_at: "2026-05-06T00:00:00Z"
archive:
  tag: archive/epoch-1/aabbccdd
  bundle_sha256: 0123456789abcdef0123456789abcdef0123456789abcdef0123456789abcdef
"#;
    let f = write_yaml(yaml);
    let err = EpochFile::load_from_path(f.path()).expect_err("schema_version 0 must reject");
    assert!(
        matches!(err, EpochError::UnsupportedSchemaVersion(0)),
        "expected UnsupportedSchemaVersion(0), got {err:?}",
    );
}

#[test]
fn rejects_active_without_snapshot() {
    let yaml = r#"
schema_version: 1
status: active
epoch: 2
branch: main-epoch-2
archive:
  tag: archive/epoch-1/aabbccdd
  bundle_sha256: 0123456789abcdef0123456789abcdef0123456789abcdef0123456789abcdef
"#;
    let f = write_yaml(yaml);
    let err = EpochFile::load_from_path(f.path()).expect_err("active without snapshot must reject");
    assert!(
        matches!(err, EpochError::MissingSnapshot),
        "expected MissingSnapshot, got {err:?}",
    );
}

#[test]
fn rejects_redirected_without_redirect() {
    let yaml = r#"
schema_version: 1
status: redirected
epoch: 1
branch: main
archive:
  tag: archive/epoch-1/aabbccdd
  bundle_sha256: 0123456789abcdef0123456789abcdef0123456789abcdef0123456789abcdef
"#;
    let f = write_yaml(yaml);
    let err =
        EpochFile::load_from_path(f.path()).expect_err("redirected without redirect must reject");
    assert!(
        matches!(err, EpochError::MissingRedirect),
        "expected MissingRedirect, got {err:?}",
    );
}

// --------------------------------------------------------------------------
// Phase B PR 3 / Task 1: constructors + atomic save_to_path.
//
// The constructors are the canonical way for the pack coordinator (and any
// future writer) to build a well-formed `EpochFile` — they fix the schema
// version, populate the matching `snapshot` / `redirect` arm for the chosen
// status, and accept the archive as an optional tuple so callers can defer
// bundle SHA computation until after the orphan commit lands. `save_to_path`
// must be atomic on the existing-target overwrite path so reader processes
// never observe a partial file mid-rotation.
// --------------------------------------------------------------------------

use std::path::PathBuf;
use tempfile::TempDir;

#[test]
fn new_active_constructs_valid_active_file() {
    let f = EpochFile::new_active(
        2,
        "main-epoch-2".to_string(),
        "main".to_string(),
        "aabbccddeeff00112233445566778899aabbccdd".to_string(),
        "1122334455667788990011223344556677889900".to_string(),
        "2026-05-21T00:00:00Z".to_string(),
        Some(("archive/epoch-1/aabbccdd".to_string(), "0".repeat(64))),
    );
    assert_eq!(f.status, EpochStatus::Active);
    assert_eq!(f.epoch, 2);
    assert_eq!(f.branch, "main-epoch-2");
    assert!(f.snapshot.is_some());
    assert!(f.redirect.is_none());
    f.validate().expect("constructed active should validate");
}

#[test]
fn new_redirect_constructs_valid_redirected_file() {
    let f = EpochFile::new_redirect(
        1,
        "main".to_string(),
        2,
        "main-epoch-2".to_string(),
        "1122334455667788990011223344556677889900".to_string(),
        "aabbccddeeff00112233445566778899aabbccdd".to_string(),
        "2026-05-21T00:00:00Z".to_string(),
        Some(("archive/epoch-1/aabbccdd".to_string(), "0".repeat(64))),
    );
    assert_eq!(f.status, EpochStatus::Redirected);
    assert_eq!(f.epoch, 1);
    assert_eq!(f.branch, "main");
    assert!(f.redirect.is_some());
    assert!(f.snapshot.is_none());
    f.validate().expect("constructed redirect should validate");
}

#[test]
fn save_to_path_round_trip() {
    let tmp = TempDir::new().unwrap();
    let path: PathBuf = tmp.path().join("gitim.epoch.yaml");

    let f = EpochFile::new_active(
        2,
        "main-epoch-2".to_string(),
        "main".to_string(),
        "a".repeat(40),
        "b".repeat(40),
        "2026-05-21T00:00:00Z".to_string(),
        None,
    );
    f.save_to_path(&path).expect("save");
    assert!(path.exists());

    let loaded = EpochFile::load_from_path(&path)
        .expect("load")
        .expect("present");
    assert_eq!(loaded.status, EpochStatus::Active);
    assert_eq!(loaded.epoch, 2);
    assert_eq!(loaded.branch, "main-epoch-2");
    assert!(loaded.archive.is_none());
}

#[test]
fn save_to_path_is_atomic_no_partial_on_existing() {
    // Existing valid file is preserved on overwrite (atomic rename).
    let tmp = TempDir::new().unwrap();
    let path = tmp.path().join("gitim.epoch.yaml");
    let f1 = EpochFile::new_active(
        1,
        "main".to_string(),
        "main".to_string(),
        "a".repeat(40),
        "b".repeat(40),
        "2026-05-21T00:00:00Z".to_string(),
        None,
    );
    f1.save_to_path(&path).unwrap();

    let f2 = EpochFile::new_redirect(
        1,
        "main".to_string(),
        2,
        "main-epoch-2".to_string(),
        "c".repeat(40),
        "d".repeat(40),
        "2026-05-21T01:00:00Z".to_string(),
        None,
    );
    f2.save_to_path(&path).unwrap();

    let loaded = EpochFile::load_from_path(&path).unwrap().unwrap();
    assert_eq!(loaded.status, EpochStatus::Redirected);
    let redirect = loaded.redirect.as_ref().unwrap();
    assert_eq!(redirect.target_branch, "main-epoch-2");
}
