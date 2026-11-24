//! Capability #6 (4.3): Non-destructive multi-format Forgotten Asset Recovery (`K1`, `K2`, `K4`, `R7`, `D1`).

pub mod formats;
pub mod recovery;

pub use formats::{AssetFamily, VisualHeader, classify_asset_family, inspect_visual_header};
pub use recovery::{
    AssetDriveSummary, AssetFamilyCount, AssetRecoverItem, AssetScanOptions, ForgottenAsset,
    ForgottenAssetsReport, ForgottenReason, dismiss_forgotten_asset, enrich_asset_metadata,
    propose_asset_recovery, scan_forgotten_assets, undismiss_forgotten_asset,
};
