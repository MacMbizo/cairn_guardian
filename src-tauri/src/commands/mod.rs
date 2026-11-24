//! Tauri 2 IPC command handlers aligned with `C1_IPC_Action_Contracts.md` (v1.2).

pub mod assets;
pub mod care;
pub mod context;
pub mod cross_project;
pub mod drives;
pub mod embed;
pub mod engine;
pub mod filing;
pub mod folderless;
pub mod healing;
pub mod knowledge;
pub mod librarian;
pub mod rag;
pub mod vault;
pub mod versions;

use aegis_core::contracts::{
    ActionItem, ActionStatus, AegisEvent, AtomicUnit, AuditEntry, ConflictResolutionChoice,
    CoverageReport,
    DiskRescueReport, DoctorReport, DupeReport, ExecuteResult, FileDetail, HardwareProfile,
    HealthBreakdown, IndexStats, LifecycleState, QuarantineEntry, QuarantineSpaceSummary,
    RefilePlan, RollbackResult, RootConfig, ScanStatus, SearchFilters, SearchMode, SearchPage,
    Simulation, VolumeInfo,
};
use aegis_core::db::SqliteStore;
use aegis_core::db::actions::{
    approve_batch, disk_rescue_report, exact_dedupe_report, execute_batch, health_breakdown,
    list_actions, propose_batch, propose_dupe_quarantine, reject_batch, resolve_rollback_conflict,
    rollback_batch, simulate_batch,
};
use aegis_core::db::audit::{append_audit, list_audit};
use aegis_core::db::crypto::DbKey;
use aegis_core::db::files::{
    get_file_detail_with_options, set_file_lifecycle, set_file_sensitivity,
};
use aegis_core::db::search::search_hybrid;
use aegis_core::db::vec::vec_coverage;
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
    /// Background jobs (watcher, catalog, top-up) open their own encrypted connection to this file.
    pub db_path: PathBuf,
    /// In-memory database key held once DPAPI-unsealed or unlocked via recovery phrase.
    pub db_key: Mutex<Option<std::sync::Arc<DbKey>>>,
    pub vault_locked: std::sync::atomic::AtomicBool,
    pub vault_lock_reason: Mutex<Option<String>>,
    pub vault_migrated: std::sync::atomic::AtomicBool,
    /// Epoch ms until which sensitive file previews are unlocked (`0` = locked).
    pub sensitive_unlocked_until: std::sync::atomic::AtomicI64,
    /// Receiver parked when startup finds a locked vault; consumed once unlocked.
    pub pending_engine_rx: Mutex<Option<std::sync::mpsc::Receiver<crate::engine_host::EngineMsg>>>,
    /// On-demand local embedding engine and single-flight indexing lock (`K3`, `K5`, `R4`).
    pub embedder: std::sync::Arc<aegis_embed::OnDemandEmbedder>,
    pub embed_running: std::sync::Arc<std::sync::atomic::AtomicBool>,
    /// On-demand local RAG LLM engine and cooperative stream cancellation (`K3`, `K5`, `R4`).
    pub llm: std::sync::Arc<aegis_llm::OnDemandLlm>,
    pub rag_running: std::sync::Arc<std::sync::atomic::AtomicBool>,
    pub rag_cancel: std::sync::Arc<std::sync::atomic::AtomicBool>,
    /// Set to pause the running top-up; it stops cleanly and resumes next time.
    pub topup_cancel: std::sync::Arc<std::sync::atomic::AtomicBool>,
    pub topup_running: std::sync::Arc<std::sync::atomic::AtomicBool>,
    /// Drives with a catalog running, so a repeat click never starts a second walk.
    pub cataloguing: InFlight,
    /// Screen a notification or the tray asked for, until the page takes it.
    pub nav: crate::shell::NavSlot,
    /// Pause end (ms since epoch); 0 or past = not paused.
    pub paused_until: std::sync::atomic::AtomicI64,
    pub shortcut_ok: std::sync::atomic::AtomicBool,
    /// Wakes the engine thread and carries file-system events to it.
    pub engine_tx: Mutex<std::sync::mpsc::Sender<crate::engine_host::EngineMsg>>,
    pub tray_items: Mutex<Option<crate::shell::TrayItems>>,
}

impl AppState {
    /// Opens an independent SQLCipher-encrypted connection for background workers/commands.
    pub fn open_store(&self) -> aegis_core::error::Result<SqliteStore> {
        if self.vault_locked.load(std::sync::atomic::Ordering::SeqCst) {
            return Err(AegisError::VaultLocked(
                "Vault is locked — enter your 24-word recovery key.".into(),
            ));
        }
        let key = self
            .db_key
            .lock()
            .ok()
            .and_then(|g| g.clone())
            .ok_or_else(|| AegisError::Vault("Database key unavailable.".into()))?;
        SqliteStore::open_encrypted(&self.db_path, &key)
    }

    pub fn is_sensitive_unlocked(&self) -> bool {
        !self.vault_locked.load(std::sync::atomic::Ordering::SeqCst)
            && self.sensitive_unlocked_until.load(std::sync::atomic::Ordering::SeqCst) > aegis_core::now_ms()
    }
}

/// A set of ids with a job in flight. `try_start` hands out a guard that removes the id
/// when dropped, so a failed or panicking job never leaves its id stuck.
#[derive(Clone, Default)]
pub struct InFlight(std::sync::Arc<Mutex<std::collections::HashSet<String>>>);

pub struct InFlightGuard {
    set: InFlight,
    id: String,
}

