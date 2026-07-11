use super::{AssetError, AssetMetadata, AssetService, AssetStore, RequestBudget};
use axum::body::Body;
use axum::extract::{DefaultBodyLimit, Multipart, Path, RawQuery, State};
use axum::http::{header, HeaderMap, HeaderValue, Method, Request, StatusCode};
use axum::middleware::{self, Next};
use axum::response::{IntoResponse, Response};
use axum::routing::{get, post};
use axum::{Json, Router};
use futures::StreamExt;
use gitim_core::types::{AssetRef, MAX_ASSET_FILENAME_BYTES, MAX_ASSET_REF_BYTES};
use percent_encoding::{percent_decode_str, utf8_percent_encode, AsciiSet, CONTROLS};
use serde::{Deserialize, Serialize};
use std::path::PathBuf;
use std::sync::atomic::Ordering;
use std::sync::Arc;
use tower::ServiceExt;
use tower_http::services::ServeFile;

use crate::git_config::{GitProvider, WorkspaceConfig};
use crate::http::{SharedRuntimeState, WorkspaceSlug};

const MAX_UPLOAD_HTTP_BYTES: usize = 201 * 1024 * 1024;
const CACHE_CONTROL_VALUE: &str = "private, immutable, max-age=31536000";
const MAX_INLINE_IMAGE_AXIS: u32 = 32_768;
const MAX_INLINE_IMAGE_PIXELS: u64 = 100_000_000;
const FILENAME_ENCODE_SET: &AsciiSet = &CONTROLS
    .add(b' ')
    .add(b'"')
    .add(b'%')
    .add(b'\'')
    .add(b'(')
    .add(b')')
    .add(b'*')
    .add(b',')
    .add(b'/')
    .add(b':')
    .add(b';')
    .add(b'<')
    .add(b'=')
    .add(b'>')
    .add(b'?')
    .add(b'@')
    .add(b'[')
    .add(b'\\')
    .add(b']')
    .add(b'{')
    .add(b'}');

#[derive(Serialize)]
struct ErrorResponse {
    ok: bool,
    error: String,
    error_code: &'static str,
}

#[derive(Serialize)]
struct UploadResponse {
    ok: bool,
    assets: Vec<UploadedAsset>,
}

#[derive(Serialize)]
struct UploadedAsset {
    #[serde(rename = "ref")]
    asset_ref: String,
    sha256: String,
    name: String,
    media_type: String,
    size: u64,
    #[serde(skip_serializing_if = "Option::is_none")]
    width: Option<u32>,
    #[serde(skip_serializing_if = "Option::is_none")]
    height: Option<u32>,
}

struct WorkspaceAssetSnapshot {
    workspace_root: PathBuf,
    binding: String,
    runtime_id: String,
    service: Arc<AssetService>,
}

struct WorkspaceSnapshotError {
    status: StatusCode,
    error_code: &'static str,
    message: &'static str,
}

impl WorkspaceSnapshotError {
    fn into_response(self) -> Response {
        error_response(self.status, self.error_code, self.message)
    }
}

#[derive(Deserialize)]
struct ResolvePath {
    slug: String,
    origin: String,
    hash: String,
}

#[derive(Deserialize)]
struct ObjectPath {
    slug: String,
    hash: String,
}

#[derive(Default)]
struct ResolveOptions {
    name: Option<String>,
    download: bool,
}

#[derive(Clone, Copy)]
enum BrowserRoute {
    Upload,
    Resolve,
}

#[derive(Default)]
struct FetchMetadata<'a> {
    site: Option<&'a str>,
    mode: Option<&'a str>,
    dest: Option<&'a str>,
    user: Option<&'a str>,
}

impl FetchMetadata<'_> {
    fn any(&self) -> bool {
        self.site.is_some() || self.mode.is_some() || self.dest.is_some() || self.user.is_some()
    }
}

