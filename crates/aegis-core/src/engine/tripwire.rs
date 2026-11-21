//! Ransomware tripwire (spec §6.2): watches the burst rate of changes per PC root. A burst
//! that rewrites a large share of a folder into random-looking bytes, or renames many files to
//! an extension the folder never had, trips it: the owner is told at once and that folder's
//! backup is locked so the damage is never copied over the good backup.

use super::{Finding, FindingAction, Level, Screen};
use crate::backup::guard::{ENTROPY_SUSPECT_FRACTION, looks_encrypted, looks_random};
use crate::db::files::path_is_within;
use crate::error::Result;
use rusqlite::Connection;
use serde::{Deserialize, Serialize};
use std::collections::{HashMap, HashSet, VecDeque};
use std::path::{Path, PathBuf};
use ts_rs::TS;

pub const TRIP_WINDOW_MS: i64 = 60_000;
pub const TRIP_MIN_FILES: usize = 200;
pub const TRIP_MIN_FRACTION: f64 = 0.20;
pub const TRIP_SAMPLE: usize = 20;
pub const TRIP_NEW_EXT_RENAMES: usize = 100;
/// A root is re-evaluated at most this often during a burst (each evaluation scans the index).
pub const EVAL_EVERY_MS: i64 = 1_000;

#[derive(Debug, Clone, PartialEq)]
pub enum ChangeKind {
    Created,
    Modified,
    Removed,
    Renamed { to: PathBuf },
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, TS)]
#[ts(export)]
pub struct Trip {
    pub root: String,
    #[ts(type = "number")]
    pub changed: i64,
    #[ts(type = "number")]
    pub total: i64,
    #[ts(type = "number")]
    pub suspect_encrypted: i64,
    #[ts(type = "number")]
    pub new_ext_renames: i64,
}

struct Event {
    at: i64,
    path: PathBuf,
    kind: ChangeKind,
}

/// Sliding-window change counter over the watched PC roots.
pub struct Tripwire {
    roots: Vec<String>,
    events: HashMap<String, VecDeque<Event>>,
    /// Roots already reported for the current burst; cleared when their window empties.
    tripped: HashSet<String>,
    last_eval: HashMap<String, i64>,
    scans: usize,
}

fn ext_of(p: &Path) -> Option<String> {
    p.extension().and_then(|e| e.to_str()).map(|e| e.to_ascii_lowercase())
}

impl Tripwire {
    pub fn new(roots: Vec<String>) -> Self {
        Self { roots, events: HashMap::new(), tripped: HashSet::new(), last_eval: HashMap::new(), scans: 0 }
    }

    /// Records one change; changes outside every watched root are ignored.
    pub fn observe(&mut self, path: &Path, kind: ChangeKind, now: i64) {
        let s = path.to_string_lossy();
        let Some(root) = self.roots.iter().filter(|r| path_is_within(&s, r, false)).max_by_key(|r| r.len()) else {
            return;
        };
        self.events
            .entry(root.clone())
            .or_default()
            .push_back(Event { at: now, path: path.to_path_buf(), kind });
    }

    /// How many times the index has been scanned (for tests: evaluation is throttled).
    pub fn scans(&self) -> usize {
        self.scans
    }

