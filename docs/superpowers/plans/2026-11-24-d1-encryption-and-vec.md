# D1 Encryption, Key Sealing (R2), Offline Recovery, Sensitive Views (R7), and sqlite-vec Foundation

**Status:** Done — built, verified, and fast-forward merged into `master` on 2026-11-24 (not pushed).

**Goal:** Land Owner Decision D1 and the `sqlite-vec` foundation before document embeddings and hybrid search:
- Encrypt the entire Aegis SQLite catalog (`aegis.db`) at rest with SQLCipher AES-256 (`sqlcipher-src` 4.20.0 + `libtomcrypt` 1.18.2, zero Perl/OpenSSL build dependency on Windows MSVC).
- Register `sqlite-vec` (`=0.1.9`; `0.1.10-alpha.4` fails MSVC compilation due to missing `sqlite-vec-diskann.c` in its crate tarball) on every `SqliteStore` connection and add migration `0009_vault_and_vec_meta.sql` so future vector virtual tables live inside the same encrypted store.
- Automatically generate and DPAPI-seal the 256-bit database key (`aegis.key.dpapi`) so unattended tray/watcher/backup work survives reboots without login (`R2`), while seamlessly migrating an existing plaintext `aegis.db` on first startup via verified `sqlcipher_export`.
- Require offline backup of a 24-word BIP-39 (or 64-hex) recovery key (`aegis.vault.json`), plus a recovery unlock screen when `aegis.key.dpapi` is absent on another PC.
- Gate sensitive file previews (`confidential`/`secret`) in keyword Search and `FileDetail`, plus recovery key reveal, behind a timed Windows Hello / Windows Security session (`R2` + `R7`).

## North-star rules

This plan upholds the binding rules in `docs/superpowers/specs/2026-11-23-local-intelligence-north-star.md`:
- **K1 (propose, approve, undo):** File moves, quarantines, and undo history stay inside the encrypted single database with zero changes to batch safety.
- **K2 (deletes ask, 30-day quarantine):** Plaintext migration swaps the DB only after row counts and `PRAGMA quick_check` verify, restoring on failure; recovery keys never auto-delete or lock data on failed attempts.
- **K3 (local only) & R9 (no network crates):** Purely local crypto (SQLCipher/LibTomCrypt, Windows BCrypt RNG, DPAPI, CredUI, offline BIP-39); guarded by an automated test walking `Cargo.lock` for `aegis-core` and `aegis-embed`.
- **K5 (background budget):** Keeps SQLite `cache_size = -16384` (16 MB) and `sqlite-vec` inside the same process rather than launching an out-of-process vector server.
- **R2 (key handling):** Engine key is DPAPI-sealed to the PC/Windows user for unattended background operation; Windows Security (`CredUIPromptForWindowsCredentialsW`) gates sensitive views and recovery reveal.
- **R7 (personal data protected, not masked from search):** Search still indexes and finds sensitive files, while hiding snippets/previews and blocking preview leaks until unlocked.
- **R8 (no multi-pass shredding):** Normal deletion removes the verified temporary plaintext migration backup.
- **D1 (approved):** SQLCipher whole-store encryption + `sqlite-vec` + DPAPI sealing + mandatory offline 24-word/hex recovery key.

## Verification

- `cargo clippy --workspace --all-targets -- -D warnings`: clean (`0` warnings).
- `cargo test --workspace`: **511 passed, 0 failed, 1 ignored** (`usn_refresh_on_system_temp`), including **6/6** in `crates/aegis-core/tests/d1_encryption.rs` (`0.73s`), **19/19** (1 ignored) in `crates/aegis-core/tests/drives.rs`, and **7/7** in `crates/aegis-core/tests/core_invariants.rs` (`0.19s`).
- `cargo check -p aegis-desktop --features custom-protocol`: clean.
- `pnpm build` (`tsc --noEmit && vite build`): clean (`dist/` built in `932ms`).
- `C:\.aegis-quarantine`: `0` items before tests and `0` items after.
