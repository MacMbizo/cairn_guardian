//! Blueprints (spec §5, §8): where every file on a drive (or in the PC's folders) would go under
//! the owner's own folders tidied up, or under the fresh structure. A blueprint is only a plan:
//! batches are cut from it later.

use super::classify::{Classification, DateSource};
use super::naming::{proposed_name, sanitize};
use super::{BlueprintChoice, FileMeta, Kind, Scope};
use crate::db::SqliteStore;
use crate::db::files::path_is_within;
use crate::error::{AegisError, Result};
use serde::{Deserialize, Serialize};
use std::collections::{BTreeMap, HashMap, HashSet};
use std::path::{Path, PathBuf};
use ts_rs::TS;

/// Below this, Aegis asks instead of proposing a move.
pub const PROPOSE_AT: f64 = 0.60;

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, TS)]
#[serde(tag = "type", rename_all = "snake_case")]
#[ts(export)]
pub enum Target {
    File { file_id: String },
    /// A folder that moves (or stays) as one: a code project, a course, a named photo event,
    /// or a top folder being renamed.
    Group { path: String, files: usize },
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, TS)]
#[ts(export)]
pub struct Placement {
    pub target: Target,
    pub from: String,
    /// Full destination path; `None` means it stays where it is.
    pub to: Option<String>,
    pub new_name: Option<String>,
    pub confidence: f64,
    pub reason: String,
    pub rule_key: Option<String>,
    /// Aegis isn't sure: the owner picks a place.
    pub to_sort: bool,
    #[ts(type = "number")]
    pub bytes: i64,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, TS)]
#[ts(export)]
pub struct TreeNode {
    pub name: String,
    #[ts(type = "number")]
    pub files: i64,
    #[ts(type = "number")]
    pub bytes: i64,
    pub children: Vec<TreeNode>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, TS)]
#[ts(export)]
pub struct BlueprintSummary {
    pub choice: BlueprintChoice,
    pub tree: Vec<TreeNode>,
    #[ts(type = "number")]
    pub moves: i64,
    #[ts(type = "number")]
    pub renames: i64,
    #[ts(type = "number")]
    pub to_sort: i64,
    #[ts(type = "number")]
    pub stays: i64,
    /// Up to six `(before, after)` examples.
    pub examples: Vec<(String, String)>,
}

/// Windows' own folders on the PC.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum KnownFolder {
    Documents,
    Pictures,
    Music,
    Videos,
    Downloads,
    Desktop,
}

fn known_folder(path: &str) -> Option<KnownFolder> {
    let last = Path::new(path).file_name()?.to_string_lossy().to_lowercase();
    Some(match last.as_str() {
        "documents" | "my documents" => KnownFolder::Documents,
        "pictures" | "my pictures" => KnownFolder::Pictures,
        "music" | "my music" => KnownFolder::Music,
        "videos" | "my videos" => KnownFolder::Videos,
        "downloads" => KnownFolder::Downloads,
        "desktop" => KnownFolder::Desktop,
        _ => return None,
    })
}

/// The folders a scope covers: the PC's enabled roots (tagged by known folder) or a drive's
/// mount point. The Backup drive keeps its own layout.
pub fn scope_roots(store: &SqliteStore, scope: &Scope) -> Result<Vec<(String, Option<KnownFolder>)>> {
    match scope {
        Scope::Pc => Ok(store
            .list_roots()?
            .into_iter()
            .filter(|r| r.enabled)
            .map(|r| {
                let k = known_folder(&r.path);
                (r.path, k)
            })
            .collect()),
        Scope::Volume { volume_id } => {
            let (mount, role): (String, Option<String>) = store.conn().query_row(
                "SELECT mount_point, role FROM volumes WHERE id = ?1",
                [volume_id],
                |r| Ok((r.get(0)?, r.get(1)?)),
            )?;
            if role.as_deref() == Some("backup") {
                return Err(AegisError::InvalidInput("The Backup drive keeps its own layout".into()));
            }
            Ok(vec![(mount, None)])
        }
    }
}

