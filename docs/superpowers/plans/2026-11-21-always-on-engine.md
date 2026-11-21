# Always-on Engine Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Aegis runs from the tray, starts with Windows, watches the PC and drives with the window closed, runs small "watches" when the PC is calm, reminds gently, and writes a Sunday note.

**Architecture:**
- **Decision logic lives in a new `aegis_core::engine` module.** That covers the watch trait, scheduling, reminders, the tripwire, the janitor, the space forecast and the note. It's pure functions over `SqliteStore`/`Connection` plus injected clocks and probes, so all of it can be tested.
- **`src-tauri` owns the Windows plumbing.** That means the tray, single-instance, autostart, toasts, the global shortcut, idle and power probes, the `notify` file watcher, and the engine thread.
- **Long jobs keep the existing pattern:** their own thread, their own `SqliteStore` and an `InFlight` guard.

**Tech Stack:** Rust (rusqlite, blake3, notify 8, chrono), Tauri 2 (+ tray-icon, single-instance, autostart, global-shortcut, tauri-winrt-notification), React + TypeScript (ts-rs bindings).

**Spec:** `docs/superpowers/specs/2026-11-21-always-on-engine-design.md`

## Global Constraints

- Notifications are **silent**. They never open a window by themselves, and they never steal focus.
- **Convenient** reminders: at most **one per rolling 24 h** across all kinds. They are held back while the user is busy or Aegis is paused. **Now** reminders ignore the cap and repeat at most once per 24 h per subject while open.
- A reminder kind is demoted to the note after **3 dismissals in a row**. Now-level kinds are never demoted.
- Calm means:
  - idle ≥ **120 s**;
  - (AC power **or** battery ≥ **50 %**);
  - not `QUNS_BUSY` / `QUNS_RUNNING_D3D_FULL_SCREEN` / `QUNS_PRESENTATION_MODE`.

  An idle-only watch waits at most **24 h** past due, then runs anyway.
- Tripwire values: window **60 s**, more than **200** files **and** more than **20 %** of the root's indexed files; sample **20** files; trips when ≥ **5 %** of the sample look encrypted (`guard::ENTROPY_SUSPECT_FRACTION`) or more than **100** renames go to an extension new to that root.
- Live index: debounce **2 s**, at most **500** paths per `index_paths` call.
- Note: default **Sunday 18:00** local. Sections are left out when they have nothing to say. "Needs you" holds at most **3** items.
- **Nothing is deleted without approval.** Janitor output is a *proposed* Quarantine batch (agent `"janitor"`), with at most one open janitor batch at a time.
- Safety never pauses: the live watcher and the tripwire keep running while paused.
- Windows-only code sits behind `#[cfg(windows)]` with a no-op fallback, so `cargo test --workspace` stays green everywhere.
- Owner-facing copy is plain English, with no jargon. Use the exact strings given in the spec §5–§9.
- Machine rules:
  - **Adding dependencies:** use `cargo add` via PowerShell with the sandbox disabled, then check the dependency was actually written.
  - **Test runs:** `cargo test` regenerates `src/bindings/*.ts` with LF line endings. Commit only files whose content changed (`git diff -w --ignore-cr-at-eol`).
  - **The owner may be running `target\release\aegis-desktop.exe`.** Don't run `cargo build --release` while it's running, and stage only your own files.

## Review Focus

1. **Second launch while the engine runs** (auto-start, then the owner clicks the shortcut). Expected: one process, and the window opens. Test in Task 10: `single_instance_plugin_registered_first`, plus a manual check in Task 12.
2. **Notification clicked when the window is closed.** Expected: the window opens *on the right screen*. A freshly created web view misses events sent before its JS loads. Test in Task 10: `pending_navigation_is_taken_once`.
3. **The same finding raised every day for weeks.** Expected: one reminder row, shown at most once per 24 h, no duplicate rows. Test in Task 3: `raising_same_subject_twice_updates_one_row`.
4. **The owner restores or deletes a file the janitor proposed.** Expected: the next run doesn't re-propose a file that's already in an open janitor batch, and files that have gone are skipped. Test in Task 7: `janitor_skips_files_in_open_batch_and_missing_files`.
5. **A bulk legitimate edit**, such as renaming 300 holiday photos, trips the tripwire. Expected: "It was me" clears the lock and the next top-up proceeds. Test in Task 4: `owner_override_unlocks_tripped_folder`.

---

## File Structure

| File | Responsibility |
|---|---|
| `crates/aegis-core/migrations/0004_engine.sql` | Tables: `watch_state`, `reminders`, `reminder_kinds`, `space_samples`, `root_samples`, `weekly_notes` |
| `crates/aegis-core/src/engine/mod.rs` | `Level`, `Finding`, `FindingAction`, `Screen`, `Watch` trait, `Calm`, `Engine` scheduler |
| `crates/aegis-core/src/engine/reminders.rs` | Raise / resolve / deliver / dismiss / act / demote |
| `crates/aegis-core/src/engine/tripwire.rs` | Sliding-window change counter and trip evaluation |
| `crates/aegis-core/src/engine/batcher.rs` | Debounced change batching for the live index |
| `crates/aegis-core/src/engine/watches.rs` | Only-copy, quarantine expiry, drive errors, catalog resume, self-check watches |
| `crates/aegis-core/src/engine/janitor.rs` | Downloads rules, installer matching, proposal |
| `crates/aegis-core/src/engine/space.rs` | Daily samples and forecast |
| `crates/aegis-core/src/engine/note.rs` | Weekly note build, store and due check |
| `crates/aegis-core/src/backup/guard.rs` | + folder lock (`lock_folder`, `locked_folders`); `check` halts locked folders; `override_halt` unlocks |
| `crates/aegis-core/src/scanner/mod.rs` | + `mark_interrupted_jobs` |
| `crates/aegis-core/tests/engine.rs` | Integration tests for Tasks 1–9 |
| `src-tauri/src/engine_host.rs` | Engine thread, calm probe, live watcher wiring, toast delivery |
| `src-tauri/src/shell.rs` | Tray, menu, icon state, window lifecycle, single-instance, autostart, global shortcut, pending navigation |
| `src-tauri/src/commands/engine.rs` | IPC for reminders, note, pause, engine settings |
| `src-tauri/icons/tray-{calm,waiting,urgent}.png` | Tray icon variants |
| `src/screens/ThisWeek.tsx`, `src/screens/EngineSettings.tsx` | New screens |
| `src/App.tsx`, `src/components/PlugBanner.tsx`, `src/components/HomeTiles.tsx`, `src/bindings/client.ts` | Navigation, tripwire banner, "This week" link, pause chip |

