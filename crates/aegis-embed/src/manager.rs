//! On-demand embedding lifecycle manager (`R4`, `K5` <= 350 MB background budget).

use crate::discovery::{
    ModelDescriptor, discover_local_models, inspect_gguf_header, inspect_safetensors_dir,
    is_safetensors_bert_dir,
};
use crate::fallback::{LEXICAL_DIM, LEXICAL_MODEL_ID, LexicalHashEmbedder};
use crate::gguf::GgufEmbedder;
use crate::safetensors::SafetensorsBertEmbedder;
use crate::{EmbedError, EmbeddingProvider};
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub enum ModelSelection {
    /// Picks the best compact local neural model (`<= 350 MB` first), falling back to lexical hash.
    #[default]
    Auto,
    /// Pins a specific local `.gguf` file or HuggingFace Safetensors BERT folder.
    Path(PathBuf),
    /// Uses only the built-in 384-d lexical hash embedder (`< 1 MB` RAM).
    FallbackOnly,
}

impl ModelSelection {
    pub fn from_setting(val: Option<&str>) -> Self {
        match val.map(str::trim).filter(|s| !s.is_empty()) {
            None | Some("auto") => Self::Auto,
            Some(s) if s == LEXICAL_MODEL_ID || s == "builtin:fallback" => Self::FallbackOnly,
            Some(path) => Self::Path(PathBuf::from(path)),
        }
    }

    pub fn to_setting(&self) -> Option<String> {
        match self {
            Self::Auto => None,
            Self::FallbackOnly => Some(LEXICAL_MODEL_ID.to_string()),
            Self::Path(p) => Some(p.to_string_lossy().to_string()),
        }
    }
}

struct ManagerState {
    models_dir: PathBuf,
    selection: ModelSelection,
    loaded: Option<(ModelDescriptor, Arc<dyn EmbeddingProvider>)>,
    last_used: Option<Instant>,
}

/// Manages local model selection, on-demand loading, and immediate/idle weight unloading (`R4`, `K5`).
pub struct OnDemandEmbedder {
    inner: Mutex<ManagerState>,
}

impl OnDemandEmbedder {
    pub fn new(models_dir: PathBuf) -> Self {
        Self {
            inner: Mutex::new(ManagerState {
                models_dir,
                selection: ModelSelection::Auto,
                loaded: None,
                last_used: None,
            }),
        }
    }

    pub fn models_dir(&self) -> PathBuf {
        self.inner
            .lock()
            .map(|g| g.models_dir.clone())
            .unwrap_or_default()
    }

    pub fn discover_models(&self) -> Vec<ModelDescriptor> {
        let dir = self.models_dir();
        discover_local_models(&dir)
    }

    pub fn selection(&self) -> ModelSelection {
        self.inner
            .lock()
            .map(|g| g.selection.clone())
            .unwrap_or_default()
    }

    /// Switches model selection and immediately unloads any previously resident weights.
    pub fn set_selection(&self, selection: ModelSelection) {
        if let Ok(mut g) = self.inner.lock() {
            g.selection = selection;
            g.loaded = None;
            g.last_used = None;
        }
    }

    pub fn is_loaded(&self) -> bool {
        self.inner
            .lock()
            .map(|g| {
                g.loaded
                    .as_ref()
                    .is_some_and(|(desc, _)| desc.backend != "lexical_fallback")
            })
            .unwrap_or(false)
    }

    /// Resolves the active model descriptor from headers/config without loading model weights.
    pub fn active_descriptor(&self) -> ModelDescriptor {
        let (models_dir, selection, loaded_desc) = {
            let g = self.inner.lock().unwrap_or_else(|p| p.into_inner());
            (
                g.models_dir.clone(),
                g.selection.clone(),
                g.loaded.as_ref().map(|(d, _)| d.clone()),
            )
        };
        if let Some(d) = loaded_desc {
            return d;
        }
        resolve_descriptor(&models_dir, &selection)
    }

    /// Unloads resident neural weights immediately (`K5`).
    pub fn unload(&self) -> bool {
        if let Ok(mut g) = self.inner.lock() {
            let was_loaded = g.loaded.take().is_some();
            g.last_used = None;
            return was_loaded;
        }
        false
    }

