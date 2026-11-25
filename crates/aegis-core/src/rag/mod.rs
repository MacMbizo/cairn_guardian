//! Read-only Local RAG context retrieval and grounded prompt builder (`K1`, `K2`, `R7`, `D1`).

use std::collections::{BTreeSet, HashSet};

use rusqlite::{Connection, params};
use serde::{Deserialize, Serialize};
use ts_rs::TS;

use crate::contracts::{SearchFilters, SearchMode};
use crate::db::files::is_sensitive;
use crate::db::search::search_hybrid;
use crate::error::{AegisError, Result};

const DEFAULT_TOP_K: usize = 6;
const MAX_TOP_K: usize = 12;
const MAX_SNIPPET_CHARS: usize = 700;
const MAX_TOTAL_CONTEXT_CHARS: usize = 3600;

const QUESTION_STOPWORDS: &[&str] = &[
    "a", "about", "an", "and", "are", "as", "at", "be", "by", "can", "could", "did", "do",
    "does", "document", "documents", "file", "files", "find", "for", "from", "had", "has",
    "have", "how", "i", "in", "into", "is", "it", "its", "me", "mention", "mentions", "my",
    "of", "on", "or", "our", "say", "says", "show", "should", "stored", "tell", "that", "the",
    "their", "there", "these", "this", "those", "to", "was", "were", "what", "when", "where",
    "which", "who", "why", "will", "with", "would", "you", "your",
];

/// Metadata describing an offline chat LLM discovered on disk or built-in extractive fallback.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, TS)]
#[ts(export)]
pub struct LlmModelInfo {
    pub id: String,
    pub name: String,
    pub path: String,
    pub backend: String,
    pub architecture: String,
    #[ts(type = "number")]
    pub context_length: usize,
    #[ts(type = "number")]
    pub size_bytes: u64,
    pub source: String,
    pub recommended: bool,
}

/// Live local RAG chat model status surfaced to `Ask Files` and `EngineSettings` (`R4`, `K5`).
#[derive(Debug, Clone, Serialize, Deserialize, TS)]
#[ts(export)]
pub struct LlmStatus {
    pub model_id: String,
    pub model_name: String,
    pub backend: String,
    pub architecture: String,
    #[ts(type = "number")]
    pub context_length: usize,
    pub loaded: bool,
    pub selected_path: Option<String>,
    pub available_models: Vec<LlmModelInfo>,
    pub on_demand_policy: String,
}

/// Numbered source citation backing an answer in `Ask Files`.
#[derive(Debug, Clone, Serialize, Deserialize, TS)]
#[ts(export)]
pub struct RagCitation {
    #[ts(type = "number")]
    pub ref_id: usize,
    pub file_id: String,
    pub path: String,
    pub filename: String,
    pub snippet: String,
    pub score: f64,
    pub sensitivity: String,
    pub matched_by: String,
}

/// Options accepted by `rag_ask` (`K2`, `R7`).
#[derive(Debug, Clone, Default, Serialize, Deserialize, TS)]
#[ts(export)]
pub struct RagAskOptions {
    #[ts(type = "number | null")]
    pub top_k: Option<usize>,
    pub filters: Option<SearchFilters>,
    pub include_sensitive: Option<bool>,
    #[ts(type = "number | null")]
    pub max_tokens: Option<usize>,
    pub request_id: Option<String>,
}

/// Final grounded RAG answer with citations, privacy stats, and runtime metrics.
#[derive(Debug, Clone, Serialize, Deserialize, TS)]
#[ts(export)]
pub struct RagAnswer {
    pub request_id: String,
    pub question: String,
    pub answer: String,
    pub citations: Vec<RagCitation>,
    #[ts(type = "number")]
    pub excluded_sensitive_count: usize,
    pub model_id: String,
    pub model_name: String,
    pub backend: String,
    #[ts(type = "number")]
    pub prompt_tokens: usize,
    #[ts(type = "number")]
    pub tokens_generated: usize,
    #[ts(type = "number")]
    pub duration_ms: u64,
    pub cancelled: bool,
}

/// Grounded prompt and citations assembled purely from the encrypted local index (`K1`, `K2`, `R7`).
#[derive(Debug, Clone)]
pub struct RagContext {
    pub system_prompt: String,
    pub user_prompt: String,
    pub citations: Vec<RagCitation>,
    pub excluded_sensitive_count: usize,
}

