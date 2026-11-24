//! Deterministic side-by-side / unified text & metadata diffing, conceptual change badges,
//! `R7` sensitivity gating, and encrypted `version_diff_summaries` cache (`3.4`, `K5`, `D1`).

use std::collections::{BTreeSet, HashMap, HashSet};
use std::path::Path;

use rusqlite::{Connection, OptionalExtension, params};

use crate::db::files::is_sensitive;
use crate::db::vec::{cosine_similarity, load_file_vectors};
use crate::engine::bytes_text;
use crate::error::{AegisError, Result};
use crate::now_ms;

use super::{DiffHunk, DiffLine, DiffLineKind, VersionDiffReport, VersionDiffSummary};

const MAX_DIFF_LINES: usize = 1_200;
const MAX_LCS_CELLS: usize = 360_000;
const CONTEXT_LINES: usize = 3;
const MAX_PLAIN_READ_BYTES: i64 = 2_000_000;

const DIFF_STOPWORDS: &[&str] = &[
    "about", "after", "also", "because", "before", "between", "could", "draft", "every", "first",
    "from", "have", "into", "more", "other", "should", "since", "some", "such", "than", "that",
    "their", "there", "these", "this", "those", "through", "under", "using", "version", "versions",
    "were", "what", "when", "where", "which", "while", "with", "would",
];

struct DiffFileRow {
    file_id: String,
    path: String,
    filename: String,
    ext: String,
    size_bytes: i64,
    mtime: i64,
    blake3: Option<Vec<u8>>,
    sensitivity: String,
    cloud_state: String,
    drive_online: bool,
    width: Option<i64>,
    height: Option<i64>,
    dominant_color: Option<String>,
}

#[derive(Clone, Copy)]
enum EditOp {
    Equal(usize, usize),
    Remove(usize),
    Add(usize),
}

fn is_plain_text_ext(ext: &str) -> bool {
    matches!(
        ext.trim().trim_start_matches('.').to_ascii_lowercase().as_str(),
        "txt"
            | "md"
            | "markdown"
            | "json"
            | "toml"
            | "yaml"
            | "yml"
            | "csv"
            | "tsv"
            | "rs"
            | "ts"
            | "tsx"
            | "js"
            | "jsx"
            | "py"
            | "html"
            | "htm"
            | "css"
            | "scss"
            | "xml"
            | "sql"
            | "ini"
            | "cfg"
            | "conf"
            | "log"
            | "sh"
            | "ps1"
            | "bat"
            | "svg"
    )
}

fn load_diff_file(conn: &Connection, file_id: &str) -> Result<DiffFileRow> {
    struct LoadedDiffRow {
        file: DiffFileRow,
        state: String,
        role: Option<String>,
    }

    let row: Option<LoadedDiffRow> = conn
        .query_row(
            "SELECT f.id, f.path, f.name, COALESCE(f.ext, ''), f.size, f.mtime, f.blake3,
                    f.sensitivity, f.cloud_state, f.state, v.role, COALESCE(v.status = 'online', 0),
                    am.width, am.height, am.dominant_color
             FROM files f
             JOIN volumes v ON v.id = f.volume_id
             LEFT JOIN asset_meta am ON am.file_id = f.id
             WHERE f.id = ?1 AND f.deleted_at IS NULL",
            params![file_id],
            |r| {
                let ext_opt: Option<String> = r.get(3)?;
                Ok(LoadedDiffRow {
                    file: DiffFileRow {
                        file_id: r.get(0)?,
                        path: r.get(1)?,
                        filename: r.get(2)?,
                        ext: ext_opt.unwrap_or_default(),
                        size_bytes: r.get(4)?,
                        mtime: r.get(5)?,
                        blake3: r.get(6)?,
                        sensitivity: r.get(7)?,
                        cloud_state: r.get(8)?,
                        drive_online: r.get::<_, i64>(11)? != 0,
                        width: r.get(12)?,
                        height: r.get(13)?,
                        dominant_color: r.get(14)?,
                    },
                    state: r.get(9)?,
                    role: r.get(10)?,
                })
            },
        )
        .optional()?;

    let Some(LoadedDiffRow { file, state, role }) = row else {
        return Err(AegisError::NotFound(format!("file {file_id}")));
    };

    if state == "quarantine" || state == "missing" {
        return Err(AegisError::InvalidInput(format!(
            "Cannot compare `{}` because it is in quarantine or missing (K2).",
            file.filename
        )));
    }
    if role.as_deref() == Some("backup") {
        return Err(AegisError::PermissionDenied(
            "Backup drives are read-only and never inspected directly (K4).".into(),
        ));
    }

    Ok(file)
}

