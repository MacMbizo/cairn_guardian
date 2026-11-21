//! Backup mirror top-up with a 90-day safety window (spec §7.7.1).
//!
//! Every file is copied to `<dest>.aegis-partial`, checked against the source (unchanged
//! during the copy), then renamed into place, so an unplugged drive never holds a half file
//! at a live path. Replaced and retired copies move to `_Previous versions\<date>\…`.

use super::guard::{self, GuardHalt, GuardVerdict};
use super::priority::{self, TierShortfall};
use super::{BACKUP_DIR, PREVIOUS_DIR, backup_dest_for, backup_root, rel_under};
use crate::db::SqliteStore;
use crate::db::audit::append_audit_in_tx;
use crate::db::files::{normalize_path, path_is_within, resolve_online_volume, upsert_discovered_file};
use crate::error::{AegisError, Result};
use crate::scanner::os_adapter::inspect_path_metadata;
use crate::{format_date_ms, new_id, now_ms};
use rusqlite::{Connection, OptionalExtension, params};
use serde::{Deserialize, Serialize};
use std::collections::{HashMap, HashSet};
use std::io::{Read, Write};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use ts_rs::TS;

/// How long replaced or deleted copies stay in `_Previous versions`.
pub const PREVIOUS_VERSION_TTL_MS: i64 = 90 * 86_400_000;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "snake_case")]
#[ts(export)]
pub enum TopUpKind {
    Add,
    Replace,
    RetireDeleted,
    RetireUncovered,
}

/// Who authorised a top-up run.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "snake_case")]
#[ts(export)]
pub enum TopUpAuthority {
    /// The owner approved this run.
    UserApproved,
    /// A standing approval for this Backup drive (spec §7.8.6).
    Standing,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, TS)]
#[ts(export)]
pub struct TopUpItem {
    pub kind: TopUpKind,
    pub source_file_id: Option<String>,
    pub source_path: Option<String>,
    /// Live copy's path relative to the Backup drive root (`Aegis Backup\…`).
    pub backup_rel_path: String,
    #[ts(type = "number")]
    pub bytes: i64,
    #[ts(type = "number")]
    pub tier: i64,
    /// The backup source folder this item belongs to (guard grouping).
    pub source_folder: String,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, TS)]
#[ts(export)]
pub struct TopUpPlan {
    pub backup_volume_id: String,
    pub items: Vec<TopUpItem>,
    pub halted_folders: Vec<GuardHalt>,
    pub skipped_for_space: Vec<TierShortfall>,
    /// Bytes of the oldest previous versions that must be pruned first to make room.
    #[ts(type = "number")]
    pub prune_bytes: i64,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, TS)]
#[ts(export)]
pub struct TopUpResult {
    pub job_id: String,
    #[ts(type = "number")]
    pub added: i64,
    #[ts(type = "number")]
    pub replaced: i64,
    #[ts(type = "number")]
    pub retired: i64,
    #[ts(type = "number")]
    pub bytes_copied: i64,
    /// Files that changed while being copied; they are picked up next time.
    #[ts(type = "number")]
    pub skipped_changed: i64,
    pub completed: bool,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, TS)]
#[ts(export)]
pub struct PreviousVersionItem {
    pub id: String,
    pub rel_path: String,
    pub original_rel_path: String,
    pub source_path: Option<String>,
    /// `changed` | `deleted` | `uncovered`
    pub reason: String,
    #[ts(type = "number")]
    pub size_bytes: i64,
    #[ts(type = "number")]
    pub moved_at: i64,
    #[ts(type = "number")]
    pub prune_after: i64,
}

struct Source {
    volume_id: String,
    folder: String,
    online: bool,
}

