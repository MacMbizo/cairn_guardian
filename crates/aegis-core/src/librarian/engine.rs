//! Autonomous Research Librarian engine (`2.3`) (`K1`–`K5`, `R4`, `R5`, `R7`, `R9`, `R11`, `D1`).

use std::collections::{HashMap, HashSet};
use std::path::Path;
use std::time::Instant;

use rusqlite::{Connection, OptionalExtension, params};

use crate::db::files::{is_sensitive, path_is_within};
use crate::db::vec::{load_file_vectors, vec_search_chunks};
use crate::error::{AegisError, Result};
use crate::now_ms;
use crate::versions::patterns::is_excluded_relative_path;

use super::{
    AssociationReason, BriefCitation, LibrarianOptions, RelatedReference, ResearchCompanionReport,
    ResearchDocItem, ResearchDocShelf, SharedEntityRef,
};

const DEFAULT_MAX_REFS: usize = 18;
const DEFAULT_MAX_TERMS: usize = 14;
const VEC_KNN_LIMIT: usize = 160;
const FTS_KNN_LIMIT: usize = 160;

const STOPWORDS: &[&str] = &[
    "about", "above", "after", "again", "against", "almost", "along", "already", "also",
    "although", "always", "among", "another", "around", "because", "before", "being", "below",
    "between", "both", "could", "does", "doing", "done", "during", "each", "either", "enough",
    "every", "everyone", "everything", "first", "following", "from", "further", "general",
    "going", "have", "having", "here", "herself", "himself", "however", "include", "included",
    "including", "instead", "into", "itself", "just", "keep", "largely", "later", "least",
    "like", "likely", "made", "make", "makes", "making", "many", "maybe", "more", "most",
    "mostly", "much", "must", "myself", "need", "needs", "neither", "never", "next", "none",
    "note", "notes", "nothing", "often", "once", "only", "other", "others", "otherwise", "ours",
    "ourselves", "over", "overall", "page", "part", "perhaps", "quite", "rather", "really",
    "recent", "regarding", "related", "remain", "right", "said", "same", "section", "several",
    "shall", "should", "since", "small", "some", "somehow", "someone", "something", "sometimes",
    "somewhere", "still", "such", "sure", "take", "taken", "than", "that", "their", "theirs",
    "them", "themselves", "then", "there", "therefore", "these", "they", "thing", "things",
    "think", "this", "those", "though", "through", "throughout", "thus", "today", "together",
    "toward", "towards", "under", "until", "upon", "used", "using", "usually", "very", "want",
    "well", "were", "what", "whatever", "when", "whenever", "where", "whereas", "wherever",
    "whether", "which", "while", "whose", "will", "with", "within", "without", "would", "your",
    "yours", "yourself", "yourselves",
];

struct FileDbRow {
    file_id: String,
    volume_id: String,
    mount_point: String,
    path: String,
    name: String,
    ext: String,
    size_bytes: i64,
    mtime: i64,
    drive_label: String,
    drive_online: bool,
    sensitivity: String,
}

struct CollectionLoc {
    collection_id: String,
    volume_id: String,
    root_path: String,
    case_sensitive: bool,
}

struct CollectionsLookup {
    names: HashMap<String, String>,
    locations: Vec<CollectionLoc>,
    members: HashMap<String, String>,
}

#[derive(Default)]
struct CandidateAcc {
    semantic_score: f64,
    has_fts: bool,
    bm25_strength: f64,
    shared_entities: Vec<SharedEntityRef>,
    seen_entity_ids: HashSet<String>,
    excerpt: Option<String>,
}

struct PinMeta {
    note: String,
}

/// Prompt assembly ready for on-demand `aegis-llm` brief synthesis (`K5`, `R4`, `R7`).
#[derive(Debug, Clone)]
pub struct PreparedBriefContext {
    pub title: String,
    pub system_prompt: String,
    pub user_prompt: String,
    pub citations: Vec<BriefCitation>,
}

/// Extracts ranked, stopword-filtered lowercase distinctive terms from `text`.
pub fn extract_distinctive_terms(text: &str, max_terms: usize) -> Vec<String> {
    if max_terms == 0 {
        return Vec::new();
    }
    let stops: HashSet<&'static str> = STOPWORDS.iter().copied().collect();
    let mut tf: HashMap<String, usize> = HashMap::new();
    for raw in text.split(|c: char| !c.is_alphanumeric() && c != '_') {
        let lower = raw.trim_matches('_').to_ascii_lowercase();
        if lower.len() < 4 || lower.chars().all(|c| c.is_ascii_digit()) {
            continue;
        }
        if stops.contains(lower.as_str()) {
            continue;
        }
        *tf.entry(lower).or_insert(0) += 1;
    }

    let mut ranked: Vec<(String, f64)> = tf
        .into_iter()
        .map(|(term, count)| {
            let len_boost = 1.0 + ((term.len().clamp(4, 14) - 4) as f64) * 0.08;
            let score = (count as f64) * len_boost;
            (term, score)
        })
        .collect();

    ranked.sort_by(|a, b| b.1.total_cmp(&a.1).then_with(|| a.0.cmp(&b.0)));
    ranked.into_iter().take(max_terms).map(|(t, _)| t).collect()
}

fn tokenize_distinctive_set(text: &str) -> HashSet<String> {
    let stops: HashSet<&'static str> = STOPWORDS.iter().copied().collect();
    let mut out = HashSet::new();
    for raw in text.split(|c: char| !c.is_alphanumeric() && c != '_') {
        let lower = raw.trim_matches('_').to_ascii_lowercase();
        if lower.len() >= 4
            && !lower.chars().all(|c| c.is_ascii_digit())
            && !stops.contains(lower.as_str())
        {
            out.insert(lower);
        }
    }
    out
}

fn clean_excerpt(text: &str, max_chars: usize) -> String {
    let normalized = text.split_whitespace().collect::<Vec<_>>().join(" ");
    if normalized.chars().count() <= max_chars {
        return normalized;
    }
    let truncated: String = normalized.chars().take(max_chars).collect();
    format!("{}…", truncated.trim_end())
}

fn filename_stem_tokens_text(name: &str) -> String {
    Path::new(name)
        .file_stem()
        .and_then(|s| s.to_str())
        .unwrap_or(name)
        .replace(['_', '-', '.'], " ")
}

