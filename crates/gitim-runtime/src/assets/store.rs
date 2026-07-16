use super::{inspect_bytes, AssetError};
use axum::body::Bytes;
use chrono::Utc;
use fs2::FileExt;
use futures::{Stream, StreamExt};
use gitim_core::types::{AssetRef, ASSET_REF_VERSION};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::collections::{HashMap, HashSet};
use std::fs::{self, File, OpenOptions};
use std::io::{Read, Write};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, AtomicU64, AtomicUsize, Ordering};
#[cfg(feature = "test-support")]
use std::sync::Barrier;
use std::sync::{
    Arc, Condvar, Mutex, MutexGuard, OnceLock, RwLock, RwLockReadGuard, RwLockWriteGuard, Weak,
};
use std::time::{Duration, SystemTime, UNIX_EPOCH};
use tokio::io::AsyncWriteExt;
use tokio::sync::{Notify, OwnedSemaphorePermit, Semaphore};

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
const MAX_INSPECTION_PREFIX_BYTES: usize = 64 * 1024;
static UNIQUE_SUFFIX: AtomicUsize = AtomicUsize::new(0);
static WORKSPACE_STATES: OnceLock<Mutex<HashMap<PathBuf, Weak<WorkspaceAssetState>>>> =
    OnceLock::new();
#[cfg(feature = "test-support")]
type AssetEventObserver = Arc<dyn Fn(AssetEvent) + Send + Sync>;
#[cfg(feature = "test-support")]
type AssetTestHook = Arc<dyn Fn() + Send + Sync>;

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

#[derive(Debug, Default, Clone, Copy, PartialEq, Eq, Serialize)]
pub struct AssetHealthSnapshot {
    pub bytes: u64,
    pub objects: u64,
    pub quota_bytes: u64,
}

#[derive(Debug, Clone)]
pub struct AssetWorkspaceToken {
    id: uuid::Uuid,
    revoked: Arc<AtomicBool>,
}

impl AssetWorkspaceToken {
    pub fn new() -> Self {
        Self {
            id: uuid::Uuid::new_v4(),
            revoked: Arc::new(AtomicBool::new(false)),
        }
    }

    fn revoke(&self) {
        self.revoked.store(true, Ordering::Release);
    }

    fn is_revoked(&self) -> bool {
        self.revoked.load(Ordering::Acquire)
    }
}

impl PartialEq for AssetWorkspaceToken {
    fn eq(&self, other: &Self) -> bool {
        self.id == other.id
    }
}

impl Eq for AssetWorkspaceToken {}

impl std::hash::Hash for AssetWorkspaceToken {
    fn hash<H: std::hash::Hasher>(&self, state: &mut H) {
        std::hash::Hash::hash(&self.id, state);
    }
}

#[cfg(feature = "test-support")]
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AssetEvent {
    pub event: &'static str,
    pub workspace_slug: String,
    pub hash_prefix: Option<String>,
    pub bytes: Option<u64>,
    pub origin_runtime_id: Option<String>,
    pub error_code: Option<&'static str>,
}

impl Default for AssetWorkspaceToken {
    fn default() -> Self {
        Self::new()
    }
}

#[derive(Debug, Default, Clone, Copy, PartialEq, Eq)]
pub struct RequestBudget {
    bytes: u64,
    files: usize,
}

impl RequestBudget {
    pub const fn bytes(&self) -> u64 {
        self.bytes
    }

    pub const fn files(&self) -> usize {
        self.files
    }

    fn begin_file(&mut self, limit: usize) -> Result<(), AssetError> {
        let files = self
            .files
            .checked_add(1)
            .ok_or(AssetError::TooMany { limit })?;
        if files > limit {
            return Err(AssetError::TooMany { limit });
        }
        self.files = files;
        Ok(())
    }

    fn add_bytes(&mut self, incoming: u64, limit: u64) -> Result<(), AssetError> {
        let bytes = self
            .bytes
            .checked_add(incoming)
            .ok_or(AssetError::RequestTooLarge { limit })?;
        if bytes > limit {
            return Err(AssetError::RequestTooLarge { limit });
        }
        self.bytes = bytes;
        Ok(())
    }
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

struct FileDigestSnapshot {
    sha256: String,
    inspection_prefix: Vec<u8>,
    size: u64,
    modified_ns: u64,
}

enum DedupeOutcome {
    Missing,
    Corrupt,
    Present(AssetMetadata),
}

#[derive(Debug, Serialize, Deserialize)]
struct StoreManifest {
    schema_version: u32,
    namespace: String,
}

#[cfg(feature = "test-support")]
struct MaterializePause {
    reached: Arc<Barrier>,
    resume: Arc<Barrier>,
    materialized: Arc<AtomicBool>,
}

#[cfg(feature = "test-support")]
struct FreeSpaceSampleWait {
    sampled: Arc<Barrier>,
    materialized: Arc<AtomicBool>,
}

#[cfg(feature = "test-support")]
struct LocalVerificationPause {
    remaining: usize,
    reached: Arc<Barrier>,
    resume: Arc<Barrier>,
}

pub struct AssetService {
    upload_slots: Arc<Semaphore>,
    peer_slots: Arc<Semaphore>,
    workspaces: Mutex<HashMap<PathBuf, WorkspaceCacheEntry>>,
    health_workspaces: Mutex<HashMap<PathBuf, Weak<WorkspaceAssetState>>>,
    lifecycle: Mutex<WorkspaceLifecycleRegistry>,
    fleet_discoveries: Mutex<HashSet<(String, String)>>,
    #[cfg(feature = "test-support")]
    before_registered_open_pause: Mutex<Option<(Arc<Barrier>, Arc<Barrier>)>>,
    #[cfg(feature = "test-support")]
    deactivate_attempt: Mutex<Option<Arc<Barrier>>>,
    #[cfg(feature = "test-support")]
    activation_transition_pause: Mutex<Option<(Arc<Barrier>, Arc<Barrier>)>>,
    #[cfg(feature = "test-support")]
    deactivation_transition_pause: Mutex<Option<(Arc<Barrier>, Arc<Barrier>)>>,
    #[cfg(feature = "test-support")]
    registered_open_attempt: Mutex<Option<Arc<Barrier>>>,
    #[cfg(feature = "test-support")]
    registered_open_wait_attempt: Mutex<Option<Arc<Barrier>>>,
    #[cfg(feature = "test-support")]
    activation_wait_attempt: Mutex<Option<Arc<Barrier>>>,
    #[cfg(feature = "test-support")]
    fail_after_activation_transition: AtomicBool,
    #[cfg(feature = "test-support")]
    rollback_config_pause: Mutex<Option<(Arc<Barrier>, Arc<Barrier>)>>,
    #[cfg(feature = "test-support")]
    before_persist_pause: Mutex<Option<(Arc<tokio::sync::Barrier>, Arc<tokio::sync::Barrier>)>>,
    #[cfg(feature = "test-support")]
    after_activation_pause: Mutex<Option<(Arc<tokio::sync::Barrier>, Arc<tokio::sync::Barrier>)>>,
    #[cfg(feature = "test-support")]
    after_config_write_hook: Mutex<Option<AssetTestHook>>,
    #[cfg(feature = "test-support")]
    event_observer: Mutex<Option<AssetEventObserver>>,
    #[cfg(feature = "test-support")]
    fail_next_activation_store: AtomicBool,
    #[cfg(feature = "test-support")]
    fail_next_activation_invariant: AtomicBool,
    #[cfg(feature = "test-support")]
    health_snapshot_attempt: Mutex<Option<Arc<Barrier>>>,
    #[cfg(feature = "test-support")]
    fallback_probe_successes: AtomicUsize,
    #[cfg(feature = "test-support")]
    fallback_probe_windows_completed: AtomicUsize,
    pub store_failures: AtomicU64,
    pub hash_mismatches: AtomicU64,
    pub fleet_fetch_failures: AtomicU64,
    pub limits: AssetLimits,
}

#[derive(Default)]
struct WorkspaceLifecycleRegistry {
    // Path entries are discovery indexes. Token entries own active and pending
    // lifecycle slots until their transition has fully completed.
    paths: HashMap<PathBuf, Weak<WorkspaceLifecycleSlot>>,
    tokens: HashMap<uuid::Uuid, Arc<WorkspaceLifecycleSlot>>,
}

struct WorkspaceLifecycleSlot {
    canonical_root: PathBuf,
    transition: Mutex<WorkspaceLifecycleState>,
    changed: Condvar,
}

#[derive(Default)]
struct WorkspaceLifecycleState {
    active: Option<RegisteredWorkspace>,
    deactivating: bool,
}

#[derive(Clone)]
struct RegisteredWorkspace {
    requested_root: PathBuf,
    binding: String,
    token: AssetWorkspaceToken,
    store: AssetStore,
}

struct DeactivationClaim {
    slot: Arc<WorkspaceLifecycleSlot>,
    armed: bool,
}

impl DeactivationClaim {
    fn finish(mut self) {
        self.clear();
    }

