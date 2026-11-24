//! Semantic Version Tracking (`3.4`, PRD `FR-004.2`): version chain detection across
//! filename patterns, project `Versions\` folders, `file_history` lineage, and encrypted
//! semantic vectors (`vec_chunks`), plus deterministic/LLM diffing and `K1` archival actions.

pub mod actions;
pub mod chains;
pub mod diff;
pub mod patterns;

use serde::{Deserialize, Serialize};
use ts_rs::TS;

pub use actions::{
    VERSION_TRACKING_AGENT, propose_version_archive, propose_version_quarantine_dupes,
};
pub use chains::{
    dismiss_version_chain, pin_latest_version, scan_version_chains, undismiss_version_chain,
};
pub use diff::{
    build_diff_llm_prompt, diff_version_pair, load_cached_diff_summary, save_cached_diff_summary,
};

/// Signal explaining why files were grouped into a semantic version chain.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize, TS)]
#[serde(rename_all = "snake_case")]
#[ts(export)]
pub enum VersionLinkReason {
    FilenamePattern,
    VersionsFolder,
    FileHistory,
    SemanticSimilarity,
}

impl VersionLinkReason {
    pub fn label(self) -> &'static str {
        match self {
            Self::FilenamePattern => "Filename pattern",
            Self::VersionsFolder => "Versions folder",
            Self::FileHistory => "Rename / move history",
            Self::SemanticSimilarity => "Semantic similarity",
        }
    }
}

/// Filter & query options for scanning semantic version chains (`K2`, `K4`, `R7`).
#[derive(Debug, Clone, Default, Serialize, Deserialize, TS)]
#[serde(default)]
#[ts(export)]
pub struct VersionScanOptions {
    pub query: Option<String>,
    pub collection_id: Option<String>,
    pub volume_id: Option<String>,
    pub signal: Option<VersionLinkReason>,
    pub min_similarity: Option<f64>,
    pub only_with_dupes: Option<bool>,
    pub include_dismissed: Option<bool>,
    #[ts(type = "number | null")]
    pub limit: Option<usize>,
}

/// One revision node inside an ordered semantic version timeline (oldest -> newest).
#[derive(Debug, Clone, Serialize, Deserialize, TS)]
#[ts(export)]
pub struct VersionNode {
    pub file_id: String,
    pub path: String,
    pub filename: String,
    pub ext: String,
    #[ts(type = "number")]
    pub size_bytes: i64,
    #[ts(type = "number")]
    pub mtime: i64,
    pub volume_id: String,
    pub drive_label: String,
    pub drive_online: bool,
    pub sensitivity: String,
    pub version_label: String,
    pub in_versions_folder: bool,
    pub is_latest: bool,
    pub is_pinned: bool,
    pub is_exact_duplicate: bool,
    pub duplicate_of_file_id: Option<String>,
    #[ts(type = "number | null")]
    pub size_delta_bytes: Option<i64>,
    #[ts(type = "number | null")]
    pub time_delta_ms: Option<i64>,
    pub semantic_similarity_prev: Option<f64>,
    pub former_paths: Vec<String>,
    pub link_reasons: Vec<VersionLinkReason>,
}

/// One detected version timeline spanning 2+ related files on a drive/collection.
#[derive(Debug, Clone, Serialize, Deserialize, TS)]
#[ts(export)]
pub struct VersionChain {
    pub chain_key: String,
    pub title: String,
    pub canonical_stem: String,
    pub ext: String,
    pub collection_id: Option<String>,
    pub collection_name: Option<String>,
    pub volume_id: String,
    pub drive_label: String,
    pub drive_online: bool,
    pub anchor_dir: String,
    pub suggested_versions_dir: String,
    pub latest_file_id: String,
    pub pinned_latest_file_id: Option<String>,
    #[ts(type = "number")]
    pub version_count: usize,
    #[ts(type = "number")]
    pub exact_duplicate_count: usize,
    #[ts(type = "number")]
    pub total_bytes: i64,
    #[ts(type = "number")]
    pub reclaimable_duplicate_bytes: i64,
    #[ts(type = "number")]
    pub oldest_mtime: i64,
    #[ts(type = "number")]
    pub newest_mtime: i64,
    pub has_semantic_links: bool,
    pub has_versions_folder: bool,
    pub has_file_history: bool,
    pub has_sensitive: bool,
    pub dismissed: bool,
    pub link_reasons: Vec<VersionLinkReason>,
    pub summary: String,
    pub nodes: Vec<VersionNode>,
}

