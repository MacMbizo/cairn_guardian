//! Encrypted vault & sensitive view Tauri commands (`D1`, `R2`, `R7`).

use super::{AppState, map_err};
use aegis_core::contracts::{RecoveryKeyBundle, VaultStatus};
use aegis_core::db::crypto::{
    confirm_recovery_backup, load_vault_meta, recover_vault_with_key, vault_meta_path,
    verify_windows_user,
};
use aegis_core::contracts::AegisEvent;
use aegis_core::error::{AegisError, IpcError};
use aegis_core::now_ms;
use std::sync::Arc;
use std::sync::atomic::Ordering;
use tauri::{AppHandle, Emitter, Manager, State};

const SENSITIVE_WINDOW_MS: i64 = 15 * 60_000;

fn key_clone(state: &AppState) -> Result<Arc<aegis_core::db::crypto::DbKey>, IpcError> {
    state
        .db_key
        .lock()
        .ok()
        .and_then(|g| g.clone())
        .ok_or_else(|| {
            AegisError::VaultLocked("Vault is locked — enter your 24-word recovery key.".into()).to_ipc()
        })
}

pub fn build_vault_status(state: &AppState) -> Result<VaultStatus, IpcError> {
    let locked = state.vault_locked.load(Ordering::SeqCst);
    let migrated = state.vault_migrated.load(Ordering::SeqCst);
    let expires_at = state.sensitive_unlocked_until.load(Ordering::SeqCst);
    let sensitive_unlocked = !locked && expires_at > now_ms();
    let sensitive_expires_at = sensitive_unlocked.then_some(expires_at);
    let lock_reason = if locked {
        state.vault_lock_reason.lock().ok().and_then(|g| g.clone())
    } else {
        None
    };

    let meta = load_vault_meta(&vault_meta_path(&state.db_path)).ok().flatten();
    let fp = state
        .db_key
        .lock()
        .ok()
        .and_then(|g| g.as_ref().map(|k| k.fingerprint()))
        .or_else(|| meta.as_ref().map(|m| m.key_fingerprint.clone()));
    let recovery_confirmed = meta.as_ref().is_some_and(|m| m.recovery_confirmed_at.is_some());
    let migrated_from_plaintext = migrated
        || meta
            .as_ref()
            .is_some_and(|m| m.migrated_from_plaintext_at.is_some());

    if locked {
        return Ok(VaultStatus {
            locked: true,
            encrypted: true,
            cipher_version: String::new(),
            cipher_provider: String::new(),
            vec_version: String::new(),
            key_fingerprint: fp,
            recovery_confirmed,
            migrated_from_plaintext,
            sensitive_unlocked: false,
            sensitive_expires_at: None,
            lock_reason,
        });
    }

    let (encrypted, cipher_version, cipher_provider, vec_version) = match state.store.lock() {
        Ok(store) => {
            let (ver, prov, vec) = store.cipher_info().unwrap_or_default();
            (store.is_encrypted(), ver, prov, vec)
        }
        Err(_) => (true, String::new(), String::new(), String::new()),
    };

    Ok(VaultStatus {
        locked: false,
        encrypted,
        cipher_version,
        cipher_provider,
        vec_version,
        key_fingerprint: fp,
        recovery_confirmed,
        migrated_from_plaintext,
        sensitive_unlocked,
        sensitive_expires_at,
        lock_reason: None,
    })
}

fn key_bundle(key: &aegis_core::db::crypto::DbKey) -> Result<RecoveryKeyBundle, IpcError> {
    let words = map_err(key.to_recovery_words_vec())?;
    let hex = key.to_recovery_hex().to_string();
    Ok(RecoveryKeyBundle {
        words,
        hex,
        fingerprint: key.fingerprint(),
    })
}

#[tauri::command(async)]
pub fn vault_status(state: State<'_, AppState>) -> Result<VaultStatus, IpcError> {
    build_vault_status(&state)
}

#[tauri::command(async)]
pub fn vault_get_initial_recovery_key(state: State<'_, AppState>) -> Result<RecoveryKeyBundle, IpcError> {
    if state.vault_locked.load(Ordering::SeqCst) {
        return Err(AegisError::VaultLocked("Vault is locked.".into()).to_ipc());
    }
    let confirmed = load_vault_meta(&vault_meta_path(&state.db_path))
        .ok()
        .flatten()
        .is_some_and(|m| m.recovery_confirmed_at.is_some());
    if confirmed {
        return Err(AegisError::Vault(
            "Recovery key backup is already confirmed; use Reveal with Windows authentication.".into(),
        )
        .to_ipc());
    }
    let key = key_clone(&state)?;
    key_bundle(&key)
}

