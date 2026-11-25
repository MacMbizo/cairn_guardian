//! AI Folderless Mode (`2.1`, reshaped `R6`) Tauri IPC commands (`K1`–`K5`, `R4`, `R6`, `R7`, `D1`).

use std::sync::atomic::{AtomicBool, Ordering};

use aegis_core::contracts::AegisEvent;
use aegis_core::error::{AegisError, IpcError};
use aegis_core::folderless::engine::rule_to_input;
use aegis_core::folderless::{
    CompiledIntentDraft, FolderlessBatchSummary, FolderlessPreviewReport, FolderlessRule,
    FolderlessRuleInput, apply_llm_refinement, build_intent_llm_prompt,
    compile_intent_deterministic, delete_folderless_rule, get_folderless_rule,
    list_folderless_batches, list_folderless_rules, preview_folderless_rule,
    propose_folderless_batch, save_folderless_rule, toggle_folderless_rule,
};
use aegis_llm::GenerateRequest;
use tauri::{AppHandle, Emitter, State};

use crate::commands::{AEGIS_EVENT, AppState, lock_store, map_err};
use crate::engine_host::wake;

#[tauri::command(async)]
pub fn folderless_list_rules(state: State<'_, AppState>) -> Result<Vec<FolderlessRule>, IpcError> {
    if state.vault_locked.load(Ordering::SeqCst) {
        return Err(AegisError::VaultLocked("Vault is locked.".into()).to_ipc());
    }
    let store = lock_store(&state)?;
    map_err(list_folderless_rules(store.conn()))
}

#[tauri::command(async)]
pub fn folderless_compile_intent(
    state: State<'_, AppState>,
    intent_text: String,
    use_llm: Option<bool>,
) -> Result<CompiledIntentDraft, IpcError> {
    if state.vault_locked.load(Ordering::SeqCst) {
        return Err(AegisError::VaultLocked("Vault is locked.".into()).to_ipc());
    }
    let roots: Vec<String> = {
        let store = lock_store(&state)?;
        map_err(store.list_roots())?
            .into_iter()
            .filter(|r| r.enabled)
            .map(|r| r.path)
            .collect()
    };

    let draft = map_err(compile_intent_deterministic(&intent_text, &roots))?;
    if !use_llm.unwrap_or(false) {
        return Ok(draft);
    }

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

    let (sys, user) = build_intent_llm_prompt(&intent_text, &draft);
    let req = GenerateRequest {
        system_prompt: sys,
        user_prompt: user,
        max_tokens: 256,
        temperature: 0.1,
        top_p: 0.9,
        repetition_penalty: 1.05,
        seed: 42,
        stop_sequences: Vec::new(),
        context_snippets: Vec::new(),
    };
    let cancel = AtomicBool::new(false);
    if let Ok((_desc, out)) = state.llm.generate_streaming(&req, &cancel, |_| {}) {
        return Ok(apply_llm_refinement(draft, &out.text));
    }
    Ok(draft)
}

#[tauri::command(async)]
pub fn folderless_save_rule(
    state: State<'_, AppState>,
    input: FolderlessRuleInput,
) -> Result<FolderlessRule, IpcError> {
    if state.vault_locked.load(Ordering::SeqCst) {
        return Err(AegisError::VaultLocked("Vault is locked.".into()).to_ipc());
    }
    let store = lock_store(&state)?;
    map_err(save_folderless_rule(store.conn(), &input))
}

#[tauri::command(async)]
pub fn folderless_toggle_rule(
    state: State<'_, AppState>,
    rule_id: String,
    enabled: bool,
) -> Result<FolderlessRule, IpcError> {
    if state.vault_locked.load(Ordering::SeqCst) {
        return Err(AegisError::VaultLocked("Vault is locked.".into()).to_ipc());
    }
    let store = lock_store(&state)?;
    map_err(toggle_folderless_rule(store.conn(), &rule_id, enabled))
}

#[tauri::command(async)]
pub fn folderless_delete_rule(
    state: State<'_, AppState>,
    rule_id: String,
) -> Result<bool, IpcError> {
    if state.vault_locked.load(Ordering::SeqCst) {
        return Err(AegisError::VaultLocked("Vault is locked.".into()).to_ipc());
    }
    let store = lock_store(&state)?;
    map_err(delete_folderless_rule(store.conn(), &rule_id))
}

#[tauri::command(async)]
pub fn folderless_preview_rule(
    state: State<'_, AppState>,
    rule_id: Option<String>,
    draft: Option<FolderlessRuleInput>,
    include_sensitive: Option<bool>,
    limit: Option<usize>,
) -> Result<FolderlessPreviewReport, IpcError> {
    if state.vault_locked.load(Ordering::SeqCst) {
        return Err(AegisError::VaultLocked("Vault is locked.".into()).to_ipc());
    }
    let reveal = state.is_sensitive_unlocked();
    let store = lock_store(&state)?;
    let rule_input = if let Some(d) = draft {
        d
    } else if let Some(ref rid) = rule_id {
        let saved = map_err(get_folderless_rule(store.conn(), rid))?;
        rule_to_input(&saved)
    } else {
        return Err(AegisError::InvalidInput(
            "Provide either a saved rule_id or a draft rule to preview.".into(),
        )
        .to_ipc());
    };
    map_err(preview_folderless_rule(
        store.conn(),
        &rule_input,
        include_sensitive.unwrap_or(false),
        reveal,
        limit.unwrap_or(100),
    ))
}

#[tauri::command(async)]
pub fn folderless_propose_batch(
    app: AppHandle,
    state: State<'_, AppState>,
    rule_id: String,
    file_ids: Vec<String>,
    include_sensitive: Option<bool>,
) -> Result<String, IpcError> {
    if state.vault_locked.load(Ordering::SeqCst) {
        return Err(AegisError::VaultLocked("Vault is locked.".into()).to_ipc());
    }
    let reveal = state.is_sensitive_unlocked();
    let count = file_ids.len();
    let batch_id = {
        let mut store = lock_store(&state)?;
        map_err(propose_folderless_batch(
            &mut store,
            &rule_id,
            &file_ids,
            include_sensitive.unwrap_or(false),
            reveal,
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

#[tauri::command(async)]
pub fn folderless_recent_batches(
    state: State<'_, AppState>,
    limit: Option<usize>,
) -> Result<Vec<FolderlessBatchSummary>, IpcError> {
    if state.vault_locked.load(Ordering::SeqCst) {
        return Err(AegisError::VaultLocked("Vault is locked.".into()).to_ipc());
    }
    let store = lock_store(&state)?;
    map_err(list_folderless_batches(store.conn(), limit.unwrap_or(20)))
}
