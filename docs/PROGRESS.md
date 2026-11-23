# Aegis Guardian: progress and what remains

Updated 2026-11-23. This is the whole-project view. `HANDOFF.md` covers the working detail for the next chat.

## 1. Where the code is

| Branch | State |
|---|---|
| `master` | Projects 1–3 and Plan 1 of context-aware filing (merged 2026-11-23). Not pushed. |
| `feature/project-plans` | Plan 2 of context-aware filing, done (16 commits). **Not merged, not pushed.** Waiting for the owner. |

Verification on `feature/project-plans`:
- `cargo test --workspace`: 460 passed, 1 ignored (the USN test on C:).
- Two timing tests (`recognise_is_fast_on_a_big_catalog`, `migration_speed_and_fts_sync`) fail only while the owner's `aegis-desktop` is using about 2 CPU cores. Both passed earlier on the same code.
- Clippy and `pnpm build` are clean.

## 2. Done

### Programme (owner-approved order)

| # | Project | State |
|---|---|---|
| 1 | Drives & duplicates | Done, merged |
| 2 | Always-on engine: tray, scheduler, watches, reminders, weekly note | Done, merged |
| 3 | Auto-filing: blueprints (Tidy/Fresh), learn-then-take-over, naming, batches with undo | Done, merged 2026-11-22 |
| 3a | Context-aware filing, Plan 1: project recognition, filing never splits a project, Projects screen | Done, merged 2026-11-23 |
| 3b | Context-aware filing, Plan 2: project plans (move into `C:\Prj`, quarantine/versions/material, one batch per project with undo) | Done on `feature/project-plans` |
| 4 | System care | Not started |
| 5 | Visual app (grows alongside the others) | Ongoing |

### Plan 2 in one paragraph

Every project gets a plan per drive. The main copy moves whole into `C:\Prj`:
- identical and older copies go to quarantine, once every file is proven in the main copy by BLAKE3 (otherwise the whole copy is kept as a version);
- copies changed on both sides go to `Versions\<date> <origin>`;
- other material goes to `Docs`, `Design` or `Material`.

One approval runs it as one batch, with one undo. Safety rules from the owner's real data:
- nothing inside another project moves;
- Python environments never move;
- other drives get no plans yet;
- only homes inside a projects folder move in;
- copies changed in the last 30 days stay.

It was rehearsed on copies of the real forexTrader and DrivingSchool (117,044 files): undo restored every file exactly.

### Documents

- Specs: `docs/superpowers/specs/`. The context-aware filing design is `2026-11-22-context-aware-filing-design.md`.
- Plans: `docs/superpowers/plans/`. Plan 2 has "as built" notes and rulings per task.
- North star and binding rules for every future design: `docs/superpowers/specs/2026-11-23-local-intelligence-north-star.md`.

## 3. Waiting on the owner

1. Merge `feature/project-plans` into `master`?
2. Try the Projects screen on the PC: forexTrader's card first, then one Move from the compact list, then Undo once.
3. **D1, encryption at rest and key handling.** Needed before the embeddings/search work. Recommended: SQLCipher with a key sealed to the PC (DPAPI/TPM); Windows Hello only for sensitive views.
4. Delete 53 orphaned test folders in `C:\.aegis-quarantine` (0.3 MB; none referenced by the live database)?
5. Which comes after Plan 2 (see §4).

## 4. What remains

### Context-aware filing (spec §16 build order)

| Step | What | State |
|---|---|---|
| 1–2 | Protection, Projects list | Done (Plan 1) |
| 3 | Project plans | Done (Plan 2) |
| 4 | Sets and photo subjects: SmoothLite artwork as a design project; photos grouped by subject inside a year | Next candidate (Plan 3) |
| 5 | Put back together: undo earlier batches' scattering of project/set files, via `file_history` | To do |
| 6 | Learning wired through (aliases from "same project", learned set subjects) and the weekly-note line ("6 projects have copies in 2–3 places") | To do |
| 7 | Other modules, each with a short design: Software packages, Owner's folders, Old copies (drive D: plans belong here), Resources, Series | To do |