fn live_sources(conn: &Connection) -> Result<Vec<Source>> {
    let mut stmt = conn.prepare(
        "SELECT s.volume_id, v.mount_point, s.rel_path, v.status = 'online'
         FROM backup_sources s JOIN volumes v ON v.id = s.volume_id
         WHERE s.removed_at IS NULL ORDER BY s.added_at",
    )?;
    let rows = stmt.query_map([], |r| {
        let mount: String = r.get(1)?;
        let rel: String = r.get(2)?;
        let folder = if rel.is_empty() {
            mount
        } else {
            Path::new(&mount).join(&rel).to_string_lossy().to_string()
        };
        Ok(Source {
            volume_id: r.get(0)?,
            folder,
            online: r.get(3)?,
        })
    })?;
    Ok(rows.collect::<std::result::Result<Vec<_>, _>>()?)
}

fn norm(p: &str) -> String {
    p.replace('/', "\\").to_lowercase()
}

/// Works out what the Backup drive needs: new and changed files to copy, and copies whose
/// source is gone (from an online drive) or no longer covered, to retire.
pub fn plan_topup(conn: &Connection, backup_volume_id: &str) -> Result<TopUpPlan> {
    let backup_mount: String = conn
        .query_row("SELECT mount_point FROM volumes WHERE id = ?1", [backup_volume_id], |r| r.get(0))
        .optional()?
        .ok_or_else(|| AegisError::NotFound(format!("volume {backup_volume_id}")))?;
    let backup_area = backup_root(Path::new(&backup_mount)).to_string_lossy().to_string();
    let sources = live_sources(conn)?;

    // Existing live copies on this Backup drive.
    let mut copies: HashMap<String, (String, Option<String>, Vec<u8>)> = HashMap::new();
    {
        let mut stmt = conn.prepare(
            "SELECT backup_rel_path, source_file_id, source_blake3 FROM backup_copies WHERE backup_volume_id = ?1",
        )?;
        let rows = stmt.query_map([backup_volume_id], |r| {
            Ok((r.get::<_, String>(0)?, r.get::<_, Option<String>>(1)?, r.get::<_, Vec<u8>>(2)?))
        })?;
        for r in rows {
            let (rel, sid, h) = r?;
            copies.insert(norm(&rel), (rel, sid, h));
        }
    }

    let mut items = Vec::new();
    let mut expected: HashSet<String> = HashSet::new();
    for src in sources.iter().filter(|s| s.online) {
        let mut stmt = conn.prepare(
            "SELECT id, path, size, blake3, cloud_state FROM files
             WHERE volume_id = ?1 AND deleted_at IS NULL AND state NOT IN ('quarantine','missing')
               AND container_file_id IS NULL",
        )?;
        let rows = stmt.query_map([&src.volume_id], |r| {
            Ok((
                r.get::<_, String>(0)?,
                r.get::<_, String>(1)?,
                r.get::<_, i64>(2)?,
                r.get::<_, Option<Vec<u8>>>(3)?,
                r.get::<_, String>(4)?,
            ))
        })?;
        for row in rows {
            let (id, path, size, hash, cloud) = row?;
            if cloud == "placeholder_remote"
                || !path_is_within(&path, &src.folder, false)
                || path_is_within(&path, &backup_area, false)
            {
                continue;
            }
            let Some(tier) = priority::tier_for_file(conn, &id)? else { continue };
            let dest = backup_dest_for(conn, &src.volume_id, &path, &backup_mount)?;
            let rel = rel_under(&normalize_path(&dest, false).0, &backup_mount);
            if !expected.insert(norm(&rel)) {
                continue; // two sources cover the same file
            }
            let kind = match copies.get(&norm(&rel)) {
                None => Some(TopUpKind::Add),
                Some((_, _, stored)) => match &hash {
                    Some(h) if h == stored => None,
                    _ => Some(TopUpKind::Replace),
                },
            };
            if let Some(kind) = kind {
                items.push(TopUpItem {
                    kind,
                    source_file_id: Some(id),
                    source_path: Some(path),
                    backup_rel_path: rel,
                    bytes: size,
                    tier,
                    source_folder: src.folder.clone(),
                });
            }
        }
    }

    // Copies nothing live maps to any more.
    for (key, (rel, sid, _)) in &copies {
        if expected.contains(key) {
            continue;
        }
        let source: Option<(String, Option<i64>, String, bool)> = match sid {
            Some(sid) => conn
                .query_row(
                    "SELECT f.path, f.deleted_at, f.volume_id, v.status = 'online'
                     FROM files f JOIN volumes v ON v.id = f.volume_id WHERE f.id = ?1",
                    [sid],
                    |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?, r.get(3)?)),
                )
                .optional()?,
            None => None,
        };
        let Some((path, deleted_at, vol, online)) = source else { continue };
        if !online {
            continue; // an unplugged source is never a deletion
        }
        let covering = sources
            .iter()
            .find(|s| s.volume_id == vol && path_is_within(&path, &s.folder, false));
        let kind = if deleted_at.is_some() || !Path::new(&path).exists() {
            if Path::new(&path).exists() {
                continue; // row retired but the file is back: the next scan re-adds it
            }
            TopUpKind::RetireDeleted
        } else if covering.is_none() {
            TopUpKind::RetireUncovered
        } else {
            TopUpKind::RetireDeleted // renamed or moved: the new path is copied as an Add
        };
        items.push(TopUpItem {
            kind,
            source_file_id: sid.clone(),
            source_path: Some(path.clone()),
            backup_rel_path: rel.clone(),
            bytes: 0,
            tier: 0,
            source_folder: covering.map(|s| s.folder.clone()).unwrap_or_default(),
        });
    }
    items.sort_by(|a, b| a.tier.cmp(&b.tier).then_with(|| a.backup_rel_path.cmp(&b.backup_rel_path)));

    let mut plan = TopUpPlan {
        backup_volume_id: backup_volume_id.to_string(),
        items,
        halted_folders: Vec::new(),
        skipped_for_space: Vec::new(),
        prune_bytes: 0,
    };
    if let GuardVerdict::Halt(halts) = guard::check(conn, &plan)? {
        let halted: HashSet<String> = halts.iter().map(|h| h.source_folder.clone()).collect();
        plan.items.retain(|i| !halted.contains(&i.source_folder));
        plan.halted_folders = halts;
    }
    fill_by_priority(conn, &mut plan)?;
    Ok(plan)
}