/// Folders Aegis never files from or into.
fn protected(rel: &Path) -> bool {
    rel.components().any(|c| {
        let n = c.as_os_str().to_string_lossy().to_lowercase();
        n.starts_with(".aegis")
            || n == "$recycle.bin"
            || n == "system volume information"
            || n == "windows"
            || n == "appdata"
            || n == "aegis backup"
            || n.starts_with("program files")
    })
}

pub fn rule_key(m: &FileMeta) -> String {
    format!("{}|{}|{}", m.kind.as_str(), m.category.as_deref().unwrap_or("-"), m.who.as_deref().unwrap_or("-"))
}

struct Row {
    id: String,
    path: String,
    name: String,
    ext: String,
    size: i64,
    mtime: i64,
    meta: FileMeta,
}

fn load_rows(store: &SqliteStore, scope: &Scope, roots: &[(String, Option<KnownFolder>)]) -> Result<Vec<Row>> {
    let vol_filter = match scope {
        Scope::Volume { volume_id } => Some(volume_id.clone()),
        Scope::Pc => None,
    };
    let mut stmt = store.conn().prepare(
        "SELECT f.id, f.path, f.name, COALESCE(f.ext, ''), f.size, f.mtime, f.atomic_unit_id IS NOT NULL,
                m.kind, m.category, m.doc_type, m.who, m.doc_date, m.date_source, m.title, m.exif_taken_at,
                m.camera, m.artist, m.album, m.confidence, m.evidence_json
         FROM files f LEFT JOIN file_meta m ON m.file_id = f.id
         WHERE f.deleted_at IS NULL AND f.container_file_id IS NULL AND f.state NOT IN ('quarantine', 'missing')
           AND (?1 IS NULL OR f.volume_id = ?1)",
    )?;
    let rows = stmt.query_map([vol_filter], |r| {
        let id: String = r.get(0)?;
        let name: String = r.get(2)?;
        let ext: String = r.get(3)?;
        let in_unit: bool = r.get(6)?;
        let kind = match r.get::<_, Option<String>>(7)? {
            Some(k) => Kind::from_db(&k),
            None => super::classify::kind_of(&ext, &name, in_unit),
        };
        let evidence: Option<String> = r.get(19)?;
        let meta = FileMeta {
            file_id: id.clone(),
            kind: if in_unit { Kind::Code } else { kind },
            category: r.get(8)?,
            doc_type: r.get(9)?,
            who: r.get(10)?,
            doc_date: r.get(11)?,
            date_source: DateSource::from_db(r.get::<_, Option<String>>(12)?.as_deref()),
            title: r.get(13)?,
            exif_taken_at: r.get(14)?,
            camera: r.get(15)?,
            artist: r.get(16)?,
            album: r.get(17)?,
            confidence: r.get::<_, Option<f64>>(18)?.unwrap_or(0.0),
            evidence: evidence.and_then(|e| serde_json::from_str(&e).ok()).unwrap_or_default(),
            read_at: None,
            read_error: None,
        };
        Ok(Row { id, path: r.get(1)?, name, ext, size: r.get(4)?, mtime: r.get(5)?, meta })
    })?;
    let mut out = Vec::new();
    for r in rows {
        let r = r?;
        if let Some((root, _)) = roots.iter().find(|(root, _)| path_is_within(&r.path, root, false)) {
            let rel = Path::new(&r.path).strip_prefix(root).map(Path::to_path_buf).unwrap_or_default();
            if !protected(&rel) {
                out.push(r);
            }
        }
    }
    Ok(out)
}

fn classification_of(m: &FileMeta) -> Classification {
    Classification {
        kind: m.kind,
        category: m.category.clone(),
        doc_type: m.doc_type.clone(),
        who: m.who.clone(),
        doc_date: m.doc_date.clone(),
        date_source: m.date_source,
        confidence: m.confidence,
        type_confidence: m.confidence,
        evidence: m.evidence.clone(),
    }
}