---

### Task 1: Engine tables and watch bookkeeping

**Files:**
- Create: `crates/aegis-core/migrations/0004_engine.sql`, `crates/aegis-core/src/engine/mod.rs`, `crates/aegis-core/tests/engine.rs`
- Modify: `crates/aegis-core/src/db/mod.rs:15-19` (add `(4, "0004_engine", …)`), `crates/aegis-core/src/lib.rs` (`pub mod engine;`)

**Interfaces:**
- Produces (`aegis_core::engine`):
  - `#[derive(TS, Serialize, Deserialize, Clone, Copy, PartialEq, Eq, Debug)] #[serde(rename_all="snake_case")] pub enum Level { Now, Convenient, Note }`
  - `pub enum Screen { Home, Drives, Dupes, Search, Rescue, Simulator, Week, Settings }` (snake_case, TS)
  - `pub enum FindingAction { Open { screen: Screen }, Batch { batch_id: String }, None }` (serde `tag="type"`, TS)
  - `pub struct Finding { kind: String, subject: String, level: Level, title: String, detail: String, bytes: Option<i64>, action: FindingAction }` (TS; `bytes` as `number | null`)
  - `pub struct WatchState { name: String, last_run_at: Option<i64>, last_ok_at: Option<i64>, last_error: Option<String> }`
  - `pub fn watch_state(conn: &Connection, name: &str) -> Result<WatchState>` (default row when missing)
  - `pub fn record_run(conn: &Connection, name: &str, now: i64, error: Option<&str>) -> Result<()>` (sets `last_ok_at = now` only when `error` is `None`)

**Schema (0004_engine.sql):**
- `watch_state(name TEXT PK, last_run_at INT, last_ok_at INT, last_error TEXT)`
- `reminders(id TEXT PK, kind TEXT NOT NULL, subject TEXT NOT NULL, level TEXT NOT NULL, title TEXT NOT NULL, detail TEXT NOT NULL, bytes INT, action_json TEXT NOT NULL, state TEXT NOT NULL DEFAULT 'open', first_raised_at INT NOT NULL, last_raised_at INT NOT NULL, last_shown_at INT, resolved_at INT, UNIQUE(kind, subject))`
- `reminder_kinds(kind TEXT PK, dismiss_streak INT NOT NULL DEFAULT 0, demoted INT NOT NULL DEFAULT 0, demoted_at INT, announced INT NOT NULL DEFAULT 0)`
- `space_samples(volume_id TEXT, day TEXT, used_bytes INT, total_bytes INT, PRIMARY KEY(volume_id, day))`
- `root_samples(root TEXT, day TEXT, bytes INT, PRIMARY KEY(root, day))`
- `weekly_notes(id TEXT PK, week TEXT UNIQUE NOT NULL, built_at INT NOT NULL, json TEXT NOT NULL, read_at INT)`

- [ ] **Step 1:** Write the test `record_run_tracks_last_ok_only_on_success` in `tests/engine.rs`. Assertions:
  - after `record_run(c,"w",100,None)`, `last_ok_at==Some(100)`;
  - after `record_run(c,"w",200,Some("boom"))`, `last_run_at==Some(200)`, `last_ok_at==Some(100)` and `last_error==Some("boom")`;
  - `watch_state(c,"missing").last_run_at==None`.
- [ ] **Step 2:** `cargo test -p aegis-core --test engine` and expect a compile FAIL (no `engine` module).
- [ ] **Step 3:** Add the migration, the module, the types and the two functions.
- [ ] **Step 4:** Run again and expect PASS. Then run `cargo test -p aegis-core` and expect the existing tests to pass (the migration applies on top of 0003).
- [ ] **Step 5:** Commit `feat(engine): tables and watch bookkeeping`.

### Task 2: Watch trait, calm gating and the scheduler

**Files:** Modify `crates/aegis-core/src/engine/mod.rs`. Test in `tests/engine.rs`.

**Interfaces:**
- Consumes: Task 1 (`record_run`, `watch_state`, `Finding`).
- Produces:
  - `pub struct Calm { pub idle_secs: u64, pub on_ac: bool, pub battery_pct: Option<u8>, pub busy: bool }`. `impl Calm { pub fn is_calm(&self) -> bool; pub fn busy_only(&self) -> bool /* == busy */ }`
  - `pub const CALM_IDLE_SECS: u64 = 120; pub const CALM_BATTERY_PCT: u8 = 50; pub const IDLE_GRACE_MS: i64 = 86_400_000;`
  - `pub trait Watch: Send { fn name(&self) -> &'static str; fn every_ms(&self) -> i64; fn needs_idle(&self) -> bool; fn kinds(&self) -> &'static [&'static str]; fn run(&mut self, store: &mut SqliteStore, now: i64) -> Result<Vec<Finding>>; }`
  - `pub fn is_due(every_ms: i64, last_run_at: Option<i64>, now: i64, needs_idle: bool, calm: bool) -> bool`
  - `pub struct Engine { watches: Vec<Box<dyn Watch>> }`, `impl Engine { pub fn new(watches: Vec<Box<dyn Watch>>) -> Self; pub fn tick(&mut self, store: &mut SqliteStore, now: i64, calm: &Calm, paused: bool) -> Result<TickReport> }`
  - `pub struct TickReport { pub ran: Vec<&'static str>, pub failed: Vec<(&'static str, String)> }`

