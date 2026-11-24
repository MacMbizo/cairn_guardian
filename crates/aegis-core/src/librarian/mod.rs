//! Autonomous Research Librarian (`2.3`) (`K1`–`K5`, `R4`, `R5`, `R7`, `R9`, `R11`, `D1`).
//!
//! Strictly operates from saved documents on disk (encrypted catalog, chunks, `vec_chunks`,
//! and approved Knowledge Graph entities) plus explicit owner-entered topics/excerpts —
//! never monitors active windows, keystrokes, clipboard, or screen state (`R5`, `R11`).

pub mod engine;

pub use engine::{
    PreparedBriefContext, dismiss_reference, find_related_for_file, find_related_for_topic,
    list_recent_research_docs, pin_reference, prepare_brief_context, undismiss_reference,
    unpin_reference,
};

use serde::{Deserialize, Serialize};
use ts_rs::TS;

/// One indexed document available to select as the active reading context (`R5`).
#[derive(Debug, Clone, Serialize, Deserialize, TS)]
#[ts(export)]
pub struct ResearchDocItem {
    pub file_id: String,
    pub path: String,
    pub filename: String,
    pub ext: String,
    #[ts(type = "number")]
    pub size_bytes: i64,
    #[ts(type = "number")]
    pub mtime: i64,
    pub drive_label: String,
    pub drive_online: bool,
    pub collection_id: Option<String>,
    pub collection_name: Option<String>,
    #[ts(type = "number")]
    pub chunk_count: i64,
    pub has_vectors: bool,
    #[ts(type = "number")]
    pub entity_count: i64,
    #[ts(type = "number")]
    pub pinned_count: i64,
    pub excerpt: String,
    pub sensitivity: String,
}

/// Recent & searchable document shelf surfaced in the left pane of the Research Librarian.
#[derive(Debug, Clone, Serialize, Deserialize, TS)]
#[ts(export)]
pub struct ResearchDocShelf {
    pub docs: Vec<ResearchDocItem>,
    #[ts(type = "number")]
    pub total_indexed_docs: usize,
    #[ts(type = "number")]
    pub embedded_docs: usize,
    #[ts(type = "number")]
    pub excluded_sensitive_count: usize,
}

/// Approved Knowledge Graph entity shared between the active reading context and a related document.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, TS)]
#[ts(export)]
pub struct SharedEntityRef {
    pub entity_id: String,
    pub entity_type: String,
    pub name: String,
}

/// Primary reason a related document was surfaced by the Research Librarian.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "snake_case")]
#[ts(export)]
pub enum AssociationReason {
    /// High semantic vector proximity with zero overlapping distinctive keywords (`§2.3`).
    ConceptualParallel,
    /// Matched across both dense semantic vectors and sparse lexical concepts.
    SemanticAndLexical,
    /// Connected through shared approved People, Organizations, Projects, Decisions, or Actions.
    SharedKnowledge,
    /// Surfacing relevant historical notes or PDFs from a different project or folder.
    CrossProjectOverlap,
    /// Explicitly pinned to this document's curated reading list.
    PinnedReference,
}

/// One proactively surfaced cross-reference in the Research Librarian peripheral drawer.
#[derive(Debug, Clone, Serialize, Deserialize, TS)]
#[ts(export)]
pub struct RelatedReference {
    pub file_id: String,
    pub path: String,
    pub filename: String,
    pub ext: String,
    #[ts(type = "number")]
    pub size_bytes: i64,
    #[ts(type = "number")]
    pub mtime: i64,
    pub drive_label: String,
    pub drive_online: bool,
    pub collection_id: Option<String>,
    pub collection_name: Option<String>,
    pub cross_project: bool,
    pub association_score: f64,
    pub semantic_score: f64,
    pub lexical_score: f64,
    pub entity_score: f64,
    pub semantic_only: bool,
    pub primary_reason: AssociationReason,
    pub why_related: String,
    pub shared_terms: Vec<String>,
    pub shared_entities: Vec<SharedEntityRef>,
    pub excerpt: String,
    pub pinned: bool,
    pub pin_note: String,
    pub dismissed: bool,
}

/// Filtering and scoring options for cross-reference discovery (`R7`).
#[derive(Debug, Clone, Default, Serialize, Deserialize, TS)]
#[ts(export)]
pub struct LibrarianOptions {
    #[ts(type = "number | null")]
    pub limit: Option<usize>,
    pub min_score: Option<f64>,
    pub cross_project_only: Option<bool>,
    pub semantic_only_filter: Option<bool>,
    pub include_dismissed: Option<bool>,
    pub include_sensitive: Option<bool>,
}

/// Full peripheral drawer payload for either an active saved file or an explicit topic/excerpt.
#[derive(Debug, Clone, Serialize, Deserialize, TS)]
#[ts(export)]
pub struct ResearchCompanionReport {
    pub mode: String,
    pub source_doc: Option<ResearchDocItem>,
    pub topic_query: Option<String>,
    pub extracted_terms: Vec<String>,
    pub source_entities: Vec<SharedEntityRef>,
    pub references: Vec<RelatedReference>,
    #[ts(type = "number")]
    pub total_candidates: usize,
    #[ts(type = "number")]
    pub conceptual_count: usize,
    #[ts(type = "number")]
    pub cross_project_count: usize,
    #[ts(type = "number")]
    pub pinned_count: usize,
    #[ts(type = "number")]
    pub dismissed_count: usize,
    #[ts(type = "number")]
    pub excluded_sensitive_count: usize,
    pub used_vectors: bool,
    #[ts(type = "number")]
    pub took_ms: u64,
}

/// Numbered source citation backing an on-demand reading brief.
#[derive(Debug, Clone, Serialize, Deserialize, TS)]
#[ts(export)]
pub struct BriefCitation {
    #[ts(type = "number")]
    pub ref_id: usize,
    pub file_id: String,
    pub filename: String,
    pub path: String,
    pub collection_name: Option<String>,
    pub excerpt: String,
}

/// Grounded local LLM synthesis explaining how the active document relates to surfaced references.
#[derive(Debug, Clone, Serialize, Deserialize, TS)]
#[ts(export)]
pub struct ResearchBrief {
    pub title: String,
    pub summary: String,
    pub citations: Vec<BriefCitation>,
    pub model_id: String,
    pub model_name: String,
    pub backend: String,
    #[ts(type = "number")]
    pub duration_ms: u64,
}
