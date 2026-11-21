//! Drive registry: identity matching, clone detection, offline lifecycle, re-letter rebase (spec §7.1–§7.2).

use super::passport::{read_passport, regenerate_passport, write_passport};
use super::provider::{MountedVolume, VolumeProvider};
use crate::db::SqliteStore;
use crate::db::audit::append_audit_in_tx;
use crate::db::files::{normalize_path, path_is_within};
use crate::error::Result;
use crate::now_ms;
use crate::policy::revoke_standing_for_passport;
use rusqlite::{Connection, OptionalExtension, params};
use std::collections::{HashMap, HashSet};
use std::path::Path;

/// What changed in one `sync_mounted` pass.
#[derive(Debug, Clone, Default, PartialEq, Eq, serde::Serialize, serde::Deserialize, ts_rs::TS)]
#[ts(export)]
pub struct SyncReport {
    /// Volume ids that came online in this pass (includes new drives).
    pub arrived: Vec<String>,
    /// Volume ids that went offline in this pass.
    pub departed: Vec<String>,
    /// Volume ids first seen in this pass.
    pub new_drives: Vec<String>,
    /// `(volume id, old mount, new mount)` for known drives seen at a new mount point.
    pub relettered: Vec<(String, String, String)>,
    pub clones: Vec<CloneConflict>,
}

#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize, ts_rs::TS)]
#[ts(export)]
pub struct CloneConflict {
    pub kept_volume_id: String,
    pub new_volume_id: String,
    pub mount_point: String,
}

/// Canonical display form of a mount point, matching how scanned paths are stored.
fn canonical_mount(mount: &Path) -> String {
    let canonical = std::fs::canonicalize(mount).unwrap_or_else(|_| mount.to_path_buf());
    normalize_path(&canonical, false).0
}

struct Identified {
    mounted: MountedVolume,
    mount: String,
    passport_id: Option<String>,
}

#[derive(Debug)]
struct VolRow {
    id: String,
    mount_point: String,
    status: String,
}

fn row_by(conn: &Connection, where_sql: &str, p: &[&dyn rusqlite::ToSql]) -> Result<Option<VolRow>> {
    let sql = format!("SELECT id, mount_point, status FROM volumes WHERE {where_sql} LIMIT 1");
    Ok(conn
        .query_row(&sql, p, |r| {
            Ok(VolRow {
                id: r.get(0)?,
                mount_point: r.get(1)?,
                status: r.get(2)?,
            })
        })
        .optional()?)
}

/// Fingerprint id for drives that cannot carry a passport (read-only).
fn fingerprint_id(v: &MountedVolume) -> String {
    let key = format!(
        "{}|{}|{}",
        v.guid_path.as_deref().unwrap_or(""),
        v.serial.as_deref().unwrap_or(""),
        v.total_bytes
    );
    format!("fp-{}", &blake3::hash(key.as_bytes()).to_hex()[..16])
}