**Rules:**
- `is_due`: never run means due. Otherwise `now - last >= every`, and when `needs_idle` also (`calm` or `now - last >= every + IDLE_GRACE_MS`).
- `tick`: for each due watch, skip `needs_idle` ones while `paused`. Run it and `record_run`. Pass its findings to `reminders::sync_findings(conn, watch.kinds(), &findings, now)` (Task 3). An error is recorded and does not stop the other watches. Task 2 lands a stub `sync_findings` that Task 3 replaces.

- [ ] **Step 1:** Write the tests:
  - `calm_requires_idle_power_and_not_busy`:
    - `Calm{idle_secs:120,on_ac:true,battery_pct:None,busy:false}.is_calm()`;
    - idle 119 → false;
    - `on_ac:false,battery_pct:Some(49)` → false, and `Some(50)` → true;
    - `busy:true` → false.
  - `idle_watch_runs_anyway_after_grace`:
    - `is_due(1000, Some(0), 1000, true, false)==false`;
    - `is_due(1000, Some(0), 1000+86_400_000, true, false)==true`;
    - `is_due(1000, None, 0, true, false)==true`.
  - `failing_watch_does_not_stop_others`: with two fake watches (one returns `Err`), `tick` gives `ran.len()==2` and `failed.len()==1`, and the healthy watch's `last_ok_at` is set.
  - `paused_skips_idle_watches_only`: while paused, the `needs_idle` watch doesn't run and the event watch (`needs_idle=false`) does.
- [ ] **Step 2:** Run them and expect FAIL.
- [ ] **Step 3:** Implement.
- [ ] **Step 4:** Run them and expect PASS.
- [ ] **Step 5:** Commit `feat(engine): watch trait and calm-gated scheduler`.

### Task 3: Reminders

**Files:** Create `crates/aegis-core/src/engine/reminders.rs`. Test in `tests/engine.rs`.

**Interfaces:**
- Produces:
  - `#[derive(TS)] pub struct Reminder { id, kind, subject, level: Level, title, detail, bytes: Option<i64>, action: FindingAction, state: String, first_raised_at: i64, last_shown_at: Option<i64> }`
  - `pub fn sync_findings(conn: &Connection, kinds: &[&str], findings: &[Finding], now: i64) -> Result<()>`. Upserts by `(kind, subject)`: the text, bytes and level are refreshed, `last_raised_at = now`, and a resolved row reopens. Open rows of `kinds` whose subject is absent from `findings` become `resolved`. When the kind is demoted and the finding is `Convenient`, it's stored as `Note`.
  - `pub fn to_deliver(conn: &Connection, now: i64, busy: bool, paused: bool) -> Result<Vec<Reminder>>`:
    - **Now:** every open `Now` row with `last_shown_at` null or ≤ `now - 86_400_000`.
    - **Convenient:** nothing when `busy || paused`, or when any Convenient row has `last_shown_at > now - 86_400_000`. Otherwise the single oldest open Convenient row (by `first_raised_at`) not shown in 24 h.
  - `pub fn mark_shown(conn, id: &str, now: i64) -> Result<()>`
  - `pub fn dismiss(conn, id: &str, now: i64) -> Result<bool /* kind now demoted */>`. Increments `reminder_kinds.dismiss_streak`. At ≥ 3, and only when the row's level is not `Now`, it sets `demoted=1, demoted_at=now, announced=0` and moves the kind's open Convenient rows to `Note`.
  - `pub fn act(conn, id: &str, now: i64) -> Result<()>`. Resolves the row and resets the kind's streak to 0.
  - `pub fn undemote(conn, kind: &str) -> Result<()>`
  - `pub fn list_open(conn) -> Result<Vec<Reminder>>` (ordered Now, Convenient, Note, then oldest first)
  - `pub fn demoted_kinds(conn) -> Result<Vec<(String, bool /* announced */)>>`, `pub fn mark_announced(conn, kind) -> Result<()>`
  - `pub fn tray_state(conn) -> Result<TrayState>` with `#[derive(TS)] pub enum TrayState { Calm, Waiting, Urgent }`: Urgent if any open Now row, Waiting if any open Convenient or Note row, otherwise Calm.

- [ ] **Step 1:** Write the tests:
  - `raising_same_subject_twice_updates_one_row`: one row after two `sync_findings` with the same `(kind,subject)` and the second call's title.
  - `missing_subject_resolves`: a second sync without the subject gives `state=="resolved"`. A third sync with it again gives `"open"`.
  - `one_convenient_per_day`: with two open Convenient rows, `to_deliver` returns 1. After `mark_shown` at t, `to_deliver(t+3_600_000)` returns 0 and `to_deliver(t+86_400_001)` returns 1 (the other row).
  - `busy_holds_convenient_not_now`: with one Now and one Convenient row, `to_deliver(busy=true)` returns only the Now row.
  - `three_dismissals_demote_convenient_kind`: after 3 `dismiss` calls the third returns `true`, the next sync of that kind stores `level==Note`, and `undemote` restores Convenient on the next sync.
  - `now_kind_never_demoted`: after 5 dismissals of a Now row, it's still `Now`.
  - `acting_resets_streak`: dismiss, dismiss, act, dismiss leaves it not demoted.
  - `tray_state_reflects_worst_open_level`.
- [ ] **Step 2:** Run them and expect FAIL.
- [ ] **Step 3:** Implement. Replace Task 2's stub.
- [ ] **Step 4:** Run them and expect PASS.
- [ ] **Step 5:** Commit `feat(engine): reminders with daily cap and dismissal learning`.

### Task 4: Ransomware tripwire and backup lock

**Files:**
- Create: `crates/aegis-core/src/engine/tripwire.rs`
- Modify: `crates/aegis-core/src/backup/guard.rs`
- Test: `tests/engine.rs`, `tests/backup.rs`

