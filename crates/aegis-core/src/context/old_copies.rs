//! Old copies module (spec 2026-11-22 §1A, §16 step 7): recognises copies of another PC's or
//! earlier backup's folders (`MSIDocuments`, `MSIDownloads`, `Backup of ...`), compares them
//! against live folders using `context::relate`, and routes them either to verified quarantine
//! (same drive only) or into `Archive\<device or year>\<name>`.

use super::profile::FolderProfile;
use super::projects::{AREA_NAMES, LIFE_AREAS};
use super::relate::{ContentSummary, content_summaries, fingerprint, fingerprints_match, relate};
use super::store::PlanRun;
use super::{Collection, CollectionKind, Decision, DecisionKind, Location, Relationship};
use crate::db::SqliteStore;
use crate::db::files::path_is_within;
use crate::error::Result;
use crate::new_id;
use std::collections::{BTreeMap, HashMap, HashSet};

const SEP: char = std::path::MAIN_SEPARATOR;

const KNOWN_AREA_SUFFIXES: [&str; 7] = ["documents", "downloads", "pictures", "photos", "desktop", "videos", "music"];
const NON_DEVICE_PREFIXES: [&str; 12] = [
    "my", "other", "all", "new", "work", "personal", "study", "finance", "shared", "public", "saved", "camera",
];
const EXPLICIT_BACKUP_PREFIXES: [&str; 6] = ["backup of ", "copy of ", "old ", "backup ", "backup_", "backup-"];
const EXPLICIT_BACKUP_EXACT: [&str; 5] = ["pc backup", "laptop backup", "old pc", "old laptop", "phone backup"];

/// An old-copy / backup folder recognised at the top level of a root or drive.
#[derive(Debug, Clone, PartialEq)]
pub struct OldCopyCandidate {
    pub root: String,
    pub volume_id: String,
    pub display_name: String,
    pub archive_rel: String,
    pub target_area: Option<String>,
    pub files: i64,
    pub bytes: i64,
    pub newest_mtime: i64,
    pub confidence: f64,
    pub evidence: Vec<String>,
}

fn title_word(s: &str) -> String {
    let mut c = s.chars();
    c.next().map(|f| f.to_uppercase().collect::<String>() + c.as_str()).unwrap_or_default()
}

fn canonical_area(area: &str) -> &'static str {
    match area {
        "documents" => "documents",
        "downloads" => "downloads",
        "pictures" | "photos" => "pictures",
        "desktop" => "desktop",
        "videos" => "videos",
        "music" => "music",
        _ => "documents",
    }
}

