# Automatic Knowledge Extraction Implementation Plan

**Branch:** `feature/knowledge-extraction` (cut from `master` at `6de7544`)  
**Capability:** #8 — Automatic Knowledge Extraction (`2.2`) per `docs/superpowers/specs/2026-11-23-local-intelligence-north-star.md`

---

## North-Star Constraints & Invariants

1. **K1 (Human-in-the-Loop Inbox + Undo):** Extracted entities (`Person`, `Organization`, `Project`, `Decision`, `ActionItem`) NEVER commit automatically to `entities` or `relationships`. They land as provisional rows in `knowledge_proposals` (`status = 'pending'`), support inline modification before approval, incrementally merge into existing entities on approval, and support single-click `undo_approved_proposal` reverting both `entities` and `relationships`.
2. **K2 (Quarantine & Missing Exclusion):** Extraction scans and entity provenance queries exclude deleted, quarantined, and missing files (`deleted_at IS NULL AND state NOT IN ('quarantine', 'missing')`).
3. **K3 / R9 (100% Local Zero-Network Inference & Parsing):** Pure-Rust `.vtt`/`.srt` transcript normalization, regex/structural NLP heuristics reusing `crate::filing::orgs::ORGS`, and optional on-demand CPU local LLM extraction reusing `aegis-llm` (`r9_forbids_networking_crates_in_engine_and_models`).
4. **K4 (Opt-in Observation & Backup Exclusion):** Backup drives (`COALESCE(v.role, '') = 'backup'`) are excluded at SQL query time. Meeting audio/video observation (`audio_observation_enabled`) is OFF by default in `settings`; companion `.vtt`/`.srt` pairing surfaces only when explicitly opted in.
5. **K5 & R4 (On-Demand Inference Only):** Catalog scans use zero LLM RAM and track `knowledge_file_runs` watermarks for idempotency. Deep per-file LLM extraction loads `OnDemandLlm` only when clicked by the owner, off the SQLite lock, and unloads on `K5` idle/size policy.
6. **R7 (Privacy & Sensitivity Gate):** Confidential/secret files are skipped during default extraction (`files_skipped_sensitive`), require both `include_sensitive = true` AND Windows CredUI unlock (`reveal_sensitive = true`) to extract or run LLM extraction, and mask `evidence_quote` / `summary` with `SENSITIVE_MASK` once relocked.
7. **D1 (Encrypted Metadata Storage):** `entity_aliases`, `knowledge_proposals`, and `knowledge_file_runs` live inside SQLCipher-encrypted `aegis.db` via migration `0012_knowledge_extraction.sql`.

---

## Architecture & Steps

1. **Schema (`crates/aegis-core`)**
   - `migrations/0012_knowledge_extraction.sql` + `db/mod.rs` v12 + `tests/drives.rs` (`1..=12`), reusing existing B4 `entities` & `relationships`.
2. **Core Engine (`crates/aegis-core/src/knowledge/`)**
   - `mod.rs`: Contracts (`EntityType`, `ProposalStatus`, `KnowledgeExtractOptions`, `KnowledgeExtractReport`, `ProposalEditInput`, `KnowledgeProposal`, `EntityProvenance`, `EntityRelationLink`, `KnowledgeEntity`, `AudioRecordingItem`, `EntityTypeCount`, `KnowledgeQueryOptions`, `KnowledgeDashboard`, `KnowledgeCommitResult`) and `Screen::Knowledge` (`engine/mod.rs`).
   - `parser.rs`: VTT/SRT transcript normalization, deterministic entity/decision/action extractor, `build_knowledge_llm_prompt`, `parse_llm_extraction_output`.
   - `store.rs`: Idempotent watermarked scan (`extract_knowledge_from_catalog`), `ingest_llm_candidates`, `approve_proposal` with incremental merge + undo snapshots, `undo_approved_proposal`, `dismiss_proposal`, `reopen_proposal`, `update_entity_status`, `K4` audio opt-in & companion transcript pairing, and `query_knowledge_dashboard`.
3. **Contracts & Desktop IPC (`contracts/mod.rs`, `src-tauri/`)**
   - `ts-rs` exports for all 14 `knowledge` types + `Screen::Knowledge`.
   - `commands/knowledge.rs`: 10 Tauri async commands including `knowledge_extract_file_llm`.
4. **Frontend (`src/`)**
   - `src/bindings/client.ts` (`aegis.knowledge`), `src/screens/fixtures/knowledge.ts`, `src/screens/KnowledgeBase.tsx` (`Review Inbox`, `Knowledge Graph`, `Transcripts & Audio`, `aegis.file.get` inspection), and `src/App.tsx` (`Knowledge Base` tab).
5. **Verification & Merge**
   - `crates/aegis-core/tests/knowledge_extraction.rs` (`7` integration tests) + unit tests in `knowledge::parser::tests`.
   - Full `clippy`, `cargo test --workspace`, `R9`, `cargo check -p aegis-desktop --features custom-protocol`, `pnpm build`, quarantine `0` verification, doc updates, and `--ff-only` merge into `master`.

---

## Verification Checklist

- [x] `cargo test -p aegis-core --lib` (`179 passed, 0 failed`, incl. `knowledge::parser::tests`)
- [x] `cargo test -p aegis-core --test knowledge_extraction -- --nocapture` (`7 passed, 0 failed` in `0.21s`)
- [x] `cargo test -p aegis-core --test drives` (`19 passed, 0 failed, 1 ignored`)
- [x] `cargo test -p aegis-core --test d1_encryption r9_forbids_networking_crates_in_engine_and_models` (`1 passed`)
- [x] `cargo clippy --workspace --all-targets -- -D warnings` (`0` warnings)
- [x] `cargo test --workspace` (**617 passed, 0 failed, 2 ignored**)
- [x] `cargo check -p aegis-desktop --features custom-protocol` (exit `0`)
- [x] `pnpm build` (exit `0`, `dist/` built in `1.01s`)
- [x] `C:\.aegis-quarantine` verified `0` items before and after
