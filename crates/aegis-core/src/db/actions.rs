//! Action batches, Simulator dry-run, Policy-gated Execution, Pre-flight Rollback & Reports (`B4 v2.3.1`, `C1 v1.2`, `C4 v1.1`).
//!
//! Disk/DB consistency model: every disk mutation happens OUTSIDE a database transaction and is
//! followed by ONE short transaction recording exactly that action. If the commit fails, the disk
//! change is undone (compensated) so the index never disagrees with the filesystem because of a
//! rolled-back transaction spanning several already-applied disk operations.

use crate::contracts::{
    ActionItem, ActionStatus, ActionType, BatchStatus, CacheDirEntry, ConflictResolutionChoice,
    DiskRescueReport, DupeCluster, DupeReport, ExecuteResult, FileDetail, HealthBreakdown,
    HealthDimension, ProposedActionInput, RefilePlan, RollbackConflictItem, RollbackConflictKind,
    RollbackResult, Simulation, SimulationItem, TargetKind,
};
use crate::db::SqliteStore;
use crate::db::audit::append_audit_in_tx;
use crate::db::files::{normalize_path, resolve_online_volume, volume_case_sensitive_for};
use crate::error::{AegisError, Result};
use crate::policy::{PolicyAction, PolicyContext, PolicyVerdict, evaluate_policy};
use crate::quarantine::{
    build_quarantine_dest, calculate_reclaimable_bytes, default_ttl_ms, move_to_quarantine_fs,
    space_summary,
};
use crate::scanner::dir_size_and_count;
use crate::scanner::os_adapter::inspect_path_metadata;
use crate::{new_id, now_ms};
use rusqlite::{Connection, OptionalExtension, params};
use std::collections::HashMap;
use std::fs::File;
use std::io::Read;
use std::path::{Path, PathBuf};

fn hash_file_blake3(path: &Path) -> Result<Option<Vec<u8>>> {
    // Online-only cloud files are never opened (that would download them).
    if !path.is_file() || crate::scanner::os_adapter::is_placeholder_now(path) {
        return Ok(None);
    }
    let mut f = File::open(path)?;
    let mut hasher = blake3::Hasher::new();
    let mut buf = [0u8; 65536];
    loop {
        let n = f.read(&mut buf)?;
        if n == 0 {
            break;
        }
        hasher.update(&buf[..n]);
    }
    Ok(Some(hasher.finalize().as_bytes().to_vec()))
}

/// BLAKE3 of the file at `path`, read now; `None` for folders, missing files and online-only
/// cloud files (which are never opened).
pub fn file_blake3(path: &Path) -> Result<Option<Vec<u8>>> {
    hash_file_blake3(path)
}

/// SQL condition (and its two parameters) for catalogued files strictly inside `dir`.
fn files_inside_sql(dir: &str, first_param: usize) -> (String, String, i64) {
    let prefix = format!("{}{}", dir.trim_end_matches(['\\', '/']), std::path::MAIN_SEPARATOR);
    let n = prefix.chars().count() as i64;
    (
        format!("deleted_at IS NULL AND lower(substr(path, 1, ?{})) = lower(?{})", first_param + 1, first_param),
        prefix,
        n,
    )
}

/// Removes the empty folders above `gone` (a path just moved away), innermost first, stopping
/// at the first folder that isn't empty or that also holds `other`.
fn remove_empty_ancestors(gone: &Path, other: &Path) {
    let mut cur = gone.parent();
    while let Some(d) = cur {
        if other.starts_with(d) {
            break;
        }
        let empty = std::fs::read_dir(d).map(|mut it| it.next().is_none()).unwrap_or(false);
        if !empty || std::fs::remove_dir(d).is_err() {
            break;
        }
        cur = d.parent();
    }
}

/// Re-points every catalogued file inside folder `from` to the same place inside `to` (a whole
/// folder that moved), with a `file_history` row per file when `batch_id` is given.
fn rebase_children(
    tx: &Connection,
    volume_id: Option<&str>,
    from: &str,
    to: &str,
    case_sensitive: bool,
    batch_id: Option<&str>,
    now: i64,
) -> Result<()> {
    let (inside, prefix, n) = files_inside_sql(from, 1);
    let rows: Vec<(String, String)> = if let Some(vid) = volume_id {
        let mut stmt = tx.prepare(&format!("SELECT id, path FROM files WHERE {inside} AND volume_id = ?3"))?;
        stmt.query_map(params![prefix, n, vid], |r| Ok((r.get(0)?, r.get(1)?)))?.collect::<rusqlite::Result<_>>()?
    } else {
        let mut stmt = tx.prepare(&format!("SELECT id, path FROM files WHERE {inside}"))?;
        stmt.query_map(params![prefix, n], |r| Ok((r.get(0)?, r.get(1)?)))?.collect::<rusqlite::Result<_>>()?
    };
    let to = to.trim_end_matches(['\\', '/']);
    for (id, old) in rows {
        let rest: String = old.chars().skip(n as usize).collect();
        let new = format!("{to}{}{rest}", std::path::MAIN_SEPARATOR);
        let (disp, ph) = normalize_path(Path::new(&new), case_sensitive);
        tx.execute(
            "UPDATE files SET path = ?1, path_hash = ?2, updated_at = ?3 WHERE id = ?4",
            params![disp, ph.as_slice(), now, id],
        )?;
        if let Some(b) = batch_id {
            tx.execute(
                "INSERT INTO file_history (id, file_id, old_path, new_path, batch_id, changed_at) VALUES (?1, ?2, ?3, ?4, ?5, ?6)",
                params![new_id(), id, old, disp, b, now],
            )?;
        }
    }
    // Cache folders inside it (`node_modules`, `.venv`) are recorded whole: they follow too.
    let from_trimmed = from.trim_end_matches(['\\', '/']);
    let caches: Vec<(String, String, String)> = if let Some(vid) = volume_id {
        let mut stmt = tx.prepare(
            "SELECT id, volume_id, path FROM cache_dirs
             WHERE volume_id = ?4 AND (lower(substr(path, 1, ?2)) = lower(?1) OR lower(rtrim(path, '\\/')) = lower(?3))",
        )?;
        stmt.query_map(params![prefix, n, from_trimmed, vid], |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?)))?
            .collect::<rusqlite::Result<Vec<_>>>()?
    } else {
        let mut stmt = tx.prepare(
            "SELECT id, volume_id, path FROM cache_dirs
             WHERE lower(substr(path, 1, ?2)) = lower(?1) OR lower(rtrim(path, '\\/')) = lower(?3)",
        )?;
        stmt.query_map(params![prefix, n, from_trimmed], |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?)))?
            .collect::<rusqlite::Result<Vec<_>>>()?
    };
    for (id, vid, old) in caches {
        let rest = if old.trim_end_matches(['\\', '/']).eq_ignore_ascii_case(from_trimmed) {
            String::new()
        } else {
            format!("{}{}", std::path::MAIN_SEPARATOR, old.chars().skip(n as usize).collect::<String>())
        };
        let new = format!("{to}{rest}");
        let (disp, ph) = normalize_path(Path::new(&new), case_sensitive);
        tx.execute(
            "DELETE FROM cache_dirs WHERE volume_id = ?1 AND path_hash = ?2 AND state = 'active' AND id != ?3",
            params![vid, ph.as_slice(), id],
        )?;
        tx.execute(
            "UPDATE cache_dirs SET path = ?1, path_hash = ?2 WHERE id = ?3",
            params![disp, ph.as_slice(), id],
        )?;
    }
    // Build units (a repo, a package) inside the folder keep protecting it at its new place.
    let from_key = from_trimmed.to_lowercase();
    let units: Vec<(String, String, String)> = if let Some(vid) = volume_id {
        let mut stmt = tx.prepare("SELECT id, volume_id, root_path FROM atomic_units WHERE volume_id = ?1")?;
        stmt.query_map([vid], |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?)))?.collect::<rusqlite::Result<Vec<_>>>()?
    } else {
        let mut stmt = tx.prepare("SELECT id, volume_id, root_path FROM atomic_units")?;
        stmt.query_map([], |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?)))?.collect::<rusqlite::Result<Vec<_>>>()?
    };
    for (id, vid, root) in units {
        let lower = root.to_lowercase();
        let rest = if lower == from_key {
            String::new()
        } else if lower.starts_with(&prefix.to_lowercase()) {
            format!("{}{}", std::path::MAIN_SEPARATOR, root.chars().skip(n as usize).collect::<String>())
        } else {
            continue;
        };
        let new_root = format!("{to}{rest}");
        let stale: Vec<String> = {
            let mut st = tx.prepare(
                "SELECT id FROM atomic_units WHERE volume_id = ?1 AND root_path = ?2 AND id != ?3",
            )?;
            st.query_map(params![vid, new_root, id], |r| r.get(0))?.collect::<rusqlite::Result<_>>()?
        };
        for sid in stale {
            tx.execute("UPDATE files SET atomic_unit_id = NULL WHERE atomic_unit_id = ?1", [&sid])?;
            tx.execute("UPDATE cache_dirs SET atomic_unit_id = NULL WHERE atomic_unit_id = ?1", [&sid])?;
            tx.execute("UPDATE actions SET atomic_unit_id = ?2 WHERE atomic_unit_id = ?1", params![sid, id])?;
            tx.execute("UPDATE quarantine_entries SET atomic_unit_id = ?2 WHERE atomic_unit_id = ?1", params![sid, id])?;
            tx.execute("DELETE FROM atomic_units WHERE id = ?1", [&sid])?;
        }
        tx.execute(
            "UPDATE atomic_units SET root_path = ?1, updated_at = ?2 WHERE id = ?3",
            params![new_root, now, id],
        )?;
    }
    Ok(())
}

/// The evidence of a project-plan folder step (plan 2026-11-23-project-plans): what the folder
/// looked like when the plan was made, and for a quarantine, the home it must be proven against
/// and where it goes instead if it can't be.
#[derive(Default)]
struct PlanEvidence {
    expect: Option<(i64, i64)>,
    verify_against: Option<String>,
    fallback: Option<String>,
    exact_dest: bool,
}

fn plan_evidence(evidence_json: &str) -> PlanEvidence {
    let v = serde_json::from_str::<serde_json::Value>(evidence_json).unwrap_or_default();
    PlanEvidence {
        expect: v.get("expect").and_then(|e| Some((e.get("files")?.as_i64()?, e.get("newest")?.as_i64()?))),
        verify_against: v.get("verify_against").and_then(|x| x.as_str()).map(str::to_string),
        fallback: v.get("fallback").and_then(|x| x.as_str()).map(str::to_string),
        exact_dest: v.get("exact_dest").and_then(|x| x.as_bool()).unwrap_or(false),
    }
}

fn capture_state_json(path: &Path) -> String {
    match inspect_path_metadata(path) {
        Ok(Some(st)) => serde_json::json!({
            "existed": true,
            "is_dir": st.is_dir,
            "size": st.size,
            "mtime": st.mtime,
            "nlink": st.nlink,
            "cloud_state": st.cloud_state
        })
        .to_string(),
        _ => serde_json::json!({ "existed": false }).to_string(),
    }
}