/// Returns true if either `older_file_id` or `newer_file_id` is marked `confidential` or `secret`.
pub fn is_pair_sensitive(conn: &Connection, older_file_id: &str, newer_file_id: &str) -> Result<bool> {
    let o = load_diff_file(conn, older_file_id)?;
    let n = load_diff_file(conn, newer_file_id)?;
    Ok(is_sensitive(&o.sensitivity) || is_sensitive(&n.sensitivity))
}

fn extract_file_text(conn: &Connection, f: &DiffFileRow) -> Result<Option<String>> {
    let mut st = conn.prepare("SELECT text FROM chunks WHERE file_id = ?1 ORDER BY seq ASC")?;
    let chunk_rows = st.query_map(params![f.file_id], |r| r.get::<_, String>(0))?;
    let mut parts = Vec::new();
    for r in chunk_rows {
        parts.push(r?);
    }
    if !parts.is_empty() {
        return Ok(Some(parts.join("\n")));
    }

    if f.drive_online
        && f.cloud_state == "local"
        && f.size_bytes >= 0
        && f.size_bytes <= MAX_PLAIN_READ_BYTES
        && is_plain_text_ext(&f.ext)
    {
        let p = Path::new(&f.path);
        if p.is_file()
            && let Ok(bytes) = std::fs::read(p)
        {
            return Ok(Some(String::from_utf8_lossy(&bytes).into_owned()));
        }
    }

    Ok(None)
}

/// Loads a persisted conceptual summary for `(older_file_id, newer_file_id, model_id)` if
/// its stored file hashes still match and neither file is sensitive (`R7`).
pub fn load_cached_diff_summary(
    conn: &Connection,
    older_file_id: &str,
    newer_file_id: &str,
    model_id: &str,
    model_name: &str,
    backend: &str,
) -> Result<Option<VersionDiffSummary>> {
    let exists: bool = conn.query_row(
        "SELECT EXISTS(SELECT 1 FROM sqlite_master WHERE type = 'table' AND name = 'version_diff_summaries')",
        [],
        |r| r.get(0),
    )?;
    if !exists {
        return Ok(None);
    }
    let older = load_diff_file(conn, older_file_id)?;
    let newer = load_diff_file(conn, newer_file_id)?;
    if is_sensitive(&older.sensitivity) || is_sensitive(&newer.sensitivity) {
        return Ok(None);
    }
    struct CachedSummaryRow {
        older_hash: Option<Vec<u8>>,
        newer_hash: Option<Vec<u8>>,
        summary: String,
        badges_json: String,
    }
    let row: Option<CachedSummaryRow> = conn
        .query_row(
            "SELECT older_hash, newer_hash, summary, badges_json
             FROM version_diff_summaries
             WHERE older_file_id = ?1 AND newer_file_id = ?2 AND model_id = ?3",
            params![older_file_id, newer_file_id, model_id],
            |r| {
                Ok(CachedSummaryRow {
                    older_hash: r.get(0)?,
                    newer_hash: r.get(1)?,
                    summary: r.get(2)?,
                    badges_json: r.get(3)?,
                })
            },
        )
        .optional()?;
    let Some(CachedSummaryRow {
        older_hash: oh,
        newer_hash: nh,
        summary,
        badges_json,
    }) = row
    else {
        return Ok(None);
    };
    if oh != older.blake3 || nh != newer.blake3 {
        return Ok(None);
    }
    let badges: Vec<String> = serde_json::from_str(&badges_json).unwrap_or_default();
    Ok(Some(VersionDiffSummary {
        older_file_id: older_file_id.to_string(),
        newer_file_id: newer_file_id.to_string(),
        summary,
        badges,
        model_id: model_id.to_string(),
        model_name: model_name.to_string(),
        backend: backend.to_string(),
        cached: true,
        duration_ms: 0,
    }))
}