    fn clear(&mut self) {
        if !self.armed {
            return;
        }
        lock(&self.slot.transition).deactivating = false;
        self.slot.changed.notify_all();
        self.armed = false;
    }
}

impl Drop for DeactivationClaim {
    fn drop(&mut self) {
        self.clear();
    }
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
            health_workspaces: Mutex::new(HashMap::new()),
            lifecycle: Mutex::new(WorkspaceLifecycleRegistry::default()),
            fleet_discoveries: Mutex::new(HashSet::new()),
            #[cfg(feature = "test-support")]
            before_registered_open_pause: Mutex::new(None),
            #[cfg(feature = "test-support")]
            deactivate_attempt: Mutex::new(None),
            #[cfg(feature = "test-support")]
            activation_transition_pause: Mutex::new(None),
            #[cfg(feature = "test-support")]
            deactivation_transition_pause: Mutex::new(None),
            #[cfg(feature = "test-support")]
            registered_open_attempt: Mutex::new(None),
            #[cfg(feature = "test-support")]
            registered_open_wait_attempt: Mutex::new(None),
            #[cfg(feature = "test-support")]
            activation_wait_attempt: Mutex::new(None),
            #[cfg(feature = "test-support")]
            fail_after_activation_transition: AtomicBool::new(false),
            #[cfg(feature = "test-support")]
            rollback_config_pause: Mutex::new(None),
            #[cfg(feature = "test-support")]
            before_persist_pause: Mutex::new(None),
            #[cfg(feature = "test-support")]
            after_activation_pause: Mutex::new(None),
            #[cfg(feature = "test-support")]
            after_config_write_hook: Mutex::new(None),
            #[cfg(feature = "test-support")]
            event_observer: Mutex::new(None),
            #[cfg(feature = "test-support")]
            fail_next_activation_store: AtomicBool::new(false),
            #[cfg(feature = "test-support")]
            fail_next_activation_invariant: AtomicBool::new(false),
            #[cfg(feature = "test-support")]
            health_snapshot_attempt: Mutex::new(None),
            #[cfg(feature = "test-support")]
            fallback_probe_successes: AtomicUsize::new(0),
            #[cfg(feature = "test-support")]
            fallback_probe_windows_completed: AtomicUsize::new(0),
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
        let requested_root = workspace_root.as_ref().to_path_buf();
        let workspace_root = canonical_workspace_root(&requested_root)?;
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
            let mut health_workspaces = lock(&self.health_workspaces);
            health_workspaces.insert(requested_root, Arc::downgrade(&store.state));
            health_workspaces.insert(store.workspace_root.clone(), Arc::downgrade(&store.state));
        }
        Ok(store)
    }

    fn lifecycle_slot_for_activation(
        &self,
        canonical_root: &Path,
        token: &AssetWorkspaceToken,
    ) -> Arc<WorkspaceLifecycleSlot> {
        let mut registry = lock(&self.lifecycle);
        if let Some(slot) = registry.tokens.get(&token.id) {
            return Arc::clone(slot);
        }
        let slot = registry
            .paths
            .get(canonical_root)
            .and_then(Weak::upgrade)
            .unwrap_or_else(|| {
                let slot = Arc::new(WorkspaceLifecycleSlot {
                    canonical_root: canonical_root.to_path_buf(),
                    transition: Mutex::new(WorkspaceLifecycleState::default()),
                    changed: Condvar::new(),
                });
                registry
                    .paths
                    .insert(canonical_root.to_path_buf(), Arc::downgrade(&slot));
                slot
            });
        registry.tokens.insert(token.id, Arc::clone(&slot));
        slot
    }

    fn lifecycle_slot_for_token(
        &self,
        token: &AssetWorkspaceToken,
    ) -> Option<Arc<WorkspaceLifecycleSlot>> {
        lock(&self.lifecycle).tokens.get(&token.id).cloned()
    }

    fn release_lifecycle_slot_after_transition(
        &self,
        slot: Arc<WorkspaceLifecycleSlot>,
        token: &AssetWorkspaceToken,
    ) {
        {
            let mut registry = lock(&self.lifecycle);
            if registry
                .tokens
                .get(&token.id)
                .is_some_and(|registered| Arc::ptr_eq(registered, &slot))
            {
                registry.tokens.remove(&token.id);
            }
        }
        self.prune_lifecycle_slot_after_transition(slot);
    }

    fn prune_lifecycle_slot_after_transition(&self, slot: Arc<WorkspaceLifecycleSlot>) {
        // Dropping this caller's owner may expire the path index. A concurrent
        // activation either keeps this slot alive or replaces the dead weak
        // entry; pointer identity preserves either handoff.
        let canonical_root = slot.canonical_root.clone();
        let expected = Arc::downgrade(&slot);
        drop(slot);
        let mut registry = lock(&self.lifecycle);
        if registry
            .paths
            .get(&canonical_root)
            .is_some_and(|registered| {
                registered.ptr_eq(&expected) && registered.upgrade().is_none()
            })
        {
            registry.paths.remove(&canonical_root);
        }
    }

    #[cfg(feature = "test-support")]
    #[doc(hidden)]
    pub fn lifecycle_path_is_indexed(&self, workspace_root: impl AsRef<Path>) -> bool {
        let Ok(canonical_root) = canonical_workspace_root(workspace_root.as_ref()) else {
            return false;
        };
        lock(&self.lifecycle)
            .paths
            .get(&canonical_root)
            .is_some_and(|slot| slot.upgrade().is_some())
    }

    #[cfg(feature = "test-support")]
    #[doc(hidden)]
    pub fn lifecycle_tokens_share_slot(
        &self,
        first: &AssetWorkspaceToken,
        second: &AssetWorkspaceToken,
    ) -> bool {
        let registry = lock(&self.lifecycle);
        let (Some(first), Some(second)) = (
            registry.tokens.get(&first.id),
            registry.tokens.get(&second.id),
        ) else {
            return false;
        };
        Arc::ptr_eq(first, second)
    }

    fn invalidate_workspace_store(&self, store: &AssetStore) -> Result<(), AssetError> {
        let _operation = write_lock(&store.state.operation_gate);
        {
            let mut accounting = lock(&store.state.accounting);
            accounting.generation = accounting
                .generation
                .checked_add(1)
                .ok_or(AssetError::Invariant("asset store generation overflow"))?;
            accounting.active_binding = None;
            accounting.initialized = false;
            accounting.committed = AssetUsage::default();
            accounting.reserved = 0;
            accounting.unmaterialized = 0;
            accounting.objects.clear();
            store.state.health.publish(AssetUsage::default());
        }
        lock(&self.workspaces).retain(|_, entry| !Arc::ptr_eq(&entry.state, &store.state));
        lock(&self.health_workspaces).retain(|_, state| {
            state
                .upgrade()
                .is_some_and(|state| !Arc::ptr_eq(&state, &store.state))
        });
        Ok(())
    }

    pub fn activate_workspace(
        &self,
        workspace_root: impl AsRef<Path>,
        binding: impl Into<String>,
        token: &AssetWorkspaceToken,
    ) -> Result<AssetStore, AssetError> {
        let requested_root = workspace_root.as_ref().to_path_buf();
        let binding = binding.into();
        #[cfg(feature = "test-support")]
        if self
            .fail_next_activation_store
            .swap(false, Ordering::AcqRel)
        {
            return Err(AssetError::Store(std::io::Error::other(
                "injected asset activation failure",
            )));
        }
        #[cfg(feature = "test-support")]
        if self
            .fail_next_activation_invariant
            .swap(false, Ordering::AcqRel)
        {
            return Err(AssetError::Invariant(
                "injected asset activation invariant failure",
            ));
        }
        let canonical_root = canonical_workspace_root(&requested_root)?;
        if token.is_revoked() {
            return Err(AssetError::StaleBinding);
        }
        let slot = self.lifecycle_slot_for_activation(&canonical_root, token);
        #[cfg(feature = "test-support")]
        if let Some(attempted) = lock(&self.activation_wait_attempt).take() {
            attempted.wait();
        }
        let mut lifecycle = lock(&slot.transition);
        while lifecycle.deactivating {
            lifecycle = wait_condvar(&slot.changed, lifecycle);
        }
        #[cfg(feature = "test-support")]
        let activation_pause = lock(&self.activation_transition_pause).take();
        #[cfg(feature = "test-support")]
        if let Some((reached, resume)) = activation_pause {
            reached.wait();
            resume.wait();
        }
        if slot.canonical_root != canonical_root {
            drop(lifecycle);
            self.prune_lifecycle_slot_after_transition(slot);
            return Err(AssetError::Invariant(
                "asset workspace token is already bound to another namespace",
            ));
        }
        if let Some(registered) = lifecycle.active.as_ref() {
            if registered.token.id == token.id
                && registered.requested_root == canonical_root
                && registered.binding == binding
            {
                return Ok(registered.store.clone());
            }
            if registered.token.id != token.id {
                drop(lifecycle);
                self.release_lifecycle_slot_after_transition(slot, token);
            }
            return Err(AssetError::Invariant(
                "asset workspace path is already active under another token",
            ));
        }
        if token.is_revoked() {
            drop(lifecycle);
            self.release_lifecycle_slot_after_transition(slot, token);
            return Err(AssetError::StaleBinding);
        }
        #[cfg(feature = "test-support")]
        let injected_failure = self
            .fail_after_activation_transition
            .swap(false, Ordering::AcqRel);
        #[cfg(not(feature = "test-support"))]
        let injected_failure = false;
        let store = match if injected_failure {
            Err(AssetError::Store(std::io::Error::other(
                "injected asset activation failure after transition",
            )))
        } else {
            self.open_store(&canonical_root, binding.clone())
        } {
            Ok(store) => store,
            Err(error) => {
                drop(lifecycle);
                self.release_lifecycle_slot_after_transition(slot, token);
                return Err(error);
            }
        };
        if token.is_revoked() {
            let invalidation = self.invalidate_workspace_store(&store);
            drop(lifecycle);
            self.release_lifecycle_slot_after_transition(slot, token);
            invalidation?;
            return Err(AssetError::StaleBinding);
        }
        lifecycle.active = Some(RegisteredWorkspace {
            requested_root: canonical_root,
            binding,
            token: token.clone(),
            store: store.clone(),
        });
        Ok(store)
    }

    pub fn open_registered_store(
        &self,
        workspace_root: impl AsRef<Path>,
        binding: &str,
        token: &AssetWorkspaceToken,
    ) -> Result<AssetStore, AssetError> {
        #[cfg(feature = "test-support")]
        if let Some((reached, resume)) = lock(&self.before_registered_open_pause).take() {
            reached.wait();
            resume.wait();
        }
        #[cfg(feature = "test-support")]
        if let Some(attempted) = lock(&self.registered_open_attempt).take() {
            attempted.wait();
        }
        if token.is_revoked() {
            return Err(AssetError::StaleBinding);
        }
        let canonical_root = canonical_workspace_root(workspace_root.as_ref())?;
        let slot = self
            .lifecycle_slot_for_token(token)
            .ok_or(AssetError::StaleBinding)?;
        let result = (|| {
            let registered = {
                let mut lifecycle = lock(&slot.transition);
                while lifecycle.deactivating {
                    #[cfg(feature = "test-support")]
                    if let Some(attempted) = lock(&self.registered_open_wait_attempt).take() {
                        attempted.wait();
                    }
                    lifecycle = wait_condvar(&slot.changed, lifecycle);
                }
                lifecycle
                    .active
                    .as_ref()
                    .filter(|registered| {
                        registered.token.id == token.id
                            && registered.requested_root == canonical_root
                            && registered.binding == binding
                    })
                    .cloned()
                    .ok_or(AssetError::StaleBinding)?
            };
            if token.is_revoked() {
                return Err(AssetError::StaleBinding);
            }
            let _operation = read_lock(&registered.store.state.operation_gate);
            registered.store.validate_generation()?;
            if token.is_revoked() {
                return Err(AssetError::StaleBinding);
            }
            Ok(registered.store.clone())
        })();
        if result.is_err() {
            self.prune_lifecycle_slot_after_transition(slot);
        }
        result
    }

    pub async fn deactivate_workspace(
        &self,
        token: &AssetWorkspaceToken,
    ) -> Result<bool, AssetError> {
        let Some(slot) = self.lifecycle_slot_for_token(token) else {
            token.revoke();
            return Ok(false);
        };
        let registered = {
            let mut lifecycle = lock(&slot.transition);
            #[cfg(feature = "test-support")]
            let deactivation_pause = lock(&self.deactivation_transition_pause).take();
            #[cfg(feature = "test-support")]
            if let Some((reached, resume)) = deactivation_pause {
                reached.wait();
                resume.wait();
            }
            if lifecycle.deactivating {
                return Ok(false);
            }
            let Some(registered) = lifecycle
                .active
                .as_ref()
                .filter(|registered| registered.token.id == token.id)
                .cloned()
            else {
                drop(lifecycle);
                token.revoke();
                self.release_lifecycle_slot_after_transition(slot, token);
                return Ok(false);
            };
            lifecycle.deactivating = true;
            registered
        };
        let claim = DeactivationClaim {
            slot: Arc::clone(&slot),
            armed: true,
        };
        #[cfg(feature = "test-support")]
        if let Some(attempted) = lock(&self.deactivate_attempt).take() {
            attempted.wait();
        }
        let persistence = registered.store.state.persistence.close().await;
        #[cfg(feature = "test-support")]
        registered.store.wait_at_persistence_transition();
        persistence.wait_for_idle().await;
        token.revoke();
        let invalidation = self.invalidate_workspace_store(&registered.store);
        lock(&slot.transition)
            .active
            .take_if(|active| active.token.id == token.id);
        drop(persistence);
        claim.finish();
        self.release_lifecycle_slot_after_transition(slot, token);
        invalidation?;
        Ok(true)
    }

    pub fn try_deactivate_workspace(
        &self,
        token: &AssetWorkspaceToken,
    ) -> Result<bool, AssetError> {
        let Some(slot) = self.lifecycle_slot_for_token(token) else {
            token.revoke();
            return Ok(false);
        };
        let mut lifecycle = lock(&slot.transition);
        if lifecycle.deactivating {
            return Err(AssetError::StaleBinding);
        }
        let Some(registered) = lifecycle
            .active
            .as_ref()
            .filter(|registered| registered.token.id == token.id)
            .cloned()
        else {
            drop(lifecycle);
            token.revoke();
            self.release_lifecycle_slot_after_transition(slot, token);
            return Ok(false);
        };
        lifecycle.deactivating = true;
        drop(lifecycle);
        let claim = DeactivationClaim {
            slot: Arc::clone(&slot),
            armed: true,
        };
        let persistence = registered.store.state.persistence.try_close()?;
        token.revoke();
        let invalidation = self.invalidate_workspace_store(&registered.store);
        lock(&slot.transition)
            .active
            .take_if(|active| active.token.id == token.id);
        drop(persistence);
        claim.finish();
        self.release_lifecycle_slot_after_transition(slot, token);
        invalidation?;
        Ok(true)
    }

    #[cfg(feature = "test-support")]
    #[doc(hidden)]
    pub fn inject_before_registered_open_pause(&self, reached: Arc<Barrier>, resume: Arc<Barrier>) {
        *lock(&self.before_registered_open_pause) = Some((reached, resume));
    }

    #[cfg(feature = "test-support")]
    #[doc(hidden)]
    pub fn inject_deactivate_attempt(&self, attempted: Arc<Barrier>) {
        *lock(&self.deactivate_attempt) = Some(attempted);
    }

    #[cfg(feature = "test-support")]
    #[doc(hidden)]
    pub fn inject_activation_transition_pause(&self, reached: Arc<Barrier>, resume: Arc<Barrier>) {
        *lock(&self.activation_transition_pause) = Some((reached, resume));
    }

    #[cfg(feature = "test-support")]
    #[doc(hidden)]
    pub fn inject_deactivation_transition_pause(
        &self,
        reached: Arc<Barrier>,
        resume: Arc<Barrier>,
    ) {
        *lock(&self.deactivation_transition_pause) = Some((reached, resume));
    }

    #[cfg(feature = "test-support")]
    #[doc(hidden)]
    pub fn inject_registered_open_attempt(&self, attempted: Arc<Barrier>) {
        *lock(&self.registered_open_attempt) = Some(attempted);
    }

    #[cfg(feature = "test-support")]
    #[doc(hidden)]
    pub fn inject_registered_open_wait_attempt(&self, attempted: Arc<Barrier>) {
        *lock(&self.registered_open_wait_attempt) = Some(attempted);
    }

    #[cfg(feature = "test-support")]
    #[doc(hidden)]
    pub fn inject_activation_wait_attempt(&self, attempted: Arc<Barrier>) {
        *lock(&self.activation_wait_attempt) = Some(attempted);
    }

    #[cfg(feature = "test-support")]
    #[doc(hidden)]
    pub fn inject_activation_failure_after_transition_once(&self) {
        self.fail_after_activation_transition
            .store(true, Ordering::Release);
    }

    #[cfg(feature = "test-support")]
    #[doc(hidden)]
    pub fn inject_rollback_config_pause(&self, reached: Arc<Barrier>, resume: Arc<Barrier>) {
        *lock(&self.rollback_config_pause) = Some((reached, resume));
    }

    #[cfg(feature = "test-support")]
    pub(crate) fn wait_before_rollback_config(&self) {
        if let Some((reached, resume)) = lock(&self.rollback_config_pause).take() {
            reached.wait();
            resume.wait();
        }
    }

    #[cfg(feature = "test-support")]
    #[doc(hidden)]
    pub fn inject_before_persist_pause(
        &self,
        reached: Arc<tokio::sync::Barrier>,
        resume: Arc<tokio::sync::Barrier>,
    ) {
        *lock(&self.before_persist_pause) = Some((reached, resume));
    }

    #[cfg(feature = "test-support")]
    pub(crate) async fn wait_before_persist(&self) {
        let pause = lock(&self.before_persist_pause).take();
        if let Some((reached, resume)) = pause {
            reached.wait().await;
            resume.wait().await;
        }
    }

    #[cfg(feature = "test-support")]
    #[doc(hidden)]
    pub fn inject_after_activation_pause(
        &self,
        reached: Arc<tokio::sync::Barrier>,
        resume: Arc<tokio::sync::Barrier>,
    ) {
        *lock(&self.after_activation_pause) = Some((reached, resume));
    }

    #[cfg(feature = "test-support")]
    pub(crate) async fn wait_after_activation(&self) {
        let pause = lock(&self.after_activation_pause).take();
        if let Some((reached, resume)) = pause {
            reached.wait().await;
            resume.wait().await;
        }
    }

    #[cfg(feature = "test-support")]
    pub(crate) fn run_after_config_write_hook(&self) {
        let hook = lock(&self.after_config_write_hook).take();
        if let Some(hook) = hook {
            hook();
        }
    }

    /// Read the health-path snapshot without filesystem access or store locks.
    /// The key is the workspace path exactly as registered with this service.
    pub fn health_snapshot(&self, workspace_root: impl AsRef<Path>) -> Option<AssetHealthSnapshot> {
        #[cfg(feature = "test-support")]
        if let Some(attempted) = lock(&self.health_snapshot_attempt).take() {
            attempted.wait();
        }
        let path = workspace_root.as_ref();
        let state = lock(&self.health_workspaces)
            .get(path)
            .and_then(Weak::upgrade)?;
        let usage = state.health.load();
        Some(AssetHealthSnapshot {
            bytes: usage.bytes,
            objects: usage.objects,
            quota_bytes: self.limits.workspace_quota_bytes,
        })
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
        let removed = lock(&self.workspaces).remove(&workspace_root);
        if let Some(entry) = removed.as_ref() {
            lock(&self.health_workspaces).retain(|_, state| {
                state
                    .upgrade()
                    .is_some_and(|state| !Arc::ptr_eq(&state, &entry.state))
            });
        }
        Ok(removed.is_some())
    }

    pub async fn acquire_upload(&self) -> Result<OwnedSemaphorePermit, AssetError> {
        Arc::clone(&self.upload_slots)
            .acquire_owned()
            .await
            .map_err(|_| AssetError::Invariant("asset upload semaphore closed"))
    }

    pub(crate) fn begin_fleet_discovery(&self, workspace: &str, identity: &str) -> bool {
        lock(&self.fleet_discoveries).insert((workspace.to_string(), identity.to_string()))
    }

    pub(crate) fn finish_fleet_discovery(&self, workspace: &str, identity: &str) {
        lock(&self.fleet_discoveries).remove(&(workspace.to_string(), identity.to_string()));
    }

    #[cfg(feature = "test-support")]
    pub(crate) fn record_fallback_probe_success(&self) {
        self.fallback_probe_successes.fetch_add(1, Ordering::AcqRel);
    }

    #[cfg(feature = "test-support")]
    #[doc(hidden)]
    pub fn fallback_probe_successes(&self) -> usize {
        self.fallback_probe_successes.load(Ordering::Acquire)
    }

    #[cfg(feature = "test-support")]
    pub(crate) fn record_fallback_probe_window_completed(&self) {
        self.fallback_probe_windows_completed
            .fetch_add(1, Ordering::AcqRel);
    }

    #[cfg(feature = "test-support")]
    #[doc(hidden)]
    pub fn fallback_probe_windows_completed(&self) -> usize {
        self.fallback_probe_windows_completed
            .load(Ordering::Acquire)
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

    pub(crate) fn record_store_failure(&self, workspace_slug: &str, error: &AssetError) {
        if !matches!(
            error,
            AssetError::Store(_)
                | AssetError::Invariant(_)
                | AssetError::StaleBinding
                | AssetError::LocalCorruption
        ) {
            return;
        }
        self.store_failures.fetch_add(1, Ordering::Relaxed);
        tracing::warn!(
            event = "asset_store_failure",
            workspace_slug,
            error_code = error.error_code(),
            "asset store operation failed"
        );
        #[cfg(feature = "test-support")]
        self.notify_event(AssetEvent {
            event: "asset_store_failure",
            workspace_slug: workspace_slug.to_string(),
            hash_prefix: None,
            bytes: None,
            origin_runtime_id: None,
            error_code: Some(error.error_code()),
        });
    }

    pub(crate) fn record_persistence(
        &self,
        workspace_slug: &str,
        asset_ref: &AssetRef,
        deduplicated: bool,
    ) {
        let event = if deduplicated {
            "asset_dedupe"
        } else {
            "asset_upload"
        };
        let hash_prefix = &asset_ref.sha256[..asset_ref.sha256.len().min(12)];
        tracing::info!(
            event,
            workspace_slug,
            hash_prefix,
            bytes = asset_ref.size,
            origin_runtime_id = asset_ref.origin_runtime_id,
            "asset persistence complete"
        );
        #[cfg(feature = "test-support")]
        self.notify_event(AssetEvent {
            event,
            workspace_slug: workspace_slug.to_string(),
            hash_prefix: Some(hash_prefix.to_string()),
            bytes: Some(asset_ref.size),
            origin_runtime_id: Some(asset_ref.origin_runtime_id.clone()),
            error_code: None,
        });
    }

    pub(crate) fn record_local_hit(
        &self,
        workspace_slug: &str,
        hash: &str,
        bytes: u64,
        origin_runtime_id: &str,
    ) {
        let hash_prefix = &hash[..hash.len().min(12)];
        tracing::info!(
            event = "asset_local_hit",
            workspace_slug,
            hash_prefix,
            bytes,
            origin_runtime_id,
            "asset local hit"
        );
        #[cfg(feature = "test-support")]
        self.notify_event(AssetEvent {
            event: "asset_local_hit",
            workspace_slug: workspace_slug.to_string(),
            hash_prefix: Some(hash_prefix.to_string()),
            bytes: Some(bytes),
            origin_runtime_id: Some(origin_runtime_id.to_string()),
            error_code: None,
        });
    }

    #[cfg(feature = "test-support")]
    fn notify_event(&self, event: AssetEvent) {
        let observer = lock(&self.event_observer).clone();
        if let Some(observer) = observer {
            observer(event);
        }
    }

    #[cfg(feature = "test-support")]
    #[doc(hidden)]
    pub fn set_event_observer(&self, observer: Arc<dyn Fn(AssetEvent) + Send + Sync>) {
        *lock(&self.event_observer) = Some(observer);
    }

    #[cfg(feature = "test-support")]
    #[doc(hidden)]
    pub fn inject_activation_store_failure_once(&self) {
        self.fail_next_activation_store
            .store(true, Ordering::Release);
    }

    #[cfg(feature = "test-support")]
    #[doc(hidden)]
    pub fn inject_activation_invariant_failure_once(&self) {
        self.fail_next_activation_invariant
            .store(true, Ordering::Release);
    }

    #[cfg(feature = "test-support")]
    #[doc(hidden)]
    pub fn inject_health_snapshot_attempt(&self, attempted: Arc<Barrier>) {
        *lock(&self.health_snapshot_attempt) = Some(attempted);
    }

    #[cfg(feature = "test-support")]
    #[doc(hidden)]
    pub fn set_after_config_write_hook(&self, hook: Arc<dyn Fn() + Send + Sync>) {
        *lock(&self.after_config_write_hook) = Some(hook);
    }

    #[cfg(feature = "test-support")]
    #[doc(hidden)]
    pub fn lifecycle_registry_entries(&self) -> (usize, usize) {
        let registry = lock(&self.lifecycle);
        (registry.paths.len(), registry.tokens.len())
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
#[derive(Clone)]
pub struct AssetStore {
    workspace_root: PathBuf,
    root: PathBuf,
    binding: String,
    limits: AssetLimits,
    state: Arc<WorkspaceAssetState>,
    generation: u64,
}

/// Generation-bound capability used by the asset HTTP layer.
///
/// The path never leaves the `assets` module. Callers validate immediately
/// before and after Tower opens the file; the second check prevents a store
/// rebind or pathname replacement from turning a previously verified lookup
/// into a response from a different namespace or inode.
pub(super) struct VerifiedLocalAsset {
    store: AssetStore,
    hash: String,
    path: PathBuf,
    metadata: AssetMetadata,
    file_identity: FileIdentity,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
struct FileIdentity {
    size: u64,
    modified_ns: u64,
    #[cfg(unix)]
    device: u64,
    #[cfg(unix)]
    inode: u64,
}

#[derive(Default)]
struct WorkspaceAssetState {
    // Generation transitions close persistence admission before operation_gate.
    // A stage lease spans tempfile creation through staged cleanup.
    accounting: Mutex<AccountingState>,
    persistence: Arc<PersistenceLifecycle>,
    operation_gate: RwLock<()>,
    health: HealthUsageCache,
    #[cfg(feature = "test-support")]
    fail_next_sidecar_write: AtomicBool,
    #[cfg(feature = "test-support")]
    fail_after_publish: AtomicBool,
    #[cfg(feature = "test-support")]
    before_publish_pause: Mutex<Option<(Arc<Barrier>, Arc<Barrier>)>>,
    #[cfg(feature = "test-support")]
    before_staged_cleanup_pause: Mutex<Option<(Arc<Barrier>, Arc<Barrier>)>>,
    #[cfg(feature = "test-support")]
    persistence_transition_pause: Mutex<Option<(Arc<Barrier>, Arc<Barrier>)>>,
    #[cfg(feature = "test-support")]
    hash_lock_attempts: AtomicU64,
    #[cfg(feature = "test-support")]
    dedupe_digest_attempts: AtomicU64,
    #[cfg(feature = "test-support")]
    free_space_override: Mutex<Option<(u64, u64)>>,
    #[cfg(feature = "test-support")]
    materialize_pause: Mutex<Option<MaterializePause>>,
    #[cfg(feature = "test-support")]
    after_free_space_sample_wait: Mutex<Option<FreeSpaceSampleWait>>,
    #[cfg(feature = "test-support")]
    local_verification_pause: Mutex<Option<LocalVerificationPause>>,
}

#[derive(Default)]
struct PersistenceLifecycle {
    state: Mutex<PersistenceLifecycleState>,
    changed: Notify,
}

#[derive(Default)]
struct PersistenceLifecycleState {
    inflight: usize,
    closing: bool,
}

struct PersistenceLease {
    lifecycle: Arc<PersistenceLifecycle>,
}

struct PersistenceTransition {
    lifecycle: Arc<PersistenceLifecycle>,
}

impl PersistenceLifecycle {
    fn begin(self: &Arc<Self>) -> Result<PersistenceLease, AssetError> {
        let mut state = lock(&self.state);
        if state.closing {
            return Err(AssetError::StaleBinding);
        }
        state.inflight = state
            .inflight
            .checked_add(1)
            .ok_or(AssetError::Invariant("asset persistence count overflow"))?;
        Ok(PersistenceLease {
            lifecycle: Arc::clone(self),
        })
    }

    async fn close(self: &Arc<Self>) -> PersistenceTransition {
        loop {
            let changed = self.changed.notified();
            {
                let mut state = lock(&self.state);
                if !state.closing {
                    state.closing = true;
                    return PersistenceTransition {
                        lifecycle: Arc::clone(self),
                    };
                }
            }
            changed.await;
        }
    }

    fn try_close(self: &Arc<Self>) -> Result<PersistenceTransition, AssetError> {
        let mut state = lock(&self.state);
        if state.closing || state.inflight != 0 {
            return Err(AssetError::StaleBinding);
        }
        state.closing = true;
        Ok(PersistenceTransition {
            lifecycle: Arc::clone(self),
        })
    }
}

impl PersistenceTransition {
    async fn wait_for_idle(&self) {
        loop {
            let changed = self.lifecycle.changed.notified();
            let idle = {
                let state = lock(&self.lifecycle.state);
                state.inflight == 0
            };
            if idle {
                return;
            }
            changed.await;
        }
    }
}

impl Drop for PersistenceLease {
    fn drop(&mut self) {
        let mut state = lock(&self.lifecycle.state);
        if state.inflight == 0 {
            tracing::error!("asset persistence lease released without an inflight owner");
        } else {
            state.inflight -= 1;
        }
        let idle = state.inflight == 0;
        drop(state);
        if idle {
            self.lifecycle.changed.notify_waiters();
        }
    }
}

impl Drop for PersistenceTransition {
    fn drop(&mut self) {
        lock(&self.lifecycle.state).closing = false;
        self.lifecycle.changed.notify_waiters();
    }
}

#[derive(Default)]
struct HealthUsageCache {
    sequence: AtomicU64,
    bytes: AtomicU64,
    objects: AtomicU64,
}

impl HealthUsageCache {
    fn publish(&self, usage: AssetUsage) {
        self.sequence.fetch_add(1, Ordering::AcqRel);
        self.bytes.store(usage.bytes, Ordering::Relaxed);
        self.objects.store(usage.objects, Ordering::Relaxed);
        self.sequence.fetch_add(1, Ordering::Release);
    }

    fn load(&self) -> AssetUsage {
        loop {
            let before = self.sequence.load(Ordering::Acquire);
            if !before.is_multiple_of(2) {
                std::hint::spin_loop();
                continue;
            }
            let usage = AssetUsage {
                bytes: self.bytes.load(Ordering::Relaxed),
                objects: self.objects.load(Ordering::Relaxed),
            };
            if self.sequence.load(Ordering::Acquire) == before {
                return usage;
            }
        }
    }
}

#[derive(Default)]
struct AccountingState {
    active_binding: Option<String>,
    generation: u64,
    committed: AssetUsage,
    reserved: u64,
    unmaterialized: u64,
    limits: Option<AssetLimits>,
    initialized: bool,
    objects: HashMap<String, u64>,
}

pub struct StagedAsset {
    name: String,
    sha256: String,
    size: u64,
    media_type: String,
    width: Option<u32>,
    height: Option<u32>,
    path: Option<PathBuf>,
    reservation: Option<AssetReservation>,
    state: Arc<WorkspaceAssetState>,
    generation: u64,
    binding: String,
    root: PathBuf,
    _persistence: Option<PersistenceLease>,
}

struct GenerationLockedStagedAsset(StagedAsset);

impl Drop for GenerationLockedStagedAsset {
    fn drop(&mut self) {
        self.0.cleanup_while_generation_locked();
    }
}

pub(super) struct StoredAsset {
    asset_ref: AssetRef,
    deduplicated: bool,
}

struct PersistOutcome {
    metadata: AssetMetadata,
    deduplicated: bool,
}

impl std::fmt::Debug for StagedAsset {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_struct("StagedAsset")
            .field("name", &self.name)
            .field("sha256", &self.sha256)
            .field("size", &self.size)
            .field("media_type", &self.media_type)
            .field("width", &self.width)
            .field("height", &self.height)
            .finish_non_exhaustive()
    }
}

impl Drop for StagedAsset {
    fn drop(&mut self) {
        if let Some(path) = self.path.take() {
            remove_staged_path_if_current(&self.state, self.generation, &path);
        }
    }
}

struct AssetStager<'a> {
    store: &'a AssetStore,
    name: String,
    path: Option<PathBuf>,
    file: Option<tokio::fs::File>,
    hasher: Sha256,
    size: u64,
    inspection_prefix: Vec<u8>,
    reservation: Option<AssetReservation>,
    persistence: Option<PersistenceLease>,
}

pub struct HashLock {
    file: File,
    state: Arc<WorkspaceAssetState>,
    generation: u64,
    binding: String,
    root: PathBuf,
    hash: String,
}

struct WorkspaceQuotaLock {
    file: File,
}

impl Drop for WorkspaceQuotaLock {
    fn drop(&mut self) {
        if let Err(error) = FileExt::unlock(&self.file) {
            tracing::warn!(error = %error, "failed to release asset workspace quota lock");
        }
    }
}

impl std::fmt::Debug for HashLock {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_struct("HashLock")
            .field("generation", &self.generation)
            .field("binding", &self.binding)
            .field("hash", &self.hash)
            .finish_non_exhaustive()
    }
}

