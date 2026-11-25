//! Copy-count and survivor rules that keep the last copies of a file safe (spec §7.5 S1–S3).

use crate::error::{AegisError, Result};
use crate::format_date_ms;
use rusqlite::{Connection, OptionalExtension, params};
use std::collections::HashSet;
use std::io::Read;
use std::path::Path;

/// A delete may never leave fewer than this many copies of a file.
pub const MIN_COPIES: i64 = 2;

/// Number of distinct copies of `blake3` Cairn knows about, on any volume, online or not.
/// Hardlinks (same volume + file index) are one copy. Quarantined, missing and soft-deleted
/// rows are not copies.
pub fn copy_count(conn: &Connection, blake3: &[u8]) -> Result<i64> {
    let mut stmt = conn.prepare(
        "SELECT id, volume_id, file_index FROM files
         WHERE blake3 = ?1 AND deleted_at IS NULL AND state NOT IN ('quarantine','missing')",
    )?;
    let rows = stmt.query_map(params![blake3], |r| {
        Ok((
            r.get::<_, String>(0)?,
            r.get::<_, String>(1)?,
            r.get::<_, Option<Vec<u8>>>(2)?,
        ))
    })?;
    let mut seen: HashSet<(String, Vec<u8>)> = HashSet::new();
    for r in rows {
        let (id, vol, idx) = r?;
        seen.insert((vol, idx.unwrap_or_else(|| id.into_bytes())));
    }
    Ok(seen.len() as i64)
}

fn full_hash(path: &Path) -> Option<Vec<u8>> {
    let mut f = std::fs::File::open(path).ok()?;
    let mut h = blake3::Hasher::new();
    let mut buf = vec![0u8; 256 * 1024];
    loop {
        let n = f.read(&mut buf).ok()?;
        if n == 0 {
            break;
        }
        h.update(&buf[..n]);
    }
    Some(h.finalize().as_bytes().to_vec())
}

/// Confirms the drive mounted where `volume_id` is registered is really that drive: online, and
/// carrying the passport on record. Guards against another drive taking the same letter before
/// Cairn noticed (any action uses stored absolute paths).
pub fn confirm_drive_identity(conn: &Connection, volume_id: &str) -> Result<()> {
    let (mount, status, label, passport): (String, String, Option<String>, Option<String>) = conn.query_row(
        "SELECT mount_point, status, label, passport_id FROM volumes WHERE id = ?1",
        [volume_id],
        |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?, r.get(3)?)),
    )?;
    let name = label.unwrap_or_else(|| mount.clone());
    if status != "online" {
        return Err(AegisError::DriveMismatch(format!("{name} isn't plugged in.")));
    }
    if let Some(expected) = passport {
        let on_disk = crate::drives::read_passport(Path::new(&mount))?;
        if on_disk.map(|p| p.passport_id) != Some(expected) {
            return Err(AegisError::DriveMismatch(format!(
                "The drive at {mount} isn't {name} any more. Plug {name} back in and try again."
            )));
        }
    }
    Ok(())
}

/// Confirms file `file_id` is still the file at `expected_path`, on the right drive.
pub fn confirm_file_at(conn: &Connection, file_id: &str, expected_path: &str) -> Result<()> {
    let (path, volume_id): (String, String) = conn
        .query_row("SELECT path, volume_id FROM files WHERE id = ?1", [file_id], |r| Ok((r.get(0)?, r.get(1)?)))
        .optional()?
        .ok_or_else(|| AegisError::NotFound(format!("file {file_id}")))?;
    let norm = |p: &str| p.replace('/', "\\").trim_end_matches('\\').to_lowercase();
    if norm(&path) != norm(expected_path) {
        return Err(AegisError::DriveMismatch(
            "This file has moved since the change was proposed. Propose it again.".into(),
        ));
    }
    confirm_drive_identity(conn, &volume_id)
}

/// Confirms, right now, that the surviving copy exists on an online drive and still has the
/// expected content. A catalog of an unplugged drive is never trusted for a delete (S3).
pub fn verify_survivor(conn: &Connection, survivor_file_id: &str, expected: &[u8]) -> Result<()> {
    let row: Option<(String, String, Option<String>, String, i64, String)> = conn
        .query_row(
            "SELECT f.path, v.status, v.label, v.mount_point, v.last_seen, f.volume_id
             FROM files f JOIN volumes v ON v.id = f.volume_id
             WHERE f.id = ?1 AND f.deleted_at IS NULL AND f.state NOT IN ('quarantine','missing')",
            params![survivor_file_id],
            |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?, r.get(3)?, r.get(4)?, r.get(5)?)),
        )
        .optional()?;
    let Some((path, status, label, mount, last_seen, volume_id)) = row else {
        return Err(AegisError::SurvivorUnavailable(
            "The copy this was meant to keep is no longer in the catalog, so nothing was removed.".into(),
        ));
    };
    let label = label.unwrap_or(mount);
    if status != "online" {
        return Err(AegisError::SurvivorUnavailable(format!(
            "The other copy is on {label} (last checked {}). Plug it in to confirm, or keep both.",
            format_date_ms(last_seen)
        )));
    }
    confirm_drive_identity(conn, &volume_id)?;
    if crate::scanner::os_adapter::is_placeholder_now(Path::new(&path)) {
        return Err(AegisError::SurvivorUnavailable(format!(
            "The other copy at {path} is online-only (in the cloud), so this copy was kept."
        )));
    }
    match full_hash(Path::new(&path)) {
        Some(h) if h == expected => Ok(()),
        Some(_) => Err(AegisError::SurvivorUnavailable(format!(
            "The other copy at {path} has changed since it was checked, so this copy was kept."
        ))),
        None => Err(AegisError::SurvivorUnavailable(format!(
            "The other copy at {path} is missing or unreadable, so this copy was kept."
        ))),
    }
}
