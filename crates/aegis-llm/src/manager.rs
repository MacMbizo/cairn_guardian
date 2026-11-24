//! On-demand CPU LLM manager with K5 idle/size-aware unloading and cooperative cancellation (`K5`, `R4`).

use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use crate::discovery::{
    BUILTIN_EXTRACTIVE_ID, K5_LLM_KEEP_LOADED_MAX_BYTES, LlmModelDescriptor,
    builtin_extractive_descriptor, discover_local_llms_in_dirs, inspect_llm_gguf_header,
};
use crate::extractive::ExtractiveGenerator;
use crate::gguf::GgufLlm;
use crate::{GenerateOutput, GenerateRequest, LlmError, TextGenerator};

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum LlmSelection {
    Auto,
    BuiltinExtractive,
    Path(PathBuf),
}

impl LlmSelection {
    pub fn from_setting(saved: Option<&str>) -> Self {
        match saved.map(str::trim) {
            None | Some("") => Self::Auto,
            Some(BUILTIN_EXTRACTIVE_ID) => Self::BuiltinExtractive,
            Some(p) => Self::Path(PathBuf::from(p)),
        }
    }
}

#[derive(Debug, Clone)]
pub struct LlmRuntimeStatus {
    pub active: LlmModelDescriptor,
    pub discovered: Vec<LlmModelDescriptor>,
    pub loaded: bool,
    pub loaded_model_id: Option<String>,
    pub selection_mode: String,
    pub selected_path: Option<String>,
}

struct InnerState {
    models_dirs: Vec<PathBuf>,
    selection: LlmSelection,
    loaded: Option<(LlmModelDescriptor, Arc<dyn TextGenerator>)>,
    last_used: Option<Instant>,
}

/// Thread-safe on-demand LLM lifecycle manager (`K5`, `R4`).
pub struct OnDemandLlm {
    inner: Mutex<InnerState>,
    gen_lock: Mutex<()>,
    busy: AtomicBool,
}

impl OnDemandLlm {
    pub fn new(models_dir: PathBuf) -> Self {
        Self {
            inner: Mutex::new(InnerState {
                models_dirs: vec![models_dir],
                selection: LlmSelection::Auto,
                loaded: None,
                last_used: None,
            }),
            gen_lock: Mutex::new(()),
            busy: AtomicBool::new(false),
        }
    }

    /// Scans both the app-local `models/` directory (standalone GGUFs) and the user's local Ollama models store.
    pub fn for_app_data(app_models_dir: PathBuf) -> Self {
        let mut dirs = vec![app_models_dir];
        let ollama = Self::default_ollama_models_dir();
        if !dirs.contains(&ollama) {
            dirs.push(ollama);
        }
        Self {
            inner: Mutex::new(InnerState {
                models_dirs: dirs,
                selection: LlmSelection::Auto,
                loaded: None,
                last_used: None,
            }),
            gen_lock: Mutex::new(()),
            busy: AtomicBool::new(false),
        }
    }

    pub fn default_ollama_models_dir() -> PathBuf {
        #[cfg(windows)]
        let home = std::env::var_os("USERPROFILE")
            .or_else(|| std::env::var_os("HOME"))
            .map(PathBuf::from);
        #[cfg(not(windows))]
        let home = std::env::var_os("HOME").map(PathBuf::from);

        home.unwrap_or_else(|| PathBuf::from("."))
            .join(".ollama")
            .join("models")
    }

    pub fn default_system() -> Self {
        Self::new(Self::default_ollama_models_dir())
    }

    pub fn set_selection(&self, selection: LlmSelection) -> Result<LlmRuntimeStatus, LlmError> {
        {
            let mut g = self.inner.lock().unwrap_or_else(|p| p.into_inner());
            if let LlmSelection::Path(ref p) = selection {
                let s = p.to_string_lossy();
                if s != BUILTIN_EXTRACTIVE_ID {
                    let discovered = discover_local_llms_in_dirs(&g.models_dirs);
                    let in_discovered = discovered.iter().any(|d| Path::new(&d.path) == p.as_path());
                    if !in_discovered && inspect_llm_gguf_header(p, None, "selected").is_none() {
                        return Err(LlmError::ModelNotFound(p.display().to_string()));
                    }
                }
            }
            if g.selection != selection {
                g.loaded = None;
                g.last_used = None;
                g.selection = selection;
            }
        }
        Ok(self.status())
    }