fn normalized_parent_dir(path: &str) -> String {
    let trimmed = path.strip_prefix(r"\\?\").unwrap_or(path);
    match Path::new(trimmed).parent() {
        Some(p) => p.to_string_lossy().replace('\\', "/").to_ascii_lowercase(),
        None => String::new(),
    }
}

fn ensure_sensitive_allowed(include_sensitive: bool, reveal_sensitive: bool) -> Result<()> {
    if include_sensitive && !reveal_sensitive {
        return Err(AegisError::PermissionDenied(
            "Unlock sensitive previews with Windows authentication before including confidential or secret documents in the Research Librarian (R7)."
                .into(),
        ));
    }
    Ok(())
}

fn load_collections_lookup(conn: &Connection) -> Result<CollectionsLookup> {
    let mut names = HashMap::new();
    let mut st_c = conn.prepare(
        "SELECT id, name FROM collections WHERE state NOT IN ('left_as_is', 'not_a_project')",
    )?;
    for row in st_c.query_map([], |r| Ok((r.get::<_, String>(0)?, r.get::<_, String>(1)?)))? {
        let (id, name) = row?;
        names.insert(id, name);
    }

    let mut locations = Vec::new();
    let mut st_l = conn.prepare(
        "SELECT cl.collection_id, cl.volume_id, cl.root_path, COALESCE(v.case_sensitive, 0)
         FROM collection_locations cl
         JOIN collections c ON c.id = cl.collection_id
         LEFT JOIN volumes v ON v.id = cl.volume_id
         WHERE c.state NOT IN ('left_as_is', 'not_a_project')",
    )?;
    for row in st_l.query_map([], |r| {
        Ok(CollectionLoc {
            collection_id: r.get(0)?,
            volume_id: r.get(1)?,
            root_path: r.get(2)?,
            case_sensitive: r.get::<_, i64>(3)? != 0,
        })
    })? {
        locations.push(row?);
    }

    let mut members = HashMap::new();
    let mut st_m = conn.prepare(
        "SELECT cm.file_id, cm.collection_id
         FROM collection_members cm
         JOIN collections c ON c.id = cm.collection_id
         WHERE c.state NOT IN ('left_as_is', 'not_a_project')",
    )?;
    for row in st_m.query_map([], |r| Ok((r.get::<_, String>(0)?, r.get::<_, String>(1)?)))? {
        let (fid, cid) = row?;
        members.insert(fid, cid);
    }

    Ok(CollectionsLookup {
        names,
        locations,
        members,
    })
}

fn resolve_collection<'a>(
    file: &FileDbRow,
    lookup: &'a CollectionsLookup,
) -> (Option<String>, Option<&'a str>) {
    if let Some(cid) = lookup.members.get(&file.file_id) {
        return (Some(cid.clone()), lookup.names.get(cid).map(String::as_str));
    }
    let mut best: Option<(usize, &str)> = None;
    for loc in &lookup.locations {
        if loc.volume_id == file.volume_id
            && path_is_within(&file.path, &loc.root_path, loc.case_sensitive)
        {
            let len = loc.root_path.len();
            if best.is_none_or(|(bl, _)| len > bl) {
                best = Some((len, loc.collection_id.as_str()));
            }
        }
    }
    match best {
        Some((_, cid)) => (
            Some(cid.to_string()),
            lookup.names.get(cid).map(String::as_str),
        ),
        None => (None, None),
    }
}

fn load_file_row_if_allowed(conn: &Connection, file_id: &str) -> Result<Option<FileDbRow>> {
    let row = conn
        .query_row(
            "SELECT f.id, f.volume_id, v.mount_point, f.path, f.name, COALESCE(f.ext, ''),
                    f.size, f.mtime, COALESCE(NULLIF(v.label, ''), v.mount_point),
                    CASE WHEN v.status = 'online' THEN 1 ELSE 0 END, f.sensitivity
             FROM files f
             JOIN volumes v ON v.id = f.volume_id
             WHERE f.id = ?1
               AND f.deleted_at IS NULL
               AND f.state NOT IN ('quarantine', 'missing')
               AND f.container_file_id IS NULL
               AND COALESCE(v.role, '') != 'backup'",
            params![file_id],
            |r| {
                Ok(FileDbRow {
                    file_id: r.get(0)?,
                    volume_id: r.get(1)?,
                    mount_point: r.get(2)?,
                    path: r.get(3)?,
                    name: r.get(4)?,
                    ext: r.get(5)?,
                    size_bytes: r.get(6)?,
                    mtime: r.get(7)?,
                    drive_label: r.get(8)?,
                    drive_online: r.get::<_, i64>(9)? != 0,
                    sensitivity: r.get(10)?,
                })
            },
        )
        .optional()?;
    Ok(row.filter(|r| !is_excluded_relative_path(&r.path, &r.mount_point)))
}

fn load_file_chunks(conn: &Connection, file_id: &str, max_chunks: usize) -> Result<Vec<String>> {
    let mut stmt =
        conn.prepare("SELECT text FROM chunks WHERE file_id = ?1 ORDER BY seq ASC LIMIT ?2")?;
    let rows = stmt.query_map(params![file_id, max_chunks as i64], |r| {
        r.get::<_, String>(0)
    })?;
    let mut out = Vec::new();
    for r in rows {
        out.push(r?);
    }
    Ok(out)
}

fn load_file_entities(conn: &Connection, file_id: &str) -> Result<Vec<SharedEntityRef>> {
    let mut stmt = conn.prepare(
        "SELECT DISTINCT e.id, e.entity_type, e.name
         FROM relationships r
         JOIN entities e ON e.id = r.dst_id
         WHERE r.src_id = ?1
           AND r.src_kind = 'file'
           AND r.dst_kind = 'entity'
           AND r.valid_to IS NULL
         ORDER BY e.name ASC",
    )?;
    let rows = stmt.query_map(params![file_id], |r| {
        Ok(SharedEntityRef {
            entity_id: r.get(0)?,
            entity_type: r.get(1)?,
            name: r.get(2)?,
        })
    })?;
    rows.collect::<rusqlite::Result<_>>().map_err(Into::into)
}

fn dominant_vec_model(conn: &Connection) -> Result<Option<(String, i64)>> {
    conn.query_row(
        "SELECT model_id, dim
         FROM chunk_embeddings_meta
         GROUP BY model_id, dim
         ORDER BY COUNT(*) DESC, MAX(embedded_at) DESC
         LIMIT 1",
        [],
        |r| Ok((r.get(0)?, r.get(1)?)),
    )
    .optional()
    .map_err(Into::into)
}

fn min_vec_sim_for_model(model_id: &str) -> f64 {
    if model_id.contains("lexical") {
        0.15
    } else {
        0.30
    }
}

