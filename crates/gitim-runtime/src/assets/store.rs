use super::{inspect_bytes, AssetError};
use chrono::Utc;
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::collections::HashMap;
use std::fs::{self, File, OpenOptions};
use std::io::{Read, Write};
use std::path::{Path, PathBuf};
#[cfg(feature = "test-support")]
use std::sync::atomic::AtomicBool;
use std::sync::atomic::{AtomicU64, AtomicUsize, Ordering};
use std::sync::{
    Arc, Mutex, MutexGuard, OnceLock, RwLock, RwLockReadGuard, RwLockWriteGuard, Weak,
};
use std::time::{Duration, SystemTime, UNIX_EPOCH};
use tokio::sync::{OwnedSemaphorePermit, Semaphore};

const STORE_SCHEMA_VERSION: u32 = 1;
const SIDECAR_SCHEMA_VERSION: u32 = 1;
const DEFAULT_QUOTA_BYTES: u64 = 20 * 1024 * 1024 * 1024;
const DEFAULT_MIN_FREE_BYTES: u64 = 2 * 1024 * 1024 * 1024;
const DEFAULT_FILE_BYTES: u64 = 50 * 1024 * 1024;
const DEFAULT_REQUEST_BYTES: u64 = 200 * 1024 * 1024;
const DEFAULT_MAX_FILES: usize = 10;
const DEFAULT_TEMP_TTL: Duration = Duration::from_secs(24 * 60 * 60);
const DEFAULT_UPLOAD_SLOTS: usize = 2;
const DEFAULT_PEER_SLOTS: usize = 4;
static UNIQUE_SUFFIX: AtomicUsize = AtomicUsize::new(0);
static WORKSPACE_STATES: OnceLock<Mutex<HashMap<PathBuf, Weak<WorkspaceAssetState>>>> =
    OnceLock::new();

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AssetLimits {
    pub workspace_quota_bytes: u64,
    pub min_free_bytes: u64,
    pub max_file_bytes: u64,
    pub max_request_bytes: u64,
    pub max_files: usize,
    pub temp_ttl: Duration,
    pub upload_slots: usize,
    pub peer_slots: usize,
}

impl AssetLimits {
    pub fn from_environment(mut read: impl FnMut(&str) -> Option<String>) -> Self {
        Self {
            workspace_quota_bytes: positive_env(
                &mut read,
                "GITIM_ASSET_WORKSPACE_QUOTA_BYTES",
                DEFAULT_QUOTA_BYTES,
            ),
            min_free_bytes: positive_env(
                &mut read,
                "GITIM_ASSET_MIN_FREE_BYTES",
                DEFAULT_MIN_FREE_BYTES,
            ),
            max_file_bytes: DEFAULT_FILE_BYTES,
            max_request_bytes: DEFAULT_REQUEST_BYTES,
            max_files: DEFAULT_MAX_FILES,
            temp_ttl: DEFAULT_TEMP_TTL,
            upload_slots: DEFAULT_UPLOAD_SLOTS,
            peer_slots: DEFAULT_PEER_SLOTS,
        }
    }
}

impl Default for AssetLimits {
    fn default() -> Self {
        Self::from_environment(|name| std::env::var(name).ok())
    }
}

fn positive_env(read: &mut impl FnMut(&str) -> Option<String>, name: &str, fallback: u64) -> u64 {
    read(name)
        .and_then(|value| value.parse::<u64>().ok())
        .filter(|value| *value > 0)
        .unwrap_or(fallback)
}

#[derive(Debug, Default, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct AssetUsage {
    pub bytes: u64,
    pub objects: u64,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum AssetSource {
    LocalUpload,
    FleetReplica { origin_runtime_id: String },
}

impl<'de> Deserialize<'de> for AssetSource {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: serde::Deserializer<'de>,
    {
        #[derive(Deserialize)]
        #[serde(deny_unknown_fields)]
        struct SourceWire {
            kind: String,
            #[serde(default)]
            origin_runtime_id: Option<String>,
        }

        let source = SourceWire::deserialize(deserializer)?;
        match (source.kind.as_str(), source.origin_runtime_id) {
            ("local_upload", None) => Ok(Self::LocalUpload),
            ("fleet_replica", Some(origin_runtime_id)) => {
                Ok(Self::FleetReplica { origin_runtime_id })
            }
            ("local_upload", Some(_)) => Err(serde::de::Error::custom(
                "local_upload source cannot include origin_runtime_id",
            )),
            ("fleet_replica", None) => Err(serde::de::Error::custom(
                "fleet_replica source requires origin_runtime_id",
            )),
            (kind, _) => Err(serde::de::Error::custom(format!(
                "unknown asset source kind: {kind}"
            ))),
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct AssetMetadata {
    pub schema_version: u32,
    pub sha256: String,
    pub size: u64,
    pub media_type: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub width: Option<u32>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub height: Option<u32>,
    pub object_modified_ns: u64,
    pub stored_at: String,
    pub source: AssetSource,
}

struct FileSnapshot {
    bytes: Vec<u8>,
    size: u64,
    modified_ns: u64,
}

#[derive(Debug, Serialize, Deserialize)]
struct StoreManifest {
    schema_version: u32,
    namespace: String,
}

pub struct AssetService {
    upload_slots: Arc<Semaphore>,
    peer_slots: Arc<Semaphore>,
    workspaces: Mutex<HashMap<PathBuf, WorkspaceCacheEntry>>,
    pub store_failures: AtomicU64,
    pub hash_mismatches: AtomicU64,
    pub fleet_fetch_failures: AtomicU64,
    pub limits: AssetLimits,
}

#[derive(Clone)]
struct WorkspaceCacheEntry {
    state: Arc<WorkspaceAssetState>,
    generation: u64,
}

impl AssetService {
    pub fn new(limits: AssetLimits) -> Self {
        Self {
            upload_slots: Arc::new(Semaphore::new(limits.upload_slots)),
            peer_slots: Arc::new(Semaphore::new(limits.peer_slots)),
            workspaces: Mutex::new(HashMap::new()),
            store_failures: AtomicU64::new(0),
            hash_mismatches: AtomicU64::new(0),
            fleet_fetch_failures: AtomicU64::new(0),
            limits,
        }
    }

    pub fn open_store(
        &self,
        workspace_root: impl AsRef<Path>,
        binding: impl Into<String>,
    ) -> Result<AssetStore, AssetError> {
        let workspace_root = canonical_workspace_root(workspace_root.as_ref())?;
        let workspace_state = {
            lock(&self.workspaces)
                .get(&workspace_root)
                .map(|entry| Arc::clone(&entry.state))
                .unwrap_or_else(|| shared_workspace_state(&workspace_root))
        };
        let store = AssetStore::open_with_state(
            workspace_root,
            binding.into(),
            self.limits.clone(),
            workspace_state,
        )?;
        {
            let _operation = read_lock(&store.state.operation_gate);
            store.validate_current()?;
            lock(&self.workspaces).insert(
                store.workspace_root.clone(),
                WorkspaceCacheEntry {
                    state: Arc::clone(&store.state),
                    generation: store.generation,
                },
            );
        }
        Ok(store)
    }

    pub fn cached_usage(&self, workspace_root: impl AsRef<Path>) -> Option<AssetUsage> {
        let workspace_root = canonical_workspace_root(workspace_root.as_ref()).ok()?;
        let entry = lock(&self.workspaces).get(&workspace_root).cloned();
        let (state, cached_generation) = match entry {
            Some(entry) => (entry.state, Some(entry.generation)),
            None => (existing_workspace_state(&workspace_root)?, None),
        };
        let _operation = read_lock(&state.operation_gate);
        let accounting = lock(&state.accounting);
        (accounting.initialized
            && cached_generation.is_none_or(|generation| generation == accounting.generation))
        .then_some(accounting.committed)
    }

    pub fn evict_workspace(&self, workspace_root: impl AsRef<Path>) -> Result<bool, AssetError> {
        let workspace_root = canonical_workspace_root(workspace_root.as_ref())?;
        Ok(lock(&self.workspaces).remove(&workspace_root).is_some())
    }

    pub async fn acquire_upload(&self) -> Result<OwnedSemaphorePermit, AssetError> {
        Arc::clone(&self.upload_slots)
            .acquire_owned()
            .await
            .map_err(|_| AssetError::Invariant("asset upload semaphore closed"))
    }

    pub async fn acquire_peer(&self) -> Result<OwnedSemaphorePermit, AssetError> {
        Arc::clone(&self.peer_slots)
            .acquire_owned()
            .await
            .map_err(|_| AssetError::Invariant("asset peer semaphore closed"))
    }

    pub fn available_upload_permits(&self) -> usize {
        self.upload_slots.available_permits()
    }

    pub fn available_peer_permits(&self) -> usize {
        self.peer_slots.available_permits()
    }
}

impl Default for AssetService {
    fn default() -> Self {
        Self::new(AssetLimits::default())
    }
}

/// A generation-bound interface to one workspace asset namespace.
///
/// Namespace paths remain internal to the store in production builds.
#[cfg_attr(
    not(feature = "test-support"),
    doc = r#"
```compile_fail
use gitim_runtime::assets::AssetStore;

fn raw_namespace_root_is_not_available(store: &AssetStore) {
    let _ = store.root();
}
```

```compile_fail
use gitim_runtime::assets::AssetStore;

fn raw_object_path_is_not_available(store: &AssetStore, hash: &str) {
    let _ = store.object_path(hash);
}
```

```compile_fail
use gitim_runtime::assets::AssetStore;

fn raw_metadata_path_is_not_available(store: &AssetStore, hash: &str) {
    let _ = store.metadata_path(hash);
}
```

```compile_fail
use gitim_runtime::assets::AssetStore;

fn raw_lock_path_is_not_available(store: &AssetStore, hash: &str) {
    let _ = store.lock_path(hash);
}
```

```compile_fail
use gitim_runtime::assets::AssetStore;

fn raw_temp_path_is_not_available(store: &AssetStore) {
    let _ = store.create_owned_temp();
}
```
"#
)]
pub struct AssetStore {
    workspace_root: PathBuf,
    root: PathBuf,
    binding: String,
    limits: AssetLimits,
    state: Arc<WorkspaceAssetState>,
    generation: u64,
}

#[derive(Default)]
struct WorkspaceAssetState {
    accounting: Mutex<AccountingState>,
    operation_gate: RwLock<()>,
    #[cfg(feature = "test-support")]
    fail_next_sidecar_write: AtomicBool,
}

#[derive(Default)]
struct AccountingState {
    active_binding: Option<String>,
    generation: u64,
    committed: AssetUsage,
    reserved: u64,
    limits: Option<AssetLimits>,
    initialized: bool,
    objects: HashMap<String, u64>,
}

impl AssetStore {
    pub fn open(
        workspace_root: impl AsRef<Path>,
        binding: impl Into<String>,
        limits: AssetLimits,
    ) -> Result<Self, AssetError> {
        let workspace_root = canonical_workspace_root(workspace_root.as_ref())?;
        let state = shared_workspace_state(&workspace_root);
        Self::open_with_state(workspace_root, binding.into(), limits, state)
    }

