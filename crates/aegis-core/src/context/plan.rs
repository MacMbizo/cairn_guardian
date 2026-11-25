//! Project plans (spec §8): for one project, where it should live and what happens to each of
//! its other copies, as an ordered list of whole-folder steps. Planning only reads the catalog
//! and checks whether destination names are free; it never changes anything on disk.
//!
//! Rules (plan 2026-11-23-project-plans, Task 2):
//! - one plan per drive the project lives on; no step ever leaves its drive;
//! - the home moves whole into the drive's projects home (`C:\Prj` on the PC);
//! - identical and older copies go to quarantine (with a `Versions` fallback, used when the
//!   content check at run time finds a file that isn't in the home);
//! - copies changed on both sides, and earlier versions, go to `Versions\<date> <origin>`;
//! - different material goes to `Docs`, `Design` or `Material`;
//! - a git repository is never quarantined and nothing is moved into one: material and versions
//!   for a repository home go to its sibling `<name> - material` folder;
//! - a copy that sits at the name the home needs is moved aside first, and handled after the
//!   home has moved in.

use super::profile::live_under_root_sql;
use super::relate::{choose_home, normalized_name};
use super::{Collection, CollectionKind, DecisionKind, Location, ProjectKind, Relationship};
use crate::db::SqliteStore;
use crate::db::files::path_is_within;
use crate::error::{AegisError, Result};
use crate::{new_id, now_ms};
use rusqlite::{OptionalExtension, params};
use serde::{Deserialize, Serialize};
use std::collections::{BTreeMap, HashMap, HashSet};
use std::path::Path;
use ts_rs::TS;

const SEP: char = std::path::MAIN_SEPARATOR;

/// Settings key (scope `context`) overriding a drive's projects home: `projects_home:<volume id>`.
pub const PROJECTS_HOME_KEY: &str = "projects_home";

/// Folders that are general places on a PC: never a projects home.
const GENERAL_FOLDERS: [&str; 6] = ["documents", "downloads", "pictures", "desktop", "videos", "music"];

const DOC_EXTS: [&str; 12] = ["md", "txt", "pdf", "doc", "docx", "rtf", "odt", "pptx", "ppt", "xlsx", "csv", "html"];
const DESIGN_EXTS: [&str; 14] = ["png", "jpg", "jpeg", "gif", "webp", "svg", "ico", "psd", "ai", "fig", "eps", "tif", "tiff", "bmp"];

/// What one step does with one folder.
#[derive(TS, Serialize, Deserialize, Clone, Copy, PartialEq, Eq, Debug, Hash)]
#[serde(rename_all = "snake_case")]
#[ts(export)]
pub enum StepKind {
    /// A copy steps aside so the home can take its name; a later step handles it.
    MoveAside,
    /// The home moves whole into the projects home.
    MoveHome,
    /// The copy goes to quarantine for 30 days (or to `fallback` if the content check fails).
    Quarantine,
    /// The copy is kept, whole, as a dated version.
    ToVersions,
    /// The copy's material is brought into the home.
    ToMaterial,
    /// Loose files of a design set are gathered into one design project folder.
    GatherFiles,
    /// An old copy moves whole into `Archive\<device or year>\<name>`.
    ToArchive,
}

/// One whole-folder step of a plan.
#[derive(TS, Serialize, Deserialize, Clone, PartialEq, Debug)]
#[ts(export)]
pub struct PlanStep {
    /// Steps run in this order (1 first).
    pub step: u32,
    pub kind: StepKind,
    /// The location this step acts on.
    pub location_id: String,
    pub from: String,
    /// Where the folder goes (for a quarantine, the folder name it is quarantined under).
    pub to: String,
    /// For a quarantine: where the copy goes instead if any of its files isn't in the home.
    pub fallback: Option<String>,
    /// In plain words, for the card.
    pub reason: String,
    pub warnings: Vec<String>,
}

/// The plan for one project on one drive.
#[derive(TS, Serialize, Deserialize, Clone, PartialEq, Debug)]
#[ts(export)]
pub struct ProjectPlan {
    pub collection_id: String,
    pub volume_id: String,
    /// Where the home will be once the plan has run.
    pub home_after: String,
    pub steps: Vec<PlanStep>,
    /// Why the plan can't run yet ("New Volume is unplugged"), if it can't.
    pub waits_for: Option<String>,
    /// Copies the plan leaves where they are, and why, in plain words.
    pub notes: Vec<String>,
    /// Where its card comes in the order of offers (see [`offer_rank`]); 9 with nothing to do.
    pub rank: u8,
    /// One line for the card.
    pub summary: String,
    /// Deterministic BLAKE3 digest of what this plan will do; verified when the user clicks Approve.
    pub digest: String,
}

/// Deterministic digest of what a plan will actually change on disk (`collection_id`,
/// `volume_id`, `home_after`, every step's order/kind/target/destination, and any explicit
/// set member `(file_id, path)` pairs).
pub fn plan_digest_with_members(
    collection_id: &str,
    volume_id: &str,
    home_after: &str,
    steps: &[PlanStep],
    member_paths: &[(String, String)],
) -> String {
    let mut h = blake3::Hasher::new();
    let feed = |h: &mut blake3::Hasher, s: &str| {
        h.update(&(s.len() as u64).to_le_bytes());
        h.update(s.as_bytes());
    };
    feed(&mut h, collection_id);
    feed(&mut h, volume_id);
    feed(&mut h, home_after);
    h.update(&(steps.len() as u64).to_le_bytes());
    for s in steps {
        h.update(&s.step.to_le_bytes());
        feed(&mut h, &serde_json::to_string(&s.kind).unwrap_or_default());
        feed(&mut h, &s.location_id);
        feed(&mut h, &s.from);
        feed(&mut h, &s.to);
        feed(&mut h, s.fallback.as_deref().unwrap_or(""));
    }
    if !member_paths.is_empty() {
        let mut sorted: Vec<(&str, &str)> = member_paths.iter().map(|(id, p)| (id.as_str(), p.as_str())).collect();
        sorted.sort_unstable();
        h.update(&(sorted.len() as u64).to_le_bytes());
        for (id, path) in sorted {
            feed(&mut h, id);
            feed(&mut h, path);
        }
    }
    h.finalize().to_hex().to_string()
}

/// Deterministic digest of what a folder project plan will actually change on disk.
pub fn plan_digest(collection_id: &str, volume_id: &str, home_after: &str, steps: &[PlanStep]) -> String {
    plan_digest_with_members(collection_id, volume_id, home_after, steps, &[])
}

fn is_build_name(name: &str) -> bool {
    super::profile::BUILD_DIRS.iter().any(|d| d.eq_ignore_ascii_case(name))
}

/// Every file under `dir` that isn't build output, as (path relative to `dir`, full path).
fn walk_files(dir: &Path) -> Result<Vec<(String, std::path::PathBuf)>> {
    let mut out = Vec::new();
    let mut stack = vec![dir.to_path_buf()];
    while let Some(d) = stack.pop() {
        for e in std::fs::read_dir(&d)? {
            let e = e?;
            let name = e.file_name().to_string_lossy().to_string();
            let ft = e.file_type()?;
            if ft.is_dir() {
                if !is_build_name(&name) {
                    stack.push(e.path());
                }
            } else if ft.is_file() && !name.to_lowercase().ends_with(".pyc") {
                let p = e.path();
                let rel = p.strip_prefix(dir).map(|r| r.to_string_lossy().to_lowercase()).unwrap_or_default();
                out.push((rel, p));
            }
        }
    }
    Ok(out)
}

/// A folder as it is on disk now: how many files it holds (build output aside) and the newest
/// modified time among them (ms). A plan records it, and each step checks it again before it
/// runs, so a folder that changed after the owner approved is never moved on a stale plan.
pub fn folder_state(dir: &Path) -> Result<(i64, i64)> {
    let mut newest = 0i64;
    let files = walk_files(dir)?;
    for (_, p) in &files {
        if let Ok(m) = std::fs::metadata(p).and_then(|m| m.modified()) {
            let ms = m.duration_since(std::time::UNIX_EPOCH).map(|d| d.as_millis() as i64).unwrap_or(0);
            newest = newest.max(ms);
        }
    }
    Ok((files.len() as i64, newest))
}

/// How many files in `copy` (build output aside) are **not** proven to be in `home`: a file
/// counts as proven only when a file with the same BLAKE3 hash is in `home`, read from disk now.
/// Files that can't be read (cloud placeholders, locked files) are not proven.
pub fn unproven_files(copy: &Path, home: &Path) -> Result<usize> {
    let hash = |p: &Path| crate::db::actions::file_blake3(p).ok().flatten();
    let mut missing: Vec<Vec<u8>> = Vec::new();
    let mut unreadable = 0usize;
    for (rel, p) in walk_files(copy)? {
        let Some(h) = hash(&p) else {
            unreadable += 1;
            continue;
        };
        // Usually the same file sits at the same place in the home.
        let twin = home.join(&rel);
        if twin.is_file() && hash(&twin).as_deref() == Some(h.as_slice()) {
            continue;
        }
        missing.push(h);
    }
    if !missing.is_empty() {
        // Otherwise it may have moved inside the home: compare with everything there.
        let all: std::collections::HashSet<Vec<u8>> =
            if home.is_dir() { walk_files(home)?.into_iter().filter_map(|(_, p)| hash(&p)).collect() } else { Default::default() };
        missing.retain(|h| !all.contains(h));
    }
    Ok(missing.len() + unreadable)
}

pub(crate) fn join(base: &str, leaf: &str) -> String {
    format!("{}{SEP}{leaf}", base.trim_end_matches(SEP))
}

pub(crate) fn parent(p: &str) -> &str {
    p.trim_end_matches(SEP).rfind(SEP).map_or(p, |i| &p[..i])
}

pub(crate) fn leaf(p: &str) -> &str {
    let p = p.trim_end_matches(SEP);
    p.rfind(SEP).map_or(p, |i| &p[i + 1..])
}

pub(crate) fn same_path(a: &str, b: &str) -> bool {
    a.trim_end_matches(SEP).eq_ignore_ascii_case(b.trim_end_matches(SEP))
}

/// Strictly inside `dir` (not `dir` itself).
pub(crate) fn inside(p: &str, dir: &str) -> bool {
    !same_path(p, dir) && path_is_within(p, dir, false)
}

