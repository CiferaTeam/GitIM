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
    #[error("asset store failed: {0}")]
    Store(#[from] std::io::Error),
    #[error("asset is missing")]
    Missing,
    #[error("asset origin is unavailable")]
    OriginUnavailable,
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
            Self::QuotaExceeded { .. } | Self::Store(_) => StatusCode::INSUFFICIENT_STORAGE,
            Self::Missing => StatusCode::NOT_FOUND,
            Self::OriginUnavailable => StatusCode::SERVICE_UNAVAILABLE,
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
            Self::Store(_) => "asset_store_failed",
            Self::Missing => "asset_missing",
            Self::OriginUnavailable => "asset_origin_unavailable",
            Self::HashMismatch => "asset_hash_mismatch",
            Self::PeerInvalid(_) => "asset_peer_invalid",
            Self::ForbiddenOrigin => "asset_origin_forbidden",
        }
    }
}
