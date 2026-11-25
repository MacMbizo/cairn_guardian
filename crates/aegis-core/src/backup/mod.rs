//! Backup drive: roles and drive plan, mirror top-up, guard and priority fill (spec §7.6–§7.7).
//!
//! Layout on the Backup drive (plain files, browsable without Cairn):
//! `<mount>\Cairn Backup\<source label>\<relative path>` and
//! `<mount>\Cairn Backup\_Previous versions\<yyyy-mm-dd>\<source label>\<relative path>`.
//!
//! Drives backed up before the rename to Cairn hold `Aegis Backup` instead. It is still
//! recognised everywhere, and [`adopt_legacy_backup_dir`] renames it when the drive comes online.

pub mod guard;
pub mod plan;
pub mod priority;
pub mod topup;

use crate::db::SqliteStore;
use crate::db::files::{normalize_path, path_is_within, resolve_online_volume, upsert_discovered_file};
use crate::error::Result;
use crate::now_ms;
use crate::scanner::os_adapter::inspect_path_metadata;
use rusqlite::{Connection, OptionalExtension, params};
use std::path::{Path, PathBuf};

pub const BACKUP_DIR: &str = "Cairn Backup";
/// The Backup folder's name before the rename to Cairn (same length as [`BACKUP_DIR`]).
pub const LEGACY_BACKUP_DIR: &str = "Aegis Backup";
pub const PREVIOUS_DIR: &str = "_Previous versions";

/// True for the Backup folder's name, current or legacy (any case).
pub fn is_backup_dir_name(name: &str) -> bool {
    name.eq_ignore_ascii_case(BACKUP_DIR) || name.eq_ignore_ascii_case(LEGACY_BACKUP_DIR)
}

/// `rel` (relative to the Backup drive's root) with its leading Backup-folder component removed.
pub fn strip_backup_dir(rel: &str) -> &str {
    let first = rel.split(['\\', '/']).next().unwrap_or("");
    if is_backup_dir_name(first) { rel[first.len()..].trim_start_matches(['\\', '/']) } else { rel }
}

/// `<mount>\Cairn Backup`, or the legacy `<mount>\Aegis Backup` while only that one exists
/// (its rename failed or has not run yet), so backups never split across two folders.
pub fn backup_root(mount: &Path) -> PathBuf {
    let current = mount.join(BACKUP_DIR);
    let legacy = mount.join(LEGACY_BACKUP_DIR);
    if !current.exists() && legacy.is_dir() { legacy } else { current }
}

