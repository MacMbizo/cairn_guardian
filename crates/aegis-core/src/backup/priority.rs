//! Backup priority tiers and filling the Backup drive in order (spec §7.7.2).
//!
//! 1. Pinned folders and **only copies** (files with exactly one copy anywhere Aegis knows).
//! 2. Everything else in the backup sources (the PC's Documents, Desktop, Pictures …).
//! 3. Music and video that isn't personal footage.
//!
//! Never: installers in Downloads, caches and build output, OS junk.

use super::BACKUP_DIR;
use crate::db::SqliteStore;
use crate::db::audit::append_audit_in_tx;
use crate::db::files::path_is_within;
use crate::error::{AegisError, Result};
use crate::{new_id, now_ms};
use rusqlite::{Connection, OptionalExtension, params};
use serde::{Deserialize, Serialize};
use std::path::Path;
use ts_rs::TS;

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, TS)]
#[ts(export)]
pub struct TierShortfall {
    #[ts(type = "number")]
    pub tier: i64,
    #[ts(type = "number")]
    pub bytes_needed: i64,
    #[ts(type = "number")]
    pub bytes_skipped: i64,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, TS)]
#[ts(export)]
pub struct TierCoverage {
    /// 1–3, or 0 for "never backed up".
    #[ts(type = "number")]
    pub tier: i64,
    pub label: String,
    #[ts(type = "number")]
    pub protected_bytes: i64,
    #[ts(type = "number")]
    pub unprotected_bytes: i64,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, TS)]
#[ts(export)]
pub struct BackupCoverage {
    pub backup_volume_id: String,
    pub backup_label: Option<String>,
    pub online: bool,
    #[ts(type = "number")]
    pub capacity_bytes: i64,
    #[ts(type = "number")]
    pub used_bytes: i64,
    pub tiers: Vec<TierCoverage>,
    pub suggestion: Option<String>,
}

pub fn tier_label(tier: i64) -> &'static str {
    match tier {
        1 => "Only copies",
        2 => "PC Documents etc.",
        3 => "Music & video",
        _ => "Installers & caches (skipped)",
    }
}

const INSTALLER_EXTS: [&str; 6] = ["exe", "msi", "msix", "dmg", "pkg", "iso"];
const AUDIO_EXTS: [&str; 9] = ["mp3", "flac", "wav", "m4a", "aac", "ogg", "wma", "opus", "aiff"];
const VIDEO_EXTS: [&str; 8] = ["mp4", "mkv", "avi", "mov", "wmv", "m4v", "webm", "mpg"];
const JUNK_NAMES: [&str; 4] = ["thumbs.db", "desktop.ini", ".ds_store", "ehthumbs.db"];
const PERSONAL_VIDEO_DIRS: [&str; 4] = ["pictures", "dcim", "videos", "camera"];

fn components_lower(path: &str) -> Vec<String> {
    path.split(['\\', '/']).map(|c| c.to_lowercase()).collect()
}