impl HashLock {
    pub async fn acquire(store: &AssetStore, hash: &str) -> Result<Self, AssetError> {
        let file = store.open_hash_lock_file(hash)?;
        let mut backoff = Duration::from_millis(2);
        loop {
            #[cfg(feature = "test-support")]
            store
                .state
                .hash_lock_attempts
                .fetch_add(1, Ordering::AcqRel);
            match FileExt::try_lock_exclusive(&file) {
                Ok(()) => break,
                Err(error) if error.kind() == std::io::ErrorKind::WouldBlock => {
                    store.validate_generation()?;
                    tokio::time::sleep(backoff).await;
                    backoff = backoff.saturating_mul(2).min(Duration::from_millis(40));
                }
                Err(error) => return Err(AssetError::Store(error)),
            }
        }
        let lock = Self {
            file,
            state: Arc::clone(&store.state),
            generation: store.generation,
            binding: store.binding.clone(),
            root: store.root.clone(),
            hash: hash.to_string(),
        };
        lock.ensure_current()?;
        Ok(lock)
    }

    fn acquire_blocking(store: &AssetStore, hash: &str) -> Result<Self, AssetError> {
        #[cfg(feature = "test-support")]
        store
            .state
            .hash_lock_attempts
            .fetch_add(1, Ordering::AcqRel);
        let file = lock_hash_file(store.open_hash_lock_file(hash)?)?;
        Self::from_locked_file(store, hash, file)
    }

