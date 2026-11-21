//! Re-plug refresh: USN change journal on NTFS volume roots, metadata walk otherwise (spec §7.4).

use super::usn::{self, REASON_FILE_DELETE, REASON_RENAME_NEW_NAME, REASON_RENAME_OLD_NAME, UsnRead};
use crate::db::SqliteStore;
use crate::error::{AegisError, Result};
use crate::fingerprint::{FingerprintStats, run_fingerprint};
use crate::scanner::{catalog_volume, index_paths};
use rusqlite::{OptionalExtension, params};
use std::collections::{BTreeSet, HashMap};
use std::path::{Path, PathBuf};
use std::sync::atomic::AtomicBool;

#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize, serde::Deserialize, ts_rs::TS)]
#[serde(rename_all = "snake_case")]
#[ts(export)]
pub enum RefreshMode {
    Usn,
    Walk,
}

#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize, ts_rs::TS)]
#[ts(export)]
pub struct RefreshReport {
    pub volume_id: String,
    pub mode: RefreshMode,
    #[ts(type = "number")]
    pub created: i64,
    #[ts(type = "number")]
    pub modified: i64,
    #[ts(type = "number")]
    pub renamed: i64,
    #[ts(type = "number")]
    pub deleted: i64,
    #[ts(type = "number")]
    pub folders_changed: i64,
    #[ts(type = "number")]
    pub new_duplicates: i64,
    pub fingerprint: FingerprintStats,
}

/// Current journal position for an NTFS volume root, or `None` (not a root, no journal, no access).
pub fn journal_position(mount: &Path) -> Option<(String, i64)> {
    if !usn::is_volume_root(mount) {
        return None;
    }
    usn::query_journal(mount).ok().flatten()
}

/// Records (or clears) the journal position a later refresh replays from.
pub fn store_journal(store: &SqliteStore, volume_id: &str, pos: Option<(String, i64)>) -> Result<()> {
    let (jid, next) = match pos {
        Some((j, n)) => (Some(j), Some(n)),
        None => (None, None),
    };
    store.conn().execute(
        "UPDATE volumes SET usn_journal_id = ?2, usn_next = ?3 WHERE id = ?1",
        params![volume_id, jid, next],
    )?;
    Ok(())
}

type Snapshot = HashMap<String, (String, i64, i64)>;
/// `(mount, status, fs_type, usn_journal_id, usn_next)`.
type VolumeRefreshRow = (String, String, String, Option<String>, Option<i64>);

fn snapshot(store: &SqliteStore, volume_id: &str) -> Result<Snapshot> {
    let mut stmt = store.conn().prepare(
        "SELECT id, path, size, mtime FROM files
         WHERE volume_id = ?1 AND deleted_at IS NULL AND container_file_id IS NULL",
    )?;
    let rows = stmt.query_map([volume_id], |r| {
        Ok((r.get::<_, String>(0)?, (r.get(1)?, r.get(2)?, r.get(3)?)))
    })?;
    Ok(rows.collect::<std::result::Result<_, _>>()?)
}

/// Number of exact-duplicate groups (live files sharing a full hash) across the catalog.
pub fn duplicate_group_count(store: &SqliteStore) -> Result<i64> {
    Ok(store.conn().query_row(
        "SELECT COUNT(*) FROM (SELECT blake3 FROM files
           WHERE blake3 IS NOT NULL AND deleted_at IS NULL AND size > 0
             AND state NOT IN ('quarantine','missing')
           GROUP BY blake3 HAVING COUNT(*) >= 2)",
        [],
        |r| r.get(0),
    )?)
}

fn parent_of(path: &str) -> String {
    Path::new(path)
        .parent()
        .map(|p| p.to_string_lossy().to_lowercase())
        .unwrap_or_default()
}

