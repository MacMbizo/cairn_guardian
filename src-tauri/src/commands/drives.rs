//! Drives & Duplicates IPC commands (spec 2026-11-21). Long jobs (catalog, top-up) run on
//! their own thread with their own database connection and report through `aegis://event`,
//! so they never hold the UI's store lock.

use super::{AEGIS_EVENT, AppState, lock_store, map_err};
use aegis_core::backup::guard::override_halt;
use aegis_core::backup::plan::{AppliedPlan, DrivePlan, apply_plan, recommend_plan};
use aegis_core::backup::priority::{
    BackupCoverage, BackupSourceItem, add_source_path, coverage_summary, list_sources, remove_source,
};
use aegis_core::backup::topup::{
    PreviousVersionItem, TopUpAuthority, TopUpPlan, list_previous_versions, plan_topup,
    restore_previous_version,
};
use aegis_core::contracts::{AegisEvent, DriveRole, GroupedDupeReport, VolumeInfo};
use aegis_core::db::SqliteStore;
use aegis_core::drives::{RefreshReport, SyncReport, refresh_volume, sync_mounted};
use aegis_core::dupes::decisions::{
    DecisionOutcome, DupeChoice, DupeDecisionItem, decide, list_decisions, undo_decision,
};
use aegis_core::dupes::grouped_report;
use aegis_core::error::{AegisError, IpcError};
use aegis_core::fingerprint::run_fingerprint;
use aegis_core::scanner::{ScanProgress, catalog_volume};
use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, Ordering};
use tauri::{AppHandle, Emitter, Manager, State};

fn provider() -> Box<dyn aegis_core::drives::VolumeProvider> {
    #[cfg(windows)]
    {
        Box::new(aegis_core::drives::WindowsProvider)
    }
    #[cfg(not(windows))]
    {
        Box::new(aegis_core::drives::FakeVolumeProvider::new())
    }
}

fn backup_id(store: &SqliteStore) -> Result<String, IpcError> {
    map_err(aegis_core::backup::backup_volume(store.conn()))?
        .map(|(id, _, _)| id)
        .ok_or_else(|| AegisError::InvalidInput("No Backup drive assigned yet.".into()).to_ipc())
}

/// Runs `job` on a background thread with its own connection; failures become `JobFailed`.
fn background(app: AppHandle, name: &'static str, job: impl FnOnce(&AppHandle, &mut SqliteStore) -> aegis_core::error::Result<()> + Send + 'static) {
    std::thread::spawn(move || {
        let db_path = app.state::<AppState>().db_path.clone();
        let res = SqliteStore::open(&db_path).and_then(|mut store| job(&app, &mut store));
        if let Err(e) = res {
            let _ = app.emit(AEGIS_EVENT, AegisEvent::JobFailed { v: 1, job: name.into(), message: e.to_string() });
        }
    });
}

#[tauri::command(async)]
pub fn drives_list(state: State<'_, AppState>) -> Result<Vec<VolumeInfo>, IpcError> {
    let store = lock_store(&state)?;
    let vols = map_err(store.list_volumes())?;
    Ok(vols.into_iter().filter(|v| v.passport_id.is_some() || v.id.starts_with("fp-")).collect())
}

#[tauri::command(async)]
pub fn drives_sync(state: State<'_, AppState>) -> Result<SyncReport, IpcError> {
    let mut store = lock_store(&state)?;
    map_err(sync_mounted(&mut store, provider().as_ref()))
}