/// A folder name Windows accepts: reserved characters become `_`, trailing dots and spaces go.
pub(crate) fn safe_name(name: &str) -> String {
    let s: String = name.chars().map(|c| if r#"<>:"/\|?*"#.contains(c) || c.is_control() { '_' } else { c }).collect();
    let s = s.trim().trim_end_matches(['.', ' ']).to_string();
    if s.is_empty() { "Project".into() } else { s }
}

/// The local calendar date (`YYYY-MM-DD`) of a timestamp in ms.
fn date_of(ms: i64) -> String {
    crate::engine::note::date_of(crate::filing::local_ms(ms).div_euclid(86_400_000))
}

fn kind_word(k: Option<ProjectKind>) -> &'static str {
    match k {
        Some(ProjectKind::Software) => "app",
        Some(ProjectKind::Design) => "design",
        Some(ProjectKind::Writing) => "writing",
        Some(ProjectKind::Research) => "research",
        _ => "project",
    }
}

/// Where a collection of `kind` should live under the drive's projects home `ph`:
/// software, writing, research and other projects sit directly in `ph` (`C:\Prj\<Name>`),
/// while design projects sit together in `C:\Prj\Design\<Name>`.
pub fn kind_home(ph: &str, kind: Option<ProjectKind>) -> String {
    match kind {
        Some(ProjectKind::Design) => join(ph, "Design"),
        _ => ph.to_string(),
    }
}

pub(crate) fn list_roots_conn(conn: &rusqlite::Connection) -> Result<Vec<crate::contracts::RootConfig>> {
    let raw: Option<String> = conn
        .query_row("SELECT value_json FROM settings WHERE key = 'roots' AND scope = 'global'", [], |r| r.get(0))
        .optional()?;
    match raw {
        Some(json) => Ok(serde_json::from_str(&json)?),
        None => Ok(Vec::new()),
    }
}

pub(crate) fn volume_info_conn(conn: &rusqlite::Connection, volume_id: &str) -> Result<Option<(String, String, bool)>> {
    Ok(conn
        .query_row(
            "SELECT mount_point, COALESCE(NULLIF(label, ''), mount_point), status = 'online' FROM volumes WHERE id = ?1",
            [volume_id],
            |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?)),
        )
        .optional()?)
}

fn volume_info(store: &SqliteStore, volume_id: &str) -> Result<Option<(String, String, bool)>> {
    volume_info_conn(store.conn(), volume_id)
}

pub(crate) fn catalog_has_dir(conn: &rusqlite::Connection, volume_id: &str, dir: &str) -> Result<bool> {
    let prefix = format!("{}{SEP}", dir.trim_end_matches(SEP).to_lowercase());
    let plen = prefix.chars().count() as i64;
    Ok(conn.query_row(
        "SELECT EXISTS(SELECT 1 FROM files WHERE deleted_at IS NULL AND volume_id = ?1 AND substr(lower(path), 1, ?3) = ?2)",
        params![volume_id, prefix, plen],
        |r| r.get(0),
    )?)
}

pub(crate) fn projects_home_conn(
    conn: &rusqlite::Connection,
    roots: &[crate::contracts::RootConfig],
    volume_id: &str,
) -> Result<Option<String>> {
    if let Some(p) = conn
        .query_row(
            "SELECT value_json FROM settings WHERE key = ?1 AND scope = 'context'",
            [format!("{PROJECTS_HOME_KEY}:{volume_id}")],
            |r| r.get::<_, String>(0),
        )
        .optional()?
    {
        return Ok(Some(p));
    }
    let dirs: Vec<String> = roots
        .iter()
        .filter(|r| r.enabled && r.volume_id == volume_id && !GENERAL_FOLDERS.contains(&leaf(&r.path).to_lowercase().as_str()))
        .map(|r| r.path.clone())
        .collect();
    let mut stmt = conn.prepare("SELECT root_path FROM collection_locations WHERE volume_id = ?1")?;
    let locs: Vec<String> = stmt.query_map([volume_id], |r| r.get(0))?.collect::<rusqlite::Result<_>>()?;
    let best = dirs
        .iter()
        .map(|r| {
            let design_shelf = join(r, "Design");
            let count = locs
                .iter()
                .filter(|l| same_path(parent(l), r) || same_path(parent(l), &design_shelf))
                .count();
            (count, r)
        })
        .filter(|(n, _)| *n > 0)
        .max_by_key(|(n, _)| *n)
        .map(|(_, r)| r.clone());
    if best.is_some() {
        return Ok(best);
    }
    if let Some((mount, _, online)) = volume_info_conn(conn, volume_id)? {
        let p = join(&mount, "Projects");
        if (online && Path::new(&p).is_dir()) || (!online && catalog_has_dir(conn, volume_id, &p)?) {
            return Ok(Some(p));
        }
    }
    Ok(None)
}

/// The drive's projects home: the owner's setting; else, among the PC's roots, the one that
/// directly holds the most recognised projects (general places such as Documents never count);
/// else the drive's `Projects` folder if it exists; else none (projects stay where they are).
pub fn projects_home(store: &SqliteStore, volume_id: &str) -> Result<Option<String>> {
    let roots = store.list_roots()?;
    projects_home_conn(store.conn(), &roots, volume_id)
}

/// Where material from another copy goes: `Docs` (mostly documents), `Design` (mostly images
/// and design files) or `Material`.
fn material_folder(store: &SqliteStore, l: &Location) -> Result<&'static str> {
    let root = l.root_path.trim_end_matches(SEP);
    let prefix = format!("{}{SEP}", root.to_lowercase());
    let plen = prefix.chars().count() as i64;
    let mut stmt = store.conn().prepare(&format!(
        "SELECT lower(COALESCE(ext, '')), COUNT(*) FROM files WHERE {} AND volume_id = ?3 GROUP BY 1",
        live_under_root_sql()
    ))?;
    let rows: Vec<(String, i64)> =
        stmt.query_map(params![prefix, plen, l.volume_id], |r| Ok((r.get(0)?, r.get(1)?)))?.collect::<rusqlite::Result<_>>()?;
    let all: i64 = rows.iter().map(|r| r.1).sum();
    let of = |exts: &[&str]| rows.iter().filter(|r| exts.contains(&r.0.as_str())).map(|r| r.1).sum::<i64>();
    Ok(if all == 0 {
        "Material"
    } else if of(&DOC_EXTS) * 10 >= all * 7 {
        "Docs"
    } else if of(&DESIGN_EXTS) * 10 >= all * 7 {
        "Design"
    } else {
        "Material"
    })
}

/// What every plan needs to know about the PC, read once: the roots, every saved project
/// location, project metadata, and every Python virtual environment (a folder holding `pyvenv.cfg`).
/// Planning many projects with one `Planner` costs one pass over the catalog, not one per project.
pub struct Planner {
    roots: Vec<crate::contracts::RootConfig>,
    /// (collection id, volume id, root path) of every saved location.
    locations: Vec<(String, String, String)>,
    /// collection id -> (name, project_kind, is_repo) of saved folder projects.
    project_info: HashMap<String, (String, Option<ProjectKind>, bool)>,
    /// (volume id, folder) of every virtual environment.
    venvs: Vec<(String, String)>,
    /// Offline volume id -> set of lowercase ancestor folder paths present in `files`.
    offline_dirs: HashMap<String, std::collections::HashSet<String>>,
    /// Cached projects home per volume (resolved once without repeated catalog scans).
    projects_homes: HashMap<String, Option<String>>,
    /// Cached `ContentSummary` for multi-copy non-home same-drive folders.
    summaries: HashMap<(String, String), super::relate::ContentSummary>,
}

