//! Forgotten Asset Recovery (`4.3`) Tauri IPC commands (`K1`, `K2`, `K4`, `K5`, `R7`, `D1`).

use std::sync::atomic::Ordering;

use aegis_core::assets::{
    AssetRecoverItem, AssetScanOptions, ForgottenAssetsReport, dismiss_forgotten_asset,
    enrich_asset_metadata, propose_asset_recovery, scan_forgotten_assets,
    undismiss_forgotten_asset,
};
use aegis_core::contracts::AegisEvent;
use aegis_core::db::vec::vec_coverage;
use aegis_core::error::{AegisError, IpcError};
use aegis_core::now_ms;
use tauri::{AppHandle, Emitter, State};

use crate::commands::{AEGIS_EVENT, AppState, lock_store, map_err};
use crate::engine_host::wake;

#[tauri::command(async)]
pub fn assets_scan(
    state: State<'_, AppState>,
    options: Option<AssetScanOptions>,
) -> Result<ForgottenAssetsReport, IpcError> {
    if state.vault_locked.load(Ordering::SeqCst) {
        return Err(AegisError::VaultLocked("Vault is locked.".into()).to_ipc());
    }
    let opts = options.unwrap_or_default();
    let reveal = state.is_sensitive_unlocked();

    let query = opts
        .query
        .as_deref()
        .map(str::trim)
        .filter(|s| !s.is_empty())
        .map(str::to_string);

    let has_vectors = if query.is_some() {
        let desc = state.embedder.active_descriptor();
        let store = lock_store(&state)?;
        vec_coverage(store.conn(), &desc.id, desc.dim)
            .map(|(_, embedded)| embedded > 0)
            .unwrap_or(false)
    } else {
        false
    };

    let embedded_query = if let (true, Some(q)) = (has_vectors, query.as_deref()) {
        state.embedder.embed_query(q).ok()
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
    map_err(scan_forgotten_assets(
        store.conn(),
        &opts,
        vec_arg,
        now_ms(),
        reveal,
    ))
}

#[tauri::command(async)]
pub fn assets_enrich_metadata(
    state: State<'_, AppState>,
    file_ids: Vec<String>,
) -> Result<usize, IpcError> {
    if state.vault_locked.load(Ordering::SeqCst) {
        return Err(AegisError::VaultLocked("Vault is locked.".into()).to_ipc());
    }
    let store = lock_store(&state)?;
    map_err(enrich_asset_metadata(store.conn(), &file_ids))
}

#[tauri::command(async)]
pub fn assets_dismiss(
    state: State<'_, AppState>,
    file_id: String,
    reason: Option<String>,
) -> Result<(), IpcError> {
    if state.vault_locked.load(Ordering::SeqCst) {
        return Err(AegisError::VaultLocked("Vault is locked.".into()).to_ipc());
    }
    let store = lock_store(&state)?;
    map_err(dismiss_forgotten_asset(
        store.conn(),
        &file_id,
        reason.as_deref().unwrap_or("dismissed"),
    ))
}

#[tauri::command(async)]
pub fn assets_undismiss(
    state: State<'_, AppState>,
    file_id: String,
) -> Result<(), IpcError> {
    if state.vault_locked.load(Ordering::SeqCst) {
        return Err(AegisError::VaultLocked("Vault is locked.".into()).to_ipc());
    }
    let store = lock_store(&state)?;
    map_err(undismiss_forgotten_asset(store.conn(), &file_id))
}

#[tauri::command(async)]
pub fn assets_propose_recovery(
    app: AppHandle,
    state: State<'_, AppState>,
    items: Vec<AssetRecoverItem>,
) -> Result<String, IpcError> {
    if state.vault_locked.load(Ordering::SeqCst) {
        return Err(AegisError::VaultLocked("Vault is locked.".into()).to_ipc());
    }
    let count = items.len();
    let batch_id = {
        let mut store = lock_store(&state)?;
        map_err(propose_asset_recovery(&mut store, &items))?
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
