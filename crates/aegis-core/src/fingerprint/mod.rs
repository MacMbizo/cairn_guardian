//! Duplicate fingerprinting: size grouping → quick hash → full BLAKE3 (spec §7.3).
//!
//! Only files that could be an exact copy of something are ever read: a size that occurs once
//! across the whole catalog (all drives, online or not) cannot be a duplicate.

use crate::db::SqliteStore;
use crate::error::Result;
use rusqlite::params;
use std::fs::File;
use std::io::{Read, Seek, SeekFrom};
use std::path::Path;
use std::sync::atomic::{AtomicBool, Ordering};

/// Bytes read from each end of a file for the quick hash.
pub const EDGE_BYTES: u64 = 64 * 1024;
/// At or below this size the quick hash is the file's full BLAKE3.
pub const SMALL_FILE_BYTES: u64 = 2 * EDGE_BYTES;
const BATCH: usize = 500;

#[derive(Debug, Clone, Default, PartialEq, Eq, serde::Serialize, serde::Deserialize, ts_rs::TS)]
#[ts(export)]
pub struct FingerprintStats {
    #[ts(type = "number")]
    pub size_candidates: i64,
    #[ts(type = "number")]
    pub quick_hashed: i64,
    #[ts(type = "number")]
    pub full_hashed: i64,
    #[ts(type = "number")]
    pub skipped_offline: i64,
    #[ts(type = "number")]
    pub skipped_placeholder: i64,
}

/// Quick hash of `path`: BLAKE3(size as u64 LE ‖ first 64 KiB ‖ last 64 KiB), or the full
/// BLAKE3 for files ≤ 128 KiB. The bool is true when the result is the full hash.
pub fn quick_hash_file(path: &Path, size: u64) -> Result<(Vec<u8>, bool)> {
    let mut f = File::open(path)?;
    if size <= SMALL_FILE_BYTES {
        let mut buf = Vec::with_capacity(size as usize);
        f.read_to_end(&mut buf)?;
        return Ok((blake3::hash(&buf).as_bytes().to_vec(), true));
    }
    let mut h = blake3::Hasher::new();
    h.update(&size.to_le_bytes());
    let mut edge = vec![0u8; EDGE_BYTES as usize];
    f.read_exact(&mut edge)?;
    h.update(&edge);
    f.seek(SeekFrom::End(-(EDGE_BYTES as i64)))?;
    f.read_exact(&mut edge)?;
    h.update(&edge);
    Ok((h.finalize().as_bytes().to_vec(), false))
}

fn full_hash_file(path: &Path) -> Result<Vec<u8>> {
    let mut f = File::open(path)?;
    let mut h = blake3::Hasher::new();
    let mut buf = vec![0u8; 256 * 1024];
    loop {
        let n = f.read(&mut buf)?;
        if n == 0 {
            break;
        }
        h.update(&buf[..n]);
    }
    Ok(h.finalize().as_bytes().to_vec())
}

struct Candidate {
    id: String,
    path: String,
    size: i64,
    quick_hash: Option<Vec<u8>>,
    blake3: Option<Vec<u8>>,
    online: bool,
    placeholder: bool,
}

/// Size predicate shared by both passes: live, local-or-cloud, non-empty, not archive members.
const LIVE: &str = "f.deleted_at IS NULL AND f.state NOT IN ('quarantine','missing')
                    AND f.size > 0 AND f.container_file_id IS NULL";

fn load_candidates(store: &SqliteStore) -> Result<Vec<Candidate>> {
    let sql = format!(
        "SELECT f.id, f.path, f.size, f.quick_hash, f.blake3, v.status = 'online', f.cloud_state
         FROM files f JOIN volumes v ON v.id = f.volume_id
         WHERE {LIVE} AND f.size IN (
             SELECT f.size FROM files f WHERE {LIVE} AND f.cloud_state != 'placeholder_remote'
             GROUP BY f.size HAVING COUNT(*) >= 2)
         ORDER BY f.size, f.id"
    );
    let mut stmt = store.conn().prepare(&sql)?;
    let rows = stmt.query_map([], |r| {
        Ok(Candidate {
            id: r.get(0)?,
            path: r.get(1)?,
            size: r.get(2)?,
            quick_hash: r.get(3)?,
            blake3: r.get(4)?,
            online: r.get(5)?,
            placeholder: r.get::<_, String>(6)? == "placeholder_remote",
        })
    })?;
    Ok(rows.collect::<std::result::Result<Vec<_>, _>>()?)
}

fn mark_placeholder(store: &SqliteStore, id: &str) -> Result<()> {
    store
        .conn()
        .execute("UPDATE files SET cloud_state = 'placeholder_remote' WHERE id = ?1", [id])?;
    Ok(())
}

