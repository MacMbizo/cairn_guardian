//! Per-volume `O(1)` quarantine, hardlink-safe space math, and Space Clarity (`ADR-003`, `V1.5-01`).

use crate::contracts::{QuarantineEntry, QuarantineSpaceSummary, TargetKind};
use crate::db::SqliteStore;
use crate::db::audit::append_audit_in_tx;
use crate::db::files::soft_delete_file_in_tx;
use crate::error::{AegisError, Result};
use crate::scanner::os_adapter::disk_space_for_mount;
use crate::{new_id, now_ms};
use rusqlite::{Connection, OptionalExtension, params};
use std::collections::{HashMap, HashSet};
use std::path::{Path, PathBuf};

const DEFAULT_TTL_MS: i64 = 30 * 24 * 60 * 60 * 1000;
const DEFAULT_SOFT_QUOTA_PCT: f64 = 10.0;

/// Computes exact hardlink-adjusted reclaimable bytes for a duplicate group (`ADR-003` / `B4` §5).
///
/// Entries sharing the same `(volume_id, file_index)` report `0` reclaimable bytes unless
/// every link in that inode group except the keeper is removed.
pub fn calculate_reclaimable_bytes(
    members: &[(String, Option<Vec<u8>>, i64, i64)], // (volume_id, file_index, nlink, size)
    keeper_idx: usize,
) -> (u64, bool) {
    if members.len() <= 1 || keeper_idx >= members.len() {
        return (0, false);
    }

    let mut hardlink_adjusted = false;
    let mut inode_counts: HashMap<(&str, &[u8]), (usize, i64, i64)> = HashMap::new();

    for (idx, (vol, fi_opt, nlink, size)) in members.iter().enumerate() {
        if *nlink > 1 {
            hardlink_adjusted = true;
        }
        if let Some(fi) = fi_opt {
            let entry = inode_counts
                .entry((vol.as_str(), fi.as_slice()))
                .or_insert((0, *nlink, *size));
            if idx != keeper_idx {
                entry.0 += 1;
            }
        }
    }

    let mut reclaimable: u64 = 0;
    let mut counted_inodes: HashSet<(&str, &[u8])> = HashSet::new();

    for (idx, (vol, fi_opt, nlink, size)) in members.iter().enumerate() {
        if idx == keeper_idx || *size <= 0 {
            continue;
        }
        if *nlink <= 1 {
            reclaimable = reclaimable.saturating_add(*size as u64);
            continue;
        }
        if let Some(fi) = fi_opt {
            let key = (vol.as_str(), fi.as_slice());
            if let Some(&(removed_in_cluster, total_nlink, sz)) = inode_counts.get(&key) {
                // Only reclaim inode blocks if ALL links to this inode in the cluster are removed
                // AND the keeper doesn't share this inode, OR all nlink copies are removed.
                let keeper_shares_inode = members[keeper_idx]
                    .1
                    .as_deref()
                    == Some(fi.as_slice())
                    && members[keeper_idx].0 == *vol;
                if !keeper_shares_inode
                    && removed_in_cluster as i64 >= total_nlink
                    && counted_inodes.insert(key)
                {
                    reclaimable = reclaimable.saturating_add(sz as u64);
                }
            }
        }
    }

    (reclaimable, hardlink_adjusted)
}

/// Computes the `<VolumeRoot>/.aegis-quarantine/<batch_id>/<name>` target path.
pub fn build_quarantine_dest(
    quarantine_root: &Path,
    batch_id: &str,
    action_id: &str,
    original_path: &Path,
) -> PathBuf {
    let name = original_path
        .file_name()
        .map(|s| s.to_string_lossy().to_string())
        .unwrap_or_else(|| "item".to_string());
    quarantine_root
        .join(batch_id)
        .join(format!("{action_id}_{name}"))
}

