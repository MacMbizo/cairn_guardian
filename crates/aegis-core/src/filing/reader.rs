//! Background content reader (spec §7.1): reads documents, photos and songs that are new or
//! changed since they were last read, one file at a time in isolation, and stores the text for
//! search and the metadata for filing.

use super::classify::{ClassifyInput, classify, kind_of};
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

/// Message for a file whose reading did not finish (a hang, or a crash that killed Cairn).
pub const TOO_LONG: &str = "reading took too long";

/// Runs `f` on a worker thread with a roomy stack, so a panic or an endless loop costs one
/// file, not the run. A worker that hangs can't be stopped; the caller ends its run.
pub fn run_isolated<T: Send + 'static>(
    f: impl FnOnce() -> crate::error::Result<T> + Send + 'static,
    timeout: Duration,
) -> std::result::Result<T, String> {
    let (tx, rx) = std::sync::mpsc::channel();
    let spawned = std::thread::Builder::new().stack_size(16 * 1024 * 1024).spawn(move || {
        let res = std::panic::catch_unwind(std::panic::AssertUnwindSafe(f));
        let _ = tx.send(match res {
            Ok(Ok(x)) => Ok(x),
            Ok(Err(e)) => Err(e.to_string()),
            Err(_) => Err("the file couldn't be read (its format looks damaged)".to_string()),
        });
    });
    if spawned.is_err() {
        return Err("couldn't start reading".into());
    }
    rx.recv_timeout(timeout).unwrap_or_else(|_| Err(TOO_LONG.to_string()))
}

