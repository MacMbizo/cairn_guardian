# Auto-filing Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Aegis reads document content, classifies every file, offers each drive a "tidy" or "fresh" folder structure with a naming scheme, migrates in approved batches, learns rules that graduate to automatic filing, and makes everything findable by content and former name.

**Architecture:** A new `aegis_core::filing` module holds all the logic and is testable:
- `extract`
- `reader`
- `classify`
- `orgs`
- `naming`
- `blueprint`
- `batches`
- `rules`
- `watch`

Moves and renames reuse the existing action batches: `propose_batch`, `simulate`, `approve_batch`, `execute_batch` and `rollback_batch`. The engine scheduler (Project 2) runs the reader and the filing watch. The desktop crate adds IPC commands, and the UI adds a Filing screen.

**Tech Stack:** Rust. New crates: `pdf-extract`, `zip`, `quick-xml`, `calamine`, `kamadak-exif`, `lofty`. Plus Tauri 2, React/TS and ts-rs bindings.

**Spec:** `docs/superpowers/specs/2026-11-21-auto-filing-design.md`

## Global Constraints

- **Confidence bands:** ≥ 0.90 is eligible for automatic filing; 0.60–0.89 is proposed; < 0.60 goes to **To sort** and is never part of a move.
- **Renaming** only when date and type confidence are both ≥ 0.80. The format is `<YYYY-MM-DD> <who/what> <type>.<ext>`. Screenshots become `<YYYY-MM-DD> <HH.MM.SS> Screenshot.<ext>`. Names are capped at 120 characters, with Windows-forbidden characters `<>:"/\|?*` and control characters removed. Collisions get ` (2)`, ` (3)` and so on; nothing is ever overwritten.
- **Reader limits:** files ≤ 50 MB; at most the first 64 KB of extracted text; 10 s per file; a panic or timeout marks the file `unreadable`.
- **Batches:** at most **500** actions, one source folder or category per batch, and only **one** filing batch offered per scope at a time.
- **Graduation:** 3 approvals with no rejection since make a rule automatic. One rejection, untick or undo resets it to asking with approvals = 0.
- **Rule scope:** learned on one drive, a rule applies to that drive and the PC until the owner approves it once on another drive.
- **Ongoing filing:** files must be unchanged for 10 min before filing; never files under 2 min old; a reminder once **20** or more items wait.
- **Never:**
  - touch anything inside an atomic unit (units move only whole), the Backup drive, `.aegis-*` folders, Windows, Program Files or AppData;
  - move between drives;
  - open cloud-only placeholders;
  - auto-file on a scope without a chosen blueprint, or before at least one approved batch there;
  - delete anything (only folders Aegis emptied itself are removed).
- **The fresh structure's folder names** are exactly as in spec §5. On the PC, Windows' known folders map as follows: Documents → `Documents`, Photos and Screenshots → `Pictures`, Videos → `Videos`, Music and Audio → `Music`, Software and Archives → `Downloads`.
- **Shared rules for all tasks:**
  - Owner-facing copy is plain English with no internal names.
  - Windows-only code sits behind `cfg(windows)`.
  - Add crates via PowerShell with the sandbox disabled, and check `Cargo.toml` changed.
  - Write Rust and TS with the Write/Edit tools, not shell heredocs (they mangle backslashes).
  - Commit only bindings whose content changed.
  - Tests use scratch folders and in-memory stores, never the owner's data.

## Review Focus

1. **The owner moves or renames a file between the plan and approving the batch.** Expected: that action fails cleanly, the rest of the batch completes, and no history is recorded for it. Test: `moved_away_file_fails_alone` (Task 7).
2. **Two files in one batch would get the same new name.** Expected: the second gets ` (2)` and both survive. Test: `same_target_name_in_one_batch_gets_suffix` (Task 7).
3. **A malformed or huge PDF hangs or panics the extractor.** Expected: that file is marked unreadable and the reader carries on. Test: `bad_pdf_is_marked_unreadable_and_reader_continues` (Task 3).
4. **The drive is unplugged during an approved batch.** Expected: the remaining actions fail without loss, completed moves are kept and recorded, and the batch can be retried. Test: `partially_executed_batch_records_only_done_moves` (Task 7).
5. **A very long or hostile organisation name, or one with forbidden characters.** Expected: the name is sanitised to ≤ 120 characters and stays valid on Windows. Test: `names_are_sanitised_and_capped` (Task 5).

---

## File Structure

