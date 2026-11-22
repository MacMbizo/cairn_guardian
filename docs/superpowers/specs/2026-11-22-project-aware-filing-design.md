# Project-aware filing: design

Follow-up to Auto-filing (`2026-11-21-auto-filing-design.md`). Owner brief, 2026-11-22:

> *"If you identify or see a root project folder, propose a structure. Some project folders may be on documents, like the ones in Projectsv2. However, there may be similar copies in c:\prj. Aegis needs to then be able to say there are project files in different locations, similar or different etc and then propose an intelligent way of accounting for them all. Whether together or separate, that is why Aegis has to have higher intuition than me, anticipate, create, redesign."*

And earlier: moving favicons and `.md` files out of `Dev Set` into Pictures and Archives *"means we will misplace those assets"*. Pictures that start with "SmoothLite" are a design project for the SmoothLite brand and belong together. Inside a `2026` photo folder Aegis should *"dig further to classify"*. The owner has stopped approving batches that touch project folders until this is fixed.

The owner chose **approach A, projects first** (§15). Everything not quoted above is the builder's call and is marked as such where it matters.

## 1. Intent

Filing has to understand **what files are for** before it moves anything. A file that belongs to a project, or to a set of related files, is never moved on its own merits. Things that belong together stay together, wherever they are on the disk.

This shows in three ways:

1. **Projects are recognised** even without a `.git` or `package.json`: specs, prompts, design assets and code drafts count too.
2. **Every location of a project is accounted for.** Copies in `Documents\Projectsv2\…` and `C:\Prj\…` are matched. Aegis says how each copy relates to the others and proposes one plan per project.
3. **Sets inside the big buckets are recognised.** The SmoothLite artwork in all its formats, or the photos of one subject in `Pictures\2026`, are kept together and named after their subject.

Success: the owner can approve filing anywhere, including project folders, without first checking that Aegis won't scatter something. A good proposal reads like something the owner would have done with a free weekend, and spots relationships the owner had forgotten.

## 2. What exists today (facts the design builds on)

- **Roots.** The PC roots are `Downloads`, `Documents`, `C:\Prj` and `Pictures`; the catalog holds about 363,000 files. `C:\Prj` alone has about 117,000, much of it `node_modules`. About 289,000 files already have a BLAKE3 hash, so content overlap is a lookup, not a new scan.
- **Projects today.** A project is recognised only as an *atomic unit*: a folder holding a build marker (`.git`, `Cargo.toml`, `package.json`, `pyproject.toml`, `go.mod`, `*.sln` …), 481 in the catalog. Blueprint planning keeps atomic units, course folders and named photo albums whole; everything else is filed file by file, by kind.
- **Unused tables.** A `projects` table (`id, name, description, domain, repo_url, health_score`) and `atomic_units.project_id` exist from the first schema and are unused. So are `entities` and `relationships`.
- **What went wrong, measured.**
  - In `Documents\Projectsv2\Dev Set`, 11 of 16 project folders have no build marker at the top: `chatbot`, `wellness_app`, `discipline_app`, `Nutrition`, `inventory`, `AgentsM`, `AgentsW`, `Manus`, `KuBendDown`, `notion-up`, `student_discipleship_app`. In `C:\Prj`, 9 folders have none, including `DevSet`, `AIPrompt` and `forexTrader`.
  - The owner rejected 15 proposed moves. All but one moved project material by kind:
    - `Dev Set\forexTrader\images\favicon_io\favicon-16x16.png` → `Pictures\2025\2025-06`
    - `Dev Set\paperPilot\images\logo_v3.jpg` → `Pictures\2025\2025-06`
    - `C:\Prj\DevSet\mccShepherd\all docs\archive\PRD_additions.md` → `Documents\Study\Discovery`
    - `Dev Set\discipline_app\set_files\PRD_additions.md` → `Documents\Study\Discovery`

  These become regression tests.
  - `Pictures\SmoothLite Front (CMYK).pdf`, `.zip` and `.rar` sit loose in Pictures. `SmoothLite 2.jpg` has already been filed to `Pictures\2026\2026-10`, apart from the rest of its set.