    pub fn status(&self) -> LlmRuntimeStatus {
        let (models_dirs, selection, loaded, loaded_id) = {
            let g = self.inner.lock().unwrap_or_else(|p| p.into_inner());
            let loaded_id = g.loaded.as_ref().map(|(d, _)| d.id.clone());
            (
                g.models_dirs.clone(),
                g.selection.clone(),
                g.loaded.is_some(),
                loaded_id,
            )
        };
        let discovered = discover_local_llms_in_dirs(&models_dirs);
        let active = resolve_descriptor(&discovered, &selection);
        let (selection_mode, selected_path) = match &selection {
            LlmSelection::Auto => ("auto".to_string(), None),
            LlmSelection::BuiltinExtractive => {
                ("builtin".to_string(), Some(BUILTIN_EXTRACTIVE_ID.to_string()))
            }
            LlmSelection::Path(p) => ("custom".to_string(), Some(p.to_string_lossy().to_string())),
        };

        LlmRuntimeStatus {
            active,
            discovered,
            loaded,
            loaded_model_id: loaded_id,
            selection_mode,
            selected_path,
        }
    }

    pub fn is_loaded(&self) -> bool {
        let g = self.inner.lock().unwrap_or_else(|p| p.into_inner());
        g.loaded.is_some()
    }

    pub fn unload(&self) {
        let mut g = self.inner.lock().unwrap_or_else(|p| p.into_inner());
        g.loaded = None;
        g.last_used = None;
    }

    /// Unloads any idle loaded model when `last_used.elapsed() >= max_idle_ms` (never unloads mid-generation).
    pub fn unload_if_idle(&self, max_idle_ms: u64) -> bool {
        if self.busy.load(Ordering::Acquire) {
            return false;
        }
        let mut g = self.inner.lock().unwrap_or_else(|p| p.into_inner());
        if g.loaded.is_none() {
            return false;
        }
        let expired = g
            .last_used
            .is_some_and(|t| t.elapsed() >= Duration::from_millis(max_idle_ms));
        if expired {
            g.loaded = None;
            g.last_used = None;
            return true;
        }
        false
    }

    /// Loads model on demand (or reuses warm compact model <= 2.5 GB), streams tokens,
    /// and immediately unloads > 2.5 GB models after generation (`K5`).
    pub fn generate_streaming<F>(
        &self,
        req: &GenerateRequest,
        cancel: &AtomicBool,
        mut on_token: F,
    ) -> Result<(LlmModelDescriptor, GenerateOutput), LlmError>
    where
        F: FnMut(&str),
    {
        let _gen_guard = self.gen_lock.lock().unwrap_or_else(|p| p.into_inner());
        struct BusyReset<'a>(&'a AtomicBool);
        impl Drop for BusyReset<'_> {
            fn drop(&mut self) {
                self.0.store(false, Ordering::Release);
            }
        }
        self.busy.store(true, Ordering::Release);
        let _busy_guard = BusyReset(&self.busy);

        let (desc, generator) = self.ensure_loaded()?;
        let res = generator.generate(req, cancel, &mut on_token);
        drop(generator);

        // K5: Large models (> 2.5 GB) unload immediately after generation; compact models stay warm until 60s idle.
        if desc.size_bytes > K5_LLM_KEEP_LOADED_MAX_BYTES {
            self.unload();
        } else {
            let mut g = self.inner.lock().unwrap_or_else(|p| p.into_inner());
            g.last_used = Some(Instant::now());
        }

        let output = res?;
        Ok((desc, output))
    }

    fn ensure_loaded(&self) -> Result<(LlmModelDescriptor, Arc<dyn TextGenerator>), LlmError> {
        let (models_dirs, selection, current) = {
            let mut g = self.inner.lock().unwrap_or_else(|p| p.into_inner());
            let cached = if let Some((desc, generator)) = g.loaded.as_ref() {
                let still_matches = match &g.selection {
                    LlmSelection::BuiltinExtractive => desc.id == BUILTIN_EXTRACTIVE_ID,
                    LlmSelection::Path(p) => Path::new(&desc.path) == p.as_path(),
                    LlmSelection::Auto => true,
                };
                if still_matches {
                    Some((desc.clone(), generator.clone()))
                } else {
                    None
                }
            } else {
                None
            };
            if let Some((desc, generator)) = cached {
                g.last_used = Some(Instant::now());
                return Ok((desc, generator));
            }
            (g.models_dirs.clone(), g.selection.clone(), g.loaded.take())
        };
        drop(current);

        let discovered = discover_local_llms_in_dirs(&models_dirs);
        let desc = resolve_descriptor(&discovered, &selection);
        let generator = load_generator_for_descriptor(&desc)?;

        let mut g = self.inner.lock().unwrap_or_else(|p| p.into_inner());
        g.loaded = Some((desc.clone(), generator.clone()));
        g.last_used = Some(Instant::now());
        Ok((desc, generator))
    }
}