    /// Roots whose last minute looks like an attack. Each burst is reported once. A root is
    /// evaluated at most once per [`EVAL_EVERY_MS`], so a big unzip or sync stays cheap.
    pub fn evaluate(&mut self, conn: &Connection, now: i64) -> Result<Vec<Trip>> {
        let mut trips = Vec::new();
        for root in self.roots.clone() {
            let Some(q) = self.events.get_mut(&root) else { continue };
            while q.front().is_some_and(|e| e.at < now - TRIP_WINDOW_MS) {
                q.pop_front();
            }
            if q.is_empty() {
                self.tripped.remove(&root);
                self.last_eval.remove(&root);
                continue;
            }
            if self.tripped.contains(&root) {
                continue;
            }
            let changed: HashSet<&Path> = q.iter().map(|e| e.path.as_path()).collect();
            if changed.len() <= TRIP_MIN_FILES {
                continue;
            }
            if self.last_eval.get(&root).is_some_and(|t| now - t < EVAL_EVERY_MS) {
                continue;
            }
            self.last_eval.insert(root.clone(), now);
            self.scans += 1;
            let (total, exts) = indexed_under(conn, &root)?;
            if changed.len() as f64 <= TRIP_MIN_FRACTION * total as f64 {
                continue;
            }
            // Ransomware *replaces* files; a big copy-in of a new file type removes nothing.
            // Windows reports a rename as a removal plus a creation, so "photo.jpg" ->
            // "photo.jpg.locked" is a new-extension file whose name minus that extension was
            // just removed.
            let is_new_ext = |p: &Path| ext_of(p).is_some_and(|x| !exts.contains(&x));
            let removed: HashSet<String> = q
                .iter()
                .filter(|e| e.kind == ChangeKind::Removed)
                .map(|e| e.path.to_string_lossy().to_lowercase())
                .collect();
            let created_new_ext: HashSet<&Path> = q
                .iter()
                .filter(|e| e.kind == ChangeKind::Created && is_new_ext(&e.path))
                .map(|e| e.path.as_path())
                .collect();
            let new_ext_renames = q
                .iter()
                .filter_map(|e| match &e.kind {
                    ChangeKind::Renamed { to } if is_new_ext(to) => Some(to.as_path()),
                    ChangeKind::Created
                        if created_new_ext.contains(e.path.as_path())
                            && removed.contains(&e.path.with_extension("").to_string_lossy().to_lowercase()) =>
                    {
                        Some(e.path.as_path())
                    }
                    _ => None,
                })
                .collect::<HashSet<_>>()
                .len();
            // New-extension files only look suspicious while originals are disappearing.
            let replacing = !created_new_ext.is_empty() && removed.len() * 2 >= created_new_ext.len();
            let mut seen = HashSet::new();
            let candidates: Vec<&Path> = q
                .iter()
                .filter_map(|e| match &e.kind {
                    ChangeKind::Created | ChangeKind::Modified => Some(e.path.as_path()),
                    ChangeKind::Renamed { to } => Some(to.as_path()),
                    ChangeKind::Removed => None,
                })
                .filter(|p| seen.insert(*p) && p.is_file() && !is_cloud_placeholder(p))
                .collect();
            let step = (candidates.len() / TRIP_SAMPLE).max(1);
            let sample: Vec<&Path> = candidates.iter().step_by(step).take(TRIP_SAMPLE).copied().collect();
            // Random bytes under an extension the folder never had (".locked") are as telling as
            // a document that turned into random bytes, while originals are being removed.
            let suspect = sample
                .iter()
                .filter(|p| {
                    looks_encrypted(p, ext_of(p).as_deref()).unwrap_or(false)
                        || (replacing && is_new_ext(p) && looks_random(p).unwrap_or(false))
                })
                .count();
            let scrambled = suspect >= 1 && suspect as f64 >= ENTROPY_SUSPECT_FRACTION * sample.len() as f64;
            if scrambled || new_ext_renames > TRIP_NEW_EXT_RENAMES {
                self.tripped.insert(root.clone());
                trips.push(Trip {
                    root: root.clone(),
                    changed: changed.len() as i64,
                    total,
                    suspect_encrypted: suspect as i64,
                    new_ext_renames: new_ext_renames as i64,
                });
            }
        }
        Ok(trips)
    }
}

/// Cloud-only files (OneDrive "free up space"): reading them would download them again.
#[cfg(windows)]
fn is_cloud_placeholder(p: &Path) -> bool {
    use std::os::windows::fs::MetadataExt;
    const OFFLINE: u32 = 0x1000;
    const RECALL_ON_OPEN: u32 = 0x4_0000;
    const RECALL_ON_DATA_ACCESS: u32 = 0x40_0000;
    std::fs::symlink_metadata(p).is_ok_and(|m| m.file_attributes() & (OFFLINE | RECALL_ON_OPEN | RECALL_ON_DATA_ACCESS) != 0)
}

#[cfg(not(windows))]
fn is_cloud_placeholder(_p: &Path) -> bool {
    false
}

/// Live indexed files under `root`, and the extensions they use.
fn indexed_under(conn: &Connection, root: &str) -> Result<(i64, HashSet<String>)> {
    let mut stmt = conn.prepare(
        "SELECT path, ext FROM files
         WHERE deleted_at IS NULL AND state NOT IN ('quarantine', 'missing')
           AND lower(substr(path, 1, ?2)) = lower(?1)",
    )?;
    let rows = stmt.query_map(rusqlite::params![root, root.chars().count() as i64], |r| {
        Ok((r.get::<_, String>(0)?, r.get::<_, Option<String>>(1)?))
    })?;
    let mut n = 0;
    let mut exts = HashSet::new();
    for r in rows {
        let (path, ext) = r?;
        if path_is_within(&path, root, false) {
            n += 1;
            if let Some(e) = ext {
                exts.insert(e.to_ascii_lowercase());
            }
        }
    }
    Ok((n, exts))
}

/// The urgent reminder for a trip.
pub fn trip_finding(t: &Trip) -> Finding {
    let name = Path::new(&t.root)
        .file_name()
        .map(|n| n.to_string_lossy().to_string())
        .unwrap_or_else(|| t.root.clone());
    let scrambled = if t.suspect_encrypted > 0 { format!(", {} look scrambled", t.suspect_encrypted) } else { String::new() };
    Finding {
        kind: "tripwire".into(),
        subject: t.root.clone(),
        level: Level::Now,
        title: format!("Lots of files in {name} just changed at once"),
        detail: format!(
            "{} files changed in a minute{scrambled}. Backup of this folder is paused until you check.",
            t.changed
        ),
        bytes: None,
        action: FindingAction::Open { screen: Screen::Drives },
    }
}