/// Retrieves hybrid search context over `conn`, enforcing `K2` (no quarantined/deleted files)
/// and `R7` (sensitive files excluded unless explicitly requested AND Windows-unlocked).
pub fn retrieve_rag_context(
    conn: &Connection,
    question: &str,
    query_vec: Option<(&str, &[f32], f64)>,
    options: &RagAskOptions,
    sensitive_unlocked: bool,
) -> Result<RagContext> {
    let trimmed_q = question.trim();
    if trimmed_q.is_empty() {
        return Err(AegisError::InvalidInput(
            "Ask Files question must not be empty.".into(),
        ));
    }

    let allow_sensitive = options.include_sensitive.unwrap_or(false);
    if allow_sensitive && !sensitive_unlocked {
        return Err(AegisError::PermissionDenied(
            "Unlock sensitive previews with Windows authentication before including sensitive files in Ask Files."
                .into(),
        ));
    }

    let mut filters = options.filters.clone().unwrap_or_default();
    // K2: Never surface deleted or quarantined files in RAG context.
    filters.include_quarantined = Some(false);
    if filters.mode.is_none() {
        filters.mode = Some(SearchMode::Hybrid);
    }

    if !allow_sensitive
        && filters
            .sensitivity
            .as_deref()
            .is_some_and(is_sensitive)
    {
        return Err(AegisError::PermissionDenied(
            "Cannot filter Ask Files to sensitive files while sensitive previews are locked.".into(),
        ));
    }

    let top_k = options.top_k.unwrap_or(DEFAULT_TOP_K).clamp(1, MAX_TOP_K);
    let candidate_limit = (top_k * 3).clamp(18, 60);
    let query_terms = tokenize_query_terms(trimmed_q);
    let keyword_q = if query_terms.is_empty() {
        trimmed_q.to_string()
    } else {
        query_terms.join(" ")
    };

    let mut hits = search_hybrid(
        conn,
        &keyword_q,
        query_vec,
        &filters,
        candidate_limit,
        allow_sensitive,
    )?
    .hits;

    // For natural-language questions when strict FTS AND across all extracted keywords yields no hits,
    // widen across individual content keywords via `search_hybrid` (preserving K2/R7 and hybrid vectors).
    if hits.is_empty() && query_terms.len() > 1 {
        let mut by_file: std::collections::HashMap<String, (usize, crate::contracts::SearchHit)> =
            std::collections::HashMap::new();
        for term in query_terms.iter().take(6) {
            let page = search_hybrid(conn, term, query_vec, &filters, candidate_limit, allow_sensitive)?;
            for hit in page.hits {
                by_file
                    .entry(hit.file_id.clone())
                    .and_modify(|(cnt, best)| {
                        *cnt += 1;
                        if hit.score > best.score {
                            *best = hit.clone();
                        }
                    })
                    .or_insert((1, hit));
            }
        }
        let mut combined: Vec<(usize, crate::contracts::SearchHit)> = by_file.into_values().collect();
        combined.sort_by(|a, b| {
            b.0.cmp(&a.0)
                .then_with(|| b.1.score.total_cmp(&a.1.score))
                .then_with(|| a.1.name.cmp(&b.1.name))
        });
        hits = combined.into_iter().map(|(_, h)| h).collect();
    }

    let mut excluded_sensitive_count = 0usize;
    let mut seen_files = HashSet::new();
    let mut citations = Vec::new();
    let mut total_context_chars = 0usize;

    for hit in hits {
        if (!allow_sensitive && is_sensitive(&hit.sensitivity))
            || crate::governance::is_path_excluded_from_ai(conn, &hit.path)
        {
            excluded_sensitive_count += 1;
            continue;
        }
        if !seen_files.insert(hit.file_id.clone()) {
            continue;
        }
        if citations.len() >= top_k {
            continue;
        }

        let chunk_excerpt = best_chunk_for_file(conn, &hit.file_id, &query_terms)?
            .filter(|s| !s.trim().is_empty())
            .unwrap_or_else(|| {
                if hit.snippet.trim().is_empty() {
                    hit.name.clone()
                } else {
                    hit.snippet.clone()
                }
            });

        let snippet = truncate_clean(&chunk_excerpt, MAX_SNIPPET_CHARS);
        if !citations.is_empty() && total_context_chars + snippet.len() > MAX_TOTAL_CONTEXT_CHARS {
            break;
        }
        total_context_chars += snippet.len();

        let ref_id = citations.len() + 1;
        citations.push(RagCitation {
            ref_id,
            file_id: hit.file_id,
            path: hit.path,
            filename: hit.name,
            snippet,
            score: hit.score,
            sensitivity: hit.sensitivity,
            matched_by: hit.match_reason,
        });
    }

    // Also count any matching sensitive files without loading their chunk contents (`R7`).
    if !allow_sensitive {
        let extra = count_excluded_sensitive_matches(conn, &query_terms, &seen_files)?;
        excluded_sensitive_count = excluded_sensitive_count.max(extra);
    }

    let system_prompt = "You are Aegis Guardian's 100% offline local file assistant. Answer strictly and concisely using only the numbered local file excerpts below, citing sources inline like [1], [2]. If the excerpts do not contain enough information, state that clearly without guessing.".to_string();

    let mut context_blocks = String::new();
    for c in &citations {
        context_blocks.push_str(&format!(
            "[{}] {} ({})\n{}\n\n",
            c.ref_id, c.filename, c.path, c.snippet
        ));
    }
    let user_prompt = if citations.is_empty() {
        format!("Context:\n(No matching non-sensitive local file excerpts found.)\n\nQuestion: {trimmed_q}")
    } else {
        format!("Context:\n{context_blocks}Question: {trimmed_q}")
    };

    Ok(RagContext {
        system_prompt,
        user_prompt,
        citations,
        excluded_sensitive_count,
    })
}

