use super::{
    AssetError, AssetMetadata, AssetService, AssetSource, AssetStore, HashLock, RequestBudget,
    StagedAsset,
};
use axum::body::Bytes;
use futures::StreamExt;
use percent_encoding::{utf8_percent_encode, NON_ALPHANUMERIC};
use reqwest::header::{self, HeaderMap};
use std::collections::HashSet;
use std::sync::atomic::Ordering;
use std::sync::Arc;
use std::time::Duration;

use crate::fleet::{self, FleetPeerSnapshot};
use crate::http::SharedRuntimeState;

const CONNECT_TIMEOUT: Duration = Duration::from_secs(5);
const HEADER_TIMEOUT: Duration = Duration::from_secs(10);
const CHUNK_IDLE_TIMEOUT: Duration = Duration::from_secs(15);
const CANDIDATE_TIMEOUT: Duration = Duration::from_secs(90);
const WHOLE_RESOLVE_TIMEOUT: Duration = Duration::from_secs(120);
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
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct HeadAvailability {
    pub size: u64,
    pub media_type: String,
    pub peer: AssetPeer,
}

#[derive(Clone, Copy)]
struct ResolverBudgets {
    header: Duration,
    chunk_idle: Duration,
    candidate: Duration,
    whole: Duration,
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