pub fn router() -> Router<SharedRuntimeState> {
    let upload = Router::new()
        .route("/assets", post(upload_assets))
        .route_layer(middleware::from_fn(guard_upload_browser))
        .layer(DefaultBodyLimit::max(MAX_UPLOAD_HTTP_BYTES));
    let resolve = Router::new()
        .route(
            "/assets/resolve/{origin}/{hash}",
            get(resolve_asset).head(resolve_asset),
        )
        .route_layer(middleware::from_fn(guard_resolve_browser));
    let objects = Router::new()
        .route(
            "/assets/objects/{hash}",
            get(local_object).head(local_object),
        )
        .route_layer(middleware::from_fn(reject_browser_context));
    upload.merge(resolve).merge(objects)
}

pub(crate) fn workspace_binding(config: &WorkspaceConfig) -> Result<String, AssetError> {
    match config.git.provider {
        GitProvider::Github => config
            .git
            .remote_identity()
            .map(|identity| format!("github:{identity}"))
            .ok_or_else(|| {
                AssetError::Invalid("workspace has no normalized remote identity".into())
            }),
        GitProvider::Local => {
            let created_at = config.created_at.trim();
            if created_at.is_empty() {
                Err(AssetError::Invalid(
                    "local workspace has no creation identity".into(),
                ))
            } else {
                Ok(format!("local:{created_at}"))
            }
        }
    }
}

pub(crate) async fn open_workspace_store(
    service: Arc<AssetService>,
    workspace_root: PathBuf,
    config: &WorkspaceConfig,
) -> Result<AssetStore, AssetError> {
    let binding = workspace_binding(config)?;
    open_store_async(service, workspace_root, binding).await
}

async fn guard_upload_browser(request: Request<Body>, next: Next) -> Response {
    if browser_request_allowed(request.headers(), BrowserRoute::Upload, request.method()) {
        next.run(request).await
    } else {
        forbidden_response()
    }
}

async fn guard_resolve_browser(request: Request<Body>, next: Next) -> Response {
    if browser_request_allowed(request.headers(), BrowserRoute::Resolve, request.method()) {
        next.run(request).await
    } else {
        forbidden_response()
    }
}

async fn reject_browser_context(request: Request<Body>, next: Next) -> Response {
    if has_browser_headers(request.headers()) {
        forbidden_response()
    } else {
        next.run(request).await
    }
}

fn browser_request_allowed(headers: &HeaderMap, route: BrowserRoute, method: &Method) -> bool {
    let origin = match singleton_header(headers, header::ORIGIN.as_str()) {
        Ok(origin) => origin,
        Err(()) => return false,
    };
    let metadata = match fetch_metadata(headers) {
        Ok(metadata) => metadata,
        Err(()) => return false,
    };
    if let Some(origin) = origin {
        return is_allowed_web_origin(origin)
            && matches!(
                metadata.site,
                Some("cross-site" | "same-site" | "same-origin")
            )
            && metadata.mode.is_none_or(|mode| mode == "cors")
            && metadata.dest.is_none_or(|dest| match route {
                BrowserRoute::Upload => dest == "empty",
                BrowserRoute::Resolve => matches!(dest, "empty" | "image"),
            })
            && metadata.user.is_none();
    }
    if !metadata.any() {
        return true;
    }
    matches!(route, BrowserRoute::Resolve)
        && method == Method::GET
        && matches!(
            metadata.site,
            Some("none" | "cross-site" | "same-site" | "same-origin")
        )
        && metadata.mode == Some("navigate")
        && metadata.dest == Some("document")
        && metadata.user == Some("?1")
}

fn has_browser_headers(headers: &HeaderMap) -> bool {
    headers.contains_key(header::ORIGIN) || has_fetch_metadata(headers)
}

fn has_fetch_metadata(headers: &HeaderMap) -> bool {
    FETCH_METADATA_HEADERS
        .iter()
        .any(|name| headers.contains_key(*name))
}

const FETCH_METADATA_HEADERS: [&str; 4] = [
    "sec-fetch-site",
    "sec-fetch-mode",
    "sec-fetch-dest",
    "sec-fetch-user",
];

