//! Tauri 2 IPC command handlers aligned with `C1_IPC_Action_Contracts.md` (v1.2).

use aegis_core::contracts::{
    ActionItem, ActionStatus, AegisEvent, AtomicUnit, AuditEntry, ConflictResolutionChoice,
    CoverageReport,
    DiskRescueReport, DoctorReport, DupeReport, ExecuteResult, FileDetail, HardwareProfile,
    HealthBreakdown, IndexStats, LifecycleState, QuarantineEntry, QuarantineSpaceSummary,
    RefilePlan, RollbackResult, RootConfig, ScanStatus, SearchFilters, SearchPage, Simulation,
    VolumeInfo,
};
use aegis_core::db::SqliteStore;
use aegis_core::db::actions::{
    approve_batch, disk_rescue_report, exact_dedupe_report, execute_batch, health_breakdown,
    list_actions, propose_batch, propose_dupe_quarantine, reject_batch, resolve_rollback_conflict,
    rollback_batch, simulate_batch,
};
use aegis_core::db::audit::{append_audit, list_audit};
use aegis_core::db::files::{get_file_detail, search_fts, set_file_lifecycle};
use aegis_core::doctor::run_doctor;
use aegis_core::error::{AegisError, IpcError};
use aegis_core::quarantine::{list_quarantine, purge_entries, restore_entries, space_summary};
use aegis_core::scanner::{get_coverage_report, latest_scan_status, scan_root_with_progress};
use aegis_embed::detect_hardware_capabilities;
use rusqlite::params;
use std::path::PathBuf;
use std::sync::Mutex;
use tauri::{AppHandle, Emitter, State};

/// Event channel carrying `AegisEvent` payloads to the UI (`C1` §5).
pub const AEGIS_EVENT: &str = "aegis://event";

/// All commands are `async` so they run on Tauri's worker pool, never the UI thread; a long
/// scan therefore cannot freeze the window. The single SQLite writer still serializes them.
pub struct AppState {
    pub store: Mutex<SqliteStore>,
}

fn lock_store<'r>(
    state: &'r State<'_, AppState>,
) -> Result<std::sync::MutexGuard<'r, SqliteStore>, IpcError> {
    state.store.lock().map_err(|_| IpcError {
        code: "database_error".into(),
        message: "Failed to acquire SQLite store lock".into(),
        retryable: true,
        remediation: None,
    })
}

fn map_err<T>(res: aegis_core::error::Result<T>) -> Result<T, IpcError> {
    res.map_err(|e| e.to_ipc())
}

#[tauri::command(async)]
pub fn roots_list(state: State<'_, AppState>) -> Result<Vec<RootConfig>, IpcError> {
    let store = lock_store(&state)?;
    map_err(store.list_roots())
}

#[tauri::command(async)]
pub fn roots_add(state: State<'_, AppState>, path: String) -> Result<RootConfig, IpcError> {
    let store = lock_store(&state)?;
    map_err(store.add_root(&PathBuf::from(path)))
}

#[tauri::command(async)]
pub fn roots_remove(state: State<'_, AppState>, path: String) -> Result<(), IpcError> {
    let store = lock_store(&state)?;
    map_err(store.remove_root(&path))
}

#[tauri::command(async)]
pub fn roots_toggle(
    state: State<'_, AppState>,
    path: String,
    enabled: bool,
) -> Result<(), IpcError> {
    let store = lock_store(&state)?;
    map_err(store.toggle_root(&path, enabled))
}

#[tauri::command(async)]
pub fn ignore_get(state: State<'_, AppState>, root: Option<String>) -> Result<String, IpcError> {
    let store = lock_store(&state)?;
    let key = match root {
        Some(r) => format!("ignore.root:{r}"),
        None => "ignore.global".to_string(),
    };
    map_err(store.get_setting(&key, "global")).map(|opt| opt.unwrap_or_default())
}

