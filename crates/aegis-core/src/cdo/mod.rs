//! Capability #14: AI Chief Digital Officer (CDO) Agent (`5.2`).
//!
//! Synthesizes cross-project code-vs-documentation velocity, workflow filing & security
//! bottlenecks, governance posture, and operational health into reversible `K1` proposals and
//! grounded on-demand local LLM executive briefings (`K1`–`K5`, `R4`–`R9`, `R11`, `D1`).

pub mod engine;
pub mod patterns;

use serde::{Deserialize, Serialize};
use ts_rs::TS;

pub use engine::{
    CDO_AGENT, build_cdo_dashboard, build_cdo_dashboard_with_drives, list_cdo_briefings,
    list_cdo_ratifications, prepare_cdo_briefing_prompt, ratify_cdo_recommendation,
    save_cdo_briefing_snapshot, set_cdo_recommendation_status, undo_cdo_ratification,
};

use crate::engine::Screen;
use crate::folderless::FolderlessRuleInput;
use crate::governance::GovernancePrivacyRuleInput;

/// Five executive pillars scored across Cairn subsystems.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize, TS)]
#[serde(rename_all = "snake_case")]
#[ts(export)]
pub enum CdoPillarId {
    StorageAndBackup,
    SecurityAndPrivacy,
    WorkspaceHygiene,
    KnowledgeAndAlignment,
    AutomationVelocity,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, TS)]
#[ts(export)]
pub struct CdoPillarScore {
    pub id: CdoPillarId,
    pub label: String,
    pub score: f64,
    pub weight: f64,
    pub status: String,
    pub detail: String,
    pub target_screen: Screen,
}

/// Executive bottleneck diagnostics synthesized strictly from saved disk artifacts (`R5`, `R11`).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize, TS)]
#[serde(rename_all = "snake_case")]
#[ts(export)]
pub enum CdoBottleneckKind {
    DocLagBehindCode,
    ManualFilingFriction,
    SecurityExposureDebt,
    LinkAndTaxonomyDrift,
    CrossProjectFriction,
    KnowledgeBacklog,
    StorageAndBackupRisk,
    DormantAndVersionBloat,
    PendingReviewQueue,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize, TS)]
#[serde(rename_all = "snake_case")]
#[ts(export)]
pub enum CdoSeverity {
    Low,
    Medium,
    High,
    Critical,
}

impl CdoSeverity {
    pub fn rank(self) -> u8 {
        match self {
            Self::Critical => 4,
            Self::High => 3,
            Self::Medium => 2,
            Self::Low => 1,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "snake_case")]
#[ts(export)]
pub enum CdoRecommendationStatus {
    Open,
    Ratified,
    Snoozed,
    Dismissed,
}

impl CdoRecommendationStatus {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Open => "open",
            Self::Ratified => "ratified",
            Self::Snoozed => "snoozed",
            Self::Dismissed => "dismissed",
        }
    }

    pub fn from_db(s: &str) -> Self {
        match s {
            "ratified" => Self::Ratified,
            "snoozed" => Self::Snoozed,
            "dismissed" => Self::Dismissed,
            _ => Self::Open,
        }
    }
}

/// Concrete `K1` action attached to a CDO proposal.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(tag = "type", rename_all = "snake_case")]
#[ts(export)]
pub enum CdoProposalAction {
    CreateFolderlessRule {
        draft: Box<FolderlessRuleInput>,
    },
    CreatePrivacyRule {
        draft: GovernancePrivacyRuleInput,
    },
    AdjustSetting {
        key: String,
        scope: String,
        value_json: String,
        display_label: String,
    },
    NavigateToScreen {
        screen: Screen,
        label: String,
    },
}

/// Per-project code vs documentation velocity pulse derived from disk mtimes & entities (`R5`, `R11`).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, TS)]
#[ts(export)]
pub struct CdoProjectPulse {
    pub collection_id: String,
    pub name: String,
    pub project_kind: Option<String>,
    #[ts(type = "number")]
    pub code_files: i64,
    #[ts(type = "number")]
    pub doc_files: i64,
    #[ts(type = "number")]
    pub recent_7d_files: i64,
    #[ts(type = "number")]
    pub recent_30d_files: i64,
    #[ts(type = "number | null")]
    pub newest_code_mtime: Option<i64>,
    #[ts(type = "number | null")]
    pub newest_doc_mtime: Option<i64>,
    #[ts(type = "number | null")]
    pub doc_lag_days: Option<i64>,
    #[ts(type = "number")]
    pub open_action_items: i64,
    #[ts(type = "number")]
    pub decisions_count: i64,
    pub health_flag: String,
}