    fn open_with_state(
        workspace_root: PathBuf,
        binding: String,
        limits: AssetLimits,
        state: Arc<WorkspaceAssetState>,
    ) -> Result<Self, AssetError> {
        if binding.trim().is_empty() {
            return Err(AssetError::Invalid(
                "asset namespace binding is empty".to_string(),
            ));
        }
        let root = workspace_root.join(".gitim-runtime/assets/v1");
        let _operation = write_lock(&state.operation_gate);
        let generation = {
            let mut accounting = lock(&state.accounting);
            if accounting.initialized {
                let existing_limits = accounting.limits.as_ref().ok_or(AssetError::Invariant(
                    "initialized asset store is missing limits",
                ))?;
                if existing_limits != &limits {
                    return Err(AssetError::Invalid(
                        "asset store is already open with incompatible limits".to_string(),
                    ));
                }
            }
            let same_initialized_binding = accounting.initialized
                && accounting.active_binding.as_deref() == Some(binding.as_str());
            if same_initialized_binding {
                validate_store_layout(&root)?;
            }
            if same_initialized_binding && manifest_matches(&root.join("store.json"), &binding) {
                accounting.generation
            } else {
                let generation = accounting
                    .generation
                    .checked_add(1)
                    .ok_or(AssetError::Invariant("asset store generation overflow"))?;
                accounting.active_binding = Some(binding.clone());
                accounting.generation = generation;
                accounting.committed = AssetUsage::default();
                accounting.reserved = 0;
                accounting.limits = Some(limits.clone());
                accounting.initialized = false;
                accounting.objects.clear();
                generation
            }
        };
        let store = Self {
            workspace_root,
            root,
            binding,
            limits,
            state: Arc::clone(&state),
            generation,
        };
        if !lock(&store.state.accounting).initialized {
            prepare_namespace(&store.workspace_root, &store.root, &store.binding)?;
            store.recover_under_gate()?;
        }
        Ok(store)
    }

    #[cfg(feature = "test-support")]
    pub fn root(&self) -> &Path {
        &self.root
    }

    pub fn usage(&self) -> Result<AssetUsage, AssetError> {
        let _operation = read_lock(&self.state.operation_gate);
        self.validate_current()?;
        Ok(lock(&self.state.accounting).committed)
    }

    pub fn reserved_bytes(&self) -> Result<u64, AssetError> {
        let _operation = read_lock(&self.state.operation_gate);
        self.validate_current()?;
        Ok(lock(&self.state.accounting).reserved)
    }

    #[cfg(feature = "test-support")]
    pub fn object_path(&self, hash: &str) -> Result<PathBuf, AssetError> {
        let _operation = read_lock(&self.state.operation_gate);
        self.validate_current()?;
        self.raw_object_path(hash)
    }

    #[cfg(feature = "test-support")]
    pub fn metadata_path(&self, hash: &str) -> Result<PathBuf, AssetError> {
        let _operation = read_lock(&self.state.operation_gate);
        self.validate_current()?;
        self.raw_metadata_path(hash)
    }

    #[cfg(feature = "test-support")]
    pub fn lock_path(&self, hash: &str) -> Result<PathBuf, AssetError> {
        let _operation = read_lock(&self.state.operation_gate);
        self.validate_current()?;
        validate_hash(hash)?;
        let shard = self.root.join("locks/sha256").join(&hash[..2]);
        validate_hash_shard(&shard)?;
        Ok(shard.join(format!("{hash}.lock")))
    }

    pub fn reserve(&self, incoming: u64) -> Result<AssetReservation, AssetError> {
        let _operation = read_lock(&self.state.operation_gate);
        self.validate_current()?;
        let available = fs2::available_space(&self.root)?;
        let total = fs2::total_space(&self.root)?;
        self.reserve_with_space(incoming, available, total)
    }

