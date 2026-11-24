//! Context-aware Hybrid Search combining FTS5 BM25, encrypted `sqlite-vec` KNN, filename/history
//! matching, collection filtering (`project`/`design_set`/`module`), RRF fusion (`k = 60`), and `R7` masking.

use crate::contracts::{SearchFilters, SearchHit, SearchMode, SearchPage};
use crate::db::files::{SENSITIVE_MASK, is_sensitive, path_is_within};
use crate::db::vec::vec_search_chunks;
use crate::error::Result;
use crate::now_ms;
use rusqlite::{Connection, OptionalExtension, params};
use std::collections::{HashMap, HashSet};

const RRF_K: f64 = 60.0;
const W_FTS: f64 = 1.0;
const W_VEC: f64 = 1.0;
const W_NAME: f64 = 0.85;
const W_FORMER: f64 = 0.75;

#[derive(Clone)]
struct RowRecord {
    file_id: String,
    volume_id: String,
    path: String,
    name: String,
    state: String,
    sensitivity: String,
    cloud_state: String,
    mtime: i64,
    category: Option<String>,
    who: Option<String>,
    drive_label: Option<String>,
    drive_online: bool,
}

#[derive(Default)]
struct FusedCandidate {
    row: Option<RowRecord>,
    rrf: f64,
    fts_snippet: Option<String>,
    vec_snippet: Option<String>,
    had_fts: bool,
    had_vec: bool,
    had_name: bool,
    had_former: bool,
}

struct CollectionMatcher {
    members: HashSet<String>,
    locations: Vec<(String, String, bool)>, // (volume_id, root_path, case_sensitive)
}

impl CollectionMatcher {
    fn load(conn: &Connection, collection_id: &str) -> Result<Self> {
        let mut members = HashSet::new();
        {
            let mut st = conn.prepare("SELECT file_id FROM collection_members WHERE collection_id = ?1")?;
            for r in st.query_map([collection_id], |row| row.get::<_, String>(0))? {
                members.insert(r?);
            }
        }
        let mut locations = Vec::new();
        {
            let mut st = conn.prepare(
                "SELECT cl.volume_id, cl.root_path, COALESCE(v.case_sensitive, 0)
                 FROM collection_locations cl
                 LEFT JOIN volumes v ON v.id = cl.volume_id
                 WHERE cl.collection_id = ?1",
            )?;
            for r in st.query_map([collection_id], |row| {
                Ok((row.get::<_, String>(0)?, row.get::<_, String>(1)?, row.get::<_, i64>(2)? != 0))
            })? {
                locations.push(r?);
            }
        }
        Ok(Self { members, locations })
    }

    fn matches(&self, file_id: &str, volume_id: &str, path: &str) -> bool {
        if self.members.contains(file_id) {
            return true;
        }
        self.locations
            .iter()
            .any(|(vid, root, cs)| vid == volume_id && path_is_within(path, root, *cs))
    }
}