fn display_string(path: &Path) -> String {
    let raw = path.to_string_lossy();
    raw.strip_prefix(r"\\?\").unwrap_or(&raw).to_string()
}

/// `a` and `b` name the same entry and differ only in letter case: a case-only rename, which
/// Windows sees as "already exists".
fn case_only(a: &Path, b: &Path) -> bool {
    a != b && a.to_string_lossy().eq_ignore_ascii_case(&b.to_string_lossy())
}

/// `fs::rename`, going through a temporary name for a case-only rename.
fn rename_entry(from: &Path, to: &Path) -> std::io::Result<()> {
    if case_only(from, to) {
        let name = from.file_name().map(|n| n.to_string_lossy().to_string()).unwrap_or_default();
        let tmp = from.with_file_name(format!("{name}.aegis-case"));
        std::fs::rename(from, &tmp)?;
        return std::fs::rename(&tmp, to);
    }
    std::fs::rename(from, to)
}

fn non_overwriting_dest(path: &Path) -> (PathBuf, bool) {
    if !path.exists() {
        return (path.to_path_buf(), false);
    }
    let stem = path
        .file_stem()
        .map(|s| s.to_string_lossy().to_string())
        .unwrap_or_else(|| "file".to_string());
    let ext = path.extension().map(|e| e.to_string_lossy().to_string());
    let mut n = 2usize; // spec §6: the first copy is " (2)", like Windows Explorer
    loop {
        let candidate_name = match &ext {
            Some(e) => format!("{stem} ({n}).{e}"),
            None => format!("{stem} ({n})"),
        };
        let candidate = path.with_file_name(candidate_name);
        if !candidate.exists() {
            return (candidate, true);
        }
        n += 1;
    }
}

pub fn build_policy_context(store: &SqliteStore) -> Result<PolicyContext> {
    let volumes = store.list_volumes()?;
    let active_roots = store
        .list_roots()?
        .into_iter()
        .filter(|r| r.enabled)
        .map(|r| PathBuf::from(r.path))
        // A drive Aegis has fully catalogued is managed as a whole (spec §7.8.1), while online.
        .chain(
            volumes
                .iter()
                .filter(|v| {
                    v.status == "online"
                        && v.last_catalog_complete_at.is_some()
                        && (v.passport_id.is_some() || v.id.starts_with("fp-"))
                })
                .map(|v| PathBuf::from(&v.mount_point)),
        )
        .collect();
    let volume_quarantine_dirs = volumes
        .into_iter()
        .filter_map(|v| v.quarantine_dir.map(PathBuf::from))
        .collect();
    Ok(PolicyContext {
        mvp_mode: true,
        active_roots,
        volume_quarantine_dirs,
        dials: HashMap::new(),
        care_cleanup_roots: crate::care::allowed_cleanup_roots(),
        care_disposable_dirs: crate::care::allowed_disposable_cache_dirs(),
        care_leftover_parents: crate::care::allowed_leftover_parents(),
    })
}

#[derive(Clone)]
struct DbActionRow {
    id: String,
    batch_id: String,
    agent: Option<String>,
    action_type: ActionType,
    target_kind: TargetKind,
    file_id: Option<String>,
    atomic_unit_id: Option<String>,
    from_path: String,
    to_path: Option<String>,
    post_blake3: Option<Vec<u8>>,
    reason: String,
    confidence: f64,
    status: ActionStatus,
    rollback_conflict: Option<RollbackConflictKind>,
    error_message: Option<String>,
    /// For duplicate quarantines: the copy that must still exist, verified at execute time.
    survivor_file_id: Option<String>,
    evidence_json: String,
}

impl DbActionRow {
    fn to_item(&self) -> ActionItem {
        ActionItem {
            id: self.id.clone(),
            batch_id: self.batch_id.clone(),
            agent: self.agent.clone(),
            action_type: self.action_type,
            target_kind: self.target_kind,
            file_id: self.file_id.clone(),
            atomic_unit_id: self.atomic_unit_id.clone(),
            from_path: self.from_path.clone(),
            to_path: self.to_path.clone(),
            reason: self.reason.clone(),
            confidence: self.confidence,
            status: self.status,
            rollback_conflict: self.rollback_conflict,
            error_message: self.error_message.clone(),
        }
    }
}

fn load_batch_actions(conn: &Connection, batch_id: &str) -> Result<Vec<DbActionRow>> {
    let mut stmt = conn.prepare(
        "SELECT id, batch_id, agent, action_type, target_kind, file_id, atomic_unit_id,
                from_path, to_path, post_blake3, reason, confidence,
                status, rollback_conflict, error_message, survivor_file_id, evidence_json
         FROM actions WHERE batch_id = ?1 ORDER BY step ASC, created_at ASC, id ASC",
    )?;
    let rows = stmt.query_map(params![batch_id], |r| {
        let at_str: String = r.get(3)?;
        let tk_str: String = r.get(4)?;
        let st_str: String = r.get(12)?;
        let rc_str: Option<String> = r.get(13)?;
        Ok(DbActionRow {
            id: r.get(0)?,
            batch_id: r.get(1)?,
            agent: r.get(2)?,
            action_type: ActionType::from_db(&at_str).unwrap_or(ActionType::Move),
            target_kind: TargetKind::from_db(&tk_str),
            file_id: r.get(5)?,
            atomic_unit_id: r.get(6)?,
            from_path: r.get(7)?,
            to_path: r.get(8)?,
            post_blake3: r.get(9)?,
            reason: r.get(10)?,
            confidence: r.get(11)?,
            status: ActionStatus::from_db(&st_str),
            rollback_conflict: rc_str.as_deref().and_then(RollbackConflictKind::from_db),
            error_message: r.get(14)?,
            survivor_file_id: r.get(15)?,
            evidence_json: r.get(16)?,
        })
    })?;
    let mut out = Vec::new();
    for r in rows {
        out.push(r?);
    }
    Ok(out)
}

struct PolicyTargetMeta {
    atomic_unit_root: Option<PathBuf>,
    unit_allow_split: bool,
    file_unit_split: bool,
    is_cache_dir: bool,
    cache_size_bytes: Option<i64>,
}

fn resolve_policy_target_meta(conn: &Connection, row: &DbActionRow) -> Result<PolicyTargetMeta> {
    let mut unit_id = row.atomic_unit_id.clone();
    let mut file_unit_split = false;

    if let Some(ref fid) = row.file_id
        && let Some((au_id, split)) = conn
            .query_row(
                "SELECT atomic_unit_id, unit_split_override FROM files WHERE id = ?1",
                params![fid],
                |r| Ok((r.get::<_, Option<String>>(0)?, r.get::<_, i32>(1)? != 0)),
            )
            .optional()?
    {
        if unit_id.is_none() {
            unit_id = au_id;
        }
        file_unit_split = split;
    }

    let mut is_cache_dir = false;
    let mut cache_size_bytes = None;
    if row.target_kind == TargetKind::Dir {
        let vol = resolve_online_volume(conn, Path::new(&row.from_path))?;
        let cs = vol.as_ref().map(|v| v.2).unwrap_or(false);
        let (_, ph) = normalize_path(Path::new(&row.from_path), cs);
        let cache_row = if let Some((vid, _, _)) = &vol {
            conn.query_row(
                "SELECT atomic_unit_id, size_bytes FROM cache_dirs WHERE volume_id = ?1 AND path_hash = ?2 AND state = 'active'",
                params![vid, ph.as_slice()],
                |r| Ok((r.get::<_, Option<String>>(0)?, r.get::<_, i64>(1)?)),
            )
            .optional()?
        } else {
            conn.query_row(
                "SELECT atomic_unit_id, size_bytes FROM cache_dirs WHERE path_hash = ?1 AND state = 'active'",
                params![ph.as_slice()],
                |r| Ok((r.get::<_, Option<String>>(0)?, r.get::<_, i64>(1)?)),
            )
            .optional()?
        };
        if let Some((au_id, size)) = cache_row {
            is_cache_dir = true;
            cache_size_bytes = Some(size);
            if unit_id.is_none() {
                unit_id = au_id;
            }
        }
    }

    let (atomic_unit_root, unit_allow_split) = if let Some(ref au) = unit_id {
        conn.query_row(
            "SELECT root_path, allow_split_override FROM atomic_units WHERE id = ?1",
            params![au],
            |r| Ok((Some(PathBuf::from(r.get::<_, String>(0)?)), r.get::<_, i32>(1)? != 0)),
        )
        .optional()?
        .unwrap_or((None, false))
    } else {
        (None, false)
    };

    Ok(PolicyTargetMeta {
        atomic_unit_root,
        unit_allow_split,
        file_unit_split,
        is_cache_dir,
        cache_size_bytes,
    })
}

fn resolve_volume_for_action(conn: &Connection, from_path: &Path) -> Result<(String, String, bool)> {
    resolve_online_volume(conn, from_path)?.ok_or_else(|| {
        AegisError::QuarantineUnavailable(format!(
            "No online volume matched {}",
            from_path.display()
        ))
    })
}

/// Ensures an in-memory test database never writes into a real volume's `.aegis-quarantine`.
fn guard_test_quarantine(in_memory: bool, q_path: &Path) -> Result<()> {
    if !in_memory {
        return Ok(());
    }
    let tmp = std::env::temp_dir();
    let q_disp = normalize_path(q_path, false).0;
    let tmp_disp = normalize_path(&tmp, false).0;
    let within_raw = crate::db::files::path_is_within(&q_disp, &tmp_disp, false);
    let within_canon = std::fs::canonicalize(&tmp).ok().is_some_and(|tc| {
        let tc_disp = normalize_path(&tc, false).0;
        let resolved_q = std::fs::canonicalize(q_path).ok().or_else(|| {
            let mut cur = q_path.parent();
            let mut tail = std::collections::VecDeque::new();
            if let Some(n) = q_path.file_name() {
                tail.push_front(n.to_os_string());
            }
            while let Some(p) = cur {
                if let Ok(c) = std::fs::canonicalize(p) {
                    let mut out = c;
                    for seg in tail {
                        out.push(seg);
                    }
                    return Some(out);
                }
                if let Some(n) = p.file_name() {
                    tail.push_front(n.to_os_string());
                }
                cur = p.parent();
            }
            None
        });
        resolved_q.is_some_and(|qc| crate::db::files::path_is_within(&normalize_path(&qc, false).0, &tc_disp, false))
    });
    if !(within_raw || within_canon) {
        return Err(AegisError::QuarantineUnavailable(format!(
            "in-memory test store refused to quarantine outside TempDir: {}",
            q_path.display()
        )));
    }
    Ok(())
}

fn policy_action_for<'a>(row: &'a DbActionRow, meta: &'a PolicyTargetMeta) -> PolicyAction<'a> {
    PolicyAction {
        action_type: row.action_type,
        target_kind: row.target_kind,
        agent: row.agent.as_deref(),
        from_path: Path::new(&row.from_path),
        to_path: row.to_path.as_deref().map(Path::new),
        confidence: row.confidence,
        atomic_unit_root: meta.atomic_unit_root.as_deref(),
        unit_allow_split_override: meta.unit_allow_split,
        file_unit_split_override: meta.file_unit_split,
        is_registered_cache_dir: meta.is_cache_dir,
    }
}

/// Proposes an `action_batches` + `actions` draft (`C1` `actions.propose`).
pub fn propose_batch(store: &mut SqliteStore, plan: &RefilePlan) -> Result<String> {
    if plan.actions.is_empty() {
        return Err(AegisError::InvalidInput("Plan contains no actions".into()));
    }
    let batch_id = new_id();
    let now = now_ms();
    let in_memory = store.is_in_memory();

    store.with_full_sync_tx(|tx| {
        tx.execute(
            "INSERT INTO action_batches (id, origin, status, total_actions, created_at)
             VALUES (?1, ?2, 'draft', ?3, ?4)",
            params![batch_id, plan.origin, plan.actions.len() as i64, now],
        )?;

        for item in &plan.actions {
            let action_id = new_id();
            let effective_to = if item.action_type == ActionType::Quarantine && item.to_path.is_none()
            {
                let (_, qdir, _) = resolve_volume_for_action(tx, Path::new(&item.from_path))?;
                guard_test_quarantine(in_memory, Path::new(&qdir))?;
                Some(
                    build_quarantine_dest(
                        Path::new(&qdir),
                        &batch_id,
                        &action_id,
                        Path::new(&item.from_path),
                    )
                    .to_string_lossy()
                    .to_string(),
                )
            } else {
                if item.action_type == ActionType::Quarantine
                    && let Some(ref explicit_q) = item.to_path
                {
                    guard_test_quarantine(in_memory, Path::new(explicit_q))?;
                }
                item.to_path.clone()
            };

            // Steps that must run in order carry `"step": n` in their evidence.
            let step = serde_json::from_str::<serde_json::Value>(&item.evidence_json)
                .ok()
                .and_then(|v| v.get("step").and_then(|s| s.as_i64()))
                .unwrap_or(0);
            tx.execute(
                "INSERT INTO actions (
                    id, batch_id, agent, action_type, target_kind, file_id, atomic_unit_id,
                    from_path, to_path, reason, confidence, evidence_json, status, created_at, step
                 ) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12, 'proposed', ?13, ?14)",
                params![
                    action_id,
                    batch_id,
                    item.agent,
                    item.action_type.as_str(),
                    item.target_kind.as_str(),
                    item.file_id,
                    item.atomic_unit_id,
                    item.from_path,
                    effective_to,
                    item.reason,
                    item.confidence,
                    item.evidence_json,
                    now,
                    step
                ],
            )?;
        }
        Ok(())
    })?;

    Ok(batch_id)
}

