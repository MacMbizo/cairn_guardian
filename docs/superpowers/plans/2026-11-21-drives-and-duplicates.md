# Drives & Duplicates Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Aegis should:

- recognise every external drive;
- keep its catalog while the drive is unplugged;
- update it in seconds on re-plug;
- find exact copies across all drives, decided per folder;
- maintain one Backup drive as a plain-file mirror with a 90-day safety window.

**Architecture:** Four new aegis-core modules sit on top of the existing store, scanner, action-batch and quarantine code:

- `drives`: identity, passport, online lifecycle, USN;
- `fingerprint`: size → quick → full hashing;
- `dupes`: folder grouping, safety, decisions;
- `backup`: sources, top-up, guard, priority.

A `VolumeProvider` trait is the seam between the OS and the logic. Tests use a fake provider that maps temp directories to separate "drives". Everything is exposed to the React app through new async Tauri commands and `AegisEvent` variants.

**Tech Stack:** Rust 2021 workspace, rusqlite (WAL, FTS5), blake3, windows-sys (already used in `os_adapter.rs`), ts-rs bindings, Tauri 2 async commands, React 19 + TypeScript + Tailwind 4.

**Spec:** `docs/superpowers/specs/2026-11-21-drives-and-duplicates-design.md`. Read it first; section numbers below (§7.x) refer to it.

## Global Constraints

- **Disk mutations** follow the existing pattern: the disk op runs outside any transaction, then one short `SqliteStore::with_full_sync_tx`, then `FsOutcome::undo()` if the commit fails. Everything is audited via `append_audit_in_tx`.
- **Passport** file: `<root>\.aegis\volume.json`, attributes hidden+system. JSON keys are `passport_id` (UUID v4), `created_at` (unix ms), `created_by` (`"aegis <CARGO_PKG_VERSION>"`) and `schema` (=1).
- **Identity match order:** passport_id → volume GUID path → serial + total_bytes → new drive.
- **Hashing:**
  - Quick hash = BLAKE3 over (size as u64 LE ‖ first 64 KiB ‖ last 64 KiB).
  - Files ≤ 128 KiB: the quick hash is the full BLAKE3 of the file, and it is also written to `files.blake3`.
- **Backup layout:**
  - live copies: `<backup root>\Aegis Backup\<source label>\<relative path>`;
  - previous versions: `<backup root>\Aegis Backup\_Previous versions\<yyyy-mm-dd>\<source label>\<relative path>`.
- **Safety window:** 90 days, `PREVIOUS_VERSION_TTL_MS = 90 * 86_400_000`.
- **Mass-change guard:** halts the top-up for a folder when more than 20% of its backed-up files would be overwritten or deleted, or when encrypted-looking changes exceed `ENTROPY_SUSPECT_FRACTION` (builder sets the value; a test pins it).
- **Minimum copies:** `MIN_COPIES = 2`. A copy is a file row with `deleted_at IS NULL`, `state NOT IN ('quarantine','missing')`, a matching full `blake3`, not under `_Previous versions`, on any volume, online or offline.
- **Survivor rule:** a dupe quarantine executes only if a survivor exists on an **online** volume and its fresh full BLAKE3 matches. Otherwise the action fails with the copy `"The other copy is on {label} (last checked {date}). Plug it in to confirm, or keep both."`
- **Standing approval:** scope `backup_topup`, bound to the Backup volume's `passport_id`. It is revoked (and audited) on re-passport, clone detection or a role change. It permits only copies onto the Backup drive and moves or prunes inside `Aegis Backup\`, never anything at a source.
- **Catalog exclusions:** external drives are catalogued from the root, excluding `.aegis`, `.aegis-quarantine`, `System Volume Information`, `$RECYCLE.BIN`, `Recovery` and `Aegis Backup\_Previous versions`.
- **PC starting roots,** seeded on first run if no roots exist: Downloads, Documents, Desktop, Pictures, Music (known-folder paths).
- **Platform:** Windows-only code behind `#[cfg(windows)]`. `#[cfg(unix)]` stubs return `AegisError::Unsupported` (add the variant if absent) and must compile.
- **UI copy:**
  - The words Backup drive / General / Unassigned, as written in the spec.
  - The plug-in banner format is from §7.4.
  - Lists show "only copy among the places Aegis knows about".
- **License:** AGPL-3.0-or-later (already set). Never commit `canonical/Aegis Guardian Full PRD.md`.

## Review Focus

1. **A drive letter is reused by a different drive** while the old drive's row still says `online`. `sync_mounted` must mark the old drive offline before the new drive claims the letter, and scans must never attribute files to the wrong volume. Test lives in Task 3.
2. **A drive is unplugged in the middle of a top-up copy.** No partial file may be left under the live path, and the job must resume on re-plug. Test lives in Task 10.
3. **A file is changed on the source between planning and executing a top-up.** The copy must verify against the hash taken at copy time, not the plan-time hash, and must record that hash in `backup_copies`. Test lives in Task 10.
4. **The last copy is quarantined via a stale catalog.** Suppose the survivor's row exists but the file was deleted outside Aegis. Survivor re-hash must fail and the action must fail without moving anything. Test lives in Task 7.
5. **OneDrive placeholders and hardlinks in a catalog.**
   - Placeholders must never be read by `fingerprint`.
   - Hardlinked pairs (same `file_index`) must not count as two copies for `MIN_COPIES`.

   Test lives in Task 5 (placeholders) and Task 7 (hardlinks).

---

## File structure

```
crates/aegis-core/
  migrations/0002_drives_backup.sql          (new)  schema of spec §8
  src/drives/mod.rs                          (new)  pub use; sync_mounted, refresh_volume
  src/drives/provider.rs                     (new)  VolumeProvider trait, MountedVolume, WindowsProvider, FakeVolumeProvider
  src/drives/passport.rs                     (new)  read/write/regenerate passport
  src/drives/registry.rs                     (new)  identity matching, clone detection, offline marking, path rebase
  src/drives/usn.rs                          (new)  NTFS USN journal reader (cfg(windows))
  src/fingerprint/mod.rs                     (new)  size grouping → quick hash → full hash
  src/dupes/mod.rs                           (new)  grouped report, keeper rules
  src/dupes/safety.rs                        (new)  copy counting, survivor verification
  src/dupes/decisions.rs                     (new)  dupe_decisions, decide/undo
  src/backup/mod.rs                          (new)  sources, layout helpers, status
  src/backup/topup.rs                        (new)  plan + execute top-up, previous versions, prune
  src/backup/guard.rs                        (new)  mass-change + entropy guard
  src/backup/priority.rs                     (new)  tiering + coverage summary
  src/backup/plan.rs                         (new)  drive-plan recommendation + apply
  src/db/mod.rs                              (mod)  migration runner v2; ensure_volume_for_path prefers registered volumes
  src/db/actions.rs                          (mod)  survivor check in perform_execute_fs; keeper rule in exact report
  src/contracts/mod.rs                       (mod)  new DTOs + events, exported in write_ts_bindings
  src/policy/mod.rs                          (mod)  standing approvals
  src/scanner/mod.rs                         (mod)  exclusions; Tier-1 catalog mode
  src/lib.rs                                 (mod)  pub mod drives, fingerprint, dupes, backup
  tests/common/mod.rs                        (new)  fake-drive helpers
  tests/drives.rs, tests/dupes.rs, tests/backup.rs   (new)
src-tauri/src/commands/drives.rs             (new)  Tauri commands for this project
src-tauri/src/watch.rs                       (new)  plug-in poll thread
src-tauri/src/lib.rs, commands/mod.rs        (mod)  register
src/bindings/client.ts                       (mod)  aegis.drives / dupes / backup
src/screens/Drives.tsx, src/screens/DupeReview.tsx, src/components/PlugBanner.tsx, src/components/HomeTiles.tsx  (new)
src/App.tsx                                  (mod)  wire Home tiles, Drives tab, banner
canonical/C4_Policy_Engine_Confidence_Spec.md  (mod)  standing-approval amendment
```

