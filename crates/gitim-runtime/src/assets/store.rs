use super::{inspect_bytes, AssetError};
use chrono::Utc;
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::collections::HashMap;
use std::fs::{self, File, OpenOptions};
use std::io::{Read, Write};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, AtomicUsize, Ordering};
use std::sync::{Arc, Mutex, MutexGuard};
use std::time::{Duration, SystemTime, UNIX_EPOCH};
use tempfile::NamedTempFile;
use tokio::sync::Semaphore;

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

#[derive(Debug, Clone)]
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

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum AssetSource {
    LocalUpload,
    FleetReplica { origin_runtime_id: String },
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
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

#[derive(Debug, Serialize, Deserialize)]
struct StoreManifest {
    schema_version: u32,
    namespace: String,
}

pub struct AssetService {
    pub upload_slots: Arc<Semaphore>,
    pub peer_slots: Arc<Semaphore>,
    workspaces: Mutex<HashMap<PathBuf, Arc<WorkspaceAssetState>>>,
    pub store_failures: AtomicU64,
    pub hash_mismatches: AtomicU64,
    pub fleet_fetch_failures: AtomicU64,
    pub limits: AssetLimits,
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
        let workspace_root = workspace_root.as_ref().to_path_buf();
        let workspace_state = {
            let mut cache = lock(&self.workspaces);
            cache
                .entry(workspace_root.clone())
                .or_insert_with(|| Arc::new(WorkspaceAssetState::default()))
                .clone()
        };
        AssetStore::open_with_state(
            workspace_root,
            binding.into(),
            self.limits.clone(),
            workspace_state,
        )
    }

    pub fn cached_usage(&self, workspace_root: impl AsRef<Path>) -> Option<AssetUsage> {
        let cache = lock(&self.workspaces);
        cache
            .get(workspace_root.as_ref())
            .map(|state| *lock(&state.usage))
    }
}

impl Default for AssetService {
    fn default() -> Self {
        Self::new(AssetLimits::default())
    }
}

pub struct AssetStore {
    workspace_root: PathBuf,
    root: PathBuf,
    binding: String,
    limits: AssetLimits,
    state: Arc<WorkspaceAssetState>,
}

#[derive(Default)]
struct WorkspaceAssetState {
    usage: Mutex<AssetUsage>,
    reserved: Mutex<u64>,
    binding_lock: Mutex<()>,
}

impl AssetStore {
    pub fn open(
        workspace_root: impl AsRef<Path>,
        binding: impl Into<String>,
        limits: AssetLimits,
    ) -> Result<Self, AssetError> {
        Self::open_with_state(
            workspace_root.as_ref().to_path_buf(),
            binding.into(),
            limits,
            Arc::new(WorkspaceAssetState::default()),
        )
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
        prepare_namespace(&workspace_root, &root, &binding)?;
        let store = Self {
            workspace_root,
            root,
            binding,
            limits,
            state,
        };
        store.recover()?;
        Ok(store)
    }

    pub fn root(&self) -> &Path {
        &self.root
    }

    pub fn usage(&self) -> AssetUsage {
        *lock(&self.state.usage)
    }

    pub fn reserved_bytes(&self) -> u64 {
        *lock(&self.state.reserved)
    }

    pub fn object_path(&self, hash: &str) -> Result<PathBuf, AssetError> {
        self.ensure_binding()?;
        self.raw_object_path(hash)
    }

    pub fn metadata_path(&self, hash: &str) -> Result<PathBuf, AssetError> {
        self.ensure_binding()?;
        self.raw_metadata_path(hash)
    }

    pub fn lock_path(&self, hash: &str) -> Result<PathBuf, AssetError> {
        self.ensure_binding()?;
        validate_hash(hash)?;
        let shard = self.root.join("locks/sha256").join(&hash[..2]);
        validate_hash_shard(&shard)?;
        Ok(shard.join(format!("{hash}.lock")))
    }