fn year_of(m: &FileMeta) -> Option<String> {
    m.exif_taken_at
        .map(|t| crate::engine::note::date_of(t.div_euclid(86_400_000)))
        .or_else(|| m.doc_date.clone())
        .map(|d| d[..4].to_string())
}

fn month_of(m: &FileMeta) -> Option<String> {
    m.exif_taken_at
        .map(|t| crate::engine::note::date_of(t.div_euclid(86_400_000)))
        .or_else(|| m.doc_date.clone())
        .map(|d| d[..7].to_string())
}

/// Plain words for why a file goes where it goes.
fn reason_for(m: &FileMeta) -> String {
    let what = match (m.kind, m.doc_type.as_deref(), m.who.as_deref()) {
        (Kind::Document, Some(t), Some(w)) => format!("{} from {w}", capitalise(t)),
        (Kind::Document, Some(t), None) => capitalise(t),
        (Kind::Document, None, _) => "Document".into(),
        (Kind::Photo, ..) => "Photo".into(),
        (Kind::Screenshot, ..) => "Screenshot".into(),
        (Kind::Video, ..) => "Video".into(),
        (Kind::Music, ..) => "Song".into(),
        (Kind::Audio, ..) => "Audio".into(),
        (Kind::Installer, ..) => "Installer".into(),
        (Kind::Archive, ..) => "Archive".into(),
        (Kind::Font, ..) => "Font".into(),
        (Kind::Code, ..) => "Code".into(),
        (Kind::System | Kind::Other, ..) => "File".into(),
    };
    let ev: Vec<&str> = m.evidence.iter().take(2).map(String::as_str).collect();
    if ev.is_empty() { what } else { format!("{what} ({})", ev.join("; ")) }
}

fn capitalise(s: &str) -> String {
    let mut c = s.chars();
    c.next().map(|f| f.to_uppercase().collect::<String>() + c.as_str()).unwrap_or_default()
}

/// Where the fresh structure's top areas live for this scope.
struct Bases {
    volume_root: Option<PathBuf>,
    known: HashMap<KnownFolder, PathBuf>,
}

impl Bases {
    /// `area` is one of the fresh structure's top folders.
    fn of(&self, area: &str) -> Option<PathBuf> {
        if let Some(root) = &self.volume_root {
            return Some(root.join(area));
        }
        let k = |f: KnownFolder| self.known.get(&f).cloned();
        match area {
            "Documents" => k(KnownFolder::Documents),
            "Photos" => k(KnownFolder::Pictures),
            "Screenshots" => k(KnownFolder::Pictures).map(|p| p.join("Screenshots")),
            "Videos" => k(KnownFolder::Videos),
            "Music" => k(KnownFolder::Music),
            "Audio" => k(KnownFolder::Music).map(|p| p.join("Audio")),
            "Software" => k(KnownFolder::Downloads).map(|p| p.join("Software")),
            "Archives" => k(KnownFolder::Downloads).map(|p| p.join("Archives")),
            _ => None,
        }
    }
}

const FINANCE_BY_YEAR: [&str; 3] = ["Finance/Invoices & receipts", "Finance/Banking", "Finance/Payslips"];