fn fetch_metadata(headers: &HeaderMap) -> Result<FetchMetadata<'_>, ()> {
    Ok(FetchMetadata {
        site: singleton_header(headers, "sec-fetch-site")?,
        mode: singleton_header(headers, "sec-fetch-mode")?,
        dest: singleton_header(headers, "sec-fetch-dest")?,
        user: singleton_header(headers, "sec-fetch-user")?,
    })
}

fn singleton_header<'a>(headers: &'a HeaderMap, name: &str) -> Result<Option<&'a str>, ()> {
    let values = headers.get_all(name);
    let mut values = values.iter();
    let Some(value) = values.next() else {
        return Ok(None);
    };
    if values.next().is_some() {
        return Err(());
    }
    let value = value.to_str().map_err(|_| ())?;
    if value.is_empty() || value.trim() != value || value.contains(',') {
        return Err(());
    }
    Ok(Some(value))
}

fn is_allowed_web_origin(raw: &str) -> bool {
    if !is_canonical_origin(raw) {
        return false;
    }
    if matches!(raw, "https://gitim.io" | "https://www.gitim.io")
        || matches!(
            raw,
            "http://localhost:5173"
                | "http://127.0.0.1:5173"
                | "http://[::1]:5173"
                | "http://localhost:4173"
                | "http://127.0.0.1:4173"
                | "http://[::1]:4173"
        )
    {
        return true;
    }
    std::env::var("GITIM_WEB_ORIGINS")
        .ok()
        .into_iter()
        .flat_map(|configured| {
            configured
                .split(',')
                .map(str::trim)
                .map(str::to_owned)
                .collect::<Vec<_>>()
        })
        .any(|configured| configured == raw && is_canonical_origin(&configured))
}

fn is_canonical_origin(raw: &str) -> bool {
    if raw == "null" || raw == "*" || raw.contains('@') {
        return false;
    }
    let Ok(url) = reqwest::Url::parse(raw) else {
        return false;
    };
    if !matches!(url.scheme(), "http" | "https")
        || !url.username().is_empty()
        || url.password().is_some()
        || url.path() != "/"
        || url.query().is_some()
        || url.fragment().is_some()
        || url.host_str().is_none()
    {
        return false;
    }
    url.origin().ascii_serialization() == raw
}

fn forbidden_response() -> Response {
    error_response(
        StatusCode::FORBIDDEN,
        "asset_origin_forbidden",
        "asset browser origin is not allowed",
    )
}