    fn reserve_with_space(
        &self,
        incoming: u64,
        available: u64,
        total: u64,
    ) -> Result<AssetReservation, AssetError> {
        let mut accounting = lock(&self.state.accounting);
        self.validate_accounting(&accounting)?;
        let committed_and_reserved = accounting
            .committed
            .bytes
            .checked_add(accounting.reserved)
            .ok_or(AssetError::QuotaExceeded {
                used: u64::MAX,
                quota: self.limits.workspace_quota_bytes,
            })?;
        let prospective =
            committed_and_reserved
                .checked_add(incoming)
                .ok_or(AssetError::QuotaExceeded {
                    used: committed_and_reserved,
                    quota: self.limits.workspace_quota_bytes,
                })?;
        if prospective > self.limits.workspace_quota_bytes {
            return Err(AssetError::QuotaExceeded {
                used: committed_and_reserved,
                quota: self.limits.workspace_quota_bytes,
            });
        }
        let pending =
            accounting
                .reserved
                .checked_add(incoming)
                .ok_or(AssetError::QuotaExceeded {
                    used: committed_and_reserved,
                    quota: self.limits.workspace_quota_bytes,
                })?;
        self.check_free_space(pending, committed_and_reserved, available, total)?;
        accounting.reserved = pending;
        Ok(AssetReservation {
            bytes: incoming,
            state: Arc::clone(&self.state),
            generation: self.generation,
            released: false,
        })
    }

    pub fn put_bytes(
        &self,
        bytes: &[u8],
        source: AssetSource,
    ) -> Result<AssetMetadata, AssetError> {
        let _operation = write_lock(&self.state.operation_gate);
        self.validate_current()?;
        if !valid_source(&source) {
            return Err(AssetError::Invalid(
                "fleet replica source requires a canonical runtime UUID".to_string(),
            ));
        }
        let size = u64::try_from(bytes.len()).map_err(|_| AssetError::TooLarge {
            limit: self.limits.max_file_bytes,
        })?;
        if size > self.limits.max_file_bytes {
            return Err(AssetError::TooLarge {
                limit: self.limits.max_file_bytes,
            });
        }
        let hash = sha256_hex(bytes);
        if let Some(metadata) = self.force_verify_dedupe(&hash, &source)? {
            return Ok(metadata);
        }

        let available = fs2::available_space(&self.root)?;
        let total = fs2::total_space(&self.root)?;
        let reservation = self.reserve_with_space(size, available, total)?;
        let object_path = self.raw_object_path(&hash)?;
        create_private_dir(
            object_path
                .parent()
                .ok_or_else(|| invalid_path(&object_path))?,
        )?;
        atomic_write(&object_path, bytes)?;
        let metadata = match self.metadata_from_object(&hash, source) {
            Ok(metadata) => metadata,
            Err(error) => {
                self.reconcile_accounting()?;
                return Err(error);
            }
        };
        if let Err(error) = self.write_metadata(&metadata) {
            if let Err(accounting_error) = reservation.commit(&hash, size) {
                self.reconcile_accounting()?;
                return Err(accounting_error);
            }
            return Err(error);
        }
        if let Err(error) = reservation.commit(&hash, size) {
            self.reconcile_accounting()?;
            return Err(error);
        }
        Ok(metadata)
    }

    pub fn read(&self, hash: &str) -> Result<Vec<u8>, AssetError> {
        let _operation = write_lock(&self.state.operation_gate);
        self.validate_current()?;
        let object_path = self.raw_object_path(hash)?;
        let snapshot = match read_file_snapshot(&object_path, self.limits.max_file_bytes) {
            Ok(snapshot) => snapshot,
            Err(AssetError::TooLarge { .. }) => {
                self.quarantine_corrupt(hash, &object_path)?;
                return Err(AssetError::LocalCorruption);
            }
            Err(error) => return Err(error),
        };
        let metadata_path = self.raw_metadata_path(hash)?;
        let existing_sidecar = read_sidecar(&metadata_path).ok();
        if !existing_sidecar.as_ref().is_some_and(|sidecar| {
            valid_sidecar(sidecar, hash, snapshot.size, snapshot.modified_ns)
        }) {
            self.rebuild_metadata_from_snapshot(
                hash,
                &object_path,
                &metadata_path,
                existing_sidecar.as_ref(),
                None,
                &snapshot,
            )?;
        }
        Ok(snapshot.bytes)
    }

    pub fn inspect(&self, hash: &str) -> Result<AssetMetadata, AssetError> {
        let _operation = write_lock(&self.state.operation_gate);
        self.validate_current()?;
        self.refresh_metadata(hash)
    }

    #[cfg(feature = "test-support")]
    pub fn create_owned_temp(&self) -> Result<PathBuf, AssetError> {
        let _operation = read_lock(&self.state.operation_gate);
        self.validate_current()?;
        let tmp = self.root.join("tmp");
        create_private_dir(&tmp)?;
        let tempfile = tempfile::Builder::new()
            .prefix("gitim-asset-")
            .suffix(".tmp")
            .tempfile_in(tmp)?;
        set_file_mode(tempfile.as_file())?;
        let (_file, path) = tempfile
            .keep()
            .map_err(|error| AssetError::Store(error.error))?;
        Ok(path)
    }

    #[cfg(feature = "test-support")]
    #[doc(hidden)]
    pub fn inject_sidecar_write_failure_once(&self) {
        self.state
            .fail_next_sidecar_write
            .store(true, Ordering::Release);
    }

    pub fn recover(&self) -> Result<AssetUsage, AssetError> {
        let _operation = write_lock(&self.state.operation_gate);
        self.validate_current()?;
        self.recover_under_gate()
    }

    fn recover_under_gate(&self) -> Result<AssetUsage, AssetError> {
        create_store_layout(&self.root)?;
        set_path_file_mode(&self.root.join("store.json"))?;
        self.cleanup_owned_temps()?;
        self.cleanup_atomic_write_temps()?;
        self.recover_objects()?;
        self.remove_orphan_sidecars()?;
        let (usage, objects) = self.scan_regular_object_ledger()?;
        let mut accounting = lock(&self.state.accounting);
        if accounting.generation != self.generation {
            return Err(AssetError::StaleBinding);
        }
        accounting.committed = usage;
        accounting.objects = objects;
        accounting.initialized = true;
        Ok(usage)
    }

    fn validate_current(&self) -> Result<(), AssetError> {
        {
            let accounting = lock(&self.state.accounting);
            self.validate_accounting(&accounting)?;
        }
        validate_store_layout(&self.root)?;
        if !manifest_matches(&self.root.join("store.json"), &self.binding) {
            return Err(AssetError::StaleBinding);
        }
        Ok(())
    }

    fn validate_accounting(&self, accounting: &AccountingState) -> Result<(), AssetError> {
        if accounting.initialized
            && accounting.generation == self.generation
            && accounting.active_binding.as_deref() == Some(self.binding.as_str())
        {
            Ok(())
        } else {
            Err(AssetError::StaleBinding)
        }
    }

    fn raw_object_path(&self, hash: &str) -> Result<PathBuf, AssetError> {
        validate_hash(hash)?;
        let shard = self.root.join("objects/sha256").join(&hash[..2]);
        validate_hash_shard(&shard)?;
        Ok(shard.join(hash))
    }

    fn raw_metadata_path(&self, hash: &str) -> Result<PathBuf, AssetError> {
        validate_hash(hash)?;
        let shard = self.root.join("metadata/sha256").join(&hash[..2]);
        validate_hash_shard(&shard)?;
        Ok(shard.join(format!("{hash}.json")))
    }