---

### Task 1: Migration 0002, migration runner, contract types

**Files:**
- Create: `crates/aegis-core/migrations/0002_drives_backup.sql`
- Modify: `crates/aegis-core/src/db/mod.rs:14,51-79,177-205` (runner, `list_volumes`)
- Modify: `crates/aegis-core/src/contracts/mod.rs:250-266` (VolumeInfo) and `write_ts_bindings`
- Test: `crates/aegis-core/tests/drives.rs`

**Interfaces:**
- Produces:
  - **New `volumes` columns:** `passport_id TEXT UNIQUE`, `guid_path TEXT`, `label TEXT`, `serial TEXT`, `total_bytes INTEGER`, `role TEXT NOT NULL DEFAULT 'unassigned' CHECK(role IN ('unassigned','general','backup'))`, `usn_journal_id TEXT`, `usn_next INTEGER`, `last_catalog_complete_at INTEGER`.
  - **New `files` column:** `quick_hash BLOB`, with index `idx_files_size_live ON files(size) WHERE deleted_at IS NULL`.
  - **New `actions` column:** `survivor_file_id TEXT REFERENCES files(id)`.
  - **New tables:** `dupe_decisions(id, a_passport, a_rel_path, b_passport, b_rel_path, decision CHECK IN ('backup','keep_both','delete'), decided_at, undone_at)`, `backup_sources(id, volume_id, rel_path, tier INTEGER, pinned INTEGER, added_at, removed_at)`, `backup_copies(id, source_file_id, backup_volume_id, backup_rel_path, source_blake3 BLOB, copied_at, last_verified_at)`, `backup_versions(id, backup_volume_id, rel_path, original_rel_path, reason CHECK IN ('changed','deleted','uncovered'), moved_at, prune_after)`, `standing_approvals(id, scope, volume_passport, created_at, revoked_at, revoke_reason)`, `backup_jobs(id, backup_volume_id, status, planned_json, done_count, total_count, started_at, finished_at, halted_reason)`.
  - **`VolumeInfo` gains** `label: Option<String>`, `role: String`, `passport_id: Option<String>`, `last_catalog_complete_at: Option<i64>`.
  - **New enum `DriveRole { Unassigned, General, Backup }`** with `as_str`/`from_db` like `ActionType`.

- [ ] **Step 1: Write the failing test** `migration_0002_applies_on_fresh_and_v1_databases`. It covers both a fresh db and a db already at v1:
  - Open `SqliteStore::open_in_memory()`. Assert `schema_migrations` contains versions 1 and 2.
  - Assert `PRAGMA table_info(volumes)` includes `passport_id` and `role`.
  - Create a db file with only migration 0001 applied (use `rusqlite` directly with `include_str!` of 0001). Then `SqliteStore::open` it and assert version 2 is applied and existing rows keep `role='unassigned'`.
- [ ] **Step 2: Run** `cargo test -p aegis-core --test drives migration_0002` and expect FAIL (no column `passport_id`).
- [ ] **Step 3: Implement.** Write the SQL file, and generalise `run_migrations` to a `const MIGRATIONS: &[(i64, &str, &str)]` list applied in order, each in its own transaction. Extend `VolumeInfo` plus the `list_volumes` / `ensure_volume_for_path` selects. Add `DriveRole` and export it in `write_ts_bindings`.
- [ ] **Step 4: Run** `cargo test -p aegis-core` and expect all PASS (the existing 65 plus the new one).
- [ ] **Step 5: Commit** `feat(db): migration 0002 for drives, dupes and backup`.

### Task 2: VolumeProvider seam and passport

**Files:**
- Create: `src/drives/mod.rs`, `src/drives/provider.rs`, `src/drives/passport.rs`; modify `src/lib.rs`
- Test: `crates/aegis-core/tests/drives.rs`, `crates/aegis-core/tests/common/mod.rs`

**Interfaces:**
- **Produces:**
  - **`MountedVolume`:**
    ```rust
    pub struct MountedVolume { pub mount_point: PathBuf, pub guid_path: Option<String>, pub serial: Option<String>,
      pub label: Option<String>, pub fs_type: String, pub total_bytes: u64, pub free_bytes: u64,
      pub is_removable: bool, pub is_readonly: bool, pub is_system: bool }
    ```
  - **`VolumeProvider`:**
    ```rust
    pub trait VolumeProvider: Send + Sync { fn mounted(&self) -> Result<Vec<MountedVolume>>; }
    ```
  - **`WindowsProvider`:** `#[cfg(windows)] pub struct WindowsProvider;`. It uses `GetLogicalDrives` + `GetVolumeInformationW` + `GetVolumeNameForVolumeMountPointW` + `GetDiskFreeSpaceExW`, and sets `is_system` when the mount equals `%SystemDrive%\`.
  - **`FakeVolumeProvider`:** `pub struct FakeVolumeProvider { inner: Mutex<Vec<MountedVolume>> }` with `new()`, `plug(v: MountedVolume)`, `unplug(mount: &Path)` and `set(vs)`. It is always compiled, not test-only, so the Tauri tests can use it too.
  - **`Passport`:** `pub struct Passport { pub passport_id: String, pub created_at: i64, pub created_by: String, pub schema: u32 }`, plus three functions:
    - `pub fn read_passport(root: &Path) -> Result<Option<Passport>>`
    - `pub fn write_passport(root: &Path) -> Result<Passport>`: creates `.aegis\`, writes atomically (temp + rename) and sets hidden+system on Windows.
    - `pub fn regenerate_passport(root: &Path) -> Result<Passport>`
- **Test helper (`tests/common/mod.rs`):** `pub fn fake_drive(tmp: &TempDir, name: &str, serial: &str) -> MountedVolume`. It creates `tmp/name/` as the mount point, sets `label = name`, `is_removable = true` and `total_bytes = 1_000_000_000_000`.

- [ ] **Step 1: Write failing tests:**
  - `passport_roundtrip_and_regenerate`: write, then read, and assert equal. Regenerate and assert a different `passport_id`. Malformed JSON → `read_passport` returns `Ok(None)` and doesn't crash.
  - `fake_provider_plug_unplug`.
  - `#[cfg(windows)] windows_provider_lists_system_drive`: `mounted()` contains a volume with `is_system == true` and a non-empty `fs_type`.
- [ ] **Step 2: Run** `cargo test -p aegis-core --test drives passport` and expect FAIL to compile.
- [ ] **Step 3: Implement** as specified.
- [ ] **Step 4: Run** `cargo test -p aegis-core --test drives` and expect PASS.
- [ ] **Step 5: Commit** `feat(drives): volume provider seam and drive passport`.

