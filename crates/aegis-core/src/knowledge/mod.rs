//! Automatic Knowledge Extraction (`2.2`): typed entity parsing (`Person`, `Organization`,
//! `Project`, `Decision`, `ActionItem`), `.vtt`/`.srt` transcript normalization, incremental
//! record merging (`entity_aliases` + `entities` + `relationships`), `K1` human-in-the-loop
//! inbox with undo, `K4` opt-in audio observation, and `R7` sensitivity gating.

pub mod parser;
pub mod store;

use serde::{Deserialize, Serialize};
use ts_rs::TS;

pub use parser::{
    ExtractedCandidate, build_knowledge_llm_prompt, canonical_key, extract_candidates_from_text,
    normalize_transcript_text, parse_llm_extraction_output,
};
pub use store::{
    AUDIO_OPT_IN_SETTING_KEY, approve_proposal, audio_observation_enabled, dismiss_proposal,
    extract_knowledge_from_catalog, ingest_llm_candidates, load_file_text_for_llm,
    query_knowledge_dashboard, reopen_proposal, set_audio_observation_enabled,
    undo_approved_proposal, update_entity_status,
};

/// Typed entity categories extracted by the Knowledge Extraction subsystem (`2.2`).
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize, TS)]
#[serde(rename_all = "snake_case")]
#[ts(export)]
pub enum EntityType {
    Person,
    Organization,
    Project,
    Decision,
    ActionItem,
}

impl EntityType {
    pub const ALL: [Self; 5] = [
        Self::Person,
        Self::Organization,
        Self::Project,
        Self::Decision,
        Self::ActionItem,
    ];

    pub fn as_str(self) -> &'static str {
        match self {
            Self::Person => "person",
            Self::Organization => "organization",
            Self::Project => "project",
            Self::Decision => "decision",
            Self::ActionItem => "action_item",
        }
    }

    pub fn from_db(s: &str) -> Self {
        match s.trim().to_ascii_lowercase().as_str() {
            "person" | "people" => Self::Person,
            "organization" | "org" | "company" | "vendor" | "client" => Self::Organization,
            "project" | "initiative" => Self::Project,
            "decision" => Self::Decision,
            "action_item" | "action" | "task" | "todo" => Self::ActionItem,
            _ => Self::Organization,
        }
    }

    pub fn label(self) -> &'static str {
        match self {
            Self::Person => "People",
            Self::Organization => "Organizations",
            Self::Project => "Projects",
            Self::Decision => "Decisions",
            Self::ActionItem => "Action Items",
        }
    }
}

/// Review status of a provisional extracted suggestion (`K1`).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "snake_case")]
#[ts(export)]
pub enum ProposalStatus {
    Pending,
    Approved,
    Dismissed,
}

impl ProposalStatus {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Pending => "pending",
            Self::Approved => "approved",
            Self::Dismissed => "dismissed",
        }
    }

    pub fn from_db(s: &str) -> Self {
        match s.trim().to_ascii_lowercase().as_str() {
            "approved" => Self::Approved,
            "dismissed" => Self::Dismissed,
            _ => Self::Pending,
        }
    }
}

/// Options controlling a knowledge extraction scan (`R7`, `K2`, `K4`).
#[derive(Debug, Clone, Default, Serialize, Deserialize, TS)]
#[serde(default)]
#[ts(export)]
pub struct KnowledgeExtractOptions {
    pub file_ids: Option<Vec<String>>,
    pub collection_id: Option<String>,
    pub include_sensitive: Option<bool>,
    pub force_rescan: Option<bool>,
    #[ts(type = "number | null")]
    pub limit_files: Option<usize>,
}

/// Summary returned after scanning indexed documents/transcripts for provisional entities.
#[derive(Debug, Clone, Serialize, Deserialize, TS)]
#[ts(export)]
pub struct KnowledgeExtractReport {
    #[ts(type = "number")]
    pub files_scanned: usize,
    #[ts(type = "number")]
    pub files_skipped_unchanged: usize,
    #[ts(type = "number")]
    pub files_skipped_sensitive: usize,
    #[ts(type = "number")]
    pub proposals_created: usize,
    #[ts(type = "number")]
    pub proposals_updated: usize,
    #[ts(type = "number")]
    pub total_pending: usize,
    pub audio_observation_enabled: bool,
    #[ts(type = "number")]
    pub untranscribed_audio_count: usize,
    #[ts(type = "number")]
    pub took_ms: u64,
}

/// Optional inline edits applied right before approving a provisional suggestion (`K1`).
#[derive(Debug, Clone, Default, Serialize, Deserialize, TS)]
#[serde(default)]
#[ts(export)]
pub struct ProposalEditInput {
    pub name: Option<String>,
    pub entity_type: Option<EntityType>,
    pub summary: Option<String>,
    pub assignee: Option<String>,
    pub due_date: Option<String>,
    pub project_hint: Option<String>,
}

