//! SQLCipher storage, idempotent watermark scanning, incremental entity merging with undo (`K1`),
//! `K2`/`K4` backup & opt-in audio gating, and `R7` sensitive provenance masking (`D1`).

use std::collections::{HashMap, HashSet};
use std::path::Path;
use std::time::Instant;

use rusqlite::{Connection, OptionalExtension, params};

use crate::db::SqliteStore;
use crate::db::audit::append_audit_in_tx;
use crate::db::files::{SENSITIVE_MASK, is_sensitive, path_is_within};
use crate::error::{AegisError, Result};
use crate::versions::patterns::is_excluded_relative_path;
use crate::{new_id, now_ms};

use super::parser::{
    ExtractedCandidate, canonical_key, extract_candidates_from_text, is_extractable_text_ext,
    looks_like_audio_or_video_ext, normalize_transcript_text, org_stem_key,
};
use super::{
    AudioRecordingItem, EntityProvenance, EntityRelationLink, EntityType, EntityTypeCount,
    KnowledgeCommitResult, KnowledgeDashboard, KnowledgeEntity, KnowledgeExtractOptions,
    KnowledgeExtractReport, KnowledgeProposal, KnowledgeQueryOptions, ProposalEditInput,
    ProposalStatus,
};

/// Opt-in flag controlling whether meeting audio/video recordings are surfaced for transcription (`K4`).
pub const AUDIO_OPT_IN_SETTING_KEY: &str = "knowledge.audio_observation_enabled";

const MAX_PLAIN_READ_BYTES: i64 = 2_000_000;
const DEFAULT_LIMIT_FILES: usize = 200;

struct CandidateFileRow {
    file_id: String,
    volume_id: String,
    mount_point: String,
    path: String,
    name: String,
    ext: String,
    size: i64,
    mtime: i64,
    cloud_state: String,
    drive_online: bool,
    sensitivity: String,
    who: Option<String>,
}

struct WatermarkRow {
    content_hash: Option<Vec<u8>>,
    mtime: i64,
}

struct ProposalDbRow {
    id: String,
    entity_type: String,
    name: String,
    summary: String,
    attrs_json: String,
    source_file_id: Option<String>,
    source_path: String,
    source_filename: String,
    source_kind: String,
    evidence_quote: String,
    extractor: String,
    confidence: f64,
    contains_sensitive: bool,
    matched_entity_id: Option<String>,
    matched_entity_name: Option<String>,
    status: String,
    committed_entity_id: Option<String>,
    created_at: i64,
    reviewed_at: Option<i64>,
}

struct ApproveProposalRow {
    id: String,
    entity_type: String,
    name: String,
    summary: String,
    attrs_json: String,
    source_file_id: Option<String>,
    evidence_quote: String,
    contains_sensitive: bool,
    matched_entity_id: Option<String>,
    status: String,
}

struct UndoProposalRow {
    status: String,
    committed_entity_id: Option<String>,
    committed_created_new: bool,
    committed_rel_ids_json: String,
    previous_entity_snapshot_json: Option<String>,
}

struct EntityDbRow {
    id: String,
    entity_type: String,
    name: String,
    attrs_json: String,
    created_at: i64,
    updated_at: i64,
}

struct ProvDbRow {
    id: String,
    file_id: String,
    path: String,
    filename: String,
    rel_type: String,
    evidence_json: String,
    created_at: i64,
    sensitivity: String,
}

struct LinkDbRow {
    id: String,
    other_id: String,
    other_type: String,
    other_name: String,
    rel_type: String,
    direction: String,
}

struct AudioDbRow {
    file_id: String,
    path: String,
    filename: String,
    ext: String,
    size_bytes: i64,
    mtime: i64,
    drive_label: String,
    drive_online: bool,
}

/// Reads whether `K4` opt-in audio/meeting observation is enabled (defaults to `false`).
pub fn audio_observation_enabled(conn: &Connection) -> Result<bool> {
    let raw: Option<String> = conn
        .query_row(
            "SELECT value_json FROM settings WHERE key = ?1 AND scope = 'global'",
            params![AUDIO_OPT_IN_SETTING_KEY],
            |r| r.get(0),
        )
        .optional()?;
    Ok(raw
        .and_then(|j| serde_json::from_str::<bool>(&j).ok())
        .unwrap_or(false))
}

/// Persists `K4` opt-in setting controlling audio recording discovery for local transcription.
pub fn set_audio_observation_enabled(conn: &Connection, enabled: bool) -> Result<bool> {
    let now = now_ms();
    let val = if enabled { "true" } else { "false" };
    conn.execute(
        "INSERT INTO settings (key, scope, value_json, updated_at)
         VALUES (?1, 'global', ?2, ?3)
         ON CONFLICT(key, scope) DO UPDATE SET
           value_json = excluded.value_json,
           updated_at = excluded.updated_at",
        params![AUDIO_OPT_IN_SETTING_KEY, val, now],
    )?;
    Ok(enabled)
}

/// Scans catalogued live non-backup files (`K2`, `K4`), skips locked sensitive files (`R7`),
/// tracks `knowledge_file_runs` watermarks, and lands extracted entities/decisions/actions into
/// provisional `knowledge_proposals` (`K1`).
pub fn extract_knowledge_from_catalog(
    conn: &Connection,
    options: &KnowledgeExtractOptions,
    reveal_sensitive: bool,
) -> Result<KnowledgeExtractReport> {
    let started = Instant::now();
    let include_sensitive = options.include_sensitive.unwrap_or(false);
    if include_sensitive && !reveal_sensitive {
        return Err(AegisError::PermissionDenied(
            "Unlock sensitive previews with Windows authentication before extracting from confidential/secret files."
                .into(),
        ));
    }

    let force_rescan = options.force_rescan.unwrap_or(false);
    let audio_enabled = audio_observation_enabled(conn)?;
    let ctx = load_collections_context(conn)?;
    let candidates_files = load_candidate_files(conn, options)?;

    let limit_files = options.limit_files.unwrap_or(DEFAULT_LIMIT_FILES).clamp(1, 1_000);
    let has_explicit_ids = options
        .file_ids
        .as_ref()
        .is_some_and(|v| !v.is_empty());

    let mut files_scanned = 0usize;
    let mut files_skipped_unchanged = 0usize;
    let mut files_skipped_sensitive = 0usize;
    let mut proposals_created = 0usize;
    let mut proposals_updated = 0usize;

    for f in candidates_files {
        if let Some(ref cid_filter) = options.collection_id {
            let file_col = resolve_file_collection(&f, &ctx.locations, &ctx.members);
            if file_col.as_deref() != Some(cid_filter.as_str()) {
                continue;
            }
        }

        let sensitive_file = is_sensitive(&f.sensitivity);
        if sensitive_file && !(include_sensitive && reveal_sensitive) {
            files_skipped_sensitive += 1;
            continue;
        }

        if files_scanned >= limit_files {
            break;
        }

        let Some(raw_text) = load_text_for_row(conn, &f)? else {
            continue;
        };
        if raw_text.trim().is_empty() && f.who.as_deref().map(str::trim).is_none_or(str::is_empty) {
            continue;
        }

        let text_hash = blake3::hash(raw_text.as_bytes()).as_bytes().to_vec();
        if !force_rescan && !has_explicit_ids {
            let wm: Option<WatermarkRow> = conn
                .query_row(
                    "SELECT content_hash, mtime FROM knowledge_file_runs WHERE file_id = ?1",
                    params![f.file_id],
                    |r| {
                        Ok(WatermarkRow {
                            content_hash: r.get(0)?,
                            mtime: r.get(1)?,
                        })
                    },
                )
                .optional()?;
            if let Some(wm) = wm
                && wm.mtime == f.mtime
                && wm.content_hash.as_deref() == Some(text_hash.as_slice())
            {
                files_skipped_unchanged += 1;
                continue;
            }
        }

        files_scanned += 1;
        let owning_col = resolve_file_collection(&f, &ctx.locations, &ctx.members)
            .and_then(|cid| ctx.names.get(&cid).cloned());
        let all_col_names: Vec<String> = ctx.names.values().cloned().collect();

        let extracted = extract_candidates_from_text(
            &f.name,
            &f.ext,
            &raw_text,
            &all_col_names,
            f.who.as_deref(),
            owning_col.as_deref(),
        );

        let (created, updated) = upsert_proposals_for_file(
            conn,
            &f.file_id,
            &f.path,
            &f.name,
            sensitive_file,
            &extracted,
            "deterministic",
        )?;
        proposals_created += created;
        proposals_updated += updated;

        conn.execute(
            "INSERT INTO knowledge_file_runs (file_id, content_hash, mtime, extractor, extracted_at)
             VALUES (?1, ?2, ?3, 'deterministic', ?4)
             ON CONFLICT(file_id) DO UPDATE SET
               content_hash = excluded.content_hash,
               mtime = excluded.mtime,
               extractor = excluded.extractor,
               extracted_at = excluded.extracted_at",
            params![f.file_id, text_hash, f.mtime, now_ms()],
        )?;
    }

    let total_pending: i64 = conn.query_row(
        "SELECT COUNT(*) FROM knowledge_proposals WHERE status = 'pending'",
        [],
        |r| r.get(0),
    )?;

    let untranscribed_audio_count = if audio_enabled {
        load_audio_recordings(conn)?
            .into_iter()
            .filter(|a| a.companion_transcript_file_id.is_none())
            .count()
    } else {
        0
    };

    Ok(KnowledgeExtractReport {
        files_scanned,
        files_skipped_unchanged,
        files_skipped_sensitive,
        proposals_created,
        proposals_updated,
        total_pending: total_pending.max(0) as usize,
        audio_observation_enabled: audio_enabled,
        untranscribed_audio_count,
        took_ms: started.elapsed().as_millis() as u64,
    })
}