### Task 3: Drive registry: identity, clones, offline, re-letter

**Files:**
- Create: `src/drives/registry.rs`; modify `src/drives/mod.rs`, `src/db/mod.rs:128-175`
- Test: `tests/drives.rs`

**Interfaces:**
- **Consumes:** Task 2 (`VolumeProvider`, `read_passport` / `write_passport` / `regenerate_passport`) and Task 1 columns.
- **Produces:**
  - **`sync_mounted`:**
    ```rust
    pub struct SyncReport { pub arrived: Vec<String>, pub departed: Vec<String>, pub new_drives: Vec<String>,
      pub relettered: Vec<(String, String, String)> /* id, old mount, new mount */, pub clones: Vec<CloneConflict> }
    pub struct CloneConflict { pub kept_volume_id: String, pub new_volume_id: String, pub mount_point: String }
    pub fn sync_mounted(store: &mut SqliteStore, provider: &dyn VolumeProvider) -> Result<SyncReport>
    ```
  - **Volume ids:** a passported drive is `"pp-" + passport_id`; a read-only drive with no passport is `"fp-" + hex(blake3(guid|serial|total_bytes))[..16]`; the system drive keeps the existing `win-{serial}` id.
  - **`rebase_volume_paths`:** `pub fn rebase_volume_paths(tx: &Connection, volume_id: &str, old_mount: &str, new_mount: &str, case_sensitive: bool) -> Result<usize>`. It rewrites `files.path` and `path_hash`, `cache_dirs.path`/`path_hash`, `quarantine_entries.original_path`/`quarantine_path` and `volumes.quarantine_dir`.
  - **`ensure_volume_for_path`:** checks registered online volumes first (longest mount prefix via `resolve_online_volume`). It only inspects the OS and inserts when none matches.
  - **`AegisError::Unsupported(String)`**, if not present.

**Behaviour, in order inside `sync_mounted`:**

1. Mark every DB volume `offline` (stamping `last_seen`) whose mount_point is not in `mounted()`, or whose mount is mounted but now holds a different passport. Then update `scan_jobs` that are still running on that volume to `status='interrupted'`.
2. For each mounted, non-system volume: read the passport, writing one if absent and the volume is writable. Then match ids in the global order.
3. If two mounted volumes share a passport, the one whose mount matches the DB mount keeps the id. The other gets `regenerate_passport` and becomes a new drive, with the clone recorded. Revoke standing approvals for both passports (Task 11 provides `revoke_standing_for_passport`; until then this is a no-op stub in `policy`).
4. Matched volumes become `online`. If the mount changed, call `rebase_volume_paths` in the same transaction.

- [ ] **Step 1: Write failing tests,** using `FakeVolumeProvider` with temp directories:
  - `new_drive_gets_passport_and_pp_id`
  - `unplug_marks_offline_without_touching_file_rows`: scan a fake drive with `scan_root(&mount, &[1])`, unplug, sync. Expect status `offline`, files count unchanged and `deleted_at` NULL.
  - `replug_under_new_letter_rebases_paths`: move the temp directory to a new name, plug, sync. Expect the same volume id, every `files.path` starting with the new mount, and `normalize_path(new)` hash equal to the stored `path_hash`.
  - `letter_reused_by_other_drive_marks_old_offline` (Review Focus 1): drive A is at mount M; unplug A and plug drive B at the same M. Expect A offline, B new, and A's files still on A's volume id. A subsequent scan of M attributes files to B.
  - `cloned_drive_gets_new_passport`: copy A's `.aegis\volume.json` into drive C and plug both. Expect `clones.len()==1`, C has a different passport on disk and a new id, and A is unchanged.
  - `readonly_drive_uses_fingerprint_id`: `is_readonly=true`. Expect no `.aegis` dir created and an id starting with `fp-`.
- [ ] **Step 2: Run** `cargo test -p aegis-core --test drives` and expect FAIL.
- [ ] **Step 3: Implement** `registry.rs` and the `ensure_volume_for_path` change.
- [ ] **Step 4: Run** `cargo test -p aegis-core` (whole crate, because `ensure_volume_for_path` changed) and expect all PASS.
- [ ] **Step 5: Commit** `feat(drives): drive identity, clone detection, offline lifecycle`.

### Task 4: Catalog mode and exclusions in the scanner

**Files:**
- Modify: `src/scanner/mod.rs:57-59` (exclusions), `:670-790` (catalog entry point)
- Test: `tests/drives.rs`

**Interfaces:**
- **Consumes:** Task 3 (volumes registered by `sync_mounted`).
- **Produces:**
  - `pub fn catalog_volume(store: &mut SqliteStore, volume_id: &str, progress: &mut dyn FnMut(&ScanProgress)) -> Result<ScanStatus>`. It runs a Tier-1 walk (metadata only) from the volume's mount, sets `volumes.last_catalog_complete_at` on success, and returns an error if the volume is offline.
  - `is_immutable_excluded_dir_name` also excludes `System Volume Information`, `$RECYCLE.BIN` and `Recovery`, case-insensitive, at volume root only for the last two. Add a path check to skip `Aegis Backup\_Previous versions`.
  - **`seed_default_roots(store: &SqliteStore) -> Result<Vec<RootConfig>>`.** If `list_roots()` is empty, add Downloads, Documents, Desktop, Pictures and Music that exist. On Windows they come from `SHGetKnownFolderPath`. Elsewhere use `dirs`-style `$HOME/<Name>` without adding a crate; `std::env::var("USERPROFILE"|"HOME")` is fine.

- [ ] **Step 1: Write failing tests:**
  - `catalog_reads_no_payload_and_skips_system_dirs`: a fake drive with `a.txt`, `System Volume Information\x`, `$RECYCLE.BIN\y` and `Aegis Backup\_Previous versions\2026-01-01\z`. After `catalog_volume`, only `a.txt` is in `files`, `blake3 IS NULL` and `last_catalog_complete_at` is set.
  - `catalog_offline_volume_errors`.
  - `seed_default_roots_only_when_empty`.
- [ ] **Step 2: Run** and expect FAIL.
- [ ] **Step 3: Implement.**
- [ ] **Step 4: Run** `cargo test -p aegis-core` and expect PASS.
- [ ] **Step 5: Commit** `feat(scanner): tier-1 drive catalog and system-dir exclusions`.

### Task 5: Fingerprint pipeline

**Files:**
- Create: `src/fingerprint/mod.rs`; modify `src/lib.rs`
- Test: `tests/dupes.rs`

**Interfaces:**
- **Consumes:** `files.quick_hash` (Task 1) and online volumes.
- **Produces:**
  - **`run_fingerprint`:**
    ```rust
    pub struct FingerprintStats { pub size_candidates: i64, pub quick_hashed: i64, pub full_hashed: i64, pub skipped_offline: i64, pub skipped_placeholder: i64 }
    pub fn run_fingerprint(store: &mut SqliteStore, cancel: &AtomicBool) -> Result<FingerprintStats>
    pub fn quick_hash_file(path: &Path, size: u64) -> Result<(Vec<u8>, bool /* is_full */)>
    ```
  - **Algorithm:**
    - **Candidates:** live, non-placeholder files of size > 0 whose size occurs ≥2 times across **all** volumes (online and offline).
    - **Quick hash:** files on online volumes get a quick hash if it is NULL. Files ≤ 128 KiB also get `blake3` set from the same read.
    - **Full hash:** files whose (size, quick_hash) collides with another row get a full BLAKE3 if `blake3` is NULL. The collision partner may be offline with a stored quick hash.
    - **Batching:** commit every 500 files. Check `cancel` between files and return partial stats on cancel.
    - **Offline:** files on an offline volume count in `skipped_offline` and are never opened.