fn best_chunk_for_file(
    conn: &Connection,
    file_id: &str,
    query_terms: &[String],
) -> Result<Option<String>> {
    let mut stmt =
        conn.prepare("SELECT text FROM chunks WHERE file_id = ?1 ORDER BY seq ASC LIMIT 12")?;
    let rows = stmt.query_map(params![file_id], |r| r.get::<_, String>(0))?;

    let mut best: Option<(usize, String)> = None;
    for row in rows {
        let txt = row?;
        let lower = txt.to_ascii_lowercase();
        let score = query_terms
            .iter()
            .filter(|t| lower.contains(t.as_str()))
            .count();
        match &best {
            None => best = Some((score, txt)),
            Some((best_score, _)) if score > *best_score => best = Some((score, txt)),
            _ => {}
        }
    }
    Ok(best.map(|(_, s)| s))
}

fn count_excluded_sensitive_matches(
    conn: &Connection,
    query_terms: &[String],
    already_seen_non_sensitive: &HashSet<String>,
) -> Result<usize> {
    if query_terms.is_empty() {
        return Ok(0);
    }
    // Check path or FTS5/chunk match purely inside SQLite returning only `file_id` (no chunk text loaded).
    let mut matched_ids = HashSet::new();
    let fts_query = query_terms
        .iter()
        .take(6)
        .map(|t| format!("\"{}\"", t.replace('"', "")))
        .collect::<Vec<_>>()
        .join(" OR ");

    if !fts_query.is_empty()
        && let Ok(mut stmt) = conn.prepare(
            "SELECT DISTINCT f.id
             FROM chunks_fts
             JOIN chunks c ON c.id = chunks_fts.rowid
             JOIN files f ON f.id = c.file_id
             WHERE chunks_fts MATCH ?1
               AND f.deleted_at IS NULL
               AND f.state NOT IN ('quarantine', 'missing')
               AND LOWER(f.sensitivity) IN ('confidential', 'secret')",
        )
        && let Ok(rows) = stmt.query_map(params![fts_query], |r| r.get::<_, String>(0))
    {
        for row in rows.flatten() {
            if !already_seen_non_sensitive.contains(&row) {
                matched_ids.insert(row);
            }
        }
    }

    let mut path_stmt = conn.prepare(
        "SELECT id, LOWER(path)
         FROM files
         WHERE deleted_at IS NULL
           AND state NOT IN ('quarantine', 'missing')
           AND LOWER(sensitivity) IN ('confidential', 'secret')",
    )?;
    for row in path_stmt.query_map([], |r| Ok((r.get::<_, String>(0)?, r.get::<_, String>(1)?)))? {
        let (id, lower_path) = row?;
        if !already_seen_non_sensitive.contains(&id)
            && query_terms.iter().any(|t| lower_path.contains(t.as_str()))
        {
            matched_ids.insert(id);
        }
    }

    Ok(matched_ids.len())
}

fn tokenize_query_terms(q: &str) -> Vec<String> {
    let mut seen = BTreeSet::new();
    let mut out = Vec::new();
    for raw in q.split(|c: char| !c.is_alphanumeric() && c != '_' && c != '-') {
        let lower = raw.trim().to_ascii_lowercase();
        if lower.len() >= 2
            && !QUESTION_STOPWORDS.contains(&lower.as_str())
            && seen.insert(lower.clone())
        {
            out.push(lower);
        }
    }
    out
}

fn truncate_clean(s: &str, max_chars: usize) -> String {
    let normalized: String = s.split_whitespace().collect::<Vec<_>>().join(" ");
    let mut out = String::new();
    for (count, ch) in normalized.chars().enumerate() {
        if count >= max_chars {
            out.push('…');
            break;
        }
        out.push(ch);
    }
    out
}