/// Proposes quarantining every non-keeper member of an exact-duplicate cluster.
pub fn propose_dupe_quarantine(
    store: &mut SqliteStore,
    keeper_file_id: &str,
    member_file_ids: &[String],
) -> Result<String> {
    let pairs: Vec<(String, String)> = member_file_ids
        .iter()
        .filter(|f| f.as_str() != keeper_file_id)
        .map(|m| (keeper_file_id.to_string(), m.clone()))
        .collect();
    propose_dupe_quarantine_pairs(store, &pairs)
}

/// Proposes one quarantine per `(survivor, member)` pair in a single batch. Each action records
/// its survivor, re-verified at execute time. Refused (`min_copies`) when any file would be left
/// with fewer than [`crate::dupes::MIN_COPIES`] copies, counting quarantines already pending.
pub fn propose_dupe_quarantine_pairs(
    store: &mut SqliteStore,
    pairs: &[(String, String)],
) -> Result<String> {
    if pairs.is_empty() {
        return Err(AegisError::InvalidInput("Nothing to remove".into()));
    }
    let live_hash = |store: &SqliteStore, id: &str| -> Result<Vec<u8>> {
        store
            .conn()
            .query_row(
                "SELECT blake3 FROM files WHERE id = ?1 AND deleted_at IS NULL AND state NOT IN ('quarantine','missing')",
                params![id],
                |r| r.get::<_, Option<Vec<u8>>>(0),
            )
            .optional()?
            .flatten()
            .ok_or_else(|| AegisError::NotFound(format!("file {id} has no fingerprint")))
    };
    let mut removing: HashMap<Vec<u8>, i64> = HashMap::new();
    for (keeper, member) in pairs {
        let h = live_hash(store, member)?;
        if live_hash(store, keeper)? != h {
            return Err(AegisError::InvalidInput(format!("{member} is not a copy of {keeper}")));
        }
        *removing.entry(h).or_insert(0) += 1;
    }
    for (h, n) in &removing {
        let pending: i64 = store.conn().query_row(
            "SELECT COUNT(DISTINCT a.file_id) FROM actions a JOIN files f ON f.id = a.file_id
             WHERE a.action_type = 'quarantine' AND a.status IN ('proposed','approved') AND f.blake3 = ?1",
            params![h],
            |r| r.get(0),
        )?;
        let copies = crate::dupes::copy_count(store.conn(), h)?;
        if copies - pending - n < crate::dupes::MIN_COPIES {
            return Err(AegisError::PolicyBlocked {
                rule: "min_copies".into(),
                detail: format!(
                    "Aegis keeps at least {} copies of every file, and only {copies} exist. Make this the backup copy instead of deleting it.",
                    crate::dupes::MIN_COPIES
                ),
            });
        }
    }
    let mut actions = Vec::new();
    for (keeper, fid) in pairs {
        let (path, atomic_unit_id): (String, Option<String>) = store
            .conn()
            .query_row(
                "SELECT path, atomic_unit_id FROM files
                 WHERE id = ?1 AND deleted_at IS NULL AND state != 'quarantine'",
                params![fid],
                |r| Ok((r.get(0)?, r.get(1)?)),
            )
            .optional()?
            .ok_or_else(|| AegisError::NotFound(format!("file {fid}")))?;
        actions.push(ProposedActionInput {
            agent: Some("Housekeeper".into()),
            action_type: ActionType::Quarantine,
            target_kind: TargetKind::File,
            file_id: Some(fid.clone()),
            atomic_unit_id,
            from_path: path,
            to_path: None,
            reason: "Exact duplicate (identical BLAKE3) of the kept copy".into(),
            confidence: 0.99,
            evidence_json: serde_json::json!([{ "keeper_file_id": keeper }]).to_string(),
        });
    }
    let batch = propose_batch(
        store,
        &RefilePlan {
            origin: "dedupe".into(),
            actions,
        },
    )?;
    for (keeper, member) in pairs {
        store.conn().execute(
            "UPDATE actions SET survivor_file_id = ?3 WHERE batch_id = ?1 AND file_id = ?2",
            params![batch, member, keeper],
        )?;
    }
    Ok(batch)
}

/// Dry-run Simulator evaluating collisions and `C4 v1.1` Policy verdicts (`C1` `actions.simulate`).
pub fn simulate_batch(store: &SqliteStore, batch_id: &str) -> Result<Simulation> {
    let ctx = build_policy_context(store)?;
    let rows = load_batch_actions(store.conn(), batch_id)?;
    if rows.is_empty() {
        return Err(AegisError::NotFound(format!("batch {batch_id}")));
    }

    let mut items = Vec::with_capacity(rows.len());
    let mut blocked_count = 0usize;
    let mut review_count = 0usize;
    let mut ready_count = 0usize;

    for row in rows {
        let meta = resolve_policy_target_meta(store.conn(), &row)?;
        let verdict = evaluate_policy(&policy_action_for(&row, &meta), &ctx);
        let policy_allowed = verdict.is_allowed_to_stage();
        let atomic_unit_protected = verdict == PolicyVerdict::Block("atomic_unit_boundary");

        match verdict {
            PolicyVerdict::Block(_) => blocked_count += 1,
            PolicyVerdict::HoldLowConfidence => review_count += 1,
            PolicyVerdict::RequireApproval(_) | PolicyVerdict::RequireExplicitPermanentConfirm => {
                if row.confidence >= 0.90 {
                    ready_count += 1;
                } else {
                    review_count += 1;
                }
            }
            PolicyVerdict::AutoEligible => ready_count += 1,
        }

        let (effective_to_path, collision_renamed) = match row.action_type {
            ActionType::Move | ActionType::Copy | ActionType::Rename => match &row.to_path {
                Some(dest) => {
                    let (eff, renamed) = non_overwriting_dest(Path::new(dest));
                    (Some(eff.to_string_lossy().to_string()), renamed)
                }
                None => (None, false),
            },
            _ => (row.to_path.clone(), false),
        };

        items.push(SimulationItem {
            action_id: row.id,
            from_path: row.from_path,
            to_path: row.to_path,
            effective_to_path,
            collision_renamed,
            confidence: row.confidence,
            reason: row.reason,
            policy_verdict: verdict.code().to_string(),
            policy_allowed,
            atomic_unit_protected,
        });
    }

    Ok(Simulation {
        batch_id: batch_id.to_string(),
        items,
        blocked_count,
        review_count,
        ready_count,
    })
}

pub fn approve_batch(
    store: &mut SqliteStore,
    batch_id: &str,
    selection: Option<&[String]>,
) -> Result<()> {
    store.with_full_sync_tx(|tx| {
        let mut changed = 0usize;
        match selection {
            Some(ids) => {
                for id in ids {
                    changed += tx.execute(
                        "UPDATE actions SET status = 'approved'
                         WHERE batch_id = ?1 AND id = ?2 AND status = 'proposed'",
                        params![batch_id, id],
                    )?;
                }
            }
            None => {
                changed += tx.execute(
                    "UPDATE actions SET status = 'approved'
                     WHERE batch_id = ?1 AND status = 'proposed'",
                    params![batch_id],
                )?;
            }
        }
        if changed == 0 {
            return Err(AegisError::InvalidInput(format!(
                "Batch {batch_id} has no proposed actions to approve"
            )));
        }
        tx.execute(
            "UPDATE action_batches SET status = 'approved' WHERE id = ?1 AND status = 'draft'",
            params![batch_id],
        )?;
        append_audit_in_tx(
            tx,
            "user",
            "actions.approve",
            batch_id,
            &serde_json::json!({ "approved": changed }).to_string(),
        )?;
        Ok(())
    })
}

pub fn reject_batch(store: &mut SqliteStore, batch_id: &str, reason: Option<&str>) -> Result<()> {
    let msg = reason.unwrap_or("Rejected by user");
    store.with_full_sync_tx(|tx| {
        let changed = tx.execute(
            "UPDATE actions SET status = 'rejected', error_message = ?1
             WHERE batch_id = ?2 AND status IN ('proposed','approved')",
            params![msg, batch_id],
        )?;
        if changed == 0 {
            return Err(AegisError::InvalidInput(format!(
                "Batch {batch_id} has no pending actions to reject"
            )));
        }
        // Only a batch that never touched disk becomes 'failed'; executed history is preserved.
        tx.execute(
            "UPDATE action_batches SET status = 'failed'
             WHERE id = ?1 AND NOT EXISTS (
                SELECT 1 FROM actions WHERE batch_id = ?1 AND executed_at IS NOT NULL)",
            params![batch_id],
        )?;
        append_audit_in_tx(tx, "user", "actions.reject", batch_id, msg)?;
        Ok(())
    })
}

/// Atomically writes `content` to `path` via sibling temp file + `sync_all()` + replace (`MoveFileExW` on Windows).
fn atomic_write_text_file(path: &Path, content: &[u8]) -> std::io::Result<()> {
    use std::io::Write;
    let parent = path.parent().ok_or_else(|| {
        std::io::Error::new(std::io::ErrorKind::InvalidInput, "Missing parent directory")
    })?;
    std::fs::create_dir_all(parent)?;
    let fname = path
        .file_name()
        .map(|s| s.to_string_lossy().to_string())
        .unwrap_or_else(|| "file".to_string());
    let tmp = parent.join(format!(".{fname}.aegis-tmp-{}", new_id()));
    {
        let mut out = std::fs::File::create(&tmp)?;
        out.write_all(content)?;
        out.sync_all()?;
    }
    #[cfg(windows)]
    {
        use std::os::windows::ffi::OsStrExt;
        use windows_sys::Win32::Storage::FileSystem::{
            MOVEFILE_REPLACE_EXISTING, MOVEFILE_WRITE_THROUGH, MoveFileExW,
        };
        let src_w: Vec<u16> = tmp.as_os_str().encode_wide().chain(Some(0)).collect();
        let dst_w: Vec<u16> = path.as_os_str().encode_wide().chain(Some(0)).collect();
        // SAFETY: Null-terminated UTF-16 buffers outlive synchronous Win32 MoveFileExW.
        let ok = unsafe {
            MoveFileExW(
                src_w.as_ptr(),
                dst_w.as_ptr(),
                MOVEFILE_REPLACE_EXISTING | MOVEFILE_WRITE_THROUGH,
            )
        };
        if ok == 0 {
            let err = std::io::Error::last_os_error();
            let _ = std::fs::remove_file(&tmp);
            return Err(err);
        }
    }
    #[cfg(not(windows))]
    {
        if let Err(e) = std::fs::rename(&tmp, path) {
            let _ = std::fs::remove_file(&tmp);
            return Err(e);
        }
    }
    Ok(())
}

/// A disk mutation that has been applied and can be undone if its DB commit fails.
enum FsOutcome {
    Moved { src: PathBuf, dest: PathBuf },
    Quarantined { src: PathBuf, dest: PathBuf, size_bytes: i64 },
    Copied { dest: PathBuf },
    PatchedText { path: PathBuf, original_bytes: Vec<u8> },
    MetadataOnly,
}

impl FsOutcome {
    fn dest(&self) -> Option<&Path> {
        match self {
            Self::Moved { dest, .. } | Self::Quarantined { dest, .. } | Self::Copied { dest } => {
                Some(dest)
            }
            Self::PatchedText { path, .. } => Some(path),
            Self::MetadataOnly => None,
        }
    }

    fn undo(&self) -> std::io::Result<()> {
        match self {
            Self::Moved { src, dest } | Self::Quarantined { src, dest, .. } => {
                std::fs::rename(dest, src)
            }
            Self::Copied { dest } => std::fs::remove_file(dest),
            Self::PatchedText { path, original_bytes } => {
                atomic_write_text_file(path, original_bytes)
            }
            Self::MetadataOnly => Ok(()),
        }
    }
}

