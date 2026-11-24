//! Capability #12: Self-Healing Workspace (`4.1`).
//!
//! Provides dynamic Markdown/wikilink repair across moves tracked via USN/scanner (`heal_move_log`),
//! internal history (`file_history`), and executed batches, alongside automated YAML/inline tag
//! consolidation, hex-color false-positive detection, and stale automation rule path audits —
//! every disk edit executing strictly through `K1` (`ActionType::PatchText` + Simulator + undo).

pub mod engine;
pub mod markdown;
pub mod tags;

use serde::{Deserialize, Serialize};
use ts_rs::TS;

pub use engine::{
    HEALING_AGENT, dismiss_heal_issue, heal_folderless_rule_path, list_heal_batches,
    preview_heal_patches, propose_heal_batch, scan_workspace_healing, undismiss_heal_issue,
};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "snake_case")]
#[ts(export)]
pub enum HealLinkKind {
    MarkdownLink,
    MarkdownImage,
    Wikilink,
    WikiEmbed,
    ReferenceDef,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "snake_case")]
#[ts(export)]
pub enum HealResolutionStatus {
    Resolved,
    InQuarantine,
    Ambiguous,
    Unresolved,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "snake_case")]
#[ts(export)]
pub enum HealResolutionSource {
    HealMoveLog,
    FileHistory,
    ActionLog,
    Blake3,
    UniqueStem,
    Quarantine,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, TS)]
#[ts(export)]
pub struct BrokenLinkItem {
    pub issue_key: String,
    pub source_file_id: String,
    pub source_path: String,
    pub source_name: String,
    pub drive_label: String,
    pub kind: HealLinkKind,
    #[ts(type = "number")]
    pub line_number: i64,
    pub raw_syntax: String,
    pub broken_target: String,
    pub resolved_file_id: Option<String>,
    pub resolved_path: Option<String>,
    pub suggested_target: Option<String>,
    pub replacement_syntax: Option<String>,
    pub status: HealResolutionStatus,
    pub resolution_source: Option<HealResolutionSource>,
    pub confidence: f64,
    pub quarantine_entry_id: Option<String>,
    pub candidate_paths: Vec<String>,
    pub sensitive: bool,
    pub dismissed: bool,
    pub reason: String,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "snake_case")]
#[ts(export)]
pub enum TagWeedKind {
    CaseMismatch,
    TypoOrVariant,
    HexColorFalsePositive,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, TS)]
#[ts(export)]
pub struct TagOccurrence {
    pub issue_key: String,
    pub file_id: String,
    pub file_path: String,
    pub file_name: String,
    #[ts(type = "number")]
    pub line_number: i64,
    pub in_frontmatter: bool,
    pub raw_tag: String,
    pub canonical_tag: String,
    pub patchable: bool,
    pub sensitive: bool,
    pub snippet: String,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, TS)]
#[ts(export)]
pub struct TagWeedCluster {
    pub issue_key: String,
    pub kind: TagWeedKind,
    pub canonical_tag: String,
    pub variants: Vec<String>,
    #[ts(type = "number")]
    pub total_occurrences: i64,
    #[ts(type = "number")]
    pub patchable_count: i64,
    pub confidence: f64,
    pub dismissed: bool,
    pub reason: String,
    pub occurrences: Vec<TagOccurrence>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, TS)]
#[ts(export)]
pub struct RulePathIssue {
    pub issue_key: String,
    pub rule_id: String,
    pub rule_name: String,
    pub field: String,
    pub current_path: String,
    pub suggested_path: Option<String>,
    pub status: HealResolutionStatus,
    pub dismissed: bool,
    pub reason: String,
}

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize, TS)]
#[ts(export)]
pub struct HealScanOptions {
    pub scope_root: Option<String>,
    pub include_sensitive: bool,
    pub include_dismissed: bool,
    #[ts(type = "number | null")]
    pub limit_notes: Option<usize>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, TS)]
#[ts(export)]
pub struct HealScanReport {
    #[ts(type = "number")]
    pub notes_scanned: i64,
    #[ts(type = "number")]
    pub links_checked: i64,
    #[ts(type = "number")]
    pub broken_links_count: i64,
    #[ts(type = "number")]
    pub healable_links_count: i64,
    #[ts(type = "number")]
    pub tag_weeds_count: i64,
    #[ts(type = "number")]
    pub rule_path_issues_count: i64,
    #[ts(type = "number")]
    pub excluded_sensitive_count: i64,
    pub relational_integrity_score: f64,
    pub taxonomic_hygiene_score: f64,
    #[ts(type = "number")]
    pub move_log_entries: i64,
    pub broken_links: Vec<BrokenLinkItem>,
    pub tag_clusters: Vec<TagWeedCluster>,
    pub rule_issues: Vec<RulePathIssue>,
    #[ts(type = "number")]
    pub duration_ms: i64,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, TS)]
#[ts(export)]
pub struct HealPatchPreviewFile {
    pub file_id: String,
    pub file_path: String,
    pub file_name: String,
    #[ts(type = "number")]
    pub links_healed: i64,
    #[ts(type = "number")]
    pub tags_pruned: i64,
    pub diff_preview: String,
    pub blocked_reason: Option<String>,
    pub sensitive: bool,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, TS)]
#[ts(export)]
pub struct HealBatchSummary {
    pub batch_id: String,
    pub status: String,
    #[ts(type = "number")]
    pub files_count: i64,
    #[ts(type = "number")]
    pub executed_count: i64,
    #[ts(type = "number")]
    pub rolled_back_count: i64,
    pub can_undo: bool,
    #[ts(type = "number")]
    pub created_at: i64,
    pub summary_label: String,
}
