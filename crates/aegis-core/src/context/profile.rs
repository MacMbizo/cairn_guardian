//! Per-folder statistics from the catalog (spec §7.1, §13): what each folder under the roots
//! holds, with its whole subtree rolled up. One SQL pass grouped by folder and extension, then a
//! roll-up over folders (never a loop over every file in Rust). Nothing on disk is read.

use crate::db::SqliteStore;
use crate::error::Result;
use crate::filing::Kind;
use crate::filing::classify::kind_of;
use rusqlite::params;
use std::collections::{BTreeSet, HashMap};

const SEP: char = std::path::MAIN_SEPARATOR;

/// Folders whose contents are build output: they still belong to their folder, but never count
/// toward comparisons or signals.
pub const BUILD_DIRS: [&str; 10] = ["node_modules", ".git", "target", "dist", "build", "__pycache__", ".venv", "venv", ".next", ".turbo"];

/// Names of project documents (spec §7.1), matched as words in a file name.
pub const DOC_WORDS: [&str; 13] =
    ["prd", "spec", "requirements", "tasks", "roadmap", "architecture", "persona", "prompt", "rules", "readme", "changelog", "brief", "set_files"];

/// Names of brand and UI assets, matched as words in a file name.
pub const ASSET_WORDS: [&str; 9] = ["logo", "favicon", "icon", "mockup", "wireframe", "banner", "sleeve", "cmyk", "dpi"];

/// Extensions that are design files on their own.
pub const ASSET_EXTS: [&str; 9] = ["fig", "psd", "ai", "svg", "xcf", "eps", "indd", "cdr", "sketch"];

/// Files that declare a project (spec §7.1, strong signal).
pub const MANIFESTS: [&str; 10] = [
    "package.json",
    "cargo.toml",
    "pyproject.toml",
    "requirements.txt",
    "pubspec.yaml",
    "go.mod",
    "composer.json",
    "gemfile",
    "index.html",
    "*.csproj",
];

/// Source code beyond what filing calls code (web and data files count for projects).
const EXTRA_SOURCE_EXTS: [&str; 10] = ["html", "css", "scss", "sql", "dart", "vue", "svelte", "sh", "ps1", "toml"];

/// What one folder holds. Counts, sizes, kinds, signals and `source_files` cover the whole
/// subtree; `manifests`, `has_readme`, `has_git`, `atomic_unit` and `loose_files` are the folder's own.
#[derive(Debug, Clone, PartialEq, Default)]
pub struct FolderProfile {
    pub path: String,
    pub volume_id: String,
    /// Folders below its root (the root itself is 0).
    pub depth: usize,
    /// Files that aren't build output.
    pub files: i64,
    pub bytes: i64,
    pub newest_mtime: i64,
    pub by_kind: HashMap<Kind, i64>,
    pub source_files: i64,
    /// Project-document words found in names in the subtree (`prd`, `tasks`, `set_files` ...).
    pub doc_signals: Vec<String>,
    /// Asset words and design extensions found in the subtree (`logo`, `favicon`, `.psd` ...).
    pub asset_signals: Vec<String>,
    /// The same words, from files directly in the folder (and its own name, for `set_files`).
    pub direct_doc_signals: Vec<String>,
    pub direct_asset_signals: Vec<String>,
    /// Files in the subtree whose name carries a project-document word.
    pub doc_files: i64,
    /// Files in the subtree whose name carries an asset word or design extension.
    pub asset_files: i64,
    /// (manifest file name, declared name or "") at the folder top.
    pub manifests: Vec<(String, String)>,
    pub has_readme: bool,
    pub has_git: bool,
    /// The folder is a code project found by its build marker.
    pub atomic_unit: bool,
    /// Code projects inside it, or binary/installer/resource/database clues rolled up across its
    /// subtree for Step 7 modules (0 extra SQL queries).
    /// Executables / MSI installers (`exe`, `msi`, `msix`).
    pub exe_files: i64,
    /// Installer package/payload files (`msi`, `msix`, `msp`, `mst`, `cab`, `inf`, `cat`).
    pub installer_files: i64,
    /// Native runtime/config files (`dll`, `sys`, `ocx`, `pdb`, `cfg`, `config`, `ini`).
    pub runtime_files: i64,
    /// Holds `setup.exe`, `install.exe`, `installer.exe`, `autorun.inf`, `setup.ini`, or `.msi`.
    pub has_setup_file: bool,
    /// Holds a `SKILL.md` file.
    pub has_skill_file: bool,
    /// Holds a `DESIGN.md`, `tokens.json`, or `theme.json` file.
    pub has_design_kit_file: bool,
    /// Holds a local database file (`db`, `sqlite`, `sqlite3`).
    pub has_db_file: bool,
    /// Full paths of its sub-folders (build folders left out).
    pub child_dirs: Vec<String>,
    /// Files directly inside it.
    pub loose_files: i64,
}