    fn from_locked_file(store: &AssetStore, hash: &str, file: File) -> Result<Self, AssetError> {
        let lock = Self {
            file,
            state: Arc::clone(&store.state),
            generation: store.generation,
            binding: store.binding.clone(),
            root: store.root.clone(),
            hash: hash.to_string(),
        };
        lock.ensure_current()?;
        Ok(lock)
    }

    pub fn ensure_current(&self) -> Result<(), AssetError> {
        let accounting = lock(&self.state.accounting);
        if !accounting.initialized
            || accounting.generation != self.generation
            || accounting.active_binding.as_deref() != Some(self.binding.as_str())
            || !manifest_matches(&self.root.join("store.json"), &self.binding)
        {
            return Err(AssetError::StaleBinding);
        }
        Ok(())
    }

    fn matches(&self, store: &AssetStore, hash: &str) -> bool {
        Arc::ptr_eq(&self.state, &store.state)
            && self.generation == store.generation
            && self.binding == store.binding
            && self.root == store.root
            && self.hash == hash
    }
}

impl Drop for HashLock {
    fn drop(&mut self) {
        if let Err(error) = FileExt::unlock(&self.file) {
            tracing::warn!(error = %error, "failed to release asset hash lock");
        }
    }
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
        let _persistence = state.persistence.try_close()?;
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
                state.health.publish(AssetUsage::default());
                accounting.reserved = 0;
                accounting.unmaterialized = 0;
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

    pub(super) fn limits(&self) -> &AssetLimits {
        &self.limits
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
        self.raw_lock_path(hash)
    }

    pub fn reserve(&self, incoming: u64) -> Result<AssetReservation, AssetError> {
        let _operation = read_lock(&self.state.operation_gate);
        self.validate_current()?;
        let mut accounting = lock(&self.state.accounting);
        self.validate_accounting(&accounting)?;
        let (available, total) = self.free_space()?;
        self.reserve_with_space_locked(&mut accounting, incoming, available, total)
    }

    pub async fn stage_stream<S, E>(
        &self,
        name: impl Into<String>,
        mut chunks: S,
        budget: &mut RequestBudget,
    ) -> Result<StagedAsset, AssetError>
    where
        S: Stream<Item = Result<Bytes, E>> + Unpin,
        E: Into<AssetError>,
    {
        let mut next_budget = *budget;
        next_budget.begin_file(self.limits.max_files)?;
        let mut stager = AssetStager::new(self, name.into())?;
        while let Some(chunk) = chunks.next().await {
            let chunk = match chunk {
                Ok(chunk) => chunk,
                Err(error) => return Err(error.into()),
            };
            stager.write_chunk(&chunk, &mut next_budget).await?;
        }
        let staged = stager.finish().await?;
        *budget = next_budget;
        Ok(staged)
    }

