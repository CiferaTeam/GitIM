use super::{
    AssetError, AssetMetadata, AssetService, AssetSource, AssetStore, HashLock, RequestBudget,
};
use axum::body::Bytes;
use futures::StreamExt;
use reqwest::header::{self, HeaderMap};
use std::collections::HashSet;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use std::time::Duration;

use crate::fleet::{self, FleetPeerSnapshot};
use crate::http::SharedRuntimeState;

const CONNECT_TIMEOUT: Duration = Duration::from_secs(5);
const HEADER_TIMEOUT: Duration = Duration::from_secs(10);
const CHUNK_IDLE_TIMEOUT: Duration = Duration::from_secs(15);
const CANDIDATE_TIMEOUT: Duration = Duration::from_secs(90);
const WHOLE_RESOLVE_TIMEOUT: Duration = Duration::from_secs(120);
const LEGACY_DISCOVERY_TIMEOUT: Duration = Duration::from_secs(8);
const HEAD_CONCURRENCY: usize = 8;
const PEER_CACHE_CONTROL: &str = "private, immutable, max-age=31536000";

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AssetPeer {
    pub node_id: String,
    pub runtime_id: Option<String>,
    pub base_url: String,
    pub remote_workspace_slug: String,
}

#[derive(Debug)]
pub struct ResolvedReplica {
    pub metadata: AssetMetadata,
    pub peer: AssetPeer,
    pub locality: AssetLocality,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AssetLocality {
    Local,
    Remote,
}

#[derive(Clone, Copy)]
struct ResolverBudgets {
    header: Duration,
    chunk_idle: Duration,
    candidate: Duration,
    whole: Duration,
}

#[derive(Clone, Copy)]
struct CandidateRequest<'a> {
    origin: &'a str,
    hash: &'a str,
    budgets: ResolverBudgets,
    recorder: &'a ResolutionRecorder,
}

impl Default for ResolverBudgets {
    fn default() -> Self {
        Self {
            header: HEADER_TIMEOUT,
            chunk_idle: CHUNK_IDLE_TIMEOUT,
            candidate: CANDIDATE_TIMEOUT,
            whole: WHOLE_RESOLVE_TIMEOUT,
        }
    }
}

#[derive(Debug)]
struct ValidatedPeerResponse {
    response: reqwest::Response,
    size: u64,
}

#[derive(Default)]
struct AttemptSummary {
    hash_mismatch: bool,
    peer_invalid: Option<String>,
    unavailable: bool,
    missing: bool,
}

#[derive(Clone)]
struct ResolutionRecorder {
    service: Arc<AssetService>,
    workspace_slug: String,
    hash: String,
    origin: String,
    store_recorded: Arc<AtomicBool>,
    fleet_recorded: Arc<AtomicBool>,
}

struct LegacyDiscoveryGuard {
    service: Arc<AssetService>,
    workspace_slug: String,
    workspace_identity: String,
}

impl Drop for LegacyDiscoveryGuard {
    fn drop(&mut self) {
        self.service
            .finish_fleet_discovery(&self.workspace_slug, &self.workspace_identity);
    }
}

impl ResolutionRecorder {
    fn new(service: &Arc<AssetService>, workspace_slug: &str, hash: &str, origin: &str) -> Self {
        Self {
            service: Arc::clone(service),
            workspace_slug: workspace_slug.to_string(),
            hash: hash.to_string(),
            origin: origin.to_string(),
            store_recorded: Arc::new(AtomicBool::new(false)),
            fleet_recorded: Arc::new(AtomicBool::new(false)),
        }
    }

    fn record(&self, error: &AssetError) {
        self.record_store(error);
        if is_fleet_resolution_failure(error)
            && self
                .fleet_recorded
                .compare_exchange(false, true, Ordering::AcqRel, Ordering::Acquire)
                .is_ok()
        {
            record_fetch_failure(
                &self.service,
                &self.workspace_slug,
                &self.hash,
                &self.origin,
                error,
            );
        }
    }

    fn record_store(&self, error: &AssetError) {
        if is_store_failure(error)
            && self
                .store_recorded
                .compare_exchange(false, true, Ordering::AcqRel, Ordering::Acquire)
                .is_ok()
        {
            self.service
                .record_store_failure(&self.workspace_slug, error);
        }
    }
}

impl AttemptSummary {
    fn observe(&mut self, error: AssetError) -> Result<(), AssetError> {
        match error {
            AssetError::HashMismatch => self.hash_mismatch = true,
            AssetError::PeerInvalid(message) => {
                self.peer_invalid.get_or_insert(message);
            }
            AssetError::OriginUnavailable => self.unavailable = true,
            AssetError::Missing => self.missing = true,
            error => return Err(error),
        }
        Ok(())
    }

    fn final_error(&self) -> AssetError {
        if self.hash_mismatch {
            AssetError::HashMismatch
        } else if let Some(message) = &self.peer_invalid {
            AssetError::PeerInvalid(message.clone())
        } else if self.unavailable {
            AssetError::OriginUnavailable
        } else if self.missing {
            AssetError::Missing
        } else {
            AssetError::OriginUnavailable
        }
    }
}

