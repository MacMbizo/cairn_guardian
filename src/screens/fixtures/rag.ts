import type { LlmStatus, RagAnswer } from "../../bindings/client";

export const PREVIEW_LLM_STATUS: LlmStatus = {
  model_id: "gguf:llama3.2:latest",
  model_name: "llama3.2:latest (preview)",
  backend: "candle_gguf_llama",
  architecture: "llama",
  context_length: 131072,
  loaded: false,
  selected_path: null,
  available_models: [
    {
      id: "gguf:llama3.2:latest",
      name: "llama3.2:latest",
      path: "preview://llama3.2",
      backend: "candle_gguf_llama",
      architecture: "llama",
      context_length: 131072,
      size_bytes: 2_019_377_440,
      source: "ollama_local",
      recommended: true,
    },
    {
      id: "builtin:extractive",
      name: "Built-in Extractive Summarizer (0 MB)",
      path: "builtin:extractive",
      backend: "builtin_extractive",
      architecture: "extractive",
      context_length: 8192,
      size_bytes: 0,
      source: "builtin",
      recommended: true,
    },
  ],
  on_demand_policy:
    "Loads on demand (CPU); <= 2.5 GB unloads after 60s idle or window close, > 2.5 GB unloads immediately.",
};

export function previewRagAnswer(question: string): RagAnswer {
  return {
    request_id: `preview-${Date.now()}`,
    question,
    answer:
      "Preview mode: in the desktop app, Cairn retrieves top hybrid excerpts from your SQLCipher-encrypted index (excluding quarantined, deleted, and locked sensitive files) and streams an offline Candle GGUF or built-in extractive answer with inline [1] citations.",
    citations: [
      {
        ref_id: 1,
        file_id: "preview-1",
        path: "C:\\Users\\Owner\\Documents\\Architecture\\Local-Intelligence-Notes.md",
        filename: "Local-Intelligence-Notes.md",
        snippet:
          "All RAG queries read exclusively from SQLCipher aegis.db using hybrid FTS5 + vector retrieval. Sensitive files require active Windows authentication.",
        score: 0.94,
        sensitivity: "public",
        matched_by: "hybrid",
      },
    ],
    excluded_sensitive_count: 1,
    model_id: PREVIEW_LLM_STATUS.model_id,
    model_name: PREVIEW_LLM_STATUS.model_name,
    backend: PREVIEW_LLM_STATUS.backend,
    prompt_tokens: 96,
    tokens_generated: 42,
    duration_ms: 480,
    cancelled: false,
  };
}