pub(crate) fn sanitize_fts_query(raw: &str) -> String {
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

/// Runs Hybrid / Keyword / Semantic search over `aegis.db` with mandatory quarantine exclusion (`K2`)
/// and `R7` sensitive snippet masking when `!reveal_sensitive`.
pub fn search_hybrid(
    conn: &Connection,
    query: &str,
    query_vec: Option<(&str, &[f32], f64)>,
    filters: &SearchFilters,
    limit: usize,
    reveal_sensitive: bool,
) -> Result<SearchPage> {
    let started = std::time::Instant::now();
    let trimmed = query.trim();
    if trimmed.is_empty() {
        return Ok(SearchPage {
            query: query.to_string(),
            total_hits: 0,
            hits: Vec::new(),
            took_ms: started.elapsed().as_millis() as u64,
        });
    }

    let mode = filters.mode.unwrap_or(SearchMode::Hybrid);
    let include_quarantined = filters.include_quarantined.unwrap_or(false);
    let sanitized = sanitize_fts_query(trimmed);
    let terms: Vec<String> = trimmed
        .split_whitespace()
        .map(|t| {
            t.chars()
                .filter(|c| c.is_alphanumeric() || *c == '-' || *c == '.')
                .collect::<String>()
                .to_lowercase()
        })
        .filter(|t| !t.is_empty())
        .collect();
    let limit = limit.clamp(1, 200);
    let cand_limit = (limit * 4).clamp(40, 400);
    let year = filters.year.map(|y| format!("{y:04}"));
    let category = filters.category.as_ref().map(|c| format!("{c}%"));

    let collection_matcher = match filters.collection_id.as_deref() {
        Some(cid) => Some(CollectionMatcher::load(conn, cid)?),
        None => None,
    };

    const FILTERS: &str = "f.deleted_at IS NULL
           AND (?1 = 1 OR f.state != 'quarantine')
           AND (?2 IS NULL OR f.project_id = ?2)
           AND (?3 IS NULL OR f.ext = ?3)
           AND (?4 IS NULL OR f.mtime >= ?4)
           AND (?5 IS NULL OR f.mtime <= ?5)
           AND (?6 IS NULL OR f.sensitivity = ?6)
           AND (?7 IS NULL OR m.category LIKE ?7)
           AND (?8 IS NULL OR lower(m.who) = lower(?8))
           AND (?9 IS NULL OR substr(m.doc_date, 1, 4) = ?9
                OR strftime('%Y', m.exif_taken_at / 1000, 'unixepoch') = ?9)
           AND (?10 IS NULL OR m.kind = ?10)
           AND (?11 IS NULL OR f.volume_id = ?11)";
    let base = params![
        i32::from(include_quarantined),
        filters.project_id,
        filters.ext,
        filters.after_ts,
        filters.before_ts,
        filters.sensitivity,
        category,
        filters.who,
        year,
        filters.kind,
        filters.volume_id,
    ];

    let read_row = |r: &rusqlite::Row<'_>| -> rusqlite::Result<(RowRecord, String)> {
        let rec = RowRecord {
            file_id: r.get(0)?,
            volume_id: r.get(1)?,
            path: r.get(2)?,
            name: r.get(3)?,
            state: r.get(4)?,
            sensitivity: r.get(5)?,
            cloud_state: r.get(6)?,
            mtime: r.get(7)?,
            category: r.get(8)?,
            who: r.get(9)?,
            drive_label: r.get(10)?,
            drive_online: r.get::<_, i64>(11)? != 0,
        };
        let snippet: String = r.get(12)?;
        Ok((rec, snippet))
    };

    let mut fused: HashMap<String, FusedCandidate> = HashMap::new();
    let matches_collection = |rec: &RowRecord| -> bool {
        collection_matcher
            .as_ref()
            .is_none_or(|cm| cm.matches(&rec.file_id, &rec.volume_id, &rec.path))
    };

    // 1. FTS5 BM25 channel.
    if mode != SearchMode::Semantic && !sanitized.is_empty() {
        let sql = format!(
            "SELECT f.id, f.volume_id, f.path, f.name, f.state, f.sensitivity, f.cloud_state,
                    f.mtime, m.category, m.who, COALESCE(NULLIF(v.label, ''), v.mount_point),
                    CASE WHEN v.status = 'online' THEN 1 ELSE 0 END,
                    snippet(chunks_fts, 0, '[', ']', '...', 16)
             FROM chunks_fts
             JOIN chunks c ON c.id = chunks_fts.rowid
             JOIN files f ON f.id = c.file_id
             LEFT JOIN volumes v ON v.id = f.volume_id
             LEFT JOIN file_meta m ON m.file_id = f.id
             WHERE chunks_fts MATCH ?12 AND {FILTERS}
             ORDER BY -bm25(chunks_fts) DESC LIMIT ?13"
        );
        let mut stmt = conn.prepare(&sql)?;
        let mut p: Vec<&dyn rusqlite::ToSql> = base.to_vec();
        p.push(&sanitized);
        let lim = cand_limit as i64;
        p.push(&lim);
        let mut rank = 0usize;
        let mut seen_ch = HashSet::new();
        for item in stmt.query_map(p.as_slice(), read_row)? {
            let (rec, snippet) = item?;
            if !matches_collection(&rec) || !seen_ch.insert(rec.file_id.clone()) {
                continue;
            }
            rank += 1;
            let entry = fused.entry(rec.file_id.clone()).or_default();
            entry.row.get_or_insert(rec);
            entry.rrf += W_FTS / (RRF_K + rank as f64);
            entry.fts_snippet.get_or_insert(snippet);
            entry.had_fts = true;
        }
    }

    // 2. Vector KNN channel inside SQLCipher (`vec_chunks`).
    if mode != SearchMode::Keyword
        && let Some((model_id, qvec, min_sim)) = query_vec
    {
        let raw_nn = vec_search_chunks(conn, qvec, cand_limit)?;
        let mut chunk_stmt = conn.prepare(&format!(
            "SELECT f.id, f.volume_id, f.path, f.name, f.state, f.sensitivity, f.cloud_state,
                    f.mtime, m.category, m.who, COALESCE(NULLIF(v.label, ''), v.mount_point),
                    CASE WHEN v.status = 'online' THEN 1 ELSE 0 END,
                    c.text
             FROM chunks c
             JOIN chunk_embeddings_meta cem ON cem.chunk_id = c.id
             JOIN files f ON f.id = c.file_id
             LEFT JOIN volumes v ON v.id = f.volume_id
             LEFT JOIN file_meta m ON m.file_id = f.id
             WHERE c.id = ?12 AND cem.model_id = ?13 AND {FILTERS}"
        ))?;
        let mut rank = 0usize;
        let mut seen_ch = HashSet::new();
        for (chunk_id, dist) in raw_nn {
            let sim = 1.0 - dist;
            if sim < min_sim {
                continue;
            }
            let mut p: Vec<&dyn rusqlite::ToSql> = base.to_vec();
            p.push(&chunk_id);
            p.push(&model_id);
            let found = chunk_stmt.query_row(p.as_slice(), read_row).optional()?;
            let Some((rec, body)) = found else {
                continue;
            };
            if !matches_collection(&rec) || !seen_ch.insert(rec.file_id.clone()) {
                continue;
            }
            rank += 1;
            let entry = fused.entry(rec.file_id.clone()).or_default();
            entry.row.get_or_insert(rec);
            entry.rrf += W_VEC / (RRF_K + rank as f64);
            entry
                .vec_snippet
                .get_or_insert_with(|| excerpt_snippet(&body, 180));
            entry.had_vec = true;
        }
    }

    // 3. Name and former-name lexical channels.
    if mode != SearchMode::Semantic && !terms.is_empty() {
        let like_all = |col: &str| {
            terms
                .iter()
                .enumerate()
                .map(|(i, _)| format!("lower({col}) LIKE ?{}", 14 + i))
                .collect::<Vec<_>>()
                .join(" AND ")
        };
        let likes: Vec<String> = terms.iter().map(|t| format!("%{t}%")).collect();
        for (is_former, weight, sql) in [
            (
                false,
                W_NAME,
                format!(
                    "SELECT f.id, f.volume_id, f.path, f.name, f.state, f.sensitivity, f.cloud_state,
                            f.mtime, m.category, m.who, COALESCE(NULLIF(v.label, ''), v.mount_point),
                            CASE WHEN v.status = 'online' THEN 1 ELSE 0 END, ''
                     FROM files f
                     LEFT JOIN volumes v ON v.id = f.volume_id
                     LEFT JOIN file_meta m ON m.file_id = f.id
                     WHERE {} AND {FILTERS} AND ?12 = ?12 LIMIT ?13",
                    like_all("f.name")
                ),
            ),
            (
                true,
                W_FORMER,
                format!(
                    "SELECT DISTINCT f.id, f.volume_id, f.path, f.name, f.state, f.sensitivity, f.cloud_state,
                            f.mtime, m.category, m.who, COALESCE(NULLIF(v.label, ''), v.mount_point),
                            CASE WHEN v.status = 'online' THEN 1 ELSE 0 END, ''
                     FROM file_history h
                     JOIN files f ON f.id = h.file_id
                     LEFT JOIN volumes v ON v.id = f.volume_id
                     LEFT JOIN file_meta m ON m.file_id = f.id
                     WHERE {} AND {FILTERS} AND ?12 = ?12 LIMIT ?13",
                    like_all("h.old_path")
                ),
            ),
        ] {
            let mut stmt = conn.prepare(&sql)?;
            let mut p: Vec<&dyn rusqlite::ToSql> = base.to_vec();
            p.push(&sanitized);
            let lim = cand_limit as i64;
            p.push(&lim);
            for l in &likes {
                p.push(l);
            }
            let mut rank = 0usize;
            let mut seen_ch = HashSet::new();
            for item in stmt.query_map(p.as_slice(), read_row)? {
                let (rec, _) = item?;
                if !matches_collection(&rec) || !seen_ch.insert(rec.file_id.clone()) {
                    continue;
                }
                rank += 1;
                let entry = fused.entry(rec.file_id.clone()).or_default();
                entry.row.get_or_insert(rec);
                entry.rrf += weight / (RRF_K + rank as f64);
                if is_former {
                    entry.had_former = true;
                } else {
                    entry.had_name = true;
                }
            }
        }
    }

    // Load active collections for hit enrichment and context boost.
    let col_members: HashMap<String, (String, String)> = {
        let mut map = HashMap::new();
        let mut st = conn.prepare(
            "SELECT m.file_id, c.name, c.kind
             FROM collection_members m
             JOIN collections c ON c.id = m.collection_id
             WHERE c.state NOT IN ('dismissed', 'rejected', 'no')",
        )?;
        for r in st.query_map([], |row| {
            Ok((
                row.get::<_, String>(0)?,
                row.get::<_, String>(1)?,
                row.get::<_, String>(2)?,
            ))
        })? {
            let (fid, cname, ckind) = r?;
            map.entry(fid).or_insert((cname, ckind));
        }
        map
    };
    let col_locs: Vec<(String, String, String, String, bool)> = {
        let mut st = conn.prepare(
            "SELECT cl.volume_id, cl.root_path, c.name, c.kind, COALESCE(v.case_sensitive, 0)
             FROM collection_locations cl
             JOIN collections c ON c.id = cl.collection_id
             LEFT JOIN volumes v ON v.id = cl.volume_id
             WHERE c.state NOT IN ('dismissed', 'rejected', 'no')
             ORDER BY length(cl.root_path) DESC",
        )?;
        st.query_map([], |row| {
            Ok((
                row.get::<_, String>(0)?,
                row.get::<_, String>(1)?,
                row.get::<_, String>(2)?,
                row.get::<_, String>(3)?,
                row.get::<_, i64>(4)? != 0,
            ))
        })?
        .collect::<rusqlite::Result<_>>()?
    };

    let q_lower = trimmed.to_lowercase();
    let now = now_ms();
    let mut ranked: Vec<SearchHit> = Vec::with_capacity(fused.len());
    for (fid, cand) in fused {
        let Some(rec) = cand.row else { continue };
        let (col_name, col_kind) = if let Some((n, k)) = col_members.get(&fid) {
            (Some(n.clone()), Some(k.clone()))
        } else if let Some((_, _, n, k, _)) = col_locs
            .iter()
            .find(|(vid, root, _, _, cs)| *vid == rec.volume_id && path_is_within(&rec.path, root, *cs))
        {
            (Some(n.clone()), Some(k.clone()))
        } else {
            (None, None)
        };

        let mut score = cand.rrf;
        let context_hit = col_name
            .as_deref()
            .is_some_and(|n| query_mentions_label(&q_lower, n))
            || rec
                .who
                .as_deref()
                .is_some_and(|w| query_mentions_label(&q_lower, w))
            || rec
                .category
                .as_deref()
                .is_some_and(|c| query_mentions_label(&q_lower, c));
        if context_hit {
            score += 0.10 * cand.rrf;
        }
        let age_days = ((now - rec.mtime).max(0) as f64) / 86_400_000.0;
        score += 0.05 * cand.rrf * (-age_days / 180.0).exp();

        let match_reason = if cand.had_fts && cand.had_vec {
            "hybrid"
        } else if cand.had_vec {
            "semantic"
        } else if cand.had_fts {
            "content"
        } else if cand.had_name {
            "name"
        } else {
            "former name"
        };

        let mut snippet = cand
            .fts_snippet
            .or(cand.vec_snippet)
            .unwrap_or_default();
        if !reveal_sensitive && is_sensitive(&rec.sensitivity) && !snippet.is_empty() {
            snippet = SENSITIVE_MASK.to_string();
        }

        let was = conn
            .query_row(
                "SELECT old_path FROM file_history WHERE file_id = ?1 ORDER BY changed_at ASC LIMIT 1",
                [&fid],
                |r| r.get(0),
            )
            .ok();

        ranked.push(SearchHit {
            file_id: fid,
            path: rec.path,
            name: rec.name,
            state: rec.state,
            sensitivity: rec.sensitivity,
            cloud_state: rec.cloud_state,
            snippet,
            score,
            match_reason: match_reason.to_string(),
            was,
            collection_name: col_name,
            collection_kind: col_kind,
            drive_label: rec.drive_label,
            drive_online: rec.drive_online,
        });
    }

    ranked.sort_by(|a, b| {
        b.score
            .total_cmp(&a.score)
            .then_with(|| a.name.cmp(&b.name))
    });
    ranked.truncate(limit);

    Ok(SearchPage {
        query: query.to_string(),
        total_hits: ranked.len(),
        hits: ranked,
        took_ms: started.elapsed().as_millis() as u64,
    })
}

fn query_mentions_label(q_lower: &str, label: &str) -> bool {
    label
        .split(|c: char| !c.is_alphanumeric())
        .filter(|w| w.len() >= 3)
        .any(|w| q_lower.contains(&w.to_lowercase()))
}

fn excerpt_snippet(text: &str, max_chars: usize) -> String {
    let trimmed = text.split_whitespace().collect::<Vec<_>>().join(" ");
    if trimmed.chars().count() <= max_chars {
        return trimmed;
    }
    let prefix: String = trimmed.chars().take(max_chars).collect();
    format!("{prefix}…")
}