| File | Responsibility |
|---|---|
| `crates/aegis-core/migrations/0005_filing.sql` | `file_meta`, `file_history`, `filing_blueprints`, `filing_rules`, `filing_decisions`, `files.read_at` |
| `crates/aegis-core/src/filing/mod.rs` | Types: `Kind`, `Category`, `FileMeta`, `Scope`, `BlueprintChoice`; meta store/load |
| `crates/aegis-core/src/filing/extract.rs` | Per-format extractors → `Extracted` |
| `crates/aegis-core/src/filing/reader.rs` | Reads pending files in isolation, stores text chunks and meta; `ReaderWatch` |
| `crates/aegis-core/src/filing/orgs.rs` | Built-in organisation list plus learned names |
| `crates/aegis-core/src/filing/classify.rs` | Kind, category, doc type, who, date, confidence, evidence |
| `crates/aegis-core/src/filing/naming.rs` | Proposed names and sanitising |
| `crates/aegis-core/src/filing/blueprint.rs` | Groups (units, courses, events), tidy and fresh placements, tree summary |
| `crates/aegis-core/src/filing/batches.rs` | Next batch, post-execute history and reindex, decisions, rollback hook |
| `crates/aegis-core/src/filing/rules.rs` | Rule keys, approvals and rejections, graduation, automatic lookup |
| `crates/aegis-core/src/filing/watch.rs` | `FilingWatch` for new files |
| `crates/aegis-core/src/db/files.rs` | Search over names plus `file_history`; `SearchHit.was` |
| `crates/aegis-core/tests/filing.rs` | Integration tests for Tasks 1–10 |
| `crates/aegis-core/tests/fixtures_filing/mod.rs` | Helpers that build tiny DOCX/XLSX/PDF/JPEG-EXIF/MP3 files in temp folders |
| `canonical/C4_Policy_Engine_Confidence_Spec.md` | Amendment §7: graduated filing rules |
| `src-tauri/src/commands/filing.rs`, `src-tauri/src/engine_host.rs` | IPC; register `ReaderWatch` and `FilingWatch` |
| `src/screens/Filing.tsx`, `src/screens/useFiling.ts`, `src/screens/fixtures/filing.ts` | Filing UI |
| `src/App.tsx`, `src/components/HomeTiles.tsx`, `src/bindings/client.ts` | Navigation, the "Next filing batch ready" item, client calls |

---

### Task 1: Schema and filing types

**Files:**
- Create: `crates/aegis-core/migrations/0005_filing.sql`, `crates/aegis-core/src/filing/mod.rs`, `crates/aegis-core/tests/filing.rs`
- Modify: `crates/aegis-core/src/db/mod.rs` (migration 5), `crates/aegis-core/src/lib.rs`, `crates/aegis-core/tests/drives.rs` (versions `[1,2,3,4,5]`)

**Interfaces (Produces):**
- `#[derive(TS, Serialize, Deserialize, Clone, Copy, PartialEq, Eq, Debug, Hash)] #[serde(rename_all="snake_case")] pub enum Kind { Document, Photo, Screenshot, Video, Music, Audio, Code, Installer, Archive, Font, System, Other }`
- `pub struct FileMeta { file_id: String, kind: Kind, category: Option<String>, doc_type: Option<String>, who: Option<String>, doc_date: Option<String>, title: Option<String>, exif_taken_at: Option<i64>, camera: Option<String>, artist: Option<String>, album: Option<String>, confidence: f64, evidence: Vec<String>, read_at: Option<i64>, read_error: Option<String> }` (TS)
- `pub fn save_meta(conn: &Connection, m: &FileMeta) -> Result<()>` (upsert) and `pub fn load_meta(conn: &Connection, file_id: &str) -> Result<Option<FileMeta>>`
- `#[derive(TS)] pub enum Scope { Pc, Volume { volume_id: String } }` (serde `tag="type"`), `pub enum BlueprintChoice { Tidy, Fresh }` (TS)
- `pub fn set_blueprint(conn, scope: &Scope, choice: BlueprintChoice, now: i64) -> Result<()>` and `pub fn blueprint_of(conn, scope: &Scope) -> Result<Option<BlueprintChoice>>`. The `filing_blueprints` key is `"pc"` or the volume id.

**Schema (0005_filing.sql):**
- `file_meta(file_id TEXT PK REFERENCES files(id) ON DELETE CASCADE, kind TEXT NOT NULL, category TEXT, doc_type TEXT, who TEXT, doc_date TEXT, title TEXT, exif_taken_at INT, camera TEXT, artist TEXT, album TEXT, confidence REAL NOT NULL DEFAULT 0, evidence_json TEXT NOT NULL DEFAULT '[]', read_at INT, read_error TEXT)`
- `file_history(id TEXT PK, file_id TEXT, old_path TEXT NOT NULL, new_path TEXT NOT NULL, batch_id TEXT, changed_at INT NOT NULL)`, with `CREATE INDEX idx_file_history_old ON file_history(old_path)` and `… (file_id)`
- `filing_blueprints(scope_key TEXT PK, choice TEXT NOT NULL, chosen_at INT NOT NULL)`
- `filing_rules(id TEXT PK, scope_key TEXT NOT NULL, rule_key TEXT NOT NULL, kind TEXT NOT NULL, category TEXT, who TEXT, dest_template TEXT NOT NULL, rename INT NOT NULL, approvals INT NOT NULL DEFAULT 0, rejections INT NOT NULL DEFAULT 0, state TEXT NOT NULL DEFAULT 'asking', approved_scopes_json TEXT NOT NULL DEFAULT '[]', updated_at INT NOT NULL, UNIQUE(scope_key, rule_key))`
- `filing_decisions(id TEXT PK, rule_id TEXT, file_id TEXT, batch_id TEXT, outcome TEXT NOT NULL, at INT NOT NULL)`

Tests (Step 1):
- `meta_round_trips`: `save_meta` then `load_meta` gives an equal struct; a second save updates the row.
- `blueprint_choice_per_scope`: `Pc → Fresh` and `Volume{"v1"} → Tidy` are stored independently, and an unknown scope gives `None`.

