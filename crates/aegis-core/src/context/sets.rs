//! Loose-file sets and photo subjects (spec 2026-11-22-context-aware-filing-design §10, §17):
//! - **Design sets**: loose artwork across formats (`SmoothLite`, `Back Cement`) grouped into one
//!   design project, kept whole from file-by-file filing and gathered into `<projects home>\<Name>`.
//! - **Photo subjects**: loose photos sharing a meaningful subject (`ipadcover`, `fragrances`) so
//!   Fresh files them into `Photos\<year>\<subject>` instead of `<year-month>`.

use super::profile::live_under_root_sql;
use super::relate::{fingerprints_match, normalized_name};
use super::store::PlanRun;
use super::{Collection, CollectionKind, Decision, DecisionKind, Location, ProjectKind, Relationship, SetMember};
use crate::db::SqliteStore;
use crate::db::files::path_is_within;
use crate::error::Result;
use crate::filing::blueprint::generic_photo_folder;
use crate::new_id;
use rusqlite::params;
use std::collections::{BTreeMap, HashMap, HashSet};
use std::path::Path;

const SEP: char = std::path::MAIN_SEPARATOR;

/// Minimum confidence for a recognised set to be protected/routed.
pub const SET_THRESHOLD: f64 = 0.60;
/// Default minimum matching files for an unlearned set or photo subject.
pub const MIN_SET_FILES: usize = 3;
/// Window within which two files in one folder count as "produced together".
const PRODUCED_TOGETHER_WINDOW_MS: i64 = 86_400_000;

const PHOTO_EXTS: [&str; 9] = ["jpg", "jpeg", "heic", "png", "tif", "tiff", "raw", "cr2", "nef"];
const EXTRA_IMG_EXTS: [&str; 2] = ["webp", "gif"];
const DESIGN_SRC_EXTS: [&str; 11] = ["xcf", "psd", "ai", "fig", "svg", "eps", "indd", "cdr", "sketch", "afdesign", "afphoto"];
const PRINT_ARCH_EXTS: [&str; 4] = ["pdf", "zip", "rar", "7z"];
const AUDIO_TRACK_EXTS: [&str; 7] = ["mp3", "flac", "m4a", "wav", "ogg", "wma", "aac"];
const VIDEO_EXTS: [&str; 7] = ["mp4", "mkv", "avi", "mov", "wmv", "m4v", "webm"];
const SOFTWARE_BUNDLE_EXTS: [&str; 10] = ["dll", "jar", "cab", "class", "so", "apk", "ocx", "sys", "exe", "msi"];
const GENERAL_SUBFOLDERS: [&str; 6] = ["documents", "downloads", "pictures", "desktop", "videos", "music"];
const NON_USER_PATH_SEGMENTS: [&str; 12] = [
    "plugins",
    "resources",
    "device-art-resources",
    "drawable",
    "mipmap",
    "wp-content",
    "node_modules",
    "site-packages",
    "activator",
    "crack",
    "cracks",
    "program files",
];

const BRAND_PRINT_WORDS: [&str; 9] = ["cmyk", "sleeve", "dpi", "logo", "banner", "mockup", "wireframe", "favicon", "qr"];

/// Words that never start a set subject on their own (spec §10.1, §17).
const GENERIC_WORDS: [&str; 76] = [
    "img", "image", "images", "dsc", "dscn", "dscf", "pxl", "mvimg", "vid", "mov",
    "whatsapp", "wa", "screenshot", "screenshots", "screen", "snip", "capture", "scan",
    "scanned", "vlcsnap", "photo", "photos", "pic", "pics", "picture", "pictures",
    "camera", "dcim", "document", "documents", "doc", "docs", "untitled", "new",
    "copy", "final", "file", "files", "download", "downloads", "temp", "tmp",
    "backup", "old", "draft", "test", "sample", "misc", "other", "export", "output",
    "thumb", "thumbs", "thumbnail", "icon", "icons", "the", "my", "a", "an", "top",
    "best", "all", "jan", "feb", "mar", "apr", "may", "jun", "jul", "aug", "sep",
    "sept", "oct", "nov", "dec",
];

const EXTRA_GENERIC_DOCS_AND_MONTHS: [&str; 18] = [
    "january", "february", "march", "april", "june", "july", "august", "september",
    "october", "november", "december", "invoice", "receipt", "statement", "certificate",
    "payslip", "tax", "report",
];

const EXTRA_GENERIC_SET_WORDS: [&str; 17] = [
    "albumart", "albumartsmall", "folder", "cover", "fore", "shadow", "power", "volume",
    "product", "suite", "www", "docfile", "drawing", "class", "ass", "assignment", "assignments",
];

/// Second-word qualifiers that are kept inside a 1-word brand set (`SmoothLite Front/Back`)
/// rather than splitting it into `"SmoothLite Front"`.
const POSITIONAL_SECOND_WORDS: [&str; 13] = [
    "front", "back", "side", "page", "part", "copy", "small", "large", "hres", "new", "old", "draft", "final",
];

/// Collection-level fingerprint for a loose set (`set:<normalized subject>`).
pub fn set_fingerprint(subject_key: &str) -> String {
    format!("set:{}", subject_key.to_lowercase())
}