/// Persists a non-sensitive LLM diff summary into encrypted `version_diff_summaries` (`D1`).
/// Refuses caching if either file is `confidential` or `secret` (`R7`).
pub fn save_cached_diff_summary(
    conn: &Connection,
    summary: &VersionDiffSummary,
) -> Result<()> {
    let older = load_diff_file(conn, &summary.older_file_id)?;
    let newer = load_diff_file(conn, &summary.newer_file_id)?;
    if is_sensitive(&older.sensitivity) || is_sensitive(&newer.sensitivity) {
        return Ok(());
    }
    let badges_json = serde_json::to_string(&summary.badges).unwrap_or_else(|_| "[]".into());
    conn.execute(
        "INSERT INTO version_diff_summaries (
            older_file_id, newer_file_id, older_hash, newer_hash, model_id, summary, badges_json, created_at
         ) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8)
         ON CONFLICT(older_file_id, newer_file_id, model_id) DO UPDATE SET
            older_hash = excluded.older_hash,
            newer_hash = excluded.newer_hash,
            summary = excluded.summary,
            badges_json = excluded.badges_json,
            created_at = excluded.created_at",
        params![
            summary.older_file_id,
            summary.newer_file_id,
            older.blake3,
            newer.blake3,
            summary.model_id,
            summary.summary,
            badges_json,
            now_ms(),
        ],
    )?;
    Ok(())
}