fn safe_segment(name: &str) -> String {
    let s: String = name.chars().map(|c| if r#"<>:"/\|?*"#.contains(c) || c.is_control() { '_' } else { c }).collect();
    let s = s.trim().trim_end_matches(['.', ' ']).to_string();
    if s.is_empty() { "Backup".into() } else { s }
}

fn year_from_ms(ms: i64) -> String {
    if ms > 0 {
        let local = crate::filing::local_ms(ms);
        crate::engine::note::date_of(local.div_euclid(86_400_000))[..4].to_string()
    } else {
        "Backups".into()
    }
}

/// Parses a folder name such as `MSIDocuments` or `Backup of Desktop` into
/// `(display_name, archive_rel, target_area)`.
pub fn parse_old_copy_name(name: &str, newest_mtime: i64) -> Option<(String, String, Option<String>)> {
    let trimmed = name.trim();
    if trimmed.is_empty() {
        return None;
    }
    let lower = trimmed.to_lowercase();
    if AREA_NAMES.contains(&lower.as_str()) || LIFE_AREAS.contains(&lower.as_str()) {
        return None;
    }

    for pref in EXPLICIT_BACKUP_PREFIXES {
        if let Some(rest_low) = lower.strip_prefix(pref) {
            let rest_clean = rest_low.trim_matches(|c: char| c == ' ' || c == '_' || c == '-');
            if rest_clean.is_empty() {
                continue;
            }
            if pref == "old "
                && ["project", "repo", "code", "app", "src"].iter().any(|w| rest_clean.contains(w))
            {
                continue;
            }
            let area = KNOWN_AREA_SUFFIXES
                .iter()
                .find(|&&a| rest_clean == a)
                .map(|a| canonical_area(a).to_string());
            let year = year_from_ms(newest_mtime);
            let safe = safe_segment(trimmed);
            return Some((trimmed.to_string(), format!("{year}{SEP}{safe}"), area));
        }
    }
    if EXPLICIT_BACKUP_EXACT.contains(&lower.as_str()) {
        let year = year_from_ms(newest_mtime);
        let safe = safe_segment(trimmed);
        return Some((trimmed.to_string(), format!("{year}{SEP}{safe}"), None));
    }

    for area in KNOWN_AREA_SUFFIXES {
        if let Some(prefix_len) = lower.len().checked_sub(area.len())
            && lower.ends_with(area)
            && prefix_len > 0
            && trimmed.is_char_boundary(prefix_len)
        {
            let raw_prefix = trimmed[..prefix_len].trim_end_matches([' ', '_', '-']);
            if raw_prefix.is_empty() {
                continue;
            }
            let alnum: String = raw_prefix.chars().filter(|c| c.is_alphanumeric()).collect();
            let p_low = alnum.to_lowercase();
            if !(2..=16).contains(&alnum.len())
                || raw_prefix.chars().any(|c| !c.is_alphanumeric() && !matches!(c, ' ' | '_' | '-'))
                || alnum.chars().all(|c| c.is_ascii_digit())
                || NON_DEVICE_PREFIXES.contains(&p_low.as_str())
            {
                continue;
            }
            if matches!(p_low.as_str(), "old" | "backup" | "copy") {
                let year = year_from_ms(newest_mtime);
                let safe = safe_segment(trimmed);
                return Some((trimmed.to_string(), format!("{year}{SEP}{safe}"), Some(canonical_area(area).to_string())));
            }
            let dev = if raw_prefix.chars().any(|c| c.is_uppercase()) {
                raw_prefix.trim().to_string()
            } else {
                title_word(raw_prefix.trim())
            };
            let area_title = title_word(canonical_area(area));
            let display = format!("{dev} {area_title}");
            let rel = format!("{}{SEP}{area_title}", safe_segment(&dev));
            return Some((display, rel, Some(canonical_area(area).to_string())));
        }
    }
    None
}

/// Finds old-copy folders at depth 1 under `known_roots`. Never treats a folder with git,
/// atomic build markers, or manifests (other than `index.html`) as an old copy.
pub fn find_old_copies(profiles: &[FolderProfile], known_roots: &[String]) -> Vec<OldCopyCandidate> {
    let roots_low: Vec<String> = known_roots.iter().map(|r| r.trim_end_matches(SEP).to_lowercase()).collect();
    let mut out = Vec::new();
    for p in profiles {
        if p.depth != 1 || p.files < 3 || p.has_git || p.atomic_unit {
            continue;
        }
        if p.manifests.iter().any(|(m, _)| !m.eq_ignore_ascii_case("index.html")) {
            continue;
        }
        if roots_low.iter().any(|r| r == &p.path.to_lowercase()) {
            continue;
        }
        let Some((display_name, archive_rel, target_area)) = parse_old_copy_name(p.name(), p.newest_mtime) else {
            continue;
        };
        out.push(OldCopyCandidate {
            root: p.path.clone(),
            volume_id: p.volume_id.clone(),
            display_name,
            archive_rel,
            target_area,
            files: p.files,
            bytes: p.bytes,
            newest_mtime: p.newest_mtime,
            confidence: 0.90,
            evidence: vec![format!("old PC or backup copy ({}) with {} files", p.name(), p.files)],
        });
    }
    out.sort_by_key(|c| c.root.to_lowercase());
    out
}

/// Filters `profiles` to remove anything at or inside `excluded_roots`, and trims excluded paths
/// out of `child_dirs` so project recognition never descends into or promotes an `OldCopy` tree.
pub fn filter_profiles_outside(profiles: &[FolderProfile], excluded_roots: &[String]) -> Vec<FolderProfile> {
    if excluded_roots.is_empty() {
        return profiles.to_vec();
    }
    let is_excluded = |p: &str| excluded_roots.iter().any(|ex| path_is_within(p, ex, false));
    profiles
        .iter()
        .filter(|p| !is_excluded(&p.path))
        .cloned()
        .map(|mut p| {
            p.child_dirs.retain(|c| !is_excluded(c));
            p
        })
        .collect()
}

fn leaf_name(p: &str) -> &str {
    p.trim_end_matches(SEP).rsplit(SEP).next().unwrap_or(p)
}

pub type BuiltOldCopies = (Vec<(Collection, Option<String>)>, Vec<String>);

/// Builds `CollectionKind::OldCopy` collections with relationship comparison against live
/// counterparts on the same drive and on the PC.
pub fn build_old_copy_collections(
    store: &SqliteStore,
    all_profiles: &[FolderProfile],
    roots: &[(String, Option<String>)],
    cands: &[OldCopyCandidate],
    decisions: &[Decision],
    online: &HashMap<String, bool>,
    runs: &[PlanRun],
) -> Result<BuiltOldCopies> {
    if cands.is_empty() {
        return Ok((Vec::new(), Vec::new()));
    }

    // Collect candidate live folders for each OldCopy:
    // 1. Same-volume live area or matching-name folder (depth <= 1, not an OldCopy itself)
    // 2. Cross-volume PC root or matching-name folder matching `target_area`
    let old_set: HashSet<(String, String)> = cands.iter().map(|c| (c.volume_id.clone(), c.root.to_lowercase())).collect();
    let mut live_by_key: BTreeMap<(String, String), (String, String)> = BTreeMap::new();
    let mut cand_pairs: Vec<Vec<(String, String, bool)>> = Vec::with_capacity(cands.len());

    let pc_roots: Vec<&FolderProfile> = all_profiles
        .iter()
        .filter(|p| {
            roots
                .iter()
                .any(|(r, v)| v.is_none() && r.trim_end_matches(SEP).eq_ignore_ascii_case(p.path.trim_end_matches(SEP)))
        })
        .collect();

    for c in cands {
        let mut opts: Vec<(String, String, bool)> = Vec::new();
        if let Some(area) = &c.target_area {
            // Same-volume live folder matching target area at depth 1 on `c.volume_id` (e.g. `D:\Documents`)
            for p in all_profiles.iter().filter(|p| {
                p.volume_id == c.volume_id
                    && p.depth == 1
                    && !old_set.contains(&(p.volume_id.clone(), p.path.to_lowercase()))
                    && p.name().eq_ignore_ascii_case(area)
            }) {
                let k = (p.volume_id.clone(), p.path.clone());
                live_by_key.entry((p.volume_id.clone(), p.path.to_lowercase())).or_insert_with(|| k.clone());
                opts.push((p.volume_id.clone(), p.path.clone(), true));
            }
            // Cross-volume live PC root matching target area (`C:\Users\Mac\Documents`)
            for p in &pc_roots {
                let n = leaf_name(&p.path).to_lowercase();
                if n == *area || (area == "pictures" && n == "photos") {
                    let k = (p.volume_id.clone(), p.path.clone());
                    live_by_key.entry((p.volume_id.clone(), p.path.to_lowercase())).or_insert_with(|| k.clone());
                    opts.push((p.volume_id.clone(), p.path.clone(), false));
                }
            }
        } else {
            let norm_key = super::relate::normalized_name(&c.display_name);
            if !norm_key.is_empty() {
                for p in all_profiles.iter().filter(|p| {
                    p.depth == 1
                        && !old_set.contains(&(p.volume_id.clone(), p.path.to_lowercase()))
                        && super::relate::normalized_name(p.name()) == norm_key
                }) {
                    let k = (p.volume_id.clone(), p.path.clone());
                    let same_vol = p.volume_id == c.volume_id;
                    live_by_key.entry((p.volume_id.clone(), p.path.to_lowercase())).or_insert_with(|| k.clone());
                    opts.push((p.volume_id.clone(), p.path.clone(), same_vol));
                }
            }
        }
        cand_pairs.push(opts);
    }

    let mut summary_locs: Vec<(String, String)> = cands.iter().map(|c| (c.volume_id.clone(), c.root.clone())).collect();
    let live_start = summary_locs.len();
    let mut live_index: HashMap<(String, String), usize> = HashMap::new();
    for (v, r) in live_by_key.into_values() {
        let idx = summary_locs.len();
        live_index.insert((v.clone(), r.to_lowercase()), idx);
        summary_locs.push((v, r));
    }
    let summaries: Vec<ContentSummary> = content_summaries(store, roots, &summary_locs)?;

    let answer_for = |kind: DecisionKind, fp: &str| {
        decisions
            .iter()
            .filter(|d| d.kind == kind && d.fingerprint_b.is_none() && fingerprints_match(&d.fingerprint_a, fp))
            .max_by_key(|d| d.at)
    };

    let mut out = Vec::new();
    let mut active_roots = Vec::new();
    for (i, c) in cands.iter().enumerate() {
        let old_sum = &summaries[i];
        let fp = fingerprint(&c.display_name, &[], old_sum, &c.volume_id, &c.root);
        if answer_for(DecisionKind::IsProject, &fp).is_some_and(|d| d.value == "no") {
            continue;
        }
        active_roots.push(c.root.clone());

        // Pick best matching live counterpart (prefer same-volume with overlap, else highest overlap).
        let mut best: Option<(bool, String, Relationship, i64, String)> = None;
        for (lv, lpath, same_vol) in &cand_pairs[i] {
            let Some(&idx) = live_index.get(&(lv.clone(), lpath.to_lowercase())) else {
                continue;
            };
            debug_assert!(idx >= live_start);
            let live_sum = &summaries[idx];
            if live_sum.is_empty() {
                continue;
            }
            let (rel, shared, by) = relate(live_sum, old_sum);
            let take = match &best {
                None => true,
                Some((b_same, _, _, b_shared, _)) => (*same_vol && shared > 0 && !*b_same) || (*same_vol == *b_same && shared > *b_shared),
            };
            if take {
                best = Some((*same_vol, lpath.clone(), rel, shared, by));
            }
        }

        let (relationship, shared_files, compared_by, same_vol_live, evidence) = match best {
            Some((same_vol, live_path, rel, shared, by)) => {
                let rel_phrase = match rel {
                    Relationship::Identical => "identical contents",
                    Relationship::OlderSubset => "all files already in live folder",
                    Relationship::Diverged => "changed on both sides",
                    Relationship::DifferentMaterial => "mostly distinct files",
                    _ => "compared",
                };
                let mut ev = c.evidence.clone();
                ev.push(format!("compared with {live_path}: {rel_phrase} ({shared} shared files by {by})"));
                let same_live = if same_vol && matches!(rel, Relationship::Identical | Relationship::OlderSubset) {
                    live_path
                } else {
                    String::new()
                };
                (rel, shared, by, same_live, ev)
            }
            None => (Relationship::Unknown, 0, "name".to_string(), String::new(), c.evidence.clone()),
        };

        let cname = answer_for(DecisionKind::Name, &fp).map_or_else(|| c.display_name.clone(), |d| d.value.clone());
        let state = if answer_for(DecisionKind::LeaveAsIs, &fp).is_some() {
            "left_as_is"
        } else {
            "confirmed"
        };
        let cid = new_id();
        let loc = Location {
            id: new_id(),
            collection_id: cid.clone(),
            volume_id: c.volume_id.clone(),
            root_path: c.root.clone(),
            relationship,
            files: c.files,
            bytes: c.bytes,
            newest_mtime: c.newest_mtime,
            shared_files,
            compared_by,
            is_repo: false,
            online: online.get(&c.volume_id).copied().unwrap_or(false),
            fingerprint: fp.clone(),
            keep_here: answer_for(DecisionKind::KeepHere, &fp).is_some_and(|d| d.value == "yes"),
        };
        let plan_state = runs
            .iter()
            .rev()
            .find(|r| r.fingerprints.iter().any(|f| fingerprints_match(f, &fp)))
            .map(|r| format!("{}:{}", r.state, r.batch_id));
        let col = Collection {
            id: cid,
            kind: CollectionKind::OldCopy,
            name: cname,
            project_kind: Some(super::ProjectKind::Other),
            confidence: c.confidence,
            state: state.into(),
            evidence,
            locations: vec![loc],
            sub_projects: Vec::new(),
            plan_state,
        };
        let col_fp = format!("old_copy:{}|{}", c.archive_rel, same_vol_live);
        out.push((col, Some(col_fp)));
    }
    Ok((out, active_roots))
}
