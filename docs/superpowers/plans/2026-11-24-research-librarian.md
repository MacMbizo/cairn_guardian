# Autonomous Research Librarian Implementation Plan

**Branch:** `feature/research-librarian` (cut from `master` at `87b9824`)  
**Capability:** #9 — Autonomous Research Librarian (`2.3`) per `docs/superpowers/specs/2026-11-23-local-intelligence-north-star.md`

---

## North-star rules

1. **K1 (Read-Only on Disk & Reversible Curation):** The Research Librarian is strictly read-only toward the owner's files on disk. Curated reading-list pins (`librarian_pins`) and dismissed suggestions (`librarian_dismissals`) live inside encrypted metadata and can be toggled/undone in one click.
2. **K2 (Quarantine & Missing Exclusion):** Every document shelf query, vector/lexical lookup, entity graph traversal, and reading brief skips deleted, quarantined, and missing files (`deleted_at IS NULL AND state NOT IN ('quarantine', 'missing')`).
3. **K3 / R9 (100% Local Zero-Network Inference):** All semantic association, FTS5 lexical matching, entity graph linking, and optional LLM brief synthesis run purely locally in Rust without adding any crates (`r9_forbids_networking_crates_in_engine_and_models`).
4. **K4 (Backup Drive Exclusion):** Backup volumes (`COALESCE(v.role, '') = 'backup'`) are excluded at SQL query time.
5. **K5 & R4 (On-Demand Inference Only):** File-to-file cross-referencing uses pre-computed encrypted `vec_chunks` + `chunks_fts` + approved Knowledge Graph `entities`/`relationships` at `0 MB` LLM/embedder RAM. Ad-hoc topic embedding (`OnDemandEmbedder`) and optional reading-brief synthesis (`OnDemandLlm` behind `rag_running`) execute strictly off the SQLite lock on owner action.
6. **R5 & R11 (No Screen Capture or Keystroke Monitoring):** Per `R5` and `R11`, the Research Librarian works strictly from saved documents on disk (catalog/chunks/vectors) and explicit owner-entered topic queries or pasted excerpts. It NEVER captures active windows, keystrokes, clipboard, or screen state.
7. **R7 (Privacy & Sensitivity Gate):** Confidential/secret documents are excluded from shelves, semantic similarity candidates, shared-entity traversals, and reading briefs when `!reveal_sensitive` (reporting only `excluded_sensitive_count` without leaking text or scores), and sensitive source files or `include_sensitive = true` requests are refused until Windows authentication unlocks sensitive previews.
8. **D1 (Encrypted Metadata Storage):** `librarian_pins` and `librarian_dismissals` are stored inside SQLCipher-encrypted `aegis.db` via migration `0013_research_librarian.sql`.

---

## Architecture & Steps

1. **Schema (`crates/aegis-core`)**
   - `migrations/0013_research_librarian.sql` + `db/mod.rs` v13 + `tests/drives.rs` (`1..=13`).
2. **Core Engine (`crates/aegis-core/src/librarian/`)**
   - `mod.rs`: Contracts (`ResearchDocItem`, `ResearchDocShelf`, `SharedEntityRef`, `AssociationReason`, `RelatedReference`, `LibrarianOptions`, `ResearchCompanionReport`, `BriefCitation`, `ResearchBrief`) + `Screen::Librarian` (`engine/mod.rs`).
   - `engine.rs`: Recent research document shelf (`list_recent_research_docs`), three-signal cross-referencing (`find_related_for_file` + `find_related_for_topic`) combining encrypted `vec_chunks` KNN (`semantic_only` detection for zero keyword overlap), FTS5 BM25 lexical terms, and Capability #8 approved Knowledge Graph entities/aliases + cross-project detection, `librarian_pins` / `librarian_dismissals` persistence, and grounded LLM brief prompt builder (`prepare_brief_context`).
3. **Contracts & Desktop IPC (`contracts/mod.rs`, `src-tauri/`)**
   - `ts-rs` exports for all 9 `librarian` types + `Screen::Librarian`.
   - `commands/librarian.rs`: 8 async Tauri commands (`librarian_recent_docs`, `librarian_related_for_file`, `librarian_related_for_topic` with off-lock `OnDemandEmbedder`, `librarian_synthesize_brief` with off-lock `OnDemandLlm` + `rag_running` guard, `librarian_pin`, `librarian_unpin`, `librarian_dismiss`, `librarian_undismiss`).
4. **Frontend (`src/`)**
   - `src/bindings/client.ts` (`aegis.librarian`), `src/screens/fixtures/librarian.ts`, `src/screens/ResearchLibrarian.tsx` (Active Document vs. Topic/Excerpt toggle, `R5`/`R11` badge, cross-reference filters, conceptual parallel & cross-project badges, Knowledge Base entity pills, pin/dismiss actions, read-only `aegis.file.get` preview, on-demand reading brief), and `src/App.tsx` (`Research Librarian` tab).
5. **Verification & Merge**
   - `crates/aegis-core/tests/research_librarian.rs` (`6` integration tests) + unit test in `librarian::engine::tests`.
   - Full `clippy`, `cargo test --workspace`, `R9`, `cargo check -p aegis-desktop --features custom-protocol`, `pnpm build`, quarantine `0` verification, doc updates, and `--ff-only` merge into `master`.

---

## Verification Checklist

- [x] `cargo test -p aegis-core --lib` (`189 passed, 0 failed` in `0.59s`, incl. `librarian::engine::tests`)
- [x] `cargo test -p aegis-core --test research_librarian` (`6 passed, 0 failed` in `0.16s`)
- [x] `cargo test -p aegis-core --test drives` (`19 passed, 0 failed, 1 ignored` in `1.45s`)
- [x] `cargo test -p aegis-core --test d1_encryption r9_forbids_networking_crates_in_engine_and_models` (`1 passed` within `6/6` in `d1_encryption.rs`)
- [x] `cargo clippy --workspace --all-targets -- -D warnings` (`0` warnings)
- [x] `cargo test --workspace` (**633 passed, 0 failed, 2 ignored**)
- [x] `cargo check -p aegis-desktop --features custom-protocol` (exit `0`, `38.32s`)
- [x] `pnpm build` (exit `0`, `dist/` built in `1.10s`)
- [x] `C:\.aegis-quarantine` verified `0` items before and after
