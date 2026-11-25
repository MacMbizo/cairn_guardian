//! Capability #13: Personal Data Governance (`5.1`, reshaped `R7`–`R9`).
//!
//! Provides local pure-Rust PII & secret scanning (`PaymentCard`, `PrivateKeyBlock`,
//! `CloudApiKey`, `GovernmentId`, `BankAccountOrIban`, `CredentialFile`), redacted snippet storage,
//! reversible `R7` sensitivity classification batches (`K1` undo), pattern-based privacy & AI
//! exclusion rules, `K1`/`K2`/`R8` 30-day quarantine staging, non-admin BitLocker telemetry (`R1`),
//! honest code-enforced `R9` zero-network audit, and `K4`/`R11` opt-in observation audit.

pub mod detector;
pub mod engine;
pub mod posture;

use serde::{Deserialize, Serialize};
use ts_rs::TS;

pub use engine::{
    GOVERNANCE_AGENT, apply_privacy_rule, apply_sensitivity_labels, delete_privacy_rule,
    is_path_excluded_from_ai, list_governance_dashboard, list_privacy_rules,
    propose_governance_quarantine, save_privacy_rule, scan_personal_data, set_finding_status,
    undo_sensitivity_batch,
};
pub use posture::{build_privacy_posture, build_privacy_posture_with_drives};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize, TS)]
#[serde(rename_all = "snake_case")]
#[ts(export)]
pub enum GovernanceFindingType {
    PaymentCard,
    PrivateKeyBlock,
    CloudApiKey,
    GovernmentId,
    BankAccountOrIban,
    CredentialFile,
}

impl GovernanceFindingType {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::PaymentCard => "payment_card",
            Self::PrivateKeyBlock => "private_key_block",
            Self::CloudApiKey => "cloud_api_key",
            Self::GovernmentId => "government_id",
            Self::BankAccountOrIban => "bank_account_or_iban",
            Self::CredentialFile => "credential_file",
        }
    }

    pub fn from_db(s: &str) -> Self {
        match s {
            "payment_card" => Self::PaymentCard,
            "private_key_block" => Self::PrivateKeyBlock,
            "cloud_api_key" => Self::CloudApiKey,
            "government_id" => Self::GovernmentId,
            "bank_account_or_iban" => Self::BankAccountOrIban,
            _ => Self::CredentialFile,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize, TS)]
#[serde(rename_all = "snake_case")]
#[ts(export)]
pub enum GovernanceSeverity {
    Low,
    Medium,
    High,
    Critical,
}

impl GovernanceSeverity {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Low => "low",
            Self::Medium => "medium",
            Self::High => "high",
            Self::Critical => "critical",
        }
    }

    pub fn from_db(s: &str) -> Self {
        match s {
            "critical" => Self::Critical,
            "high" => Self::High,
            "medium" => Self::Medium,
            _ => Self::Low,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "snake_case")]
#[ts(export)]
pub enum GovernanceFindingStatus {
    Open,
    Vaulted,
    AcceptedRisk,
    FalsePositive,
    Resolved,
}

impl GovernanceFindingStatus {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Open => "open",
            Self::Vaulted => "vaulted",
            Self::AcceptedRisk => "accepted_risk",
            Self::FalsePositive => "false_positive",
            Self::Resolved => "resolved",
        }
    }

    pub fn from_db(s: &str) -> Option<Self> {
        match s {
            "open" => Some(Self::Open),
            "vaulted" => Some(Self::Vaulted),
            "accepted_risk" => Some(Self::AcceptedRisk),
            "false_positive" => Some(Self::FalsePositive),
            "resolved" => Some(Self::Resolved),
            _ => None,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "snake_case")]
#[ts(export)]
pub enum GovernanceRuleAction {
    AutoConfidential,
    AutoSecret,
    ExcludeFromAi,
}

impl GovernanceRuleAction {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::AutoConfidential => "auto_confidential",
            Self::AutoSecret => "auto_secret",
            Self::ExcludeFromAi => "exclude_from_ai",
        }
    }

    pub fn from_db(s: &str) -> Option<Self> {
        match s {
            "auto_confidential" => Some(Self::AutoConfidential),
            "auto_secret" => Some(Self::AutoSecret),
            "exclude_from_ai" => Some(Self::ExcludeFromAi),
            _ => None,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, TS)]
#[ts(export)]
pub struct GovernanceFindingLocation {
    pub file_id: String,
    pub file_path: String,
    pub file_name: String,
    pub drive_label: String,
    pub sensitivity: String,
    #[ts(type = "number")]
    pub line_number: i64,
    pub redacted_snippet: String,
    pub in_atomic_unit: bool,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, TS)]