fn build_research_doc_item(
    conn: &Connection,
    row: &FileDbRow,
    lookup: &CollectionsLookup,
    entity_count: i64,
    pinned_count: i64,
    dominant_model: Option<&str>,
) -> Result<ResearchDocItem> {
    let chunk_count: i64 = conn.query_row(
        "SELECT COUNT(*) FROM chunks WHERE file_id = ?1",
        params![row.file_id],
        |r| r.get(0),
    )?;
    let has_vectors: bool = match dominant_model {
        Some(mid) => conn.query_row(
            "SELECT EXISTS(
                SELECT 1 FROM chunk_embeddings_meta WHERE file_id = ?1 AND model_id = ?2
             )",
            params![row.file_id, mid],
            |r| r.get::<_, i64>(0),
        )? != 0,
        None => conn.query_row(
            "SELECT EXISTS(SELECT 1 FROM chunk_embeddings_meta WHERE file_id = ?1)",
            params![row.file_id],
            |r| r.get::<_, i64>(0),
        )? != 0,
    };
    let first_chunk: Option<String> = conn
        .query_row(
            "SELECT text FROM chunks WHERE file_id = ?1 ORDER BY seq ASC LIMIT 1",
            params![row.file_id],
            |r| r.get(0),
        )
        .optional()?;
    let excerpt = first_chunk
        .as_deref()
        .map(|s| clean_excerpt(s, 220))
        .unwrap_or_default();
    let (collection_id, collection_name) = resolve_collection(row, lookup);

    Ok(ResearchDocItem {
        file_id: row.file_id.clone(),
        path: row.path.clone(),
        filename: row.name.clone(),
        ext: row.ext.clone(),
        size_bytes: row.size_bytes,
        mtime: row.mtime,
        drive_label: row.drive_label.clone(),
        drive_online: row.drive_online,
        collection_id,
        collection_name: collection_name.map(str::to_string),
        chunk_count,
        has_vectors,
        entity_count,
        pinned_count,
        excerpt,
        sensitivity: row.sensitivity.clone(),
    })
}

/// Lists recent indexed non-backup documents on disk (`R5`, `K2`, `K4`, `R7`) for active selection.
pub fn list_recent_research_docs(
    conn: &Connection,
    query: Option<&str>,
    collection_id: Option<&str>,
    limit: usize,
    include_sensitive: bool,
    reveal_sensitive: bool,
) -> Result<ResearchDocShelf> {
    ensure_sensitive_allowed(include_sensitive, reveal_sensitive)?;
    let allow_sensitive = include_sensitive && reveal_sensitive;
    let limit = limit.clamp(1, 200);
    let query_low = query
        .map(str::trim)
        .filter(|s| !s.is_empty())
        .map(str::to_ascii_lowercase);

    let lookup = load_collections_lookup(conn)?;
    let dominant_model = dominant_vec_model(conn)?.map(|(m, _)| m);

    let mut entity_counts: HashMap<String, i64> = HashMap::new();
    {
        let mut st = conn.prepare(
            "SELECT src_id, COUNT(DISTINCT dst_id)
             FROM relationships
             WHERE src_kind = 'file' AND dst_kind = 'entity' AND valid_to IS NULL
             GROUP BY src_id",
        )?;
        for r in st.query_map([], |row| Ok((row.get::<_, String>(0)?, row.get::<_, i64>(1)?)))? {
            let (fid, cnt) = r?;
            entity_counts.insert(fid, cnt);
        }
    }

    let mut pin_counts: HashMap<String, i64> = HashMap::new();
    {
        let mut st = conn.prepare(
            "SELECT source_file_id, COUNT(*) FROM librarian_pins GROUP BY source_file_id",
        )?;
        for r in st.query_map([], |row| Ok((row.get::<_, String>(0)?, row.get::<_, i64>(1)?)))? {
            let (fid, cnt) = r?;
            pin_counts.insert(fid, cnt);
        }
    }

    let mut stmt = conn.prepare(
        "SELECT f.id, f.volume_id, v.mount_point, f.path, f.name, COALESCE(f.ext, ''),
                f.size, f.mtime, COALESCE(NULLIF(v.label, ''), v.mount_point),
                CASE WHEN v.status = 'online' THEN 1 ELSE 0 END, f.sensitivity
         FROM files f
         JOIN volumes v ON v.id = f.volume_id
         WHERE f.deleted_at IS NULL
           AND f.state NOT IN ('quarantine', 'missing')
           AND f.container_file_id IS NULL
           AND COALESCE(v.role, '') != 'backup'
           AND EXISTS(SELECT 1 FROM chunks c WHERE c.file_id = f.id)
         ORDER BY f.mtime DESC, f.name ASC",
    )?;
    let rows = stmt.query_map([], |r| {
        Ok(FileDbRow {
            file_id: r.get(0)?,
            volume_id: r.get(1)?,
            mount_point: r.get(2)?,
            path: r.get(3)?,
            name: r.get(4)?,
            ext: r.get(5)?,
            size_bytes: r.get(6)?,
            mtime: r.get(7)?,
            drive_label: r.get(8)?,
            drive_online: r.get::<_, i64>(9)? != 0,
            sensitivity: r.get(10)?,
        })
    })?;

    let mut docs = Vec::new();
    let mut total_indexed_docs = 0usize;
    let mut embedded_docs = 0usize;
    let mut excluded_sensitive_count = 0usize;

    for row_res in rows {
        let row = row_res?;
        if is_excluded_relative_path(&row.path, &row.mount_point) {
            continue;
        }
        if is_sensitive(&row.sensitivity) && !allow_sensitive {
            excluded_sensitive_count += 1;
            continue;
        }
        let ent_cnt = entity_counts.get(&row.file_id).copied().unwrap_or(0);
        let pin_cnt = pin_counts.get(&row.file_id).copied().unwrap_or(0);
        let item = build_research_doc_item(
            conn,
            &row,
            &lookup,
            ent_cnt,
            pin_cnt,
            dominant_model.as_deref(),
        )?;

        total_indexed_docs += 1;
        if item.has_vectors {
            embedded_docs += 1;
        }

        if let Some(cid_filter) = collection_id
            && item.collection_id.as_deref() != Some(cid_filter)
        {
            continue;
        }
        if let Some(ref ql) = query_low {
            let hay = format!(
                "{} {} {} {}",
                item.filename,
                item.path,
                item.collection_name.as_deref().unwrap_or(""),
                item.excerpt
            )
            .to_ascii_lowercase();
            if !hay.contains(ql) {
                continue;
            }
        }
        if docs.len() < limit {
            docs.push(item);
        }
    }

    Ok(ResearchDocShelf {
        docs,
        total_indexed_docs,
        embedded_docs,
        excluded_sensitive_count,
    })
}

struct CollectorState<'a> {
    exclude_file_id: Option<&'a str>,
    allow_sensitive: bool,
    candidates: HashMap<String, CandidateAcc>,
    excluded_sensitive_ids: HashSet<String>,
}