- **How copies relate.** Copies of the same project relate in different ways. Measured on files outside `node_modules`/`.git`, by relative path and size:

  | Project | `C:\Prj` | `Projectsv2\Dev Set` | Shared | What it is |
  |---|---|---|---|---|
  | forexTrader | 356 | 8,901 | 356 | `C:\Prj` copy is an older subset |
  | DrivingSchool | 112 | 108 | 99 | diverged: both changed since they split, `C:\Prj` newer |
  | AIPrompt | 254 | 370 | 122 | partial overlap, each has its own files |
  | paperPilot | 14,648 | 346 | 2 | same project, different material: code in `C:\Prj`, docs set in Dev Set |
  | wellness / wellness_app | 191 | 67 | 0 | same idea, different stages |

  `Projectsv2` also holds folders such as `Inv`, `Inv2`, `Inv3`, `D1`–`D4`, `NutriP`, `NutriPlan`, `Nutri2` and `PR1`/`PR4`/`PR-back`. Their names hint at versions; only their contents can confirm it.

## 3. Goals

1. **Never split a project or a set.** Filing never proposes a file-by-file move for anything inside a recognised project, or for a member of a recognised set. This is the first thing built (§16, step 1) and unblocks the owner at once.
2. **Recognise projects without build markers**, with a confidence and evidence the owner can read.
3. **Match every location of a project** across all roots and drives, and classify each pair (§6).
4. **Propose one plan per project** that accounts for every copy: where the project lives, what happens to each other copy, and why.
5. **Recognise sets** of related files across kinds (SmoothLite), and group photos inside a year by subject.
6. **Put back together** what earlier batches already split.
7. **Learn** from the owner's corrections ("same project", "not the same", "keep separate", renames), so a correction is never needed twice.

## 4. Non-goals

- **No merging of changed files.** Aegis never merges two diverged copies into one tree file by file. Diverged copies are kept side by side and labelled; reconciling them is the owner's (or a developer's) work.
- **No git operations.** Aegis never commits, checks out, rewrites or cleans a repository.
- **No cloud.** Recognising projects on GitHub or in the cloud is out of scope (the `repo_url` column may be filled from a local `.git/config`, read-only).
- **No AI models in the first version.** Recognition is deterministic and explainable, like classification in Project 3. Embedding similarity can be added later as one more signal (§17).

## 5. Anti-goals (would be a failure even if it worked)

- **Losing work.** A file that exists in only one copy gets deleted or quarantined.
- **Breaking a repo.** A move that changes paths inside a repository, or leaves a repo with untracked files the owner didn't expect (§7.4).
- **False projects.** A Projects list full of ordinary folders ("Downloads\misc" is not a project). When unsure, Aegis asks one yes/no question; it never guesses its way into a big move.
- **Per-file questions.** The owner decides per project, not per file.
- **The same correction twice.**

## 6. Concepts

- **Project.** Something the owner made or worked on, as a whole: an app, a brand's design work, a dissertation. It has a name, aliases, a kind (`software`, `design`, `writing`, `research`, `other`) and one or more **locations**.
- **Location.** A folder holding (part of) a project: its root folder on some volume.
- **Project root.** The outermost folder that is the project, not a folder *of* projects (§7.2).
- **Container.** A folder whose children are mostly projects: `C:\Prj`, `Projectsv2`, `Dev Set`. A container is never a project itself and never moved as one.
- **Home.** The one location a project's plan centres on (§8.1).
- **Relationship** between two locations of the same project, from content (BLAKE3 where present, else relative path + size):

  | Relationship | Test (builder may tune thresholds) | Default proposal |
  |---|---|---|
  | **Identical** | ≥ 98% of files in each are in the other | Quarantine the non-home copy (30 days, asks) |
  | **Older subset** | ≥ 98% of the non-home copy's files are in home; home newer | Quarantine the non-home copy (asks) |
  | **Newer subset** | Non-home is a subset of home but has newer versions of shared files | Treated as **diverged** |
  | **Diverged** | ≥ 40% of the smaller copy shared, and each has files the other lacks | Keep both. The other copy moves into home's versions area, dated and labelled |
  | **Different material** | < 10% shared, same identity (§7.3), material differs (e.g. docs vs code) | Bring the material into home's material area |
  | **Version series** | Name series (`Inv`, `Inv2`, `Inv3`) or chained partial overlap | Newest is home; older ones become dated versions |
  | **Unrelated** | Name alike, contents unrelated and owner says "not the same" | Two projects; the decision is remembered |

  The table runs over files that aren't build output: `node_modules`, `.git`, `target`, `dist`, `build`, `__pycache__`, `*.pyc`, `.venv`, `.next` are excluded from counts and comparisons. They still move with their folder.