- [ ] **Step 1: Write failing tests:**
  - `unique_sizes_are_never_read`: files of sizes 10, 11 and 12. Expect `quick_hashed == 0`.
  - `same_size_different_content_stops_at_quick_hash`: two 300 KiB files differing in the first byte. Expect `full_hashed == 0`.
  - `same_size_same_edges_different_middle_gets_full_hash`: two 300 KiB files with identical first and last 64 KiB and different middle. Expect both full hashed and different `blake3`.
  - `small_files_get_full_hash_from_quick_pass`: two identical 1 KiB files. Expect `blake3` set and equal, and `full_hashed == 0`.
  - `offline_partner_triggers_full_hash_of_online_file`: drive A has `x` (300 KiB), fingerprinted, then unplugged. Drive B gets an identical `x`. After run: B's file has `blake3`, A's file is not touched, and A's file counts in `skipped_offline` only if it lacked a hash.
  - `placeholders_are_never_opened` (Review Focus 5): set a row's `cloud_state='placeholder_remote'` and delete the payload. The run succeeds and that row has `quick_hash IS NULL`.
  - `cancel_returns_partial_stats`.
- [ ] **Step 2: Run** `cargo test -p aegis-core --test dupes fingerprint` and expect FAIL.
- [ ] **Step 3: Implement.**
- [ ] **Step 4: Run** and expect PASS.
- [ ] **Step 5: Commit** `feat(fingerprint): size→quick→full duplicate fingerprinting`.

### Task 6: Re-plug refresh (USN with walk fallback)

**Files:**
- Create: `src/drives/usn.rs`; modify `src/drives/mod.rs`
- Test: `tests/drives.rs`

