//! Typed schema for `<repo>/gitim.epoch.yaml`.
//!
//! Phase A is read-only: daemon and runtime load this file to detect whether
//! the current branch is the active epoch or has been redirected to a newer
//! one. Phase B's pack coordinator will write these files; the `Serialize`
//! derives are kept so the writer path is a single call away.
//!
//! Two on-disk shapes share one struct (the design's "元数据文件" section):
//!
//! * `status: active` carries `snapshot` (the orphan commit on the new epoch
//!   branch points at the source commit + tree it copied from).
//! * `status: redirected` carries `redirect` (the redirect commit on the old
//!   branch points at the target epoch / branch / snapshot).
//!
//! Both states carry `archive` because the archive tag + bundle exist
//! regardless of which side of the boundary you read.
//!
//! Cross-field validation runs at load time so callers see a clean
//! `EpochError::Missing*` rather than discovering the mismatch later by
//! `unwrap()`-ing the wrong arm.

use std::fs::File;
use std::io;
use std::path::Path;

use serde::{Deserialize, Serialize};
use thiserror::Error;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum EpochStatus {
    Active,
    Redirected,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct SnapshotInfo {
    /// The branch the orphan snapshot was taken from (typically the previous
    /// epoch's `main` branch).
    pub source_branch: String,
    /// The commit on `source_branch` whose tree was copied into the new
    /// orphan snapshot commit (the "切换点 C" in the design diagram).
    pub source_commit: String,
    /// The orphan snapshot commit itself (the "S" in the design diagram).
    pub commit: String,
    /// ISO-8601 timestamp of pack creation.
    pub created_at: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct RedirectInfo {
    /// Numeric id of the target epoch (e.g. `2` for `main-epoch-2`).
    pub target_epoch: u32,
    /// Branch name agents should fetch + check out next.
    pub target_branch: String,
    /// The snapshot commit (S) on `target_branch`. Agents replay onto this.
    pub target_commit: String,
    /// The source commit (C) the snapshot was taken from. Replay uses
    /// `merge-base..HEAD` against this point.
    pub snapshot_of: String,
    /// ISO-8601 timestamp the redirect was published.
    pub created_at: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ArchiveInfo {
    /// Tag name pointing at the old epoch's source commit, of the form
    /// `archive/epoch-<N>/<source-commit-short>`.
    pub tag: String,
    /// SHA256 of the `git bundle` produced at pack time, for integrity check
    /// when restoring from the bundle.
    pub bundle_sha256: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct EpochFile {
    /// On-disk schema version. Only `1` is recognized; anything else is a
    /// hard error so old daemons don't silently misinterpret future shapes.
    pub schema_version: u32,
    pub status: EpochStatus,
    /// Numeric id of this branch's epoch (e.g. `1` for `main` after a pack,
    /// `2` for `main-epoch-2`).
    pub epoch: u32,
    /// Branch name this file lives on.
    pub branch: String,
    /// Present iff `status == Active`. Cross-field validated at load time.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub snapshot: Option<SnapshotInfo>,
    /// Present iff `status == Redirected`. Cross-field validated at load time.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub redirect: Option<RedirectInfo>,
    /// Required in both states — the archive tag and bundle exist
    /// independently of which side of the boundary you read.
    pub archive: ArchiveInfo,
}

#[derive(Debug, Error)]
pub enum EpochError {
    #[error("unsupported epoch schema_version: {0}")]
    UnsupportedSchemaVersion(u32),
    #[error("status=active requires `snapshot` field")]
    MissingSnapshot,
    #[error("status=redirected requires `redirect` field")]
    MissingRedirect,
    #[error("failed to parse epoch yaml: {0}")]
    ParseError(#[from] serde_yaml::Error),
    #[error("failed to read epoch file: {0}")]
    IoError(#[from] io::Error),
}

impl EpochFile {
    /// Read + parse + validate a `gitim.epoch.yaml` from disk.
    ///
    /// On success the returned `EpochFile` has already passed `validate()`,
    /// so callers can match on `status` and unwrap the matching arm
    /// (`snapshot` for active, `redirect` for redirected) without further
    /// checks.
    pub fn load_from_path(p: &Path) -> Result<EpochFile, EpochError> {
        let f = File::open(p)?;
        let parsed: EpochFile = serde_yaml::from_reader(f)?;
        parsed.validate()?;
        Ok(parsed)
    }

    /// Enforce the invariants that aren't expressible in serde alone:
    ///
    /// * `schema_version == 1`
    /// * `status: active` ⇒ `snapshot.is_some()`
    /// * `status: redirected` ⇒ `redirect.is_some()`
    ///
    /// The opposite-arm-must-be-None side isn't enforced here — a future
    /// schema could legitimately carry both, and Phase A's read path doesn't
    /// depend on its absence.
    pub fn validate(&self) -> Result<(), EpochError> {
        if self.schema_version != 1 {
            return Err(EpochError::UnsupportedSchemaVersion(self.schema_version));
        }
        match self.status {
            EpochStatus::Active if self.snapshot.is_none() => Err(EpochError::MissingSnapshot),
            EpochStatus::Redirected if self.redirect.is_none() => Err(EpochError::MissingRedirect),
            _ => Ok(()),
        }
    }
}