**Interfaces:**
- Consumes: `guard::looks_encrypted`, `guard::ENTROPY_SUSPECT_FRACTION`, `guard::override_halt`, `guard::check`.
- Produces:
  - `pub const TRIP_WINDOW_MS: i64 = 60_000; pub const TRIP_MIN_FILES: usize = 200; pub const TRIP_MIN_FRACTION: f64 = 0.20; pub const TRIP_SAMPLE: usize = 20; pub const TRIP_NEW_EXT_RENAMES: usize = 100;`
  - `pub enum ChangeKind { Created, Modified, Removed, Renamed { to: PathBuf } }`
  - `pub struct Tripwire { /* per-root VecDeque<(i64, PathBuf, ChangeKind)> */ }`, `impl Tripwire { pub fn new(roots: Vec<String>) -> Self; pub fn observe(&mut self, path: &Path, kind: ChangeKind, now: i64); pub fn evaluate(&mut self, conn: &Connection, now: i64) -> Result<Vec<Trip>> }`
  - `#[derive(TS)] pub struct Trip { pub root: String, pub changed: i64, pub total: i64, pub suspect_encrypted: i64, pub new_ext_renames: i64 }`
  - `pub fn trip_finding(t: &Trip) -> Finding`:
    - kind `"tripwire"`, subject = root, level `Now`;
    - title `"Lots of files in {name} just changed at once"`;
    - detail `"{changed} files changed in a minute{, N look scrambled}. Backup of this folder is paused until you check."`;
    - action `Open{Drives}`.
  - In `guard`:
    - `pub fn lock_folder(store: &mut SqliteStore, folder: &str, reason: &str) -> Result<()>` (setting key `backup.locked_folders`, JSON list; audited `backup.tripwire_lock`);
    - `pub fn locked_folders(conn: &Connection) -> Result<Vec<String>>`.
    - `check` halts any folder whose items fall inside a locked folder (`path_is_within`), even below the mass thresholds.
    - `override_halt` also removes the folder (and any locked folder it contains) from the lock list.

**Evaluate rules:**
- Drop events older than `now - TRIP_WINDOW_MS`.
- `total` = live indexed files under the root.
- A trip needs `changed > TRIP_MIN_FILES` and `changed as f64 > TRIP_MIN_FRACTION * total`.
- Then either:
  - **suspect:** `looks_encrypted` on up to `TRIP_SAMPLE` existing changed paths, with suspect/sampled ≥ `ENTROPY_SUSPECT_FRACTION`; or
  - **renames:** more than `TRIP_NEW_EXT_RENAMES` renames whose target extension doesn't appear among the root's indexed files.
- A root that tripped isn't re-reported until its window empties.

- [ ] **Step 1:** Write the tests:
  - `burst_of_random_rewrites_trips`: a temp root with 300 indexed files; overwrite 250 with `common::noise`; observe 250 `Modified` within 10 s. Expect one `Trip` with `suspect_encrypted >= 1`.
  - `burst_of_normal_edits_does_not_trip`: 250 text edits give no trip.
  - `small_folder_change_below_fraction_does_not_trip`: 1,500 files, 210 changed (14 %), gives none.
  - `mass_rename_to_new_extension_trips`: 150 `Renamed{to: x.locked}` gives a trip with `new_ext_renames==150`.
  - `old_events_fall_out_of_window`.
  - In `tests/backup.rs`, `locked_folder_halts_topup` (below the mass thresholds, `check` → `Halt` for the locked folder) and `owner_override_unlocks_tripped_folder` (`override_halt` → `locked_folders` empty and `check` → `Proceed`).
- [ ] **Step 2:** Run them and expect FAIL.
- [ ] **Step 3:** Implement.
- [ ] **Step 4:** Run them and expect PASS. Then run `cargo test -p aegis-core --test backup` to confirm the existing guard tests still pass.
- [ ] **Step 5:** Commit `feat(engine): ransomware tripwire with backup folder lock`.

### Task 5: Live-index batching and interrupted jobs

**Files:**
- Create: `crates/aegis-core/src/engine/batcher.rs`
- Modify: `crates/aegis-core/src/scanner/mod.rs`
- Test: `tests/engine.rs`

**Interfaces:**
- Produces:
  - `pub const DEBOUNCE_MS: i64 = 2_000; pub const MAX_PATHS_PER_TX: usize = 500;`
  - `pub struct ChangeBatcher`, `impl ChangeBatcher { pub fn new() -> Self; pub fn push(&mut self, path: PathBuf, now: i64); pub fn drain_ready(&mut self, now: i64) -> Vec<Vec<PathBuf>> }`. It returns nothing until `now - last_push >= DEBOUNCE_MS`, then all pending paths, de-duplicated and sorted, in chunks of ≤ `MAX_PATHS_PER_TX`.
  - `pub fn index_changed(store: &mut SqliteStore, volume_id: &str, chunks: Vec<Vec<PathBuf>>) -> Result<usize>` calls `scanner::index_paths` once per chunk, so each chunk is its own transaction.
  - `scanner::mark_interrupted_jobs(conn: &Connection, now: i64) -> Result<usize>` sets every `scan_jobs` row with `status='running'` to `status='interrupted', finished_at=now`. It's called once at engine start, before any job runs.

- [ ] **Step 1:** Write the tests:
  - `batcher_waits_for_quiet_then_chunks`: push 1,200 distinct paths (plus 100 repeats) at t=0..100. `drain_ready(1_500)` is empty. `drain_ready(2_100)` gives chunk sizes `[500, 500, 200]`. A second drain is empty.
  - `index_changed_applies_all_chunks`: 600 new files under a catalogued fake drive give `index_changed` → 600, and they're live in `files`.
  - `stale_running_jobs_marked_interrupted`.
- [ ] **Step 2:** Run them and expect FAIL.
- [ ] **Step 3:** Implement.
- [ ] **Step 4:** Run them and expect PASS.
- [ ] **Step 5:** Commit `feat(engine): debounced live-index batches; mark interrupted scans`.

### Task 6: Safety watches

**Files:** Create `crates/aegis-core/src/engine/watches.rs`. Test in `tests/engine.rs`.