/// Computes a visual diff + conceptual summary between `older_file_id` and `newer_file_id`.
/// Enforces `K2` (no quarantined/missing), `K4` (no backup read), and `R7` (redacts locked sensitive pairs).
pub fn diff_version_pair(
    conn: &Connection,
    older_file_id: &str,
    newer_file_id: &str,
    reveal_sensitive: bool,
) -> Result<VersionDiffReport> {
    let older = load_diff_file(conn, older_file_id)?;
    let newer = load_diff_file(conn, newer_file_id)?;

    let size_delta_bytes = newer.size_bytes - older.size_bytes;
    let time_delta_ms = newer.mtime - older.mtime;
    let identical_bytes = older.blake3.is_some() && older.blake3 == newer.blake3;

    if (is_sensitive(&older.sensitivity) || is_sensitive(&newer.sensitivity)) && !reveal_sensitive {
        return Ok(VersionDiffReport {
            older_file_id: older.file_id,
            newer_file_id: newer.file_id,
            older_filename: older.filename,
            newer_filename: newer.filename,
            older_path: older.path,
            newer_path: newer.path,
            older_size_bytes: older.size_bytes,
            newer_size_bytes: newer.size_bytes,
            older_mtime: older.mtime,
            newer_mtime: newer.mtime,
            size_delta_bytes,
            time_delta_ms,
            identical_bytes,
            diff_kind: "redacted_sensitive".into(),
            redacted_sensitive: true,
            truncated: false,
            added_lines: 0,
            removed_lines: 0,
            unchanged_lines: 0,
            semantic_similarity: None,
            conceptual_badges: vec!["Sensitive content locked (R7)".into()],
            deterministic_summary: "[Sensitive content hidden — unlock sensitive previews with Windows authentication to compare file revisions.]".into(),
            hunks: Vec::new(),
            cached_llm_summary: None,
        });
    }

    let vectors = load_file_vectors(conn, &[older.file_id.clone(), newer.file_id.clone()])?;
    let semantic_similarity = match (vectors.get(&older.file_id), vectors.get(&newer.file_id)) {
        (Some(va), Some(vb)) => Some(cosine_similarity(va, vb)),
        _ => None,
    };

    // Check any cached non-sensitive LLM summary already generated for this pair.
    let cached_llm_summary = if !is_sensitive(&older.sensitivity) && !is_sensitive(&newer.sensitivity) {
        load_any_cached_summary(conn, &older, &newer)?
    } else {
        None
    };

    let older_text_opt = extract_file_text(conn, &older)?;
    let newer_text_opt = extract_file_text(conn, &newer)?;

    let (Some(older_text), Some(newer_text)) = (older_text_opt, newer_text_opt) else {
        let mut badges = Vec::new();
        if identical_bytes {
            badges.push("Byte-identical".into());
        } else {
            badges.push("Binary / metadata revision".into());
            if size_delta_bytes > 0 {
                badges.push(format!("Expanded +{}", bytes_text(size_delta_bytes)));
            } else if size_delta_bytes < 0 {
                badges.push(format!("Reduced -{}", bytes_text(-size_delta_bytes)));
            }
        }
        if let Some(sim) = semantic_similarity {
            badges.push(format!("{:.0}% semantic match", sim * 100.0));
        }

        let dim_note = match (
            older.width.zip(older.height),
            newer.width.zip(newer.height),
        ) {
            (Some((ow, oh)), Some((nw, nh))) if (ow, oh) != (nw, nh) => {
                format!(" Dimensions changed from {ow}×{oh} to {nw}×{nh}.")
            }
            (Some((ow, oh)), Some(_)) => format!(" Canvas {ow}×{oh} unchanged."),
            _ => String::new(),
        };
        let color_note = match (&older.dominant_color, &newer.dominant_color) {
            (Some(oc), Some(nc)) if oc != nc => {
                format!(" Dominant palette shifted from {oc} to {nc}.")
            }
            _ => String::new(),
        };

        let deterministic_summary = if identical_bytes {
            format!(
                "`{}` and `{}` are byte-identical (matching BLAKE3 checksums).",
                older.filename, newer.filename
            )
        } else if size_delta_bytes == 0 {
            format!(
                "Binary revision with identical byte size ({}).{}{}",
                bytes_text(newer.size_bytes),
                dim_note,
                color_note
            )
        } else if size_delta_bytes > 0 {
            format!(
                "Binary revision grew by {} ({} -> {}).{}{}",
                bytes_text(size_delta_bytes),
                bytes_text(older.size_bytes),
                bytes_text(newer.size_bytes),
                dim_note,
                color_note
            )
        } else {
            format!(
                "Binary revision shrank by {} ({} -> {}).{}{}",
                bytes_text(-size_delta_bytes),
                bytes_text(older.size_bytes),
                bytes_text(newer.size_bytes),
                dim_note,
                color_note
            )
        };

        return Ok(VersionDiffReport {
            older_file_id: older.file_id,
            newer_file_id: newer.file_id,
            older_filename: older.filename,
            newer_filename: newer.filename,
            older_path: older.path,
            newer_path: newer.path,
            older_size_bytes: older.size_bytes,
            newer_size_bytes: newer.size_bytes,
            older_mtime: older.mtime,
            newer_mtime: newer.mtime,
            size_delta_bytes,
            time_delta_ms,
            identical_bytes,
            diff_kind: "binary_metadata".into(),
            redacted_sensitive: false,
            truncated: false,
            added_lines: 0,
            removed_lines: 0,
            unchanged_lines: 0,
            semantic_similarity,
            conceptual_badges: badges,
            deterministic_summary,
            hunks: Vec::new(),
            cached_llm_summary,
        });
    };

    let older_norm = older_text.replace("\r\n", "\n");
    let newer_norm = newer_text.replace("\r\n", "\n");
    let all_old_lines: Vec<&str> = older_norm.lines().collect();
    let all_new_lines: Vec<&str> = newer_norm.lines().collect();

    let truncated = all_old_lines.len() > MAX_DIFF_LINES || all_new_lines.len() > MAX_DIFF_LINES;
    let old_lines = &all_old_lines[..all_old_lines.len().min(MAX_DIFF_LINES)];
    let new_lines = &all_new_lines[..all_new_lines.len().min(MAX_DIFF_LINES)];

    let ops = compute_line_diff(old_lines, new_lines);
    let mut added_lines = 0usize;
    let mut removed_lines = 0usize;
    let mut unchanged_lines = 0usize;
    let mut added_texts: Vec<&str> = Vec::new();
    let mut removed_texts: Vec<&str> = Vec::new();

    for op in &ops {
        match *op {
            EditOp::Equal(_, _) => unchanged_lines += 1,
            EditOp::Remove(oi) => {
                removed_lines += 1;
                removed_texts.push(old_lines[oi]);
            }
            EditOp::Add(ni) => {
                added_lines += 1;
                added_texts.push(new_lines[ni]);
            }
        }
    }

    let hunks = build_hunks(&ops, old_lines, new_lines);
    let (conceptual_badges, deterministic_summary) = analyze_conceptual_changes(
        identical_bytes,
        added_lines,
        removed_lines,
        unchanged_lines,
        semantic_similarity,
        &removed_texts,
        &added_texts,
    );

    Ok(VersionDiffReport {
        older_file_id: older.file_id,
        newer_file_id: newer.file_id,
        older_filename: older.filename,
        newer_filename: newer.filename,
        older_path: older.path,
        newer_path: newer.path,
        older_size_bytes: older.size_bytes,
        newer_size_bytes: newer.size_bytes,
        older_mtime: older.mtime,
        newer_mtime: newer.mtime,
        size_delta_bytes,
        time_delta_ms,
        identical_bytes,
        diff_kind: "text".into(),
        redacted_sensitive: false,
        truncated,
        added_lines,
        removed_lines,
        unchanged_lines,
        semantic_similarity,
        conceptual_badges,
        deterministic_summary,
        hunks,
        cached_llm_summary,
    })
}