/// `(area, sub-folders)` in the fresh structure, or `None` when the file stays put.
fn fresh_area(row: &Row) -> Option<(&'static str, Vec<String>)> {
    let m = &row.meta;
    let year = || year_of(m).unwrap_or_else(|| "Undated".into());
    match m.kind {
        Kind::Document => {
            let cat = m.category.as_deref()?;
            let mut sub: Vec<String> = cat.split('/').map(str::to_string).collect();
            if (cat == "Work" || cat == "Study")
                && let Some(w) = &m.who
            {
                sub.push(sanitize(w));
            }
            if FINANCE_BY_YEAR.contains(&cat) {
                sub.push(year());
            }
            Some(("Documents", sub))
        }
        Kind::Photo => Some(("Photos", vec![year(), month_of(m).unwrap_or_else(|| "Undated".into())])),
        Kind::Screenshot => Some(("Screenshots", vec![year()])),
        Kind::Video => {
            let n = row.name.to_uppercase();
            if ["VID_", "IMG_", "PXL_", "DSC", "MOV_"].iter().any(|p| n.starts_with(p)) {
                Some(("Videos", vec!["Personal".into(), year()]))
            } else if row.size > 300 * 1024 * 1024 {
                Some(("Videos", vec!["Movies & shows".into()]))
            } else {
                Some(("Videos", vec!["Other videos".into()]))
            }
        }
        Kind::Music => Some((
            "Music",
            vec![
                sanitize(m.artist.as_deref().unwrap_or("Unknown artist")),
                sanitize(m.album.as_deref().unwrap_or("Unknown album")),
            ],
        )),
        Kind::Audio => Some(("Audio", vec![])),
        Kind::Installer => Some(("Software", vec!["Installers".into()])),
        Kind::Font => Some(("Software", vec!["Fonts".into()])),
        Kind::Archive => Some(("Archives", vec![])),
        Kind::Code | Kind::System | Kind::Other => None,
    }
}

fn parent_of(p: &str) -> PathBuf {
    Path::new(p).parent().map(Path::to_path_buf).unwrap_or_default()
}

fn same_path(a: &Path, b: &Path) -> bool {
    a.to_string_lossy().eq_ignore_ascii_case(&b.to_string_lossy())
}

/// A placement for one file given its destination folder (`None` = stays, maybe renamed).
fn file_placement(row: &Row, dest_dir: Option<PathBuf>, new_name: Option<String>, to_sort: bool) -> Placement {
    let from_dir = parent_of(&row.path);
    let dir = dest_dir.unwrap_or_else(|| from_dir.clone());
    let name = new_name.clone().unwrap_or_else(|| row.name.clone());
    let to_path = dir.join(&name);
    let to = (!same_path(&to_path, Path::new(&row.path))).then(|| to_path.to_string_lossy().to_string());
    Placement {
        target: Target::File { file_id: row.id.clone() },
        from: row.path.clone(),
        to: if to_sort { None } else { to },
        new_name: if to_sort { None } else { new_name },
        confidence: row.meta.confidence,
        reason: reason_for(&row.meta),
        rule_key: Some(rule_key(&row.meta)),
        to_sort,
        bytes: row.size,
    }
}

fn stays(row: &Row, reason: &str) -> Placement {
    Placement {
        target: Target::File { file_id: row.id.clone() },
        from: row.path.clone(),
        to: None,
        new_name: None,
        confidence: row.meta.confidence,
        reason: reason.into(),
        rule_key: None,
        to_sort: false,
        bytes: row.size,
    }
}

struct Group {
    path: String,
    /// A code project (never renamed around or moved).
    code: bool,
    to: Option<PathBuf>,
    reason: String,
    confidence: f64,
}

fn is_course_name(name: &str) -> bool {
    let n = name.to_lowercase();
    n.chars().next().is_some_and(|c| c.is_ascii_digit())
        || ["lesson", "lecture", "module", "part ", "chapter", "episode", "section"].iter().any(|w| n.contains(w))
}

fn generic_photo_folder(name: &str) -> bool {
    let n = name.to_lowercase();
    n.chars().all(|c| c.is_ascii_digit() || c == '-' || c == '_' || c == ' ' || c == '.')
        || ["dcim", "camera", "camera roll", "pictures", "photos", "images", "100andro", "whatsapp images"]
            .iter()
            .any(|g| n == *g || n.starts_with("100"))
}