/// Renames an online Backup drive's legacy `Aegis Backup` folder to `Cairn Backup` and rewrites
/// every stored path under it in one transaction. If the catalog update fails, the folder is
/// renamed back. Returns `true` when the folder was renamed.
pub fn adopt_legacy_backup_dir(store: &mut SqliteStore) -> Result<bool> {
    let Some((volume_id, mount, true)) = backup_volume(store.conn())? else { return Ok(false) };
    let legacy = Path::new(&mount).join(LEGACY_BACKUP_DIR);
    let current = Path::new(&mount).join(BACKUP_DIR);
    if !legacy.is_dir() || current.exists() {
        return Ok(false);
    }
    std::fs::rename(&legacy, &current)?;
    let (old_abs, new_abs) = (legacy.to_string_lossy().to_string(), current.to_string_lossy().to_string());
    let passport: Option<String> = store
        .conn()
        .query_row("SELECT passport_id FROM volumes WHERE id = ?1", [&volume_id], |r| r.get(0))
        .optional()?
        .flatten();
    let res = store.with_full_sync_tx(|tx| {
        crate::drives::registry::rebase_volume_paths(tx, &volume_id, &old_abs, &new_abs, false)?;
        // Executed actions (undo history) are not covered by the re-lettering rebase.
        for col in ["from_path", "to_path"] {
            tx.execute(
                &format!(
                    "UPDATE actions SET {col} = ?2 || substr({col}, length(?1) + 1)
                     WHERE lower({col}) = lower(?1)
                        OR lower(substr({col}, 1, length(?1) + 1)) IN (lower(?1) || '\\', lower(?1) || '/')"
                ),
                params![old_abs, new_abs],
            )?;
        }
        // Paths stored relative to the Backup drive's root start with the folder name.
        let rel_where = |col: &str| {
            format!(
                "(lower({col}) = lower(?1)
                  OR lower(substr({col}, 1, length(?1) + 1)) IN (lower(?1) || '\\', lower(?1) || '/'))"
            )
        };
        let set_rel = |col: &str| format!("{col} = ?2 || substr({col}, length(?1) + 1)");
        tx.execute(
            &format!(
                "UPDATE backup_copies SET {} WHERE backup_volume_id = ?3 AND {}",
                set_rel("backup_rel_path"),
                rel_where("backup_rel_path")
            ),
            params![LEGACY_BACKUP_DIR, BACKUP_DIR, volume_id],
        )?;
        for col in ["rel_path", "original_rel_path"] {
            tx.execute(
                &format!("UPDATE backup_versions SET {} WHERE backup_volume_id = ?3 AND {}", set_rel(col), rel_where(col)),
                params![LEGACY_BACKUP_DIR, BACKUP_DIR, volume_id],
            )?;
        }
        if let Some(pp) = &passport {
            for side in ["a", "b"] {
                let col = format!("{side}_rel_path");
                tx.execute(
                    &format!(
                        "UPDATE dupe_decisions SET {} WHERE {side}_passport = ?3 AND {}",
                        set_rel(&col),
                        rel_where(&col)
                    ),
                    params![LEGACY_BACKUP_DIR, BACKUP_DIR, pp],
                )?;
            }
        }
        Ok(())
    });
    if let Err(e) = res {
        let _ = std::fs::rename(&current, &legacy);
        return Err(e);
    }
    Ok(true)
}

/// The drive currently holding the Backup role: `(volume id, mount point, online)`.
pub fn backup_volume(conn: &Connection) -> Result<Option<(String, String, bool)>> {
    Ok(conn
        .query_row(
            "SELECT id, mount_point, status = 'online' FROM volumes WHERE role = 'backup' LIMIT 1",
            [],
            |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?)),
        )
        .optional()?)
}

fn safe_component(s: &str) -> String {
    let cleaned: String = s
        .chars()
        .map(|c| if "<>:\"/\\|?*".contains(c) || c.is_control() { '_' } else { c })
        .collect();
    let trimmed = cleaned.trim().trim_end_matches('.').to_string();
    if trimmed.is_empty() { "Drive".into() } else { trimmed }
}

/// Label for the folder a source's files are backed up under, plus the base folder that
/// relative paths are measured from: a drive's own label (base = its root), or
/// `PC (<root folder>)` for the PC's configured folders, e.g. `PC (Documents)`.
pub fn source_label_for(conn: &Connection, volume_id: &str, path: &str) -> Result<(String, String)> {
    let (mount, label, managed): (String, Option<String>, bool) = conn.query_row(
        "SELECT mount_point, label, passport_id IS NOT NULL OR id LIKE 'fp-%' FROM volumes WHERE id = ?1",
        [volume_id],
        |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?)),
    )?;
    if managed {
        let tail: String = volume_id.chars().skip(volume_id.chars().count().saturating_sub(6)).collect();
        let name = label.unwrap_or_else(|| format!("Drive {tail}"));
        return Ok((safe_component(&name), mount));
    }
    let roots: Vec<String> = {
        let raw: Option<String> = conn
            .query_row("SELECT value_json FROM settings WHERE key = 'roots' AND scope = 'global'", [], |r| r.get(0))
            .optional()?;
        raw.and_then(|j| serde_json::from_str::<Vec<crate::contracts::RootConfig>>(&j).ok())
            .unwrap_or_default()
            .into_iter()
            .filter(|r| r.enabled)
            .map(|r| r.path)
            .collect()
    };
    if let Some(root) = roots
        .iter()
        .filter(|r| path_is_within(path, r, false))
        .max_by_key(|r| r.len())
    {
        let name = Path::new(root)
            .file_name()
            .map(|n| n.to_string_lossy().to_string())
            .unwrap_or_else(|| root.clone());
        return Ok((format!("PC ({})", safe_component(&name)), root.clone()));
    }
    Ok(("PC".into(), mount))
}

