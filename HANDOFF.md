# Handoff: Aegis Guardian

Updated 2026-11-22 (after designing context-aware filing). Read this first in a new chat.

## NEXT CHAT: do this first

**Execute Plan 1 of context-aware filing:** `docs/superpowers/plans/2026-11-22-context-layer-projects.md`.

- **Spec:** `docs/superpowers/specs/2026-11-22-context-aware-filing-design.md`. The owner approved it ("the plan is fine").
- **Branch:** create `feature/context-layer` from `master` before Task 1.
- **Process:** use the executing-plans approach (inline), with a ledger in `.superpowers/sdd/2026-11-22-context-layer-projects/`.
- **Why it matters:** the owner has **stopped approving filing batches that touch project folders** until Task 4 (protection) lands. Tell them as soon as it does.
- **Before Task 9:** show the owner a mockup of the project card. They pick visual options themselves.
- **Owner's framing (2026-11-22):** *"Do not be blinkered into making the system about projects … it is a complete system for my PC."*
  - Projects is **one module** of a whole-PC context layer (spec §1A).
  - The other modules follow later, each with a short design of its own: Software packages, Owner's folders, Resources, Series, Photo subjects, Sets, Old copies.
- **Owner decisions in the spec:**
  - A git repo may move **whole** into `C:\Prj` when the owner approves its plan. The card warns about `.venv` and recent changes.
  - **All** projects, software or not (e.g. SmoothLite artwork), live in the one projects home, `C:\Prj` on the PC.
- **Measured facts the spec relies on (spec §2):**
  - C:\Prj is a root.
  - Copies of the same project relate in different ways: forexTrader (the `C:\Prj` copy is an older subset), DrivingSchool (changed on both sides), paperPilot (different material), wellness (no overlap).
  - 14 of the owner's 15 rejected moves (all except a Downloads `.zip`) moved project files by kind. They become regression tests.

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

0. **Plan 1 of context-aware filing** (see the top of this file).
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

- The `.aegis-partial` file isn't re-hashed before the rename.
- `path_hash` lookups lack a volume filter.
- A `backup_jobs` row stays `running` after an error.
- The first top-up isn't shown for approval.
- The keeper radio button in duplicate review isn't sent to the backend.
- `rebase_volume_paths` misses some tables.
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