async fn upload_assets(
    State(state): State<SharedRuntimeState>,
    WorkspaceSlug(slug): WorkspaceSlug,
    multipart: Result<Multipart, axum::extract::multipart::MultipartRejection>,
) -> Response {
    let snapshot = match snapshot_workspace(&state, &slug, true) {
        Ok(snapshot) => snapshot,
        Err(error) => return error.into_response(),
    };
    let _permit = match snapshot.service.acquire_upload().await {
        Ok(permit) => permit,
        Err(error) => return asset_error_response(&snapshot.service, &slug, error),
    };
    let store = match open_store_async(
        Arc::clone(&snapshot.service),
        snapshot.workspace_root,
        snapshot.binding,
    )
    .await
    {
        Ok(store) => store,
        Err(error) => return asset_error_response(&snapshot.service, &slug, error),
    };
    let mut multipart = match multipart {
        Ok(multipart) => multipart,
        Err(rejection) => {
            let error = if rejection.status() == StatusCode::PAYLOAD_TOO_LARGE {
                AssetError::RequestTooLarge {
                    limit: store.limits().max_request_bytes,
                }
            } else {
                AssetError::Invalid("invalid multipart asset upload".into())
            };
            return asset_error_response(&snapshot.service, &slug, error);
        }
    };

    let mut budget = RequestBudget::default();
    let mut staged = Vec::new();
    loop {
        let field = match multipart.next_field().await {
            Ok(Some(field)) => field,
            Ok(None) => break,
            Err(error) => {
                let asset_error = if error.status() == StatusCode::PAYLOAD_TOO_LARGE {
                    AssetError::RequestTooLarge {
                        limit: store.limits().max_request_bytes,
                    }
                } else {
                    AssetError::Invalid("invalid multipart asset upload".into())
                };
                return asset_error_response(&snapshot.service, &slug, asset_error);
            }
        };
        if field.name() != Some("file") {
            return asset_error_response(
                &snapshot.service,
                &slug,
                AssetError::Invalid("asset upload contains an unknown multipart field".into()),
            );
        }
        let name = match sanitize_upload_name(field.file_name()) {
            Ok(name) => name,
            Err(error) => return asset_error_response(&snapshot.service, &slug, error),
        };
        let chunks = field
            .map(|chunk| chunk.map_err(|_| std::io::Error::other("multipart asset stream failed")));
        match store.stage_stream(name, chunks, &mut budget).await {
            Ok(asset) => staged.push(asset),
            Err(error) => return asset_error_response(&snapshot.service, &slug, error),
        }
    }

    let stored = match store
        .persist_batch_with_outcomes(&snapshot.runtime_id, staged)
        .await
    {
        Ok(stored) => stored,
        Err(error) => return asset_error_response(&snapshot.service, &slug, error),
    };
    for asset in &stored {
        emit_persistence_event(
            &slug,
            asset.asset_ref(),
            asset.deduplicated(),
            &snapshot.runtime_id,
        );
    }
    let refs = stored
        .into_iter()
        .map(super::store::StoredAsset::into_asset_ref)
        .collect::<Vec<_>>();
    let assets = refs.into_iter().map(uploaded_asset).collect::<Vec<_>>();
    Json(UploadResponse { ok: true, assets }).into_response()
}

fn emit_persistence_event(
    workspace: &str,
    asset_ref: &AssetRef,
    deduplicated: bool,
    origin_runtime_id: &str,
) {
    tracing::info!(
        event = if deduplicated {
            "asset_dedupe"
        } else {
            "asset_upload"
        },
        workspace,
        hash_prefix = short_hash(&asset_ref.sha256),
        bytes = asset_ref.size,
        origin_runtime_id,
        "asset persistence complete"
    );
}

fn sanitize_upload_name(raw: Option<&str>) -> Result<String, AssetError> {
    let basename = raw
        .unwrap_or_default()
        .rsplit(['/', '\\'])
        .next()
        .unwrap_or_default();
    let cleaned = basename
        .chars()
        .filter(|character| !character.is_control())
        .collect::<String>();
    let name = if cleaned.is_empty() {
        "attachment".to_string()
    } else {
        cleaned
    };
    if name.len() > MAX_ASSET_FILENAME_BYTES {
        return Err(AssetError::Invalid(
            "asset filename exceeds the 255-byte limit".into(),
        ));
    }
    Ok(name)
}

fn uploaded_asset(asset: AssetRef) -> UploadedAsset {
    UploadedAsset {
        asset_ref: asset.to_string(),
        sha256: asset.sha256,
        name: asset.name,
        media_type: asset.media_type,
        size: asset.size,
        width: asset.width,
        height: asset.height,
    }
}

async fn resolve_asset(
    State(state): State<SharedRuntimeState>,
    Path(path): Path<ResolvePath>,
    RawQuery(raw_query): RawQuery,
    request: Request<Body>,
) -> Response {
    if crate::slug::validate(&path.slug).is_err()
        || !valid_origin_and_hash(&path.origin, &path.hash)
    {
        return invalid_ref_response();
    }
    let options = match parse_resolve_options(raw_query.as_deref()) {
        Ok(options) => options,
        Err(()) => return invalid_ref_response(),
    };
    serve_local(state, path.slug, path.hash, options, request).await
}