**Interfaces:**
- Consumes: `Watch`, `Finding`, `coverage_summary`, `backup::backup_volume`, `quarantine::list_quarantine`, `InFlight`-free decision functions.
- Produces these watch structs, each `impl Watch`:

| Struct | name | every_ms | needs_idle | kinds |
|---|---|---|---|---|
| `OnlyCopyWatch` | `only_copy` | 86_400_000 | false | `["only_copy"]` |
| `QuarantineExpiryWatch` | `quarantine_expiry` | 86_400_000 | false | `["quarantine_expiry"]` |
| `DriveErrorsWatch` | `drive_errors` | 3_600_000 | false | `["drive_errors"]` |
| `SelfCheckWatch` | `self_check` | 3_600_000 | false | `["engine_health"]` |

  - **Only-copy:**
    - unprotected = the sum of `unprotected_bytes` for tiers 1 and 2;
    - days since the Backup drive's `last_seen`;
    - raise when (unprotected ≥ 1_000_000_000 and days ≥ 7) or (unprotected > 0 and days ≥ 14);
    - subject `"backup"`, Convenient;
    - title `"{bytes} exist only on this PC"`;
    - detail `"Your Backup drive {label} hasn't been plugged in for {days} days."`;
    - action `Open{Drives}`.
    - With no Backup drive assigned there's no finding (the plan card covers that case).
  - **Quarantine expiry:** live entries with `expires_at` in `(now, now + 2 days]`. One finding, subject `"quarantine"`, Convenient, title `"{n} files are deleted for good on {weekday}"`, detail `"Keep any? They're in Disk Rescue & Quarantine."`, action `Open{Rescue}`.
  - **Drive errors:** for each volume's latest finished `scan_jobs` row on its mount root, with `errors_json` array length ≥ 10. Subject = volume id, `Now`, title `"{label} had trouble reading {n} files"`, detail `"Copy what matters off it soon."`, action `Open{Drives}`.
  - **Self-check:** for the `heartbeats: Vec<(&'static str, i64 /* every_ms */)>` given at construction, a watch whose `last_ok_at` is older than `3 * every_ms` gives a `Note` finding, subject = name, title `"{name} stopped working and was restarted"`. Also, when the db file size grew more than 20 % since the size stored in `watch_state.last_error` of the pseudo-row `"db_size"`, a Note `"Aegis's database grew {pct}% this week"`.
  - `pub fn catalogs_to_resume(conn: &Connection) -> Result<Vec<String>>`: online volumes with a `passport_id`, `last_catalog_complete_at IS NULL`, and at least one `scan_jobs` row whose `root_path` equals their mount point. (The engine host runs them through `InFlight`.)

- [ ] **Step 1:** Write the tests:
  - `only_copy_raises_after_seven_days_with_a_gigabyte` (and none at 6 days);
  - `only_copy_raises_any_amount_after_fourteen_days`;
  - `only_copy_silent_without_backup_drive`;
  - `quarantine_expiry_counts_next_two_days_only`;
  - `drive_errors_raise_now_at_ten`;
  - `self_check_reports_stale_heartbeat`;
  - `resume_only_previously_started_uncatalogued_drives`.
- [ ] **Step 2:** Run them and expect FAIL.
- [ ] **Step 3:** Implement.
- [ ] **Step 4:** Run them and expect PASS.
- [ ] **Step 5:** Commit `feat(engine): only-copy, quarantine expiry, drive errors, self-check watches`.

### Task 7: Downloads janitor

**Files:** Create `crates/aegis-core/src/engine/janitor.rs`. Modify the workspace `windows-sys` features to add `Win32_System_Registry`. Test in `tests/engine.rs`.

