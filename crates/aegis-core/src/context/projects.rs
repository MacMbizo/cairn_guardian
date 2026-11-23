//! The Projects module (spec §7.1, §7.2): which folders are projects, where each project's root
//! is, and which folders are containers of projects. Deterministic and explainable: every
//! confidence comes with evidence the owner can read.
//!
//! The walk starts at the roots and goes down. Roots, Windows' own folders and the fresh
//! structure's areas are never projects. A **container** (a folder holding two or more projects
//! and few files of its own) is never a project either; its children are looked at in turn. Any
//! other folder is scored on what it holds itself plus its **parts** (`docs`, `images`, `src`,
//! `set_files` ...). At 0.60 or more it is a project root, and code projects found deeper inside
//! it are its sub-projects. Below 0.60 the walk goes on into its sub-folders.

use super::ProjectKind;
use super::profile::FolderProfile;
use crate::filing::Kind;
use std::collections::{BTreeSet, HashMap};

const SEP: char = std::path::MAIN_SEPARATOR;

/// A folder is a project from this confidence; filing protects it from here.
pub const PROJECT_THRESHOLD: f64 = 0.60;
/// From this confidence Aegis is sure; between the two it asks "Is this a project?".
pub const SURE_THRESHOLD: f64 = 0.80;

/// A folder recognised as a project.
#[derive(Debug, Clone, PartialEq)]
pub struct ProjectCandidate {
    pub root: String,
    pub volume_id: String,
    pub confidence: f64,
    pub evidence: Vec<String>,
    pub project_kind: ProjectKind,
    /// Code projects inside it, relative to its root (`template\prompt-pigeon`).
    pub sub_projects: Vec<String>,
    pub is_repo: bool,
    /// Names its manifests declare (`package.json` "name" and the like).
    pub declared_names: Vec<String>,
    /// Files that aren't build output, their size and the newest one's time.
    pub files: i64,
    pub bytes: i64,
    pub newest_mtime: i64,
}

impl ProjectCandidate {
    pub fn name(&self) -> &str {
        self.root.rsplit(SEP).next().unwrap_or(&self.root)
    }
}

/// Names of folders that are parts of a project rather than projects of their own.
const PART_NAMES: [&str; 52] = [
    "docs", "doc", "documentation", "documents", "images", "img", "imgs", "assets", "design", "designs", "src", "source",
    "lib", "app", "backup", "backups", "archive", "archives", "old", "set_files", "set files", "ui", "screens", "notes",
    "prompts", "prompt", "template", "templates", "resources", "res", "static", "public", "scripts", "test", "tests",
    "data", "media", "icons", "logos", "fonts", "styles", "css", "js", "components", "pages", "config", "versions",
    "drafts", "all docs", "others", "mockups", "exports",
];

/// Windows' own folders and the fresh structure's areas: never projects.
pub(crate) const AREA_NAMES: [&str; 18] = [
    "documents", "my documents", "pictures", "my pictures", "music", "my music", "videos", "my videos", "downloads",
    "desktop", "photos", "screenshots", "audio", "software", "archives", "to sort", "projects", "courses",
];

/// Life areas inside the fresh structure's Documents.
pub(crate) const LIFE_AREAS: [&str; 6] = ["finance", "personal", "study", "work", "other documents", "to sort"];

/// A folder name without version and copy suffixes: `set_files_v2(backup)` → `set_files`.
fn base_name(name: &str) -> String {
    let mut n = name.to_lowercase();
    if let Some(i) = n.find('(') {
        n.truncate(i);
    }
    let n = n.trim().trim_end_matches(|c: char| c.is_ascii_digit() || c == '.' || c == '_' || c == '-' || c == ' ');
    let n = n.strip_suffix("_v").or_else(|| n.strip_suffix(" v")).or_else(|| n.strip_suffix("-v")).unwrap_or(n);
    n.trim_end_matches(['_', '-', ' ']).to_string()
}

pub fn is_part_name(name: &str) -> bool {
    let b = base_name(name);
    PART_NAMES.contains(&b.as_str()) || b.starts_with("set_files") || b.starts_with("set files")
}

struct Ctx<'a> {
    by_path: HashMap<String, &'a FolderProfile>,
    roots: BTreeSet<String>,
    /// Roots that are general places (a drive's top level): they never lend project status.
    general: BTreeSet<String>,
    containers: HashMap<String, bool>,
    /// For each container, the share of its sizeable sub-folders that are projects by themselves.
    project_share: HashMap<String, f64>,
}

