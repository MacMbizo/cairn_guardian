//! Encrypted `sqlite-vec` (`vec_chunks`) vector table lifecycle, incremental sync queue,
//! orphan pruning, and KNN search inside `aegis.db` (`D1`, `K2`, `K5`).

use crate::error::{AegisError, Result};
use crate::now_ms;
use rusqlite::{Connection, OptionalExtension, params};

const MAX_EMBED_TEXT_CHARS: usize = 2_400;

/// One document chunk pending vector embedding for the currently selected model/dimension.
#[derive(Debug, Clone)]
pub struct PendingChunk {
    pub chunk_id: i64,
    pub chunk_uuid: String,
    pub file_id: String,
    pub embed_text: String,
    pub content_hash: [u8; 32],
}

/// Builds the canonical text embedded for a chunk, combining filename and filing metadata with chunk body.
pub fn chunk_embed_text(
    file_name: &str,
    category: Option<&str>,
    who: Option<&str>,
    text: &str,
) -> String {
    let mut meta_parts = Vec::new();
    if let Some(c) = category.map(str::trim).filter(|s| !s.is_empty()) {
        meta_parts.push(c.to_string());
    }
    if let Some(w) = who.map(str::trim).filter(|s| !s.is_empty()) {
        meta_parts.push(w.to_string());
    }
    let header = if meta_parts.is_empty() {
        file_name.trim().to_string()
    } else {
        format!("[{}] {}", meta_parts.join(" · "), file_name.trim())
    };
    let full = if header.is_empty() {
        text.to_string()
    } else {
        format!("{header}\n{text}")
    };
    truncate_chars(&full, MAX_EMBED_TEXT_CHARS)
}

/// BLAKE3 digest of `embed_text` stored in `chunk_embeddings_meta.content_hash` to skip unchanged chunks.
pub fn chunk_content_hash(embed_text: &str) -> [u8; 32] {
    *blake3::hash(embed_text.as_bytes()).as_bytes()
}

/// Serializes `f32` vector coordinates into packed little-endian bytes for `sqlite-vec`.
pub fn f32s_to_le_bytes(vals: &[f32]) -> Vec<u8> {
    let mut out = Vec::with_capacity(vals.len() * 4);
    for v in vals {
        out.extend_from_slice(&v.to_le_bytes());
    }
    out
}

pub fn vec_table_exists(conn: &Connection) -> Result<bool> {
    let exists: i64 = conn.query_row(
        "SELECT EXISTS(SELECT 1 FROM sqlite_master WHERE type = 'table' AND name = 'vec_chunks')",
        [],
        |r| r.get(0),
    )?;
    Ok(exists != 0)
}

fn current_vec_ddl(conn: &Connection) -> Result<Option<String>> {
    conn.query_row(
        "SELECT sql FROM sqlite_master WHERE type = 'table' AND name = 'vec_chunks'",
        [],
        |r| r.get(0),
    )
    .optional()
    .map_err(Into::into)
}

/// Ensures `vec_chunks` exists with cosine metric for `dim` and evicts vectors belonging to a
/// different `model_id` (returning `true` when the virtual table had to be recreated).
pub fn ensure_vec_table(conn: &Connection, model_id: &str, dim: usize) -> Result<bool> {
    if dim == 0 {
        return Err(AegisError::InvalidInput("embedding dimension must be > 0".into()));
    }
    let expected_type = format!("float[{dim}]");
    let ddl_opt = current_vec_ddl(conn)?;
    let matches_dim = ddl_opt
        .as_deref()
        .is_some_and(|sql| sql.contains(&expected_type) && sql.contains("cosine"));

    if !matches_dim {
        conn.execute_batch("DROP TABLE IF EXISTS vec_chunks;")?;
        conn.execute("DELETE FROM chunk_embeddings_meta", [])?;
        conn.execute_batch(&format!(
            "CREATE VIRTUAL TABLE vec_chunks USING vec0(embedding float[{dim}] distance_metric=cosine);"
        ))?;
        return Ok(true);
    }

    // Same dimension, different model: purge stale vectors for old models cleanly.
    let stale_ids: Vec<i64> = {
        let mut st = conn.prepare(
            "SELECT chunk_id FROM chunk_embeddings_meta WHERE model_id != ?1 OR dim != ?2",
        )?;
        let rows = st.query_map(params![model_id, dim as i64], |r| r.get::<_, i64>(0))?;
        rows.collect::<rusqlite::Result<_>>()?
    };
    if !stale_ids.is_empty() {
        for cid in &stale_ids {
            conn.execute("DELETE FROM vec_chunks WHERE rowid = ?1", params![cid])?;
        }
        conn.execute(
            "DELETE FROM chunk_embeddings_meta WHERE model_id != ?1 OR dim != ?2",
            params![model_id, dim as i64],
        )?;
    }
    Ok(false)
}

