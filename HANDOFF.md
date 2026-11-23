# Handoff: Aegis Guardian

Updated 2026-11-23 (Plan 3 + Design-vs-Software separation built, verified, and fast-forward merged into `master`; D1 approved). Read this first in a new chat.

**The whole-project view** (what is done, what is waiting on the owner, everything that remains) is in
`docs/PROGRESS.md`. Keep it current at the end of each plan.

Current state:
- `master`: Projects 1–3, Context-aware filing Plans 1–3 (including Design-vs-Software shelf separation at `C:\Prj\Design\<Name>`, inline `Answer::SetKind` reclassification, Projects UI kind filter bar/badges, and `[features] custom-protocol` in `src-tauri/Cargo.toml`), fast-forward merged from `feature/sets-and-photo-subjects` (not pushed).
- `feature/sets-and-photo-subjects`: merged into `master` on 2026-11-23 (not pushed).
- `checkpoint/pre-stabilization-2026-11-23` (`084e3d5`): rollback tag preserving pre-stabilization baseline plus parked UI refresh docs (`canonical/D1_Design_System_v1.0.md`, `canonical/D3_Critical_UX_Flows_v1.0.md`, `canonical/suite_reconciliation.md`, `design-system/`).
- Leave the two untracked owner docs (`canonical/Aegis Guardian Full PRD.md` and `canonical/local-intelligence-system-architecture.md`) uncommitted and untouched.

## NEXT CHAT: do this first

