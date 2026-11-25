//! Capability #15: Digital Estate Management (`5.3`, reshaped `R10`, `D1`, `D3`, `R2`, `R7`, `R8`) Tauri 2 IPC commands.

use std::path::Path;
use std::sync::atomic::Ordering;

use aegis_core::error::{AegisError, IpcError};
use aegis_core::estate::{
    EstateContinuityExport, EstateDashboard, EstateHeartbeatStatus, EstatePacket,
    EstatePacketInput, EstateShardSetBundle, EstateShardVerifyResult, EstateTrustee,
    EstateTrusteeInput, build_estate_dashboard, delete_estate_packet, delete_estate_trustee,
    export_estate_continuity_kit, generate_estate_shard_set, mark_estate_shard_delivered,
    record_estate_checkin, revoke_estate_shard_set, save_estate_heartbeat_config,
    save_estate_packet, save_estate_trustee, verify_estate_shard_quorum,
};
use aegis_core::now_ms;
use tauri::State;

use crate::commands::governance::posture_context;
use crate::commands::{AppState, lock_store, map_err};

#[tauri::command(async)]
pub fn estate_dashboard(state: State<'_, AppState>) -> Result<EstateDashboard, IpcError> {
    if state.vault_locked.load(Ordering::SeqCst) {
        return Err(AegisError::VaultLocked("Vault is locked.".into()).to_ipc());
    }
    let reveal = state.is_sensitive_unlocked();
    let vctx = posture_context(&state)?;
    let store = lock_store(&state)?;
    map_err(build_estate_dashboard(store.conn(), &vctx, reveal, now_ms()))
}

#[tauri::command(async)]
pub fn estate_save_trustee(
    state: State<'_, AppState>,
    input: EstateTrusteeInput,
) -> Result<EstateTrustee, IpcError> {
    if state.vault_locked.load(Ordering::SeqCst) {
        return Err(AegisError::VaultLocked("Vault is locked.".into()).to_ipc());
    }
    let mut store = lock_store(&state)?;
    map_err(save_estate_trustee(&mut store, &input))
}

#[tauri::command(async)]
pub fn estate_delete_trustee(
    state: State<'_, AppState>,
    trustee_id: String,
) -> Result<bool, IpcError> {
    if state.vault_locked.load(Ordering::SeqCst) {
        return Err(AegisError::VaultLocked("Vault is locked.".into()).to_ipc());
    }
    let mut store = lock_store(&state)?;
    map_err(delete_estate_trustee(&mut store, &trustee_id))
}

#[tauri::command(async)]
pub fn estate_save_packet(
    state: State<'_, AppState>,
    input: EstatePacketInput,
) -> Result<EstatePacket, IpcError> {
    if state.vault_locked.load(Ordering::SeqCst) {
        return Err(AegisError::VaultLocked("Vault is locked.".into()).to_ipc());
    }
    let reveal = state.is_sensitive_unlocked();
    let mut store = lock_store(&state)?;
    map_err(save_estate_packet(&mut store, &input, reveal))
}

#[tauri::command(async)]
pub fn estate_delete_packet(
    state: State<'_, AppState>,
    packet_id: String,
) -> Result<bool, IpcError> {
    if state.vault_locked.load(Ordering::SeqCst) {
        return Err(AegisError::VaultLocked("Vault is locked.".into()).to_ipc());
    }
    let reveal = state.is_sensitive_unlocked();
    let mut store = lock_store(&state)?;
    map_err(delete_estate_packet(&mut store, &packet_id, reveal))
}

