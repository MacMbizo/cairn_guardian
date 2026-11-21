//! Mass-change / ransomware guard for backup top-ups (spec §7.7.1).
//!
//! A top-up never silently mirrors an attack or an accidental mass delete onto the Backup
//! drive. A folder whose backed-up files would change too much at once, or whose changes
//! look encrypted, is held back until the owner answers.

use super::topup::{TopUpKind, TopUpPlan};
use crate::db::SqliteStore;
use crate::db::audit::append_audit_in_tx;
use crate::db::files::path_is_within;
use crate::error::Result;
use rusqlite::Connection;
use serde::{Deserialize, Serialize};
use std::collections::{BTreeMap, HashSet};
use std::io::Read;
use std::path::Path;
use ts_rs::TS;

/// A top-up halts for a folder when more than this share of its backed-up files would be
/// overwritten or retired at once.
pub const MASS_CHANGE_FRACTION: f64 = 0.20;

/// …or when at least this share of its backed-up files (and at least one file) changed into
/// something that looks encrypted. Ransomware rewrites files in place; real edits almost never
/// turn a document into random bytes, so even a few such files are a strong signal. 5% keeps a
/// single odd file in a big folder from tripping it, while catching an attack early.
pub const ENTROPY_SUSPECT_FRACTION: f64 = 0.05;

/// Bits of entropy per byte above which content is indistinguishable from random.
const RANDOM_BITS_PER_BYTE: f64 = 7.5;

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, TS)]
#[ts(export)]
pub struct GuardHalt {
    pub source_folder: String,
    #[ts(type = "number")]
    pub changed: i64,
    #[ts(type = "number")]
    pub total: i64,
    #[ts(type = "number")]
    pub suspect_encrypted: i64,
}

impl GuardHalt {
    /// Owner-facing question for this halt.
    pub fn message(&self) -> String {
        let name = Path::new(&self.source_folder)
            .file_name()
            .map(|n| n.to_string_lossy().to_string())
            .unwrap_or_else(|| self.source_folder.clone());
        format!(
            "An unusual number of files changed in {name} ({} of {}). Back them up anyway?",
            self.changed, self.total
        )
    }
}

#[derive(Debug, Clone, PartialEq)]
pub enum GuardVerdict {
    Proceed,
    Halt(Vec<GuardHalt>),
}

fn entropy(bytes: &[u8]) -> f64 {
    if bytes.is_empty() {
        return 0.0;
    }
    let mut counts = [0usize; 256];
    for &b in bytes {
        counts[b as usize] += 1;
    }
    let n = bytes.len() as f64;
    counts
        .iter()
        .filter(|&&c| c > 0)
        .map(|&c| {
            let p = c as f64 / n;
            -p * p.log2()
        })
        .sum()
}

/// `Some(true)` when the leading bytes match the format `ext` promises, `Some(false)` when they
/// don't, `None` for extensions without a signature (unknown or plain text).
fn magic_matches(ext: &str, head: &[u8]) -> Option<bool> {
    let starts = |sig: &[u8]| head.starts_with(sig);
    Some(match ext {
        "jpg" | "jpeg" => starts(&[0xFF, 0xD8, 0xFF]),
        "png" => starts(&[0x89, b'P', b'N', b'G']),
        "gif" => starts(b"GIF8"),
        "bmp" => starts(b"BM"),
        "pdf" => starts(b"%PDF"),
        "zip" | "docx" | "xlsx" | "pptx" | "odt" | "ods" | "odp" | "epub" | "jar" => starts(b"PK\x03\x04") || starts(b"PK\x05\x06"),
        "doc" | "xls" | "ppt" | "msg" => starts(&[0xD0, 0xCF, 0x11, 0xE0]),
        "7z" => starts(&[b'7', b'z', 0xBC, 0xAF]),
        "gz" | "tgz" => starts(&[0x1F, 0x8B]),
        "rar" => starts(b"Rar!"),
        "exe" | "dll" => starts(b"MZ"),
        "mp3" => starts(b"ID3") || (head.len() > 1 && head[0] == 0xFF && head[1] & 0xE0 == 0xE0),
        "wav" | "avi" | "webp" => starts(b"RIFF"),
        "mp4" | "m4a" | "m4v" | "mov" | "heic" => head.len() >= 8 && &head[4..8] == b"ftyp",
        "mkv" | "webm" => starts(&[0x1A, 0x45, 0xDF, 0xA3]),
        "flac" => starts(b"fLaC"),
        _ => return None,
    })
}

const TEXT_EXTS: [&str; 14] = [
    "txt", "md", "csv", "tsv", "json", "xml", "html", "htm", "log", "rtf", "ini", "yaml", "yml", "tex",
];