    fn finish(self) -> AssetError {
        if self.hash_mismatch {
            AssetError::HashMismatch
        } else if let Some(message) = self.peer_invalid {
            AssetError::PeerInvalid(message)
        } else if self.unavailable {
            AssetError::OriginUnavailable
        } else {
            AssetError::Missing
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
    let mut seen_runtime_ids = HashSet::new();
    peers.retain(|peer| {
        peer.runtime_id
            .as_deref()
            .is_none_or(|runtime_id| seen_runtime_ids.insert(runtime_id.to_string()))
    });
    peers.dedup();
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
    if let Some(metadata) = local_metadata(store, hash)? {
        return Ok(local_replica(metadata, workspace_slug));
    }

    let result = match tokio::time::timeout(budgets.whole, async {
        let hash_lock = HashLock::acquire(store, hash).await?;
        if let Some(metadata) = local_metadata(store, hash)? {
            return Ok(local_replica(metadata, workspace_slug));
        }
        let client = peer_client()?;
        let (staged, peer) = fetch_candidates(
            state,
            service,
            store,
            &client,
            workspace_slug,
            workspace_identity,
            origin,
            hash,
            budgets,
        )
        .await?;
        let metadata = store
            .persist_staged_with_lock(
                staged,
                AssetSource::FleetReplica {
                    origin_runtime_id: origin.to_string(),
                },
                hash_lock,
            )
            .await?;
        Ok(ResolvedReplica { metadata, peer })
    })
    .await
    {
        Ok(result) => result,
        Err(_) => Err(AssetError::OriginUnavailable),
    };

    match result {
        Ok(replica) => {
            record_fetch_success(service, workspace_slug, hash, origin, &replica);
            Ok(replica)
        }
        Err(error) => {
            record_fetch_failure(service, workspace_slug, hash, origin, &error);
            Err(error)
        }
    }
}

pub async fn resolve_head(
    state: &SharedRuntimeState,
    service: &Arc<AssetService>,
    store: &AssetStore,
    workspace_slug: &str,
    workspace_identity: &str,
    origin: &str,
    hash: &str,
) -> Result<HeadAvailability, AssetError> {
    if let Some(metadata) = local_metadata(store, hash)? {
        return Ok(HeadAvailability {
            size: metadata.size,
            media_type: metadata.media_type,
            peer: AssetPeer {
                node_id: "local".to_string(),
                runtime_id: None,
                base_url: String::new(),
                remote_workspace_slug: workspace_slug.to_string(),
            },
        });
    }
    let budgets = ResolverBudgets::default();
    let result = match tokio::time::timeout(budgets.whole, async {
        let client = peer_client()?;
        probe_candidates(
            state,
            service,
            &client,
            workspace_slug,
            workspace_identity,
            origin,
            hash,
            service.limits.max_file_bytes,
            budgets,
        )
        .await
    })
    .await
    {
        Ok(result) => result,
        Err(_) => Err(AssetError::OriginUnavailable),
    };
    if let Err(error) = &result {
        record_fetch_failure(service, workspace_slug, hash, origin, error);
    }
    result
}

fn local_metadata(store: &AssetStore, hash: &str) -> Result<Option<AssetMetadata>, AssetError> {
    match store.verified_local_asset(hash) {
        Ok(local) => Ok(Some(local.metadata().clone())),
        Err(AssetError::Missing | AssetError::LocalCorruption) => Ok(None),
        Err(error) => Err(error),
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
    }
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
) -> Result<(StagedAsset, AssetPeer), AssetError> {
    let mut peers = snapshot_peers(state, workspace_slug, workspace_identity, origin);
    let mut summary = AttemptSummary::default();
    let mut attempted = HashSet::new();

    if let Some(peer) = peers
        .iter()
        .find(|peer| peer.runtime_id.as_deref() == Some(origin))
        .cloned()
    {
        attempted.insert(peer_key(&peer));
        match download_candidate(service, store, client, &peer, hash, budgets).await {
            Ok(staged) => return Ok((staged, peer)),
            Err(error) => {
                record_candidate_failure(service, workspace_slug, hash, origin, &peer, &error);
                summary.observe(error)?;
            }
        }
    }

    if peers.iter().any(|peer| peer.runtime_id.is_none()) {
        fleet::discover_asset_legacy_identities(state, workspace_slug, workspace_identity).await;
        peers = snapshot_peers(state, workspace_slug, workspace_identity, origin);
        if let Some(peer) = peers
            .iter()
            .find(|peer| {
                peer.runtime_id.as_deref() == Some(origin) && !attempted.contains(&peer_key(peer))
            })
            .cloned()
        {
            attempted.insert(peer_key(&peer));
            match download_candidate(service, store, client, &peer, hash, budgets).await {
                Ok(staged) => return Ok((staged, peer)),
                Err(error) => {
                    record_candidate_failure(service, workspace_slug, hash, origin, &peer, &error);
                    summary.observe(error)?;
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
    let mut available = probe_fallbacks(
        client,
        fallbacks,
        hash,
        store.limits().max_file_bytes,
        budgets,
        &mut summary,
        service,
        workspace_slug,
        origin,
    )
    .await?;
    available.sort_by_key(|(index, _)| *index);
    for (_, peer) in available {
        match download_candidate(service, store, client, &peer, hash, budgets).await {
            Ok(staged) => return Ok((staged, peer)),
            Err(error) => {
                record_candidate_failure(service, workspace_slug, hash, origin, &peer, &error);
                summary.observe(error)?;
            }
        }
    }
    Err(summary.finish())
}

#[allow(clippy::too_many_arguments)]
async fn probe_candidates(
    state: &SharedRuntimeState,
    service: &AssetService,
    client: &reqwest::Client,
    workspace_slug: &str,
    workspace_identity: &str,
    origin: &str,
    hash: &str,
    max_file_bytes: u64,
    budgets: ResolverBudgets,
) -> Result<HeadAvailability, AssetError> {
    let mut peers = snapshot_peers(state, workspace_slug, workspace_identity, origin);
    let mut summary = AttemptSummary::default();
    let mut attempted = HashSet::new();
    if let Some(peer) = peers
        .iter()
        .find(|peer| peer.runtime_id.as_deref() == Some(origin))
        .cloned()
    {
        attempted.insert(peer_key(&peer));
        match probe_candidate(client, &peer, hash, max_file_bytes, budgets).await {
            Ok(availability) => return Ok(availability),
            Err(error) => {
                record_candidate_failure(service, workspace_slug, hash, origin, &peer, &error);
                summary.observe(error)?;
            }
        }
    }
    if peers.iter().any(|peer| peer.runtime_id.is_none()) {
        fleet::discover_asset_legacy_identities(state, workspace_slug, workspace_identity).await;
        peers = snapshot_peers(state, workspace_slug, workspace_identity, origin);
        if let Some(peer) = peers
            .iter()
            .find(|peer| {
                peer.runtime_id.as_deref() == Some(origin) && !attempted.contains(&peer_key(peer))
            })
            .cloned()
        {
            attempted.insert(peer_key(&peer));
            match probe_candidate(client, &peer, hash, max_file_bytes, budgets).await {
                Ok(availability) => return Ok(availability),
                Err(error) => {
                    record_candidate_failure(service, workspace_slug, hash, origin, &peer, &error);
                    summary.observe(error)?;
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
    let mut probes = futures::stream::iter(fallbacks.into_iter().map(|peer| async move {
        let result = probe_candidate(client, &peer, hash, max_file_bytes, budgets).await;
        (peer, result)
    }))
    .buffer_unordered(HEAD_CONCURRENCY);
    while let Some((peer, result)) = probes.next().await {
        match result {
            Ok(availability) => return Ok(availability),
            Err(error) => {
                record_candidate_failure(service, workspace_slug, hash, origin, &peer, &error);
                summary.observe(error)?;
            }
        }
    }
    Err(summary.finish())
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

fn peer_key(peer: &AssetPeer) -> (String, String, String) {
    (
        peer.node_id.clone(),
        peer.base_url.clone(),
        peer.remote_workspace_slug.clone(),
    )
}

#[allow(clippy::too_many_arguments)]
async fn probe_fallbacks(
    client: &reqwest::Client,
    peers: Vec<AssetPeer>,
    hash: &str,
    max_file_bytes: u64,
    budgets: ResolverBudgets,
    summary: &mut AttemptSummary,
    service: &AssetService,
    workspace_slug: &str,
    origin: &str,
) -> Result<Vec<(usize, AssetPeer)>, AssetError> {
    let mut probes = futures::stream::iter(peers.into_iter().enumerate().map(
        |(index, peer)| async move {
            let result = probe_candidate(client, &peer, hash, max_file_bytes, budgets).await;
            (index, peer, result)
        },
    ))
    .buffer_unordered(HEAD_CONCURRENCY);
    let mut available = Vec::new();
    while let Some((index, peer, result)) = probes.next().await {
        match result {
            Ok(_) => available.push((index, peer)),
            Err(error) => {
                record_candidate_failure(service, workspace_slug, hash, origin, &peer, &error);
                summary.observe(error)?;
            }
        }
    }
    Ok(available)
}

async fn probe_candidate(
    client: &reqwest::Client,
    peer: &AssetPeer,
    hash: &str,
    max_file_bytes: u64,
    budgets: ResolverBudgets,
) -> Result<HeadAvailability, AssetError> {
    let response = send_peer_request(client, peer, hash, reqwest::Method::HEAD, budgets).await?;
    let validated = validate_peer_response(response, hash, max_file_bytes)?;
    Ok(HeadAvailability {
        size: validated.size,
        media_type: "application/octet-stream".to_string(),
        peer: peer.clone(),
    })
}

async fn download_candidate(
    service: &Arc<AssetService>,
    store: &AssetStore,
    client: &reqwest::Client,
    peer: &AssetPeer,
    hash: &str,
    budgets: ResolverBudgets,
) -> Result<StagedAsset, AssetError> {
    let _permit = service.acquire_peer().await?;
    tokio::time::timeout(budgets.candidate, async {
        let response = send_peer_request(client, peer, hash, reqwest::Method::GET, budgets).await?;
        let validated = validate_peer_response(response, hash, store.limits().max_file_bytes)?;
        let declared_size = validated.size;
        let idle = budgets.chunk_idle;
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
        if staged.sha256() != hash {
            return Err(AssetError::HashMismatch);
        }
        Ok(staged)
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
    let mut url = reqwest::Url::parse(&peer.base_url)
        .map_err(|_| AssetError::PeerInvalid("fleet peer base URL is invalid".to_string()))?;
    if !matches!(url.scheme(), "http" | "https")
        || !url.username().is_empty()
        || url.password().is_some()
        || url.query().is_some()
        || url.fragment().is_some()
        || url.host_str().is_none()
    {
        return Err(AssetError::PeerInvalid(
            "fleet peer base URL is invalid".to_string(),
        ));
    }
    let workspace = utf8_percent_encode(&peer.remote_workspace_slug, NON_ALPHANUMERIC).to_string();
    url.set_path(&format!("/workspaces/{workspace}/assets/objects/{hash}"));
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
    if replica.peer.node_id == "local" {
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
    if !matches!(
        error,
        AssetError::HashMismatch
            | AssetError::PeerInvalid(_)
            | AssetError::OriginUnavailable
            | AssetError::Missing
    ) {
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

#[cfg(test)]
mod tests {
    use super::*;
    #[cfg(feature = "test-support")]
    use crate::http::RuntimeState;
    #[cfg(feature = "test-support")]
    use sha2::{Digest, Sha256};
    #[cfg(feature = "test-support")]
    use std::sync::Mutex;

    #[test]
    fn production_network_budgets_are_exact() {
        assert_eq!(CONNECT_TIMEOUT, Duration::from_secs(5));
        assert_eq!(HEADER_TIMEOUT, Duration::from_secs(10));
        assert_eq!(CHUNK_IDLE_TIMEOUT, Duration::from_secs(15));
        assert_eq!(CANDIDATE_TIMEOUT, Duration::from_secs(90));
        assert_eq!(WHOLE_RESOLVE_TIMEOUT, Duration::from_secs(120));
        assert_eq!(HEAD_CONCURRENCY, 8);
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
}