/// Keeps copies in tier order while they fit (spec §7.7.2). Space held by previous versions
/// counts as available: the oldest are pruned first (`prune_bytes`). Live copies are never
/// removed to make room; whatever doesn't fit is skipped and reported per tier.
fn fill_by_priority(conn: &Connection, plan: &mut TopUpPlan) -> Result<()> {
    let (free, versions) = priority::backup_space(conn, &plan.backup_volume_id)?;
    let mut free_left = free;
    let mut versions_left = versions;
    let mut needed: std::collections::BTreeMap<i64, (i64, i64)> = Default::default();
    plan.items.retain(|it| {
        if !matches!(it.kind, TopUpKind::Add | TopUpKind::Replace) {
            return true;
        }
        let slot = needed.entry(it.tier).or_insert((0, 0));
        slot.0 += it.bytes;
        if it.bytes <= free_left {
            free_left -= it.bytes;
            true
        } else if it.bytes <= free_left + versions_left {
            versions_left -= it.bytes - free_left;
            free_left = 0;
            true
        } else {
            slot.1 += it.bytes;
            false
        }
    });
    plan.prune_bytes = versions - versions_left;
    plan.skipped_for_space = needed
        .into_iter()
        .filter(|(_, (_, skipped))| *skipped > 0)
        .map(|(tier, (bytes_needed, bytes_skipped))| TierShortfall { tier, bytes_needed, bytes_skipped })
        .collect();
    Ok(())
}

