//! Autonomous Research Librarian (`2.3`) Tauri IPC commands (`K1`–`K5`, `R4`, `R5`, `R7`, `D1`).

use std::sync::atomic::{AtomicBool, Ordering};

use aegis_core::db::vec::vec_coverage;
use aegis_core::error::{AegisError, IpcError};
use aegis_core::librarian::{
    LibrarianOptions, ResearchBrief, ResearchCompanionReport, ResearchDocShelf, dismiss_reference,
    find_related_for_file, find_related_for_topic, list_recent_research_docs, pin_reference,
    prepare_brief_context, undismiss_reference, unpin_reference,
};
use aegis_llm::{ContextSnippet, GenerateRequest};
use tauri::State;

use crate::commands::{AppState, lock_store, map_err};

#[tauri::command(async)]
pub fn librarian_recent_docs(
    state: State<'_, AppState>,
    query: Option<String>,
    collection_id: Option<String>,
    limit: Option<usize>,
    include_sensitive: Option<bool>,
) -> Result<ResearchDocShelf, IpcError> {
    if state.vault_locked.load(Ordering::SeqCst) {
        return Err(AegisError::VaultLocked("Vault is locked.".into()).to_ipc());
    }
    let reveal = state.is_sensitive_unlocked();
    let store = lock_store(&state)?;
    map_err(list_recent_research_docs(
        store.conn(),
        query.as_deref(),
        collection_id.as_deref(),
        limit.unwrap_or(40),
        include_sensitive.unwrap_or(false),
        reveal,
    ))
}

#[tauri::command(async)]
pub fn librarian_related_for_file(
    state: State<'_, AppState>,
    file_id: String,
    options: Option<LibrarianOptions>,
) -> Result<ResearchCompanionReport, IpcError> {
    if state.vault_locked.load(Ordering::SeqCst) {
        return Err(AegisError::VaultLocked("Vault is locked.".into()).to_ipc());
    }
    let reveal = state.is_sensitive_unlocked();
    let opts = options.unwrap_or_default();
    let store = lock_store(&state)?;
    map_err(find_related_for_file(store.conn(), &file_id, &opts, reveal))
}

#[tauri::command(async)]
pub fn librarian_related_for_topic(
    state: State<'_, AppState>,
    topic: String,
    options: Option<LibrarianOptions>,
) -> Result<ResearchCompanionReport, IpcError> {
    if state.vault_locked.load(Ordering::SeqCst) {
        return Err(AegisError::VaultLocked("Vault is locked.".into()).to_ipc());
    }
    let reveal = state.is_sensitive_unlocked();
    let opts = options.unwrap_or_default();
    let trimmed = topic.trim();

    let has_vectors = if !trimmed.is_empty() {
        let desc = state.embedder.active_descriptor();
        let store = lock_store(&state)?;
        vec_coverage(store.conn(), &desc.id, desc.dim)
            .map(|(_, embedded)| embedded > 0)
            .unwrap_or(false)
    } else {
        false
    };

    // Run on-demand embedding off the SQLite mutex (`K5`, `R4`).
    let embedded_query = if has_vectors {
        state.embedder.embed_query(trimmed).ok()
    } else {
        None
    };
    let min_sim = embedded_query
        .as_ref()
        .map(|(d, _)| if d.backend == "lexical_fallback" { 0.15 } else { 0.35 })
        .unwrap_or(0.35);

    let store = lock_store(&state)?;
    let vec_arg = embedded_query
        .as_ref()
        .map(|(d, v)| (d.id.as_str(), v.as_slice(), min_sim));
    map_err(find_related_for_topic(
        store.conn(),
        trimmed,
        vec_arg,
        &opts,
        reveal,
    ))
}

#[tauri::command(async)]
pub fn librarian_synthesize_brief(
    state: State<'_, AppState>,
    source_file_id: Option<String>,
    topic: Option<String>,
    related_file_ids: Vec<String>,
    include_sensitive: Option<bool>,
) -> Result<ResearchBrief, IpcError> {
    if state.vault_locked.load(Ordering::SeqCst) {
        return Err(AegisError::VaultLocked("Vault is locked.".into()).to_ipc());
    }
    let reveal = state.is_sensitive_unlocked();
    let prep = {
        let store = lock_store(&state)?;
        map_err(prepare_brief_context(
            store.conn(),
            source_file_id.as_deref(),
            topic.as_deref(),
            &related_file_ids,
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

    Ok(ResearchBrief {
        title: prep.title,
        summary: out.text,
        citations: prep.citations,
        model_id: desc.id,
        model_name: desc.name,
        backend: desc.backend,
        duration_ms: out.duration_ms,
    })
}

#[tauri::command(async)]
pub fn librarian_pin(
    state: State<'_, AppState>,
    source_file_id: String,
    related_file_id: String,
    note: Option<String>,
) -> Result<bool, IpcError> {
    if state.vault_locked.load(Ordering::SeqCst) {
        return Err(AegisError::VaultLocked("Vault is locked.".into()).to_ipc());
    }
    let store = lock_store(&state)?;
    map_err(pin_reference(
        store.conn(),
        &source_file_id,
        &related_file_id,
        note.as_deref(),
    ))
}

#[tauri::command(async)]
pub fn librarian_unpin(
    state: State<'_, AppState>,
    source_file_id: String,
    related_file_id: String,
) -> Result<bool, IpcError> {
    if state.vault_locked.load(Ordering::SeqCst) {
        return Err(AegisError::VaultLocked("Vault is locked.".into()).to_ipc());
    }
    let store = lock_store(&state)?;
    map_err(unpin_reference(store.conn(), &source_file_id, &related_file_id))
}

#[tauri::command(async)]
pub fn librarian_dismiss(
    state: State<'_, AppState>,
    source_file_id: String,
    related_file_id: String,
) -> Result<bool, IpcError> {
    if state.vault_locked.load(Ordering::SeqCst) {
        return Err(AegisError::VaultLocked("Vault is locked.".into()).to_ipc());
    }
    let store = lock_store(&state)?;
    map_err(dismiss_reference(
        store.conn(),
        &source_file_id,
        &related_file_id,
    ))
}

#[tauri::command(async)]
pub fn librarian_undismiss(
    state: State<'_, AppState>,
    source_file_id: String,
    related_file_id: String,
) -> Result<bool, IpcError> {
    if state.vault_locked.load(Ordering::SeqCst) {
        return Err(AegisError::VaultLocked("Vault is locked.".into()).to_ipc());
    }
    let store = lock_store(&state)?;
    map_err(undismiss_reference(
        store.conn(),
        &source_file_id,
        &related_file_id,
    ))
}