pub fn order_peers(origin: &str, peers: Vec<AssetPeer>) -> Vec<AssetPeer> {
    let mut peers = peers;
    peers.sort_by(|a, b| {
        (a.runtime_id.as_deref() != Some(origin))
            .cmp(&(b.runtime_id.as_deref() != Some(origin)))
            .then_with(|| a.node_id.cmp(&b.node_id))
            .then_with(|| a.remote_workspace_slug.cmp(&b.remote_workspace_slug))
            .then_with(|| a.base_url.cmp(&b.base_url))
    });
    let mut seen_endpoints = HashSet::new();
    peers.retain(|peer| seen_endpoints.insert(peer_key(peer)));
    peers
}

pub async fn resolve_get(
    state: &SharedRuntimeState,
    service: &Arc<AssetService>,
    store: &AssetStore,
    workspace_slug: &str,
    workspace_identity: &str,
    origin: &str,
    hash: &str,
) -> Result<ResolvedReplica, AssetError> {
    resolve_get_with_budgets(
        state,
        service,
        store,
        workspace_slug,
        workspace_identity,
        origin,
        hash,
        ResolverBudgets::default(),
    )
    .await
}

#[allow(clippy::too_many_arguments)]
async fn resolve_get_with_budgets(
    state: &SharedRuntimeState,
    service: &Arc<AssetService>,
    store: &AssetStore,
    workspace_slug: &str,
    workspace_identity: &str,
    origin: &str,
    hash: &str,
    budgets: ResolverBudgets,
) -> Result<ResolvedReplica, AssetError> {
    let mut summary = AttemptSummary::default();
    let recorder = ResolutionRecorder::new(service, workspace_slug, hash, origin);
    let network_attempted = AtomicBool::new(false);
    let whole_timed_out = AtomicBool::new(false);
    let result = match tokio::time::timeout(budgets.whole, async {
        if let Some(metadata) = local_metadata_owned(store, hash, &recorder).await? {
            return Ok(local_replica(metadata, workspace_slug));
        }
        if workspace_identity.is_empty() {
            return Err(AssetError::Missing);
        }
        let hash_lock = HashLock::acquire(store, hash).await?;
        let (hash_lock, local) =
            local_metadata_with_lock_owned(store, hash, hash_lock, &recorder).await?;
        if let Some(metadata) = local {
            return Ok(local_replica(metadata, workspace_slug));
        }
        let mut hash_lock = Some(hash_lock);
        network_attempted.store(true, Ordering::Release);
        let client = peer_client()?;
        let (metadata, peer) = fetch_candidates(
            state,
            service,
            store,
            &client,
            workspace_slug,
            workspace_identity,
            origin,
            hash,
            budgets,
            &mut summary,
            &mut hash_lock,
            &recorder,
        )
        .await?;
        Ok(ResolvedReplica {
            metadata,
            peer,
            locality: AssetLocality::Remote,
        })
    })
    .await
    {
        Ok(result) => result,
        Err(_) => {
            whole_timed_out.store(true, Ordering::Release);
            summary.observe(AssetError::OriginUnavailable)?;
            Err(summary.final_error())
        }
    };

    match result {
        Ok(replica) => {
            record_fetch_success(service, workspace_slug, hash, origin, &replica);
            Ok(replica)
        }
        Err(error) => {
            recorder.record_store(&error);
            if network_attempted.load(Ordering::Acquire) || whole_timed_out.load(Ordering::Acquire)
            {
                recorder.record(&error);
            }
            Err(error)
        }
    }
}

async fn local_metadata_owned(
    store: &AssetStore,
    hash: &str,
    recorder: &ResolutionRecorder,
) -> Result<Option<AssetMetadata>, AssetError> {
    let store = store.clone();
    let hash = hash.to_string();
    let recorder = recorder.clone();
    tokio::task::spawn_blocking(move || local_metadata_blocking(&store, &hash, &recorder))
        .await
        .map_err(|error| {
            AssetError::Store(std::io::Error::other(format!(
                "asset local verification task failed: {error}"
            )))
        })?
}

async fn local_metadata_with_lock_owned(
    store: &AssetStore,
    hash: &str,
    hash_lock: HashLock,
    recorder: &ResolutionRecorder,
) -> Result<(HashLock, Option<AssetMetadata>), AssetError> {
    let store = store.clone();
    let hash = hash.to_string();
    let recorder = recorder.clone();
    let (hash_lock, result) = tokio::task::spawn_blocking(move || {
        let result = local_metadata_blocking(&store, &hash, &recorder);
        (hash_lock, result)
    })
    .await
    .map_err(|error| {
        AssetError::Store(std::io::Error::other(format!(
            "asset local verification task failed: {error}"
        )))
    })?;
    result.map(|metadata| (hash_lock, metadata))
}

fn local_metadata_blocking(
    store: &AssetStore,
    hash: &str,
    recorder: &ResolutionRecorder,
) -> Result<Option<AssetMetadata>, AssetError> {
    match store.verified_local_asset(hash) {
        Ok(local) => Ok(Some(local.metadata().clone())),
        Err(AssetError::Missing) => Ok(None),
        Err(error @ AssetError::LocalCorruption) => {
            recorder.record_store(&error);
            Ok(None)
        }
        Err(error) => {
            recorder.record_store(&error);
            Err(error)
        }
    }
}

fn local_replica(metadata: AssetMetadata, workspace_slug: &str) -> ResolvedReplica {
    ResolvedReplica {
        metadata,
        peer: AssetPeer {
            node_id: "local".to_string(),
            runtime_id: None,
            base_url: String::new(),
            remote_workspace_slug: workspace_slug.to_string(),
        },
        locality: AssetLocality::Local,
    }
}

