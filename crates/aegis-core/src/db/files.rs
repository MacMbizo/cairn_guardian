//! File identity, normalization, Rename-vs-Hardlink resolution, chunks & FTS5 search (`B4 v2.3.1`).

use crate::contracts::{
    FileDetail, LifecycleState, SearchFilters, SearchHit, SearchPage,
};
use crate::error::{AegisError, Result};
use crate::scanner::os_adapter;
use crate::{new_id, now_ms};
use rusqlite::{Connection, OptionalExtension, Transaction, params};
use std::path::{Path, PathBuf};
use std::time::Instant;
use unicode_normalization::UnicodeNormalization;

/// Input descriptor for upserting a discovered filesystem entry (`B4 v2.3.1` §3.1).
#[derive(Debug, Clone)]
pub struct DiscoveredFileInput {
    pub volume_id: String,
    pub has_stable_inode: bool,
    pub case_sensitive: bool,
    pub file_index: Option<Vec<u8>>,
    pub nlink: i64,
    pub atomic_unit_id: Option<String>,
    pub path: PathBuf,
    pub size: i64,
    pub mtime: i64,
    pub ctime: i64,
    pub blake3: Option<Vec<u8>>,
    pub cloud_state: String,
}

/// Canonical path normalization (`B4` §3.1): strips `\\?\`, normalizes separators,
/// applies Unicode NFC, case-folds when `!case_sensitive`, and hashes with `BLAKE3`.
pub fn normalize_path(path: &Path, case_sensitive: bool) -> (String, [u8; 32]) {
    let raw = path.to_string_lossy();
    let stripped = raw.strip_prefix(r"\\?\").unwrap_or(&raw);
    let display = if stripped.len() > 3 {
        stripped.trim_end_matches(['\\', '/']).to_string()
    } else {
        stripped.to_string()
    };

    let mut slashed = display.replace('\\', "/");
    while slashed.contains("//") && !slashed.starts_with("//") {
        slashed = slashed.replace("//", "/");
    }
    let nfc: String = slashed.nfc().collect();
    let key = if case_sensitive {
        nfc
    } else {
        nfc.to_lowercase()
    };
    let hash = *blake3::hash(key.as_bytes()).as_bytes();
    (display, hash)
}

fn file_name_and_ext(path: &Path) -> (String, Option<String>) {
    let name = path
        .file_name()
        .map(|s| s.to_string_lossy().to_string())
        .unwrap_or_else(|| path.to_string_lossy().to_string());
    let ext = path
        .extension()
        .map(|s| s.to_string_lossy().to_lowercase());
    (name, ext)
}

struct CandidateRow {
    id: String,
    path: String,
    size: i64,
    mtime: i64,
    ctime: i64,
    blake3: Option<Vec<u8>>,
}

fn passes_inode_reuse_guard(row: &CandidateRow, input: &DiscoveredFileInput) -> bool {
    if let (Some(a), Some(b)) = (&row.blake3, &input.blake3) {
        return a == b;
    }
    row.size == input.size && (row.mtime == input.mtime || row.ctime == input.ctime)
}

/// Executes `B4 v2.3.1` §3.1 Rename-vs-Hardlink Resolution Algorithm and returns `files.id`.
pub fn upsert_discovered_file(conn: &Connection, input: &DiscoveredFileInput) -> Result<String> {
    let (display_path, path_hash) = normalize_path(&input.path, input.case_sensitive);
    let (name, ext) = file_name_and_ext(&input.path);
    let now = now_ms();

    if !input.has_stable_inode {
        return upsert_unstable_inode(
            conn,
            input,
            &display_path,
            &path_hash,
            &name,
            ext.as_deref(),
            now,
        );
    }

    // Step 1: check existing live row at this exact normalized path
    let live_at_path: Option<(String, Option<Vec<u8>>)> = conn
        .query_row(
            "SELECT id, file_index FROM files
             WHERE volume_id = ?1 AND path_hash = ?2
               AND container_file_id IS NULL AND deleted_at IS NULL
               AND state NOT IN ('quarantine','missing')",
            params![input.volume_id, path_hash.as_slice()],
            |r| Ok((r.get(0)?, r.get(1)?)),
        )
        .optional()?;

    if let Some((existing_id, existing_fi)) = live_at_path {
        if existing_fi == input.file_index {
            conn.execute(
                "UPDATE files
                 SET path = ?1, name = ?2, ext = ?3, nlink = ?4, size = ?5, mtime = ?6, ctime = ?7,
                     atomic_unit_id = COALESCE(?8, atomic_unit_id), cloud_state = ?9,
                     blake3 = COALESCE(?10, blake3), last_seen = ?11, updated_at = ?11
                 WHERE id = ?12",
                params![
                    display_path,
                    name,
                    ext,
                    input.nlink,
                    input.size,
                    input.mtime,
                    input.ctime,
                    input.atomic_unit_id,
                    input.cloud_state,
                    input.blake3,
                    now,
                    existing_id
                ],
            )?;
            return Ok(existing_id);
        }
        // Atomic-save replacement at same path: mark old row 'missing' so idx_files_live_path is freed
        // while preserving its row if its old inode was moved elsewhere.
        conn.execute(
            "UPDATE files SET state = 'missing', updated_at = ?1 WHERE id = ?2",
            params![now, existing_id],
        )?;
    }

    // Step 2: inspect candidate rows sharing (volume_id, file_index)
    if let Some(ref fi) = input.file_index {
        let mut stmt = conn.prepare(
            "SELECT id, path, size, mtime, ctime, blake3
             FROM files
             WHERE volume_id = ?1 AND file_index = ?2 AND deleted_at IS NULL AND state != 'quarantine'",
        )?;
        let rows = stmt.query_map(params![input.volume_id, fi.as_slice()], |r| {
            Ok(CandidateRow {
                id: r.get(0)?,
                path: r.get(1)?,
                size: r.get(2)?,
                mtime: r.get(3)?,
                ctime: r.get(4)?,
                blake3: r.get(5)?,
            })
        })?;
        let mut candidates = Vec::new();
        for row in rows {
            candidates.push(row?);
        }

        if !candidates.is_empty() {
            let mut rename_target: Option<CandidateRow> = None;
            let mut stale_ids: Vec<String> = Vec::new();
            let mut all_still_valid_links = true;

            for cand in candidates {
                let (_, cand_hash) = normalize_path(Path::new(&cand.path), input.case_sensitive);
                let is_case_only_rename = !input.case_sensitive && cand_hash == path_hash;
                let stat_res = os_adapter::inspect_path_metadata(Path::new(&cand.path))
                    .ok()
                    .flatten();
                let same_inode_still_at_old_path = match &stat_res {
                    Some(st) => st.file_index.as_deref() == Some(fi.as_slice()),
                    None => false,
                };

                if input.nlink == 1 || !same_inode_still_at_old_path || is_case_only_rename {
                    all_still_valid_links = false;
                    if rename_target.is_none() && passes_inode_reuse_guard(&cand, input) {
                        rename_target = Some(cand);
                    } else {
                        stale_ids.push(cand.id);
                    }
                }
            }

            for stale_id in stale_ids {
                conn.execute(
                    "UPDATE files SET state = 'missing', deleted_at = ?1, updated_at = ?1 WHERE id = ?2",
                    params![now, stale_id],
                )?;
            }

            if let Some(target) = rename_target {
                conn.execute(
                    "UPDATE files
                     SET path = ?1, path_hash = ?2, name = ?3, ext = ?4, nlink = ?5,
                         size = ?6, mtime = ?7, ctime = ?8, atomic_unit_id = ?9,
                         cloud_state = ?10, state = CASE WHEN state = 'missing' THEN 'active' ELSE state END,
                         blake3 = COALESCE(?11, blake3), last_seen = ?12, updated_at = ?12
                     WHERE id = ?13",
                    params![
                        display_path,
                        path_hash.as_slice(),
                        name,
                        ext,
                        input.nlink,
                        input.size,
                        input.mtime,
                        input.ctime,
                        input.atomic_unit_id,
                        input.cloud_state,
                        input.blake3,
                        now,
                        target.id
                    ],
                )?;
                return Ok(target.id);
            }

            if all_still_valid_links && input.nlink > 1 {
                let new_id = insert_file_row(
                    conn,
                    input,
                    &display_path,
                    &path_hash,
                    &name,
                    ext.as_deref(),
                    now,
                )?;
                conn.execute(
                    "UPDATE files SET nlink = ?1, updated_at = ?2
                     WHERE volume_id = ?3 AND file_index = ?4 AND deleted_at IS NULL",
                    params![input.nlink, now, input.volume_id, fi.as_slice()],
                )?;
                return Ok(new_id);
            }
        }
    }

    insert_file_row(
        conn,
        input,
        &display_path,
        &path_hash,
        &name,
        ext.as_deref(),
        now,
    )
}

fn upsert_unstable_inode(
    conn: &Connection,
    input: &DiscoveredFileInput,
    display_path: &str,
    path_hash: &[u8; 32],
    name: &str,
    ext: Option<&str>,
    now: i64,
) -> Result<String> {
    let existing: Option<String> = conn
        .query_row(
            "SELECT id FROM files
             WHERE volume_id = ?1 AND path_hash = ?2
               AND container_file_id IS NULL AND deleted_at IS NULL
               AND state NOT IN ('quarantine','missing')",
            params![input.volume_id, path_hash.as_slice()],
            |r| r.get(0),
        )
        .optional()?;

    if let Some(id) = existing {
        conn.execute(
            "UPDATE files SET path = ?1, name = ?2, ext = ?3, size = ?4, mtime = ?5, ctime = ?6,
             atomic_unit_id = ?7, cloud_state = ?8, blake3 = COALESCE(?9, blake3),
             last_seen = ?10, updated_at = ?10 WHERE id = ?11",
            params![
                display_path,
                name,
                ext,
                input.size,
                input.mtime,
                input.ctime,
                input.atomic_unit_id,
                input.cloud_state,
                input.blake3,
                now,
                id
            ],
        )?;
        return Ok(id);
    }

    // Rename heuristic on exFAT/SMB: match missing/disappeared row with identical size + mtime (+ blake3 if available)
    let mut stmt = conn.prepare(
        "SELECT id, path, blake3 FROM files
         WHERE volume_id = ?1 AND size = ?2 AND mtime = ?3
           AND deleted_at IS NULL AND state != 'quarantine'",
    )?;
    let rows = stmt.query_map(params![input.volume_id, input.size, input.mtime], |r| {
        Ok((
            r.get::<_, String>(0)?,
            r.get::<_, String>(1)?,
            r.get::<_, Option<Vec<u8>>>(2)?,
        ))
    })?;

    for row in rows {
        let (cand_id, cand_path, cand_blake) = row?;
        let blake_ok = match (&cand_blake, &input.blake3) {
            (Some(a), Some(b)) => a == b,
            _ => true,
        };
        if blake_ok && !Path::new(&cand_path).exists() {
            conn.execute(
                "UPDATE files SET path = ?1, path_hash = ?2, name = ?3, ext = ?4,
                 state = 'active', atomic_unit_id = ?5, last_seen = ?6, updated_at = ?6
                 WHERE id = ?7",
                params![
                    display_path,
                    path_hash.as_slice(),
                    name,
                    ext,
                    input.atomic_unit_id,
                    now,
                    cand_id
                ],
            )?;
            return Ok(cand_id);
        }
    }

    let mut clean_input = input.clone();
    clean_input.file_index = None;
    insert_file_row(conn, &clean_input, display_path, path_hash, name, ext, now)
}

fn insert_file_row(
    conn: &Connection,
    input: &DiscoveredFileInput,
    display_path: &str,
    path_hash: &[u8; 32],
    name: &str,
    ext: Option<&str>,
    now: i64,
) -> Result<String> {
    let id = new_id();
    conn.execute(
        "INSERT INTO files (
            id, volume_id, file_index, nlink, atomic_unit_id, path, path_hash,
            name, ext, size, mtime, ctime, blake3, cloud_state, state,
            first_seen, last_seen, created_at, updated_at
         ) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12, ?13, ?14, 'active', ?15, ?15, ?15, ?15)",
        params![
            id,
            input.volume_id,
            input.file_index.as_deref(),
            input.nlink.max(1),
            input.atomic_unit_id,
            display_path,
            path_hash.as_slice(),
            name,
            ext,
            input.size,
            input.mtime,
            input.ctime,
            input.blake3.as_deref(),
            input.cloud_state,
            now
        ],
    )?;
    Ok(id)
}

