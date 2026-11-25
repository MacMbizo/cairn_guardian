# Capability #15: Digital Estate Management (`5.3`, reshaped `R10`)

Branch: `feature/digital-estate` (from `master` at `0b971e5`)  
North-Star reference: `docs/superpowers/specs/2026-11-23-local-intelligence-north-star.md` §2.2 #15 (`5.3`)

## Architecture & North-Star Rules

- **R10 (No destructive dead-man's switch — hard invariant):** Missed check-ins transition heartbeat state (`Active` -> `Warning` -> `GracePeriod` -> `ReadyForRelease`) strictly to unlock/prepare non-destructive trustee instructions and offline recovery guides. Never deletes, shreds, quarantines, or locks owner files or `aegis.db`.
- **R8 (No multi-pass shredding):** Multi-pass shredding stays disabled; relies on `D1` SQLCipher + BitLocker at rest.
- **R2 & R7 (Windows CredUI sensitive gating):** High-level readiness overview works normally; viewing/editing succession packet bodies (`body_markdown`), generating $K$-of-$N$ Shamir key shards from live `DbKey`, verifying quorum against the live key, and exporting an offline Continuity Kit require Windows CredUI unlock (`reveal_sensitive = true`, returning `PermissionDenied` when locked). Raw shard secrets are **never** stored in `aegis.db`.
- **D1 (Encrypted storage):** Migration `0019_digital_estate.sql` stores trustees, shard metadata, sealed succession packets, and check-in history inside SQLCipher-encrypted `aegis.db`.
- **K1–K4 & R9 (Local, pure-Rust, backup-aware):** GF(256) Shamir's Secret Sharing uses existing OS CSPRNG (`db::crypto::fill_random_bytes`), `blake3`, and `zeroize` with zero new Cargo crates and zero networking dependencies. Continuity Kit exports only to an explicit owner-chosen folder and never deletes existing files.

## Scope

1. **Migration 19 & Crypto Helper:** `0019_digital_estate.sql` (`estate_trustees`, `estate_shard_records`, `estate_packets`, `estate_checkins`), registered in `db/mod.rs` and `tests/drives.rs`; `pub fn fill_random_bytes` in `db/crypto.rs`.
2. **Core Estate Module (`crates/aegis-core/src/estate/`):**
   - `shamir.rs`: Pure-Rust GF(256) Shamir secret splitting (`AEGIS-SHARD-V1`) & Lagrange recovery with `blake3` checksum validation and zeroized buffers.
   - `mod.rs`: 16 TS-exported DTOs.
   - `engine.rs`: Readiness dashboard, trustee & packet CRUD with `R7` masking, $K$-of-$N$ shard set metadata lifecycle & offline quorum verification, non-destructive heartbeat (`R10`), and Continuity Kit export.
3. **Contracts & Bindings:** `Screen::Estate`, export all 16 DTOs in `contracts/mod.rs`, regenerate `src/bindings/`, add `aegis.estate` in `src/bindings/client.ts`.
4. **Desktop IPC & UI:** `src-tauri/src/commands/estate.rs` (12 commands), `src/screens/DigitalEstate.tsx`, `src/screens/fixtures/estate.ts`, wired into `src/App.tsx`.
5. **Tests & Gate:** `crates/aegis-core/tests/digital_estate.rs` (7 integration tests), full `clippy`, `cargo test --workspace`, `cargo check -p aegis-desktop --features custom-protocol`, `pnpm build`, quarantine `0` verification, and `--ff-only` merge into `master`.

## Verification Checklist (Observed 2026-11-25)

- `cargo clippy --workspace --all-targets -- -D warnings`: clean (`0` warnings, `39.16s`).
- `cargo test --workspace`: **`768` passed, `0` failed, `2` ignored** (`usn_refresh_on_system_temp`, `live_llama32_grounded_rag_answer`), including all **`7`** tests in `crates/aegis-core/tests/digital_estate.rs` (`0.15s`), **`284`** in `aegis-core --lib` (`1.59s`), **`7`** in `crates/aegis-core/tests/cdo_agent.rs` (`0.23s`), **`7`** in `crates/aegis-core/tests/personal_data_governance.rs` (`0.21s`), **`6`** in `crates/aegis-core/tests/d1_encryption.rs` (`1.33s`), **`19`** in `crates/aegis-core/tests/drives.rs` (`1.80s`), **`11`** in `aegis-desktop` (`0.00s`), **`5`** in `aegis-embed` (`9.17s`), and **`4`** in `aegis-llm` (`0.04s`).
- `cargo check -p aegis-desktop --features custom-protocol`: clean (`44.69s`).
- `pnpm build` (`tsc --noEmit && vite build`): clean (`2430` modules transformed, built in `899ms`).
- `C:\.aegis-quarantine` folder count after verification: **`0`**.