    pub async fn stage_bytes(
        &self,
        name: impl Into<String>,
        bytes: &[u8],
    ) -> Result<StagedAsset, AssetError> {
        let mut budget = RequestBudget::default();
        self.stage_stream(
            name,
            futures::stream::iter([Ok::<Bytes, std::io::Error>(Bytes::copy_from_slice(bytes))]),
            &mut budget,
        )
        .await
    }

    pub async fn persist_batch(
        &self,
        origin_runtime_id: &str,
        staged: Vec<StagedAsset>,
    ) -> Result<Vec<AssetRef>, AssetError> {
        self.persist_batch_with_outcomes(origin_runtime_id, staged)
            .await
            .map(|stored| stored.into_iter().map(|asset| asset.asset_ref).collect())
    }

    pub(super) async fn persist_batch_with_outcomes(
        &self,
        origin_runtime_id: &str,
        staged: Vec<StagedAsset>,
    ) -> Result<Vec<StoredAsset>, AssetError> {
        if staged.is_empty() {
            return Err(AssetError::Invalid(
                "asset upload must contain at least one file".to_string(),
            ));
        }
        if staged.len() > self.limits.max_files {
            return Err(AssetError::TooMany {
                limit: self.limits.max_files,
            });
        }
        let aggregate_size = staged.iter().try_fold(0_u64, |total, asset| {
            total
                .checked_add(asset.size)
                .ok_or(AssetError::RequestTooLarge {
                    limit: self.limits.max_request_bytes,
                })
        })?;
        if aggregate_size > self.limits.max_request_bytes {
            return Err(AssetError::RequestTooLarge {
                limit: self.limits.max_request_bytes,
            });
        }
        let mut refs = Vec::with_capacity(staged.len());
        {
            let _operation = read_lock(&self.state.operation_gate);
            self.validate_current()?;
            for asset in &staged {
                asset.validate_file_for(self)?;
                let asset_ref = AssetRef {
                    version: ASSET_REF_VERSION,
                    origin_runtime_id: origin_runtime_id.to_string(),
                    sha256: asset.sha256.clone(),
                    name: asset.name.clone(),
                    media_type: asset.media_type.clone(),
                    size: asset.size,
                    width: asset.width,
                    height: asset.height,
                };
                asset_ref
                    .validate()
                    .map_err(|error| AssetError::Invalid(error.to_string()))?;
                refs.push(asset_ref);
            }
        }
        let mut stored = Vec::with_capacity(refs.len());
        for (asset, asset_ref) in staged.into_iter().zip(refs) {
            let outcome = self
                .persist_staged_outcome(asset, AssetSource::LocalUpload)
                .await?;
            stored.push(StoredAsset {
                asset_ref,
                deduplicated: outcome.deduplicated,
            });
        }
        Ok(stored)
    }

    pub async fn persist_staged(
        &self,
        staged: StagedAsset,
        source: AssetSource,
    ) -> Result<AssetMetadata, AssetError> {
        self.persist_staged_outcome(staged, source)
            .await
            .map(|outcome| outcome.metadata)
    }

    async fn persist_staged_outcome(
        &self,
        staged: StagedAsset,
        source: AssetSource,
    ) -> Result<PersistOutcome, AssetError> {
        staged.validate_generation_for(self)?;
        let hash_lock = HashLock::acquire(self, &staged.sha256).await?;
        self.persist_staged_with_lock_outcome(staged, source, hash_lock)
            .await
    }

    pub async fn persist_staged_with_lock(
        &self,
        staged: StagedAsset,
        source: AssetSource,
        hash_lock: HashLock,
    ) -> Result<AssetMetadata, AssetError> {
        self.persist_staged_with_lock_outcome(staged, source, hash_lock)
            .await
            .map(|outcome| outcome.metadata)
    }

    async fn persist_staged_with_lock_outcome(
        &self,
        staged: StagedAsset,
        source: AssetSource,
        hash_lock: HashLock,
    ) -> Result<PersistOutcome, AssetError> {
        let store = self.clone();
        tokio::task::spawn_blocking(move || store.persist_staged_locked(staged, source, &hash_lock))
            .await
            .map_err(|error| {
                AssetError::Store(std::io::Error::other(format!(
                    "asset persistence task failed: {error}"
                )))
            })?
    }

    fn persist_staged_locked(
        &self,
        staged: StagedAsset,
        source: AssetSource,
        hash_lock: &HashLock,
    ) -> Result<PersistOutcome, AssetError> {
        let mut staged = GenerationLockedStagedAsset(staged);
        let operation = write_lock(&self.state.operation_gate);
        let result = self.persist_staged_locked_inner(&mut staged.0, source, hash_lock);
        drop(staged);
        drop(operation);
        result
    }

    fn persist_staged_locked_inner(
        &self,
        staged: &mut StagedAsset,
        source: AssetSource,
        hash_lock: &HashLock,
    ) -> Result<PersistOutcome, AssetError> {
        if !valid_source(&source) {
            return Err(AssetError::Invalid(
                "fleet replica source requires a canonical runtime UUID".to_string(),
            ));
        }
        staged.validate_generation_for(self)?;
        if !hash_lock.matches(self, &staged.sha256) {
            return Err(AssetError::StaleBinding);
        }
        hash_lock.ensure_current()?;
        self.validate_current()?;
        staged.validate_file_for(self)?;

        match self.force_verify_dedupe(&staged.sha256, &source)? {
            DedupeOutcome::Present(metadata) => {
                staged
                    .reservation
                    .take()
                    .ok_or(AssetError::Invariant("staged asset reservation is missing"))?
                    .release()?;
                return Ok(PersistOutcome {
                    metadata,
                    deduplicated: true,
                });
            }
            DedupeOutcome::Missing | DedupeOutcome::Corrupt => {}
        }

        let _quota_lock = self.acquire_workspace_quota_lock()?;
        self.reconcile_accounting()?;
        match self.force_verify_dedupe(&staged.sha256, &source)? {
            DedupeOutcome::Present(metadata) => {
                staged
                    .reservation
                    .take()
                    .ok_or(AssetError::Invariant("staged asset reservation is missing"))?
                    .release()?;
                return Ok(PersistOutcome {
                    metadata,
                    deduplicated: true,
                });
            }
            DedupeOutcome::Missing | DedupeOutcome::Corrupt => {}
        }
        self.ensure_new_object_quota(staged.size)?;

        #[cfg(feature = "test-support")]
        if let Some((reached, resume)) = lock(&self.state.before_publish_pause).take() {
            reached.wait();
            resume.wait();
        }

        let object_path = self.raw_object_path(&staged.sha256)?;
        let object_parent = object_path
            .parent()
            .ok_or_else(|| invalid_path(&object_path))?;
        create_private_dir(object_parent)?;
        let temp_path = staged
            .path
            .as_ref()
            .ok_or(AssetError::Invariant("staged asset path is missing"))?;
        for attempt in 0..3 {
            match fs::hard_link(temp_path, &object_path) {
                Ok(()) => break,
                Err(error) if error.kind() == std::io::ErrorKind::AlreadyExists => {
                    match self.force_verify_dedupe(&staged.sha256, &source)? {
                        DedupeOutcome::Present(metadata) => {
                            staged
                                .reservation
                                .take()
                                .ok_or(AssetError::Invariant(
                                    "staged asset reservation is missing",
                                ))?
                                .release()?;
                            return Ok(PersistOutcome {
                                metadata,
                                deduplicated: true,
                            });
                        }
                        DedupeOutcome::Missing if attempt == 2 => {
                            return Err(AssetError::Store(std::io::Error::other(
                                "asset object path repeatedly disappeared during publication",
                            )));
                        }
                        DedupeOutcome::Corrupt if attempt == 2 => {
                            return Err(AssetError::LocalCorruption);
                        }
                        DedupeOutcome::Missing | DedupeOutcome::Corrupt => {}
                    }
                }
                Err(error) => return Err(AssetError::Store(error)),
            }
        }
        let reservation = staged
            .reservation
            .take()
            .ok_or(AssetError::Invariant("staged asset reservation is missing"))?;
        let publish_result = (|| {
            #[cfg(feature = "test-support")]
            if self.state.fail_after_publish.swap(false, Ordering::AcqRel) {
                return Err(AssetError::Store(std::io::Error::other(
                    "injected failure after asset object publication",
                )));
            }
            set_path_file_mode(&object_path)?;
            open_options_no_follow(&object_path, false)?.sync_all()?;
            sync_parent_best_effort(object_parent);

            let object_metadata = fs::symlink_metadata(&object_path)?;
            let metadata = AssetMetadata {
                schema_version: SIDECAR_SCHEMA_VERSION,
                sha256: staged.sha256.clone(),
                size: staged.size,
                media_type: staged.media_type.clone(),
                width: staged.width,
                height: staged.height,
                object_modified_ns: modified_ns(&object_metadata)?,
                stored_at: Utc::now().to_rfc3339(),
                source,
            };
            self.write_metadata(&metadata)?;
            Ok(metadata)
        })();
        if let Err(error) = reservation.commit(&staged.sha256, staged.size) {
            self.reconcile_accounting()?;
            return Err(error);
        }
        publish_result.map(|metadata| PersistOutcome {
            metadata,
            deduplicated: false,
        })
    }

    #[cfg(test)]
    fn reserve_with_space(
        &self,
        incoming: u64,
        available: u64,
        total: u64,
    ) -> Result<AssetReservation, AssetError> {
        let mut accounting = lock(&self.state.accounting);
        self.validate_accounting(&accounting)?;
        self.reserve_with_space_locked(&mut accounting, incoming, available, total)
    }

    fn reserve_with_space_locked(
        &self,
        accounting: &mut AccountingState,
        incoming: u64,
        available: u64,
        total: u64,
    ) -> Result<AssetReservation, AssetError> {
        let pending =
            accounting
                .reserved
                .checked_add(incoming)
                .ok_or(AssetError::QuotaExceeded {
                    used: accounting.committed.bytes,
                    quota: self.limits.workspace_quota_bytes,
                })?;
        let unmaterialized =
            accounting
                .unmaterialized
                .checked_add(incoming)
                .ok_or(AssetError::Invariant(
                    "asset unmaterialized byte count overflow",
                ))?;
        self.check_free_space(unmaterialized, accounting.committed.bytes, available, total)?;
        accounting.reserved = pending;
        accounting.unmaterialized = unmaterialized;
        Ok(AssetReservation {
            bytes: incoming,
            unmaterialized: incoming,
            state: Arc::clone(&self.state),
            generation: self.generation,
            released: false,
        })
    }

    fn extend_reservation(
        &self,
        reservation: &mut AssetReservation,
        incoming: u64,
    ) -> Result<(), AssetError> {
        let _operation = read_lock(&self.state.operation_gate);
        self.validate_current()?;
        if !Arc::ptr_eq(&reservation.state, &self.state)
            || reservation.generation != self.generation
            || reservation.released
        {
            return Err(AssetError::StaleBinding);
        }
        let mut accounting = lock(&self.state.accounting);
        self.validate_accounting(&accounting)?;
        let (available, total) = self.free_space()?;
        let reserved =
            accounting
                .reserved
                .checked_add(incoming)
                .ok_or(AssetError::QuotaExceeded {
                    used: accounting.committed.bytes,
                    quota: self.limits.workspace_quota_bytes,
                })?;
        let unmaterialized =
            accounting
                .unmaterialized
                .checked_add(incoming)
                .ok_or(AssetError::QuotaExceeded {
                    used: accounting.committed.bytes,
                    quota: self.limits.workspace_quota_bytes,
                })?;
        self.check_free_space(unmaterialized, accounting.committed.bytes, available, total)?;
        reservation.bytes =
            reservation
                .bytes
                .checked_add(incoming)
                .ok_or(AssetError::Invariant(
                    "asset reservation byte count overflow",
                ))?;
        reservation.unmaterialized =
            reservation
                .unmaterialized
                .checked_add(incoming)
                .ok_or(AssetError::Invariant(
                    "asset reservation disk claim overflow",
                ))?;
        accounting.reserved = reserved;
        accounting.unmaterialized = unmaterialized;
        Ok(())
    }