/// Code projects, course folders and named photo events: folders that move (or stay) whole.
fn find_groups(store: &SqliteStore, rows: &[&Row], root: &str, choice: BlueprintChoice, bases: &Bases) -> Result<Vec<Group>> {
    let mut groups = Vec::new();
    // Code projects (outermost only) stay where they are, so tools keep working.
    let units: Vec<String> = {
        let mut stmt = store.conn().prepare("SELECT root_path FROM atomic_units")?;
        let all: Vec<String> = stmt.query_map([], |r| r.get(0))?.collect::<rusqlite::Result<_>>()?;
        let within: Vec<String> = all.into_iter().filter(|u| path_is_within(u, root, false) && u != root).collect();
        within
            .iter()
            .filter(|u| !within.iter().any(|o| o != *u && path_is_within(u, o, false)))
            .cloned()
            .collect()
    };
    for u in units {
        groups.push(Group {
            path: u,
            code: true,
            to: None,
            reason: "Code project: stays where it is so your tools keep working".into(),
            confidence: 1.0,
        });
    }
    // Per-folder counts of videos, subtitles and photos.
    let mut dirs: HashMap<PathBuf, (usize, usize, usize, usize, Vec<i64>)> = HashMap::new(); // videos, course-named, srt, photos, photo years
    for r in rows {
        let d = parent_of(&r.path);
        let e = dirs.entry(d).or_default();
        match r.meta.kind {
            Kind::Video => {
                e.0 += 1;
                if is_course_name(&r.name) {
                    e.1 += 1;
                }
            }
            Kind::Photo => {
                e.3 += 1;
                if let Some(y) = year_of(&r.meta).and_then(|y| y.parse().ok()) {
                    e.4.push(y);
                }
            }
            _ => {}
        }
        if r.ext.eq_ignore_ascii_case("srt") || r.ext.eq_ignore_ascii_case("vtt") {
            e.2 += 1;
        }
    }
    let root_p = Path::new(root);
    let depth = |p: &Path| p.strip_prefix(root_p).map(|r| r.components().count()).unwrap_or(0);
    let qualifies = |d: &PathBuf| {
        dirs.get(d).is_some_and(|(v, named, srt, _, _)| *v >= 5 && (*srt > 0 || *named * 10 >= *v * 6))
    };
    let mut course_roots: HashSet<PathBuf> = HashSet::new();
    for d in dirs.keys().filter(|d| qualifies(d)) {
        // Climb to the course folder: sections ("Section 1", "Section 2") belong to one course.
        let mut c = d.clone();
        while let Some(parent) = c.parent().map(Path::to_path_buf) {
            if depth(&parent) < 2 {
                break;
            }
            let siblings = dirs.keys().filter(|o| o.parent() == Some(parent.as_path()) && qualifies(o)).count();
            if qualifies(&parent) || siblings >= 2 {
                c = parent;
            } else {
                break;
            }
        }
        if depth(&c) >= 1 {
            course_roots.insert(c);
        }
    }
    for c in course_roots {
        let name = c.file_name().map(|n| n.to_string_lossy().to_string()).unwrap_or_default();
        let to = match choice {
            BlueprintChoice::Fresh => bases.of("Videos").map(|b| b.join("Courses").join(&name)),
            BlueprintChoice::Tidy => None,
        };
        groups.push(Group { path: c.to_string_lossy().to_string(), code: false, to, reason: format!("Course: {name}"), confidence: 0.9 });
    }
    // Named photo events ("Cape Town Dec 2023").
    for (d, (_, _, _, photos, years)) in &dirs {
        let name = d.file_name().map(|n| n.to_string_lossy().to_string()).unwrap_or_default();
        if *photos < 10 || depth(d) == 0 || generic_photo_folder(&name) || groups.iter().any(|g| path_is_within(&d.to_string_lossy(), &g.path, false)) {
            continue;
        }
        let mut ys = years.clone();
        ys.sort_unstable();
        let year = ys.get(ys.len() / 2).map(|y| y.to_string()).unwrap_or_else(|| "Undated".into());
        let to = match choice {
            BlueprintChoice::Fresh => bases.of("Photos").map(|b| b.join(&year).join(&name)),
            BlueprintChoice::Tidy => None,
        };
        groups.push(Group { path: d.to_string_lossy().to_string(), code: false, to, reason: format!("Photo album: {name}"), confidence: 0.9 });
    }
    Ok(groups)
}