/// Brings the `volumes` table in line with what is mounted right now.
///
/// Order: read/write passports → resolve clones → match each mounted drive to a known row
/// (passport → GUID → serial+size → new) → mark every other registry-managed volume offline.
pub fn sync_mounted(store: &mut SqliteStore, provider: &dyn VolumeProvider) -> Result<SyncReport> {
    let mut report = SyncReport::default();
    let mounted = provider.mounted()?;

    // 1. Passports.
    let mut found: Vec<Identified> = Vec::new();
    for m in mounted.into_iter().filter(|m| !m.is_system) {
        let mount = canonical_mount(&m.mount_point);
        let passport = match read_passport(&m.mount_point)? {
            Some(p) => Some(p),
            None if !m.is_readonly => write_passport(&m.mount_point).ok(),
            None => None,
        };
        found.push(Identified {
            passport_id: passport.map(|p| p.passport_id),
            mounted: m,
            mount,
        });
    }

    // 2. Clones: two mounted drives carrying the same passport.
    let mut by_passport: HashMap<String, Vec<usize>> = HashMap::new();
    for (i, f) in found.iter().enumerate() {
        if let Some(p) = &f.passport_id {
            by_passport.entry(p.clone()).or_default().push(i);
        }
    }
    let mut clone_of: Vec<(usize, String)> = Vec::new(); // (index of clone, kept passport)
    for (pid, idxs) in by_passport.iter().filter(|(_, v)| v.len() > 1) {
        let known_mount = row_by(store.conn(), "passport_id = ?1", &[pid])?.map(|r| r.mount_point);
        let keep = idxs
            .iter()
            .copied()
            .find(|&i| known_mount.as_deref().is_some_and(|m| m.eq_ignore_ascii_case(&found[i].mount)))
            .unwrap_or(idxs[0]);
        for &i in idxs.iter().filter(|&&i| i != keep) {
            let fresh = regenerate_passport(&found[i].mounted.mount_point)?;
            found[i].passport_id = Some(fresh.passport_id);
            clone_of.push((i, pid.clone()));
        }
    }

    // 3. Match and upsert each mounted drive.
    let ts = now_ms();
    let mut online_ids: HashSet<String> = HashSet::new();
    let mut ids_by_index: Vec<String> = Vec::with_capacity(found.len());
    for f in &found {
        let m = &f.mounted;
        let existing = match &f.passport_id {
            Some(pid) => {
                let by_pp = row_by(store.conn(), "passport_id = ?1", &[pid])?;
                if by_pp.is_some() {
                    by_pp
                } else {
                    legacy_match(store.conn(), m)?
                }
            }
            None => {
                let fp = fingerprint_id(m);
                match row_by(store.conn(), "id = ?1", &[&fp])? {
                    Some(r) => Some(r),
                    None => legacy_match(store.conn(), m)?,
                }
            }
        };
        let id = match &existing {
            Some(r) => r.id.clone(),
            None => match &f.passport_id {
                Some(pid) => format!("pp-{pid}"),
                None => fingerprint_id(m),
            },
        };

        let qdir = Path::new(&f.mount).join(".aegis-quarantine").to_string_lossy().to_string();
        let has_stable_inode = matches!(m.fs_type.as_str(), "ntfs" | "refs");
        let mount = f.mount.clone();
        let passport_id = f.passport_id.clone();
        let existing_ref = existing.as_ref();
        store.with_full_sync_tx(|tx| {
            if let Some(r) = existing_ref
                && !r.mount_point.eq_ignore_ascii_case(&mount)
            {
                rebase_volume_paths(tx, &r.id, &r.mount_point, &mount, false)?;
            }
            tx.execute(
                "INSERT INTO volumes (id, mount_point, fs_type, is_removable, is_readonly, has_stable_inode,
                                      case_sensitive, quarantine_dir, status, last_seen,
                                      passport_id, guid_path, label, serial, total_bytes)
                 VALUES (?1, ?2, ?3, ?4, ?5, ?6, 0, ?7, 'online', ?8, ?9, ?10, ?11, ?12, ?13)
                 ON CONFLICT(id) DO UPDATE SET
                   mount_point = excluded.mount_point, fs_type = excluded.fs_type,
                   is_removable = excluded.is_removable, is_readonly = excluded.is_readonly,
                   has_stable_inode = excluded.has_stable_inode, quarantine_dir = excluded.quarantine_dir,
                   status = 'online', last_seen = excluded.last_seen,
                   passport_id = excluded.passport_id, guid_path = excluded.guid_path,
                   label = excluded.label, serial = excluded.serial, total_bytes = excluded.total_bytes",
                params![
                    id,
                    mount,
                    m.fs_type,
                    i32::from(m.is_removable),
                    i32::from(m.is_readonly),
                    i32::from(has_stable_inode),
                    qdir,
                    ts,
                    passport_id,
                    m.guid_path,
                    m.label,
                    m.serial,
                    m.total_bytes as i64
                ],
            )?;
            Ok(())
        })?;

        match &existing {
            None => {
                report.new_drives.push(id.clone());
                report.arrived.push(id.clone());
            }
            Some(r) => {
                if r.status != "online" {
                    report.arrived.push(id.clone());
                }
                if !r.mount_point.eq_ignore_ascii_case(&f.mount) {
                    report.relettered.push((id.clone(), r.mount_point.clone(), f.mount.clone()));
                }
            }
        }
        online_ids.insert(id.clone());
        ids_by_index.push(id);
    }

    for (i, kept_pid) in clone_of {
        let kept = row_by(store.conn(), "passport_id = ?1", &[&kept_pid])?
            .map(|r| r.id)
            .unwrap_or_default();
        let new_pid = found[i].passport_id.clone().unwrap_or_default();
        let conflict = CloneConflict {
            kept_volume_id: kept,
            new_volume_id: ids_by_index[i].clone(),
            mount_point: found[i].mount.clone(),
        };
        store.with_full_sync_tx(|tx| {
            revoke_standing_for_passport(tx, &kept_pid, "clone_detected")?;
            revoke_standing_for_passport(tx, &new_pid, "clone_detected")?;
            append_audit_in_tx(
                tx,
                "system",
                "drive.clone_detected",
                &conflict.new_volume_id,
                &serde_json::json!({
                    "kept_volume_id": conflict.kept_volume_id,
                    "mount_point": conflict.mount_point,
                })
                .to_string(),
            )?;
            Ok(())
        })?;
        report.clones.push(conflict);
    }

    // 4. Every other registry-managed volume that was online is now offline.
    let stale: Vec<VolRow> = {
        let mut stmt = store.conn().prepare(
            "SELECT id, mount_point, status FROM volumes
             WHERE status = 'online' AND (passport_id IS NOT NULL OR id LIKE 'fp-%')",
        )?;
        let rows = stmt.query_map([], |r| {
            Ok(VolRow {
                id: r.get(0)?,
                mount_point: r.get(1)?,
                status: r.get(2)?,
            })
        })?;
        rows.collect::<std::result::Result<Vec<_>, _>>()?
            .into_iter()
            .filter(|r| !online_ids.contains(&r.id))
            .collect()
    };
    for r in stale {
        let jobs: Vec<(String, String)> = {
            let mut stmt = store
                .conn()
                .prepare("SELECT id, root_path FROM scan_jobs WHERE status = 'running'")?;
            let rows = stmt.query_map([], |row| Ok((row.get(0)?, row.get(1)?)))?;
            rows.collect::<std::result::Result<Vec<_>, _>>()?
        };
        store.with_full_sync_tx(|tx| {
            tx.execute(
                "UPDATE volumes SET status = 'offline', last_seen = ?2 WHERE id = ?1",
                params![r.id, ts],
            )?;
            for (job, _) in jobs.iter().filter(|(_, root)| path_is_within(root, &r.mount_point, false)) {
                tx.execute(
                    "UPDATE scan_jobs SET status = 'interrupted', finished_at = ?2 WHERE id = ?1",
                    params![job, ts],
                )?;
            }
            Ok(())
        })?;
        report.departed.push(r.id);
    }

    Ok(report)
}