/// One provisional extracted entity, decision, or action item waiting for owner review (`K1`).
#[derive(Debug, Clone, Serialize, Deserialize, TS)]
#[ts(export)]
pub struct KnowledgeProposal {
    pub id: String,
    pub entity_type: EntityType,
    pub name: String,
    pub summary: String,
    pub attrs_json: String,
    pub assignee: Option<String>,
    pub due_date: Option<String>,
    pub project_hint: Option<String>,
    pub source_file_id: Option<String>,
    pub source_path: String,
    pub source_filename: String,
    pub source_kind: String,
    pub evidence_quote: String,
    pub extractor: String,
    pub confidence: f64,
    pub contains_sensitive: bool,
    pub redacted: bool,
    pub matched_entity_id: Option<String>,
    pub matched_entity_name: Option<String>,
    pub status: ProposalStatus,
    pub committed_entity_id: Option<String>,
    #[ts(type = "number")]
    pub created_at: i64,
    #[ts(type = "number | null")]
    pub reviewed_at: Option<i64>,
}

/// Provenance edge connecting a canonical entity to a live source document or transcript.
#[derive(Debug, Clone, Serialize, Deserialize, TS)]
#[ts(export)]
pub struct EntityProvenance {
    pub relationship_id: String,
    pub file_id: String,
    pub path: String,
    pub filename: String,
    pub rel_type: String,
    pub quote: String,
    pub redacted: bool,
    #[ts(type = "number")]
    pub recorded_at: i64,
}

/// Cross-entity edge in the approved knowledge graph (e.g., `owns_action`, `belongs_to_project`).
#[derive(Debug, Clone, Serialize, Deserialize, TS)]
#[ts(export)]
pub struct EntityRelationLink {
    pub relationship_id: String,
    pub other_entity_id: String,
    pub other_entity_type: EntityType,
    pub other_entity_name: String,
    pub rel_type: String,
    pub direction: String,
}

/// Canonical approved node in `entities` with merged attributes, provenance, and links (`D1`, `R7`).
#[derive(Debug, Clone, Serialize, Deserialize, TS)]
#[ts(export)]
pub struct KnowledgeEntity {
    pub id: String,
    pub entity_type: EntityType,
    pub name: String,
    pub summary: String,
    pub aliases: Vec<String>,
    pub assignee: Option<String>,
    pub due_date: Option<String>,
    pub status_tag: Option<String>,
    pub project_name: Option<String>,
    #[ts(type = "number")]
    pub mention_count: usize,
    pub contains_sensitive: bool,
    #[ts(type = "number")]
    pub created_at: i64,
    #[ts(type = "number")]
    pub updated_at: i64,
    pub provenance: Vec<EntityProvenance>,
    pub related_entities: Vec<EntityRelationLink>,
}

/// Meeting recording surfaced only when `K4` audio observation opt-in is enabled.
#[derive(Debug, Clone, Serialize, Deserialize, TS)]
#[ts(export)]
pub struct AudioRecordingItem {
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
    pub companion_transcript_file_id: Option<String>,
    pub companion_transcript_name: Option<String>,
}

/// Counts per typed object (`Person`, `Organization`, `Project`, `Decision`, `ActionItem`).
#[derive(Debug, Clone, Serialize, Deserialize, TS)]
#[ts(export)]
pub struct EntityTypeCount {
    pub entity_type: EntityType,
    pub label: String,
    #[ts(type = "number")]
    pub pending_count: usize,
    #[ts(type = "number")]
    pub entity_count: usize,
}

/// Filters for loading the Knowledge Base inbox and graph.
#[derive(Debug, Clone, Default, Serialize, Deserialize, TS)]
#[serde(default)]
#[ts(export)]
pub struct KnowledgeQueryOptions {
    pub query: Option<String>,
    pub entity_type: Option<EntityType>,
    pub status: Option<ProposalStatus>,
    #[ts(type = "number | null")]
    pub limit: Option<usize>,
}

/// Complete state payload for the `Knowledge Base` UI (`K1`, `K4`, `R7`, `D1`).
#[derive(Debug, Clone, Serialize, Deserialize, TS)]
#[ts(export)]
pub struct KnowledgeDashboard {
    #[ts(type = "number")]
    pub pending_count: usize,
    #[ts(type = "number")]
    pub approved_count: usize,
    #[ts(type = "number")]
    pub dismissed_count: usize,
    #[ts(type = "number")]
    pub total_entities: usize,
    pub counts_by_type: Vec<EntityTypeCount>,
    pub audio_observation_enabled: bool,
    pub audio_recordings: Vec<AudioRecordingItem>,
    pub proposals: Vec<KnowledgeProposal>,
    pub entities: Vec<KnowledgeEntity>,
}

/// Outcome returned when approving one provisional suggestion into the canonical graph.
#[derive(Debug, Clone, Serialize, Deserialize, TS)]
#[ts(export)]
pub struct KnowledgeCommitResult {
    pub proposal_id: String,
    pub entity_id: String,
    pub created_new: bool,
    pub merged_into_existing: bool,
    #[ts(type = "number")]
    pub relationships_added: usize,
}