/// Loads text for one non-backup, live file for optional on-demand local LLM extraction (`K2`, `K4`, `R7`).
pub fn load_file_text_for_llm(
    conn: &Connection,
    file_id: &str,
    reveal_sensitive: bool,
) -> Result<(String, String, Option<String>, bool)> {
    struct SingleFileRow {
        file_id: String,
        volume_id: String,
        mount_point: String,
        path: String,
        name: String,
        ext: String,
        size: i64,
        mtime: i64,
        cloud_state: String,
        drive_online: bool,
        sensitivity: String,
        state: String,
        role: Option<String>,
    }

    let row: Option<SingleFileRow> = conn
        .query_row(
            "SELECT f.id, f.volume_id, v.mount_point, f.path, f.name, COALESCE(f.ext, ''),
                    f.size, f.mtime, f.cloud_state, COALESCE(v.status = 'online', 0),
                    f.sensitivity, f.state, v.role
             FROM files f
             JOIN volumes v ON v.id = f.volume_id
             WHERE f.id = ?1 AND f.deleted_at IS NULL",
            params![file_id],
            |r| {
                Ok(SingleFileRow {
                    file_id: r.get(0)?,
                    volume_id: r.get(1)?,
                    mount_point: r.get(2)?,
                    path: r.get(3)?,
                    name: r.get(4)?,
                    ext: r.get(5)?,
                    size: r.get(6)?,
                    mtime: r.get(7)?,
                    cloud_state: r.get(8)?,
                    drive_online: r.get::<_, i64>(9)? != 0,
                    sensitivity: r.get(10)?,
                    state: r.get(11)?,
                    role: r.get(12)?,
                })
            },
        )
        .optional()?;

    let Some(r) = row else {
        return Err(AegisError::NotFound(format!("file {file_id}")));
    };
    if r.state == "quarantine" || r.state == "missing" {
        return Err(AegisError::InvalidInput(format!(
            "Cannot extract from `{}` because it is quarantined or missing.",
            r.name
        )));
    }
    if r.role.as_deref() == Some("backup") {
        return Err(AegisError::PermissionDenied(
            "Backup drives are read-only and never processed.".into(),
        ));
    }
    if is_sensitive(&r.sensitivity) && !reveal_sensitive {
        return Err(AegisError::PermissionDenied(
            "Unlock sensitive previews with Windows authentication before running LLM extraction on sensitive files."
                .into(),
        ));
    }

    let cand = CandidateFileRow {
        file_id: r.file_id,
        volume_id: r.volume_id,
        mount_point: r.mount_point,
        path: r.path,
        name: r.name,
        ext: r.ext,
        size: r.size,
        mtime: r.mtime,
        cloud_state: r.cloud_state,
        drive_online: r.drive_online,
        sensitivity: r.sensitivity,
        who: None,
    };
    let text = load_text_for_row(conn, &cand)?.ok_or_else(|| {
        AegisError::InvalidInput(format!(
            "No indexed chunks or readable local text found for `{}`.",
            cand.name
        ))
    })?;
    let (normalized, is_tr) = normalize_transcript_text(&cand.ext, &text);
    let ctx = load_collections_context(conn)?;
    let owning_col = resolve_file_collection(&cand, &ctx.locations, &ctx.members)
        .and_then(|cid| ctx.names.get(&cid).cloned());

    Ok((cand.name, normalized, owning_col, is_tr))
}

/// Inserts or updates provisional `pending` proposals produced by local LLM extraction (`extractor = 'local_llm'`).
pub fn ingest_llm_candidates(
    conn: &Connection,
    file_id: &str,
    candidates: &[ExtractedCandidate],
    reveal_sensitive: bool,
) -> Result<usize> {
    struct TargetMeta {
        path: String,
        name: String,
        sensitivity: String,
        state: String,
        role: Option<String>,
    }
    let meta: Option<TargetMeta> = conn
        .query_row(
            "SELECT f.path, f.name, f.sensitivity, f.state, v.role
             FROM files f JOIN volumes v ON v.id = f.volume_id
             WHERE f.id = ?1 AND f.deleted_at IS NULL",
            params![file_id],
            |r| {
                Ok(TargetMeta {
                    path: r.get(0)?,
                    name: r.get(1)?,
                    sensitivity: r.get(2)?,
                    state: r.get(3)?,
                    role: r.get(4)?,
                })
            },
        )
        .optional()?;
    let Some(m) = meta else {
        return Err(AegisError::NotFound(format!("file {file_id}")));
    };
    if m.state == "quarantine" || m.state == "missing" {
        return Err(AegisError::InvalidInput(
            "Cannot ingest knowledge from a quarantined or missing file.".into(),
        ));
    }
    if m.role.as_deref() == Some("backup") {
        return Err(AegisError::PermissionDenied(
            "Backup drives are read-only and never processed.".into(),
        ));
    }
    let sensitive_file = is_sensitive(&m.sensitivity);
    if sensitive_file && !reveal_sensitive {
        return Err(AegisError::PermissionDenied(
            "Sensitive file access requires Windows unlock.".into(),
        ));
    }
    let (created, updated) = upsert_proposals_for_file(
        conn,
        file_id,
        &m.path,
        &m.name,
        sensitive_file,
        candidates,
        "local_llm",
    )?;
    Ok(created + updated)
}

