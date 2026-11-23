//! Owner's folders module (spec 2026-11-22 §1A, §16 step 7): recognises purpose-named
//! document folders whose contents agree with the folder name (`CVs`, `PM`, `Ass. Bursar`,
//! `Finance Manager`, `PhD`, `Last30Days`, `DocAtlas Notes`), protects them as whole units, and
//! routes them under Fresh into the matching life area while keeping local-database folders and
//! already-organized folders in place.

use super::ClaimedIndex;
use super::profile::{FolderProfile, live_under_root_sql, name_words};
use super::projects::{AREA_NAMES, LIFE_AREAS, PART_NAMES};
use crate::db::SqliteStore;
use crate::db::files::path_is_within;
use crate::error::Result;
use crate::filing::Kind;
use rusqlite::params;
use std::collections::{HashMap, HashSet};
use std::path::Path;

const SEP: char = std::path::MAIN_SEPARATOR;

const DUMP_NAMES: [&str; 16] = [
    "misc",
    "temp",
    "tmp",
    "new folder",
    "untitled",
    "stuff",
    "other",
    "downloads",
    "desktop",
    "to sort",
    "archive",
    "backup",
    "now",
    "fri",
    "to go",
    "frinow",
];

const WORK_WORDS: [&str; 25] = [
    "cv",
    "cvs",
    "resume",
    "resumes",
    "bursar",
    "pm",
    "manager",
    "career",
    "job",
    "jobs",
    "hr",
    "minutes",
    "meeting",
    "meetings",
    "client",
    "clients",
    "contract",
    "contracts",
    "application",
    "applications",
    "membership",
    "admin",
    "staff",
    "payroll",
    "work",
];

const STUDY_WORDS: [&str; 24] = [
    "phd",
    "msc",
    "bsc",
    "mba",
    "thesis",
    "dissertation",
    "study",
    "studies",
    "school",
    "uni",
    "university",
    "college",
    "coursework",
    "assignment",
    "assignments",
    "exam",
    "exams",
    "lecture",
    "lectures",
    "research",
    "academic",
    "ielts",
    "degree",
    "module",
];

const FINANCE_WORDS: [&str; 22] = [
    "tax",
    "taxes",
    "invoice",
    "invoices",
    "receipt",
    "receipts",
    "bank",
    "banking",
    "payslip",
    "payslips",
    "audit",
    "budget",
    "budgets",
    "accounting",
    "accounts",
    "insurance",
    "investment",
    "investments",
    "loan",
    "loans",
    "mortgage",
    "billing",
];

const PERSONAL_WORDS: [&str; 18] = [
    "id",
    "ids",
    "passport",
    "certificates",
    "medical",
    "health",
    "home",
    "house",
    "lease",
    "vehicle",
    "vehicles",
    "car",
    "travel",
    "visa",
    "family",
    "legal",
    "personal",
    "estate",
];

/// A purpose-named document folder owned by the user.
#[derive(Debug, Clone, PartialEq)]
pub struct OwnersFolderCandidate {
    pub root: String,
    pub volume_id: String,
    pub name: String,
    /// Top-level Documents life area (`"Work"`, `"Study"`, `"Finance"`, `"Personal"`, or `""` to protect in place).
    pub life_area: String,
    pub confidence: f64,
    pub evidence: Vec<String>,
    pub files: i64,
    pub bytes: i64,
    pub newest_mtime: i64,
}

/// Combines camelCase-split words from `name_words` with lowercase whole tokens split on
/// non-alphanumerics so uppercase/mixed-case acronyms like `PhD` preserve `"phd"`.
fn folder_tokens(s: &str) -> Vec<String> {
    let mut out = name_words(s);
    for raw in s.to_lowercase().split(|c: char| !c.is_alphanumeric()).filter(|t| !t.is_empty()) {
        if !out.iter().any(|x| x == raw) {
            out.push(raw.to_string());
        }
    }
    out
}

fn area_from_name(name: &str) -> Option<&'static str> {
    let words = folder_tokens(name);
    if words.iter().any(|w| WORK_WORDS.contains(&w.as_str())) {
        Some("Work")
    } else if words.iter().any(|w| STUDY_WORDS.contains(&w.as_str())) {
        Some("Study")
    } else if words.iter().any(|w| FINANCE_WORDS.contains(&w.as_str())) {
        Some("Finance")
    } else if words.iter().any(|w| PERSONAL_WORDS.contains(&w.as_str())) {
        Some("Personal")
    } else {
        None
    }
}

fn parent_leaf(path: &str) -> Option<String> {
    Path::new(path).parent()?.file_name().map(|n| n.to_string_lossy().to_lowercase())
}

