# Always-on engine: design

Project 2 of 5. Approved by the owner on 2026-11-21 ("nothing, proceed with the dev").

## 1. Intent

Aegis stops being an app you open and becomes something that looks after the PC by itself. It handles the ordinary chores people forget or do by hand: safety, tidiness, staying ahead of problems. It tells the owner only what matters, gently. The goal is the "I didn't know I needed this" reaction: Aegis notices things the owner would never have checked.

The owner's rule on interruptions is **"just a reminder is fine"**. Aegis may remind, and it never demands attention: no modal windows, no sound, no stream of notifications.

## 2. Goals

1. **Always running.** Aegis starts with Windows, lives in the tray and keeps working with the window closed. Closing the window destroys the web view, so the background process stays light (target: idle RSS ≤ 350 MB, from V1.5-05).
2. **An engine of small "watches".** Each watch is one chore. It runs on a schedule or reacts to an event, and reports findings. Projects 3 and 4 add watches without changing the engine.
3. **The starred watches ship in this project** (§6).
4. **One reminder system** with four levels (§5), a daily cap, and learning from dismissals.
5. **The Sunday note:** one calm weekly page.

## 3. Non-goals

- Auto-filing, renaming and folder blueprints (Project 3).
- SMART health, startup items, unused apps, temp and caches (Project 4). Space history is recorded from now on so that Project 4 has data.
- Explorer context menus, a CLI and a REST API (V1.5 backlog).
- macOS and Linux. The OS plumbing is Windows-only, behind `cfg(windows)`, with no-op fallbacks so the workspace still builds and tests elsewhere.

## 4. Anti-goals (a failure even if everything "works")

- **Nagging.** More than one reminder a day (urgent ones aside), sound, or stealing focus.
- **Deleting anything without asking.** Every janitor finding is a *proposal* that goes through the existing batch → approve → 30-day quarantine flow (owner decision).
- **Making the PC slower.** Heavy work runs only when the PC is idle, on mains power, and not in a game, full-screen video or presentation.
- **Two engines at once.** A second launch, such as auto-start plus a click, must hand over to the running instance. Today's Phase 1 bug showed what two writers on one database do.
- **Failing silently.** If a watch stops ticking, the self-check notices.

## 5. Reminder levels

| Level | Triggers in this project | Delivery |
|---|---|---|
| **Now** | Ransomware tripwire; read errors on a drive | Quiet Windows notification with no sound. A click opens the right screen. Repeats at most once a day while unresolved. Ignores the daily cap. |
| **When convenient** | Only-copy risk over the threshold; quarantine expiring within 2 days; Backup drive unseen for 14 days while there are unprotected bytes | Quiet notification. **At most one a day across all kinds.** Held back while the user is busy (full-screen, presentation, game) and shown at the next calm moment. |
| **Sunday note** | Everything else | One "Your weekly note is ready" notification. The note is a page in the app. |
| **Glance** | Always | Tray icon state: calm, waiting, or urgent. |

**Learning from dismissals:** when a reminder *kind* is dismissed (closed without acting) 3 times in a row, Aegis demotes that kind to the Sunday note and says so once in the next note ("I stopped reminding you about X; it's in your note now"). Undo is in Settings. Acting on a reminder resets the count. Urgent kinds can't be demoted.

**Persistence:** a `reminders` table, keyed by `(kind, subject)`. A subject is a folder, drive or batch. It stores `first_raised_at`, `last_shown_at`, `state` (open / resolved / snoozed), and per-kind `dismiss_streak` and `demoted`. Findings that are still true are re-raised idempotently and don't create duplicates.

## 6. Watches in this project

Each watch implements `name`, `cadence` (an interval, an event or both), `needs_idle` (bool) and `run(ctx) -> Vec<Finding>`. A `Finding` carries `kind`, `subject`, `level`, `title`, a one-line `detail`, an optional `bytes` count, and an `action` (which screen to open, or a proposed batch id).