impl Planner {
    pub fn new(store: &SqliteStore) -> Result<Self> {
        let roots = store.list_roots()?;
        let locations: Vec<(String, String, String)> = {
            let mut stmt = store.conn().prepare("SELECT collection_id, volume_id, root_path FROM collection_locations")?;
            stmt.query_map([], |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?)))?.collect::<rusqlite::Result<_>>()?
        };
        let project_info = {
            let mut stmt = store.conn().prepare(
                "SELECT c.id, c.name, c.project_kind,
                        EXISTS(SELECT 1 FROM collection_locations l WHERE l.collection_id = c.id AND l.is_repo != 0)
                 FROM collections c WHERE c.kind = 'project'",
            )?;
            stmt.query_map([], |r| {
                let pk: Option<String> = r.get(2)?;
                Ok((r.get::<_, String>(0)?, (r.get::<_, String>(1)?, pk.map(|s| ProjectKind::from_db(&s)), r.get::<_, i64>(3)? != 0)))
            })?
            .collect::<rusqlite::Result<_>>()?
        };
        let venvs = {
            let mut stmt =
                store.conn().prepare("SELECT volume_id, path FROM files WHERE deleted_at IS NULL AND name = 'pyvenv.cfg' COLLATE NOCASE")?;
            let rows: Vec<(String, String)> = stmt.query_map([], |r| Ok((r.get(0)?, r.get(1)?)))?.collect::<rusqlite::Result<_>>()?;
            let mut v: Vec<(String, String)> = rows.into_iter().map(|(v, p)| (v, parent(&p).to_string())).collect();
            // `.venv` and `venv` folders aren't catalogued file by file: the scanner records them
            // whole as cache folders.
            let mut stmt =
                store.conn().prepare("SELECT volume_id, path FROM cache_dirs WHERE cache_kind = 'python_venv' AND state = 'active'")?;
            v.extend(stmt.query_map([], |r| Ok((r.get(0)?, r.get(1)?)))?.collect::<rusqlite::Result<Vec<(String, String)>>>()?);
            v
        };
        let volumes: HashMap<String, (String, String, bool)> = {
            let mut stmt = store.conn().prepare(
                "SELECT id, mount_point, COALESCE(NULLIF(label, ''), mount_point), status = 'online' FROM volumes",
            )?;
            stmt.query_map([], |r| Ok((r.get::<_, String>(0)?, (r.get::<_, String>(1)?, r.get::<_, String>(2)?, r.get::<_, bool>(3)?))))?
                .collect::<rusqlite::Result<_>>()?
        };
        let offline_vols: Vec<String> = volumes.iter().filter(|(_, (_, _, online))| !*online).map(|(id, _)| id.clone()).collect();
        let mut offline_dirs: HashMap<String, std::collections::HashSet<String>> = HashMap::new();
        if !offline_vols.is_empty() {
            let mut stmt = store.conn().prepare("SELECT path FROM files WHERE deleted_at IS NULL AND volume_id = ?1")?;
            for vol_id in offline_vols {
                let mut dirs = std::collections::HashSet::new();
                let mut rows = stmt.query([&vol_id])?;
                while let Some(row) = rows.next()? {
                    let path: String = row.get(0)?;
                    let lower = path.to_lowercase();
                    let mut cur = lower.as_str();
                    while let Some(i) = cur.rfind(SEP) {
                        cur = &cur[..i];
                        if cur.is_empty() || !dirs.insert(cur.to_string()) {
                            break;
                        }
                    }
                }
                offline_dirs.insert(vol_id, dirs);
            }
        }
        let mut projects_homes = HashMap::new();
        for vol_id in volumes.keys() {
            let ph = Self::resolve_projects_home(store.conn(), &roots, &locations, &volumes, &offline_dirs, vol_id)?;
            projects_homes.insert(vol_id.clone(), ph);
        }
        let summaries = {
            let mut stmt = store.conn().prepare(
                "SELECT l.collection_id, l.volume_id, l.root_path, l.relationship
                 FROM collection_locations l
                 JOIN collections c ON c.id = l.collection_id
                 WHERE c.kind = 'project' AND c.state NOT IN ('left_as_is', 'not_a_project')",
            )?;
            let rows: Vec<(String, String, String, String)> =
                stmt.query_map([], |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?, r.get(3)?)))?.collect::<rusqlite::Result<_>>()?;
            let mut by_cv: HashMap<(String, String), (bool, Vec<String>)> = HashMap::new();
            for (cid, vid, root, rel) in rows {
                let e = by_cv.entry((cid, vid)).or_default();
                if rel == "home" {
                    e.0 = true;
                }
                e.1.push(root);
            }
            let mut raw_targets: Vec<(String, String)> = Vec::new();
            for ((_, vid), (has_home, paths)) in by_cv {
                if !has_home
                    && paths.len() >= 2
                    && volumes.get(&vid).is_some_and(|(m, _, _)| !m.is_empty())
                {
                    for p in paths {
                        if volumes.get(&vid).is_some_and(|(m, _, _)| path_is_within(&p, m, false)) {
                            raw_targets.push((vid.clone(), p));
                        }
                    }
                }
            }
            raw_targets.sort();
            raw_targets.dedup();
            let targets: Vec<(String, String)> = raw_targets
                .iter()
                .filter(|(v, p)| !raw_targets.iter().any(|(ov, op)| ov == v && !same_path(op, p) && (inside(p, op) || inside(op, p))))
                .cloned()
                .collect();
            let mut map = HashMap::new();
            if !targets.is_empty() {
                let mut root_scopes: Vec<(String, Option<String>)> = Vec::new();
                let mut seen_vols = std::collections::HashSet::new();
                for (v, _) in &targets {
                    if seen_vols.insert(v.clone())
                        && let Some((mount, _, _)) = volumes.get(v)
                    {
                        root_scopes.push((mount.clone(), Some(v.clone())));
                    }
                }
                let sums = super::relate::content_summaries(store, &root_scopes, &targets)?;
                for ((vid, path), sum) in targets.into_iter().zip(sums) {
                    map.insert((vid, path.trim_end_matches(SEP).to_lowercase()), sum);
                }
            }
            map
        };
        Ok(Self {
            roots,
            locations,
            project_info,
            venvs,
            offline_dirs,
            projects_homes,
            summaries,
        })
    }

    fn resolve_projects_home(
        conn: &rusqlite::Connection,
        roots: &[crate::contracts::RootConfig],
        locations: &[(String, String, String)],
        volumes: &HashMap<String, (String, String, bool)>,
        offline_dirs: &HashMap<String, std::collections::HashSet<String>>,
        volume_id: &str,
    ) -> Result<Option<String>> {
        if let Some(p) = conn
            .query_row(
                "SELECT value_json FROM settings WHERE key = ?1 AND scope = 'context'",
                [format!("{PROJECTS_HOME_KEY}:{volume_id}")],
                |r| r.get::<_, String>(0),
            )
            .optional()?
        {
            return Ok(Some(p));
        }
        let dirs: Vec<&str> = roots
            .iter()
            .filter(|r| r.enabled && r.volume_id == volume_id && !GENERAL_FOLDERS.contains(&leaf(&r.path).to_lowercase().as_str()))
            .map(|r| r.path.as_str())
            .collect();
        let best = dirs
            .iter()
            .map(|r| {
                let design_shelf = join(r, "Design");
                let count = locations
                    .iter()
                    .filter(|(_, v, l)| v == volume_id && (same_path(parent(l), r) || same_path(parent(l), &design_shelf)))
                    .count();
                (count, *r)
            })
            .filter(|(n, _)| *n > 0)
            .max_by_key(|(n, _)| *n)
            .map(|(_, r)| r.to_string());
        if best.is_some() {
            return Ok(best);
        }
        if let Some((mount, _, online)) = volumes.get(volume_id) {
            let p = join(mount, "Projects");
            let has = if *online {
                Path::new(&p).is_dir()
            } else if let Some(dirs) = offline_dirs.get(volume_id) {
                dirs.contains(&p.trim_end_matches(SEP).to_lowercase())
            } else {
                catalog_has_dir(conn, volume_id, &p)?
            };
            if has {
                return Ok(Some(p));
            }
        }
        Ok(None)
    }

    fn dir_exists(&self, store: &SqliteStore, volume_id: &str, vol_online: bool, path: &str) -> bool {
        if vol_online {
            Path::new(path).exists()
        } else if let Some(dirs) = self.offline_dirs.get(volume_id) {
            dirs.contains(&path.trim_end_matches(SEP).to_lowercase())
        } else {
            catalog_has_dir(store.conn(), volume_id, path).unwrap_or(false)
        }
    }

    fn projects_home_for(&self, store: &SqliteStore, volume_id: &str) -> Result<Option<String>> {
        if let Some(ph) = self.projects_homes.get(volume_id) {
            return Ok(ph.clone());
        }
        projects_home_conn(store.conn(), &self.roots, volume_id)
    }

    fn design_home_for(&self, store: &SqliteStore, volume_id: &str) -> Result<Option<String>> {
        if let Some(ph) = self.projects_home_for(store, volume_id)? {
            return Ok(Some(ph));
        }
        Ok(self
            .roots
            .iter()
            .find(|r| r.enabled && r.volume_id == volume_id && matches!(leaf(&r.path).to_lowercase().as_str(), "prj" | "projects"))
            .map(|r| r.path.clone()))
    }

    fn content_summary_for(&self, store: &SqliteStore, volume_id: &str, root: &str) -> Result<super::relate::ContentSummary> {
        let key = (volume_id.to_string(), root.trim_end_matches(SEP).to_lowercase());
        if let Some(sum) = self.summaries.get(&key) {
            return Ok(sum.clone());
        }
        super::relate::content_summary(store, volume_id, root)
    }

    fn is_venv(&self, l: &Location) -> bool {
        self.venvs.iter().any(|(v, d)| *v == l.volume_id && same_path(d, &l.root_path))
    }

    fn holds_venv(&self, l: &Location) -> bool {
        self.venvs.iter().any(|(v, d)| *v == l.volume_id && path_is_within(d, &l.root_path, false))
    }

    /// Warnings for moving a folder: a Python virtual environment inside it (any folder) will
    /// need recreating; a git repository that changed lately may be open in an editor.
    fn repo_warnings(&self, l: &Location) -> Vec<String> {
        let mut out = Vec::new();
        if self.holds_venv(l) {
            out.push("It holds a Python virtual environment, which will need recreating after the move.".into());
        }
        if l.is_repo && l.newest_mtime >= crate::now_ms() - 30 * 86_400_000 {
            out.push("It changed in the last 30 days: close any editor or terminal using it first.".into());
        }
        out
    }

    /// The plans for one project, design set, or old copy: one per drive it lives on. None for
    /// items left as they are, dismissed as not a project, or loose media/folder modules handled by
    /// blueprint filing directly.
    pub fn plan_for(&self, store: &SqliteStore, c: &Collection) -> Result<Vec<ProjectPlan>> {
        if matches!(c.state.as_str(), "left_as_is" | "not_a_project")
            || !matches!(c.kind, CollectionKind::Project | CollectionKind::DesignSet | CollectionKind::OldCopy)
            || c.locations.is_empty()
        {
            return Ok(Vec::new());
        }
        let mut volumes: Vec<&str> = Vec::new();
        for l in &c.locations {
            if !volumes.contains(&l.volume_id.as_str()) {
                volumes.push(&l.volume_id);
            }
        }
        match c.kind {
            CollectionKind::DesignSet => volumes.iter().map(|v| plan_design_set(self, store, c, v)).collect(),
            CollectionKind::OldCopy => volumes.iter().map(|v| plan_old_copy(self, store, c, v)).collect(),
            CollectionKind::Project => volumes.iter().map(|v| plan_on(self, store, c, v)).collect(),
            _ => Ok(Vec::new()),
        }
    }
}

/// The plans for one project (see [`Planner::plan_for`]; use one `Planner` for many).
pub fn plan_for(store: &SqliteStore, c: &Collection) -> Result<Vec<ProjectPlan>> {
    Planner::new(store)?.plan_for(store, c)
}