    pub fn reserve(&self, incoming: u64) -> Result<AssetReservation, AssetError> {
        self.ensure_binding()?;
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
        let used = self.usage().bytes;
        let mut reserved = lock(&self.state.reserved);
        let committed_and_reserved =
            used.checked_add(*reserved)
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
        let pending = reserved
            .checked_add(incoming)
            .ok_or(AssetError::QuotaExceeded {
                used: committed_and_reserved,
                quota: self.limits.workspace_quota_bytes,
            })?;
        self.check_free_space(pending, committed_and_reserved, available, total)?;
        *reserved = pending;
        Ok(AssetReservation {
            bytes: incoming,
            state: Arc::clone(&self.state),
            released: false,
        })
    }

    pub fn put_bytes(
        &self,
        bytes: &[u8],
        source: AssetSource,
    ) -> Result<AssetMetadata, AssetError> {
        self.ensure_binding()?;
        let size = u64::try_from(bytes.len()).map_err(|_| AssetError::TooLarge {
            limit: self.limits.max_file_bytes,
        })?;
        if size > self.limits.max_file_bytes {
            return Err(AssetError::TooLarge {
                limit: self.limits.max_file_bytes,
            });
        }
        let hash = sha256_hex(bytes);
        match self.refresh_metadata(&hash) {
            Ok(metadata) => return Ok(metadata),
            Err(AssetError::Missing | AssetError::HashMismatch) => {}
            Err(error) => return Err(error),
        }

        let reservation = self.reserve(size)?;
        let object_path = self.raw_object_path(&hash)?;
        create_private_dir(
            object_path
                .parent()
                .ok_or_else(|| invalid_path(&object_path))?,
        )?;
        atomic_write(&object_path, bytes)?;
        let metadata = self.metadata_from_object(&hash, source)?;
        self.write_metadata(&metadata)?;
        {
            let mut usage = lock(&self.state.usage);
            usage.bytes = usage
                .bytes
                .checked_add(size)
                .ok_or(AssetError::QuotaExceeded {
                    used: usage.bytes,
                    quota: self.limits.workspace_quota_bytes,
                })?;
            usage.objects = usage.objects.checked_add(1).ok_or_else(|| {
                AssetError::Store(std::io::Error::other("asset object count overflow"))
            })?;
        }
        drop(reservation);
        Ok(metadata)
    }

    pub fn read(&self, hash: &str) -> Result<Vec<u8>, AssetError> {
        self.ensure_binding()?;
        self.refresh_metadata(hash)?;
        let path = self.raw_object_path(hash)?;
        read_regular_file(&path)
    }

    pub fn inspect(&self, hash: &str) -> Result<AssetMetadata, AssetError> {
        self.ensure_binding()?;
        self.refresh_metadata(hash)
    }

    pub fn create_owned_temp(&self) -> Result<PathBuf, AssetError> {
        self.ensure_binding()?;
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

    pub fn recover(&self) -> Result<AssetUsage, AssetError> {
        self.ensure_binding()?;
        self.cleanup_owned_temps()?;
        let usage = self.recover_objects()?;
        self.remove_orphan_sidecars()?;
        *lock(&self.state.usage) = usage;
        Ok(usage)
    }

    fn ensure_binding(&self) -> Result<(), AssetError> {
        if manifest_matches(&self.root.join("store.json"), &self.binding) {
            validate_store_layout(&self.root)?;
            return Ok(());
        }
        let _guard = lock(&self.state.binding_lock);
        if manifest_matches(&self.root.join("store.json"), &self.binding) {
            validate_store_layout(&self.root)?;
            return Ok(());
        }
        prepare_namespace(&self.workspace_root, &self.root, &self.binding)?;
        *lock(&self.state.usage) = AssetUsage::default();
        Ok(())
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
            self.quarantine_corrupt(hash, &object_path, object_metadata.len())?;
            return Err(AssetError::TooLarge {
                limit: self.limits.max_file_bytes,
            });
        }
        let modified_ns = modified_ns(&object_metadata)?;
        let metadata_path = self.raw_metadata_path(hash)?;
        if let Ok(sidecar) = read_sidecar(&metadata_path) {
            if valid_sidecar(&sidecar, hash, object_metadata.len(), modified_ns) {
                set_path_file_mode(&metadata_path)?;
                return Ok(sidecar);
            }
        }

        let bytes = read_regular_file(&object_path)?;
        if sha256_hex(&bytes) != hash {
            self.quarantine_corrupt(hash, &object_path, object_metadata.len())?;
            return Err(AssetError::HashMismatch);
        }
        let source = read_sidecar(&metadata_path)
            .ok()
            .map(|sidecar| sidecar.source)
            .unwrap_or(AssetSource::LocalUpload);
        let metadata = metadata_for_bytes(hash, &bytes, modified_ns, source)?;
        self.write_metadata(&metadata)?;
        Ok(metadata)
    }

