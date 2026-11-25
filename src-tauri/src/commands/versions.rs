//! Semantic Version Tracking (`3.4`, PRD `FR-004.2`) Tauri IPC commands (`K1`, `K2`, `K4`, `K5`, `R7`, `D1`).

use std::sync::atomic::{AtomicBool, Ordering};
use std::time::Instant;

use aegis_core::contracts::AegisEvent;
use aegis_core::error::{AegisError, IpcError};
use aegis_core::now_ms;
use aegis_core::versions::{
    VersionDiffReport, VersionDiffSummary, VersionScanOptions, VersionScanReport,
    build_diff_llm_prompt, diff_version_pair, dismiss_version_chain, load_cached_diff_summary,
    pin_latest_version, propose_version_archive, propose_version_quarantine_dupes,
    save_cached_diff_summary, scan_version_chains, undismiss_version_chain,
};
use aegis_core::versions::diff::is_pair_sensitive;
use aegis_llm::{ContextSnippet, GenerateRequest};
use tauri::{AppHandle, Emitter, State};

use crate::commands::{AEGIS_EVENT, AppState, lock_store, map_err};
use crate::engine_host::wake;

#[tauri::command(async)]
pub fn versions_scan(
    state: State<'_, AppState>,
    options: Option<VersionScanOptions>,
) -> Result<VersionScanReport, IpcError> {
    if state.vault_locked.load(Ordering::SeqCst) {
        return Err(AegisError::VaultLocked("Vault is locked.".into()).to_ipc());
    }
    let reveal = state.is_sensitive_unlocked();
    let opts = options.unwrap_or_default();
    let store = lock_store(&state)?;
    map_err(scan_version_chains(store.conn(), &opts, now_ms(), reveal))
}

#[tauri::command(async)]
pub fn versions_diff(
    state: State<'_, AppState>,
    older_file_id: String,
    newer_file_id: String,
) -> Result<VersionDiffReport, IpcError> {
    if state.vault_locked.load(Ordering::SeqCst) {
        return Err(AegisError::VaultLocked("Vault is locked.".into()).to_ipc());
    }
    let reveal = state.is_sensitive_unlocked();
    let store = lock_store(&state)?;
    map_err(diff_version_pair(
        store.conn(),
        &older_file_id,
        &newer_file_id,
        reveal,
    ))
}

#[tauri::command(async)]
pub fn versions_summarize_diff(
    state: State<'_, AppState>,
    older_file_id: String,
    newer_file_id: String,
) -> Result<VersionDiffSummary, IpcError> {
    if state.vault_locked.load(Ordering::SeqCst) {
        return Err(AegisError::VaultLocked("Vault is locked.".into()).to_ipc());
    }
    let reveal = state.is_sensitive_unlocked();
    let active_desc = state.llm.status().active;

    let (diff, either_sensitive, cached_opt) = {
        let store = lock_store(&state)?;
        let diff = map_err(diff_version_pair(
            store.conn(),
            &older_file_id,
            &newer_file_id,
            reveal,
        ))?;
        if diff.redacted_sensitive {
            return Err(AegisError::PermissionDenied(
                "Unlock sensitive previews with Windows authentication before summarizing changes for sensitive files."
                    .into(),
            )
            .to_ipc());
        }
        let either_sensitive =
            map_err(is_pair_sensitive(store.conn(), &older_file_id, &newer_file_id))?;
        let cached = if !either_sensitive {
            map_err(load_cached_diff_summary(
                store.conn(),
                &older_file_id,
                &newer_file_id,
                &active_desc.id,
                &active_desc.name,
                &active_desc.backend,
            ))?
        } else {
            None
        };
        (diff, either_sensitive, cached)
    };

    if let Some(cached) = cached_opt {
        return Ok(cached);
    }

    let Some((system_prompt, user_prompt, snippets)) = build_diff_llm_prompt(&diff) else {
        return Ok(VersionDiffSummary {
            older_file_id,
            newer_file_id,
            summary: diff.deterministic_summary,
            badges: diff.conceptual_badges,
            model_id: "deterministic".into(),
            model_name: "Deterministic diff analysis".into(),
            backend: "deterministic".into(),
            cached: false,
            duration_ms: 0,
        });
    };

    if state
        .rag_running
        .compare_exchange(false, true, Ordering::SeqCst, Ordering::SeqCst)
        .is_err()
    {
        return Err(IpcError {
            code: "busy".into(),
            message: "The local model is busy with another request.".into(),
            retryable: true,
            remediation: Some("Wait for it to finish, then try again.".into()),
        });
    }
    struct RunningReset<'a>(&'a AtomicBool);
    impl Drop for RunningReset<'_> {
        fn drop(&mut self) {
            self.0.store(false, Ordering::SeqCst);
        }
    }
    let _guard = RunningReset(&state.rag_running);

    let ctx_snippets: Vec<ContextSnippet> = snippets
        .into_iter()
        .enumerate()
        .map(|(i, text)| ContextSnippet {
            ref_id: i + 1,
            label: diff.newer_filename.clone(),
            text,
        })
        .collect();

    let req = GenerateRequest {
        system_prompt,
        user_prompt,
        max_tokens: 220,
        temperature: 0.2,
        top_p: 0.9,
        repetition_penalty: 1.1,
        seed: 42,
        stop_sequences: Vec::new(),
        context_snippets: ctx_snippets,
    };

    let started = Instant::now();
    let cancel = AtomicBool::new(false);
    let summary = match state.llm.generate_streaming(&req, &cancel, |_| {}) {
        Ok((desc, out)) => {
            let trimmed = out.text.trim().to_string();
            if trimmed.is_empty() {
                VersionDiffSummary {
                    older_file_id,
                    newer_file_id,
                    summary: diff.deterministic_summary,
                    badges: diff.conceptual_badges,
                    model_id: "deterministic".into(),
                    model_name: "Deterministic diff analysis".into(),
                    backend: "deterministic".into(),
                    cached: false,
                    duration_ms: started.elapsed().as_millis() as u64,
                }
            } else {
                VersionDiffSummary {
                    older_file_id,
                    newer_file_id,
                    summary: trimmed,
                    badges: diff.conceptual_badges,
                    model_id: desc.id,
                    model_name: desc.name,
                    backend: desc.backend,
                    cached: false,
                    duration_ms: out.duration_ms,
                }
            }
        }
        Err(_) => VersionDiffSummary {
            older_file_id,
            newer_file_id,
            summary: diff.deterministic_summary,
            badges: diff.conceptual_badges,
            model_id: "deterministic".into(),
            model_name: "Deterministic diff analysis".into(),
            backend: "deterministic".into(),
            cached: false,
            duration_ms: started.elapsed().as_millis() as u64,
        },
    };

    if !either_sensitive && summary.model_id != "deterministic" {
        let store = lock_store(&state)?;
        let _ = save_cached_diff_summary(store.conn(), &summary);
    }

    Ok(summary)
}