fn upsert_proposals_for_file(
    conn: &Connection,
    file_id: &str,
    file_path: &str,
    filename: &str,
    contains_sensitive: bool,
    candidates: &[ExtractedCandidate],
    extractor: &str,
) -> Result<(usize, usize)> {
    let now = now_ms();
    let mut created = 0usize;
    let mut updated = 0usize;

    for c in candidates {
        let key = canonical_key(c.entity_type, &c.name);
        if key.len() < 2 {
            continue;
        }
        let mut hasher = blake3::Hasher::new();
        hasher.update(file_id.as_bytes());
        hasher.update(b"\0");
        hasher.update(c.entity_type.as_str().as_bytes());
        hasher.update(b"\0");
        hasher.update(key.as_bytes());
        let dedupe_key = hasher.finalize().to_hex().to_string();

        let matched_entity_id = find_matching_entity(conn, c.entity_type, &c.name)?;
        let attrs_json = serde_json::json!({
            "assignee": c.assignee,
            "due_date": c.due_date,
            "project_hint": c.project_hint,
        })
        .to_string();

        let existing: Option<(String, String)> = conn
            .query_row(
                "SELECT id, status FROM knowledge_proposals WHERE dedupe_key = ?1",
                params![dedupe_key],
                |r| Ok((r.get(0)?, r.get(1)?)),
            )
            .optional()?;

        if let Some((pid, status)) = existing {
            if status == "pending" {
                conn.execute(
                    "UPDATE knowledge_proposals
                     SET name = ?2, summary = ?3, attrs_json = ?4, source_path = ?5,
                         source_filename = ?6, source_kind = ?7, evidence_quote = ?8,
                         extractor = ?9, confidence = ?10, contains_sensitive = ?11,
                         matched_entity_id = ?12
                     WHERE id = ?1",
                    params![
                        pid,
                        c.name,
                        c.summary,
                        attrs_json,
                        file_path,
                        filename,
                        c.source_kind,
                        c.evidence_quote,
                        extractor,
                        c.confidence,
                        i32::from(contains_sensitive),
                        matched_entity_id
                    ],
                )?;
                updated += 1;
            }
            // Approved or dismissed proposals remain untouched (`K1` preserves owner decisions).
        } else {
            conn.execute(
                "INSERT INTO knowledge_proposals (
                    id, dedupe_key, entity_type, name, summary, attrs_json,
                    source_file_id, source_path, source_filename, source_kind,
                    evidence_quote, extractor, confidence, contains_sensitive,
                    matched_entity_id, status, created_at
                 ) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12, ?13, ?14, ?15, 'pending', ?16)",
                params![
                    new_id(),
                    dedupe_key,
                    c.entity_type.as_str(),
                    c.name,
                    c.summary,
                    attrs_json,
                    file_id,
                    file_path,
                    filename,
                    c.source_kind,
                    c.evidence_quote,
                    extractor,
                    c.confidence,
                    i32::from(contains_sensitive),
                    matched_entity_id,
                    now
                ],
            )?;
            created += 1;
        }
    }

    Ok((created, updated))
}

fn find_matching_entity(conn: &Connection, etype: EntityType, name: &str) -> Result<Option<String>> {
    let key = canonical_key(etype, name);
    if let Some(eid) = conn
        .query_row(
            "SELECT entity_id FROM entity_aliases WHERE entity_type = ?1 AND normalized_key = ?2",
            params![etype.as_str(), key],
            |r| r.get::<_, String>(0),
        )
        .optional()?
    {
        return Ok(Some(eid));
    }
    if etype == EntityType::Organization
        && let Some(stem) = org_stem_key(name)
        && let Some(eid) = conn
            .query_row(
                "SELECT entity_id FROM entity_aliases WHERE entity_type = ?1 AND normalized_key = ?2",
                params![etype.as_str(), stem],
                |r| r.get::<_, String>(0),
            )
            .optional()?
    {
        return Ok(Some(eid));
    }
    Ok(None)
}