/// Deletes the oldest previous versions until at least `bytes` are freed. Audited.
fn prune_oldest(store: &mut SqliteStore, backup_volume_id: &str, mount: &Path, bytes: i64) -> Result<i64> {
    let rows: Vec<(String, String, i64)> = {
        let mut stmt = store.conn().prepare(
            "SELECT id, rel_path, size_bytes FROM backup_versions
             WHERE backup_volume_id = ?1 AND pruned_at IS NULL ORDER BY moved_at ASC, id ASC",
        )?;
        let r = stmt.query_map([backup_volume_id], |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?)))?;
        r.collect::<std::result::Result<Vec<_>, _>>()?
    };
    let now = now_ms();
    let mut freed = 0i64;
    let mut count = 0i64;
    for (id, rel, size) in rows {
        if freed >= bytes {
            break;
        }
        match std::fs::remove_file(mount.join(&rel)) {
            Ok(()) => {}
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => {}
            Err(_) => continue,
        }
        store.conn().execute("UPDATE backup_versions SET pruned_at = ?2 WHERE id = ?1", params![id, now])?;
        freed += size;
        count += 1;
    }
    if count > 0 {
        store.with_full_sync_tx(|tx| {
            append_audit_in_tx(
                tx,
                "system",
                "backup.prune",
                backup_volume_id,
                &serde_json::json!({ "pruned": count, "bytes": freed, "reason": "space" }).to_string(),
            )?;
            Ok(())
        })?;
    }
    Ok(freed)
}

fn remove_partials(dir: &Path) {
    let Ok(rd) = std::fs::read_dir(dir) else { return };
    for e in rd.flatten() {
        let p = e.path();
        if p.is_dir() {
            remove_partials(&p);
        } else if p.to_string_lossy().ends_with(".aegis-partial") {
            let _ = std::fs::remove_file(&p);
        }
    }
}

fn mtime_ms(m: &std::fs::Metadata) -> i64 {
    m.modified()
        .ok()
        .and_then(|t| t.duration_since(std::time::UNIX_EPOCH).ok())
        .map(|d| d.as_millis() as i64)
        .unwrap_or(0)
}

/// Moves the live copy at `live` into `_Previous versions\<today>\…` and records it.
fn retire_copy(
    store: &mut SqliteStore,
    backup_volume_id: &str,
    backup_mount: &Path,
    live_rel: &str,
    source_path: Option<&str>,
    reason: &str,
    now: i64,
) -> Result<()> {
    let live = backup_mount.join(live_rel);
    let under_backup = live_rel
        .strip_prefix(BACKUP_DIR)
        .unwrap_or(live_rel)
        .trim_start_matches(['\\', '/']);
    let prev = backup_root(backup_mount)
        .join(PREVIOUS_DIR)
        .join(format_date_ms(now))
        .join(under_backup);
    let prev = unique_path(&prev, None);
    let size = std::fs::metadata(&live).map(|m| m.len() as i64).unwrap_or(0);
    if live.exists() {
        if let Some(parent) = prev.parent() {
            std::fs::create_dir_all(parent)?;
        }
        std::fs::rename(&live, &prev)?;
    }
    let prev_rel = rel_under(&normalize_path(&prev, false).0, &normalize_path(backup_mount, false).0);
    let (_, live_hash) = normalize_path(&live, false);
    let res = store.with_full_sync_tx(|tx| {
        tx.execute(
            "INSERT INTO backup_versions (id, backup_volume_id, rel_path, original_rel_path, reason, size_bytes,
                                          moved_at, prune_after, source_path)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9)",
            params![new_id(), backup_volume_id, prev_rel, live_rel, reason, size, now, now + PREVIOUS_VERSION_TTL_MS, source_path],
        )?;
        if reason != "changed" {
            tx.execute(
                "DELETE FROM backup_copies WHERE backup_volume_id = ?1 AND backup_rel_path = ?2",
                params![backup_volume_id, live_rel],
            )?;
        }
        tx.execute(
            "UPDATE files SET deleted_at = ?2, state = 'missing', updated_at = ?2
             WHERE path_hash = ?1 AND deleted_at IS NULL",
            params![live_hash.as_slice(), now],
        )?;
        Ok(())
    });
    if res.is_err() && prev.exists() {
        let _ = std::fs::rename(&prev, &live); // undo the disk move
    }
    res
}