**Owner-approved roadmap order (2026-11-23):**
1. **Context-aware filing Plan 4** ("Put back together", spec §11) → Plan 5 / remaining §16 modules (learning/weekly note §12, Software packages, Owner's folders, Old copies, Resources, Series)
2. **1. D1 Encryption + Embeddings & Hybrid Search** (SQLCipher whole-store + `sqlite-vec` + DPAPI/TPM `R2` + mandatory offline recovery key, then embeddings & FTS5+vector hybrid search inside the 350 MB K5 budget)
3. **Project 4: System care**

- Plan 3 doc: `docs/superpowers/plans/2026-11-23-sets-and-photo-subjects.md` (with mandatory North-star rules section, dry-run tuning rulings, and Design-vs-Software separation).
- **Owner Decision D1 approved (2026-11-23):** SQLCipher whole-store encryption + `sqlite-vec` inside the encrypted store (350 MB K5 budget), DPAPI/TPM-sealed engine key (`R2`, Windows Hello for sensitive views only), and mandatory offline 24-word/hex recovery key; owner also confirmed keeping `R1–R11` as written (including `R2` and `R6`).
- **Owner approved `SmoothLite` and `Back Cement`** on live PC (`C:\Prj\SmoothLite` and `C:\Prj\Back Cement` now await one-click Move into `C:\Prj\Design\*`).
- **Verification gate on `master`:**
  - `cargo test --workspace`: **487 passed, 0 failed, 1 ignored** (`usn_refresh_on_system_temp`), including all **11** tests in `crates/aegis-core/tests/sets.rs`, `recognise_is_fast_on_a_big_catalog`, and `migration_0002_applies_on_fresh_and_v1_databases`; `0` folders in `C:\.aegis-quarantine` before and `0` after.
  - Live-DB dry run (`examples/filing_dry_run.rs` on a temporary read-only `VACUUM INTO` copy): `256` projects (`87` multi-location), `1` remaining ungathered design set (`MBIZO 1edited` on `D:`), `5` photo subjects (`ipadcover` 12, `Mcgs Splash` 12, `Copilot` 7, `Designer` 4, `fragrances` 5), `53` project plans (`3.1s`) proposing `Back Cement -> C:\Prj\Design\Back Cement`, `SmoothLite -> C:\Prj\Design\SmoothLite`, and `Disst -> C:\Prj\Design\Disst` with zero software regressions and zero false `C:\Prj\Design` project cards.
  - `cargo clippy --workspace --all-targets -- -D warnings`: clean (`0` warnings); `cargo check -p aegis-desktop --features custom-protocol`: clean; `pnpm build`: clean (`dist/` built in `625ms`).

1. **Owner items still open:**
   - app rename choice (another "eagis" / Aegis app exists).
2. **Note on `src/bindings/` and `node:sqlite`:**
   - Keep `canonical/D1_Design_System_v1.0.md` parked until core features finish.
   - `cargo test` writes LF line endings into `src/bindings/*.ts`; run `git checkout -f --` on unchanged bindings so only real binding diffs are committed.

**What Plan 3 & Design-vs-Software differentiation built:**
- **Schema & Store:** Migration `0008_sets.sql` (`collections.fingerprint`, `collection_members`), `store::save_collections_with_members` (keeping set members out of `collection_locations`), `synthesize_set_locations`, live-path `list_members_with_collection`, `drives::registry::rebase_volume_paths` rebasing `collection_members.path`.
- **Recogniser (`context::sets`, `projects`, `recognise`, `answers`):** Multi-format artwork matching, false-positive filters, `DesignSet`/`PhotoSubject` classification, `DecisionKind::ProjectKind` + `Answer::SetKind`, `Ctx::is_kind_shelf` treating `<projects home>\Design` as a Design shelf while leaving nested `<CodeRepo>\Design` folders untouched, `ASSET_EXTS` expanded to 9 formats, `RecogniseReport::{design_sets, photo_subjects}`.
- **Plans & Filing (`context::plan`, `db::actions`, `filing`):** `StepKind::GatherFiles`, `kind_home` (`C:\Prj\Design\<Name>` for Design vs `C:\Prj\<Name>` for Software/Writing/Research/Other), `eligible_home_move` + kind-aware `already_home`, `follow_moves` preserving explicit `project_kind`, `unfollow_moves` + `after_undo` cleaning empty `Design` shelf parent folders, cross-root hoisted `protected_files`/`photo_subjects`, `LearnedSubject` catch on new single arrivals.
- **UI & Build (`src-tauri/Cargo.toml`, `Projects.tsx`, `fixtures/projects.ts`):** `[features] custom-protocol = ["tauri/custom-protocol"]` for standalone `dist/` builds, `DesignSet` IPC support, color-coded `KindBadge` icons, Design fuchsia left accent, kind filter pill bar with counts, inline `<select>` reclassification, `SmoothLite` fixture + updated `ts-rs` bindings.

**Earlier baseline (merged to `master` at `76708b8`):**
- Plan 1, Plan 2 (project plans), and post-Plan-2 stabilization (`c63e967`, `b9899b2`, `6a4afde`, doc reconciliation): `docs/superpowers/plans/2026-11-23-project-plans.md`.

**What Plan 2 built:**
- **Planner** (`context::plan`):
  - one plan per project per drive;
  - the main copy moves whole into `C:\Prj`;
  - identical and older copies go to quarantine. Every file is proven by BLAKE3 at run time; otherwise the whole copy is kept as a version;
  - copies changed on both sides, and earlier versions, go to `Versions\<date> <origin>`;
  - material goes to `Docs`, `Design` or `Material`;
  - a git repo home gets a sibling `<name> - material` folder;
  - a copy at the target name is moved aside first;
  - cards are ordered safest first (`rank`).
- **Executor** (`db/actions.rs`):
  - step order;
  - stale-folder check;
  - exact destinations (never ` (2)`);
  - a project batch stops at its first failure, and `plan::run` undoes it;
  - the catalog, `atomic_units` and `cache_dirs` follow moved folders at once;
  - undo replays chained steps in reverse;
  - folder quarantine takes its files' catalog state with it.
- **UI** (owner's design A):
  - a plan line under each place;
  - Keep here / Include in plan;
  - Change main copy;
  - Approve plan, which runs at once with Undo on the card;
  - a Move button on single projects in the compact list;
  - Recent batches shows "Project plan: <name>".
  - IPC: `context_plans`, `context_run_plan` (single-flight with recognition), `filing_undo` routing to `plan::undo`.
- **Rulings from the owner's real data:** they only make plans move less.
  - Nothing inside another project (or holding one) moves.
  - Python environments never move. Moving a folder that holds one warns.
  - Other drives get no plan yet (the Old copies module comes later).
  - A home moves in only from inside a projects folder.
  - Copies changed in the last 30 days stay.
  - The home keeps its own folder name.
- **Dry run on a copy of the live database:** 51 plans in 1.3 s. 41 homes move in; 1 copy goes to quarantine; 21 are kept as versions; 1 folder of material is brought in.
- **Rehearsal** (`examples/plan_rehearsal.rs`) on copies of the real forexTrader and DrivingSchool (117,044 files): both ran, and undo restored every file exactly.
- **Verified (post-stabilization):** `cargo test --workspace` **476 passed, 0 failed, 1 ignored** (`usn_refresh_on_system_temp`), with both timing tests (`recognise_is_fast_on_a_big_catalog`, `migration_speed_and_fts_sync`) passing and **0** new folders leaked into `C:\.aegis-quarantine` (`57` before / `57` after). Clippy and `pnpm build` are clean.
- **Nothing has moved on the owner's real folders.**

**Noticed, not fixed:**
- *(Resolved 2026-11-23 in `b9899b2`, `6a4afde`: `aegis-desktop` background RAM/CPU churn and real `C:\.aegis-quarantine` test leaks.)*
- UI baseline docs (`canonical/D1_Design_System_v1.0.md`, `canonical/D3_Critical_UX_Flows_v1.0.md`, `canonical/suite_reconciliation.md`, `design-system/`) are saved at `checkpoint/pre-stabilization-2026-11-23` (`084e3d5`). Leave untracked `canonical/Aegis Guardian Full PRD.md` and `canonical/local-intelligence-system-architecture.md` uncommitted.
- Copying the live database: no `sqlite3` or Python on this PC. Use `node` with `node:sqlite` (read-only `VACUUM INTO`); the script lived in the session scratchpad.

**North star:** `docs/superpowers/specs/2026-11-23-local-intelligence-north-star.md` holds the binding rules for every new spec or plan.

Ideas backlog below: ask the owner which to design after Plan 2.

**What Plan 1 built** (`crates/aegis-core/src/context/`):
- `profile` (per-folder stats from the catalog), `projects` (recogniser), `relate` (identity, relationships, fingerprints),
  `recognise` (full pass over PC roots and every catalogued drive except Backup), `answers`, `store`, `watch` (`ContextWatch`,
  runs before `FilingWatch`, at most every 10 min when the catalog or answers changed). Migration 0006.
- **Filing never splits a project:** `find_groups` keeps every project root (confidence 0.60 or more) whole; waiting batches are
  re-checked (`recheck_open_batch`, moves inside projects become `superseded`). The owner was told filing is safe to resume.
- **Projects screen** under Filing (owner chose layout C with B's highlights): confirm list, multi-place cards, compact list.
  IPC `context_projects`, `context_recognising`, `context_recognise_now` (single-flight), `context_answer`.
- Dry run (`filing_dry_run`) prints projects per container and every multi-place project with relationships.

**Measured on a copy of the owner's database (2026-11-22):** 253 projects, 86 in more than one place, 17.5 s (release);
14 of 14 turned-down project moves protected. Verification: `cargo test --workspace` 418 passed, 1 ignored (USN on C:);
clippy clean; `pnpm build` clean; Projects screen checked in the browser preview.

**Tuning rulings made on real data (owner may overrule):**
- Files and declared names found in projects of 3+ different names are boilerplate (starter kits): ignored for identity.
- Differently named folders are one project only with 80% of the smaller copy in common; same names need one distinctive file.
- "Possibly the same project" (name only) pairs only two folders that have nothing else.
- A drive's top level, Documents, Downloads and Pictures never lend "sits among other projects"; other containers lend it only
  when 40% of their sizeable sub-folders are projects by themselves. In such a container, a folder of 3+ files is asked about (0.60).
- An answer names one folder (same drive by id, same path or nearly the same files), never its copies.

**Known, not done:**
- Code exercise folders inside course folders (Udemy) count as small projects, so those courses are not moved to Videos\Courses.
- Loose files directly inside a container (e.g. `Dev Set` itself) are still filed by Fresh (spec §9, 2nd bullet).
- `D:\xampp`, Python venvs (`env38`), `Downloads\DDU …` are kept whole as "projects": right for protection; they belong to the
  later Software packages module. `D:\MSIDownloads` holds a `.git`, so it is a repo.
- On a card, "files" counts all non-build files while "shared" counts files of 1 KB or more; the copy is phrased from the
  relationship, not the numbers.
- The app shell is not responsive at phone width (pre-existing; the Projects screen itself is fluid).

**Owner's framing (2026-11-22):** *"Do not be blinkered into making the system about projects … it is a complete system for my PC."*
  - Projects is **one module** of a whole-PC context layer (spec §1A).
  - The other modules follow later, each with a short design of its own: Software packages, Owner's folders, Resources, Series, Photo subjects, Sets, Old copies.
- **Owner decisions in the spec:**
  - A git repo may move **whole** into `C:\Prj` when the owner approves its plan. The card warns about `.venv` and recent changes.
  - **All** projects, software or not (e.g. SmoothLite artwork), live in the one projects home, `C:\Prj` on the PC.
- **Measured facts the spec relies on (spec §2):**
  - C:\Prj is a root.
  - Copies of the same project relate in different ways: forexTrader (the `C:\Prj` copy is an older subset), DrivingSchool (changed on both sides), paperPilot (different material), wellness (no overlap).
  - 14 of the owner's 15 rejected moves (all except a Downloads `.zip`) moved project files by kind. They become regression tests.

## Owner's vision document (read 2026-11-22)

**2026-11-23:** the review below is now recorded as binding rules and a build order in
`docs/superpowers/specs/2026-11-23-local-intelligence-north-star.md`. Every new spec and plan must add a
"North-star rules" section. Owner decision D1 (encryption at rest and key handling) is due before the embeddings work.

`canonical/local-intelligence-system-architecture.md` (the owner's; untracked, **never commit it**) describes the 14 backlog
ideas below as one local-first "intelligence OS". Claude's review, given to the owner:
- Treat it as the north star, mapped onto Aegis, not a restart: Aegis already has the USN journal (read **without** admin),
  SQLite store, idle-gated scheduler, content reader, batches with undo, duplicates, the janitor, and now the context layer.
- Decide early (before more data piles up): encryption at rest and key handling. A key that needs Windows Hello would stop the
  tray engine (tripwire, backup) after a reboot; prefer a DPAPI/TPM-sealed key for the engine, biometrics only to open sensitive views.
- Avoid or reshape: direct MFT parsing (needs admin/elevation); multi-pass shredding (ineffective on SSDs; use BitLocker);
  a dead-man's switch that destroys data (release only, never auto-shred); registry cleaning; Recall-style screen capture;
  PII masking of the search index (would break finding one's own documents); "mathematical" zero-network claims.
- LLM routing ("Folderless") should compile plain-language intents into visible rules that feed the existing
  propose/approve/undo pipeline, not decide per file. Local LLMs run on demand only (the 350 MB background budget stands).
- Suggested order: Plan 2 (project plans), then embeddings + hybrid search (aegis-embed + FTS), then local RAG over it,
  then dead files / health score (largely exists), then knowledge extraction, then cross-project intelligence;
  CDO agent and estate management last. Each gets a short design of its own, as before.

## Ideas backlog (owner asked about these, 2026-11-22)

AI Folderless Mode, Digital Estate Management, Automatic Knowledge Extraction, Personal Data Governance,
Cross-Project Intelligence, Semantic Version Tracking, Dead File Detection, Forgotten Asset Recovery, Local RAG Engine,
Autonomous Research Librarian, Self-Healing Workspace, Context-Aware Search, Workspace Health Score, AI Chief Digital
Officer Agent.

- Each one was explained, with how it maps onto Aegis, in the last message of the design chat.
- Most build on the context layer, so it comes first.
- Not yet scheduled. Ask the owner which to design next once Plan 1 lands.

## Where things stand (2026-11-22)

- **Branch:** `master` only. Projects 1, 2 and 3 are all merged. Nothing is pushed.
  - The only untracked file is `canonical/Aegis Guardian Full PRD.md`. It belongs to the owner: **never commit it.**
- **Project 3, Auto-filing:** merged to `master` on 2026-11-22 (fast-forward to `fa3f120`), after 13 of the 14 deferred minor fixes:
  collision suffix ` (2)`, local-time dates, Windows reserved names, Tidy keeps digits, per-item folder suggestions
  under Documents, honest cached counts ("Plug in to see"), **Recent batches** with Undo (linked from the weekly note),
  home tile file count, safe change of structure mid-way, plain failure reasons, dry-run refuses the live database,
  the Backup drive is never read, and search filters (kind, category, organisation, year, drive).
  - Design: `docs/superpowers/specs/2026-11-21-auto-filing-design.md`; plan: `docs/superpowers/plans/2026-11-21-auto-filing.md`.
- **Verification on the merged master:** `cargo test --workspace` green (374 passed, 1 ignored on purpose: the USN test on C:),
  `cargo clippy --workspace --all-targets` clean, `pnpm build` clean. The Filing screens were checked in the browser preview.
- **Dry-run tool:** `cargo run --release -p aegis-core --example filing_dry_run -- <COPY of aegis.db>`.
  Make the copy with the SQLite backup API first. It now refuses the live database and any app data folder.
  Its times are UTC (the core only knows local time when the desktop sets it).

### Rulings made in the minor-fix pass (owner may overrule)

- Collisions start at ` (2)` for **every** move and copy-back, not only filing (one convention, like Explorer).
- Screenshots are renamed only when the date comes from the name, EXIF or content, never from the modified date.
- Reserved names get a trailing `_` (`CON` → `CON_`); empty music tags fall back to "Unknown artist/album".
- Undo, Skip and new arrivals clear the cached counts; they are re-planned on the next visit.
- Undone moves stay "declined" (existing design), so undo does not offer those files again.
- Changing structure mid-way marks the waiting batch `superseded` (not a "no" for the files or their rules).
- The weekly note links "Filed N files by itself" by matching that wording.

## Do next, in this order

0. **Plan 2 of context-aware filing** (project plans; see the top of this file). Plan 1 is done.
1. **Dry run with the external drive (deferred item 14).** Plug in D:, copy `aegis.db`, run the dry-run tool on the copy.
   Report sample renames and the to-sort share; fix misclassifications test-first.
2. **Try Filing on the PC**, starting with the Fresh structure; then Recent batches → Undo once to see it work.
3. **Owner checks that need the installed build.** Quit the running `target\release` Aegis first. Then:
   1. `pnpm tauri build`, then install the NSIS installer.
   2. Notifications show under "Aegis Guardian"; clicking one opens the right screen, also from Action Center.
   3. Ctrl+Shift+Space opens Search. After a reboot, Aegis starts in the tray with no window.
   4. Background memory with the window closed is at most 350 MB.
4. **Phase 1 numbers.** Re-plug D: and report catalog time, fingerprint time, file count, duplicate groups, re-plug refresh mode and time.
5. Then Project 4 (System care).

### Small follow-ups noticed while fixing (not done)

- Search filters only apply to a typed query (the backend returns nothing for an empty query), and changing a filter
  needs another press of Search.
- The filing watch only notices arrivals on scopes that have had a batch approved; new files on a re-plugged drive
  don't clear its cached counts.

## What Project 2 does

- **Running all the time:**
  - Tray icon (the shield with a status dot), and a single instance.
  - Starts with Windows (release builds only).
  - Closing the window keeps the engine running; the webview unloads.
  - Ctrl+Shift+Space opens search from anywhere.
- **Two engine threads, each with its own database connection:**
  - Event thread: live index of the PC roots and the ransomware tripwire.
  - Scheduler thread, below normal priority. It runs the watches:
    - only-copy alarm
    - quarantine expiry
    - drive read errors
    - self-check
    - space recorder and forecast
    - Downloads janitor (proposals only)
    - tripwire lock reminder
  - The scheduler also builds the weekly note, delivers reminders and sets the tray state.
- **Reminders:**
  - "Now" reminders show at once.
  - "When convenient" reminders: at most one a day, held while the owner is busy or Aegis is paused.
  - Dismissing a kind of reminder 3 times moves it to the note.
  - Clicking or closing a notification counts as acting on it or as "not now".
- **UI:**
  - A **This week** screen and a **Settings** screen.
  - A tripwire banner with "It was me". It also appears after a restart while a folder is locked.
  - A "This week" link and a pause chip on Home.

### Measured (debug build, scratch database)

- Window closed: 33–56 MB, 0 webview processes.
- A second launch hands over to the running copy.
- Live test with 300 files:
  - Creating them: indexed, no alarm.
  - Encrypting them to `.locked` and deleting the originals: alarm, lock and notification.
- Janitor dry run on the owner's real Downloads (proposals went to a scratch database): 7.5 GB found, including 25 installers for programs already installed.

## Deferred minor issues

**From the Project 2 final review:**

- The pending navigation slot gets set even when the window already exists, so a later "Open Aegis" can land on an old screen.
- The tray label hard-codes "Ctrl+Shift+Space". Saving an invalid shortcut also unregisters the working one.
- `mark_interrupted_jobs` runs after the other threads have started (a small race at startup).
- The installer-name matcher accepts a match on the first word alone for names of 3+ words, so it can mislabel an installer as "already installed".
- A Windows rename counts both its From and To events toward the tripwire's 200-file threshold.
- Copy fixes:
  - "Paused until 07:00" doesn't say which day.
  - The janitor title is always "old installers and repeat downloads".
  - The only-copy title doesn't say where the files are.
- A locked root's halt cards read "(0 of N)" and repeat for each sub-folder.
- A busy error while reading `watch_state` or writing reminders aborts the rest of that tick.
- The engine's 24 h grace clock for heavy first runs lives in memory, so it resets when the app restarts.
- Clicking a notification from Action Center after the popup has gone probably lands on Home. Check this on the installed build.

**Still open from Project 1:**
*(Resolved in `6a4afde`: `.aegis-partial` on-disk BLAKE3 re-hash, `path_hash` `volume_id` filter, duplicate keeper radio `keeper_index` wiring, and extended `rebase_volume_paths`.)*

- A `backup_jobs` row stays `running` after an error.
- The first top-up isn't shown for approval.
- There are two verified-copy implementations.
- `index_paths` applies a large USN replay in one transaction. The live index now batches 500 paths per transaction, but the USN refresh path doesn't yet.

## Programme (owner-approved order)

1. Drives & duplicates (done, merged)
2. Always-on engine (done, merged)
3. Auto-filing (done, merged 2026-11-22):
   - blueprint choice
   - learn-then-take-over
   - naming as `<date> <who/what> <type>`
   - migration in batches
4. System care: temp files, caches, installers, unused apps, startup items, SMART drive health, disk-space trends. Space history is now recorded daily by the engine.
5. Visual app, which grows alongside the others.

Owner decisions:

- learn-then-take-over autonomy
- deletes always ask and go to the 30-day quarantine
- one Backup drive, kept as a mirror with a 90-day window
- start with the PC folders
- a command-centre home
- **reminders, not interruptions ("just a reminder is fine")**
- **the "I didn't know I needed this" factor**

## Working notes for this machine

- **Shell text gets mangled.** Heredocs and Python heredocs halve backslashes. Write Rust/TS with Write or Edit; heredocs only for content without backslashes.
- **Line-ending noise in bindings.** `cargo test` rewrites `src/bindings/*.ts` with LF. Commit only files with real content changes (`git diff -w --ignore-cr-at-eol`).
- **Test runs never touch the owner's data.** Set `AEGIS_DATA_DIR=<scratch>` to give the app its own database and WebView2 profile. Without it, a second copy fails with WebView2 0x800700AA while the owner's copy runs.
- **A background git (the editor) briefly holds `.git/index.lock`.** Batch git commands and retry.
- **A Vite server is often already running on 1420.** Reuse it.
- **The owner often runs `target\release\aegis-desktop.exe`.** Stage only your own files, and don't rebuild release while it runs.