/// Lists up to `limit` live non-quarantined chunks whose vector for `(model_id, dim)` is missing or stale.
pub fn pending_chunks(
    conn: &Connection,
    model_id: &str,
    dim: usize,
    limit: usize,
) -> Result<Vec<PendingChunk>> {
    if limit == 0 {
        return Ok(Vec::new());
    }
    let mut stmt = conn.prepare(
        "SELECT c.id, c.chunk_uuid, c.file_id, f.name, m.category, m.who, c.text,
                cem.model_id, cem.dim, cem.content_hash
         FROM chunks c
         JOIN files f ON f.id = c.file_id
         LEFT JOIN file_meta m ON m.file_id = f.id
         LEFT JOIN chunk_embeddings_meta cem ON cem.chunk_id = c.id
         WHERE f.deleted_at IS NULL AND f.state != 'quarantine'
         ORDER BY (cem.chunk_id IS NOT NULL) ASC, f.mtime DESC, c.seq ASC",
    )?;
    let rows = stmt.query_map([], |r| {
        Ok((
            r.get::<_, i64>(0)?,
            r.get::<_, String>(1)?,
            r.get::<_, String>(2)?,
            r.get::<_, String>(3)?,
            r.get::<_, Option<String>>(4)?,
            r.get::<_, Option<String>>(5)?,
            r.get::<_, String>(6)?,
            r.get::<_, Option<String>>(7)?,
            r.get::<_, Option<i64>>(8)?,
            r.get::<_, Option<Vec<u8>>>(9)?,
        ))
    })?;

    let mut out = Vec::new();
    for row in rows {
        if out.len() >= limit {
            break;
        }
        let (chunk_id, chunk_uuid, file_id, name, cat, who, text, existing_model, existing_dim, existing_hash) =
            row?;
        let embed_text = chunk_embed_text(&name, cat.as_deref(), who.as_deref(), &text);
        let content_hash = chunk_content_hash(&embed_text);
        let up_to_date = existing_model.as_deref() == Some(model_id)
            && existing_dim == Some(dim as i64)
            && existing_hash.as_deref() == Some(content_hash.as_slice());
        if !up_to_date {
            out.push(PendingChunk {
                chunk_id,
                chunk_uuid,
                file_id,
                embed_text,
                content_hash,
            });
        }
    }
    Ok(out)
}