fn is_candidate_location(p: &FolderProfile, roots_map: &HashMap<String, Vec<String>>) -> bool {
    let Some(root) = roots_map
        .get(&p.volume_id)
        .and_then(|rs| rs.iter().find(|r| path_is_within(&p.path, r, false)))
    else {
        return false;
    };
    let root_leaf = Path::new(root).file_name().map(|n| n.to_string_lossy().to_lowercase()).unwrap_or_default();
    let is_docs_root = matches!(root_leaf.as_str(), "documents" | "my documents");
    if is_docs_root {
        p.depth == 1
            || (p.depth == 2
                && parent_leaf(&p.path).is_some_and(|pl| LIFE_AREAS.contains(&pl.as_str())))
    } else {
        // On a drive mount root (or general root), allow depth 1 (`D:\CVs`) and depth 2 inside `<drive>\Documents`
        // (`D:\Documents\Audit 2021`), or inside `<drive>\Documents\<LifeArea>` at depth 3.
        p.depth == 1
            || (p.depth == 2 && parent_leaf(&p.path).is_some_and(|pl| matches!(pl.as_str(), "documents" | "my documents")))
    }
}

fn synonym_matches(folder_words: &[String], file_words: &[String]) -> bool {
    let f_has = |w: &str| folder_words.iter().any(|x| x == w || (w.len() >= 3 && x.starts_with(w)));
    let m_has = |w: &str| file_words.iter().any(|x| x == w || (w.len() >= 3 && x.starts_with(w)));
    for group in [
        &["cv", "cvs", "resume", "cover", "letter"][..],
        &["pm", "minutes", "agenda", "project", "manager"][..],
        &["bursar", "cv", "cover", "letter"][..],
        &["phd", "research", "thesis", "dissertation", "concept"][..],
        &["note", "notes", "journal", "diary"][..],
        &["audit", "invoice", "receipt", "tax", "financial"][..],
        &["membership", "application", "mot"][..],
    ] {
        if group.iter().any(|w| f_has(w)) && group.iter().any(|w| m_has(w)) {
            return true;
        }
    }
    false
}

fn is_cv_like(name: &str, doc_type: Option<&str>) -> bool {
    if doc_type.is_some_and(|dt| dt.eq_ignore_ascii_case("CV")) {
        return true;
    }
    let words = name_words(name);
    words.iter().any(|w| matches!(w.as_str(), "cv" | "cvs" | "resume" | "resumes"))
        || words.windows(2).any(|w| w[0] == "cover" && w[1] == "letter")
}

