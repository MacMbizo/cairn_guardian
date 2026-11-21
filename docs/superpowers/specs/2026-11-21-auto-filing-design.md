# Auto-filing: design

Project 3 of 5. Owner brief, 2026-11-21: *"Dive in, choose what is best. Be able to read content. I need folder structure, naming scheme and all the right logic. This goes for all my drives, not just the messy external. I want to find things and also have a drive that makes sense when I open it."*

Earlier owner decisions this builds on:

- **Blueprint choice.** The owner sees a tidied-up version of their own folders next to a fresh structure, and picks one.
- **Learn-then-take-over.** Three approvals make a rule automatic; one rejection takes it back.
- **Naming.** Files are named `<date> <who/what> <type>`.
- **Batches.** Migration happens in batches.
- **Safety.** Deletes always ask. Nothing is lost.

Everything else in this document is the builder's call, and is marked as such where it matters.

## 1. Intent

Two outcomes, equally weighted:

1. **Find anything.** A search for "Vodacom invoice March" or "my ID scan" finds the file, whatever it was called and wherever it was. Search covers a file's content, its new name and its old name.
2. **Every drive makes sense when you open it.**
   - The same handful of top-level folders appears on each drive, with obvious names.
   - Inside them, years and subjects.
   - Documents have names that say what they are.

## 2. What exists today (facts the design builds on)

- **Shape of the data.** The PC folders hold little personal material. Documents is about 50,000 files of code repositories; Downloads has around 200 loose files, mostly installers and media; Pictures holds loose screenshots. The external drive "New Volume" holds 905 GB in 56 top-level folders: copies of older PCs (`downloads`, `msidownloads`, `msidocuments`), course videos (`self development`, 275 GB), installers (`software`), and copies of code (`projectsv2`).
- **Available plumbing.** Move and Rename actions already go through batches with simulate, approve, execute and rollback. The policy engine has autonomy dials and atomic-unit protection, and `files.classification` and `files.confidence` exist but are unused. Atomic units (git repos and other code projects) are already detected. Text chunks and a full-text index exist, but only for plain-text files.
- **No content extraction yet.** There is no PDF or Office extraction and no ML model.
- **Engine (Project 2).** Watches, reminders, the weekly note and a live index of PC folders are already in place.

## 3. Goals

1. **Content reading.** Text from PDF, DOCX, XLSX, PPTX, ODT, RTF, TXT, MD and CSV; EXIF from photos; tags from audio. The text feeds search and classification.
2. **Classification.** Every file gets a *kind*, a *category* (life area), optional *who* (an organisation or person) and *date*, a confidence score, and evidence that says why.
3. **Blueprints per drive.** For each drive and PC folder set, two options built from the drive's own contents, shown side by side as trees with counts:
   - **Tidy:** the owner's own folders, cleaned up.
   - **Fresh:** the standard structure in §5.

   The owner picks one per drive and can switch before migrating.
4. **Naming scheme** (§6), applied where Aegis is confident. Original names are always kept and stay searchable.
5. **Migration in batches** that the owner approves after a tree preview. All of it is reversible.
6. **Learn-then-take-over.** Rules graduate to automatic after 3 approvals and drop back after 1 rejection.
7. **Ongoing filing.** New files arriving in Downloads, the Desktop or a drive's root are filed by learned rules, or proposed for filing.
8. **Finding.** Search matches content, current names and former names. Results show "was: old name, old place".

## 4. Non-goals

- Moving files between drives. Every drive is filed within itself.
- Image OCR, speech-to-text, and reading video content. Videos and photos are filed by metadata.
- Changing anything inside code projects or other atomic units. They move only as whole units.
- The Backup drive. It is a mirror, and Project 1 owns its layout.
- Cloud-only placeholders (OneDrive "free up space" files). They are filed by name and type only, never opened.

## 5. The fresh structure

The same folders are used on every drive. A folder that would be empty on a drive is not created there.

```
Documents\
  Finance\          Banking · Tax · Invoices & receipts · Insurance · Payslips · Investments
  Work\             <employer or client>\<year>
  Study\            <course or institution>
  Personal\         ID & certificates · Health · Home · Vehicles · Family · Travel
  Projects\         <project name>              (non-code project files)
  Other documents\  <year>
Photos\             <year>\<year>-<month>
Screenshots\        <year>
Videos\             Personal\<year> · Courses\<course> · Movies & shows
Music\              <artist>\<album>
Audio\              Podcasts · Recordings · Audiobooks
Code\               <project>                   (atomic units, moved whole)
Software\           Installers · Drivers · Portable apps
Archives\           (zip/rar/7z that aren't installers)
From old PCs\       <original folder name>      (only what can't be classified)
To sort\            (uncertain items: Aegis asks)
```