impl FolderProfile {
    pub fn name(&self) -> &str {
        self.path.rsplit(SEP).next().unwrap_or(&self.path)
    }

    pub fn kind_count(&self, k: Kind) -> i64 {
        self.by_kind.get(&k).copied().unwrap_or(0)
    }
}

/// Words of a file or folder name: split on anything not alphanumeric and on lower→upper case
/// changes (`fxLogo` → `fx`, `logo`), lowercased.
pub fn name_words(name: &str) -> Vec<String> {
    let mut words = Vec::new();
    let mut cur = String::new();
    let mut prev_lower = false;
    for c in name.chars() {
        if !c.is_alphanumeric() {
            if !cur.is_empty() {
                words.push(std::mem::take(&mut cur));
            }
            prev_lower = false;
            continue;
        }
        if c.is_uppercase() && prev_lower && !cur.is_empty() {
            words.push(std::mem::take(&mut cur));
        }
        prev_lower = c.is_lowercase() || c.is_ascii_digit();
        cur.extend(c.to_lowercase());
    }
    if !cur.is_empty() {
        words.push(cur);
    }
    words
}

fn word_matches(word: &str, want: &str) -> bool {
    word == want || (word.len() == want.len() + 1 && word.starts_with(want) && word.ends_with('s'))
}

/// The project-document words in a file name (`PRD_additions.md` → `prd`).
pub fn doc_words_in(name: &str) -> Vec<&'static str> {
    let stem = name.rsplit_once('.').map_or(name, |(s, _)| s);
    let words = name_words(stem);
    let lower = stem.to_lowercase();
    let mut out: Vec<&'static str> = DOC_WORDS
        .iter()
        .copied()
        .filter(|w| if w.contains('_') { lower.contains(w) } else { words.iter().any(|x| word_matches(x, w)) })
        .collect();
    if words.windows(2).any(|p| p[0] == "design" && word_matches(&p[1], "doc")) {
        out.push("design doc");
    }
    out
}

/// The asset words in a file name, plus its extension when it is a design file.
pub fn asset_words_in(name: &str, ext: &str) -> Vec<String> {
    let stem = name.rsplit_once('.').map_or(name, |(s, _)| s);
    let words = name_words(stem);
    let mut out: Vec<String> = ASSET_WORDS
        .iter()
        .filter(|w| words.iter().any(|x| word_matches(x, w) || x.starts_with(*w) && **w == "favicon"))
        .map(|w| (*w).to_string())
        .collect();
    let e = ext.to_ascii_lowercase();
    if ASSET_EXTS.contains(&e.as_str()) {
        out.push(format!(".{e}"));
    }
    out
}

fn is_manifest(name_lower: &str) -> bool {
    MANIFESTS.iter().any(|m| m.strip_prefix('*').map_or(name_lower == *m, |suffix| name_lower.ends_with(suffix)))
}

/// The name a manifest declares, from text the reader already stored (`"name": "x"` or `name = "x"`).
fn declared_name(text: &str) -> String {
    for line in text.lines().take(60) {
        let t = line.trim().trim_end_matches(',');
        let rest = t.strip_prefix("\"name\"").or_else(|| t.strip_prefix("name"));
        if let Some(rest) = rest {
            let rest = rest.trim_start();
            if let Some(v) = rest.strip_prefix(':').or_else(|| rest.strip_prefix('=')) {
                let v = v.trim().trim_matches('"').trim_matches('\'');
                if !v.is_empty() && !v.contains(' ') {
                    return v.to_string();
                }
            }
        }
    }
    String::new()
}

fn is_source_ext(ext: &str) -> bool {
    kind_of(ext, "", false) == Kind::Code || EXTRA_SOURCE_EXTS.contains(&ext)
}

