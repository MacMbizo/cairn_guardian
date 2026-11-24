//! Pure-Rust local LLM discovery, quantized GGUF inference, and zero-RAM extractive fallback (`K3`, `K5`, `R4`, `R9`).

pub mod discovery;
pub mod extractive;
pub mod gguf;
pub mod manager;
pub mod tokenizer;

use std::sync::atomic::AtomicBool;

use serde::{Deserialize, Serialize};
use thiserror::Error;

pub use discovery::{
    BUILTIN_EXTRACTIVE_ID, K5_LLM_KEEP_LOADED_MAX_BYTES, K5_LLM_RECOMMEND_MAX_BYTES,
    LlmModelDescriptor, builtin_extractive_descriptor, discover_local_llms,
    discover_local_llms_in_dirs, inspect_llm_gguf_header,
};
pub use extractive::ExtractiveGenerator;
pub use gguf::GgufLlm;
pub use manager::{LlmRuntimeStatus, LlmSelection, OnDemandLlm};

#[derive(Debug, Error)]
pub enum LlmError {
    #[error("local LLM model not found: {0}")]
    ModelNotFound(String),
    #[error("local LLM load error: {0}")]
    ModelLoad(String),
    #[error("local LLM inference error: {0}")]
    Inference(String),
    #[error("generation cancelled")]
    Cancelled,
    #[error("I/O error: {0}")]
    Io(#[from] std::io::Error),
}

impl From<candle_core::Error> for LlmError {
    fn from(err: candle_core::Error) -> Self {
        Self::Inference(err.to_string())
    }
}

/// Optional structured context snippet used by zero-RAM extractive generation.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ContextSnippet {
    pub ref_id: usize,
    pub label: String,
    pub text: String,
}

/// Parameters controlling local text generation.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct GenerateRequest {
    pub system_prompt: String,
    pub user_prompt: String,
    pub max_tokens: usize,
    pub temperature: f32,
    pub top_p: f32,
    pub repetition_penalty: f32,
    pub seed: u64,
    pub stop_sequences: Vec<String>,
    pub context_snippets: Vec<ContextSnippet>,
}

impl Default for GenerateRequest {
    fn default() -> Self {
        Self {
            system_prompt: "You are a helpful local file assistant.".to_string(),
            user_prompt: String::new(),
            max_tokens: 384,
            temperature: 0.2,
            top_p: 0.9,
            repetition_penalty: 1.1,
            seed: 42,
            stop_sequences: Vec::new(),
            context_snippets: Vec::new(),
        }
    }
}

/// Completed generation metadata and full assembled text.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct GenerateOutput {
    pub text: String,
    pub tokens_generated: usize,
    pub prompt_tokens: usize,
    pub duration_ms: u64,
    pub cancelled: bool,
    pub model_id: String,
    pub backend: String,
}

/// Local text generator trait (`R9` — zero network access).
pub trait TextGenerator: Send + Sync {
    fn descriptor(&self) -> &LlmModelDescriptor;

    fn generate(
        &self,
        req: &GenerateRequest,
        cancel: &AtomicBool,
        on_token: &mut dyn FnMut(&str),
    ) -> Result<GenerateOutput, LlmError>;
}
