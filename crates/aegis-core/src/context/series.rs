//! TV series module (spec 2026-11-22 §1A, §16 step 7): groups TV/video episodes (with their
//! companion subtitles) outside claimed folder collections and routes them in Fresh into
//! `Videos\Shows\<Show>\Season <n>`.

use super::profile::BUILD_DIRS;
use super::sets::FoundSets;
use super::store::PlanRun;
use super::{Collection, CollectionKind, Decision, DecisionKind, ProjectKind, SetMember};
use crate::db::SqliteStore;
use crate::db::files::path_is_within;
use crate::error::Result;
use crate::new_id;
use rusqlite::params;
use std::collections::{BTreeMap, HashMap, HashSet};
use std::path::Path;

const SEP: char = std::path::MAIN_SEPARATOR;

const VIDEO_EXTS: [&str; 7] = ["mp4", "mkv", "avi", "mov", "wmv", "m4v", "webm"];
const SUBTITLE_EXTS: [&str; 4] = ["srt", "vtt", "ass", "sub"];
const RELEASE_NOISE_WORDS: [&str; 8] = ["us", "uk", "it", "dub", "dubbed", "sub", "subbed", "eng"];
const GENERIC_SHOW_WORDS: [&str; 8] = ["img", "vid", "dsc", "lesson", "lecture", "module", "chapter", "part"];

pub fn series_fingerprint(norm_show: &str, season: u32) -> String {
    format!("series:{norm_show}|s{season}")
}

fn title_word(w: &str) -> String {
    if w.chars().all(|c| c.is_ascii_digit()) {
        return w.to_string();
    }
    if w.len() <= 3 && w.chars().all(|c| c.is_ascii_uppercase()) {
        return w.to_string();
    }
    let mut c = w.chars();
    c.next()
        .map(|f| f.to_uppercase().collect::<String>() + &c.as_str().to_lowercase())
        .unwrap_or_default()
}

fn clean_show(raw: &str) -> Option<(String, String)> {
    let mut s = String::with_capacity(raw.len());
    for c in raw.chars() {
        if c.is_alphanumeric() || c == '\'' || c == '@' {
            s.push(c);
        } else {
            s.push(' ');
        }
    }
    let mut words: Vec<String> = s
        .split_whitespace()
        .filter(|w| !w.starts_with('@'))
        .map(|w| w.trim_matches('\'').to_string())
        .filter(|w| !w.is_empty())
        .collect();
    if words.is_empty() {
        return None;
    }
    // Strip trailing release/region tags only while a real show title remains in front.
    loop {
        if words.len() <= 1 {
            break;
        }
        let last = words.last().unwrap().to_lowercase();
        let is_year = last.len() == 4
            && last.chars().all(|c| c.is_ascii_digit())
            && (last.starts_with("19") || last.starts_with("20"));
        if is_year || RELEASE_NOISE_WORDS.contains(&last.as_str()) {
            words.pop();
        } else {
            break;
        }
    }
    if words.is_empty() {
        return None;
    }
    let norm = words.iter().map(|w| w.to_lowercase()).collect::<Vec<_>>().join(" ");
    if norm.len() < 2 || GENERIC_SHOW_WORDS.contains(&norm.as_str()) {
        return None;
    }
    let has_mixed = words.iter().any(|w| {
        let up = w.chars().any(|c| c.is_uppercase());
        let lo = w.chars().any(|c| c.is_lowercase());
        up && lo
    });
    let display = if has_mixed {
        words.join(" ")
    } else {
        words.iter().map(|w| title_word(w)).collect::<Vec<_>>().join(" ")
    };
    Some((norm, display))
}

fn find_sxe(text: &str) -> Option<(usize, u32, u32)> {
    let b = text.as_bytes();
    let len = b.len();
    for i in 0..len {
        if !matches!(b[i], b's' | b'S') {
            continue;
        }
        if i > 0 && b[i - 1].is_ascii_alphanumeric() {
            continue;
        }
        let mut j = i + 1;
        let s_start = j;
        while j < len && b[j].is_ascii_digit() && j - s_start < 2 {
            j += 1;
        }
        if j == s_start || (j < len && b[j].is_ascii_digit()) {
            continue;
        }
        while j < len && b[j] == b' ' {
            j += 1;
        }
        if j >= len || !matches!(b[j], b'e' | b'E') {
            continue;
        }
        j += 1;
        while j < len && b[j] == b' ' {
            j += 1;
        }
        let e_start = j;
        while j < len && b[j].is_ascii_digit() && j - e_start < 3 {
            j += 1;
        }
        if j == e_start || (j < len && b[j].is_ascii_alphanumeric()) {
            continue;
        }
        let Ok(season) = text[s_start..s_start + (j.min(len) - s_start).min(2)].trim().parse::<u32>() else {
            continue;
        };
        let s_raw: String = text[s_start..].chars().take_while(|c| c.is_ascii_digit()).collect();
        let e_raw = &text[e_start..j];
        let (Ok(s), Ok(e)) = (s_raw.parse::<u32>(), e_raw.parse::<u32>()) else {
            continue;
        };
        let _ = season;
        if (1..=50).contains(&s) && (1..=300).contains(&e) {
            return Some((i, s, e));
        }
    }
    None
}