fn perform_execute_fs(
    row: &DbActionRow,
    meta: &PolicyTargetMeta,
    patch_payload: Option<(Vec<u8>, Vec<u8>)>,
) -> Result<FsOutcome> {
    let from = Path::new(&row.from_path);
    match row.action_type {
        ActionType::Quarantine => {
            let dest = PathBuf::from(row.to_path.as_deref().ok_or_else(|| {
                AegisError::QuarantineUnavailable("Missing quarantine to_path".into())
            })?);
            let size_bytes = if from.is_dir() {
                meta.cache_size_bytes
                    .filter(|s| *s > 0)
                    .unwrap_or_else(|| dir_size_and_count(from).0)
            } else {
                std::fs::metadata(from).map(|m| m.len() as i64).unwrap_or(0)
            };
            move_to_quarantine_fs(from, &dest)?;
            Ok(FsOutcome::Quarantined {
                src: from.to_path_buf(),
                dest,
                size_bytes,
            })
        }
        ActionType::Move | ActionType::Rename => {
            let raw_to = row.to_path.as_deref().ok_or_else(|| {
                AegisError::InvalidInput("Missing to_path for move/rename".into())
            })?;
            if !from.exists() {
                return Err(AegisError::NotFound(row.from_path.clone()));
            }
            // A project plan names its folders exactly: never " (2)", stop instead.
            if plan_evidence(&row.evidence_json).exact_dest && Path::new(raw_to).exists() && !case_only(from, Path::new(raw_to)) {
                return Err(AegisError::InvalidInput(format!(
                    "{raw_to} appeared in the way. Aegis will make a fresh plan."
                )));
            }
            let (eff_to, _) = if case_only(from, Path::new(raw_to)) {
                (PathBuf::from(raw_to), false)
            } else {
                non_overwriting_dest(Path::new(raw_to))
            };
            if let Some(parent) = eff_to.parent() {
                std::fs::create_dir_all(parent)?;
            }
            rename_entry(from, &eff_to)?;
            Ok(FsOutcome::Moved {
                src: from.to_path_buf(),
                dest: eff_to,
            })
        }
        ActionType::Copy => {
            let raw_to = row
                .to_path
                .as_deref()
                .ok_or_else(|| AegisError::InvalidInput("Missing to_path for copy".into()))?;
            let (eff_to, _) = non_overwriting_dest(Path::new(raw_to));
            if let Some(parent) = eff_to.parent() {
                std::fs::create_dir_all(parent)?;
            }
            let partial = PathBuf::from(format!("{}.aegis-partial", eff_to.display()));
            let copied = (|| -> Result<Vec<u8>> {
                use std::io::{Read, Write};
                let mut src = std::fs::File::open(from)?;
                let mut out = std::fs::File::create(&partial)?;
                let mut h = blake3::Hasher::new();
                let mut buf = vec![0u8; 256 * 1024];
                loop {
                    let n = src.read(&mut buf)?;
                    if n == 0 {
                        break;
                    }
                    h.update(&buf[..n]);
                    out.write_all(&buf[..n])?;
                }
                out.sync_all()?;
                drop(out);
                Ok(h.finalize().as_bytes().to_vec())
            })();
            let verified = copied.and_then(|written| {
                let on_disk = hash_file_blake3(&partial)?;
                if on_disk.as_deref() != Some(written.as_slice()) {
                    return Err(AegisError::Io(std::io::Error::other(format!(
                        "{} failed on-disk BLAKE3 verification before rename",
                        partial.display()
                    ))));
                }
                let fresh = hash_file_blake3(from)?.unwrap_or_default();
                if fresh == written {
                    Ok(())
                } else {
                    Err(AegisError::InvalidInput(format!(
                        "{} changed while it was being copied; try again",
                        from.display()
                    )))
                }
            });
            if let Err(e) = verified {
                let _ = std::fs::remove_file(&partial);
                return Err(e);
            }
            std::fs::rename(&partial, &eff_to)?;
            Ok(FsOutcome::Copied { dest: eff_to })
        }
        ActionType::PatchText => {
            if !from.is_file() {
                return Err(AegisError::NotFound(row.from_path.clone()));
            }
            let (original_bytes, patched_bytes) = patch_payload.ok_or_else(|| {
                AegisError::NotFound(format!("heal payload for action {}", row.id))
            })?;
            atomic_write_text_file(from, &patched_bytes)?;
            Ok(FsOutcome::PatchedText {
                path: from.to_path_buf(),
                original_bytes,
            })
        }
        ActionType::Archive => Ok(FsOutcome::MetadataOnly),
        ActionType::Purge | ActionType::Restore => Err(AegisError::InvalidInput(
            "Use quarantine.restore/purge commands for quarantine lifecycle ops".into(),
        )),
    }
}

/// The file row created by the executed copy action carrying `pair == token` in `batch_id`.
fn copy_survivor_for(conn: &Connection, batch_id: &str, token: &str) -> Result<Option<String>> {
    let mut stmt = conn.prepare(
        "SELECT to_path, evidence_json FROM actions
         WHERE batch_id = ?1 AND action_type = 'copy' AND status = 'executed'",
    )?;
    let rows = stmt.query_map(params![batch_id], |r| Ok((r.get::<_, Option<String>>(0)?, r.get::<_, String>(1)?)))?;
    for r in rows {
        let (to, ev) = r?;
        let matches = serde_json::from_str::<serde_json::Value>(&ev)
            .ok()
            .and_then(|v| v.get("pair").and_then(|t| t.as_str()).map(|t| t == token))
            .unwrap_or(false);
        if let (true, Some(to)) = (matches, to) {
            let vol = resolve_online_volume(conn, Path::new(&to))?;
            let cs = vol.as_ref().map(|v| v.2).unwrap_or(false);
            let (_, ph) = normalize_path(Path::new(&to), cs);
            return if let Some((vid, _, _)) = vol {
                Ok(conn
                    .query_row(
                        "SELECT id FROM files WHERE volume_id = ?1 AND path_hash = ?2 AND deleted_at IS NULL
                           AND state NOT IN ('quarantine','missing')",
                        params![vid, ph.as_slice()],
                        |r| r.get(0),
                    )
                    .optional()?)
            } else {
                Ok(conn
                    .query_row(
                        "SELECT id FROM files WHERE path_hash = ?1 AND deleted_at IS NULL
                           AND state NOT IN ('quarantine','missing')",
                        params![ph.as_slice()],
                        |r| r.get(0),
                    )
                    .optional()?)
            };
        }
    }
    Ok(None)
}

struct ExecutionRecord<'a> {
    batch_id: &'a str,
    volume: Option<&'a (String, String, bool)>,
    pre_blake3: Option<Vec<u8>>,
    pre_state_json: String,
    post_blake3: Option<Vec<u8>>,
    post_state_json: String,
    now: i64,
}

fn record_executed_in_tx(
    tx: &Connection,
    row: &DbActionRow,
    outcome: &FsOutcome,
    rec: &ExecutionRecord<'_>,
) -> Result<()> {
    let now = rec.now;
    let eff_to = outcome.dest().map(display_string);
    match outcome {
        FsOutcome::Quarantined { dest, size_bytes, .. } => {
            let (vid, _, _) = rec.volume.ok_or_else(|| {
                AegisError::QuarantineUnavailable("Volume unavailable".into())
            })?;
            tx.execute(
                "INSERT INTO quarantine_entries (
                    id, action_id, target_kind, file_id, atomic_unit_id, volume_id,
                    original_path, quarantine_path, size_bytes, blake3,
                    quarantined_at, expires_at
                 ) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12)",
                params![
                    new_id(),
                    row.id,
                    row.target_kind.as_str(),
                    row.file_id,
                    row.atomic_unit_id,
                    vid,
                    row.from_path,
                    display_string(dest),
                    size_bytes,
                    rec.pre_blake3.as_deref(),
                    now,
                    now + default_ttl_ms()
                ],
            )?;
            if let Some(ref fid) = row.file_id {
                tx.execute(
                    "UPDATE files SET state = 'quarantine', updated_at = ?1 WHERE id = ?2",
                    params![now, fid],
                )?;
                tx.execute(
                    "INSERT INTO outbox_events (aggregate_type, aggregate_id, op, payload_json, created_at)
                     VALUES ('file_payload', ?1, 'upsert', '{\"state\":\"quarantine\"}', ?2)",
                    params![fid, now],
                )?;
            }
            if row.target_kind == TargetKind::Dir {
                let (inside, prefix, n) = files_inside_sql(&row.from_path, 1);
                let exact_from = row.from_path.trim_end_matches(['\\', '/']);
                tx.execute(
                    "UPDATE cache_dirs SET state = 'quarantine', updated_at = ?1
                     WHERE volume_id = ?4 AND state = 'active'
                       AND (lower(rtrim(path, '\\/')) = lower(?5) OR lower(substr(path, 1, ?3)) = lower(?2))",
                    params![now, prefix, n, vid, exact_from],
                )?;
                // The folder's catalogued files went with it: out of search, duplicates and
                // recognition until it is restored or purged. Their paths stay the originals.
                let fids: Vec<String> = {
                    let mut stmt = tx.prepare(&format!(
                        "SELECT id FROM files WHERE {inside} AND volume_id = ?3 AND state != 'quarantine'"
                    ))?;
                    stmt.query_map(params![prefix, n, vid], |r| r.get(0))?
                        .collect::<rusqlite::Result<_>>()?
                };
                for fid in fids {
                    tx.execute(
                        "UPDATE files SET state = 'quarantine', updated_at = ?1 WHERE id = ?2",
                        params![now, fid],
                    )?;
                    tx.execute(
                        "INSERT INTO outbox_events (aggregate_type, aggregate_id, op, payload_json, created_at)
                         VALUES ('file_payload', ?1, 'upsert', '{\"state\":\"quarantine\"}', ?2)",
                        params![fid, now],
                    )?;
                }
            }
        }
        FsOutcome::Moved { dest, .. } => {
            if let Some(ref fid) = row.file_id {
                let case_sensitive = rec.volume.map(|v| v.2).unwrap_or(false);
                let (disp, ph) = normalize_path(dest, case_sensitive);
                let name = dest
                    .file_name()
                    .map(|n| n.to_string_lossy().to_string())
                    .unwrap_or_default();
                tx.execute(
                    "UPDATE files SET path = ?1, path_hash = ?2, name = ?3, updated_at = ?4 WHERE id = ?5",
                    params![disp, ph.as_slice(), name, now, fid],
                )?;
                if plan_evidence(&row.evidence_json).exact_dest {
                    tx.execute(
                        "INSERT INTO file_history (id, file_id, old_path, new_path, batch_id, changed_at)
                         VALUES (?1, ?2, ?3, ?4, ?5, ?6)",
                        params![new_id(), fid, row.from_path, disp, rec.batch_id, now],
                    )?;
                }
                if let Ok(ev) = serde_json::from_str::<serde_json::Value>(&row.evidence_json) {
                    crate::backup::record_backup_copy_in_tx(tx, &ev, dest, Some(fid), rec.post_blake3.as_deref())?;
                }
            }
            // A project-plan folder move: its files' catalog paths follow it at once, so the
            // plan's later steps (and their bookkeeping) see where things really are.
            if row.target_kind == TargetKind::Dir && plan_evidence(&row.evidence_json).exact_dest {
                let case_sensitive = rec.volume.map(|v| v.2).unwrap_or(false);
                let vid = rec.volume.map(|v| v.0.as_str());
                rebase_children(tx, vid, &row.from_path, &display_string(dest), case_sensitive, Some(rec.batch_id), now)?;
            }
        }
        FsOutcome::Copied { dest } => {
            if let Ok(ev) = serde_json::from_str::<serde_json::Value>(&row.evidence_json) {
                crate::backup::record_backup_copy_in_tx(tx, &ev, dest, None, rec.post_blake3.as_deref())?;
            }
        }
        FsOutcome::PatchedText { path, .. } => {
            if let Some(ref fid) = row.file_id {
                let (size, mtime) = inspect_path_metadata(path)
                    .ok()
                    .flatten()
                    .map(|st| (st.size, st.mtime))
                    .unwrap_or_else(|| {
                        let len = std::fs::metadata(path).map(|m| m.len() as i64).unwrap_or(0);
                        (len, now)
                    });
                tx.execute(
                    "UPDATE files
                     SET size = ?1, mtime = ?2, blake3 = ?3, quick_hash = NULL,
                         last_seen = ?4, updated_at = ?4
                     WHERE id = ?5",
                    params![size, mtime, rec.post_blake3.as_deref(), now, fid],
                )?;
                crate::scanner::refresh_text_chunks_if_local(tx, fid, path, "local", size, false)?;
            }
        }
        FsOutcome::MetadataOnly => {
            if row.action_type == ActionType::Archive
                && let Some(ref fid) = row.file_id
            {
                tx.execute(
                    "UPDATE files SET state = 'archive', updated_at = ?1 WHERE id = ?2",
                    params![now, fid],
                )?;
            }
        }
    }

    tx.execute(
        "UPDATE actions
         SET to_path = COALESCE(?1, to_path), pre_blake3 = ?2, post_blake3 = ?3,
             pre_state_json = ?4, post_state_json = ?5, status = 'executed', executed_at = ?6,
             error_message = NULL
         WHERE id = ?7",
        params![
            eff_to,
            rec.pre_blake3.as_deref(),
            rec.post_blake3.as_deref(),
            rec.pre_state_json,
            rec.post_state_json,
            now,
            row.id
        ],
    )?;
    append_audit_in_tx(
        tx,
        row.agent.as_deref().unwrap_or("user"),
        &format!("actions.{}", row.action_type.as_str()),
        &row.from_path,
        &serde_json::json!({
            "action_id": row.id,
            "batch_id": rec.batch_id,
            "to_path": eff_to
        })
        .to_string(),
    )?;
    Ok(())
}