/// A container lends "sits among other projects" only when at least this share of its sizeable
/// sub-folders (three files or more) are projects on their own merits.
pub const LENDING_SHARE: f64 = 0.4;

/// What a folder holds itself, plus its parts (sub-folders that are parts of it).
#[derive(Default)]
struct Own {
    files: i64,
    doc_files: i64,
    asset_files: i64,
    source: i64,
    docs: BTreeSet<String>,
    assets: BTreeSet<String>,
    by_kind: HashMap<Kind, i64>,
}

fn key(p: &str) -> String {
    p.to_lowercase()
}

impl<'a> Ctx<'a> {
    fn get(&self, p: &str) -> Option<&'a FolderProfile> {
        self.by_path.get(&key(p)).copied()
    }

    fn children(&self, p: &FolderProfile) -> Vec<&'a FolderProfile> {
        p.child_dirs.iter().filter_map(|c| self.get(c)).collect()
    }

    fn is_root(&self, p: &FolderProfile) -> bool {
        p.depth == 0 || self.roots.contains(&key(&p.path))
    }

    /// A root, one of Windows' own folders, or an area of the fresh structure.
    fn is_area(&self, p: &FolderProfile) -> bool {
        if self.is_root(p) {
            return true;
        }
        let n = p.name().to_lowercase();
        if p.depth <= 1 && AREA_NAMES.contains(&n.as_str()) {
            return true;
        }
        let parent_is_area = p
            .path
            .rfind(SEP)
            .and_then(|i| self.get(&p.path[..i]))
            .is_some_and(|parent| parent.depth <= 1 && AREA_NAMES.contains(&parent.name().to_lowercase().as_str()) || self.is_root(parent));
        parent_is_area && LIFE_AREAS.contains(&n.as_str())
    }

    fn own(&self, p: &FolderProfile) -> Own {
        let mut o = Own {
            files: p.files,
            doc_files: p.doc_files,
            asset_files: p.asset_files,
            source: p.source_files,
            docs: p.direct_doc_signals.iter().cloned().collect(),
            assets: p.direct_asset_signals.iter().cloned().collect(),
            by_kind: p.by_kind.clone(),
        };
        for c in self.children(p) {
            if is_part_name(c.name()) {
                o.docs.extend(c.doc_signals.iter().cloned());
                o.assets.extend(c.asset_signals.iter().cloned());
            } else {
                o.files -= c.files;
                o.doc_files -= c.doc_files;
                o.asset_files -= c.asset_files;
                o.source -= c.source_files;
                for (k, n) in &c.by_kind {
                    *o.by_kind.entry(*k).or_default() -= n;
                }
            }
        }
        o
    }

    /// The confidence that `p` is a project root, with evidence. `among` names the container it
    /// sits in, when that container holds other projects.
    fn score(&self, p: &FolderProfile, among: Option<&str>) -> (f64, Vec<String>) {
        let own = self.own(p);
        let mut ev = Vec::new();
        let html_app = p.manifests.iter().any(|(m, _)| m.eq_ignore_ascii_case("index.html")) && own.source >= 2;
        let manifests: Vec<&str> =
            p.manifests.iter().map(|(m, _)| m.as_str()).filter(|m| !m.eq_ignore_ascii_case("index.html") || html_app).collect();
        let mut strong = 0;
        if p.has_git {
            ev.push("is a git repository".to_string());
            strong += 1;
        } else if p.atomic_unit {
            ev.push("is a code project (it has a build file)".to_string());
            strong += 1;
        }
        if !manifests.is_empty() {
            ev.push(format!("declares itself in {}", manifests.join(", ")));
            strong += 1;
        }
        if p.has_readme {
            ev.push("has a README".to_string());
            strong += 1;
        }

        let mut material = 0.0;
        let mut material_signals = 0;
        if own.doc_files > 0 {
            let words: Vec<String> = own.docs.iter().map(|d| display_word(d)).collect();
            material += 0.35 + 0.05 * (words.len().saturating_sub(1).min(3)) as f64;
            ev.push(format!("project documents: {}", words.join(", ")));
            material_signals += 1;
        }
        if own.source >= 3 {
            material += 0.35;
            ev.push(format!("source code ({} files)", own.source));
            material_signals += 1;
        } else if own.source >= 1 {
            material += 0.2;
            ev.push(format!("some source code ({} files)", own.source));
            material_signals += 1;
        }
        let asset_led = own.asset_files >= 3 && own.asset_files * 10 >= own.files * 6;
        if own.asset_files > 0 {
            material += if own.asset_files >= 3 { 0.3 } else { 0.2 };
            if asset_led {
                material += 0.35;
            }
            let words: Vec<String> = own.assets.iter().map(|w| display_word(w)).collect();
            ev.push(format!("brand and design files: {}", words.join(", ")));
            material_signals += 1;
        }
        let kinds = |ks: &[Kind]| ks.iter().map(|k| own.by_kind.get(k).copied().unwrap_or(0)).sum::<i64>();
        let docs = kinds(&[Kind::Document]);
        let images = kinds(&[Kind::Photo, Kind::Screenshot]);
        let groups = [docs > 0, images > 0, own.source > 0].iter().filter(|b| **b).count();
        if groups >= 2 && material_signals > 0 {
            material += 0.15;
            ev.push(match (docs > 0, images > 0, own.source > 0) {
                (true, true, true) => "documents, images and code together".to_string(),
                (true, true, false) => "documents and images together".to_string(),
                (true, false, true) => "documents and code together".to_string(),
                _ => "images and code together".to_string(),
            });
        }
        let media = kinds(&[Kind::Photo, Kind::Screenshot, Kind::Video, Kind::Music, Kind::Audio]);
        if own.files > 0 && media * 10 >= own.files * 8 && own.doc_files == 0 && own.source == 0 && !asset_led {
            material = material.min(0.3);
        }
        let dump = kinds(&[Kind::Installer, Kind::Archive]);
        if own.files > 0 && dump * 10 >= own.files * 3 && own.source == 0 {
            material -= 0.3;
        }
        if let Some(c) = among {
            // Judged on its own files, or on its whole subtree when it has none of its own.
            let (files, media, dump, source) = if own.files > 0 {
                (own.files, media, dump, own.source)
            } else {
                let sub = |ks: &[Kind]| ks.iter().map(|k| p.kind_count(*k)).sum::<i64>();
                (
                    p.files,
                    sub(&[Kind::Photo, Kind::Screenshot, Kind::Video, Kind::Music, Kind::Audio]),
                    sub(&[Kind::Installer, Kind::Archive]),
                    p.source_files,
                )
            };
            let media_led = files > 0 && media * 10 >= files * 8 && !asset_led;
            let dump_led = files > 0 && dump * 10 >= files * 3 && source == 0;
            if material > 0.0 {
                material = (material + 0.1).max(0.65);
                ev.push(format!("sits among other projects in {c}"));
            } else if (own.files >= 3 || p.files >= 3 && self.sub_projects(p).is_empty()) && !media_led && !dump_led {
                // Nothing in it says so, but everything around it is a project: ask. A folder
                // that only wraps another project (`D2\class-harmony-hub`) hands over to it.
                material = PROJECT_THRESHOLD;
                ev.push(format!("sits among other projects in {c}"));
            }
        }

        let conf = if strong > 0 {
            (0.9 + 0.02 * (strong - 1 + material_signals) as f64).min(0.98)
        } else {
            material.clamp(0.0, 0.85)
        };
        (conf, ev)
    }

    fn strong(&self, p: &FolderProfile) -> bool {
        p.atomic_unit || p.has_readme || p.manifests.iter().any(|(m, _)| !m.eq_ignore_ascii_case("index.html"))
    }

    /// Whether `p` is a container of projects: two or more of its own sub-folders are projects
    /// (or containers), and it holds few files of its own.
    fn is_container(&mut self, p: &FolderProfile) -> bool {
        if let Some(c) = self.containers.get(&key(&p.path)) {
            return *c;
        }
        let mut scores = Vec::new();
        let (mut sizeable, mut projects) = (0usize, 0usize);
        for c in self.children(p) {
            if is_part_name(c.name()) {
                continue;
            }
            let s = if self.is_container(c) { 1.0 } else { self.score(c, None).0 };
            if c.files >= 3 {
                sizeable += 1;
                if s >= PROJECT_THRESHOLD {
                    projects += 1;
                }
            }
            scores.push(s);
        }
        let c = !self.strong(p) && is_container(p, &scores);
        self.containers.insert(key(&p.path), c);
        self.project_share.insert(key(&p.path), projects as f64 / sizeable.max(1) as f64);
        c
    }

    fn visit(&mut self, p: &'a FolderProfile, among: Option<String>, out: &mut Vec<ProjectCandidate>) {
        let area = self.is_area(p);
        let container = self.is_container(p);
        if area || container {
            let lends = container
                && !self.general.contains(&key(&p.path))
                && !(area && self.is_root(p) && AREA_NAMES.contains(&p.name().to_lowercase().as_str()))
                && self.project_share.get(&key(&p.path)).copied().unwrap_or(0.0) >= LENDING_SHARE;
            let here = lends.then(|| p.name().to_string());
            for c in self.children(p) {
                self.visit(c, here.clone(), out);
            }
            return;
        }
        let (confidence, evidence) = self.score(p, among.as_deref());
        if confidence >= PROJECT_THRESHOLD {
            out.push(ProjectCandidate {
                root: p.path.clone(),
                volume_id: p.volume_id.clone(),
                confidence: (confidence * 100.0).round() / 100.0,
                evidence,
                project_kind: kind_of_project(p),
                sub_projects: self.sub_projects(p),
                is_repo: p.has_git,
                declared_names: p.manifests.iter().map(|(_, n)| n.clone()).filter(|n| !n.is_empty()).collect(),
                files: p.files,
                bytes: p.bytes,
                newest_mtime: p.newest_mtime,
            });
        } else {
            for c in self.children(p) {
                self.visit(c, None, out);
            }
        }
    }

    /// Code projects inside `root` (outermost only), relative to it.
    fn sub_projects(&self, root: &FolderProfile) -> Vec<String> {
        let mut out = Vec::new();
        let mut stack = self.children(root);
        while let Some(c) = stack.pop() {
            if self.strong(c) {
                out.push(c.path[root.path.len()..].trim_start_matches(SEP).to_string());
            } else {
                stack.extend(self.children(c));
            }
        }
        out.sort();
        out
    }
}