    fn metadata_from_object(
        &self,
        hash: &str,
        source: AssetSource,
    ) -> Result<AssetMetadata, AssetError> {
        let object_path = self.raw_object_path(hash)?;
        let object_metadata = fs::symlink_metadata(&object_path)?;
        let bytes = read_regular_file(&object_path)?;
        metadata_for_bytes(hash, &bytes, modified_ns(&object_metadata)?, source)
    }

    fn write_metadata(&self, metadata: &AssetMetadata) -> Result<(), AssetError> {
        let path = self.raw_metadata_path(&metadata.sha256)?;
        let parent = path.parent().ok_or_else(|| invalid_path(&path))?;
        create_private_dir(parent)?;
        let bytes = serde_json::to_vec_pretty(metadata)
            .map_err(|error| std::io::Error::other(format!("serialize asset metadata: {error}")))?;
        atomic_write(&path, &bytes)
    }

    fn recover_objects(&self) -> Result<AssetUsage, AssetError> {
        let mut usage = AssetUsage::default();
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
                    Ok(metadata) => {
                        usage.bytes = usage.bytes.checked_add(metadata.size).ok_or_else(|| {
                            AssetError::Store(std::io::Error::other("asset usage overflow"))
                        })?;
                        usage.objects = usage.objects.checked_add(1).ok_or_else(|| {
                            AssetError::Store(std::io::Error::other("asset object count overflow"))
                        })?;
                    }
                    Err(
                        AssetError::HashMismatch
                        | AssetError::Missing
                        | AssetError::TooLarge { .. },
                    ) => {}
                    Err(error) => return Err(error),
                }
            }
        }
        Ok(usage)
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

    fn quarantine_corrupt(
        &self,
        hash: &str,
        object_path: &Path,
        object_size: u64,
    ) -> Result<(), AssetError> {
        let root = self
            .workspace_root
            .join(".gitim-runtime/orphaned-assets/corrupt-objects");
        create_private_dir(&root)?;
        let destination = unique_path(&root, &format!("corrupt-{hash}"));
        fs::rename(object_path, destination)?;
        let metadata_path = self.raw_metadata_path(hash)?;
        remove_file_if_exists(&metadata_path)?;
        let mut usage = lock(&self.state.usage);
        if usage.objects > 0 && usage.bytes >= object_size {
            usage.objects -= 1;
            usage.bytes -= object_size;
        }
        Ok(())
    }
}

pub struct AssetReservation {
    bytes: u64,
    state: Arc<WorkspaceAssetState>,
    released: bool,
}

impl AssetReservation {
    pub fn release(mut self) {
        self.release_inner();
    }

    fn release_inner(&mut self) {
        if self.released {
            return;
        }
        let mut reserved = lock(&self.state.reserved);
        *reserved = reserved.saturating_sub(self.bytes);
        self.released = true;
    }
}