1. **Live folder watcher (event).** Watches the PC roots (Downloads, Documents, Desktop, Pictures, Music, plus any root the owner added) live with the window closed. Changes are debounced (2 s) and fed to `index_paths` in **small batches** (≤ 500 paths per transaction) so UI commands never wait. This addresses the deferred "large USN replay in one transaction" issue for this path. It feeds the tripwire.
2. **Ransomware tripwire (event, urgent).** Counts changes per top-level root folder in a sliding 60-second window. When more than 200 files *and* more than 20 % of the folder change, it samples up to 20 of the changed files with `guard::looks_encrypted`. If at least 5 % look scrambled, or more than 100 files were renamed to an extension that is new in that folder, it trips:
   - It raises a **Now** reminder.
   - It sets a **backup lock**: the standing top-up is suspended for that folder (reusing the `GuardHalt` and override path), so the backup never copies the damage.
   - The lock clears only when the owner chooses "It was me" or "Back them up anyway" (the existing override).
3. **Only-copy alarm (daily, convenient).** Measures bytes in tiers 1 and 2 that are not protected on the Backup drive (`coverage_summary`) and days since the Backup drive was last seen. It raises a reminder when unprotected ≥ 1 GB and the Backup drive hasn't been seen for ≥ 7 days, or when unprotected > 0 and it hasn't been seen for ≥ 14 days. The text gives the amount and where it lives: "3.2 GB of new photos exist only on this PC."
4. **Quarantine expiry (daily, convenient).** Quarantine entries that purge within 2 days. "14 files are deleted for good on Friday. Keep any?" Opens Disk Rescue & Quarantine.
5. **Downloads janitor (daily, needs idle, note).** Each finding becomes a *proposed* quarantine batch, summarised in the note, never executed by itself:
   - **Abandoned partial downloads:** `.crdownload`, `.part`, `.partial` or `.download` files older than 24 h.
   - **Repeat downloads:** `name (n).ext` where the blake3 hash equals that of `name.ext` (or another `(m)` copy). The oldest is kept.
   - **Installers already used:** `.exe`, `.msi` or `.msix` files in Downloads older than 14 days. When the installer's name matches an installed program's DisplayName in the Uninstall registry keys (HKLM and HKCU, 64- and 32-bit views; a case-insensitive token match on the first word or two of the product name), the text says "already installed". Otherwise it says "not opened in N days".