fn mark_action(store: &mut SqliteStore, action_id: &str, status: &str, msg: &str) -> Result<()> {
    store.with_full_sync_tx(|tx| {
        tx.execute(
            "UPDATE actions SET status = ?1, error_message = ?2 WHERE id = ?3",
            params![status, msg, action_id],
        )?;
        Ok(())
    })
}

/// Checks a project-plan folder step just before it runs (plan 2026-11-23-project-plans):
/// - the folder must hold what it held when the plan was made (`expect`), or the step stops;
/// - a quarantine with `verify_against` goes ahead only if every file in the folder is proven
///   to be in that home by BLAKE3, read now. Otherwise the step becomes a move of the whole
///   folder to its `fallback` (a dated version), and the returned row says so.
///
/// Folder actions without plan evidence pass unchanged.
fn check_plan_folder(store: &mut SqliteStore, row: &DbActionRow) -> Result<Option<DbActionRow>> {
    let ev = plan_evidence(&row.evidence_json);
    let from = Path::new(&row.from_path);
    if let Some(expected) = ev.expect {
        if !from.is_dir() {
            return Err(AegisError::NotFound(format!(
                "{} is no longer there. Aegis will make a fresh plan.",
                row.from_path
            )));
        }
        if crate::context::plan::folder_state(from)? != expected {
            return Err(AegisError::InvalidInput(format!(
                "{} changed since this plan was made. Aegis will make a fresh plan.",
                row.from_path
            )));
        }
    }
    if row.action_type != ActionType::Quarantine {
        return Ok(None);
    }
    let Some(home) = ev.verify_against.as_deref() else { return Ok(None) };
    let missing = crate::context::plan::unproven_files(from, Path::new(home))?;
    if missing == 0 {
        return Ok(None);
    }
    let what = if missing == 1 { "1 file here isn't".to_string() } else { format!("{missing} files here aren't") };
    let Some(fallback) = ev.fallback else {
        return Err(AegisError::InvalidInput(format!("{what} in the main copy, so nothing was quarantined.")));
    };
    let reason = format!("{what} in the main copy, so it was kept whole as a version in {fallback}.");
    store.conn().execute(
        "UPDATE actions SET action_type = 'move', to_path = ?1, reason = ?2 WHERE id = ?3",
        params![fallback, reason, row.id],
    )?;
    let mut moved = row.clone();
    moved.action_type = ActionType::Move;
    moved.to_path = Some(fallback);
    moved.reason = reason;
    Ok(Some(moved))
}

/// Executes all `approved` actions in `batch_id` through `C4 v1.1` Policy Engine (`C1` `actions.execute`).
///
/// Each action is applied to disk and then committed in its own short transaction; a failed
/// commit undoes that action's disk change before the action is marked `failed`.
pub fn execute_batch(store: &mut SqliteStore, batch_id: &str) -> Result<ExecuteResult> {
    let ctx = build_policy_context(store)?;
    let rows = load_batch_actions(store.conn(), batch_id)?;
    if rows.is_empty() {
        return Err(AegisError::NotFound(format!("batch {batch_id}")));
    }
    if !rows.iter().any(|r| r.status == ActionStatus::Approved) {
        return Err(AegisError::InvalidInput(format!(
            "Batch {batch_id} has no approved actions; approve it in the Simulator before executing"
        )));
    }

    let mut metas = HashMap::new();
    let mut vol_lookup = HashMap::new();
    let in_memory = store.is_in_memory();
    for r in &rows {
        metas.insert(r.id.clone(), resolve_policy_target_meta(store.conn(), r)?);
        if let Ok(v) = resolve_volume_for_action(store.conn(), Path::new(&r.from_path)) {
            if r.action_type == ActionType::Quarantine {
                guard_test_quarantine(in_memory, Path::new(&v.1))?;
            }
            vol_lookup.insert(r.id.clone(), v);
        }
        if r.action_type == ActionType::Quarantine
            && let Some(ref dest) = r.to_path
        {
            guard_test_quarantine(in_memory, Path::new(dest))?;
        }
    }

    let mut executed_count = 0usize;
    let mut failed_count = 0usize;
    let mut skipped_count = 0usize;

    store.with_full_sync_tx(|tx| {
        tx.execute(
            "UPDATE action_batches SET status = 'executing' WHERE id = ?1",
            params![batch_id],
        )?;
        Ok(())
    })?;

    // A project plan is all or nothing: after one step fails or is held back, the rest wait
    // (and the caller undoes what was done).
    let plan_batch = rows.iter().any(|r| plan_evidence(&r.evidence_json).exact_dest);
    let mut held = 0usize;
    for row in &rows {
        if row.status != ActionStatus::Approved {
            skipped_count += 1;
            continue;
        }
        if plan_batch && failed_count + held > 0 {
            skipped_count += 1;
            mark_action(store, &row.id, "skipped", "An earlier step of this plan didn't run, so this one was left.")?;
            continue;
        }
        // Project-plan folder steps: the folder must be as it was when the plan was made, and a
        // copy is quarantined only if every file in it is proven to be in the home.
        let rewritten: Option<DbActionRow> = if row.target_kind == TargetKind::Dir {
            match check_plan_folder(store, row) {
                Ok(r) => r,
                Err(e) => {
                    failed_count += 1;
                    mark_action(store, &row.id, "failed", &e.to_string())?;
                    continue;
                }
            }
        } else {
            None
        };
        let row: &DbActionRow = rewritten.as_ref().unwrap_or(row);
        let meta = &metas[&row.id];
        let verdict = evaluate_policy(&policy_action_for(row, meta), &ctx);
        if let PolicyVerdict::Block(rule) = verdict {
            failed_count += 1;
            mark_action(store, &row.id, "failed", &format!("policy_blocked:{rule}"))?;
            continue;
        }
        if verdict == PolicyVerdict::HoldLowConfidence {
            skipped_count += 1;
            held += 1;
            mark_action(store, &row.id, "skipped", "hold_low_confidence")?;
            continue;
        }
        let volume = vol_lookup.get(&row.id);
        if row.action_type == ActionType::Quarantine && volume.is_none() {
            failed_count += 1;
            mark_action(
                store,
                &row.id,
                "failed",
                "quarantine_unavailable: no online volume for this path",
            )?;
            continue;
        }

        let from = Path::new(&row.from_path);

        // The file must still be where the action says, on the drive it was catalogued on.
        // Stored paths are absolute: a different drive at the same letter must never be touched.
        let identity = match (row.file_id.as_deref(), row.target_kind, row.action_type) {
            (
                Some(fid),
                TargetKind::File,
                ActionType::Quarantine | ActionType::Move | ActionType::Rename | ActionType::PatchText,
            ) => crate::dupes::safety::confirm_file_at(store.conn(), fid, &row.from_path),
            _ => Ok(()),
        }
        .and_then(|_| {
            let ev = serde_json::from_str::<serde_json::Value>(&row.evidence_json).unwrap_or_default();
            if let Some(member) = ev.get("member_file_id").and_then(|v| v.as_str()) {
                crate::dupes::safety::confirm_file_at(store.conn(), member, &row.from_path)?;
            }
            if let Some(vol) = ev.pointer("/backup_copy/backup_volume_id").and_then(|v| v.as_str()) {
                crate::dupes::safety::confirm_drive_identity(store.conn(), vol)?;
            }
            Ok(())
        });
        if let Err(e) = identity {
            failed_count += 1;
            mark_action(store, &row.id, "failed", &e.to_string())?;
            continue;
        }

        let hashes_content = row.target_kind == TargetKind::File || row.action_type == ActionType::Copy;
        let pre_blake3 = if hashes_content {
            hash_file_blake3(from).ok().flatten()
        } else {
            None
        };
        let pre_state_json = capture_state_json(from);

        // A quarantine that depends on a copy made earlier in this batch keeps that copy.
        let mut survivor_id = row.survivor_file_id.clone();
        if let Some(token) = serde_json::from_str::<serde_json::Value>(&row.evidence_json)
            .ok()
            .and_then(|v| v.get("after_copy").and_then(|t| t.as_str()).map(str::to_string))
        {
            match copy_survivor_for(store.conn(), batch_id, &token)? {
                Some(id) => survivor_id = Some(id),
                None => {
                    failed_count += 1;
                    mark_action(
                        store,
                        &row.id,
                        "failed",
                        "The Backup drive copy wasn't made, so this copy was kept.",
                    )?;
                    continue;
                }
            }
        }

        // Duplicate deletes: the other copy must be online and unchanged right now (S3).
        if let Some(survivor) = survivor_id.as_deref() {
            let check = match pre_blake3.as_deref() {
                Some(h) => crate::dupes::verify_survivor(store.conn(), survivor, h),
                None => Err(AegisError::SurvivorUnavailable(
                    "This copy could not be read to compare with the one being kept.".into(),
                )),
            };
            // Never the same physical file, and never below the copy floor (S1/S2), re-checked now.
            let check = check.and_then(|_| {
                let survivor_path: String =
                    store.conn().query_row("SELECT path FROM files WHERE id = ?1", [survivor], |r| r.get(0))?;
                let same = |a: &str, b: &str| a.replace('/', "\\").eq_ignore_ascii_case(&b.replace('/', "\\"));
                if same(&survivor_path, &row.from_path) {
                    return Err(AegisError::SurvivorUnavailable(
                        "The copy to keep is this same file, so nothing was removed.".into(),
                    ));
                }
                let h = pre_blake3.as_deref().unwrap_or_default();
                if crate::dupes::copy_count(store.conn(), h)? - 1 < crate::dupes::MIN_COPIES {
                    return Err(AegisError::PolicyBlocked {
                        rule: "min_copies".into(),
                        detail: format!(
                            "Removing this would leave fewer than {} copies, so it was kept.",
                            crate::dupes::MIN_COPIES
                        ),
                    });
                }
                Ok(())
            });
            if let Err(e) = check {
                failed_count += 1;
                mark_action(store, &row.id, "failed", &e.to_string())?;
                continue;
            }
        }

        let patch_payload = if row.action_type == ActionType::PatchText {
            let payload_row: Option<(String, String, Vec<u8>)> = store
                .conn()
                .query_row(
                    "SELECT original_content, patched_content, original_blake3
                     FROM heal_patch_payloads WHERE action_id = ?1",
                    params![row.id],
                    |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?)),
                )
                .optional()?;
            let Some((orig_s, patched_s, expected_blake3)) = payload_row else {
                failed_count += 1;
                mark_action(
                    store,
                    &row.id,
                    "failed",
                    "Missing heal_patch_payloads snapshot for patch_text action.",
                )?;
                continue;
            };
            if pre_blake3.as_deref() != Some(expected_blake3.as_slice()) {
                failed_count += 1;
                mark_action(
                    store,
                    &row.id,
                    "failed",
                    "File changed on disk since healing proposal was staged; rescan to refresh.",
                )?;
                continue;
            }
            Some((orig_s.into_bytes(), patched_s.into_bytes()))
        } else {
            None
        };

        // 1. Disk first, outside any transaction.
        let outcome = match perform_execute_fs(row, meta, patch_payload) {
            Ok(o) => o,
            Err(e) => {
                failed_count += 1;
                let ipc = e.to_ipc();
                mark_action(store, &row.id, "failed", &format!("{}: {}", ipc.code, ipc.message))?;
                continue;
            }
        };

        let (post_blake3, post_state_json) = match outcome.dest() {
            Some(dest) => {
                let hash = if hashes_content {
                    hash_file_blake3(dest).ok().flatten()
                } else {
                    None
                };
                (hash, capture_state_json(dest))
            }
            None => (pre_blake3.clone(), pre_state_json.clone()),
        };

        // 2. One short transaction recording exactly this action.
        let rec = ExecutionRecord {
            batch_id,
            volume,
            pre_blake3,
            pre_state_json,
            post_blake3,
            post_state_json,
            now: now_ms(),
        };
        let commit = store.with_full_sync_tx(|tx| record_executed_in_tx(tx, row, &outcome, &rec));
        match commit {
            Ok(()) => executed_count += 1,
            Err(e) => {
                failed_count += 1;
                let undo_note = match outcome.undo() {
                    Ok(()) => "disk change was undone".to_string(),
                    Err(ue) => format!(
                        "disk change could NOT be undone ({ue}); item remains at {}",
                        outcome.dest().map(|d| d.display().to_string()).unwrap_or_default()
                    ),
                };
                let _ = mark_action(
                    store,
                    &row.id,
                    "failed",
                    &format!("database commit failed: {e}; {undo_note}"),
                );
            }
        }
    }

    let batch_status = if failed_count == 0 && executed_count > 0 {
        BatchStatus::Executed
    } else if executed_count > 0 {
        BatchStatus::Partial
    } else {
        BatchStatus::Failed
    };
    let finished = now_ms();
    store.with_full_sync_tx(|tx| {
        tx.execute(
            "UPDATE action_batches SET status = ?1, executed_at = ?2 WHERE id = ?3",
            params![batch_status.as_str(), finished, batch_id],
        )?;
        Ok(())
    })?;

    let updated = load_batch_actions(store.conn(), batch_id)?
        .into_iter()
        .map(|r| r.to_item())
        .collect();

    Ok(ExecuteResult {
        batch_id: batch_id.to_string(),
        status: batch_status,
        executed_count,
        failed_count,
        skipped_count,
        actions: updated,
    })
}