/// Tombstones a purged file while deleting its `chunks` (firing FTS5 delete triggers),
/// enqueuing Qdrant deletes, and expiring graph relationships (`B4 v2.3.1` §3.1).
pub fn soft_delete_file_in_tx(tx: &Transaction<'_>, file_id: &str) -> Result<()> {
    let now = now_ms();
    let mut chunk_uuids = Vec::new();
    {
        let mut stmt = tx.prepare("SELECT chunk_uuid FROM chunks WHERE file_id = ?1")?;
        let rows = stmt.query_map(params![file_id], |r| r.get::<_, String>(0))?;
        for row in rows {
            chunk_uuids.push(row?);
        }
    }

    for uuid in chunk_uuids {
        tx.execute(
            "INSERT INTO outbox_events (aggregate_type, aggregate_id, op, payload_json, created_at)
             VALUES ('chunk', ?1, 'delete', '{}', ?2)",
            params![uuid, now],
        )?;
    }

    tx.execute("DELETE FROM chunks WHERE file_id = ?1", params![file_id])?;
    tx.execute(
        "UPDATE relationships SET valid_to = ?1
         WHERE (src_id = ?2 OR dst_id = ?2) AND valid_to IS NULL",
        params![now, file_id],
    )?;
    tx.execute(
        "UPDATE files SET deleted_at = ?1, state = 'missing', updated_at = ?1 WHERE id = ?2",
        params![now, file_id],
    )?;
    Ok(())
}

