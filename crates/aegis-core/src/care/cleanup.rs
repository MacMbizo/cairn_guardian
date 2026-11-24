//! Reclaimable cleanup scanner & safe quarantine batch builder (`K1`, `K2`, `R1`, `R8`).
//!
//! Covers temporary files, crash dumps, browser/dev/build caches, Downloads tidy-ups, and
//! stale unowned large files (`DeadFiles`) outside projects and atomic units.

use super::{
    CleanupCandidate, CleanupCategory, CleanupGroupSummary, CleanupScanReport, SYSTEM_CARE_AGENT,
    display_path, path_contains_aegis_internal,
};
use crate::contracts::{ActionType, ProposedActionInput, RefilePlan, TargetKind};
use crate::db::SqliteStore;
use crate::db::actions::propose_batch;
use crate::db::files::{
    DiscoveredFileInput, path_is_within, resolve_online_volume, upsert_discovered_file,
};
use crate::engine::janitor::{installed_programs, scan_downloads};
use crate::error::{AegisError, Result};
use crate::now_ms;
use crate::scanner::os_adapter::inspect_path_metadata;
use rusqlite::{Connection, OptionalExtension, params};
use std::collections::{BTreeMap, HashSet};
use std::path::{Path, PathBuf};

const DAY_MS: i64 = 86_400_000;
const TEMP_MIN_AGE_MS: i64 = DAY_MS;
const DEFAULT_DEAD_FILE_MIN_BYTES: i64 = 100 * 1024 * 1024;
const DEFAULT_DEAD_FILE_AGE_DAYS: i64 = 180;
const MAX_SCAN_DEPTH: usize = 6;
const MAX_CANDIDATES_PER_CATEGORY: usize = 200;

/// Injectable scan paths and thresholds (host paths in production, `TempDir` paths in tests).
#[derive(Debug, Clone)]
pub struct CleanupScanConfig {
    pub temp_dirs: Vec<PathBuf>,
    pub crash_dirs: Vec<PathBuf>,
    pub browser_cache_dirs: Vec<(String, PathBuf)>,
    pub dev_cache_dirs: Vec<(String, PathBuf)>,
    pub downloads_dir: Option<PathBuf>,
    pub installed_names: Vec<String>,
    pub now_ms: i64,
    pub dead_file_min_bytes: i64,
    pub dead_file_age_days: i64,
}