async fn local_object(
    State(state): State<SharedRuntimeState>,
    Path(path): Path<ObjectPath>,
    request: Request<Body>,
) -> Response {
    if crate::slug::validate(&path.slug).is_err() || !valid_hash(&path.hash) {
        return invalid_ref_response();
    }
    serve_local(
        state,
        path.slug,
        path.hash,
        ResolveOptions::default(),
        request,
    )
    .await
}

async fn serve_local(
    state: SharedRuntimeState,
    slug: String,
    hash: String,
    options: ResolveOptions,
    request: Request<Body>,
) -> Response {
    let snapshot = match snapshot_workspace(&state, &slug, false) {
        Ok(snapshot) => snapshot,
        Err(error) => return error.into_response(),
    };
    let service = Arc::clone(&snapshot.service);
    let store = match open_store_async(
        Arc::clone(&service),
        snapshot.workspace_root,
        snapshot.binding,
    )
    .await
    {
        Ok(store) => store,
        Err(error) => return asset_error_response(&service, &slug, error),
    };
    let verified = match tokio::task::spawn_blocking({
        let store = store.clone();
        let hash = hash.clone();
        move || store.verified_local_asset(&hash)
    })
    .await
    {
        Ok(Ok(verified)) => verified,
        Ok(Err(error)) => return asset_error_response(&service, &slug, error),
        Err(_) => {
            return asset_error_response(
                &service,
                &slug,
                AssetError::Store(std::io::Error::other("asset lookup task failed")),
            )
        }
    };
    let verified = Arc::new(verified);
    let etag = format!("\"sha256-{hash}\"");
    if let Err(error) = ensure_serve_capability(Arc::clone(&verified)).await {
        return asset_error_response(&service, &slug, error);
    }
    let if_none_match = request.headers().get_all(header::IF_NONE_MATCH);
    let mut if_none_match = if_none_match.iter();
    let exact_strong_match = if_none_match.next().and_then(|value| value.to_str().ok())
        == Some(etag.as_str())
        && if_none_match.next().is_none();
    if exact_strong_match {
        return not_modified_response(&etag);
    }
    let metadata = verified.metadata().clone();
    let mime = match metadata.media_type.parse::<mime::Mime>() {
        Ok(mime) => mime,
        Err(_) => return asset_error_response(&service, &slug, AssetError::LocalCorruption),
    };
    let mut request = request;
    request.headers_mut().remove(header::IF_MODIFIED_SINCE);
    request.headers_mut().remove(header::IF_UNMODIFIED_SINCE);
    let response = match ServeFile::new_with_mime(verified.path(), &mime)
        .oneshot(request)
        .await
    {
        Ok(response) => response.map(Body::new),
        Err(_) => {
            return asset_error_response(
                &service,
                &slug,
                AssetError::Store(std::io::Error::other("asset file open failed")),
            )
        }
    };
    if let Err(error) = ensure_serve_capability(Arc::clone(&verified)).await {
        return asset_error_response(&service, &slug, error);
    }
    tracing::info!(workspace = %slug, hash = %short_hash(&hash), bytes = metadata.size, "asset local hit");
    decorate_file_response(response, &metadata, &etag, &options)
}

async fn ensure_serve_capability(
    verified: Arc<super::store::VerifiedLocalAsset>,
) -> Result<(), AssetError> {
    tokio::task::spawn_blocking(move || verified.ensure_current())
        .await
        .map_err(|_| AssetError::Store(std::io::Error::other("asset validation task failed")))?
}