Steps:
- [ ] Step 1: Write the tests.
- [ ] Step 2: Run them (expect a FAIL to compile).
- [ ] Step 3: Implement.
- [ ] Step 4: `cargo test -p aegis-core --test filing --test drives` and expect PASS.
- [ ] Step 5: Commit `feat(filing): schema and types`.

### Task 2: Extractors

**Files:**
- Create: `crates/aegis-core/src/filing/extract.rs`, `crates/aegis-core/tests/fixtures_filing/mod.rs`
- Modify: `crates/aegis-core/Cargo.toml` (`pdf-extract`, `zip`, `quick-xml`, `calamine`, `kamadak-exif`, `lofty`; `zip` also as a dev-dependency for the fixtures)

**Interfaces (Produces):**
- `pub struct Exif { pub taken_at: Option<i64>, pub camera: Option<String>, pub has_gps: bool }`
- `pub struct Tags { pub artist: Option<String>, pub album: Option<String>, pub title: Option<String> }`
- `pub struct Extracted { pub text: Option<String>, pub exif: Option<Exif>, pub tags: Option<Tags> }`
- `pub const MAX_TEXT_BYTES: usize = 65_536; pub const MAX_FILE_BYTES: u64 = 52_428_800;`
- `pub fn readable(ext: &str) -> bool` covers pdf, docx, pptx, odt, xlsx, xls, ods, txt, md, csv, rtf, jpg, jpeg, heic, tif, tiff, mp3, m4a, flac.
- `pub fn extract(path: &Path) -> Result<Extracted>`. It dispatches on the lowercase extension and truncates text at a char boundary ≤ `MAX_TEXT_BYTES`. XLSX returns sheet names plus the first 200 non-empty cells, joined with spaces. RTF strips `{\…}` control words.
- Fixtures (test helpers):
  - `docx(dir, name, text) -> PathBuf` (zip with `word/document.xml` `<w:t>text</w:t>`)
  - `xlsx(dir, name, cells: &[&str])` (zip with a minimal workbook, sharedStrings and sheet1)
  - `pdf(dir, name, text)` (a minimal one-page PDF with a Helvetica text object)
  - `jpeg_with_exif(dir, name, "2024:03:14 10:22:05", camera)` (SOI + APP1 Exif TIFF IFD with DateTimeOriginal and Model)
  - `mp3_with_tags(dir, name, artist, album, title)` (ID3v2.3 TPE1/TALB/TIT2)

Tests:
- `docx_text_is_extracted`: the text contains "Tax invoice".
- `xlsx_cells_are_extracted`: it contains "Balance" and "1234.50".
- `pdf_text_is_extracted`: it contains "Statement".
- `exif_date_and_camera`: `taken_at` equals the epoch for 2024-03-14 10:22:05 (as UTC) and `camera == Some("Pixel 7")`.
- `mp3_tags`: the artist, album and title match.
- `text_is_capped_at_64kb`: a 200 KB .txt gives text length ≤ 65_536.
- `rtf_control_words_stripped`: `{\rtf1\ansi Hello {\b world}}` gives "Hello world" after trimming.

Steps:
- [ ] Step 1: Write the tests.
- [ ] Step 2: Run them and expect FAIL.
- [ ] Step 3: Add the crates, then implement.
- [ ] Step 4: Run them and expect PASS.
- [ ] Step 5: Commit `feat(filing): content extractors`.

### Task 3: Reader

**Files:** Create `crates/aegis-core/src/filing/reader.rs`.

**Interfaces:**
- Consumes: `extract`, `readable`, `save_meta`, `load_meta`, `Kind`.
- Produces:
  - `pub struct ReadReport { pub read: usize, pub unreadable: usize, pub skipped: usize }`
  - `pub fn read_pending(store: &mut SqliteStore, limit: usize, timeout: Duration) -> Result<ReadReport>`. For live, readable, local (non-placeholder), ≤ 50 MB files whose `file_meta.read_at` is null or older than `files.mtime`, it extracts each one on a worker thread joined with `timeout` (catching panics). It replaces the file's `chunks` with the text in ≤ 2,000-char chunks (`token_count` = words), then sets `read_at`, `exif_taken_at`/`camera`, `artist`/`album`/`title`, `kind` (from `classify::kind_of`, Task 4) and `read_error`. A failure stores `read_error`, sets `read_at` and counts as unreadable.
  - `pub struct ReaderWatch` (`impl Watch`: name `reader`, every 600_000 ms, `needs_idle` true, kinds `[]`). Each run processes up to 2,000 files.

Tests:
- `reader_stores_text_and_makes_it_searchable`: a DOCX with "Vodacom tax invoice" is indexed, `read_pending` runs, and `search_fts(conn, "Vodacom", …)` has one hit.
- `bad_pdf_is_marked_unreadable_and_reader_continues`: a garbage `.pdf` plus a good `.docx` give `read == 1`, `unreadable == 1`, and `read_error` set on the bad one.
- `unchanged_files_are_not_read_twice`: a second run gives `read == 0`.
- `big_files_are_skipped`: the file size is set to 60 MB in the row and the file is skipped.

