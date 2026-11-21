//! Live-index batching (spec §6.1): file-system changes are collected until the folder has been
//! quiet for a moment, then applied in small transactions so UI commands never wait long.

use crate::db::SqliteStore;
use crate::error::Result;
use std::collections::BTreeSet;
use std::path::PathBuf;

/// Quiet time before a burst of changes is applied.
pub const DEBOUNCE_MS: i64 = 2_000;
/// Paths per `index_paths` call (one transaction each).
pub const MAX_PATHS_PER_TX: usize = 500;

#[derive(Default)]
pub struct ChangeBatcher {
    pending: BTreeSet<PathBuf>,
    last_push: i64,
}

impl ChangeBatcher {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn push(&mut self, path: PathBuf, now: i64) {
        self.pending.insert(path);
        self.last_push = self.last_push.max(now);
    }

    /// Everything pending, de-duplicated and sorted, in chunks — once changes have stopped for
    /// [`DEBOUNCE_MS`]. Empty while changes are still arriving.
    pub fn drain_ready(&mut self, now: i64) -> Vec<Vec<PathBuf>> {
        if self.pending.is_empty() || now - self.last_push < DEBOUNCE_MS {
            return Vec::new();
        }
        let all: Vec<PathBuf> = std::mem::take(&mut self.pending).into_iter().collect();
        all.chunks(MAX_PATHS_PER_TX).map(|c| c.to_vec()).collect()
    }
}

/// Applies drained chunks to the index, one transaction per chunk. Returns paths applied.
pub fn index_changed(store: &mut SqliteStore, volume_id: &str, chunks: Vec<Vec<PathBuf>>) -> Result<usize> {
    let mut n = 0;
    for chunk in chunks {
        n += crate::scanner::index_paths(store, volume_id, &chunk)?;
    }
    Ok(n)
}