#[ts(export)]
pub struct GovernanceFinding {
    pub id: String,
    pub finding_type: GovernanceFindingType,
    pub severity: GovernanceSeverity,
    pub context_risk_score: f64,
    pub status: GovernanceFindingStatus,
    #[ts(type = "number")]
    pub created_at: i64,
    #[ts(type = "number | null")]
    pub resolved_at: Option<i64>,
    pub summary: String,
    pub recommended_action: String,
    pub locations: Vec<GovernanceFindingLocation>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, TS)]
#[ts(export)]
pub struct SensitiveFileSummary {
    pub file_id: String,
    pub path: String,
    pub name: String,
    pub drive_label: String,
    pub sensitivity: String,
    #[ts(type = "number")]
    pub size_bytes: i64,
    #[ts(type = "number")]
    pub mtime: i64,
    #[ts(type = "number")]
    pub open_findings_count: i64,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, TS)]
#[ts(export)]
pub struct GovernancePrivacyRule {
    pub id: String,
    pub name: String,
    pub path_prefix: Option<String>,
    pub filename_pattern: Option<String>,
    pub rule_action: GovernanceRuleAction,
    pub enabled: bool,
    #[ts(type = "number")]
    pub created_at: i64,
    #[ts(type = "number")]
    pub updated_at: i64,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, TS)]
#[ts(export)]
pub struct GovernancePrivacyRuleInput {
    pub id: Option<String>,
    pub name: String,
    pub path_prefix: Option<String>,
    pub filename_pattern: Option<String>,
    pub rule_action: GovernanceRuleAction,
    pub enabled: Option<bool>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, TS)]
#[ts(export)]
pub struct GovernanceLabelBatch {
    pub id: String,
    pub target_sensitivity: String,
    #[ts(type = "number")]
    pub file_count: i64,
    pub reason: String,
    pub status: String,
    #[ts(type = "number")]
    pub created_at: i64,
    #[ts(type = "number | null")]
    pub undone_at: Option<i64>,
    pub can_undo: bool,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, TS)]
#[ts(export)]
pub struct DriveEncryptionStatus {
    pub mount_point: String,
    pub label: String,
    pub protection_status: String,
    pub encryption_method: String,
    pub recommended_action: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, TS)]
#[ts(export)]
pub struct OptInCapabilityAudit {
    pub id: String,
    pub label: String,
    pub state: String,
    pub rule_ref: String,
    pub detail: String,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, TS)]
#[ts(export)]
pub struct PrivacyPostureReport {
    pub privacy_score: f64,
    pub db_encrypted: bool,
    pub cipher_label: String,
    pub dpapi_sealed: bool,
    pub recovery_confirmed: bool,
    pub sensitive_unlocked: bool,
    #[ts(type = "number | null")]
    pub sensitive_expires_at: Option<i64>,
    pub zero_network_status: String,
    pub firewall_command: String,
    pub shredding_policy: String,
    pub drives_encryption: Vec<DriveEncryptionStatus>,
    pub opt_in_audits: Vec<OptInCapabilityAudit>,
    #[ts(type = "number")]
    pub open_findings_count: i64,
    #[ts(type = "number")]
    pub vaulted_findings_count: i64,
    #[ts(type = "number")]
    pub confidential_files_count: i64,
    #[ts(type = "number")]
    pub secret_files_count: i64,
    #[ts(type = "number")]
    pub active_rules_count: i64,
    #[ts(type = "number")]
    pub quarantined_items_count: i64,
}

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize, TS)]
#[ts(export)]
pub struct GovernanceScanOptions {
    pub scope_root: Option<String>,
    pub include_sensitive: bool,
    pub force_rescan: bool,
    #[ts(type = "number | null")]
    pub max_files: Option<usize>,
}

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize, TS)]
#[ts(export)]
pub struct GovernanceQueryOptions {
    pub status_filter: Option<GovernanceFindingStatus>,
    pub severity_filter: Option<GovernanceSeverity>,
    pub include_sensitive: bool,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, TS)]
#[ts(export)]
pub struct GovernanceScanReport {
    #[ts(type = "number")]
    pub files_inspected: i64,
    #[ts(type = "number")]
    pub files_skipped_unchanged: i64,
    #[ts(type = "number")]
    pub excluded_sensitive_count: i64,
    #[ts(type = "number")]
    pub findings_discovered: i64,
    #[ts(type = "number")]
    pub findings_resolved: i64,
    #[ts(type = "number")]
    pub duration_ms: i64,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, TS)]
#[ts(export)]
pub struct GovernanceDashboard {
    pub posture: PrivacyPostureReport,
    pub findings: Vec<GovernanceFinding>,
    pub sensitive_files: Vec<SensitiveFileSummary>,
    pub rules: Vec<GovernancePrivacyRule>,
    pub label_batches: Vec<GovernanceLabelBatch>,
    #[ts(type = "number")]
    pub excluded_sensitive_count: i64,
}
