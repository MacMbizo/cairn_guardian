# Local RAG Engine Implementation Plan

**Branch:** `feature/local-rag` (cut from `master` at `07fc762`)  
**Capability:** #3 — Local RAG Engine (`Ask Files`) per `docs/superpowers/specs/2026-11-23-local-intelligence-north-star.md`

---

## North-Star Constraints

1. **K1 / K2 (Read-only & Quarantine Exclusion):** Retrieve chunks read-only over the SQLCipher-encrypted `aegis.db` via `search_hybrid`; always enforce `include_quarantined = Some(false)`.
2. **K3 / R9 (100% Local Pure-Rust Inference):** All LLM inference runs in-process via `candle-core` + `candle-nn` (`0.11.1`) or the built-in zero-RAM extractive fallback. Zero HTTP calls, zero Ollama daemon calls, zero networking crates (`r9_forbids_networking_crates_in_engine_and_models` extended to `["aegis-core", "aegis-embed", "aegis-llm"]`).
3. **K5 / R4 (On-Demand Memory & Unload Discipline):**
   - Models load strictly on demand on CPU.
   - Compact models (`<= 2.5 GB`) stay cached until `60s` idle (`unload_if_idle(60_000)`) or `WindowEvent::Destroyed`.
   - Large models (`> 2.5 GB`) unload immediately upon generation completion.
   - Models `<= 4,500 MB` are marked `recommended` during Ollama manifest discovery (selecting local 1.9 GB `llama3.2` automatically when present, with `builtin:extractive` always available).
4. **R7 (Privacy & Sensitivity Gate):**
   - Files marked `confidential` or `secret` are excluded from RAG prompts by default, with `excluded_sensitive_count` counted without loading their chunk contents.
   - Setting `include_sensitive = true` is rejected with `AegisError::PermissionDenied` unless Windows CredUI sensitive unlock is active in the current session.
5. **D1 / D2 (Encrypted Index Integrity):** Vectors and chunk previews are read directly from SQLCipher `aegis.db`; no sidecar plaintext files are written.

---

## Architecture & Steps

1. **Crate `crates/aegis-llm` + Workspace & `R9` Extension**
   - `discovery.rs`: Recursive walk of `%USERPROFILE%\.ollama\models\manifests\` (`registry.ollama.ai` and `hf.co` layouts), SHA-256 blob verification, GGUF header metadata inspection, plus `builtin:extractive`.
   - `tokenizer.rs`: Pure-Rust GGUF BPE/GPT-2 (`llama-bpe`, `qwen2`), SentencePiece, and chat template formatting (`llama3`, `chatml`, `fallback`).
   - `extractive.rs`: Zero-RAM grounded extractive summarizer and citation formatter.
   - `gguf.rs`: Pure-Rust quantized GGUF inference (`Llama` interleaved `rope_i` + optional `rope_freqs.weight` + tied LM head fallback, and `Qwen2` split `rope` + QKV biases) with KV cache and top-p/temperature/repetition sampling.
   - `manager.rs`: Thread-safe `OnDemandLlm` with idle unload (`K5`), large-model immediate unload, and cooperative cancellation (`AtomicBool`).
   - Update `Cargo.toml` (`"crates/aegis-llm"`) and `crates/aegis-core/tests/d1_encryption.rs` (`["aegis-core", "aegis-embed", "aegis-llm"]`).
2. **Core RAG Retrieval & Contracts (`crates/aegis-core`)**
   - `crates/aegis-core/src/rag/mod.rs`: `retrieve_rag_context`, grounded prompt construction, strict `K2` & `R7` enforcement, and `RagAnswer` / `RagCitation` assembly.
   - `crates/aegis-core/src/contracts/mod.rs`: `AegisEvent::RagToken` and TS exports for `LlmModelInfo`, `LlmStatus`, `RagCitation`, `RagAnswer`, `RagAskOptions`.
3. **Desktop IPC & Lifecycle (`src-tauri`)**
   - `src-tauri/src/commands/rag.rs`: `rag_status`, `rag_select_model`, `rag_unload`, `rag_cancel`, `rag_ask`.
   - Persist `rag.model_path` in encrypted `kv_meta`, restore on startup/unlock, hook `unload_if_idle(60s)` in background host tick and `state.llm.unload()` on `WindowEvent::Destroyed`.
4. **Frontend UI (`src/`)**
   - `src/bindings/client.ts`: `aegis.rag` typed wrappers + `rag-token` event listener.
   - `src/screens/AskFiles.tsx` + `src/screens/fixtures/rag.ts`: Streaming Q&A console, model selector, sensitive gate toggle/feedback, clickable file citations with reveal-in-Explorer.
   - `src/screens/EngineSettings.tsx`: **Local RAG Chat Model (`R4` / `K5`)** settings card.
   - `src/App.tsx`: `Ask Files` navigation tab (`id: "ask"`).
5. **Verification & Integration**
   - Unit tests + live `llama3.2` grounded answer test (`--ignored`) in `aegis-llm`.
   - Integration tests in `crates/aegis-core/tests/local_rag.rs`.
   - Full workspace `clippy`, `cargo test --workspace`, `cargo check -p aegis-desktop --features custom-protocol`, `pnpm build`, quarantine cleanliness check, documentation updates, and `--ff-only` merge into `master`.

---

## Verification Checklist

- [x] `cargo test -p aegis-llm -- --nocapture` (`4 passed, 0 failed, 1 ignored`)
- [x] `cargo test -p aegis-llm --release -- --ignored --nocapture` (`1 passed` in `13.37s`, live `1.9 GB` `llama3.2` grounded answer citing `4092` and `[1]`)
- [x] `cargo test -p aegis-core --test d1_encryption r9_forbids_networking_crates_in_engine_and_models` (`1 passed` covering `["aegis-core", "aegis-embed", "aegis-llm"]`)
- [x] `cargo test -p aegis-core --lib` (`139 passed`, exports TS bindings cleanly)
- [x] `cargo check -p aegis-desktop --features custom-protocol` (exit `0`)
- [x] `cargo test -p aegis-desktop --lib` (`11 passed`)
- [x] `pnpm build` (exit `0`, `dist/` built in `862ms`)
- [x] `cargo test -p aegis-core --test local_rag` (`5 passed, 0 failed`)
- [x] `cargo clippy --workspace --all-targets -- -D warnings` (`0` warnings)
- [x] `cargo test --workspace` (**557 passed, 0 failed, 2 ignored**)
- [x] `C:\.aegis-quarantine` verified `0` items before and after