/// Lowercase name without "msi", "old", "copy of", "backup", "my" or digits, to spot folders
/// that are really the same thing (`documents` and `msidocuments`).
fn folder_key(name: &str) -> String {
    let mut n = name.to_lowercase();
    for p in ["copy of ", "backup of ", "backup ", "old ", "msi", "my "] {
        if let Some(rest) = n.strip_prefix(p) {
            n = rest.to_string();
        }
    }
    n.chars().filter(|c| c.is_alphabetic()).collect()
}

fn title_case(name: &str) -> String {
    name.split(' ')
        .map(|w| {
            let mut c = w.chars();
            c.next().map(|f| f.to_uppercase().collect::<String>() + c.as_str()).unwrap_or_default()
        })
        .collect::<Vec<_>>()
        .join(" ")
}

/// Plans every file in `scope` under `choice`.
pub fn plan(store: &SqliteStore, scope: &Scope, choice: BlueprintChoice) -> Result<Vec<Placement>> {
    plan_files(store, scope, choice, None)
}

/// Plans one file (for ongoing filing of new arrivals).
pub fn plan_one(store: &SqliteStore, scope: &Scope, choice: BlueprintChoice, file_id: &str) -> Result<Option<Placement>> {
    let only: HashSet<String> = [file_id.to_string()].into();
    Ok(plan_files(store, scope, choice, Some(&only))?.into_iter().find(|p| p.target == Target::File { file_id: file_id.into() }))
}

