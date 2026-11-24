//! Local embedding & vector coverage IPC commands plus shared sync helper (`K3`, `K5`, `R4`).

use crate::commands::{AEGIS_EVENT, AppState};
use aegis_core::contracts::{AegisEvent, DiscoveredModel, EmbedStatus, EmbedSyncReport};
use aegis_core::db::SqliteStore;
use aegis_core::db::vec::{
    ensure_vec_table, pending_chunks, prune_orphan_vectors, upsert_chunk_embeddings, vec_coverage,
};
use aegis_core::error::{AegisError, IpcError, Result};
use aegis_embed::{LEXICAL_MODEL_ID, ModelDescriptor, ModelSelection, OnDemandEmbedder};
use std::path::Path;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::Instant;
use tauri::{AppHandle, Emitter, State};

pub(crate) struct AtomicFlagGuard(pub Arc<AtomicBool>);

impl AtomicFlagGuard {
    pub fn try_acquire(flag: &Arc<AtomicBool>) -> Option<Self> {
        flag.compare_exchange(false, true, Ordering::SeqCst, Ordering::SeqCst)
            .ok()
            .map(|_| Self(flag.clone()))
    }
}

impl Drop for AtomicFlagGuard {
    fn drop(&mut self) {
        self.0.store(false, Ordering::SeqCst);
    }
}

fn map_err<T>(r: Result<T>) -> std::result::Result<T, IpcError> {
    r.map_err(|e| e.to_ipc())
}

pub(crate) fn to_contract_model(m: ModelDescriptor) -> DiscoveredModel {
    DiscoveredModel {
        id: m.id,
        name: m.name,
        path: m.path,
        backend: m.backend,
        dim: m.dim,
        size_bytes: m.size_bytes,
        source: m.source,
        within_budget: m.within_budget,
    }
}

pub(crate) fn build_embed_status(conn: &rusqlite::Connection, state: &AppState) -> Result<EmbedStatus> {
    let desc = state.embedder.active_descriptor();
    let models = state
        .embedder
        .discover_models()
        .into_iter()
        .map(to_contract_model)
        .collect();
    let (total_chunks, embedded_chunks) = vec_coverage(conn, &desc.id, desc.dim)?;
    let pending_chunks = (total_chunks - embedded_chunks).max(0);
    Ok(EmbedStatus {
        model_id: desc.id,
        model_name: desc.name,
        backend: desc.backend,
        dim: desc.dim,
        loaded: state.embedder.is_loaded(),
        selected_path: state.embedder.selection().to_setting(),
        total_chunks,
        embedded_chunks,
        pending_chunks,
        available_models: models,
        on_demand_policy:
            "Loads local model weights on demand and unloads after indexing or 60s idle (K5 / R4)"
                .into(),
    })
}

/// Ensures `vec_chunks` matches the active model, embeds up to `limit` pending chunks, writes
/// vectors inside the encrypted store, prunes orphans, and immediately unloads model weights (`K5`).
pub(crate) fn sync_pending_chunks(
    store: &SqliteStore,
    embedder: &OnDemandEmbedder,
    limit: usize,
) -> Result<EmbedSyncReport> {
    let started = Instant::now();
    let desc = embedder.active_descriptor();
    ensure_vec_table(store.conn(), &desc.id, desc.dim)?;

    let batch = pending_chunks(store.conn(), &desc.id, desc.dim, limit)?;
    let (used_desc, embedded_count) = if batch.is_empty() {
        (desc, 0)
    } else {
        let refs: Vec<&str> = batch.iter().map(|c| c.embed_text.as_str()).collect();
        let (actual_desc, vecs) = embedder
            .embed_documents_and_unload(&refs)
            .map_err(|e| AegisError::Io(std::io::Error::other(e.to_string())))?;
        let pairs: Vec<_> = batch.into_iter().zip(vecs).collect();
        let n = upsert_chunk_embeddings(store.conn(), &actual_desc.id, actual_desc.dim, &pairs)?;
        (actual_desc, n)
    };

    let pruned_orphans = prune_orphan_vectors(store.conn())?;
    let (total, embedded) = vec_coverage(store.conn(), &used_desc.id, used_desc.dim)?;
    store.shrink_memory();

    Ok(EmbedSyncReport {
        model_id: used_desc.id,
        embedded_count,
        pruned_orphans,
        pending_remaining: (total - embedded).max(0),
        took_ms: started.elapsed().as_millis() as u64,
    })
}

#[tauri::command(async)]
pub fn embed_status(state: State<'_, AppState>) -> std::result::Result<EmbedStatus, IpcError> {
    let store = crate::commands::lock_store(&state)?;
    map_err(build_embed_status(store.conn(), &state))
}

#[tauri::command(async)]
pub fn embed_sync_now(
    app: AppHandle,
    state: State<'_, AppState>,
    limit: Option<usize>,
) -> std::result::Result<EmbedSyncReport, IpcError> {
    let Some(_guard) = AtomicFlagGuard::try_acquire(&state.embed_running) else {
        return Err(IpcError {
            code: "busy".into(),
            message: "Semantic indexing is already running.".into(),
            retryable: true,
            remediation: None,
        });
    };
    let store = map_err(state.open_store())?;
    let report = map_err(sync_pending_chunks(
        &store,
        &state.embedder,
        limit.unwrap_or(200).clamp(1, 2000),
    ))?;
    let _ = app.emit(AEGIS_EVENT, AegisEvent::EngineUpdated { v: 1 });
    Ok(report)
}

#[tauri::command(async)]
pub fn embed_select_model(
    state: State<'_, AppState>,
    model_path: Option<String>,
) -> std::result::Result<EmbedStatus, IpcError> {
    let selection = match model_path.as_deref().map(str::trim).filter(|s| !s.is_empty()) {
        None | Some("auto") => ModelSelection::Auto,
        Some(s) if s == LEXICAL_MODEL_ID || s == "builtin:fallback" => ModelSelection::FallbackOnly,
        Some(raw_path) => {
            let p = Path::new(raw_path);
            let valid_gguf = p.is_file()
                && aegis_embed::discovery::inspect_gguf_header(p, None, "selected").is_some();
            let valid_safe = aegis_embed::discovery::is_safetensors_bert_dir(p)
                && aegis_embed::discovery::inspect_safetensors_dir(p, "selected").is_some();
            if !valid_gguf && !valid_safe {
                return Err(AegisError::InvalidInput(format!(
                    "Unsupported local embedding model path '{raw_path}'. Select a BERT/Nomic .gguf file or a Safetensors BERT folder containing config.json, vocab.txt, and model.safetensors."
                ))
                .to_ipc());
            }
            ModelSelection::Path(p.to_path_buf())
        }
    };

    let setting_json = match selection.to_setting() {
        Some(s) => serde_json::to_string(&s).unwrap_or_else(|_| "null".into()),
        None => "null".into(),
    };
    let store = crate::commands::lock_store(&state)?;
    map_err(store.set_setting("embed.model_path", "global", &setting_json))?;
    state.embedder.set_selection(selection);
    let desc = state.embedder.active_descriptor();
    map_err(ensure_vec_table(store.conn(), &desc.id, desc.dim))?;
    map_err(build_embed_status(store.conn(), &state))
}

#[tauri::command(async)]
pub fn embed_unload(state: State<'_, AppState>) -> std::result::Result<EmbedStatus, IpcError> {
    state.embedder.unload();
    let store = crate::commands::lock_store(&state)?;
    map_err(build_embed_status(store.conn(), &state))
}