#[tauri::command(async)]
pub fn ignore_set(
    state: State<'_, AppState>,
    content: String,
    root: Option<String>,
) -> Result<(), IpcError> {
    let store = lock_store(&state)?;
    let key = match root {
        Some(r) => format!("ignore.root:{r}"),
        None => "ignore.global".to_string(),
    };
    let json = serde_json::to_string(&content).map_err(|e| AegisError::Serde(e).to_ipc())?;
    map_err(store.set_setting(&key, "global", &json))
}

#[tauri::command(async)]
pub fn scan_start(
    app: AppHandle,
    state: State<'_, AppState>,
    roots: Option<Vec<String>>,
    tiers: Option<Vec<u8>>,
) -> Result<Vec<ScanStatus>, IpcError> {
    let mut store = lock_store(&state)?;
    let target_roots: Vec<PathBuf> = match roots {
        Some(list) if !list.is_empty() => list.into_iter().map(PathBuf::from).collect(),
        _ => map_err(store.list_roots())?
            .into_iter()
            .filter(|r| r.enabled)
            .map(|r| PathBuf::from(r.path))
            .collect(),
    };
    let tier_list = tiers.unwrap_or_else(|| vec![1, 2]);
    let mut statuses = Vec::with_capacity(target_roots.len());
    let mut emit_progress = |p: &aegis_core::scanner::ScanProgress| {
        let _ = app.emit(
            AEGIS_EVENT,
            AegisEvent::ScanProgress {
                v: 1,
                job_id: p.job_id.clone(),
                files_seen: p.files_seen,
                files_indexed: p.files_indexed,
                coverage_pct: p.coverage_pct,
            },
        );
    };
    for root in target_roots {
        let status = map_err(scan_root_with_progress(
            &mut store,
            &root,
            &tier_list,
            &mut emit_progress,
        ))?;
        let _ = app.emit(
            AEGIS_EVENT,
            AegisEvent::IndexUpdated {
                v: 1,
                root_path: status.root_path.clone(),
                files_changed: status.files_indexed.max(0) as usize,
            },
        );
        statuses.push(status);
    }
    Ok(statuses)
}

#[tauri::command(async)]
pub fn scan_status(
    state: State<'_, AppState>,
    job: Option<String>,
) -> Result<Option<ScanStatus>, IpcError> {
    let store = lock_store(&state)?;
    map_err(latest_scan_status(store.conn(), job.as_deref()))
}

#[tauri::command(async)]
pub fn scan_coverage_report(
    state: State<'_, AppState>,
    job: Option<String>,
) -> Result<CoverageReport, IpcError> {
    let store = lock_store(&state)?;
    map_err(get_coverage_report(store.conn(), job.as_deref()))
}

#[tauri::command(async)]
pub fn volumes_list(state: State<'_, AppState>) -> Result<Vec<VolumeInfo>, IpcError> {
    let store = lock_store(&state)?;
    map_err(store.list_volumes())
}

#[tauri::command(async)]
pub fn index_stats(state: State<'_, AppState>) -> Result<IndexStats, IpcError> {
    let store = lock_store(&state)?;
    map_err(store.index_stats())
}

#[tauri::command(async)]
pub fn search_query(
    state: State<'_, AppState>,
    q: String,
    filters: Option<SearchFilters>,
    limit: Option<usize>,
) -> Result<SearchPage, IpcError> {
    let store = lock_store(&state)?;
    map_err(search_fts(
        store.conn(),
        &q,
        &filters.unwrap_or_default(),
        limit.unwrap_or(20),
    ))
}

#[tauri::command(async)]
pub fn file_get(state: State<'_, AppState>, id: String) -> Result<FileDetail, IpcError> {
    let store = lock_store(&state)?;
    map_err(get_file_detail(store.conn(), &id))
}

#[tauri::command(async)]
pub fn file_set_lifecycle(
    state: State<'_, AppState>,
    id: String,
    lifecycle: LifecycleState,
) -> Result<(), IpcError> {
    let store = lock_store(&state)?;
    map_err(set_file_lifecycle(store.conn(), &id, lifecycle))
}

