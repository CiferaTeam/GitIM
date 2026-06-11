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
    /// The archive tag + bundle that seal the old epoch's history. Optional
    /// because the pack coordinator builds an `EpochFile` *before* the orphan
    /// commit lands (so the bundle SHA isn't known yet) and patches the
    /// archive block in later. Fixture YAML written by humans / Phase A
    /// populates it; Task 5's `try_fire_rotation` leaves it `None` on first
    /// write.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub archive: Option<ArchiveInfo>,
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
    /// Returned by `save_to_path` when `serde_yaml::to_string` rejects the
    /// value (currently only reachable via deliberately corrupt struct
    /// fields — `validate()` runs first to keep the common case clean).
    #[error("serialize epoch yaml: {0}")]
    Serialize(String),
}

impl EpochFile {
    /// Read + parse + validate a `gitim.epoch.yaml` from disk.
    ///
    /// File-not-found is a normal "no epoch metadata yet" state (legacy repos
    /// and freshly-cloned pre-pack workspaces both look like this), so the
    /// missing-file case returns `Ok(None)` rather than an error. Any other
    /// IO failure, parse failure, or validate failure surfaces as `Err`.
    ///
    /// On `Ok(Some(file))` the file has already passed `validate()`, so
    /// callers can match on `status` and unwrap the matching arm (`snapshot`
    /// for active, `redirect` for redirected) without further checks.
    pub fn load_from_path(p: &Path) -> Result<Option<EpochFile>, EpochError> {
        let f = match File::open(p) {
            Ok(f) => f,
            Err(e) if e.kind() == io::ErrorKind::NotFound => return Ok(None),
            Err(e) => return Err(e.into()),
        };
        let parsed: EpochFile = serde_yaml::from_reader(f)?;
        parsed.validate()?;
        Ok(Some(parsed))
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

    /// Build an `Active`-state epoch file pointing at `branch` with the
    /// `source_*` fields describing the commit that was sealed as snapshot
    /// ancestor. `archive` is optional because the bundle SHA isn't known
    /// until after the orphan commit lands — Task 5's `try_fire_rotation`
    /// writes the YAML first with `None`, then patches the archive block in
    /// later phases if needed.
    #[allow(clippy::too_many_arguments)]
    pub fn new_active(
        epoch: u32,
        branch: String,
        source_branch: String,
        source_commit: String,
        commit: String,
        created_at: String,
        archive: Option<(String, String)>,
    ) -> Self {
        Self {
            schema_version: 1,
            status: EpochStatus::Active,
            epoch,
            branch,
            snapshot: Some(SnapshotInfo {
                source_branch,
                source_commit,
                commit,
                created_at,
            }),
            redirect: None,
            archive: archive.map(|(tag, sha)| ArchiveInfo {
                tag,
                bundle_sha256: sha,
            }),
        }
    }

    /// Build a `Redirected`-state epoch file on the sealed `branch` pointing
    /// at `target_branch` (the freshly-opened next epoch). See `new_active`
    /// re. `archive` being optional.
    #[allow(clippy::too_many_arguments)]
    pub fn new_redirect(
        epoch: u32,
        branch: String,
        target_epoch: u32,
        target_branch: String,
        target_commit: String,
        snapshot_of: String,
        created_at: String,
        archive: Option<(String, String)>,
    ) -> Self {
        Self {
            schema_version: 1,
            status: EpochStatus::Redirected,
            epoch,
            branch,
            snapshot: None,
            redirect: Some(RedirectInfo {
                target_epoch,
                target_branch,
                target_commit,
                snapshot_of,
                created_at,
            }),
            archive: archive.map(|(tag, sha)| ArchiveInfo {
                tag,
                bundle_sha256: sha,
            }),
        }
    }

    /// Atomically write this file to `path`. `validate()` runs first so a
    /// caller can never persist a malformed state. Implementation: write to
    /// `<path>.tmp` then `rename(.tmp, .)` so a reader process never observes
    /// a partial file across the rotation boundary — `rename` is atomic on
    /// the same filesystem on both Unix (`renameat`) and modern Windows.
    pub fn save_to_path(&self, path: &Path) -> Result<(), EpochError> {
        self.validate()?;
        let yaml = serde_yaml::to_string(self).map_err(|e| EpochError::Serialize(e.to_string()))?;
        let tmp = path.with_extension("yaml.tmp");
        std::fs::write(&tmp, yaml)?;
        std::fs::rename(&tmp, path)?;
        Ok(())
    }
}