/// `path`, or `stem (restored).ext` / `stem (2).ext` … when it is taken.
fn unique_path(path: &Path, tag: Option<&str>) -> PathBuf {
    if !path.exists() {
        return path.to_path_buf();
    }
    let stem = path.file_stem().map(|s| s.to_string_lossy().to_string()).unwrap_or_default();
    let ext = path.extension().map(|e| format!(".{}", e.to_string_lossy())).unwrap_or_default();
    for n in 1.. {
        let label = match (tag, n) {
            (Some(t), 1) => t.to_string(),
            (Some(t), n) => format!("{t} {n}"),
            (None, n) => (n + 1).to_string(),
        };
        let candidate = path.with_file_name(format!("{stem} ({label}){ext}"));
        if !candidate.exists() {
            return candidate;
        }
    }
    unreachable!()
}

fn index_backup_file(tx: &Connection, dest: &Path, hash: &[u8]) -> Result<Option<String>> {
    let Some((vid, _, cs)) = resolve_online_volume(tx, dest)? else { return Ok(None) };
    let stable: bool = tx.query_row("SELECT has_stable_inode FROM volumes WHERE id = ?1", [&vid], |r| r.get(0))?;
    let Some(stat) = inspect_path_metadata(dest)? else { return Ok(None) };
    Ok(Some(upsert_discovered_file(
        tx,
        &crate::db::files::DiscoveredFileInput {
            volume_id: vid,
            has_stable_inode: stable,
            case_sensitive: cs,
            file_index: stat.file_index,
            nlink: stat.nlink,
            atomic_unit_id: None,
            path: dest.to_path_buf(),
            size: stat.size,
            mtime: stat.mtime,
            ctime: stat.ctime,
            blake3: Some(hash.to_vec()),
            cloud_state: stat.cloud_state,
        },
    )?))
}

enum CopyOutcome {
    Done { hash: Vec<u8>, bytes: i64 },
    Changed,
    Cancelled,
}

fn copy_verified(
    src: &Path,
    partial: &Path,
    cancel: &AtomicBool,
    done: &mut i64,
    total: i64,
    progress: &mut dyn FnMut(i64, i64),
) -> Result<CopyOutcome> {
    let before = std::fs::metadata(src)?;
    let mut input = std::fs::File::open(src)?;
    let mut out = std::fs::File::create(partial)?;
    let mut h = blake3::Hasher::new();
    let mut buf = vec![0u8; 256 * 1024];
    let mut bytes = 0i64;
    loop {
        let n = input.read(&mut buf)?;
        if n == 0 {
            break;
        }
        h.update(&buf[..n]);
        out.write_all(&buf[..n])?;
        bytes += n as i64;
        *done += n as i64;
        progress(*done, total);
        if cancel.load(Ordering::Relaxed) {
            drop(out);
            let _ = std::fs::remove_file(partial);
            return Ok(CopyOutcome::Cancelled);
        }
    }
    out.sync_all()?;
    drop(out);
    let after = std::fs::metadata(src)?;
    if after.len() != before.len() || mtime_ms(&after) != mtime_ms(&before) {
        let _ = std::fs::remove_file(partial);
        return Ok(CopyOutcome::Changed);
    }
    Ok(CopyOutcome::Done { hash: h.finalize().as_bytes().to_vec(), bytes })
}