/// Approves a pending proposal (`K1`), incrementally merging into an existing canonical entity
/// when a matching alias exists, recording provenance/cross-entity edges in `relationships` and
/// saving an undo snapshot so `undo_approved_proposal` can reverse the commit cleanly.
pub fn approve_proposal(
    store: &mut SqliteStore,
    proposal_id: &str,
    edit: Option<ProposalEditInput>,
    reveal_sensitive: bool,
) -> Result<KnowledgeCommitResult> {
    let now = now_ms();
    store.with_full_sync_tx(|tx| {
        let p: Option<ApproveProposalRow> = tx
            .query_row(
                "SELECT id, entity_type, name, summary, attrs_json, source_file_id,
                        evidence_quote, contains_sensitive, matched_entity_id, status
                 FROM knowledge_proposals WHERE id = ?1",
                params![proposal_id],
                |r| {
                    Ok(ApproveProposalRow {
                        id: r.get(0)?,
                        entity_type: r.get(1)?,
                        name: r.get(2)?,
                        summary: r.get(3)?,
                        attrs_json: r.get(4)?,
                        source_file_id: r.get(5)?,
                        evidence_quote: r.get(6)?,
                        contains_sensitive: r.get::<_, i64>(7)? != 0,
                        matched_entity_id: r.get(8)?,
                        status: r.get(9)?,
                    })
                },
            )
            .optional()?;
        let Some(prop) = p else {
            return Err(AegisError::NotFound(format!("proposal {proposal_id}")));
        };
        if prop.contains_sensitive && !reveal_sensitive {
            return Err(AegisError::PermissionDenied(
                "Unlock sensitive previews with Windows authentication before approving sensitive proposals."
                    .into(),
            ));
        }
        if prop.status == "approved" {
            return Err(AegisError::InvalidInput(
                "Proposal has already been approved.".into(),
            ));
        }

        let edit_ref = edit.as_ref();
        let final_type = edit_ref
            .and_then(|e| e.entity_type)
            .unwrap_or_else(|| EntityType::from_db(&prop.entity_type));
        let final_name = edit_ref
            .and_then(|e| e.name.as_deref())
            .map(str::trim)
            .filter(|s| !s.is_empty())
            .unwrap_or(&prop.name)
            .to_string();
        let final_summary = edit_ref
            .and_then(|e| e.summary.as_deref())
            .map(str::trim)
            .filter(|s| !s.is_empty())
            .unwrap_or(&prop.summary)
            .to_string();

        let orig_attrs: serde_json::Value =
            serde_json::from_str(&prop.attrs_json).unwrap_or_else(|_| serde_json::json!({}));
        let final_assignee = edit_ref
            .and_then(|e| e.assignee.clone())
            .filter(|s| !s.trim().is_empty())
            .or_else(|| {
                orig_attrs
                    .get("assignee")
                    .and_then(|v| v.as_str())
                    .map(str::to_string)
            });
        let final_due = edit_ref
            .and_then(|e| e.due_date.clone())
            .filter(|s| !s.trim().is_empty())
            .or_else(|| {
                orig_attrs
                    .get("due_date")
                    .and_then(|v| v.as_str())
                    .map(str::to_string)
            });
        let final_project = edit_ref
            .and_then(|e| e.project_hint.clone())
            .filter(|s| !s.trim().is_empty())
            .or_else(|| {
                orig_attrs
                    .get("project_hint")
                    .and_then(|v| v.as_str())
                    .map(str::to_string)
            });

        // Resolve canonical entity: edited/current alias first, then prop.matched_entity_id if same type.
        let mut target_entity_id = find_matching_entity(tx, final_type, &final_name)?;
        if target_entity_id.is_none()
            && let Some(ref matched_id) = prop.matched_entity_id
        {
            let same_type: Option<String> = tx
                .query_row(
                    "SELECT id FROM entities WHERE id = ?1 AND entity_type = ?2",
                    params![matched_id, final_type.as_str()],
                    |r| r.get(0),
                )
                .optional()?;
            target_entity_id = same_type;
        }

        let (entity_id, created_new, snapshot_json) = if let Some(eid) = target_entity_id {
            let (cur_name, cur_attrs_str, cur_updated): (String, String, i64) = tx.query_row(
                "SELECT name, attrs_json, updated_at FROM entities WHERE id = ?1",
                params![eid],
                |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?)),
            )?;
            let snapshot = serde_json::json!({
                "name": cur_name,
                "attrs_json": cur_attrs_str,
                "updated_at": cur_updated,
            })
            .to_string();

            let mut attrs: serde_json::Map<String, serde_json::Value> =
                serde_json::from_str(&cur_attrs_str).unwrap_or_default();
            if !final_summary.is_empty() {
                let cur_sum = attrs
                    .get("summary")
                    .and_then(|v| v.as_str())
                    .unwrap_or("")
                    .trim()
                    .to_string();
                let merged = if cur_sum.is_empty() {
                    final_summary.clone()
                } else if !cur_sum.to_ascii_lowercase().contains(&final_summary.to_ascii_lowercase())
                    && cur_sum.len() < 400
                {
                    format!("{cur_sum}; {final_summary}")
                } else {
                    cur_sum
                };
                attrs.insert("summary".into(), serde_json::json!(merged));
            }
            if attrs.get("assignee").is_none_or(|v| v.is_null()) && final_assignee.is_some() {
                attrs.insert("assignee".into(), serde_json::json!(final_assignee));
            }
            if attrs.get("due_date").is_none_or(|v| v.is_null()) && final_due.is_some() {
                attrs.insert("due_date".into(), serde_json::json!(final_due));
            }
            if attrs.get("project_name").is_none_or(|v| v.is_null()) && final_project.is_some() {
                attrs.insert("project_name".into(), serde_json::json!(final_project));
            }
            if prop.contains_sensitive {
                attrs.insert("contains_sensitive".into(), serde_json::json!(true));
            }
            let mut aliases: Vec<String> = attrs
                .get("aliases")
                .and_then(|v| v.as_array())
                .map(|arr| {
                    arr.iter()
                        .filter_map(|x| x.as_str().map(str::to_string))
                        .collect()
                })
                .unwrap_or_default();
            if !cur_name.eq_ignore_ascii_case(&final_name)
                && !aliases.iter().any(|a| a.eq_ignore_ascii_case(&final_name))
            {
                aliases.push(final_name.clone());
            }
            attrs.insert("aliases".into(), serde_json::json!(aliases));

            let new_attrs_json = serde_json::Value::Object(attrs).to_string();
            tx.execute(
                "UPDATE entities SET attrs_json = ?2, updated_at = ?3 WHERE id = ?1",
                params![eid, new_attrs_json, now],
            )?;
            register_entity_aliases(tx, final_type, &eid, &final_name, now)?;
            (eid, false, Some(snapshot))
        } else {
            let eid = new_id();
            let status_tag = match final_type {
                EntityType::ActionItem => "open",
                EntityType::Decision => "recorded",
                _ => "active",
            };
            let attrs_json = serde_json::json!({
                "summary": final_summary,
                "assignee": final_assignee,
                "due_date": final_due,
                "project_name": final_project,
                "status_tag": status_tag,
                "aliases": Vec::<String>::new(),
                "contains_sensitive": prop.contains_sensitive,
            })
            .to_string();
            tx.execute(
                "INSERT INTO entities (id, entity_type, name, attrs_json, vector_synced, created_at, updated_at)
                 VALUES (?1, ?2, ?3, ?4, 0, ?5, ?5)",
                params![eid, final_type.as_str(), final_name, attrs_json, now],
            )?;
            register_entity_aliases(tx, final_type, &eid, &final_name, now)?;
            (eid, true, None)
        };

        let mut created_rel_ids: Vec<String> = Vec::new();

        // File -> Entity provenance link.
        if let Some(ref fid) = prop.source_file_id {
            let rel_id = new_id();
            let rel_type = match final_type {
                EntityType::Decision => "decided_in",
                EntityType::ActionItem => "assigned_in",
                _ => "mentioned_in",
            };
            let ev_json = serde_json::json!({
                "quote": prop.evidence_quote,
                "proposal_id": prop.id,
            })
            .to_string();
            tx.execute(
                "INSERT INTO relationships (
                    id, src_id, src_kind, dst_id, dst_kind, rel_type, weight, evidence_json, valid_from, valid_to, created_at
                 ) VALUES (?1, ?2, 'file', ?3, ?4, ?5, 1.0, ?6, ?7, NULL, ?7)",
                params![rel_id, fid, entity_id, final_type.as_str(), rel_type, ev_json, now],
            )?;
            created_rel_ids.push(rel_id);
        }

        // Cross-entity link: Person -> ActionItem (`owns_action`).
        if final_type == EntityType::ActionItem
            && let Some(ref assignee) = final_assignee
            && let Some(person_id) = find_matching_entity(tx, EntityType::Person, assignee)?
        {
            let rel_id = new_id();
            tx.execute(
                "INSERT INTO relationships (
                    id, src_id, src_kind, dst_id, dst_kind, rel_type, weight, evidence_json, valid_from, valid_to, created_at
                 ) VALUES (?1, ?2, 'person', ?3, 'action_item', 'owns_action', 1.0, '[]', ?4, NULL, ?4)",
                params![rel_id, person_id, entity_id, now],
            )?;
            created_rel_ids.push(rel_id);
        }

        // Cross-entity link: Entity -> Project (`belongs_to_project`).
        if final_type != EntityType::Project
            && let Some(ref proj) = final_project
            && let Some(proj_id) = find_matching_entity(tx, EntityType::Project, proj)?
        {
            let rel_id = new_id();
            tx.execute(
                "INSERT INTO relationships (
                    id, src_id, src_kind, dst_id, dst_kind, rel_type, weight, evidence_json, valid_from, valid_to, created_at
                 ) VALUES (?1, ?2, ?3, ?4, 'project', 'belongs_to_project', 1.0, '[]', ?5, NULL, ?5)",
                params![rel_id, entity_id, final_type.as_str(), proj_id, now],
            )?;
            created_rel_ids.push(rel_id);
        }

        let rel_ids_json = serde_json::to_string(&created_rel_ids).unwrap_or_else(|_| "[]".into());
        let final_attrs_prop = serde_json::json!({
            "assignee": final_assignee,
            "due_date": final_due,
            "project_hint": final_project,
        })
        .to_string();

        tx.execute(
            "UPDATE knowledge_proposals
             SET entity_type = ?2, name = ?3, summary = ?4, attrs_json = ?5,
                 status = 'approved', committed_entity_id = ?6,
                 committed_created_new = ?7, committed_rel_ids_json = ?8,
                 previous_entity_snapshot_json = ?9, reviewed_at = ?10
             WHERE id = ?1",
            params![
                proposal_id,
                final_type.as_str(),
                final_name,
                final_summary,
                final_attrs_prop,
                entity_id,
                i32::from(created_new),
                rel_ids_json,
                snapshot_json,
                now
            ],
        )?;

        append_audit_in_tx(
            tx,
            "owner",
            "knowledge.proposal.approved",
            &entity_id,
            &serde_json::json!({
                "proposal_id": proposal_id,
                "created_new": created_new,
                "entity_type": final_type.as_str(),
            })
            .to_string(),
        )?;

        Ok(KnowledgeCommitResult {
            proposal_id: proposal_id.to_string(),
            entity_id,
            created_new,
            merged_into_existing: !created_new,
            relationships_added: created_rel_ids.len(),
        })
    })
}