- **Set.** Files outside any project that belong together across kinds: a shared subject in the name (`SmoothLite …`), produced together (same minute or day, same source folder), or the same artwork in several formats (`.png`, `.pdf`, `.zip`, `.rar` with one stem). A set of only photos is a **photo subject**; a set with design or print files is a **design project** (§10).

## 7. Recognising projects

### 7.1 Signals

Per folder, from the catalog (no extra disk reads except README/manifest text, which the reader already extracts):

- **Strong (one is enough):** an existing atomic unit; `.aegisboundary`; a README at the folder top; a manifest (`package.json`, `Cargo.toml`, `pyproject.toml`, `requirements.txt`, `pubspec.yaml`, `*.csproj`, `go.mod`, `composer.json`, `Gemfile`, `index.html` with assets beside it).
- **Material signals (weighted, summed):**
  - project documents by name or content: `PRD`, `spec`, `requirements`, `TASKS`, `roadmap`, `architecture`, `persona`, `prompt`, `rules`, `design doc`, `brief`, `README`, `CHANGELOG`, `set_files`;
  - source files (`.ts`, `.tsx`, `.js`, `.py`, `.rs`, `.dart`, `.cs`, `.html`, `.css`, `.sql` …) even without a manifest;
  - brand and UI assets: `logo`, `favicon`, `icon`, `mockup`, `wireframe`, `screen`, `banner`, `sleeve`, `CMYK`, `dpi`, `.fig`, `.psd`, `.ai`, `.svg`;
  - **mixed kinds with one purpose**: docs + images + code in one folder tree;
  - sibling evidence: the folder sits in a container (§7.2).
- **Against:** the folder is a known folder (Documents, Downloads, Pictures …) or a fresh-structure area; its children are mostly unrelated loose files (a dump); it is mostly media by kind (a photo year, a music album).

Confidence comes from the sum, capped: strong signal → ≥ 0.9; material signals only → 0.6–0.85. Below 0.6 the folder is not a project. Between 0.6 and 0.8 Aegis shows it as **"Is this a project?"**: one tap, remembered. Its files are protected from file-by-file moves meanwhile (Goal 1 protects at ≥ 0.6).

### 7.2 Choosing the root

Signals often sit deep (`AIPrompt\template\prompt-pigeon\.git`). The root is found by climbing:

1. Start from every folder with a signal.
2. Climb while the parent is part of the same project: the parent's other children are its parts (`docs`, `images`, `set_files`, `src`, `backup`, `archive`, versioned names like `set_files_v2(backup)`), not other projects.
3. Stop below a **container**: a folder with ≥ 2 children that are projects (or project candidates) and few loose files of its own, or one of the roots.
4. A project inside another project's root stays part of the outer one (`AIPrompt\template\prompt-pigeon` belongs to `AIPrompt`), and is listed inside it as a **sub-project**.

`C:\Prj`, `Projectsv2` and `Dev Set` come out as containers; `C:\Prj\DevSet` is also a container (it holds `mccShepherd` and others).

### 7.3 Identity: is it the same project?

Two locations are candidates for one project when **any** of:

- **Name match:** normalized names are equal or near-equal. Normalization: lowercase; drop separators, spaces, `_app`, `-main`, `(backup)`, `copy`, `old`, `final`, version suffixes (`v2`, `2`, `_v1.1`); the same alias list as the owner's corrections. `wellness` ≈ `wellness_app`; `nutrition` ≈ `Nutrition` ≈ `NutriPlan`? Not by name alone: prefix matches need content support.
- **Manifest match:** the same `name` in `package.json`/`Cargo.toml`/`pyproject.toml`, or the same git remote.
- **Content match:** ≥ 20 shared non-trivial files (by hash; trivial = < 1 KB, or generic like `LICENSE`, `.gitignore`, `favicon.ico`), or ≥ 30% of the smaller copy.
- **Owner said so** ("same project").

