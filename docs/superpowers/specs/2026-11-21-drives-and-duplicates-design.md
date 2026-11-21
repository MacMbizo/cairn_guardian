# Drives & Duplicates — Design

Date: 2026-11-21 · Status: draft for owner approval · Project 1 of 5 in the "orderly PC" programme

## 1. Intent

The owner has a PC plus at least three 1 TB external drives holding mixed content, with backup copies scattered randomly across them. They want Aegis to:

- recognise each drive every time it is plugged in;
- remember what is on a drive while it is unplugged (a "snapshot"), so work is never redone;
- find exact copies across all drives and the PC and ask the right question about them, e.g. "this is an exact copy of what's on Drive C — backup, or delete?";
- turn one drive into a dedicated **Backup drive** and leave the others as general storage.

The aim is zero friction. Decisions are made per folder or drive, never per file. Re-plugging a drive takes seconds. Nothing is lost through a mistake.

### Where this sits in the programme

The five projects are:

1. Drives & duplicates (this doc)
2. Always-on engine: tray app, watchers, idle scheduling, weekly note
3. Auto-filing: blueprint choice, classifier, naming, learn-then-automate rules
4. System care: junk, installers, unused apps, startup, SMART, disk trends
5. Visual app: command-centre home, library grid, Ask bar, timeline. It grows with each project.

The owner chose this order. The global decisions already made apply here:

- learn-then-take-over autonomy;
- deletes always ask and go through the 30-day quarantine;
- every action can be undone;
- the home screen is a command-centre dashboard (mockup option A), with the library grid (option B) one click away.

## 2. Goals