fn register_entity_aliases(
    tx: &Connection,
    etype: EntityType,
    entity_id: &str,
    name: &str,
    now: i64,
) -> Result<()> {
    let key = canonical_key(etype, name);
    if key.len() >= 2 {
        tx.execute(
            "INSERT OR IGNORE INTO entity_aliases (normalized_key, entity_type, entity_id, alias_text, created_at)
             VALUES (?1, ?2, ?3, ?4, ?5)",
            params![key, etype.as_str(), entity_id, name, now],
        )?;
    }
    if etype == EntityType::Organization
        && let Some(stem) = org_stem_key(name)
        && stem.len() >= 3
    {
        tx.execute(
            "INSERT OR IGNORE INTO entity_aliases (normalized_key, entity_type, entity_id, alias_text, created_at)
             VALUES (?1, ?2, ?3, ?4, ?5)",
            params![stem, etype.as_str(), entity_id, name, now],
        )?;
    }
    Ok(())
}

/// Atomically reverts an approved knowledge proposal back to `pending`, deleting its committed
/// relationship edges and either deleting a newly created entity or restoring its pre-merge snapshot (`K1`).
pub fn undo_approved_proposal(store: &mut SqliteStore, proposal_id: &str) -> Result<()> {
    store.with_full_sync_tx(|tx| {
        let row: Option<UndoProposalRow> = tx
            .query_row(
                "SELECT status, committed_entity_id, committed_created_new,
                        committed_rel_ids_json, previous_entity_snapshot_json
                 FROM knowledge_proposals WHERE id = ?1",
                params![proposal_id],
                |r| {
                    Ok(UndoProposalRow {
                        status: r.get(0)?,
                        committed_entity_id: r.get(1)?,
                        committed_created_new: r.get::<_, i64>(2)? != 0,
                        committed_rel_ids_json: r.get(3)?,
                        previous_entity_snapshot_json: r.get(4)?,
                    })
                },
            )
            .optional()?;
        let Some(u) = row else {
            return Err(AegisError::NotFound(format!("proposal {proposal_id}")));
        };
        if u.status != "approved" {
            return Err(AegisError::InvalidInput(
                "Only approved knowledge proposals can be undone.".into(),
            ));
        }

        let rel_ids: Vec<String> =
            serde_json::from_str(&u.committed_rel_ids_json).unwrap_or_default();
        for rid in &rel_ids {
            tx.execute("DELETE FROM relationships WHERE id = ?1", params![rid])?;
        }

        if let Some(ref eid) = u.committed_entity_id {
            let other_refs: i64 = tx.query_row(
                "SELECT COUNT(*) FROM knowledge_proposals
                 WHERE committed_entity_id = ?1 AND id != ?2 AND status = 'approved'",
                params![eid, proposal_id],
                |r| r.get(0),
            )?;
            if u.committed_created_new && other_refs == 0 {
                tx.execute(
                    "DELETE FROM relationships WHERE src_id = ?1 OR dst_id = ?1",
                    params![eid],
                )?;
                tx.execute("DELETE FROM entity_aliases WHERE entity_id = ?1", params![eid])?;
                tx.execute("DELETE FROM entities WHERE id = ?1", params![eid])?;
            } else if let Some(ref snap_str) = u.previous_entity_snapshot_json
                && let Ok(snap) = serde_json::from_str::<serde_json::Value>(snap_str)
                && let (Some(prev_name), Some(prev_attrs), Some(prev_updated)) = (
                    snap.get("name").and_then(|v| v.as_str()),
                    snap.get("attrs_json").and_then(|v| v.as_str()),
                    snap.get("updated_at").and_then(|v| v.as_i64()),
                )
            {
                tx.execute(
                    "UPDATE entities SET name = ?2, attrs_json = ?3, updated_at = ?4 WHERE id = ?1",
                    params![eid, prev_name, prev_attrs, prev_updated],
                )?;
            }
        }

        tx.execute(
            "UPDATE knowledge_proposals
             SET status = 'pending', committed_entity_id = NULL,
                 committed_created_new = 0, committed_rel_ids_json = '[]',
                 previous_entity_snapshot_json = NULL, reviewed_at = NULL
             WHERE id = ?1",
            params![proposal_id],
        )?;

        append_audit_in_tx(
            tx,
            "owner",
            "knowledge.proposal.undone",
            proposal_id,
            "{}",
        )?;
        Ok(())
    })
}

/// Marks a pending proposal as `dismissed` (`K1` keeps owner's negative preference across rescans).
pub fn dismiss_proposal(conn: &Connection, proposal_id: &str) -> Result<()> {
    let changed = conn.execute(
        "UPDATE knowledge_proposals SET status = 'dismissed', reviewed_at = ?2 WHERE id = ?1 AND status != 'approved'",
        params![proposal_id, now_ms()],
    )?;
    if changed == 0 {
        return Err(AegisError::NotFound(format!(
            "non-approved proposal {proposal_id}"
        )));
    }
    Ok(())
}

/// Restores a dismissed proposal back to `pending`.
pub fn reopen_proposal(conn: &Connection, proposal_id: &str) -> Result<()> {
    let changed = conn.execute(
        "UPDATE knowledge_proposals SET status = 'pending', reviewed_at = NULL WHERE id = ?1 AND status = 'dismissed'",
        params![proposal_id],
    )?;
    if changed == 0 {
        return Err(AegisError::NotFound(format!(
            "dismissed proposal {proposal_id}"
        )));
    }
    Ok(())
}

/// Updates an entity's workflow status tag (e.g., marking an `ActionItem` `"done"` or `"open"`).
pub fn update_entity_status(conn: &Connection, entity_id: &str, status_tag: &str) -> Result<()> {
    let clean = status_tag.trim();
    if clean.is_empty() {
        return Err(AegisError::InvalidInput("status_tag must not be empty.".into()));
    }
    let attrs_str: Option<String> = conn
        .query_row(
            "SELECT attrs_json FROM entities WHERE id = ?1",
            params![entity_id],
            |r| r.get(0),
        )
        .optional()?;
    let Some(attrs_str) = attrs_str else {
        return Err(AegisError::NotFound(format!("entity {entity_id}")));
    };
    let mut map: serde_json::Map<String, serde_json::Value> =
        serde_json::from_str(&attrs_str).unwrap_or_default();
    map.insert("status_tag".into(), serde_json::json!(clean));
    let next = serde_json::Value::Object(map).to_string();
    conn.execute(
        "UPDATE entities SET attrs_json = ?2, updated_at = ?3 WHERE id = ?1",
        params![entity_id, next, now_ms()],
    )?;
    Ok(())
}

