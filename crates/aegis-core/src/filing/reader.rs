//! Background content reader (spec §7.1): reads documents, photos and songs that are new or
//! changed since they were last read, one file at a time in isolation, and stores the text for
//! search and the metadata for filing.

use super::classify::kind_of;
use super::extract::{Extracted, MAX_FILE_BYTES, extract, readable};
use super::{FileMeta, load_meta, save_meta};
use crate::db::SqliteStore;
use crate::db::files::{delete_chunks_for_file, insert_chunk};
use crate::engine::{Finding, Watch};
use crate::error::Result;
use crate::now_ms;
use std::path::PathBuf;
use std::time::Duration;

/// Text is stored in pieces of about this many characters.
const CHUNK_CHARS: usize = 2_000;
/// Per-file limit before a file counts as unreadable.
pub const READ_TIMEOUT: Duration = Duration::from_secs(10);

#[derive(Debug, Default, Clone, Copy, PartialEq, Eq)]
pub struct ReadReport {
    pub read: usize,
    pub unreadable: usize,
    pub skipped: usize,
}

/// Runs `extract` on a worker thread so a panic or an endless loop costs one file, not the run.
fn extract_isolated(path: PathBuf, timeout: Duration) -> std::result::Result<Extracted, String> {
    let (tx, rx) = std::sync::mpsc::channel();
    std::thread::spawn(move || {
        let res = std::panic::catch_unwind(|| extract(&path));
        let _ = tx.send(match res {
            Ok(Ok(x)) => Ok(x),
            Ok(Err(e)) => Err(e.to_string()),
            Err(_) => Err("the file couldn't be read (its format looks damaged)".to_string()),
        });
    });
    rx.recv_timeout(timeout).unwrap_or_else(|_| Err("reading took too long".to_string()))
}

fn store_text(store: &SqliteStore, file_id: &str, text: &str) -> Result<()> {
    delete_chunks_for_file(store.conn(), file_id)?;
    let mut seq = 0i64;
    let mut cur = String::new();
    for word in text.split_whitespace() {
        if cur.len() + word.len() + 1 > CHUNK_CHARS && !cur.is_empty() {
            let words = cur.split_whitespace().count() as i64;
            insert_chunk(store.conn(), file_id, seq, &cur, words.max(1), seq == 0)?;
            seq += 1;
            cur.clear();
        }
        if !cur.is_empty() {
            cur.push(' ');
        }
        cur.push_str(word);
    }
    if !cur.is_empty() {
        let words = cur.split_whitespace().count() as i64;
        insert_chunk(store.conn(), file_id, seq, &cur, words.max(1), seq == 0)?;
    }
    Ok(())
}

/// Reads up to `limit` files that are new or changed since they were last read.
pub fn read_pending(store: &mut SqliteStore, limit: usize, timeout: Duration) -> Result<ReadReport> {
    // Local, live, not too big, not inside a code project, and new or changed since last read.
    let rows: Vec<(String, String, String, Option<String>)> = {
        let mut stmt = store.conn().prepare(
            "SELECT f.id, f.path, f.name, f.ext FROM files f
             LEFT JOIN file_meta m ON m.file_id = f.id
             WHERE f.deleted_at IS NULL AND f.state NOT IN ('quarantine', 'missing')
               AND f.cloud_state = 'local' AND f.size <= ?1 AND f.atomic_unit_id IS NULL
               AND f.container_file_id IS NULL
               AND (m.read_at IS NULL OR m.read_at < f.mtime)
             ORDER BY f.mtime DESC",
        )?;
        let rows = stmt.query_map([MAX_FILE_BYTES as i64], |r| {
            Ok((r.get::<_, String>(0)?, r.get::<_, String>(1)?, r.get::<_, String>(2)?, r.get::<_, Option<String>>(3)?))
        })?;
        rows.filter_map(|r| r.ok())
            .filter(|(_, _, _, ext)| ext.as_deref().is_some_and(readable))
            .take(limit)
            .collect()
    };
    let mut rep = ReadReport::default();
    for (id, path, name, ext) in rows {
        let ext = ext.unwrap_or_default();
        let mut meta = load_meta(store.conn(), &id)?.unwrap_or_else(|| FileMeta::new(&id, kind_of(&ext, &name, false)));
        meta.kind = kind_of(&ext, &name, false);
        let now = now_ms();
        match extract_isolated(PathBuf::from(&path), timeout) {
            Ok(x) => {
                if let Some(text) = &x.text {
                    store_text(store, &id, text)?;
                }
                if let Some(e) = x.exif {
                    meta.exif_taken_at = e.taken_at;
                    meta.camera = e.camera;
                }
                if let Some(t) = x.tags {
                    meta.artist = t.artist;
                    meta.album = t.album;
                    meta.title = meta.title.or(t.title);
                }
                meta.read_error = None;
                rep.read += 1;
            }
            Err(e) => {
                meta.read_error = Some(e);
                rep.unreadable += 1;
            }
        }
        meta.read_at = Some(now);
        save_meta(store.conn(), &meta)?;
    }
    Ok(rep)
}

/// Reads new and changed files while the PC is idle.
pub struct ReaderWatch;

impl Watch for ReaderWatch {
    fn name(&self) -> &'static str {
        "reader"
    }
    fn every_ms(&self) -> i64 {
        600_000
    }
    fn needs_idle(&self) -> bool {
        true
    }
    fn kinds(&self) -> &'static [&'static str] {
        &[]
    }
    fn run(&mut self, store: &mut SqliteStore, _now: i64) -> Result<Vec<Finding>> {
        read_pending(store, 2_000, READ_TIMEOUT)?;
        Ok(vec![])
    }
}
