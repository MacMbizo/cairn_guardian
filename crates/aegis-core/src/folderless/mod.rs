//! AI Folderless Mode (`2.1`, reshaped `R6`) (`K1`–`K5`, `R4`, `R6`, `R7`, `R9`, `D1`).
//!
//! Compiles plain-language organization intents into transparent, editable, deletable filing
//! rules (`folderless_rules`), extracts content anchors (dates, quarters, orgs, amounts) purely
//! locally, and routes every file change through Simulator batches with full undo (`K1`, `R6`).

pub mod compiler;
pub mod engine;

pub use compiler::{
    apply_llm_refinement, build_intent_llm_prompt, compile_intent_deterministic,
};
pub use engine::{
    delete_folderless_rule, get_folderless_rule, list_folderless_batches, list_folderless_rules,
    preview_folderless_rule, propose_folderless_batch, save_folderless_rule,
    toggle_folderless_rule, validate_dest_root,
};

use serde::{Deserialize, Serialize};
use ts_rs::TS;

/// Visible, editable intent-compiled filing rule persisted inside SQLCipher `aegis.db` (`R6`, `D1`).
#[derive(Debug, Clone, Serialize, Deserialize, TS)]
#[ts(export)]
pub struct FolderlessRule {
    pub id: String,
    pub name: String,
    pub intent_text: String,
    pub compiled_by: String,
    pub enabled: bool,
    pub scope_root: Option<String>,
    pub match_kinds: Vec<String>,
    pub match_exts: Vec<String>,
    pub match_categories: Vec<String>,
    pub match_doc_types: Vec<String>,
    pub match_keywords: Vec<String>,
    pub match_orgs: Vec<String>,
    #[ts(type = "number | null")]
    pub min_age_days: Option<i64>,
    pub dest_root: String,
    pub path_template: String,
    pub rename_template: Option<String>,
    #[ts(type = "number")]
    pub approvals: i64,
    #[ts(type = "number")]
    pub rejections: i64,
    pub state: String,
    #[ts(type = "number")]
    pub created_at: i64,
    #[ts(type = "number")]
    pub updated_at: i64,
}

/// Editable draft or save payload for creating/updating a Folderless rule (`R6`).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, TS)]
#[ts(export)]
pub struct FolderlessRuleInput {
    pub id: Option<String>,
    pub name: String,
    pub intent_text: String,
    pub compiled_by: Option<String>,
    pub enabled: Option<bool>,
    pub scope_root: Option<String>,
    pub match_kinds: Vec<String>,
    pub match_exts: Vec<String>,
    pub match_categories: Vec<String>,
    pub match_doc_types: Vec<String>,
    pub match_keywords: Vec<String>,
    pub match_orgs: Vec<String>,
    #[ts(type = "number | null")]
    pub min_age_days: Option<i64>,
    pub dest_root: String,
    pub path_template: String,
    pub rename_template: Option<String>,
}

/// Compiler output presented to the owner for inspection and optional editing before saving (`R6`).
#[derive(Debug, Clone, Serialize, Deserialize, TS)]
#[ts(export)]
pub struct CompiledIntentDraft {
    pub rule: FolderlessRuleInput,
    pub explanation: String,
    pub warnings: Vec<String>,
    pub compiled_by: String,
}

/// One proposed file placement produced during a non-destructive dry-run preview (`K1`).
#[derive(Debug, Clone, Serialize, Deserialize, TS)]
#[ts(export)]
pub struct FolderlessMatchPreview {
    pub file_id: String,
    pub filename: String,
    pub from_path: String,
    pub to_path: String,
    pub new_filename: String,
    #[ts(type = "number")]
    pub size_bytes: i64,
    #[ts(type = "number")]
    pub mtime: i64,
    pub drive_label: String,
    pub matched_reasons: Vec<String>,
    pub extracted_anchors: Vec<String>,
    pub confidence: f64,
    pub already_in_open_batch: bool,
}

/// Dry-run preview summary for a rule or unsaved draft rule (`K1`, `K2`, `R7`).
#[derive(Debug, Clone, Serialize, Deserialize, TS)]
#[ts(export)]
pub struct FolderlessPreviewReport {
    pub rule_name: String,
    pub dest_root: String,
    pub path_template: String,
    pub matches: Vec<FolderlessMatchPreview>,
    #[ts(type = "number")]
    pub total_matches: usize,
    #[ts(type = "number")]
    pub total_bytes: i64,
    #[ts(type = "number")]
    pub skipped_atomic_unit_count: usize,
    #[ts(type = "number")]
    pub excluded_sensitive_count: usize,
    #[ts(type = "number")]
    pub took_ms: u64,
}

/// Executed/pending Simulator batch created by Folderless Mode with rollback capability (`K1`).
#[derive(Debug, Clone, Serialize, Deserialize, TS)]
#[ts(export)]
pub struct FolderlessBatchSummary {
    pub batch_id: String,
    pub origin: String,
    pub rule_id: String,
    pub status: String,
    #[ts(type = "number")]
    pub total_actions: i64,
    #[ts(type = "number")]
    pub executed_count: i64,
    #[ts(type = "number")]
    pub rolled_back_count: i64,
    pub can_undo: bool,
    #[ts(type = "number")]
    pub created_at: i64,
    #[ts(type = "number | null")]
    pub executed_at: Option<i64>,
}