/// Finds owner purpose-named folders under `known_roots` outside `claimed_roots`.
pub fn find_owners_folders(
    store: &SqliteStore,
    profiles: &[FolderProfile],
    known_roots: &[String],
    claimed_roots: &[(Option<String>, String)],
) -> Result<Vec<OwnersFolderCandidate>> {
    let mut roots_by_vol: HashMap<String, Vec<String>> = HashMap::new();
    for p in profiles.iter().filter(|p| p.depth == 0) {
        if known_roots.iter().any(|r| r.trim_end_matches(SEP).eq_ignore_ascii_case(p.path.trim_end_matches(SEP))) {
            roots_by_vol.entry(p.volume_id.clone()).or_default().push(p.path.trim_end_matches(SEP).to_string());
        }
    }

    let claimed = ClaimedIndex::new(claimed_roots);
    let mut pre: Vec<&FolderProfile> = Vec::new();
    for p in profiles {
        if p.files == 0
            || p.files > 500
            || p.has_git
            || p.atomic_unit
            || p.exe_files > 0
            || p.source_files >= 3
            || {
                let (in_c, holds_c) = claimed.overlaps(&p.volume_id, &p.path);
                in_c || holds_c
            }
        {
            continue;
        }
        if !is_candidate_location(p, &roots_by_vol) {
            continue;
        }
        let nlow = p.name().to_lowercase();
        if AREA_NAMES.contains(&nlow.as_str())
            || LIFE_AREAS.contains(&nlow.as_str())
            || PART_NAMES.contains(&nlow.as_str())
            || DUMP_NAMES.contains(&nlow.as_str())
        {
            continue;
        }
        let doc_count = p.kind_count(Kind::Document);
        let photo_count = p.kind_count(Kind::Photo) + p.kind_count(Kind::Screenshot);
        let db_bonus = i64::from(p.has_db_file);
        if doc_count < 1 || doc_count * 10 < p.files * 4 || (doc_count + photo_count + db_bonus) * 10 < p.files * 8 {
            continue;
        }
        pre.push(p);
    }

    if pre.is_empty() {
        return Ok(Vec::new());
    }

    // Load file names & classification metadata once across the roots holding candidate folders.
    let filter = live_under_root_sql()
        .replace("deleted_at", "f.deleted_at")
        .replace("container_file_id", "f.container_file_id")
        .replace("state != 'quarantine'", "f.state NOT IN ('quarantine', 'missing')")
        .replace("substr(lower(path)", "substr(lower(f.path)")
        .replace("lower(COALESCE(ext", "lower(COALESCE(f.ext")
        .replace("instr(substr(lower(path)", "instr(substr(lower(f.path)");
    let sql = format!(
        "SELECT f.volume_id, f.path, f.name, m.category, m.doc_type
         FROM files f LEFT JOIN file_meta m ON m.file_id = f.id
         WHERE {filter}"
    );
    let mut stmt = store.conn().prepare(&sql)?;
    type OwnerFileRow = (String, Option<String>, Option<String>);
    let mut files_by_cand: Vec<Vec<OwnerFileRow>> = vec![Vec::new(); pre.len()];
    let cand_roots: Vec<(&str, String)> = pre.iter().map(|p| (p.volume_id.as_str(), p.path.to_lowercase())).collect();
    let seen_roots: HashSet<String> = known_roots.iter().map(|r| r.trim_end_matches(SEP).to_lowercase()).collect();

    for root in seen_roots {
        let prefix = format!("{root}{SEP}");
        let plen = prefix.chars().count() as i64;
        let rows = stmt.query_map(params![prefix, plen], |r| {
            Ok((
                r.get::<_, String>(0)?,
                r.get::<_, String>(1)?,
                r.get::<_, String>(2)?,
                r.get::<_, Option<String>>(3)?,
                r.get::<_, Option<String>>(4)?,
            ))
        })?;
        for row in rows {
            let (vol, path, name, cat, doc_type) = row?;
            for (idx, (cv, cp)) in cand_roots.iter().enumerate() {
                if *cv == vol && path_is_within(&path, cp, false) {
                    files_by_cand[idx].push((name, cat, doc_type));
                    break;
                }
            }
        }
    }

    let mut out = Vec::new();
    for (idx, p) in pre.into_iter().enumerate() {
        let items = &files_by_cand[idx];
        if items.is_empty() {
            continue;
        }
        let folder_words: Vec<String> = folder_tokens(p.name())
            .into_iter()
            .filter(|w| !matches!(w.as_str(), "the" | "and" | "for" | "my" | "all"))
            .collect();
        let name_life = area_from_name(p.name());

        let mut name_agreed = 0usize;
        let mut cv_hits = 0usize;
        let mut area_counts: HashMap<String, usize> = HashMap::new();
        let mut classified = 0usize;

        for (fname, cat, doc_type) in items {
            let stem = fname.rsplit_once('.').map_or(fname.as_str(), |(s, _)| s);
            let fwords = folder_tokens(stem);
            let direct = folder_words.iter().any(|fw| {
                fw.len() >= 2
                    && fwords.iter().any(|w| {
                        w == fw
                            || (fw.len() >= 4 && w.starts_with(fw))
                            || (w.len() >= 4 && fw.starts_with(w))
                    })
            });
            if direct || synonym_matches(&folder_words, &fwords) {
                name_agreed += 1;
            }
            if is_cv_like(fname, doc_type.as_deref()) {
                cv_hits += 1;
            }
            if let Some(c) = cat
                && let Some(top) = c.split('/').next()
                && !top.is_empty()
            {
                classified += 1;
                *area_counts.entry(top.to_string()).or_default() += 1;
            }
        }

        let dominant_meta = area_counts
            .into_iter()
            .max_by(|a, b| a.1.cmp(&b.1).then_with(|| b.0.cmp(&a.0)))
            .filter(|(_, n)| *n * 10 >= classified.max(1) * 6);

        let name_agrees = name_agreed * 2 >= items.len();
        let meta_agrees = dominant_meta.is_some() && (items.len() >= 2 || name_life.is_some());
        if !name_agrees && !meta_agrees {
            continue;
        }

        let life_area = if p.has_db_file {
            String::new()
        } else if cv_hits * 2 >= items.len() {
            "Work".to_string()
        } else if let Some(a) = name_life {
            a.to_string()
        } else if let Some((a, _)) = &dominant_meta {
            a.clone()
        } else {
            String::new()
        };

        let confidence = if (name_agrees && name_life.is_some()) || cv_hits * 2 >= items.len() {
            0.90
        } else {
            0.85
        };
        let why_area = if life_area.is_empty() {
            "kept in place".to_string()
        } else {
            format!("belongs under {life_area}")
        };
        out.push(OwnersFolderCandidate {
            root: p.path.clone(),
            volume_id: p.volume_id.clone(),
            name: p.name().to_string(),
            life_area,
            confidence,
            evidence: vec![format!("your {} folder ({} files · {why_area})", p.name(), p.files)],
            files: p.files,
            bytes: p.bytes,
            newest_mtime: p.newest_mtime,
        });
    }
    out.sort_by_key(|o| o.root.to_lowercase());
    Ok(out)
}