fn find_season_episode_words(text: &str) -> Option<(usize, u32, u32)> {
    let lower = text.to_lowercase();
    let idx = lower.find("season")?;
    if idx > 0 && lower.as_bytes()[idx - 1].is_ascii_alphanumeric() {
        return None;
    }
    let after_season = lower[idx + 6..].trim_start_matches([' ', '.', '_', '-']);
    let s_digits: String = after_season.chars().take_while(|c| c.is_ascii_digit()).collect();
    let s: u32 = s_digits.parse().ok()?;
    let rest = after_season[s_digits.len()..].trim_start_matches([' ', '.', '_', '-']);
    let after_ep = rest.strip_prefix("episode").or_else(|| rest.strip_prefix("ep"))?;
    let after_sep = after_ep.trim_start_matches([' ', '.', '_', '-']);
    let e_digits: String = after_sep.chars().take_while(|c| c.is_ascii_digit()).collect();
    let e: u32 = e_digits.parse().ok()?;
    if (1..=50).contains(&s) && (1..=300).contains(&e) {
        Some((idx, s, e))
    } else {
        None
    }
}

fn find_nxnn(text: &str) -> Option<(usize, u32, u32)> {
    let b = text.as_bytes();
    let len = b.len();
    for i in 0..len {
        if !b[i].is_ascii_digit() || (i > 0 && b[i - 1].is_ascii_alphanumeric()) {
            continue;
        }
        let mut j = i;
        while j < len && b[j].is_ascii_digit() && j - i < 2 {
            j += 1;
        }
        if j == i || j >= len || !matches!(b[j], b'x' | b'X') {
            continue;
        }
        let e_start = j + 1;
        let mut k = e_start;
        while k < len && b[k].is_ascii_digit() && k - e_start < 2 {
            k += 1;
        }
        if k - e_start != 2 || (k < len && b[k].is_ascii_alphanumeric()) {
            continue;
        }
        let (Ok(s), Ok(e)) = (text[i..j].parse::<u32>(), text[e_start..k].parse::<u32>()) else {
            continue;
        };
        if (1..=50).contains(&s) && (1..=99).contains(&e) {
            return Some((i, s, e));
        }
    }
    None
}

/// Parses an episode file name (falling back to its parent folder if needed) into
/// `(normalized_show_key, display_show_name, season_number, episode_number)`.
pub fn parse_episode(file_name: &str, parent_dir_name: &str) -> Option<(String, String, u32, u32)> {
    let stem = file_name.rsplit_once('.').map_or(file_name, |(s, _)| s);
    let hit = find_sxe(stem).or_else(|| find_season_episode_words(stem)).or_else(|| find_nxnn(stem));
    let (idx, season, ep, raw_show) = match hit {
        Some((i, s, e)) => {
            let prefix = &stem[..i];
            if prefix.chars().any(|c| c.is_alphanumeric()) {
                (i, s, e, prefix)
            } else {
                (i, s, e, parent_dir_name)
            }
        }
        None => {
            let (i, s, e) = find_sxe(parent_dir_name)
                .or_else(|| find_season_episode_words(parent_dir_name))?;
            (i, s, e, &parent_dir_name[..i])
        }
    };
    let _ = idx;
    let (norm, display) = clean_show(raw_show)?;
    Some((norm, display, season, ep))
}

fn stem_key(name: &str) -> String {
    name.rsplit_once('.').map_or(name, |(s, _)| s).to_lowercase()
}

fn parent_dir(p: &str) -> String {
    Path::new(p).parent().map(|d| d.to_string_lossy().to_string()).unwrap_or_default()
}