/// Queries the Knowledge Base dashboard: typed counts, `K1` proposals inbox, deduplicated
/// `entities` with live non-quarantined provenance (`K2`), and `K4` opt-in audio recordings (`R7`).
pub fn query_knowledge_dashboard(
    conn: &Connection,
    options: &KnowledgeQueryOptions,
    reveal_sensitive: bool,
) -> Result<KnowledgeDashboard> {
    let audio_enabled = audio_observation_enabled(conn)?;

    let pending_count: i64 = conn.query_row(
        "SELECT COUNT(*) FROM knowledge_proposals WHERE status = 'pending'",
        [],
        |r| r.get(0),
    )?;
    let approved_count: i64 = conn.query_row(
        "SELECT COUNT(*) FROM knowledge_proposals WHERE status = 'approved'",
        [],
        |r| r.get(0),
    )?;
    let dismissed_count: i64 = conn.query_row(
        "SELECT COUNT(*) FROM knowledge_proposals WHERE status = 'dismissed'",
        [],
        |r| r.get(0),
    )?;
    let total_entities: i64 = conn.query_row("SELECT COUNT(*) FROM entities", [], |r| r.get(0))?;

    let mut counts_by_type = Vec::with_capacity(EntityType::ALL.len());
    for et in EntityType::ALL {
        let pc: i64 = conn.query_row(
            "SELECT COUNT(*) FROM knowledge_proposals WHERE entity_type = ?1 AND status = 'pending'",
            params![et.as_str()],
            |r| r.get(0),
        )?;
        let ec: i64 = conn.query_row(
            "SELECT COUNT(*) FROM entities WHERE entity_type = ?1",
            params![et.as_str()],
            |r| r.get(0),
        )?;
        counts_by_type.push(EntityTypeCount {
            entity_type: et,
            label: et.label().to_string(),
            pending_count: pc.max(0) as usize,
            entity_count: ec.max(0) as usize,
        });
    }

    let query_low = options
        .query
        .as_deref()
        .map(str::trim)
        .filter(|s| !s.is_empty())
        .map(|s| s.to_ascii_lowercase());
    let limit = options.limit.unwrap_or(120).clamp(1, 500);

    // Load proposals
    let status_filter = options.status.unwrap_or(ProposalStatus::Pending);
    let mut prop_st = conn.prepare(
        "SELECT p.id, p.entity_type, p.name, p.summary, p.attrs_json,
                p.source_file_id, p.source_path, p.source_filename, p.source_kind,
                p.evidence_quote, p.extractor, p.confidence, p.contains_sensitive,
                p.matched_entity_id, e.name, p.status, p.committed_entity_id,
                p.created_at, p.reviewed_at
         FROM knowledge_proposals p
         LEFT JOIN entities e ON e.id = p.matched_entity_id
         WHERE p.status = ?1
         ORDER BY p.confidence DESC, p.created_at DESC",
    )?;
    let prop_rows = prop_st.query_map(params![status_filter.as_str()], |r| {
        Ok(ProposalDbRow {
            id: r.get(0)?,
            entity_type: r.get(1)?,
            name: r.get(2)?,
            summary: r.get(3)?,
            attrs_json: r.get(4)?,
            source_file_id: r.get(5)?,
            source_path: r.get(6)?,
            source_filename: r.get(7)?,
            source_kind: r.get(8)?,
            evidence_quote: r.get(9)?,
            extractor: r.get(10)?,
            confidence: r.get(11)?,
            contains_sensitive: r.get::<_, i64>(12)? != 0,
            matched_entity_id: r.get(13)?,
            matched_entity_name: r.get(14)?,
            status: r.get(15)?,
            committed_entity_id: r.get(16)?,
            created_at: r.get(17)?,
            reviewed_at: r.get(18)?,
        })
    })?;

    let mut proposals = Vec::new();
    for row in prop_rows {
        let pr = row?;
        let et = EntityType::from_db(&pr.entity_type);
        if let Some(et_f) = options.entity_type
            && et != et_f
        {
            continue;
        }
        let redacted = pr.contains_sensitive && !reveal_sensitive;
        let attrs_val: serde_json::Value =
            serde_json::from_str(&pr.attrs_json).unwrap_or_else(|_| serde_json::json!({}));
        let assignee = if redacted {
            None
        } else {
            attrs_val
                .get("assignee")
                .and_then(|v| v.as_str())
                .map(str::to_string)
        };
        let due_date = if redacted {
            None
        } else {
            attrs_val
                .get("due_date")
                .and_then(|v| v.as_str())
                .map(str::to_string)
        };
        let project_hint = if redacted {
            None
        } else {
            attrs_val
                .get("project_hint")
                .and_then(|v| v.as_str())
                .map(str::to_string)
        };
        let summary = if redacted {
            SENSITIVE_MASK.to_string()
        } else {
            pr.summary
        };
        let evidence_quote = if redacted {
            SENSITIVE_MASK.to_string()
        } else {
            pr.evidence_quote
        };
        let display_name = if redacted && matches!(et, EntityType::Decision | EntityType::ActionItem) {
            SENSITIVE_MASK.to_string()
        } else {
            pr.name
        };

        if let Some(ref ql) = query_low {
            let mut hay = format!("{} {}", display_name, pr.source_filename).to_ascii_lowercase();
            if !redacted {
                hay.push(' ');
                hay.push_str(&summary.to_ascii_lowercase());
                hay.push(' ');
                hay.push_str(&evidence_quote.to_ascii_lowercase());
            }
            if !hay.contains(ql) {
                continue;
            }
        }

        proposals.push(KnowledgeProposal {
            id: pr.id,
            entity_type: et,
            name: display_name,
            summary,
            attrs_json: if redacted { "{}".into() } else { pr.attrs_json },
            assignee,
            due_date,
            project_hint,
            source_file_id: pr.source_file_id,
            source_path: pr.source_path,
            source_filename: pr.source_filename,
            source_kind: pr.source_kind,
            evidence_quote,
            extractor: pr.extractor,
            confidence: pr.confidence,
            contains_sensitive: pr.contains_sensitive,
            redacted,
            matched_entity_id: pr.matched_entity_id,
            matched_entity_name: pr.matched_entity_name,
            status: ProposalStatus::from_db(&pr.status),
            committed_entity_id: pr.committed_entity_id,
            created_at: pr.created_at,
            reviewed_at: pr.reviewed_at,
        });
        if proposals.len() >= limit {
            break;
        }
    }

    // Load approved canonical entities
    let mut ent_st = conn.prepare(
        "SELECT id, entity_type, name, attrs_json, created_at, updated_at
         FROM entities
         ORDER BY updated_at DESC, created_at DESC",
    )?;
    let ent_rows = ent_st.query_map([], |r| {
        Ok(EntityDbRow {
            id: r.get(0)?,
            entity_type: r.get(1)?,
            name: r.get(2)?,
            attrs_json: r.get(3)?,
            created_at: r.get(4)?,
            updated_at: r.get(5)?,
        })
    })?;

    let mut entities = Vec::new();
    for row in ent_rows {
        let er = row?;
        let et = EntityType::from_db(&er.entity_type);
        if let Some(et_f) = options.entity_type
            && et != et_f
        {
            continue;
        }
        let attrs: serde_json::Value =
            serde_json::from_str(&er.attrs_json).unwrap_or_else(|_| serde_json::json!({}));
        let contains_sensitive = attrs
            .get("contains_sensitive")
            .and_then(|v| v.as_bool())
            .unwrap_or(false);
        let redacted = contains_sensitive && !reveal_sensitive;

        let provenance = load_entity_provenance(conn, &er.id, reveal_sensitive)?;
        let related_entities = load_entity_links(conn, &er.id)?;
        let mention_count = provenance.len().max(1);

        let summary = if redacted {
            SENSITIVE_MASK.to_string()
        } else {
            attrs
                .get("summary")
                .and_then(|v| v.as_str())
                .unwrap_or("")
                .to_string()
        };
        let display_name = if redacted && matches!(et, EntityType::Decision | EntityType::ActionItem) {
            SENSITIVE_MASK.to_string()
        } else {
            er.name
        };
        let aliases: Vec<String> = if redacted {
            Vec::new()
        } else {
            attrs
                .get("aliases")
                .and_then(|v| v.as_array())
                .map(|a| {
                    a.iter()
                        .filter_map(|x| x.as_str().map(str::to_string))
                        .collect()
                })
                .unwrap_or_default()
        };
        let assignee = if redacted {
            None
        } else {
            attrs
                .get("assignee")
                .and_then(|v| v.as_str())
                .map(str::to_string)
        };
        let due_date = if redacted {
            None
        } else {
            attrs
                .get("due_date")
                .and_then(|v| v.as_str())
                .map(str::to_string)
        };
        let status_tag = attrs
            .get("status_tag")
            .and_then(|v| v.as_str())
            .map(str::to_string);
        let project_name = if redacted {
            None
        } else {
            attrs
                .get("project_name")
                .and_then(|v| v.as_str())
                .map(str::to_string)
        };

        if let Some(ref ql) = query_low {
            let mut hay = display_name.to_ascii_lowercase();
            if !redacted {
                hay.push(' ');
                hay.push_str(&summary.to_ascii_lowercase());
                for a in &aliases {
                    hay.push(' ');
                    hay.push_str(&a.to_ascii_lowercase());
                }
            }
            if !hay.contains(ql) {
                continue;
            }
        }

        entities.push(KnowledgeEntity {
            id: er.id,
            entity_type: et,
            name: display_name,
            summary,
            aliases,
            assignee,
            due_date,
            status_tag,
            project_name,
            mention_count,
            contains_sensitive,
            created_at: er.created_at,
            updated_at: er.updated_at,
            provenance,
            related_entities,
        });
        if entities.len() >= limit {
            break;
        }
    }

    let audio_recordings = if audio_enabled {
        load_audio_recordings(conn)?
    } else {
        Vec::new()
    };

    Ok(KnowledgeDashboard {
        pending_count: pending_count.max(0) as usize,
        approved_count: approved_count.max(0) as usize,
        dismissed_count: dismissed_count.max(0) as usize,
        total_entities: total_entities.max(0) as usize,
        counts_by_type,
        audio_observation_enabled: audio_enabled,
        audio_recordings,
        proposals,
        entities,
    })
}

