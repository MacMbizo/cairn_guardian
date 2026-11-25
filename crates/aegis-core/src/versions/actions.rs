//! `K1` reversible Simulator batches for Semantic Version Tracking:
//! - Archiving superseded revisions into `<Versions>` (`ActionType::Move`, `PolicyContext` checked)
//! - Quarantining exact byte-identical duplicates (`ActionType::Quarantine`, delegating to `propose_dupe_quarantine_pairs`)

use std::collections::HashSet;
use std::path::{Path, PathBuf};

use rusqlite::{OptionalExtension, params};

use crate::assets::recovery::{open_batch_paths, path_key, resolve_unique_dest, same_parent_dir};
use crate::care::path_contains_aegis_internal;
use crate::contracts::{ActionType, ProposedActionInput, RefilePlan, TargetKind};
use crate::db::SqliteStore;
use crate::db::actions::{build_policy_context, propose_batch, propose_dupe_quarantine_pairs};
use crate::error::{AegisError, Result};
use crate::policy::{PolicyAction, PolicyVerdict, evaluate_policy};

/// Agent identifier attached to semantic version archival proposals in `action_batches`.
pub const VERSION_TRACKING_AGENT: &str = "semantic_version_tracking";

struct ArchiveTargetRow {
    from_path: String,
    filename: String,
    state: String,
    deleted_at: Option<i64>,
    atomic_unit_id: Option<String>,
    vol_status: String,
    vol_role: Option<String>,
}

/// Stages a reversible `Move` batch moving selected older revisions into `dest_dir` (usually
/// `<project>\Versions`). Enforces `K1`, `K2`, `K4`, and `PolicyContext`.
pub fn propose_version_archive(
    store: &mut SqliteStore,
    file_ids: &[String],
    dest_dir: &str,
) -> Result<String> {
    if file_ids.is_empty() {
        return Err(AegisError::InvalidInput(
            "Select at least one older version to archive.".into(),
        ));
    }
    let clean_dest = dest_dir.trim();
    if clean_dest.is_empty() {
        return Err(AegisError::InvalidInput(
            "Versions destination folder must not be empty.".into(),
        ));
    }
    let dest_path_base = Path::new(clean_dest);
    if path_contains_aegis_internal(dest_path_base) {
        return Err(AegisError::PermissionDenied(
            "Cannot archive versions into Cairn internal directories.".into(),
        ));
    }

    let policy_ctx = build_policy_context(store)?;
    let busy_paths = open_batch_paths(store.conn())?;
    let mut seen = HashSet::new();
    let mut reserved_dests = HashSet::new();
    let mut actions = Vec::with_capacity(file_ids.len());

    for fid in file_ids {
        if !seen.insert(fid.clone()) {
            continue;
        }
        let row: Option<ArchiveTargetRow> = store
            .conn()
            .query_row(
                "SELECT f.path, f.name, f.state, f.deleted_at, f.atomic_unit_id,
                        COALESCE(v.status, 'offline'), v.role
                 FROM files f
                 JOIN volumes v ON v.id = f.volume_id
                 WHERE f.id = ?1",
                params![fid],
                |r| {
                    Ok(ArchiveTargetRow {
                        from_path: r.get(0)?,
                        filename: r.get(1)?,
                        state: r.get(2)?,
                        deleted_at: r.get(3)?,
                        atomic_unit_id: r.get(4)?,
                        vol_status: r.get(5)?,
                        vol_role: r.get(6)?,
                    })
                },
            )
            .optional()?;

        let Some(target) = row else {
            return Err(AegisError::NotFound(format!("file {fid}")));
        };

        if target.deleted_at.is_some() || target.state == "quarantine" || target.state == "missing" {
            return Err(AegisError::InvalidInput(format!(
                "Cannot archive `{}` because it is deleted or in quarantine (K2).",
                target.filename
            )));
        }
        if target.vol_role.as_deref() == Some("backup") {
            return Err(AegisError::PermissionDenied(
                "Backup drives are read-only and never modified (K4).".into(),
            ));
        }
        if target.vol_status != "online" {
            return Err(AegisError::InvalidInput(format!(
                "Cannot move `{}` because its drive is offline; plug the drive in first.",
                target.filename
            )));
        }
        let from_p = Path::new(&target.from_path);
        if !from_p.is_file() {
            return Err(AegisError::InvalidInput(format!(
                "Source revision is missing on disk: {}",
                target.from_path
            )));
        }
        if busy_paths.contains(&path_key(from_p)) {
            return Err(AegisError::InvalidInput(format!(
                "`{}` already has a pending batch action in the Simulator.",
                target.filename
            )));
        }
        if same_parent_dir(&target.from_path, clean_dest) {
            return Err(AegisError::InvalidInput(format!(
                "`{}` is already in `{clean_dest}`.",
                target.filename
            )));
        }

        let dest_file = resolve_unique_dest(dest_path_base, &target.filename, &mut reserved_dests);
        let dest_str = dest_file.to_string_lossy().to_string();

        let (unit_root, unit_allow_split) = if let Some(ref uid) = target.atomic_unit_id {
            store
                .conn()
                .query_row(
                    "SELECT root_path, allow_split_override FROM atomic_units WHERE id = ?1",
                    params![uid],
                    |r| Ok((Some(PathBuf::from(r.get::<_, String>(0)?)), r.get::<_, i32>(1)? != 0)),
                )
                .optional()?
                .unwrap_or((None, false))
        } else {
            (None, false)
        };

        let pa = PolicyAction {
            action_type: ActionType::Move,
            target_kind: TargetKind::File,
            agent: Some(VERSION_TRACKING_AGENT),
            from_path: from_p,
            to_path: Some(&dest_file),
            confidence: 0.94,
            atomic_unit_root: unit_root.as_deref(),
            unit_allow_split_override: unit_allow_split,
            file_unit_split_override: false,
            is_registered_cache_dir: false,
        };
        if let PolicyVerdict::Block(rule) = evaluate_policy(&pa, &policy_ctx) {
            return Err(AegisError::PermissionDenied(format!(
                "Policy blocked version archive move for `{}`: {rule}",
                target.filename
            )));
        }

        actions.push(ProposedActionInput {
            agent: Some(VERSION_TRACKING_AGENT.to_string()),
            action_type: ActionType::Move,
            target_kind: TargetKind::File,
            file_id: Some(fid.clone()),
            atomic_unit_id: target.atomic_unit_id,
            from_path: target.from_path,
            to_path: Some(dest_str),
            reason: format!("Archive superseded version `{}` to Versions folder", target.filename),
            confidence: 0.94,
            evidence_json: serde_json::json!({
                "source": "semantic_version_archive",
                "dest_dir": clean_dest,
            })
            .to_string(),
        });
    }

    propose_batch(
        store,
        &RefilePlan {
            origin: "semantic_version_archive".to_string(),
            actions,
        },
    )
}