impl Drop for AssetReservation {
    fn drop(&mut self) {
        self.release_inner();
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
    let destination = unique_path(&orphaned, "assets-v1");
    fs::rename(root, destination)?;
    sync_parent_best_effort(&orphaned);
    Ok(())
}

fn unique_path(parent: &Path, label: &str) -> PathBuf {
    loop {
        let nanos = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap_or_default()
            .as_nanos();
        let sequence = UNIQUE_SUFFIX.fetch_add(1, Ordering::Relaxed);
        let candidate = parent.join(format!("{label}-{nanos}-{}-{sequence}", std::process::id()));
        if fs::symlink_metadata(&candidate).is_err() {
            return candidate;
        }
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
    let file = open_options_no_follow(path, true)?;
    set_file_mode(&file)
}

fn atomic_write(path: &Path, bytes: &[u8]) -> Result<(), AssetError> {
    let parent = path.parent().ok_or_else(|| invalid_path(path))?;
    create_private_dir(parent)?;
    let mut temp = NamedTempFile::new_in(parent)?;
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

fn valid_sidecar(sidecar: &AssetMetadata, hash: &str, size: u64, modified_ns: u64) -> bool {
    let dimensions_valid = match (sidecar.width, sidecar.height) {
        (None, None) => true,
        (Some(width), Some(height)) => width > 0 && height > 0,
        _ => false,
    };
    let source_valid = match &sidecar.source {
        AssetSource::LocalUpload => true,
        AssetSource::FleetReplica { origin_runtime_id } => {
            !origin_runtime_id.trim().is_empty() && origin_runtime_id.len() <= 128
        }
    };
    sidecar.schema_version == SIDECAR_SCHEMA_VERSION
        && sidecar.sha256 == hash
        && sidecar.size == size
        && sidecar.object_modified_ns == modified_ns
        && sidecar.media_type.len() <= 127
        && sidecar.media_type.parse::<mime::Mime>().is_ok()
        && chrono::DateTime::parse_from_rfc3339(&sidecar.stored_at).is_ok()
        && dimensions_valid
        && source_valid
}

fn metadata_for_bytes(
    hash: &str,
    bytes: &[u8],
    object_modified_ns: u64,
    source: AssetSource,
) -> Result<AssetMetadata, AssetError> {
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

fn read_regular_file(path: &Path) -> Result<Vec<u8>, AssetError> {
    let mut file = open_options_no_follow(path, false).map_err(map_missing)?;
    let metadata = file.metadata()?;
    if !metadata.file_type().is_file() {
        return Err(AssetError::Missing);
    }
    let capacity = usize::try_from(metadata.len())
        .map_err(|_| AssetError::Invalid("asset size cannot fit address space".to_string()))?;
    let mut bytes = Vec::with_capacity(capacity);
    file.read_to_end(&mut bytes)?;
    Ok(bytes)
}

fn read_bounded_regular_file(path: &Path, limit: u64) -> Result<Vec<u8>, AssetError> {
    let mut file = open_options_no_follow(path, false).map_err(map_missing)?;
    let metadata = file.metadata()?;
    if !metadata.file_type().is_file() || metadata.len() > limit {
        return Err(AssetError::Invalid(format!(
            "asset metadata file exceeds the {limit}-byte limit or is not regular"
        )));
    }
    let capacity = usize::try_from(metadata.len()).map_err(|_| {
        AssetError::Invalid("asset metadata size cannot fit address space".to_string())
    })?;
    let mut bytes = Vec::with_capacity(capacity);
    file.read_to_end(&mut bytes)?;
    Ok(bytes)
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

fn invalid_path(path: &Path) -> AssetError {
    AssetError::Invalid(format!("asset path has no parent: {}", path.display()))
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

fn lock<T>(mutex: &Mutex<T>) -> MutexGuard<'_, T> {
    mutex
        .lock()
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
}