/// Checks if a file or directory can be moved atomically via `O(1)` rename into quarantine.
pub fn move_to_quarantine_fs(from_path: &Path, quarantine_path: &Path) -> Result<()> {
    if !from_path.exists() {
        return Err(AegisError::NotFound(from_path.display().to_string()));
    }
    if let Some(parent) = quarantine_path.parent() {
        std::fs::create_dir_all(parent).map_err(|e| {
            AegisError::QuarantineUnavailable(format!(
                "Cannot create quarantine dir {}: {e}",
                parent.display()
            ))
        })?;
    }
    std::fs::rename(from_path, quarantine_path).map_err(|e| {
        if e.kind() == std::io::ErrorKind::CrossesDevices {
            AegisError::QuarantineUnavailable(format!(
                "Cross-volume move refused without explicit confirmation: {e}"
            ))
        } else if e.kind() == std::io::ErrorKind::PermissionDenied {
            AegisError::PermissionDenied(from_path.display().to_string())
        } else {
            AegisError::QuarantineUnavailable(e.to_string())
        }
    })
}

pub fn list_quarantine(
    conn: &Connection,
    volume_id: Option<&str>,
) -> Result<Vec<QuarantineEntry>> {
    let sql = match volume_id {
        Some(_) => {
            "SELECT id, action_id, target_kind, file_id, atomic_unit_id, volume_id,
                    original_path, quarantine_path, size_bytes, quarantined_at,
                    expires_at, restored_at, purged_at
             FROM quarantine_entries
             WHERE volume_id = ?1 AND restored_at IS NULL AND purged_at IS NULL
             ORDER BY quarantined_at DESC"
        }
        None => {
            "SELECT id, action_id, target_kind, file_id, atomic_unit_id, volume_id,
                    original_path, quarantine_path, size_bytes, quarantined_at,
                    expires_at, restored_at, purged_at
             FROM quarantine_entries
             WHERE restored_at IS NULL AND purged_at IS NULL
             ORDER BY quarantined_at DESC"
        }
    };

    let mut stmt = conn.prepare(sql)?;
    let map_row = |r: &rusqlite::Row<'_>| -> rusqlite::Result<QuarantineEntry> {
        let tk: String = r.get(2)?;
        Ok(QuarantineEntry {
            id: r.get(0)?,
            action_id: r.get(1)?,
            target_kind: TargetKind::from_db(&tk),
            file_id: r.get(3)?,
            atomic_unit_id: r.get(4)?,
            volume_id: r.get(5)?,
            original_path: r.get(6)?,
            quarantine_path: r.get(7)?,
            size_bytes: r.get(8)?,
            quarantined_at: r.get(9)?,
            expires_at: r.get(10)?,
            restored_at: r.get(11)?,
            purged_at: r.get(12)?,
        })
    };

    let mut out = Vec::new();
    if let Some(vid) = volume_id {
        for r in stmt.query_map(params![vid], map_row)? {
            out.push(r?);
        }
    } else {
        for r in stmt.query_map([], map_row)? {
            out.push(r?);
        }
    }
    Ok(out)
}

/// Returns `Staged in Quarantine (30d)` vs. `Actually Freed on Disk` + 10% soft quota warning (`V1.5-01` promoted to v1.0).
pub fn space_summary(
    store: &SqliteStore,
    volume_id: Option<&str>,
) -> Result<Vec<QuarantineSpaceSummary>> {
    let conn = store.conn();
    let soft_quota_pct: f64 = store
        .get_setting("quarantine.max_volume_pct", "global")?
        .and_then(|s| serde_json::from_str::<f64>(&s).ok())
        .unwrap_or(DEFAULT_SOFT_QUOTA_PCT);

    let mut stmt = match volume_id {
        Some(_) => conn.prepare(
            "SELECT id, mount_point FROM volumes WHERE id = ?1 ORDER BY mount_point",
        )?,
        None => conn.prepare("SELECT id, mount_point FROM volumes ORDER BY mount_point")?,
    };

    let mut vols = Vec::new();
    if let Some(vid) = volume_id {
        for r in stmt.query_map(params![vid], |r| {
            Ok((r.get::<_, String>(0)?, r.get::<_, String>(1)?))
        })? {
            vols.push(r?);
        }
    } else {
        for r in stmt.query_map([], |r| {
            Ok((r.get::<_, String>(0)?, r.get::<_, String>(1)?))
        })? {
            vols.push(r?);
        }
    }

    let mut out = Vec::new();
    for (vid, mount_point) in vols {
        let (staged_bytes, staged_count): (i64, i64) = conn.query_row(
            "SELECT COALESCE(SUM(size_bytes), 0), COUNT(*)
             FROM quarantine_entries
             WHERE volume_id = ?1 AND restored_at IS NULL AND purged_at IS NULL",
            params![vid],
            |r| Ok((r.get(0)?, r.get(1)?)),
        )?;
        let (freed_bytes, purged_count): (i64, i64) = conn.query_row(
            "SELECT COALESCE(SUM(size_bytes), 0), COUNT(*)
             FROM quarantine_entries
             WHERE volume_id = ?1 AND purged_at IS NOT NULL",
            params![vid],
            |r| Ok((r.get(0)?, r.get(1)?)),
        )?;
        let (total_bytes, free_bytes) = disk_space_for_mount(&mount_point);
        let staged_u64 = staged_bytes.max(0) as u64;
        let quota_exceeded = total_bytes > 0
            && (((staged_u64 as f64 / total_bytes as f64) * 100.0) >= soft_quota_pct
                || ((free_bytes as f64 / total_bytes as f64) * 100.0) < 10.0);

        out.push(QuarantineSpaceSummary {
            volume_id: vid,
            staged_bytes: staged_u64,
            staged_count: staged_count.max(0) as usize,
            freed_bytes: freed_bytes.max(0) as u64,
            purged_count: purged_count.max(0) as usize,
            volume_total_bytes: total_bytes,
            volume_free_bytes: free_bytes,
            soft_quota_pct,
            quota_warning: quota_exceeded && staged_u64 > 0,
        });
    }
    Ok(out)
}