fn plan_on(ctx: &Planner, store: &SqliteStore, c: &Collection, volume_id: &str) -> Result<ProjectPlan> {
    let here: Vec<&Location> = c.locations.iter().filter(|l| l.volume_id == volume_id).collect();
    // The project's home if it is on this drive; otherwise this drive's best copy. Copies on a
    // drive without the home were related to the home, not to each other: they get no step.
    let main = here.iter().find(|l| l.relationship == Relationship::Home).copied();
    let home: &Location = main.unwrap_or_else(|| {
        let owned: Vec<Location> = here.iter().map(|l| (*l).clone()).collect();
        here[choose_home(&owned)]
    });
    let info = volume_info(store, volume_id)?;
    let waits_for = match &info {
        Some((_, label, false)) => Some(format!("{label} is unplugged.")),
        None => Some("That drive isn't known any more.".into()),
        _ if here.iter().any(|l| !l.online) => Some("One of its drives is unplugged.".into()),
        _ => None,
    };
    let nothing = |summary: &str| ProjectPlan {
        collection_id: c.id.clone(),
        volume_id: volume_id.to_string(),
        home_after: home.root_path.clone(),
        steps: Vec::new(),
        waits_for: waits_for.clone(),
        notes: Vec::new(),
        rank: 9,
        summary: summary.to_string(),
        digest: plan_digest(&c.id, volume_id, &home.root_path, &[]),
    };
    // Per-drive planning (spec §8, §17): every step stays strictly inside `volume_id`. When a
    // drive doesn't hold the project's global home, copies on it are NEVER quarantined against the
    // home on another drive — same-drive copies are compared strictly against `volume_id`'s own
    // best copy (`home`), while a sole copy on that drive only moves into that drive's Projects
    // folder if eligible.
    let roots = &ctx.roots;
    let mount = info.as_ref().map(|(m, _, _)| m.as_str());
    let vol_online = info.as_ref().is_some_and(|(_, _, on)| *on) && waits_for.is_none();
    let dir_exists = |p: &str| ctx.dir_exists(store, volume_id, vol_online, p);
    // - Nothing inside another project (or holding one) is moved on this project's behalf, and
    //   nothing that is a Python virtual environment is moved at all (it would break).
    let others: Vec<String> =
        ctx.locations.iter().filter(|(cid, v, _)| v == volume_id && *cid != c.id).map(|(_, _, p)| p.clone()).collect();
    let nested = |l: &Location| others.iter().any(|o| inside(&l.root_path, o) || inside(o, &l.root_path));
    // - A copy changed in the last 30 days is live work: it stays where it is.
    let recent = crate::now_ms() - 30 * 86_400_000;
    let mut blocked: std::collections::HashSet<String> = std::collections::HashSet::new();
    let mut notes: Vec<String> = Vec::new();
    for l in &here {
        let why = if nested(l) {
            Some("it sits inside another project")
        } else if ctx.is_venv(l) {
            Some("it is a Python environment, which breaks if moved")
        } else if l.id != home.id && l.newest_mtime >= recent {
            Some("it changed in the last 30 days, so it may be work in progress")
        } else {
            None
        };
        if let Some(why) = why {
            blocked.insert(l.id.clone());
            notes.push(format!("{} stays where it is: {why}.", l.root_path));
        }
    }
    if blocked.contains(&home.id) {
        let mut p = nothing("Kept where it is.");
        p.notes = notes;
        return Ok(p);
    }
    // - A home moves into the projects home only from inside a projects folder (a folder that
    //   isn't a general place such as Documents or Downloads and holds two or more projects):
    //   a tool found elsewhere stays put.
    let all_locs: Vec<&String> = others.iter().chain(here.iter().map(|l| &l.root_path)).collect();
    let is_root = |d: &str| roots.iter().any(|r| same_path(&r.path, d)) || mount.is_some_and(|m| same_path(m, d));
    let mut in_projects_folder = false;
    let mut d = parent(&home.root_path);
    while !d.is_empty() && !is_root(d) && d.contains(SEP) {
        if !GENERAL_FOLDERS.contains(&leaf(d).to_lowercase().as_str()) && all_locs.iter().filter(|p| inside(p, d)).count() >= 2 {
            in_projects_folder = true;
            break;
        }
        let up = parent(d);
        if up == d {
            break;
        }
        d = up;
    }
    let ph = ctx.projects_home_for(store, volume_id)?;
    let already_in_ph = ph.as_deref().is_some_and(|ph| path_is_within(&home.root_path, ph, false));
    let eligible_home_move = in_projects_folder || already_in_ph;

    // Where the home ends up: under its own folder name. A name the owner gives the project on
    // the card is a label; renaming the folder could break paths inside it.
    let name = safe_name(leaf(&home.root_path));
    let mut steps: Vec<PlanStep> = Vec::new();
    let mut aside: Option<(&Location, String)> = None;
    let mut home_after = home.root_path.clone();
    if eligible_home_move
        && let Some(ph) = ph.as_deref()
    {
        let design_shelf = join(ph, "Design");
        let already_home = match c.project_kind {
            Some(ProjectKind::Design) => path_is_within(&home.root_path, &design_shelf, false),
            _ => path_is_within(&home.root_path, ph, false) && !path_is_within(&home.root_path, &design_shelf, false),
        };
        if !already_home {
            let dest_parent = kind_home(ph, c.project_kind);
            let mut target = join(&dest_parent, &name);
            let occupant = here.iter().find(|l| l.id != home.id && same_path(&l.root_path, &target)).copied();
            let taken_by_other = occupant.is_none() && dir_exists(&target);
            if taken_by_other || occupant.is_some_and(|o| o.keep_here || blocked.contains(&o.id)) {
                let year = &date_of(home.newest_mtime)[..4];
                target = join(&dest_parent, &format!("{name} ({} {year})", kind_word(c.project_kind)));
            } else if let Some(o) = occupant {
                let mut moved = join(&dest_parent, &format!("{name} (old copy)"));
                let mut n = 2;
                while dir_exists(&moved) {
                    moved = join(&dest_parent, &format!("{name} (old copy {n})"));
                    n += 1;
                }
                aside = Some((o, moved.clone()));
                steps.push(PlanStep {
                    step: 1,
                    kind: StepKind::MoveAside,
                    location_id: o.id.clone(),
                    from: o.root_path.clone(),
                    to: moved,
                    fallback: None,
                    reason: "Moved aside first, so the main copy can take its name.".into(),
                    warnings: ctx.repo_warnings(o),
                });
            }
            if dir_exists(&target) && !aside.as_ref().is_some_and(|(o, _)| same_path(&o.root_path, &target)) {
                // Even the disambiguated name is taken: leave the home where it is.
                target = home.root_path.clone();
            }
            if !same_path(&target, &home.root_path) {
                let where_label = if c.project_kind == Some(ProjectKind::Design) {
                    "your design projects folder"
                } else {
                    "your projects home"
                };
                steps.push(PlanStep {
                    step: 2,
                    kind: StepKind::MoveHome,
                    location_id: home.id.clone(),
                    from: home.root_path.clone(),
                    to: target.clone(),
                    fallback: None,
                    reason: format!("Moves whole to {target} ({where_label})."),
                    warnings: ctx.repo_warnings(home),
                });
                home_after = target;
            }
        }
    }

    // Where material and versions from the other copies on this same drive go.
    let base = if home.is_repo { join(parent(&home_after), &format!("{} - material", leaf(&home_after))) } else { home_after.clone() };
    let versions = join(&base, "Versions");
    let origin = |l: &Location| leaf(parent(&l.root_path)).to_string();
    let mut taken: std::collections::HashSet<String> = std::collections::HashSet::new();
    let mut unique = |p: String| -> String {
        let mut out = p.clone();
        let mut n = 2;
        while taken.contains(&out.to_lowercase()) || dir_exists(&out) {
            out = format!("{p} {n}");
            n += 1;
        }
        taken.insert(out.to_lowercase());
        out
    };
    let same_vol_rel: HashMap<String, Relationship> = if main.is_some() {
        here.iter().filter(|l| l.id != home.id).map(|l| (l.id.clone(), l.relationship)).collect()
    } else if here.len() >= 2 {
        let home_sum = ctx.content_summary_for(store, volume_id, &home.root_path)?;
        let mut m = HashMap::new();
        if !home_sum.is_empty() {
            for l in here.iter().filter(|l| l.id != home.id) {
                let other_sum = ctx.content_summary_for(store, volume_id, &l.root_path)?;
                if !other_sum.is_empty() {
                    let (mut rel, _, _) = super::relate::relate(&home_sum, &other_sum);
                    if super::relate::version_series(leaf(&home.root_path), leaf(&l.root_path))
                        && !matches!(rel, Relationship::Identical | Relationship::OlderSubset)
                    {
                        rel = Relationship::Version;
                    }
                    m.insert(l.id.clone(), rel);
                }
            }
        }
        m
    } else {
        HashMap::new()
    };
    if !same_vol_rel.is_empty() {
        for l in here.iter().filter(|l| l.id != home.id && !l.keep_here && !blocked.contains(&l.id)) {
            let Some(&rel) = same_vol_rel.get(&l.id) else {
                continue;
            };
            // A copy inside the home (or holding it) is not moved on its own.
            if inside(&l.root_path, &home.root_path) || inside(&home.root_path, &l.root_path) {
                continue;
            }
            let (from, step) = match &aside {
                Some((o, moved)) if o.id == l.id => (moved.clone(), 3),
                _ => (l.root_path.clone(), 4),
            };
            let dated = |label: &str| join(&versions, &format!("{} {label}", date_of(l.newest_mtime)));
            let (kind, to, fallback, reason) = match rel {
                Relationship::Identical | Relationship::OlderSubset if !l.is_repo => {
                    let why = if rel == Relationship::Identical {
                        "Same as the main copy on this drive: to quarantine for 30 days."
                    } else {
                        "Older copy: everything here is in this drive's main copy. To quarantine for 30 days."
                    };
                    (StepKind::Quarantine, from.clone(), Some(unique(dated(&origin(l)))), why.to_string())
                }
                Relationship::Identical | Relationship::OlderSubset => {
                    let to = unique(dated(&origin(l)));
                    (StepKind::ToVersions, to.clone(), None, format!("A git repository, so it is kept as a version in {to}, never quarantined."))
                }
                Relationship::Diverged => {
                    let to = unique(dated(&origin(l)));
                    (StepKind::ToVersions, to.clone(), None, format!("Changed on both sides: kept whole as a version in {to}."))
                }
                Relationship::Version => {
                    let to = unique(dated(leaf(&l.root_path)));
                    (StepKind::ToVersions, to.clone(), None, format!("An earlier version: kept in {to}."))
                }
                Relationship::DifferentMaterial => {
                    let to = unique(join(&base, material_folder(store, l)?));
                    (StepKind::ToMaterial, to.clone(), None, format!("Different material: brought into {to}."))
                }
                _ => continue,
            };
            steps.push(PlanStep {
                step,
                kind,
                location_id: l.id.clone(),
                from,
                to,
                fallback,
                reason,
                warnings: ctx.repo_warnings(l),
            });
        }
    }
    // An aside copy always gets its later step, or it would be left under its temporary name.
    if let Some((o, moved)) = &aside
        && !steps.iter().any(|s| s.step == 3)
    {
        let to = unique(join(&versions, &format!("{} {}", date_of(o.newest_mtime), origin(o))));
        steps.push(PlanStep {
            step: 3,
            kind: StepKind::ToVersions,
            location_id: o.id.clone(),
            from: moved.clone(),
            to: to.clone(),
            fallback: None,
            reason: format!("Kept whole as a version in {to}."),
            warnings: Vec::new(),
        });
    }
    steps.sort_by_key(|s| s.step);
    // Renumber 1.. in order, so the card and the batch agree.
    for (i, s) in steps.iter_mut().enumerate() {
        s.step = i as u32 + 1;
    }
    if let Some((mount, _, _)) = &info
        && let Some(bad) = steps.iter().find(|s| !path_is_within(&s.from, mount, false) || !path_is_within(&s.to, mount, false))
    {
        return Err(AegisError::InvalidInput(format!("A plan step would leave its drive ({} → {}).", bad.from, bad.to)));
    }
    let summary = summarise(&steps, &home_after);
    let digest = plan_digest(&c.id, volume_id, &home_after, &steps);
    let mut plan = ProjectPlan { collection_id: c.id.clone(), volume_id: volume_id.to_string(), home_after, steps, waits_for, notes, summary, rank: 9, digest };
    if !plan.steps.is_empty() {
        plan.rank = offer_rank(c, &plan);
    }
    Ok(plan)
}