fn load_any_cached_summary(
    conn: &Connection,
    older: &DiffFileRow,
    newer: &DiffFileRow,
) -> Result<Option<VersionDiffSummary>> {
    let exists: bool = conn.query_row(
        "SELECT EXISTS(SELECT 1 FROM sqlite_master WHERE type = 'table' AND name = 'version_diff_summaries')",
        [],
        |r| r.get(0),
    )?;
    if !exists {
        return Ok(None);
    }
    struct AnyCachedSummaryRow {
        model_id: String,
        older_hash: Option<Vec<u8>>,
        newer_hash: Option<Vec<u8>>,
        summary: String,
        badges_json: String,
    }
    let row: Option<AnyCachedSummaryRow> = conn
        .query_row(
            "SELECT model_id, older_hash, newer_hash, summary, badges_json
             FROM version_diff_summaries
             WHERE older_file_id = ?1 AND newer_file_id = ?2
             ORDER BY created_at DESC LIMIT 1",
            params![older.file_id, newer.file_id],
            |r| {
                Ok(AnyCachedSummaryRow {
                    model_id: r.get(0)?,
                    older_hash: r.get(1)?,
                    newer_hash: r.get(2)?,
                    summary: r.get(3)?,
                    badges_json: r.get(4)?,
                })
            },
        )
        .optional()?;
    let Some(AnyCachedSummaryRow {
        model_id,
        older_hash: oh,
        newer_hash: nh,
        summary,
        badges_json,
    }) = row
    else {
        return Ok(None);
    };
    if oh != older.blake3 || nh != newer.blake3 {
        return Ok(None);
    }
    let badges: Vec<String> = serde_json::from_str(&badges_json).unwrap_or_default();
    Ok(Some(VersionDiffSummary {
        older_file_id: older.file_id.clone(),
        newer_file_id: newer.file_id.clone(),
        summary,
        badges,
        model_id: model_id.clone(),
        model_name: model_id,
        backend: "cached".into(),
        cached: true,
        duration_ms: 0,
    }))
}

