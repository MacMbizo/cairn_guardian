//! Cross-Project Intelligence (`3.3`) (`K1`–`K5`, `R4`, `R5`, `R7`, `R9`, `R11`, `D1`).
//!
//! Synthesizes multi-project metadata, duplicate effort, conflicting architectural decisions,
//! stack version divergence, and cross-functional dependency links strictly from saved files,
//! encrypted `vec_chunks`, and Knowledge Graph entities on disk — never watching screen or input.

pub mod engine;
pub mod manifests;

pub use engine::{
    PreparedCrossProjectBrief, prepare_cross_project_brief, scan_cross_project_intelligence,
    set_insight_status,
};

use serde::{Deserialize, Serialize};
use ts_rs::TS;

/// Category of cross-project overlap or conflict surfaced (`3.3`).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "snake_case")]
#[ts(export)]
pub enum FindingKind {
    DuplicateEffort,
    ConflictingDecision,
    VersionDivergence,
}

/// Severity assigned to a cross-project finding.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize, TS)]
#[serde(rename_all = "snake_case")]
#[ts(export)]
pub enum FindingSeverity {
    Low,
    Medium,
    High,
}

/// Persisted triage state for a cross-project insight (`K1`, `D1`).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "snake_case")]
#[ts(export)]
pub enum InsightStatus {
    Open,
    Acknowledged,
    Resolved,
    Dismissed,
}

impl InsightStatus {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Open => "open",
            Self::Acknowledged => "acknowledged",
            Self::Resolved => "resolved",
            Self::Dismissed => "dismissed",
        }
    }

    pub fn from_db(s: &str) -> Self {
        match s.trim().to_ascii_lowercase().as_str() {
            "acknowledged" => Self::Acknowledged,
            "resolved" => Self::Resolved,
            "dismissed" => Self::Dismissed,
            _ => Self::Open,
        }
    }
}

/// Nature of dependency connecting two distinct projects (`3.3`).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "snake_case")]
#[ts(export)]
pub enum DependencyKind {
    SharedStakeholder,
    SharedDecision,
    CrossProjectAction,
    DocumentReference,
    SharedStack,
}

/// Aggregated intelligence profile for one active project / design set on disk.
#[derive(Debug, Clone, Serialize, Deserialize, TS)]
#[ts(export)]
pub struct ProjectNodeSummary {
    pub collection_id: String,
    pub name: String,
    pub kind: String,
    pub project_kind: Option<String>,
    pub root_paths: Vec<String>,
    #[ts(type = "number")]
    pub file_count: usize,
    #[ts(type = "number")]
    pub indexed_doc_count: usize,
    #[ts(type = "number")]
    pub embedded_doc_count: usize,
    #[ts(type = "number")]
    pub total_bytes: i64,
    #[ts(type = "number")]
    pub last_activity_at: i64,
    #[ts(type = "number")]
    pub open_action_count: usize,
    #[ts(type = "number")]
    pub decision_count: usize,
    #[ts(type = "number")]
    pub people_count: usize,
    #[ts(type = "number")]
    pub related_project_count: usize,
    #[ts(type = "number")]
    pub finding_count: usize,
    pub top_terms: Vec<String>,
}

/// Concise reference to a live non-backup file involved in a cross-project link or finding.
#[derive(Debug, Clone, Serialize, Deserialize, TS)]
#[ts(export)]
pub struct CrossProjectFileRef {
    pub file_id: String,
    pub filename: String,
    pub path: String,
    pub collection_id: String,
    pub collection_name: String,
    #[ts(type = "number")]
    pub mtime: i64,
    pub excerpt: String,
}

/// Duplicate effort, decision conflict, or stack version divergence across two projects.
#[derive(Debug, Clone, Serialize, Deserialize, TS)]
#[ts(export)]
pub struct CrossProjectFinding {
    pub insight_key: String,
    pub kind: FindingKind,
    pub severity: FindingSeverity,
    pub status: InsightStatus,
    pub note: String,
    pub title: String,
    pub summary: String,
    pub project_a_id: String,
    pub project_a_name: String,
    pub project_b_id: String,
    pub project_b_name: String,
    pub similarity: f64,
    #[ts(type = "number")]
    pub age_delta_days: i64,
    pub shared_terms: Vec<String>,
    pub file_a: Option<CrossProjectFileRef>,
    pub file_b: Option<CrossProjectFileRef>,
}

/// Cross-functional dependency edge linking two distinct projects.
#[derive(Debug, Clone, Serialize, Deserialize, TS)]
#[ts(export)]
pub struct CrossProjectEdge {
    pub edge_key: String,
    pub kind: DependencyKind,
    pub directed: bool,
    pub from_project_id: String,
    pub from_project_name: String,
    pub to_project_id: String,
    pub to_project_name: String,
    pub strength: f64,
    pub summary: String,
    pub shared_labels: Vec<String>,
    pub evidence_files: Vec<CrossProjectFileRef>,
}

/// Filter and threshold options for Cross-Project Intelligence scanning (`R7`).
#[derive(Debug, Clone, Default, Serialize, Deserialize, TS)]
#[ts(export)]
pub struct CrossProjectOptions {
    pub project_id: Option<String>,
    pub kind: Option<FindingKind>,
    pub min_similarity: Option<f64>,
    pub include_design_sets: Option<bool>,
    pub include_dismissed: Option<bool>,
    pub include_sensitive: Option<bool>,
    #[ts(type = "number | null")]
    pub limit_findings: Option<usize>,
    #[ts(type = "number | null")]
    pub limit_edges: Option<usize>,
}

/// Full Cross-Project Intelligence report across active project folders.
#[derive(Debug, Clone, Serialize, Deserialize, TS)]
#[ts(export)]
pub struct CrossProjectReport {
    pub projects: Vec<ProjectNodeSummary>,
    pub findings: Vec<CrossProjectFinding>,
    pub edges: Vec<CrossProjectEdge>,
    #[ts(type = "number")]
    pub total_projects: usize,
    #[ts(type = "number")]
    pub duplicate_effort_count: usize,
    #[ts(type = "number")]
    pub conflict_count: usize,
    #[ts(type = "number")]
    pub dependency_edge_count: usize,
    #[ts(type = "number")]
    pub dismissed_count: usize,
    #[ts(type = "number")]
    pub excluded_sensitive_count: usize,
    pub used_vectors: bool,
    #[ts(type = "number")]
    pub took_ms: u64,
}

/// Grounded source citation backing an on-demand executive cross-project synthesis.
#[derive(Debug, Clone, Serialize, Deserialize, TS)]
#[ts(export)]
pub struct CrossProjectBriefCitation {
    #[ts(type = "number")]
    pub ref_id: usize,
    pub file_id: String,
    pub filename: String,
    pub path: String,
    pub project_name: String,
    pub excerpt: String,
}

/// On-demand Local LLM synthesis summarizing cross-project overlaps, conflicts, and dependencies.
#[derive(Debug, Clone, Serialize, Deserialize, TS)]
#[ts(export)]
pub struct CrossProjectBrief {
    pub title: String,
    pub summary: String,
    pub citations: Vec<CrossProjectBriefCitation>,
    pub model_id: String,
    pub model_name: String,
    pub backend: String,
    #[ts(type = "number")]
    pub duration_ms: u64,
}