/// True when a file's first 64 KB are indistinguishable from random bytes.
pub fn looks_random(path: &Path) -> Result<bool> {
    let mut f = std::fs::File::open(path)?;
    let mut head = vec![0u8; 64 * 1024];
    let n = f.read(&mut head)?;
    head.truncate(n);
    Ok(head.len() >= 256 && entropy(&head) > RANDOM_BITS_PER_BYTE)
}

/// True when a file's content looks encrypted for what its name says it is: near-random bytes
/// in a text file, or near-random bytes without the signature of a known binary format.
/// Genuine compressed formats (JPEG, ZIP-based Office files, video) keep their signature.
pub fn looks_encrypted(path: &Path, ext: Option<&str>) -> Result<bool> {
    let mut f = std::fs::File::open(path)?;
    let mut head = vec![0u8; 64 * 1024];
    let n = f.read(&mut head)?;
    head.truncate(n);
    if head.len() < 256 {
        return Ok(false);
    }
    let random = entropy(&head) > RANDOM_BITS_PER_BYTE;
    if !random {
        return Ok(false);
    }
    let ext = ext.map(|e| e.to_ascii_lowercase()).unwrap_or_default();
    if TEXT_EXTS.contains(&ext.as_str()) {
        return Ok(true);
    }
    Ok(matches!(magic_matches(&ext, &head), Some(false)))
}

fn overrides_key(backup_volume_id: &str) -> String {
    format!("guard.override:{backup_volume_id}")
}

fn overrides(conn: &Connection, backup_volume_id: &str) -> Result<HashSet<String>> {
    let raw: Option<String> = conn
        .query_row(
            "SELECT value_json FROM settings WHERE key = ?1 AND scope = 'global'",
            [overrides_key(backup_volume_id)],
            |r| r.get(0),
        )
        .ok();
    Ok(raw
        .and_then(|j| serde_json::from_str::<Vec<String>>(&j).ok())
        .unwrap_or_default()
        .into_iter()
        .collect())
}

const LOCKS_KEY: &str = "backup.locked_folders";

/// Folders the ransomware tripwire locked: their backup waits until the owner checks.
pub fn locked_folders(conn: &Connection) -> Result<Vec<String>> {
    let raw: Option<String> = conn
        .query_row(
            "SELECT value_json FROM settings WHERE key = ?1 AND scope = 'global'",
            [LOCKS_KEY],
            |r| r.get(0),
        )
        .ok();
    Ok(raw.and_then(|j| serde_json::from_str::<Vec<String>>(&j).ok()).unwrap_or_default())
}

fn save_locks(store: &SqliteStore, locks: &[String]) -> Result<()> {
    store.set_setting(LOCKS_KEY, "global", &serde_json::to_string(locks)?)
}

/// Two folders overlap when one contains the other.
fn overlaps(a: &str, b: &str) -> bool {
    path_is_within(a, b, false) || path_is_within(b, a, false)
}

/// Locks `folder`'s backup after a suspicious burst of changes (spec §6.2). Audited.
pub fn lock_folder(store: &mut SqliteStore, folder: &str, reason: &str) -> Result<()> {
    let mut locks = locked_folders(store.conn())?;
    if !locks.iter().any(|l| l == folder) {
        locks.push(folder.to_string());
        save_locks(store, &locks)?;
    }
    // An earlier "back them up anyway" must not let this new burst through.
    let keys: Vec<(String, String)> = {
        let mut stmt = store
            .conn()
            .prepare("SELECT key, value_json FROM settings WHERE scope = 'global' AND key LIKE 'guard.override:%'")?;
        stmt.query_map([], |r| Ok((r.get(0)?, r.get(1)?)))?.collect::<rusqlite::Result<_>>()?
    };
    for (key, json) in keys {
        let set: Vec<String> = serde_json::from_str(&json).unwrap_or_default();
        let kept: Vec<&String> = set.iter().filter(|o| !overlaps(o, folder)).collect();
        if kept.len() != set.len() {
            store.set_setting(&key, "global", &serde_json::to_string(&kept)?)?;
        }
    }
    store.with_full_sync_tx(|tx| {
        append_audit_in_tx(
            tx,
            "system",
            "backup.tripwire_lock",
            folder,
            &serde_json::json!({ "reason": reason }).to_string(),
        )?;
        Ok(())
    })
}

fn unlock_overlapping(store: &SqliteStore, folder: &str) -> Result<()> {
    let locks = locked_folders(store.conn())?;
    let kept: Vec<String> = locks.iter().filter(|l| !overlaps(l, folder)).cloned().collect();
    if kept.len() != locks.len() {
        save_locks(store, &kept)?;
    }
    Ok(())
}