6. **Space recorder and forecast (daily, note).** Records one `space_samples(volume_id, day, used_bytes, total_bytes)` row per online volume per day. It also records `root_samples(root, day, bytes)` for each PC root, which is cheap to sum from the index. After ≥ 7 samples, a least-squares fit over the last 28 days gives "C: fills up in about N weeks" (shown only when that's under 26 weeks) plus the fastest-growing root. A convenient-level reminder fires when the forecast is under 3 weeks.
7. **Resume interrupted catalog (event).** When an online drive has no `last_catalog_complete_at` and isn't being catalogued, and the owner has started a catalog of it before, the catalog resumes on plug-in. This closes the gap between Phase 1 spec §7.3 and the code. It goes through the same `InFlight` guard as manual catalogs.
8. **Drive read errors (event, urgent).** When a catalog or refresh reports ≥ 10 read or I/O errors on a drive, it raises a **Now** reminder: "WD C had trouble reading 14 files. Copy what matters off it soon." This is a cheap early warning ahead of Project 4's SMART checks.
9. **Self-check (hourly).** Each watch records `last_ok_at`. When an event watcher's heartbeat is older than 3× its cadence, the engine restarts it. If it fails again, the Sunday note reports it. It also notes when the database has grown more than 20 % in a week.

## 7. The engine

- **Scheduler.** One engine thread ticks every 30 s. It runs due watches in order. A watch with `needs_idle` runs only when the system is "calm":
  - no input for ≥ 2 min (`GetLastInputInfo`);
  - on AC power, or battery ≥ 50 % (`GetSystemPowerStatus`);
  - not busy: `SHQueryUserNotificationState` is not `QUNS_BUSY`, `QUNS_RUNNING_D3D_FULL_SCREEN` or `QUNS_PRESENTATION_MODE`.

  A watch that comes due while the system isn't calm waits up to 24 h and then runs anyway at low priority.
- **Jobs.** Long work (janitor hashing, catalog resume) runs on its own thread with its own `SqliteStore` connection, which is the existing pattern. Each job has an `InFlight` guard.
- **Pause.** From the tray: "Pause for 1 hour" or "Pause until tomorrow". Pausing stops idle work and convenient reminders. The watcher and the tripwire keep running, because safety never pauses.
- **Persistence.** The tables are `watch_state(name, last_run_at, last_ok_at, last_error)`, `reminders`, `space_samples`, `root_samples` and `weekly_notes(id, week_start, built_at, json, read_at)`. They're created by a schema migration, following the existing migration pattern.

## 8. Living in the tray

- **Single instance** (`tauri-plugin-single-instance`). A second launch focuses the existing window, or opens it if it is closed.
- **Start with Windows** (`tauri-plugin-autostart`). On by default after first run, with a toggle in Settings. Auto-start launches with `--background` and doesn't open a window.
- **Closing the window** destroys it, which unloads WebView2. `RunEvent::ExitRequested` is prevented, so the process stays alive. The first close shows a one-time line: "Aegis keeps watching in the tray. Right-click it to quit."
- **Tray icon:** the Aegis shield with a state dot (none = calm, amber = waiting, red = urgent). These are three variants of the existing app icon, not a generic icon. Tooltip: "Aegis: all calm", "Aegis: 2 things waiting" or "Aegis: needs you now".
- **Tray menu:**
  - Open Aegis
  - Search everywhere (Ctrl+Shift+Space)
  - This week's note
  - Back up now (only when the Backup drive is plugged in)
  - Pause ▸ 1 hour / until tomorrow / resume
  - Quit Aegis

  A left click opens the window.
- **Find anything, anywhere:** a global shortcut, Ctrl+Shift+Space by default and changeable in Settings (`tauri-plugin-global-shortcut`). It opens the window on the search screen with the box focused. Search covers every catalogued drive, including unplugged ones, and results show "on WD C, last seen 3 March". If registering the shortcut fails (it's taken), the tray menu says so and Settings offers another.
- **Notifications** (`tauri-plugin-notification`) are silent. Clicking one opens the matching screen through a deep-link payload. Caveat: Windows ties toasts to an installed app identity (AppUserModelID). A raw `target\release` exe may show them under a generic name or drop them, so notifications are verified on an installed (bundled) build.

## 9. The Sunday note

- Built on the chosen day at 18:00 (default Sunday, configurable) by the engine. When the PC is off at that time, it's built at the next calm moment.
- Stored as JSON in `weekly_notes`. Shown on a "This week" page that is reachable from the tray, from Home, and from the notification.
- **Sections, in order** (a section with nothing to say is left out):
  1. **Headline:** one sentence, such as "A quiet week. Everything important is backed up."
  2. **Needs you (max 3):** the most valuable pending proposals or questions, each with its action button.
  3. **What Aegis did by itself:** files indexed, backups topped up, drives refreshed, catalogs resumed.
  4. **Safety:** only-copy bytes and the Backup drive's last-seen date.
  5. **Space:** the forecast line and the fastest-growing folder.
  6. **Tidy-up found:** the janitor's totals, such as "2.1 GB of old installers and repeat downloads, ready to review".
  7. **Housekeeping about Aegis:** watches that failed or restarted, and reminder kinds demoted from notifications.
- The Home "Needs you" tile also gains a "This week" link.

## 10. UI changes

- A new **This week** screen.
- **Settings additions:** start with Windows, note day, the search shortcut, demoted reminder kinds (with undo), and pause state.
- **Home:** a "This week" link, and a "Paused until…" chip while paused.
- **A Now-level banner in the app:** the existing PlugBanner gains a `tripwire` kind with "It was me" and "Show me" buttons.

## 11. Approaches considered

- **A. The Tauri app is the engine (chosen).** One binary. The tray keeps the process alive and the window is created only on demand. It reuses all the existing code and the single-writer database pattern. Cost: when the owner quits from the tray, nothing watches until next login. That's acceptable, because it's what the owner chose.
- **B. A Windows service plus the UI as a client.** Runs even when logged out. But it needs an admin install and an IPC layer, and a service can't show notifications or a tray. Too heavy for a personal PC.
- **C. Task Scheduler runs.** Light, but it can't watch live, so the tripwire and the live index are impossible.

## 12. Risks and calls left to the builder

- Tripwire thresholds (200 files / 20 % / 5 % / 100 renames) are starting values. Keep them as constants in one place, and test them with synthetic bursts.
- Installer to installed-program matching is heuristic. When unsure, say "not opened in N days" rather than "already installed".
- Exact tray icon artwork: derive the variants from the existing icon. The owner will see them in screenshots.
- The order of work within the plan is the builder's call. Ship in slices that each leave the app working: engine core and tables → tray/single-instance/auto-start → reminders and notifications → watches → the note → UI.
