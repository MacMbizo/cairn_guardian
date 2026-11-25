//! Offline Local RAG (`Ask Files`) IPC commands and lifecycle management (`K1`, `K2`, `K3`, `K5`, `R4`, `R7`, `R9`).

use std::path::Path;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::Instant;

use aegis_core::contracts::{AegisEvent, SearchMode};
use aegis_core::db::vec::vec_coverage;
use aegis_core::error::{AegisError, IpcError};
use aegis_core::rag::{
    LlmModelInfo, LlmStatus, RagAnswer, RagAskOptions, retrieve_rag_context,
};
use aegis_llm::{ContextSnippet, GenerateRequest, LlmModelDescriptor, LlmSelection};
use tauri::{AppHandle, Emitter, State};

use super::{AEGIS_EVENT, AppState, lock_store, map_err};

fn to_model_info(d: LlmModelDescriptor) -> LlmModelInfo {
    LlmModelInfo {
        id: d.id,
        name: d.name,
        path: d.path,
        backend: d.backend,
        architecture: d.architecture,
        context_length: d.context_length,
        size_bytes: d.size_bytes,
        source: d.source,
        recommended: d.recommended,
    }
}

fn build_llm_status(state: &AppState) -> LlmStatus {
    let rt = state.llm.status();
    LlmStatus {
        model_id: rt.active.id,
        model_name: rt.active.name,
        backend: rt.active.backend,
        architecture: rt.active.architecture,
        context_length: rt.active.context_length,
        loaded: rt.loaded,
        selected_path: rt.selected_path,
        available_models: rt.discovered.into_iter().map(to_model_info).collect(),
        on_demand_policy:
            "Loads on demand (CPU); <= 2.5 GB unloads after 60s idle or window close, > 2.5 GB unloads immediately."
                .to_string(),
    }
}

#[tauri::command(async)]
pub fn rag_status(state: State<'_, AppState>) -> Result<LlmStatus, IpcError> {
    Ok(build_llm_status(&state))
}

#[tauri::command(async)]
pub fn rag_select_model(
    state: State<'_, AppState>,
    model_path: Option<String>,
) -> Result<LlmStatus, IpcError> {
    let cleaned = model_path
        .as_deref()
        .map(str::trim)
        .filter(|s| !s.is_empty())
        .map(str::to_string);

    let selection = LlmSelection::from_setting(cleaned.as_deref());
    state.llm.set_selection(selection).map_err(|e| IpcError {
        code: "invalid_input".into(),
        message: e.to_string(),
        retryable: false,
        remediation: None,
    })?;

    if !state.vault_locked.load(Ordering::SeqCst) {
        let store = lock_store(&state)?;
        let json_val = serde_json::to_string(&cleaned).unwrap_or_else(|_| "null".into());
        map_err(store.set_setting("rag.model_path", "global", &json_val))?;
    }

    Ok(build_llm_status(&state))
}

#[tauri::command(async)]
pub fn rag_unload(state: State<'_, AppState>) -> Result<LlmStatus, IpcError> {
    state.llm.unload();
    Ok(build_llm_status(&state))
}

#[tauri::command(async)]
pub fn rag_cancel(state: State<'_, AppState>) -> Result<(), IpcError> {
    state.rag_cancel.store(true, Ordering::SeqCst);
    Ok(())
}