fn compute_line_diff(old_lines: &[&str], new_lines: &[&str]) -> Vec<EditOp> {
    let mut prefix = 0usize;
    while prefix < old_lines.len()
        && prefix < new_lines.len()
        && old_lines[prefix] == new_lines[prefix]
    {
        prefix += 1;
    }

    let mut suffix = 0usize;
    while prefix + suffix < old_lines.len()
        && prefix + suffix < new_lines.len()
        && old_lines[old_lines.len() - 1 - suffix] == new_lines[new_lines.len() - 1 - suffix]
    {
        suffix += 1;
    }

    let mut ops = Vec::with_capacity(old_lines.len().max(new_lines.len()) + 8);
    for i in 0..prefix {
        ops.push(EditOp::Equal(i, i));
    }

    let old_mid_end = old_lines.len() - suffix;
    let new_mid_end = new_lines.len() - suffix;
    let old_mid = &old_lines[prefix..old_mid_end];
    let new_mid = &new_lines[prefix..new_mid_end];

    let m = old_mid.len();
    let n = new_mid.len();
    if m == 0 {
        for j in 0..n {
            ops.push(EditOp::Add(prefix + j));
        }
    } else if n == 0 {
        for i in 0..m {
            ops.push(EditOp::Remove(prefix + i));
        }
    } else if m * n <= MAX_LCS_CELLS {
        let cols = n + 1;
        let mut dp = vec![0u32; (m + 1) * cols];
        for i in (0..m).rev() {
            for j in (0..n).rev() {
                dp[i * cols + j] = if old_mid[i] == new_mid[j] {
                    dp[(i + 1) * cols + (j + 1)] + 1
                } else {
                    dp[(i + 1) * cols + j].max(dp[i * cols + (j + 1)])
                };
            }
        }
        let mut i = 0usize;
        let mut j = 0usize;
        while i < m && j < n {
            if old_mid[i] == new_mid[j] {
                ops.push(EditOp::Equal(prefix + i, prefix + j));
                i += 1;
                j += 1;
            } else if dp[(i + 1) * cols + j] >= dp[i * cols + (j + 1)] {
                ops.push(EditOp::Remove(prefix + i));
                i += 1;
            } else {
                ops.push(EditOp::Add(prefix + j));
                j += 1;
            }
        }
        while i < m {
            ops.push(EditOp::Remove(prefix + i));
            i += 1;
        }
        while j < n {
            ops.push(EditOp::Add(prefix + j));
            j += 1;
        }
    } else {
        // Large divergent middle fallback: anchor matching lines greedily without huge allocation.
        let mut new_pos_map: HashMap<&str, Vec<usize>> = HashMap::new();
        for (j, &line) in new_mid.iter().enumerate() {
            new_pos_map.entry(line).or_default().push(j);
        }
        let mut j = 0usize;
        for (i, &line) in old_mid.iter().enumerate() {
            if let Some(next_j) = new_pos_map
                .get(line)
                .and_then(|v| v.iter().copied().find(|&cand| cand >= j))
                && next_j - j <= 40
            {
                while j < next_j {
                    ops.push(EditOp::Add(prefix + j));
                    j += 1;
                }
                ops.push(EditOp::Equal(prefix + i, prefix + j));
                j += 1;
            } else {
                ops.push(EditOp::Remove(prefix + i));
            }
        }
        while j < n {
            ops.push(EditOp::Add(prefix + j));
            j += 1;
        }
    }

    for k in 0..suffix {
        ops.push(EditOp::Equal(old_mid_end + k, new_mid_end + k));
    }

    ops
}