/// Location-level fingerprint for one folder holding loose members of a set.
pub fn location_fingerprint(set_fp: &str, volume_id: &str, dir: &str) -> String {
    format!("{set_fp}|{volume_id}|{}", dir.trim_end_matches(SEP).to_lowercase())
}

/// Deterministic synthetic location id for a folder inside a loose set.
pub fn location_id(collection_id: &str, volume_id: &str, dir: &str) -> String {
    let mut h = blake3::Hasher::new();
    h.update(collection_id.as_bytes());
    h.update(b"|");
    h.update(volume_id.as_bytes());
    h.update(b"|");
    h.update(dir.trim_end_matches(SEP).to_lowercase().as_bytes());
    format!("setloc:{}", &h.finalize().to_hex()[..16])
}

#[derive(Clone)]
struct CandidateFile {
    id: String,
    volume_id: String,
    path: String,
    dir: String,
    name: String,
    ext: String,
    size: i64,
    mtime: i64,
    raw_tokens: Vec<String>,
    norm_tokens: Vec<String>,
}

fn is_ext(ext: &str, list: &[&str]) -> bool {
    list.contains(&ext)
}

fn is_all_set_ext(ext: &str) -> bool {
    is_ext(ext, &PHOTO_EXTS) || is_ext(ext, &EXTRA_IMG_EXTS) || is_ext(ext, &DESIGN_SRC_EXTS) || is_ext(ext, &PRINT_ARCH_EXTS)
}

fn is_generic_word(w: &str) -> bool {
    GENERIC_WORDS.contains(&w) || EXTRA_GENERIC_DOCS_AND_MONTHS.contains(&w) || EXTRA_GENERIC_SET_WORDS.contains(&w)
}

fn is_dimension_token(tok: &str) -> bool {
    let lower = tok.to_lowercase();
    let Some((a, b)) = lower.split_once('x') else {
        return false;
    };
    !a.is_empty() && !b.is_empty() && a.chars().all(|c| c.is_ascii_digit()) && b.chars().all(|c| c.is_ascii_digit())
}

fn is_protected_subfolder(dir: &str, normalized_roots: &[String]) -> bool {
    let d_low = dir.trim_end_matches(SEP).to_lowercase();
    if normalized_roots.iter().any(|r| r == &d_low) {
        return true;
    }
    let leaf = Path::new(dir).file_name().map(|n| n.to_string_lossy().to_string()).unwrap_or_default();
    if leaf.is_empty() {
        return true;
    }
    let llow = leaf.to_lowercase();
    GENERAL_SUBFOLDERS.contains(&llow.as_str()) || generic_photo_folder(&leaf)
}

fn has_non_user_segment(dir: &str, roots: &[(String, Option<String>)], vol: &str) -> bool {
    let matching_root = roots
        .iter()
        .filter(|(r, v)| v.as_deref().is_none_or(|rv| rv == vol) && path_is_within(dir, r, false))
        .max_by_key(|(r, _)| r.len())
        .map(|(r, _)| r.as_str());
    let Some(root) = matching_root else {
        return false;
    };
    let Ok(rel) = Path::new(dir).strip_prefix(Path::new(root)) else {
        return false;
    };
    rel.components().any(|c| {
        let seg = c.as_os_str().to_string_lossy().to_lowercase();
        NON_USER_PATH_SEGMENTS.contains(&seg.as_str())
    })
}

fn in_excluded_tree(vol: &str, dir: &str, excluded_dirs: &HashSet<(String, String)>, excluded_trees: &HashSet<(String, String)>) -> bool {
    let d_low = dir.trim_end_matches(SEP).to_lowercase();
    if excluded_dirs.contains(&(vol.to_string(), d_low)) {
        return true;
    }
    if excluded_trees.is_empty() {
        return false;
    }
    for anc in Path::new(dir).ancestors() {
        let a_low = anc.to_string_lossy().trim_end_matches(SEP).to_lowercase();
        if a_low.is_empty() {
            break;
        }
        if excluded_trees.contains(&(vol.to_string(), a_low)) {
            return true;
        }
    }
    false
}

fn alpha_count(s: &str) -> usize {
    s.chars().filter(|c| c.is_ascii_alphabetic()).count()
}

/// Strips trailing version (`v2`, `v1.1`) and digit suffixes while at least two letters remain.
fn strip_num_suffix(tok: &str) -> String {
    let mut s = tok.to_lowercase();
    loop {
        let mut next = s.as_str();
        if let Some(pos) = next.rfind('v') {
            let after = &next[pos + 1..];
            if !after.is_empty() && after.chars().all(|c| c.is_ascii_digit() || c == '.') && alpha_count(&next[..pos]) >= 2 {
                next = &next[..pos];
            }
        }
        if next.len() == s.len() {
            let trimmed = next.trim_end_matches(|c: char| c.is_ascii_digit());
            if trimmed.len() < next.len() && alpha_count(trimmed) >= 2 {
                next = trimmed;
            }
        }
        if next.len() == s.len() {
            break;
        }
        s = next.to_string();
    }
    s
}

/// Preserves original casing of `raw` up to any stripped version/digit suffix.
fn strip_num_preserve_case(tok: &str) -> String {
    let norm = strip_num_suffix(tok);
    tok.chars().take(norm.chars().count()).collect()
}