#[tauri::command(async)]
pub fn atomic_unit_list(
    state: State<'_, AppState>,
    volume_id: Option<String>,
) -> Result<Vec<AtomicUnit>, IpcError> {
    let store = lock_store(&state)?;
    let conn = store.conn();
    let res: aegis_core::error::Result<Vec<AtomicUnit>> = (|| {
        let sql = match volume_id {
            Some(_) => {
                "SELECT id, volume_id, root_path, unit_type, marker_file, project_id, allow_split_override
                 FROM atomic_units WHERE volume_id = ?1 ORDER BY root_path"
            }
            None => {
                "SELECT id, volume_id, root_path, unit_type, marker_file, project_id, allow_split_override
                 FROM atomic_units ORDER BY root_path"
            }
        };
        let mut stmt = conn.prepare(sql)?;
        let map_row = |r: &rusqlite::Row<'_>| -> rusqlite::Result<AtomicUnit> {
            Ok(AtomicUnit {
                id: r.get(0)?,
                volume_id: r.get(1)?,
                root_path: r.get(2)?,
                unit_type: r.get(3)?,
                marker_file: r.get(4)?,
                project_id: r.get(5)?,
                allow_split_override: r.get::<_, i32>(6)? != 0,
            })
        };
        let mut out = Vec::new();
        if let Some(vid) = volume_id {
            for r in stmt.query_map(params![vid], map_row)? {
                out.push(r?);
            }
        } else {
            for r in stmt.query_map([], map_row)? {
                out.push(r?);
            }
        }
        Ok(out)
    })();
    map_err(res)
}

#[tauri::command(async)]
pub fn atomic_unit_override(
    state: State<'_, AppState>,
    unit_id: Option<String>,
    file_id: Option<String>,
    allow_split: bool,
) -> Result<(), IpcError> {
    let mut store = lock_store(&state)?;
    let flag = i32::from(allow_split);
    let now = aegis_core::now_ms();
    if let Some(uid) = unit_id {
        map_err(store.conn().execute(
            "UPDATE atomic_units SET allow_split_override = ?1, updated_at = ?2 WHERE id = ?3",
            params![flag, now, uid],
        ).map_err(AegisError::Database))?;
        map_err(append_audit(
            store.conn_mut(),
            "user",
            "atomic_unit.override",
            &uid,
            &serde_json::json!({ "allow_split": allow_split }).to_string(),
        ))?;
        return Ok(());
    }
    if let Some(fid) = file_id {
        map_err(store.conn().execute(
            "UPDATE files SET unit_split_override = ?1, updated_at = ?2 WHERE id = ?3",
            params![flag, now, fid],
        ).map_err(AegisError::Database))?;
        map_err(append_audit(
            store.conn_mut(),
            "user",
            "atomic_unit.file_override",
            &fid,
            &serde_json::json!({ "allow_split": allow_split }).to_string(),
        ))?;
        return Ok(());
    }
    Err(AegisError::InvalidInput("Provide unitId or fileId".into()).to_ipc())
}

#[tauri::command(async)]
pub fn dedupe_report(state: State<'_, AppState>) -> Result<DupeReport, IpcError> {
    let store = lock_store(&state)?;
    map_err(exact_dedupe_report(store.conn()))
}

#[tauri::command(async)]
pub fn dedupe_propose_quarantine(
    state: State<'_, AppState>,
    keeper_file_id: String,
    member_file_ids: Vec<String>,
) -> Result<String, IpcError> {
    let mut store = lock_store(&state)?;
    map_err(propose_dupe_quarantine(
        &mut store,
        &keeper_file_id,
        &member_file_ids,
    ))
}

#[tauri::command(async)]
pub fn actions_propose(state: State<'_, AppState>, plan: RefilePlan) -> Result<String, IpcError> {
    let mut store = lock_store(&state)?;
    map_err(propose_batch(&mut store, &plan))
}

#[tauri::command(async)]
pub fn actions_simulate(
    state: State<'_, AppState>,
    batch: String,
) -> Result<Simulation, IpcError> {
    let store = lock_store(&state)?;
    map_err(simulate_batch(&store, &batch))
}

#[tauri::command(async)]
pub fn actions_approve(
    state: State<'_, AppState>,
    batch: String,
    selection: Option<Vec<String>>,
) -> Result<(), IpcError> {
    let mut store = lock_store(&state)?;
    map_err(approve_batch(&mut store, &batch, selection.as_deref()))
}