fn decorate_file_response(
    mut response: Response,
    metadata: &AssetMetadata,
    etag: &str,
    options: &ResolveOptions,
) -> Response {
    let content_length = response.headers().get(header::CONTENT_LENGTH).cloned();
    let content_range = response.headers().get(header::CONTENT_RANGE).cloned();
    let accept_ranges = response.headers().get(header::ACCEPT_RANGES).cloned();
    response.headers_mut().clear();
    if let Ok(value) = HeaderValue::from_str(&metadata.media_type) {
        response.headers_mut().insert(header::CONTENT_TYPE, value);
    }
    if let Some(value) = content_length {
        response.headers_mut().insert(header::CONTENT_LENGTH, value);
    }
    if let Some(value) = content_range {
        response.headers_mut().insert(header::CONTENT_RANGE, value);
    }
    if let Some(value) = accept_ranges {
        response.headers_mut().insert(header::ACCEPT_RANGES, value);
    }
    response.headers_mut().insert(
        header::CACHE_CONTROL,
        HeaderValue::from_static(CACHE_CONTROL_VALUE),
    );
    if let Ok(value) = HeaderValue::from_str(etag) {
        response.headers_mut().insert(header::ETAG, value);
    }
    response.headers_mut().insert(
        "x-content-type-options",
        HeaderValue::from_static("nosniff"),
    );
    let inline = !options.download && inline_safe(metadata);
    if let Ok(value) = content_disposition(inline, options.name.as_deref()) {
        response
            .headers_mut()
            .insert(header::CONTENT_DISPOSITION, value);
    }
    response
}

fn inline_safe(metadata: &AssetMetadata) -> bool {
    if !matches!(
        metadata.media_type.as_str(),
        "image/png" | "image/jpeg" | "image/gif" | "image/webp" | "image/avif"
    ) {
        return false;
    }
    let (Some(width), Some(height)) = (metadata.width, metadata.height) else {
        return false;
    };
    width <= MAX_INLINE_IMAGE_AXIS
        && height <= MAX_INLINE_IMAGE_AXIS
        && u64::from(width)
            .checked_mul(u64::from(height))
            .is_some_and(|pixels| pixels <= MAX_INLINE_IMAGE_PIXELS)
}

fn content_disposition(inline: bool, name: Option<&str>) -> Result<HeaderValue, AssetError> {
    let kind = if inline { "inline" } else { "attachment" };
    let filename = name.unwrap_or("attachment");
    let encoded = utf8_percent_encode(filename, FILENAME_ENCODE_SET).to_string();
    HeaderValue::from_str(&format!(
        "{kind}; filename=\"attachment\"; filename*=UTF-8''{encoded}"
    ))
    .map_err(|_| AssetError::Invalid("asset download filename is invalid".into()))
}

fn not_modified_response(etag: &str) -> Response {
    let mut response = StatusCode::NOT_MODIFIED.into_response();
    response.headers_mut().insert(
        header::CACHE_CONTROL,
        HeaderValue::from_static(CACHE_CONTROL_VALUE),
    );
    if let Ok(value) = HeaderValue::from_str(etag) {
        response.headers_mut().insert(header::ETAG, value);
    }
    response.headers_mut().insert(
        "x-content-type-options",
        HeaderValue::from_static("nosniff"),
    );
    response
}

fn parse_resolve_options(raw: Option<&str>) -> Result<ResolveOptions, ()> {
    let mut options = ResolveOptions::default();
    let Some(raw) = raw else {
        return Ok(options);
    };
    if raw.len() > MAX_ASSET_REF_BYTES {
        return Err(());
    }
    if raw.is_empty() {
        return Ok(options);
    }
    for field in raw.split('&') {
        let (key, value) = field.split_once('=').ok_or(())?;
        match key {
            "name" if options.name.is_none() => {
                if !valid_percent_encoding(value) {
                    return Err(());
                }
                let decoded = percent_decode_str(value).decode_utf8().map_err(|_| ())?;
                validate_download_name(&decoded)?;
                options.name = Some(decoded.into_owned());
            }
            "download" if !options.download && value == "1" => options.download = true,
            _ => return Err(()),
        }
    }
    Ok(options)
}

fn validate_download_name(name: &str) -> Result<(), ()> {
    if name.is_empty()
        || name.len() > MAX_ASSET_FILENAME_BYTES
        || name
            .chars()
            .any(|character| character.is_control() || matches!(character, '/' | '\\'))
    {
        return Err(());
    }
    Ok(())
}