/// "It was me": lifts the tripwire lock on `folder` and lets the next top-up of the Backup
/// drive copy those changes without asking again. Audited.
pub fn release_lock(store: &mut SqliteStore, backup_volume_id: Option<&str>, folder: &str) -> Result<()> {
    unlock_overlapping(store, folder)?;
    match backup_volume_id {
        Some(id) => override_halt(store, id, folder),
        None => store.with_full_sync_tx(|tx| {
            append_audit_in_tx(tx, "user", "backup.tripwire_release", folder, "{}")?;
            Ok(())
        }),
    }
}

/// The owner answered "back them up anyway" for `source_folder`: the next top-up of this drive
/// skips the guard for that folder (and folders inside it) once, and any tripwire lock around
/// or inside it is lifted. Audited.
pub fn override_halt(store: &mut SqliteStore, backup_volume_id: &str, source_folder: &str) -> Result<()> {
    unlock_overlapping(store, source_folder)?;
    let mut set = overrides(store.conn(), backup_volume_id)?;
    set.insert(source_folder.to_string());
    let list: Vec<&String> = set.iter().collect();
    store.set_setting(&overrides_key(backup_volume_id), "global", &serde_json::to_string(&list)?)?;
    store.with_full_sync_tx(|tx| {
        append_audit_in_tx(
            tx,
            "user",
            "backup.guard_override",
            backup_volume_id,
            &serde_json::json!({ "folder": source_folder }).to_string(),
        )?;
        Ok(())
    })
}

/// Consumes the one-shot overrides after a completed top-up.
pub(crate) fn clear_overrides(store: &SqliteStore, backup_volume_id: &str) -> Result<()> {
    store.conn().execute(
        "DELETE FROM settings WHERE key = ?1 AND scope = 'global'",
        [overrides_key(backup_volume_id)],
    )?;
    Ok(())
}

/// Checks a planned top-up, folder by folder.
pub fn check(conn: &Connection, plan: &TopUpPlan) -> Result<GuardVerdict> {
    let skip = overrides(conn, &plan.backup_volume_id)?;
    let locks = locked_folders(conn)?;
    let mut locked_hit: HashSet<&str> = HashSet::new();
    let mut by_folder: BTreeMap<&str, (i64, i64)> = BTreeMap::new(); // (changed, suspect)
    for it in &plan.items {
        if it.source_folder.is_empty() {
            continue;
        }
        // Locks come first: an override never lets a locked folder through.
        let in_lock = locks.iter().any(|l| {
            path_is_within(&it.source_folder, l, false)
                || it.source_path.as_deref().is_some_and(|p| path_is_within(p, l, false))
        });
        if in_lock {
            locked_hit.insert(it.source_folder.as_str());
        } else if skip.iter().any(|s| path_is_within(&it.source_folder, s, false)) {
            continue;
        }
        let entry = by_folder.entry(it.source_folder.as_str()).or_insert((0, 0));
        match it.kind {
            TopUpKind::Replace => {
                entry.0 += 1;
                if let Some(p) = it.source_path.as_deref() {
                    let path = Path::new(p);
                    let ext = path.extension().and_then(|e| e.to_str());
                    if looks_encrypted(path, ext).unwrap_or(false) {
                        entry.1 += 1;
                    }
                }
            }
            TopUpKind::RetireDeleted => entry.0 += 1,
            TopUpKind::Add | TopUpKind::RetireUncovered => {}
        }
    }
    let mut halts = Vec::new();
    for (folder, (changed, suspect)) in by_folder {
        if locked_hit.contains(folder) {
            let total = backed_up_in(conn, &plan.backup_volume_id, folder)?;
            halts.push(GuardHalt { source_folder: folder.to_string(), changed, total, suspect_encrypted: suspect });
            continue;
        }
        if changed == 0 {
            continue;
        }
        let total = backed_up_in(conn, &plan.backup_volume_id, folder)?;
        if total == 0 {
            continue;
        }
        let mass = changed as f64 / total as f64 > MASS_CHANGE_FRACTION;
        let encrypted = suspect >= 1 && suspect as f64 >= ENTROPY_SUSPECT_FRACTION * total as f64;
        if mass || encrypted {
            halts.push(GuardHalt {
                source_folder: folder.to_string(),
                changed,
                total,
                suspect_encrypted: suspect,
            });
        }
    }
    Ok(if halts.is_empty() { GuardVerdict::Proceed } else { GuardVerdict::Halt(halts) })
}

/// Number of files from `folder` that currently have a live copy on the Backup drive.
fn backed_up_in(conn: &Connection, backup_volume_id: &str, folder: &str) -> Result<i64> {
    let mut stmt = conn.prepare(
        "SELECT f.path FROM backup_copies c JOIN files f ON f.id = c.source_file_id
         WHERE c.backup_volume_id = ?1",
    )?;
    let rows = stmt.query_map([backup_volume_id], |r| r.get::<_, String>(0))?;
    let mut n = 0;
    for r in rows {
        if path_is_within(&r?, folder, false) {
            n += 1;
        }
    }
    Ok(n)
}