/// Pre-flight checks for undoing `rows` (newest first). A project plan's steps are chained (a
/// copy moved aside frees a name the home then takes), so its steps are checked in undo order
/// against the disk as the earlier undo steps will leave it, not as it is now.
fn preflight(rows: &[DbActionRow]) -> Vec<RollbackConflictItem> {
    if !rows.iter().any(|r| plan_evidence(&r.evidence_json).exact_dest) {
        return rows.iter().filter_map(|r| check_single_action_preflight(r, &|p: &Path| p.exists())).collect();
    }
    let key = |p: &str| p.trim_end_matches(['\\', '/']).to_lowercase();
    let mut will_exist: HashMap<String, bool> = HashMap::new();
    let mut out = Vec::new();
    for r in rows {
        let found = {
            let exists = |p: &Path| will_exist.get(&key(&p.to_string_lossy())).copied().unwrap_or_else(|| p.exists());
            check_single_action_preflight(r, &exists)
        };
        match found {
            Some(c) => out.push(c),
            None => {
                if let Some(t) = &r.to_path {
                    will_exist.insert(key(t), false);
                }
                will_exist.insert(key(&r.from_path), true);
            }
        }
    }
    out
}

fn check_single_action_preflight(row: &DbActionRow, exists: &dyn Fn(&Path) -> bool) -> Option<RollbackConflictItem> {
    if row.action_type == ActionType::PatchText {
        let from_path = Path::new(&row.from_path);
        if !exists(from_path) {
            return Some(RollbackConflictItem {
                action_id: row.id.clone(),
                from_path: row.from_path.clone(),
                to_path: row.to_path.clone(),
                conflict: RollbackConflictKind::MissingTarget,
                detail: format!("Patched file missing at {}", row.from_path),
                recommended_choice: ConflictResolutionChoice::Skip,
            });
        }
        if let Some(expected) = &row.post_blake3
            && let Ok(Some(actual)) = hash_file_blake3(from_path)
            && &actual != expected
        {
            return Some(RollbackConflictItem {
                action_id: row.id.clone(),
                from_path: row.from_path.clone(),
                to_path: row.to_path.clone(),
                conflict: RollbackConflictKind::Modified,
                detail: format!("File at {} was modified after Aegis healed it", row.from_path),
                recommended_choice: ConflictResolutionChoice::CopyBackKeepingBoth,
            });
        }
        return None;
    }
    if !matches!(
        row.action_type,
        ActionType::Move | ActionType::Rename | ActionType::Quarantine | ActionType::Copy
    ) {
        return None;
    }
    let to_str = row.to_path.as_deref()?;
    let to_path = Path::new(to_str);
    let from_path = Path::new(&row.from_path);

    if !exists(to_path) {
        return Some(RollbackConflictItem {
            action_id: row.id.clone(),
            from_path: row.from_path.clone(),
            to_path: row.to_path.clone(),
            conflict: RollbackConflictKind::MissingTarget,
            detail: format!("Target missing at {to_str}"),
            recommended_choice: ConflictResolutionChoice::Skip,
        });
    }

    if row.target_kind == TargetKind::File
        && let Some(expected) = &row.post_blake3
        && let Ok(Some(actual)) = hash_file_blake3(to_path)
        && &actual != expected
    {
        return Some(RollbackConflictItem {
            action_id: row.id.clone(),
            from_path: row.from_path.clone(),
            to_path: row.to_path.clone(),
            conflict: RollbackConflictKind::Modified,
            detail: format!("File at {to_str} was modified after Aegis moved it"),
            recommended_choice: ConflictResolutionChoice::CopyBackKeepingBoth,
        });
    }

    let case_rename = row.to_path.as_deref().is_some_and(|t| case_only(from_path, Path::new(t)));
    if row.action_type != ActionType::Copy && exists(from_path) && !case_rename {
        return Some(RollbackConflictItem {
            action_id: row.id.clone(),
            from_path: row.from_path.clone(),
            to_path: row.to_path.clone(),
            conflict: RollbackConflictKind::Collision,
            detail: format!("Original location {} is now occupied", row.from_path),
            recommended_choice: ConflictResolutionChoice::RestoreAsRename,
        });
    }

    None
}

enum RevertMode {
    /// Move the item back to its original path (which must be free).
    Normal,
    /// Copy the edited item back (never overwriting) and leave the moved copy in place.
    CopyBackKeepingBoth,
    /// Move the item back under an explicit alternative name.
    RestoreAs(PathBuf),
}

fn db_revert_in_tx(
    tx: &Connection,
    row: &DbActionRow,
    dest: &Path,
    copy_back: bool,
    case_sensitive: bool,
    now: i64,
) -> Result<()> {
    let dest_str = display_string(dest);
    if let Some(ref fid) = row.file_id
        && row.action_type != ActionType::Copy
        && !(row.action_type == ActionType::PatchText && copy_back)
    {
        if row.action_type == ActionType::PatchText {
            let (size, mtime) = inspect_path_metadata(dest)
                .ok()
                .flatten()
                .map(|st| (st.size, st.mtime))
                .unwrap_or_else(|| {
                    let len = std::fs::metadata(dest).map(|m| m.len() as i64).unwrap_or(0);
                    (len, now)
                });
            let blake = hash_file_blake3(dest).ok().flatten();
            tx.execute(
                "UPDATE files SET state = 'active', size = ?1, mtime = ?2, blake3 = ?3, quick_hash = NULL, updated_at = ?4
                 WHERE id = ?5",
                params![size, mtime, blake.as_deref(), now, fid],
            )?;
            crate::scanner::refresh_text_chunks_if_local(tx, fid, dest, "local", size, false)?;
        } else {
            let (_, ph) = normalize_path(dest, case_sensitive);
            let name = dest
                .file_name()
                .map(|n| n.to_string_lossy().to_string())
                .unwrap_or_default();
            tx.execute(
                "UPDATE files SET state = 'active', path = ?1, path_hash = ?2, name = ?3, updated_at = ?4
                 WHERE id = ?5",
                params![dest_str, ph.as_slice(), name, now, fid],
            )?;
            if row.action_type == ActionType::Quarantine {
                tx.execute(
                    "INSERT INTO outbox_events (aggregate_type, aggregate_id, op, payload_json, created_at)
                     VALUES ('file_payload', ?1, 'upsert', '{\"state\":\"active\"}', ?2)",
                    params![fid, now],
                )?;
            }
        }
    }
    if matches!(row.action_type, ActionType::Move | ActionType::Rename)
        && row.target_kind == TargetKind::Dir
        && plan_evidence(&row.evidence_json).exact_dest
        && let Some(to) = row.to_path.as_deref()
    {
        let vol_id = resolve_online_volume(tx, dest)?.map(|v| v.0);
        rebase_children(tx, vol_id.as_deref(), to, &dest_str, case_sensitive, None, now)?;
    }
    if row.action_type == ActionType::Quarantine {
        if copy_back {
            // The original stays staged in quarantine (and expires/purges normally), but it no
            // longer represents the live file row, so purging it must not tombstone that row.
            tx.execute(
                "UPDATE quarantine_entries SET file_id = NULL
                 WHERE action_id = ?1 AND restored_at IS NULL AND purged_at IS NULL",
                params![row.id],
            )?;
        } else {
            tx.execute(
                "UPDATE quarantine_entries SET restored_at = ?1 WHERE action_id = ?2 AND restored_at IS NULL",
                params![now, row.id],
            )?;
            if row.target_kind == TargetKind::Dir {
                let vol_id = resolve_online_volume(tx, dest)?
                    .or(resolve_online_volume(tx, Path::new(&row.from_path))?)
                    .map(|v| v.0);
                let (inside, prefix, n) = files_inside_sql(&row.from_path, 1);
                let exact_from = row.from_path.trim_end_matches(['\\', '/']);
                if let Some(vid) = vol_id.as_deref() {
                    tx.execute(
                        "UPDATE cache_dirs SET state = 'active', updated_at = ?1
                         WHERE volume_id = ?4 AND state = 'quarantine'
                           AND (lower(rtrim(path, '\\/')) = lower(?5) OR lower(substr(path, 1, ?3)) = lower(?2))",
                        params![now, prefix, n, vid, exact_from],
                    )?;
                } else {
                    tx.execute(
                        "UPDATE cache_dirs SET state = 'active', updated_at = ?1
                         WHERE state = 'quarantine'
                           AND (lower(rtrim(path, '\\/')) = lower(?4) OR lower(substr(path, 1, ?3)) = lower(?2))",
                        params![now, prefix, n, exact_from],
                    )?;
                }
                // Its files are back, at their original paths. Only files really there again
                // become active: a file quarantined on its own earlier keeps its state.
                let ids: Vec<(String, String)> = if let Some(vid) = vol_id.as_deref() {
                    let mut stmt = tx.prepare(&format!(
                        "SELECT id, path FROM files WHERE {inside} AND volume_id = ?3 AND state = 'quarantine'"
                    ))?;
                    stmt.query_map(params![prefix, n, vid], |r| Ok((r.get(0)?, r.get(1)?)))?
                        .collect::<rusqlite::Result<_>>()?
                } else {
                    let mut stmt = tx.prepare(&format!(
                        "SELECT id, path FROM files WHERE {inside} AND state = 'quarantine'"
                    ))?;
                    stmt.query_map(params![prefix, n], |r| Ok((r.get(0)?, r.get(1)?)))?
                        .collect::<rusqlite::Result<_>>()?
                };
                for (id, path) in ids {
                    let back = Path::new(&path).strip_prefix(&row.from_path).map(|rest| dest.join(rest)).unwrap_or_else(|_| PathBuf::from(&path));
                    if back.is_file() {
                        tx.execute("UPDATE files SET state = 'active', updated_at = ?1 WHERE id = ?2", params![now, id])?;
                        tx.execute(
                            "INSERT INTO outbox_events (aggregate_type, aggregate_id, op, payload_json, created_at)
                             VALUES ('file_payload', ?1, 'upsert', '{\"state\":\"active\"}', ?2)",
                            params![id, now],
                        )?;
                    }
                }
            }
        }
    }
    if row.action_type == ActionType::Archive
        && let Some(ref fid) = row.file_id
    {
        tx.execute(
            "UPDATE files SET state = 'active', updated_at = ?1 WHERE id = ?2",
            params![now, fid],
        )?;
    }

    tx.execute(
        "UPDATE actions SET status = 'rolled_back', rollback_conflict = NULL, rolled_back_at = ?1 WHERE id = ?2",
        params![now, row.id],
    )?;
    append_audit_in_tx(
        tx,
        "user",
        "actions.rollback",
        &dest_str,
        &serde_json::json!({ "action_id": row.id, "batch_id": row.batch_id, "copy_back": copy_back })
            .to_string(),
    )?;
    Ok(())
}