struct ActiveQuarantineRow {
    target_kind: String,
    file_id: Option<String>,
    atomic_unit_id: Option<String>,
    original_path: String,
    quarantine_path: String,
}

fn fetch_active_quarantine_row(
    tx: &rusqlite::Transaction<'_>,
    entry_id: &str,
) -> Result<Option<ActiveQuarantineRow>> {
    tx.query_row(
        "SELECT target_kind, file_id, atomic_unit_id, original_path, quarantine_path
         FROM quarantine_entries
         WHERE id = ?1 AND restored_at IS NULL AND purged_at IS NULL",
        params![entry_id],
        |r| {
            Ok(ActiveQuarantineRow {
                target_kind: r.get(0)?,
                file_id: r.get(1)?,
                atomic_unit_id: r.get(2)?,
                original_path: r.get(3)?,
                quarantine_path: r.get(4)?,
            })
        },
    )
    .optional()
    .map_err(AegisError::Database)
}

/// Permanently deletes quarantined items ONLY when `confirmed_permanent == true` (`C1` §3).
pub fn purge_entries(
    store: &mut SqliteStore,
    entry_ids: &[String],
    confirmed_permanent: bool,
) -> Result<String> {
    if !confirmed_permanent {
        return Err(AegisError::PolicyBlocked {
            rule: "purge_requires_confirm".into(),
            detail: "Permanent quarantine purge requires confirmedPermanent=true.".into(),
        });
    }
    let batch_id = new_id();
    let now = now_ms();

    store.with_full_sync_tx(|tx| {
        tx.execute(
            "INSERT INTO action_batches (id, origin, status, total_actions, created_at, executed_at)
             VALUES (?1, 'user_undo', 'executed', ?2, ?3, ?3)",
            params![batch_id, entry_ids.len() as i64, now],
        )?;

        for entry_id in entry_ids {
            let Some(row) = fetch_active_quarantine_row(tx, entry_id)? else {
                continue;
            };

            let q = Path::new(&row.quarantine_path);
            if q.exists() {
                if q.is_dir() {
                    std::fs::remove_dir_all(q)?;
                } else {
                    std::fs::remove_file(q)?;
                }
            }

            if let Some(ref fid) = row.file_id {
                soft_delete_file_in_tx(tx, fid)?;
            }
            if row.target_kind == "dir" {
                tx.execute(
                    "UPDATE cache_dirs SET state = 'missing', updated_at = ?1 WHERE path = ?2",
                    params![now, row.original_path],
                )?;
            }

            let action_id = new_id();
            tx.execute(
                "INSERT INTO actions (
                    id, batch_id, agent, action_type, target_kind, file_id, atomic_unit_id,
                    from_path, to_path, reason, confidence, evidence_json, status, created_at, executed_at
                 ) VALUES (?1, ?2, NULL, 'purge', ?3, ?4, ?5, ?6, NULL, 'Permanent quarantine purge', 1.0, '[]', 'executed', ?7, ?7)",
                params![
                    action_id,
                    batch_id,
                    row.target_kind,
                    row.file_id,
                    row.atomic_unit_id,
                    row.quarantine_path,
                    now
                ],
            )?;

            tx.execute(
                "UPDATE quarantine_entries SET purged_at = ?1 WHERE id = ?2",
                params![now, entry_id],
            )?;

            append_audit_in_tx(
                tx,
                "user",
                "quarantine.purge",
                &row.quarantine_path,
                &serde_json::json!({ "entry_id": entry_id, "action_id": action_id }).to_string(),
            )?;
        }
        Ok(())
    })?;

    Ok(batch_id)
}