/// Runs a top-up plan. Each file is its own disk step plus a short transaction; cancelling
/// (or unplugging) stops cleanly and the next run picks up where this one left off.
/// `progress` receives `(bytes copied so far, total bytes to copy)`.
pub fn run_topup(
    store: &mut SqliteStore,
    plan: &TopUpPlan,
    authority: TopUpAuthority,
    cancel: &AtomicBool,
    progress: &mut dyn FnMut(i64, i64),
) -> Result<TopUpResult> {
    let (mount, online): (String, bool) = store
        .conn()
        .query_row(
            "SELECT mount_point, status = 'online' FROM volumes WHERE id = ?1",
            [&plan.backup_volume_id],
            |r| Ok((r.get(0)?, r.get(1)?)),
        )
        .optional()?
        .ok_or_else(|| AegisError::NotFound(format!("volume {}", plan.backup_volume_id)))?;
    if !online {
        return Err(AegisError::VolumeOffline("Plug in the Backup drive first.".into()));
    }
    if authority == TopUpAuthority::Standing && !crate::policy::has_standing_topup(store.conn(), &plan.backup_volume_id)? {
        return Err(AegisError::PolicyBlocked {
            rule: "standing_approval".into(),
            detail: "Automatic backups need your approval of a first top-up on this drive.".into(),
        });
    }
    let backup_mount = PathBuf::from(&mount);
    let area = backup_root(&backup_mount);
    // Defence in depth: every item must stay inside "Aegis Backup" on this drive.
    for it in &plan.items {
        let target = backup_mount.join(&it.backup_rel_path);
        let escapes = it.backup_rel_path.split(['\\', '/']).any(|c| c == "..")
            || !path_is_within(&target.to_string_lossy(), &area.to_string_lossy(), false);
        if escapes {
            return Err(AegisError::PolicyBlocked {
                rule: "backup_scope".into(),
                detail: format!("{} is outside the Backup drive's Aegis Backup folder", it.backup_rel_path),
            });
        }
    }
    remove_partials(&area);
    if plan.prune_bytes > 0 {
        prune_oldest(store, &plan.backup_volume_id, &backup_mount, plan.prune_bytes)?;
    }

    let job_id = new_id();
    let started = now_ms();
    let total_bytes: i64 = plan
        .items
        .iter()
        .filter(|i| matches!(i.kind, TopUpKind::Add | TopUpKind::Replace))
        .map(|i| i.bytes)
        .sum();
    store.conn().execute(
        "INSERT INTO backup_jobs (id, backup_volume_id, authority, status, total_count, started_at)
         VALUES (?1, ?2, ?3, 'running', ?4, ?5)",
        params![
            job_id,
            plan.backup_volume_id,
            if authority == TopUpAuthority::Standing { "standing" } else { "user" },
            plan.items.len() as i64,
            started
        ],
    )?;
    let mut res = TopUpResult {
        job_id: job_id.clone(),
        added: 0,
        replaced: 0,
        retired: 0,
        bytes_copied: 0,
        skipped_changed: 0,
        completed: false,
    };
    let mut done_bytes = 0i64;
    let mut done_items = 0i64;
    let mut cancelled = false;

    for it in &plan.items {
        if cancel.load(Ordering::Relaxed) {
            cancelled = true;
            break;
        }
        let now = now_ms();
        match it.kind {
            TopUpKind::Add | TopUpKind::Replace => {
                let Some(src) = it.source_path.as_deref().map(PathBuf::from) else { continue };
                if !src.is_file() {
                    continue;
                }
                let dest = backup_mount.join(&it.backup_rel_path);
                if let Some(parent) = dest.parent() {
                    std::fs::create_dir_all(parent)?;
                }
                let partial = PathBuf::from(format!("{}.aegis-partial", dest.display()));
                match copy_verified(&src, &partial, cancel, &mut done_bytes, total_bytes, progress)? {
                    CopyOutcome::Cancelled => {
                        cancelled = true;
                        break;
                    }
                    CopyOutcome::Changed => {
                        res.skipped_changed += 1;
                        continue;
                    }
                    CopyOutcome::Done { hash, bytes } => {
                        if dest.exists() {
                            retire_copy(store, &plan.backup_volume_id, &backup_mount, &it.backup_rel_path, it.source_path.as_deref(), "changed", now)?;
                        }
                        std::fs::rename(&partial, &dest)?;
                        let src_meta = std::fs::metadata(&src).ok();
                        let (size, mtime) = src_meta.map(|m| (m.len() as i64, mtime_ms(&m))).unwrap_or((-1, -1));
                        let committed = store.with_full_sync_tx(|tx| {
                            tx.execute(
                                "INSERT INTO backup_copies (id, source_file_id, backup_volume_id, backup_rel_path, source_blake3, copied_at, last_verified_at)
                                 VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?6)
                                 ON CONFLICT(backup_volume_id, backup_rel_path) DO UPDATE SET
                                   source_file_id = excluded.source_file_id, source_blake3 = excluded.source_blake3,
                                   copied_at = excluded.copied_at, last_verified_at = excluded.last_verified_at",
                                params![new_id(), it.source_file_id, plan.backup_volume_id, it.backup_rel_path, hash, now],
                            )?;
                            // The copy proved the source's content; remember it for next time.
                            if let Some(sid) = &it.source_file_id {
                                tx.execute(
                                    "UPDATE files SET blake3 = ?2 WHERE id = ?1 AND size = ?3 AND mtime = ?4",
                                    params![sid, hash, size, mtime],
                                )?;
                            }
                            index_backup_file(tx, &dest, &hash)?;
                            Ok(())
                        });
                        if let Err(e) = committed {
                            let _ = std::fs::remove_file(&dest);
                            return Err(e);
                        }
                        res.bytes_copied += bytes;
                        if it.kind == TopUpKind::Add {
                            res.added += 1;
                        } else {
                            res.replaced += 1;
                        }
                    }
                }
            }
            TopUpKind::RetireDeleted | TopUpKind::RetireUncovered => {
                let reason = if it.kind == TopUpKind::RetireDeleted { "deleted" } else { "uncovered" };
                retire_copy(store, &plan.backup_volume_id, &backup_mount, &it.backup_rel_path, it.source_path.as_deref(), reason, now)?;
                res.retired += 1;
            }
        }
        done_items += 1;
        store.conn().execute(
            "UPDATE backup_jobs SET done_count = ?2, bytes_copied = ?3 WHERE id = ?1",
            params![job_id, done_items, res.bytes_copied],
        )?;
    }

    res.completed = !cancelled;
    if res.completed {
        guard::clear_overrides(store, &plan.backup_volume_id)?;
    }
    let finished = now_ms();
    store.with_full_sync_tx(|tx| {
        tx.execute(
            "UPDATE backup_jobs SET status = ?2, finished_at = ?3, done_count = ?4, bytes_copied = ?5 WHERE id = ?1",
            params![job_id, if cancelled { "interrupted" } else { "completed" }, finished, done_items, res.bytes_copied],
        )?;
        append_audit_in_tx(
            tx,
            if authority == TopUpAuthority::Standing { "system" } else { "user" },
            "backup.topup",
            &plan.backup_volume_id,
            &serde_json::json!({
                "job_id": job_id,
                "authority": if authority == TopUpAuthority::Standing { "standing" } else { "user" },
                "added": res.added, "replaced": res.replaced, "retired": res.retired,
                "bytes": res.bytes_copied, "completed": res.completed,
            })
            .to_string(),
        )?;
        Ok(())
    })?;
    Ok(res)
}

