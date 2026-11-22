# Handoff: Aegis Guardian

Updated 2026-11-21 (evening). Read this first in a new chat.

## IN PROGRESS: Project 3, Auto-filing (read this first)

- **Branch:** `feature/auto-filing`. Project 2 is merged to master.
- **Design:** `docs/superpowers/specs/2026-11-21-auto-filing-design.md`
- **Plan:** `docs/superpowers/plans/2026-11-21-auto-filing.md`
- **Ledger with rulings:** `.superpowers/sdd/2026-11-21-auto-filing/progress.md` (git-ignored; keep it).
- **Built:** all 12 tasks.
  - The whole branch had a fresh review, and every critical and important finding was fixed.
  - `cargo test --workspace` and clippy are green; `pnpm build` is clean.
  - Not merged and not pushed; that waits for the owner.
- **Ledger copy:** `<scratchpad>\ledger-auto-filing.md`. The rulings and deferred minors are also in the final chat message.
- **Dry-run tool:** `cargo run --release -p aegis-core --example filing_dry_run -- <COPY of aegis.db>`.
  - Make the copy first with the SQLite backup API (see `scratchpad/copy_db.py`).
  - Never point it at the live database: it writes metadata into whatever database it is given.
- **Owner next:**
  1. Merge.
  2. Plug in the external drive and re-run the dry run. The drive holds most personal documents; the first run only read the PC.
  3. Try Filing on the PC first, starting with the Fresh structure.

## Where things stand

- **Branch:** `feature/always-on-engine` (Project 2), branched from `master` at `102ac12`. All work is committed and the tree is clean.
  - The only untracked file is `canonical/Aegis Guardian Full PRD.md`. It belongs to the owner: **never commit it.**
  - Not merged and not pushed. Merging waits for the owner.
- **Project 1, Drives & duplicates:** merged to `master` on 2026-11-21.
  - Also on master: the fix that makes Catalog run once per drive (`234e231`).
  - That fix adds live progress, and a guard so a second click doesn't start a second catalog.
- **Project 2, Always-on engine:** built.
  - It has had a whole-branch review by a fresh reviewer, and every critical and important finding was fixed.
  - Design: `docs/superpowers/specs/2026-11-21-always-on-engine-design.md`
  - Plan: `docs/superpowers/plans/2026-11-21-always-on-engine.md`
  - The ledger (`.superpowers/sdd/…`, git-ignored) was deleted at the end. The rulings and deferred minors are in this file.
- **Verification:**
  - `cargo test --workspace` is green (engine 67, backup 34, desktop 10, plus the rest). One test is ignored on purpose (the USN test on C:).
  - `cargo clippy --workspace --all-targets` and `pnpm build` are clean.

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

## Do next, in this order

1. **Owner reviews** the screenshots (This week, Settings, Home with the tripwire banner) and decides on merging `feature/always-on-engine` to `master`.
2. **Owner checks that need the installed build.** Quit the running `target\release` Aegis first: `cargo build --release` can't overwrite a running exe. Then:
   1. `pnpm tauri build`, then install the NSIS installer.
   2. Notifications show under "Aegis Guardian". Clicking one opens the right screen even with the window closed. Also try one clicked later from Action Center.
   3. Ctrl+Shift+Space opens Search.
   4. After a reboot, Aegis starts in the tray with no window.
   5. In Task Manager, background memory with the window closed is at most 350 MB.
3. **Phase 1 numbers.** The D: catalog took about 39 min for 194,682 files, but that was inflated by five duplicate runs, a bug that is now fixed. Re-plug D: on the new build and report catalog time, fingerprint time, file count, duplicate groups, and the re-plug refresh mode and time.
4. Then Project 3 (auto-filing).

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
2. Always-on engine (built, awaiting the owner's merge)
3. Auto-filing:
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