impl CleanupScanConfig {
    pub fn for_host() -> Self {
        let mut temp_dirs = Vec::new();
        let mut seen_temp = HashSet::new();
        let mut push_temp = |p: PathBuf| {
            let k = display_path(&p).to_lowercase();
            if !k.is_empty() && seen_temp.insert(k) {
                temp_dirs.push(p);
            }
        };
        for var in ["TEMP", "TMP"] {
            if let Some(v) = std::env::var_os(var) {
                push_temp(PathBuf::from(v));
            }
        }
        push_temp(std::env::temp_dir());
        if let Some(win_dir) = std::env::var_os("WINDIR") {
            let wt = PathBuf::from(win_dir).join("Temp");
            if wt.is_dir() {
                push_temp(wt);
            }
        }

        let local_app = std::env::var_os("LOCALAPPDATA").map(PathBuf::from);
        let roaming_app = std::env::var_os("APPDATA").map(PathBuf::from);
        let user_profile = std::env::var_os("USERPROFILE").map(PathBuf::from);

        let mut crash_dirs = Vec::new();
        if let Some(ref la) = local_app {
            crash_dirs.push(la.join("CrashDumps"));
            crash_dirs.push(la.join("Microsoft").join("Windows").join("WER").join("ReportArchive"));
            crash_dirs.push(la.join("Microsoft").join("Windows").join("WER").join("ReportQueue"));
        }

        let mut browser_cache_dirs = Vec::new();
        if let Some(ref la) = local_app {
            for (browser, base) in [
                ("Chrome", la.join("Google").join("Chrome").join("User Data")),
                ("Edge", la.join("Microsoft").join("Edge").join("User Data")),
                ("Brave", la.join("BraveSoftware").join("Brave-Browser").join("User Data")),
                ("Vivaldi", la.join("Vivaldi").join("User Data")),
            ] {
                for prof in ["Default", "Profile 1", "Profile 2"] {
                    let pdir = base.join(prof);
                    for (sub_label, rel) in [
                        ("Cache", Path::new("Cache").join("Cache_Data")),
                        ("Code Cache", PathBuf::from("Code Cache")),
                        ("GPUCache", PathBuf::from("GPUCache")),
                    ] {
                        browser_cache_dirs.push((format!("{browser} ({prof} {sub_label})"), pdir.join(rel)));
                    }
                }
            }
            let ff_profiles = la.join("Mozilla").join("Firefox").join("Profiles");
            if let Ok(rd) = std::fs::read_dir(&ff_profiles) {
                for entry in rd.flatten() {
                    let p = entry.path().join("cache2").join("entries");
                    let name = entry.file_name().to_string_lossy().to_string();
                    browser_cache_dirs.push((format!("Firefox ({name})"), p));
                }
            }
        }

        let mut dev_cache_dirs = Vec::new();
        if let Some(ref la) = local_app {
            dev_cache_dirs.push(("pip cache".into(), la.join("pip").join("cache")));
            dev_cache_dirs.push(("npm cache".into(), la.join("npm-cache")));
            dev_cache_dirs.push(("uv cache".into(), la.join("uv").join("cache")));
            dev_cache_dirs.push(("NuGet v3 cache".into(), la.join("NuGet").join("v3-cache")));
        }
        if let Some(ref ra) = roaming_app {
            dev_cache_dirs.push(("npm roaming cache".into(), ra.join("npm-cache")));
        }

        let downloads_dir = user_profile.map(|p| p.join("Downloads"));
        Self {
            temp_dirs,
            crash_dirs,
            browser_cache_dirs,
            dev_cache_dirs,
            downloads_dir,
            installed_names: installed_programs(),
            now_ms: now_ms(),
            dead_file_min_bytes: DEFAULT_DEAD_FILE_MIN_BYTES,
            dead_file_age_days: DEFAULT_DEAD_FILE_AGE_DAYS,
        }
    }
}

fn modified_ms(meta: &std::fs::Metadata) -> i64 {
    meta.modified()
        .ok()
        .and_then(|t| t.duration_since(std::time::UNIX_EPOCH).ok())
        .map_or(0, |d| d.as_millis() as i64)
}

/// Computes `(total_bytes, file_count, newest_mtime_ms)` without following symlinks or junctions.
pub(crate) fn dir_stats_bounded(root: &Path, max_depth: usize) -> (i64, i64, i64) {
    fn walk(dir: &Path, depth: usize, max_depth: usize, bytes: &mut i64, count: &mut i64, newest: &mut i64) {
        if depth > max_depth || path_contains_aegis_internal(dir) {
            return;
        }
        let Ok(rd) = std::fs::read_dir(dir) else { return };
        for entry in rd.flatten() {
            let path = entry.path();
            let Ok(meta) = std::fs::symlink_metadata(&path) else { continue };
            if meta.file_type().is_symlink() {
                continue;
            }
            if meta.is_dir() {
                walk(&path, depth + 1, max_depth, bytes, count, newest);
            } else if meta.is_file() {
                *bytes = bytes.saturating_add(meta.len() as i64);
                *count += 1;
                let mt = modified_ms(&meta);
                if mt > *newest {
                    *newest = mt;
                }
            }
        }
    }

    let mut bytes = 0i64;
    let mut count = 0i64;
    let mut newest = 0i64;
    walk(root, 0, max_depth, &mut bytes, &mut count, &mut newest);
    if newest == 0 {
        newest = std::fs::symlink_metadata(root).map(|m| modified_ms(&m)).unwrap_or(0);
    }
    (bytes, count, newest)
}

fn make_id(category: CleanupCategory, path: &str) -> String {
    let mut h = blake3::Hasher::new();
    h.update(&[category.sort_order()]);
    h.update(path.to_lowercase().as_bytes());
    h.finalize().to_hex()[..16].to_string()
}

fn open_batch_paths(conn: &Connection) -> Result<HashSet<String>> {
    let mut st = conn.prepare(
        "SELECT lower(a.from_path) FROM actions a
         JOIN action_batches b ON b.id = a.batch_id
         WHERE a.status IN ('proposed', 'approved') AND b.status IN ('draft', 'approved')",
    )?;
    Ok(st.query_map([], |r| r.get(0))?.collect::<rusqlite::Result<_>>()?)
}