/// Deletes previous versions whose 90-day window has passed. They are already the second
/// copy, so this does not go through quarantine. Audited.
pub fn prune_previous_versions(store: &mut SqliteStore, backup_volume_id: &str, now: i64) -> Result<i64> {
    let mount: String = store
        .conn()
        .query_row("SELECT mount_point FROM volumes WHERE id = ?1", [backup_volume_id], |r| r.get(0))?;
    let due: Vec<(String, String)> = {
        let mut stmt = store.conn().prepare(
            "SELECT id, rel_path FROM backup_versions
             WHERE backup_volume_id = ?1 AND pruned_at IS NULL AND prune_after <= ?2",
        )?;
        let rows = stmt.query_map(params![backup_volume_id, now], |r| Ok((r.get(0)?, r.get(1)?)))?;
        rows.collect::<std::result::Result<Vec<_>, _>>()?
    };
    let mut pruned = 0i64;
    for (id, rel) in &due {
        let path = Path::new(&mount).join(rel);
        match std::fs::remove_file(&path) {
            Ok(()) => {}
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => {}
            Err(_) => continue, // locked or unreadable: try again next time
        }
        store.conn().execute(
            "UPDATE backup_versions SET pruned_at = ?2 WHERE id = ?1",
            params![id, now],
        )?;
        pruned += 1;
    }
    if pruned > 0 {
        store.with_full_sync_tx(|tx| {
            append_audit_in_tx(
                tx,
                "system",
                "backup.prune",
                backup_volume_id,
                &serde_json::json!({ "pruned": pruned }).to_string(),
            )?;
            Ok(())
        })?;
    }
    Ok(pruned)
}

