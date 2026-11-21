# Handoff: Aegis Guardian

Updated 2026-11-21. Read this first in a new chat.

## Where things stand

- **Branch:** `feature/drives-dupes`, 20 commits ahead of `master`, all committed, working tree clean. The only untracked file is `canonical/Aegis Guardian Full PRD.md`, which belongs to the owner. **Never commit it.**
- **Verification:**
  - `cargo test --workspace`: 166 passed. One test is ignored on purpose (`usn_refresh_on_system_temp`, which needs a real NTFS change journal; it passed when run manually on C:).
  - `cargo clippy --workspace --all-targets`: clean.
  - `pnpm build`: clean.
- **License:** AGPL-3.0-or-later. The `LICENSE` file was added and the manifests were updated.
- **Project 1 of 5, Drives & Duplicates, is built and has been through a whole-branch review:**
  - Design: `docs/superpowers/specs/2026-11-21-drives-and-duplicates-design.md`
  - Plan: `docs/superpowers/plans/2026-11-21-drives-and-duplicates.md`
  - Standing-approval exception: amendment §6 in `canonical/C4_Policy_Engine_Confidence_Spec.md`

## Do next, in this order

1. **Merge to master.** The owner approved this. `git checkout master`, then `git merge feature/drives-dupes`, then run `cargo test --workspace` and `pnpm build` on the merged result. Delete the branch once that is green. Do not push unless asked.
2. **Fix the preview cards.** In the browser preview, `src/screens/fixtures/drives.ts` already marks Seagate B as `role: "backup"`. That hides the Drive plan card, because `src/screens/Drives.tsx` only shows the card when no Backup drive exists or some drive is unassigned. The owner wants to see **both** the Drive plan card (the recommendation) and the Backup card with its priority-tier bars.
   - Make the preview show both. For example: a preview-only toggle, or fixtures with one drive still unassigned, so the plan card renders next to an existing backup.
   - Keep the real app's logic correct.
   - Verify with Playwright screenshots from a Vite dev server on port 1420. Delete `.playwright-mcp/` afterwards.
3. **Owner checks:** the owner reviews the screenshots of Home, Drives & Backup and Duplicates. Then the owner plugs in one real external drive and clicks **Catalog**. Report catalog time, fingerprint time, file count, duplicate groups, and the re-plug refresh mode and time.

## Deferred minor issues (from the final review)

- The `.aegis-partial` file is not re-hashed before the rename (`backup/topup.rs`).
- `path_hash` lookups in the top-up retire step and in `copy_survivor_for` lack a volume filter.
- A `backup_jobs` row stays `running` after an error mid-copy, and owner-approved runs don't auto-resume.
- "Back up now" runs the first top-up without showing its item list for approval. The spec says the first top-up should be shown first.
- The keeper radio in Duplicates isn't sent to the backend, so keeper rule 1 ("the location the owner chose") is not implemented.
- `index_paths` applies a large USN replay in one transaction, which can make UI commands wait.
- `rebase_volume_paths` doesn't rewrite `backup_versions.source_path`, the roots in settings, or `scan_jobs`.
- There are two verified-copy implementations (the `actions.rs` Copy arm and `topup::copy_verified`). They should become one helper.

## Programme (owner-approved order)

1. Drives & duplicates (done)
2. Always-on engine: tray app, start with Windows, watchers, idle scheduling, weekly note
3. Auto-filing:
   - Show the blueprint choice: a tidied-up version of the owner's folders next to a fresh structure.
   - Learn-then-take-over: three approvals make a rule automatic; one rejection reverts it.
   - Naming: `<date> <who/what> <type>`.
   - Migration happens in batches.
4. System care: temp files, caches, installers, unused apps, startup items, SMART drive health, disk-space trends
5. Visual app: command-centre home, library grid, Ask bar, timeline. This grows alongside the others.

Owner decisions already made:

- learn-then-take-over autonomy;
- deletes always ask and go through the 30-day quarantine;
- one Backup drive, kept as a mirror with a 90-day safety window; the other drives are General;
- start with the PC's Downloads, Documents, Desktop, Pictures and Music, then gradually offer OneDrive and other drives;
- command-centre home, with the library one click away.

## Working notes for this machine

- **Shell text gets mangled.** Bash heredocs mangle backslashes in Python and Rust strings. For non-trivial edits, write the Python edit scripts to the scratchpad with the Write tool, or use Edit.
- **Long jobs use their own database connection.** The watcher, catalog and top-up threads each open their own `SqliteStore` on the same database file (`AppState.db_path`).
- **Bindings regenerate during tests.** `cargo test` regenerates `src/bindings/*.ts` through ts-rs. Commit those changes with the Rust changes.
- **The owner may be running the app.** Stage only your own files.
