//! Copy-count and survivor rules that keep the last copies of a file safe (spec §7.5 S1–S3).

use crate::error::{AegisError, Result};
use crate::format_date_ms;
use rusqlite::{Connection, OptionalExtension, params};
use std::collections::HashSet;
use std::io::Read;
use std::path::Path;

/// A delete may never leave fewer than this many copies of a file.
pub const MIN_COPIES: i64 = 2;

/// Number of distinct copies of `blake3` Aegis knows about, on any volume, online or not.
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

/// Confirms, right now, that the surviving copy exists on an online drive and still has the
/// expected content. A catalog of an unplugged drive is never trusted for a delete (S3).
pub fn verify_survivor(conn: &Connection, survivor_file_id: &str, expected: &[u8]) -> Result<()> {
    let row: Option<(String, String, Option<String>, String, i64)> = conn
        .query_row(
            "SELECT f.path, v.status, v.label, v.mount_point, v.last_seen
             FROM files f JOIN volumes v ON v.id = f.volume_id
             WHERE f.id = ?1 AND f.deleted_at IS NULL AND f.state NOT IN ('quarantine','missing')",
            params![survivor_file_id],
            |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?, r.get(3)?, r.get(4)?)),
        )
        .optional()?;
    let Some((path, status, label, mount, last_seen)) = row else {
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