**Interfaces:**
- **Consumes:** Tasks 3, 4 and 5.
- **Produces:**
  - **`refresh_volume`:**
    ```rust
    pub enum RefreshMode { Usn, Walk }
    pub struct RefreshReport { pub volume_id: String, pub mode: RefreshMode, pub created: i64, pub modified: i64,
      pub renamed: i64, pub deleted: i64, pub folders_changed: i64, pub new_duplicates: i64 }
    pub fn refresh_volume(store: &mut SqliteStore, volume_id: &str, cancel: &AtomicBool) -> Result<RefreshReport>
    ```
  - **`usn.rs` (`#[cfg(windows)]`):** `pub fn query_journal(mount: &Path) -> Result<Option<(String /*journal id*/, i64 /*next usn*/)>>` and `pub fn read_changes(mount: &Path, journal_id: &str, from_usn: i64) -> Result<UsnRead>`. `UsnRead` is either `Changes(Vec<UsnChange>)` or `Reset` (journal id mismatch or `from_usn` below the journal's first USN). Each `UsnChange { file_ref: Vec<u8>, parent_ref, name, reason_flags }` is resolved to a path with `OpenFileById` + `GetFinalPathNameByHandleW`. A file that cannot be opened is treated as deleted.
  - **Rules:**
    - **When it runs:** only if the volume is NTFS, `usn_journal_id` is stored and the journal ID matches. It applies changes through the existing `upsert_discovered_file` / `soft_delete_file_in_tx`, so renames keep `files.id`.
    - **Modified files:** NULL out `quick_hash` and `blake3`.
    - **Fallback:** `Walk` mode runs `catalog_volume` and `sweep_unseen`. The existing `unchanged_hash` keeps hashes for unchanged files.
    - **Finishing:** run `run_fingerprint`, then store the new journal id and next USN. `new_duplicates` is the number of dupe clusters gained, from a before/after count of the `dupes::grouped_report` cluster count. Until Task 7 lands, count blake3 groups.
    - **Catalog bootstrap:** `catalog_volume` (Task 4) also records `query_journal` on success.

- [ ] **Step 1: Write failing tests:**
  - `walk_refresh_detects_create_modify_delete_rename`: on a fake drive (exFAT behaviour), catalog, mutate on disk, refresh. Expect the counts. The renamed file keeps its `files.id` when `has_stable_inode`; when it doesn't (fake drives on NTFS temp dirs do have file ids), assert create+delete instead. Pick based on `volumes.has_stable_inode`.
  - `refresh_reuses_hashes_for_unchanged_files`: count reads via a modified-mtime guard. Assert `blake3` unchanged and `fingerprint` stats `quick_hashed == 0` for unchanged files.
  - `#[cfg(windows)] #[ignore = "needs an NTFS volume with journal; run manually"] usn_refresh_on_system_temp`: uses the real `%TEMP%` on NTFS, creates and renames a file, `read_changes` returns them.
  - `usn_reset_falls_back_to_walk`: store a bogus journal id and assert `mode == Walk`.
- [ ] **Step 2: Run** and expect FAIL.
- [ ] **Step 3: Implement.**
- [ ] **Step 4: Run** `cargo test -p aegis-core` and expect PASS. Then run the ignored USN test once with `cargo test -p aegis-core --test drives usn_refresh -- --ignored`, and record the result in the commit message body: pass or fail, honestly.
- [ ] **Step 5: Commit** `feat(drives): fast re-plug refresh via USN journal with walk fallback`.

### Task 7: Grouped duplicates, keeper rules, copy safety

**Files:**
- Create: `src/dupes/mod.rs`, `src/dupes/safety.rs`; modify `src/db/actions.rs:344-377` (`propose_dupe_quarantine`), `:548-614` (`perform_execute_fs`), `:1371-1441` (keeper order)
- Test: `tests/dupes.rs`

**Interfaces:**
- **Consumes:** Task 5 hashes and Task 3 volumes.
- **Produces (contract DTOs, ts-exported):**
  ```rust
  pub struct DupeLocation { pub file_id: Option<String>, pub volume_id: String, pub volume_label: Option<String>,
    pub online: bool, pub path: String, pub is_backup: bool, pub last_seen: i64 }
  pub enum DupeGroupKind { Folder, PartialFolder, Files }
  pub struct DupeGroup { pub group_id: String /* stable: blake3 of sorted (volume passport|rel path) */,
    pub kind: DupeGroupKind, pub size_bytes: i64, pub file_count: i64, pub locations: Vec<DupeLocation> /* ≥2 */,
    pub keeper_index: usize, pub differences: Vec<String> /* rel paths only in one side, PartialFolder only */,
    pub needs_question: bool }
  pub struct GroupedDupeReport { pub groups: Vec<DupeGroup>, pub reclaimable_bytes: i64 }
  pub fn grouped_report(conn: &Connection) -> Result<GroupedDupeReport>
  // safety.rs
  pub fn copy_count(conn: &Connection, blake3: &[u8]) -> Result<i64>      // hardlinks (same volume+file_index) count once
  pub fn verify_survivor(conn: &Connection, survivor_file_id: &str, expected: &[u8]) -> Result<()>  // AegisError::SurvivorUnavailable(copy)
  ```
- **Grouping rules:**
  - **Folder:** two directories (any volumes) whose sets of `(relative path, blake3)` are equal, at least 2 files, ignoring `Thumbs.db`/`desktop.ini`/`.DS_Store`. Only the topmost matching pair is reported, never its sub-folders.
  - **PartialFolder:** ≥90% of A's files (by count) are in B at the same relative paths.
  - **Files:** remaining hash groups, batched by (parent folder of each location) pair.
  - **`needs_question`:** false when any location is on the Backup drive under `Aegis Backup\` with a `backup_copies` row, or when a `keep_both` decision exists (Task 8).
- **Keeper order (§7.5):**
  1. an explicit override (not stored in this project; skip);
  2. not on the Backup drive;
  3. most siblings in the same folder group;
  4. oldest mtime;
  5. shortest path.

  `exact_dedupe_report` uses the same ordering.
- **Execute-time rule:** `propose_dupe_quarantine` sets `actions.survivor_file_id = keeper`. It refuses (`AegisError::PolicyBlocked`) any member whose `copy_count` minus already-proposed quarantines would fall below `MIN_COPIES`. `perform_execute_fs` calls `verify_survivor` before any quarantine whose `survivor_file_id` is set. The survivor's volume must be online, its file must exist and its fresh full BLAKE3 must equal the action's `pre_blake3`.

- [ ] **Step 1: Write failing tests:**
  - `identical_folders_on_two_drives_form_one_folder_group`: drive A `Photos\2019\{a,b,c}.jpg` and drive C `Old\Photos\2019\{a,b,c}.jpg`. Expect exactly 1 group, kind Folder, `file_count 3`, and no extra groups for the sub-folders.
  - `ninety_percent_overlap_is_partial_with_differences`: 10 vs 9 shared files plus 1 extra. Expect PartialFolder and `differences == ["extra.jpg"]`.
  - `keeper_prefers_non_backup_then_oldest`
  - `refuses_delete_below_min_copies`: exactly 2 copies on general drives, propose quarantine of one. Expect `Err(PolicyBlocked)`.
  - `hardlinks_count_once` (Review Focus 5): two hardlinked paths plus one other copy. `copy_count == 2`.
  - `survivor_offline_fails_action_without_moving` (spec S3): 3 copies, propose and approve quarantine of one, unplug the survivor's drive and sync, execute. Expect the action `failed`, `error_message` containing `"Plug it in to confirm"` and the file still in place.
  - `survivor_deleted_outside_aegis_fails` (Review Focus 4): delete the survivor from disk without rescanning, then execute. Expect failed and nothing moved.
  - `backup_copy_needs_no_question`
- [ ] **Step 2: Run** `cargo test -p aegis-core --test dupes` and expect FAIL.
- [ ] **Step 3: Implement.** Add `AegisError::SurvivorUnavailable(String)`, mapped in the IPC error conversion.
- [ ] **Step 4: Run** `cargo test -p aegis-core` and expect PASS, including the existing `dedupe_proposal` regression test (update its expectation only if the keeper rule legitimately changes its keeper; note it in the commit).
- [ ] **Step 5: Commit** `feat(dupes): folder-grouped duplicates with copy-count and survivor safety`.

### Task 8: Duplicate decisions

**Files:**
- Create: `src/dupes/decisions.rs`
- Test: `tests/dupes.rs`

**Interfaces:**
- **Consumes:**
  - Task 7: `DupeGroup` and `propose_dupe_quarantine`.
  - Task 9: `backup::plan::propose_move_to_backup`, for the `Backup` choice. Build Task 9 first, or stub it to return `Unsupported` and wire it in Task 9's final step.
- **Produces:**
  ```rust
  pub enum DupeChoice { ItsABackup, KeepBoth, DeleteThisCopy { location_index: usize }, MakeThisTheBackupCopy { location_index: usize } }
  pub struct DecisionOutcome { pub decision_id: String, pub batch_id: Option<String> }
  pub fn decide(store: &mut SqliteStore, group_id: &str, choice: DupeChoice) -> Result<DecisionOutcome>
  pub fn undo_decision(store: &mut SqliteStore, decision_id: &str) -> Result<()>   // sets undone_at; does not roll back batches (use rollback)
  pub fn list_decisions(conn: &Connection) -> Result<Vec<DupeDecisionItem>>
  ```
- **Semantics:**
  - **Keys.** Decisions are keyed by `(passport, rel_path)` of the two locations, ordered canonically, so re-scans and re-letters never re-ask.
  - **`KeepBoth`.** Records only; the group's `needs_question` becomes false.
  - **`DeleteThisCopy`.** Calls `propose_dupe_quarantine` with the keeper as survivor. If it is refused below `MIN_COPIES`, the error message offers "Make this the backup copy".
  - **`ItsABackup` and `MakeThisTheBackupCopy`.** Call `propose_move_to_backup` (Task 9), and fail with "No Backup drive assigned yet" when none exists.

- [ ] **Step 1: Write failing tests:**
  - `keep_both_suppresses_question_across_relettering`: decide KeepBoth, re-letter drive C (Task 3 helper), re-scan. The group has `needs_question == false`.
  - `delete_this_copy_creates_quarantine_batch`: 3 copies; expect a batch with 1 quarantine action and the survivor set.
  - `delete_refused_suggests_backup_copy`: 2 copies; the error text contains `"Make this the backup copy"`.
  - `undo_decision_reasks`
- [ ] **Step 2: Run** and expect FAIL.
- [ ] **Step 3: Implement.**
- [ ] **Step 4: Run** and expect PASS.
- [ ] **Step 5: Commit** `feat(dupes): remembered per-folder-pair duplicate decisions`.

### Task 9: Drive roles, drive plan, cross-drive move to backup

**Files:**
- Create: `src/backup/mod.rs`, `src/backup/plan.rs`; modify `src/lib.rs`
- Test: `tests/backup.rs`

**Interfaces:**
- **Consumes:** Tasks 3, 7 and 8.
- **Produces:**
  ```rust
  pub fn backup_root(mount: &Path) -> PathBuf                       // <mount>\Aegis Backup
  pub fn source_label(conn: &Connection, volume_id: &str, rel_root: &str) -> Result<String>  // "PC (Documents)" or drive label
  pub struct DrivePlanEntry { pub volume_id: String, pub label: Option<String>, pub proposed_role: DriveRole,
    pub used_bytes: i64, pub total_bytes: i64, pub backup_copy_share: f64, pub duplicate_bytes: i64 }
  pub struct DrivePlan { pub entries: Vec<DrivePlanEntry>, pub recommended_backup: Option<String>, pub ready: bool /* all known externals catalogued */ }
  pub fn recommend_plan(conn: &Connection) -> Result<DrivePlan>
  pub struct AppliedPlan { pub adopt_batch: Option<String>, pub stray_batch: Option<String>, pub only_copies_on_backup: i64 }
  pub fn apply_plan(store: &mut SqliteStore, roles: &[(String, DriveRole)]) -> Result<AppliedPlan>
  pub fn propose_move_to_backup(store: &mut SqliteStore, group_id: &str, location_index: usize) -> Result<String /* batch */>
  ```
- **Rules:**
  - **Recommendation.** Rank non-system externals by `0.6 * backup_copy_share + 0.4 * free_fraction`. `backup_copy_share` is the fraction of duplicate bytes (across all groups) that have a copy on that drive. Recommend only when `ready`.
  - **One Backup drive.** `apply_plan` rejects more than one Backup drive and calls `revoke_standing_for_passport` for any drive leaving the Backup role.
  - **Adopting.** For files on the Backup drive, outside `Aegis Backup\`, that have a copy elsewhere, `apply_plan` creates a **Move** batch within that drive to `Aegis Backup\<keeper's source label>\<keeper's rel path>`. On execute, each move inserts a `backup_copies` row and adds the keeper's top folder to `backup_sources` (tier 2, or tier 1 if it is an only-copy location).
  - **Only copies.** Files that exist only on the Backup drive are counted in `only_copies_on_backup`, never moved.
  - **Stray copies.** `propose_move_to_backup` and `stray_batch` create, per file, a **Copy** action to the backup path, plus a **Quarantine** action of the original whose `survivor_file_id` is the new backup copy. That survivor row is created at copy time. The batch requires normal approval and is never covered by standing approval.
  - **`Copy` execution.** Writes to `<dest>.aegis-partial`, fsyncs, verifies BLAKE3 against the source's fresh hash, renames, and records `post_blake3`. Extend `perform_execute_fs`'s `ActionType::Copy` arm if it doesn't already verify.

- [ ] **Step 1: Write failing tests:**
  - `plan_not_ready_until_all_catalogued`
  - `recommends_drive_holding_most_backup_copies`
  - `apply_plan_adopts_existing_copies_into_aegis_backup`: after approve and execute, the file is at `Aegis Backup\Seagate A\Photos\x.jpg`, with a `backup_copies` row and a `backup_sources` row.
  - `only_copy_on_backup_drive_is_flagged_not_moved`
  - `stray_backup_moves_via_copy_then_quarantine`: after execute, the backup has the file, the original is in quarantine and `copy_count` is unchanged.
  - `second_backup_role_rejected`
- [ ] **Step 2: Run** and expect FAIL.
- [ ] **Step 3: Implement,** then wire `decide()`'s backup choices to `propose_move_to_backup`.
- [ ] **Step 4: Run** `cargo test -p aegis-core` and expect PASS.
- [ ] **Step 5: Commit** `feat(backup): drive roles, drive plan, adopt and move to backup`.

### Task 10: Backup top-up, previous versions, prune

**Files:**
- Create: `src/backup/topup.rs`
- Test: `tests/backup.rs`

**Interfaces:**
- **Consumes:** Task 9 (layout, sources), Task 12 (`guard::check`; stub returns `Ok(GuardVerdict::Proceed)` until Task 12), Task 13 (priority; until then, all sources fit). This task creates `guard.rs` with the final `GuardHalt`/`GuardVerdict` types and `priority.rs` with the final `TierShortfall` type and stub functions, so the signatures don't change later.
- **Produces:**
  ```rust
  pub struct TopUpItem { pub kind: TopUpKind /* Add|Replace|RetireDeleted|RetireUncovered */, pub source_file_id: Option<String>,
    pub source_path: Option<String>, pub backup_rel_path: String, pub bytes: i64, pub tier: i64 }
  pub struct TopUpPlan { pub backup_volume_id: String, pub items: Vec<TopUpItem>, pub halted_folders: Vec<GuardHalt>, pub skipped_for_space: Vec<TierShortfall> }
  pub fn plan_topup(conn: &Connection, backup_volume_id: &str) -> Result<TopUpPlan>
  pub struct TopUpResult { pub job_id: String, pub added: i64, pub replaced: i64, pub retired: i64, pub bytes_copied: i64, pub completed: bool }
  pub fn run_topup(store: &mut SqliteStore, plan: &TopUpPlan, cancel: &AtomicBool, progress: &mut dyn FnMut(i64, i64)) -> Result<TopUpResult>
  pub fn prune_previous_versions(store: &mut SqliteStore, backup_volume_id: &str, now: i64) -> Result<i64 /* pruned */>
  pub fn list_previous_versions(conn: &Connection, backup_volume_id: &str) -> Result<Vec<PreviousVersionItem>>
  pub fn restore_previous_version(store: &mut SqliteStore, version_id: &str, to: Option<PathBuf>) -> Result<PathBuf>
  ```
- **Rules:**
  - **Add/Replace:**
    - Copy the source into `<dest>.aegis-partial`, hashing while copying. Verify the source has an unchanged size and mtime after the copy; if it changed, discard the copy and leave the item for the next run.
    - For Replace, move the old live copy to `_Previous versions\<today>\…` first, insert `backup_versions(reason='changed', prune_after = now + PREVIOUS_VERSION_TTL_MS)`, then rename the partial file into place.
    - Upsert `backup_copies` with the hash computed during the copy (Review Focus 3).
  - **RetireDeleted:**
    - Only when the source volume is online, the source row is soft-deleted, and the file is confirmed absent on disk.
    - The live copy moves into `_Previous versions` with reason `deleted`.
    - An offline source is never retired.
  - **RetireUncovered:** for removed `backup_sources`, with reason `uncovered`.
  - **Job state:** each item is its own disk op plus a short transaction. `backup_jobs` tracks progress.
  - **Startup cleanup:** leftover `*.aegis-partial` files under `Aegis Backup\` are deleted at the start of every `run_topup` (Review Focus 2).
  - **Prune:** deletes `backup_versions` past `prune_after`, from disk and DB. It is audited, without quarantine.

- [ ] **Step 1: Write failing tests:**
  - `topup_adds_new_files_as_plain_copies`: the file bytes are equal at `Aegis Backup\PC (Documents)\a.txt`.
  - `changed_source_moves_old_copy_to_previous_versions`: `_Previous versions\<yyyy-mm-dd>\PC (Documents)\a.txt` holds the old content.
  - `deleted_source_retires_only_when_source_online`: unplug the source drive and plan; expect no RetireDeleted. Plug it in with the file gone and rescan; expect RetireDeleted.
  - `interrupted_copy_leaves_no_partial_at_live_path_and_resumes` (Review Focus 2): set `cancel` from the progress callback after the first chunk of a 2 MiB file. Expect no file at the live path, and the next `run_topup` completes it and removes the stale partial.
  - `source_changed_during_copy_is_discarded` (Review Focus 3): modify the source mtime inside the progress callback. Expect no `backup_copies` row for it this run.
  - `prune_removes_only_expired_versions`: call `now = moved_at + 90 days + 1` against one older and one newer version.
  - `restore_previous_version_never_overwrites`: the target exists, so it restores to `"name (restored).ext"`, reusing `non_overwriting_dest` (make it `pub(crate)`).
- [ ] **Step 2: Run** and expect FAIL.
- [ ] **Step 3: Implement.**
- [ ] **Step 4: Run** `cargo test -p aegis-core --test backup` and expect PASS.
- [ ] **Step 5: Commit** `feat(backup): mirror top-up with 90-day previous versions`.

### Task 11: Standing approval

**Files:**
- Modify: `src/policy/mod.rs`, `src/backup/topup.rs`, `src/drives/registry.rs` (call revoke), `canonical/C4_Policy_Engine_Confidence_Spec.md`
- Test: `tests/backup.rs`

**Interfaces:**
- **Produces:**
  ```rust
  pub fn grant_standing_topup(store: &mut SqliteStore, backup_volume_id: &str) -> Result<String>   // requires a completed, user-approved first top-up job
  pub fn has_standing_topup(conn: &Connection, backup_volume_id: &str) -> Result<bool>          // matches the volume's CURRENT passport_id
  pub fn revoke_standing_for_passport(tx: &Connection, passport_id: &str, reason: &str) -> Result<usize>
  ```
  `run_topup` gains a parameter, `authority: TopUpAuthority { UserApproved, Standing }`. With `Standing`, it errors unless `has_standing_topup` is true. It also asserts every item writes only under `backup_root(backup mount)`, a defence-in-depth path check. Grant, revoke and each standing-run summary are written to `audit_log`.

  The C4 doc gets a short section, "Amendment 2026-11-21: Standing approval for backup top-ups (Drives & Duplicates spec §7.8.6)". It states:
  - the scope of the exception;
  - that it is bound to the passport;
  - the revocation triggers;
  - that deletes at a source are never covered.

- [ ] **Step 1: Write failing tests:**
  - `standing_requires_first_approved_topup`
  - `standing_revoked_on_clone_and_role_change`: grant, then create a clone (Task 3 scenario). `has_standing_topup` returns false. Re-grant after a fresh approved run; then call `apply_plan` with the drive as General, and it is false again.
  - `standing_run_refuses_paths_outside_backup_root`: craft a plan item whose path contains `..`. Expect `Err` and nothing written.
- [ ] **Step 2: Run** and expect FAIL.
- [ ] **Step 3: Implement** and write the C4 amendment.
- [ ] **Step 4: Run** `cargo test -p aegis-core` and expect PASS.
- [ ] **Step 5: Commit** `feat(policy): passport-bound standing approval for backup top-ups`.

### Task 12: Mass-change / ransomware guard

**Files:**
- Create: `src/backup/guard.rs`
- Test: `tests/backup.rs`

**Interfaces:**
- **Produces:**
  ```rust
  pub const MASS_CHANGE_FRACTION: f64 = 0.20;
  pub const ENTROPY_SUSPECT_FRACTION: f64 = /* builder sets; document reasoning in a comment */;
  pub struct GuardHalt { pub source_folder: String, pub changed: i64, pub total: i64, pub suspect_encrypted: i64 }
  pub enum GuardVerdict { Proceed, Halt(Vec<GuardHalt>) }
  pub fn check(conn: &Connection, plan: &TopUpPlan) -> Result<GuardVerdict>
  pub fn looks_encrypted(path: &Path, ext: Option<&str>) -> Result<bool>  // Shannon entropy of first 64 KiB > 7.5 bits/byte AND magic bytes don't match a known type for ext; known-compressed exts (zip, jpg, png, mp4, mp3, 7z, gz, docx, xlsx, pptx, pdf) only flagged when their magic bytes are wrong
  pub fn override_halt(store: &mut SqliteStore, backup_volume_id: &str, source_folder: &str) -> Result<()>  // one-shot, audited
  ```
  - **Grouping:** by `backup_sources` top folder.
  - **Measure:** `(Replace + RetireDeleted) / backed_up_files_in_folder`.
  - **Halt:** halted folders' items are removed from the plan and listed in `TopUpPlan.halted_folders`. The UI text is `"An unusual number of files changed in {folder} ({changed} of {total}). Back them up anyway?"`.

- [ ] **Step 1: Write failing tests:**
  - `twenty_one_percent_change_halts_folder_only`: two source folders, one with 21% replaced. Only that folder halts and the other proceeds.
  - `twenty_percent_exactly_proceeds`
  - `encrypted_looking_files_halt_below_mass_threshold`: overwrite 10% of files with random bytes keeping `.docx`, and assert a halt with `suspect_encrypted > 0`.
  - `real_jpgs_are_not_suspect`: use a small valid JPEG fixture, with JPEG magic and high entropy.
  - `override_allows_next_run_once`
- [ ] **Step 2: Run** and expect FAIL.
- [ ] **Step 3: Implement,** and call `check` from `plan_topup`.
- [ ] **Step 4: Run** and expect PASS.
- [ ] **Step 5: Commit** `feat(backup): mass-change and encryption guard`.

### Task 13: Priority fill and coverage summary

**Files:**
- Create: `src/backup/priority.rs`
- Test: `tests/backup.rs`

**Interfaces:**
- **Produces:**
  ```rust
  pub fn tier_for_file(conn: &Connection, file_id: &str) -> Result<Option<i64>> // 1..3, None = never back up
  pub fn add_source(store: &mut SqliteStore, volume_id: &str, rel_path: &str, pinned: bool) -> Result<String>
  pub fn remove_source(store: &mut SqliteStore, source_id: &str) -> Result<i64 /* bytes that will become 'uncovered' */>
  pub struct TierShortfall { pub tier: i64, pub bytes_needed: i64, pub bytes_skipped: i64 }
  pub struct BackupCoverage { pub backup_volume_id: String, pub capacity_bytes: i64, pub used_bytes: i64,
    pub tiers: Vec<TierCoverage> /* tier, label, protected_bytes, unprotected_bytes */, pub suggestion: Option<String> }
  pub fn coverage_summary(conn: &Connection) -> Result<Option<BackupCoverage>>
  ```
- **Tiers:**
  - **Tier 1:** pinned sources, plus only-copy files (`copy_count == 1`) on any volume, excluding tier "never".
  - **Tier 2:** files under the seeded PC Documents/Desktop/Pictures roots, and adopted sources.
  - **Tier 3:** music and video by ext/mime.
  - **Never:** installers (`.exe .msi .msix .dmg .pkg .iso` in Downloads), anything under a `cache_dirs` path, and OS junk.

  Labels: "Only copies", "PC Documents etc.", "Music & video", "Installers & caches (skipped)".
- **Fill:** `plan_topup` fills tiers in order:
  1. free space is counted after pruning `_Previous versions` oldest-first, if needed;
  2. live copies are never removed for space;
  3. items that don't fit go to `skipped_for_space` as `TierShortfall`.
- **Suggestion copy** when anything is unprotected: `"{n} GB doesn't fit. Add a second backup drive or free space on {label}."`

- [ ] **Step 1: Write failing tests:**
  - `only_copies_are_tier_one`
  - `installers_and_caches_never_backed_up`
  - `fills_by_tier_when_space_short`: fake backup `total_bytes` small. Tier 1 is fully planned, tier 3 is partially skipped, and the shortfall is reported.
  - `prunes_oldest_versions_before_skipping_live`
  - `remove_source_asks_bytes_and_retires_as_uncovered`: `remove_source` returns the bytes, and the next plan has RetireUncovered items.
- [ ] **Step 2: Run** and expect FAIL.
- [ ] **Step 3: Implement.**
- [ ] **Step 4: Run** `cargo test -p aegis-core` and expect PASS.
- [ ] **Step 5: Commit** `feat(backup): priority fill and coverage summary`.

### Task 14: Tauri commands, events, plug-in watcher, TS client

**Files:**
- Create: `src-tauri/src/commands/drives.rs`, `src-tauri/src/watch.rs`
- Modify: `src-tauri/src/commands/mod.rs`, `src-tauri/src/lib.rs`, `crates/aegis-core/src/contracts/mod.rs` (events + DTO exports), `src/bindings/client.ts`, regenerated `src/bindings/*`

**Interfaces:**
- **Commands** (all `#[tauri::command(async)]`):
  - `drives_list -> Vec<VolumeInfo>`
  - `drives_sync -> SyncReport`
  - `drive_catalog(volume_id)` (spawns; emits progress)
  - `drive_refresh(volume_id) -> RefreshReport`
  - `drive_plan_get -> DrivePlan`
  - `drive_plan_apply(roles: Vec<(String, DriveRole)>) -> AppliedPlan`
  - `dupes_grouped -> GroupedDupeReport`
  - `dupe_decide(group_id, choice: DupeChoice) -> DecisionOutcome`
  - `dupe_decisions_list`, `dupe_decision_undo(id)`
  - `backup_coverage -> Option<BackupCoverage>`
  - `backup_sources_list`, `backup_source_add(volume_id, rel_path, pinned)`, `backup_source_remove(id) -> i64`
  - `backup_topup_plan -> TopUpPlan`
  - `backup_topup_start(authority)`, `backup_topup_pause`
  - `backup_guard_override(folder)`
  - `backup_standing_grant`
  - `previous_versions_list`, `previous_version_restore(id, to?)`
- **New `AegisEvent` variants:**
  - `DriveArrived { v, volume_id, label, last_seen, known: bool }`
  - `DriveDeparted { v, volume_id }`
  - `DriveRefreshed { v, report: RefreshReport }`
  - `TopUpProgress { v, job_id, done, total, bytes }`
  - `TopUpFinished { v, result: TopUpResult }`
  - `GuardHalted { v, halts: Vec<GuardHalt> }`
- **`watch.rs`:** a thread started in `setup` that polls `WindowsProvider` every **3 s** and calls `sync_mounted` when the set of mounts changes. Then:
  - **Known drive arrives:** emit `DriveArrived`, then run `refresh_volume` in the background and emit `DriveRefreshed`.
  - **Unknown drive:** emit `DriveArrived{known:false}` only; cataloguing waits for the user to click.
  - **Backup drive arrives with standing approval:** start a top-up automatically with `TopUpAuthority::Standing`.

  The store is shared through the existing app state; follow how `scan_start` gets it. Pause sets the job's `AtomicBool`.
- **Client:** `aegis.drives.*`, `aegis.dupes.*`, `aegis.backup.*` mirroring the commands.

- [ ] **Step 1: Write a failing test** in `src-tauri` (or aegis-core, if src-tauri has no test harness): `watch_diff_detects_arrival_and_departure`. Extract the pure function `fn diff_mounts(prev: &[PathBuf], now: &[PathBuf]) -> (Vec<PathBuf>, Vec<PathBuf>)` and test it.
- [ ] **Step 2: Run** `cargo test --workspace` and expect FAIL.
- [ ] **Step 3: Implement** the commands, events and watcher. Regenerate bindings the way the repo does it now (find the `write_ts_bindings` caller), then extend `client.ts`.
- [ ] **Step 4: Verify** `cargo test --workspace`, `cargo clippy --workspace --all-targets -- -D warnings` and `pnpm tsc --noEmit`. Expect all clean.
- [ ] **Step 5: Commit** `feat(desktop): drive, dupe and backup commands with plug-in watcher`.

### Task 15: UI: Home tiles, Drives screen, duplicate review, plug-in banner

**Files:**
- Create: `src/components/HomeTiles.tsx`, `src/components/PlugBanner.tsx`, `src/screens/Drives.tsx`, `src/screens/DupeReview.tsx`
- Modify: `src/App.tsx` (Home becomes the command-centre layout from mockup A; add a Drives nav entry; mount `PlugBanner`)

**Interfaces:**
- **Consumes:** Task 14's client methods and events.
- **What each piece shows:**
  - **HomeTiles:**
    - a **Drives** tile listing each drive with its role chip, online/unplugged and "last seen {date}", and a backup ✓/◐ status;
    - a **Needs you** tile showing the drive plan (when ready and not yet applied), the count of duplicate groups with `needs_question`, guard halts and unknown drives.
  - **Drives screen:**
    - the drive list;
    - the drive-plan card (spec §7.6 layout, with Approve plan / Change roles);
    - per-drive catalog progress;
    - backup coverage bars by tier (spec §7.7.2 layout, with the suggestion);
    - backup sources with pin and remove (removal shows the confirm copy from spec §7.8.5);
    - the Decisions list with Undo;
    - a previous-versions browser with Restore.
  - **DupeReview:** one card per group, with locations side by side, a keeper radio, PartialFolder differences, and buttons "It's a backup" / "Keep both" / "Delete this copy". Offline locations show "on {label} (unplugged, last seen {date})". A refused delete shows the "Make this the backup copy" button.
  - **PlugBanner:** spec §7.4 copy with [Review] [Later]. For a standing top-up it shows progress plus **Pause**, then the summary "Backup up to date: {n} files added, {GB} GB."
- **Look:** follow the existing Tailwind tokens in the app. Dark mode should match the current theme. Icons must be meaningful (drive, shield, copy); no generic stars.

- [ ] **Step 1: Build the screens** against the real commands, and add a browser-preview fallback, as `isTauriRuntime()` already does. In preview, show fixture data so the screens render outside Tauri. Put fixtures in `src/screens/fixtures/drives.ts`.
- [ ] **Step 2: Verify** with `pnpm tsc --noEmit && pnpm build` (expect success). Then take Playwright screenshots of Home, Drives and DupeReview from the existing dev server (port 1420; do not start a second one). Delete `.playwright-mcp/` afterwards.
- [ ] **Step 3: Owner check.** Show the owner the screenshots and adjust layout or copy from their feedback before committing.
- [ ] **Step 4: Commit** `feat(ui): command-centre home tiles, drives screen, duplicate review, plug-in banner`.

### Task 16: End-to-end check and real-drive dry run

**Files:**
- Test: `crates/aegis-core/tests/backup.rs`

- [ ] **Step 1: Write** `end_to_end_three_drives`. It walks the whole flow on fake drives A, B and C plus a fake "PC" root:
  1. catalog all, fingerprint, and get `recommend_plan` → B;
  2. `apply_plan`, then approve and execute the adopt and stray batches;
  3. decide every remaining group;
  4. first top-up as UserApproved, then `grant_standing_topup`;
  5. change one PC file and delete one;
  6. run a Standing top-up.

  Assert:
  - S1: no hash ever drops to zero copies;
  - the previous version exists;
  - coverage reports everything protected;
  - `audit_log` contains the grant and the run.
- [ ] **Step 2: Run** `cargo test --workspace` and expect all PASS. Then run `cargo clippy --workspace --all-targets -- -D warnings` and expect it to be clean.
- [ ] **Step 3: Owner real-drive dry run.** Ask the owner to plug in one real external drive and run Catalog plus Fingerprint from the app (read-only; nothing is proposed or executed without approval). Report:
  - elapsed time for the catalog and the fingerprint;
  - file count;
  - duplicate groups found;
  - re-plug refresh time and mode (USN or Walk).
- [ ] **Step 4: Commit** `test: end-to-end drives, dupes and backup scenario`. Update `HANDOFF.md` if it exists with what was shipped and the dry-run numbers.
