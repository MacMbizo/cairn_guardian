//! Personal Data Governance (`5.1`, reshaped `R7`–`R9`) Tauri 2 IPC commands (`K1`–`K5`, `R1`, `R2`, `R7`, `R8`, `R9`, `D1`).

use std::sync::atomic::Ordering;

use aegis_core::contracts::AegisEvent;
use aegis_core::db::crypto::sealed_key_path;
use aegis_core::error::{AegisError, IpcError};
use aegis_core::governance::posture::VaultPostureContext;
use aegis_core::governance::{
    GovernanceDashboard, GovernanceFindingStatus, GovernanceLabelBatch, GovernancePrivacyRule,
    GovernancePrivacyRuleInput, GovernanceQueryOptions, GovernanceScanOptions,
    GovernanceScanReport, apply_privacy_rule, apply_sensitivity_labels, delete_privacy_rule,
    list_governance_dashboard, list_privacy_rules, propose_governance_quarantine,
    save_privacy_rule, scan_personal_data, set_finding_status, undo_sensitivity_batch,
};
use tauri::{AppHandle, Emitter, State};

use crate::commands::vault::build_vault_status;
use crate::commands::{AEGIS_EVENT, AppState, lock_store, map_err};
use crate::engine_host::wake;

fn posture_context(state: &AppState) -> Result<VaultPostureContext, IpcError> {
    let vstatus = build_vault_status(state)?;
    let dpapi_sealed = sealed_key_path(&state.db_path).is_file();
    let cipher_label = if vstatus.cipher_version.is_empty() {
        "SQLCipher AES-256".to_string()
    } else {
        format!("SQLCipher {} ({})", vstatus.cipher_version, vstatus.cipher_provider)
    };
    Ok(VaultPostureContext {
        db_encrypted: vstatus.encrypted,
        cipher_label,
        dpapi_sealed,
        recovery_confirmed: vstatus.recovery_confirmed,
        sensitive_unlocked: vstatus.sensitive_unlocked,
        sensitive_expires_at: vstatus.sensitive_expires_at,
    })
}

#[tauri::command(async)]
pub fn governance_dashboard(
    state: State<'_, AppState>,
    options: Option<GovernanceQueryOptions>,
    live_probe_bitlocker: Option<bool>,
) -> Result<GovernanceDashboard, IpcError> {
    if state.vault_locked.load(Ordering::SeqCst) {
        return Err(AegisError::VaultLocked("Vault is locked.".into()).to_ipc());
    }
    let opts = options.unwrap_or_default();
    let reveal = state.is_sensitive_unlocked();
    let vctx = posture_context(&state)?;
    let store = lock_store(&state)?;
    map_err(list_governance_dashboard(
        store.conn(),
        &opts,
        &vctx,
        reveal,
        live_probe_bitlocker.unwrap_or(true),
    ))
}

#[tauri::command(async)]
pub fn governance_scan(
    state: State<'_, AppState>,
    options: Option<GovernanceScanOptions>,
) -> Result<GovernanceScanReport, IpcError> {
    if state.vault_locked.load(Ordering::SeqCst) {
        return Err(AegisError::VaultLocked("Vault is locked.".into()).to_ipc());
    }
    let opts = options.unwrap_or_default();
    let reveal = state.is_sensitive_unlocked();
    let store = lock_store(&state)?;
    map_err(scan_personal_data(store.conn(), &opts, reveal))
}

#[tauri::command(async)]
pub fn governance_set_finding_status(
    state: State<'_, AppState>,
    finding_id: String,
    status: GovernanceFindingStatus,
) -> Result<(), IpcError> {
    if state.vault_locked.load(Ordering::SeqCst) {
        return Err(AegisError::VaultLocked("Vault is locked.".into()).to_ipc());
    }
    let mut store = lock_store(&state)?;
    map_err(set_finding_status(&mut store, &finding_id, status))
}

#[tauri::command(async)]
pub fn governance_apply_labels(
    state: State<'_, AppState>,
    file_ids: Vec<String>,
    target_sensitivity: String,
    finding_ids: Vec<String>,
    reason: Option<String>,
) -> Result<GovernanceLabelBatch, IpcError> {
    if state.vault_locked.load(Ordering::SeqCst) {
        return Err(AegisError::VaultLocked("Vault is locked.".into()).to_ipc());
    }
    let mut store = lock_store(&state)?;
    let clean_reason = reason
        .as_deref()
        .map(str::trim)
        .filter(|s| !s.is_empty())
        .unwrap_or("Manual R7 privacy protection");
    map_err(apply_sensitivity_labels(
        &mut store,
        &file_ids,
        &target_sensitivity,
        &finding_ids,
        clean_reason,
    ))
}

#[tauri::command(async)]
pub fn governance_undo_label_batch(
    state: State<'_, AppState>,
    batch_id: String,
) -> Result<GovernanceLabelBatch, IpcError> {
    if state.vault_locked.load(Ordering::SeqCst) {
        return Err(AegisError::VaultLocked("Vault is locked.".into()).to_ipc());
    }
    let mut store = lock_store(&state)?;
    map_err(undo_sensitivity_batch(&mut store, &batch_id))
}

#[tauri::command(async)]
pub fn governance_propose_quarantine(
    app: AppHandle,
    state: State<'_, AppState>,
    file_ids: Vec<String>,
    include_sensitive: bool,
) -> Result<String, IpcError> {
    if state.vault_locked.load(Ordering::SeqCst) {
        return Err(AegisError::VaultLocked("Vault is locked.".into()).to_ipc());
    }
    let reveal = state.is_sensitive_unlocked();
    let (batch_id, count) = {
        let mut store = lock_store(&state)?;
        let batch_id = map_err(propose_governance_quarantine(
            &mut store,
            &file_ids,
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
pub fn governance_list_rules(
    state: State<'_, AppState>,
) -> Result<Vec<GovernancePrivacyRule>, IpcError> {
    if state.vault_locked.load(Ordering::SeqCst) {
        return Err(AegisError::VaultLocked("Vault is locked.".into()).to_ipc());
    }
    let store = lock_store(&state)?;
    map_err(list_privacy_rules(store.conn()))
}

#[tauri::command(async)]
pub fn governance_save_rule(
    state: State<'_, AppState>,
    input: GovernancePrivacyRuleInput,
) -> Result<GovernancePrivacyRule, IpcError> {
    if state.vault_locked.load(Ordering::SeqCst) {
        return Err(AegisError::VaultLocked("Vault is locked.".into()).to_ipc());
    }
    let store = lock_store(&state)?;
    map_err(save_privacy_rule(store.conn(), &input))
}

#[tauri::command(async)]
pub fn governance_delete_rule(
    state: State<'_, AppState>,
    rule_id: String,
) -> Result<bool, IpcError> {
    if state.vault_locked.load(Ordering::SeqCst) {
        return Err(AegisError::VaultLocked("Vault is locked.".into()).to_ipc());
    }
    let store = lock_store(&state)?;
    map_err(delete_privacy_rule(store.conn(), &rule_id))
}

#[tauri::command(async)]
pub fn governance_apply_rule(
    state: State<'_, AppState>,
    rule_id: String,
) -> Result<GovernanceLabelBatch, IpcError> {
    if state.vault_locked.load(Ordering::SeqCst) {
        return Err(AegisError::VaultLocked("Vault is locked.".into()).to_ipc());
    }
    let mut store = lock_store(&state)?;
    map_err(apply_privacy_rule(&mut store, &rule_id))
}