#[tauri::command(async)]
pub fn vault_confirm_recovery_backup(
    state: State<'_, AppState>,
    checks: Vec<(usize, String)>,
) -> Result<VaultStatus, IpcError> {
    if state.vault_locked.load(Ordering::SeqCst) {
        return Err(AegisError::VaultLocked("Vault is locked.".into()).to_ipc());
    }
    let key = key_clone(&state)?;
    map_err(confirm_recovery_backup(&state.db_path, &key, &checks))?;
    build_vault_status(&state)
}

#[tauri::command(async)]
pub fn vault_reveal_recovery_key(state: State<'_, AppState>) -> Result<RecoveryKeyBundle, IpcError> {
    if state.vault_locked.load(Ordering::SeqCst) {
        return Err(AegisError::VaultLocked("Vault is locked.".into()).to_ipc());
    }
    let ok = map_err(verify_windows_user(
        "Aegis Guardian — Recovery Key",
        "Confirm your Windows sign-in to display your 24-word offline recovery phrase.",
    ))?;
    if !ok {
        return Err(IpcError {
            code: "cancelled".into(),
            message: "Windows authentication was cancelled.".into(),
            retryable: true,
            remediation: None,
        });
    }
    let key = key_clone(&state)?;
    key_bundle(&key)
}

#[tauri::command(async)]
pub fn vault_unlock_with_recovery(
    app: AppHandle,
    state: State<'_, AppState>,
    input: String,
) -> Result<VaultStatus, IpcError> {
    let (new_store, new_key, _meta) = map_err(recover_vault_with_key(&state.db_path, &input))?;
    crate::apply_saved_embed_model(&new_store, &state.embedder);
    crate::apply_saved_rag_model(&new_store, &state.llm);
    let shortcut = crate::init_unlocked_settings(&app, &new_store).map_err(|e| {
        IpcError {
            code: "database_error".into(),
            message: e.to_string(),
            retryable: false,
            remediation: None,
        }
    })?;

    {
        let mut store_guard = state.store.lock().map_err(|_| IpcError {
            code: "database_error".into(),
            message: "Failed to acquire store lock.".into(),
            retryable: true,
            remediation: None,
        })?;
        *store_guard = new_store;
    }
    if let Ok(mut kg) = state.db_key.lock() {
        *kg = Some(Arc::new(new_key));
    }
    if let Ok(mut rg) = state.vault_lock_reason.lock() {
        *rg = None;
    }
    state.vault_locked.store(false, Ordering::SeqCst);

    crate::shell::register_shortcut(&app, &shortcut);
    if let Some(rx) = state.pending_engine_rx.lock().ok().and_then(|mut g| g.take()) {
        crate::watch::spawn(app.clone());
        crate::engine_host::spawn(app.clone(), rx);
    }
    let _ = app.emit(super::AEGIS_EVENT, AegisEvent::EngineUpdated { v: 1 });
    build_vault_status(app.state::<AppState>().inner())
}

#[tauri::command(async)]
pub fn vault_unlock_sensitive(state: State<'_, AppState>) -> Result<VaultStatus, IpcError> {
    if state.vault_locked.load(Ordering::SeqCst) {
        return Err(AegisError::VaultLocked("Vault is locked.".into()).to_ipc());
    }
    let ok = map_err(verify_windows_user(
        "Aegis Guardian — Sensitive Views",
        "Confirm your Windows sign-in to reveal sensitive file previews for 15 minutes.",
    ))?;
    if !ok {
        return Err(IpcError {
            code: "cancelled".into(),
            message: "Windows authentication was cancelled.".into(),
            retryable: true,
            remediation: None,
        });
    }
    state
        .sensitive_unlocked_until
        .store(now_ms() + SENSITIVE_WINDOW_MS, Ordering::SeqCst);
    build_vault_status(&state)
}

#[tauri::command(async)]
pub fn vault_lock_sensitive(state: State<'_, AppState>) -> Result<VaultStatus, IpcError> {
    state.sensitive_unlocked_until.store(0, Ordering::SeqCst);
    build_vault_status(&state)
}