/// Restores quarantined items back to `original_path` (using unique collision suffix if reoccupied).
pub fn restore_entries(store: &mut SqliteStore, entry_ids: &[String]) -> Result<String> {
    let batch_id = new_id();
    let now = now_ms();

    store.with_full_sync_tx(|tx| {
        tx.execute(
            "INSERT INTO action_batches (id, origin, status, total_actions, created_at, executed_at)
             VALUES (?1, 'user_undo', 'executed', ?2, ?3, ?3)",
            params![batch_id, entry_ids.len() as i64, now],
        )?;

        for entry_id in entry_ids {
            let Some(row) = fetch_active_quarantine_row(tx, entry_id)? else {
                continue;
            };
            let tk = row.target_kind;
            let file_id = row.file_id;
            let atomic_unit_id = row.atomic_unit_id;
            let orig = row.original_path;
            let qpath = row.quarantine_path;

            let src = Path::new(&qpath);
            if !src.exists() {
                return Err(AegisError::NotFound(format!("quarantine path {qpath}")));
            }
            let mut dest = PathBuf::from(&orig);
            if dest.exists() {
                let stem = dest
                    .file_stem()
                    .map(|s| s.to_string_lossy().to_string())
                    .unwrap_or_else(|| "restored".to_string());
                let ext = dest.extension().map(|e| e.to_string_lossy().to_string());
                let new_name = match ext {
                    Some(e) => format!("{stem}.restored-{now}.{e}"),
                    None => format!("{stem}.restored-{now}"),
                };
                dest.set_file_name(new_name);
            }
            if let Some(parent) = dest.parent() {
                std::fs::create_dir_all(parent)?;
            }
            std::fs::rename(src, &dest)?;

            if let Some(ref fid) = file_id {
                let dest_str = dest.to_string_lossy().to_string();
                let (_, dest_hash) = crate::db::files::normalize_path(&dest, false);
                tx.execute(
                    "UPDATE files SET state = 'active', path = ?1, path_hash = ?2, updated_at = ?3 WHERE id = ?4",
                    params![dest_str, dest_hash.as_slice(), now, fid],
                )?;
                tx.execute(
                    "INSERT INTO outbox_events (aggregate_type, aggregate_id, op, payload_json, created_at)
                     VALUES ('file_payload', ?1, 'upsert', '{\"state\":\"active\"}', ?2)",
                    params![fid, now],
                )?;
            }
            if tk == "dir" {
                tx.execute(
                    "UPDATE cache_dirs SET state = 'active', updated_at = ?1 WHERE path = ?2",
                    params![now, orig],
                )?;
            }

            let action_id = new_id();
            let dest_str = dest.to_string_lossy().to_string();
            tx.execute(
                "INSERT INTO actions (
                    id, batch_id, agent, action_type, target_kind, file_id, atomic_unit_id,
                    from_path, to_path, reason, confidence, evidence_json, status, created_at, executed_at
                 ) VALUES (?1, ?2, NULL, 'restore', ?3, ?4, ?5, ?6, ?7, 'User restore from quarantine', 1.0, '[]', 'executed', ?8, ?8)",
                params![action_id, batch_id, tk, file_id, atomic_unit_id, qpath, dest_str, now],
            )?;

            tx.execute(
                "UPDATE quarantine_entries SET restored_at = ?1 WHERE id = ?2",
                params![now, entry_id],
            )?;

            append_audit_in_tx(
                tx,
                "user",
                "quarantine.restore",
                &dest_str,
                &serde_json::json!({ "entry_id": entry_id, "from": qpath }).to_string(),
            )?;
        }
        Ok(())
    })?;

    Ok(batch_id)
}

pub fn default_ttl_ms() -> i64 {
    DEFAULT_TTL_MS
}