/// Inserts or updates vector rows in `vec_chunks` + `chunk_embeddings_meta` and marks chunk outbox events processed.
pub fn upsert_chunk_embeddings(
    conn: &Connection,
    model_id: &str,
    dim: usize,
    items: &[(PendingChunk, Vec<f32>)],
) -> Result<usize> {
    if items.is_empty() {
        return Ok(0);
    }
    ensure_vec_table(conn, model_id, dim)?;
    let now = now_ms();
    conn.execute_batch("SAVEPOINT aegis_vec_upsert;")?;
    let res: Result<usize> = (|| {
        let mut count = 0usize;
        for (item, vec) in items {
            if vec.len() != dim {
                return Err(AegisError::InvalidInput(format!(
                    "embedding vector length {} != expected {dim}",
                    vec.len()
                )));
            }
            let bytes = f32s_to_le_bytes(vec);
            conn.execute("DELETE FROM vec_chunks WHERE rowid = ?1", params![item.chunk_id])?;
            conn.execute(
                "INSERT INTO vec_chunks (rowid, embedding) VALUES (?1, ?2)",
                params![item.chunk_id, bytes],
            )?;
            conn.execute(
                "INSERT INTO chunk_embeddings_meta (chunk_id, file_id, model_id, dim, content_hash, embedded_at)
                 VALUES (?1, ?2, ?3, ?4, ?5, ?6)
                 ON CONFLICT(chunk_id) DO UPDATE SET
                    file_id = excluded.file_id,
                    model_id = excluded.model_id,
                    dim = excluded.dim,
                    content_hash = excluded.content_hash,
                    embedded_at = excluded.embedded_at",
                params![
                    item.chunk_id,
                    item.file_id,
                    model_id,
                    dim as i64,
                    item.content_hash.as_slice(),
                    now
                ],
            )?;
            conn.execute(
                "UPDATE outbox_events SET processed_at = ?1
                 WHERE aggregate_type = 'chunk' AND aggregate_id = ?2 AND processed_at IS NULL",
                params![now, item.chunk_uuid],
            )?;
            count += 1;
        }
        Ok(count)
    })();
    match res {
        Ok(n) => {
            conn.execute_batch("RELEASE SAVEPOINT aegis_vec_upsert;")?;
            Ok(n)
        }
        Err(e) => {
            let _ = conn.execute_batch(
                "ROLLBACK TO SAVEPOINT aegis_vec_upsert; RELEASE SAVEPOINT aegis_vec_upsert;",
            );
            Err(e)
        }
    }
}

/// Removes vectors belonging to `file_id` (called before deleting its chunks).
pub fn delete_vectors_for_file(conn: &Connection, file_id: &str) -> Result<()> {
    conn.execute(
        "DELETE FROM chunk_embeddings_meta WHERE file_id = ?1",
        params![file_id],
    )?;
    if vec_table_exists(conn)? {
        let ids: Vec<i64> = {
            let mut st = conn.prepare("SELECT id FROM chunks WHERE file_id = ?1")?;
            let rows = st.query_map(params![file_id], |r| r.get::<_, i64>(0))?;
            rows.collect::<rusqlite::Result<_>>()?
        };
        for id in ids {
            conn.execute("DELETE FROM vec_chunks WHERE rowid = ?1", params![id])?;
        }
    }
    Ok(())
}

/// Counts `(vec_orphans, meta_orphans)` where chunk rows are gone or files were deleted/quarantined.
pub fn count_orphan_vectors(conn: &Connection) -> Result<usize> {
    let meta_orphans: i64 = conn.query_row(
        "SELECT COUNT(*) FROM chunk_embeddings_meta m
         WHERE NOT EXISTS (
            SELECT 1 FROM chunks c JOIN files f ON f.id = c.file_id
            WHERE c.id = m.chunk_id AND f.deleted_at IS NULL AND f.state != 'quarantine'
         )",
        [],
        |r| r.get(0),
    )?;
    if !vec_table_exists(conn)? {
        return Ok(meta_orphans.max(0) as usize);
    }
    let vec_orphans: i64 = conn.query_row(
        "SELECT COUNT(*) FROM vec_chunks v
         WHERE NOT EXISTS (
            SELECT 1 FROM chunks c JOIN files f ON f.id = c.file_id
            WHERE c.id = v.rowid AND f.deleted_at IS NULL AND f.state != 'quarantine'
         )",
        [],
        |r| r.get(0),
    )?;
    Ok(meta_orphans.max(vec_orphans).max(0) as usize)
}