/// Catalogs a whole drive (metadata walk) and fingerprints duplicate candidates, in the background.
#[tauri::command(async)]
pub fn drive_catalog(app: AppHandle, volume_id: String) -> Result<(), IpcError> {
    background(app, "drive_catalog", move |app, store| {
        let mut emit_progress = |p: &ScanProgress| {
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
        let status = catalog_volume(store, &volume_id, &mut emit_progress)?;
        run_fingerprint(store, &AtomicBool::new(false))?;
        let _ = app.emit(
            AEGIS_EVENT,
            AegisEvent::DriveCatalogued { v: 1, volume_id: volume_id.clone(), files: status.files_seen },
        );
        Ok(())
    });
    Ok(())
}

#[tauri::command(async)]
pub fn drive_refresh(state: State<'_, AppState>, volume_id: String) -> Result<RefreshReport, IpcError> {
    let mut store = lock_store(&state)?;
    map_err(refresh_volume(&mut store, &volume_id, &AtomicBool::new(false)))
}

#[tauri::command(async)]
pub fn drive_plan_get(state: State<'_, AppState>) -> Result<DrivePlan, IpcError> {
    let store = lock_store(&state)?;
    map_err(recommend_plan(store.conn()))
}

#[tauri::command(async)]
pub fn drive_plan_apply(state: State<'_, AppState>, roles: Vec<(String, DriveRole)>) -> Result<AppliedPlan, IpcError> {
    let mut store = lock_store(&state)?;
    map_err(apply_plan(&mut store, &roles))
}

#[tauri::command(async)]
pub fn dupes_grouped(state: State<'_, AppState>) -> Result<GroupedDupeReport, IpcError> {
    let store = lock_store(&state)?;
    map_err(grouped_report(store.conn()))
}

#[tauri::command(async)]
pub fn dupe_decide(state: State<'_, AppState>, group_id: String, choice: DupeChoice) -> Result<DecisionOutcome, IpcError> {
    let mut store = lock_store(&state)?;
    map_err(decide(&mut store, &group_id, choice))
}

#[tauri::command(async)]
pub fn dupe_decisions_list(state: State<'_, AppState>) -> Result<Vec<DupeDecisionItem>, IpcError> {
    let store = lock_store(&state)?;
    map_err(list_decisions(store.conn()))
}

#[tauri::command(async)]
pub fn dupe_decision_undo(state: State<'_, AppState>, id: String) -> Result<(), IpcError> {
    let mut store = lock_store(&state)?;
    map_err(undo_decision(&mut store, &id))
}

#[tauri::command(async)]
pub fn backup_coverage(state: State<'_, AppState>) -> Result<Option<BackupCoverage>, IpcError> {
    let store = lock_store(&state)?;
    map_err(coverage_summary(store.conn()))
}

#[tauri::command(async)]
pub fn backup_sources_list(state: State<'_, AppState>) -> Result<Vec<BackupSourceItem>, IpcError> {
    let store = lock_store(&state)?;
    map_err(list_sources(store.conn()))
}

#[tauri::command(async)]
pub fn backup_source_add(state: State<'_, AppState>, path: String, pinned: bool) -> Result<String, IpcError> {
    let mut store = lock_store(&state)?;
    map_err(add_source_path(&mut store, &PathBuf::from(path), pinned))
}

/// Returns the bytes that will stop being backed up (kept 90 days in previous versions).
#[tauri::command(async)]
pub fn backup_source_remove(state: State<'_, AppState>, id: String) -> Result<i64, IpcError> {
    let mut store = lock_store(&state)?;
    map_err(remove_source(&mut store, &id))
}

#[tauri::command(async)]
pub fn backup_topup_plan(state: State<'_, AppState>) -> Result<TopUpPlan, IpcError> {
    let store = lock_store(&state)?;
    let id = backup_id(&store)?;
    map_err(plan_topup(store.conn(), &id))
}

/// Starts a top-up in the background; progress arrives as `TopUpProgress` / `TopUpFinished`.
#[tauri::command(async)]
pub fn backup_topup_start(app: AppHandle, state: State<'_, AppState>, authority: TopUpAuthority) -> Result<(), IpcError> {
    let id = {
        let store = lock_store(&state)?;
        backup_id(&store)?
    };
    if state.topup_running.load(Ordering::SeqCst) {
        return Err(AegisError::InvalidInput("A backup is already running.".into()).to_ipc());
    }
    background(app, "backup_topup", move |app, store| {
        crate::watch::run_topup_job(app, store, &id, authority);
        Ok(())
    });
    Ok(())
}

#[tauri::command(async)]
pub fn backup_topup_pause(state: State<'_, AppState>) -> Result<(), IpcError> {
    state.topup_cancel.store(true, Ordering::SeqCst);
    Ok(())
}

#[tauri::command(async)]
pub fn backup_guard_override(state: State<'_, AppState>, folder: String) -> Result<(), IpcError> {
    let mut store = lock_store(&state)?;
    let id = backup_id(&store)?;
    map_err(override_halt(&mut store, &id, &folder))
}

#[tauri::command(async)]
pub fn backup_standing_grant(state: State<'_, AppState>) -> Result<String, IpcError> {
    let mut store = lock_store(&state)?;
    let id = backup_id(&store)?;
    map_err(aegis_core::policy::grant_standing_topup(&mut store, &id))
}

#[tauri::command(async)]
pub fn backup_standing_status(state: State<'_, AppState>) -> Result<bool, IpcError> {
    let store = lock_store(&state)?;
    let Ok(id) = backup_id(&store) else { return Ok(false) };
    map_err(aegis_core::policy::has_standing_topup(store.conn(), &id))
}

#[tauri::command(async)]
pub fn previous_versions_list(state: State<'_, AppState>) -> Result<Vec<PreviousVersionItem>, IpcError> {
    let store = lock_store(&state)?;
    let id = backup_id(&store)?;
    map_err(list_previous_versions(store.conn(), &id))
}

#[tauri::command(async)]
pub fn previous_version_restore(state: State<'_, AppState>, id: String, to: Option<String>) -> Result<String, IpcError> {
    let mut store = lock_store(&state)?;
    let out = map_err(restore_previous_version(&mut store, &id, to.map(PathBuf::from)))?;
    Ok(out.to_string_lossy().to_string())
}