pub(crate) fn design_projects_home_conn(
    conn: &rusqlite::Connection,
    roots: &[crate::contracts::RootConfig],
    volume_id: &str,
) -> Result<Option<String>> {
    if let Some(ph) = projects_home_conn(conn, roots, volume_id)? {
        return Ok(Some(ph));
    }
    Ok(roots
        .iter()
        .find(|r| r.enabled && r.volume_id == volume_id && matches!(leaf(&r.path).to_lowercase().as_str(), "prj" | "projects"))
        .map(|r| r.path.clone()))
}

pub(crate) fn existing_design_project_dir(
    locations: &[(String, String, String)],
    project_info: &HashMap<String, (String, Option<ProjectKind>, bool)>,
    volume_id: &str,
    ph: &str,
    shelf: &str,
    collection_name: &str,
) -> Option<String> {
    let norm_c = normalized_name(collection_name);
    let mut cands: Vec<(&str, bool)> = locations
        .iter()
        .filter_map(|(cid, v, r)| {
            if v != volume_id || !path_is_within(r, ph, false) {
                return None;
            }
            let (pname, pk, is_repo) = project_info.get(cid)?;
            if *is_repo || *pk != Some(ProjectKind::Design) {
                return None;
            }
            if normalized_name(pname) == norm_c || normalized_name(leaf(r)) == norm_c {
                Some((r.as_str(), path_is_within(r, shelf, false)))
            } else {
                None
            }
        })
        .collect();
    cands.sort_by_key(|(r, in_shelf)| (std::cmp::Reverse(*in_shelf), r.to_lowercase()));
    cands.first().map(|(r, _)| (*r).to_string())
}

fn plan_design_set(ctx: &Planner, store: &SqliteStore, c: &Collection, volume_id: &str) -> Result<ProjectPlan> {
    let mut here: Vec<&Location> = c.locations.iter().filter(|l| l.volume_id == volume_id).collect();
    here.sort_by_key(|l| (std::cmp::Reverse(l.files), l.root_path.to_lowercase()));
    let home_dir = here.first().map(|l| l.root_path.clone()).unwrap_or_default();
    let info = volume_info(store, volume_id)?;
    let waits_for = match &info {
        Some((_, label, false)) => Some(format!("{label} is unplugged.")),
        None => Some("That drive isn't known any more.".into()),
        _ if here.iter().any(|l| !l.online) => Some("One of its drives is unplugged.".into()),
        _ => None,
    };
    let nothing = |summary: &str| ProjectPlan {
        collection_id: c.id.clone(),
        volume_id: volume_id.to_string(),
        home_after: home_dir.clone(),
        steps: Vec::new(),
        waits_for: waits_for.clone(),
        notes: Vec::new(),
        rank: 9,
        summary: summary.to_string(),
        digest: plan_digest_with_members(&c.id, volume_id, &home_dir, &[], &[]),
    };
    let vol_online = info.as_ref().is_some_and(|(_, _, on)| *on) && waits_for.is_none();
    let dir_exists = |p: &str| ctx.dir_exists(store, volume_id, vol_online, p);
    let Some(ph) = ctx.design_home_for(store, volume_id)? else {
        return Ok(nothing("No projects home is set on this drive yet."));
    };
    let shelf = kind_home(&ph, c.project_kind);
    let base_name = safe_name(&c.name);
    let is_design_target = c.project_kind.unwrap_or(ProjectKind::Design) == ProjectKind::Design;
    let matches_existing_design = |dir: &str| {
        ctx.locations.iter().any(|(cid, v, r)| {
            v == volume_id
                && same_path(r, dir)
                && ctx.project_info.get(cid).is_some_and(|(pname, pk, is_repo)| {
                    !*is_repo && (*pk == Some(ProjectKind::Design) || normalized_name(pname) == normalized_name(&c.name))
                })
        })
    };
    let existing_match = if is_design_target {
        existing_design_project_dir(&ctx.locations, &ctx.project_info, volume_id, &ph, &shelf, &c.name)
    } else {
        None
    };
    let mut target = existing_match.unwrap_or_else(|| join(&shelf, &base_name));
    let conflicts = |dir: &str| {
        (dir_exists(dir) || ctx.locations.iter().any(|(_, v, r)| v == volume_id && same_path(r, dir)))
            && (!is_design_target || !matches_existing_design(dir))
    };
    if conflicts(&target) {
        let newest = here.iter().map(|l| l.newest_mtime).max().unwrap_or_else(now_ms);
        let year = &date_of(newest)[..4];
        target = join(&shelf, &format!("{base_name} (design {year})"));
        if conflicts(&target) {
            return Ok(nothing("A folder with that name is already in your projects home."));
        }
    }

    let members: Vec<_> = super::store::list_members(store.conn(), &c.id)?
        .into_iter()
        .filter(|m| m.volume_id == volume_id)
        .collect();
    let mut steps: Vec<PlanStep> = Vec::new();
    let mut notes: Vec<String> = Vec::new();
    let mut digest_members: Vec<(String, String)> = Vec::new();
    let mut moved_files = 0usize;
    let mut moved_dirs = 0usize;
    for l in &here {
        let dir_members: Vec<_> = members.iter().filter(|m| same_path(parent(&m.path), &l.root_path)).collect();
        if dir_members.is_empty() {
            continue;
        }
        if l.keep_here {
            notes.push(format!("{} stays where it is: you chose Keep here.", l.root_path));
            continue;
        }
        if same_path(&l.root_path, &target) {
            notes.push(format!("{} is already in {target}.", l.root_path));
            continue;
        }
        moved_dirs += 1;
        moved_files += dir_members.len();
        for m in &dir_members {
            digest_members.push((m.file_id.clone(), m.path.clone()));
        }
        steps.push(PlanStep {
            step: steps.len() as u32 + 1,
            kind: StepKind::GatherFiles,
            location_id: l.id.clone(),
            from: l.root_path.clone(),
            to: target.clone(),
            fallback: None,
            reason: format!(
                "Brings {} {} {} into {target}.",
                dir_members.len(),
                c.name,
                if dir_members.len() == 1 { "file" } else { "files" }
            ),
            warnings: Vec::new(),
        });
    }
    if let Some((mount, _, _)) = &info
        && let Some(bad) = steps.iter().find(|s| !path_is_within(&s.from, mount, false) || !path_is_within(&s.to, mount, false))
    {
        return Err(AegisError::InvalidInput(format!("A plan step would leave its drive ({} → {}).", bad.from, bad.to)));
    }
    let summary = if steps.is_empty() {
        "Nothing to do: these files stay where they are.".into()
    } else {
        format!(
            "Brings {moved_files} {} from {moved_dirs} {} into {target}.",
            if moved_files == 1 { "file" } else { "files" },
            if moved_dirs == 1 { "folder" } else { "folders" },
        )
    };
    let digest = plan_digest_with_members(&c.id, volume_id, &target, &steps, &digest_members);
    let rank = if steps.is_empty() { 9 } else { 2 };
    Ok(ProjectPlan {
        collection_id: c.id.clone(),
        volume_id: volume_id.to_string(),
        home_after: target,
        steps,
        waits_for,
        notes,
        summary,
        rank,
        digest,
    })
}

fn plan_old_copy(ctx: &Planner, store: &SqliteStore, c: &Collection, volume_id: &str) -> Result<ProjectPlan> {
    let Some(loc) = c.locations.iter().find(|l| l.volume_id == volume_id) else {
        return Err(AegisError::InvalidInput("Old copy has no location on that drive.".into()));
    };
    let info = volume_info(store, volume_id)?;
    let waits_for = match &info {
        Some((_, label, false)) => Some(format!("{label} is unplugged.")),
        None => Some("That drive isn't known any more.".into()),
        _ if !loc.online => Some("One of its drives is unplugged.".into()),
        _ => None,
    };
    let nothing = |summary: &str| ProjectPlan {
        collection_id: c.id.clone(),
        volume_id: volume_id.to_string(),
        home_after: loc.root_path.clone(),
        steps: Vec::new(),
        waits_for: waits_for.clone(),
        notes: Vec::new(),
        rank: 9,
        summary: summary.to_string(),
        digest: plan_digest(&c.id, volume_id, &loc.root_path, &[]),
    };
    if loc.keep_here {
        return Ok(nothing("Kept where it is: you chose Keep here."));
    }
    let fp_str = super::store::collection_fingerprint(store.conn(), &c.id)?;
    let (archive_rel, same_vol_live) = match fp_str.as_deref().and_then(|s| s.strip_prefix("old_copy:")) {
        Some(rest) => match rest.split_once('|') {
            Some((rel, live)) => (rel.to_string(), live.to_string()),
            None => (rest.to_string(), String::new()),
        },
        None => (safe_name(&c.name), String::new()),
    };
    let on_pc = ctx.roots.iter().any(|r| r.enabled && path_is_within(&loc.root_path, &r.path, false));
    let mount = info.as_ref().map(|(m, _, _)| m.clone()).unwrap_or_default();
    let archive_base = if on_pc {
        let docs = ctx
            .roots
            .iter()
            .find(|r| {
                r.enabled
                    && r.volume_id == volume_id
                    && matches!(leaf(&r.path).to_lowercase().as_str(), "documents" | "my documents")
            })
            .or_else(|| ctx.roots.iter().find(|r| r.enabled && r.volume_id == volume_id));
        docs.map(|r| join(&r.path, "Archive")).unwrap_or_else(|| join(&mount, "Archive"))
    } else if !mount.is_empty() {
        join(&mount, "Archive")
    } else {
        return Ok(nothing("That drive isn't known any more."));
    };
    if path_is_within(&loc.root_path, &archive_base, false) {
        return Ok(nothing("Already in Archive."));
    }
    let vol_online = info.as_ref().is_some_and(|(_, _, on)| *on) && waits_for.is_none();
    let dir_exists = |p: &str| ctx.dir_exists(store, volume_id, vol_online, p);
    let mut target = join(&archive_base, &archive_rel);
    let base_target = target.clone();
    let mut n = 2;
    while dir_exists(&target) || ctx.locations.iter().any(|(_, v, r)| v == volume_id && same_path(r, &target)) {
        target = format!("{base_target} {n}");
        n += 1;
    }

    let can_quarantine = !same_vol_live.is_empty()
        && matches!(loc.relationship, Relationship::Identical | Relationship::OlderSubset)
        && info.as_ref().is_some_and(|(m, _, _)| path_is_within(&same_vol_live, m, false));

    let (steps, home_after) = if can_quarantine {
        (
            vec![PlanStep {
                step: 1,
                kind: StepKind::Quarantine,
                location_id: loc.id.clone(),
                from: loc.root_path.clone(),
                to: loc.root_path.clone(),
                fallback: Some(target),
                reason: format!("Same as {same_vol_live} on this drive: to quarantine for 30 days (verified file by file)."),
                warnings: ctx.repo_warnings(loc),
            }],
            same_vol_live,
        )
    } else {
        (
            vec![PlanStep {
                step: 1,
                kind: StepKind::ToArchive,
                location_id: loc.id.clone(),
                from: loc.root_path.clone(),
                to: target.clone(),
                fallback: None,
                reason: format!("Moves whole to archive at {target}."),
                warnings: ctx.repo_warnings(loc),
            }],
            target,
        )
    };
    if let Some((m, _, _)) = &info
        && let Some(bad) = steps.iter().find(|s| !path_is_within(&s.from, m, false) || !path_is_within(&s.to, m, false))
    {
        return Err(AegisError::InvalidInput(format!("A plan step would leave its drive ({} → {}).", bad.from, bad.to)));
    }
    let summary = summarise(&steps, &home_after);
    let digest = plan_digest(&c.id, volume_id, &home_after, &steps);
    let mut plan = ProjectPlan {
        collection_id: c.id.clone(),
        volume_id: volume_id.to_string(),
        home_after,
        steps,
        waits_for,
        notes: Vec::new(),
        rank: 9,
        summary,
        digest,
    };
    if !plan.steps.is_empty() {
        plan.rank = offer_rank(c, &plan);
    }
    Ok(plan)
}