/// Backup tier of a file (1 = most irreplaceable … 3), or `None` when it is never backed up.
pub fn tier_for_file(conn: &Connection, file_id: &str) -> Result<Option<i64>> {
    let row: Option<(String, String, i64, Option<Vec<u8>>)> = conn
        .query_row(
            "SELECT volume_id, path, size, blake3 FROM files WHERE id = ?1",
            [file_id],
            |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?, r.get(3)?)),
        )
        .optional()?;
    let Some((volume_id, path, size, hash)) = row else { return Ok(None) };
    let name = Path::new(&path)
        .file_name()
        .map(|n| n.to_string_lossy().to_lowercase())
        .unwrap_or_default();
    let ext = Path::new(&path)
        .extension()
        .map(|e| e.to_string_lossy().to_lowercase())
        .unwrap_or_default();
    let parts = components_lower(&path);

    // Never.
    if JUNK_NAMES.contains(&name.as_str()) || name.starts_with("~$") || ext == "tmp" || ext == "aegis-partial" {
        return Ok(None);
    }
    if INSTALLER_EXTS.contains(&ext.as_str()) && parts.iter().any(|p| p == "downloads") {
        return Ok(None);
    }
    let in_cache: bool = {
        let mut stmt = conn.prepare("SELECT path FROM cache_dirs WHERE volume_id = ?1 AND state = 'active'")?;
        let rows = stmt.query_map([&volume_id], |r| r.get::<_, String>(0))?;
        let mut hit = false;
        for r in rows {
            if path_is_within(&path, &r?, false) {
                hit = true;
                break;
            }
        }
        hit
    };
    if in_cache {
        return Ok(None);
    }

    // Pinned.
    let pinned: bool = {
        let mut stmt = conn.prepare(
            "SELECT v.mount_point, s.rel_path FROM backup_sources s JOIN volumes v ON v.id = s.volume_id
             WHERE s.removed_at IS NULL AND s.pinned = 1 AND s.volume_id = ?1",
        )?;
        let rows = stmt.query_map([&volume_id], |r| Ok((r.get::<_, String>(0)?, r.get::<_, String>(1)?)))?;
        let mut hit = false;
        for r in rows {
            let (mount, rel) = r?;
            let folder = Path::new(&mount).join(rel).to_string_lossy().to_string();
            if path_is_within(&path, &folder, false) {
                hit = true;
                break;
            }
        }
        hit
    };
    if pinned {
        return Ok(Some(1));
    }

    // Media that can usually be had again.
    let personal_video = parts.iter().any(|p| PERSONAL_VIDEO_DIRS.contains(&p.as_str()));
    if AUDIO_EXTS.contains(&ext.as_str()) || (VIDEO_EXTS.contains(&ext.as_str()) && !personal_video) {
        return Ok(Some(3));
    }

    // Only copy anywhere Aegis knows about?
    let only_copy = match hash {
        Some(h) => crate::dupes::copy_count(conn, &h)? <= 1,
        None => {
            // Never fingerprinted: a unique size means no copy can exist.
            let same_size: i64 = conn.query_row(
                "SELECT COUNT(*) FROM files WHERE size = ?1 AND deleted_at IS NULL
                   AND state NOT IN ('quarantine','missing')",
                [size],
                |r| r.get(0),
            )?;
            same_size <= 1
        }
    };
    Ok(Some(if only_copy { 1 } else { 2 }))
}

/// Adds (or re-adds) a backup source folder. Pinned folders are always tier 1.
pub fn add_source(store: &mut SqliteStore, volume_id: &str, rel_path: &str, pinned: bool) -> Result<String> {
    if rel_path.split(['\\', '/']).next().is_some_and(|c| c.eq_ignore_ascii_case(BACKUP_DIR)) {
        return Err(AegisError::InvalidInput("The Backup drive's own Aegis Backup folder can't be a source.".into()));
    }
    let existing: Option<String> = store
        .conn()
        .query_row(
            "SELECT id FROM backup_sources WHERE volume_id = ?1 AND rel_path = ?2 AND removed_at IS NULL",
            params![volume_id, rel_path],
            |r| r.get(0),
        )
        .optional()?;
    let id = existing.clone().unwrap_or_else(new_id);
    let now = now_ms();
    store.with_full_sync_tx(|tx| {
        if existing.is_some() {
            tx.execute(
                "UPDATE backup_sources SET pinned = ?2, tier = ?3 WHERE id = ?1",
                params![id, i32::from(pinned), if pinned { 1 } else { 2 }],
            )?;
        } else {
            tx.execute(
                "INSERT INTO backup_sources (id, volume_id, rel_path, tier, pinned, added_at) VALUES (?1, ?2, ?3, ?4, ?5, ?6)",
                params![id, volume_id, rel_path, if pinned { 1 } else { 2 }, i32::from(pinned), now],
            )?;
        }
        append_audit_in_tx(
            tx,
            "user",
            "backup.source_added",
            &format!("{volume_id}:{rel_path}"),
            &serde_json::json!({ "pinned": pinned }).to_string(),
        )?;
        Ok(())
    })?;
    Ok(id)
}

