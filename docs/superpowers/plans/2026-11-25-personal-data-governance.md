# Capability #13: Personal Data Governance (`5.1`, reshaped `R7`–`R9`) Implementation Plan

Date: 2026-11-25  
Branch: `feature/personal-data-governance` (from `master` at `9860936`)

## Goal

Build **Personal Data Governance** (`5.1` in `docs/superpowers/specs/2026-11-23-local-intelligence-north-star.md` §2.2, explicitly reshaped by `R7`–`R9`), unifying:
1. **Local High-Precision PII & Secret Discovery (`governance::detector`)** scanning indexed non-backup files purely in Rust (`PaymentCard` with Luhn, `PrivateKeyBlock` PEM headers, `CloudApiKey` AWS/GitHub/Stripe/Slack/Google/high-entropy `.env` skipping placeholders, `GovernmentId` context-gated SSN/SA-ID with checksum/passport/tax, `BankAccountOrIban` MOD-97 + context-gated accounts, `CredentialFile` keystores/env files), storing deterministic `blake3` secret fingerprints and strictly redacted snippets in `security_findings` + `security_finding_locations`.
2. **Reversible `R7` Protection, Auto-Classification Rules & `K1` Quarantine (`governance::engine`)** offering one-click `confidential` / `secret` classification batches with single-click undo (`governance_label_batches` + `governance_label_entries`), persistent privacy rules (`governance_privacy_rules`) with AI exclusion enforcement inside `rag::retrieve_rag_context`, and `PolicyContext`-gated 30-day quarantine staging via `ActionType::Quarantine` (`K1`/`K2`/`R8`).
3. **Reshaped Privacy Posture Audit & BitLocker Telemetry (`governance::posture`)** combining SQLCipher `D1` + DPAPI `R2` + 24-word recovery verification, non-admin Windows BitLocker status checks (`R1`/`R8`), honest code-enforced `R9` zero-network guarantee (no "mathematical" claims + optional Windows Firewall rule), and live `K4`/`R11` opt-in observation audits.

## North-star rules

- **K1 (Propose, approve, undo):** Every file sensitivity batch records revertible snapshots (`governance_label_batches` / `governance_label_entries`) with single-click undo; every exposed-file removal routes strictly through `propose_batch` (`ActionType::Quarantine`) for Simulator approval & rollback.
- **K2 (Deletes go to quarantine):** Quarantined/deleted files are excluded from scanning and locations pruned; cleanup of exposed secret files uses reversible 30-day quarantine, never permanent deletion.
- **K3 (Local only):** Pure-Rust pattern, checksum, and entropy detectors with zero external APIs.
- **K4 (Opt-in observation):** Scans only indexed non-backup online local files (`<= 2 MiB`), excludes cloud placeholders and dev caches/repos from auto-quarantine, and audits `knowledge.audio_opt_in` + permanently-disabled screen/keystroke/browser capture.
- **K5 (Background budget):** Pure-Rust deterministic scanning on demand without extra resident weights.
- **R1 (No direct MFT parsing / elevation):** Read-only PowerShell BitLocker probe degrades cleanly without admin elevation.
- **R2 (Key handling):** Surfaces SQLCipher + DPAPI key sealing + 24-word offline recovery readiness inside Privacy Posture.
- **R7 (Protected, not masked from owner):** Confidential/secret files stay searchable in the owner's index (`files`/`chunks_fts`), while snippets are masked (`SENSITIVE_MASK`) and files skipped from unlocked governance scans until Windows CredUI session unlock (`include_sensitive && reveal_sensitive`).
- **R8 (No multi-pass shredding):** Recommends BitLocker whole-disk encryption and normal purge after reversible 30-day quarantine rather than SSD-ineffective wiping.
- **R9 (Code-enforced zero network):** Never makes "mathematical" zero-network claims; relies on zero networking crates in `aegis-core`/`aegis-embed`/`aegis-llm` verified by `r9_forbids_networking_crates_in_engine_and_models` plus an optional owner Windows Firewall rule.
- **R11 (No screen/input capture):** Privacy Posture explicitly confirms zero screen, keystroke, or active-window monitoring.
- **D1 (Encrypted at rest):** All governance findings, redacted locations, reversible label batches, scan watermarks, and privacy rules reside inside SQLCipher-encrypted `aegis.db` via migration `0017_personal_data_governance.sql`.

## Architecture & File Map

1. **Schema & Volume Rebase (`D1`)**
   - `crates/aegis-core/migrations/0017_personal_data_governance.sql`
   - `crates/aegis-core/src/db/mod.rs` (migration `17`), `crates/aegis-core/src/drives/registry.rs` (`governance_privacy_rules.path_prefix` rebase), `crates/aegis-core/tests/drives.rs`
2. **Core Governance Engine & AI Exclusion**
   - `crates/aegis-core/src/governance/{mod.rs,detector.rs,posture.rs,engine.rs}`
   - `crates/aegis-core/src/rag/mod.rs` (`exclude_from_ai` enforcement in `retrieve_rag_context`)
   - `crates/aegis-core/src/lib.rs`, `crates/aegis-core/src/engine/mod.rs` (`Screen::Governance`), `crates/aegis-core/src/contracts/mod.rs`
3. **Desktop IPC & UI**
   - `src-tauri/src/commands/governance.rs`, `src-tauri/src/commands/mod.rs`, `src-tauri/src/lib.rs`
   - `src/bindings/*`, `src/screens/fixtures/governance.ts`, `src/screens/DataGovernance.tsx`, `src/App.tsx`
4. **Verification & Docs**
   - `crates/aegis-core/tests/personal_data_governance.rs` (7 integration tests) + unit tests in `detector::tests` & `posture::tests`.
   - Full `clippy`, `cargo test --workspace`, `cargo check -p aegis-desktop --features custom-protocol`, `pnpm build`, quarantine `0` verification, and `--ff-only` merge into `master`.

## Verification Checklist (Observed 2026-11-25)

- `cargo clippy --workspace --all-targets -- -D warnings`: clean (`0` warnings, `34.06s`).
- `cargo test --workspace`: **`717` passed, `0` failed, `2` ignored** (`usn_refresh_on_system_temp`, `live_llama32_grounded_rag_answer`), including all **`7`** tests in `crates/aegis-core/tests/personal_data_governance.rs` (`0.22s`), **`247`** in `aegis-core --lib` (`1.90s`), **`6`** in `crates/aegis-core/tests/d1_encryption.rs` (`0.81s`), **`19`** in `crates/aegis-core/tests/drives.rs` (`1.54s`), **`11`** in `aegis-desktop`, **`5`** in `aegis-embed` (`9.09s`), and **`4`** in `aegis-llm` (`0.06s`).
- `cargo check -p aegis-desktop --features custom-protocol`: clean (`33.60s`).
- `pnpm build` (`tsc --noEmit && vite build`): clean (`2395` modules transformed, built in `911ms`).
- `C:\.aegis-quarantine` folder count after verification: **`0`**.