    fn check_free_space(
        &self,
        incoming: u64,
        used: u64,
        available: u64,
        total: u64,
    ) -> Result<(), AssetError> {
        let reserve = self.limits.min_free_bytes.max(total / 20);
        if available
            .checked_sub(incoming)
            .is_none_or(|remaining| remaining < reserve)
        {
            return Err(AssetError::QuotaExceeded {
                used,
                quota: self.limits.workspace_quota_bytes,
            });
        }
        Ok(())
    }

    fn refresh_metadata(&self, hash: &str) -> Result<AssetMetadata, AssetError> {
        let object_path = self.raw_object_path(hash)?;
        let object_metadata = fs::symlink_metadata(&object_path).map_err(map_missing)?;
        if !object_metadata.file_type().is_file() {
            return Err(AssetError::Missing);
        }
        set_path_file_mode(&object_path)?;
        if object_metadata.len() > self.limits.max_file_bytes {
            self.quarantine_corrupt(hash, &object_path)?;
            return Err(AssetError::LocalCorruption);
        }
        let modified_ns = modified_ns(&object_metadata)?;
        let metadata_path = self.raw_metadata_path(hash)?;
        let existing_sidecar = read_sidecar(&metadata_path).ok();
        if let Some(sidecar) = &existing_sidecar {
            if valid_sidecar(sidecar, hash, object_metadata.len(), modified_ns) {
                set_path_file_mode(&metadata_path)?;
                return Ok(sidecar.clone());
            }
        }

        let snapshot = match read_file_snapshot(&object_path, self.limits.max_file_bytes) {
            Ok(snapshot) => snapshot,
            Err(AssetError::TooLarge { .. }) => {
                self.quarantine_corrupt(hash, &object_path)?;
                return Err(AssetError::LocalCorruption);
            }
            Err(error) => return Err(error),
        };
        self.rebuild_metadata_from_snapshot(
            hash,
            &object_path,
            &metadata_path,
            existing_sidecar.as_ref(),
            None,
            &snapshot,
        )
    }

    fn rebuild_metadata_from_snapshot(
        &self,
        hash: &str,
        object_path: &Path,
        metadata_path: &Path,
        existing_sidecar: Option<&AssetMetadata>,
        fallback_source: Option<&AssetSource>,
        snapshot: &FileSnapshot,
    ) -> Result<AssetMetadata, AssetError> {
        if sha256_hex(&snapshot.bytes) != hash {
            self.quarantine_corrupt(hash, object_path)?;
            return Err(AssetError::LocalCorruption);
        }
        let source = existing_sidecar
            .map(|sidecar| &sidecar.source)
            .filter(|source| valid_source(source))
            .cloned()
            .or_else(|| read_recoverable_source(metadata_path))
            .or_else(|| {
                fallback_source
                    .filter(|source| valid_source(source))
                    .cloned()
            })
            .unwrap_or(AssetSource::LocalUpload);
        let metadata = metadata_for_bytes(hash, &snapshot.bytes, snapshot.modified_ns, source)?;
        self.write_metadata(&metadata)?;
        Ok(metadata)
    }

    fn force_verify_dedupe(
        &self,
        hash: &str,
        source: &AssetSource,
    ) -> Result<Option<AssetMetadata>, AssetError> {
        let object_path = self.raw_object_path(hash)?;
        let snapshot = match read_file_snapshot(&object_path, self.limits.max_file_bytes) {
            Ok(snapshot) => snapshot,
            Err(AssetError::Missing) => return Ok(None),
            Err(AssetError::TooLarge { .. }) => {
                self.quarantine_corrupt(hash, &object_path)?;
                return Ok(None);
            }
            Err(error) => return Err(error),
        };
        if sha256_hex(&snapshot.bytes) != hash {
            self.quarantine_corrupt(hash, &object_path)?;
            return Ok(None);
        }
        let metadata_path = self.raw_metadata_path(hash)?;
        let existing_sidecar = read_sidecar(&metadata_path).ok();
        let metadata = if existing_sidecar.as_ref().is_some_and(|sidecar| {
            valid_sidecar(sidecar, hash, snapshot.size, snapshot.modified_ns)
        }) {
            existing_sidecar.ok_or(AssetError::Invariant(
                "asset sidecar disappeared during dedupe verification",
            ))?
        } else {
            match self.rebuild_metadata_from_snapshot(
                hash,
                &object_path,
                &metadata_path,
                existing_sidecar.as_ref(),
                Some(source),
                &snapshot,
            ) {
                Ok(metadata) => metadata,
                Err(error) => {
                    self.register_existing_object(hash, snapshot.size)?;
                    return Err(error);
                }
            }
        };
        self.register_existing_object(hash, snapshot.size)?;
        Ok(Some(metadata))
    }

    fn metadata_from_object(
        &self,
        hash: &str,
        source: AssetSource,
    ) -> Result<AssetMetadata, AssetError> {
        let object_path = self.raw_object_path(hash)?;
        let snapshot = read_file_snapshot(&object_path, self.limits.max_file_bytes)?;
        metadata_for_bytes(hash, &snapshot.bytes, snapshot.modified_ns, source)
    }

    fn register_existing_object(&self, hash: &str, size: u64) -> Result<(), AssetError> {
        let mut accounting = lock(&self.state.accounting);
        self.validate_accounting(&accounting)?;
        if accounting.objects.contains_key(hash) {
            return Ok(());
        }
        let committed =
            AssetUsage {
                bytes: accounting
                    .committed
                    .bytes
                    .checked_add(size)
                    .ok_or(AssetError::Invariant("asset committed byte count overflow"))?,
                objects: accounting.committed.objects.checked_add(1).ok_or(
                    AssetError::Invariant("asset committed object count overflow"),
                )?,
            };
        accounting.objects.insert(hash.to_string(), size);
        accounting.committed = committed;
        Ok(())
    }

    fn reconcile_accounting(&self) -> Result<(), AssetError> {
        let (usage, objects) = self.scan_regular_object_ledger()?;
        let mut accounting = lock(&self.state.accounting);
        self.validate_accounting(&accounting)?;
        accounting.committed = usage;
        accounting.objects = objects;
        Ok(())
    }

    fn write_metadata(&self, metadata: &AssetMetadata) -> Result<(), AssetError> {
        #[cfg(feature = "test-support")]
        if self
            .state
            .fail_next_sidecar_write
            .swap(false, Ordering::AcqRel)
        {
            return Err(AssetError::Store(std::io::Error::other(
                "injected asset sidecar write failure",
            )));
        }
        let path = self.raw_metadata_path(&metadata.sha256)?;
        let parent = path.parent().ok_or_else(|| invalid_path(&path))?;
        create_private_dir(parent)?;
        let bytes = serde_json::to_vec_pretty(metadata)
            .map_err(|error| std::io::Error::other(format!("serialize asset metadata: {error}")))?;
        atomic_write(&path, &bytes)
    }

    fn recover_objects(&self) -> Result<(), AssetError> {
        let objects_root = self.root.join("objects/sha256");
        for shard in read_dir_or_empty(&objects_root)? {
            let shard = shard?;
            let shard_type = shard.file_type()?;
            let shard_name = shard.file_name();
            let shard_name = shard_name.to_string_lossy();
            if !shard_type.is_dir() || !valid_shard(&shard_name) {
                continue;
            }
            set_dir_mode(&shard.path())?;
            for object in read_dir_or_empty(&shard.path())? {
                let object = object?;
                let file_type = object.file_type()?;
                let hash = object.file_name().to_string_lossy().into_owned();
                if !file_type.is_file()
                    || validate_hash(&hash).is_err()
                    || !hash.starts_with(&*shard_name)
                {
                    if file_type.is_symlink() {
                        fs::remove_file(object.path())?;
                    }
                    continue;
                }
                set_path_file_mode(&object.path())?;
                match self.refresh_metadata(&hash) {
                    Ok(_) => {}
                    Err(
                        AssetError::LocalCorruption
                        | AssetError::Missing
                        | AssetError::TooLarge { .. },
                    ) => {}
                    Err(error) => return Err(error),
                }
            }
        }
        Ok(())
    }