/// Reverts one executed action: disk first, then a short transaction; a failed commit
/// undoes the disk change. Never overwrites an occupied destination.
fn revert_one(store: &mut SqliteStore, row: &DbActionRow, mode: RevertMode) -> Result<()> {
    let now = now_ms();
    let cs = volume_case_sensitive_for(store.conn(), Path::new(&row.from_path));
    match row.action_type {
        ActionType::Copy => {
            if let Some(ref to_str) = row.to_path {
                let p = Path::new(to_str);
                if p.is_file() {
                    std::fs::remove_file(p)?;
                }
            }
            let from = PathBuf::from(&row.from_path);
            store.with_full_sync_tx(|tx| db_revert_in_tx(tx, row, &from, false, cs, now))
        }
        ActionType::Move | ActionType::Rename | ActionType::Quarantine => {
            let src = PathBuf::from(row.to_path.as_deref().ok_or_else(|| {
                AegisError::InvalidInput("Executed action missing to_path".into())
            })?);
            let original = PathBuf::from(&row.from_path);
            let copy_back = matches!(mode, RevertMode::CopyBackKeepingBoth);
            let dest = match mode {
                RevertMode::Normal => original,
                RevertMode::RestoreAs(p) => p,
                RevertMode::CopyBackKeepingBoth => non_overwriting_dest(&original).0,
            };
            if dest.exists() && !case_only(&src, &dest) {
                return Err(AegisError::InvalidInput(format!(
                    "{} is occupied; choose 'restore as rename' or skip",
                    dest.display()
                )));
            }
            if copy_back && src.is_dir() {
                return Err(AegisError::InvalidInput(
                    "Copy-back is only supported for files".into(),
                ));
            }
            if let Some(parent) = dest.parent() {
                std::fs::create_dir_all(parent)?;
            }
            if copy_back {
                std::fs::copy(&src, &dest)?;
            } else {
                rename_entry(&src, &dest)?;
            }
            let res = store.with_full_sync_tx(|tx| db_revert_in_tx(tx, row, &dest, copy_back, cs, now));
            if let Err(e) = res {
                let _ = if copy_back {
                    std::fs::remove_file(&dest)
                } else {
                    std::fs::rename(&dest, &src)
                };
                return Err(e);
            }
            // A project plan step leaves no empty folders it made (`Versions`, a companion
            // folder) behind, so later steps of the undo find their places free.
            if row.target_kind == TargetKind::Dir && plan_evidence(&row.evidence_json).exact_dest {
                remove_empty_ancestors(&src, &dest);
            }
            Ok(())
        }
        ActionType::PatchText => {
            let (original_content, patched_content): (String, String) = store
                .conn()
                .query_row(
                    "SELECT original_content, patched_content
                     FROM heal_patch_payloads WHERE action_id = ?1",
                    params![row.id],
                    |r| Ok((r.get(0)?, r.get(1)?)),
                )
                .optional()?
                .ok_or_else(|| AegisError::NotFound(format!("heal payload for action {}", row.id)))?;
            let original = PathBuf::from(&row.from_path);
            let copy_back = matches!(mode, RevertMode::CopyBackKeepingBoth);
            let (dest, had_prev_dest) = match mode {
                RevertMode::Normal => (original, true),
                RevertMode::RestoreAs(p) => {
                    if p.exists() {
                        return Err(AegisError::InvalidInput(format!(
                            "{} is occupied; choose 'restore as rename' or skip",
                            p.display()
                        )));
                    }
                    (p, false)
                }
                RevertMode::CopyBackKeepingBoth => (non_overwriting_dest(&original).0, false),
            };
            let live_before_revert = if had_prev_dest {
                std::fs::read(&dest).ok()
            } else {
                None
            };
            atomic_write_text_file(&dest, original_content.as_bytes())?;
            let res =
                store.with_full_sync_tx(|tx| db_revert_in_tx(tx, row, &dest, copy_back, cs, now));
            if let Err(e) = res {
                if had_prev_dest {
                    let compensate = live_before_revert
                        .as_deref()
                        .unwrap_or(patched_content.as_bytes());
                    let _ = atomic_write_text_file(&dest, compensate);
                } else {
                    let _ = std::fs::remove_file(&dest);
                }
                return Err(e);
            }
            Ok(())
        }
        ActionType::Archive => {
            let from = PathBuf::from(&row.from_path);
            store.with_full_sync_tx(|tx| db_revert_in_tx(tx, row, &from, false, cs, now))
        }
        ActionType::Purge | ActionType::Restore => Err(AegisError::InvalidInput(format!(
            "{} actions cannot be rolled back",
            row.action_type.as_str()
        ))),
    }
}

fn executed_rows_reversed(conn: &Connection, batch_id: &str) -> Result<Vec<DbActionRow>> {
    let mut rows = load_batch_actions(conn, batch_id)?;
    rows.reverse();
    Ok(rows
        .into_iter()
        .filter(|r| r.status == ActionStatus::Executed)
        .collect())
}

/// Pre-flights every still-executed action; pauses on conflicts, otherwise reverts them all
/// and settles the batch status (`C1 v1.2` §4).
fn continue_rollback(
    store: &mut SqliteStore,
    batch_id: &str,
    mut reverted_count: usize,
) -> Result<RollbackResult> {
    let executed = executed_rows_reversed(store.conn(), batch_id)?;
    let conflicts: Vec<RollbackConflictItem> = preflight(&executed);

    if !conflicts.is_empty() {
        store.with_full_sync_tx(|tx| {
            tx.execute(
                "UPDATE actions SET rollback_conflict = NULL WHERE batch_id = ?1 AND status = 'executed'",
                params![batch_id],
            )?;
            for c in &conflicts {
                tx.execute(
                    "UPDATE actions SET rollback_conflict = ?1 WHERE id = ?2",
                    params![c.conflict.as_str(), c.action_id],
                )?;
            }
            tx.execute(
                "UPDATE action_batches SET status = 'partial' WHERE id = ?1",
                params![batch_id],
            )?;
            Ok(())
        })?;
        return Ok(RollbackResult {
            batch_id: batch_id.to_string(),
            status: BatchStatus::Partial,
            reverted_count,
            conflicts,
        });
    }

    for row in &executed {
        if let Err(e) = revert_one(store, row, RevertMode::Normal) {
            let _ = store.with_full_sync_tx(|tx| {
                tx.execute(
                    "UPDATE action_batches SET status = 'partial' WHERE id = ?1",
                    params![batch_id],
                )?;
                Ok(())
            });
            return Err(AegisError::InvalidInput(format!(
                "Rollback stopped after reverting {reverted_count} action(s): {e}"
            )));
        }
        reverted_count += 1;
    }

    // Fully rolled back only if every action that ever executed is now rolled back.
    let not_reverted: i64 = store.conn().query_row(
        "SELECT COUNT(*) FROM actions
         WHERE batch_id = ?1 AND executed_at IS NOT NULL AND rolled_back_at IS NULL",
        params![batch_id],
        |r| r.get(0),
    )?;
    let status = if not_reverted == 0 {
        BatchStatus::RolledBack
    } else {
        BatchStatus::Partial
    };
    let now = now_ms();
    store.with_full_sync_tx(|tx| {
        tx.execute(
            "UPDATE action_batches SET status = ?1, rolled_back_at = ?2 WHERE id = ?3",
            params![status.as_str(), now, batch_id],
        )?;
        Ok(())
    })?;

    Ok(RollbackResult {
        batch_id: batch_id.to_string(),
        status,
        reverted_count,
        conflicts: Vec::new(),
    })
}

/// Pre-flights ALL `executed` actions in `batch_id` before mutating disk (`C1 v1.2` §4).
pub fn rollback_batch(store: &mut SqliteStore, batch_id: &str) -> Result<RollbackResult> {
    let executed = executed_rows_reversed(store.conn(), batch_id)?;
    if executed.is_empty() {
        return Err(AegisError::NotFound(format!(
            "No executed actions to roll back in batch {batch_id}"
        )));
    }
    if executed
        .iter()
        .any(|r| matches!(r.action_type, ActionType::Purge | ActionType::Restore))
    {
        return Err(AegisError::InvalidInput(
            "Purge/restore batches cannot be rolled back".into(),
        ));
    }
    continue_rollback(store, batch_id, 0)
}

/// Resolves a pre-flight rollback conflict on one action, then resumes the batch rollback
/// (reverting the remaining clean actions once no conflicts are left) (`C1 v1.2` §4).
pub fn resolve_rollback_conflict(
    store: &mut SqliteStore,
    action_id: &str,
    choice: ConflictResolutionChoice,
) -> Result<RollbackResult> {
    let batch_id: String = store
        .conn()
        .query_row(
            "SELECT batch_id FROM actions WHERE id = ?1",
            params![action_id],
            |r| r.get(0),
        )
        .optional()?
        .ok_or_else(|| AegisError::NotFound(format!("action {action_id}")))?;

    let target = load_batch_actions(store.conn(), &batch_id)?
        .into_iter()
        .find(|r| r.id == action_id)
        .ok_or_else(|| AegisError::NotFound(format!("action {action_id}")))?;

    if target.status != ActionStatus::Executed || target.rollback_conflict.is_none() {
        return Err(AegisError::InvalidInput(format!(
            "Action {action_id} has no pending rollback conflict"
        )));
    }

    let mut reverted_count = 0usize;
    match choice {
        ConflictResolutionChoice::AbortBatch => {
            store.with_full_sync_tx(|tx| {
                tx.execute(
                    "UPDATE actions SET rollback_conflict = NULL WHERE batch_id = ?1 AND status = 'executed'",
                    params![batch_id],
                )?;
                // Actions already reverted by earlier resolutions stay reverted.
                let any_reverted: i64 = tx.query_row(
                    "SELECT COUNT(*) FROM actions WHERE batch_id = ?1 AND rolled_back_at IS NOT NULL",
                    params![batch_id],
                    |r| r.get(0),
                )?;
                let status = if any_reverted > 0 { "partial" } else { "executed" };
                tx.execute(
                    "UPDATE action_batches SET status = ?1 WHERE id = ?2",
                    params![status, batch_id],
                )?;
                append_audit_in_tx(tx, "user", "actions.rollback_aborted", &batch_id, "{}")?;
                Ok(())
            })?;
            return Ok(RollbackResult {
                batch_id,
                status: BatchStatus::Executed,
                reverted_count: 0,
                conflicts: Vec::new(),
            });
        }
        ConflictResolutionChoice::Skip => {
            let next_status =
                if target.rollback_conflict == Some(RollbackConflictKind::MissingTarget) {
                    "unrecoverable_missing"
                } else {
                    "skipped"
                };
            store.with_full_sync_tx(|tx| {
                tx.execute(
                    "UPDATE actions SET status = ?1, rollback_conflict = NULL WHERE id = ?2",
                    params![next_status, action_id],
                )?;
                append_audit_in_tx(
                    tx,
                    "user",
                    "actions.rollback_skipped",
                    action_id,
                    &serde_json::json!({ "status": next_status }).to_string(),
                )?;
                Ok(())
            })?;
        }
        ConflictResolutionChoice::RevertKeepingEdits => {
            revert_one(store, &target, RevertMode::Normal)?;
            reverted_count += 1;
        }
        ConflictResolutionChoice::CopyBackKeepingBoth => {
            revert_one(store, &target, RevertMode::CopyBackKeepingBoth)?;
            reverted_count += 1;
        }
        ConflictResolutionChoice::RestoreAsRename => {
            let orig = PathBuf::from(&target.from_path);
            let now = now_ms();
            let stem = orig
                .file_stem()
                .map(|s| s.to_string_lossy().to_string())
                .unwrap_or_else(|| "restored".to_string());
            let renamed_name = match orig.extension().and_then(|e| e.to_str()) {
                Some(ext) => format!("{stem}.restored-{now}.{ext}"),
                None => format!("{stem}.restored-{now}"),
            };
            let renamed_dest = non_overwriting_dest(&orig.with_file_name(renamed_name)).0;
            revert_one(store, &target, RevertMode::RestoreAs(renamed_dest))?;
            reverted_count += 1;
        }
    }

    continue_rollback(store, &batch_id, reverted_count)
}