/// Stops backing up a source folder. Its copies are retired to `_Previous versions` (kept 90
/// days) by the next top-up. Returns the bytes that will stop being backed up, for the
/// "Stop backing up …? Its N GB backup will be kept for 90 days, then removed." question.
pub fn remove_source(store: &mut SqliteStore, source_id: &str) -> Result<i64> {
    let (volume_id, rel): (String, String) = store
        .conn()
        .query_row(
            "SELECT volume_id, rel_path FROM backup_sources WHERE id = ?1 AND removed_at IS NULL",
            [source_id],
            |r| Ok((r.get(0)?, r.get(1)?)),
        )
        .optional()?
        .ok_or_else(|| AegisError::NotFound(format!("backup source {source_id}")))?;
    let mount: String = store
        .conn()
        .query_row("SELECT mount_point FROM volumes WHERE id = ?1", [&volume_id], |r| r.get(0))?;
    let folder = if rel.is_empty() { mount } else { Path::new(&mount).join(&rel).to_string_lossy().to_string() };
    let bytes: i64 = {
        let mut stmt = store.conn().prepare(
            "SELECT f.path, f.size FROM backup_copies c JOIN files f ON f.id = c.source_file_id WHERE f.volume_id = ?1",
        )?;
        let rows = stmt.query_map([&volume_id], |r| Ok((r.get::<_, String>(0)?, r.get::<_, i64>(1)?)))?;
        let mut sum = 0;
        for r in rows {
            let (p, s) = r?;
            if path_is_within(&p, &folder, false) {
                sum += s;
            }
        }
        sum
    };
    let now = now_ms();
    store.with_full_sync_tx(|tx| {
        tx.execute("UPDATE backup_sources SET removed_at = ?2 WHERE id = ?1", params![source_id, now])?;
        append_audit_in_tx(
            tx,
            "user",
            "backup.source_removed",
            &format!("{volume_id}:{rel}"),
            &serde_json::json!({ "bytes": bytes }).to_string(),
        )?;
        Ok(())
    })?;
    Ok(bytes)
}

/// Space the Backup drive can give a top-up: `(free bytes, bytes held by previous versions)`.
pub(crate) fn backup_space(conn: &Connection, backup_volume_id: &str) -> Result<(i64, i64)> {
    let (total, mount): (i64, String) = conn.query_row(
        "SELECT COALESCE(total_bytes, 0), mount_point FROM volumes WHERE id = ?1",
        [backup_volume_id],
        |r| Ok((r.get(0)?, r.get(1)?)),
    )?;
    let used: i64 = conn.query_row(
        "SELECT COALESCE(SUM(size), 0) FROM files WHERE volume_id = ?1 AND deleted_at IS NULL",
        [backup_volume_id],
        |r| r.get(0),
    )?;
    let versions: i64 = conn.query_row(
        "SELECT COALESCE(SUM(size_bytes), 0) FROM backup_versions WHERE backup_volume_id = ?1 AND pruned_at IS NULL",
        [backup_volume_id],
        |r| r.get(0),
    )?;
    let mut free = (total - used - versions).max(0);
    let (os_total, os_free) = crate::scanner::os_adapter::disk_space_for_mount(&mount);
    if os_total > 0 {
        free = free.min(os_free as i64);
    }
    Ok((free, versions))
}