    fn scan_regular_object_ledger(&self) -> Result<(AssetUsage, HashMap<String, u64>), AssetError> {
        let mut usage = AssetUsage::default();
        let mut objects = HashMap::new();
        let objects_root = self.root.join("objects/sha256");
        for shard in read_dir_or_empty(&objects_root)? {
            let shard = shard?;
            let shard_name = shard.file_name();
            let shard_name = shard_name.to_string_lossy();
            let shard_metadata = fs::symlink_metadata(shard.path())?;
            if !shard_metadata.file_type().is_dir() || !valid_shard(&shard_name) {
                continue;
            }
            for object in read_dir_or_empty(&shard.path())? {
                let object = object?;
                let hash = object.file_name().to_string_lossy().into_owned();
                if validate_hash(&hash).is_err() || !hash.starts_with(&*shard_name) {
                    continue;
                }
                let metadata = fs::symlink_metadata(object.path())?;
                if !metadata.file_type().is_file() || metadata.len() > self.limits.max_file_bytes {
                    continue;
                }
                usage.bytes = usage.bytes.checked_add(metadata.len()).ok_or_else(|| {
                    AssetError::Store(std::io::Error::other("asset usage overflow"))
                })?;
                usage.objects = usage.objects.checked_add(1).ok_or_else(|| {
                    AssetError::Store(std::io::Error::other("asset object count overflow"))
                })?;
                objects.insert(hash, metadata.len());
            }
        }
        Ok((usage, objects))
    }

    fn remove_orphan_sidecars(&self) -> Result<(), AssetError> {
        let metadata_root = self.root.join("metadata/sha256");
        for shard in read_dir_or_empty(&metadata_root)? {
            let shard = shard?;
            if !shard.file_type()?.is_dir() {
                continue;
            }
            set_dir_mode(&shard.path())?;
            for sidecar in read_dir_or_empty(&shard.path())? {
                let sidecar = sidecar?;
                let file_type = sidecar.file_type()?;
                if !file_type.is_file() && !file_type.is_symlink() {
                    continue;
                }
                let name = sidecar.file_name().to_string_lossy().into_owned();
                let Some(hash) = name.strip_suffix(".json") else {
                    continue;
                };
                if validate_hash(hash).is_err() {
                    continue;
                }
                let object_path = self.raw_object_path(hash)?;
                let has_regular_object = fs::symlink_metadata(object_path)
                    .map(|metadata| metadata.file_type().is_file())
                    .unwrap_or(false);
                if !has_regular_object {
                    fs::remove_file(sidecar.path())?;
                }
            }
        }
        Ok(())
    }

    fn cleanup_owned_temps(&self) -> Result<(), AssetError> {
        let now = SystemTime::now();
        let tmp = self.root.join("tmp");
        for entry in read_dir_or_empty(&tmp)? {
            let entry = entry?;
            let file_type = entry.file_type()?;
            if !file_type.is_file() {
                continue;
            }
            let name = entry.file_name();
            let name = name.to_string_lossy();
            if !name.starts_with("gitim-asset-") || !name.ends_with(".tmp") {
                continue;
            }
            set_path_file_mode(&entry.path())?;
            let metadata = entry.metadata()?;
            let age = now.duration_since(metadata.modified()?).unwrap_or_default();
            if age > self.limits.temp_ttl {
                fs::remove_file(entry.path())?;
            }
        }
        Ok(())
    }

    fn cleanup_atomic_write_temps(&self) -> Result<(), AssetError> {
        let now = SystemTime::now();
        self.cleanup_atomic_write_temps_in(&self.root, now)?;
        for root in [
            self.root.join("objects/sha256"),
            self.root.join("metadata/sha256"),
        ] {
            for shard in read_dir_or_empty(&root)? {
                let shard = shard?;
                let shard_name = shard.file_name().to_string_lossy().into_owned();
                if !shard.file_type()?.is_dir() || !valid_shard(&shard_name) {
                    continue;
                }
                self.cleanup_atomic_write_temps_in(&shard.path(), now)?;
            }
        }
        Ok(())
    }

    fn cleanup_atomic_write_temps_in(
        &self,
        directory: &Path,
        now: SystemTime,
    ) -> Result<(), AssetError> {
        for entry in read_dir_or_empty(directory)? {
            let entry = entry?;
            if !entry.file_type()?.is_file() {
                continue;
            }
            let name = entry.file_name();
            let name = name.to_string_lossy();
            if !name.starts_with("gitim-atomic-") || !name.ends_with(".tmp") {
                continue;
            }
            let metadata = entry.metadata()?;
            let age = now.duration_since(metadata.modified()?).unwrap_or_default();
            if age > self.limits.temp_ttl {
                fs::remove_file(entry.path())?;
            } else {
                set_path_file_mode(&entry.path())?;
            }
        }
        Ok(())
    }

    fn quarantine_corrupt(&self, hash: &str, object_path: &Path) -> Result<(), AssetError> {
        let root = self
            .workspace_root
            .join(".gitim-runtime/orphaned-assets/corrupt-objects");
        create_private_dir(&root)?;
        quarantine_rename(object_path, &root, &format!("corrupt-{hash}"))?;
        let metadata_path = self.raw_metadata_path(hash)?;
        remove_file_if_exists(&metadata_path)?;
        let (usage, objects) = self.scan_regular_object_ledger()?;
        let mut accounting = lock(&self.state.accounting);
        if accounting.generation == self.generation {
            accounting.committed = usage;
            accounting.objects = objects;
        }
        Ok(())
    }
}

pub struct AssetReservation {
    bytes: u64,
    state: Arc<WorkspaceAssetState>,
    generation: u64,
    released: bool,
}

impl AssetReservation {
    pub fn release(mut self) -> Result<(), AssetError> {
        self.release_inner()
    }

    fn commit(mut self, hash: &str, size: u64) -> Result<(), AssetError> {
        let mut accounting = lock(&self.state.accounting);
        if accounting.generation != self.generation {
            self.released = true;
            return Err(AssetError::StaleBinding);
        }
        let reserved = accounting
            .reserved
            .checked_sub(self.bytes)
            .ok_or(AssetError::Invariant("asset reservation commit underflow"))?;
        let is_new = !accounting.objects.contains_key(hash);
        let committed = if is_new {
            AssetUsage {
                bytes: accounting
                    .committed
                    .bytes
                    .checked_add(size)
                    .ok_or(AssetError::Invariant("asset committed byte count overflow"))?,
                objects: accounting.committed.objects.checked_add(1).ok_or(
                    AssetError::Invariant("asset committed object count overflow"),
                )?,
            }
        } else {
            accounting.committed
        };
        accounting.reserved = reserved;
        accounting.committed = committed;
        if is_new {
            accounting.objects.insert(hash.to_string(), size);
        }
        self.released = true;
        Ok(())
    }