Then the relationship (§6) is computed per pair. A name-only candidate with no shared content and no manifest match is shown as **"Possibly the same project?"** rather than assumed.

### 7.4 Repositories

A location that is a git repository (an atomic unit of type `git_repo`) is **pinned**: it is never split and nothing is ever moved *into* its tracked tree. **Owner decision (2026-11-22):** a repo folder *may* move whole into the projects home as part of a plan the owner approves. The card warns when the repo holds a Python virtual environment (`.venv`, `venv`, `env` with `pyvenv.cfg`) that will need recreating, or when it changed in the last 30 days. When a repo is a project's home, material and versions arriving from other copies go into a sibling **companion folder** next to the repo, named `<repo name> — material` (builder may refine the name). That way the owner's `git status` stays clean. A repository is never a quarantine target.

## 8. The project plan

### 8.1 Choosing the home

In order:

1. A git repository location (if several, the one with the most recent commit file times). If it sits outside the projects home, the plan moves it there whole (§7.4).
2. Otherwise the location with the most recent activity (newest non-build file) and the most files, inside the **projects home** (§8.3) if one is there.
3. Otherwise the most complete copy.

The owner can change the home on the card; that choice is remembered for this project.

### 8.2 What happens to each other copy

From the relationship table (§6). In words:

- **Identical / older subset:** everything is already in home. The copy is proposed for quarantine as a whole (30-day undo, like duplicates). Every file is checked by hash against home at execute time; anything not found in home is moved into the versions area instead, never quarantined.
- **Diverged:** the copy is moved whole into `<home or companion>\Versions\<YYYY-MM-DD> <where it came from>` (date = its newest file). Nothing is deleted. The card shows how many files differ and which copy is newer per shared file, so the owner can reconcile later.
- **Different material:** moved into `<home or companion>\<Material>`, where Material is `Docs`, `Design` or `Material` by what it holds (docs set, assets, mixed).
- **Version series:** older versions move into `Versions\<YYYY-MM-DD> <original name>`; the newest is home.
- **Sub-projects** move with their parent; they are not planned separately.

### 8.3 The projects home

One folder where projects live on each volume. On the PC the default is the container that already holds the most projects: today `C:\Prj`. On an external drive it is the fresh structure's `Projects` folder (added to the fresh structure as a top-level area; Tidy keeps the owner's own container).

A project whose home is outside the projects home (e.g. only in `Documents\Projectsv2\Dev Set\chatbot`) is proposed to move there whole: `C:\Prj\chatbot`. This is how `Projectsv2` and `Dev Set` empty out over time. Empty containers are proposed for removal last (to quarantine, which asks).

Name collisions in the projects home (`C:\Prj\wellness` already exists and is a different project) get the project's kind or a disambiguator, never a ` (2)`: `wellness (2025 app)`. Builder's call on the exact form.

### 8.4 A project card (what the owner sees)

Illustrative (numbers from §2; the third location is hypothetical):

```
forexTrader                                   Software · 3 locations · sure
  Home     Documents\Projectsv2\Dev Set\forexTrader   newest, most complete (8,901 files)
        → moves whole to C:\Prj\forexTrader (your projects home)
  ─ C:\Prj\forexTrader          older subset: all 356 files are in the home copy
        → quarantine first, 30 days (frees the name for the home copy)
  ─ Documents\Projectsv2\F1     same logos and favicons (31 shared files)
        → C:\Prj\forexTrader\Design
  [Approve plan]  [Change home]  [These aren't all the same project]  [Leave as is]
```

Steps inside one plan run in a safe order: anything that frees a name runs before the move that needs it, and every step is in the same batch, so one undo restores all of it.

One approve moves everything for that project as one batch, with undo. "Leave as is" protects the project from filing and stops asking.

### 8.5 Order of proposals

