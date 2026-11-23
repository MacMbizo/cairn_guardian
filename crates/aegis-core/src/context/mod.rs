//! Context layer (spec 2026-11-22-context-aware-filing-design §1A): before filing moves any single
//! file, Aegis asks "what does this belong to?". The answer is a collection: a group of files that
//! belong together, recognised by a module. Projects are the first module; sets and photo subjects
//! build alongside them.
//!
//! - `profile`: per-folder statistics from the catalog
//! - `store`: persistence of collections, locations, loose set members and the owner's answers

pub mod answers;
pub mod old_copies;
pub mod owners;
pub mod plan;
pub mod profile;
pub mod projects;
pub mod recognise;
pub mod relate;
pub mod resources;
pub mod reunite;
pub mod series;
pub mod sets;
pub mod software;
pub mod store;
pub mod watch;

pub use answers::{Answer, answer};
pub use recognise::{RecogniseReport, recognise};
pub use reunite::{
    ReuniteItem, ReunitePlan, SplitBatchRef, is_reunite_batch, propose_reunite, reunite_plans, reunite_plans_conn, run_reunite,
    undo_reunite,
};

use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use ts_rs::TS;

/// Which module recognised a collection.
#[derive(TS, Serialize, Deserialize, Clone, Copy, PartialEq, Eq, Debug, Hash)]
#[serde(rename_all = "snake_case")]
#[ts(export)]
pub enum CollectionKind {
    Project,
    DesignSet,
    PhotoSubject,
    SoftwarePackage,
    OwnersFolder,
    Resource,
    Series,
    OldCopy,
}

impl CollectionKind {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Project => "project",
            Self::DesignSet => "design_set",
            Self::PhotoSubject => "photo_subject",
            Self::SoftwarePackage => "software_package",
            Self::OwnersFolder => "owners_folder",
            Self::Resource => "resource",
            Self::Series => "series",
            Self::OldCopy => "old_copy",
        }
    }

    pub fn from_db(s: &str) -> Option<Self> {
        match s {
            "project" => Some(Self::Project),
            "design_set" => Some(Self::DesignSet),
            "photo_subject" => Some(Self::PhotoSubject),
            "software_package" => Some(Self::SoftwarePackage),
            "owners_folder" => Some(Self::OwnersFolder),
            "resource" => Some(Self::Resource),
            "series" => Some(Self::Series),
            "old_copy" => Some(Self::OldCopy),
            _ => None,
        }
    }

    /// True when this collection owns whole folders in `collection_locations` rather than
    /// file-level rows in `collection_members`.
    pub fn is_folder_collection(self) -> bool {
        matches!(self, Self::Project | Self::SoftwarePackage | Self::OwnersFolder | Self::Resource | Self::OldCopy)
    }

    /// True when this collection owns loose member files in `collection_members`.
    pub fn is_member_collection(self) -> bool {
        !self.is_folder_collection()
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
    SetFiles,
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
            Self::SetFiles => "set_files",
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
            "set_files" => Self::SetFiles,
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
    /// The owner said "keep this copy here": the project's plan leaves it where it is.
    pub keep_here: bool,
}

/// One loose file belonging to a design set or photo subject collection.
#[derive(Clone, PartialEq, Eq, Debug)]
pub struct SetMember {
    pub collection_id: String,
    pub file_id: String,
    pub volume_id: String,
    pub path: String,
    pub size: i64,
    pub mtime: i64,
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
    /// The latest plan proposed or run for this project, as `<state>:<batch id>` with state
    /// `proposed`, `done` or `undone`; `None` when there has been none.
    pub plan_state: Option<String>,
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
    /// The owner chose this location as the project's home.
    Home,
    /// The owner wants this location left where it is (`yes`), or no longer (`no`).
    KeepHere,
    /// Approved set subject remembered for future single-file arrivals (`design:<Name>` or `photo:<Name>`).
    LearnedSubject,
    /// The owner explicitly reclassified this project's kind (`software`, `design`, `writing`, `research`, `other`).
    ProjectKind,
}

