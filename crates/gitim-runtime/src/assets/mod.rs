mod error;
pub(crate) mod http;
mod inspect;
mod resolver;
mod store;

/// Pixel-dimension ceilings for serving an image inline
/// (`Content-Disposition: inline`). Single definition shared by upload-time
/// inspection (`inspect`) and resolve-time serving (`http`) so both sides
/// agree on what may render inline. The frontend mirrors these values in
/// `products/gitim/frontend/src/components/chat/asset-fragment.tsx`.
pub(crate) const MAX_INLINE_IMAGE_AXIS: u32 = 32_768;
/// See [`MAX_INLINE_IMAGE_AXIS`].
pub(crate) const MAX_INLINE_IMAGE_PIXELS: u64 = 100_000_000;

pub use error::AssetError;
pub use inspect::{checked_dimensions, inspect_bytes, AssetInspection};
#[cfg(feature = "test-support")]
pub use resolver::resolve_get as resolve_fleet_asset_for_test;
#[cfg(feature = "test-support")]
pub use store::AssetEvent;
pub use store::{
    AssetHealthSnapshot, AssetLimits, AssetMetadata, AssetReservation, AssetService, AssetSource,
    AssetStore, AssetUsage, AssetWorkspaceToken, HashLock, RequestBudget, StagedAsset,
};