/// Proposes moving exact byte-identical duplicates in a version chain to the 30-day per-volume
/// quarantine (`K1`, `K2`). Rejects any non-identical semantic revision (`FR-004.2`) and delegates
/// to `propose_dupe_quarantine_pairs` so `MIN_COPIES = 2`, `survivor_file_id`, and `guard_test_quarantine` apply.
pub fn propose_version_quarantine_dupes(
    store: &mut SqliteStore,
    keeper_file_id: &str,
    duplicate_file_ids: &[String],
) -> Result<String> {
    let dup_ids: Vec<String> = duplicate_file_ids
        .iter()
        .filter(|id| id.as_str() != keeper_file_id)
        .cloned()
        .collect();
    if dup_ids.is_empty() {
        return Err(AegisError::InvalidInput(
            "Select at least one exact duplicate revision (not the keeper) to quarantine.".into(),
        ));
    }

    let keeper_meta: Option<(Vec<u8>, Option<String>)> = store
        .conn()
        .query_row(
            "SELECT f.blake3, v.role
             FROM files f
             JOIN volumes v ON v.id = f.volume_id
             WHERE f.id = ?1 AND f.deleted_at IS NULL
               AND f.state NOT IN ('quarantine', 'missing')
               AND f.blake3 IS NOT NULL",
            params![keeper_file_id],
            |r| Ok((r.get(0)?, r.get(1)?)),
        )
        .optional()?;
    let Some((keeper_hash, keeper_role)) = keeper_meta else {
        return Err(AegisError::NotFound(format!(
            "live hashed keeper file {keeper_file_id}"
        )));
    };
    if keeper_role.as_deref() == Some("backup") {
        return Err(AegisError::PermissionDenied(
            "Backup drives are read-only and cannot act as source/target (K4).".into(),
        ));
    }

    let busy_paths = open_batch_paths(store.conn())?;
    let mut pairs = Vec::with_capacity(dup_ids.len());
    let mut seen = HashSet::new();

    struct DupMetaRow {
        path: String,
        filename: String,
        blake3: Option<Vec<u8>>,
        vol_status: String,
        vol_role: Option<String>,
    }

    for dup_id in dup_ids {
        if !seen.insert(dup_id.clone()) {
            continue;
        }
        let dup_meta: Option<DupMetaRow> = store
            .conn()
            .query_row(
                "SELECT f.path, f.name, f.blake3, COALESCE(v.status, 'offline'), v.role
                 FROM files f
                 JOIN volumes v ON v.id = f.volume_id
                 WHERE f.id = ?1 AND f.deleted_at IS NULL
                   AND f.state NOT IN ('quarantine', 'missing')",
                params![dup_id],
                |r| {
                    Ok(DupMetaRow {
                        path: r.get(0)?,
                        filename: r.get(1)?,
                        blake3: r.get(2)?,
                        vol_status: r.get(3)?,
                        vol_role: r.get(4)?,
                    })
                },
            )
            .optional()?;
        let Some(DupMetaRow {
            path,
            filename,
            blake3: dup_hash_opt,
            vol_status,
            vol_role,
        }) = dup_meta
        else {
            return Err(AegisError::NotFound(format!("live duplicate file {dup_id}")));
        };
        if vol_role.as_deref() == Some("backup") {
            return Err(AegisError::PermissionDenied(
                "Backup drives are read-only and never modified (K4).".into(),
            ));
        }
        if vol_status != "online" {
            return Err(AegisError::InvalidInput(format!(
                "Cannot quarantine `{filename}` while its drive is offline."
            )));
        }
        if dup_hash_opt.as_deref() != Some(keeper_hash.as_slice()) {
            return Err(AegisError::InvalidInput(format!(
                "Only exact byte-identical duplicates can be quarantined; `{filename}` differs semantically and is never auto-deleted (FR-004.2)."
            )));
        }
        let dup_p = Path::new(&path);
        if !dup_p.is_file() {
            return Err(AegisError::InvalidInput(format!(
                "Duplicate file is missing on disk: {path}"
            )));
        }
        if busy_paths.contains(&path_key(dup_p)) {
            return Err(AegisError::InvalidInput(format!(
                "`{filename}` already has a pending batch action in the Simulator."
            )));
        }
        pairs.push((keeper_file_id.to_string(), dup_id));
    }

    propose_dupe_quarantine_pairs(store, &pairs)
}