fn declined_care_paths(conn: &Connection) -> Result<HashSet<String>> {
    let mut st = conn.prepare(
        "SELECT lower(a.from_path) FROM actions a
         WHERE a.agent = ?1 AND a.status = 'rejected'
         UNION
         SELECT lower(q.original_path) FROM quarantine_entries q
         JOIN actions a ON a.id = q.action_id
         WHERE a.agent = ?1 AND q.restored_at IS NOT NULL",
    )?;
    Ok(st.query_map([SYSTEM_CARE_AGENT], |r| r.get(0))?.collect::<rusqlite::Result<_>>()?)
}

fn scan_aged_children(
    dir: &Path,
    category: CleanupCategory,
    group_label: &str,
    min_age_ms: i64,
    now_ms: i64,
    blocked: &HashSet<String>,
    out: &mut Vec<CleanupCandidate>,
) {
    if !dir.is_dir() || path_contains_aegis_internal(dir) {
        return;
    }
    let Ok(rd) = std::fs::read_dir(dir) else { return };
    for entry in rd.flatten() {
        if out.len() >= MAX_CANDIDATES_PER_CATEGORY {
            break;
        }
        let path = entry.path();
        if path_contains_aegis_internal(&path) {
            continue;
        }
        let Ok(meta) = std::fs::symlink_metadata(&path) else { continue };
        if meta.file_type().is_symlink() {
            continue;
        }
        let disp = display_path(&path);
        if blocked.contains(&disp.to_lowercase()) {
            continue;
        }
        let (target_kind, bytes, file_count, mtime) = if meta.is_file() {
            (TargetKind::File, meta.len() as i64, 1, modified_ms(&meta))
        } else if meta.is_dir() {
            let (b, c, newest) = dir_stats_bounded(&path, MAX_SCAN_DEPTH);
            if b <= 0 && c == 0 {
                continue;
            }
            (TargetKind::Dir, b, c.max(1), newest)
        } else {
            continue;
        };

        if mtime <= 0 || now_ms - mtime < min_age_ms {
            continue;
        }
        let age_days = ((now_ms - mtime).max(0)) / DAY_MS;
        out.push(CleanupCandidate {
            id: make_id(category, &disp),
            category,
            group_label: group_label.to_string(),
            path: disp,
            target_kind,
            bytes,
            file_count,
            age_days,
            reason: format!("{group_label} entry untouched for {age_days} day(s)"),
            file_id: None,
            safety_note: "Moves to 30-day quarantine; apps recreate temp/cache data automatically if needed.".into(),
        });
    }
}

fn scan_whole_cache_dirs(
    dirs: &[(String, PathBuf)],
    category: CleanupCategory,
    now_ms: i64,
    blocked: &HashSet<String>,
    out: &mut Vec<CleanupCandidate>,
) {
    for (label, dir) in dirs {
        if out.len() >= MAX_CANDIDATES_PER_CATEGORY {
            break;
        }
        if !dir.is_dir() || path_contains_aegis_internal(dir) {
            continue;
        }
        let Ok(meta) = std::fs::symlink_metadata(dir) else { continue };
        if meta.file_type().is_symlink() {
            continue;
        }
        let disp = display_path(dir);
        if blocked.contains(&disp.to_lowercase()) {
            continue;
        }
        let (bytes, file_count, newest) = dir_stats_bounded(dir, MAX_SCAN_DEPTH);
        if bytes <= 0 || file_count == 0 {
            continue;
        }
        let age_days = if newest > 0 {
            ((now_ms - newest).max(0)) / DAY_MS
        } else {
            0
        };
        out.push(CleanupCandidate {
            id: make_id(category, &disp),
            category,
            group_label: label.clone(),
            path: disp,
            target_kind: TargetKind::Dir,
            bytes,
            file_count,
            age_days,
            reason: format!("{label} cache ({file_count} files)"),
            file_id: None,
            safety_note: "Re-created automatically next time the app runs; bookmarks, sessions, and source code stay intact.".into(),
        });
    }
}