fn start_legacy_discovery(
    state: &SharedRuntimeState,
    service: &Arc<AssetService>,
    workspace_slug: &str,
    workspace_identity: &str,
) {
    if !service.begin_fleet_discovery(workspace_slug, workspace_identity) {
        return;
    }
    let guard = LegacyDiscoveryGuard {
        service: Arc::clone(service),
        workspace_slug: workspace_slug.to_string(),
        workspace_identity: workspace_identity.to_string(),
    };
    let state = Arc::clone(state);
    let workspace_slug = workspace_slug.to_string();
    let workspace_identity = workspace_identity.to_string();
    tokio::spawn(async move {
        let _guard = guard;
        let _ = tokio::time::timeout(
            LEGACY_DISCOVERY_TIMEOUT,
            fleet::discover_asset_legacy_identities(&state, &workspace_slug, &workspace_identity),
        )
        .await;
    });
}

#[allow(clippy::too_many_arguments)]
async fn fetch_candidates(
    state: &SharedRuntimeState,
    service: &Arc<AssetService>,
    store: &AssetStore,
    client: &reqwest::Client,
    workspace_slug: &str,
    workspace_identity: &str,
    origin: &str,
    hash: &str,
    budgets: ResolverBudgets,
    summary: &mut AttemptSummary,
    hash_lock: &mut Option<HashLock>,
    recorder: &ResolutionRecorder,
) -> Result<(AssetMetadata, AssetPeer), AssetError> {
    let peers = snapshot_peers(state, workspace_slug, workspace_identity, origin);
    let mut attempted = HashSet::new();
    let request = CandidateRequest {
        origin,
        hash,
        budgets,
        recorder,
    };
    if peers.iter().any(|peer| peer.runtime_id.is_none()) {
        start_legacy_discovery(state, service, workspace_slug, workspace_identity);
    }

    let exact: Vec<_> = peers
        .iter()
        .filter(|peer| peer.runtime_id.as_deref() == Some(origin))
        .cloned()
        .collect();
    for peer in exact {
        attempted.insert(peer_key(&peer));
        match download_candidate(service, store, client, &peer, request, hash_lock).await {
            Ok(metadata) => return Ok((metadata, peer)),
            Err(error) => {
                record_candidate_failure(service, workspace_slug, hash, origin, &peer, &error);
                summary.observe(error)?;
                if hash_lock.is_none() {
                    return Err(summary.final_error());
                }
            }
        }
    }

    let fallbacks: Vec<_> = peers
        .into_iter()
        .filter(|peer| {
            peer.runtime_id.as_deref() != Some(origin) && attempted.insert(peer_key(peer))
        })
        .collect();
    if let Some(resolved) = fetch_fallbacks(
        service,
        store,
        client,
        fallbacks,
        request,
        budgets,
        summary,
        workspace_slug,
        origin,
        hash_lock,
    )
    .await?
    {
        return Ok(resolved);
    }
    Err(summary.final_error())
}

fn snapshot_peers(
    state: &SharedRuntimeState,
    workspace_slug: &str,
    workspace_identity: &str,
    origin: &str,
) -> Vec<AssetPeer> {
    let peers = fleet::snapshot_asset_peers(state, workspace_slug, workspace_identity)
        .into_iter()
        .map(peer_from_snapshot)
        .collect();
    order_peers(origin, peers)
}

fn peer_from_snapshot(peer: FleetPeerSnapshot) -> AssetPeer {
    AssetPeer {
        node_id: peer.node_id,
        runtime_id: peer.runtime_id,
        base_url: peer.base_url,
        remote_workspace_slug: peer.remote_workspace_id,
    }
}

fn peer_key(peer: &AssetPeer) -> (String, String) {
    (peer.base_url.clone(), peer.remote_workspace_slug.clone())
}

#[allow(clippy::too_many_arguments)]
async fn fetch_fallbacks(
    service: &Arc<AssetService>,
    store: &AssetStore,
    client: &reqwest::Client,
    peers: Vec<AssetPeer>,
    request: CandidateRequest<'_>,
    budgets: ResolverBudgets,
    summary: &mut AttemptSummary,
    workspace_slug: &str,
    origin: &str,
    hash_lock: &mut Option<HashLock>,
) -> Result<Option<(AssetMetadata, AssetPeer)>, AssetError> {
    let max_file_bytes = store.limits().max_file_bytes;
    for window in peers.chunks(HEAD_CONCURRENCY) {
        let mut probes = futures::stream::iter(window.iter().cloned().enumerate().map(
            |(index, peer)| async move {
                let result =
                    probe_candidate(client, &peer, request.hash, max_file_bytes, budgets).await;
                (index, peer, result)
            },
        ))
        .buffer_unordered(HEAD_CONCURRENCY);
        let mut available = Vec::new();
        while let Some((index, peer, result)) = probes.next().await {
            match result {
                Ok(_) => {
                    #[cfg(feature = "test-support")]
                    service.record_fallback_probe_success();
                    available.push((index, peer));
                }
                Err(error) => {
                    record_candidate_failure(
                        service,
                        workspace_slug,
                        request.hash,
                        origin,
                        &peer,
                        &error,
                    );
                    summary.observe(error)?;
                }
            }
        }
        #[cfg(feature = "test-support")]
        service.record_fallback_probe_window_completed();
        available.sort_by_key(|(index, _)| *index);
        for (_, peer) in available {
            match download_candidate(service, store, client, &peer, request, hash_lock).await {
                Ok(metadata) => return Ok(Some((metadata, peer))),
                Err(error) => {
                    record_candidate_failure(
                        service,
                        workspace_slug,
                        request.hash,
                        origin,
                        &peer,
                        &error,
                    );
                    summary.observe(error)?;
                    if hash_lock.is_none() {
                        return Err(summary.final_error());
                    }
                }
            }
        }
    }
    Ok(None)
}