impl DecisionKind {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::IsProject => "is_project",
            Self::SameAs => "same_as",
            Self::NotSameAs => "not_same_as",
            Self::LeaveAsIs => "leave_as_is",
            Self::Name => "name",
            Self::Home => "home",
            Self::KeepHere => "keep_here",
            Self::LearnedSubject => "learned_subject",
            Self::ProjectKind => "project_kind",
        }
    }

    pub fn from_db(s: &str) -> Option<Self> {
        Some(match s {
            "is_project" => Self::IsProject,
            "same_as" => Self::SameAs,
            "not_same_as" => Self::NotSameAs,
            "leave_as_is" => Self::LeaveAsIs,
            "name" => Self::Name,
            "home" => Self::Home,
            "keep_here" => Self::KeepHere,
            "learned_subject" => Self::LearnedSubject,
            "project_kind" => Self::ProjectKind,
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

/// Fast `O(depth)` index of already-claimed roots (`Project`, `OldCopy`, etc.) so subsequent
/// modules never run quadratic `path_is_within` scans across hundreds of projects.
pub(crate) struct ClaimedIndex {
    exact: HashMap<String, Vec<Option<String>>>,
    strict_ancestors: HashMap<String, Vec<Option<String>>>,
}

impl ClaimedIndex {
    pub(crate) fn new(roots: &[(Option<String>, String)]) -> Self {
        let sep = std::path::MAIN_SEPARATOR;
        let mut exact: HashMap<String, Vec<Option<String>>> = HashMap::new();
        let mut strict_ancestors: HashMap<String, Vec<Option<String>>> = HashMap::new();
        for (v, r) in roots {
            let norm = r.trim_end_matches(sep).to_lowercase();
            if norm.is_empty() {
                continue;
            }
            exact.entry(norm.clone()).or_default().push(v.clone());
            let mut cur = norm.as_str();
            while let Some(pos) = cur.rfind(sep) {
                cur = &cur[..pos];
                if cur.is_empty() || cur.ends_with(':') {
                    if !cur.is_empty() {
                        strict_ancestors.entry(format!("{cur}{sep}")).or_default().push(v.clone());
                    }
                    break;
                }
                strict_ancestors.entry(cur.to_string()).or_default().push(v.clone());
            }
        }
        Self { exact, strict_ancestors }
    }

    /// Returns `(inside_claimed, holds_claimed)` for `(volume_id, path)`.
    pub(crate) fn overlaps(&self, volume_id: &str, path: &str) -> (bool, bool) {
        if self.exact.is_empty() {
            return (false, false);
        }
        let sep = std::path::MAIN_SEPARATOR;
        let low = path.trim_end_matches(sep).to_lowercase();
        let vol_matches = |vols: &Vec<Option<String>>| vols.iter().any(|v| v.as_deref().is_none_or(|rv| rv == volume_id));

        let mut inside = false;
        let mut cur = low.as_str();
        loop {
            if let Some(vols) = self.exact.get(cur)
                && vol_matches(vols)
            {
                inside = true;
                break;
            }
            let Some(pos) = cur.rfind(sep) else { break };
            if pos == 0 {
                break;
            }
            let parent = &cur[..pos];
            if parent.ends_with(':') {
                let root = format!("{parent}{sep}");
                if let Some(vols) = self.exact.get(&root)
                    && vol_matches(vols)
                {
                    inside = true;
                }
                break;
            }
            cur = parent;
        }
        let holds = self.strict_ancestors.get(&low).is_some_and(vol_matches)
            || (low.ends_with(':') && self.strict_ancestors.get(&format!("{low}{sep}")).is_some_and(vol_matches));
        (inside, holds)
    }
}

/// A folder claimed as a whole by one of the Step 7 context modules (`SoftwarePackage`,
/// `OwnersFolder`, `Resource`, or `OldCopy`), used by blueprint filing and open-batch re-checks.
#[derive(Clone, Debug, PartialEq)]
pub struct ModuleFolderGroup {
    pub kind: CollectionKind,
    pub name: String,
    pub path: String,
    pub hint: String,
    pub confidence: f64,
    pub left_as_is: bool,
}

/// Extracts the module-specific routing hint stored in `collections.fingerprint`.
pub(crate) fn module_hint(kind: CollectionKind, fingerprint: Option<&str>) -> String {
    let Some(fp) = fingerprint else { return String::new() };
    match kind {
        CollectionKind::SoftwarePackage => fp.strip_prefix("software:").unwrap_or("tools").to_string(),
        CollectionKind::OwnersFolder => fp.strip_prefix("owners:").unwrap_or("").to_string(),
        CollectionKind::Resource => fp.strip_prefix("resource:").unwrap_or("").to_string(),
        CollectionKind::OldCopy => fp.strip_prefix("old_copy:").and_then(|s| s.split('|').next()).unwrap_or("").to_string(),
        _ => String::new(),
    }
}

/// Project roots filing must keep whole, with each project's name: every saved project location
/// at 0.60 or more that the owner hasn't said is not a project, under `roots`. Before the first
/// recognition pass has run, projects are recognised on the spot (nothing is saved).
pub fn protected_roots(store: &crate::db::SqliteStore, roots: &[String]) -> crate::error::Result<Vec<(String, String)>> {
    use crate::db::files::path_is_within;
    let saved = store::list_collections(store.conn(), CollectionKind::Project)?;
    let never_ran = store.get_setting(recognise::RECOGNISED_AT, "context")?.is_none();
    let found: Vec<(String, String)> = if never_ran {
        let ps = profile::profiles(store, roots)?;
        let old = old_copies::find_old_copies(&ps, roots);
        let old_paths: Vec<String> = old.iter().map(|o| o.root.clone()).collect();
        let ps_proj = old_copies::filter_profiles_outside(&ps, &old_paths);
        projects::find_projects(&ps_proj, roots).into_iter().map(|c| (c.root.clone(), c.name().to_string())).collect()
    } else {
        saved
            .into_iter()
            .filter(|c| c.confidence >= projects::PROJECT_THRESHOLD && c.state != "not_a_project")
            .flat_map(|c| {
                let name = c.name.clone();
                c.locations.into_iter().map(move |l| (l.root_path, name.clone()))
            })
            .collect()
    };
    Ok(found.into_iter().filter(|(p, _)| roots.iter().any(|r| path_is_within(p, r, false))).collect())
}

/// Folder-level Step 7 module groups (`SoftwarePackage`, `OwnersFolder`, `Resource`, `OldCopy`)
/// under `roots` that blueprint filing keeps whole (and moves whole in Fresh). Falls back to
/// on-the-spot detection before the first recognition pass has run.
pub fn module_folder_groups(store: &crate::db::SqliteStore, roots: &[String]) -> crate::error::Result<Vec<ModuleFolderGroup>> {
    use crate::db::files::path_is_within;
    let never_ran = store.get_setting(recognise::RECOGNISED_AT, "context")?.is_none();
    if never_ran {
        let ps = profile::profiles(store, roots)?;
        let old = old_copies::find_old_copies(&ps, roots);
        let mut claimed: Vec<(Option<String>, String)> = old.iter().map(|o| (Some(o.volume_id.clone()), o.root.clone())).collect();
        let old_paths: Vec<String> = old.iter().map(|o| o.root.clone()).collect();
        let ps_proj = old_copies::filter_profiles_outside(&ps, &old_paths);
        for p in projects::find_projects(&ps_proj, roots) {
            claimed.push((Some(p.volume_id.clone()), p.root));
        }
        let sw = software::find_software_packages(&ps, roots, &claimed);
        for s in &sw {
            claimed.push((Some(s.volume_id.clone()), s.root.clone()));
        }
        let ow = owners::find_owners_folders(store, &ps, roots, &claimed)?;
        for o in &ow {
            claimed.push((Some(o.volume_id.clone()), o.root.clone()));
        }
        let res = resources::find_resources(&ps, roots, &claimed);
        let mut out = Vec::new();
        for o in old {
            out.push(ModuleFolderGroup {
                kind: CollectionKind::OldCopy,
                name: o.display_name,
                path: o.root,
                hint: o.archive_rel,
                confidence: o.confidence,
                left_as_is: false,
            });
        }
        for s in sw {
            out.push(ModuleFolderGroup {
                kind: CollectionKind::SoftwarePackage,
                name: s.name,
                path: s.root,
                hint: s.bucket.to_string(),
                confidence: s.confidence,
                left_as_is: false,
            });
        }
        for o in ow {
            out.push(ModuleFolderGroup {
                kind: CollectionKind::OwnersFolder,
                name: o.name,
                path: o.root,
                hint: o.life_area,
                confidence: o.confidence,
                left_as_is: false,
            });
        }
        for r in res {
            out.push(ModuleFolderGroup {
                kind: CollectionKind::Resource,
                name: r.name,
                path: r.root,
                hint: r.topic.to_string(),
                confidence: r.confidence,
                left_as_is: false,
            });
        }
        return Ok(out);
    }

    let rows = store::list_collections_with_fingerprints(
        store.conn(),
        &[CollectionKind::SoftwarePackage, CollectionKind::OwnersFolder, CollectionKind::Resource, CollectionKind::OldCopy],
    )?;
    let mut out = Vec::new();
    for (c, fp) in rows {
        if c.confidence < projects::PROJECT_THRESHOLD || c.state == "not_a_project" {
            continue;
        }
        let hint = module_hint(c.kind, fp.as_deref());
        let left_as_is = c.state == "left_as_is";
        for l in c.locations {
            if roots.iter().any(|r| path_is_within(&l.root_path, r, false)) {
                out.push(ModuleFolderGroup {
                    kind: c.kind,
                    name: c.name.clone(),
                    path: l.root_path,
                    hint: hint.clone(),
                    confidence: c.confidence,
                    left_as_is,
                });
            }
        }
    }
    Ok(out)
}

/// Episodes and companion subtitles belonging to a TV series (`CollectionKind::Series`), keyed by
/// lowercase file path → `(show_name, season_folder)` (`"Reacher"`, `"Season 1"`), within `roots`.
pub fn series_episodes(store: &crate::db::SqliteStore, roots: &[String]) -> crate::error::Result<HashMap<String, (String, String)>> {
    use crate::db::files::path_is_within;
    let mut out = HashMap::new();
    let never_ran = store.get_setting(recognise::RECOGNISED_AT, "context")?.is_none();
    if never_ran {
        let (cols, members) = series::find_series_for_roots(store, roots)?;
        let eligible: HashMap<String, (String, String)> = cols
            .into_iter()
            .filter(|(c, _)| c.kind == CollectionKind::Series && c.confidence >= sets::SET_THRESHOLD && c.state != "not_a_project")
            .filter_map(|(c, _)| {
                let show = c.sub_projects.first()?.clone();
                let season = c.sub_projects.get(1)?.clone();
                Some((c.id, (show, season)))
            })
            .collect();
        for m in members {
            if let Some(meta) = eligible.get(&m.collection_id)
                && roots.iter().any(|r| path_is_within(&m.path, r, false))
            {
                out.insert(m.path.to_lowercase(), meta.clone());
            }
        }
        return Ok(out);
    }
    let rows = store::list_members_with_collection_meta(store.conn(), CollectionKind::Series)?;
    for (_, _, subs, conf, state, m) in rows {
        if conf >= sets::SET_THRESHOLD
            && state != "not_a_project"
            && let (Some(show), Some(season)) = (subs.first(), subs.get(1))
            && roots.iter().any(|r| path_is_within(&m.path, r, false))
        {
            out.insert(m.path.to_lowercase(), (show.clone(), season.clone()));
        }
    }
    Ok(out)
}

/// Loose files belonging to a design project (`CollectionKind::DesignSet`), keyed by lowercase
/// file path → design project name, within `roots`. Filing keeps every member where it is until
/// the design project plan runs. Before the first recognition pass has run, sets are recognised
/// on the spot without saving.
pub fn protected_files(store: &crate::db::SqliteStore, roots: &[String]) -> crate::error::Result<HashMap<String, String>> {
    set_member_map(store, roots, CollectionKind::DesignSet, sets::SET_THRESHOLD)
}

/// Loose photos belonging to a photo subject (`CollectionKind::PhotoSubject`), keyed by
/// lowercase file path → subject display name, within `roots`. Fresh files them into
/// `Photos\<year>\<subject>` instead of `<year-month>`.
pub fn photo_subjects(store: &crate::db::SqliteStore, roots: &[String]) -> crate::error::Result<HashMap<String, String>> {
    set_member_map(store, roots, CollectionKind::PhotoSubject, sets::SET_THRESHOLD)
}

fn set_member_map(
    store: &crate::db::SqliteStore,
    roots: &[String],
    want_kind: CollectionKind,
    min_confidence: f64,
) -> crate::error::Result<HashMap<String, String>> {
    use crate::db::files::path_is_within;
    let mut out = HashMap::new();
    let never_ran = store.get_setting(recognise::RECOGNISED_AT, "context")?.is_none();
    if never_ran {
        let (cols, members) = sets::find_sets_for_roots(store, roots)?;
        let eligible: HashMap<String, String> = cols
            .into_iter()
            .filter(|(c, _)| c.kind == want_kind && c.confidence >= min_confidence && c.state != "not_a_project")
            .map(|(c, _)| (c.id, c.name))
            .collect();
        for m in members {
            if let Some(name) = eligible.get(&m.collection_id)
                && roots.iter().any(|r| path_is_within(&m.path, r, false))
            {
                out.insert(m.path.to_lowercase(), name.clone());
            }
        }
        return Ok(out);
    }
    let rows = store::list_members_with_collection(store.conn(), want_kind)?;
    for (_, name, conf, state, m) in rows {
        if conf >= min_confidence && state != "not_a_project" && roots.iter().any(|r| path_is_within(&m.path, r, false)) {
            out.insert(m.path.to_lowercase(), name);
        }
    }
    Ok(out)
}