/// Updates `files.state` to a lifecycle state and enqueues a Qdrant `file_payload` update.
pub fn set_file_lifecycle(
    conn: &Connection,
    file_id: &str,
    lifecycle: LifecycleState,
) -> Result<()> {
    let now = now_ms();
    let state_str = lifecycle.as_str();
    let changed = conn.execute(
        "UPDATE files SET state = ?1, updated_at = ?2 WHERE id = ?3 AND deleted_at IS NULL",
        params![state_str, now, file_id],
    )?;
    if changed == 0 {
        return Err(AegisError::NotFound(format!("file {file_id}")));
    }
    let payload = serde_json::json!({ "file_id": file_id, "state": state_str }).to_string();
    conn.execute(
        "INSERT INTO outbox_events (aggregate_type, aggregate_id, op, payload_json, created_at)
         VALUES ('file_payload', ?1, 'upsert', ?2, ?3)",
        params![file_id, payload, now],
    )?;
    Ok(())
}

/// Inserts or replaces a text chunk (syncing `chunks_fts` via trigger and outbox).
pub fn insert_chunk(
    conn: &Connection,
    file_id: &str,
    seq: i64,
    text: &str,
    token_count: i64,
    is_summary: bool,
) -> Result<String> {
    let chunk_uuid = new_id();
    let now = now_ms();
    conn.execute(
        "INSERT INTO chunks (chunk_uuid, file_id, seq, text, token_count, is_summary, created_at)
         VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7)",
        params![
            chunk_uuid,
            file_id,
            seq,
            text,
            token_count,
            i32::from(is_summary),
            now
        ],
    )?;
    let payload = serde_json::json!({ "file_id": file_id, "seq": seq }).to_string();
    conn.execute(
        "INSERT INTO outbox_events (aggregate_type, aggregate_id, op, payload_json, created_at)
         VALUES ('chunk', ?1, 'upsert', ?2, ?3)",
        params![chunk_uuid, payload, now],
    )?;
    Ok(chunk_uuid)
}