/// Whether `batch_id` is a project plan's batch (undone with [`undo`], not as a filing batch).
pub fn is_project_batch(conn: &rusqlite::Connection, batch_id: &str) -> Result<bool> {
    Ok(conn
        .query_row("SELECT origin LIKE 'project:%' FROM action_batches WHERE id = ?1", [batch_id], |r| r.get(0))
        .optional()?
        .unwrap_or(false))
}

/// The order cards are offered in (spec §8.5), simplest and safest first: 0 identical copies
/// (space back, nothing lost), 1 older copies, 2 projects that only move into the projects
/// home (or old copies moving to Archive), 3 anything that keeps versions or brings material in.
pub fn offer_rank(c: &Collection, p: &ProjectPlan) -> u8 {
    let rel = |id: &str| c.locations.iter().find(|l| l.id == id).map(|l| l.relationship);
    if p.steps.iter().any(|s| matches!(s.kind, StepKind::ToVersions | StepKind::ToMaterial)) {
        3
    } else if p.steps.iter().any(|s| s.kind == StepKind::Quarantine && rel(&s.location_id) == Some(Relationship::OlderSubset)) {
        1
    } else if p.steps.iter().any(|s| s.kind == StepKind::Quarantine) {
        0
    } else {
        2
    }
}

/// How a plan run ended.
#[derive(TS, Serialize, Deserialize, Clone, PartialEq, Debug)]
#[ts(export)]
pub struct PlanOutcome {
    pub batch_id: String,
    /// Everything was done. When false, nothing was changed: steps already done were undone.
    pub done: bool,
    /// Why it stopped, in plain words.
    pub reason: Option<String>,
    /// Files that moved (counting every file inside the folders).
    #[ts(type = "number")]
    pub moved_files: i64,
    /// Space quarantined copies will free when their 30 days are up.
    #[ts(type = "number")]
    pub freed_bytes: i64,
}

/// The batch origin of a project plan: `project:<scope key>:<project name>`, where the scope
/// key is `pc` for the PC's drive and the volume id otherwise.
fn origin_of(store: &SqliteStore, c: &Collection, plan: &ProjectPlan) -> Result<String> {
    let roots = store.list_roots()?;
    let on_pc = c
        .locations
        .iter()
        .filter(|l| l.volume_id == plan.volume_id)
        .any(|l| roots.iter().any(|r| r.enabled && path_is_within(&l.root_path, &r.path, false)));
    let key = if on_pc { "pc".to_string() } else { plan.volume_id.clone() };
    Ok(format!("project:{key}:{}", c.name))
}

pub(crate) fn unique_file_in_dir(dir: &str, name: &str, taken: &mut HashSet<String>) -> String {
    let base = join(dir, name);
    if !Path::new(&base).exists() && taken.insert(base.to_lowercase()) {
        return base;
    }
    let p = Path::new(name);
    let stem = p.file_stem().map(|s| s.to_string_lossy().to_string()).unwrap_or_else(|| name.to_string());
    let ext = p.extension().map(|e| format!(".{}", e.to_string_lossy())).unwrap_or_default();
    for n in 2.. {
        let cand = join(dir, &format!("{stem} ({n}){ext}"));
        if !Path::new(&cand).exists() && taken.insert(cand.to_lowercase()) {
            return cand;
        }
    }
    unreachable!()
}

/// Proposes `plan` (for collection `c`) as one batch of folder steps (or file moves for a
/// design set), replacing any plan for the same project that is still waiting.
pub fn propose(store: &mut SqliteStore, c: &Collection, plan: &ProjectPlan) -> Result<String> {
    use crate::contracts::{ActionType, ProposedActionInput, RefilePlan, TargetKind};
    if let Some(w) = &plan.waits_for {
        return Err(AegisError::InvalidInput(w.clone()));
    }
    if plan.steps.is_empty() {
        return Err(AegisError::InvalidInput("Nothing to do: it is already where it belongs.".into()));
    }
    let origin = origin_of(store, c, plan)?;
    store.conn().execute(
        "UPDATE actions SET status = 'superseded'
         WHERE status IN ('proposed', 'approved')
           AND batch_id IN (SELECT id FROM action_batches WHERE origin = ?1 AND status IN ('draft', 'approved'))",
        [&origin],
    )?;
    if c.kind == CollectionKind::DesignSet {
        let set_fp = super::store::collection_fingerprint(store.conn(), &c.id)?
            .unwrap_or_else(|| super::sets::set_fingerprint(&c.name));
        let members: Vec<_> = super::store::list_members(store.conn(), &c.id)?
            .into_iter()
            .filter(|m| m.volume_id == plan.volume_id)
            .collect();
        let mut taken: HashSet<String> = HashSet::new();
        let mut actions = Vec::new();
        for st in &plan.steps {
            if st.kind != StepKind::GatherFiles {
                continue;
            }
            let mut matching: Vec<_> = members.iter().filter(|m| same_path(parent(&m.path), &st.from)).collect();
            matching.sort_by_key(|m| m.path.to_lowercase());
            for m in matching {
                if !Path::new(&m.path).is_file() {
                    return Err(AegisError::NotFound(format!(
                        "{} is no longer there. Cairn will make a fresh plan.",
                        m.path
                    )));
                }
                let dest = unique_file_in_dir(&st.to, leaf(&m.path), &mut taken);
                let ev = serde_json::json!({
                    "step": st.step,
                    "exact_dest": true,
                    "project": c.name,
                    "kind": StepKind::GatherFiles,
                    "collection_id": c.id,
                    "location_id": st.location_id,
                    "set_fingerprint": set_fp,
                    "dest_dir": st.to,
                });
                actions.push(ProposedActionInput {
                    agent: Some("context.plan".into()),
                    action_type: ActionType::Move,
                    target_kind: TargetKind::File,
                    file_id: Some(m.file_id.clone()),
                    atomic_unit_id: None,
                    from_path: m.path.clone(),
                    to_path: Some(dest),
                    reason: st.reason.clone(),
                    confidence: c.confidence,
                    evidence_json: ev.to_string(),
                });
            }
        }
        if actions.is_empty() {
            return Err(AegisError::NotFound("The files in this set have moved. Cairn will look again.".into()));
        }
        let id = crate::db::actions::propose_batch(store, &RefilePlan { origin, actions })?;
        super::store::record_plan_run(store.conn(), &id, &plan.volume_id, &[set_fp], "proposed")?;
        return Ok(id);
    }
    let mut actions = Vec::new();
    for st in &plan.steps {
        let loc = c
            .locations
            .iter()
            .find(|l| l.id == st.location_id)
            .ok_or_else(|| AegisError::NotFound("A folder in this plan is no longer part of the project.".into()))?;
        let (files, newest) = folder_state(Path::new(&loc.root_path))?;
        let mut ev = serde_json::json!({
            "step": st.step,
            "exact_dest": true,
            "expect": {"files": files, "newest": newest},
            "project": c.name,
            "kind": st.kind,
            "collection_id": c.id,
            "location_id": st.location_id,
            "location": loc,
        });
        let kind = if st.kind == StepKind::Quarantine {
            ev["verify_against"] = serde_json::json!(plan.home_after);
            ev["fallback"] = serde_json::json!(st.fallback);
            ActionType::Quarantine
        } else {
            ActionType::Move
        };
        actions.push(ProposedActionInput {
            agent: Some("context.plan".into()),
            action_type: kind,
            target_kind: TargetKind::Dir,
            file_id: None,
            atomic_unit_id: None,
            from_path: st.from.clone(),
            to_path: if kind == ActionType::Quarantine { None } else { Some(st.to.clone()) },
            reason: st.reason.clone(),
            confidence: c.confidence,
            evidence_json: ev.to_string(),
        });
    }
    let id = crate::db::actions::propose_batch(store, &RefilePlan { origin, actions })?;
    let fps: Vec<String> = c.locations.iter().filter(|l| l.volume_id == plan.volume_id).map(|l| l.fingerprint.clone()).collect();
    super::store::record_plan_run(store.conn(), &id, &plan.volume_id, &fps, "proposed")?;
    Ok(id)
}

/// A failure message as the owner should read it.
pub(crate) fn plain_reason(msg: &str, folder: &str) -> String {
    let lower = msg.to_lowercase();
    if lower.contains("os error 5") || lower.contains("os error 32") || lower.contains("access is denied") || lower.contains("used by another process") {
        return format!("A program is using files in {folder}. Close it and try again.");
    }
    if lower.contains("os error 3") || lower.contains("os error 21") || lower.contains("device is not ready") {
        return format!("The drive holding {folder} went away.");
    }
    // Our own messages come as "code: message"; keep the message.
    match msg.split_once(": ") {
        Some((code, rest)) if !code.contains(' ') => rest.to_string(),
        _ => msg.to_string(),
    }
}