fn build_hunks(ops: &[EditOp], old_lines: &[&str], new_lines: &[&str]) -> Vec<DiffHunk> {
    let changed_indices: Vec<usize> = ops
        .iter()
        .enumerate()
        .filter_map(|(idx, op)| (!matches!(op, EditOp::Equal(_, _))).then_some(idx))
        .collect();
    if changed_indices.is_empty() {
        return Vec::new();
    }

    let mut ranges: Vec<(usize, usize)> = Vec::new();
    for ci in changed_indices {
        let start = ci.saturating_sub(CONTEXT_LINES);
        let end = (ci + CONTEXT_LINES + 1).min(ops.len());
        if let Some(last) = ranges.last_mut()
            && start <= last.1 + 1
        {
            last.1 = last.1.max(end);
        } else {
            ranges.push((start, end));
        }
    }

    let mut hunks = Vec::with_capacity(ranges.len());
    for (start, end) in ranges {
        let mut lines = Vec::with_capacity(end - start);
        let mut old_start: Option<usize> = None;
        let mut new_start: Option<usize> = None;
        let mut old_count = 0usize;
        let mut new_count = 0usize;

        for &op in &ops[start..end] {
            match op {
                EditOp::Equal(oi, ni) => {
                    let o1 = oi + 1;
                    let n1 = ni + 1;
                    old_start.get_or_insert(o1);
                    new_start.get_or_insert(n1);
                    old_count += 1;
                    new_count += 1;
                    lines.push(DiffLine {
                        kind: DiffLineKind::Context,
                        old_line: Some(o1),
                        new_line: Some(n1),
                        text: old_lines[oi].to_string(),
                    });
                }
                EditOp::Remove(oi) => {
                    let o1 = oi + 1;
                    old_start.get_or_insert(o1);
                    old_count += 1;
                    lines.push(DiffLine {
                        kind: DiffLineKind::Removed,
                        old_line: Some(o1),
                        new_line: None,
                        text: old_lines[oi].to_string(),
                    });
                }
                EditOp::Add(ni) => {
                    let n1 = ni + 1;
                    new_start.get_or_insert(n1);
                    new_count += 1;
                    lines.push(DiffLine {
                        kind: DiffLineKind::Added,
                        old_line: None,
                        new_line: Some(n1),
                        text: new_lines[ni].to_string(),
                    });
                }
            }
        }

        hunks.push(DiffHunk {
            old_start: old_start.unwrap_or(1),
            old_count,
            new_start: new_start.unwrap_or(1),
            new_count,
            lines,
        });
    }
    hunks
}

fn extract_heading(line: &str) -> Option<String> {
    let trimmed = line.trim();
    if trimmed.starts_with('#') {
        let cleaned = trimmed.trim_start_matches('#').trim();
        if !cleaned.is_empty() && cleaned.len() <= 60 {
            return Some(cleaned.to_string());
        }
    }
    None
}

fn word_counts(lines: &[&str]) -> HashMap<String, i32> {
    let mut map = HashMap::new();
    for &line in lines {
        for raw in line.split(|c: char| !c.is_alphabetic()) {
            let low = raw.to_ascii_lowercase();
            if low.len() >= 4 && !DIFF_STOPWORDS.contains(&low.as_str()) {
                *map.entry(low).or_insert(0) += 1;
            }
        }
    }
    map
}

fn analyze_conceptual_changes(
    identical_bytes: bool,
    added_lines: usize,
    removed_lines: usize,
    unchanged_lines: usize,
    semantic_similarity: Option<f64>,
    removed_texts: &[&str],
    added_texts: &[&str],
) -> (Vec<String>, String) {
    if identical_bytes || (added_lines == 0 && removed_lines == 0) {
        let mut badges = vec!["Byte-identical".to_string()];
        if let Some(sim) = semantic_similarity {
            badges.push(format!("{:.0}% semantic match", sim * 100.0));
        }
        return (
            badges,
            "No textual differences between these two revisions.".to_string(),
        );
    }

    let mut badges: Vec<String> = Vec::new();
    let total_lines = (unchanged_lines + added_lines.max(removed_lines)).max(1);
    let churn_ratio = (added_lines + removed_lines) as f64 / total_lines as f64;
    if churn_ratio >= 0.45 && (added_lines + removed_lines) >= 6 {
        badges.push("Substantial rewrite".into());
    } else if added_lines > removed_lines + 2 {
        badges.push(format!("Expanded (+{} lines)", added_lines - removed_lines));
    } else if removed_lines > added_lines + 2 {
        badges.push(format!("Condensed (-{} lines)", removed_lines - added_lines));
    } else {
        badges.push("Minor revision".into());
    }

    if let Some(sim) = semantic_similarity {
        badges.push(format!("{:.0}% semantic match", sim * 100.0));
    }

    let old_headings: BTreeSet<String> = removed_texts.iter().filter_map(|l| extract_heading(l)).collect();
    let new_headings: BTreeSet<String> = added_texts.iter().filter_map(|l| extract_heading(l)).collect();
    let added_sections: Vec<String> = new_headings.difference(&old_headings).take(3).cloned().collect();
    let removed_sections: Vec<String> = old_headings.difference(&new_headings).take(3).cloned().collect();

    if !added_sections.is_empty() {
        badges.push(format!("+Section: {}", added_sections.join(", ")));
    }
    if !removed_sections.is_empty() {
        badges.push(format!("-Section: {}", removed_sections.join(", ")));
    }

    let old_words = word_counts(removed_texts);
    let new_words = word_counts(added_texts);
    let all_words: HashSet<&String> = old_words.keys().chain(new_words.keys()).collect();

    let mut gained: Vec<(i32, &str)> = Vec::new();
    let mut dropped: Vec<(i32, &str)> = Vec::new();
    for w in all_words {
        let oc = old_words.get(w).copied().unwrap_or(0);
        let nc = new_words.get(w).copied().unwrap_or(0);
        if nc > oc {
            gained.push((nc - oc, w.as_str()));
        } else if oc > nc {
            dropped.push((oc - nc, w.as_str()));
        }
    }
    gained.sort_by(|a, b| b.0.cmp(&a.0).then_with(|| a.1.cmp(b.1)));
    dropped.sort_by(|a, b| b.0.cmp(&a.0).then_with(|| a.1.cmp(b.1)));

    let top_gained: Vec<&str> = gained.into_iter().take(4).map(|(_, w)| w).collect();
    let top_dropped: Vec<&str> = dropped.into_iter().take(3).map(|(_, w)| w).collect();

    let mut sentences = vec![format!(
        "Changed +{added_lines} / -{removed_lines} lines ({unchanged_lines} unchanged)."
    )];
    if !added_sections.is_empty() {
        sentences.push(format!("Added sections: {}.", added_sections.join(", ")));
    }
    if !removed_sections.is_empty() {
        sentences.push(format!("Removed sections: {}.", removed_sections.join(", ")));
    }
    if !top_gained.is_empty() {
        sentences.push(format!("Emphasized terms: {}.", top_gained.join(", ")));
    }
    if !top_dropped.is_empty() {
        sentences.push(format!("Removed references: {}.", top_dropped.join(", ")));
    }

    (badges, sentences.join(" "))
}

