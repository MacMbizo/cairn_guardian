# Plan 6B: Local Embeddings & Context-Aware Hybrid Search

**Status:** Done — built, verified, and fast-forward merged into `master` on 2026-11-24 (not pushed).

**Goal:** Deliver local-only semantic embeddings (`crates/aegis-embed`) and context-aware FTS5 + `sqlite-vec` hybrid search inside the SQLCipher-encrypted `aegis.db` vault while respecting every North Star constraint.

## North-star rules

This plan upholds the binding rules in `docs/superpowers/specs/2026-11-23-local-intelligence-north-star.md`:
- **K1 (propose, approve, undo):** Search, semantic indexing, and vector pruning are purely read/index operations over the encrypted catalog and touch zero owner files on disk.
- **K2 (quarantined files excluded by default):** Every vector KNN candidate passes through the authoritative SQLite `FILTERS` post-filter excluding deleted and quarantined files unless `include_quarantined` is explicitly enabled.
- **K3 (local only) & R9 (no network crates):** Pure-Rust inference via `candle-core` + `candle-nn` (CPU default features) over local GGUF/Safetensors models plus a built-in lexical hash fallback; no HTTP calls, no Ollama daemon requests, and verified by `r9_forbids_networking_crates_in_engine_and_models`.
- **K5 (background budget <= 350 MB):** Compact quantized `QTensor` / `QMatMul` weights, single-sequence inference, immediate unload after batch indexing (`embed_documents_and_unload`), idle unload timer (`60s`), `store.shrink_memory()`, and `needs_ac()` AC-power gating on background neural `EmbedWatch` passes.
- **R4 (on-demand models on CPU):** Models load only when actively embedding or searching and unload on idle or window close; zero CUDA/MKL/CMake/Perl/NASM requirements.
- **R7 (personal data protected, not masked from search):** Sensitive files (`confidential`/`secret`) are indexed and searchable semantically and lexically, while snippets from both FTS5 and semantic chunks stay masked (`SENSITIVE_MASK`) unless unlocked via Windows CredUI.
- **D1 (encrypted store):** Embeddings live inside `vec_chunks` (`sqlite-vec` `0.1.9`) and `chunk_embeddings_meta` inside the single SQLCipher-encrypted `aegis.db`.

## Architecture & Verification Checklist

1. **Step 1 (`crates/aegis-embed`):** `WordPieceTokenizer`, `GgufEmbedder` (`nomic-bert` + `bert`), `SafetensorsBertEmbedder`, `LexicalHashEmbedder` (`384`-d fallback), local model discovery (app `models/` + local `.ollama/models` blobs via GGUF header metadata only), and `OnDemandEmbedder` lifecycle manager; unit tests + live `nomic-embed-text` semantic margin check (`> 0.20`) + immediate `R9` `Cargo.lock` verification.
2. **Step 2 (`crates/aegis-core`):** `vec.rs` (`vec_chunks` table lifecycle, change-hashed pending queue, outbox marking, orphan pruning), `search.rs` (RRF `k = 60` hybrid fusion + context collection filtering + context boosts + `R7` snippet masking), Doctor check #4 orphan repair, and `Watch::needs_ac()` scheduler gating.
3. **Step 3 (`src-tauri`):** `AppState.embedder`, `commands/embed.rs` (`embed_status`, `embed_sync_now`, `embed_select_model`, `embed_unload`), hybrid `search_query`, `EmbedWatch` with AC-power gating + idle unload, and window-destroy weight unload.
4. **Step 4 (`src/`):** Generated TS bindings, `aegis.embed` client methods, `Project / Set` filter in `SearchFilterBar.tsx`, mode toggle (`Hybrid` / `Keyword` / `Semantic`) + enrichment badges in `App.tsx`, and **Semantic Search & Local Embeddings** panel in `EngineSettings.tsx`.
5. **Step 5–6:** `crates/aegis-core/tests/hybrid_search.rs` (6 integration tests), `cargo clippy --workspace --all-targets -- -D warnings`, full `cargo test --workspace`, `cargo check -p aegis-desktop --features custom-protocol`, `pnpm build`, quarantine `0` before/after, doc updates, and fast-forward merge into `master`.

## Verification

- `cargo test --workspace`: **525 passed, 0 failed, 1 ignored** (`usn_refresh_on_system_temp` in `drives.rs`), including `6/6` in `crates/aegis-core/tests/hybrid_search.rs` (`0.12s`), `6/6` in `crates/aegis-core/tests/d1_encryption.rs` (`0.64s`), `5/5` in `crates/aegis-embed` (`8.46s`, including live `nomic_gguf_live_semantic_check`), and `11/11` in `aegis-desktop`.
- `cargo clippy --workspace --all-targets -- -D warnings`: clean (`0` warnings).
- `cargo check -p aegis-desktop --features custom-protocol`: clean (`exit 0`).
- `pnpm build` (`tsc --noEmit && vite build`): clean (`dist/` built in `808ms`).
- `C:\.aegis-quarantine`: `0` folders before and `0` after.