/// Workspace-wide operational velocity and automation leverage snapshot (`R5`, `R11`).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, TS)]
#[ts(export)]
pub struct CdoVelocitySnapshot {
    #[ts(type = "number")]
    pub horizon_days: i64,
    #[ts(type = "number")]
    pub files_touched_7d: i64,
    #[ts(type = "number")]
    pub files_touched_30d: i64,
    #[ts(type = "number")]
    pub batches_executed_30d: i64,
    #[ts(type = "number")]
    pub batches_rolled_back_30d: i64,
    pub filing_automation_ratio: f64,
    #[ts(type = "number")]
    pub loose_inbox_files: i64,
    #[ts(type = "number")]
    pub loose_invoice_docs: i64,
    #[ts(type = "number")]
    pub open_action_items: i64,
    #[ts(type = "number")]
    pub pending_knowledge_proposals: i64,
    #[ts(type = "number")]
    pub pending_action_batches: i64,
    #[ts(type = "number")]
    pub active_folderless_rules: i64,
    #[ts(type = "number")]
    pub active_privacy_rules: i64,
    #[ts(type = "number")]
    pub open_security_findings: i64,
    pub observation_mode: String,
}

/// Diagnosed workflow friction point backed strictly by disk & catalog evidence.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, TS)]
#[ts(export)]
pub struct CdoBottleneck {
    pub id: String,
    pub kind: CdoBottleneckKind,
    pub severity: CdoSeverity,
    pub title: String,
    pub diagnosis: String,
    pub impact_metric: String,
    pub affected_projects: Vec<String>,
    pub evidence_paths: Vec<String>,
    pub recommended_screen: Screen,
}

/// Actionable strategic proposal waiting for owner ratification or navigation (`K1`).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, TS)]
#[ts(export)]
pub struct CdoRecommendation {
    pub rec_key: String,
    pub title: String,
    pub rationale: String,
    pub bottleneck_kind: CdoBottleneckKind,
    pub severity: CdoSeverity,
    pub status: CdoRecommendationStatus,
    pub owner_note: String,
    pub can_ratify_directly: bool,
    pub proposal: CdoProposalAction,
}

/// Reversible record of an owner-ratified CDO structural change (`K1`).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, TS)]
#[ts(export)]
pub struct CdoRatificationRecord {
    pub id: String,
    pub rec_key: String,
    pub rec_title: String,
    pub action_kind: String,
    pub target_ref: String,
    pub applied_summary: String,
    pub status: String,
    #[ts(type = "number")]
    pub created_at: i64,
    #[ts(type = "number | null")]
    pub undone_at: Option<i64>,
    pub can_undo: bool,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, TS)]
#[ts(export)]
pub struct CdoBriefingCitation {
    #[ts(type = "number")]
    pub ref_id: usize,
    pub title: String,
    pub source_kind: String,
    pub detail: String,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, TS)]
#[ts(export)]
pub struct CdoExecutiveBriefing {
    pub id: String,
    #[ts(type = "number")]
    pub horizon_days: i64,
    pub executive_score: f64,
    pub headline: String,
    pub summary_markdown: String,
    pub citations: Vec<CdoBriefingCitation>,
    pub model_id: String,
    pub model_name: String,
    pub backend: String,
    #[ts(type = "number")]
    pub created_at: i64,
}

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize, TS)]
#[ts(export)]
pub struct CdoScanOptions {
    #[ts(type = "number | null")]
    pub horizon_days: Option<i64>,
    pub include_sensitive: bool,
    pub include_dismissed: bool,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, TS)]
#[ts(export)]
pub struct CdoDashboard {
    pub executive_score: f64,
    pub executive_grade: String,
    pub posture_summary: String,
    pub pillars: Vec<CdoPillarScore>,
    pub velocity: CdoVelocitySnapshot,
    pub project_pulses: Vec<CdoProjectPulse>,
    pub bottlenecks: Vec<CdoBottleneck>,
    pub recommendations: Vec<CdoRecommendation>,
    pub recent_ratifications: Vec<CdoRatificationRecord>,
    pub latest_briefing: Option<CdoExecutiveBriefing>,
    #[ts(type = "number")]
    pub excluded_sensitive_count: i64,
    #[ts(type = "number")]
    pub generated_at: i64,
    #[ts(type = "number")]
    pub took_ms: i64,
}

/// Prepared prompt context for off-lock on-demand local LLM briefing generation (`K5`, `R4`).
#[derive(Debug, Clone)]
pub struct PreparedCdoBriefing {
    pub horizon_days: i64,
    pub executive_score: f64,
    pub headline: String,
    pub system_prompt: String,
    pub user_prompt: String,
    pub citations: Vec<CdoBriefingCitation>,
}