fn plan_files(store: &SqliteStore, scope: &Scope, choice: BlueprintChoice, only: Option<&HashSet<String>>) -> Result<Vec<Placement>> {
    let roots = scope_roots(store, scope)?;
    let rows = load_rows(store, scope, &roots)?;
    let bases = Bases {
        volume_root: matches!(scope, Scope::Volume { .. }).then(|| PathBuf::from(&roots[0].0)),
        known: roots.iter().filter_map(|(p, k)| k.map(|k| (k, PathBuf::from(p)))).collect(),
    };
    let mut out = Vec::new();
    for (root, _) in &roots {
        let root_rows: Vec<&Row> = rows.iter().filter(|r| path_is_within(&r.path, root, false)).collect();
        let groups = find_groups(store, &root_rows, root, choice, &bases)?;
        let in_group = |p: &str| groups.iter().any(|g| path_is_within(p, &g.path, false));
        if only.is_none() {
            for g in &groups {
                let files: Vec<&&Row> = root_rows.iter().filter(|r| path_is_within(&r.path, &g.path, false)).collect();
                out.push(Placement {
                    target: Target::Group { path: g.path.clone(), files: files.len() },
                    from: g.path.clone(),
                    to: g.to.as_ref().map(|t| t.to_string_lossy().to_string()).filter(|t| !t.eq_ignore_ascii_case(&g.path)),
                    new_name: None,
                    confidence: g.confidence,
                    reason: g.reason.clone(),
                    rule_key: None,
                    to_sort: false,
                    bytes: files.iter().map(|r| r.size).sum(),
                });
            }
        }
        // Tidy: top folders that are the same thing merge; all-lowercase names get Title Case.
        let root_p = PathBuf::from(root);
        let mut tops: BTreeMap<String, usize> = BTreeMap::new();
        for r in &root_rows {
            if let Ok(rel) = Path::new(&r.path).strip_prefix(&root_p)
                && rel.components().count() >= 2
            {
                *tops.entry(rel.components().next().unwrap().as_os_str().to_string_lossy().to_string()).or_default() += 1;
            }
        }
        let mut merge_into: HashMap<String, String> = HashMap::new();
        if choice == BlueprintChoice::Tidy {
            let mut by_key: HashMap<String, Vec<(&String, usize)>> = HashMap::new();
            for (t, n) in &tops {
                by_key.entry(folder_key(t)).or_default().push((t, *n));
            }
            for (_, mut same) in by_key.into_iter().filter(|(k, v)| !k.is_empty() && v.len() > 1) {
                same.sort_by_key(|x| std::cmp::Reverse(x.1));
                let keep = same[0].0.clone();
                for (t, _) in &same[1..] {
                    merge_into.insert((*t).clone(), keep.clone());
                }
            }
            if only.is_none() {
                for t in tops.keys().filter(|t| !merge_into.contains_key(*t)) {
                    let letters: Vec<char> = t.chars().filter(|c| c.is_alphabetic()).collect();
                    // Renaming a folder that holds code would break every path inside it.
                    let top_path = root_p.join(t).to_string_lossy().to_string();
                    let holds_code = groups.iter().any(|g| g.code && path_is_within(&g.path, &top_path, false));
                    if !holds_code && !letters.is_empty() && letters.iter().all(|c| c.is_lowercase()) {
                        let files = root_rows.iter().filter(|r| path_is_within(&r.path, &root_p.join(t).to_string_lossy(), false)).count();
                        out.push(Placement {
                            target: Target::Group { path: root_p.join(t).to_string_lossy().to_string(), files },
                            from: root_p.join(t).to_string_lossy().to_string(),
                            to: Some(root_p.join(title_case(t)).to_string_lossy().to_string()),
                            new_name: None,
                            confidence: 1.0,
                            reason: format!("Folder name tidied: {t} → {}", title_case(t)),
                            rule_key: None,
                            to_sort: false,
                            bytes: 0,
                        });
                    }
                }
            }
        }
        for r in &root_rows {
            if only.is_some_and(|o| !o.contains(&r.id)) || in_group(&r.path) {
                continue;
            }
            let c = classification_of(&r.meta);
            let stem = Path::new(&r.name).file_stem().map(|s| s.to_string_lossy().to_string()).unwrap_or_default();
            let new_name = proposed_name(&c, &stem, &r.ext, r.mtime);
            let low = r.meta.confidence < PROPOSE_AT;
            let is_doc = r.meta.kind == Kind::Document;
            let rel = Path::new(&r.path).strip_prefix(&root_p).map(Path::to_path_buf).unwrap_or_default();
            let loose = rel.components().count() == 1;
            let p = match choice {
                BlueprintChoice::Fresh => match fresh_area(r) {
                    _ if low && is_doc => file_placement(r, None, None, true),
                    _ if low => stays(r, "Left where it is: Aegis isn't sure what this is"),
                    Some((area, sub)) => match bases.of(area) {
                        Some(base) => {
                            let dir = sub.iter().fold(base, |p, s| p.join(s));
                            file_placement(r, Some(dir), new_name, false)
                        }
                        None => stays(r, "Left where it is: this PC has no folder for it"),
                    },
                    None => stays(r, "Left where it is: Aegis files documents, photos, music, videos and installers"),
                },
                BlueprintChoice::Tidy => {
                    let top = rel.components().next().map(|c| c.as_os_str().to_string_lossy().to_string()).unwrap_or_default();
                    if let Some(keep) = merge_into.get(&top) {
                        let rest: PathBuf = rel.components().skip(1).collect();
                        let dir = root_p.join(keep).join(rest.parent().unwrap_or(Path::new("")));
                        file_placement(r, Some(dir), new_name, false)
                    } else if loose {
                        match fresh_area(r) {
                            _ if low && is_doc => file_placement(r, None, None, true),
                            _ if low => stays(r, "Left where it is: Aegis isn't sure what this is"),
                            Some((area, _)) => {
                                let existing = tops.keys().find(|t| folder_key(t) == folder_key(area)).cloned();
                                let dir = root_p.join(existing.unwrap_or_else(|| area.to_string()));
                                file_placement(r, Some(dir), new_name, false)
                            }
                            None => stays(r, "Left where it is"),
                        }
                    } else if new_name.is_some() && !low {
                        file_placement(r, None, new_name, false)
                    } else {
                        stays(r, "Left where it is")
                    }
                }
            };
            out.push(p);
        }
    }
    Ok(out)
}