/// Summary report returned by `scan_version_chains`.
#[derive(Debug, Clone, Serialize, Deserialize, TS)]
#[ts(export)]
pub struct VersionScanReport {
    #[ts(type = "number")]
    pub scanned_at: i64,
    #[ts(type = "number")]
    pub total_chains: usize,
    #[ts(type = "number")]
    pub total_versions: usize,
    #[ts(type = "number")]
    pub chains_with_dupes: usize,
    #[ts(type = "number")]
    pub reclaimable_duplicate_bytes: i64,
    #[ts(type = "number")]
    pub semantic_chains_count: usize,
    #[ts(type = "number")]
    pub versions_folder_chains_count: usize,
    #[ts(type = "number")]
    pub dismissed_count: usize,
    pub chains: Vec<VersionChain>,
}

/// Classification of one line inside a textual diff hunk.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "snake_case")]
#[ts(export)]
pub enum DiffLineKind {
    Context,
    Added,
    Removed,
}

/// Single line inside a `DiffHunk`.
#[derive(Debug, Clone, Serialize, Deserialize, TS)]
#[ts(export)]
pub struct DiffLine {
    pub kind: DiffLineKind,
    #[ts(type = "number | null")]
    pub old_line: Option<usize>,
    #[ts(type = "number | null")]
    pub new_line: Option<usize>,
    pub text: String,
}

/// Contiguous unified/side-by-side diff hunk with surrounding context lines.
#[derive(Debug, Clone, Serialize, Deserialize, TS)]
#[ts(export)]
pub struct DiffHunk {
    #[ts(type = "number")]
    pub old_start: usize,
    #[ts(type = "number")]
    pub old_count: usize,
    #[ts(type = "number")]
    pub new_start: usize,
    #[ts(type = "number")]
    pub new_count: usize,
    pub lines: Vec<DiffLine>,
}

/// Cached or on-demand local LLM conceptual summary for a version pair (`K5`, `R7`, `D1`).
#[derive(Debug, Clone, Serialize, Deserialize, TS)]
#[ts(export)]
pub struct VersionDiffSummary {
    pub older_file_id: String,
    pub newer_file_id: String,
    pub summary: String,
    pub badges: Vec<String>,
    pub model_id: String,
    pub model_name: String,
    pub backend: String,
    pub cached: bool,
    #[ts(type = "number")]
    pub duration_ms: u64,
}

/// Side-by-side / unified comparison between two file revisions (`K2`, `K4`, `R7`).
#[derive(Debug, Clone, Serialize, Deserialize, TS)]
#[ts(export)]
pub struct VersionDiffReport {
    pub older_file_id: String,
    pub newer_file_id: String,
    pub older_filename: String,
    pub newer_filename: String,
    pub older_path: String,
    pub newer_path: String,
    #[ts(type = "number")]
    pub older_size_bytes: i64,
    #[ts(type = "number")]
    pub newer_size_bytes: i64,
    #[ts(type = "number")]
    pub older_mtime: i64,
    #[ts(type = "number")]
    pub newer_mtime: i64,
    #[ts(type = "number")]
    pub size_delta_bytes: i64,
    #[ts(type = "number")]
    pub time_delta_ms: i64,
    pub identical_bytes: bool,
    /// `"text"`, `"binary_metadata"`, or `"redacted_sensitive"`.
    pub diff_kind: String,
    pub redacted_sensitive: bool,
    pub truncated: bool,
    #[ts(type = "number")]
    pub added_lines: usize,
    #[ts(type = "number")]
    pub removed_lines: usize,
    #[ts(type = "number")]
    pub unchanged_lines: usize,
    pub semantic_similarity: Option<f64>,
    pub conceptual_badges: Vec<String>,
    pub deterministic_summary: String,
    pub hunks: Vec<DiffHunk>,
    pub cached_llm_summary: Option<VersionDiffSummary>,
}
