use axum::http::StatusCode;

#[derive(Debug, thiserror::Error)]
pub enum AssetError {
    #[error("invalid asset: {0}")]
    Invalid(String),
    #[error("asset exceeds the {limit}-byte file limit")]
    TooLarge { limit: u64 },
    #[error("asset request exceeds the {limit}-byte request limit")]
    RequestTooLarge { limit: u64 },
    #[error("asset request exceeds the {limit}-file limit")]
    TooMany { limit: usize },
    #[error("asset quota exceeded: {used} of {quota} bytes already committed or reserved")]
    QuotaExceeded { used: u64, quota: u64 },
    #[error("asset store handle is stale")]
    StaleBinding,
    #[error("asset store invariant failed: {0}")]
    Invariant(&'static str),
    #[error("asset store operation failed")]
    Store(
        #[from]
        #[source]
        std::io::Error,
    ),
    #[error("asset is missing")]
    Missing,
    #[error("asset origin is unavailable")]
    OriginUnavailable,
    #[error("local asset data is corrupt")]
    LocalCorruption,
    #[error("asset hash mismatch")]
    HashMismatch,
    #[error("asset peer response is invalid: {0}")]
    PeerInvalid(String),
    #[error("asset origin is forbidden")]
    ForbiddenOrigin,
}

impl AssetError {
    pub const fn status_code(&self) -> StatusCode {
        match self {
            Self::Invalid(_) => StatusCode::BAD_REQUEST,
            Self::TooLarge { .. } | Self::RequestTooLarge { .. } => StatusCode::PAYLOAD_TOO_LARGE,
            Self::TooMany { .. } => StatusCode::UNPROCESSABLE_ENTITY,
            Self::QuotaExceeded { .. } => StatusCode::INSUFFICIENT_STORAGE,
            Self::Store(_) => StatusCode::INSUFFICIENT_STORAGE,
            Self::StaleBinding => StatusCode::CONFLICT,
            Self::Invariant(_) => StatusCode::INTERNAL_SERVER_ERROR,
            Self::Missing => StatusCode::NOT_FOUND,
            Self::OriginUnavailable => StatusCode::SERVICE_UNAVAILABLE,
            Self::LocalCorruption => StatusCode::INTERNAL_SERVER_ERROR,
            Self::HashMismatch | Self::PeerInvalid(_) => StatusCode::BAD_GATEWAY,
            Self::ForbiddenOrigin => StatusCode::FORBIDDEN,
        }
    }

    pub const fn error_code(&self) -> &'static str {
        match self {
            Self::Invalid(_) => "invalid_asset",
            Self::TooLarge { .. } => "asset_too_large",
            Self::RequestTooLarge { .. } => "asset_request_too_large",
            Self::TooMany { .. } => "too_many_assets",
            Self::QuotaExceeded { .. } => "asset_quota_exceeded",
            Self::StaleBinding => "asset_store_stale",
            Self::Invariant(_) => "asset_store_failed",
            Self::Store(_) => "asset_store_failed",
            Self::Missing => "asset_missing",
            Self::OriginUnavailable => "asset_origin_unavailable",
            Self::LocalCorruption => "asset_local_corruption",
            Self::HashMismatch => "asset_hash_mismatch",
            Self::PeerInvalid(_) => "asset_peer_invalid",
            Self::ForbiddenOrigin => "asset_origin_forbidden",
        }
    }
}
