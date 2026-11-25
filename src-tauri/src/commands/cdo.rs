//! Capability #14: AI Chief Digital Officer (CDO) Agent (`5.2`) Tauri 2 IPC commands.

use std::sync::atomic::{AtomicBool, Ordering};

use aegis_core::cdo::{
    CdoDashboard, CdoExecutiveBriefing, CdoRatificationRecord, CdoRecommendation,
    CdoRecommendationStatus, CdoScanOptions, build_cdo_dashboard, list_cdo_briefings,
    list_cdo_ratifications, prepare_cdo_briefing_prompt, ratify_cdo_recommendation,
    save_cdo_briefing_snapshot, set_cdo_recommendation_status, undo_cdo_ratification,
};
use aegis_core::error::{AegisError, IpcError};
use aegis_llm::{ContextSnippet, GenerateRequest};
use tauri::State;

use crate::commands::governance::posture_context;
use crate::commands::{AppState, lock_store, map_err};

#[tauri::command(async)]
pub fn cdo_dashboard(
    state: State<'_, AppState>,
    options: Option<CdoScanOptions>,
    live_probe_bitlocker: Option<bool>,
) -> Result<CdoDashboard, IpcError> {
    if state.vault_locked.load(Ordering::SeqCst) {
        return Err(AegisError::VaultLocked("Vault is locked.".into()).to_ipc());
    }
    let opts = options.unwrap_or_default();
    let reveal = state.is_sensitive_unlocked();
    let vctx = posture_context(&state)?;
    let store = lock_store(&state)?;
    map_err(build_cdo_dashboard(
        store.conn(),
        &opts,
        &vctx,
        reveal,
        live_probe_bitlocker.unwrap_or(false),
    ))
}

#[tauri::command(async)]
pub fn cdo_set_recommendation_status(
    state: State<'_, AppState>,
    rec_key: String,
    status: CdoRecommendationStatus,
    note: Option<String>,
) -> Result<(), IpcError> {
    if state.vault_locked.load(Ordering::SeqCst) {
        return Err(AegisError::VaultLocked("Vault is locked.".into()).to_ipc());
    }
    let mut store = lock_store(&state)?;
    map_err(set_cdo_recommendation_status(
        &mut store,
        &rec_key,
        status,
        note.as_deref(),
    ))
}

#[tauri::command(async)]
pub fn cdo_ratify_recommendation(
    state: State<'_, AppState>,
    recommendation: CdoRecommendation,
) -> Result<CdoRatificationRecord, IpcError> {
    if state.vault_locked.load(Ordering::SeqCst) {
        return Err(AegisError::VaultLocked("Vault is locked.".into()).to_ipc());
    }
    let mut store = lock_store(&state)?;
    map_err(ratify_cdo_recommendation(&mut store, &recommendation))
}

#[tauri::command(async)]
pub fn cdo_undo_ratification(
    state: State<'_, AppState>,
    ratification_id: String,
) -> Result<CdoRatificationRecord, IpcError> {
    if state.vault_locked.load(Ordering::SeqCst) {
        return Err(AegisError::VaultLocked("Vault is locked.".into()).to_ipc());
    }
    let mut store = lock_store(&state)?;
    map_err(undo_cdo_ratification(&mut store, &ratification_id))
}

#[tauri::command(async)]
pub fn cdo_synthesize_briefing(
    state: State<'_, AppState>,
    horizon_days: Option<i64>,
    include_sensitive: Option<bool>,
    live_probe_bitlocker: Option<bool>,
) -> Result<CdoExecutiveBriefing, IpcError> {
    if state.vault_locked.load(Ordering::SeqCst) {
        return Err(AegisError::VaultLocked("Vault is locked.".into()).to_ipc());
    }
    let reveal = state.is_sensitive_unlocked();
    let vctx = posture_context(&state)?;
    let prep = {
        let store = lock_store(&state)?;
        map_err(prepare_cdo_briefing_prompt(
            store.conn(),
            horizon_days.unwrap_or(30),
            include_sensitive.unwrap_or(false),
            &vctx,
            reveal,
            live_probe_bitlocker.unwrap_or(false),
        ))?
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

    let ctx_snippets: Vec<ContextSnippet> = prep
        .citations
        .iter()
        .map(|c| ContextSnippet {
            ref_id: c.ref_id,
            label: c.title.clone(),
            text: c.detail.clone(),
        })
        .collect();
    let req = GenerateRequest {
        system_prompt: prep.system_prompt,
        user_prompt: prep.user_prompt,
        max_tokens: 448,
        temperature: 0.2,
        top_p: 0.9,
        repetition_penalty: 1.1,
        seed: 42,
        stop_sequences: Vec::new(),
        context_snippets: ctx_snippets,
    };
    let cancel = AtomicBool::new(false);
    let (desc, out) = state
        .llm
        .generate_streaming(&req, &cancel, |_| {})
        .map_err(|e| IpcError {
            code: "engine_error".into(),
            message: e.to_string(),
            retryable: true,
            remediation: Some(
                "Check the selected local GGUF model or switch to Built-in Extractive in Settings."
                    .into(),
            ),
        })?;

    let briefing = CdoExecutiveBriefing {
        id: aegis_core::new_id(),
        horizon_days: prep.horizon_days,
        executive_score: prep.executive_score,
        headline: prep.headline,
        summary_markdown: out.text,
        citations: prep.citations,
        model_id: desc.id,
        model_name: desc.name,
        backend: desc.backend,
        created_at: aegis_core::now_ms(),
    };

    let store = lock_store(&state)?;
    map_err(save_cdo_briefing_snapshot(store.conn(), &briefing))?;
    Ok(briefing)
}

#[tauri::command(async)]
pub fn cdo_list_briefings(
    state: State<'_, AppState>,
    limit: Option<usize>,
) -> Result<Vec<CdoExecutiveBriefing>, IpcError> {
    if state.vault_locked.load(Ordering::SeqCst) {
        return Err(AegisError::VaultLocked("Vault is locked.".into()).to_ipc());
    }
    let store = lock_store(&state)?;
    map_err(list_cdo_briefings(store.conn(), limit.unwrap_or(10)))
}

#[tauri::command(async)]
pub fn cdo_list_ratifications(
    state: State<'_, AppState>,
    limit: Option<usize>,
) -> Result<Vec<CdoRatificationRecord>, IpcError> {
    if state.vault_locked.load(Ordering::SeqCst) {
        return Err(AegisError::VaultLocked("Vault is locked.".into()).to_ipc());
    }
    let store = lock_store(&state)?;
    map_err(list_cdo_ratifications(store.conn(), limit.unwrap_or(15)))
}