fn tokenize_stem(name: &str) -> Vec<String> {
    let stem = name.rsplit_once('.').map_or(name, |(s, _)| s);
    stem.split([' ', '_', '-', '(', ')', '[', ']', '.', ',', '/'])
        .filter(|s| !s.is_empty())
        .map(str::to_string)
        .collect()
}

/// Normalized artwork stem for spotting one piece in multiple formats (`SmoothLite Front (CMYK).pdf`
/// vs `SmoothLite Front (sleeve).png`, or `back cement-3.pdf` vs `back cement-3.png`).
fn artwork_stem(name: &str) -> String {
    let stem = name.rsplit_once('.').map_or(name, |(s, _)| s).to_lowercase();
    let mut cleaned = String::with_capacity(stem.len());
    let mut rest = stem.as_str();
    while let Some(a) = rest.find('(') {
        cleaned.push_str(&rest[..a]);
        if let Some(b) = rest[a..].find(')') {
            let inner = rest[a + 1..a + b].trim();
            let drop_paren = ["cmyk", "rgb", "sleeve", "copy", "final", "edited"].contains(&inner)
                || inner.chars().all(|c| c.is_ascii_digit());
            if !drop_paren {
                cleaned.push(' ');
                cleaned.push_str(inner);
                cleaned.push(' ');
            }
            rest = &rest[a + b + 1..];
        } else {
            cleaned.push_str(&rest[a..]);
            rest = "";
            break;
        }
    }
    cleaned.push_str(rest);
    cleaned
        .split([' ', '_', '-', '(', ')', '[', ']', '.', ',', '/'])
        .filter(|s| !s.is_empty())
        .map(strip_num_suffix)
        .filter(|t| !t.is_empty() && !t.chars().all(|c| c.is_ascii_digit()) && !["dpi", "hres", "small", "large", "fixed", "copy", "final", "v"].contains(&t.as_str()))
        .collect::<Vec<_>>()
        .join(" ")
}

fn title_case_phrase(s: &str) -> String {
    s.split_whitespace()
        .map(|w| {
            let mut ch = w.chars();
            match ch.next() {
                Some(f) => f.to_uppercase().collect::<String>() + ch.as_str(),
                None => String::new(),
            }
        })
        .collect::<Vec<_>>()
        .join(" ")
}

fn display_name_for(raw_phrases: &[String], key: &str) -> String {
    let mut counts: HashMap<String, usize> = HashMap::new();
    for p in raw_phrases {
        *counts.entry(p.clone()).or_default() += 1;
    }
    let best = counts
        .into_iter()
        .max_by(|(ka, va), (kb, vb)| va.cmp(vb).then_with(|| kb.cmp(ka)))
        .map(|(k, _)| k)
        .unwrap_or_else(|| key.to_string());
    if best.chars().any(|c| c.is_uppercase()) {
        best
    } else if key.contains(' ') {
        title_case_phrase(&best)
    } else {
        best
    }
}

fn parent_dir(p: &str) -> String {
    Path::new(p).parent().map(|d| d.to_string_lossy().to_string()).unwrap_or_default()
}

fn answer_for<'a>(decisions: &'a [Decision], kind: DecisionKind, fp: &str) -> Option<&'a Decision> {
    decisions
        .iter()
        .filter(|d| d.kind == kind && d.fingerprint_b.is_none() && fingerprints_match(&d.fingerprint_a, fp))
        .max_by_key(|d| d.at)
}

/// Latest learned subjects parsed from `DecisionKind::LearnedSubject`, keyed by lowercase subject
/// key (`"smoothlite"` / `"back cement"` / `"ipadcover"`), returning `(kind_tag, display_name, at)`
/// where `kind_tag` is `"design"` or `"photo"`.
pub(crate) fn parse_learned_subjects(decisions: &[Decision]) -> HashMap<String, (String, String, i64)> {
    let mut best: HashMap<String, (String, String, i64)> = HashMap::new();
    for d in decisions.iter().filter(|d| d.kind == DecisionKind::LearnedSubject && d.fingerprint_b.is_none()) {
        let Some(key) = d.fingerprint_a.strip_prefix("set:") else { continue };
        let Some((kind_tag, display)) = d.value.split_once(':') else { continue };
        let key = key.trim().to_lowercase();
        if key.is_empty() || display.trim().is_empty() {
            continue;
        }
        let replace = best.get(&key).is_none_or(|(_, _, prev)| d.at >= *prev);
        if replace {
            best.insert(key, (kind_tag.to_string(), display.trim().to_string(), d.at));
        }
    }
    best
}

fn is_not_same_dir(decisions: &[Decision], loc_fp: &str, set_fp: &str, min_after: i64) -> bool {
    decisions.iter().any(|d| {
        d.kind == DecisionKind::NotSameAs
            && d.at >= min_after
            && fingerprints_match(&d.fingerprint_a, loc_fp)
            && d.fingerprint_b.as_deref().is_some_and(|b| fingerprints_match(b, set_fp))
            && d.value == "no"
    })
}

fn has_produced_together(files: &[&CandidateFile]) -> bool {
    let mut by_dir: HashMap<String, Vec<i64>> = HashMap::new();
    for f in files {
        by_dir.entry(f.dir.to_lowercase()).or_default().push(f.mtime);
    }
    for mut mtimes in by_dir.into_values() {
        if mtimes.len() < 2 {
            continue;
        }
        mtimes.sort_unstable();
        if mtimes.windows(2).any(|w| (w[1] - w[0]).abs() <= PRODUCED_TOGETHER_WINDOW_MS) {
            return true;
        }
    }
    false
}