async fn probe_candidate(
    client: &reqwest::Client,
    peer: &AssetPeer,
    hash: &str,
    max_file_bytes: u64,
    budgets: ResolverBudgets,
) -> Result<(), AssetError> {
    let response = send_peer_request(client, peer, hash, reqwest::Method::HEAD, budgets).await?;
    validate_peer_response(response, hash, max_file_bytes)?;
    Ok(())
}

async fn download_candidate(
    service: &Arc<AssetService>,
    store: &AssetStore,
    client: &reqwest::Client,
    peer: &AssetPeer,
    request: CandidateRequest<'_>,
    hash_lock: &mut Option<HashLock>,
) -> Result<AssetMetadata, AssetError> {
    tokio::time::timeout(request.budgets.candidate, async {
        let permit = service.acquire_peer().await?;
        let response = send_peer_request(
            client,
            peer,
            request.hash,
            reqwest::Method::GET,
            request.budgets,
        )
        .await?;
        let validated =
            validate_peer_response(response, request.hash, store.limits().max_file_bytes)?;
        let declared_size = validated.size;
        let idle = request.budgets.chunk_idle;
        let stream = validated.response.bytes_stream();
        let chunks = futures::stream::unfold(stream, move |mut stream| async move {
            match tokio::time::timeout(idle, stream.next()).await {
                Ok(Some(Ok(bytes))) => Some((Ok::<Bytes, AssetError>(bytes), stream)),
                Ok(Some(Err(_))) => Some((
                    Err(AssetError::PeerInvalid(
                        "peer response body framing is invalid".to_string(),
                    )),
                    stream,
                )),
                Err(_) => Some((Err(AssetError::OriginUnavailable), stream)),
                Ok(None) => None,
            }
        })
        .boxed();
        let mut budget = RequestBudget::default();
        let staged = match store.stage_stream("attachment", chunks, &mut budget).await {
            Ok(staged) => staged,
            Err(AssetError::TooLarge { .. } | AssetError::RequestTooLarge { .. }) => {
                return Err(AssetError::PeerInvalid(
                    "peer object exceeds the transfer limit".to_string(),
                ));
            }
            Err(error) => return Err(error),
        };
        if staged.size() != declared_size {
            return Err(AssetError::PeerInvalid(
                "peer content length does not match the response body".to_string(),
            ));
        }
        if staged.sha256() != request.hash {
            return Err(AssetError::HashMismatch);
        }
        let hash_lock = hash_lock.take().ok_or(AssetError::Invariant(
            "fleet hash lock was already consumed",
        ))?;
        let store = store.clone();
        let recorder = request.recorder.clone();
        let source = AssetSource::FleetReplica {
            origin_runtime_id: request.origin.to_string(),
        };
        let settlement = tokio::spawn(async move {
            let result = store
                .persist_staged_with_lock(staged, source, hash_lock)
                .await;
            if let Err(error) = &result {
                recorder.record(error);
            }
            drop(permit);
            result
        });
        settlement.await.map_err(|error| {
            AssetError::Store(std::io::Error::other(format!(
                "asset settlement task failed: {error}"
            )))
        })?
    })
    .await
    .map_err(|_| AssetError::OriginUnavailable)?
}

async fn send_peer_request(
    client: &reqwest::Client,
    peer: &AssetPeer,
    hash: &str,
    method: reqwest::Method,
    budgets: ResolverBudgets,
) -> Result<reqwest::Response, AssetError> {
    let url = peer_object_url(peer, hash)?;
    let request = client
        .request(method, url)
        .header(header::ACCEPT_ENCODING, "identity");
    let response = tokio::time::timeout(budgets.header, request.send())
        .await
        .map_err(|_| AssetError::OriginUnavailable)?
        .map_err(|_| AssetError::OriginUnavailable)?;
    match response.status() {
        reqwest::StatusCode::OK => Ok(response),
        reqwest::StatusCode::NOT_FOUND => Err(AssetError::Missing),
        status
            if status.is_server_error()
                || status == reqwest::StatusCode::REQUEST_TIMEOUT
                || status == reqwest::StatusCode::TOO_MANY_REQUESTS =>
        {
            Err(AssetError::OriginUnavailable)
        }
        status => Err(AssetError::PeerInvalid(format!(
            "peer returned unexpected status {status}"
        ))),
    }
}

