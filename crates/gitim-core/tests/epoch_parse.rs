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
    let epoch = EpochFile::load_from_path(f.path()).expect("load active epoch file");

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

    assert_eq!(epoch.archive.tag, "archive/epoch-1/aabbccdd");
    assert_eq!(
        epoch.archive.bundle_sha256,
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
    let epoch = EpochFile::load_from_path(f.path()).expect("load redirected epoch file");

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

    assert_eq!(epoch.archive.tag, "archive/epoch-1/aabbccdd");
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
