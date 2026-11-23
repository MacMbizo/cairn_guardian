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
use super::relate::choose_home;
use super::{Collection, Location, ProjectKind, Relationship};
use crate::db::SqliteStore;
use crate::db::files::path_is_within;
use crate::error::{AegisError, Result};
use rusqlite::{OptionalExtension, params};
use serde::{Deserialize, Serialize};
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

fn join(base: &str, leaf: &str) -> String {
    format!("{}{SEP}{leaf}", base.trim_end_matches(SEP))
}

fn parent(p: &str) -> &str {
    p.trim_end_matches(SEP).rfind(SEP).map_or(p, |i| &p[..i])
}

fn leaf(p: &str) -> &str {
    let p = p.trim_end_matches(SEP);
    p.rfind(SEP).map_or(p, |i| &p[i + 1..])
}

fn same_path(a: &str, b: &str) -> bool {
    a.trim_end_matches(SEP).eq_ignore_ascii_case(b.trim_end_matches(SEP))
}

/// Strictly inside `dir` (not `dir` itself).
fn inside(p: &str, dir: &str) -> bool {
    !same_path(p, dir) && path_is_within(p, dir, false)
}

/// A folder name Windows accepts: reserved characters become `_`, trailing dots and spaces go.
fn safe_name(name: &str) -> String {
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

fn volume_info(store: &SqliteStore, volume_id: &str) -> Result<Option<(String, String, bool)>> {
    Ok(store
        .conn()
        .query_row(
            "SELECT mount_point, COALESCE(NULLIF(label, ''), mount_point), status = 'online' FROM volumes WHERE id = ?1",
            [volume_id],
            |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?)),
        )
        .optional()?)
}