pub fn list_previous_versions(conn: &Connection, backup_volume_id: &str) -> Result<Vec<PreviousVersionItem>> {
    let mut stmt = conn.prepare(
        "SELECT id, rel_path, original_rel_path, source_path, reason, size_bytes, moved_at, prune_after
         FROM backup_versions WHERE backup_volume_id = ?1 AND pruned_at IS NULL
         ORDER BY moved_at ASC, id ASC",
    )?;
    let rows = stmt.query_map([backup_volume_id], |r| {
        Ok(PreviousVersionItem {
            id: r.get(0)?,
            rel_path: r.get(1)?,
            original_rel_path: r.get(2)?,
            source_path: r.get(3)?,
            reason: r.get(4)?,
            size_bytes: r.get(5)?,
            moved_at: r.get(6)?,
            prune_after: r.get(7)?,
        })
    })?;
    Ok(rows.collect::<std::result::Result<Vec<_>, _>>()?)
}

/// Copies a previous version back to `to` (default: where its source file lived). Never
/// overwrites: an occupied name becomes `name (restored).ext`. The version itself stays.
pub fn restore_previous_version(store: &mut SqliteStore, version_id: &str, to: Option<PathBuf>) -> Result<PathBuf> {
    let (vol, rel, source): (String, String, Option<String>) = store
        .conn()
        .query_row(
            "SELECT backup_volume_id, rel_path, source_path FROM backup_versions WHERE id = ?1 AND pruned_at IS NULL",
            [version_id],
            |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?)),
        )
        .optional()?
        .ok_or_else(|| AegisError::NotFound(format!("previous version {version_id}")))?;
    let (mount, online): (String, bool) = store
        .conn()
        .query_row("SELECT mount_point, status = 'online' FROM volumes WHERE id = ?1", [&vol], |r| Ok((r.get(0)?, r.get(1)?)))?;
    if !online {
        return Err(AegisError::VolumeOffline("Plug in the Backup drive first.".into()));
    }
    let from = Path::new(&mount).join(&rel);
    let wanted = to
        .or_else(|| source.map(PathBuf::from))
        .ok_or_else(|| AegisError::InvalidInput("Choose where to restore this file.".into()))?;
    let target = unique_path(&wanted, Some("restored"));
    if let Some(parent) = target.parent() {
        std::fs::create_dir_all(parent)?;
    }
    std::fs::copy(&from, &target)?;
    store.with_full_sync_tx(|tx| {
        append_audit_in_tx(
            tx,
            "user",
            "backup.restore_version",
            version_id,
            &serde_json::json!({ "to": target.to_string_lossy() }).to_string(),
        )?;
        Ok(())
    })?;
    Ok(target)
}