/// (files, bytes, per second-level folder (files, bytes)).
type TopCount = (i64, i64, BTreeMap<String, (i64, i64)>);

/// Counts, a two-level tree of where everything ends up, and a few examples.
pub fn summarize(placements: &[Placement], choice: BlueprintChoice, store: &SqliteStore) -> Result<BlueprintSummary> {
    let roots: Vec<String> = {
        let mut r: Vec<String> = store.list_roots()?.into_iter().map(|r| r.path).collect();
        let mut stmt = store.conn().prepare("SELECT mount_point FROM volumes")?;
        r.extend(stmt.query_map([], |row| row.get::<_, String>(0))?.filter_map(|m| m.ok()));
        r.sort_by_key(|p| std::cmp::Reverse(p.len()));
        r
    };
    // (top, second) -> (files, bytes)
    let mut tree: BTreeMap<String, TopCount> = BTreeMap::new();
    let (mut moves, mut renames, mut to_sort, mut stays) = (0i64, 0i64, 0i64, 0i64);
    let mut examples = Vec::new();
    for p in placements {
        let files = match &p.target {
            Target::File { .. } => 1,
            Target::Group { files, .. } => *files as i64,
        };
        let moved = p.to.as_ref().is_some_and(|t| !same_path(&parent_of(t), &parent_of(&p.from)));
        if p.to_sort {
            to_sort += files;
        } else if moved {
            moves += files;
        } else if p.to.is_none() {
            stays += files;
        }
        if p.new_name.is_some() {
            renames += 1;
        }
        if examples.len() < 6 && (p.new_name.is_some() || (moved && matches!(p.target, Target::File { .. }))) {
            let name = |s: &str| Path::new(s).file_name().map(|n| n.to_string_lossy().to_string()).unwrap_or_default();
            if let Some(t) = &p.to {
                // A move shows where it lands; a rename in place just the new name.
                let after = match roots.iter().find(|r| path_is_within(t, r, false)) {
                    Some(r) if moved => Path::new(t).strip_prefix(r).map(|x| x.to_string_lossy().to_string()).unwrap_or_else(|_| name(t)),
                    _ => name(t),
                };
                examples.push((name(&p.from), after));
            }
        }
        let final_path = p.to.clone().unwrap_or_else(|| p.from.clone());
        let Some(root) = roots.iter().find(|r| path_is_within(&final_path, r, false)) else { continue };
        let rel = Path::new(&final_path).strip_prefix(root).map(Path::to_path_buf).unwrap_or_default();
        let mut parts: Vec<String> = rel.components().map(|c| c.as_os_str().to_string_lossy().to_string()).collect();
        // PC folders are named after their root ("Documents", "Pictures").
        if store.list_roots()?.iter().any(|r| &r.path == root)
            && let Some(n) = Path::new(root).file_name()
        {
            parts.insert(0, n.to_string_lossy().to_string());
        }
        let is_group = matches!(p.target, Target::Group { .. });
        let (top, second) = if parts.len() <= 1 && !is_group {
            ("Loose files".to_string(), None)
        } else {
            (parts[0].clone(), parts.get(1).filter(|_| parts.len() > 2 || is_group).cloned())
        };
        let e = tree.entry(top).or_default();
        e.0 += files;
        e.1 += p.bytes;
        if let Some(s) = second {
            let c = e.2.entry(s).or_default();
            c.0 += files;
            c.1 += p.bytes;
        }
    }
    let tree = tree
        .into_iter()
        .map(|(name, (files, bytes, kids))| TreeNode {
            name,
            files,
            bytes,
            children: kids
                .into_iter()
                .map(|(name, (files, bytes))| TreeNode { name, files, bytes, children: vec![] })
                .collect(),
        })
        .collect();
    Ok(BlueprintSummary { choice, tree, moves, renames, to_sort, stays, examples })
}
