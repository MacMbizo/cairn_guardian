# Capability #12: Self-Healing Workspace (`4.1`) Implementation Plan

Date: 2026-11-24  
Branch: `feature/self-healing-workspace` (from `master` at `2d41176`)

## Goal

Build **Self-Healing Workspace** (`4.1` in `docs/superpowers/specs/2026-11-23-local-intelligence-north-star.md` §2.2), delivering:
1. **Dynamic Link & Path Healing** across local Markdown notes & wikilinks (`[label](target)`, `![alt](target)`, `[[target]]`, `![[embed]]`, `[ref]: target`), tracing external moves/renames captured via the USN Journal / scanner (`heal_move_log`) plus internal moves (`file_history` and executed `actions`), relative path reconstruction, `blake3`/stem resolution, ambiguity guards, and quarantine detection (`K2`).
2. **Automated Tag Pruning & Weeding** across YAML frontmatter `tags:` and inline Markdown `#tags`, clustering casing mismatches (`CaseMismatch`), separator/Damerau-Levenshtein variants (`TypoOrVariant`, shielding numeric sequences like `#phase1` vs `#phase2`), and flagging hex color false positives (`#A3E2F1` outside code spans/headings/URLs, patching only YAML frontmatter lists cleanly).
3. **Stale Automation Rule Path Audits & Transaction Integrity** auditing `folderless_rules` (`scope_root`, `dest_root`) for moved/missing targets alongside `aegis doctor` integrity verification (`D1`/`E4`).

All file repairs execute **strictly under `K1`** via a first-class `ActionType::PatchText` integrated into `PolicyContext` -> `propose_batch` -> `simulate_batch` -> `approve_batch` -> `execute_batch` -> `rollback_batch`, using atomic sibling temp file + flush + replace (`MoveFileExW` on Windows) and TOCTOU `blake3` verification before patching and before rollback.

## North-star rules

- **K1 (Propose, approve, undo):** Neither scans nor previews write to disk. Every text repair stages an `ActionType::PatchText` batch via `propose_batch` with full pre-patch/post-patch snapshots persisted in `heal_patch_payloads`, atomic replacement on execution, TOCTOU digest checks on execute/rollback, and one-click undo (`rollback_batch` + `CopyBackKeepingBoth` / `RestoreAsRename` conflict resolution).
- **K2 (Deletes go to quarantine):** Broken links pointing to files currently inside `quarantine_entries` are explicitly flagged as `InQuarantine` rather than auto-patched, and quarantined/deleted files never become healing sources or targets.
- **K3 (Local only):** Pure-Rust Markdown, wikilink, YAML frontmatter, and tag parsers with zero network calls.
- **K4 (Opt-in observation):** Scans only indexed non-backup volumes (`role != 'backup'`), skips cloud placeholders, offline drives, excluded paths, and files over `2 MiB`.
- **K5 (Background budget):** Deterministic on-demand pure-Rust scanning without extra model weights loaded in memory.
- **R1 (No direct MFT parsing):** Relies on the non-admin Windows USN change journal (`drives::refresh` + `scanner`) and catalog `heal_move_log`.
- **R7 (Personal data protection):** Confidential/secret notes are skipped by default (`excluded_sensitive_count` counted without leaking content) and gated on `include_sensitive && reveal_sensitive` (Windows CredUI active unlock).
- **R8 (No multi-pass shredding):** Atomic replacement + normal OS unlinking + reversible pre-patch snapshots.
- **R9 (Code-enforced zero network):** Uses only existing pure-Rust crates (`rusqlite`, `blake3`, `windows-sys` `Win32_Storage_FileSystem`); passes `r9_forbids_networking_crates_in_engine_and_models`.
- **D1 (Encrypted at rest):** All `heal_patch_payloads`, `heal_move_log`, and `heal_dismissals` tables reside inside SQLCipher-encrypted `aegis.db` via migration `0016_self_healing.sql`.

## Architecture & File Map

1. **Schema & Move Tracking**
   - `crates/aegis-core/migrations/0016_self_healing.sql`
   - `crates/aegis-core/src/db/mod.rs` (migration `16`)
   - `crates/aegis-core/src/db/files.rs` (record renames in `heal_move_log` without polluting `file_history`)
   - `crates/aegis-core/src/drives/registry.rs` (rebase `heal_move_log` on drive re-lettering)
2. **`K1` Pipeline Extension (`ActionType::PatchText`)**
   - `crates/aegis-core/src/contracts/mod.rs` (`ActionType::PatchText` + `ts-rs` exports)
   - `crates/aegis-core/src/policy/mod.rs` (`RequireApproval("patch_text_requires_approval")`)
   - `crates/aegis-core/src/scanner/mod.rs` (`pub(crate) fn refresh_text_chunks_if_local`)
   - `crates/aegis-core/src/db/actions.rs` (atomic write, TOCTOU check, execute/preflight/rollback)
3. **Core Healing Engine**
   - `crates/aegis-core/src/healing/{mod.rs,markdown.rs,tags.rs,engine.rs}`
   - `crates/aegis-core/src/engine/mod.rs` (`Screen::Healing`), `crates/aegis-core/src/lib.rs`
4. **Desktop IPC & UI**
   - `src-tauri/src/commands/healing.rs`, `src-tauri/src/commands/mod.rs`, `src-tauri/src/lib.rs`
   - `src/bindings/*`, `src/screens/fixtures/healing.ts`, `src/screens/SelfHealing.tsx`, `src/App.tsx`
5. **Verification & Docs**
   - `crates/aegis-core/tests/self_healing.rs` (7 integration tests) + unit tests in `markdown::tests` & `tags::tests`.
   - Full workspace `clippy`, `cargo test --workspace`, `cargo check -p aegis-desktop --features custom-protocol`, `pnpm build`, quarantine `0` verification, and `--ff-only` merge into `master`.

## Verification Checklist

- [x] `cargo test -p aegis-core --test self_healing` — `7 passed; 0 failed` (`0.25s`)
- [x] `cargo test -p aegis-core --test drives` — `19 passed; 0 failed; 1 ignored` (`1.47s`)
- [x] `cargo test -p aegis-core --test d1_encryption r9_forbids_networking_crates_in_engine_and_models` — passed inside workspace run
- [x] `cargo clippy --workspace --all-targets -- -D warnings` — `0` warnings (`1.29s`)
- [x] `cargo test --workspace` — `690 passed; 0 failed; 2 ignored` (`227` `aegis-core` unit + `443` integration across 24 test binaries + `11` `aegis_desktop` + `5` `aegis_embed` + `4` `aegis_llm`; ignored: `usn_refresh_on_system_temp`, `live_llama32_grounded_rag_answer`)
- [x] `cargo check -p aegis-desktop --features custom-protocol` — passed (`0.48s`)
- [x] `pnpm build` (`tsc --noEmit && vite build`) — `2376` modules built in `909ms`
- [x] `C:\.aegis-quarantine` item count: `0`
