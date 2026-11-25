//! Automatic Knowledge Extraction (`2.2`) Tauri IPC commands (`K1`, `K2`, `K4`, `K5`, `R4`, `R7`, `D1`).

use std::sync::atomic::{AtomicBool, Ordering};
use std::time::Instant;

use aegis_core::error::{AegisError, IpcError};
use aegis_core::knowledge::{
    KnowledgeCommitResult, KnowledgeDashboard, KnowledgeExtractOptions, KnowledgeExtractReport,
    KnowledgeQueryOptions, ProposalEditInput, approve_proposal, audio_observation_enabled,
    build_knowledge_llm_prompt, dismiss_proposal, extract_candidates_from_text,
    extract_knowledge_from_catalog, ingest_llm_candidates, load_file_text_for_llm,
    parse_llm_extraction_output, query_knowledge_dashboard, reopen_proposal,
    set_audio_observation_enabled, undo_approved_proposal, update_entity_status,
};
use aegis_llm::{ContextSnippet, GenerateRequest};
use tauri::State;

use crate::commands::{AppState, lock_store, map_err};

#[tauri::command(async)]
pub fn knowledge_dashboard(
    state: State<'_, AppState>,
    options: Option<KnowledgeQueryOptions>,
) -> Result<KnowledgeDashboard, IpcError> {
    if state.vault_locked.load(Ordering::SeqCst) {
        return Err(AegisError::VaultLocked("Vault is locked.".into()).to_ipc());
    }
    let reveal = state.is_sensitive_unlocked();
    let opts = options.unwrap_or_default();
    let store = lock_store(&state)?;
    map_err(query_knowledge_dashboard(store.conn(), &opts, reveal))
}

#[tauri::command(async)]
pub fn knowledge_extract_scan(
    state: State<'_, AppState>,
    options: Option<KnowledgeExtractOptions>,
) -> Result<KnowledgeExtractReport, IpcError> {
    if state.vault_locked.load(Ordering::SeqCst) {
        return Err(AegisError::VaultLocked("Vault is locked.".into()).to_ipc());
    }
    let reveal = state.is_sensitive_unlocked();
    let opts = options.unwrap_or_default();
    let store = lock_store(&state)?;
    map_err(extract_knowledge_from_catalog(store.conn(), &opts, reveal))
}

#[tauri::command(async)]
pub fn knowledge_extract_file_llm(
    state: State<'_, AppState>,
    file_id: String,
) -> Result<KnowledgeExtractReport, IpcError> {
    if state.vault_locked.load(Ordering::SeqCst) {
        return Err(AegisError::VaultLocked("Vault is locked.".into()).to_ipc());
    }
    let started = Instant::now();
    let reveal = state.is_sensitive_unlocked();

    let (filename, text, owning_col, is_transcript) = {
        let store = lock_store(&state)?;
        map_err(load_file_text_for_llm(store.conn(), &file_id, reveal))?
    };

    let source_kind = if is_transcript { "transcript" } else { "document" };
    let det_cands = extract_candidates_from_text(
        &filename,
        "",
        &text,
        &[],
        None,
        owning_col.as_deref(),
    );

    let mut llm_cands = Vec::new();
    if let Some((sys_prompt, user_prompt, snippets)) =
        build_knowledge_llm_prompt(&filename, &text, owning_col.as_deref())
    {
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
            .map(|(i, s)| ContextSnippet {
                ref_id: i + 1,
                label: filename.clone(),
                text: s,
            })
            .collect();
        let req = GenerateRequest {
            system_prompt: sys_prompt,
            user_prompt,
            max_tokens: 384,
            temperature: 0.1,
            top_p: 0.9,
            repetition_penalty: 1.1,
            seed: 42,
            stop_sequences: Vec::new(),
            context_snippets: ctx_snippets,
        };
        let cancel = AtomicBool::new(false);
        if let Ok((_desc, out)) = state.llm.generate_streaming(&req, &cancel, |_| {}) {
            llm_cands = parse_llm_extraction_output(&out.text, source_kind);
        }
    }

    let final_cands = if llm_cands.is_empty() {
        det_cands
    } else {
        llm_cands
    };

    let store = lock_store(&state)?;
    let upserted = map_err(ingest_llm_candidates(
        store.conn(),
        &file_id,
        &final_cands,
        reveal,
    ))?;
    let total_pending: i64 = store
        .conn()
        .query_row(
            "SELECT COUNT(*) FROM knowledge_proposals WHERE status = 'pending'",
            [],
            |r| r.get(0),
        )
        .unwrap_or(0);
    let audio_enabled = audio_observation_enabled(store.conn()).unwrap_or(false);

    Ok(KnowledgeExtractReport {
        files_scanned: 1,
        files_skipped_unchanged: 0,
        files_skipped_sensitive: 0,
        proposals_created: upserted,
        proposals_updated: 0,
        total_pending: total_pending.max(0) as usize,
        audio_observation_enabled: audio_enabled,
        untranscribed_audio_count: 0,
        took_ms: started.elapsed().as_millis() as u64,
    })
}