fn validate_peer_response(
    response: reqwest::Response,
    hash: &str,
    max_file_bytes: u64,
) -> Result<ValidatedPeerResponse, AssetError> {
    let headers = response.headers();
    let size = singleton_header(headers, header::CONTENT_LENGTH)?
        .ok_or_else(|| AssetError::PeerInvalid("peer omitted content-length".to_string()))?
        .parse::<u64>()
        .map_err(|_| AssetError::PeerInvalid("peer content-length is invalid".to_string()))?;
    if size > max_file_bytes {
        return Err(AssetError::PeerInvalid(
            "peer object exceeds the transfer limit".to_string(),
        ));
    }
    let media_type = singleton_header(headers, header::CONTENT_TYPE)?
        .ok_or_else(|| AssetError::PeerInvalid("peer omitted content-type".to_string()))?;
    media_type
        .parse::<mime::Mime>()
        .map_err(|_| AssetError::PeerInvalid("peer content-type is invalid".to_string()))?;
    let expected_etag = format!("\"sha256-{hash}\"");
    if singleton_header(headers, header::ETAG)?.as_deref() != Some(expected_etag.as_str()) {
        return Err(AssetError::PeerInvalid(
            "peer asset etag is invalid".to_string(),
        ));
    }
    if singleton_header(headers, "x-content-type-options")?.as_deref() != Some("nosniff") {
        return Err(AssetError::PeerInvalid(
            "peer omitted the nosniff metadata".to_string(),
        ));
    }
    if singleton_header_with_commas(headers, header::CACHE_CONTROL)?.as_deref()
        != Some(PEER_CACHE_CONTROL)
    {
        return Err(AssetError::PeerInvalid(
            "peer immutable cache metadata is invalid".to_string(),
        ));
    }
    if singleton_header(headers, header::ACCEPT_RANGES)?.as_deref() != Some("bytes") {
        return Err(AssetError::PeerInvalid(
            "peer range metadata is invalid".to_string(),
        ));
    }
    if headers.contains_key(header::CONTENT_ENCODING) {
        return Err(AssetError::PeerInvalid(
            "peer content encoding is not supported".to_string(),
        ));
    }
    if headers.contains_key(header::TRANSFER_ENCODING)
        || headers.contains_key(header::CONTENT_RANGE)
    {
        return Err(AssetError::PeerInvalid(
            "peer returned partial or ambiguous framing metadata".to_string(),
        ));
    }
    Ok(ValidatedPeerResponse { response, size })
}

fn singleton_header(
    headers: &HeaderMap,
    name: impl reqwest::header::AsHeaderName,
) -> Result<Option<String>, AssetError> {
    let values = headers.get_all(name);
    let mut values = values.iter();
    let Some(value) = values.next() else {
        return Ok(None);
    };
    if values.next().is_some() {
        return Err(AssetError::PeerInvalid(
            "peer repeated a singleton response header".to_string(),
        ));
    }
    let value = value
        .to_str()
        .map_err(|_| AssetError::PeerInvalid("peer response header is invalid".to_string()))?;
    if value.is_empty() || value.trim() != value || value.contains(',') {
        return Err(AssetError::PeerInvalid(
            "peer response header is invalid".to_string(),
        ));
    }
    Ok(Some(value.to_string()))
}

fn singleton_header_with_commas(
    headers: &HeaderMap,
    name: impl reqwest::header::AsHeaderName,
) -> Result<Option<String>, AssetError> {
    let values = headers.get_all(name);
    let mut values = values.iter();
    let Some(value) = values.next() else {
        return Ok(None);
    };
    if values.next().is_some() {
        return Err(AssetError::PeerInvalid(
            "peer repeated a singleton response header".to_string(),
        ));
    }
    let value = value
        .to_str()
        .map_err(|_| AssetError::PeerInvalid("peer response header is invalid".to_string()))?;
    if value.is_empty() || value.trim() != value {
        return Err(AssetError::PeerInvalid(
            "peer response header is invalid".to_string(),
        ));
    }
    Ok(Some(value.to_string()))
}

fn peer_object_url(peer: &AssetPeer, hash: &str) -> Result<reqwest::Url, AssetError> {
    let mut url = reqwest::Url::parse(&peer.base_url).map_err(|_| AssetError::OriginUnavailable)?;
    if !matches!(url.scheme(), "http" | "https")
        || !url.username().is_empty()
        || url.password().is_some()
        || url.query().is_some()
        || url.fragment().is_some()
        || url.host_str().is_none()
    {
        return Err(AssetError::OriginUnavailable);
    }
    url.path_segments_mut()
        .map_err(|()| AssetError::OriginUnavailable)?
        .pop_if_empty()
        .extend([
            "workspaces",
            peer.remote_workspace_slug.as_str(),
            "assets",
            "objects",
            hash,
        ]);
    Ok(url)
}

fn peer_client() -> Result<reqwest::Client, AssetError> {
    reqwest::Client::builder()
        .connect_timeout(CONNECT_TIMEOUT)
        .redirect(reqwest::redirect::Policy::none())
        .no_proxy()
        .build()
        .map_err(|error| {
            AssetError::Store(std::io::Error::other(format!(
                "failed to build asset peer client: {error}"
            )))
        })
}