#[tauri::command(async)]
pub fn rag_ask(
    app: AppHandle,
    state: State<'_, AppState>,
    question: String,
    options: Option<RagAskOptions>,
) -> Result<RagAnswer, IpcError> {
    if state.vault_locked.load(Ordering::SeqCst) {
        return Err(AegisError::VaultLocked("Vault is locked.".into()).to_ipc());
    }

    if state
        .rag_running
        .compare_exchange(false, true, Ordering::SeqCst, Ordering::SeqCst)
        .is_err()
    {
        return Err(IpcError {
            code: "busy".into(),
            message: "Another Ask Files query is currently running.".into(),
            retryable: true,
            remediation: Some("Wait for the active answer to finish or click Stop.".into()),
        });
    }

    struct RunningReset<'a>(&'a AtomicBool);
    impl Drop for RunningReset<'_> {
        fn drop(&mut self) {
            self.0.store(false, Ordering::SeqCst);
        }
    }
    let _running_guard = RunningReset(&state.rag_running);
    state.rag_cancel.store(false, Ordering::SeqCst);

    let opts = options.unwrap_or_default();
    let request_id = opts
        .request_id
        .as_deref()
        .map(str::trim)
        .filter(|s| !s.is_empty())
        .map(str::to_string)
        .unwrap_or_else(aegis_core::new_id);

    let reveal_sensitive = state.is_sensitive_unlocked();
    if opts.include_sensitive.unwrap_or(false) && !reveal_sensitive {
        return Err(AegisError::PermissionDenied(
            "Unlock sensitive previews with Windows authentication before including sensitive files in Ask Files."
                .into(),
        )
        .to_ipc());
    }

    let mode = opts
        .filters
        .as_ref()
        .and_then(|f| f.mode)
        .unwrap_or(SearchMode::Hybrid);

    let has_vectors = if mode != SearchMode::Keyword && !question.trim().is_empty() {
        let desc = state.embedder.active_descriptor();
        let store = lock_store(&state)?;
        vec_coverage(store.conn(), &desc.id, desc.dim)
            .map(|(_, embedded)| embedded > 0)
            .unwrap_or(false)
    } else {
        false
    };

    let embedded_query = if has_vectors {
        state.embedder.embed_query(&question).ok()
    } else {
        None
    };
    let min_sim = embedded_query
        .as_ref()
        .map(|(d, _)| if d.backend == "lexical_fallback" { 0.15 } else { 0.45 })
        .unwrap_or(0.45);

    let ctx = {
        let store = lock_store(&state)?;
        let vec_arg = embedded_query
            .as_ref()
            .map(|(d, v)| (d.id.as_str(), v.as_slice(), min_sim));
        map_err(retrieve_rag_context(
            store.conn(),
            &question,
            vec_arg,
            &opts,
            reveal_sensitive,
        ))?
    };

    let started = Instant::now();
    if ctx.citations.is_empty() {
        let active = state.llm.status().active;
        let msg = if ctx.excluded_sensitive_count > 0 {
            format!(
                "No non-sensitive indexed file snippets matched your question ({} sensitive file(s) left out because they are marked sensitive).",
                ctx.excluded_sensitive_count
            )
        } else {
            "I could not find any matching indexed files for that question.".to_string()
        };
        let _ = app.emit(
            AEGIS_EVENT,
            AegisEvent::RagToken {
                v: 1,
                request_id: request_id.clone(),
                token: msg.clone(),
                done: false,
            },
        );
        let _ = app.emit(
            AEGIS_EVENT,
            AegisEvent::RagToken {
                v: 1,
                request_id: request_id.clone(),
                token: String::new(),
                done: true,
            },
        );
        return Ok(RagAnswer {
            request_id,
            question,
            answer: msg,
            citations: ctx.citations,
            excluded_sensitive_count: ctx.excluded_sensitive_count,
            model_id: active.id,
            model_name: active.name,
            backend: active.backend,
            prompt_tokens: 0,
            tokens_generated: 0,
            duration_ms: started.elapsed().as_millis() as u64,
            cancelled: false,
        });
    }

    let snippets: Vec<ContextSnippet> = ctx
        .citations
        .iter()
        .map(|c| ContextSnippet {
            ref_id: c.ref_id,
            label: c.filename.clone(),
            text: c.snippet.clone(),
        })
        .collect();

    let req = GenerateRequest {
        system_prompt: ctx.system_prompt,
        user_prompt: ctx.user_prompt,
        max_tokens: opts.max_tokens.unwrap_or(320).clamp(32, 1024),
        temperature: 0.2,
        top_p: 0.9,
        repetition_penalty: 1.1,
        seed: 42,
        stop_sequences: Vec::new(),
        context_snippets: snippets,
    };

    let active_before = state.llm.status().active;
    let stream_res = state.llm.generate_streaming(&req, &state.rag_cancel, |tok| {
        let _ = app.emit(
            AEGIS_EVENT,
            AegisEvent::RagToken {
                v: 1,
                request_id: request_id.clone(),
                token: tok.to_string(),
                done: false,
            },
        );
    });

    let _ = app.emit(
        AEGIS_EVENT,
        AegisEvent::RagToken {
            v: 1,
            request_id: request_id.clone(),
            token: String::new(),
            done: true,
        },
    );

    match stream_res {
        Ok((desc, out)) => Ok(RagAnswer {
            request_id,
            question,
            answer: out.text,
            citations: ctx.citations,
            excluded_sensitive_count: ctx.excluded_sensitive_count,
            model_id: desc.id,
            model_name: desc.name,
            backend: desc.backend,
            prompt_tokens: out.prompt_tokens,
            tokens_generated: out.tokens_generated,
            duration_ms: out.duration_ms,
            cancelled: out.cancelled,
        }),
        Err(aegis_llm::LlmError::Cancelled) => Ok(RagAnswer {
            request_id,
            question,
            answer: String::new(),
            citations: ctx.citations,
            excluded_sensitive_count: ctx.excluded_sensitive_count,
            model_id: active_before.id,
            model_name: active_before.name,
            backend: active_before.backend,
            prompt_tokens: 0,
            tokens_generated: 0,
            duration_ms: started.elapsed().as_millis() as u64,
            cancelled: true,
        }),
        Err(err) => {
            // If a selected GGUF failed unexpectedly while loading/inferring, surface clean IPC error.
            let _ = Path::new(&active_before.path);
            Err(IpcError {
                code: "engine_error".into(),
                message: err.to_string(),
                retryable: true,
                remediation: Some("Check the selected GGUF model or switch to Built-in Extractive in Settings.".into()),
            })
        }
    }
}
