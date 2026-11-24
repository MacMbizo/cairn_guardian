//! Cross-Project Intelligence (`3.3`) Tauri IPC commands (`K1`–`K5`, `R4`, `R5`, `R7`, `D1`).

use std::sync::atomic::{AtomicBool, Ordering};

use aegis_core::cross_project::{
    CrossProjectBrief, CrossProjectOptions, CrossProjectReport, InsightStatus,
    prepare_cross_project_brief, scan_cross_project_intelligence, set_insight_status,
};
use aegis_core::error::{AegisError, IpcError};
use aegis_llm::{ContextSnippet, GenerateRequest};
use tauri::State;

use crate::commands::{AppState, lock_store, map_err};

#[tauri::command(async)]
pub fn cross_project_scan(
    state: State<'_, AppState>,
    options: Option<CrossProjectOptions>,
) -> Result<CrossProjectReport, IpcError> {
    if state.vault_locked.load(Ordering::SeqCst) {
        return Err(AegisError::VaultLocked("Vault is locked.".into()).to_ipc());
    }
    let reveal = state.is_sensitive_unlocked();
    let opts = options.unwrap_or_default();
    let store = lock_store(&state)?;
    map_err(scan_cross_project_intelligence(store.conn(), &opts, reveal))
}

#[tauri::command(async)]
pub fn cross_project_set_insight_status(
    state: State<'_, AppState>,
    insight_key: String,
    status: InsightStatus,
    note: Option<String>,
) -> Result<bool, IpcError> {
    if state.vault_locked.load(Ordering::SeqCst) {
        return Err(AegisError::VaultLocked("Vault is locked.".into()).to_ipc());
    }
    let store = lock_store(&state)?;
    map_err(set_insight_status(
        store.conn(),
        &insight_key,
        status,
        note.as_deref(),
    ))
}

#[tauri::command(async)]
pub fn cross_project_synthesize_brief(
    state: State<'_, AppState>,
    project_a_id: Option<String>,
    project_b_id: Option<String>,
    include_sensitive: Option<bool>,
) -> Result<CrossProjectBrief, IpcError> {
    if state.vault_locked.load(Ordering::SeqCst) {
        return Err(AegisError::VaultLocked("Vault is locked.".into()).to_ipc());
    }
    let reveal = state.is_sensitive_unlocked();
    let prep = {
        let store = lock_store(&state)?;
        map_err(prepare_cross_project_brief(
            store.conn(),
            project_a_id.as_deref(),
            project_b_id.as_deref(),
            include_sensitive.unwrap_or(false),
            reveal,
        ))?
    };

    if state
        .rag_running
        .compare_exchange(false, true, Ordering::SeqCst, Ordering::SeqCst)
        .is_err()
    {
        return Err(IpcError {
            code: "busy".into(),
            message: "Another local LLM task is currently running.".into(),
            retryable: true,
            remediation: Some("Wait for the active LLM task to finish and try again.".into()),
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
            label: c.filename.clone(),
            text: c.excerpt.clone(),
        })
        .collect();
    let req = GenerateRequest {
        system_prompt: prep.system_prompt,
        user_prompt: prep.user_prompt,
        max_tokens: 384,
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

    Ok(CrossProjectBrief {
        title: prep.title,
        summary: out.text,
        citations: prep.citations,
        model_id: desc.id,
        model_name: desc.name,
        backend: desc.backend,
        duration_ms: out.duration_ms,
    })
}