    /// Unloads resident neural weights if idle for longer than `ttl` (`R4`).
    pub fn unload_if_idle(&self, ttl: Duration) -> bool {
        if let Ok(mut g) = self.inner.lock()
            && g.loaded.is_some()
            && g.last_used.is_some_and(|t| t.elapsed() >= ttl)
        {
            g.loaded = None;
            g.last_used = None;
            return true;
        }
        false
    }

    /// Loads (or reuses cached) active model to embed a search query, updating the idle timer.
    pub fn embed_query(&self, query: &str) -> Result<(ModelDescriptor, Vec<f32>), EmbedError> {
        let (desc, provider) = self.ensure_loaded()?;
        let vec = provider.embed_query(query)?;
        if let Ok(mut g) = self.inner.lock() {
            g.last_used = Some(Instant::now());
        }
        Ok((desc, vec))
    }

    /// Loads the active model, embeds `texts`, and immediately drops weights from RAM (`K5`).
    pub fn embed_documents_and_unload(
        &self,
        texts: &[&str],
    ) -> Result<(ModelDescriptor, Vec<Vec<f32>>), EmbedError> {
        if texts.is_empty() {
            return Ok((self.active_descriptor(), Vec::new()));
        }
        let (desc, provider) = self.ensure_loaded()?;
        let out = provider.embed_documents(texts);
        drop(provider);
        self.unload();
        let vecs = out?;
        Ok((desc, vecs))
    }

    fn ensure_loaded(&self) -> Result<(ModelDescriptor, Arc<dyn EmbeddingProvider>), EmbedError> {
        {
            let mut g = self.inner.lock().unwrap_or_else(|p| p.into_inner());
            if let Some((desc, prov)) = g.loaded.as_ref().map(|(d, p)| (d.clone(), p.clone())) {
                g.last_used = Some(Instant::now());
                return Ok((desc, prov));
            }
        }

        let (models_dir, selection) = {
            let g = self.inner.lock().unwrap_or_else(|p| p.into_inner());
            (g.models_dir.clone(), g.selection.clone())
        };
        let desc = resolve_descriptor(&models_dir, &selection);
        let provider = load_provider_for_descriptor(&desc)?;

        let mut g = self.inner.lock().unwrap_or_else(|p| p.into_inner());
        g.loaded = Some((desc.clone(), provider.clone()));
        g.last_used = Some(Instant::now());
        Ok((desc, provider))
    }
}

fn resolve_descriptor(models_dir: &Path, selection: &ModelSelection) -> ModelDescriptor {
    match selection {
        ModelSelection::FallbackOnly => fallback_descriptor(),
        ModelSelection::Path(p) => {
            if p.is_file()
                && let Some(d) = inspect_gguf_header(p, None, "selected")
            {
                return d;
            }
            if is_safetensors_bert_dir(p)
                && let Some(d) = inspect_safetensors_dir(p, "selected")
            {
                return d;
            }
            // Also check discovered list in case `p` matches an Ollama blob path with display name.
            let discovered = discover_local_models(models_dir);
            if let Some(d) = discovered.into_iter().find(|d| Path::new(&d.path) == p) {
                return d;
            }
            fallback_descriptor()
        }
        ModelSelection::Auto => {
            let discovered = discover_local_models(models_dir);
            discovered
                .into_iter()
                .next()
                .unwrap_or_else(fallback_descriptor)
        }
    }
}

fn load_provider_for_descriptor(
    desc: &ModelDescriptor,
) -> Result<Arc<dyn EmbeddingProvider>, EmbedError> {
    match desc.backend.as_str() {
        "lexical_fallback" => Ok(Arc::new(LexicalHashEmbedder)),
        "candle_safetensors_bert" => Ok(Arc::new(SafetensorsBertEmbedder::load(Path::new(&desc.path))?)),
        b if b.starts_with("candle_gguf_") => Ok(Arc::new(GgufEmbedder::load(
            Path::new(&desc.path),
            Some(&desc.name),
        )?)),
        other => Err(EmbedError::ModelLoad(format!("unknown embedder backend: {other}"))),
    }
}

fn fallback_descriptor() -> ModelDescriptor {
    ModelDescriptor {
        id: LEXICAL_MODEL_ID.to_string(),
        name: "Built-in Lexical Hash (fallback)".to_string(),
        path: LEXICAL_MODEL_ID.to_string(),
        backend: "lexical_fallback".to_string(),
        dim: LEXICAL_DIM,
        size_bytes: 0,
        source: "builtin".to_string(),
        within_budget: true,
    }
}