/// Label for a source folder given as `(volume, path relative to its mount)`.
pub fn source_label(conn: &Connection, volume_id: &str, rel_root: &str) -> Result<String> {
    let mount: String = conn.query_row("SELECT mount_point FROM volumes WHERE id = ?1", [volume_id], |r| r.get(0))?;
    let full = Path::new(&mount).join(rel_root).to_string_lossy().to_string();
    Ok(source_label_for(conn, volume_id, &full)?.0)
}

/// Relative part of `path` under `base` (OS separators).
pub fn rel_under(path: &str, base: &str) -> String {
    path.get(base.len()..)
        .unwrap_or("")
        .trim_start_matches(['\\', '/'])
        .to_string()
}

/// Where a copy of `source_path` (on `source_volume`) lives on the Backup drive at `backup_mount`.
pub fn backup_dest_for(conn: &Connection, source_volume: &str, source_path: &str, backup_mount: &str) -> Result<PathBuf> {
    let (label, base) = source_label_for(conn, source_volume, source_path)?;
    Ok(backup_root(Path::new(backup_mount))
        .join(label)
        .join(rel_under(source_path, &base)))
}

/// Records, inside the execute transaction, that `dest` on the Backup drive is a copy of the
/// evidence's source file: a `files` row (for a fresh copy), a `backup_copies` row and the
/// source folder in `backup_sources`. Returns the backup copy's file id.
pub(crate) fn record_backup_copy_in_tx(
    tx: &Connection,
    evidence: &serde_json::Value,
    dest: &Path,
    moved_file_id: Option<&str>,
    hash: Option<&[u8]>,
) -> Result<Option<String>> {
    let Some(bc) = evidence.get("backup_copy") else {
        return Ok(None);
    };
    let s = |k: &str| bc.get(k).and_then(|v| v.as_str()).map(str::to_string);
    let (Some(source_file_id), Some(backup_volume_id)) = (s("source_file_id"), s("backup_volume_id")) else {
        return Ok(None);
    };
    let Some(hash) = hash else { return Ok(None) };
    let file_id = match moved_file_id {
        Some(id) => id.to_string(),
        None => {
            let Some((vid, _, cs)) = resolve_online_volume(tx, dest)? else {
                return Ok(None);
            };
            let stable: bool = tx.query_row("SELECT has_stable_inode FROM volumes WHERE id = ?1", [&vid], |r| r.get(0))?;
            let Some(stat) = inspect_path_metadata(dest)? else { return Ok(None) };
            upsert_discovered_file(
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
            )?
        }
    };
    let mount: String = tx.query_row("SELECT mount_point FROM volumes WHERE id = ?1", [&backup_volume_id], |r| r.get(0))?;
    let (disp, _) = normalize_path(dest, false);
    let rel = rel_under(&disp, &mount);
    let now = now_ms();
    tx.execute(
        "INSERT INTO backup_copies (id, source_file_id, backup_volume_id, backup_rel_path, source_blake3, copied_at, last_verified_at)
         VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?6)
         ON CONFLICT(backup_volume_id, backup_rel_path) DO UPDATE SET
           source_file_id = excluded.source_file_id, source_blake3 = excluded.source_blake3,
           copied_at = excluded.copied_at, last_verified_at = excluded.last_verified_at",
        params![crate::new_id(), source_file_id, backup_volume_id, rel, hash, now],
    )?;
    if let (Some(src_vol), Some(src_rel)) = (s("source_volume_id"), s("source_rel_folder")) {
        tx.execute(
            "INSERT OR IGNORE INTO backup_sources (id, volume_id, rel_path, tier, pinned, added_at)
             VALUES (?1, ?2, ?3, 2, 0, ?4)",
            params![crate::new_id(), src_vol, src_rel, now],
        )?;
    }
    Ok(Some(file_id))
}