    fn materialize_reservation(
        &self,
        reservation: &mut AssetReservation,
        written: u64,
    ) -> Result<(), AssetError> {
        #[cfg(feature = "test-support")]
        let materialized_signal = lock(&self.state.materialize_pause).take().map(|pause| {
            pause.reached.wait();
            pause.resume.wait();
            pause.materialized
        });
        let mut accounting = lock(&self.state.accounting);
        self.validate_accounting(&accounting)?;
        if !Arc::ptr_eq(&reservation.state, &self.state)
            || reservation.generation != self.generation
            || reservation.released
        {
            return Err(AssetError::StaleBinding);
        }
        let reservation_unmaterialized =
            reservation
                .unmaterialized
                .checked_sub(written)
                .ok_or(AssetError::Invariant(
                    "asset reservation disk claim underflow",
                ))?;
        let unmaterialized =
            accounting
                .unmaterialized
                .checked_sub(written)
                .ok_or(AssetError::Invariant(
                    "asset unmaterialized byte count underflow",
                ))?;
        reservation.unmaterialized = reservation_unmaterialized;
        accounting.unmaterialized = unmaterialized;
        #[cfg(feature = "test-support")]
        if let Some(signal) = materialized_signal {
            signal.store(true, Ordering::Release);
        }
        Ok(())
    }

    pub fn put_bytes(
        &self,
        bytes: &[u8],
        source: AssetSource,
    ) -> Result<AssetMetadata, AssetError> {
        if !valid_source(&source) {
            return Err(AssetError::Invalid(
                "fleet replica source requires a canonical runtime UUID".to_string(),
            ));
        }
        let staged = self.stage_bytes_blocking("attachment", bytes)?;
        let hash_lock = HashLock::acquire_blocking(self, &staged.sha256)?;
        self.persist_staged_locked(staged, source, &hash_lock)
            .map(|outcome| outcome.metadata)
    }

    fn stage_bytes_blocking(&self, name: &str, bytes: &[u8]) -> Result<StagedAsset, AssetError> {
        let size = u64::try_from(bytes.len()).map_err(|_| AssetError::TooLarge {
            limit: self.limits.max_file_bytes,
        })?;
        if size > self.limits.max_file_bytes {
            return Err(AssetError::TooLarge {
                limit: self.limits.max_file_bytes,
            });
        }
        let persistence = self.state.persistence.begin()?;
        let mut reservation = self.reserve(size)?;
        let tmp = self.root.join("tmp");
        create_private_dir(&tmp)?;
        let mut tempfile = tempfile::Builder::new()
            .prefix("gitim-asset-")
            .suffix(".tmp")
            .tempfile_in(tmp)?;
        set_file_mode(tempfile.as_file())?;
        tempfile.write_all(bytes)?;
        tempfile.flush()?;
        tempfile.as_file().sync_all()?;
        self.materialize_reservation(&mut reservation, size)?;
        let inspection = inspect_bytes(bytes, name)?;
        let (_file, path) = tempfile
            .keep()
            .map_err(|error| AssetError::Store(error.error))?;
        Ok(StagedAsset {
            name: name.to_string(),
            sha256: sha256_hex(bytes),
            size,
            media_type: inspection.media_type,
            width: inspection.width,
            height: inspection.height,
            path: Some(path),
            reservation: Some(reservation),
            state: Arc::clone(&self.state),
            generation: self.generation,
            binding: self.binding.clone(),
            root: self.root.clone(),
            _persistence: Some(persistence),
        })
    }