fn sanitize_fts_query(raw: &str) -> String {
    let tokens: Vec<String> = raw
        .split_whitespace()
        .filter_map(|tok| {
            let cleaned: String = tok
                .chars()
                .filter(|c| c.is_alphanumeric() || *c == '_')
                .collect();
            if cleaned.is_empty() {
                None
            } else {
                Some(format!("\"{cleaned}\""))
            }
        })
        .collect();
    tokens.join(" AND ")
}

/// Hybrid/FTS5 search with mandatory SQLite `files.state != 'quarantine'` authoritative post-filter (`B4` §4, `B6` §2).
pub fn search_fts(
    conn: &Connection,
    query: &str,
    filters: &SearchFilters,
    limit: usize,
) -> Result<SearchPage> {
    let started = Instant::now();
    let include_quarantined = filters.include_quarantined.unwrap_or(false);
    let sanitized = sanitize_fts_query(query);
    if sanitized.is_empty() {
        return Ok(SearchPage {
            query: query.to_string(),
            total_hits: 0,
            hits: Vec::new(),
            took_ms: started.elapsed().as_millis() as u64,
        });
    }

    let limit_i64 = (limit.clamp(1, 200)) as i64;
    let mut stmt = conn.prepare(
        "SELECT f.id, f.path, f.name, f.state, f.sensitivity, f.cloud_state,
                snippet(chunks_fts, 0, '[', ']', '...', 16) AS snip,
                -bm25(chunks_fts) AS rank_score
         FROM chunks_fts
         JOIN chunks c ON c.id = chunks_fts.rowid
         JOIN files f ON f.id = c.file_id
         WHERE chunks_fts MATCH ?1
           AND f.deleted_at IS NULL
           AND (?2 = 1 OR f.state != 'quarantine')
           AND (?3 IS NULL OR f.project_id = ?3)
           AND (?4 IS NULL OR f.ext = ?4)
           AND (?5 IS NULL OR f.mtime >= ?5)
           AND (?6 IS NULL OR f.mtime <= ?6)
           AND (?7 IS NULL OR f.sensitivity = ?7)
         ORDER BY rank_score DESC
         LIMIT ?8",
    )?;

    let rows = stmt.query_map(
        params![
            sanitized,
            i32::from(include_quarantined),
            filters.project_id,
            filters.ext,
            filters.after_ts,
            filters.before_ts,
            filters.sensitivity,
            limit_i64
        ],
        |r| {
            Ok(SearchHit {
                file_id: r.get(0)?,
                path: r.get(1)?,
                name: r.get(2)?,
                state: r.get(3)?,
                sensitivity: r.get(4)?,
                cloud_state: r.get(5)?,
                snippet: r.get(6)?,
                score: r.get(7)?,
                match_reason: "FTS5 BM25".to_string(),
            })
        },
    )?;

    let mut hits = Vec::new();
    for row in rows {
        hits.push(row?);
    }

    Ok(SearchPage {
        query: query.to_string(),
        total_hits: hits.len(),
        hits,
        took_ms: started.elapsed().as_millis() as u64,
    })
}