fn scan_build_caches(
    conn: &Connection,
    now_ms: i64,
    blocked: &HashSet<String>,
    out: &mut Vec<CleanupCandidate>,
) -> Result<()> {
    let mut st = conn.prepare(
        "SELECT path, cache_kind, size_bytes, file_count, source_max_mtime
         FROM cache_dirs
         WHERE state = 'active' AND size_bytes > 0
         ORDER BY size_bytes DESC LIMIT 100",
    )?;
    for row in st.query_map([], |r| {
        Ok((
            r.get::<_, String>(0)?,
            r.get::<_, String>(1)?,
            r.get::<_, i64>(2)?,
            r.get::<_, i64>(3)?,
            r.get::<_, Option<i64>>(4)?,
        ))
    })? {
        let (path, kind, size_bytes, file_count, src_mtime) = row?;
        if blocked.contains(&path.to_lowercase()) || !Path::new(&path).is_dir() {
            continue;
        }
        let age_days = src_mtime
            .map(|m| ((now_ms - m).max(0)) / DAY_MS)
            .unwrap_or(0);
        out.push(CleanupCandidate {
            id: make_id(CleanupCategory::BuildCache, &path),
            category: CleanupCategory::BuildCache,
            group_label: kind.clone(),
            path,
            target_kind: TargetKind::Dir,
            bytes: size_bytes,
            file_count,
            age_days,
            reason: format!("Detected {kind} build/package output (safe_build_cache carve-out)"),
            file_id: None,
            safety_note: "Rebuilt by tooling; protected from accidental project splitting until reviewed here.".into(),
        });
    }
    Ok(())
}

fn scan_dead_large_files(
    conn: &Connection,
    cfg: &CleanupScanConfig,
    blocked: &HashSet<String>,
    out: &mut Vec<CleanupCandidate>,
) -> Result<()> {
    let cutoff = cfg.now_ms - cfg.dead_file_age_days.max(1) * DAY_MS;

    // Protect files belonging to active contexts (`K1` / Plan 1-5 context preservation).
    let member_ids: HashSet<String> = {
        let mut st = conn.prepare(
            "SELECT m.file_id FROM collection_members m
             JOIN collections c ON c.id = m.collection_id
             WHERE c.state NOT IN ('dismissed', 'rejected', 'no')",
        )?;
        st.query_map([], |r| r.get(0))?.collect::<rusqlite::Result<_>>()?
    };
    let protected_roots: Vec<(String, String, bool)> = {
        let mut st = conn.prepare(
            "SELECT cl.volume_id, cl.root_path, COALESCE(v.case_sensitive, 0)
             FROM collection_locations cl
             JOIN collections c ON c.id = cl.collection_id
             LEFT JOIN volumes v ON v.id = cl.volume_id
             WHERE c.state NOT IN ('dismissed', 'rejected', 'no')",
        )?;
        st.query_map([], |r| {
            Ok((
                r.get::<_, String>(0)?,
                r.get::<_, String>(1)?,
                r.get::<_, i64>(2)? != 0,
            ))
        })?
        .collect::<rusqlite::Result<_>>()?
    };

    let mut st = conn.prepare(
        "SELECT f.id, f.volume_id, f.path, f.size, f.mtime
         FROM files f
         JOIN volumes v ON v.id = f.volume_id
         WHERE f.deleted_at IS NULL
           AND f.state = 'active'
           AND v.role != 'backup'
           AND f.atomic_unit_id IS NULL
           AND f.size >= ?1
           AND f.mtime <= ?2
         ORDER BY f.size DESC
         LIMIT 200",
    )?;

    for row in st.query_map(params![cfg.dead_file_min_bytes, cutoff], |r| {
        Ok((
            r.get::<_, String>(0)?,
            r.get::<_, String>(1)?,
            r.get::<_, String>(2)?,
            r.get::<_, i64>(3)?,
            r.get::<_, i64>(4)?,
        ))
    })? {
        if out.len() >= 50 {
            break;
        }
        let (file_id, vol_id, path, size, mtime) = row?;
        if blocked.contains(&path.to_lowercase()) || path_contains_aegis_internal(Path::new(&path)) {
            continue;
        }
        if member_ids.contains(&file_id)
            || protected_roots
                .iter()
                .any(|(vid, root, cs)| *vid == vol_id && path_is_within(&path, root, *cs))
        {
            continue;
        }
        if !Path::new(&path).is_file() {
            continue;
        }
        let age_days = ((cfg.now_ms - mtime).max(0)) / DAY_MS;
        out.push(CleanupCandidate {
            id: make_id(CleanupCategory::DeadFiles, &path),
            category: CleanupCategory::DeadFiles,
            group_label: "Dead large file".into(),
            path,
            target_kind: TargetKind::File,
            bytes: size,
            file_count: 1,
            age_days,
            reason: format!("Unmodified for {age_days} days; outside recognised projects and atomic units"),
            file_id: Some(file_id),
            safety_note: "Staged in 30-day quarantine; restore at any time before expiry.".into(),
        });
    }
    Ok(())
}