fn record_candidate_failure(
    service: &AssetService,
    workspace_slug: &str,
    hash: &str,
    origin: &str,
    peer: &AssetPeer,
    error: &AssetError,
) {
    match error {
        AssetError::HashMismatch => {
            service.hash_mismatches.fetch_add(1, Ordering::Relaxed);
            tracing::warn!(
                event = "asset_hash_mismatch",
                workspace_slug,
                hash_prefix = &hash[..hash.len().min(12)],
                origin_runtime_id = origin,
                fleet_alias = peer.node_id,
                peer_runtime_id = peer.runtime_id.as_deref().unwrap_or("legacy"),
                "Fleet peer returned asset bytes with the wrong hash"
            );
        }
        AssetError::OriginUnavailable => {
            tracing::warn!(
                event = "asset_origin_unavailable",
                workspace_slug,
                hash_prefix = &hash[..hash.len().min(12)],
                origin_runtime_id = origin,
                fleet_alias = peer.node_id,
                peer_runtime_id = peer.runtime_id.as_deref().unwrap_or("legacy"),
                "Fleet asset candidate is unavailable"
            );
        }
        AssetError::PeerInvalid(_) => {
            tracing::warn!(
                event = "asset_fleet_fetch_failure",
                workspace_slug,
                hash_prefix = &hash[..hash.len().min(12)],
                origin_runtime_id = origin,
                fleet_alias = peer.node_id,
                peer_runtime_id = peer.runtime_id.as_deref().unwrap_or("legacy"),
                error_code = error.error_code(),
                "Fleet asset candidate returned an invalid response"
            );
        }
        AssetError::Missing => {
            tracing::debug!(
                event = "asset_fleet_fetch_failure",
                workspace_slug,
                hash_prefix = &hash[..hash.len().min(12)],
                origin_runtime_id = origin,
                fleet_alias = peer.node_id,
                peer_runtime_id = peer.runtime_id.as_deref().unwrap_or("legacy"),
                error_code = error.error_code(),
                "Fleet asset candidate does not hold the object"
            );
        }
        _ => {}
    }
}

fn record_fetch_success(
    _service: &AssetService,
    workspace_slug: &str,
    hash: &str,
    origin: &str,
    replica: &ResolvedReplica,
) {
    if replica.locality == AssetLocality::Local {
        return;
    }
    let event = if replica.peer.runtime_id.as_deref() == Some(origin) {
        "asset_origin_hit"
    } else {
        "asset_fallback_replica_hit"
    };
    tracing::info!(
        event,
        workspace_slug,
        hash_prefix = &hash[..hash.len().min(12)],
        bytes = replica.metadata.size,
        origin_runtime_id = origin,
        fleet_alias = replica.peer.node_id,
        peer_runtime_id = replica.peer.runtime_id.as_deref().unwrap_or("legacy"),
        "Fleet asset resolution complete"
    );
}

fn record_fetch_failure(
    service: &AssetService,
    workspace_slug: &str,
    hash: &str,
    origin: &str,
    error: &AssetError,
) {
    if !is_fleet_resolution_failure(error) {
        return;
    }
    service.fleet_fetch_failures.fetch_add(1, Ordering::Relaxed);
    if matches!(error, AssetError::OriginUnavailable) {
        tracing::warn!(
            event = "asset_origin_unavailable",
            workspace_slug,
            hash_prefix = &hash[..hash.len().min(12)],
            origin_runtime_id = origin,
            "no Fleet peer could answer the asset request"
        );
    }
    tracing::warn!(
        event = "asset_fleet_fetch_failure",
        workspace_slug,
        hash_prefix = &hash[..hash.len().min(12)],
        origin_runtime_id = origin,
        error_code = error.error_code(),
        "Fleet asset resolution failed"
    );
}

fn is_store_failure(error: &AssetError) -> bool {
    matches!(
        error,
        AssetError::Store(_)
            | AssetError::Invariant(_)
            | AssetError::StaleBinding
            | AssetError::LocalCorruption
    )
}