**Interfaces:**
- Produces:
  - `#[derive(TS)] pub enum JanitorKind { Partial, Repeat { of: String }, Installer { already_installed: bool, age_days: i64 } }`
  - `#[derive(TS)] pub struct JanitorItem { pub path: String, pub bytes: i64, pub kind: JanitorKind }`
  - `pub fn scan_downloads(conn: &Connection, downloads: &Path, installed: &[String], now: i64) -> Result<Vec<JanitorItem>>`. It reads the directory (one level and subfolders; it doesn't follow links) and skips paths that are in an open (`proposed`/`approved`) batch of any agent, or that no longer exist.
    - **Partial:** extensions `crdownload|part|partial|download`, with mtime older than 24 h.
    - **Repeat:** the stem matches `^(.*) \((\d+)\)$` and the file has the same size and blake3 as `<base>.<ext>` or another `(m)` sibling. The oldest by mtime is kept, the rest are items.
    - **Installer:** `exe|msi|msix`, mtime older than 14 days.
  - `pub fn installer_matches(file_stem: &str, installed: &[String]) -> bool`:
    - tokenize by splitting on non-alphanumerics and camelCase boundaries, lowercase;
    - drop `setup installer install x64 x86 amd64 arm64 win win64 win32 full user` and tokens that are all digits;
    - true when the first remaining token equals the first token of a DisplayName (same tokenizer), and the second tokens also match when both have one.
  - `#[cfg(windows)] pub fn installed_programs() -> Vec<String>` reads DisplayName under `SOFTWARE\Microsoft\Windows\CurrentVersion\Uninstall` (HKLM 64-bit, HKLM WOW6432Node, HKCU). The non-Windows version returns an empty Vec.
  - `pub fn propose(store: &mut SqliteStore, items: &[JanitorItem]) -> Result<Option<String>>`. With no items, or an open batch whose agent is `"janitor"` already present, it returns `None`. Otherwise it creates a Quarantine batch via `propose_batch` with `agent: Some("janitor")` and a reason per kind:
    - `"Unfinished download, untouched for a day"`;
    - `"Same file as {of}"`;
    - `"Installer for a program you already have"` or `"Installer not opened in {n} days"`.
  - `pub struct JanitorWatch { downloads: PathBuf, installed: Box<dyn Fn() -> Vec<String> + Send> }`, `impl Watch` (name `janitor`, every 86_400_000, needs_idle true, kinds `["janitor"]`). It gives one Note finding, subject `"downloads"`, title `"{bytes} of old installers and repeat downloads"`, action `Batch{batch_id}`.

- [ ] **Step 1:** Write the tests:
  - `partial_downloads_older_than_a_day`;
  - `repeat_download_with_same_hash_flagged_keeps_oldest`;
  - `repeat_name_but_different_content_not_flagged`;
  - `installer_age_and_match`. Assertions: `installer_matches("ZoomInstallerFull", &["Zoom Workplace".into()])`, `installer_matches("Firefox Setup 133.0", &["Mozilla Firefox (x64 en-US)".into()]) == false`, `installer_matches("vlc-3.0.21-win64", &["VLC media player".into()])`;
  - `janitor_skips_files_in_open_batch_and_missing_files`;
  - `propose_once_while_batch_open`.
- [ ] **Step 2:** Run them and expect FAIL.
- [ ] **Step 3:** Implement.
- [ ] **Step 4:** Run them and expect PASS.
- [ ] **Step 5:** Commit `feat(engine): downloads janitor proposals`.

### Task 8: Space recorder and forecast

**Files:** Create `crates/aegis-core/src/engine/space.rs`. Test in `tests/engine.rs`.

**Interfaces:**
- Produces:
  - `pub fn record_day(conn: &Connection, day: &str, volumes: &[(String, i64 /* used */, i64 /* total */)], roots: &[(String, i64)]) -> Result<()>` (upsert, so one row per day)
  - `pub fn root_sizes(conn: &Connection) -> Result<Vec<(String, i64)>>` sums live `files.size` under each enabled root (`roots_list`)
  - `#[derive(TS)] pub struct Forecast { pub volume_id: String, pub weeks_left: f64, pub fastest_root: Option<String>, pub fastest_root_bytes_per_week: i64 }`
  - `pub fn forecast(conn: &Connection, volume_id: &str, today: &str) -> Result<Option<Forecast>>`. It uses samples in the last 28 days and needs ≥ 7. It fits least squares (x = day index, y = used). It returns `None` when the slope ≤ 0 or `weeks_left >= 26`. `fastest_root` = the root with the largest (last − first)/weeks over the same window, ignoring ≤ 0.
  - `pub struct SpaceWatch { volumes: Box<dyn Fn() -> Vec<(String, i64, i64)> + Send>, today: Box<dyn Fn() -> String + Send> }`, `impl Watch` (name `space`, every 21_600_000 so it gets one sample a day even with a short uptime, not idle, kinds `["space"]`). It gives a Convenient finding when `weeks_left < 3`: title `"{label} fills up in about {n} weeks"`, detail `"{root} is growing fastest."`, action `Open{Week}`.

- [ ] **Step 1:** Write the tests:
  - `forecast_needs_seven_samples`;
  - `steady_growth_gives_weeks_left` (used grows 10 GB/day with 700 GB free gives `weeks_left` ≈ 10.0 ± 0.1);
  - `shrinking_or_far_off_gives_none`;
  - `same_day_recorded_twice_keeps_one_row`;
  - `fastest_root_picked`.
- [ ] **Step 2:** Run them and expect FAIL.
- [ ] **Step 3:** Implement.
- [ ] **Step 4:** Run them and expect PASS.
- [ ] **Step 5:** Commit `feat(engine): daily space samples and fill-up forecast`.

### Task 9: The Sunday note

**Files:** Create `crates/aegis-core/src/engine/note.rs`. Test in `tests/engine.rs`.

**Interfaces:**
- Consumes: reminders (`list_open`, `demoted_kinds`, `mark_announced`), `coverage_summary`, `space::forecast`, `watch_state`.
- Produces:
  - `#[derive(TS)] pub struct WeeklyNote { pub week: String, pub headline: String, pub needs_you: Vec<Reminder> /* ≤3 */, pub did: Vec<String>, pub safety: Option<String>, pub space: Vec<String>, pub tidy: Option<String>, pub about_aegis: Vec<String>, pub read: bool }`
  - `pub struct LocalNow { pub iso_week: String /* "2026-W47" */, pub weekday: u8 /* 0=Mon..6=Sun */, pub hour: u8 }`
  - `pub fn is_due(conn: &Connection, local: &LocalNow, note_day: u8, note_hour: u8) -> Result<bool>`: no note stored for `local.iso_week`, and either (weekday == note_day and hour ≥ note_hour) or weekday is after note_day.
  - `pub fn build(conn: &Connection, week: &str, week_start_ms: i64, now: i64) -> Result<WeeklyNote>`:
    - **needs_you:** Now first, then Convenient, then Note, max 3.
    - **did:** these lines, each only when its count > 0:
      - `"Indexed {n} new files"` (files `first_seen` in the week);
      - `"Topped up the backup {n} times"` (completed `backup_jobs`);
      - `"Catalogued {n} drives"`.
    - **safety:**
      - `"Everything important is backed up."` when tier 1+2 unprotected = 0;
      - else `"{bytes} exist only on this PC. Backup drive last seen {date}."`;
      - `None` without a Backup drive.
    - **space:** one forecast line per volume that has one.
    - **tidy:** from the open janitor reminder.
    - **about_aegis:** Note findings of kind `engine_health`, plus `"I stopped reminding you about {kind}; it's here instead."` for each demoted, unannounced kind (then `mark_announced`).
    - **headline:** `"Needs you: {n} things."` when needs_you contains a Now; `"A quiet week. Everything important is backed up."` when safety is fully backed up and needs_you is empty; otherwise `"A quiet week. {n} things to look at when you have a minute."`
  - `pub fn save(conn, note: &WeeklyNote, now: i64) -> Result<()>`, `pub fn latest(conn) -> Result<Option<WeeklyNote>>`, `pub fn mark_read(conn, week: &str, now: i64) -> Result<()>`

- [ ] **Step 1:** Write the tests:
  - `due_on_note_day_after_hour_once_per_week`: Sun 17:00 → false; Sun 18:00 → true; after `save` → false; Mon of the same ISO week (missed Sunday) → true when unsaved.
  - `empty_sections_left_out`.
  - `needs_you_capped_at_three_now_first`.
  - `headline_variants`.
  - `demoted_kind_announced_once`.
- [ ] **Step 2:** Run them and expect FAIL.
- [ ] **Step 3:** Implement.
- [ ] **Step 4:** Run them and expect PASS.
- [ ] **Step 5:** Commit `feat(engine): weekly note`.

### Task 10: Desktop shell: tray, single instance, autostart, window lifecycle, engine host

**Files:**
- Create: `src-tauri/src/shell.rs`, `src-tauri/src/engine_host.rs`, `src-tauri/src/commands/engine.rs`, `src-tauri/icons/tray-{calm,waiting,urgent}.png`
- Modify: `src-tauri/src/lib.rs`, `src-tauri/Cargo.toml`, root `Cargo.toml` (tauri features `tray-icon`, `image-png`), `src-tauri/tauri.conf.json` (main window `"create": false`), `src-tauri/capabilities/default.json`, `src-tauri/src/watch.rs` (catalog resume and the drive-errors trigger use `InFlight`), `crates/aegis-core/src/contracts/mod.rs` (events)

**Dependencies:**
- `tauri-plugin-single-instance = "2"`, `tauri-plugin-autostart = "2"`, `tauri-plugin-global-shortcut = "2"`
- `[target.'cfg(windows)'] tauri-winrt-notification = "0.7"`
- `notify = "8"`, `chrono = "0.4"`
- `windows-sys` features `Win32_UI_Input_KeyboardAndMouse`, `Win32_System_Power`, `Win32_System_SystemInformation`

**Interfaces:**
- Consumes: Tasks 1–9, `InFlight`, `AppState`.
- Produces:
  - **New `AegisEvent` variants:** `Navigate { v, screen: Screen }`, `EngineUpdated { v }`, `Tripped { v, trip: Trip }`.
  - **`AppState` additions:**
    - `pending_nav: Mutex<Option<Screen>>`;
    - `paused_until: AtomicI64`;
    - `engine_wake: std::sync::mpsc::Sender<EngineMsg>`, where `enum EngineMsg { FsChange(PathBuf, ChangeKind), Tick, Reload }`.
  - **`shell.rs`:**
    - `pub fn open_main(app: &AppHandle, screen: Option<Screen>)` creates the window from `app.config().app.windows[0]` when missing, or shows and focuses it. With `screen`, it sets `pending_nav` *and* emits `Navigate`.
    - `pub fn set_tray_state(app: &AppHandle, s: TrayState, waiting: usize)` sets the icon and the tooltip `"Aegis: all calm"` / `"Aegis: {n} things waiting"` / `"Aegis: needs you now"`.
    - `pub fn build_tray(app: &AppHandle) -> tauri::Result<()>`. The menu has: Open Aegis, Search everywhere (Ctrl+Shift+Space), This week's note, Back up now (enabled only while the Backup drive is online), Pause ▸ (1 hour, Until tomorrow, Resume), Quit Aegis. A left click calls `open_main(None)`.
  - **`engine_host.rs`:**
    - `pub fn calm_now() -> Calm` uses `GetLastInputInfo` + `GetTickCount64`, `GetSystemPowerStatus` and `SHQueryUserNotificationState`.
    - `pub fn spawn(app: AppHandle, rx: Receiver<EngineMsg>)` does the following on its own thread with its own `SqliteStore`:
      1. calls `mark_interrupted_jobs`;
      2. builds `Engine` with all watches;
      3. runs a `notify::RecommendedWatcher` on the enabled roots, feeding `FsChange`;
      4. loops on `rx.recv_timeout(30s)`. `FsChange` goes to `ChangeBatcher` + `Tripwire`. Every loop it:
         - drains batches → `index_changed`;
         - evaluates the tripwire; on a trip it runs `lock_folder`, `sync_findings`, emits `Tripped`, and delivers the toast at once;
         - runs `engine.tick(calm, paused)`;
         - checks `note::is_due` (local time from `chrono::Local`) → build + save + toast `"Your weekly note is ready"`;
         - runs `reminders::to_deliver(busy, paused)` → toast each → `mark_shown`;
         - calls `set_tray_state`;
         - emits `EngineUpdated` when anything changed.
    - `fn toast(app: &AppHandle, r: &Reminder)` uses `tauri_winrt_notification::Toast`:
      - app id = the bundle identifier when running installed, else `Toast::POWERSHELL_APP_ID`;
      - `.title(&r.title).text1(&r.detail).sound(None)`;
      - `.on_activated(move |_| open_main(app, Some(screen_of(&r.action))))`.
  - **`commands/engine.rs`:**
    - `reminders_open() -> Vec<Reminder>`, `reminder_dismiss(id) -> bool`, `reminder_act(id)`
    - `note_latest() -> Option<WeeklyNote>`, `note_mark_read(week)`
    - `engine_pause(mode: "hour" | "tomorrow" | "resume")`
    - `engine_status() -> EngineStatus { paused_until: Option<i64>, tray: TrayState, autostart: bool, note_day: u8, note_hour: u8, shortcut: String, demoted: Vec<String>, shortcut_ok: bool }`
    - `engine_settings_set(autostart: Option<bool>, note_day: Option<u8>, note_hour: Option<u8>, shortcut: Option<String>)`, `reminder_undemote(kind)`
    - `nav_take() -> Option<Screen>` (takes and clears `pending_nav`)
    - `tripwire_was_me(folder)`, which calls `override_halt` for the Backup drive, resolves the tripwire reminder and emits `EngineUpdated`.

**Lifecycle rules:**
- The single-instance plugin is registered **first**. Its callback calls `open_main(None)`.
- Autostart is enabled once, on first run (setting `engine.autostart_initialised`), with args `["--background"]`.
- In setup, the window is created unless the args contain `--background`.
- `RunEvent::ExitRequested { code: None, api, .. }` calls `api.prevent_exit()`. "Quit Aegis" calls `app.exit(0)`.
- On the first `WindowEvent::Destroyed` ever (setting `engine.close_hint_shown`), a toast says `"Aegis keeps watching in the tray. Right-click it to quit."`
- The global shortcut (default `CommandOrControl+Shift+Space`, or the saved setting) calls `open_main(Some(Screen::Search))`. If registration fails, `shortcut_ok=false`.

**Tray icons:** generate from `src-tauri/icons/32x32.png` with Python PIL in the scratchpad. Waiting = an amber `#F59E0B` dot and urgent = a red `#EF4444` dot, both 12 px with a 2 px dark ring, at the bottom right. Calm = unchanged. Commit the three PNGs and load them with `include_bytes!` + `Image::from_bytes`.

- [ ] **Step 1:** Write the tests (unit tests in `src-tauri`):
  - `pending_navigation_is_taken_once`: set `Some(Week)`; `nav_take` → `Some(Week)`, then `None`.
  - `single_instance_plugin_registered_first`: factor the builder plugin order into `fn plugin_order() -> &'static [&'static str]` and assert `[0] == "single-instance"`.
  - `pause_modes_compute_until`: `pause_until("hour", now) == now + 3_600_000`; `"tomorrow"` = next local midnight + 7 h; `"resume"` = 0.
  - `calm_probe_does_not_panic` (`#[cfg(windows)]`): `calm_now()` returns.
- [ ] **Step 2:** Run `cargo test -p aegis-desktop` and expect FAIL.
- [ ] **Step 3:** Add the dependencies (PowerShell, sandbox disabled; check that `Cargo.toml` changed), the icons, the shell, the engine host and the commands, and register them in `lib.rs`. Have `watch.rs` call `InFlight`-guarded `drive_catalog` logic for `catalogs_to_resume` on arrival.
- [ ] **Step 4:** Run `cargo test --workspace` and `cargo clippy --workspace --all-targets` and expect them to be clean.
- [ ] **Step 5:** Commit `feat(desktop): tray, single instance, autostart, engine host, toasts`.

### Task 11: UI: This week, settings, tripwire banner, navigation

**Files:**
- Create: `src/screens/ThisWeek.tsx`, `src/screens/EngineSettings.tsx`, `src/screens/fixtures/engine.ts`
- Modify: `src/App.tsx`, `src/components/PlugBanner.tsx`, `src/components/HomeTiles.tsx`, `src/bindings/client.ts`

**Interfaces:**
- Consumes: the Task 10 commands and events, and the bindings `WeeklyNote`, `Reminder`, `EngineStatus`, `Screen`, `Trip`.
- Produces:
  - `aegis.engine.{remindersOpen, dismiss, act, noteLatest, noteMarkRead, pause, status, settingsSet, undemote, navTake, tripwireWasMe}`.
  - `TabId` gains `"week" | "settings"`. On mount, App calls `navTake()` and listens for `Navigate`, mapping `Screen` → tab (`search` focuses the search box).

**UI rules:**
- **ThisWeek:** the sections in the spec order, leaving out empty ones. "Needs you" rows have "Show me" (runs the action) and "Not now" (`dismiss`). Opening the screen calls `noteMarkRead`. With no note yet: `"Your first note arrives on {day} evening."`
- **EngineSettings:**
  - "Start with Windows" switch;
  - note day and hour selects;
  - shortcut field (shows `"That shortcut is taken; pick another."` when `!shortcut_ok`);
  - "Reminders moved to your note" list with "Remind me again";
  - pause buttons.
- **PlugBanner:** a `tripped` kind from the `Tripped` event: `"Lots of files in {name} just changed at once. Backup of this folder is paused."` with buttons "It was me" (`tripwireWasMe`) and "Show me" (opens Drives).
- **Home:** a "This week" link in the Needs-you tile, and a `"Paused until {time}"` chip with "Resume" while paused.
- **Preview** (no Tauri): fixtures give a sample note with every section, 2 open reminders and paused=false.

- [ ] **Step 1:** Run `pnpm build` and expect a FAIL until the types are wired (the bindings are already regenerated by Task 10 tests).
- [ ] **Step 2:** Implement the screens, client calls and navigation.
- [ ] **Step 3:** Run `pnpm build` and expect it to be clean.
- [ ] **Step 4:** Using the Vite server on 1420 (reuse it if it's already running), take Playwright screenshots of This week, Settings, Home with the link, and the tripwire banner (trigger it from the preview fixture). Move them to the scratchpad and delete `.playwright-mcp/`.
- [ ] **Step 5:** Commit `feat(ui): this week, engine settings, tripwire banner, navigation`.

### Task 12: Whole-app verification and handoff

- [ ] **Step 1:** Run `cargo test --workspace`, `cargo clippy --workspace --all-targets` and `pnpm build`, and expect all to be green.
- [ ] **Step 2:** Ask the owner to quit the running Aegis (tray → Quit, or close it). Then run `pnpm tauri build` to produce the NSIS installer, and have the owner install it (toasts need the installed identity).
- [ ] **Step 3:** Manual checks:
  1. A second launch focuses the existing instance and shows one `aegis-desktop` process in Task Manager.
  2. Closing the window leaves the tray icon, and the `msedgewebview2` children of Aegis exit. Measure the working set with `Get-Process aegis-desktop` (target ≤ 350 MB).
  3. Ctrl+Shift+Space opens Search.
  4. A test reminder (debug command `engine_debug_raise`, only in `cfg(debug_assertions)` builds, or a Downloads file seeded for the janitor) shows a silent toast, and clicking it opens the right screen with the window closed.
  5. Copying 300 random-content files over 300 indexed ones in a scratch root added as a PC root trips the tripwire, and "It was me" clears it.
  6. After a reboot, Aegis starts in the tray with no window.
- [ ] **Step 4:** Update `HANDOFF.md`: Project 2 status, the measured numbers, and any deferred items. Commit `docs: handoff after always-on engine`.