impl<'a> CollectorState<'a> {
    fn new(exclude_file_id: Option<&'a str>, allow_sensitive: bool) -> Self {
        Self {
            exclude_file_id,
            allow_sensitive,
            candidates: HashMap::new(),
            excluded_sensitive_ids: HashSet::new(),
        }
    }
}

fn collect_vec_candidates(
    conn: &Connection,
    model_id: &str,
    query_vec: &[f32],
    min_sim: f64,
    state: &mut CollectorState<'_>,
) -> Result<()> {
    let raw_nn = vec_search_chunks(conn, query_vec, VEC_KNN_LIMIT)?;
    if raw_nn.is_empty() {
        return Ok(());
    }
    let mut chunk_stmt = conn.prepare(
        "SELECT f.id, v.mount_point, f.path, f.sensitivity, c.text
         FROM chunks c
         JOIN chunk_embeddings_meta cem ON cem.chunk_id = c.id
         JOIN files f ON f.id = c.file_id
         JOIN volumes v ON v.id = f.volume_id
         WHERE c.id = ?1
           AND cem.model_id = ?2
           AND f.deleted_at IS NULL
           AND f.state NOT IN ('quarantine', 'missing')
           AND f.container_file_id IS NULL
           AND COALESCE(v.role, '') != 'backup'",
    )?;
    for (chunk_id, dist) in raw_nn {
        let sim = (1.0 - dist).clamp(0.0, 1.0);
        if sim < min_sim {
            continue;
        }
        let hit: Option<(String, String, String, String, String)> = chunk_stmt
            .query_row(params![chunk_id, model_id], |r| {
                Ok((r.get(0)?, r.get(1)?, r.get(2)?, r.get(3)?, r.get(4)?))
            })
            .optional()?;
        let Some((fid, mount_point, path, sensitivity, text)) = hit else {
            continue;
        };
        if state.exclude_file_id == Some(fid.as_str())
            || is_excluded_relative_path(&path, &mount_point)
        {
            continue;
        }
        if is_sensitive(&sensitivity) && !state.allow_sensitive {
            state.excluded_sensitive_ids.insert(fid);
            continue;
        }
        let acc = state.candidates.entry(fid).or_default();
        if sim > acc.semantic_score {
            acc.semantic_score = sim;
            acc.excerpt = Some(clean_excerpt(&text, 280));
        } else if acc.excerpt.is_none() {
            acc.excerpt = Some(clean_excerpt(&text, 280));
        }
    }
    Ok(())
}

fn collect_fts_candidates(
    conn: &Connection,
    terms: &[String],
    state: &mut CollectorState<'_>,
) -> Result<()> {
    let fts_tokens: Vec<String> = terms
        .iter()
        .take(DEFAULT_MAX_TERMS)
        .filter_map(|t| {
            let cleaned: String = t
                .chars()
                .filter(|c| c.is_alphanumeric() || *c == '_')
                .collect();
            if cleaned.len() >= 3 {
                Some(format!("\"{cleaned}\""))
            } else {
                None
            }
        })
        .collect();
    if fts_tokens.is_empty() {
        return Ok(());
    }
    let fts_query = fts_tokens.join(" OR ");
    let mut stmt = conn.prepare(
        "SELECT f.id, v.mount_point, f.path, f.sensitivity, c.text, bm25(chunks_fts)
         FROM chunks_fts
         JOIN chunks c ON c.id = chunks_fts.rowid
         JOIN files f ON f.id = c.file_id
         JOIN volumes v ON v.id = f.volume_id
         WHERE chunks_fts MATCH ?1
           AND f.deleted_at IS NULL
           AND f.state NOT IN ('quarantine', 'missing')
           AND f.container_file_id IS NULL
           AND COALESCE(v.role, '') != 'backup'
         ORDER BY -bm25(chunks_fts) DESC
         LIMIT ?2",
    )?;
    let rows = stmt.query_map(params![fts_query, FTS_KNN_LIMIT as i64], |r| {
        Ok((
            r.get::<_, String>(0)?,
            r.get::<_, String>(1)?,
            r.get::<_, String>(2)?,
            r.get::<_, String>(3)?,
            r.get::<_, String>(4)?,
            r.get::<_, f64>(5)?,
        ))
    })?;
    for row in rows {
        let (fid, mount_point, path, sensitivity, text, bm25) = row?;
        if state.exclude_file_id == Some(fid.as_str())
            || is_excluded_relative_path(&path, &mount_point)
        {
            continue;
        }
        if is_sensitive(&sensitivity) && !state.allow_sensitive {
            state.excluded_sensitive_ids.insert(fid);
            continue;
        }
        let strength = (-bm25).max(0.0);
        let acc = state.candidates.entry(fid).or_default();
        acc.has_fts = true;
        if strength > acc.bm25_strength {
            acc.bm25_strength = strength;
            acc.excerpt.get_or_insert_with(|| clean_excerpt(&text, 280));
        } else if acc.excerpt.is_none() {
            acc.excerpt = Some(clean_excerpt(&text, 280));
        }
    }
    Ok(())
}

fn collect_entity_candidates(
    conn: &Connection,
    entities: &[SharedEntityRef],
    state: &mut CollectorState<'_>,
) -> Result<()> {
    if entities.is_empty() {
        return Ok(());
    }
    let mut stmt = conn.prepare(
        "SELECT f.id, v.mount_point, f.path, f.sensitivity
         FROM relationships r
         JOIN files f ON f.id = r.src_id
         JOIN volumes v ON v.id = f.volume_id
         WHERE r.dst_id = ?1
           AND r.src_kind = 'file'
           AND r.dst_kind = 'entity'
           AND r.valid_to IS NULL
           AND f.deleted_at IS NULL
           AND f.state NOT IN ('quarantine', 'missing')
           AND f.container_file_id IS NULL
           AND COALESCE(v.role, '') != 'backup'",
    )?;
    for ent in entities {
        let rows = stmt.query_map(params![ent.entity_id], |r| {
            Ok((
                r.get::<_, String>(0)?,
                r.get::<_, String>(1)?,
                r.get::<_, String>(2)?,
                r.get::<_, String>(3)?,
            ))
        })?;
        for row in rows {
            let (fid, mount_point, path, sensitivity) = row?;
            if state.exclude_file_id == Some(fid.as_str())
                || is_excluded_relative_path(&path, &mount_point)
            {
                continue;
            }
            if is_sensitive(&sensitivity) && !state.allow_sensitive {
                state.excluded_sensitive_ids.insert(fid);
                continue;
            }
            let acc = state.candidates.entry(fid).or_default();
            if acc.seen_entity_ids.insert(ent.entity_id.clone()) {
                acc.shared_entities.push(ent.clone());
            }
        }
    }
    Ok(())
}