impl InFlight {
    /// `None` when a job for `id` is already running.
    pub fn try_start(&self, id: &str) -> Option<InFlightGuard> {
        let mut set = self.0.lock().unwrap_or_else(|p| p.into_inner());
        set.insert(id.to_string()).then(|| InFlightGuard { set: self.clone(), id: id.to_string() })
    }

    pub fn ids(&self) -> Vec<String> {
        let set = self.0.lock().unwrap_or_else(|p| p.into_inner());
        let mut ids: Vec<String> = set.iter().cloned().collect();
        ids.sort();
        ids
    }
}

impl Drop for InFlightGuard {
    fn drop(&mut self) {
        let mut set = self.set.0.lock().unwrap_or_else(|p| p.into_inner());
        set.remove(&self.id);
    }
}

#[cfg(test)]
mod in_flight_tests {
    use super::InFlight;

    #[test]
    fn second_start_for_same_id_is_refused_until_the_first_ends() {
        let jobs = InFlight::default();
        let first = jobs.try_start("d1").expect("first start");
        assert!(jobs.try_start("d1").is_none(), "repeat click must not start a second job");
        assert!(jobs.try_start("d2").is_some(), "other drives are independent");
        assert_eq!(jobs.ids(), vec!["d1".to_string()]);
        drop(first);
        assert!(jobs.ids().is_empty());
        assert!(jobs.try_start("d1").is_some(), "can start again after the job ends");
    }

    #[test]
    fn guard_is_released_when_the_job_thread_panics() {
        let jobs = InFlight::default();
        let j = jobs.clone();
        let _ = std::thread::spawn(move || {
            let _g = j.try_start("d1").unwrap();
            panic!("job blew up");
        })
        .join();
        assert!(jobs.try_start("d1").is_some());
    }
}

pub(crate) fn lock_store<'r>(
    state: &'r State<'_, AppState>,
) -> Result<std::sync::MutexGuard<'r, SqliteStore>, IpcError> {
    if state.vault_locked.load(std::sync::atomic::Ordering::SeqCst) {
        return Err(IpcError {
            code: "vault_locked".into(),
            message: "Enter your 24-word recovery key to unlock the Aegis database.".into(),
            retryable: true,
            remediation: None,
        });
    }
    state.store.lock().map_err(|_| IpcError {
        code: "database_error".into(),
        message: "Failed to acquire SQLite store lock".into(),
        retryable: true,
        remediation: None,
    })
}

pub(crate) fn map_err<T>(res: aegis_core::error::Result<T>) -> Result<T, IpcError> {
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
    let reveal = state.is_sensitive_unlocked();
    let f = filters.unwrap_or_default();
    let mode = f.mode.unwrap_or(SearchMode::Hybrid);
    let lim = limit.unwrap_or(20);

    let has_vectors = if mode != SearchMode::Keyword && !q.trim().is_empty() {
        let desc = state.embedder.active_descriptor();
        let store = lock_store(&state)?;
        vec_coverage(store.conn(), &desc.id, desc.dim)
            .map(|(_, embedded)| embedded > 0)
            .unwrap_or(false)
    } else {
        false
    };

    let embedded_query = if has_vectors {
        state.embedder.embed_query(&q).ok()
    } else {
        None
    };
    let min_sim = embedded_query
        .as_ref()
        .map(|(d, _)| if d.backend == "lexical_fallback" { 0.15 } else { 0.45 })
        .unwrap_or(0.45);

    let store = lock_store(&state)?;
    let vec_arg = embedded_query
        .as_ref()
        .map(|(d, v)| (d.id.as_str(), v.as_slice(), min_sim));
    map_err(search_hybrid(store.conn(), &q, vec_arg, &f, lim, reveal))
}

#[tauri::command(async)]
pub fn file_get(state: State<'_, AppState>, id: String) -> Result<FileDetail, IpcError> {
    let reveal = state.is_sensitive_unlocked();
    let store = lock_store(&state)?;
    map_err(get_file_detail_with_options(store.conn(), &id, reveal))
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
pub fn file_set_sensitivity(
    state: State<'_, AppState>,
    id: String,
    sensitivity: String,
) -> Result<(), IpcError> {
    let store = lock_store(&state)?;
    map_err(set_file_sensitivity(store.conn(), &id, &sensitivity))
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
    let res = map_err(execute_batch(&mut store, &batch))?;
    // Filing batches also record where files were and teach the filing rules.
    if is_filing_batch(&store, &batch) {
        map_err(aegis_core::filing::batches::after_execute(&mut store, &batch))?;
    }
    Ok(res)
}

fn is_filing_batch(store: &SqliteStore, batch: &str) -> bool {
    store
        .conn()
        .query_row("SELECT origin FROM action_batches WHERE id = ?1", [batch], |r| r.get::<_, String>(0))
        .is_ok_and(|o| o.starts_with("filing"))
}

#[tauri::command(async)]
pub fn actions_reject(
    state: State<'_, AppState>,
    batch: String,
    reason: Option<String>,
) -> Result<(), IpcError> {
    let mut store = lock_store(&state)?;
    map_err(reject_batch(&mut store, &batch, reason.as_deref()))?;
    // Turning down a filing batch also sends its rules back to asking.
    if is_filing_batch(&store, &batch) {
        map_err(aegis_core::filing::batches::after_reject(&mut store, &batch))?;
    }
    Ok(())
}

#[tauri::command(async)]
pub fn actions_rollback(
    state: State<'_, AppState>,
    batch: String,
) -> Result<RollbackResult, IpcError> {
    let mut store = lock_store(&state)?;
    let res = map_err(rollback_batch(&mut store, &batch))?;
    if is_filing_batch(&store, &batch) {
        map_err(aegis_core::filing::batches::after_rollback(&mut store, &batch))?;
    }
    Ok(res)
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
