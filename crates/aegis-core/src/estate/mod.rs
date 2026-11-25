//! Capability #15: Digital Estate Management (`5.3`, reshaped `R10`).
//!
//! Provides trustee succession directories (`D3`), GF(256) $K$-of-$N$ Shamir key sharding over
//! the `D1` `DbKey` (storing metadata only, never raw shard secrets in `aegis.db`), `R2`/`R7`
//! CredUI-gated succession packets & Continuity Kit exports, and a strictly non-destructive
//! inactivity heartbeat (`R10`: never shreds, deletes, or locks owner files).

pub mod engine;
pub mod shamir;

use serde::{Deserialize, Serialize};
use ts_rs::TS;

pub use engine::{
    build_estate_dashboard, compute_heartbeat_status, delete_estate_packet, delete_estate_trustee,
    export_estate_continuity_kit, generate_estate_shard_set, mark_estate_shard_delivered,
    record_estate_checkin, revoke_estate_shard_set, save_estate_heartbeat_config,
    save_estate_packet, save_estate_trustee, verify_estate_shard_quorum,
};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "snake_case")]
#[ts(export)]
pub enum TrusteeRole {
    PrimaryExecutor,
    CoTrustee,
    Beneficiary,
    LegalCounsel,
}

impl TrusteeRole {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::PrimaryExecutor => "primary_executor",
            Self::CoTrustee => "co_trustee",
            Self::Beneficiary => "beneficiary",
            Self::LegalCounsel => "legal_counsel",
        }
    }

    pub fn from_db(s: &str) -> Self {
        match s {
            "primary_executor" => Self::PrimaryExecutor,
            "beneficiary" => Self::Beneficiary,
            "legal_counsel" => Self::LegalCounsel,
            _ => Self::CoTrustee,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, TS)]
#[ts(export)]
pub struct EstateTrustee {
    pub id: String,
    pub name: String,
    pub relationship: String,
    pub contact_hint: String,
    pub role: TrusteeRole,
    pub notes: String,
    #[ts(type = "number")]
    pub created_at: i64,
    #[ts(type = "number")]
    pub updated_at: i64,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, TS)]
#[ts(export)]
pub struct EstateTrusteeInput {
    pub id: Option<String>,
    pub name: String,
    pub relationship: String,
    pub contact_hint: String,
    pub role: TrusteeRole,
    pub notes: String,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "snake_case")]
#[ts(export)]
pub enum EstatePacketKind {
    Letter,
    LegalInstructions,
    AccountDirectory,
    DriveMap,
}

impl EstatePacketKind {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Letter => "letter",
            Self::LegalInstructions => "legal_instructions",
            Self::AccountDirectory => "account_directory",
            Self::DriveMap => "drive_map",
        }
    }

    pub fn from_db(s: &str) -> Self {
        match s {
            "legal_instructions" => Self::LegalInstructions,
            "account_directory" => Self::AccountDirectory,
            "drive_map" => Self::DriveMap,
            _ => Self::Letter,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, TS)]
#[ts(export)]
pub struct EstatePacket {
    pub id: String,
    pub title: String,
    pub trustee_id: Option<String>,
    pub trustee_name: Option<String>,
    pub packet_kind: EstatePacketKind,
    pub body_markdown: String,
    pub include_backup_map: bool,
    pub include_project_inventory: bool,
    pub masked: bool,
    #[ts(type = "number")]
    pub created_at: i64,
    #[ts(type = "number")]
    pub updated_at: i64,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, TS)]
#[ts(export)]
pub struct EstatePacketInput {
    pub id: Option<String>,
    pub title: String,
    pub trustee_id: Option<String>,
    pub packet_kind: EstatePacketKind,
    pub body_markdown: String,
    pub include_backup_map: bool,
    pub include_project_inventory: bool,
}