#[tauri::command(async)]
pub fn estate_generate_shards(
    state: State<'_, AppState>,
    threshold_k: u8,
    total_n: u8,
    trustee_ids: Vec<Option<String>>,
) -> Result<EstateShardSetBundle, IpcError> {
    if state.vault_locked.load(Ordering::SeqCst) {
        return Err(AegisError::VaultLocked("Vault is locked.".into()).to_ipc());
    }
    let reveal = state.is_sensitive_unlocked();
    let db_key = state
        .db_key
        .lock()
        .ok()
        .and_then(|g| g.clone())
        .ok_or_else(|| {
            AegisError::VaultLocked(
                "Vault key is not loaded in memory; unlock D1 vault first.".into(),
            )
            .to_ipc()
        })?;
    let mut store = lock_store(&state)?;
    map_err(generate_estate_shard_set(
        &mut store,
        &db_key,
        threshold_k,
        total_n,
        &trustee_ids,
        reveal,
    ))
}

#[tauri::command(async)]
pub fn estate_mark_shard_delivered(
    state: State<'_, AppState>,
    shard_id: String,
) -> Result<bool, IpcError> {
    if state.vault_locked.load(Ordering::SeqCst) {
        return Err(AegisError::VaultLocked("Vault is locked.".into()).to_ipc());
    }
    let mut store = lock_store(&state)?;
    map_err(mark_estate_shard_delivered(&mut store, &shard_id))
}

#[tauri::command(async)]
pub fn estate_revoke_shard_set(
    state: State<'_, AppState>,
    shard_set_id: String,
) -> Result<bool, IpcError> {
    if state.vault_locked.load(Ordering::SeqCst) {
        return Err(AegisError::VaultLocked("Vault is locked.".into()).to_ipc());
    }
    let mut store = lock_store(&state)?;
    map_err(revoke_estate_shard_set(&mut store, &shard_set_id))
}

#[tauri::command(async)]
pub fn estate_verify_quorum(
    state: State<'_, AppState>,
    shard_tokens: Vec<String>,
) -> Result<EstateShardVerifyResult, IpcError> {
    if state.vault_locked.load(Ordering::SeqCst) {
        return Err(AegisError::VaultLocked("Vault is locked.".into()).to_ipc());
    }
    let reveal = state.is_sensitive_unlocked();
    let active_fp = state
        .db_key
        .lock()
        .ok()
        .and_then(|g| g.as_ref().map(|k| k.fingerprint()));
    let mut store = lock_store(&state)?;
    map_err(verify_estate_shard_quorum(
        &mut store,
        &shard_tokens,
        active_fp.as_deref(),
        reveal,
    ))
}

#[tauri::command(async)]
pub fn estate_save_heartbeat(
    state: State<'_, AppState>,
    enabled: bool,
    interval_days: i64,
    grace_days: i64,
) -> Result<EstateHeartbeatStatus, IpcError> {
    if state.vault_locked.load(Ordering::SeqCst) {
        return Err(AegisError::VaultLocked("Vault is locked.".into()).to_ipc());
    }
    let mut store = lock_store(&state)?;
    map_err(save_estate_heartbeat_config(
        &mut store,
        enabled,
        interval_days,
        grace_days,
    ))
}

#[tauri::command(async)]
pub fn estate_checkin(
    state: State<'_, AppState>,
    method: Option<String>,
    note: Option<String>,
) -> Result<EstateHeartbeatStatus, IpcError> {
    if state.vault_locked.load(Ordering::SeqCst) {
        return Err(AegisError::VaultLocked("Vault is locked.".into()).to_ipc());
    }
    let mut store = lock_store(&state)?;
    map_err(record_estate_checkin(
        &mut store,
        method.as_deref().unwrap_or("manual"),
        note.as_deref(),
    ))
}

#[tauri::command(async)]
pub fn estate_export_continuity_kit(
    state: State<'_, AppState>,
    output_dir: String,
) -> Result<EstateContinuityExport, IpcError> {
    if state.vault_locked.load(Ordering::SeqCst) {
        return Err(AegisError::VaultLocked("Vault is locked.".into()).to_ipc());
    }
    let reveal = state.is_sensitive_unlocked();
    let store = lock_store(&state)?;
    map_err(export_estate_continuity_kit(
        store.conn(),
        Path::new(output_dir.trim()),
        reveal,
    ))
}