fn valid_percent_encoding(value: &str) -> bool {
    let bytes = value.as_bytes();
    let mut index = 0;
    while index < bytes.len() {
        if bytes[index] == b'%' {
            if index + 2 >= bytes.len()
                || !bytes[index + 1].is_ascii_hexdigit()
                || !bytes[index + 2].is_ascii_hexdigit()
            {
                return false;
            }
            index += 3;
        } else {
            index += 1;
        }
    }
    true
}

fn valid_origin_and_hash(origin: &str, hash: &str) -> bool {
    uuid::Uuid::parse_str(origin)
        .ok()
        .is_some_and(|uuid| uuid.to_string() == origin)
        && valid_hash(hash)
}

fn valid_hash(hash: &str) -> bool {
    hash.len() == 64
        && hash
            .bytes()
            .all(|byte| byte.is_ascii_digit() || matches!(byte, b'a'..=b'f'))
}

fn snapshot_workspace(
    state: &SharedRuntimeState,
    slug: &str,
    require_runtime_id: bool,
) -> Result<WorkspaceAssetSnapshot, WorkspaceSnapshotError> {
    let runtime = crate::preconditions::arc_mutex_lock(state);
    let Some(workspace) = runtime.workspaces.get(slug) else {
        return Err(WorkspaceSnapshotError {
            status: StatusCode::NOT_FOUND,
            error_code: "workspace_not_found",
            message: "unknown workspace",
        });
    };
    let Some(config) = workspace.git_config.as_ref() else {
        return Err(WorkspaceSnapshotError {
            status: StatusCode::CONFLICT,
            error_code: "workspace_not_initialized",
            message: "workspace asset binding is unavailable",
        });
    };
    let binding = workspace_binding(config).map_err(|_| WorkspaceSnapshotError {
        status: StatusCode::CONFLICT,
        error_code: "workspace_not_initialized",
        message: "workspace asset binding is unavailable",
    })?;
    if require_runtime_id
        && uuid::Uuid::parse_str(&runtime.runtime_id)
            .ok()
            .is_none_or(|uuid| uuid.to_string() != runtime.runtime_id)
    {
        return Err(WorkspaceSnapshotError {
            status: StatusCode::SERVICE_UNAVAILABLE,
            error_code: "runtime_identity_unavailable",
            message: "runtime identity is unavailable",
        });
    }
    Ok(WorkspaceAssetSnapshot {
        workspace_root: workspace.path.clone(),
        binding,
        runtime_id: runtime.runtime_id.clone(),
        service: Arc::clone(&runtime.assets),
    })
}

async fn open_store_async(
    service: Arc<AssetService>,
    workspace_root: PathBuf,
    binding: String,
) -> Result<AssetStore, AssetError> {
    tokio::task::spawn_blocking(move || service.open_store(workspace_root, binding))
        .await
        .map_err(|_| AssetError::Store(std::io::Error::other("asset store task failed")))?
}

fn asset_error_response(service: &AssetService, workspace: &str, error: AssetError) -> Response {
    if matches!(
        error,
        AssetError::Store(_)
            | AssetError::Invariant(_)
            | AssetError::StaleBinding
            | AssetError::LocalCorruption
    ) {
        service.store_failures.fetch_add(1, Ordering::Relaxed);
        emit_store_failure_event(workspace, error.error_code());
    }
    error_response(error.status_code(), error.error_code(), &error.to_string())
}

fn emit_store_failure_event(workspace: &str, error_code: &str) {
    tracing::warn!(
        event = "asset_store_failure",
        workspace,
        error_code,
        "asset store operation failed"
    );
}

fn invalid_ref_response() -> Response {
    error_response(
        StatusCode::BAD_REQUEST,
        "invalid_asset_ref",
        "asset reference parameters are invalid",
    )
}

fn error_response(status: StatusCode, error_code: &'static str, error: &str) -> Response {
    (
        status,
        Json(ErrorResponse {
            ok: false,
            error: error.to_string(),
            error_code,
        }),
    )
        .into_response()
}