pub fn get_file_detail(conn: &Connection, file_id: &str) -> Result<FileDetail> {
    let mut detail = conn
        .query_row(
            "SELECT id, volume_id, path, name, ext, size, mtime, nlink, state, cloud_state,
                    sensitivity, atomic_unit_id, unit_split_override
             FROM files WHERE id = ?1",
            params![file_id],
            |r| {
                Ok(FileDetail {
                    id: r.get(0)?,
                    volume_id: r.get(1)?,
                    path: r.get(2)?,
                    name: r.get(3)?,
                    ext: r.get(4)?,
                    size: r.get(5)?,
                    mtime: r.get(6)?,
                    nlink: r.get(7)?,
                    state: r.get(8)?,
                    cloud_state: r.get(9)?,
                    sensitivity: r.get(10)?,
                    atomic_unit_id: r.get(11)?,
                    unit_split_override: r.get::<_, i32>(12)? != 0,
                    chunks_preview: Vec::new(),
                })
            },
        )
        .optional()?
        .ok_or_else(|| AegisError::NotFound(format!("file {file_id}")))?;

    let mut stmt = conn.prepare(
        "SELECT text FROM chunks WHERE file_id = ?1 ORDER BY seq ASC LIMIT 5",
    )?;
    let rows = stmt.query_map(params![file_id], |r| r.get::<_, String>(0))?;
    for row in rows {
        detail.chunks_preview.push(row?);
    }
    Ok(detail)
}