pub fn list_actions(conn: &Connection, status: Option<ActionStatus>) -> Result<Vec<ActionItem>> {
    let sql = match status {
        Some(_) => {
            "SELECT id, batch_id, agent, action_type, target_kind, file_id, atomic_unit_id,
                    from_path, to_path, pre_blake3, post_blake3, reason, confidence,
                    status, rollback_conflict, error_message
             FROM actions WHERE status = ?1 ORDER BY created_at DESC"
        }
        None => {
            "SELECT id, batch_id, agent, action_type, target_kind, file_id, atomic_unit_id,
                    from_path, to_path, pre_blake3, post_blake3, reason, confidence,
                    status, rollback_conflict, error_message
             FROM actions ORDER BY created_at DESC LIMIT 200"
        }
    };
    let mut stmt = conn.prepare(sql)?;
    let map_row = |r: &rusqlite::Row<'_>| -> rusqlite::Result<ActionItem> {
        let at: String = r.get(3)?;
        let tk: String = r.get(4)?;
        let st: String = r.get(13)?;
        let rc: Option<String> = r.get(14)?;
        Ok(ActionItem {
            id: r.get(0)?,
            batch_id: r.get(1)?,
            agent: r.get(2)?,
            action_type: ActionType::from_db(&at).unwrap_or(ActionType::Move),
            target_kind: TargetKind::from_db(&tk),
            file_id: r.get(5)?,
            atomic_unit_id: r.get(6)?,
            from_path: r.get(7)?,
            to_path: r.get(8)?,
            reason: r.get(11)?,
            confidence: r.get(12)?,
            status: ActionStatus::from_db(&st),
            rollback_conflict: rc.as_deref().and_then(RollbackConflictKind::from_db),
            error_message: r.get(15)?,
        })
    };

    let mut out = Vec::new();
    if let Some(st) = status {
        for r in stmt.query_map(params![st.as_str()], map_row)? {
            out.push(r?);
        }
    } else {
        for r in stmt.query_map([], map_row)? {
            out.push(r?);
        }
    }
    Ok(out)
}

/// Generates an exact-duplicate report adjusted honestly for hardlinks (`ADR-003`).
pub fn exact_dedupe_report(conn: &Connection) -> Result<DupeReport> {
    let mut stmt = conn.prepare(
        "SELECT f.id, f.volume_id, f.file_index, f.nlink, f.size, f.blake3, f.path, f.last_access
         FROM files f LEFT JOIN volumes v ON v.id = f.volume_id
         WHERE f.deleted_at IS NULL AND f.state NOT IN ('quarantine','missing')
           AND f.blake3 IS NOT NULL AND f.size > 0
         ORDER BY f.blake3, COALESCE(v.role = 'backup', 0) ASC, f.mtime ASC, length(f.path) ASC",
    )?;

    #[allow(clippy::type_complexity)]
    let mut groups: HashMap<Vec<u8>, Vec<(String, String, Option<Vec<u8>>, i64, i64)>> =
        HashMap::new();
    let rows = stmt.query_map([], |r| {
        Ok((
            r.get::<_, String>(0)?,
            r.get::<_, String>(1)?,
            r.get::<_, Option<Vec<u8>>>(2)?,
            r.get::<_, i64>(3)?,
            r.get::<_, i64>(4)?,
            r.get::<_, Vec<u8>>(5)?,
        ))
    })?;

    for row in rows {
        let (id, vol, fi, nlink, size, hash) = row?;
        groups
            .entry(hash)
            .or_default()
            .push((id, vol, fi, nlink, size));
    }

    let mut clusters = Vec::new();
    let mut total_reclaimable: u64 = 0;

    for (hash, members) in groups {
        if members.len() < 2 {
            continue;
        }
        let keeper_file_id = members[0].0.clone();
        let member_file_ids: Vec<String> = members.iter().map(|m| m.0.clone()).collect();
        let total_bytes: u64 = members.iter().map(|m| m.4.max(0) as u64).sum();
        let tuples: Vec<(String, Option<Vec<u8>>, i64, i64)> = members
            .iter()
            .map(|(_, v, fi, nl, sz)| (v.clone(), fi.clone(), *nl, *sz))
            .collect();
        let (reclaimable_bytes, hardlink_adjusted) = calculate_reclaimable_bytes(&tuples, 0);
        total_reclaimable = total_reclaimable.saturating_add(reclaimable_bytes);

        let key = hash.iter().take(8).map(|b| format!("{b:02x}")).collect();
        clusters.push(DupeCluster {
            cluster_key: key,
            keeper_file_id,
            member_file_ids,
            total_bytes,
            reclaimable_bytes,
            hardlink_adjusted,
        });
    }
    clusters.sort_by(|a, b| {
        b.reclaimable_bytes
            .cmp(&a.reclaimable_bytes)
            .then_with(|| a.cluster_key.cmp(&b.cluster_key))
    });

    Ok(DupeReport {
        group_by: "exact".to_string(),
        clusters,
        total_reclaimable_bytes: total_reclaimable,
    })
}

pub fn disk_rescue_report(store: &SqliteStore) -> Result<DiskRescueReport> {
    let conn = store.conn();
    let dupes = exact_dedupe_report(conn)?;

    let mut cstmt = conn.prepare(
        "SELECT id, volume_id, atomic_unit_id, path, cache_kind, size_bytes, file_count, source_max_mtime, state
         FROM cache_dirs WHERE state = 'active' ORDER BY size_bytes DESC",
    )?;
    let mut cache_dirs = Vec::new();
    for r in cstmt.query_map([], |r| {
        Ok(CacheDirEntry {
            id: r.get(0)?,
            volume_id: r.get(1)?,
            atomic_unit_id: r.get(2)?,
            path: r.get(3)?,
            cache_kind: r.get(4)?,
            size_bytes: r.get(5)?,
            file_count: r.get(6)?,
            source_max_mtime: r.get(7)?,
            state: r.get(8)?,
        })
    })? {
        cache_dirs.push(r?);
    }

    let stale_cutoff = now_ms() - 180i64 * 24 * 60 * 60 * 1000;
    let oversized_threshold = 500i64 * 1024 * 1024;
    let mut fstmt = conn.prepare(
        "SELECT id, volume_id, path, name, ext, size, mtime, nlink, state, cloud_state,
                sensitivity, atomic_unit_id, unit_split_override
         FROM files
         WHERE deleted_at IS NULL AND state = 'active'
           AND size >= ?1 AND mtime <= ?2
         ORDER BY size DESC LIMIT 50",
    )?;
    let mut stale_oversized_files = Vec::new();
    for r in fstmt.query_map(params![oversized_threshold, stale_cutoff], |r| {
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
    })? {
        stale_oversized_files.push(r?);
    }

    let space_summary = space_summary(store, None)?;
    Ok(DiskRescueReport {
        exact_dupe_clusters: dupes.clusters,
        cache_dirs,
        stale_oversized_files,
        space_summary,
    })
}

pub fn health_breakdown(conn: &Connection) -> Result<HealthBreakdown> {
    let now = now_ms();
    let dead_cutoff = now - 180i64 * 24 * 60 * 60 * 1000;
    let dead_min_bytes = 100i64 * 1024 * 1024;
    let expiry_soon = now + 3i64 * 24 * 60 * 60 * 1000;

    // 1. Storage headroom & drive SMART/read health (`North Star §2.2 #5`).
    let mut low_headroom_vols = 0usize;
    let mut worst_headroom_penalty = 0.0f64;
    {
        let mut st = conn.prepare(
            "SELECT v.id,
                    (SELECT used_bytes FROM space_samples s WHERE s.volume_id = v.id ORDER BY day DESC LIMIT 1),
                    (SELECT total_bytes FROM space_samples s WHERE s.volume_id = v.id ORDER BY day DESC LIMIT 1)
             FROM volumes v WHERE v.status = 'online'",
        )?;
        for row in st.query_map([], |r| Ok((r.get::<_, Option<i64>>(1)?, r.get::<_, Option<i64>>(2)?)))? {
            if let (Some(used), Some(total)) = row?
                && total > 0
            {
                let free_pct = ((total - used).max(0) as f64 / total as f64) * 100.0;
                if free_pct < 15.0 {
                    low_headroom_vols += 1;
                    worst_headroom_penalty = worst_headroom_penalty.max((15.0 - free_pct) * 4.0);
                }
            }
        }
    }
    let drive_alerts: i64 = conn.query_row(
        "SELECT COUNT(*) FROM reminders WHERE kind IN ('drive_smart', 'drive_errors') AND resolved_at IS NULL",
        [],
        |r| r.get(0),
    )?;
    let storage_score = (100.0 - worst_headroom_penalty - (drive_alerts as f64) * 25.0).clamp(15.0, 100.0);
    let storage_offenders = low_headroom_vols + drive_alerts.max(0) as usize;

    // 2. Backup coverage & security/integrity posture.
    let open_sec: i64 = conn.query_row(
        "SELECT COUNT(*) FROM security_findings WHERE status = 'open'",
        [],
        |r| r.get(0),
    )?;
    let (backup_shortfalls, unprotected_penalty) = match crate::backup::priority::coverage_summary(conn)? {
        Some(cov) => {
            let unprot: i64 = cov.tiers.iter().filter(|t| t.tier == 1 || t.tier == 2).map(|t| t.unprotected_bytes).sum();
            let total: i64 = cov.tiers.iter().filter(|t| t.tier == 1 || t.tier == 2).map(|t| t.protected_bytes + t.unprotected_bytes).sum();
            if unprot > 0 && total > 0 {
                let ratio = (unprot as f64 / total as f64).clamp(0.0, 1.0);
                (1usize, ratio * 35.0)
            } else {
                (0usize, 0.0)
            }
        }
        None => (0usize, 0.0),
    };
    let backup_score = (100.0 - unprotected_penalty - (open_sec as f64) * 15.0).clamp(10.0, 100.0);
    let backup_offenders = backup_shortfalls + open_sec.max(0) as usize;

    // 3. Exact duplicate hygiene (SQL group count).
    let dupe_offenders: i64 = conn.query_row(
        "SELECT COUNT(*) FROM (
            SELECT blake3 FROM files
            WHERE deleted_at IS NULL AND state NOT IN ('quarantine','missing')
              AND blake3 IS NOT NULL AND size > 0
            GROUP BY blake3 HAVING COUNT(*) > 1)",
        [],
        |r| r.get(0),
    )?;
    let dup_score = (100.0 - (dupe_offenders as f64) * 5.0).max(20.0);

    // 4. Build caches & stale unowned large dead files (`North Star §2.2 #4`).
    let cache_count: i64 = conn.query_row(
        "SELECT COUNT(*) FROM cache_dirs WHERE state = 'active'",
        [],
        |r| r.get(0),
    )?;
    let dead_large_count: i64 = conn.query_row(
        "SELECT COUNT(*) FROM files f
         JOIN volumes v ON v.id = f.volume_id
         WHERE f.deleted_at IS NULL AND f.state = 'active'
           AND v.role != 'backup' AND f.atomic_unit_id IS NULL
           AND f.size >= ?1 AND f.mtime <= ?2
           AND NOT EXISTS (SELECT 1 FROM collection_members cm WHERE cm.file_id = f.id)",
        params![dead_min_bytes, dead_cutoff],
        |r| r.get(0),
    )?;
    let junk_count = cache_count + dead_large_count;
    let hygiene_score = (100.0 - (junk_count as f64) * 4.0).max(25.0);

    // 5. Pending actions & soon-to-expire quarantine reviews.
    let pending_actions: i64 = conn.query_row(
        "SELECT COUNT(*) FROM actions WHERE status = 'proposed'",
        [],
        |r| r.get(0),
    )?;
    let expiring_q: i64 = conn.query_row(
        "SELECT COUNT(*) FROM quarantine_entries
         WHERE restored_at IS NULL AND purged_at IS NULL AND expires_at <= ?1",
        params![expiry_soon],
        |r| r.get(0),
    )?;
    let review_count = pending_actions + expiring_q;
    let org_score = (100.0 - (review_count as f64) * 2.0).max(35.0);

    let overall = ((storage_score + backup_score + dup_score + hygiene_score + org_score) / 5.0 * 10.0)
        .round()
        / 10.0;

    Ok(HealthBreakdown {
        overall_score: overall,
        dimensions: vec![
            HealthDimension {
                id: "storage".into(),
                label: "Disk Headroom & Drive Health".into(),
                score: storage_score,
                offenders_count: storage_offenders,
            },
            HealthDimension {
                id: "backup".into(),
                label: "Backup & Integrity Protection".into(),
                score: backup_score,
                offenders_count: backup_offenders,
            },
            HealthDimension {
                id: "duplication".into(),
                label: "Duplication Hygiene".into(),
                score: dup_score,
                offenders_count: dupe_offenders.max(0) as usize,
            },
            HealthDimension {
                id: "caches".into(),
                label: "Build Caches & Dead Files".into(),
                score: hygiene_score,
                offenders_count: junk_count.max(0) as usize,
            },
            HealthDimension {
                id: "organization".into(),
                label: "Pending Reviews & Quarantine".into(),
                score: org_score,
                offenders_count: review_count.max(0) as usize,
            },
        ],
    })
}