fn extract_isolated(path: PathBuf, timeout: Duration) -> std::result::Result<Extracted, String> {
    run_isolated(move || extract(&path), timeout)
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
    let learned = learned_orgs(store.conn())?;
    let rows: Vec<(String, String, String, Option<String>, i64)> = {
        let mut stmt = store.conn().prepare(
            "SELECT f.id, f.path, f.name, f.ext, f.mtime FROM files f
             JOIN volumes v ON v.id = f.volume_id AND v.status = 'online' AND COALESCE(v.role, '') != 'backup'
             LEFT JOIN file_meta m ON m.file_id = f.id
             WHERE f.deleted_at IS NULL AND f.state NOT IN ('quarantine', 'missing')
               AND f.cloud_state = 'local' AND f.size <= ?1 AND f.atomic_unit_id IS NULL
               AND f.container_file_id IS NULL
               AND (m.read_at IS NULL OR m.read_at < f.mtime)
             ORDER BY f.mtime DESC",
        )?;
        let rows = stmt.query_map([MAX_FILE_BYTES as i64], |r| {
            Ok((
                r.get::<_, String>(0)?,
                r.get::<_, String>(1)?,
                r.get::<_, String>(2)?,
                r.get::<_, Option<String>>(3)?,
                r.get::<_, i64>(4)?,
            ))
        })?;
        rows.filter_map(|r| r.ok())
            .filter(|(_, _, _, ext, _)| ext.as_deref().is_some_and(readable))
            .take(limit)
            .collect()
    };
    let mut rep = ReadReport::default();
    for (id, path, name, ext, mtime) in rows {
        let ext = ext.unwrap_or_default();
        // Gone since it was indexed (or its drive just went away): the live index will catch
        // up; it isn't unreadable.
        if !std::path::Path::new(&path).is_file() {
            rep.skipped += 1;
            continue;
        }
        let mut meta = load_meta(store.conn(), &id)?.unwrap_or_else(|| FileMeta::new(&id, kind_of(&ext, &name, false)));
        meta.kind = kind_of(&ext, &name, false);
        let now = now_ms();
        // Marked before opening: if a parser crashes Cairn outright, this file isn't tried first
        // again on every start.
        meta.read_at = Some(now);
        meta.read_error = Some(TOO_LONG.into());
        save_meta(store.conn(), &meta)?;
        let mut text: Option<String> = None;
        let result = extract_isolated(PathBuf::from(&path), timeout);
        let hung = matches!(&result, Err(e) if e == TOO_LONG);
        match result {
            Ok(x) => {
                if let Some(t) = &x.text {
                    store_text(store, &id, t)?;
                }
                text = x.text;
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
        apply_classification(&mut meta, &name, &ext, text.as_deref(), mtime, false, &learned);
        meta.read_at = Some(now);
        save_meta(store.conn(), &meta)?;
        if hung {
            // A stuck reader keeps a core busy until Cairn restarts: don't start more now.
            break;
        }
    }
    Ok(rep)
}

/// Organisation names the owner has approved in filing rules.
fn learned_orgs(conn: &rusqlite::Connection) -> Result<Vec<String>> {
    let mut stmt = conn.prepare("SELECT DISTINCT who FROM filing_rules WHERE who IS NOT NULL AND approvals > 0")?;
    Ok(stmt.query_map([], |r| r.get(0))?.collect::<rusqlite::Result<_>>()?)
}

fn apply_classification(
    meta: &mut FileMeta,
    name: &str,
    ext: &str,
    text: Option<&str>,
    mtime: i64,
    in_atomic_unit: bool,
    learned: &[String],
) {
    let c = classify(&ClassifyInput {
        name,
        ext,
        text,
        exif_taken_at: meta.exif_taken_at,
        mtime,
        in_atomic_unit,
        learned_orgs: learned,
    });
    meta.kind = c.kind;
    meta.category = c.category;
    meta.doc_type = c.doc_type;
    meta.who = c.who;
    meta.doc_date = c.doc_date;
    meta.date_source = c.date_source;
    meta.confidence = c.confidence;
    meta.evidence = c.evidence;
}

/// Gives files Cairn can't read (installers, videos, code, system files) their kind and
/// confidence from name and type alone. Each file is done once. Returns files classified.
pub fn classify_unread(store: &mut SqliteStore, limit: usize) -> Result<usize> {
    let rows: Vec<(String, String, Option<String>, i64, bool)> = {
        let mut stmt = store.conn().prepare(
            "SELECT f.id, f.name, f.ext, f.mtime, f.atomic_unit_id IS NOT NULL FROM files f
             LEFT JOIN file_meta m ON m.file_id = f.id
             WHERE m.file_id IS NULL AND f.deleted_at IS NULL AND f.container_file_id IS NULL
               AND f.state NOT IN ('quarantine', 'missing')
               -- The Backup drive holds copies of files that are read where they live.
               AND NOT EXISTS (SELECT 1 FROM volumes v WHERE v.id = f.volume_id AND v.role = 'backup')",
        )?;
        let rows = stmt.query_map([], |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?, r.get(3)?, r.get(4)?)))?;
        rows.filter_map(|r| r.ok())
            .filter(|(_, _, ext, _, unit): &(String, String, Option<String>, i64, bool)| {
                *unit || !ext.as_deref().is_some_and(readable)
            })
            .take(limit)
            .collect()
    };
    let learned = learned_orgs(store.conn())?;
    let n = rows.len();
    let tx = store.conn().unchecked_transaction()?;
    for (id, name, ext, mtime, unit) in rows {
        let ext = ext.unwrap_or_default();
        let mut meta = FileMeta::new(&id, kind_of(&ext, &name, unit));
        apply_classification(&mut meta, &name, &ext, None, mtime, unit, &learned);
        save_meta(&tx, &meta)?;
    }
    tx.commit()?;
    Ok(n)
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
        classify_unread(store, 20_000)?;
        read_pending(store, 2_000, READ_TIMEOUT)?;
        Ok(vec![])
    }
}

#[cfg(test)]
mod tests {
    use super::{TOO_LONG, run_isolated};
    use std::time::{Duration, Instant};

    #[test]
    fn a_hanging_reader_is_abandoned_on_time() {
        let t = Instant::now();
        let r: Result<(), String> = run_isolated(
            || {
                std::thread::sleep(Duration::from_secs(5));
                Ok(())
            },
            Duration::from_millis(100),
        );
        assert_eq!(r, Err(TOO_LONG.to_string()));
        assert!(t.elapsed() < Duration::from_secs(2));
    }

    #[test]
    fn a_panicking_reader_costs_one_file() {
        let r: Result<(), String> = run_isolated(|| panic!("bad pdf"), Duration::from_secs(5));
        assert!(r.unwrap_err().contains("damaged"));
    }
}