Steps:
- [ ] Step 1: Write the tests.
- [ ] Step 2: Run them and expect FAIL.
- [ ] Step 3: Implement. Tasks 3 and 4 share `kind_of`, so implement `classify::kind_of` (Task 4's interface) here first.
- [ ] Step 4: Run them and expect PASS.
- [ ] Step 5: Commit `feat(filing): background content reader`.

### Task 4: Classifier and organisations

**Files:** Create `crates/aegis-core/src/filing/classify.rs` and `crates/aegis-core/src/filing/orgs.rs`.

**Interfaces (Produces):**
- `pub fn kind_of(ext: &str, name: &str, in_atomic_unit: bool) -> Kind`. Code: inside a unit, or an extension in rs, py, ts, tsx, js, jsx, java, c, cpp, h, cs, go, php, rb, vb, ipynb. System: dll, cab, cat, mst, pyc, pyi, sys, ini, dat, cfg, resources, manifest, msp, tmp, log. Installer: exe, msi, msix. Archive: zip, rar, 7z, tar, gz. Screenshot: png or jpg whose name contains "screenshot" or "screen shot", or matches `^Screenshot` or `^Capture`.
- `pub struct Classification { pub kind: Kind, pub category: Option<String>, pub doc_type: Option<String>, pub who: Option<String>, pub doc_date: Option<String>, pub date_source: DateSource, pub confidence: f64, pub type_confidence: f64, pub evidence: Vec<String> }`
- `pub enum DateSource { Content, Name, Exif, Modified, None }`
- `pub struct ClassifyInput<'a> { pub name: &'a str, pub ext: &'a str, pub text: Option<&'a str>, pub exif_taken_at: Option<i64>, pub mtime: i64, pub in_atomic_unit: bool, pub learned_orgs: &'a [String] }`
- `pub fn classify(input: &ClassifyInput) -> Classification`
- `pub fn find_org(text: &str, learned: &[String]) -> Option<String>` returns the canonical display name, matching case-insensitively on word boundaries.
- `pub fn find_date(text: &str) -> Option<String>` returns `YYYY-MM-DD` from `2024-03-14`, `14/03/2024`, `14 March 2024`, `March 14, 2024` or `20240314`, rejecting impossible dates. When there are several, it prefers one next to "date", "invoice date" or "statement date".

**Categories and dictionaries:** a `const` table `(category, doc_type, &[(keyword, weight)])` per spec §7.2:

| Category | Doc type |
|---|---|
| Finance/Banking | statement |
| Finance/Tax | assessment / tax return |
| Finance/Invoices & receipts | invoice / receipt / quote |
| Finance/Payslips | payslip |
| Finance/Insurance | policy / claim |
| Finance/Investments | statement |
| Personal/ID & certificates | ID / certificate |
| Personal/Health | — |
| Personal/Home | — |
| Personal/Vehicles | — |
| Personal/Family | — |
| Personal/Travel | — |
| Work | contract / CV / timesheet / letter / report |
| Study | assignment / notes / transcript / certificate |

**Confidence:** best score ÷ (best + second + 1.0), clamped to 0–1. A filename-only match is capped at 0.75. A matching organisation adds +0.15, up to 1.0. Non-documents get 0.95 when the kind is unambiguous (photo with EXIF, music with tags, installer, code); photos without EXIF get 0.75.

**Organisations:** about 80 built-in names, for example Vodacom, MTN, Telkom, Cell C, FNB, ABSA, Standard Bank, Nedbank, Capitec, Discovery, Old Mutual, Sanlam, Momentum, Outsurance, SARS, City of Johannesburg, City of Cape Town, Eskom, UNISA, Wits, UCT, Takealot, Checkers, Woolworths, Pick n Pay, Microsoft, Google, Apple, Amazon, PayPal, Netflix, Uber.

Tests:
- `tax_invoice_from_vodacom`: the text "TAX INVOICE Vodacom (Pty) Ltd Invoice date: 14/03/2026 VAT No … Amount due" gives Finance/Invoices & receipts, doc_type "invoice", who "Vodacom", doc_date "2026-03-14", `date_source` Content, confidence ≥ 0.90.
- `sars_assessment`: Finance/Tax, "assessment", who "SARS".
- `filename_only_is_capped`: the name "payslip march.pdf" with no text gives category Finance/Payslips and confidence ≤ 0.75.
- `unclear_document_is_below_060`: the text "hello world notes" gives confidence < 0.60.
- `kinds`: `kind_of("dll",…)` is System; `kind_of("png","Screenshot 2024-01-02 101010.png",false)` is Screenshot; `kind_of("rs","main.rs",false)` is Code; `kind_of("txt","x.txt",true)` is Code.
- `dates_parse_and_reject_impossible`: "31/02/2024" gives None; "March 14, 2024" gives "2024-03-14"; "20240314" gives "2024-03-14".

Steps:
- [ ] Step 1: Write the tests.
- [ ] Step 2: Run them and expect FAIL.
- [ ] Step 3: Implement.
- [ ] Step 4: Run them and expect PASS.
- [ ] Step 5: Commit `feat(filing): classifier and organisations`.

### Task 5: Naming

**Files:** Create `crates/aegis-core/src/filing/naming.rs`.

**Interfaces (Produces):**
- `pub fn sanitize(name: &str) -> String`: removes `<>:"/\|?*` and control characters, collapses whitespace, trims dots and spaces at the end, and caps at 120 characters (keeping the extension).
- `pub fn proposed_name(c: &Classification, original_stem: &str, ext: &str, mtime: i64) -> Option<String>`:
  - Documents: only when `type_confidence ≥ 0.80`, the date source is Content, Name or Exif, and a doc_type exists. Format `"{date} {who or title-cased subject} {doc_type}.{ext}"`. The subject is `title`, or the original stem stripped of dates, digits-only tokens and "(1)".
  - Screenshots: `"{date} {HH.MM.SS} Screenshot.{ext}"`, using the date and time from the name if present, otherwise from mtime.
  - Everything else: `None`.
  - When the proposal equals the original name (ignoring case), returns `None`.

Tests:
- `invoice_name`: gives `"2026-03-14 Vodacom invoice.pdf"`.
- `no_rename_when_date_only_from_mtime`: gives None.
- `no_rename_below_080`: gives None.
- `screenshot_name`: "Screenshot 2024-01-02 101010.png" gives "2024-01-02 10.10.10 Screenshot.png".
- `names_are_sanitised_and_capped`: who = `"A<B>C:D|E" + "x".repeat(300)` gives a result with no forbidden characters, length ≤ 120, still ending in ".pdf".

Steps:
- [ ] Step 1: Write the tests.
- [ ] Step 2: Run them and expect FAIL.
- [ ] Step 3: Implement.
- [ ] Step 4: Run them and expect PASS.
- [ ] Step 5: Commit `feat(filing): naming scheme`.

### Task 6: Blueprints

**Files:** Create `crates/aegis-core/src/filing/blueprint.rs`.

**Interfaces:**
- Consumes: `FileMeta`, `Classification` (via stored meta), atomic units, `proposed_name`, `Scope`, `BlueprintChoice`.
- Produces:
  - `#[derive(TS)] pub enum Target { File { file_id: String }, Group { path: String, files: usize } }`
  - `#[derive(TS)] pub struct Placement { pub target: Target, pub from: String, pub to: Option<String> /* None = stays */, pub new_name: Option<String>, pub confidence: f64, pub reason: String, pub rule_key: Option<String>, pub to_sort: bool }`
  - `#[derive(TS)] pub struct TreeNode { pub name: String, pub files: i64, pub bytes: i64, pub children: Vec<TreeNode> }` (the top two levels)
  - `#[derive(TS)] pub struct BlueprintSummary { pub choice: BlueprintChoice, pub tree: Vec<TreeNode>, pub moves: i64, pub renames: i64, pub to_sort: i64, pub stays: i64, pub examples: Vec<(String, String)> /* ≤ 6 old → new */ }`
  - `pub fn scope_roots(store: &SqliteStore, scope: &Scope) -> Result<Vec<(String /*root path*/, Option<KnownFolder>)>>`:
    - PC: the enabled roots, each tagged as a known folder (Documents, Pictures, Music, Videos, Downloads, Desktop).
    - Volume: the mount point.
    - The Backup drive returns `Err(InvalidInput("The Backup drive keeps its own layout"))`.
  - `pub fn plan(store: &SqliteStore, scope: &Scope, choice: BlueprintChoice) -> Result<Vec<Placement>>`
  - `pub fn summarize(placements: &[Placement], choice: BlueprintChoice, store: &SqliteStore) -> Result<BlueprintSummary>`
  - `pub fn rule_key(m: &FileMeta) -> String` = `"{kind}|{category or -}|{who or -}"`.

**Groups:** these become `Target::Group` and are never split:
- every atomic unit → `Code\<unit folder name>`;
- course folders: ≥ 5 videos plus (any .srt, or ≥ 60% of video names starting with a number or containing "lesson", "lecture", "module" or "part") → `Videos\Courses\<folder name>`;
- event photo folders: ≥ 10 photos and a folder name that isn't a year, date or "DCIM" or "Camera" → `Photos\<year of the median photo>\<folder name>`.

**Fresh:** each file maps by kind and category to the spec §5 paths:
- Documents with category C → `Documents\<C with "/" as "\">[\<who> for Work/Study]`. Finance subfolders add `\<year of doc_date>` for Invoices & receipts, Banking and Payslips.
- Photo → `Photos\<yyyy>\<yyyy>-<mm>`.
- Screenshot → `Screenshots\<yyyy>`.
- Video → `Videos\Personal\<yyyy>`, or `Videos\Movies & shows` when there's no camera metadata and the file is > 300 MB.
- Music → `Music\<artist>\<album>`, or `Music\Unknown artist` without tags.
- Audio → `Audio`.
- Installer → `Software\Installers`.
- Archive → `Archives`.
- Font → `Software\Fonts`.
- System or Other files directly inside a top folder that isn't otherwise classified stay put. If that top folder is being dissolved, they go under `From old PCs\<top folder>`.
- Confidence < 0.60 → `To sort` (`to_sort = true`, and the file stays where it is until the owner chooses).

A known folder on the PC rewrites the top level: Documents\… into the Documents root, Photos and Screenshots into Pictures, and so on. A file already at its destination gets `to: None`.

**Tidy:**
- Keep the top folders.
- Merge pairs whose names, lowercased, equal another top folder's name after stripping prefixes such as "msi", "old ", "copy of ", "backup " and digits. The smaller folder moves into the larger.
- Loose root files go to the top folder that the fresh mapping's first segment points to, creating it if absent.
- Folder names are Title Cased only when they are all lower case.
- Renames apply as in fresh.

**Never placed:** anything under the Backup role, `.aegis-*`, `$RECYCLE.BIN`, `System Volume Information`, `Windows`, `Program Files*`, or `AppData`.

Tests (they build a scratch drive with files plus metadata via `save_meta`):
- `fresh_places_documents_photos_and_installers`: an invoice goes to `Documents\Finance\Invoices & receipts\2026\2026-03-14 Vodacom invoice.pdf`; a photo with EXIF 2024-03-14 goes to `Photos\2024\2024-03`; setup.exe goes to `Software\Installers`.
- `atomic_unit_moves_whole`: a repo folder with `.git` gives one `Group` placement to `Code\<name>` and no `File` placements inside it.
- `course_folder_kept_together`: 6 `01 Intro.mp4`… plus `.srt` files give a `Group` to `Videos\Courses\<name>`.
- `low_confidence_goes_to_sort`.
- `tidy_merges_msidocuments_into_documents_and_files_loose_items`.
- `pc_scope_maps_to_known_folders`: on PC roots Documents and Pictures, a photo found in Downloads gets `to` under the Pictures root.
- `backup_drive_is_refused`.
- `summary_counts_and_examples`: moves, renames and to_sort are counted correctly, and examples ≤ 6.

Steps:
- [ ] Step 1: Write the tests.
- [ ] Step 2: Run them and expect FAIL.
- [ ] Step 3: Implement.
- [ ] Step 4: Run them and expect PASS.
- [ ] Step 5: Commit `feat(filing): tidy and fresh blueprints`.

### Task 7: Batches, history and decisions

**Files:** Create `crates/aegis-core/src/filing/batches.rs`.

**Interfaces:**
- Consumes: `plan`, `blueprint_of`, `propose_batch`, `execute_batch`, `index_changed` (engine batcher), the rules functions from Task 8 (`record_outcome`). Land a stub in Task 7 that Task 8 replaces, the same way Project 2's Task 2 did.
- Produces:
  - `pub const BATCH_MAX: usize = 500;`
  - `pub fn open_filing_batch(conn, scope: &Scope) -> Result<Option<String>>`: a draft or approved batch with origin `"filing:<scope_key>"`.
  - `pub fn next_batch(store: &mut SqliteStore, scope: &Scope) -> Result<Option<String>>`:
    - It returns `None` when there's no blueprint or a batch is already open.
    - Otherwise it re-plans and groups the moves by (source top folder, destination category) in this order: loose root items, then folders named like downloads, then the largest by bytes. It takes the first group, up to 500 placements, skipping `to_sort` and stays, and proposes it.
    - Each placement becomes a Move action: `target_kind` File (with file_id) or Dir (Group, no file_id), `agent: Some("filing")`, `to_path = dest folder + (new_name or original name)`, `reason` = the placement reason, `confidence`, and `evidence_json = {"rule_key", "new_name", "from_name"}`.
    - Within the batch, duplicate `to_path` values get ` (2)` and so on before proposing.
  - `pub fn after_execute(store: &mut SqliteStore, batch_id: &str) -> Result<()>`. It must be called after `execute_batch` on a filing batch. For each action:
    - `executed` → a `file_history` row (the old and new file path; for a Dir, one row per indexed file under it, with paths rebased), then `index_changed` on the old and new dir paths;
    - `executed` → `record_outcome(rule_key, approved)`;
    - still `proposed` (unticked) → mark `rejected` and `record_outcome(rejected)`;
    - `failed` → nothing.

    It then removes empty folders that this batch emptied (only when empty).
  - `pub fn after_rollback(store: &mut SqliteStore, batch_id: &str) -> Result<()>` → `record_outcome(rule_key, undone)` for each rolled-back action, and deletes the matching `file_history` rows.

Tests:
- `first_batch_is_loose_root_items_capped_at_500`: 620 loose classified files give a first batch of 500 actions, and a second `next_batch` gives `None` while the first is open.
- `same_target_name_in_one_batch_gets_suffix`.
- `executed_batch_records_history_and_old_name_is_findable`: approve and execute, call `after_execute`, and `file_history` has the old path.
- `moved_away_file_fails_alone`: one source file is renamed before execute; that action is failed, the others are executed, and history covers only the executed ones.
- `partially_executed_batch_records_only_done_moves`: one action is forced to fail (its source deleted); `after_execute` creates no history for it, and the batch can still be rolled back.
- `group_move_reindexes_children`: after moving a course folder, the children's `files.path` values are under the new folder.
- `unticked_item_counts_as_rejection`.
- `rollback_counts_as_undo`.

Steps:
- [ ] Step 1: Write the tests.
- [ ] Step 2: Run them and expect FAIL.
- [ ] Step 3: Implement.
- [ ] Step 4: Run them and expect PASS.
- [ ] Step 5: Commit `feat(filing): migration batches and history`.

### Task 8: Rules (learn-then-take-over)

**Files:** Create `crates/aegis-core/src/filing/rules.rs`. Modify `canonical/C4_Policy_Engine_Confidence_Spec.md` (amendment §7).

**Interfaces (Produces):**
- `pub const GRADUATE_AT: i64 = 3;`
- `#[derive(TS)] pub struct FilingRule { pub id: String, pub scope_key: String, pub rule_key: String, pub kind: Kind, pub category: Option<String>, pub who: Option<String>, pub dest_template: String, pub rename: bool, pub approvals: i64, pub rejections: i64, pub state: String, pub approved_scopes: Vec<String> }`
- `pub enum Outcome { Approved, Rejected, Undone }`
- `pub fn record_outcome(conn, scope_key: &str, rule_key: &str, dest_template: &str, rename: bool, outcome: Outcome, now: i64) -> Result<FilingRule>`:
  - It upserts the rule.
  - Approved: `approvals += 1`; add the scope to `approved_scopes`; state becomes `automatic` when `approvals ≥ 3`.
  - Rejected or Undone: `rejections += 1`, `approvals = 0`, `state = 'asking'`.
- `pub fn automatic_rule_for(conn, scope_key: &str, rule_key: &str) -> Result<Option<FilingRule>>`: returns an automatic rule only when `scope_key` is in `approved_scopes`, or `scope_key == "pc"` and the rule's home scope approved it.
- `pub fn list_rules(conn) -> Result<Vec<FilingRule>>`, `pub fn stop_automatic(conn, id) -> Result<()>` (state asking, approvals 0), `pub fn delete_rule(conn, id) -> Result<()>`.
- **C4 amendment §7, "Graduated filing rules":** after 3 owner approvals of the same filing rule, with none rejected since, the engine may approve and execute filing Move actions with confidence ≥ 0.90 for that rule, within the scopes the owner approved it in. It is the same standing-approval pattern as §6. It never covers Quarantine, Purge, atomic-unit interiors, cross-drive moves or the Backup drive, and it is revoked by any rejection or undo.

Tests:
- `three_approvals_graduate`.
- `one_rejection_resets`.
- `undo_resets`.
- `rule_learned_on_drive_not_automatic_on_another_until_approved_there`.
- `pc_rules_apply_to_pc`.

Steps:
- [ ] Step 1: Write the tests.
- [ ] Step 2: Run them and expect FAIL.
- [ ] Step 3: Implement, replacing Task 7's stub, and write the amendment.
- [ ] Step 4: Run them and expect PASS.
- [ ] Step 5: Commit `feat(filing): learn-then-take-over rules`.

### Task 9: Filing watch (new files)

**Files:** Create `crates/aegis-core/src/filing/watch.rs`.

**Interfaces (Produces):**
- `pub const SETTLE_MS: i64 = 600_000; pub const MIN_AGE_MS: i64 = 120_000; pub const REMIND_AT: i64 = 20;`
- `pub struct FilingWatch` (`impl Watch`: name `filing`, every 900_000 ms, `needs_idle` true, kinds `["filing"]`). It scans these places for live files whose `mtime` is ≤ now − 10 min and whose `first_seen` is ≥ the watch's previous run (or from the start on the first run):
  - PC: Downloads, Desktop and the top level of Pictures;
  - each general, online drive with a blueprint: its root.

  It skips files inside units, partial downloads and placeholders, and requires a chosen blueprint for the scope plus at least one approved batch there. For each file it reclassifies from stored meta and plans that single file with the scope's blueprint:
  - an automatic rule with confidence ≥ 0.90 → collected into one auto batch: `propose_batch`, `approve_batch`, `execute_batch`, `after_execute`;
  - confidence ≥ 0.60 → added to (or creates) the open "new items" batch for that scope;
  - otherwise → to_sort.

  Findings:
  - Note-level `"N new files ready to file"` (subject = scope_key, action `Open{Filing}`);
  - Convenient when N ≥ 20;
  - and an `"Aegis filed N files by itself"` line recorded in `file_history` (batch origin `filing-auto:<scope>`), for the weekly note.
- `Screen` gains a `Filing` variant (engine enum and `TAB_FOR` in the UI).
- `note::build` gains a `did` line: `"Filed {n} files by itself"`, counted from executed actions of `filing-auto:*` batches this week.

Tests:
- `new_invoice_in_downloads_files_itself_when_rule_is_automatic`: a graduated rule plus a fresh blueprint on the PC scope, and a settled invoice in Downloads, end with the file at its fresh destination and a `file_history` row.
- `waits_for_settle_time`.
- `without_blueprint_nothing_happens`.
- `confident_but_not_automatic_is_proposed_not_moved`.
- `note_counts_auto_filed`.

Steps:
- [ ] Step 1: Write the tests.
- [ ] Step 2: Run them and expect FAIL.
- [ ] Step 3: Implement.
- [ ] Step 4: Run them and expect PASS.
- [ ] Step 5: Commit `feat(filing): ongoing filing of new files`.

### Task 10: Search by name and former name

**Files:** Modify `crates/aegis-core/src/db/files.rs` (`search_fts`) and `crates/aegis-core/src/contracts/mod.rs` (`SearchHit` gains `was: Option<String>`).

**Interfaces:** `SearchFilters` gains `category: Option<String>`, `who: Option<String>` and `year: Option<i32>`, matched against `file_meta` (category prefix match, `who` case-insensitive, `year` = the year of `doc_date` or `exif_taken_at`). Add test `filters_by_category_who_and_year`. `search_fts` returns content hits plus name hits (`files.name LIKE %term%`, any term) plus former-name hits (`file_history.old_path LIKE %term%` → the current file). Results are de-duplicated by `file_id`, ranked content > name > former name, and each has `match_reason` of "content", "name" or "former name". `was` = the latest `file_history.old_path` for the file, if any.

Tests:
- `finds_by_name_without_content`.
- `finds_by_former_name_after_filing`: shows `was == old path`.
- `content_hit_ranks_first`.

Steps:
- [ ] Step 1: Write the tests.
- [ ] Step 2: Run them and expect FAIL.
- [ ] Step 3: Implement.
- [ ] Step 4: Run them and expect PASS.
- [ ] Step 5: Commit `feat(search): names and former names`.

### Task 11: Desktop and UI

**Files:**
- Create: `src-tauri/src/commands/filing.rs`, `src/screens/Filing.tsx`, `src/screens/useFiling.ts`, `src/screens/fixtures/filing.ts`
- Modify: `src-tauri/src/lib.rs`, `src-tauri/src/engine_host.rs` (register `ReaderWatch` and `FilingWatch`), `src-tauri/src/commands/mod.rs`, `src/App.tsx`, `src/bindings/client.ts`, `src/components/HomeTiles.tsx`, `src/screens/ThisWeek.tsx` (show `was` in search too: modify the search result rendering in `src/App.tsx`)

**Commands (Rust, `#[tauri::command(async)]`, using the UI store):**
- `filing_scopes() -> Vec<ScopeStatus>`, where `#[derive(TS)] ScopeStatus { scope: Scope, label: String, online: bool, read_pct: f64, blueprint: Option<BlueprintChoice>, open_batch: Option<String>, filed: i64, remaining: i64, to_sort: i64 }`. Defined in core `filing/mod.rs`, with `pub fn scope_statuses(store) -> Result<Vec<ScopeStatus>>`; it includes the PC and the general drives, never the Backup drive.
- `filing_blueprints(scope) -> (BlueprintSummary, BlueprintSummary)`: runs on a worker thread with its own connection (it can be slow).
- `filing_choose(scope, choice)`, `filing_next_batch(scope) -> Option<String>`, `filing_after_execute(batch_id)`, `filing_after_rollback(batch_id)`.
- `filing_to_sort(scope) -> Vec<Placement>`, `filing_place(file_id, dest_folder)`: a one-item batch that is approved and executed at once (the owner chose it).
- `filing_rules() -> Vec<FilingRule>`, `filing_rule_stop(id)`, `filing_rule_delete(id)`.
- `actions_execute` and `actions_rollback` (existing) call `after_execute` / `after_rollback` when the batch's origin starts with "filing".

**UI, Filing screen (nav "Filing", icon `FolderTree`):**
- **Scope cards:** "This PC" and each drive, with the progress line "Filed X of Y · N batches left · To sort: K", and a button that depends on the state: "Plan this drive" / "Review next batch" / "Done".
- **Plan view:** two columns (Tidy | Fresh). Each shows a tree (top two levels, counts and sizes), "Moves N · Renames M · To sort K", up to 6 example renames (old → new), and a "Use this structure" button.
- **Batch review:** each row shows a checkbox, old path → new path (the name diff highlighted), a confidence pill (green ≥ 0.90, amber 0.60–0.89) and a "Why?" toggle showing the reason. Buttons: Approve all / Approve ticked / Skip this folder. Approving calls the existing `approve` (with the selection) and `execute`, then `filing_after_execute`, then shows "Filed N files. Undo" (Undo = `rollback` + `filing_after_rollback`).
- **To sort:** a list with the 3 suggested folders as buttons, plus "Leave here".
- **Rules:** a table with what it matches, where it files, the state ("Asking · 2 of 3 approvals" / "Automatic"), Stop automatic and Delete.
- **Browser preview:** fixtures for every view.
- **Home "Needs you":** "Next filing batch ready (N files)" when there's an open filing batch.
- **Search results:** "was: <old name> in <old folder>" when `was` is set.

Steps:
- [ ] Step 1: `cargo test --workspace` (bindings regenerate).
- [ ] Step 2: Implement the commands and UI.
- [ ] Step 3: `pnpm build` clean; `cargo clippy --workspace --all-targets` clean.
- [ ] Step 4: Playwright screenshots (Filing overview, plan view, batch review, rules) from the Vite server on 1420; move them to the scratchpad and delete `.playwright-mcp/`.
- [ ] Step 5: Commit `feat(ui): filing screen`.

### Task 12: Live dry run and handoff

- [ ] Step 1: Run the reader and both blueprints for the PC scope and the "New Volume" drive against a **scratch copy of the owner's database** (`AEGIS_DATA_DIR`, read-only on the drives; no batch executed). Record:
  - read rate and unreadable count;
  - category distribution;
  - % to_sort;
  - moves, renames and to_sort per blueprint;
  - 20 sample renames.

  Fix any crash or obvious misclassification with a failing test first.
- [ ] Step 2: Full suite, clippy and build.
- [ ] Step 3: Final whole-branch review (fresh reviewer), and one fix pass.
- [ ] Step 4: Update `HANDOFF.md` and commit.