/// Builds `(system_prompt, user_prompt, extractive_lines)` for optional on-demand local LLM
/// explanation of a textual `VersionDiffReport`. Returns `None` if redacted, binary, or unchanged.
pub fn build_diff_llm_prompt(diff: &VersionDiffReport) -> Option<(String, String, Vec<String>)> {
    if diff.redacted_sensitive
        || diff.identical_bytes
        || diff.diff_kind != "text"
        || diff.hunks.is_empty()
    {
        return None;
    }

    let mut diff_excerpt = String::new();
    let mut snippets = Vec::new();
    for hunk in &diff.hunks {
        let header = format!(
            "@@ -{},{} +{},{} @@\n",
            hunk.old_start, hunk.old_count, hunk.new_start, hunk.new_count
        );
        if diff_excerpt.len() + header.len() > 2_400 {
            break;
        }
        diff_excerpt.push_str(&header);
        for line in &hunk.lines {
            let prefix = match line.kind {
                DiffLineKind::Context => ' ',
                DiffLineKind::Added => '+',
                DiffLineKind::Removed => '-',
            };
            let formatted = format!("{prefix} {}\n", line.text);
            if diff_excerpt.len() + formatted.len() > 2_400 {
                break;
            }
            diff_excerpt.push_str(&formatted);
            if line.kind != DiffLineKind::Context && snippets.len() < 8 && !line.text.trim().is_empty() {
                snippets.push(format!("{prefix} {}", line.text.trim()));
            }
        }
    }

    if snippets.is_empty() {
        return None;
    }

    let system_prompt = "You summarize document revisions concisely for the file owner. In 2-3 clear sentences, explain how the newer version changed conceptually (topics added, removed, or restructured). Cite only facts visible in the diff hunks and deterministic summary.".to_string();
    let user_prompt = format!(
        "Compare `{}` (older) -> `{}` (newer).\nSignals: {}\nSummary stats: {}\n\nDiff excerpt:\n{}",
        diff.older_filename,
        diff.newer_filename,
        diff.conceptual_badges.join(", "),
        diff.deterministic_summary,
        diff_excerpt
    );

    Some((system_prompt, user_prompt, snippets))
}