fn load_pins_for_source(conn: &Connection, source_id: &str) -> Result<HashMap<String, PinMeta>> {
    let mut out = HashMap::new();
    let mut stmt = conn.prepare(
        "SELECT related_file_id, note FROM librarian_pins WHERE source_file_id = ?1",
    )?;
    for r in stmt.query_map(params![source_id], |row| {
        Ok((row.get::<_, String>(0)?, row.get::<_, String>(1)?))
    })? {
        let (fid, note) = r?;
        out.insert(fid, PinMeta { note });
    }
    Ok(out)
}

fn load_dismissals_for_source(conn: &Connection, source_id: &str) -> Result<HashSet<String>> {
    let mut out = HashSet::new();
    let mut stmt = conn.prepare(
        "SELECT related_file_id FROM librarian_dismissals WHERE source_file_id = ?1",
    )?;
    for r in stmt.query_map(params![source_id], |row| row.get::<_, String>(0))? {
        out.insert(r?);
    }
    Ok(out)
}

struct FinalizeInput<'a> {
    conn: &'a Connection,
    lookup: &'a CollectionsLookup,
    source_collection_id: Option<&'a str>,
    source_parent_dir: Option<&'a str>,
    source_terms: &'a [String],
    min_vec_sim: f64,
    used_vectors: bool,
    allow_sensitive: bool,
    options: &'a LibrarianOptions,
    pins: &'a HashMap<String, PinMeta>,
    dismissals: &'a HashSet<String>,
    candidates: HashMap<String, CandidateAcc>,
    excluded_sensitive_ids: HashSet<String>,
}

struct FinalizedRefs {
    references: Vec<RelatedReference>,
    total_candidates: usize,
    conceptual_count: usize,
    cross_project_count: usize,
    pinned_count: usize,
    dismissed_count: usize,
    excluded_sensitive_count: usize,
}

fn finalize_candidates(mut input: FinalizeInput<'_>) -> Result<FinalizedRefs> {
    let min_score = input.options.min_score.unwrap_or(0.18).clamp(0.0, 1.0);
    let cross_project_only = input.options.cross_project_only.unwrap_or(false);
    let semantic_only_filter = input.options.semantic_only_filter.unwrap_or(false);
    let include_dismissed = input.options.include_dismissed.unwrap_or(false);
    let limit = input.options.limit.unwrap_or(DEFAULT_MAX_REFS).clamp(1, 100);

    for pinned_fid in input.pins.keys() {
        input.candidates.entry(pinned_fid.clone()).or_default();
    }

    let mut references = Vec::new();
    let mut total_candidates = 0usize;
    let mut conceptual_count = 0usize;
    let mut cross_project_count = 0usize;
    let mut pinned_count = 0usize;
    let mut dismissed_count = 0usize;

    for (fid, mut acc) in input.candidates {
        let Some(row) = load_file_row_if_allowed(input.conn, &fid)? else {
            continue;
        };
        if is_sensitive(&row.sensitivity) && !input.allow_sensitive {
            input.excluded_sensitive_ids.insert(fid);
            continue;
        }

        let cand_chunks = load_file_chunks(input.conn, &fid, 6)?;
        let mut combined_text = filename_stem_tokens_text(&row.name);
        for ch in &cand_chunks {
            combined_text.push('\n');
            combined_text.push_str(ch);
        }
        let cand_tokens = tokenize_distinctive_set(&combined_text);

        let shared_terms: Vec<String> = input
            .source_terms
            .iter()
            .filter(|t| cand_tokens.contains(t.as_str()))
            .take(8)
            .cloned()
            .collect();

        let excerpt = acc
            .excerpt
            .take()
            .filter(|s| !s.is_empty())
            .or_else(|| cand_chunks.first().map(|c| clean_excerpt(c, 280)))
            .unwrap_or_else(|| row.name.clone());

        let lexical_score = if shared_terms.is_empty() && !acc.has_fts {
            0.0
        } else {
            let term_ratio = (shared_terms.len() as f64)
                / (input.source_terms.len().clamp(1, 4) as f64);
            let bm25_norm = if acc.has_fts {
                let s = acc.bm25_strength;
                (s / (s + 3.0)).clamp(0.20, 1.0)
            } else {
                0.0
            };
            (0.55 * term_ratio.min(1.0) + 0.45 * bm25_norm).clamp(0.0, 1.0)
        };

        let entity_score = (acc.shared_entities.len() as f64 / 3.0).min(1.0);
        let semantic_score = acc.semantic_score.clamp(0.0, 1.0);
        let semantic_only = semantic_score >= input.min_vec_sim
            && shared_terms.is_empty()
            && acc.shared_entities.is_empty();

        let (col_id, col_name) = resolve_collection(&row, input.lookup);
        let cand_parent = normalized_parent_dir(&row.path);
        let cross_project = match (input.source_collection_id, col_id.as_deref()) {
            (Some(sc), Some(cc)) => sc != cc,
            (Some(_), None) | (None, Some(_)) => true,
            (None, None) => input
                .source_parent_dir
                .is_some_and(|sp| !sp.is_empty() && sp != cand_parent),
        };

        let raw = if input.used_vectors {
            0.56 * semantic_score + 0.24 * lexical_score + 0.20 * entity_score
        } else {
            0.62 * lexical_score + 0.38 * entity_score
        };
        let base = raw
            .max(semantic_score * 0.88)
            .max(entity_score * 0.78)
            .max(lexical_score * 0.72);
        let association_score = (base + if cross_project { 0.05 } else { 0.0 }).clamp(0.0, 1.0);

        let pin_meta = input.pins.get(&fid);
        let pinned = pin_meta.is_some();
        let pin_note = pin_meta.map(|p| p.note.clone()).unwrap_or_default();
        let dismissed = input.dismissals.contains(&fid);

        if association_score < min_score && !pinned {
            continue;
        }

        total_candidates += 1;
        if semantic_only {
            conceptual_count += 1;
        }
        if cross_project {
            cross_project_count += 1;
        }
        if pinned {
            pinned_count += 1;
        }
        if dismissed {
            dismissed_count += 1;
            if !include_dismissed {
                continue;
            }
        }
        if cross_project_only && !cross_project {
            continue;
        }
        if semantic_only_filter && !semantic_only {
            continue;
        }

        let primary_reason = if semantic_only {
            AssociationReason::ConceptualParallel
        } else if !acc.shared_entities.is_empty()
            && entity_score >= semantic_score
            && entity_score >= lexical_score
        {
            AssociationReason::SharedKnowledge
        } else if semantic_score > 0.0 && lexical_score > 0.0 {
            AssociationReason::SemanticAndLexical
        } else if cross_project && (semantic_score > 0.0 || lexical_score > 0.0) {
            AssociationReason::CrossProjectOverlap
        } else if pinned {
            AssociationReason::PinnedReference
        } else {
            AssociationReason::SemanticAndLexical
        };

        let why_related = build_why_related(
            primary_reason,
            semantic_score,
            &shared_terms,
            &acc.shared_entities,
            col_name,
            cross_project,
            pinned,
        );

        references.push(RelatedReference {
            file_id: row.file_id,
            path: row.path,
            filename: row.name,
            ext: row.ext,
            size_bytes: row.size_bytes,
            mtime: row.mtime,
            drive_label: row.drive_label,
            drive_online: row.drive_online,
            collection_id: col_id,
            collection_name: col_name.map(str::to_string),
            cross_project,
            association_score,
            semantic_score,
            lexical_score,
            entity_score,
            semantic_only,
            primary_reason,
            why_related,
            shared_terms,
            shared_entities: acc.shared_entities,
            excerpt,
            pinned,
            pin_note,
            dismissed,
        });
    }

    references.sort_by(|a, b| {
        b.pinned
            .cmp(&a.pinned)
            .then_with(|| b.association_score.total_cmp(&a.association_score))
            .then_with(|| b.mtime.cmp(&a.mtime))
            .then_with(|| a.file_id.cmp(&b.file_id))
    });
    references.truncate(limit);

    Ok(FinalizedRefs {
        references,
        total_candidates,
        conceptual_count,
        cross_project_count,
        pinned_count,
        dismissed_count,
        excluded_sensitive_count: input.excluded_sensitive_ids.len(),
    })
}