Cards are offered one at a time, simplest and safest first: identical copies (space back, nothing lost), then subsets, then single-location projects that only move into the projects home, then diverged and version series. A weekly-note line summarises: "6 projects have copies in 2–3 places. Review."

## 9. Filing changes (what stops today)

- `blueprint::plan` treats every file inside a project root at confidence ≥ 0.6 like a code group: **stays, reason "Part of project X"**. No file-by-file move, no rename, in both Tidy and Fresh.
- The Fresh structure no longer files *project containers'* contents at all; they are handled by project plans.
- A batch that was proposed before this change and still waits is re-checked when opened; items now inside a project are dropped from it (marked `superseded`).
- Filing rules (learn-then-take-over) never apply inside a project.

## 10. Sets and subjects

### 10.1 Finding sets

Among files outside projects:

- **Shared subject:** ≥ 3 files whose names share a leading phrase of ≥ 1 meaningful word that isn't generic (`IMG`, `Screenshot`, `Scan`, `Document`, `Untitled`, camera prefixes, dates, numbers). "SmoothLite" qualifies; "IMG" doesn't.
- **Same artwork, several formats:** one stem, different extensions (`SmoothLite Front (sleeve).png`, `….pdf`, `(CMYK).zip`), even with small suffixes (`dpi`, `(CMYK)`, `v2`).
- **Produced together:** files from one source folder within a short span, only as supporting evidence.
- Learned subjects: once the owner approves a set, its subject becomes a known word for matching.

### 10.2 Where sets go

- **Design project** (any design, print or archive file in the set, or a brand-like subject): becomes a project of kind `design` and gets a project card: home `<projects home>\SmoothLite` on the PC. **Owner decision (2026-11-22):** all projects, software or not, live in the one projects home (`C:\Prj` on the PC).
- **Photo subject** (photos/screenshots only): Fresh files photos as `Photos\<year>\<subject>` instead of `Photos\<year>\<year-month>` when they belong to a subject of ≥ 3; others keep the month folder.
- Files already filed apart (`SmoothLite 2.jpg` in `Pictures\2026\2026-10`) join the set's proposal (§11).

## 11. Putting back together

When projects and sets are first recognised, and after each catalog update, Aegis looks at `file_history` for files that earlier batches moved out of what is now a project root or away from their set. It proposes a **"Put back together"** batch per project or set, returning each file to its project (its original relative path if the project is still where it was) or into the set's new home. The batch says which earlier batch split them. Undo as usual.

## 12. Learning

Owner decisions are remembered per project identity, never per file:

- "Same project" / "Not the same project": stored as links between location fingerprints (path + manifest name + hash summary), so they survive renames and drive letters.
- "Is this a project?" yes/no: stored per folder fingerprint.
- Changed home, renamed project, "Leave as is": stored on the project.
- Normalized-name aliases learned from "same project" answers feed identity matching (§7.3) for future projects ("NutriP" = "Nutrition").

## 13. Engine and performance

- **When:** project recognition runs in the scheduler after a catalog or live-index change touches a root (debounced, below-normal priority), incrementally per changed container; a full pass on first run.
- **Budget:** a full pass over the current catalog (363,000 files) completes in under 60 s on the owner's PC, using SQL aggregates over `files` (paths, sizes, hashes) rather than walking the disk. Excluded build folders are filtered in SQL.
- **Hashes:** files without a BLAKE3 compare by relative path + size + mtime; the card says "compared by name and size" in that case. Hashes needed for a quarantine are computed at execute time (as today).
- **Unplugged drives:** locations on unplugged drives are listed from the catalog ("on New Volume, unplugged"); their plans wait until the drive is plugged in.

## 14. Data model (migration 0006)

Builder's call in detail. The shape:

- **`projects`** (existing table): add `kind`, `confidence`, `home_location_id`, `state` (`candidate`, `confirmed`, `left_as_is`), `aliases_json`.
- **`project_locations`**: `id, project_id, volume_id, root_path, role` (`home`, `copy`, `version`, `material`, `sub`), `relationship`, `files`, `bytes`, `newest_mtime`, `is_repo`, `evidence_json`, `computed_at`.
- **`project_decisions`**: the owner's answers (§12), keyed by fingerprint.
- **`file_sets`** and **`file_set_members`** for sets (§10), or sets stored as `projects` of kind `design`/`photo_subject` with member rows; builder's call.
- `atomic_units.project_id` gets filled for units inside a project.