/// Matches a drive with no passport row to a row registered before passports existed
/// (GUID, then serial + size, then the legacy `win-<serial>` id). Rows already owning a
/// different passport are never matched: the passport on the drive is authoritative.
fn legacy_match(conn: &Connection, m: &MountedVolume) -> Result<Option<VolRow>> {
    if let Some(g) = &m.guid_path
        && let Some(r) = row_by(conn, "guid_path = ?1 AND passport_id IS NULL", &[g])?
    {
        return Ok(Some(r));
    }
    if let Some(s) = &m.serial {
        let total = m.total_bytes as i64;
        if let Some(r) = row_by(
            conn,
            "serial = ?1 AND total_bytes = ?2 AND passport_id IS NULL",
            &[s, &total],
        )? {
            return Ok(Some(r));
        }
        let legacy = format!("win-{s}");
        if let Some(r) = row_by(conn, "id = ?1 AND passport_id IS NULL", &[&legacy])? {
            return Ok(Some(r));
        }
    }
    Ok(None)
}

fn rebased(path: &str, old_mount: &str, new_mount: &str, case_sensitive: bool) -> Option<String> {
    if !path_is_within(path, old_mount, case_sensitive) {
        return None;
    }
    let rest = path.get(old_mount.len()..).unwrap_or("");
    let rest = rest.trim_start_matches(['\\', '/']);
    if rest.is_empty() {
        return Some(new_mount.to_string());
    }
    Some(Path::new(new_mount).join(rest).to_string_lossy().to_string())
}

/// Rewrites every stored path of `volume_id` from `old_mount` to `new_mount` (re-lettered drive).
/// Returns the number of file rows rewritten.
pub fn rebase_volume_paths(
    tx: &Connection,
    volume_id: &str,
    old_mount: &str,
    new_mount: &str,
    case_sensitive: bool,
) -> Result<usize> {
    let mut changed = 0usize;
    for table in ["files", "cache_dirs"] {
        let rows: Vec<(String, String)> = {
            let mut stmt =
                tx.prepare(&format!("SELECT id, path FROM {table} WHERE volume_id = ?1"))?;
            let rows = stmt.query_map([volume_id], |r| Ok((r.get(0)?, r.get(1)?)))?;
            rows.collect::<std::result::Result<Vec<_>, _>>()?
        };
        for (id, path) in rows {
            if let Some(np) = rebased(&path, old_mount, new_mount, case_sensitive) {
                let (display, hash) = normalize_path(Path::new(&np), case_sensitive);
                tx.execute(
                    &format!("UPDATE {table} SET path = ?2, path_hash = ?3 WHERE id = ?1"),
                    params![id, display, hash.as_slice()],
                )?;
                if table == "files" {
                    changed += 1;
                }
            }
        }
    }
    let q: Vec<(String, String, String)> = {
        let mut stmt = tx.prepare(
            "SELECT id, original_path, quarantine_path FROM quarantine_entries WHERE volume_id = ?1",
        )?;
        let rows = stmt.query_map([volume_id], |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?)))?;
        rows.collect::<std::result::Result<Vec<_>, _>>()?
    };
    for (id, orig, qpath) in q {
        let o = rebased(&orig, old_mount, new_mount, case_sensitive).unwrap_or(orig);
        let p = rebased(&qpath, old_mount, new_mount, case_sensitive).unwrap_or(qpath);
        tx.execute(
            "UPDATE quarantine_entries SET original_path = ?2, quarantine_path = ?3 WHERE id = ?1",
            params![id, o, p],
        )?;
    }
    let units: Vec<(String, String)> = {
        let mut stmt = tx.prepare("SELECT id, root_path FROM atomic_units WHERE volume_id = ?1")?;
        let rows = stmt.query_map([volume_id], |r| Ok((r.get(0)?, r.get(1)?)))?;
        rows.collect::<std::result::Result<Vec<_>, _>>()?
    };
    for (id, root) in units {
        if let Some(nr) = rebased(&root, old_mount, new_mount, case_sensitive) {
            tx.execute("UPDATE atomic_units SET root_path = ?2 WHERE id = ?1", params![id, nr])?;
        }
    }
    Ok(changed)
}