### Programme

- **Project 4, System care:**
  - temp files, caches, installers;
  - unused apps (uninstall through Windows' own uninstaller only; **no registry cleaning**);
  - startup items;
  - SMART drive health;
  - disk-space trends (daily space history is already recorded).
- **Owner checks that need the installed build:**
  - `pnpm tauri build` and install;
  - notifications under "Aegis Guardian" open the right screen;
  - Ctrl+Shift+Space opens Search;
  - starts in the tray after a reboot;
  - at most 350 MB in the background.
- **Filing:**
  - dry run with drive D: plugged in (deferred item 14);
  - try Filing on the PC with the Fresh structure.
- **Phase 1 numbers with D: re-plugged:**
  - catalog time and fingerprint time;
  - file count and duplicate groups;
  - refresh mode and time.

### Vision (north-star order, each with its own short design)

1. Embeddings + hybrid search (FTS5 + vectors). Needs D1 first.
2. Local RAG over it (models on demand only).
3. Dead files and workspace health score (largely in System care).
4. Forgotten asset recovery.
5. Semantic version tracking (builds on the `Versions` folders).
6. Knowledge extraction.
7. Research librarian and cross-project intelligence (work from files on disk, not the screen).
8. Folderless mode: plain-language requests become visible filing rules.
9. Self-healing workspace.
10. Chief Digital Officer agent and digital estate (no destructive dead-man's switch).

## 5. Known issues and follow-ups

**Noticed 2026-11-23:**
- The owner's running `aegis-desktop` used about 2 cores continuously and about 1 GB of memory (budget: 350 MB). Investigate.
- Older test files (not the plan tests) still write into the real `C:\.aegis-quarantine`. Give them a private quarantine, as the plan tests now have.
- Two timing tests are sensitive to machine load. Consider a looser bound or marking them `#[ignore]` with a manual run.
- Project plans' "files moved" counts catalogued files only (`node_modules` and `.venv` contents aren't catalogued).
- Copies of a project on a drive without its home have no relationship to each other; they wait for the Old copies module.

**Filing follow-ups:**
- Search filters only apply to a typed query, and changing a filter needs another press of Search.
- The filing watch only notices arrivals on scopes that have had a batch approved.
- Code exercise folders inside course folders count as small projects, so those courses are not moved to `Videos\Courses`.

**Deferred from Project 2:**
- The pending-navigation slot can land on an old screen.
- The hard-coded shortcut label.
- Saving an invalid shortcut unregisters the working one.
- The `mark_interrupted_jobs` race at startup.
- The installer-name first-word match.
- Rename events counted twice by the tripwire.
- Copy fixes: the pause day, the janitor title, where only-copy files are.
- Halt cards "(0 of N)".
- A busy error aborts a tick.
- The 24 h grace clock resets on restart.
- Action Center click after the popup has gone.

**Deferred from Project 1:**
- `.aegis-partial` isn't re-hashed before the rename.
- `path_hash` lookups lack a volume filter.
- A `backup_jobs` row stays `running` after an error.
- The first top-up isn't shown for approval.
- The keeper radio in duplicate review isn't sent to the backend.
- `rebase_volume_paths` misses tables.
- There are two verified-copy implementations.
- A large USN replay runs in one transaction.

## 6. Owner decisions on record (keep)

- Learn-then-take-over autonomy. Deletes always ask and go to the 30-day quarantine.
- One Backup drive (mirror, 90-day window). The Backup drive is never read.
- Reminders, not interruptions. The "I didn't know I needed this" factor.
- All projects, software or not, live in `C:\Prj`. A git repo may move whole when its plan is approved.
- Projects are one module of a whole-PC context layer, not the whole system.
- Default licence AGPL-3.0.
- Never commit the owner's `canonical/` documents.