fn display_word(w: &str) -> String {
    match w {
        "prd" | "cmyk" | "dpi" => w.to_uppercase(),
        "readme" => "README".into(),
        "changelog" => "CHANGELOG".into(),
        "tasks" => "TASKS".into(),
        _ => w.to_string(),
    }
}

/// What sort of project a folder is, from its whole subtree.
fn kind_of_project(p: &FolderProfile) -> ProjectKind {
    let n = p.name().to_lowercase();
    if p.atomic_unit || p.source_files >= 3 {
        return ProjectKind::Software;
    }
    if p.asset_files >= 3 && p.asset_files * 2 >= p.files {
        return ProjectKind::Design;
    }
    if ["research", "thesis", "phd", "dissertation"].iter().any(|w| n.contains(w)) {
        return ProjectKind::Research;
    }
    if p.kind_count(Kind::Document) * 10 >= p.files * 6 {
        return ProjectKind::Writing;
    }
    if p.asset_files > 0 {
        return ProjectKind::Design;
    }
    ProjectKind::Other
}

/// A container holds two or more projects among its own sub-folders (`child_scores`, parts left
/// out) and few files of its own. The roots are containers too; the caller checks those.
pub fn is_container(p: &FolderProfile, child_scores: &[f64]) -> bool {
    let projects = child_scores.iter().filter(|s| **s >= PROJECT_THRESHOLD).count();
    let few_loose = p.loose_files <= 5 || p.loose_files * 10 <= p.files;
    projects >= 2 && few_loose
}

/// Every project under the roots (spec §7.1, §7.2). `known_roots` are the PC roots and drive
/// mount points; folders at depth 0 in `profiles` are roots too.
pub fn find_projects(profiles: &[FolderProfile], known_roots: &[String]) -> Vec<ProjectCandidate> {
    find_projects_with(profiles, known_roots, &[])
}

/// Like [`find_projects`], with `general_roots` (drive mount points) that never lend project
/// status to what sits at their top level.
pub fn find_projects_with(profiles: &[FolderProfile], known_roots: &[String], general_roots: &[String]) -> Vec<ProjectCandidate> {
    let mut ctx = Ctx {
        by_path: profiles.iter().map(|p| (key(&p.path), p)).collect(),
        roots: known_roots.iter().map(|r| key(r.trim_end_matches(SEP))).collect(),
        general: general_roots.iter().map(|r| key(r.trim_end_matches(SEP))).collect(),
        containers: HashMap::new(),
        project_share: HashMap::new(),
    };
    let mut out = Vec::new();
    for p in profiles.iter().filter(|p| p.depth == 0) {
        ctx.visit(p, None, &mut out);
    }
    out.sort_by_key(|a| a.root.to_lowercase());
    out
}