fn build_why_related(
    reason: AssociationReason,
    semantic_score: f64,
    shared_terms: &[String],
    shared_entities: &[SharedEntityRef],
    collection_name: Option<&str>,
    cross_project: bool,
    pinned: bool,
) -> String {
    let mut parts = Vec::new();
    match reason {
        AssociationReason::ConceptualParallel => {
            parts.push(format!(
                "Conceptual match ({}% vector similarity) with zero shared keywords",
                (semantic_score * 100.0).round() as i64
            ));
        }
        AssociationReason::SharedKnowledge => {
            let names: Vec<&str> = shared_entities
                .iter()
                .take(3)
                .map(|e| e.name.as_str())
                .collect();
            parts.push(format!("Shares approved knowledge: {}", names.join(", ")));
        }
        AssociationReason::SemanticAndLexical => {
            if semantic_score > 0.0 && !shared_terms.is_empty() {
                parts.push(format!(
                    "{}% semantic match & {} shared concept(s)",
                    (semantic_score * 100.0).round() as i64,
                    shared_terms.len()
                ));
            } else if semantic_score > 0.0 {
                parts.push(format!(
                    "{}% semantic vector proximity",
                    (semantic_score * 100.0).round() as i64
                ));
            } else {
                parts.push(format!(
                    "Matches {} key concept(s): {}",
                    shared_terms.len(),
                    shared_terms
                        .iter()
                        .take(3)
                        .cloned()
                        .collect::<Vec<_>>()
                        .join(", ")
                ));
            }
        }
        AssociationReason::CrossProjectOverlap => {
            parts.push("Overlapping topic from another project or folder".to_string());
        }
        AssociationReason::PinnedReference => {
            parts.push("Pinned to reading list".to_string());
        }
    }

    if cross_project {
        if let Some(col) = collection_name {
            parts.push(format!("in project \"{col}\""));
        } else {
            parts.push("in a separate folder".to_string());
        }
    }
    if pinned && reason != AssociationReason::PinnedReference {
        parts.push("pinned to reading list".to_string());
    }
    parts.join(" · ")
}

/// Discovers cross-referenced documents for a saved file on disk (`R5`, `K2`, `K4`, `R7`).
pub fn find_related_for_file(
    conn: &Connection,
    source_file_id: &str,
    options: &LibrarianOptions,
    reveal_sensitive: bool,
) -> Result<ResearchCompanionReport> {
    let started = Instant::now();
    let include_sensitive = options.include_sensitive.unwrap_or(false);
    ensure_sensitive_allowed(include_sensitive, reveal_sensitive)?;
    let allow_sensitive = include_sensitive && reveal_sensitive;

    let Some(source_row) = load_file_row_if_allowed(conn, source_file_id)? else {
        return Err(AegisError::NotFound(format!("file {source_file_id}")));
    };
    if is_sensitive(&source_row.sensitivity) && !allow_sensitive {
        return Err(AegisError::PermissionDenied(
            "Unlock sensitive previews with Windows authentication before researching confidential or secret files (R7)."
                .into(),
        ));
    }

    let lookup = load_collections_lookup(conn)?;
    let source_entities = load_file_entities(conn, source_file_id)?;
    let pins = load_pins_for_source(conn, source_file_id)?;
    let dismissals = load_dismissals_for_source(conn, source_file_id)?;
    let dominant = dominant_vec_model(conn)?;
    let source_item = build_research_doc_item(
        conn,
        &source_row,
        &lookup,
        source_entities.len() as i64,
        pins.len() as i64,
        dominant.as_ref().map(|(m, _)| m.as_str()),
    )?;

    let chunks = load_file_chunks(conn, source_file_id, 12)?;
    let mut source_text = filename_stem_tokens_text(&source_row.name);
    for ch in &chunks {
        source_text.push('\n');
        source_text.push_str(ch);
    }
    let extracted_terms = extract_distinctive_terms(&source_text, DEFAULT_MAX_TERMS);

    let mut collector = CollectorState::new(Some(source_file_id), allow_sensitive);

    // Signal 1: Dense semantic similarity via encrypted `vec_chunks`.
    let mut used_vectors = false;
    let mut min_vec_sim = 0.30;
    if let Some((ref model_id, _dim)) = dominant {
        min_vec_sim = min_vec_sim_for_model(model_id);
        let source_id_owned = source_file_id.to_string();
        let vecs = load_file_vectors(conn, std::slice::from_ref(&source_id_owned))?;
        if let Some(src_vec) = vecs.get(source_file_id) {
            used_vectors = true;
            collect_vec_candidates(conn, model_id, src_vec, min_vec_sim, &mut collector)?;
        }
    }

    // Signal 2: Sparse FTS5 lexical concepts.
    collect_fts_candidates(conn, &extracted_terms, &mut collector)?;

    // Signal 3: Shared approved Knowledge Graph entities (Capability #8 synergy).
    collect_entity_candidates(conn, &source_entities, &mut collector)?;

    let source_parent = normalized_parent_dir(&source_row.path);
    let finalized = finalize_candidates(FinalizeInput {
        conn,
        lookup: &lookup,
        source_collection_id: source_item.collection_id.as_deref(),
        source_parent_dir: Some(&source_parent),
        source_terms: &extracted_terms,
        min_vec_sim,
        used_vectors,
        allow_sensitive,
        options,
        pins: &pins,
        dismissals: &dismissals,
        candidates: collector.candidates,
        excluded_sensitive_ids: collector.excluded_sensitive_ids,
    })?;

    Ok(ResearchCompanionReport {
        mode: "file".into(),
        source_doc: Some(source_item),
        topic_query: None,
        extracted_terms,
        source_entities,
        references: finalized.references,
        total_candidates: finalized.total_candidates,
        conceptual_count: finalized.conceptual_count,
        cross_project_count: finalized.cross_project_count,
        pinned_count: finalized.pinned_count,
        dismissed_count: finalized.dismissed_count,
        excluded_sensitive_count: finalized.excluded_sensitive_count,
        used_vectors,
        took_ms: started.elapsed().as_millis() as u64,
    })
}