fn pick_display(counts: &HashMap<String, usize>, norm: &str) -> String {
    let score = |s: &str| {
        let mixed = s.chars().any(|c| c.is_uppercase()) && s.chars().any(|c| c.is_lowercase());
        let all_cap = s.chars().filter(|c| c.is_alphabetic()).all(|c| c.is_uppercase());
        if mixed { 2u8 } else if !all_cap { 1u8 } else { 0u8 }
    };
    counts
        .iter()
        .max_by(|(sa, ca), (sb, cb)| score(sa).cmp(&score(sb)).then_with(|| ca.cmp(cb)).then_with(|| sb.cmp(sa)))
        .map(|(s, _)| s.clone())
        .unwrap_or_else(|| norm.split_whitespace().map(title_word).collect::<Vec<_>>().join(" "))
}

/// Finds all TV series seasons among live video files outside `claimed_roots`.
pub fn find_series(
    store: &SqliteStore,
    roots: &[(String, Option<String>)],
    claimed_roots: &[(Option<String>, String)],
    decisions: &[Decision],
    _online: &HashMap<String, bool>,
    _runs: &[PlanRun],
) -> Result<FoundSets> {
    let has_build_dir = |path: &str, root: &str| -> bool {
        let Ok(rel) = Path::new(path).strip_prefix(Path::new(root)) else {
            return false;
        };
        rel.parent()
            .into_iter()
            .flat_map(|p| p.components())
            .any(|c| {
                let s = c.as_os_str().to_string_lossy();
                BUILD_DIRS.iter().any(|b| s.eq_ignore_ascii_case(b))
            })
    };
    let ext_sql = VIDEO_EXTS
        .iter()
        .chain(SUBTITLE_EXTS.iter())
        .map(|e| format!("'{e}'"))
        .collect::<Vec<_>>()
        .join(", ");
    let sql = format!(
        "SELECT f.id, f.volume_id, f.path, f.name, lower(f.ext), f.size, f.mtime
         FROM files f LEFT JOIN volumes v ON v.id = f.volume_id
         WHERE f.deleted_at IS NULL
           AND f.container_file_id IS NULL
           AND f.state NOT IN ('quarantine', 'missing')
           AND f.atomic_unit_id IS NULL
           AND lower(COALESCE(f.ext, '')) IN ({ext_sql})
           AND COALESCE(v.role, 'unassigned') != 'backup'
           AND (?3 IS NULL OR f.volume_id = ?3)
           AND substr(lower(f.path), 1, ?2) = ?1"
    );
    let in_claimed = |path: &str, vol: &str| {
        claimed_roots
            .iter()
            .any(|(cv, cr)| cv.as_deref().is_none_or(|v| v == vol) && path_is_within(path, cr, false))
    };

    #[derive(Clone)]
    struct Ep {
        id: String,
        volume_id: String,
        path: String,
        dir_low: String,
        stem_low: String,
        size: i64,
        mtime: i64,
        norm: String,
        display: String,
        season: u32,
    }
    struct Sub {
        id: String,
        volume_id: String,
        path: String,
        dir_low: String,
        stem_low: String,
        size: i64,
        mtime: i64,
    }

    let mut seen_ids: HashSet<String> = HashSet::new();
    let mut episodes: Vec<Ep> = Vec::new();
    let mut subs: Vec<Sub> = Vec::new();
    let mut stmt = store.conn().prepare(&sql)?;
    for (root, vol_filter) in roots {
        let root_t = root.trim_end_matches(SEP).to_string();
        let prefix = format!("{}{SEP}", root_t.to_lowercase());
        let plen = prefix.chars().count() as i64;
        let rows = stmt.query_map(params![prefix, plen, vol_filter], |r| {
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
            if in_claimed(&path, &vol) || has_build_dir(&path, &root_t) || !seen_ids.insert(id.clone()) {
                continue;
            }
            let dir = parent_dir(&path);
            let dir_low = dir.trim_end_matches(SEP).to_lowercase();
            if VIDEO_EXTS.contains(&ext.as_str()) {
                let p_name = Path::new(&dir).file_name().map(|n| n.to_string_lossy().to_string()).unwrap_or_default();
                if let Some((norm, display, season, _ep)) = parse_episode(&name, &p_name) {
                    episodes.push(Ep {
                        id,
                        volume_id: vol,
                        path,
                        dir_low,
                        stem_low: stem_key(&name),
                        size,
                        mtime,
                        norm,
                        display,
                        season,
                    });
                }
            } else if SUBTITLE_EXTS.contains(&ext.as_str()) {
                subs.push(Sub { id, volume_id: vol, path, dir_low, stem_low: stem_key(&name), size, mtime });
            }
        }
    }

    // Group episodes into (volume_id, norm_show, season)
    struct Bucket {
        displays: HashMap<String, usize>,
        members: Vec<(String, String, i64, i64)>,
        ep_count: usize,
        dirs: HashSet<String>,
    }
    type DirEpRef = (String, String, u32, String);
    let mut groups: BTreeMap<(String, String, u32), Bucket> = BTreeMap::new();
    let mut dir_eps: HashMap<(String, String), Vec<DirEpRef>> = HashMap::new();
    for ep in &episodes {
        let k = (ep.volume_id.clone(), ep.norm.clone(), ep.season);
        let b = groups.entry(k).or_insert_with(|| Bucket {
            displays: HashMap::new(),
            members: Vec::new(),
            ep_count: 0,
            dirs: HashSet::new(),
        });
        *b.displays.entry(ep.display.clone()).or_default() += 1;
        b.members.push((ep.id.clone(), ep.path.clone(), ep.size, ep.mtime));
        b.ep_count += 1;
        b.dirs.insert(parent_dir(&ep.path));
        dir_eps
            .entry((ep.volume_id.clone(), ep.dir_low.clone()))
            .or_default()
            .push((ep.norm.clone(), ep.stem_low.clone(), ep.season, ep.id.clone()));
    }

    for sub in subs {
        let Some(cands) = dir_eps.get(&(sub.volume_id.clone(), sub.dir_low.clone())) else {
            continue;
        };
        let matched = cands
            .iter()
            .find(|(_, estem, _, _)| sub.stem_low == *estem || sub.stem_low.starts_with(estem.as_str()))
            .or_else(|| if cands.len() == 1 { cands.first() } else { None });
        if let Some((norm, _, season, _)) = matched
            && let Some(b) = groups.get_mut(&(sub.volume_id, norm.clone(), *season))
        {
            b.members.push((sub.id, sub.path, sub.size, sub.mtime));
        }
    }

    let answer_for = |kind: DecisionKind, fp: &str| {
        decisions
            .iter()
            .filter(|d| d.kind == kind && d.fingerprint_b.is_none() && super::relate::fingerprints_match(&d.fingerprint_a, fp))
            .max_by_key(|d| d.at)
    };

    let mut out_cols = Vec::new();
    let mut out_members = Vec::new();
    for ((vol, norm, season), b) in groups {
        let fp = series_fingerprint(&norm, season);
        if answer_for(DecisionKind::IsProject, &fp).is_some_and(|d| d.value == "no") {
            continue;
        }
        let raw_show = pick_display(&b.displays, &norm);
        let show = answer_for(DecisionKind::Name, &fp).map_or(raw_show, |d| d.value.clone());
        let season_label = format!("Season {season}");
        let state = if answer_for(DecisionKind::LeaveAsIs, &fp).is_some() {
            "left_as_is"
        } else if answer_for(DecisionKind::IsProject, &fp).is_some_and(|d| d.value == "yes") || b.ep_count >= 2 {
            "confirmed"
        } else {
            "candidate"
        };
        let conf = if b.ep_count >= 2 { 0.90 } else { 0.85 };
        let cid = new_id();
        for (fid, path, size, mtime) in &b.members {
            out_members.push(SetMember {
                collection_id: cid.clone(),
                file_id: fid.clone(),
                volume_id: vol.clone(),
                path: path.clone(),
                size: *size,
                mtime: *mtime,
            });
        }
        out_cols.push((
            Collection {
                id: cid,
                kind: CollectionKind::Series,
                name: format!("{show} — {season_label}"),
                project_kind: Some(ProjectKind::Other),
                confidence: conf,
                state: state.into(),
                evidence: vec![format!(
                    "{} episode{} of {show} ({season_label}) across {} folder{}",
                    b.ep_count,
                    if b.ep_count == 1 { "" } else { "s" },
                    b.dirs.len(),
                    if b.dirs.len() == 1 { "" } else { "s" },
                )],
                locations: Vec::new(),
                sub_projects: vec![show, season_label],
                plan_state: None,
            },
            Some(fp),
        ));
    }
    out_cols.sort_by_key(|(c, _)| c.name.to_lowercase());
    Ok((out_cols, out_members))
}

/// On-the-spot series recognition for `roots` when full recognition hasn't saved collections yet.
pub fn find_series_for_roots(store: &SqliteStore, roots: &[String]) -> Result<FoundSets> {
    let mut claimed: Vec<(Option<String>, String)> = super::protected_roots(store, roots)?
        .into_iter()
        .map(|(p, _)| (None, p))
        .collect();
    for mg in super::module_folder_groups(store, roots)? {
        claimed.push((None, mg.path));
    }
    let decisions = super::store::decisions(store.conn())?;
    let rs: Vec<(String, Option<String>)> = roots.iter().map(|r| (r.clone(), None)).collect();
    find_series(store, &rs, &claimed, &decisions, &HashMap::new(), &[])
}
