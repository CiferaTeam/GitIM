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
    if browser_request_allowed(request.headers(), false, request.method()) {
        next.run(request).await
    } else {
        forbidden_response()
    }
}

async fn guard_resolve_browser(request: Request<Body>, next: Next) -> Response {
    if browser_request_allowed(request.headers(), true, request.method()) {
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

fn browser_request_allowed(headers: &HeaderMap, navigation_allowed: bool, method: &Method) -> bool {
    let origins = headers.get_all(header::ORIGIN);
    if origins.iter().count() > 1 {
        return false;
    }
    if let Some(origin) = origins.iter().next() {
        return origin.to_str().ok().is_some_and(is_allowed_web_origin);
    }
    if !has_fetch_metadata(headers) {
        return true;
    }
    navigation_allowed
        && method == Method::GET
        && header_equals(headers, "sec-fetch-mode", "navigate")
        && header_equals(headers, "sec-fetch-dest", "document")
        && header_equals(headers, "sec-fetch-user", "?1")
}

fn has_browser_headers(headers: &HeaderMap) -> bool {
    headers.contains_key(header::ORIGIN) || has_fetch_metadata(headers)
}

fn has_fetch_metadata(headers: &HeaderMap) -> bool {
    [
        "sec-fetch-site",
        "sec-fetch-mode",
        "sec-fetch-dest",
        "sec-fetch-user",
    ]
    .iter()
    .any(|name| headers.contains_key(*name))
}

fn header_equals(headers: &HeaderMap, name: &'static str, expected: &str) -> bool {
    let values = headers.get_all(name);
    let mut values = values.iter();
    values.next().and_then(|value| value.to_str().ok()) == Some(expected) && values.next().is_none()
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
        Err(error) => return asset_error_response(&snapshot.service, error),
    };
    let store = match open_store_async(
        Arc::clone(&snapshot.service),
        snapshot.workspace_root,
        snapshot.binding,
    )
    .await
    {
        Ok(store) => store,
        Err(error) => return asset_error_response(&snapshot.service, error),
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
            return asset_error_response(&snapshot.service, error);
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
                return asset_error_response(&snapshot.service, asset_error);
            }
        };
        if field.name() != Some("file") {
            return asset_error_response(
                &snapshot.service,
                AssetError::Invalid("asset upload contains an unknown multipart field".into()),
            );
        }
        let name = match sanitize_upload_name(field.file_name()) {
            Ok(name) => name,
            Err(error) => return asset_error_response(&snapshot.service, error),
        };
        let chunks = field
            .map(|chunk| chunk.map_err(|_| std::io::Error::other("multipart asset stream failed")));
        match store.stage_stream(name, chunks, &mut budget).await {
            Ok(asset) => staged.push(asset),
            Err(error) => return asset_error_response(&snapshot.service, error),
        }
    }

    let refs = match store.persist_batch(&snapshot.runtime_id, staged).await {
        Ok(refs) => refs,
        Err(error) => return asset_error_response(&snapshot.service, error),
    };
    let assets = refs.into_iter().map(uploaded_asset).collect::<Vec<_>>();
    tracing::info!(workspace = %slug, files = assets.len(), bytes = budget.bytes(), "asset upload persisted");
    Json(UploadResponse { ok: true, assets }).into_response()
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
        Err(error) => return asset_error_response(&service, error),
    };
    let verified = match tokio::task::spawn_blocking({
        let store = store.clone();
        let hash = hash.clone();
        move || store.verified_local_asset(&hash)
    })
    .await
    {
        Ok(Ok(verified)) => verified,
        Ok(Err(error)) => return asset_error_response(&service, error),
        Err(_) => {
            return asset_error_response(
                &service,
                AssetError::Store(std::io::Error::other("asset lookup task failed")),
            )
        }
    };
    let verified = Arc::new(verified);
    let etag = format!("\"sha256-{hash}\"");
    if let Err(error) = ensure_serve_capability(Arc::clone(&verified)).await {
        return asset_error_response(&service, error);
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
        Err(_) => return asset_error_response(&service, AssetError::LocalCorruption),
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
                AssetError::Store(std::io::Error::other("asset file open failed")),
            )
        }
    };
    if let Err(error) = ensure_serve_capability(Arc::clone(&verified)).await {
        return asset_error_response(&service, error);
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

fn asset_error_response(service: &AssetService, error: AssetError) -> Response {
    if matches!(
        error,
        AssetError::Store(_)
            | AssetError::Invariant(_)
            | AssetError::StaleBinding
            | AssetError::LocalCorruption
    ) {
        service.store_failures.fetch_add(1, Ordering::Relaxed);
        tracing::warn!(
            error_code = error.error_code(),
            "asset store operation failed"
        );
    }
    error_response(error.status_code(), error.error_code(), &error.to_string())
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
}