/// Runs a proposed project batch as one unit: every step, or none. If any step fails, the
/// steps already done are undone at once and the outcome says why.
pub fn run(store: &mut SqliteStore, batch_id: &str) -> Result<PlanOutcome> {
    use crate::db::actions::{approve_batch, execute_batch, rollback_batch};
    approve_batch(store, batch_id, None)?;
    execute_batch(store, batch_id)?;
    let first_bad: Option<(String, String)> = store
        .conn()
        .query_row(
            "SELECT from_path, COALESCE(error_message, status) FROM actions
             WHERE batch_id = ?1 AND status NOT IN ('executed', 'superseded')
             ORDER BY step, created_at, id LIMIT 1",
            [batch_id],
            |r| Ok((r.get(0)?, r.get(1)?)),
        )
        .optional()?;
    if let Some((folder, msg)) = first_bad {
        let step_why = plain_reason(&msg, &folder);
        let (state, reason) = match rollback_batch(store, batch_id) {
            Ok(rb) if rb.status == crate::contracts::BatchStatus::RolledBack && rb.conflicts.is_empty() => {
                ("failed", format!("{step_why} Nothing was changed."))
            }
            Ok(rb) => (
                "partial",
                format!(
                    "{step_why} Automatic undo hit {} conflict(s) and left some steps in place. Open Recent batches to finish restoring.",
                    rb.conflicts.len()
                ),
            ),
            Err(re) => (
                "partial",
                format!(
                    "{step_why} Automatic undo could not finish ({}). Open Recent batches to finish restoring.",
                    plain_reason(&re.to_string(), &folder)
                ),
            ),
        };
        let _ = after_undo(store, batch_id);
        set_run_state(store, batch_id, state)?;
        return Ok(PlanOutcome {
            batch_id: batch_id.to_string(),
            done: false,
            reason: Some(reason),
            moved_files: 0,
            freed_bytes: 0,
        });
    }
    set_run_state(store, batch_id, "done")?;
    follow_moves(store, batch_id)?;
    let moved_files: i64 = store.conn().query_row("SELECT COUNT(*) FROM file_history WHERE batch_id = ?1", [batch_id], |r| r.get(0))?;
    let freed_bytes: i64 = store.conn().query_row(
        "SELECT COALESCE(SUM(q.size_bytes), 0) FROM quarantine_entries q JOIN actions a ON a.id = q.action_id WHERE a.batch_id = ?1",
        [batch_id],
        |r| r.get(0),
    )?;
    Ok(PlanOutcome { batch_id: batch_id.to_string(), done: true, reason: None, moved_files, freed_bytes })
}

/// Undoes a project plan that ran: every folder back where it was, quarantined copies restored.
pub fn undo(store: &mut SqliteStore, batch_id: &str) -> Result<crate::contracts::RollbackResult> {
    let r = crate::db::actions::rollback_batch(store, batch_id)?;
    after_undo(store, batch_id)?;
    unfollow_moves(store, batch_id)?;
    let clean = r.status == crate::contracts::BatchStatus::RolledBack && r.conflicts.is_empty();
    set_run_state(store, batch_id, if clean { "undone" } else { "partial" })?;
    Ok(r)
}

/// Restores `collection_locations` (or `collection_members` for design sets) immediately after a
/// batch (or part of one) is undone, so the Projects card reflects restored paths without waiting
/// for the next background recognition pass.
fn unfollow_moves(store: &mut SqliteStore, batch_id: &str) -> Result<()> {
    let rows: Vec<(String, Option<String>, String)> = {
        let mut stmt = store.conn().prepare(
            "SELECT evidence_json, file_id, from_path FROM actions
             WHERE batch_id = ?1 AND status = 'rolled_back' ORDER BY step DESC",
        )?;
        stmt.query_map([batch_id], |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?)))?
            .collect::<rusqlite::Result<_>>()?
    };
    let mut dir_locs: Vec<(String, Location)> = Vec::new();
    let mut restored_members: Vec<(String, String, String)> = Vec::new();
    let mut restored_cids: HashSet<String> = HashSet::new();
    let mut dest_dirs: HashSet<String> = HashSet::new();
    for (ev, file_id, from_path) in rows {
        let Ok(v) = serde_json::from_str::<serde_json::Value>(&ev) else { continue };
        let Some(kind) = v.get("kind").and_then(|x| x.as_str()).map(str::to_string) else { continue };
        if kind == "gather_files" {
            if let Some(d) = v.get("dest_dir").and_then(|x| x.as_str()) {
                dest_dirs.insert(d.to_string());
            }
            if let (Some(cid), Some(fid)) = (v.get("collection_id").and_then(|x| x.as_str()), file_id) {
                restored_cids.insert(cid.to_string());
                restored_members.push((cid.to_string(), fid, from_path));
            }
            continue;
        }
        if let Some(loc) = v.get("location").and_then(|x| serde_json::from_value::<Location>(x.clone()).ok()) {
            dir_locs.push((kind, loc));
        }
    }
    let tx = store.conn().unchecked_transaction()?;
    let mut seen: HashSet<String> = HashSet::new();
    // Reverse `move_home` / `to_archive` first so the original destination path is freed before
    // reinserting any copy that had occupied it.
    for (kind, loc) in &dir_locs {
        if (kind != "move_home" && kind != "to_archive") || !seen.insert(loc.id.clone()) {
            continue;
        }
        let n = tx.execute(
            "UPDATE collection_locations SET root_path = ?1 WHERE id = ?2",
            params![loc.root_path, loc.id],
        )?;
        if n == 0 {
            tx.execute(
                "UPDATE collection_locations SET root_path = ?1 WHERE volume_id = ?2 AND fingerprint = ?3",
                params![loc.root_path, loc.volume_id, loc.fingerprint],
            )?;
        }
    }
    for (kind, loc) in &dir_locs {
        if kind == "move_home" || kind == "to_archive" || !seen.insert(loc.id.clone()) {
            continue;
        }
        let exists: bool = tx.query_row(
            "SELECT EXISTS(SELECT 1 FROM collections WHERE id = ?1)",
            [&loc.collection_id],
            |r| r.get(0),
        )?;
        if exists {
            tx.execute(
                "INSERT INTO collection_locations (id, collection_id, volume_id, root_path, relationship, files, bytes,
                                                   newest_mtime, shared_files, compared_by, is_repo, fingerprint, keep_here)
                 VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12, ?13)
                 ON CONFLICT(volume_id, root_path) DO NOTHING",
                params![
                    loc.id,
                    loc.collection_id,
                    loc.volume_id,
                    loc.root_path,
                    loc.relationship.as_str(),
                    loc.files,
                    loc.bytes,
                    loc.newest_mtime,
                    loc.shared_files,
                    loc.compared_by,
                    i32::from(loc.is_repo),
                    loc.fingerprint,
                    i32::from(loc.keep_here),
                ],
            )?;
        }
    }
    for d in dest_dirs {
        if !Path::new(&d).exists() {
            let cids: Vec<String> = {
                let mut stmt =
                    tx.prepare("SELECT DISTINCT collection_id FROM collection_locations WHERE lower(root_path) = lower(?1)")?;
                stmt.query_map([&d], |r| r.get(0))?.collect::<rusqlite::Result<_>>()?
            };
            tx.execute("DELETE FROM collection_locations WHERE lower(root_path) = lower(?1)", [&d])?;
            for cid in cids {
                let left: i64 =
                    tx.query_row("SELECT COUNT(*) FROM collection_locations WHERE collection_id = ?1", [&cid], |r| r.get(0))?;
                if left == 0 {
                    if restored_cids.contains(&cid) {
                        tx.execute("UPDATE collections SET kind = 'design_set' WHERE id = ?1", [&cid])?;
                    } else {
                        tx.execute("DELETE FROM collections WHERE id = ?1 AND kind = 'project'", [&cid])?;
                    }
                }
            }
        }
    }
    for (cid, fid, from_path) in restored_members {
        let exists: bool = tx.query_row("SELECT EXISTS(SELECT 1 FROM collections WHERE id = ?1)", [&cid], |r| r.get(0))?;
        if !exists {
            continue;
        }
        if let Some((vol, size, mtime)) = tx
            .query_row("SELECT volume_id, size, mtime FROM files WHERE id = ?1", [&fid], |r| {
                Ok((r.get::<_, String>(0)?, r.get::<_, i64>(1)?, r.get::<_, i64>(2)?))
            })
            .optional()?
        {
            tx.execute(
                "INSERT INTO collection_members (collection_id, file_id, volume_id, path, size, mtime)
                 VALUES (?1, ?2, ?3, ?4, ?5, ?6)
                 ON CONFLICT(collection_id, file_id) DO UPDATE SET path = excluded.path",
                params![cid, fid, vol, from_path, size, mtime],
            )?;
        }
    }
    tx.commit()?;
    Ok(())
}

/// Forgets the "was here" records of files that really are back where they were, and removes any
/// newly-created destination folder left empty after undoing a design set plan.
fn after_undo(store: &mut SqliteStore, batch_id: &str) -> Result<()> {
    store.conn().execute(
        "DELETE FROM file_history WHERE batch_id = ?1 AND EXISTS (
            SELECT 1 FROM files f WHERE f.id = file_history.file_id AND lower(f.path) = lower(file_history.old_path))",
        [batch_id],
    )?;
    let mut archive_parents: HashSet<String> = HashSet::new();
    let dest_dirs: HashSet<String> = {
        let mut stmt = store.conn().prepare(
            "SELECT evidence_json, to_path FROM actions WHERE batch_id = ?1 AND status = 'rolled_back'",
        )?;
        let rows: Vec<(String, Option<String>)> = stmt
            .query_map([batch_id], |r| Ok((r.get(0)?, r.get(1)?)))?
            .collect::<rusqlite::Result<_>>()?;
        rows.into_iter()
            .filter_map(|(ev, to_path)| {
                let v: serde_json::Value = serde_json::from_str(&ev).ok()?;
                let kind = v.get("kind")?.as_str()?;
                if kind == "gather_files" {
                    v.get("dest_dir")?.as_str().map(str::to_string)
                } else if kind == "move_home" {
                    to_path.filter(|p| leaf(parent(p)).eq_ignore_ascii_case("design"))
                } else if kind == "to_archive" {
                    if let Some(p) = to_path {
                        archive_parents.insert(parent(&p).to_string());
                    }
                    None
                } else {
                    None
                }
            })
            .collect()
    };
    if !dest_dirs.is_empty() || !archive_parents.is_empty() {
        let mut roots: Vec<String> = store.list_roots()?.into_iter().map(|r| r.path).collect();
        let mut vstmt = store.conn().prepare("SELECT mount_point FROM volumes WHERE status = 'online'")?;
        roots.extend(vstmt.query_map([], |r| r.get::<_, String>(0))?.filter_map(|m| m.ok()));
        let strictly_inside = |p: &str| roots.iter().any(|r| !same_path(p, r) && path_is_within(p, r, false));
        for d in dest_dirs {
            if strictly_inside(&d) && Path::new(&d).is_dir() {
                let _ = std::fs::remove_dir(&d);
            }
            let shelf = parent(&d);
            if leaf(shelf).eq_ignore_ascii_case("design") && strictly_inside(shelf) && Path::new(shelf).is_dir() {
                let _ = std::fs::remove_dir(shelf);
            }
        }
        for start in archive_parents {
            let mut cur = start;
            while strictly_inside(&cur) && Path::new(&cur).is_dir() {
                if std::fs::remove_dir(&cur).is_err() {
                    break;
                }
                if leaf(&cur).eq_ignore_ascii_case("archive") {
                    break;
                }
                let next = parent(&cur);
                if same_path(next, &cur) {
                    break;
                }
                cur = next.to_string();
            }
        }
    }
    Ok(())
}