/// SQL that keeps only files below a root (`?1` = lowercased root plus separator, `?2` = its
/// length) that aren't build output, ignoring the root's own path when looking for build folders.
pub(crate) fn live_under_root_sql() -> String {
    let mut s = String::from(
        "deleted_at IS NULL AND container_file_id IS NULL AND state != 'quarantine'
         AND substr(lower(path), 1, ?2) = ?1 AND lower(COALESCE(ext, '')) != 'pyc'",
    );
    for d in BUILD_DIRS {
        s.push_str(&format!(" AND instr(substr(lower(path), ?2), '{SEP}{d}{SEP}') = 0"));
    }
    s
}

fn parent_dir(path: &str) -> Option<&str> {
    path.rfind(SEP).map(|i| &path[..i])
}

#[derive(Default)]
struct Acc {
    display: String,
    volume_id: String,
    files: i64,
    bytes: i64,
    newest: i64,
    by_kind: HashMap<Kind, i64>,
    source: i64,
    docs: BTreeSet<String>,
    assets: BTreeSet<String>,
    doc_files: i64,
    asset_files: i64,
    manifests: Vec<(String, String)>,
    readme: bool,
    exe_files: i64,
    installer_files: i64,
    runtime_files: i64,
    has_setup_file: bool,
    has_skill_file: bool,
    has_design_kit_file: bool,
    has_db_file: bool,
    loose: i64,
    children: BTreeSet<String>,
}

/// Profiles of every folder under `roots` (PC roots or drive mount points), the roots included.
pub fn profiles(store: &SqliteStore, roots: &[String]) -> Result<Vec<FolderProfile>> {
    let roots: Vec<(String, Option<String>)> = roots.iter().map(|r| (r.clone(), None)).collect();
    profiles_for(store, &roots)
}