/// Scans all reclaimable categories cleanly without touching any file on disk.
pub fn scan_cleanup(conn: &Connection, cfg: &CleanupScanConfig) -> Result<CleanupScanReport> {
    let mut blocked = open_batch_paths(conn)?;
    blocked.extend(declined_care_paths(conn)?);

    let mut temp_items = Vec::new();
    for dir in &cfg.temp_dirs {
        scan_aged_children(dir, CleanupCategory::TempFiles, "Temporary", TEMP_MIN_AGE_MS, cfg.now_ms, &blocked, &mut temp_items);
    }

    let mut crash_items = Vec::new();
    for dir in &cfg.crash_dirs {
        scan_aged_children(dir, CleanupCategory::CrashDumps, "Crash Dump / WER", TEMP_MIN_AGE_MS, cfg.now_ms, &blocked, &mut crash_items);
    }

    let mut browser_items = Vec::new();
    scan_whole_cache_dirs(&cfg.browser_cache_dirs, CleanupCategory::BrowserCache, cfg.now_ms, &blocked, &mut browser_items);

    let mut dev_items = Vec::new();
    scan_whole_cache_dirs(&cfg.dev_cache_dirs, CleanupCategory::DevCache, cfg.now_ms, &blocked, &mut dev_items);

    let mut build_items = Vec::new();
    scan_build_caches(conn, cfg.now_ms, &blocked, &mut build_items)?;

    let mut downloads_items = Vec::new();
    if let Some(ref dl) = cfg.downloads_dir
        && dl.is_dir()
    {
        for j in scan_downloads(conn, dl, &cfg.installed_names, cfg.now_ms)? {
            if blocked.contains(&j.path.to_lowercase()) {
                continue;
            }
            let (reason, age_days) = match j.kind {
                crate::engine::janitor::JanitorKind::Partial => ("Unfinished partial download (24h+ untouched)".into(), 1),
                crate::engine::janitor::JanitorKind::Repeat { ref of } => (format!("Duplicate download of {of}"), 0),
                crate::engine::janitor::JanitorKind::Installer { already_installed: true, age_days } => {
                    (format!("Old installer ({age_days}d) for an already installed app"), age_days)
                }
                crate::engine::janitor::JanitorKind::Installer { already_installed: false, age_days } => {
                    (format!("Installer unused for {age_days} days"), age_days)
                }
            };
            downloads_items.push(CleanupCandidate {
                id: make_id(CleanupCategory::DownloadsJanitor, &j.path),
                category: CleanupCategory::DownloadsJanitor,
                group_label: "Downloads".into(),
                path: j.path,
                target_kind: TargetKind::File,
                bytes: j.bytes,
                file_count: 1,
                age_days,
                reason,
                file_id: None,
                safety_note: "Moves to 30-day quarantine; never removes without your approval.".into(),
            });
        }
    }

    let mut dead_items = Vec::new();
    scan_dead_large_files(conn, cfg, &blocked, &mut dead_items)?;

    let mut grouped: BTreeMap<CleanupCategory, Vec<CleanupCandidate>> = BTreeMap::new();
    for (cat, list) in [
        (CleanupCategory::TempFiles, temp_items),
        (CleanupCategory::CrashDumps, crash_items),
        (CleanupCategory::BrowserCache, browser_items),
        (CleanupCategory::DevCache, dev_items),
        (CleanupCategory::BuildCache, build_items),
        (CleanupCategory::DownloadsJanitor, downloads_items),
        (CleanupCategory::DeadFiles, dead_items),
    ] {
        if !list.is_empty() {
            grouped.insert(cat, list);
        }
    }

    let mut groups: Vec<CleanupGroupSummary> = grouped
        .into_iter()
        .map(|(category, mut items)| {
            items.sort_by(|a, b| b.bytes.cmp(&a.bytes).then_with(|| a.path.cmp(&b.path)));
            let total_bytes: i64 = items.iter().map(|i| i.bytes).sum();
            CleanupGroupSummary {
                category,
                label: category.label().to_string(),
                description: category.description().to_string(),
                total_bytes,
                item_count: items.len(),
                items,
            }
        })
        .collect();
    groups.sort_by(|a, b| {
        b.total_bytes
            .cmp(&a.total_bytes)
            .then_with(|| a.category.sort_order().cmp(&b.category.sort_order()))
    });

    let total_reclaimable_bytes = groups.iter().map(|g| g.total_bytes).sum();
    Ok(CleanupScanReport {
        scanned_at: cfg.now_ms,
        total_reclaimable_bytes,
        groups,
    })
}