/// The file on disk still has the catalogued size (otherwise a refresh must re-catalog it first).
fn unchanged_on_disk(c: &Candidate) -> bool {
    std::fs::metadata(&c.path).is_ok_and(|m| m.is_file() && m.len() as i64 == c.size)
}

enum Update {
    Quick { id: String, quick: Vec<u8>, full: bool },
    Full { id: String, full: Vec<u8> },
}

fn flush(store: &mut SqliteStore, pending: &mut Vec<Update>) -> Result<()> {
    if pending.is_empty() {
        return Ok(());
    }
    let tx = store.conn_mut().transaction()?;
    for u in pending.drain(..) {
        match u {
            Update::Quick { id, quick, full } => {
                tx.execute(
                    "UPDATE files SET quick_hash = ?2, blake3 = CASE WHEN ?3 THEN ?2 ELSE blake3 END
                     WHERE id = ?1",
                    params![id, quick, full],
                )?;
            }
            Update::Full { id, full } => {
                tx.execute("UPDATE files SET blake3 = ?2 WHERE id = ?1", params![id, full])?;
            }
        }
    }
    tx.commit()?;
    Ok(())
}

/// Runs both fingerprint passes over the whole catalog. Files on offline drives and cloud
/// placeholders are never opened. Stops early (keeping finished work) when `cancel` is set.
pub fn run_fingerprint(store: &mut SqliteStore, cancel: &AtomicBool) -> Result<FingerprintStats> {
    let mut stats = FingerprintStats::default();
    let mut candidates = load_candidates(store)?;
    stats.size_candidates = candidates.len() as i64;
    let mut pending = Vec::new();

    // Pass 1: quick hash.
    for c in candidates.iter_mut() {
        if c.placeholder {
            stats.skipped_placeholder += 1;
            continue;
        }
        if c.quick_hash.is_some() {
            continue;
        }
        if !c.online {
            stats.skipped_offline += 1;
            continue;
        }
        if cancel.load(Ordering::Relaxed) {
            flush(store, &mut pending)?;
            return Ok(stats);
        }
        if crate::scanner::os_adapter::is_placeholder_now(Path::new(&c.path)) {
            // Became online-only since the catalog: never open it (that downloads it).
            stats.skipped_placeholder += 1;
            c.placeholder = true;
            mark_placeholder(store, &c.id)?;
            continue;
        }
        if !unchanged_on_disk(c) {
            continue;
        }
        let Ok((quick, full)) = quick_hash_file(Path::new(&c.path), c.size as u64) else {
            continue; // unreadable now (locked, permissions); tried again next run
        };
        stats.quick_hashed += 1;
        c.quick_hash = Some(quick.clone());
        if full {
            c.blake3 = Some(quick.clone());
        }
        pending.push(Update::Quick { id: c.id.clone(), quick, full });
        if pending.len() >= BATCH {
            flush(store, &mut pending)?;
        }
    }
    flush(store, &mut pending)?;

    // Pass 2: full hash where (size, quick hash) collides.
    let mut i = 0;
    while i < candidates.len() {
        let mut j = i + 1;
        while j < candidates.len() && candidates[j].size == candidates[i].size {
            j += 1;
        }
        let group = &candidates[i..j];
        let mut by_quick: std::collections::HashMap<&[u8], Vec<usize>> = Default::default();
        for (k, c) in group.iter().enumerate() {
            if let Some(q) = c.quick_hash.as_deref().filter(|_| !c.placeholder) {
                by_quick.entry(q).or_default().push(k);
            }
        }
        let mut to_hash: Vec<usize> = by_quick
            .values()
            .filter(|ks| ks.len() >= 2)
            .flatten()
            .copied()
            .filter(|&k| group[k].blake3.is_none())
            .collect();
        to_hash.sort_unstable();
        for k in to_hash {
            let c = &group[k];
            if !c.online {
                stats.skipped_offline += 1;
                continue;
            }
            if cancel.load(Ordering::Relaxed) {
                flush(store, &mut pending)?;
                return Ok(stats);
            }
            if !unchanged_on_disk(c) || crate::scanner::os_adapter::is_placeholder_now(Path::new(&c.path)) {
                continue;
            }
            if let Ok(full) = full_hash_file(Path::new(&c.path)) {
                stats.full_hashed += 1;
                pending.push(Update::Full { id: c.id.clone(), full });
                if pending.len() >= BATCH {
                    flush(store, &mut pending)?;
                }
            }
        }
        i = j;
    }
    flush(store, &mut pending)?;
    Ok(stats)
}