- **G1 Drive identity.** Each drive is recognised reliably across replugs, letter changes, renames and reformat-free use. Cloned drives are detected and told apart.
- **G2 Offline catalog.** A drive's files stay searchable and usable for duplicate matching while it is unplugged. They are clearly marked "on Seagate A (unplugged, last seen 3 Mar)". Files are never marked missing just because their drive is unplugged (PRD FR-001.8).
- **G3 Fast re-plug.** A previously catalogued drive is brought up to date in seconds to a few minutes, by examining only what changed.
- **G4 Fast first catalog.** A 1 TB first catalog avoids reading every byte. It can be interrupted (unplugged) and resumes.
- **G5 Cross-drive duplicates, decided in bulk.** Exact copies are grouped by folder where possible. Each group gets one question, and the answer becomes a remembered decision.
- **G6 Drive roles.** Each drive is General, Backup or Unassigned. After the first catalog, the app recommends a Backup drive and shows a drive plan for one-time approval.
- **G7 Backup with a safety window.** The Backup drive mirrors the chosen content. Files deleted or overwritten at the source go to `_Previous versions\<date>\` on the Backup drive and are pruned after 90 days. Backups are plain files, browsable without the app.
- **G8 Priority fill.** When the Backup drive can't hold everything, it fills by priority and reports what is unprotected.

## 3. Non-goals for this project

- **Sorting content into a folder structure or renaming files.** That is Project 3. Here, general drives keep their layout. Only duplicates, the drive plan and the backup change.
- **Background watching while the app is closed, tray icon and Windows toasts.** That is Project 2. In this project, plug-in detection works while the app is open.
- **Near-duplicates** (similar photos, edited versions) and **SMART health.** These belong to Projects 3/4.
- **Cloud backup and NAS/SMB as backup targets.**
- **Non-Windows platforms.** Unix code paths must keep compiling, but drive features may return "unsupported" there.

## 4. Anti-goals (failure even if it "works")

- Data loss of any kind. That includes losing the last copy of a file, a deletion based on a stale snapshot, and a backup that silently mirrors a ransomware or mass-delete event.
- Asking thousands of per-file questions.
- Re-reading a whole 1 TB drive on every plug-in.
- A backup only the app can read (proprietary archives, chunk stores).
- Treating "drive unplugged" as "files deleted".

## 5. Current state (from code reading, 2026-11-21)

- **Volume identity.** `volumes.id = "win-{serial:08x}"` from `GetVolumeInformationW`, with a BLAKE3(mount point) fallback when the serial is 0 (`scanner/os_adapter.rs:279-283`). No volume GUID or label is collected. Serials change on reformat and are duplicated by cloning.
- **Online/offline tracking.** `volumes.status` supports `online|offline|degraded`, but nothing ever writes `offline`. `resolve_online_volume` (`db/files.rs:75`) only considers online volumes.
- **Hashing.** The scanner hashes every file in full (`scanner/mod.rs:246`, streamed BLAKE3) at tier ≥2. `unchanged_hash` reuses the hash when size and mtime match.
- **Dedupe.** `exact_dedupe_report` (`db/actions.rs:1371`) groups by `blake3` across volumes. It keeps the shortest path, then the most recently accessed copy. It is per file only, with no folder grouping. `propose_dupe_quarantine` quarantines the non-keepers through the normal batch flow.
- **Change detection.** None (no USN, no watcher). The scan is a full walk.
- **Safety.** The action batch model (propose → approve → execute → rollback), per-volume quarantine directories and the disk-first/DB-commit/undo-on-failure pattern (`FsOutcome`) all exist and are reused.

## 6. Approaches considered

**Drive identity**

| Option | Trade-off |
|---|---|
| Serial number only (today) | Breaks on reformat; clones collide. |
| Volume GUID (`\\?\Volume{…}`) | Stable per partition on this PC, but Windows assigns it per machine. It's a different ID on another PC, and a re-partition changes it. |
| **Passport file on the drive + GUID/serial fallback (chosen)** | Travels with the drive and survives letter changes. Clones are detectable because two online drives would share one passport. Needs a writable drive; read-only drives fall back to a fingerprint. |

**Change detection on re-plug**

| Option | Trade-off |
|---|---|
| Full re-walk and re-hash | Simple; hours per drive. |
| Walk plus size/mtime check (existing `unchanged_hash`) | Minutes per drive, works on every filesystem. Still walks all directories. |
| **NTFS USN change journal, falling back to walk plus size/mtime (chosen)** | Seconds on NTFS. Falls back safely when the journal was reset or wrapped, or on exFAT/FAT drives. More Windows-specific code. |

**Backup engine**

| Option | Trade-off |
|---|---|
| Robocopy / external tool | Proven, but no integration with undo, audit, quarantine or the priority fill. Output must be parsed. |
| Deduplicating store (restic-style) | Space-efficient, but not plain files. Violates an anti-goal. |
| **Own copy engine in aegis-core, reusing the action/`FsOutcome` model (chosen)** | Plain files, full audit and undo, priority fill and a ransomware guard. More code to own. |

## 7. Design

### 7.1 Drive passport and identity

- **Writing the passport.** On first sight of a writable drive, write `<root>\.aegis\volume.json` with attributes hidden+system. Its contents are:
  ```json
  { "passport_id": "<uuid v4>", "created_at": <unix>, "created_by": "aegis <version>",
    "schema": 1 }
  ```
  The per-volume quarantine directory may also live under `.aegis\`, which is the builder's call.
- **Recorded alongside it.** Volume GUID path, serial, label, total size and filesystem type.
- **Matching order on plug-in:**
  1. passport_id
  2. volume GUID
  3. serial plus total size
  4. none of those match: a new drive.
- **Re-attaching rows.** When a match is found under a new letter, update `mount_point`; file rows stay attached. `files.path` must therefore be stored relative to the volume root, or rewritten on remount. The builder's call: relative storage is preferred if migration cost allows.
- **Clone detection.** If two online drives present the same passport, show the prompt "These two drives look like clones of each other." The second drive gets a new passport and is catalogued as a new drive. Its existing rows are not touched.
- **Read-only drives.** No passport is written. Identity uses the GUID/serial/size fingerprint, and the UI notes that recognition is weaker.
- **What counts as a drive here.** The managed set is removable or external volumes plus internal non-system volumes the owner adds. The system drive is handled as "the PC".

### 7.2 Online/offline lifecycle

- **Detection.** While the app is open, it detects arrival and removal of drives. A lightweight poll of logical drives every few seconds is acceptable; `WM_DEVICECHANGE` is optional.
- **Removal.** The volume becomes `offline` and `last_seen` is stamped. Any in-progress scan or backup job is checkpointed. File rows are untouched and keep their state (G2).
- **Search and the library.** Offline files show the badge "on <label> (unplugged, last seen <date>)". Opening one prompts "Plug in <label> to open".
- **Duplicate matching.** Offline rows are included, so a file on the PC can be reported as a copy of one on an unplugged drive.
- **Destructive actions.** These require every involved volume to be online (§7.5).

### 7.3 Fingerprinting pipeline (first catalog)

1. **Metadata walk.** Tier 1 already does this: path, size, mtime, file ID.
2. **Size grouping across the whole catalog**, all volumes, online and offline. A file whose size is unique everywhere cannot be an exact duplicate, so it isn't read at this stage.
3. **Quick hash** for files in groups of the same size: BLAKE3 of size + first 64 KiB + last 64 KiB. Files ≤128 KiB get a full hash, which equals the quick hash's cost. This is stored in a new `files.quick_hash`.
4. **Full BLAKE3** only for files whose quick hashes collide. This goes in the existing `files.blake3`.

Files the backup engine copies get a full hash during the copy anyway. Other files with no duplicate candidates keep `blake3 = NULL` until something needs it.

- **Resumable.** Progress is committed in batches, as the scanner already does. Unplugging mid-catalog marks the volume offline. Re-plugging continues with unhashed candidates.
- **Throttled** so the PC stays usable. Hashing yields to user activity at a basic level; full idle scheduling is Project 2.

### 7.4 Re-plug update

- **NTFS.** Store the USN journal ID and next USN per volume. On re-plug, read changes since then and apply creates, renames, deletes and modifications.
  - Renames and moves keep `files.id`, matching on file ID (PRD FR-001.6).
  - Modified files lose their hashes and re-enter the pipeline.
- **Fallback.** A changed journal ID, a truncated journal, or a non-NTFS filesystem fall back to a metadata walk with the size/mtime check (`unchanged_hash`).
- **Retiring files.** Files absent after a confirmed full walk are retired with `sweep_unseen`, which exists already.
- **Plug-in banner** (in-app):
  ```
  🔌 Seagate A, last seen 3 March
     214 new files · 3 folders changed · 2 new duplicates   [Review] [Later]
  ```

### 7.5 Duplicates: grouping, questions, decisions

**Grouping**

- **Folder groups.** When two directories have identical content trees (the same set of relative path → hash, ignoring OS junk files), they become one group: "This 12 GB folder exists on Seagate A and WD C."
- **Partial overlap.** A folder that is ≥90% contained in another is shown as one group, with the differences listed. "Contained in" is the builder's call on the exact rule.
- **Single files.** Remaining duplicates are grouped by hash and shown in batches by folder pair, never as a flat list of thousands.

**Questions.** One question per group, phrased in terms of drives and roles:

- When a group involves the Backup drive, the copy on the Backup drive counts as healthy and **no question is asked**.
- Otherwise the question is: "Folder X on WD C is an exact copy of folder Y on Seagate A. What is it?"
  - **It's a backup.** The app offers to move it to the Backup drive, or if the Backup drive already has a copy, to quarantine this one.
  - **Keep both.** This is remembered and never asked again for this folder pair.
  - **Delete this copy.** It goes to quarantine.

**Remembered decisions.** Answers are stored per folder pair, keyed by volume passport + relative path. A rescan never re-asks, and the decision is undoable from a "Decisions" list.

**Choosing the keeper** (replacing "shortest path"), in this order:

1. a copy whose location the owner has explicitly chosen;
2. a non-Backup copy;
3. the copy whose folder holds more of its siblings (it keeps a folder whole);
4. the oldest mtime, i.e. the original;
5. the shortest path.

The keeper can always be changed in the UI.

**Safety rules (hard invariants)**

- **S1. Never quarantine the last copy.** Every quarantine of a duplicate requires at least one other verified copy to remain.
- **S2. Copy count.** Default minimum copies = 2, where one of them is on the Backup drive if one is assigned. Deleting a duplicate that would drop a file below 2 copies is refused. In that case the app offers "Move to Backup drive" instead.
- **S3. Verify before deleting.** The surviving copy must be on an **online** volume and is re-hashed (full BLAKE3) immediately before the quarantine executes. A snapshot of an unplugged drive is never trusted for a delete.
  - UI text: "The other copy is on WD C (last checked 3 months ago). Plug it in to confirm, or keep both."
- **S4. The normal flow.** All deletes go through the existing batch flow (propose → approve → execute → quarantine → 30-day expiry).

### 7.6 Drive roles and the drive plan

- **Roles.** `volumes.role ∈ {unassigned, general, backup}`, and at most one drive is `backup` in this project. The schema should allow more later.
- **Recommendation.** After all known drives have a first catalog, the app recommends a Backup drive. It scores free space and the share of existing duplicate copies already on each drive, so the least data has to move.
- **Drive plan screen.** One approval, and changeable later:
  ```
  Drive plan
    Seagate A  1TB   General  · 410 GB used
    Seagate B  1TB   BACKUP   · holds 62% of your backup copies already
    WD C       1TB   General  · 780 GB used, 90 GB duplicates found
    [ Approve plan ]  [ Change roles ]
  ```
- **What the approved plan generates:**
  1. a batch that moves stray backup copies from general drives onto the Backup drive;
  2. duplicate questions for the rest.

  Both are previewed and approved in normal batches, sized to stay reviewable (≈ a folder or a few hundred files per batch; the builder's call).

### 7.7 Backup mirror with safety window

- **Sources.** Chosen folders on the PC and general drives. The defaults are the PC's Documents, Desktop and Pictures, plus "only copies" from any drive (§7.7.2). The owner can add or remove folders, and pin a folder as **always back up**.
- **Layout on the Backup drive.** Plain files, browsable without the app:
  ```
  <Backup>\Aegis Backup\<source label>\<original relative path>
  <Backup>\Aegis Backup\_Previous versions\<yyyy-mm-dd>\<source label>\<relative path>
  ```
  The source label is, for example, `PC (Documents)` or `Seagate A`.

**7.7.1 Top-up**, which runs when the Backup drive is plugged in, after approval:

- **New or changed source file.** Copy to a temp name on the Backup drive, verify the hash, then rename into place. A changed file's previous backup copy is first moved into `_Previous versions\<today>\`.
- **Deleted source file.** The backup copy moves into `_Previous versions\<today>\`. A deletion only counts as one if the source volume is online and the file is confirmed gone; an unplugged source is not a deletion.
- **Pruning.** Previous versions older than 90 days are deleted. They are already the second copy, so this does not use quarantine. Each prune is audited.
- **Autonomy.** The first top-up is shown as a batch for approval. After the drive plan and backup sources are approved, later top-ups run automatically as a **standing approval**, with a summary afterwards: "Backup up to date: 320 files added, 2.1 GB." This is the first auto-executing path in Aegis. It is limited to additive copies and moves within the Backup drive, so it never deletes anything at a source. Policy-engine changes must be explicit and audited (§8).
- **Ransomware / mass-change guard.** Before a top-up, compute:
  - the share of files in each source folder that would be overwritten or deleted;
  - the share of changed files whose content looks encrypted (high entropy, magic bytes no longer matching the extension).

  If more than 20% of a folder would change, or encrypted-looking changes exceed a threshold (the builder sets it, with a test), the automatic top-up halts for that folder. It asks: "An unusual number of files changed in Documents (412 of 1,800). Back them up anyway?" Good copies are left untouched until the owner answers.
- **Interruption.** Unplugging mid-top-up is safe, because each file uses temp + verify + rename. The job resumes on the next plug-in.

**7.7.2 Priority fill.** When the content doesn't fit, it fills in this order:

1. Pinned folders and **only copies**: files with exactly one copy anywhere, excluding installers and caches.
2. The PC's Documents, Desktop and Pictures.
3. Music and video.
4. Never: installers, caches, build output (existing `cache_dirs`), OS junk.

When space runs out:

- previous versions are pruned oldest-first before any live copy is skipped;
- live copies are never removed to make room for new ones; lower priorities are skipped instead.

The Drives screen shows protected and unprotected amounts by tier, with a suggested fix:

```
Backup drive  B   ███████████████░░░░  780 GB / 1 TB
  ✓ Only copies         212 GB
  ✓ PC Documents etc.   340 GB
  ◐ Music               228 GB   (40 GB doesn't fit)
  ✗ Installers & caches skipped
```

### 7.8 Resolved details (from builder review; all Claude's calls)

1. **Catalog coverage.**
   - External drives are catalogued whole from the root, excluding `.aegis\`, `System Volume Information`, `$RECYCLE.BIN` and the usual OS junk.
   - On the PC, the scanned locations are Downloads, Documents, Desktop, Pictures and Music (the owner's chosen starting set) plus any roots the owner adds. These are seeded as roots on first run.
   - "Only copy" is judged against everything catalogued. The UI says "only copy among the places Aegis knows about".

2. **Existing content on the drive chosen as Backup.**
   - Files outside `Aegis Backup\` that duplicate a file elsewhere are **adopted**. They're renamed within the drive into `Aegis Backup\<keeper's source label>\<keeper's relative path>` and recorded in `backup_copies`. This is cheap because it's a rename on the same volume.
   - Files that exist **only** on the Backup drive are **not** treated as backed up. They stay where they are, shown as "Only copy, on the Backup drive itself, not protected". The drive plan offers to move them to a general drive that has room, with no default move.
   - "Copy on the Backup drive counts as healthy" (§7.5) applies **only** to files inside `Aegis Backup\` that are recorded in `backup_copies`.

3. **"It's a backup" / stray backup copies.**
   - On the Backup drive, the moved copy lands at `Aegis Backup\<keeper's source label>\<keeper's relative path>`.
   - The keeper's folder is added to `backup_sources`, so the mirror keeps it current from then on.
   - Across drives, "move" means: copy → verify hash → record → then quarantine the original. The last step is a delete at a source, so it always runs in a normal approved batch, never under the standing approval.

4. **What counts as a copy for S1/S2.** A copy is a non-deleted, non-quarantined file row with a matching full BLAKE3, on any volume, online or offline.
   - Files in `_Previous versions` and quarantine do **not** count.
   - Offline copies count toward the minimum of 2, but S3 still requires an *online*, freshly re-hashed survivor before anything executes.
   - With exactly two general-drive copies and no Backup copy, "Delete this copy" is refused by design. The offered alternative is "Make this the backup copy": move one copy to the Backup drive, which keeps the count at 2 and puts one of them on the Backup drive.

5. **When mirror coverage shrinks.**
   - **A source folder is removed from `backup_sources`.** Its backup copies move to `_Previous versions\<today>\` with reason `uncovered`, and are pruned after 90 days. Before applying, the app asks once: "Stop backing up Music? Its 228 GB backup will be kept for 90 days, then removed."
   - **A file stops being an "only copy"** because a duplicate appeared. Its backup copy stays; protection is never reduced automatically.
   - **A tier no longer fits.** Existing live copies stay (§7.7.2). Only *new* lower-tier copies are skipped.

6. **Standing approval.**
   - It is stored as a policy record scoped to `backup_topup` and to the Backup volume's **passport_id**. Re-passporting, clone detection or a role change revokes it, and that revocation is audited.
   - Once approved, top-ups start automatically on plug-in. The plug-in banner shows progress with a **Pause** button and the summary afterwards. No click is needed.
   - The standing approval only permits copies onto the Backup drive and moves within it. That covers previous versions and the 90-day prune. It never permits deletes or moves at a source.
   - The C4 spec amendment and a threat-model note are part of this project.

### 7.9 UI surfaces in this project

- **Home (command centre)** gets two tiles:
  - **Drives**: each drive with its role, online state, last seen and backup status;
  - **Needs you**: duplicate groups, the drive plan, guard prompts.
- **The Drives screen** shows:
  - the drive list and drive plan;
  - per-drive catalog progress;
  - backup status by tier;
  - the Decisions list;
  - the previous-versions browser with restore.
- **The duplicate review** is grouped by folder, with side-by-side locations, keeper selection and the three answers.
- **The plug-in banner** appears in-app.

Visual style follows the chosen command-centre direction. Detailed screens are mocked during implementation for the owner to see before they are finalised.

## 8. Data model changes (migration 0002)

The column and table names are the builder's call; the meaning below is fixed.

- **`volumes`** gets: `passport_id` (unique, nullable), `guid_path`, `label`, `serial`, `total_bytes`, `role`, `usn_journal_id`, `usn_next`, `last_catalog_complete_at`.
- **`files`** gets: `quick_hash`.
  - Possibly path storage relative to the volume root (§7.1).
- **`dupe_decisions`**: pair of (volume passport, relative folder path) × 2, decision, decided_at, undone_at.
- **`backup_sources`**: source volume, relative path, priority tier, pinned flag.
- **`backup_copies`**: source file id → backup path, source hash at copy time, copied_at, last_verified_at.
- **`backup_versions`**: path in `_Previous versions`, original source path, reason (changed/deleted), moved_at, prune_after.
- **Policy.** A `standing_approvals` concept, or an equivalent in the policy engine, scoped to `backup_topup` for a specific Backup volume. Creation and revocation are audited.
  - This extends the C4 v1.0 "zero auto-execute" invariant.
  - The C4 spec doc gets an amendment noting the exception: additive or within-backup operations only, never deletes at a source.

## 9. Constraints

- All destructive operations follow the existing disk-first → short `with_full_sync_tx` → undo-on-failure pattern and are audited.
- OneDrive placeholders are never read for hashing (PRD FR-001.9). Hardlinks are not double-counted (FR-001.10).
- Long-running work runs off the UI thread, with progress events on `aegis://event`. This is the existing pattern.
- No new network access; everything is local.
- USN and device code is Windows-only behind `cfg(windows)`, with the walk fallback on other platforms.

## 10. Testing expectations

- **Fakes.** Multi-drive behaviour needs a seam, for example a `VolumeProvider` trait. Tests can then simulate several volumes as temp directories with fake passports, unplug them (offline), re-plug them under new letters and clone them.
- **Invariant tests**:
  - S1–S3 (never the last copy; refused below 2 copies; no delete against an offline survivor; re-hash mismatch aborts);
  - offline drive ≠ missing files;
  - top-up moves overwritten and deleted files into `_Previous versions`;
  - prune only after 90 days;
  - the mass-change guard halts a top-up;
  - an interrupted copy leaves no partial file in place.
- **USN path**: tested on a real NTFS temp volume where available. Otherwise it's covered by fallback tests, and the gap is reported honestly.
- **Real-world check.** Before calling it done, the owner runs a dry catalog of one real external drive, and the timing and duplicate counts are reported.

## 11. Decided by the owner

- Project order (drives first).
- One drive becomes Backup, the others General.
- Mirror with a 90-day safety window.
- Exact copies are reported and the app asks backup-or-delete.
- Deletes always ask and go through quarantine.
- Learn-then-take-over autonomy.
- Command-centre home with the library one click away.
- Proceed to build.

## 12. Calls made by Claude (owner may override)

- The passport file plus fallbacks.
- The USN journal with walk fallback.
- Size → quick hash → full hash pipeline.
- Folder-level grouping and per-folder-pair remembered decisions.
- Keeper rules.
- Minimum 2 copies.
- Re-hash the survivor online before any delete.
- Backup layout and plain files.
- Standing approval for top-ups after the first.
- 20% mass-change guard.
- Priority fill tiers.
- Plug-in detection in-app only until Project 2.
- Previous versions pruned without quarantine.
- Everything in §7.8:
  - whole-drive catalog of externals; PC starting set seeded as roots;
  - adopting duplicates already on the Backup drive;
  - "only copy" files on the Backup drive are flagged, not moved;
  - what counts as a copy;
  - "Make this the backup copy" instead of a refused delete;
  - uncovered backups kept 90 days;
  - standing approval tied to the passport, starting automatically with Pause.

## 13. Left to the builder

- Exact schema names.
- Relative vs absolute path storage.
- Folder-containment rule details.
- Batch sizing.
- Entropy threshold.
- Device-detection mechanism.
- Where per-volume quarantine lives.
- Throttling.
- Plug-in detection polling interval.