#[tauri::command(async)]
pub fn actions_execute(
    state: State<'_, AppState>,
    batch: String,
) -> Result<ExecuteResult, IpcError> {
    let mut store = lock_store(&state)?;
    map_err(execute_batch(&mut store, &batch))
}

#[tauri::command(async)]
pub fn actions_reject(
    state: State<'_, AppState>,
    batch: String,
    reason: Option<String>,
) -> Result<(), IpcError> {
    let mut store = lock_store(&state)?;
    map_err(reject_batch(&mut store, &batch, reason.as_deref()))
}

#[tauri::command(async)]
pub fn actions_rollback(
    state: State<'_, AppState>,
    batch: String,
) -> Result<RollbackResult, IpcError> {
    let mut store = lock_store(&state)?;
    map_err(rollback_batch(&mut store, &batch))
}

#[tauri::command(async)]
pub fn actions_resolve_conflict(
    state: State<'_, AppState>,
    action_id: String,
    choice: ConflictResolutionChoice,
) -> Result<RollbackResult, IpcError> {
    let mut store = lock_store(&state)?;
    map_err(resolve_rollback_conflict(&mut store, &action_id, choice))
}

#[tauri::command(async)]
pub fn actions_list(
    state: State<'_, AppState>,
    status: Option<ActionStatus>,
) -> Result<Vec<ActionItem>, IpcError> {
    let store = lock_store(&state)?;
    map_err(list_actions(store.conn(), status))
}

#[tauri::command(async)]
pub fn quarantine_list(
    state: State<'_, AppState>,
    volume_id: Option<String>,
) -> Result<Vec<QuarantineEntry>, IpcError> {
    let store = lock_store(&state)?;
    map_err(list_quarantine(store.conn(), volume_id.as_deref()))
}

#[tauri::command(async)]
pub fn quarantine_space_summary(
    state: State<'_, AppState>,
    volume_id: Option<String>,
) -> Result<Vec<QuarantineSpaceSummary>, IpcError> {
    let store = lock_store(&state)?;
    map_err(space_summary(&store, volume_id.as_deref()))
}

#[tauri::command(async)]
pub fn quarantine_restore(
    state: State<'_, AppState>,
    entry_ids: Vec<String>,
) -> Result<String, IpcError> {
    let mut store = lock_store(&state)?;
    map_err(restore_entries(&mut store, &entry_ids))
}

#[tauri::command(async)]
pub fn quarantine_purge(
    state: State<'_, AppState>,
    entry_ids: Vec<String>,
    confirmed_permanent: bool,
) -> Result<String, IpcError> {
    let mut store = lock_store(&state)?;
    map_err(purge_entries(&mut store, &entry_ids, confirmed_permanent))
}

#[tauri::command(async)]
pub fn health_get(state: State<'_, AppState>) -> Result<HealthBreakdown, IpcError> {
    let store = lock_store(&state)?;
    map_err(health_breakdown(store.conn()))
}

#[tauri::command(async)]
pub fn playbook_disk_rescue(state: State<'_, AppState>) -> Result<DiskRescueReport, IpcError> {
    let store = lock_store(&state)?;
    map_err(disk_rescue_report(&store))
}

#[tauri::command(async)]
pub fn audit_list(
    state: State<'_, AppState>,
    limit: Option<i64>,
    offset: Option<i64>,
) -> Result<Vec<AuditEntry>, IpcError> {
    let store = lock_store(&state)?;
    map_err(list_audit(
        store.conn(),
        limit.unwrap_or(50),
        offset.unwrap_or(0),
    ))
}

#[tauri::command(async)]
pub fn hw_profile() -> HardwareProfile {
    detect_hardware_capabilities().profile
}

#[tauri::command(async)]
pub fn doctor_run(
    state: State<'_, AppState>,
    repair: Option<bool>,
) -> Result<DoctorReport, IpcError> {
    let mut store = lock_store(&state)?;
    map_err(run_doctor(&mut store, repair.unwrap_or(false)))
}