fn match_entities_in_topic(conn: &Connection, topic: &str) -> Result<Vec<SharedEntityRef>> {
    let topic_low = topic.to_ascii_lowercase();
    let mut matched = Vec::new();
    let mut seen = HashSet::new();

    let mut st = conn.prepare("SELECT id, entity_type, name FROM entities ORDER BY name ASC")?;
    for r in st.query_map([], |row| {
        Ok(SharedEntityRef {
            entity_id: row.get(0)?,
            entity_type: row.get(1)?,
            name: row.get(2)?,
        })
    })? {
        let ent = r?;
        let needle = ent.name.trim().to_ascii_lowercase();
        if needle.len() >= 3 && topic_low.contains(&needle) && seen.insert(ent.entity_id.clone()) {
            matched.push(ent);
        }
    }

    let mut st_a = conn.prepare(
        "SELECT DISTINCT e.id, e.entity_type, e.name, ea.alias_text
         FROM entity_aliases ea
         JOIN entities e ON e.id = ea.entity_id",
    )?;
    for r in st_a.query_map([], |row| {
        Ok((
            row.get::<_, String>(0)?,
            row.get::<_, String>(1)?,
            row.get::<_, String>(2)?,
            row.get::<_, String>(3)?,
        ))
    })? {
        let (entity_id, entity_type, name, alias_text) = r?;
        let needle = alias_text.trim().to_ascii_lowercase();
        if needle.len() >= 3 && topic_low.contains(&needle) && seen.insert(entity_id.clone()) {
            matched.push(SharedEntityRef {
                entity_id,
                entity_type,
                name,
            });
        }
    }

    Ok(matched)
}

/// Discovers cross-referenced documents for an explicit research topic or pasted excerpt (`R5`).
pub fn find_related_for_topic(
    conn: &Connection,
    topic: &str,
    query_vec: Option<(&str, &[f32], f64)>,
    options: &LibrarianOptions,
    reveal_sensitive: bool,
) -> Result<ResearchCompanionReport> {
    let started = Instant::now();
    let trimmed = topic.trim();
    if trimmed.is_empty() {
        return Err(AegisError::InvalidInput(
            "Enter a research topic or excerpt to search cross-references.".into(),
        ));
    }

    let include_sensitive = options.include_sensitive.unwrap_or(false);
    ensure_sensitive_allowed(include_sensitive, reveal_sensitive)?;
    let allow_sensitive = include_sensitive && reveal_sensitive;

    let lookup = load_collections_lookup(conn)?;
    let extracted_terms = extract_distinctive_terms(trimmed, DEFAULT_MAX_TERMS);
    let source_entities = match_entities_in_topic(conn, trimmed)?;

    let mut collector = CollectorState::new(None, allow_sensitive);

    let mut used_vectors = false;
    let mut min_vec_sim = 0.30;
    if let Some((model_id, qvec, sim_floor)) = query_vec {
        used_vectors = true;
        min_vec_sim = sim_floor;
        collect_vec_candidates(conn, model_id, qvec, min_vec_sim, &mut collector)?;
    }

    collect_fts_candidates(conn, &extracted_terms, &mut collector)?;

    collect_entity_candidates(conn, &source_entities, &mut collector)?;

    let empty_pins = HashMap::new();
    let empty_dismissals = HashSet::new();
    let finalized = finalize_candidates(FinalizeInput {
        conn,
        lookup: &lookup,
        source_collection_id: None,
        source_parent_dir: None,
        source_terms: &extracted_terms,
        min_vec_sim,
        used_vectors,
        allow_sensitive,
        options,
        pins: &empty_pins,
        dismissals: &empty_dismissals,
        candidates: collector.candidates,
        excluded_sensitive_ids: collector.excluded_sensitive_ids,
    })?;

    Ok(ResearchCompanionReport {
        mode: "topic".into(),
        source_doc: None,
        topic_query: Some(trimmed.to_string()),
        extracted_terms,
        source_entities,
        references: finalized.references,
        total_candidates: finalized.total_candidates,
        conceptual_count: finalized.conceptual_count,
        cross_project_count: finalized.cross_project_count,
        pinned_count: 0,
        dismissed_count: finalized.dismissed_count,
        excluded_sensitive_count: finalized.excluded_sensitive_count,
        used_vectors,
        took_ms: started.elapsed().as_millis() as u64,
    })
}

/// Pins `related_file_id` onto the reading list for `source_file_id` inside SQLCipher `aegis.db` (`D1`).
pub fn pin_reference(
    conn: &Connection,
    source_file_id: &str,
    related_file_id: &str,
    note: Option<&str>,
) -> Result<bool> {
    if source_file_id.trim().is_empty()
        || related_file_id.trim().is_empty()
        || source_file_id == related_file_id
    {
        return Err(AegisError::InvalidInput(
            "Source and related file IDs must be distinct non-empty values.".into(),
        ));
    }
    let now = now_ms();
    let note_clean = note.unwrap_or("").trim();
    conn.execute(
        "INSERT INTO librarian_pins (source_file_id, related_file_id, note, pinned_at)
         VALUES (?1, ?2, ?3, ?4)
         ON CONFLICT(source_file_id, related_file_id) DO UPDATE SET
           note = excluded.note,
           pinned_at = excluded.pinned_at",
        params![source_file_id, related_file_id, note_clean, now],
    )?;
    conn.execute(
        "DELETE FROM librarian_dismissals WHERE source_file_id = ?1 AND related_file_id = ?2",
        params![source_file_id, related_file_id],
    )?;
    Ok(true)
}