/// Protection summary for the Drives screen, by tier.
pub fn coverage_summary(conn: &Connection) -> Result<Option<BackupCoverage>> {
    let Some((backup_id, _mount, online)) = super::backup_volume(conn)? else { return Ok(None) };
    let (label, capacity): (Option<String>, i64) = conn.query_row(
        "SELECT label, COALESCE(total_bytes, 0) FROM volumes WHERE id = ?1",
        [&backup_id],
        |r| Ok((r.get(0)?, r.get(1)?)),
    )?;
    let used: i64 = conn.query_row(
        "SELECT COALESCE(SUM(size), 0) FROM files WHERE volume_id = ?1 AND deleted_at IS NULL",
        [&backup_id],
        |r| r.get(0),
    )?;
    let mut tiers: Vec<TierCoverage> = [1, 2, 3, 0]
        .iter()
        .map(|&t| TierCoverage { tier: t, label: tier_label(t).into(), protected_bytes: 0, unprotected_bytes: 0 })
        .collect();
    let sources: Vec<String> = {
        let mut stmt = conn.prepare(
            "SELECT v.mount_point, s.rel_path FROM backup_sources s JOIN volumes v ON v.id = s.volume_id
             WHERE s.removed_at IS NULL",
        )?;
        let rows = stmt.query_map([], |r| Ok((r.get::<_, String>(0)?, r.get::<_, String>(1)?)))?;
        rows.map(|r| r.map(|(m, rel)| Path::new(&m).join(rel).to_string_lossy().to_string()))
            .collect::<std::result::Result<Vec<_>, _>>()?
    };
    let mut stmt = conn.prepare(
        "SELECT f.id, f.path, f.size, f.blake3,
                EXISTS(SELECT 1 FROM backup_copies c WHERE c.source_file_id = f.id AND c.backup_volume_id = ?1
                         AND (f.blake3 IS NULL OR c.source_blake3 = f.blake3))
         FROM files f
         WHERE f.deleted_at IS NULL AND f.state NOT IN ('quarantine','missing') AND f.volume_id != ?1
           AND f.container_file_id IS NULL AND f.cloud_state != 'placeholder_remote'",
    )?;
    let rows = stmt.query_map([&backup_id], |r| {
        Ok((r.get::<_, String>(0)?, r.get::<_, String>(1)?, r.get::<_, i64>(2)?, r.get::<_, bool>(4)?))
    })?;
    for r in rows {
        let (id, path, size, protected) = r?;
        if !sources.iter().any(|s| path_is_within(&path, s, false)) {
            continue;
        }
        let tier = tier_for_file(conn, &id)?.unwrap_or(0);
        let slot = tiers.iter_mut().find(|t| t.tier == tier).expect("tier slot");
        if protected {
            slot.protected_bytes += size;
        } else {
            slot.unprotected_bytes += size;
        }
    }
    let (free, versions) = backup_space(conn, &backup_id)?;
    let missing: i64 = tiers.iter().filter(|t| t.tier != 0).map(|t| t.unprotected_bytes).sum();
    let short = missing - free - versions;
    let shown = label.clone().unwrap_or_else(|| "the Backup drive".into());
    let suggestion = (short > 0).then(|| {
        let gb = short as f64 / 1_000_000_000.0;
        let amount = if gb >= 0.1 { format!("{gb:.1} GB") } else { format!("{} KB", (short + 999) / 1000) };
        format!("{amount} doesn't fit. Add a second backup drive or free space on {shown}.")
    });
    Ok(Some(BackupCoverage {
        backup_volume_id: backup_id,
        backup_label: label,
        online,
        capacity_bytes: capacity,
        used_bytes: used,
        tiers,
        suggestion,
    }))
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, TS)]
#[ts(export)]
pub struct BackupSourceItem {
    pub id: String,
    pub volume_id: String,
    pub drive_label: Option<String>,
    pub rel_path: String,
    /// Full folder path on its drive.
    pub folder: String,
    pub pinned: bool,
    #[ts(type = "number")]
    pub tier: i64,
}

/// Folders currently backed up.
pub fn list_sources(conn: &Connection) -> Result<Vec<BackupSourceItem>> {
    let mut stmt = conn.prepare(
        "SELECT s.id, s.volume_id, v.label, s.rel_path, v.mount_point, s.pinned, s.tier
         FROM backup_sources s JOIN volumes v ON v.id = s.volume_id
         WHERE s.removed_at IS NULL ORDER BY s.pinned DESC, s.added_at",
    )?;
    let rows = stmt.query_map([], |r| {
        let mount: String = r.get(4)?;
        let rel: String = r.get(3)?;
        Ok(BackupSourceItem {
            id: r.get(0)?,
            volume_id: r.get(1)?,
            drive_label: r.get(2)?,
            folder: if rel.is_empty() { mount.clone() } else { Path::new(&mount).join(&rel).to_string_lossy().to_string() },
            rel_path: rel,
            pinned: r.get::<_, i64>(5)? != 0,
            tier: r.get(6)?,
        })
    })?;
    Ok(rows.collect::<std::result::Result<Vec<_>, _>>()?)
}

/// Adds the folder at `path` (on whichever known, online drive holds it) as a backup source.
pub fn add_source_path(store: &mut SqliteStore, path: &Path, pinned: bool) -> Result<String> {
    let canonical = std::fs::canonicalize(path).unwrap_or_else(|_| path.to_path_buf());
    let (display, _) = crate::db::files::normalize_path(&canonical, false);
    let vol = store.ensure_volume_for_path(&canonical)?;
    let rel = super::rel_under(&display, &vol.mount_point);
    add_source(store, &vol.id, &rel, pinned)
}
