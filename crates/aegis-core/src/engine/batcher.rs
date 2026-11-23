//! Live-index batching (spec §6.1): file-system changes are collected until the folder has been
//! quiet for a moment, then applied in small transactions so UI commands never wait long.

use crate::db::SqliteStore;
use crate::db::files::{normalize_path, path_is_within};
use crate::error::Result;
use std::collections::BTreeSet;
use std::path::{Path, PathBuf};

/// Quiet time before a burst of changes is applied.
pub const DEBOUNCE_MS: i64 = 2_000;
/// A file written non-stop (an open mailbox) never goes quiet: apply anyway after this long.
pub const MAX_WAIT_MS: i64 = 30_000;
/// Paths per `index_paths` call (one transaction each).
pub const MAX_PATHS_PER_TX: usize = 500;

#[derive(Default)]
pub struct ChangeBatcher {
    pending: BTreeSet<PathBuf>,
    last_push: i64,
    first_pending: Option<i64>,
}

impl ChangeBatcher {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn push(&mut self, path: PathBuf, now: i64) {
        self.pending.insert(path);
        self.last_push = self.last_push.max(now);
        self.first_pending.get_or_insert(now);
    }

    /// Puts paths back after a failed apply; they go out with the next quiet moment.
    pub fn requeue(&mut self, paths: impl IntoIterator<Item = PathBuf>, now: i64) {
        for p in paths {
            self.push(p, now);
        }
    }

    pub fn is_empty(&self) -> bool {
        self.pending.is_empty()
    }

    /// Everything pending, de-duplicated and sorted, in chunks — once changes have stopped for
    /// [`DEBOUNCE_MS`], or once the oldest has waited [`MAX_WAIT_MS`].
    pub fn drain_ready(&mut self, now: i64) -> Vec<Vec<PathBuf>> {
        let quiet = now - self.last_push >= DEBOUNCE_MS;
        let overdue = self.first_pending.is_some_and(|t| now - t >= MAX_WAIT_MS);
        if self.pending.is_empty() || !(quiet || overdue) {
            return Vec::new();
        }
        self.first_pending = None;
        let all: Vec<PathBuf> = std::mem::take(&mut self.pending).into_iter().collect();
        all.chunks(MAX_PATHS_PER_TX).map(|c| c.to_vec()).collect()
    }
}

fn files_under(dir: &Path, out: &mut Vec<PathBuf>) {
    let Ok(rd) = std::fs::read_dir(dir) else { return };
    for e in rd.flatten() {
        let p = e.path();
        if crate::scanner::is_live_excluded_path(&p) {
            continue;
        }
        let Ok(meta) = std::fs::symlink_metadata(&p) else { continue };
        if meta.file_type().is_symlink() {
            continue;
        }
        if meta.is_dir() {
            files_under(&p, out);
        } else if meta.is_file() {
            out.push(PathBuf::from(normalize_path(&p, false).0));
        }
    }
}

/// Windows reports a moved, renamed or deleted folder by the folder's own path only: expands
/// a folder into its files, and a vanished path into the indexed files that were under it.
fn expand(store: &SqliteStore, volume_id: &str, paths: Vec<PathBuf>) -> Result<Vec<PathBuf>> {
    let mut out = BTreeSet::new();
    let mut vanished_dirs: Vec<String> = Vec::new();
    let mut file_check = store.conn().prepare(
        "SELECT EXISTS(SELECT 1 FROM files WHERE volume_id = ?1 AND path_hash = ?2 AND deleted_at IS NULL)",
    )?;
    for p in paths {
        if crate::scanner::is_live_excluded_path(&p) {
            continue;
        }
        if p.is_dir() {
            let mut files = Vec::new();
            files_under(&p, &mut files);
            out.extend(files);
        } else if !p.exists() {
            let (_, ph) = normalize_path(&p, false);
            let exact_file: bool = file_check.query_row(rusqlite::params![volume_id, ph.as_slice()], |r| r.get(0))?;
            if exact_file {
                out.insert(p);
            } else {
                vanished_dirs.push(p.to_string_lossy().to_string());
                out.insert(p);
            }
        } else {
            out.insert(p);
        }
    }
    if !vanished_dirs.is_empty() {
        let mut stmt = store.conn().prepare(
            "SELECT path FROM files WHERE volume_id = ?1 AND deleted_at IS NULL",
        )?;
        let rows = stmt.query_map([volume_id], |r| r.get::<_, String>(0))?;
        for r in rows {
            let child = r?;
            if vanished_dirs.iter().any(|pref| path_is_within(&child, pref, false)) {
                out.insert(PathBuf::from(child));
            }
        }
    }
    Ok(out.into_iter().collect())
}

/// Applies drained chunks to the index, one transaction per chunk of at most
/// [`MAX_PATHS_PER_TX`] files (folders expanded first). Returns paths applied. On an error the
/// caller requeues the chunk (re-applying a path is harmless).
pub fn index_changed(store: &mut SqliteStore, volume_id: &str, chunks: Vec<Vec<PathBuf>>) -> Result<usize> {
    let mut n = 0;
    for chunk in chunks {
        let files = expand(store, volume_id, chunk)?;
        for part in files.chunks(MAX_PATHS_PER_TX) {
            n += crate::scanner::index_paths(store, volume_id, part)?;
        }
    }
    Ok(n)
}