fn short_hash(hash: &str) -> &str {
    hash.get(..12).unwrap_or(hash)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::BTreeMap;
    use std::fmt;
    use std::sync::{Arc, Mutex};
    use tracing::field::{Field, Visit};
    use tracing::{Event, Subscriber};
    use tracing_subscriber::layer::{Context, SubscriberExt};
    use tracing_subscriber::Layer;

    #[derive(Clone, Default)]
    struct EventCapture {
        events: Arc<Mutex<Vec<BTreeMap<String, String>>>>,
    }

    impl<S> Layer<S> for EventCapture
    where
        S: Subscriber,
    {
        fn on_event(&self, event: &Event<'_>, _context: Context<'_, S>) {
            let mut fields = BTreeMap::new();
            event.record(&mut FieldCapture(&mut fields));
            self.events
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner)
                .push(fields);
        }
    }

    struct FieldCapture<'a>(&'a mut BTreeMap<String, String>);

    impl Visit for FieldCapture<'_> {
        fn record_u64(&mut self, field: &Field, value: u64) {
            self.0.insert(field.name().to_string(), value.to_string());
        }

        fn record_str(&mut self, field: &Field, value: &str) {
            self.0.insert(field.name().to_string(), value.to_string());
        }

        fn record_debug(&mut self, field: &Field, value: &dyn fmt::Debug) {
            self.0
                .insert(field.name().to_string(), format!("{value:?}"));
        }
    }

    #[test]
    fn canonical_origins_reject_normalization_and_authority_tricks() {
        assert!(is_canonical_origin("https://gitim.io"));
        for raw in [
            "https://gitim.io/",
            "HTTPS://gitim.io",
            "https://gitim.io:443",
            "https://user@gitim.io",
            "https://gitim.io/path",
            "https://gitim.io?query",
            "null",
            "*",
        ] {
            assert!(!is_canonical_origin(raw), "{raw}");
        }
    }

    #[test]
    fn persistence_events_capture_upload_and_dedupe_fields_without_paths() {
        let capture = EventCapture::default();
        let subscriber = tracing_subscriber::registry().with(capture.clone());
        let dispatch = tracing::Dispatch::new(subscriber);
        let asset_ref = AssetRef {
            version: 1,
            origin_runtime_id: "24a6489c-762e-4461-9247-a824807a6080".to_string(),
            sha256: "a".repeat(64),
            name: "file.bin".to_string(),
            media_type: "application/octet-stream".to_string(),
            size: 4,
            width: None,
            height: None,
        };
        tracing::dispatcher::with_default(&dispatch, || {
            tracing::callsite::rebuild_interest_cache();
            emit_persistence_event("room", &asset_ref, false, &asset_ref.origin_runtime_id);
            emit_persistence_event("room", &asset_ref, true, &asset_ref.origin_runtime_id);
            emit_store_failure_event("room", "asset_store_failed");
        });

        let events = capture
            .events
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        for (index, expected_event) in ["asset_upload", "asset_dedupe"].iter().enumerate() {
            let event = &events[index];
            assert_eq!(
                event.get("event").map(String::as_str),
                Some(*expected_event)
            );
            assert_eq!(event.get("workspace").map(String::as_str), Some("room"));
            assert_eq!(
                event.get("hash_prefix").map(String::as_str),
                Some("aaaaaaaaaaaa")
            );
            assert_eq!(event.get("bytes").map(String::as_str), Some("4"));
            assert_eq!(
                event.get("origin_runtime_id").map(String::as_str),
                Some("24a6489c-762e-4461-9247-a824807a6080")
            );
            assert!(!format!("{event:?}").contains("/workspace"));
        }
        let failure = &events[2];
        assert_eq!(
            failure.get("event").map(String::as_str),
            Some("asset_store_failure")
        );
        assert_eq!(failure.get("workspace").map(String::as_str), Some("room"));
        assert_eq!(
            failure.get("error_code").map(String::as_str),
            Some("asset_store_failed")
        );
        assert!(!format!("{failure:?}").contains("/workspace"));
    }
}