/// Removes `related_file_id` from the reading list for `source_file_id`.
pub fn unpin_reference(
    conn: &Connection,
    source_file_id: &str,
    related_file_id: &str,
) -> Result<bool> {
    let changed = conn.execute(
        "DELETE FROM librarian_pins WHERE source_file_id = ?1 AND related_file_id = ?2",
        params![source_file_id, related_file_id],
    )?;
    Ok(changed > 0)
}

/// Persistently dismisses a noisy cross-reference suggestion for `source_file_id` (`D1`).
pub fn dismiss_reference(
    conn: &Connection,
    source_file_id: &str,
    related_file_id: &str,
) -> Result<bool> {
    if source_file_id.trim().is_empty() || related_file_id.trim().is_empty() {
        return Err(AegisError::InvalidInput(
            "Source and related file IDs must be non-empty.".into(),
        ));
    }
    let now = now_ms();
    conn.execute(
        "INSERT INTO librarian_dismissals (source_file_id, related_file_id, dismissed_at)
         VALUES (?1, ?2, ?3)
         ON CONFLICT(source_file_id, related_file_id) DO UPDATE SET
           dismissed_at = excluded.dismissed_at",
        params![source_file_id, related_file_id, now],
    )?;
    conn.execute(
        "DELETE FROM librarian_pins WHERE source_file_id = ?1 AND related_file_id = ?2",
        params![source_file_id, related_file_id],
    )?;
    Ok(true)
}

/// Restores a previously dismissed cross-reference suggestion.
pub fn undismiss_reference(
    conn: &Connection,
    source_file_id: &str,
    related_file_id: &str,
) -> Result<bool> {
    let changed = conn.execute(
        "DELETE FROM librarian_dismissals WHERE source_file_id = ?1 AND related_file_id = ?2",
        params![source_file_id, related_file_id],
    )?;
    Ok(changed > 0)
}

/// Prepares a strictly grounded prompt + `[1]..[N]` citations for `OnDemandLlm` reading-brief synthesis (`K2`, `K4`, `K5`, `R7`).
pub fn prepare_brief_context(
    conn: &Connection,
    source_file_id: Option<&str>,
    topic: Option<&str>,
    related_file_ids: &[String],
    include_sensitive: bool,
    reveal_sensitive: bool,
) -> Result<PreparedBriefContext> {
    ensure_sensitive_allowed(include_sensitive, reveal_sensitive)?;
    let allow_sensitive = include_sensitive && reveal_sensitive;
    let lookup = load_collections_lookup(conn)?;

    let (title, focus_block) = if let Some(fid) = source_file_id.filter(|s| !s.trim().is_empty()) {
        let Some(row) = load_file_row_if_allowed(conn, fid)? else {
            return Err(AegisError::NotFound(format!("file {fid}")));
        };
        if is_sensitive(&row.sensitivity) && !allow_sensitive {
            return Err(AegisError::PermissionDenied(
                "Unlock sensitive previews with Windows authentication before synthesizing a brief from confidential files (R7)."
                    .into(),
            ));
        }
        let chunks = load_file_chunks(conn, fid, 3)?;
        let excerpt = clean_excerpt(&chunks.join(" "), 500);
        (
            format!("Reading Brief — {}", row.name),
            format!("Active document: {}\nExcerpt: {}", row.name, excerpt),
        )
    } else if let Some(t) = topic.map(str::trim).filter(|s| !s.is_empty()) {
        (
            format!("Reading Brief — {}", clean_excerpt(t, 60)),
            format!("Research focus / excerpt:\n{}", clean_excerpt(t, 500)),
        )
    } else {
        return Err(AegisError::InvalidInput(
            "Provide either a source document or a research topic to synthesize a reading brief."
                .into(),
        ));
    };

    let mut citations = Vec::new();
    let mut context_blocks = Vec::new();
    let mut seen = HashSet::new();

    for fid in related_file_ids.iter().take(6) {
        if source_file_id == Some(fid.as_str()) || !seen.insert(fid.clone()) {
            continue;
        }
        let Some(row) = load_file_row_if_allowed(conn, fid)? else {
            continue;
        };
        if is_sensitive(&row.sensitivity) && !allow_sensitive {
            return Err(AegisError::PermissionDenied(
                "Unlock sensitive previews with Windows authentication before synthesizing a brief that includes confidential references (R7)."
                    .into(),
            ));
        }
        let chunks = load_file_chunks(conn, fid, 3)?;
        let excerpt = if chunks.is_empty() {
            format!("Indexed file {} ({})", row.name, row.path)
        } else {
            clean_excerpt(&chunks.join(" "), 450)
        };
        let (_, col_name) = resolve_collection(&row, &lookup);
        let ref_id = citations.len() + 1;
        context_blocks.push(format!(
            "[{}] {} ({}): {}",
            ref_id,
            row.name,
            col_name.unwrap_or(&row.drive_label),
            excerpt
        ));
        citations.push(BriefCitation {
            ref_id,
            file_id: row.file_id,
            filename: row.name,
            path: row.path,
            collection_name: col_name.map(str::to_string),
            excerpt,
        });
    }

    if citations.is_empty() {
        return Err(AegisError::InvalidInput(
            "No eligible indexed cross-references were available for a reading brief.".into(),
        ));
    }

    let system_prompt = "You are Aegis Guardian's offline Autonomous Research Librarian. \
         Synthesize a concise reading brief explaining how the active research focus connects to \
         the numbered local references ([1], [2], ...). Ground every claim exclusively in the \
         provided excerpts, highlight conceptual parallels across projects, and cite [N] inline."
        .to_string();

    let user_prompt = format!(
        "{focus_block}\n\nRelated local references:\n{}\n\n\
         Write a concise 3-5 sentence synthesis highlighting key connections, shared themes, and \
         recommended reading order with inline [N] citations.",
        context_blocks.join("\n\n")
    );

    Ok(PreparedBriefContext {
        title,
        system_prompt,
        user_prompt,
        citations,
    })
}

#[cfg(test)]
mod tests {
    use super::extract_distinctive_terms;

    #[test]
    fn extract_distinctive_terms_filters_stopwords_and_ranks() {
        let text = "The telemetry pipeline should track telemetry metrics and zero-trust hardware isolation \
                    while telemetry benchmarks validate throughput.";
        let terms = extract_distinctive_terms(text, 6);
        assert_eq!(terms[0], "telemetry");
        assert!(terms.contains(&"benchmarks".to_string()));
        assert!(!terms.contains(&"should".to_string()));
        assert!(!terms.contains(&"while".to_string()));
    }
}