fn is_fleet_resolution_failure(error: &AssetError) -> bool {
    matches!(
        error,
        AssetError::HashMismatch
            | AssetError::PeerInvalid(_)
            | AssetError::OriginUnavailable
            | AssetError::Missing
            | AssetError::Store(_)
            | AssetError::Invariant(_)
            | AssetError::StaleBinding
            | AssetError::LocalCorruption
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    #[cfg(feature = "test-support")]
    use crate::http::RuntimeState;
    #[cfg(feature = "test-support")]
    use sha2::{Digest, Sha256};
    #[cfg(feature = "test-support")]
    use std::sync::Mutex;
    #[cfg(feature = "test-support")]
    use std::task::Poll;

    #[test]
    fn production_network_budgets_are_exact() {
        assert_eq!(CONNECT_TIMEOUT, Duration::from_secs(5));
        assert_eq!(HEADER_TIMEOUT, Duration::from_secs(10));
        assert_eq!(CHUNK_IDLE_TIMEOUT, Duration::from_secs(15));
        assert_eq!(CANDIDATE_TIMEOUT, Duration::from_secs(90));
        assert_eq!(WHOLE_RESOLVE_TIMEOUT, Duration::from_secs(120));
        assert_eq!(HEAD_CONCURRENCY, 8);
    }

    #[tokio::test(start_paused = true)]
    #[cfg(feature = "test-support")]
    async fn candidate_budget_includes_waiting_for_peer_slot() {
        let workspace = tempfile::tempdir().unwrap();
        let limits = super::super::AssetLimits {
            workspace_quota_bytes: 1024 * 1024,
            min_free_bytes: 1,
            peer_slots: 4,
            ..Default::default()
        };
        let service = Arc::new(AssetService::new(limits.clone()));
        let store = AssetStore::open(workspace.path(), "local:test", limits).unwrap();
        let mut permits = Vec::new();
        for _ in 0..4 {
            permits.push(service.acquire_peer().await.unwrap());
        }
        let peer = AssetPeer {
            node_id: "blocked".to_string(),
            runtime_id: Some("3c6a295e-744a-41dc-ba60-5c21bb94e5a2".to_string()),
            base_url: "http://127.0.0.1:9".to_string(),
            remote_workspace_slug: "room".to_string(),
        };
        let client = peer_client().unwrap();
        let hash = format!("{:x}", Sha256::digest(b"expected"));
        let recorder =
            ResolutionRecorder::new(&service, "room", &hash, peer.runtime_id.as_deref().unwrap());
        let mut hash_lock = Some(HashLock::acquire(&store, &hash).await.unwrap());
        {
            let future = download_candidate(
                &service,
                &store,
                &client,
                &peer,
                CandidateRequest {
                    origin: peer.runtime_id.as_deref().unwrap(),
                    hash: &hash,
                    budgets: ResolverBudgets::default(),
                    recorder: &recorder,
                },
                &mut hash_lock,
            );
            tokio::pin!(future);
            assert!(matches!(futures::poll!(&mut future), Poll::Pending));

            tokio::time::advance(Duration::from_secs(89)).await;
            assert!(matches!(futures::poll!(&mut future), Poll::Pending));
            tokio::time::advance(Duration::from_secs(1)).await;
            let result = tokio::time::timeout(Duration::from_secs(1), &mut future).await;

            assert!(matches!(result, Ok(Err(AssetError::OriginUnavailable))));
        }
        assert!(hash_lock.is_some());
        assert_eq!(service.available_peer_permits(), 0);
        assert_eq!(store.reserved_bytes().unwrap(), 0);
        assert_eq!(service.fleet_fetch_failures.load(Ordering::Acquire), 0);
        drop(permits);
        assert_eq!(service.available_peer_permits(), 4);
        tokio::task::yield_now().await;
        assert_eq!(service.available_peer_permits(), 4);
    }

    #[tokio::test]
    #[cfg(feature = "test-support")]
    async fn local_lookup_io_failure_is_not_reclassified_as_remote_missing() {
        let workspace = tempfile::tempdir().unwrap();
        let limits = super::super::AssetLimits {
            workspace_quota_bytes: 1024 * 1024,
            min_free_bytes: 1,
            ..Default::default()
        };
        let service = Arc::new(AssetService::new(limits.clone()));
        let store = AssetStore::open(workspace.path(), "local:test", limits).unwrap();
        let bytes = b"valid object with blocked sidecar";
        let hash = format!("{:x}", Sha256::digest(bytes));
        let root = workspace.path().join(".gitim-runtime/assets/v1");
        let object = root.join("objects/sha256").join(&hash[..2]).join(&hash);
        std::fs::create_dir_all(object.parent().unwrap()).unwrap();
        std::fs::write(object, bytes).unwrap();
        store.inject_sidecar_write_failure_once();
        let state = Arc::new(Mutex::new(RuntimeState::default()));

        let error = resolve_get(
            &state,
            &service,
            &store,
            "room",
            "github.com/acme/room",
            "3c6a295e-744a-41dc-ba60-5c21bb94e5a2",
            &hash,
        )
        .await
        .unwrap_err();

        assert!(matches!(error, AssetError::Store(_)), "{error:?}");
        assert_eq!(service.fleet_fetch_failures.load(Ordering::Acquire), 0);
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    #[cfg(feature = "test-support")]
    async fn slow_initial_local_verification_stays_inside_whole_budget() {
        let workspace = tempfile::tempdir().unwrap();
        let limits = super::super::AssetLimits {
            workspace_quota_bytes: 1024 * 1024,
            min_free_bytes: 1,
            ..Default::default()
        };
        let service = Arc::new(AssetService::new(limits.clone()));
        let store = AssetStore::open(workspace.path(), "local:test", limits).unwrap();
        let metadata = store
            .put_bytes(b"local object", AssetSource::LocalUpload)
            .unwrap();
        let reached = Arc::new(std::sync::Barrier::new(2));
        let resume = Arc::new(std::sync::Barrier::new(2));
        store.inject_local_verification_pause_after(0, Arc::clone(&reached), Arc::clone(&resume));
        let state = Arc::new(Mutex::new(RuntimeState::default()));
        let task_state = Arc::clone(&state);
        let task_service = Arc::clone(&service);
        let task_store = store.clone();
        let hash = metadata.sha256.clone();
        let request = tokio::spawn(async move {
            resolve_get_with_budgets(
                &task_state,
                &task_service,
                &task_store,
                "room",
                "github.com/acme/room",
                "3c6a295e-744a-41dc-ba60-5c21bb94e5a2",
                &hash,
                ResolverBudgets {
                    whole: Duration::from_millis(50),
                    ..ResolverBudgets::default()
                },
            )
            .await
        });
        tokio::task::spawn_blocking(move || reached.wait())
            .await
            .unwrap();

        let responsive = tokio::spawn(async {
            tokio::task::yield_now().await;
            true
        });
        assert!(tokio::time::timeout(Duration::from_millis(100), responsive)
            .await
            .unwrap()
            .unwrap());
        tokio::time::sleep(Duration::from_millis(100)).await;
        let finished_within_budget = request.is_finished();
        tokio::task::spawn_blocking(move || resume.wait())
            .await
            .unwrap();
        let result = request.await.unwrap();

        assert!(finished_within_budget);
        assert!(matches!(result, Err(AssetError::OriginUnavailable)));
        assert_eq!(service.fleet_fetch_failures.load(Ordering::Acquire), 1);
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    #[cfg(feature = "test-support")]
    async fn cancelled_post_lock_verification_retains_hash_lock_until_completion() {
        let workspace = tempfile::tempdir().unwrap();
        let limits = super::super::AssetLimits {
            workspace_quota_bytes: 1024 * 1024,
            min_free_bytes: 1,
            ..Default::default()
        };
        let service = Arc::new(AssetService::new(limits.clone()));
        let store = AssetStore::open(workspace.path(), "local:test", limits).unwrap();
        let reached = Arc::new(std::sync::Barrier::new(2));
        let resume = Arc::new(std::sync::Barrier::new(2));
        store.inject_local_verification_pause_after(1, Arc::clone(&reached), Arc::clone(&resume));
        let state = Arc::new(Mutex::new(RuntimeState::default()));
        let hash = format!("{:x}", Sha256::digest(b"missing"));
        let task_state = Arc::clone(&state);
        let task_service = Arc::clone(&service);
        let task_store = store.clone();
        let task_hash = hash.clone();
        let request = tokio::spawn(async move {
            resolve_get_with_budgets(
                &task_state,
                &task_service,
                &task_store,
                "room",
                "github.com/acme/room",
                "3c6a295e-744a-41dc-ba60-5c21bb94e5a2",
                &task_hash,
                ResolverBudgets::default(),
            )
            .await
        });
        tokio::task::spawn_blocking(move || reached.wait())
            .await
            .unwrap();
        request.abort();
        assert!(request.await.is_err());

        let contender_store = store.clone();
        let contender_hash = hash.clone();
        let contender_finished = Arc::new(AtomicBool::new(false));
        let contender_signal = Arc::clone(&contender_finished);
        let contender = std::thread::spawn(move || {
            let runtime = tokio::runtime::Builder::new_current_thread()
                .enable_time()
                .build()
                .unwrap();
            let result = runtime.block_on(HashLock::acquire(&contender_store, &contender_hash));
            contender_signal.store(true, Ordering::Release);
            result
        });
        std::thread::sleep(Duration::from_millis(25));
        assert!(!contender_finished.load(Ordering::Acquire));
        assert!(tokio::time::timeout(Duration::from_millis(100), async {
            tokio::task::yield_now().await;
            true
        })
        .await
        .unwrap());

        tokio::task::spawn_blocking(move || resume.wait())
            .await
            .unwrap();
        let lock = tokio::task::spawn_blocking(move || contender.join().unwrap())
            .await
            .unwrap()
            .unwrap();
        drop(lock);
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    #[cfg(feature = "test-support")]
    async fn detached_local_verification_records_eventual_store_failure() {
        let workspace = tempfile::tempdir().unwrap();
        let limits = super::super::AssetLimits {
            workspace_quota_bytes: 1024 * 1024,
            min_free_bytes: 1,
            ..Default::default()
        };
        let service = Arc::new(AssetService::new(limits.clone()));
        let store = AssetStore::open(workspace.path(), "local:test", limits).unwrap();
        let bytes = b"object requiring metadata rebuild";
        let hash = format!("{:x}", Sha256::digest(bytes));
        let root = workspace.path().join(".gitim-runtime/assets/v1");
        let object = root.join("objects/sha256").join(&hash[..2]).join(&hash);
        std::fs::create_dir_all(object.parent().unwrap()).unwrap();
        std::fs::write(object, bytes).unwrap();
        store.inject_sidecar_write_failure_once();
        let reached = Arc::new(std::sync::Barrier::new(2));
        let resume = Arc::new(std::sync::Barrier::new(2));
        store.inject_local_verification_pause_after(0, Arc::clone(&reached), Arc::clone(&resume));
        let state = Arc::new(Mutex::new(RuntimeState::default()));
        let task_state = Arc::clone(&state);
        let task_service = Arc::clone(&service);
        let task_store = store.clone();
        let task_hash = hash.clone();
        let request = tokio::spawn(async move {
            resolve_get_with_budgets(
                &task_state,
                &task_service,
                &task_store,
                "room",
                "github.com/acme/room",
                "3c6a295e-744a-41dc-ba60-5c21bb94e5a2",
                &task_hash,
                ResolverBudgets {
                    whole: Duration::from_millis(50),
                    ..ResolverBudgets::default()
                },
            )
            .await
        });
        tokio::task::spawn_blocking(move || reached.wait())
            .await
            .unwrap();
        tokio::time::sleep(Duration::from_millis(100)).await;
        assert!(matches!(
            request.await.unwrap(),
            Err(AssetError::OriginUnavailable)
        ));
        assert_eq!(service.store_failures.load(Ordering::Acquire), 0);

        tokio::task::spawn_blocking(move || resume.wait())
            .await
            .unwrap();
        let deadline = std::time::Instant::now() + Duration::from_secs(5);
        while service.store_failures.load(Ordering::Acquire) == 0
            && std::time::Instant::now() < deadline
        {
            tokio::task::yield_now().await;
        }
        assert_eq!(service.store_failures.load(Ordering::Acquire), 1);
        assert_eq!(service.fleet_fetch_failures.load(Ordering::Acquire), 1);
    }
}