#[tauri::command(async)]
pub fn knowledge_approve_proposal(
    state: State<'_, AppState>,
    proposal_id: String,
    edit: Option<ProposalEditInput>,
) -> Result<KnowledgeCommitResult, IpcError> {
    if state.vault_locked.load(Ordering::SeqCst) {
        return Err(AegisError::VaultLocked("Vault is locked.".into()).to_ipc());
    }
    let reveal = state.is_sensitive_unlocked();
    let mut store = lock_store(&state)?;
    map_err(approve_proposal(&mut store, &proposal_id, edit, reveal))
}

#[tauri::command(async)]
pub fn knowledge_approve_all(
    state: State<'_, AppState>,
    proposal_ids: Vec<String>,
) -> Result<Vec<KnowledgeCommitResult>, IpcError> {
    if state.vault_locked.load(Ordering::SeqCst) {
        return Err(AegisError::VaultLocked("Vault is locked.".into()).to_ipc());
    }
    if proposal_ids.is_empty() {
        return Ok(Vec::new());
    }
    let reveal = state.is_sensitive_unlocked();
    let mut store = lock_store(&state)?;
    let mut results = Vec::with_capacity(proposal_ids.len());
    for pid in &proposal_ids {
        if let Ok(res) = approve_proposal(&mut store, pid, None, reveal) {
            results.push(res);
        }
    }
    Ok(results)
}

#[tauri::command(async)]
pub fn knowledge_undo_proposal(
    state: State<'_, AppState>,
    proposal_id: String,
) -> Result<(), IpcError> {
    if state.vault_locked.load(Ordering::SeqCst) {
        return Err(AegisError::VaultLocked("Vault is locked.".into()).to_ipc());
    }
    let mut store = lock_store(&state)?;
    map_err(undo_approved_proposal(&mut store, &proposal_id))
}

#[tauri::command(async)]
pub fn knowledge_dismiss_proposal(
    state: State<'_, AppState>,
    proposal_id: String,
) -> Result<(), IpcError> {
    if state.vault_locked.load(Ordering::SeqCst) {
        return Err(AegisError::VaultLocked("Vault is locked.".into()).to_ipc());
    }
    let store = lock_store(&state)?;
    map_err(dismiss_proposal(store.conn(), &proposal_id))
}

#[tauri::command(async)]
pub fn knowledge_reopen_proposal(
    state: State<'_, AppState>,
    proposal_id: String,
) -> Result<(), IpcError> {
    if state.vault_locked.load(Ordering::SeqCst) {
        return Err(AegisError::VaultLocked("Vault is locked.".into()).to_ipc());
    }
    let store = lock_store(&state)?;
    map_err(reopen_proposal(store.conn(), &proposal_id))
}

#[tauri::command(async)]
pub fn knowledge_set_entity_status(
    state: State<'_, AppState>,
    entity_id: String,
    status_tag: String,
) -> Result<(), IpcError> {
    if state.vault_locked.load(Ordering::SeqCst) {
        return Err(AegisError::VaultLocked("Vault is locked.".into()).to_ipc());
    }
    let store = lock_store(&state)?;
    map_err(update_entity_status(store.conn(), &entity_id, &status_tag))
}

#[tauri::command(async)]
pub fn knowledge_set_audio_opt_in(
    state: State<'_, AppState>,
    enabled: bool,
) -> Result<bool, IpcError> {
    if state.vault_locked.load(Ordering::SeqCst) {
        return Err(AegisError::VaultLocked("Vault is locked.".into()).to_ipc());
    }
    let store = lock_store(&state)?;
    map_err(set_audio_observation_enabled(store.conn(), enabled))
}