/// Proposes quarantining `items` in one reviewable batch (`origin = "system_care"`). Zero direct deletes.
pub fn propose_care_quarantine(store: &mut SqliteStore, items: &[CleanupCandidate]) -> Result<String> {
    if items.is_empty() {
        return Err(AegisError::InvalidInput("Select at least one cleanup item to propose.".into()));
    }
    let busy = open_batch_paths(store.conn())?;
    let mut seen = HashSet::new();
    let mut actions = Vec::new();

    for it in items {
        let key = it.path.to_lowercase();
        if !seen.insert(key.clone()) || busy.contains(&key) {
            continue;
        }
        let p = Path::new(&it.path);
        if path_contains_aegis_internal(p) || !p.exists() {
            continue;
        }
        let vol_id = match resolve_online_volume(store.conn(), p)? {
            Some((vid, _, _)) => vid,
            None => store.ensure_volume_for_path(p)?.id,
        };
        let mut file_id = match it.file_id.clone() {
            Some(fid) => Some(fid),
            None => store
                .conn()
                .query_row(
                    "SELECT id FROM files WHERE lower(path) = lower(?1) AND deleted_at IS NULL LIMIT 1",
                    [&it.path],
                    |r| r.get(0),
                )
                .optional()?,
        };
        if it.target_kind == TargetKind::File && file_id.is_none() {
            if let Ok(Some(stat)) = inspect_path_metadata(p)
                && !stat.is_dir
                && !stat.is_symlink
            {
                let (has_stable_inode, case_sensitive): (bool, bool) = store.conn().query_row(
                    "SELECT has_stable_inode, case_sensitive FROM volumes WHERE id = ?1",
                    [&vol_id],
                    |r| Ok((r.get::<_, i64>(0)? != 0, r.get::<_, i64>(1)? != 0)),
                )?;
                let up_id = store.with_full_sync_tx(|tx| {
                    upsert_discovered_file(
                        tx,
                        &DiscoveredFileInput {
                            volume_id: vol_id.clone(),
                            has_stable_inode,
                            case_sensitive,
                            file_index: stat.file_index,
                            nlink: stat.nlink,
                            atomic_unit_id: None,
                            path: p.to_path_buf(),
                            size: stat.size,
                            mtime: stat.mtime,
                            ctime: stat.ctime,
                            blake3: None,
                            cloud_state: stat.cloud_state,
                        },
                    )
                })?;
                file_id = Some(up_id);
            } else {
                continue;
            }
        }
        let evidence = serde_json::json!({
            "care_category": it.category,
            "group": it.group_label,
            "bytes": it.bytes,
            "file_count": it.file_count,
            "reason": it.reason,
        })
        .to_string();

        actions.push(ProposedActionInput {
            agent: Some(SYSTEM_CARE_AGENT.into()),
            action_type: ActionType::Quarantine,
            target_kind: it.target_kind,
            file_id,
            atomic_unit_id: None,
            from_path: it.path.clone(),
            to_path: None,
            reason: it.reason.clone(),
            confidence: 0.95,
            evidence_json: evidence,
        });
    }

    if actions.is_empty() {
        return Err(AegisError::InvalidInput(
            "Selected items no longer exist or are already queued in an open batch.".into(),
        ));
    }

    propose_batch(
        store,
        &RefilePlan {
            origin: SYSTEM_CARE_AGENT.into(),
            actions,
        },
    )
}