    fn release_inner(&mut self) -> Result<(), AssetError> {
        if self.released {
            return Ok(());
        }
        let mut accounting = lock(&self.state.accounting);
        if accounting.generation != self.generation {
            self.released = true;
            return Ok(());
        }
        accounting.reserved = accounting
            .reserved
            .checked_sub(self.bytes)
            .ok_or(AssetError::Invariant("asset reservation release underflow"))?;
        self.released = true;
        Ok(())
    }
}

impl Drop for AssetReservation {
    fn drop(&mut self) {
        if let Err(error) = self.release_inner() {
            tracing::error!(error = %error, "asset reservation release invariant failed");
        }
    }
}

fn prepare_namespace(workspace_root: &Path, root: &Path, binding: &str) -> Result<(), AssetError> {
    let manifest_path = root.join("store.json");
    let root_metadata = fs::symlink_metadata(root);
    let root_exists = match root_metadata {
        Ok(metadata) => {
            if !metadata.file_type().is_dir() {
                quarantine_namespace(workspace_root, root)?;
                false
            } else {
                true
            }
        }
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => false,
        Err(error) => return Err(AssetError::Store(error)),
    };

    if root_exists && manifest_matches(&manifest_path, binding) {
        create_store_layout(root)?;
        set_path_file_mode(&manifest_path)?;
        return Ok(());
    }

    if root_exists {
        let manifest_exists = fs::symlink_metadata(&manifest_path).is_ok();
        if manifest_exists || namespace_has_data(root)? {
            quarantine_namespace(workspace_root, root)?;
        }
    }

    create_store_layout(root)?;
    write_manifest(root, binding)
}

fn manifest_matches(path: &Path, binding: &str) -> bool {
    read_bounded_regular_file(path, 64 * 1024)
        .ok()
        .and_then(|bytes| serde_json::from_slice::<StoreManifest>(&bytes).ok())
        .is_some_and(|manifest| {
            manifest.schema_version == STORE_SCHEMA_VERSION && manifest.namespace == binding
        })
}

fn write_manifest(root: &Path, binding: &str) -> Result<(), AssetError> {
    let manifest = StoreManifest {
        schema_version: STORE_SCHEMA_VERSION,
        namespace: binding.to_string(),
    };
    let bytes = serde_json::to_vec_pretty(&manifest)
        .map_err(|error| std::io::Error::other(format!("serialize asset manifest: {error}")))?;
    atomic_write(&root.join("store.json"), &bytes)
}

fn namespace_has_data(root: &Path) -> Result<bool, AssetError> {
    for entry in read_dir_or_empty(root)? {
        let entry = entry?;
        if entry.file_name() == "store.json" {
            continue;
        }
        let file_type = entry.file_type()?;
        if file_type.is_file() || file_type.is_symlink() {
            return Ok(true);
        }
        if file_type.is_dir() && namespace_has_data(&entry.path())? {
            return Ok(true);
        }
    }
    Ok(false)
}

fn quarantine_namespace(workspace_root: &Path, root: &Path) -> Result<(), AssetError> {
    let orphaned = workspace_root.join(".gitim-runtime/orphaned-assets");
    create_private_dir(&orphaned)?;
    quarantine_rename(root, &orphaned, "assets-v1")?;
    Ok(())
}

fn quarantine_rename(
    source: &Path,
    destination_parent: &Path,
    label: &str,
) -> Result<PathBuf, AssetError> {
    let source_parent = source.parent().ok_or_else(|| invalid_path(source))?;
    let source_is_dir = fs::symlink_metadata(source)?.file_type().is_dir();
    loop {
        let nanos = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap_or_default()
            .as_nanos();
        let sequence = UNIQUE_SUFFIX.fetch_add(1, Ordering::Relaxed);
        let candidate =
            destination_parent.join(format!("{label}-{nanos}-{}-{sequence}", std::process::id()));
        let reserved = if source_is_dir {
            fs::create_dir(&candidate).and_then(|()| {
                set_dir_mode(&candidate).map_err(|error| match error {
                    AssetError::Store(error) => error,
                    other => std::io::Error::other(other.to_string()),
                })
            })
        } else {
            OpenOptions::new()
                .write(true)
                .create_new(true)
                .open(&candidate)
                .and_then(|file| {
                    set_file_mode(&file).map_err(|error| match error {
                        AssetError::Store(error) => error,
                        other => std::io::Error::other(other.to_string()),
                    })
                })
        };
        match reserved {
            Ok(()) => {}
            Err(error) if error.kind() == std::io::ErrorKind::AlreadyExists => continue,
            Err(error) => return Err(AssetError::Store(error)),
        }
        if let Err(error) = fs::rename(source, &candidate) {
            if source_is_dir {
                let _ = fs::remove_dir(&candidate);
            } else {
                let _ = fs::remove_file(&candidate);
            }
            return Err(AssetError::Store(error));
        }
        sync_parent_best_effort(source_parent);
        sync_parent_best_effort(destination_parent);
        return Ok(candidate);
    }
}

fn create_store_layout(root: &Path) -> Result<(), AssetError> {
    for path in store_layout_paths(root)? {
        create_private_dir(&path)?;
    }
    Ok(())
}

fn validate_store_layout(root: &Path) -> Result<(), AssetError> {
    for path in store_layout_paths(root)? {
        match fs::symlink_metadata(&path) {
            Ok(metadata) if metadata.file_type().is_dir() => {}
            Ok(_) => return Err(invalid_directory(&path)),
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
                return Err(invalid_directory(&path));
            }
            Err(error) => return Err(AssetError::Store(error)),
        }
    }
    Ok(())
}

fn store_layout_paths(root: &Path) -> Result<[PathBuf; 9], AssetError> {
    let assets_root = root
        .parent()
        .ok_or_else(|| invalid_path(root))?
        .to_path_buf();
    Ok([
        assets_root,
        root.to_path_buf(),
        root.join("objects"),
        root.join("objects/sha256"),
        root.join("metadata"),
        root.join("metadata/sha256"),
        root.join("locks"),
        root.join("locks/sha256"),
        root.join("tmp"),
    ])
}

fn validate_hash_shard(path: &Path) -> Result<(), AssetError> {
    match fs::symlink_metadata(path) {
        Ok(metadata) if metadata.file_type().is_dir() => Ok(()),
        Ok(_) => Err(invalid_directory(path)),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(()),
        Err(error) => Err(AssetError::Store(error)),
    }
}

fn create_private_dir(path: &Path) -> Result<(), AssetError> {
    match fs::symlink_metadata(path) {
        Ok(metadata) if metadata.file_type().is_dir() => set_dir_mode(path),
        Ok(_) => Err(AssetError::Store(std::io::Error::new(
            std::io::ErrorKind::InvalidData,
            format!(
                "asset directory is not a regular directory: {}",
                path.display()
            ),
        ))),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
            fs::create_dir_all(path)?;
            let metadata = fs::symlink_metadata(path)?;
            if !metadata.file_type().is_dir() {
                return Err(AssetError::Store(std::io::Error::new(
                    std::io::ErrorKind::InvalidData,
                    format!(
                        "asset directory is not a regular directory: {}",
                        path.display()
                    ),
                )));
            }
            set_dir_mode(path)
        }
        Err(error) => Err(AssetError::Store(error)),
    }
}

#[cfg(unix)]
fn set_dir_mode(path: &Path) -> Result<(), AssetError> {
    use std::os::unix::fs::PermissionsExt;
    fs::set_permissions(path, fs::Permissions::from_mode(0o700))?;
    Ok(())
}

#[cfg(not(unix))]
fn set_dir_mode(_path: &Path) -> Result<(), AssetError> {
    Ok(())
}

#[cfg(unix)]
fn set_file_mode(file: &File) -> Result<(), AssetError> {
    use std::os::unix::fs::PermissionsExt;
    file.set_permissions(fs::Permissions::from_mode(0o600))?;
    Ok(())
}

#[cfg(not(unix))]
fn set_file_mode(_file: &File) -> Result<(), AssetError> {
    Ok(())
}