On the PC, Windows' own Documents, Pictures, Music, Videos and Downloads folders stay where they are, and the structure goes inside the matching one. For example, `Documents\Finance\…` sits inside the owner's Documents folder, and `Photos\…` maps to Pictures.

**Builder's call on "From old PCs".** Old-PC dumps are taken apart and filed by kind. Anything left over that can't be classified stays together under `From old PCs\<name>`, rather than being scattered.

### Tidy blueprint (the owner's own folders, cleaned up)

- Keeps the owner's top-level folders.
- Merges folders that are clearly the same thing: `documents` + `msidocuments`, and `downloads` + `msidownloads`.
- Files loose root items into the matching existing folder.
- Normalises folder names to Title Case and strips "(1)" and "Copy of".
- Leaves everything else where it is.

## 6. Naming scheme

`<YYYY-MM-DD> <who/what> <type>.<ext>`. Examples:

- `2026-03-14 Vodacom invoice.pdf`
- `2025-02-28 SARS tax assessment.pdf`
- `2024-11-02 Discovery Health claim.pdf`

**Date.** Taken in this order:

1. A date in the content (invoice, statement or letter date).
2. A date in the filename.
3. The EXIF date.
4. The file's modified date. If this is the only source, the filename isn't changed (it's too unreliable).

**Who/what.** An organisation or person found in the content or the name (§7.3). Without one, the subject (for example "Lease agreement") is used.

**Type.** One word from the classification: invoice, statement, receipt, payslip, contract, letter, certificate, ID, policy, quote, report, CV, assignment, notes.

**What gets renamed.** Only documents whose date and type confidence are both ≥ 0.80, and screenshots (`<date> <time> Screenshot`). Photos, music, videos, code and installers keep their names.

**Collisions.** Append ` (2)`, ` (3)` and so on. Never overwrite.

**Name cleanup.** Characters Windows forbids are removed, and names are capped at 120 characters.

**Original names.** The old name and path are stored in a `file_history` table, are searchable, and are shown in search results.

## 7. Content reading and classification

### 7.1 Reading

A background reader watch runs only while the PC is calm, on its own thread and connection with an `InFlight` guard.

- **What it reads.** Files of a readable type up to 50 MB, at most the first 64 KB of extracted text per file. Text goes into the existing `chunks`/FTS tables; metadata goes into a new `file_meta` table.
- **Error handling.** Each file is read in isolation: a panic or a 10-second timeout marks the file `unreadable` and moves on.
- **Resuming.** It resumes across restarts and re-plugs, using a new `files.read_at`.
- **Extractors (builder's choice, pure Rust):**

| Extractor | File types |
|---|---|
| `pdf-extract` | PDF |
| `zip` + `quick-xml` | DOCX, PPTX, ODT |
| `calamine` | XLSX, XLS, ODS (sheet names + first 200 cells) |
| `kamadak-exif` | JPEG, HEIC, TIFF (date taken, camera, GPS presence) |
| `lofty` | MP3, M4A, FLAC (artist, album, title) |
| Plain read | TXT, MD, CSV, RTF (control words stripped) |

### 7.2 Classification

A deterministic, explainable classifier. Every decision carries evidence strings, for example "content mentions 'Tax invoice'", "VAT number found", "sender: Vodacom".

**Kind**, from the extension plus magic bytes:

- document
- photo
- screenshot (PNG/JPG whose name, size or EXIF suggests a screenshot)
- video
- music
- audio
- code (inside an atomic unit or a code extension)
- installer
- archive
- font
- system (dll, cab, cat, mst, pyc, and the like): never filed on its own
- other

**Category** (documents), from weighted keyword features on content and name. A scored dictionary per category and type is kept in code and can be extended:

- Finance/Banking: statement, account number, balance
- Finance/Tax: SARS, IRP5, assessment, tax return
- Finance/Invoices: invoice, tax invoice, VAT, amount due
- Finance/Payslips: payslip, gross, nett, PAYE
- Finance/Insurance: policy, premium, claim
- Personal/ID: identity, passport, birth certificate
- Personal/Health: medical aid, prescription, patient
- Personal/Vehicles: licence disc, VIN, registration
- Work: CV, contract of employment, timesheet, plus employer names learned from approvals
- Study: assignment, module, lecture, transcript, plus institution names
- Projects: matching an existing project name

**Confidence.** The scores are normalised and combined into a single 0–1 value. The canonical bands apply:

| Confidence | What happens |
|---|---|
| ≥ 0.90 | Eligible for automatic filing (once a rule has graduated) |
| 0.60–0.89 | Proposed, with evidence |
| < 0.60 | Goes to **To sort**, never moved by a proposal |

**Photos, video, music.**

- Photos go to `Photos\<year>\<year>-<month>`, using the EXIF date, else the file date (lower confidence).
- Videos:
  - Course videos are detected by a folder that holds many numbered or lesson-named videos plus `.srt` files. They stay together as one unit under `Videos\Courses\<folder name>`.
  - Personal videos are those with camera metadata or `VID_`/`IMG_` names.
- Music goes to `Music\<artist>\<album>` from its tags.

**Groups stay together.**

- Atomic units always.
- Course folders.
- Photo folders with an event name, for example "Cape Town Dec 2023": these keep their folder name under the year.

### 7.3 Who/what

- **Organisations:** a built-in list of common South African and international organisations (banks, telcos, insurers, SARS, municipalities, universities, big retailers), plus names the owner has approved in earlier batches (learned).
- **People:** the owner's own name is excluded.
- **No match:** the who is left blank and the subject is used instead.

## 8. Blueprint choice and migration

1. **Ready to plan.** After a drive is catalogued and read (or the PC folders indexed and read), the Filing screen shows **"Plan this drive"**.
2. **Blueprints.** Aegis builds both blueprints. Each is a list of proposed destinations per file and group, which is a plan, not yet any batches.
3. **Side-by-side choice.** Two trees, each showing:
   - the top two levels with file counts and sizes;
   - "Moves: N files · Renames: M · To sort: K";
   - a few example renames.

   The owner picks one. The choice is stored per drive (`filing_blueprints`).
4. **Migration batches**, cut in this order:
   - loose items at the root;
   - Downloads-style dumps;
   - the biggest top folders;
   - the rest.

   Each batch holds at most 500 actions and covers one source folder or category. Only one batch is offered at a time. The next batch is prepared once the previous one is approved and done, so plans never go stale.
5. **Review screen** for each batch:
   - a tree diff (old → new);
   - a confidence pill per item (green ≥ 0.90, amber 0.60–0.89);
   - "Why?" with the evidence;
   - untick an item to leave it where it is;
   - **Approve all** / **Approve ticked** / **Skip this folder**.
6. **Execution** uses the existing execute path: verified moves, journaled, with collision suffixes. Afterwards the original names are recorded in `file_history`.
7. **Undo.** Any batch can be undone from the Filing screen or Simulator & Actions, for as long as it exists. Moves have no time limit.
8. **Progress.** The Filing screen shows per drive: "Filed 4,200 of 31,000 files · 12 batches left · To sort: 340".

## 9. Learn-then-take-over

**What a rule is.** `(kind, category, who?, source scope) → destination template + naming on/off`. For example: *Finance/Invoices from Vodacom, anywhere on this PC → `Documents\Finance\Invoices & receipts\<year>`, renamed*.

**Rule lifecycle.**

| Event | Effect |
|---|---|
| Owner approves a move in a batch | Increments `approvals` on the matching rule (created on first approval) |
| Owner unticks or rejects the item | Records a rejection and resets the rule to **asking** with approvals = 0 |
| Owner undoes an automatic move | Same as a rejection |
| 3 approvals with no rejection since | Rule becomes **automatic** |

**What automatic means.** New matching files with confidence ≥ 0.90 are filed by the engine with no question. They still go through a batch that is executed and audited, so each can be undone. The weekly note gets a line "Aegis filed 23 files by itself" with a link to the list.

**Scope rules.**

- A rule learned on one drive applies only to that drive and the PC until the owner approves it once on another drive.
- Migration batches on existing data count towards graduation too.

**Rules screen.** Lists each rule with what it matches, where it files, the state (Asking or Automatic, plus its approval count), and **Stop automatic** / **Delete**.

## 10. Ongoing filing (engine integration)

A new **filing watch** (scheduler thread, calm only) looks at files that arrived since its last run in Downloads, the Desktop, Pictures, and the root of each general drive.

- **Settle time.** Files must be unchanged for 10 minutes, and partial downloads are skipped.
- **Automatic rules.** Matching files are filed immediately, as described in §9.
- **Other confident files (≥ 0.60).** Collected into one "File N new items" batch. That produces a Note-level finding in the weekly note, or a Convenient reminder once 20 or more are waiting.
- **Below 0.60.** Go to To sort.
- **Never touched:** files open in another program (sharing violation, so retry next run), files under 2 minutes old, and anything inside an atomic unit.

## 11. Finding

- **Search** uses the existing full-text index, now fed by extracted content, plus current names and `file_history` former names.
- **Results** show the current path. When a file was moved by Aegis, they also say "was: `<old name>` in `<old folder>`".
- **Filters:** category, who, year, kind, and drive (including unplugged drives, from the catalog).

## 12. Data model (migration 0005)

- `file_meta(file_id PK, kind, category, who, doc_type, doc_date, title, exif_taken_at, camera, artist, album, confidence, evidence_json, read_at, read_error)`
- `file_history(id, file_id, old_path, new_path, batch_id, changed_at)`, plus an index on `old_path` for search.
- `filing_blueprints(volume_id PK, choice 'tidy'|'fresh', chosen_at)`
- `filing_rules(id, scope_volume_id NULL = PC, kind, category, who NULL, dest_template, rename INT, approvals INT, rejections INT, state 'asking'|'automatic', updated_at)`
- `filing_decisions(id, rule_id, file_id, batch_id, outcome 'approved'|'rejected'|'undone', at)`
- Batches reuse `action_batches` and `actions` with agent `filing`. Each action's `evidence_json` carries the rule id, confidence and evidence.

## 13. Safety (must never)

- Move or rename anything inside an atomic unit, the Backup drive, `.aegis-*`, Windows or Program Files, or AppData.
- Move between drives.
- Overwrite.
- Act on files below confidence 0.60.
- Open cloud-only placeholders.
- Apply automatic rules on a drive the owner hasn't chosen a blueprint for.
- Do anything while the owner hasn't approved at least one batch on that drive.
- Delete anything. Filing only moves and renames, and empty folders left behind are only removed when Aegis created them or emptied them itself.

## 14. UI

New **Filing** screen (nav item "Filing"):

1. Per drive, a card showing the PC folders and each general drive: status (Not planned / Planning… / Blueprint chosen / Filing N% / Done) and a **Plan this drive** button.
2. Blueprint choice: two trees side by side with counts, example renames and a **Use this structure** button.
3. Batch review: as in §8.5.
4. **To sort:** uncertain items with quick buttons (top 3 suggested destinations, Leave here).
5. **Rules:** as in §9.

Home's "Needs you" gains "Next filing batch ready (N files)". The weekly note gains "What Aegis filed by itself".

## 15. Approaches considered

- **A. Deterministic rules plus content keywords (chosen).** Explainable, fast, no model to download, and it works on every hardware tier. It may misjudge unusual documents. Those land in To sort, and approvals teach rules.
- **B. Local embeddings and a small classifier (ONNX).** Better on unusual documents, but it needs a model download (~90 MB+) and a calibration set the owner doesn't have yet, and its decisions are harder to explain. It can come later behind the same `classify` interface.
- **C. A local LLM.** Most flexible, but slow on 200k files, heavy on RAM, and the policy specs keep it off for bulk work.

## 16. Builder's calls (summary)

- The fresh structure in §5, including the folder names.
- The extractor crates.
- The thresholds: 50 MB, 64 KB, 10 s, 0.80 for renaming, 500 per batch, 10-minute settle time, 20 for the reminder.
- Courses kept whole, and leftovers under "From old PCs".
- Rules scoped to the drive and the PC until approved elsewhere.
- Only one batch offered at a time.

## 17. Build order

1. Reader and extractors, with `file_meta`.
2. Classifier and who/what.
3. Naming.
4. Blueprint builder (tidy and fresh).
5. Batch cutter and history.
6. Rules and learning.
7. Filing watch.
8. Search with history.
9. UI.
10. Live dry run on the owner's real drives into a scratch database, then review.