## 15. Approaches considered

- **A. Projects first (chosen by the owner).** Recognise projects and sets before filing; account for every location with a per-project plan; never split. Most work, but it is the only option that does what the brief asks: Aegis anticipates and proposes, the owner approves per project.
- **B. Recognise only, move nothing.** A Projects view ties locations together and only exact duplicates are cleared. Safe and cheap, but the clutter stays and the brief asks Aegis to "propose a structure".
- **C. Merge everything into `C:\Prj`.** Tidiest disk, but merging diverged copies file by file and moving repositories risks losing work. Ruled out.

## 16. Build order

Each step is usable on its own.

1. **Protection now.** Folder-level project recognition (§7.1–7.2) at ≥ 0.6 feeds `blueprint::plan` (§9). The 15 rejected moves above become tests and are no longer proposed. Waiting batches are re-checked.
2. **Projects list (read-only).** Identity, locations and relationships (§6, §7.3), shown on a Projects screen under Filing with evidence. "Is this a project?" and "same / not the same" answers.
3. **Project plans.** Home choice, per-copy proposals, project cards, batches with undo (§8).
4. **Sets and photo subjects** (§10).
5. **Put back together** (§11).
6. **Learning** wired through (§12), and the weekly-note line.

## 17. Settled after the builder check (designer's calls)

- **Moves stay on one volume.** A project plan only moves, versions or quarantines within one drive. Locations on other drives are matched and shown on the card (with their relationship), and get their own plan inside that drive's projects home, but nothing is copied or moved between drives. Copies on another drive are never quarantined because a PC copy exists: until the owner has a Backup drive, that may be the only other copy. **The Backup drive takes no part** in recognition or matching (consistent with the reader skipping it).
- **Folder moves.** A whole location moves as one folder action (`target_kind = 'dir'`), like course groups today; `after_execute` writes one `file_history` row per file inside, as it already does for groups (test `group_move_reindexes_children`). Quarantine of a whole copy is likewise one folder action into the volume's quarantine. Undo uses the existing rollback.
- **Weights and the dry-run tool.** The tool exists (`examples/filing_dry_run.rs`). Step 1 extends it to print recognised projects, roots and confidence per container. Weights are tuned on the owner's folders with it. Of the 15 rejected moves, the 14 inside `Dev Set`, `C:\Prj\DevSet`, `C:\Prj\forexTrader` and `Projectsv2\F1` must be stopped by project protection on material signals alone (most have no marker). The Downloads `.zip` is not project material and is out of scope.
- **Two protection layers.** Atomic units (build markers) keep their existing protection unchanged as the inner layer. Project protection is the outer layer. "Pinned" (§7.4) applies to the repository folder only, not to the whole project around it.
- **Fresh structure.** `Projects` becomes a Fresh area (in `Bases::of`, the tree preview and the summary). On a drive with no `Projects` folder, Fresh leaves project folders where they are ("Part of project X"); only an approved project plan moves them.
- **Waiting batches.** They are re-checked lazily, when a batch is opened or next proposed. Items inside a project are marked `superseded` (an existing status that doesn't count as a "no"). No data migration.
- **UI is in scope.** Steps 2 and 3 include the Projects screen under Filing (TypeScript) and its IPC commands. The owner sees mockups of the project card before it is built.
- **Set words.** The generic-word list in §10.1 starts with camera and app prefixes (`IMG`, `DSC`, `PXL`, `VID`, `WhatsApp`, `Screenshot`, `Scan`, `Document`, `Untitled`, `New`, `Copy`, `Final`), month names and numbers. Learned subjects are stored with the project decisions.

## 18. Builder's calls and later

- Thresholds in §6 and §7.1 (tune against the owner's real folders with the dry-run tool, then freeze them in tests).
- Exact names of companion and version folders.
- Later: embedding similarity (the `aegis-embed` crate) as an extra identity signal for projects with no shared files, such as `wellness` and `wellness_app`.