struct ExecutedStep {
    collection_id: String,
    location_id: String,
    kind: String,
    to: String,
    file_id: Option<String>,
    set_fingerprint: Option<String>,
    project: Option<String>,
    dest_dir: Option<String>,
}

/// Metadata of each executed step of a batch.
fn executed_steps(store: &SqliteStore, batch_id: &str) -> Result<Vec<ExecutedStep>> {
    let mut stmt = store.conn().prepare(
        "SELECT evidence_json, COALESCE(to_path, ''), file_id FROM actions
         WHERE batch_id = ?1 AND status = 'executed' ORDER BY step",
    )?;
    let rows: Vec<(String, String, Option<String>)> =
        stmt.query_map([batch_id], |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?)))?.collect::<rusqlite::Result<_>>()?;
    Ok(rows
        .into_iter()
        .filter_map(|(ev, to, file_id)| {
            let v: serde_json::Value = serde_json::from_str(&ev).ok()?;
            let s = |k: &str| v.get(k).and_then(|x| x.as_str()).map(str::to_string);
            Some(ExecutedStep {
                collection_id: s("collection_id")?,
                location_id: s("location_id")?,
                kind: s("kind")?,
                to,
                file_id,
                set_fingerprint: s("set_fingerprint"),
                project: s("project"),
                dest_dir: s("dest_dir"),
            })
        })
        .collect())
}

#[derive(Default)]
struct GatheredDest {
    project_name: String,
    set_fingerprint: String,
    volume_id: String,
    files: i64,
    bytes: i64,
    newest_mtime: i64,
}

/// The saved project follows the plan at once, before the next recognition pass: the home's
/// location moves with it, copies that went to quarantine/versions/material stop being separate
/// locations, and gathered design-set members leave `collection_members`, register the destination
/// folder as a protected design project, and learn their subject so new arrivals stay recognised.
fn follow_moves(store: &mut SqliteStore, batch_id: &str) -> Result<()> {
    let steps = executed_steps(store, batch_id)?;
    let run_volume_id: Option<String> = store
        .conn()
        .query_row("SELECT volume_id FROM project_plan_runs WHERE batch_id = ?1", [batch_id], |r| r.get(0))
        .optional()?;
    let mut learned: HashSet<(String, String)> = HashSet::new();
    let mut gathered: BTreeMap<(String, String), GatheredDest> = BTreeMap::new();
    let tx = store.conn().unchecked_transaction()?;
    for st in &steps {
        if st.kind == "gather_files" {
            if let Some(fid) = &st.file_id {
                let file_meta: Option<(String, i64, i64)> = tx
                    .query_row("SELECT volume_id, size, mtime FROM files WHERE id = ?1", [fid], |r| {
                        Ok((r.get(0)?, r.get(1)?, r.get(2)?))
                    })
                    .optional()?;
                tx.execute(
                    "DELETE FROM collection_members WHERE collection_id = ?1 AND file_id = ?2",
                    params![st.collection_id, fid],
                )?;
                if let Some(dest) = &st.dest_dir {
                    let e = gathered.entry((st.collection_id.clone(), dest.clone())).or_default();
                    if e.project_name.is_empty() {
                        e.project_name = st.project.clone().unwrap_or_else(|| leaf(dest).to_string());
                        e.set_fingerprint = st
                            .set_fingerprint
                            .clone()
                            .unwrap_or_else(|| super::sets::set_fingerprint(&e.project_name));
                    }
                    if let Some((vol, size, mtime)) = file_meta {
                        if e.volume_id.is_empty() {
                            e.volume_id = vol;
                        }
                        e.files += 1;
                        e.bytes += size;
                        e.newest_mtime = e.newest_mtime.max(mtime);
                    } else {
                        if e.volume_id.is_empty()
                            && let Some(rv) = &run_volume_id
                        {
                            e.volume_id = rv.clone();
                        }
                        e.files += 1;
                    }
                }
            }
            if let (Some(fp), Some(proj)) = (&st.set_fingerprint, &st.project) {
                let is_design = tx
                    .query_row(
                        "SELECT COALESCE(NULLIF(project_kind, ''), 'design') = 'design' FROM collections WHERE id = ?1",
                        [&st.collection_id],
                        |r| r.get::<_, bool>(0),
                    )
                    .unwrap_or(true);
                if is_design {
                    learned.insert((fp.clone(), proj.clone()));
                }
            }
        } else if st.kind != "move_home" && st.kind != "to_archive" {
            tx.execute("DELETE FROM collection_locations WHERE id = ?1", [&st.location_id])?;
        }
    }
    let at = now_ms();
    for ((cid, dest_dir), g) in gathered {
        if g.volume_id.is_empty() {
            continue;
        }
        let already_protected: bool = tx.query_row(
            "SELECT EXISTS(SELECT 1 FROM collection_locations WHERE volume_id = ?1 AND lower(root_path) = lower(?2))",
            params![g.volume_id, dest_dir],
            |r| r.get(0),
        )?;
        if already_protected {
            continue;
        }
        let remaining: i64 =
            tx.query_row("SELECT COUNT(*) FROM collection_members WHERE collection_id = ?1", [&cid], |r| r.get(0))?;
        let eff_kind: String = tx
            .query_row("SELECT COALESCE(NULLIF(project_kind, ''), 'design') FROM collections WHERE id = ?1", [&cid], |r| r.get(0))
            .unwrap_or_else(|_| "design".to_string());
        let target_cid = if remaining == 0 {
            tx.execute(
                "UPDATE collections SET kind = 'project', project_kind = ?1, state = 'confirmed', confidence = MAX(confidence, 0.90)
                 WHERE id = ?2",
                params![eff_kind, cid],
            )?;
            cid
        } else {
            let new_cid = new_id();
            tx.execute(
                "INSERT INTO collections (id, kind, name, project_kind, confidence, state, evidence_json, sub_projects_json, computed_at, plan_state, fingerprint)
                 VALUES (?1, 'project', ?2, ?3, 0.90, 'confirmed', '[\"confirmed project\"]', '[]', ?4, NULL, NULL)",
                params![new_cid, g.project_name, eff_kind, at],
            )?;
            new_cid
        };
        tx.execute(
            "INSERT INTO collection_locations (id, collection_id, volume_id, root_path, relationship, files, bytes,
                                               newest_mtime, shared_files, compared_by, is_repo, fingerprint, keep_here)
             VALUES (?1, ?2, ?3, ?4, 'home', ?5, ?6, ?7, ?5, 'name', 0, ?8, 0)
             ON CONFLICT(volume_id, root_path) DO NOTHING",
            params![new_id(), target_cid, g.volume_id, dest_dir, g.files, g.bytes, g.newest_mtime, g.set_fingerprint],
        )?;
    }
    for st in steps.iter().filter(|s| s.kind == "move_home" || s.kind == "to_archive") {
        tx.execute("UPDATE collection_locations SET root_path = ?1 WHERE id = ?2", params![st.to, st.location_id])?;
    }
    for (fp, proj) in learned {
        tx.execute(
            "INSERT INTO context_decisions (id, kind, fingerprint_a, fingerprint_b, value, at)
             VALUES (?1, ?2, ?3, NULL, ?4, ?5)",
            params![new_id(), DecisionKind::LearnedSubject.as_str(), fp, format!("design:{proj}"), at],
        )?;
    }
    tx.commit()?;
    Ok(())
}

fn set_run_state(store: &SqliteStore, batch_id: &str, state: &str) -> Result<()> {
    store.conn().execute("UPDATE project_plan_runs SET state = ?1, at = ?2 WHERE batch_id = ?3", params![state, crate::now_ms(), batch_id])?;
    // The saved project shows it at once (recognition recomputes it from the runs later).
    let ids: Vec<String> = {
        let mut stmt = store.conn().prepare("SELECT evidence_json FROM actions WHERE batch_id = ?1")?;
        let evs: Vec<String> = stmt.query_map([batch_id], |r| r.get(0))?.collect::<rusqlite::Result<_>>()?;
        let mut ids: Vec<String> = evs
            .iter()
            .filter_map(|e| serde_json::from_str::<serde_json::Value>(e).ok()?.get("collection_id")?.as_str().map(str::to_string))
            .collect();
        ids.dedup();
        ids
    };
    for id in ids {
        store.conn().execute("UPDATE collections SET plan_state = ?1 WHERE id = ?2", params![format!("{state}:{batch_id}"), id])?;
    }
    Ok(())
}

fn summarise(steps: &[PlanStep], home_after: &str) -> String {
    if steps.is_empty() {
        return "Nothing to do: it is already where it belongs.".into();
    }
    let n = |k: StepKind| steps.iter().filter(|s| s.kind == k).count();
    let mut parts: Vec<String> = Vec::new();
    if n(StepKind::MoveHome) > 0 {
        parts.push(format!("moves to {home_after}"));
    }
    if n(StepKind::ToArchive) > 0 {
        parts.push(format!("moves to archive at {home_after}"));
    }
    let plural = |n: usize, one: &str, many: &str| if n == 1 { format!("1 {one}") } else { format!("{n} {many}") };
    if n(StepKind::Quarantine) > 0 {
        parts.push(format!("{} to quarantine", plural(n(StepKind::Quarantine), "copy", "copies")));
    }
    if n(StepKind::ToVersions) > 0 {
        parts.push(format!("{} kept as versions", plural(n(StepKind::ToVersions), "copy", "copies")));
    }
    if n(StepKind::ToMaterial) > 0 {
        parts.push(format!("{} brought in", plural(n(StepKind::ToMaterial), "folder of material", "folders of material")));
    }
    if n(StepKind::GatherFiles) > 0 {
        parts.push(format!("brings matching files into {home_after}"));
    }
    let mut s = parts.join(", ");
    if let Some(first) = s.get_mut(0..1) {
        first.make_ascii_uppercase();
    }
    s.push('.');
    s
}