/// Removes orphan vectors for deleted or quarantined files cleanly and retires chunk delete outbox events.
pub fn prune_orphan_vectors(conn: &Connection) -> Result<usize> {
    let mut removed = 0usize;
    let has_vec = vec_table_exists(conn)?;
    if has_vec {
        let dead_vec_ids: Vec<i64> = {
            let mut st = conn.prepare(
                "SELECT v.rowid FROM vec_chunks v
                 WHERE NOT EXISTS (
                    SELECT 1 FROM chunks c JOIN files f ON f.id = c.file_id
                    WHERE c.id = v.rowid AND f.deleted_at IS NULL AND f.state != 'quarantine'
                 )",
            )?;
            let rows = st.query_map([], |r| r.get::<_, i64>(0))?;
            rows.collect::<rusqlite::Result<_>>()?
        };
        for id in dead_vec_ids {
            removed += conn.execute("DELETE FROM vec_chunks WHERE rowid = ?1", params![id])?;
        }
    }
    let meta_removed = conn.execute(
        "DELETE FROM chunk_embeddings_meta
         WHERE NOT EXISTS (
            SELECT 1 FROM chunks c JOIN files f ON f.id = c.file_id
            WHERE c.id = chunk_embeddings_meta.chunk_id AND f.deleted_at IS NULL AND f.state != 'quarantine'
         )",
        [],
    )?;
    conn.execute(
        "UPDATE outbox_events SET processed_at = ?1
         WHERE aggregate_type = 'chunk' AND op = 'delete' AND processed_at IS NULL",
        params![now_ms()],
    )?;
    Ok(removed.max(meta_removed))
}

/// Returns `(total_live_chunks, embedded_live_chunks_for_model)` without modifying the database.
pub fn vec_coverage(conn: &Connection, model_id: &str, dim: usize) -> Result<(i64, i64)> {
    let total_chunks: i64 = conn.query_row(
        "SELECT COUNT(*) FROM chunks c JOIN files f ON f.id = c.file_id
         WHERE f.deleted_at IS NULL AND f.state != 'quarantine'",
        [],
        |r| r.get(0),
    )?;
    if !vec_table_exists(conn)? {
        return Ok((total_chunks, 0));
    }
    let expected_type = format!("float[{dim}]");
    let ddl_ok = current_vec_ddl(conn)?
        .as_deref()
        .is_some_and(|sql| sql.contains(&expected_type));
    if !ddl_ok {
        return Ok((total_chunks, 0));
    }
    let embedded_chunks: i64 = conn.query_row(
        "SELECT COUNT(*) FROM chunk_embeddings_meta cem
         JOIN chunks c ON c.id = cem.chunk_id
         JOIN files f ON f.id = c.file_id
         WHERE f.deleted_at IS NULL AND f.state != 'quarantine'
           AND cem.model_id = ?1 AND cem.dim = ?2",
        params![model_id, dim as i64],
        |r| r.get(0),
    )?;
    Ok((total_chunks, embedded_chunks))
}

/// Executes top-`k` KNN cosine distance search inside `vec_chunks`, returning `(chunk_id, cosine_distance)`.
pub fn vec_search_chunks(conn: &Connection, query_vec: &[f32], k: usize) -> Result<Vec<(i64, f64)>> {
    if k == 0 || query_vec.is_empty() || !vec_table_exists(conn)? {
        return Ok(Vec::new());
    }
    let expected_type = format!("float[{}]", query_vec.len());
    let ddl_ok = current_vec_ddl(conn)?
        .as_deref()
        .is_some_and(|sql| sql.contains(&expected_type));
    if !ddl_ok {
        return Ok(Vec::new());
    }
    let qbytes = f32s_to_le_bytes(query_vec);
    let mut stmt = conn.prepare(
        "SELECT rowid, distance FROM vec_chunks WHERE embedding MATCH ?1 ORDER BY distance LIMIT ?2",
    )?;
    let rows = stmt.query_map(params![qbytes, k as i64], |r| Ok((r.get(0)?, r.get(1)?)))?;
    rows.collect::<rusqlite::Result<_>>().map_err(Into::into)
}

fn truncate_chars(s: &str, max_chars: usize) -> String {
    if s.chars().count() <= max_chars {
        return s.to_string();
    }
    s.chars().take(max_chars).collect()
}
