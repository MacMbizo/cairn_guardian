# Context layer and Projects module: Plan 4 (Put back together, learned aliases, and weekly-note summary)

**Status:** Built, verified, and fast-forward merged into `master` (2026-11-23). Nothing pushed.

**Goal:** Complete spec §11, §12, §8.5, and §16 steps 5–6 so that:
- files moved out of a project root, design set, or photo subject by earlier `filing:*` / `filing-auto:*` batches are detected via `file_history` and offered as one atomic **"Put back together"** batch per collection and drive (`ReunitePlan`), citing the batch(es) that split them and undoing cleanly;
- explicit `"Same project"` answers (`Answer::SameAs`) teach normalized-name aliases across future projects (`learned_aliases`, e.g. `NutriP` <-> `Nutrition`), while `"Not the same"` (`Answer::NotSameAs`) vetoes and revokes chronologically;
- the weekly note (`ThisWeek`) calmly summarizes multi-location projects (`"87 projects have copies in 2–20 places."`) and pending reunite groups with one click to review.

**Tech Stack:** Rust, SQLite (no schema migration required), Tauri 2 IPC, React/TypeScript with `ts-rs` bindings. No new crates.

**Spec:** `docs/superpowers/specs/2026-11-22-context-aware-filing-design.md` (§8.5, §11, §12, §16 steps 5–6).

## North-star rules

This plan upholds the binding rules in `docs/superpowers/specs/2026-11-23-local-intelligence-north-star.md` (§3):
- **K1 (propose, approve, undo):** Every reunite move runs as an ordinary proposed `action_batches` batch with BLAKE3 `digest` verification before proposing (`context_run_reunite`), atomic rollback on first failure (`run_reunite`), `file_history` tracking, and one-click undo (`undo_reunite` routed through `filing_undo` / Recent batches). Nothing moves outside `execute_batch`.
- **K2 (deletes ask, 30-day quarantine):** Reunite only moves files back into their project, design shelf, or photo subject destination with `(2)` collision suffixing — it never deletes or overwrites files, and post-undo directory cleanup removes only newly emptied destination folders strictly inside configured roots.
- **K3 (local only):** Uses deterministic catalog/history SQL, normalized-name equivalence graphs (`learned_aliases`), and zero network calls or external models.
- **K5 (background budget):** `reunite_plans_conn` short-circuits immediately when no live filing `file_history` rows exist, uses existing `file_history`/`actions` indexes, and adds zero resident background threads.
- **D1 compatibility:** Reads and writes only existing single-store SQLite tables (`file_history`, `action_batches`, `actions`, `collections`, `collection_locations`, `collection_members`, `context_decisions`, `weekly_notes`), directly compatible with the upcoming SQLCipher + `sqlite-vec` unified store migration.

## Architecture & File Structure

| File | Responsibility |
|---|---|
| `crates/aegis-core/src/context/reunite.rs` | `SplitBatchRef`, `ReuniteItem`, `ReunitePlan`, `reunite_plans_conn` / `reunite_plans` (`Project` > `DesignSet` > `PhotoSubject` claim priority, historical `MoveHome` tracing, per-volume isolation), collision-safe `propose_reunite`, atomic `run_reunite` (syncs `collection_members` & `LearnedSubject`), `is_reunite_batch`, `undo_reunite` |
| `crates/aegis-core/src/context/mod.rs` | Exports `pub mod reunite;` and re-exports `reunite::*` types and functions |
| `crates/aegis-core/src/context/plan.rs` | Exposes shared `_conn` helpers (`list_roots_conn`, `volume_info_conn`, `projects_home_conn`, `design_projects_home_conn`, `existing_design_project_dir`, `join`, `parent`, `leaf`, `same_path`, `inside`, `safe_name`, `unique_file_in_dir`, `plain_reason`) so `plan` and `reunite` share one destination rule set |
| `crates/aegis-core/src/context/projects.rs` | Makes `PART_NAMES` `pub(crate)` for alias noise filtering |
| `crates/aegis-core/src/context/relate.rs` | `learned_aliases` (chronological `SameAs`/`NotSameAs` replay + union-find over normalized names `>= 3` chars), `aliases_match`, `identity_of_with_aliases` (confirmed when sharing files, `Unknown` in distinct parents when sharing 0 files) |
| `crates/aegis-core/src/context/recognise.rs` | Indexes `a:<group>` keys and evaluates candidate pairs via `identity_of_with_aliases` |
| `crates/aegis-core/src/filing/batches.rs` | Includes `reunite:%` batches in `recent_batches` as `"Put back together: <Name>"` |
| `crates/aegis-core/src/engine/note.rs` | Adds `#[serde(default)] pub projects: Option<String>` to `WeeklyNote` and populates multi-location project + reunite counts in `build` |
| `src-tauri/src/commands/context.rs`, `filing.rs`, `src-tauri/src/lib.rs` | Adds/registers `context_reunite_plans`, `context_run_reunite`, and routes `reunite:%` undo through `undo_reunite` |
| `src/bindings/` (`ReuniteItem.ts`, `ReunitePlan.ts`, `SplitBatchRef.ts`, `WeeklyNote.ts`, `index.ts`, `client.ts`) | Adds TypeScript bindings and `aegis.context.{reunitePlans, runReunite}` IPC helpers |
| `src/screens/Projects.tsx`, `ThisWeek.tsx`, `Filing.tsx`, `App.tsx`, `fixtures/` | Renders `"Put back together"` section with split-batch citation & Undo, `"Same project…"` picker on project cards, `"Put back together"` formatting in Recent batches, `"Projects"` summary section + deep-link in `ThisWeek` |
| `crates/aegis-core/examples/filing_dry_run.rs` | Prints `put back together` plans and weekly note `projects` line |
| `crates/aegis-core/tests/reunite_and_learning.rs` | 6 integration tests covering project reunite + `MoveHome` tracing + `undo_reunite`, `DesignSet`/`PhotoSubject` reunite + learning + no-loop, collisions & stale source refusal, `SameAs` learned aliases + `NotSameAs` revocation, and `WeeklyNote.projects` summary + legacy JSON deserialization |

## Live-DB Dry Run & Verification

- **Live-DB dry run (`examples/filing_dry_run.rs` on a temporary read-only `VACUUM INTO` copy, deleted afterward):**
  - `put back together (1)`: `ipadcover [PhotoSubject]` — `Puts 7 files back into C:\Users\Mac\Pictures\2026\ipadcover` (`[batches: 01a203ee (7)]` moved earlier into `Pictures\2026\2026-08`).
  - Zero false positives on already-reunited `SmoothLite` / `Back Cement` (`live_path` already inside project folder) or named album `Pictures\Undated\mugs`.
  - `weekly note projects line`: `Some("87 projects have copies in 2–20 places. 1 group has files from earlier batches to put back together.")`.
- **Workspace verification gate:**
  - `cargo test --workspace`: **496 passed, 0 failed, 1 ignored** (`usn_refresh_on_system_temp`), including all **6** tests in `crates/aegis-core/tests/reunite_and_learning.rs` (`0.30s`) and all **11** tests in `crates/aegis-core/tests/sets.rs` (`0.62s`); `C:\.aegis-quarantine` verified at `0` before and `0` after.
  - `cargo clippy --workspace --all-targets -- -D warnings`: clean (`0` warnings).
  - `cargo check -p aegis-desktop --features custom-protocol`: clean.
  - `pnpm build` (`tsc --noEmit && vite build`): clean.
