//! Context layer (spec 2026-11-22-context-aware-filing-design §1A): before filing moves any single
//! file, Aegis asks "what does this belong to?". The answer is a collection: a group of files that
//! belong together, recognised by a module. Projects are the first module.
//!
//! - `profile`: per-folder statistics from the catalog
//! - `store`: persistence of collections, locations and the owner's answers

pub mod profile;
pub mod projects;
pub mod store;

use serde::{Deserialize, Serialize};
use ts_rs::TS;

/// Which module recognised a collection. Further modules add variants later.
#[derive(TS, Serialize, Deserialize, Clone, Copy, PartialEq, Eq, Debug, Hash)]
#[serde(rename_all = "snake_case")]
#[ts(export)]
pub enum CollectionKind {
    Project,
}

impl CollectionKind {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Project => "project",
        }
    }

    pub fn from_db(s: &str) -> Option<Self> {
        match s {
            "project" => Some(Self::Project),
            _ => None,
        }
    }
}

/// What sort of project it is.
#[derive(TS, Serialize, Deserialize, Clone, Copy, PartialEq, Eq, Debug, Hash)]
#[serde(rename_all = "snake_case")]
#[ts(export)]
pub enum ProjectKind {
    Software,
    Design,
    Writing,
    Research,
    Other,
}

impl ProjectKind {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Software => "software",
            Self::Design => "design",
            Self::Writing => "writing",
            Self::Research => "research",
            Self::Other => "other",
        }
    }

    pub fn from_db(s: &str) -> Self {
        match s {
            "software" => Self::Software,
            "design" => Self::Design,
            "writing" => Self::Writing,
            "research" => Self::Research,
            _ => Self::Other,
        }
    }
}

/// How one location of a collection relates to its home (spec §6).
#[derive(TS, Serialize, Deserialize, Clone, Copy, PartialEq, Eq, Debug, Hash)]
#[serde(rename_all = "snake_case")]
#[ts(export)]
pub enum Relationship {
    Home,
    Identical,
    OlderSubset,
    Diverged,
    DifferentMaterial,
    Version,
    Unrelated,
    Unknown,
}

impl Relationship {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Home => "home",
            Self::Identical => "identical",
            Self::OlderSubset => "older_subset",
            Self::Diverged => "diverged",
            Self::DifferentMaterial => "different_material",
            Self::Version => "version",
            Self::Unrelated => "unrelated",
            Self::Unknown => "unknown",
        }
    }

    pub fn from_db(s: &str) -> Self {
        match s {
            "home" => Self::Home,
            "identical" => Self::Identical,
            "older_subset" => Self::OlderSubset,
            "diverged" => Self::Diverged,
            "different_material" => Self::DifferentMaterial,
            "version" => Self::Version,
            "unrelated" => Self::Unrelated,
            _ => Self::Unknown,
        }
    }
}

/// A folder holding (part of) a collection: its root folder on some volume.
#[derive(TS, Serialize, Deserialize, Clone, PartialEq, Debug)]
#[ts(export)]
pub struct Location {
    pub id: String,
    pub collection_id: String,
    pub volume_id: String,
    pub root_path: String,
    pub relationship: Relationship,
    /// Files that aren't build output.
    #[ts(type = "number")]
    pub files: i64,
    #[ts(type = "number")]
    pub bytes: i64,
    #[ts(type = "number")]
    pub newest_mtime: i64,
    /// Files also found in the home location.
    #[ts(type = "number")]
    pub shared_files: i64,
    /// `content` (by hash) or `name and size`.
    pub compared_by: String,
    pub is_repo: bool,
    /// Whether its drive is plugged in (from the volume, not stored).
    pub online: bool,
    /// Stable identity for the owner's answers: survives renames and drive letters.
    pub fingerprint: String,
}

/// A group of files that belong together, with every place it lives.
#[derive(TS, Serialize, Deserialize, Clone, PartialEq, Debug)]
#[ts(export)]
pub struct Collection {
    pub id: String,
    pub kind: CollectionKind,
    pub name: String,
    pub project_kind: Option<ProjectKind>,
    pub confidence: f64,
    /// `candidate`, `confirmed`, `left_as_is` or `not_a_project`.
    pub state: String,
    pub evidence: Vec<String>,
    pub locations: Vec<Location>,
    pub sub_projects: Vec<String>,
}

/// Which question the owner answered.
#[derive(TS, Serialize, Deserialize, Clone, Copy, PartialEq, Eq, Debug, Hash)]
#[serde(rename_all = "snake_case")]
#[ts(export)]
pub enum DecisionKind {
    IsProject,
    SameAs,
    NotSameAs,
    LeaveAsIs,
    Name,
}

impl DecisionKind {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::IsProject => "is_project",
            Self::SameAs => "same_as",
            Self::NotSameAs => "not_same_as",
            Self::LeaveAsIs => "leave_as_is",
            Self::Name => "name",
        }
    }

    pub fn from_db(s: &str) -> Option<Self> {
        Some(match s {
            "is_project" => Self::IsProject,
            "same_as" => Self::SameAs,
            "not_same_as" => Self::NotSameAs,
            "leave_as_is" => Self::LeaveAsIs,
            "name" => Self::Name,
            _ => return None,
        })
    }
}

/// One answer from the owner, keyed by location fingerprints (spec §12).
#[derive(TS, Serialize, Deserialize, Clone, PartialEq, Eq, Debug)]
#[ts(export)]
pub struct Decision {
    pub id: String,
    pub kind: DecisionKind,
    pub fingerprint_a: String,
    pub fingerprint_b: Option<String>,
    pub value: String,
    #[ts(type = "number")]
    pub at: i64,
}