fn set_path_file_mode(path: &Path) -> Result<(), AssetError> {
    let file = open_options_no_follow(path, false)?;
    set_file_mode(&file)
}

fn atomic_write(path: &Path, bytes: &[u8]) -> Result<(), AssetError> {
    let parent = path.parent().ok_or_else(|| invalid_path(path))?;
    create_private_dir(parent)?;
    let mut temp = tempfile::Builder::new()
        .prefix("gitim-atomic-")
        .suffix(".tmp")
        .tempfile_in(parent)?;
    set_file_mode(temp.as_file())?;
    temp.write_all(bytes)?;
    temp.flush()?;
    temp.as_file().sync_all()?;
    temp.persist(path)
        .map_err(|error| AssetError::Store(error.error))?;
    set_path_file_mode(path)?;
    sync_parent_best_effort(parent);
    Ok(())
}

fn sync_parent_best_effort(parent: &Path) {
    if let Err(error) = File::open(parent).and_then(|directory| directory.sync_all()) {
        tracing::warn!(
            path = %parent.display(),
            error = %error,
            "asset write committed but parent directory sync failed"
        );
    }
}

fn read_sidecar(path: &Path) -> Result<AssetMetadata, AssetError> {
    let bytes = read_bounded_regular_file(path, 64 * 1024)?;
    serde_json::from_slice(&bytes)
        .map_err(|error| AssetError::Invalid(format!("invalid asset metadata: {error}")))
}

fn read_recoverable_source(path: &Path) -> Option<AssetSource> {
    let bytes = read_bounded_regular_file(path, 64 * 1024).ok()?;
    let value = serde_json::from_slice::<serde_json::Value>(&bytes).ok()?;
    let source = serde_json::from_value::<AssetSource>(value.get("source")?.clone()).ok()?;
    valid_source(&source).then_some(source)
}

fn valid_source(source: &AssetSource) -> bool {
    match source {
        AssetSource::LocalUpload => true,
        AssetSource::FleetReplica { origin_runtime_id } => uuid::Uuid::parse_str(origin_runtime_id)
            .is_ok_and(|runtime_id| runtime_id.to_string() == *origin_runtime_id),
    }
}

fn canonical_media_type(value: &str) -> Option<mime::Mime> {
    if value.len() > 127 || value.bytes().any(|byte| byte.is_ascii_uppercase()) {
        return None;
    }
    let media_type = value.parse::<mime::Mime>().ok()?;
    if media_type.type_() == mime::STAR
        || media_type.subtype() == mime::STAR
        || media_type.essence_str() != value
        || media_type.params().next().is_some()
    {
        return None;
    }
    Some(media_type)
}

fn valid_sidecar(sidecar: &AssetMetadata, hash: &str, size: u64, modified_ns: u64) -> bool {
    let Some(media_type) = canonical_media_type(&sidecar.media_type) else {
        return false;
    };
    let dimensions_valid = match (sidecar.width, sidecar.height) {
        (None, None) => true,
        (Some(width), Some(height)) => width > 0 && height > 0 && media_type.type_() == mime::IMAGE,
        _ => false,
    };
    sidecar.schema_version == SIDECAR_SCHEMA_VERSION
        && sidecar.sha256 == hash
        && sidecar.size == size
        && sidecar.object_modified_ns == modified_ns
        && chrono::DateTime::parse_from_rfc3339(&sidecar.stored_at).is_ok()
        && dimensions_valid
        && valid_source(&sidecar.source)
}

fn metadata_for_bytes(
    hash: &str,
    bytes: &[u8],
    object_modified_ns: u64,
    source: AssetSource,
) -> Result<AssetMetadata, AssetError> {
    if !valid_source(&source) {
        return Err(AssetError::Invalid(
            "fleet replica source requires a canonical runtime UUID".to_string(),
        ));
    }
    let inspection = inspect_bytes(bytes, "")?;
    let size = u64::try_from(bytes.len())
        .map_err(|_| AssetError::Invalid("asset size cannot fit u64".to_string()))?;
    Ok(AssetMetadata {
        schema_version: SIDECAR_SCHEMA_VERSION,
        sha256: hash.to_string(),
        size,
        media_type: inspection.media_type,
        width: inspection.width,
        height: inspection.height,
        object_modified_ns,
        stored_at: Utc::now().to_rfc3339(),
        source,
    })
}

fn modified_ns(metadata: &fs::Metadata) -> Result<u64, AssetError> {
    let duration = metadata
        .modified()?
        .duration_since(UNIX_EPOCH)
        .map_err(|error| {
            AssetError::Store(std::io::Error::new(
                std::io::ErrorKind::InvalidData,
                format!("asset modification time predates unix epoch: {error}"),
            ))
        })?;
    u64::try_from(duration.as_nanos()).map_err(|_| {
        AssetError::Store(std::io::Error::new(
            std::io::ErrorKind::InvalidData,
            "asset modification timestamp cannot fit u64 nanoseconds",
        ))
    })
}

fn read_file_snapshot(path: &Path, limit: u64) -> Result<FileSnapshot, AssetError> {
    read_file_snapshot_with_hook(path, limit, || {})
}

fn read_file_snapshot_with_hook(
    path: &Path,
    limit: u64,
    after_open: impl FnOnce(),
) -> Result<FileSnapshot, AssetError> {
    let mut file = open_options_no_follow(path, false).map_err(map_missing)?;
    let before = file.metadata()?;
    if !before.file_type().is_file() {
        return Err(AssetError::Missing);
    }
    if before.len() > limit {
        return Err(AssetError::TooLarge { limit });
    }
    set_file_mode(&file)?;
    let before_modified_ns = modified_ns(&before)?;
    after_open();
    let capacity = usize::try_from(before.len().min(64 * 1024)).map_err(|_| {
        AssetError::Invalid("asset snapshot capacity cannot fit address space".to_string())
    })?;
    let mut bytes = Vec::with_capacity(capacity);
    (&mut file)
        .take(limit.saturating_add(1))
        .read_to_end(&mut bytes)?;
    if u64::try_from(bytes.len()).map_or(true, |length| length > limit) {
        return Err(AssetError::TooLarge { limit });
    }
    let after = file.metadata()?;
    let byte_len = u64::try_from(bytes.len())
        .map_err(|_| AssetError::Invalid("asset size cannot fit u64".to_string()))?;
    if !after.file_type().is_file()
        || before.len() != after.len()
        || byte_len != after.len()
        || before_modified_ns != modified_ns(&after)?
    {
        return Err(AssetError::LocalCorruption);
    }
    Ok(FileSnapshot {
        bytes,
        size: byte_len,
        modified_ns: before_modified_ns,
    })
}

fn read_bounded_regular_file(path: &Path, limit: u64) -> Result<Vec<u8>, AssetError> {
    read_file_snapshot(path, limit)
        .map(|snapshot| snapshot.bytes)
        .map_err(|error| match error {
            AssetError::TooLarge { .. } | AssetError::LocalCorruption => AssetError::Invalid(
                format!("asset metadata file exceeds the {limit}-byte limit or changed while read"),
            ),
            other => other,
        })
}

fn open_options_no_follow(path: &Path, write: bool) -> std::io::Result<File> {
    let mut options = OpenOptions::new();
    options.read(true).write(write);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        options.custom_flags(libc::O_NOFOLLOW);
    }
    options.open(path)
}

fn sha256_hex(bytes: &[u8]) -> String {
    format!("{:x}", Sha256::digest(bytes))
}