/// Non-secret metadata stored in `aegis.db`; raw shard secrets are NEVER persisted (`D1`, `R2`).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, TS)]
#[ts(export)]
pub struct EstateShardRecord {
    pub id: String,
    pub shard_set_id: String,
    pub trustee_id: Option<String>,
    pub trustee_name: Option<String>,
    pub share_index: u8,
    pub threshold_k: u8,
    pub total_n: u8,
    pub key_fingerprint: String,
    pub checksum_prefix: String,
    #[ts(type = "number | null")]
    pub delivered_at: Option<i64>,
    #[ts(type = "number | null")]
    pub verified_at: Option<i64>,
    #[ts(type = "number | null")]
    pub revoked_at: Option<i64>,
    #[ts(type = "number")]
    pub created_at: i64,
}

/// One-time generated shard payload returned to the owner for offline trustee distribution.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, TS)]
#[ts(export)]
pub struct EstateGeneratedShard {
    pub record_id: String,
    pub trustee_id: Option<String>,
    pub trustee_name: Option<String>,
    pub share_index: u8,
    pub checksum_prefix: String,
    pub shard_token: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, TS)]
#[ts(export)]
pub struct EstateShardSetBundle {
    pub shard_set_id: String,
    pub threshold_k: u8,
    pub total_n: u8,
    pub key_fingerprint: String,
    pub shards: Vec<EstateGeneratedShard>,
    #[ts(type = "number")]
    pub created_at: i64,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, TS)]
#[ts(export)]
pub struct EstateShardVerifyResult {
    pub valid: bool,
    pub shard_set_id: String,
    pub threshold_k: u8,
    pub total_n: u8,
    pub shares_checked: u8,
    pub matches_active_vault: bool,
    pub key_fingerprint: String,
    pub detail: String,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "snake_case")]
#[ts(export)]
pub enum EstateHeartbeatState {
    Disabled,
    Active,
    Warning,
    GracePeriod,
    ReadyForRelease,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, TS)]
#[ts(export)]
pub struct EstateHeartbeatConfig {
    pub enabled: bool,
    #[ts(type = "number")]
    pub interval_days: i64,
    #[ts(type = "number")]
    pub grace_days: i64,
    #[ts(type = "number")]
    pub last_checkin_at: i64,
    pub non_destructive_policy: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, TS)]
#[ts(export)]
pub struct EstateHeartbeatStatus {
    pub config: EstateHeartbeatConfig,
    pub state: EstateHeartbeatState,
    #[ts(type = "number | null")]
    pub next_due_at: Option<i64>,
    #[ts(type = "number | null")]
    pub grace_ends_at: Option<i64>,
    #[ts(type = "number | null")]
    pub days_remaining: Option<i64>,
    pub r10_guarantee: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, TS)]
#[ts(export)]
pub struct EstateCheckinRecord {
    pub id: String,
    pub method: String,
    pub note: String,
    #[ts(type = "number")]
    pub checked_in_at: i64,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, TS)]
#[ts(export)]
pub struct EstateContinuityExport {
    pub output_dir: String,
    pub manifest_path: String,
    pub files_written: Vec<String>,
    #[ts(type = "number")]
    pub exported_at: i64,
    pub r10_notice: String,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, TS)]
#[ts(export)]
pub struct EstateDashboard {
    pub readiness_score: f64,
    pub readiness_grade: String,
    pub db_encrypted: bool,
    pub recovery_confirmed: bool,
    #[ts(type = "number")]
    pub backup_drives_count: i64,
    #[ts(type = "number")]
    pub backup_drives_online: i64,
    pub heartbeat: EstateHeartbeatStatus,
    pub trustees: Vec<EstateTrustee>,
    pub shard_records: Vec<EstateShardRecord>,
    pub packets: Vec<EstatePacket>,
    pub packets_masked: bool,
    pub recent_checkins: Vec<EstateCheckinRecord>,
    pub r10_policy_summary: String,
    pub r8_policy_summary: String,
}