/// The drive's projects home: the owner's setting; else, among the PC's roots, the one that
/// directly holds the most recognised projects (general places such as Documents never count);
/// else the drive's `Projects` folder if it exists; else none (projects stay where they are).
pub fn projects_home(store: &SqliteStore, volume_id: &str) -> Result<Option<String>> {
    if let Some(p) = store.get_setting(&format!("{PROJECTS_HOME_KEY}:{volume_id}"), "context")? {
        return Ok(Some(p));
    }
    let roots: Vec<String> = store
        .list_roots()?
        .into_iter()
        .filter(|r| r.enabled && !GENERAL_FOLDERS.contains(&leaf(&r.path).to_lowercase().as_str()))
        .map(|r| r.path)
        .collect();
    let mut stmt = store.conn().prepare("SELECT root_path FROM collection_locations WHERE volume_id = ?1")?;
    let locs: Vec<String> = stmt.query_map([volume_id], |r| r.get(0))?.collect::<rusqlite::Result<_>>()?;
    let best = roots
        .iter()
        .map(|r| (locs.iter().filter(|l| same_path(parent(l), r)).count(), r))
        .filter(|(n, _)| *n > 0)
        .max_by_key(|(n, _)| *n)
        .map(|(_, r)| r.clone());
    if best.is_some() {
        return Ok(best);
    }
    if let Some((mount, _, online)) = volume_info(store, volume_id)? {
        let p = join(&mount, "Projects");
        if online && Path::new(&p).is_dir() {
            return Ok(Some(p));
        }
    }
    Ok(None)
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
/// location, and every Python virtual environment (a folder holding `pyvenv.cfg`). Planning
/// many projects with one `Planner` costs one pass over the catalog, not one per project.
pub struct Planner {
    roots: Vec<crate::contracts::RootConfig>,
    /// (collection id, volume id, root path) of every saved location.
    locations: Vec<(String, String, String)>,
    /// (volume id, folder) of every virtual environment.
    venvs: Vec<(String, String)>,
}

impl Planner {
    pub fn new(store: &SqliteStore) -> Result<Self> {
        let locations = {
            let mut stmt = store.conn().prepare("SELECT collection_id, volume_id, root_path FROM collection_locations")?;
            stmt.query_map([], |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?)))?.collect::<rusqlite::Result<_>>()?
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
        Ok(Self { roots: store.list_roots()?, locations, venvs })
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

    /// The plans for one project: one per drive it lives on. None for projects the owner left
    /// as they are or said aren't projects.
    pub fn plan_for(&self, store: &SqliteStore, c: &Collection) -> Result<Vec<ProjectPlan>> {
        if matches!(c.state.as_str(), "left_as_is" | "not_a_project") || c.locations.is_empty() {
            return Ok(Vec::new());
        }
        let mut volumes: Vec<&str> = Vec::new();
        for l in &c.locations {
            if !volumes.contains(&l.volume_id.as_str()) {
                volumes.push(&l.volume_id);
            }
        }
        volumes.iter().map(|v| plan_on(self, store, c, v)).collect()
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
    };
    // Rulings from the dry run on the owner's database (2026-11-23):
    // - Other drives get no plan yet: they hold old copies and coursework, which the later
    //   Old copies module is for. Their copies are listed on the card.
    let roots = &ctx.roots;
    let on_pc = here.iter().any(|l| roots.iter().any(|r| r.enabled && path_is_within(&l.root_path, &r.path, false)));
    if !on_pc {
        return Ok(nothing("Copies on this drive are listed. Plans for other drives come later."));
    }
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
    let is_root = |d: &str| roots.iter().any(|r| same_path(&r.path, d));
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
    let ph = if in_projects_folder { projects_home(store, volume_id)? } else { None };

    // Where the home ends up: under its own folder name. A name the owner gives the project on
    // the card is a label; renaming the folder could break paths inside it.
    let name = safe_name(leaf(&home.root_path));
    let mut steps: Vec<PlanStep> = Vec::new();
    let mut aside: Option<(&Location, String)> = None;
    let mut home_after = home.root_path.clone();
    if let Some(ph) = ph.as_deref().filter(|ph| !path_is_within(&home.root_path, ph, false)) {
        let mut target = join(ph, &name);
        let occupant = here.iter().find(|l| l.id != home.id && same_path(&l.root_path, &target)).copied();
        let taken_by_other = occupant.is_none() && Path::new(&target).exists();
        if taken_by_other || occupant.is_some_and(|o| o.keep_here) {
            let year = &date_of(home.newest_mtime)[..4];
            target = join(ph, &format!("{name} ({} {year})", kind_word(c.project_kind)));
        } else if let Some(o) = occupant {
            let moved = join(ph, &format!("{name} (old copy)"));
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
        if Path::new(&target).exists() && !aside.as_ref().is_some_and(|(o, _)| same_path(&o.root_path, &target)) {
            // Even the disambiguated name is taken: leave the home where it is.
            target = home.root_path.clone();
        }
        if !same_path(&target, &home.root_path) {
            steps.push(PlanStep {
                step: 2,
                kind: StepKind::MoveHome,
                location_id: home.id.clone(),
                from: home.root_path.clone(),
                to: target.clone(),
                fallback: None,
                reason: format!("Moves whole to {target} (your projects home)."),
                warnings: ctx.repo_warnings(home),
            });
            home_after = target;
        }
    }

    // Where material and versions from the other copies go.
    let base = if home.is_repo { join(parent(&home_after), &format!("{} - material", leaf(&home_after))) } else { home_after.clone() };
    let versions = join(&base, "Versions");
    let origin = |l: &Location| leaf(parent(&l.root_path)).to_string();
    let mut taken: std::collections::HashSet<String> = std::collections::HashSet::new();
    let mut unique = |p: String| -> String {
        let mut out = p.clone();
        let mut n = 2;
        while taken.contains(&out.to_lowercase()) || Path::new(&out).exists() {
            out = format!("{p} {n}");
            n += 1;
        }
        taken.insert(out.to_lowercase());
        out
    };
    if main.is_some() {
        for l in here.iter().filter(|l| l.id != home.id && !l.keep_here && !blocked.contains(&l.id)) {
            // A copy inside the home (or holding it) is not moved on its own.
            if inside(&l.root_path, &home.root_path) || inside(&home.root_path, &l.root_path) {
                continue;
            }
            let (from, step) = match &aside {
                Some((o, moved)) if o.id == l.id => (moved.clone(), 3),
                _ => (l.root_path.clone(), 4),
            };
            let dated = |label: &str| join(&versions, &format!("{} {label}", date_of(l.newest_mtime)));
            let (kind, to, fallback, reason) = match l.relationship {
                Relationship::Identical | Relationship::OlderSubset if !l.is_repo => {
                    let why = if l.relationship == Relationship::Identical {
                        "Same as the main copy: to quarantine for 30 days."
                    } else {
                        "Older copy: everything here is in the main copy. To quarantine for 30 days."
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
    let mut plan = ProjectPlan { collection_id: c.id.clone(), volume_id: volume_id.to_string(), home_after, steps, waits_for, notes, summary, rank: 9 };
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
/// home, 3 anything that keeps versions or brings material in.
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

/// Proposes `plan` (for collection `c`) as one batch of folder steps, replacing any plan for
/// the same project that is still waiting. Each step records the folder as it is now, so it
/// stops if the folder changes before it runs.
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
fn plain_reason(msg: &str, folder: &str) -> String {
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
        rollback_batch(store, batch_id)?;
        after_undo(store, batch_id)?;
        set_run_state(store, batch_id, "failed")?;
        return Ok(PlanOutcome {
            batch_id: batch_id.to_string(),
            done: false,
            reason: Some(format!("{} Nothing was changed.", plain_reason(&msg, &folder))),
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
    set_run_state(store, batch_id, "undone")?;
    Ok(r)
}

/// Forgets the "was here" records of files that really are back where they were.
fn after_undo(store: &mut SqliteStore, batch_id: &str) -> Result<()> {
    store.conn().execute(
        "DELETE FROM file_history WHERE batch_id = ?1 AND EXISTS (
            SELECT 1 FROM files f WHERE f.id = file_history.file_id AND lower(f.path) = lower(file_history.old_path))",
        [batch_id],
    )?;
    Ok(())
}

/// The (collection id, location id, step kind, destination) of each executed step of a batch.
fn executed_steps(store: &SqliteStore, batch_id: &str) -> Result<Vec<(String, String, String, String)>> {
    let mut stmt = store
        .conn()
        .prepare("SELECT evidence_json, COALESCE(to_path, '') FROM actions WHERE batch_id = ?1 AND status = 'executed' ORDER BY step")?;
    let rows: Vec<(String, String)> = stmt.query_map([batch_id], |r| Ok((r.get(0)?, r.get(1)?)))?.collect::<rusqlite::Result<_>>()?;
    Ok(rows
        .into_iter()
        .filter_map(|(ev, to)| {
            let v: serde_json::Value = serde_json::from_str(&ev).ok()?;
            let s = |k: &str| v.get(k).and_then(|x| x.as_str()).map(str::to_string);
            Some((s("collection_id")?, s("location_id")?, s("kind")?, to))
        })
        .collect())
}

/// The saved project follows the plan at once, before the next recognition pass: the home's
/// location moves with it, and copies that went to quarantine, versions or material stop being
/// separate locations (they are inside the home now, or gone). The next pass confirms it: the
/// moves changed the catalog, so the context watch re-recognises.
fn follow_moves(store: &mut SqliteStore, batch_id: &str) -> Result<()> {
    let steps = executed_steps(store, batch_id)?;
    let tx = store.conn().unchecked_transaction()?;
    // A copy moved aside and then handled has two steps; deleting it twice is harmless.
    for (_, loc, _, _) in steps.iter().filter(|s| s.2 != "move_home") {
        tx.execute("DELETE FROM collection_locations WHERE id = ?1", [loc])?;
    }
    for (_, loc, _, to) in steps.iter().filter(|s| s.2 == "move_home") {
        tx.execute("UPDATE collection_locations SET root_path = ?1 WHERE id = ?2", params![to, loc])?;
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
    let mut s = parts.join(", ");
    if let Some(first) = s.get_mut(0..1) {
        first.make_ascii_uppercase();
    }
    s.push('.');
    s
}