fn resolve_descriptor(
    discovered: &[LlmModelDescriptor],
    selection: &LlmSelection,
) -> LlmModelDescriptor {
    match selection {
        LlmSelection::BuiltinExtractive => builtin_extractive_descriptor(),
        LlmSelection::Path(p) => {
            if p.to_string_lossy() == BUILTIN_EXTRACTIVE_ID {
                return builtin_extractive_descriptor();
            }
            if let Some(d) = discovered.iter().find(|d| Path::new(&d.path) == p.as_path()) {
                return d.clone();
            }
            if p.is_file()
                && let Some(d) = inspect_llm_gguf_header(p, None, "selected")
            {
                return d;
            }
            builtin_extractive_descriptor()
        }
        LlmSelection::Auto => discovered
            .iter()
            .find(|d| d.recommended && d.id != BUILTIN_EXTRACTIVE_ID)
            .cloned()
            .unwrap_or_else(builtin_extractive_descriptor),
    }
}

fn load_generator_for_descriptor(
    desc: &LlmModelDescriptor,
) -> Result<Arc<dyn TextGenerator>, LlmError> {
    match desc.backend.as_str() {
        "builtin_extractive" => Ok(Arc::new(ExtractiveGenerator::new())),
        b if b.starts_with("candle_gguf_") => Ok(Arc::new(GgufLlm::load(
            Path::new(&desc.path),
            Some(&desc.name),
        )?)),
        other => Err(LlmError::ModelLoad(format!(
            "unknown LLM backend: {other}"
        ))),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::ContextSnippet;

    #[test]
    fn auto_selection_falls_back_to_extractive_and_unloads_when_idle() {
        let dir = tempfile::tempdir().unwrap();
        let mgr = OnDemandLlm::new(dir.path().to_path_buf());
        let st = mgr.status();
        assert_eq!(st.active.id, BUILTIN_EXTRACTIVE_ID);
        assert!(!st.loaded);

        let cancel = AtomicBool::new(false);
        let req = GenerateRequest {
            user_prompt: "What is the Atlas backup port?".to_string(),
            context_snippets: vec![ContextSnippet {
                ref_id: 1,
                label: "infra.md".to_string(),
                text: "Atlas backup listens on port 8443 via local socket.".to_string(),
            }],
            ..Default::default()
        };

        let mut tokens = Vec::new();
        let (desc, out) = mgr
            .generate_streaming(&req, &cancel, |tok| tokens.push(tok.to_string()))
            .unwrap();
        assert_eq!(desc.id, BUILTIN_EXTRACTIVE_ID);
        assert!(out.text.contains("8443"));
        assert!(!tokens.is_empty());
        assert!(mgr.is_loaded());

        // Zero-ms idle threshold unloads the warm model immediately (`K5`).
        assert!(mgr.unload_if_idle(0));
        assert!(!mgr.is_loaded());
    }
}