/// Like [`profiles`], each root limited to one volume when given (a drive letter can belong to
/// an unplugged drive and a plugged-in one at once).
pub fn profiles_for(store: &SqliteStore, roots: &[(String, Option<String>)]) -> Result<Vec<FolderProfile>> {
    let conn = store.conn();
    let units: HashMap<String, String> = {
        let mut stmt = conn.prepare("SELECT lower(root_path), unit_type FROM atomic_units")?;
        stmt.query_map([], |r| Ok((r.get::<_, String>(0)?, r.get::<_, String>(1)?)))?.collect::<rusqlite::Result<_>>()?
    };
    let filter = live_under_root_sql();
    let mut out = Vec::new();
    for (root, volume) in roots {
        let root = root.trim_end_matches(SEP).to_string();
        let prefix = format!("{}{SEP}", root.to_lowercase());
        let plen = prefix.chars().count() as i64;
        let root_key = root.to_lowercase();
        let mut acc: HashMap<String, Acc> = HashMap::new();

const SETUP_FILE_NAMES: [&str; 5] = ["setup.exe", "install.exe", "installer.exe", "autorun.inf", "setup.ini"];
const DESIGN_KIT_FILE_NAMES: [&str; 3] = ["design.md", "tokens.json", "theme.json"];

fn is_exe_ext(ext: &str) -> bool {
    matches!(ext, "exe" | "msi" | "msix")
}

fn is_installer_ext(ext: &str) -> bool {
    matches!(ext, "msi" | "msix" | "msp" | "mst" | "cab" | "inf" | "cat")
}

fn is_runtime_ext(ext: &str) -> bool {
    matches!(ext, "dll" | "sys" | "ocx" | "pdb" | "cfg" | "config" | "ini")
}

fn is_db_ext(ext: &str) -> bool {
    matches!(ext, "db" | "sqlite" | "sqlite3")
}

        // Folder totals by extension.
        let mut stmt = conn.prepare(&format!(
            "SELECT volume_id, substr(path, 1, length(path) - length(name) - 1) AS dir, lower(COALESCE(ext, '')),
                    COUNT(*), SUM(size), MAX(mtime)
             FROM files WHERE {filter} AND (?3 IS NULL OR volume_id = ?3) GROUP BY volume_id, dir, lower(COALESCE(ext, ''))"
        ))?;
        let rows = stmt.query_map(params![prefix, plen, volume], |r| {
            Ok((
                r.get::<_, String>(0)?,
                r.get::<_, String>(1)?,
                r.get::<_, String>(2)?,
                r.get::<_, i64>(3)?,
                r.get::<_, i64>(4)?,
                r.get::<_, i64>(5)?,
            ))
        })?;
        for row in rows {
            let (vol, dir, ext, n, bytes, newest) = row?;
            let a = acc.entry(dir.to_lowercase()).or_default();
            if a.display.is_empty() {
                a.display = dir;
                a.volume_id = vol;
            }
            a.files += n;
            a.loose += n;
            a.bytes += bytes;
            a.newest = a.newest.max(newest);
            *a.by_kind.entry(kind_of(&ext, "", false)).or_default() += n;
            if is_source_ext(&ext) {
                a.source += n;
            }
            if is_exe_ext(&ext) {
                a.exe_files += n;
            }
            if is_installer_ext(&ext) {
                a.installer_files += n;
            }
            if is_runtime_ext(&ext) {
                a.runtime_files += n;
            }
            if ext == "msi" {
                a.has_setup_file = true;
            }
            if is_db_ext(&ext) {
                a.has_db_file = true;
            }
        }

        // Names that carry a signal, a README, a manifest, setup marker or resource kit marker.
        let mut like: Vec<String> = DOC_WORDS.iter().chain(ASSET_WORDS.iter()).map(|w| format!("%{}%", w.replace('_', ""))).collect();
        like.push("%readme%".into());
        let special_names = SETUP_FILE_NAMES
            .iter()
            .chain(std::iter::once(&"skill.md"))
            .chain(DESIGN_KIT_FILE_NAMES.iter())
            .map(|f| format!("'{f}'"))
            .collect::<Vec<_>>()
            .join(", ");
        let name_test: String = like
            .iter()
            .map(|l| format!("replace(lower(name), '_', '') LIKE '{l}'"))
            .chain(MANIFESTS.iter().map(|m| match m.strip_prefix('*') {
                Some(suffix) => format!("lower(name) LIKE '%{suffix}'"),
                None => format!("lower(name) = '{m}'"),
            }))
            .chain(ASSET_EXTS.iter().map(|e| format!("lower(COALESCE(ext, '')) = '{e}'")))
            .chain(std::iter::once(format!("lower(name) IN ({special_names})")))
            .collect::<Vec<_>>()
            .join(" OR ");
        let mut stmt = conn.prepare(&format!(
            "SELECT id, substr(path, 1, length(path) - length(name) - 1), name, COALESCE(ext, '')
             FROM files WHERE {filter} AND (?3 IS NULL OR volume_id = ?3) AND ({name_test})"
        ))?;
        let named: Vec<(String, String, String, String)> = stmt
            .query_map(params![prefix, plen, volume], |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?, r.get(3)?)))?
            .collect::<rusqlite::Result<_>>()?;
        let mut text_stmt = conn.prepare("SELECT text FROM chunks WHERE file_id = ?1 ORDER BY seq LIMIT 1")?;
        for (id, dir, name, ext) in named {
            let Some(a) = acc.get_mut(&dir.to_lowercase()) else { continue };
            let lower = name.to_lowercase();
            let docs = doc_words_in(&name);
            if !docs.is_empty() {
                a.doc_files += 1;
                a.docs.extend(docs.iter().map(|d| d.to_string()));
            }
            let assets = asset_words_in(&name, &ext);
            if !assets.is_empty() {
                a.asset_files += 1;
                a.assets.extend(assets);
            }
            if lower.starts_with("readme") {
                a.readme = true;
            }
            if SETUP_FILE_NAMES.contains(&lower.as_str()) {
                a.has_setup_file = true;
            }
            if lower == "skill.md" {
                a.has_skill_file = true;
            }
            if DESIGN_KIT_FILE_NAMES.contains(&lower.as_str()) {
                a.has_design_kit_file = true;
            }
            if is_manifest(&lower) {
                let text: Option<String> = text_stmt.query_row([&id], |r| r.get(0)).ok();
                a.manifests.push((name.clone(), text.as_deref().map(declared_name).unwrap_or_default()));
            }
        }

        // Every folder between a file and the root exists, even when it holds no files itself.
        let keys: Vec<String> = acc.keys().cloned().collect();
        for k in keys {
            let mut child = k.clone();
            let mut display = acc[&k].display.clone();
            let vol = acc[&k].volume_id.clone();
            while child.len() > root_key.len() {
                let (Some(pk), Some(pd)) = (parent_dir(&child), parent_dir(&display)) else { break };
                let (pk, pd) = (pk.to_string(), pd.to_string());
                let p = acc.entry(pk.clone()).or_default();
                if p.display.is_empty() {
                    p.display = pd.clone();
                    p.volume_id = vol.clone();
                }
                p.children.insert(display.clone());
                child = pk;
                display = pd;
            }
        }
        // A folder named like `set_files` is a project-document signal for itself.
        for a in acc.values_mut() {
            let n = a.display.rsplit(SEP).next().unwrap_or("").to_lowercase();
            if n.starts_with("set_files") || n.starts_with("set files") {
                a.docs.insert("set_files".into());
            }
        }

        let direct: HashMap<String, (Vec<String>, Vec<String>)> = acc
            .iter()
            .map(|(k, a)| (k.clone(), (a.docs.iter().cloned().collect(), a.assets.iter().cloned().collect())))
            .collect();
        // Roll the subtree up, deepest folders first.
        let mut order: Vec<String> = acc.keys().cloned().collect();
        order.sort_by_key(|k| std::cmp::Reverse(k.matches(SEP).count()));
        for k in order {
            if k.len() <= root_key.len() {
                continue;
            }
            let Some(pk) = parent_dir(&k).map(str::to_string) else { continue };
            let Some(c) = acc.get(&k) else { continue };
            let (
                files,
                bytes,
                newest,
                source,
                kinds,
                docs,
                assets,
                df,
                af,
                exe_files,
                installer_files,
                runtime_files,
                has_setup_file,
                has_skill_file,
                has_design_kit_file,
                has_db_file,
            ) = (
                c.files,
                c.bytes,
                c.newest,
                c.source,
                c.by_kind.clone(),
                c.docs.clone(),
                c.assets.clone(),
                c.doc_files,
                c.asset_files,
                c.exe_files,
                c.installer_files,
                c.runtime_files,
                c.has_setup_file,
                c.has_skill_file,
                c.has_design_kit_file,
                c.has_db_file,
            );
            if let Some(p) = acc.get_mut(&pk) {
                p.files += files;
                p.bytes += bytes;
                p.newest = p.newest.max(newest);
                p.source += source;
                for (kind, n) in kinds {
                    *p.by_kind.entry(kind).or_default() += n;
                }
                p.docs.extend(docs);
                p.assets.extend(assets);
                p.doc_files += df;
                p.asset_files += af;
                p.exe_files += exe_files;
                p.installer_files += installer_files;
                p.runtime_files += runtime_files;
                p.has_setup_file |= has_setup_file;
                p.has_skill_file |= has_skill_file;
                p.has_design_kit_file |= has_design_kit_file;
                p.has_db_file |= has_db_file;
            }
        }

        let root_depth = root_key.matches(SEP).count();
        for (k, a) in acc {
            if !(k == root_key || k.starts_with(&prefix)) {
                continue;
            }
            let unit = units.get(&k);
            let (direct_doc_signals, direct_asset_signals) = direct.get(&k).cloned().unwrap_or_default();
            out.push(FolderProfile {
                direct_doc_signals,
                direct_asset_signals,
                depth: k.matches(SEP).count() - root_depth,
                path: a.display,
                volume_id: a.volume_id,
                files: a.files,
                bytes: a.bytes,
                newest_mtime: a.newest,
                by_kind: a.by_kind,
                source_files: a.source,
                doc_signals: a.docs.into_iter().collect(),
                asset_signals: a.assets.into_iter().collect(),
                doc_files: a.doc_files,
                asset_files: a.asset_files,
                manifests: a.manifests,
                has_readme: a.readme,
                has_git: unit.is_some_and(|t| t == "git_repo"),
                atomic_unit: unit.is_some(),
                exe_files: a.exe_files,
                installer_files: a.installer_files,
                runtime_files: a.runtime_files,
                has_setup_file: a.has_setup_file,
                has_skill_file: a.has_skill_file,
                has_design_kit_file: a.has_design_kit_file,
                has_db_file: a.has_db_file,
                child_dirs: a.children.into_iter().collect(),
                loose_files: a.loose,
            });
        }
    }
    out.sort_by_key(|a| a.path.to_lowercase());
    Ok(out)
}