fn load_entity_provenance(
    conn: &Connection,
    entity_id: &str,
    reveal_sensitive: bool,
) -> Result<Vec<EntityProvenance>> {
    let mut st = conn.prepare(
        "SELECT r.id, f.id, f.path, f.name, r.rel_type, r.evidence_json, r.created_at, f.sensitivity
         FROM relationships r
         JOIN files f ON f.id = r.src_id
         JOIN volumes v ON v.id = f.volume_id
         WHERE r.dst_id = ?1
           AND r.src_kind = 'file'
           AND f.deleted_at IS NULL
           AND f.state NOT IN ('quarantine', 'missing')
           AND COALESCE(v.role, '') != 'backup'
         ORDER BY r.created_at DESC",
    )?;
    let rows = st.query_map(params![entity_id], |r| {
        Ok(ProvDbRow {
            id: r.get(0)?,
            file_id: r.get(1)?,
            path: r.get(2)?,
            filename: r.get(3)?,
            rel_type: r.get(4)?,
            evidence_json: r.get(5)?,
            created_at: r.get(6)?,
            sensitivity: r.get(7)?,
        })
    })?;
    let mut out = Vec::new();
    for r in rows {
        let p = r?;
        let redacted = is_sensitive(&p.sensitivity) && !reveal_sensitive;
        let quote = if redacted {
            SENSITIVE_MASK.to_string()
        } else {
            serde_json::from_str::<serde_json::Value>(&p.evidence_json)
                .ok()
                .and_then(|v| v.get("quote").and_then(|q| q.as_str()).map(str::to_string))
                .unwrap_or_default()
        };
        out.push(EntityProvenance {
            relationship_id: p.id,
            file_id: p.file_id,
            path: p.path,
            filename: p.filename,
            rel_type: p.rel_type,
            quote,
            redacted,
            recorded_at: p.created_at,
        });
    }
    Ok(out)
}

fn load_entity_links(conn: &Connection, entity_id: &str) -> Result<Vec<EntityRelationLink>> {
    let mut out = Vec::new();
    let mut st_out = conn.prepare(
        "SELECT r.id, e.id, e.entity_type, e.name, r.rel_type
         FROM relationships r
         JOIN entities e ON e.id = r.dst_id
         WHERE r.src_id = ?1 AND r.src_kind != 'file'",
    )?;
    let out_rows = st_out.query_map(params![entity_id], |r| {
        Ok(LinkDbRow {
            id: r.get(0)?,
            other_id: r.get(1)?,
            other_type: r.get(2)?,
            other_name: r.get(3)?,
            rel_type: r.get(4)?,
            direction: "outgoing".into(),
        })
    })?;
    for r in out_rows {
        let l = r?;
        out.push(EntityRelationLink {
            relationship_id: l.id,
            other_entity_id: l.other_id,
            other_entity_type: EntityType::from_db(&l.other_type),
            other_entity_name: l.other_name,
            rel_type: l.rel_type,
            direction: l.direction,
        });
    }

    let mut st_in = conn.prepare(
        "SELECT r.id, e.id, e.entity_type, e.name, r.rel_type
         FROM relationships r
         JOIN entities e ON e.id = r.src_id
         WHERE r.dst_id = ?1 AND r.src_kind != 'file'",
    )?;
    let in_rows = st_in.query_map(params![entity_id], |r| {
        Ok(LinkDbRow {
            id: r.get(0)?,
            other_id: r.get(1)?,
            other_type: r.get(2)?,
            other_name: r.get(3)?,
            rel_type: r.get(4)?,
            direction: "incoming".into(),
        })
    })?;
    for r in in_rows {
        let l = r?;
        out.push(EntityRelationLink {
            relationship_id: l.id,
            other_entity_id: l.other_id,
            other_entity_type: EntityType::from_db(&l.other_type),
            other_entity_name: l.other_name,
            rel_type: l.rel_type,
            direction: l.direction,
        });
    }

    Ok(out)
}