    pub fn read(&self, hash: &str) -> Result<Vec<u8>, AssetError> {
        let _operation = write_lock(&self.state.operation_gate);
        self.validate_current()?;
        let object_path = self.raw_object_path(hash)?;
        let snapshot = match read_file_snapshot(&object_path, self.limits.max_file_bytes) {
            Ok(snapshot) => snapshot,
            Err(AssetError::TooLarge { .. } | AssetError::LocalCorruption) => {
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

    pub(super) fn verified_local_asset(
        &self,
        hash: &str,
    ) -> Result<VerifiedLocalAsset, AssetError> {
        let _operation = write_lock(&self.state.operation_gate);
        #[cfg(feature = "test-support")]
        {
            let pause = {
                let mut pause = lock(&self.state.local_verification_pause);
                if let Some(configured) = pause.as_mut() {
                    if configured.remaining == 0 {
                        pause.take()
                    } else {
                        configured.remaining -= 1;
                        None
                    }
                } else {
                    None
                }
            };
            if let Some(pause) = pause {
                pause.reached.wait();
                pause.resume.wait();
            }
        }
        self.validate_current()?;
        let metadata = self.refresh_metadata(hash)?;
        let path = self.raw_object_path(hash)?;
        let file_identity = file_identity(&path)?;
        if file_identity.size != metadata.size
            || file_identity.modified_ns != metadata.object_modified_ns
        {
            return Err(AssetError::LocalCorruption);
        }
        Ok(VerifiedLocalAsset {
            store: self.clone(),
            hash: hash.to_string(),
            path,
            metadata,
            file_identity,
        })
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

    #[cfg(feature = "test-support")]
    #[doc(hidden)]
    pub fn inject_after_publish_failure_once(&self) {
        self.state.fail_after_publish.store(true, Ordering::Release);
    }

    #[cfg(feature = "test-support")]
    #[doc(hidden)]
    pub fn inject_before_publish_pause(&self, reached: Arc<Barrier>, resume: Arc<Barrier>) {
        *lock(&self.state.before_publish_pause) = Some((reached, resume));
    }

    #[cfg(feature = "test-support")]
    #[doc(hidden)]
    pub fn inject_before_staged_cleanup_pause(&self, reached: Arc<Barrier>, resume: Arc<Barrier>) {
        *lock(&self.state.before_staged_cleanup_pause) = Some((reached, resume));
    }

    #[cfg(feature = "test-support")]
    #[doc(hidden)]
    pub fn inject_persistence_transition_pause(&self, reached: Arc<Barrier>, resume: Arc<Barrier>) {
        *lock(&self.state.persistence_transition_pause) = Some((reached, resume));
    }

    #[cfg(feature = "test-support")]
    fn wait_at_persistence_transition(&self) {
        let pause = lock(&self.state.persistence_transition_pause).take();
        if let Some((reached, resume)) = pause {
            reached.wait();
            resume.wait();
        }
    }

    #[cfg(feature = "test-support")]
    #[doc(hidden)]
    pub fn hash_lock_attempts(&self) -> u64 {
        self.state.hash_lock_attempts.load(Ordering::Acquire)
    }

    #[cfg(feature = "test-support")]
    #[doc(hidden)]
    pub fn dedupe_digest_attempts(&self) -> u64 {
        self.state.dedupe_digest_attempts.load(Ordering::Acquire)
    }

    #[cfg(feature = "test-support")]
    #[doc(hidden)]
    pub fn inject_free_space(&self, available: u64, total: u64) {
        *lock(&self.state.free_space_override) = Some((available, total));
    }

    #[cfg(feature = "test-support")]
    #[doc(hidden)]
    pub fn inject_materialize_pause(
        &self,
        reached: Arc<Barrier>,
        resume: Arc<Barrier>,
        materialized: Arc<AtomicBool>,
    ) {
        *lock(&self.state.materialize_pause) = Some(MaterializePause {
            reached,
            resume,
            materialized,
        });
    }

    #[cfg(feature = "test-support")]
    #[doc(hidden)]
    pub fn inject_after_free_space_sample_wait(
        &self,
        sampled: Arc<Barrier>,
        materialized: Arc<AtomicBool>,
    ) {
        *lock(&self.state.after_free_space_sample_wait) = Some(FreeSpaceSampleWait {
            sampled,
            materialized,
        });
    }

    #[cfg(feature = "test-support")]
    #[doc(hidden)]
    pub fn inject_local_verification_pause_after(
        &self,
        remaining: usize,
        reached: Arc<Barrier>,
        resume: Arc<Barrier>,
    ) {
        *lock(&self.state.local_verification_pause) = Some(LocalVerificationPause {
            remaining,
            reached,
            resume,
        });
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
        self.state.health.publish(usage);
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

    fn validate_generation(&self) -> Result<(), AssetError> {
        let accounting = lock(&self.state.accounting);
        self.validate_accounting(&accounting)
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

    fn raw_lock_path(&self, hash: &str) -> Result<PathBuf, AssetError> {
        validate_hash(hash)?;
        let shard = self.root.join("locks/sha256").join(&hash[..2]);
        validate_hash_shard(&shard)?;
        Ok(shard.join(format!("{hash}.lock")))
    }

    fn open_hash_lock_file(&self, hash: &str) -> Result<File, AssetError> {
        let _operation = read_lock(&self.state.operation_gate);
        self.open_hash_lock_file_under_operation(hash)
    }

    fn open_hash_lock_file_under_operation(&self, hash: &str) -> Result<File, AssetError> {
        self.validate_current()?;
        let path = self.raw_lock_path(hash)?;
        create_private_dir(path.parent().ok_or_else(|| invalid_path(&path))?)?;
        open_hash_lock_file(&path)
    }

    fn acquire_workspace_quota_lock(&self) -> Result<WorkspaceQuotaLock, AssetError> {
        let path = self.root.join("locks/quota.lock");
        let file = open_hash_lock_file(&path)?;
        FileExt::lock_exclusive(&file)?;
        Ok(WorkspaceQuotaLock { file })
    }

    fn ensure_new_object_quota(&self, size: u64) -> Result<(), AssetError> {
        let accounting = lock(&self.state.accounting);
        self.validate_accounting(&accounting)?;
        let prospective =
            accounting
                .committed
                .bytes
                .checked_add(size)
                .ok_or(AssetError::QuotaExceeded {
                    used: accounting.committed.bytes,
                    quota: self.limits.workspace_quota_bytes,
                })?;
        if prospective > self.limits.workspace_quota_bytes {
            return Err(AssetError::QuotaExceeded {
                used: accounting.committed.bytes,
                quota: self.limits.workspace_quota_bytes,
            });
        }
        Ok(())
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

    fn free_space(&self) -> Result<(u64, u64), AssetError> {
        #[cfg(feature = "test-support")]
        let override_space = *lock(&self.state.free_space_override);
        #[cfg(not(feature = "test-support"))]
        let override_space: Option<(u64, u64)> = None;
        let space = match override_space {
            Some(space) => space,
            None => (
                fs2::available_space(&self.root)?,
                fs2::total_space(&self.root)?,
            ),
        };
        #[cfg(feature = "test-support")]
        if let Some(wait) = lock(&self.state.after_free_space_sample_wait).take() {
            wait.sampled.wait();
            let deadline = std::time::Instant::now() + Duration::from_millis(200);
            while !wait.materialized.load(Ordering::Acquire) && std::time::Instant::now() < deadline
            {
                std::thread::sleep(Duration::from_millis(1));
            }
        }
        Ok(space)
    }

    fn refresh_metadata(&self, hash: &str) -> Result<AssetMetadata, AssetError> {
        let object_path = self.raw_object_path(hash)?;
        let object_metadata = fs::symlink_metadata(&object_path).map_err(map_missing)?;
        if !object_metadata.file_type().is_file() {
            self.quarantine_corrupt(hash, &object_path)?;
            return Err(AssetError::LocalCorruption);
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
    ) -> Result<DedupeOutcome, AssetError> {
        let object_path = self.raw_object_path(hash)?;
        let object_metadata = match fs::symlink_metadata(&object_path) {
            Ok(metadata) if !metadata.file_type().is_file() => {
                self.quarantine_corrupt(hash, &object_path)?;
                return Ok(DedupeOutcome::Corrupt);
            }
            Ok(metadata) => metadata,
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
                return Ok(DedupeOutcome::Missing);
            }
            Err(error) => return Err(AssetError::Store(error)),
        };
        set_path_file_mode(&object_path)?;
        let metadata_path = self.raw_metadata_path(hash)?;
        // Same trust shortcut as the hot read path (`refresh_metadata`): a
        // valid sidecar matching object size + mtime means the bytes were
        // verified when they were written, so the forced re-hash is skipped.
        // Corruption that preserves both size and mtime is not detected here;
        // that residual risk is accepted because it matches the hot path's
        // trust model.
        if object_metadata.len() <= self.limits.max_file_bytes {
            let modified_ns = modified_ns(&object_metadata)?;
            let trusted = read_sidecar(&metadata_path)
                .ok()
                .filter(|sidecar| valid_sidecar(sidecar, hash, object_metadata.len(), modified_ns));
            if let Some(sidecar) = trusted {
                set_path_file_mode(&metadata_path)?;
                self.register_existing_object(hash, sidecar.size)?;
                return Ok(DedupeOutcome::Present(sidecar));
            }
        }
        #[cfg(feature = "test-support")]
        self.state
            .dedupe_digest_attempts
            .fetch_add(1, Ordering::AcqRel);
        let snapshot = match read_file_digest(&object_path, self.limits.max_file_bytes) {
            Ok(snapshot) => snapshot,
            Err(AssetError::Missing) => return Ok(DedupeOutcome::Missing),
            Err(AssetError::TooLarge { .. } | AssetError::LocalCorruption) => {
                self.quarantine_corrupt(hash, &object_path)?;
                return Ok(DedupeOutcome::Corrupt);
            }
            Err(error) => return Err(error),
        };
        if snapshot.sha256 != hash {
            self.quarantine_corrupt(hash, &object_path)?;
            return Ok(DedupeOutcome::Corrupt);
        }
        let existing_sidecar = read_sidecar(&metadata_path).ok();
        let metadata = if existing_sidecar.as_ref().is_some_and(|sidecar| {
            valid_sidecar(sidecar, hash, snapshot.size, snapshot.modified_ns)
        }) {
            set_path_file_mode(&metadata_path)?;
            existing_sidecar.ok_or(AssetError::Invariant(
                "asset sidecar disappeared during dedupe verification",
            ))?
        } else {
            match self.rebuild_metadata_from_digest(
                hash,
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
        Ok(DedupeOutcome::Present(metadata))
    }

    fn rebuild_metadata_from_digest(
        &self,
        hash: &str,
        metadata_path: &Path,
        existing_sidecar: Option<&AssetMetadata>,
        fallback_source: Option<&AssetSource>,
        snapshot: &FileDigestSnapshot,
    ) -> Result<AssetMetadata, AssetError> {
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
        let inspection = inspect_bytes(&snapshot.inspection_prefix, "")?;
        let metadata = AssetMetadata {
            schema_version: SIDECAR_SCHEMA_VERSION,
            sha256: hash.to_string(),
            size: snapshot.size,
            media_type: inspection.media_type,
            width: inspection.width,
            height: inspection.height,
            object_modified_ns: snapshot.modified_ns,
            stored_at: Utc::now().to_rfc3339(),
            source,
        };
        self.write_metadata(&metadata)?;
        Ok(metadata)
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
        self.state.health.publish(committed);
        Ok(())
    }

    fn reconcile_accounting(&self) -> Result<(), AssetError> {
        let (usage, objects) = self.scan_regular_object_ledger()?;
        let mut accounting = lock(&self.state.accounting);
        self.validate_accounting(&accounting)?;
        accounting.committed = usage;
        accounting.objects = objects;
        self.state.health.publish(usage);
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
        self.prepare_metadata_target(&metadata.sha256, &path)?;
        atomic_write(&path, &bytes)
    }

    fn prepare_metadata_target(&self, hash: &str, path: &Path) -> Result<(), AssetError> {
        match fs::symlink_metadata(path) {
            Ok(metadata) if metadata_entry_is_atomically_replaceable(&metadata.file_type()) => {
                Ok(())
            }
            Ok(_) => self.quarantine_metadata_entry(hash, path),
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(()),
            Err(error) => Err(AssetError::Store(error)),
        }
    }

    fn remove_or_quarantine_metadata_entry(
        &self,
        hash: &str,
        path: &Path,
    ) -> Result<(), AssetError> {
        match fs::symlink_metadata(path) {
            Ok(metadata) if metadata_entry_is_atomically_replaceable(&metadata.file_type()) => {
                remove_file_if_exists(path)
            }
            Ok(_) => self.quarantine_metadata_entry(hash, path),
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(()),
            Err(error) => Err(AssetError::Store(error)),
        }
    }

    fn quarantine_metadata_entry(&self, hash: &str, path: &Path) -> Result<(), AssetError> {
        let root = self
            .workspace_root
            .join(".gitim-runtime/orphaned-assets/corrupt-metadata");
        create_private_dir(&root)?;
        quarantine_rename(path, &root, &format!("corrupt-{hash}.json"))?;
        Ok(())
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
                let name = sidecar.file_name().to_string_lossy().into_owned();
                let Some(hash) = name.strip_suffix(".json") else {
                    continue;
                };
                if validate_hash(hash).is_err() {
                    continue;
                }
                if !metadata_entry_is_atomically_replaceable(&file_type) {
                    self.quarantine_metadata_entry(hash, &sidecar.path())?;
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
        let metadata_result = self.remove_or_quarantine_metadata_entry(hash, &metadata_path);
        self.reconcile_accounting()?;
        metadata_result
    }
}

impl VerifiedLocalAsset {
    pub(super) fn metadata(&self) -> &AssetMetadata {
        &self.metadata
    }

    pub(super) fn path(&self) -> &Path {
        &self.path
    }

    pub(super) fn ensure_current(&self) -> Result<(), AssetError> {
        let _operation = read_lock(&self.store.state.operation_gate);
        self.store.validate_current()?;
        if self.store.raw_object_path(&self.hash)? != self.path
            || file_identity(&self.path)? != self.file_identity
        {
            return Err(AssetError::LocalCorruption);
        }
        Ok(())
    }
}

impl StoredAsset {
    pub(super) fn asset_ref(&self) -> &AssetRef {
        &self.asset_ref
    }

    pub(super) const fn deduplicated(&self) -> bool {
        self.deduplicated
    }

    pub(super) fn into_asset_ref(self) -> AssetRef {
        self.asset_ref
    }
}

impl StagedAsset {
    fn cleanup_while_generation_locked(&mut self) {
        #[cfg(feature = "test-support")]
        if let Some((reached, resume)) = lock(&self.state.before_staged_cleanup_pause).take() {
            reached.wait();
            resume.wait();
        }
        let Some(path) = self.path.take() else {
            return;
        };
        remove_staged_path_if_generation_current(&self.state, self.generation, &path);
    }

    pub fn name(&self) -> &str {
        &self.name
    }

    pub fn sha256(&self) -> &str {
        &self.sha256
    }

    pub const fn size(&self) -> u64 {
        self.size
    }

    pub fn media_type(&self) -> &str {
        &self.media_type
    }

    pub const fn width(&self) -> Option<u32> {
        self.width
    }

    pub const fn height(&self) -> Option<u32> {
        self.height
    }

    fn validate_generation_for(&self, store: &AssetStore) -> Result<(), AssetError> {
        if !Arc::ptr_eq(&self.state, &store.state)
            || self.generation != store.generation
            || self.binding != store.binding
            || self.root != store.root
        {
            return Err(AssetError::StaleBinding);
        }
        let accounting = lock(&self.state.accounting);
        store.validate_accounting(&accounting)?;
        Ok(())
    }

    fn validate_file_for(&self, store: &AssetStore) -> Result<(), AssetError> {
        self.validate_generation_for(store)?;
        let path = self
            .path
            .as_ref()
            .ok_or(AssetError::Invariant("staged asset path is missing"))?;
        if path.parent() != Some(store.root.join("tmp").as_path()) {
            return Err(AssetError::Invalid(
                "asset staging path escaped its workspace".to_string(),
            ));
        }
        let metadata = fs::symlink_metadata(path).map_err(map_missing)?;
        if !metadata.file_type().is_file() || metadata.len() != self.size {
            return Err(AssetError::Invalid(
                "asset staging file changed before persistence".to_string(),
            ));
        }
        Ok(())
    }
}

impl<'a> AssetStager<'a> {
    fn new(store: &'a AssetStore, name: String) -> Result<Self, AssetError> {
        let persistence = store.state.persistence.begin()?;
        let _operation = read_lock(&store.state.operation_gate);
        store.validate_current()?;
        let tmp = store.root.join("tmp");
        create_private_dir(&tmp)?;
        let tempfile = tempfile::Builder::new()
            .prefix("gitim-asset-")
            .suffix(".tmp")
            .tempfile_in(tmp)?;
        set_file_mode(tempfile.as_file())?;
        let (file, path) = tempfile
            .keep()
            .map_err(|error| AssetError::Store(error.error))?;
        Ok(Self {
            store,
            name,
            path: Some(path),
            file: Some(tokio::fs::File::from_std(file)),
            hasher: Sha256::new(),
            size: 0,
            inspection_prefix: Vec::with_capacity(MAX_INSPECTION_PREFIX_BYTES),
            reservation: Some(AssetReservation {
                bytes: 0,
                unmaterialized: 0,
                state: Arc::clone(&store.state),
                generation: store.generation,
                released: false,
            }),
            persistence: Some(persistence),
        })
    }

    async fn write_chunk(
        &mut self,
        chunk: &[u8],
        budget: &mut RequestBudget,
    ) -> Result<(), AssetError> {
        let incoming = u64::try_from(chunk.len()).map_err(|_| AssetError::TooLarge {
            limit: self.store.limits.max_file_bytes,
        })?;
        let size = self
            .size
            .checked_add(incoming)
            .ok_or(AssetError::TooLarge {
                limit: self.store.limits.max_file_bytes,
            })?;
        if size > self.store.limits.max_file_bytes {
            return Err(AssetError::TooLarge {
                limit: self.store.limits.max_file_bytes,
            });
        }
        let previous_budget_bytes = budget.bytes;
        budget.add_bytes(incoming, self.store.limits.max_request_bytes)?;
        let reservation = self
            .reservation
            .as_mut()
            .ok_or(AssetError::Invariant("asset stager reservation is missing"))?;
        if let Err(error) = self.store.extend_reservation(reservation, incoming) {
            budget.bytes = previous_budget_bytes;
            return Err(error);
        }
        let file = self
            .file
            .as_mut()
            .ok_or(AssetError::Invariant("asset staging file is closed"))?;
        if let Err(error) = file.write_all(chunk).await {
            budget.bytes = previous_budget_bytes;
            return Err(AssetError::Store(error));
        }
        self.store.materialize_reservation(reservation, incoming)?;
        self.hasher.update(chunk);
        let prefix_remaining = MAX_INSPECTION_PREFIX_BYTES - self.inspection_prefix.len();
        self.inspection_prefix
            .extend_from_slice(&chunk[..chunk.len().min(prefix_remaining)]);
        self.size = size;
        Ok(())
    }

    async fn finish(mut self) -> Result<StagedAsset, AssetError> {
        let mut file = self
            .file
            .take()
            .ok_or(AssetError::Invariant("asset staging file is closed"))?;
        file.flush().await?;
        file.sync_all().await?;
        drop(file);
        let inspection = inspect_bytes(&self.inspection_prefix, &self.name)?;
        let sha256 = format!("{:x}", self.hasher.clone().finalize());
        Ok(StagedAsset {
            name: self.name.clone(),
            sha256,
            size: self.size,
            media_type: inspection.media_type,
            width: inspection.width,
            height: inspection.height,
            path: self.path.take(),
            reservation: self.reservation.take(),
            state: Arc::clone(&self.store.state),
            generation: self.store.generation,
            binding: self.store.binding.clone(),
            root: self.store.root.clone(),
            _persistence: self.persistence.take(),
        })
    }
}

impl Drop for AssetStager<'_> {
    fn drop(&mut self) {
        drop(self.file.take());
        if let Some(path) = self.path.take() {
            remove_staged_path_if_current(&self.store.state, self.store.generation, &path);
        }
    }
}

fn remove_staged_path_if_current(state: &Arc<WorkspaceAssetState>, generation: u64, path: &Path) {
    let _operation = read_lock(&state.operation_gate);
    remove_staged_path_if_generation_current(state, generation, path);
}

fn remove_staged_path_if_generation_current(
    state: &WorkspaceAssetState,
    generation: u64,
    path: &Path,
) {
    let is_current = {
        let accounting = lock(&state.accounting);
        accounting.initialized && accounting.generation == generation
    };
    if is_current {
        if let Err(error) = remove_file_if_exists(path) {
            tracing::warn!(error = %error, "failed to remove asset staging file");
        }
    }
}

pub struct AssetReservation {
    bytes: u64,
    unmaterialized: u64,
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
        let unmaterialized = accounting
            .unmaterialized
            .checked_sub(self.unmaterialized)
            .ok_or(AssetError::Invariant(
                "asset reservation commit disk claim underflow",
            ))?;
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
        accounting.unmaterialized = unmaterialized;
        accounting.committed = committed;
        if is_new {
            accounting.objects.insert(hash.to_string(), size);
        }
        self.state.health.publish(committed);
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
        let reserved = accounting
            .reserved
            .checked_sub(self.bytes)
            .ok_or(AssetError::Invariant("asset reservation release underflow"))?;
        let unmaterialized = accounting
            .unmaterialized
            .checked_sub(self.unmaterialized)
            .ok_or(AssetError::Invariant(
                "asset reservation release disk claim underflow",
            ))?;
        accounting.reserved = reserved;
        accounting.unmaterialized = unmaterialized;
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
    for _ in 0..32 {
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
        let privacy_result = if source_is_dir {
            set_dir_mode(&candidate)
        } else {
            Ok(())
        };
        sync_parent_best_effort(source_parent);
        sync_parent_best_effort(destination_parent);
        privacy_result?;
        return Ok(candidate);
    }
    Err(AssetError::Store(std::io::Error::new(
        std::io::ErrorKind::AlreadyExists,
        "asset quarantine name collisions exceeded the retry limit",
    )))
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

fn file_identity(path: &Path) -> Result<FileIdentity, AssetError> {
    let metadata = fs::symlink_metadata(path).map_err(map_missing)?;
    if !metadata.file_type().is_file() {
        return Err(AssetError::LocalCorruption);
    }
    #[cfg(unix)]
    {
        use std::os::unix::fs::MetadataExt;
        Ok(FileIdentity {
            size: metadata.len(),
            modified_ns: modified_ns(&metadata)?,
            device: metadata.dev(),
            inode: metadata.ino(),
        })
    }
    #[cfg(not(unix))]
    {
        Ok(FileIdentity {
            size: metadata.len(),
            modified_ns: modified_ns(&metadata)?,
        })
    }
}

fn read_file_snapshot(path: &Path, limit: u64) -> Result<FileSnapshot, AssetError> {
    read_file_snapshot_with_hook(path, limit, || {})
}

fn read_file_digest(path: &Path, limit: u64) -> Result<FileDigestSnapshot, AssetError> {
    let metadata = fs::symlink_metadata(path).map_err(map_missing)?;
    if !metadata.file_type().is_file() {
        return Err(AssetError::LocalCorruption);
    }
    let mut file = open_options_no_follow_nonblocking(path).map_err(map_missing)?;
    let before = file.metadata()?;
    if !before.file_type().is_file() {
        return Err(AssetError::LocalCorruption);
    }
    if before.len() > limit {
        return Err(AssetError::TooLarge { limit });
    }
    let before_modified_ns = modified_ns(&before)?;
    set_file_mode(&file)?;
    let mut hasher = Sha256::new();
    let mut inspection_prefix = Vec::with_capacity(MAX_INSPECTION_PREFIX_BYTES);
    let mut size = 0_u64;
    let mut buffer = [0_u8; 64 * 1024];
    loop {
        let read = file.read(&mut buffer)?;
        if read == 0 {
            break;
        }
        let read_u64 = u64::try_from(read).map_err(|_| AssetError::TooLarge { limit })?;
        size = size
            .checked_add(read_u64)
            .ok_or(AssetError::TooLarge { limit })?;
        if size > limit {
            return Err(AssetError::TooLarge { limit });
        }
        hasher.update(&buffer[..read]);
        let remaining = MAX_INSPECTION_PREFIX_BYTES - inspection_prefix.len();
        inspection_prefix.extend_from_slice(&buffer[..read.min(remaining)]);
    }
    let after = file.metadata()?;
    if after.len() != before.len()
        || after.len() != size
        || modified_ns(&after)? != before_modified_ns
    {
        return Err(AssetError::LocalCorruption);
    }
    Ok(FileDigestSnapshot {
        sha256: format!("{:x}", hasher.finalize()),
        inspection_prefix,
        size,
        modified_ns: before_modified_ns,
    })
}

fn read_file_snapshot_with_hook(
    path: &Path,
    limit: u64,
    after_open: impl FnOnce(),
) -> Result<FileSnapshot, AssetError> {
    let metadata = fs::symlink_metadata(path).map_err(map_missing)?;
    if !metadata.file_type().is_file() {
        return Err(AssetError::LocalCorruption);
    }
    let mut file = open_options_no_follow_nonblocking(path).map_err(map_missing)?;
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

fn open_options_no_follow_nonblocking(path: &Path) -> std::io::Result<File> {
    let mut options = OpenOptions::new();
    options.read(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        options.custom_flags(libc::O_NOFOLLOW | libc::O_NONBLOCK);
    }
    options.open(path)
}

fn open_hash_lock_file(path: &Path) -> Result<File, AssetError> {
    match fs::symlink_metadata(path) {
        Ok(metadata) if !metadata.file_type().is_file() => {
            return Err(AssetError::Invariant(
                "asset hash lock path is not a regular file",
            ));
        }
        Ok(_) => {}
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
        Err(error) => return Err(AssetError::Store(error)),
    }
    let mut options = OpenOptions::new();
    options.read(true).write(true).create(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        options.custom_flags(libc::O_NOFOLLOW).mode(0o600);
    }
    let file = options.open(path)?;
    if !file.metadata()?.file_type().is_file() {
        return Err(AssetError::Invariant(
            "asset hash lock path is not a regular file",
        ));
    }
    set_file_mode(&file)?;
    Ok(file)
}

fn lock_hash_file(file: File) -> Result<File, AssetError> {
    FileExt::lock_exclusive(&file)?;
    Ok(file)
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

fn metadata_entry_is_atomically_replaceable(file_type: &fs::FileType) -> bool {
    if file_type.is_file() || file_type.is_symlink() {
        return true;
    }
    #[cfg(unix)]
    {
        use std::os::unix::fs::FileTypeExt;
        file_type.is_fifo()
    }
    #[cfg(not(unix))]
    {
        false
    }
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

fn wait_condvar<'a, T>(condvar: &Condvar, guard: MutexGuard<'a, T>) -> MutexGuard<'a, T> {
    condvar
        .wait(guard)
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

    #[test]
    fn verified_local_asset_capability_rejects_a_namespace_rebind() {
        let workspace = tempfile::TempDir::new().expect("temporary workspace");
        let store = AssetStore::open(workspace.path(), "local:first", test_limits(100))
            .expect("open asset store");
        let metadata = store
            .put_bytes(b"verified", AssetSource::LocalUpload)
            .expect("put object");
        let capability = store
            .verified_local_asset(&metadata.sha256)
            .expect("verified capability");

        AssetStore::open(workspace.path(), "local:second", test_limits(100)).expect("rebind store");

        assert!(matches!(
            capability.ensure_current(),
            Err(AssetError::StaleBinding)
        ));
    }

    #[cfg(unix)]
    #[test]
    fn verified_local_asset_capability_rejects_path_replacement() {
        let workspace = tempfile::TempDir::new().expect("temporary workspace");
        let store = AssetStore::open(workspace.path(), "local:test", test_limits(100))
            .expect("open asset store");
        let metadata = store
            .put_bytes(b"verified", AssetSource::LocalUpload)
            .expect("put object");
        let capability = store
            .verified_local_asset(&metadata.sha256)
            .expect("verified capability");
        let replacement = workspace.path().join("replacement");
        fs::write(&replacement, b"verified").expect("write replacement");
        fs::rename(&replacement, capability.path()).expect("replace object pathname");

        assert!(matches!(
            capability.ensure_current(),
            Err(AssetError::LocalCorruption)
        ));
    }

    #[tokio::test]
    async fn batch_outcomes_distinguish_created_objects_from_verified_dedupe() {
        let workspace = tempfile::TempDir::new().expect("temporary workspace");
        let store = AssetStore::open(workspace.path(), "local:test", test_limits(100))
            .expect("open asset store");
        let first = store
            .stage_bytes("first.bin", b"same")
            .await
            .expect("stage");
        let first = store
            .persist_batch_with_outcomes("24a6489c-762e-4461-9247-a824807a6080", vec![first])
            .await
            .expect("persist created object");
        assert!(!first[0].deduplicated());

        let second = store
            .stage_bytes("second.bin", b"same")
            .await
            .expect("stage duplicate");
        let second = store
            .persist_batch_with_outcomes("24a6489c-762e-4461-9247-a824807a6080", vec![second])
            .await
            .expect("persist duplicate");
        assert!(second[0].deduplicated());
        assert_eq!(first[0].asset_ref().sha256, second[0].asset_ref().sha256);
    }

    #[test]
    fn registered_workspace_token_prevents_stale_reopen_after_same_path_recreate() {
        let workspace = tempfile::TempDir::new().expect("temporary workspace");
        let service = AssetService::new(test_limits(100));
        let old_token = AssetWorkspaceToken::new();
        let old = service
            .activate_workspace(workspace.path(), "local:old", &old_token)
            .expect("activate old workspace");
        old.put_bytes(b"old", AssetSource::LocalUpload)
            .expect("persist old object");

        assert!(service
            .try_deactivate_workspace(&old_token)
            .expect("deactivate old workspace"));
        assert!(matches!(
            service.open_registered_store(workspace.path(), "local:old", &old_token),
            Err(AssetError::StaleBinding)
        ));
        assert!(matches!(
            old.put_bytes(b"stale", AssetSource::LocalUpload),
            Err(AssetError::StaleBinding)
        ));

        let new_token = AssetWorkspaceToken::new();
        let new = service
            .activate_workspace(workspace.path(), "local:new", &new_token)
            .expect("activate recreated workspace");
        let current = new
            .put_bytes(b"current", AssetSource::LocalUpload)
            .expect("persist recreated object");

        assert!(matches!(
            service.open_registered_store(workspace.path(), "local:old", &old_token),
            Err(AssetError::StaleBinding)
        ));
        assert_eq!(
            service
                .open_registered_store(workspace.path(), "local:new", &new_token)
                .expect("open current workspace")
                .read(&current.sha256)
                .expect("read current object"),
            b"current"
        );
    }

    #[test]
    fn deactivating_an_unactivated_token_prevents_late_activation() {
        let workspace = tempfile::TempDir::new().expect("temporary workspace");
        let service = AssetService::new(test_limits(100));
        let token = AssetWorkspaceToken::new();

        assert!(!service
            .try_deactivate_workspace(&token)
            .expect("revoke pending workspace"));
        assert!(matches!(
            service.activate_workspace(workspace.path(), "local:late", &token),
            Err(AssetError::StaleBinding)
        ));
        assert!(!workspace.path().join(".gitim-runtime/assets").exists());
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