#[tauri::command(async)]
pub fn versions_pin_latest(
    state: State<'_, AppState>,
    chain_key: String,
    file_id: String,
) -> Result<(), IpcError> {
    if state.vault_locked.load(Ordering::SeqCst) {
        return Err(AegisError::VaultLocked("Vault is locked.".into()).to_ipc());
    }
    let store = lock_store(&state)?;
    map_err(pin_latest_version(store.conn(), &chain_key, &file_id))
}

#[tauri::command(async)]
pub fn versions_dismiss(
    state: State<'_, AppState>,
    chain_key: String,
) -> Result<(), IpcError> {
    if state.vault_locked.load(Ordering::SeqCst) {
        return Err(AegisError::VaultLocked("Vault is locked.".into()).to_ipc());
    }
    let store = lock_store(&state)?;
    map_err(dismiss_version_chain(store.conn(), &chain_key))
}

#[tauri::command(async)]
pub fn versions_undismiss(
    state: State<'_, AppState>,
    chain_key: String,
) -> Result<(), IpcError> {
    if state.vault_locked.load(Ordering::SeqCst) {
        return Err(AegisError::VaultLocked("Vault is locked.".into()).to_ipc());
    }
    let store = lock_store(&state)?;
    map_err(undismiss_version_chain(store.conn(), &chain_key))
}

#[tauri::command(async)]
pub fn versions_propose_archive(
    app: AppHandle,
    state: State<'_, AppState>,
    file_ids: Vec<String>,
    dest_dir: String,
) -> Result<String, IpcError> {
    if state.vault_locked.load(Ordering::SeqCst) {
        return Err(AegisError::VaultLocked("Vault is locked.".into()).to_ipc());
    }
    let count = file_ids.len();
    let batch_id = {
        let mut store = lock_store(&state)?;
        map_err(propose_version_archive(&mut store, &file_ids, &dest_dir))?
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
pub fn versions_propose_quarantine_dupes(
    app: AppHandle,
    state: State<'_, AppState>,
    keeper_file_id: String,
    duplicate_file_ids: Vec<String>,
) -> Result<String, IpcError> {
    if state.vault_locked.load(Ordering::SeqCst) {
        return Err(AegisError::VaultLocked("Vault is locked.".into()).to_ipc());
    }
    let count = duplicate_file_ids.len();
    let batch_id = {
        let mut store = lock_store(&state)?;
        map_err(propose_version_quarantine_dupes(
            &mut store,
            &keeper_file_id,
            &duplicate_file_ids,
        ))?
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
