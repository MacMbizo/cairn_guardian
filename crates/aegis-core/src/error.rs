//! Error types and `C1` §5 structured IPC error taxonomy.

use serde::{Deserialize, Serialize};
use thiserror::Error;
use ts_rs::TS;

/// Structured IPC error returned across Tauri commands (`C1` §5).
#[derive(Debug, Clone, Serialize, Deserialize, TS)]
#[ts(export)]
pub struct IpcError {
    pub code: String,
    pub message: String,
    pub retryable: bool,
    pub remediation: Option<String>,
}

/// Core engine errors mapped cleanly to `C1` §5 codes.
#[derive(Debug, Error)]
pub enum AegisError {
    #[error("permission denied: {0}")]
    PermissionDenied(String),

    #[error("cloud placeholder requires opt-in hydration: {0}")]
    CloudPlaceholder(String),

    #[error("unsupported filesystem: {0}")]
    UnsupportedFs(String),

    #[error("file locked: {0}")]
    LockedFile(String),

    #[error("path too long: {0}")]
    PathTooLong(String),

    #[error("parse failed ({format}): {detail}")]
    ParseFailed { format: String, detail: String },

    #[error("volume offline: {0}")]
    VolumeOffline(String),

    #[error("quarantine unavailable: {0}")]
    QuarantineUnavailable(String),

    #[error("policy blocked ({rule}): {detail}")]
    PolicyBlocked { rule: String, detail: String },

    #[error("not found: {0}")]
    NotFound(String),

    #[error("invalid argument: {0}")]
    InvalidInput(String),

    /// The drive at a stored mount point is not the drive a file belongs to (swapped letters).
    #[error("{0}")]
    DriveMismatch(String),

    /// The copy a duplicate delete relies on could not be confirmed right now (spec §7.5 S3).
    #[error("{0}")]
    SurvivorUnavailable(String),

    #[error("rollback conflict detected on batch {0}")]
    RollbackConflict(String),

    #[error("vault locked: {0}")]
    VaultLocked(String),

    #[error("vault error: {0}")]
    Vault(String),

    #[error("database error: {0}")]
    Database(#[from] rusqlite::Error),

    #[error("i/o error: {0}")]
    Io(#[from] std::io::Error),

    #[error("serialization error: {0}")]
    Serde(#[from] serde_json::Error),
}

impl AegisError {
    /// Converts an engine error into a structured `IpcError` (`C1` §5).
    pub fn to_ipc(&self) -> IpcError {
        match self {
            Self::PermissionDenied(msg) => IpcError {
                code: "permission_denied".into(),
                message: msg.clone(),
                retryable: false,
                remediation: Some("Grant folder access permissions and retry.".into()),
            },
            Self::CloudPlaceholder(msg) => IpcError {
                code: "cloud_placeholder".into(),
                message: msg.clone(),
                retryable: false,
                remediation: Some("Opt in per root to hydrate cloud placeholders.".into()),
            },
            Self::UnsupportedFs(msg) => IpcError {
                code: "unsupported_fs".into(),
                message: msg.clone(),
                retryable: false,
                remediation: None,
            },
            Self::LockedFile(msg) => IpcError {
                code: "locked_file".into(),
                message: msg.clone(),
                retryable: true,
                remediation: Some("Close applications holding this file lock and retry.".into()),
            },
            Self::PathTooLong(msg) => IpcError {
                code: "path_too_long".into(),
                message: msg.clone(),
                retryable: false,
                remediation: None,
            },
            Self::ParseFailed { format, detail } => IpcError {
                code: format!("parse_failed:{format}"),
                message: detail.clone(),
                retryable: false,
                remediation: None,
            },
            Self::VolumeOffline(msg) => IpcError {
                code: "volume_offline".into(),
                message: msg.clone(),
                retryable: true,
                remediation: Some("Reconnect the drive and run reconciliation.".into()),
            },
            Self::QuarantineUnavailable(msg) => IpcError {
                code: "quarantine_unavailable".into(),
                message: msg.clone(),
                retryable: true,
                remediation: Some("Ensure volume is online and writable for .aegis-quarantine.".into()),
            },
            Self::DriveMismatch(msg) => IpcError {
                code: "drive_mismatch".into(),
                message: msg.clone(),
                retryable: true,
                remediation: Some("Plug the right drive back in, let Aegis recognise it, then try again.".into()),
            },
            Self::SurvivorUnavailable(msg) => IpcError {
                code: "survivor_unavailable".into(),
                message: msg.clone(),
                retryable: true,
                remediation: Some("Plug in the drive holding the other copy, or keep both.".into()),
            },
            Self::PolicyBlocked { rule, detail } => IpcError {
                code: format!("policy_blocked:{rule}"),
                message: detail.clone(),
                retryable: false,
                remediation: Some("Review policy gate or confirm explicit override if permitted.".into()),
            },
            Self::NotFound(msg) => IpcError {
                code: "not_found".into(),
                message: msg.clone(),
                retryable: false,
                remediation: None,
            },
            Self::InvalidInput(msg) => IpcError {
                code: "invalid_input".into(),
                message: msg.clone(),
                retryable: false,
                remediation: None,
            },
            Self::RollbackConflict(batch) => IpcError {
                code: "rollback_conflict".into(),
                message: format!("External changes detected on batch {batch}"),
                retryable: false,
                remediation: Some("Resolve each conflict in the 3-way rollback dialog.".into()),
            },
            Self::VaultLocked(msg) => IpcError {
                code: "vault_locked".into(),
                message: msg.clone(),
                retryable: true,
                remediation: Some("Enter your 24-word recovery key to unlock the database.".into()),
            },
            Self::Vault(msg) => IpcError {
                code: "vault_error".into(),
                message: msg.clone(),
                retryable: false,
                remediation: None,
            },
            Self::Database(e) => IpcError {
                code: "database_error".into(),
                message: e.to_string(),
                retryable: false,
                remediation: Some("Run `aegis doctor` to check SQLite integrity.".into()),
            },
            Self::Io(e) => IpcError {
                code: "io_error".into(),
                message: e.to_string(),
                retryable: true,
                remediation: None,
            },
            Self::Serde(e) => IpcError {
                code: "serde_error".into(),
                message: e.to_string(),
                retryable: false,
                remediation: None,
            },
        }
    }
}

pub type Result<T> = std::result::Result<T, AegisError>;