fn matched_phrase_for_file(
    f: &CandidateFile,
    learned: &HashMap<String, (String, String, i64)>,
    vol_designs: &[(String, String)],
) -> Option<String> {
    let max_k = f.norm_tokens.len().min(3);
    for k in (1..=max_k).rev() {
        let phrase = f.norm_tokens[..k].join(" ");
        if alpha_count(&phrase) < 3 {
            continue;
        }
        if learned.contains_key(&phrase) {
            return Some(phrase);
        }
        let pnorm = normalized_name(&phrase);
        if let Some((proj_key, _)) = vol_designs.iter().find(|(pn, _)| *pn == pnorm && !pn.is_empty()) {
            // Only match when either exact phrase matches or single-word token matches normalized project name.
            if k == 1 || f.norm_tokens.len() >= k {
                return Some(if phrase.contains(' ') { phrase } else { proj_key.clone() });
            }
        }
    }
    None
}

/// Recognised sets `(collection, set_fingerprint)` and their loose `SetMember` rows.
pub type FoundSets = (Vec<(Collection, Option<String>)>, Vec<SetMember>);

/// Finds all design sets and photo subjects among loose files outside folder projects and named
/// photo albums. Never reads file payloads or touches the disk.
pub fn find_sets(
    store: &SqliteStore,
    roots: &[(String, Option<String>)],
    project_roots: &[(Option<String>, String)],
    existing_design_projects: &HashMap<String, Vec<(String, String)>>,
    decisions: &[Decision],
    online: &HashMap<String, bool>,
    runs: &[PlanRun],
) -> Result<FoundSets> {
    let filter = live_under_root_sql().replace("deleted_at", "f.deleted_at")
        .replace("container_file_id", "f.container_file_id")
        .replace("state != 'quarantine'", "f.state NOT IN ('quarantine', 'missing')")
        .replace("substr(lower(path)", "substr(lower(f.path)")
        .replace("lower(COALESCE(ext", "lower(COALESCE(f.ext")
        .replace("instr(substr(lower(path)", "instr(substr(lower(f.path)");
    #[derive(Default)]
    struct FolderSignals {
        dir: String,
        has_audio: bool,
        video_count: usize,
        has_software_bundle: bool,
    }
    let ext_sql: String = PHOTO_EXTS
        .iter()
        .chain(EXTRA_IMG_EXTS.iter())
        .chain(DESIGN_SRC_EXTS.iter())
        .chain(PRINT_ARCH_EXTS.iter())
        .chain(AUDIO_TRACK_EXTS.iter())
        .chain(VIDEO_EXTS.iter())
        .chain(SOFTWARE_BUNDLE_EXTS.iter())
        .map(|e| format!("'{e}'"))
        .collect::<Vec<_>>()
        .join(", ");

    let mut proj_map: HashMap<String, Vec<Option<String>>> = HashMap::new();
    for (pv, pr) in project_roots {
        let key = pr.strip_prefix(r"\\?\").unwrap_or(pr).replace('/', "\\").trim_end_matches(SEP).to_lowercase();
        proj_map.entry(key).or_default().push(pv.clone());
    }
    let in_project_root = |path: &str, vol: &str| -> bool {
        if proj_map.is_empty() {
            return false;
        }
        let norm = path.strip_prefix(r"\\?\").unwrap_or(path);
        let mut low = norm.trim_end_matches(SEP).to_lowercase();
        if low.contains('/') {
            low = low.replace('/', "\\");
        }
        let mut cur = low.as_str();
        loop {
            if let Some(vols) = proj_map.get(cur)
                && vols.iter().any(|v| v.as_deref().is_none_or(|rv| rv == vol))
            {
                return true;
            }
            let Some(pos) = cur.rfind(SEP) else { break };
            if pos == 0 {
                break;
            }
            cur = &cur[..pos];
        }
        false
    };

    let mut seen_ids: HashSet<String> = HashSet::new();
    let mut raw_files: Vec<CandidateFile> = Vec::new();
    let mut folder_signals: HashMap<(String, String), FolderSignals> = HashMap::new();
    let sql = format!(
        "SELECT f.id, f.volume_id, f.path, f.name, lower(COALESCE(f.ext, '')), f.size, f.mtime
         FROM files f LEFT JOIN volumes v ON v.id = f.volume_id
         WHERE {filter}
           AND COALESCE(v.role, 'unassigned') != 'backup'
           AND f.atomic_unit_id IS NULL
           AND (?3 IS NULL OR f.volume_id = ?3)
           AND lower(COALESCE(f.ext, '')) IN ({ext_sql})"
    );
    let mut stmt = store.conn().prepare(&sql)?;
    let normalized_roots: Vec<String> = roots.iter().map(|(r, _)| r.trim_end_matches(SEP).to_lowercase()).collect();

    for (root, volume) in roots {
        let root_t = root.trim_end_matches(SEP).to_string();
        let prefix = format!("{}{SEP}", root_t.to_lowercase());
        let plen = prefix.chars().count() as i64;
        let rows = stmt.query_map(params![prefix, plen, volume], |r| {
            Ok((
                r.get::<_, String>(0)?,
                r.get::<_, String>(1)?,
                r.get::<_, String>(2)?,
                r.get::<_, String>(3)?,
                r.get::<_, String>(4)?,
                r.get::<_, i64>(5)?,
                r.get::<_, i64>(6)?,
            ))
        })?;
        for row in rows {
            let (id, vol, path, name, ext, size, mtime) = row?;
            if in_project_root(&path, &vol) {
                continue;
            }
            if is_all_set_ext(&ext) {
                if !seen_ids.insert(id.clone()) {
                    continue;
                }
                let nlow = name.to_lowercase();
                if nlow.starts_with("~$")
                    || nlow.starts_with('.')
                    || nlow == "desktop.ini"
                    || nlow == "thumbs.db"
                    || nlow.ends_with(".aegis-partial")
                    || nlow.ends_with(".9.png")
                {
                    continue;
                }
                let dir = parent_dir(&path);
                let raw_tokens = tokenize_stem(&name);
                if raw_tokens.is_empty() || raw_tokens.last().is_some_and(|t| is_dimension_token(t)) {
                    continue;
                }
                let norm_tokens: Vec<String> = raw_tokens.iter().map(|t| strip_num_suffix(t)).collect();
                raw_files.push(CandidateFile { id, volume_id: vol, path, dir, name, ext, size, mtime, raw_tokens, norm_tokens });
            } else {
                let dir = parent_dir(&path);
                let d_low = dir.trim_end_matches(SEP).to_lowercase();
                let e = folder_signals.entry((vol, d_low)).or_default();
                if e.dir.is_empty() {
                    e.dir = dir;
                }
                if is_ext(&ext, &AUDIO_TRACK_EXTS) {
                    e.has_audio = true;
                } else if is_ext(&ext, &VIDEO_EXTS) {
                    e.video_count += 1;
                } else if is_ext(&ext, &SOFTWARE_BUNDLE_EXTS) {
                    e.has_software_bundle = true;
                }
            }
        }
    }

    // Named photo albums (>= 10 photos in a non-generic subfolder at depth >= 1 below its root)
    // stay intact under `filing::blueprint::find_groups` and are never split into subjects.
    let mut dir_photo_counts: HashMap<(String, String), (usize, String)> = HashMap::new();
    for f in &raw_files {
        if is_ext(&f.ext, &PHOTO_EXTS) {
            let key = (f.volume_id.clone(), f.dir.trim_end_matches(SEP).to_lowercase());
            let entry = dir_photo_counts.entry(key).or_insert_with(|| (0, f.dir.clone()));
            entry.0 += 1;
        }
    }
    let named_albums: HashSet<(String, String)> = dir_photo_counts
        .iter()
        .filter(|((_, d_low), (count, d_disp))| {
            let is_root = normalized_roots.iter().any(|r| r == d_low);
            let leaf = Path::new(d_disp).file_name().map(|n| n.to_string_lossy().to_string()).unwrap_or_default();
            *count >= 10 && !is_root && !leaf.is_empty() && !generic_photo_folder(&leaf)
        })
        .map(|(k, _)| k.clone())
        .collect();

    let mut excluded_dirs: HashSet<(String, String)> = HashSet::new();
    let mut excluded_trees: HashSet<(String, String)> = HashSet::new();
    for ((vol, d_low), sig) in folder_signals {
        if is_protected_subfolder(&sig.dir, &normalized_roots) {
            continue;
        }
        if sig.has_software_bundle {
            excluded_trees.insert((vol.clone(), d_low.clone()));
        }
        let photo_count = dir_photo_counts.get(&(vol.clone(), d_low.clone())).map_or(0, |(c, _)| *c);
        if sig.has_audio || (sig.video_count >= 1 && photo_count <= 3) {
            excluded_dirs.insert((vol, d_low));
        }
    }

    let eligible: Vec<&CandidateFile> = raw_files
        .iter()
        .filter(|f| {
            let d_low = f.dir.trim_end_matches(SEP).to_lowercase();
            !named_albums.contains(&(f.volume_id.clone(), d_low))
                && !has_non_user_segment(&f.dir, roots, &f.volume_id)
                && !in_excluded_tree(&f.volume_id, &f.dir, &excluded_dirs, &excluded_trees)
        })
        .collect();

    let learned = parse_learned_subjects(decisions);

    // Bucket by (volume_id, key). Known learned/existing-design multi-word phrases are matched first.
    let mut initial: BTreeMap<(String, String), Vec<&CandidateFile>> = BTreeMap::new();
    for f in eligible {
        let vol_designs = existing_design_projects.get(&f.volume_id).map(Vec::as_slice).unwrap_or(&[]);
        if let Some(known_key) = matched_phrase_for_file(f, &learned, vol_designs) {
            initial.entry((f.volume_id.clone(), known_key)).or_default().push(f);
            continue;
        }
        let w0 = f.norm_tokens.first().map(String::as_str).unwrap_or("");
        if alpha_count(w0) < 3 || is_generic_word(w0) {
            continue;
        }
        initial.entry((f.volume_id.clone(), w0.to_string())).or_default().push(f);
    }

    // Refine 1-word buckets into 2-word phrases when meaningful non-positional second words co-occur
    // (`Back Cement`), while leaving positional qualifiers (`SmoothLite Front`/`Back`) grouped together.
    let mut refined: Vec<(String, String, Vec<&CandidateFile>)> = Vec::new();
    for ((vol, key), items) in initial {
        if key.contains(' ') {
            refined.push((vol, key, items));
            continue;
        }
        let min_needed = if learned.contains_key(&key)
            || existing_design_projects
                .get(&vol)
                .is_some_and(|ds| ds.iter().any(|(dn, _)| *dn == normalized_name(&key)))
        {
            1
        } else {
            2
        };
        if items.len() < min_needed {
            continue;
        }
        let mut by_w1: BTreeMap<String, Vec<&CandidateFile>> = BTreeMap::new();
        for f in &items {
            if let Some(w1) = f.norm_tokens.get(1)
                && alpha_count(w1) >= 3
                && !is_generic_word(w1)
                && !POSITIONAL_SECOND_WORDS.contains(&w1.as_str())
            {
                by_w1.entry(w1.clone()).or_default().push(*f);
            }
        }
        let dominant = by_w1
            .iter()
            .filter(|(_, g)| g.len() >= MIN_SET_FILES && g.len() * 10 >= items.len() * 7)
            .max_by_key(|(_, g)| g.len())
            .map(|(w1, _)| w1.clone());
        if let Some(w1) = dominant {
            let group = by_w1.remove(&w1).unwrap_or_default();
            refined.push((vol, format!("{key} {w1}"), group));
            continue;
        }
        let mut claimed: HashSet<String> = HashSet::new();
        for (w1, group) in &by_w1 {
            if group.len() >= MIN_SET_FILES {
                for f in group {
                    claimed.insert(f.id.clone());
                }
                refined.push((vol.clone(), format!("{key} {w1}"), group.clone()));
            }
        }
        let rem: Vec<&CandidateFile> = items.into_iter().filter(|f| !claimed.contains(&f.id)).collect();
        if !rem.is_empty() {
            refined.push((vol, key, rem));
        }
    }

    let mut out_collections: Vec<(Collection, Option<String>)> = Vec::new();
    let mut out_members: Vec<SetMember> = Vec::new();
    let mut claimed_files: HashSet<String> = HashSet::new();

    for (vol, subject_key, mut files) in refined {
        if !subject_key.contains(' ') && POSITIONAL_SECOND_WORDS.contains(&subject_key.as_str()) {
            continue;
        }
        files.retain(|f| !claimed_files.contains(&f.id));
        if files.is_empty() {
            continue;
        }
        let set_fp = set_fingerprint(&subject_key);
        let is_proj_ans = answer_for(decisions, DecisionKind::IsProject, &set_fp);
        if is_proj_ans.is_some_and(|d| d.value == "no") {
            continue;
        }
        let min_after_yes = is_proj_ans.filter(|d| d.value == "yes").map_or(i64::MIN, |d| d.at);
        files.retain(|f| {
            let lfp = location_fingerprint(&set_fp, &f.volume_id, &f.dir);
            !is_not_same_dir(decisions, &lfp, &set_fp, min_after_yes)
        });
        if files.is_empty() {
            continue;
        }

        let learned_info = learned.get(&subject_key).filter(|(_, _, at)| {
            is_proj_ans.is_none_or(|d| d.value == "yes" || *at >= d.at)
        });
        let existing_design = existing_design_projects
            .get(&vol)
            .and_then(|ds| ds.iter().find(|(dn, _)| *dn == normalized_name(&subject_key)));
        let learned_design = learned_info.is_some_and(|(k, _, _)| k == "design");
        let learned_photo = learned_info.is_some_and(|(k, _, _)| k == "photo");

        let n_img = files
            .iter()
            .filter(|f| is_ext(&f.ext, &PHOTO_EXTS) || is_ext(&f.ext, &EXTRA_IMG_EXTS))
            .count();
        let n_design_src = files.iter().filter(|f| is_ext(&f.ext, &DESIGN_SRC_EXTS)).count();
        let n_print_arch = files.iter().filter(|f| is_ext(&f.ext, &PRINT_ARCH_EXTS)).count();
        let mut by_art_stem: HashMap<String, (HashSet<&str>, HashSet<String>)> = HashMap::new();
        let mut distinct_names: HashSet<String> = HashSet::new();
        for f in &files {
            let nlow = f.name.to_lowercase();
            distinct_names.insert(nlow.clone());
            let entry = by_art_stem.entry(artwork_stem(&f.name)).or_default();
            entry.0.insert(f.ext.as_str());
            entry.1.insert(nlow);
        }
        let mut multi_format_stems: HashSet<String> = HashSet::new();
        let mut multi_format_names: HashSet<String> = HashSet::new();
        for (astem, (exts, fnames)) in &by_art_stem {
            let bridges = exts.len() >= 2
                && exts.iter().any(|e| is_ext(e, &DESIGN_SRC_EXTS) || is_ext(e, &PRINT_ARCH_EXTS))
                && exts.iter().any(|e| is_ext(e, &PHOTO_EXTS) || is_ext(e, &EXTRA_IMG_EXTS) || is_ext(e, &DESIGN_SRC_EXTS));
            let concise_or_src = exts.iter().any(|e| is_ext(e, &DESIGN_SRC_EXTS)) || astem.split_whitespace().count() <= 5;
            if bridges && concise_or_src {
                multi_format_stems.insert(astem.clone());
                multi_format_names.extend(fnames.iter().cloned());
            }
        }
        let shared_artwork_formats = !multi_format_stems.is_empty();
        let word_count = subject_key.split_whitespace().count();
        let has_brand_words = files.iter().any(|f| {
            f.raw_tokens
                .iter()
                .any(|t| BRAND_PRINT_WORDS.contains(&t.to_lowercase().as_str()))
        });
        let min_design = if learned_design || existing_design.is_some() {
            1
        } else if n_design_src >= 1 && shared_artwork_formats {
            2
        } else {
            MIN_SET_FILES
        };
        let distinct_ok_design = learned_design || existing_design.is_some() || distinct_names.len() >= min_design;
        let cohesion_ok = if n_design_src == 0 && !has_brand_words && !learned_design && existing_design.is_none() {
            let sufficient_multi = multi_format_names.len() >= 3 || multi_format_stems.len() >= 2;
            let mut cohesive_names: HashSet<String> = HashSet::new();
            for f in &files {
                let astem = artwork_stem(&f.name);
                if multi_format_stems.contains(&astem) || f.norm_tokens.len() <= word_count + 2 {
                    cohesive_names.insert(f.name.to_lowercase());
                }
            }
            sufficient_multi && cohesive_names.len() * 10 >= distinct_names.len() * 6
        } else {
            true
        };

        let is_design = (n_img + n_design_src) >= 1
            && (n_design_src >= 1 || shared_artwork_formats || (n_print_arch >= 1 && has_brand_words) || learned_design || existing_design.is_some())
            && files.len() >= min_design
            && distinct_ok_design
            && cohesion_ok;

        let phrase_for = |f: &&CandidateFile| -> String {
            f.raw_tokens
                .iter()
                .take(word_count)
                .map(|t| strip_num_preserve_case(t))
                .collect::<Vec<_>>()
                .join(" ")
        };

        let name_override = answer_for(decisions, DecisionKind::Name, &set_fp).map(|d| d.value.clone());
        let kind_override = answer_for(decisions, DecisionKind::ProjectKind, &set_fp).map(|d| ProjectKind::from_db(&d.value));
        let leave_as_is = answer_for(decisions, DecisionKind::LeaveAsIs, &set_fp).is_some_and(|d| d.value == "yes");
        let confirmed_yes = is_proj_ans.is_some_and(|d| d.value == "yes");
        let produced_together = has_produced_together(&files);

        if is_design {
            let phrases: Vec<String> = files.iter().map(phrase_for).collect();
            let display = name_override
                .or_else(|| learned_info.map(|(_, d, _)| d.clone()))
                .unwrap_or_else(|| display_name_for(&phrases, &subject_key));
            let mut conf: f64 = 0.70;
            if shared_artwork_formats {
                conf += 0.15;
            }
            if n_design_src >= 1 {
                conf += 0.10;
            }
            if has_brand_words {
                conf += 0.05;
            }
            if files.len() >= 6 {
                conf += 0.05;
            }
            if produced_together {
                conf += 0.05;
            }
            if confirmed_yes || learned_design || existing_design.is_some() {
                conf = conf.max(0.85);
            }
            conf = (conf.min(0.95) * 100.0).round() / 100.0;
            if conf < SET_THRESHOLD {
                continue;
            }
            let state = if leave_as_is {
                "left_as_is"
            } else if confirmed_yes || learned_design || existing_design.is_some() {
                "confirmed"
            } else {
                "candidate"
            };
            let cid = new_id();
            let locations = build_locations(&cid, &set_fp, &files, decisions, online);
            let mut evidence = vec![format!(
                "{} loose design & print files sharing \"{}\" across {} {}",
                files.len(),
                display,
                locations.len(),
                if locations.len() == 1 { "folder" } else { "folders" }
            )];
            if shared_artwork_formats {
                evidence.push("same artwork across image, design or print formats".into());
            }
            if n_design_src > 0 {
                evidence.push(format!("includes {n_design_src} layered design file(s)"));
            }
            if has_brand_words {
                evidence.push("carries print/brand qualifiers (CMYK, sleeve, QR, mockup)".into());
            }
            if let Some((_, existing_path)) = existing_design {
                evidence.push(format!("matches existing design project at {existing_path}"));
            }
            let plan_state = runs
                .iter()
                .rev()
                .find(|r| r.volume_id == vol && r.fingerprints.iter().any(|f| fingerprints_match(f, &set_fp)))
                .map(|r| format!("{}:{}", r.state, r.batch_id));
            for f in &files {
                claimed_files.insert(f.id.clone());
                out_members.push(SetMember {
                    collection_id: cid.clone(),
                    file_id: f.id.clone(),
                    volume_id: f.volume_id.clone(),
                    path: f.path.clone(),
                    size: f.size,
                    mtime: f.mtime,
                });
            }
            out_collections.push((
                Collection {
                    id: cid,
                    kind: CollectionKind::DesignSet,
                    name: display,
                    project_kind: Some(kind_override.unwrap_or(ProjectKind::Design)),
                    confidence: conf,
                    state: state.into(),
                    evidence,
                    locations,
                    sub_projects: Vec::new(),
                    plan_state,
                },
                Some(set_fp),
            ));
            continue;
        }

        // Otherwise check PhotoSubject: loose photos sharing a subject outside projects and named albums.
        let photos: Vec<&CandidateFile> = files
            .into_iter()
            .filter(|f| is_ext(&f.ext, &PHOTO_EXTS))
            .collect();
        let min_photo = if learned_photo { 1 } else { MIN_SET_FILES };
        let distinct_photo_names: HashSet<String> = photos.iter().map(|f| f.name.to_lowercase()).collect();
        if photos.len() < min_photo || (!learned_photo && distinct_photo_names.len() < MIN_SET_FILES) {
            continue;
        }
        let phrases: Vec<String> = photos.iter().map(phrase_for).collect();
        let display = name_override
            .or_else(|| learned_info.map(|(_, d, _)| d.clone()))
            .unwrap_or_else(|| display_name_for(&phrases, &subject_key));
        let mut conf: f64 = 0.80;
        if photos.len() >= 5 {
            conf += 0.05;
        }
        if produced_together {
            conf += 0.05;
        }
        conf = (conf.min(0.90) * 100.0).round() / 100.0;
        let state = if leave_as_is {
            "left_as_is"
        } else if confirmed_yes || learned_photo {
            "confirmed"
        } else {
            "candidate"
        };
        let cid = new_id();
        let locations = build_locations(&cid, &set_fp, &photos, decisions, online);
        let evidence = vec![format!(
            "{} photos sharing subject \"{}\" across {} {}",
            photos.len(),
            display,
            locations.len(),
            if locations.len() == 1 { "folder" } else { "folders" }
        )];
        for f in &photos {
            claimed_files.insert(f.id.clone());
            out_members.push(SetMember {
                collection_id: cid.clone(),
                file_id: f.id.clone(),
                volume_id: f.volume_id.clone(),
                path: f.path.clone(),
                size: f.size,
                mtime: f.mtime,
            });
        }
        out_collections.push((
            Collection {
                id: cid,
                kind: CollectionKind::PhotoSubject,
                name: display,
                project_kind: None,
                confidence: conf,
                state: state.into(),
                evidence,
                locations,
                sub_projects: Vec::new(),
                plan_state: None,
            },
            Some(set_fp),
        ));
    }

    Ok((out_collections, out_members))
}

fn build_locations(
    collection_id: &str,
    set_fp: &str,
    files: &[&CandidateFile],
    decisions: &[Decision],
    online: &HashMap<String, bool>,
) -> Vec<Location> {
    #[derive(Default)]
    struct Acc {
        volume_id: String,
        dir: String,
        files: i64,
        bytes: i64,
        newest: i64,
    }
    let mut by_dir: BTreeMap<(String, String), Acc> = BTreeMap::new();
    for f in files {
        let key = (f.volume_id.to_lowercase(), f.dir.to_lowercase());
        let e = by_dir.entry(key).or_default();
        if e.dir.is_empty() {
            e.volume_id = f.volume_id.clone();
            e.dir = f.dir.clone();
        }
        e.files += 1;
        e.bytes += f.size;
        e.newest = e.newest.max(f.mtime);
    }
    let mut locs: Vec<Location> = by_dir
        .into_values()
        .map(|a| {
            let loc_fp = location_fingerprint(set_fp, &a.volume_id, &a.dir);
            let keep_here = answer_for(decisions, DecisionKind::KeepHere, &loc_fp).is_some_and(|d| d.value == "yes");
            Location {
                id: location_id(collection_id, &a.volume_id, &a.dir),
                collection_id: collection_id.to_string(),
                volume_id: a.volume_id.clone(),
                root_path: a.dir,
                relationship: Relationship::SetFiles,
                files: a.files,
                bytes: a.bytes,
                newest_mtime: a.newest,
                shared_files: a.files,
                compared_by: "name".into(),
                is_repo: false,
                online: online.get(&a.volume_id).copied().unwrap_or(false),
                fingerprint: loc_fp,
                keep_here,
            }
        })
        .collect();
    locs.sort_by_key(|l| (std::cmp::Reverse(l.files), l.root_path.to_lowercase()));
    locs
}

/// On-the-spot fallback when `recognise` hasn't run yet: finds sets within `roots` without
/// writing to the database.
pub fn find_sets_for_roots(store: &SqliteStore, roots: &[String]) -> Result<FoundSets> {
    let projs: Vec<(Option<String>, String)> = super::protected_roots(store, roots)?
        .into_iter()
        .map(|(p, _)| (None, p))
        .collect();
    let root_tuples: Vec<(String, Option<String>)> = roots.iter().map(|r| (r.clone(), None)).collect();
    let decisions = super::store::decisions(store.conn())?;
    let online: HashMap<String, bool> = {
        let mut stmt = store.conn().prepare("SELECT id, status = 'online' FROM volumes")?;
        stmt.query_map([], |r| Ok((r.get(0)?, r.get(1)?)))?.collect::<rusqlite::Result<_>>()?
    };
    let runs = super::store::plan_runs(store.conn())?;
    find_sets(store, &root_tuples, &projs, &HashMap::new(), &decisions, &online, &runs)
}