fn load_audio_recordings(conn: &Connection) -> Result<Vec<AudioRecordingItem>> {
    let mut st = conn.prepare(
        "SELECT f.id, f.path, f.name, COALESCE(f.ext, ''), f.size, f.mtime,
                COALESCE(NULLIF(TRIM(v.label), ''), v.mount_point, v.id) AS drive_label,
                COALESCE(v.status = 'online', 0) AS drive_online
         FROM files f
         JOIN volumes v ON v.id = f.volume_id
         WHERE f.deleted_at IS NULL
           AND f.state NOT IN ('quarantine', 'missing')
           AND f.container_file_id IS NULL
           AND COALESCE(v.role, '') != 'backup'
         ORDER BY f.mtime DESC",
    )?;
    let rows = st.query_map([], |r| {
        Ok(AudioDbRow {
            file_id: r.get(0)?,
            path: r.get(1)?,
            filename: r.get(2)?,
            ext: r.get(3)?,
            size_bytes: r.get(4)?,
            mtime: r.get(5)?,
            drive_label: r.get(6)?,
            drive_online: r.get::<_, i64>(7)? != 0,
        })
    })?;

    let mut transcripts_by_dir_stem: HashMap<(String, String), (String, String)> = HashMap::new();
    let mut audio_rows = Vec::new();

    for row in rows {
        let a = row?;
        let ext_low = a.ext.trim().trim_start_matches('.').to_ascii_lowercase();
        let parent_low = Path::new(&a.path)
            .parent()
            .map(|p| p.to_string_lossy().replace('\\', "/").to_ascii_lowercase())
            .unwrap_or_default();
        let stem_low = Path::new(&a.filename)
            .file_stem()
            .map(|s| s.to_string_lossy().to_ascii_lowercase())
            .unwrap_or_default();

        if matches!(ext_low.as_str(), "vtt" | "srt") {
            transcripts_by_dir_stem
                .entry((parent_low, stem_low))
                .or_insert((a.file_id.clone(), a.filename.clone()));
            continue;
        }
        if matches!(ext_low.as_str(), "txt" | "md") && stem_low.contains("transcript") {
            let trimmed_stem = stem_low
                .replace(".transcript", "")
                .replace("_transcript", "")
                .replace(" transcript", "");
            transcripts_by_dir_stem
                .entry((parent_low, trimmed_stem))
                .or_insert((a.file_id.clone(), a.filename.clone()));
            continue;
        }

        if looks_like_audio_or_video_ext(&ext_low) {
            audio_rows.push(a);
        }
    }

    let mut out = Vec::new();
    for a in audio_rows {
        let parent_low = Path::new(&a.path)
            .parent()
            .map(|p| p.to_string_lossy().replace('\\', "/").to_ascii_lowercase())
            .unwrap_or_default();
        let stem_low = Path::new(&a.filename)
            .file_stem()
            .map(|s| s.to_string_lossy().to_ascii_lowercase())
            .unwrap_or_default();
        let companion = transcripts_by_dir_stem.get(&(parent_low, stem_low));
        out.push(AudioRecordingItem {
            file_id: a.file_id,
            path: a.path,
            filename: a.filename,
            ext: a.ext,
            size_bytes: a.size_bytes,
            mtime: a.mtime,
            drive_label: a.drive_label,
            drive_online: a.drive_online,
            companion_transcript_file_id: companion.map(|c| c.0.clone()),
            companion_transcript_name: companion.map(|c| c.1.clone()),
        });
    }
    out.truncate(80);
    Ok(out)
}

fn load_text_for_row(conn: &Connection, f: &CandidateFileRow) -> Result<Option<String>> {
    let mut st = conn.prepare("SELECT text FROM chunks WHERE file_id = ?1 ORDER BY seq ASC")?;
    let rows = st.query_map(params![f.file_id], |r| r.get::<_, String>(0))?;
    let mut parts = Vec::new();
    for r in rows {
        parts.push(r?);
    }
    if !parts.is_empty() {
        return Ok(Some(parts.join("\n")));
    }

    if f.drive_online
        && f.cloud_state == "local"
        && f.size >= 0
        && f.size <= MAX_PLAIN_READ_BYTES
        && is_extractable_text_ext(&f.ext)
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

struct CollectionsContext {
    names: HashMap<String, String>,
    locations: Vec<(String, String, String)>,
    members: HashMap<String, String>,
}

fn load_collections_context(conn: &Connection) -> Result<CollectionsContext> {
    let mut names = HashMap::new();
    let mut st_c = conn.prepare(
        "SELECT id, name FROM collections WHERE state NOT IN ('left_as_is', 'not_a_project')",
    )?;
    for r in st_c.query_map([], |r| Ok((r.get::<_, String>(0)?, r.get::<_, String>(1)?)))? {
        let (id, name) = r?;
        names.insert(id, name);
    }

    let mut locations = Vec::new();
    let mut st_l = conn.prepare(
        "SELECT cl.collection_id, cl.volume_id, cl.root_path
         FROM collection_locations cl
         JOIN collections c ON c.id = cl.collection_id
         WHERE c.state NOT IN ('left_as_is', 'not_a_project')",
    )?;
    for r in st_l.query_map([], |r| {
        Ok((
            r.get::<_, String>(0)?,
            r.get::<_, String>(1)?,
            r.get::<_, String>(2)?,
        ))
    })? {
        locations.push(r?);
    }

    let mut members = HashMap::new();
    let mut st_m = conn.prepare(
        "SELECT cm.file_id, cm.collection_id
         FROM collection_members cm
         JOIN collections c ON c.id = cm.collection_id
         WHERE c.state NOT IN ('left_as_is', 'not_a_project')",
    )?;
    for r in st_m.query_map([], |r| Ok((r.get::<_, String>(0)?, r.get::<_, String>(1)?)))? {
        let (fid, cid) = r?;
        members.insert(fid, cid);
    }

    Ok(CollectionsContext {
        names,
        locations,
        members,
    })
}

fn resolve_file_collection(
    f: &CandidateFileRow,
    col_locs: &[(String, String, String)],
    members: &HashMap<String, String>,
) -> Option<String> {
    if let Some(cid) = members.get(&f.file_id) {
        return Some(cid.clone());
    }
    let mut best: Option<(usize, &str)> = None;
    for (cid, vid, root) in col_locs {
        if vid == &f.volume_id && path_is_within(&f.path, root, false) {
            let len = root.len();
            if best.is_none_or(|(bl, _)| len > bl) {
                best = Some((len, cid.as_str()));
            }
        }
    }
    best.map(|(_, cid)| cid.to_string())
}

fn load_candidate_files(
    conn: &Connection,
    options: &KnowledgeExtractOptions,
) -> Result<Vec<CandidateFileRow>> {
    let filter_ids: Option<HashSet<String>> = options
        .file_ids
        .as_ref()
        .filter(|v| !v.is_empty())
        .map(|v| v.iter().cloned().collect());

    let mut st = conn.prepare(
        "SELECT f.id, f.volume_id, v.mount_point, f.path, f.name, COALESCE(f.ext, ''),
                f.size, f.mtime, f.cloud_state, COALESCE(v.status = 'online', 0),
                f.sensitivity, m.who
         FROM files f
         JOIN volumes v ON v.id = f.volume_id
         LEFT JOIN file_meta m ON m.file_id = f.id
         WHERE f.deleted_at IS NULL
           AND f.state NOT IN ('quarantine', 'missing')
           AND f.container_file_id IS NULL
           AND COALESCE(v.role, '') != 'backup'
         ORDER BY f.mtime DESC",
    )?;
    let rows = st.query_map([], |r| {
        Ok(CandidateFileRow {
            file_id: r.get(0)?,
            volume_id: r.get(1)?,
            mount_point: r.get(2)?,
            path: r.get(3)?,
            name: r.get(4)?,
            ext: r.get(5)?,
            size: r.get(6)?,
            mtime: r.get(7)?,
            cloud_state: r.get(8)?,
            drive_online: r.get::<_, i64>(9)? != 0,
            sensitivity: r.get(10)?,
            who: r.get(11)?,
        })
    })?;

    let mut out = Vec::new();
    for r in rows {
        let f = r?;
        if let Some(ref fids) = filter_ids
            && !fids.contains(&f.file_id)
        {
            continue;
        }
        if is_excluded_relative_path(&f.path, &f.mount_point) {
            continue;
        }
        out.push(f);
    }
    Ok(out)
}