fn validate_hash(hash: &str) -> Result<(), AssetError> {
    if hash.len() == 64
        && hash
            .as_bytes()
            .iter()
            .all(|byte| byte.is_ascii_digit() || matches!(byte, b'a'..=b'f'))
    {
        Ok(())
    } else {
        Err(AssetError::Invalid(
            "asset hash must be 64 lowercase hexadecimal characters".to_string(),
        ))
    }
}

fn valid_shard(shard: &str) -> bool {
    shard.len() == 2
        && shard
            .as_bytes()
            .iter()
            .all(|byte| byte.is_ascii_digit() || matches!(byte, b'a'..=b'f'))
}

fn read_dir_or_empty(
    path: &Path,
) -> Result<impl Iterator<Item = std::io::Result<fs::DirEntry>>, AssetError> {
    match fs::read_dir(path) {
        Ok(entries) => Ok(Some(entries).into_iter().flatten()),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
            Ok(None.into_iter().flatten())
        }
        Err(error) => Err(AssetError::Store(error)),
    }
}

fn remove_file_if_exists(path: &Path) -> Result<(), AssetError> {
    match fs::remove_file(path) {
        Ok(()) => Ok(()),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(()),
        Err(error) => Err(AssetError::Store(error)),
    }
}

fn map_missing(error: std::io::Error) -> AssetError {
    if error.kind() == std::io::ErrorKind::NotFound {
        AssetError::Missing
    } else {
        AssetError::Store(error)
    }
}

fn invalid_path(_path: &Path) -> AssetError {
    AssetError::Invalid("asset store path is invalid".to_string())
}

fn invalid_directory(path: &Path) -> AssetError {
    AssetError::Store(std::io::Error::new(
        std::io::ErrorKind::InvalidData,
        format!(
            "asset directory is missing or is not a regular directory: {}",
            path.display()
        ),
    ))
}

fn canonical_workspace_root(path: &Path) -> Result<PathBuf, AssetError> {
    let canonical = fs::canonicalize(path).map_err(AssetError::Store)?;
    if fs::symlink_metadata(&canonical)?.file_type().is_dir() {
        Ok(canonical)
    } else {
        Err(AssetError::Invalid(
            "asset workspace root must be an existing directory".to_string(),
        ))
    }
}

fn shared_workspace_state(workspace_root: &Path) -> Arc<WorkspaceAssetState> {
    let registry = WORKSPACE_STATES.get_or_init(|| Mutex::new(HashMap::new()));
    let mut states = lock(registry);
    states.retain(|_, state| state.strong_count() > 0);
    if let Some(state) = states.get(workspace_root).and_then(Weak::upgrade) {
        return state;
    }
    let state = Arc::new(WorkspaceAssetState::default());
    states.insert(workspace_root.to_path_buf(), Arc::downgrade(&state));
    state
}

fn existing_workspace_state(workspace_root: &Path) -> Option<Arc<WorkspaceAssetState>> {
    let registry = WORKSPACE_STATES.get_or_init(|| Mutex::new(HashMap::new()));
    let mut states = lock(registry);
    states.retain(|_, state| state.strong_count() > 0);
    states.get(workspace_root).and_then(Weak::upgrade)
}

fn lock<T>(mutex: &Mutex<T>) -> MutexGuard<'_, T> {
    mutex
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner)
}

fn read_lock<T>(lock: &RwLock<T>) -> RwLockReadGuard<'_, T> {
    lock.read()
        .unwrap_or_else(std::sync::PoisonError::into_inner)
}

fn write_lock<T>(lock: &RwLock<T>) -> RwLockWriteGuard<'_, T> {
    lock.write()
        .unwrap_or_else(std::sync::PoisonError::into_inner)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn free_space_check_counts_all_live_reservations() {
        let workspace = tempfile::TempDir::new().expect("temporary workspace");
        let limits = AssetLimits {
            workspace_quota_bytes: 1_000,
            min_free_bytes: 90,
            max_file_bytes: 1_000,
            max_request_bytes: 1_000,
            max_files: 10,
            temp_ttl: Duration::from_secs(60),
            upload_slots: 2,
            peer_slots: 4,
        };
        let store =
            AssetStore::open(workspace.path(), "local:test", limits).expect("open asset store");

        let first = store
            .reserve_with_space(5, 100, 1_000)
            .expect("first reservation");
        assert!(matches!(
            store.reserve_with_space(6, 100, 1_000),
            Err(AssetError::QuotaExceeded { .. })
        ));
        drop(first);
    }

    #[test]
    fn bounded_snapshot_rejects_growth_after_open() {
        let directory = tempfile::TempDir::new().expect("temporary directory");
        let path = directory.path().join("object");
        fs::write(&path, b"abc").expect("write object");
        let result = read_file_snapshot_with_hook(&path, 3, || {
            let mut file = OpenOptions::new()
                .append(true)
                .open(&path)
                .expect("open growing object");
            file.write_all(b"d").expect("grow object");
        });
        assert!(matches!(
            result,
            Err(AssetError::TooLarge { limit: 3 }) | Err(AssetError::LocalCorruption)
        ));
    }

    #[test]
    fn bounded_snapshot_keeps_the_opened_inode_when_path_is_replaced() {
        let directory = tempfile::TempDir::new().expect("temporary directory");
        let path = directory.path().join("object");
        let replacement = directory.path().join("replacement");
        fs::write(&path, b"good").expect("write object");
        fs::write(&replacement, b"evil").expect("write replacement");
        let snapshot = read_file_snapshot_with_hook(&path, 4, || {
            fs::rename(&replacement, &path).expect("replace path");
        })
        .expect("read stable opened inode");
        assert_eq!(snapshot.bytes, b"good");
    }

    #[test]
    fn same_generation_reservation_underflow_is_reported() {
        let workspace = tempfile::TempDir::new().expect("temporary workspace");
        let store = AssetStore::open(workspace.path(), "local:test", test_limits(10))
            .expect("open asset store");
        let reservation = store.reserve(5).expect("reserve bytes");
        lock(&store.state.accounting).reserved = 0;
        assert!(matches!(
            reservation.release(),
            Err(AssetError::Invariant("asset reservation release underflow"))
        ));
    }

    #[test]
    fn cached_usage_waits_for_generation_transition_and_rejects_uninitialized_state() {
        let workspace = tempfile::TempDir::new().expect("temporary workspace");
        let service = AssetService::new(test_limits(10));
        let store = service
            .open_store(workspace.path(), "local:test")
            .expect("open asset store");
        let state = Arc::clone(&store.state);
        let operation = write_lock(&state.operation_gate);
        {
            let mut accounting = lock(&state.accounting);
            accounting.generation += 1;
            accounting.initialized = false;
        }

        let (started_tx, started_rx) = std::sync::mpsc::channel();
        let (result_tx, result_rx) = std::sync::mpsc::channel();
        std::thread::scope(|scope| {
            scope.spawn(|| {
                started_tx.send(()).expect("signal cached usage start");
                result_tx
                    .send(service.cached_usage(workspace.path()))
                    .expect("send cached usage result");
            });
            started_rx.recv().expect("wait for cached usage start");
            assert!(result_rx.recv_timeout(Duration::from_millis(100)).is_err());
            drop(operation);
            assert_eq!(
                result_rx
                    .recv_timeout(Duration::from_secs(1))
                    .expect("cached usage must finish after transition"),
                None
            );
        });
    }

    fn test_limits(quota: u64) -> AssetLimits {
        AssetLimits {
            workspace_quota_bytes: quota,
            min_free_bytes: 0,
            max_file_bytes: 1_000,
            max_request_bytes: 1_000,
            max_files: 10,
            temp_ttl: Duration::from_secs(60),
            upload_slots: 2,
            peer_slots: 4,
        }
    }
}