/// Tries the USN path. `Ok(None)` means "fall back to a walk".
fn try_usn(store: &mut SqliteStore, volume_id: &str, mount: &Path, jid: &str, from: i64) -> Result<Option<()>> {
    let changes = match usn::read_changes(mount, jid, from) {
        Ok(UsnRead::Changes(c)) => c,
        Ok(UsnRead::Reset) | Err(_) => return Ok(None),
    };
    // A renamed or deleted folder changes every path beneath it without a record per child.
    if changes
        .iter()
        .any(|c| c.is_dir && c.reason_flags & (REASON_RENAME_OLD_NAME | REASON_RENAME_NEW_NAME | REASON_FILE_DELETE) != 0)
    {
        return Ok(None);
    }
    let mut present: BTreeSet<PathBuf> = BTreeSet::new();
    let mut gone: BTreeSet<u64> = BTreeSet::new();
    for c in changes.iter().filter(|c| !c.is_dir) {
        match &c.path {
            Some(p) => {
                present.insert(p.clone());
            }
            None => {
                gone.insert(c.file_ref);
            }
        }
    }
    // Existing files first: renames move their row by file index, keeping `files.id`.
    index_paths(store, volume_id, &present.into_iter().collect::<Vec<_>>())?;
    // Files that no longer resolve are found by their stored file index.
    let mut gone_paths = Vec::new();
    for r in gone {
        for form in usn::file_index_forms(r) {
            let mut stmt = store.conn().prepare(
                "SELECT path FROM files WHERE volume_id = ?1 AND file_index = ?2 AND deleted_at IS NULL",
            )?;
            let rows = stmt.query_map(params![volume_id, form], |row| row.get::<_, String>(0))?;
            for p in rows {
                let p = PathBuf::from(p?);
                if !p.exists() {
                    gone_paths.push(p);
                }
            }
        }
    }
    index_paths(store, volume_id, &gone_paths)?;
    Ok(Some(()))
}

/// Brings a re-plugged drive's catalog up to date and fingerprints new duplicate candidates.
pub fn refresh_volume(store: &mut SqliteStore, volume_id: &str, cancel: &AtomicBool) -> Result<RefreshReport> {
    let row: Option<VolumeRefreshRow> = store
        .conn()
        .query_row(
            "SELECT mount_point, status, fs_type, usn_journal_id, usn_next FROM volumes WHERE id = ?1",
            params![volume_id],
            |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?, r.get(3)?, r.get(4)?)),
        )
        .optional()?;
    let (mount, status, fs_type, jid, next) =
        row.ok_or_else(|| AegisError::NotFound(format!("volume {volume_id}")))?;
    if status != "online" {
        return Err(AegisError::VolumeOffline(volume_id.to_string()));
    }
    let mount_path = PathBuf::from(&mount);
    let before = snapshot(store, volume_id)?;
    let dupes_before = duplicate_group_count(store)?;

    let mut mode = RefreshMode::Walk;
    if fs_type == "ntfs"
        && usn::is_volume_root(&mount_path)
        && let (Some(j), Some(n)) = (jid.as_deref(), next)
    {
        let pos = journal_position(&mount_path);
        if try_usn(store, volume_id, &mount_path, j, n)?.is_some() {
            mode = RefreshMode::Usn;
            store_journal(store, volume_id, pos)?;
        }
    }
    if mode == RefreshMode::Walk {
        catalog_volume(store, volume_id, &mut |_| {})?; // also records the journal position
    }

    let after = snapshot(store, volume_id)?;
    let mut report = RefreshReport {
        volume_id: volume_id.to_string(),
        mode,
        created: 0,
        modified: 0,
        renamed: 0,
        deleted: 0,
        folders_changed: 0,
        new_duplicates: 0,
        fingerprint: FingerprintStats::default(),
    };
    let mut folders: BTreeSet<String> = BTreeSet::new();
    for (id, (path, size, mtime)) in &after {
        match before.get(id) {
            None => {
                report.created += 1;
                folders.insert(parent_of(path));
            }
            Some((old_path, old_size, old_mtime)) => {
                if old_path != path {
                    report.renamed += 1;
                    folders.insert(parent_of(old_path));
                    folders.insert(parent_of(path));
                }
                if old_size != size || old_mtime != mtime {
                    report.modified += 1;
                    folders.insert(parent_of(path));
                }
            }
        }
    }
    for (id, (path, _, _)) in &before {
        if !after.contains_key(id) {
            report.deleted += 1;
            folders.insert(parent_of(path));
        }
    }
    report.folders_changed = folders.len() as i64;
    report.fingerprint = run_fingerprint(store, cancel)?;
    report.new_duplicates = (duplicate_group_count(store)? - dupes_before).max(0);
    Ok(report)
}
