//! Self-Healing Workspace (`4.1`) Tauri 2 IPC commands (`K1`–`K5`, `R1`, `R7`, `R8`, `R9`, `D1`).

use std::sync::atomic::Ordering;

use aegis_core::contracts::{AegisEvent, DoctorReport};
use aegis_core::doctor::run_doctor;
use aegis_core::error::{AegisError, IpcError};
use aegis_core::healing::{
    HealBatchSummary, HealPatchPreviewFile, HealScanOptions, HealScanReport, dismiss_heal_issue,
    heal_folderless_rule_path, list_heal_batches, preview_heal_patches, propose_heal_batch,
    scan_workspace_healing, undismiss_heal_issue,
};
use tauri::{AppHandle, Emitter, State};

use crate::commands::{AEGIS_EVENT, AppState, lock_store, map_err};
use crate::engine_host::wake;

#[tauri::command(async)]
pub fn healing_scan(
    state: State<'_, AppState>,
    options: Option<HealScanOptions>,
) -> Result<HealScanReport, IpcError> {
    if state.vault_locked.load(Ordering::SeqCst) {
        return Err(AegisError::VaultLocked("Vault is locked.".into()).to_ipc());
    }
    let opts = options.unwrap_or_default();
    let reveal = state.is_sensitive_unlocked();
    let store = lock_store(&state)?;
    map_err(scan_workspace_healing(store.conn(), &opts, reveal))
}

#[tauri::command(async)]
pub fn healing_preview_patches(
    state: State<'_, AppState>,
    link_issue_keys: Vec<String>,
    tag_issue_keys: Vec<String>,
    include_sensitive: bool,
) -> Result<Vec<HealPatchPreviewFile>, IpcError> {
    if state.vault_locked.load(Ordering::SeqCst) {
        return Err(AegisError::VaultLocked("Vault is locked.".into()).to_ipc());
    }
    let reveal = state.is_sensitive_unlocked();
    let store = lock_store(&state)?;
    map_err(preview_heal_patches(
        &store,
        &link_issue_keys,
        &tag_issue_keys,
        include_sensitive,
        reveal,
    ))
}

#[tauri::command(async)]
pub fn healing_propose_batch(
    app: AppHandle,
    state: State<'_, AppState>,
    link_issue_keys: Vec<String>,
    tag_issue_keys: Vec<String>,
    include_sensitive: bool,
) -> Result<String, IpcError> {
    if state.vault_locked.load(Ordering::SeqCst) {
        return Err(AegisError::VaultLocked("Vault is locked.".into()).to_ipc());
    }
    let reveal = state.is_sensitive_unlocked();
    let (batch_id, count) = {
        let mut store = lock_store(&state)?;
        let batch_id = map_err(propose_heal_batch(
            &mut store,
            &link_issue_keys,
            &tag_issue_keys,
            include_sensitive,
            reveal,
        ))?;
        let count: usize = store
            .conn()
            .query_row(
                "SELECT COUNT(*) FROM actions WHERE batch_id = ?1",
                rusqlite::params![batch_id],
                |r| r.get::<_, i64>(0),
            )
            .unwrap_or(0)
            .max(0) as usize;
        (batch_id, count)
    };
    let _ = app.emit(
        AEGIS_EVENT,
        AegisEvent::ActionProposed {
            v: 1,
            batch_id: batch_id.clone(),
            count,
        },
    );
    wake(&app);
    Ok(batch_id)
}

#[tauri::command(async)]
pub fn healing_dismiss(
    state: State<'_, AppState>,
    issue_key: String,
    issue_kind: String,
    note: Option<String>,
) -> Result<(), IpcError> {
    if state.vault_locked.load(Ordering::SeqCst) {
        return Err(AegisError::VaultLocked("Vault is locked.".into()).to_ipc());
    }
    let store = lock_store(&state)?;
    map_err(dismiss_heal_issue(
        store.conn(),
        &issue_key,
        &issue_kind,
        note.as_deref(),
    ))
}

#[tauri::command(async)]
pub fn healing_undismiss(
    state: State<'_, AppState>,
    issue_key: String,
) -> Result<(), IpcError> {
    if state.vault_locked.load(Ordering::SeqCst) {
        return Err(AegisError::VaultLocked("Vault is locked.".into()).to_ipc());
    }
    let store = lock_store(&state)?;
    map_err(undismiss_heal_issue(store.conn(), &issue_key))
}

#[tauri::command(async)]
pub fn healing_update_rule_path(
    state: State<'_, AppState>,
    rule_id: String,
    field: String,
    new_path: String,
) -> Result<(), IpcError> {
    if state.vault_locked.load(Ordering::SeqCst) {
        return Err(AegisError::VaultLocked("Vault is locked.".into()).to_ipc());
    }
    let mut store = lock_store(&state)?;
    map_err(heal_folderless_rule_path(
        &mut store, &rule_id, &field, &new_path,
    ))
}

#[tauri::command(async)]
pub fn healing_recent_batches(
    state: State<'_, AppState>,
    limit: Option<usize>,
) -> Result<Vec<HealBatchSummary>, IpcError> {
    if state.vault_locked.load(Ordering::SeqCst) {
        return Err(AegisError::VaultLocked("Vault is locked.".into()).to_ipc());
    }
    let store = lock_store(&state)?;
    map_err(list_heal_batches(store.conn(), limit.unwrap_or(20)))
}

#[tauri::command(async)]
pub fn healing_run_doctor_check(
    state: State<'_, AppState>,
    repair: Option<bool>,
) -> Result<DoctorReport, IpcError> {
    if state.vault_locked.load(Ordering::SeqCst) {
        return Err(AegisError::VaultLocked("Vault is locked.".into()).to_ipc());
    }
    let mut store = lock_store(&state)?;
    map_err(run_doctor(&mut store, repair.unwrap_or(false)))
}
