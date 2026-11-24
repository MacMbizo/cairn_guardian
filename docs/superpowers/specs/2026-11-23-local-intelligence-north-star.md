# Local intelligence: north star and binding rules

The owner's vision document, `canonical/local-intelligence-system-architecture.md`, describes fourteen capabilities that make one local-first "intelligence OS". That file is the owner's: it is never committed, and nothing here copies it. This document does three things:
- maps the vision onto Aegis as it is;
- sets the order in which its parts are built;
- records the **binding rules**: what every future design and plan must keep, avoid or reshape.

Written 2026-11-23 from the review given to the owner on 2026-11-22. The owner approved recording it.

## 1. How to read this

- **The vision is the north star, not a restart.** Aegis already has much of the foundation the vision asks for. New capabilities are built onto it, each with a short design of its own, as before.
- **The binding rules (§3) apply to every spec and plan from now on.** Each new design states which of them it touches and how it keeps them. A design that needs to break one must say so and get the owner's decision first.
- **The vision's own safety principle is the spine:** nothing changes without review, every change can be undone, and Aegis proposes while the owner decides. Aegis already works this way. Every capability below plugs into the same pipeline: propose → approve → batch → undo.

## 2. The vision mapped onto Aegis

### 2.1 Foundation (vision §1)

| Vision | Aegis today | Gap / decision |
|---|---|---|
| Open files stay the owner's; metadata in an embedded database | Files stay where the owner keeps them; SQLite store | Fits as is |
| Metadata encrypted at rest | SQLCipher AES-256 (`libtomcrypt`) + DPAPI key sealing (`R2`) + 24-word/64-hex recovery (`merged 2026-11-24`) | **Owner decision D1 approved (2026-11-23)** (§4) and landed in Plan 6A (`feature/d1-encryption-and-vec`, merged 2026-11-24) before embeddings/search |
| Key held by the OS security hardware, opened by biometrics | n/a | Reshaped (rule R2): the engine's key is sealed to the PC; biometrics only open sensitive views |
| Fast file tracking | USN journal read without admin, live index, catalog | Fits. Direct MFT parsing is avoided (R1) |
| Background content parsing | Content reader | Fits |
| Local models, NPU/GPU/CPU | `aegis-embed` (`GgufEmbedder`, `SafetensorsBertEmbedder`, `LexicalHashEmbedder` fallback; `OnDemandEmbedder` wired via Plan 6B) | On demand only (R4); pure-Rust CPU via `candle-core`/`candle-nn` (`K5` <= 350 MB, unloads after batch / 60s idle / window close, AC-gated background indexing) |
| Vector index | `vec_chunks` (`sqlite-vec` `0.1.9`) + `chunk_embeddings_meta` inside SQLCipher-encrypted `aegis.db` (Plan 6A + 6B merged 2026-11-24) | Live inside encrypted `aegis.db`; excludes deleted/quarantined (`K2`), repairs orphans via Doctor check #4 |
| Low-priority worker queue, idle- and power-gated | Scheduler thread below normal priority, idle-gated watches, single-flight jobs, AC-power gating (`Watch::needs_ac`) | Implemented with AC-power gating for background `EmbedWatch` |
| Revert buffer for every automatic change | Batches, `file_history`, rollback, 30-day quarantine | Fits. Already the core of Aegis |

### 2.2 The fourteen capabilities

| # | Capability (vision §) | Builds on | Order |
|---|---|---|---|
| 1 | Context layer: projects, sets, modules (not a vision item; its prerequisite) | catalog | **done** (Plans 1–5 merged 2026-11-24: projects, plans, sets/photo subjects, put back together, §16 step 7 modules & external drive plans) |
| 2 | Context-aware search (3.2) | Search with filters + embeddings + full-text (FTS5) | **done** (Plan 6A encryption merged; Plan 6B local embeddings & hybrid search merged 2026-11-24) |
| 3 | Local RAG engine (3.1) | hybrid search | after 2 |
| 4 | Dead file detection (4.2) | janitor, duplicates, Project 4 System care | with Project 4 |
| 5 | Workspace health score (4.4) | space recorder, duplicates, self-check | with Project 4; storage and integrity first, taxonomy later |
| 6 | Forgotten asset recovery (4.3) | catalog, project matching across drives, offline-drive listing | after 2 (needs search to retrieve them) |
| 7 | Semantic version tracking (3.4) | project `Versions` folders (Plan 2), `file_history` | after 3 |
| 8 | Automatic knowledge extraction (2.2) | content reader, local model | after 3 |
| 9 | Autonomous research librarian (2.3) | RAG, embeddings | after 8 |
| 10 | Cross-project intelligence (3.3) | context layer + embeddings | after 8 |
| 11 | AI Folderless mode (2.1) | filing rules, learn-then-take-over | after 3; reshaped (R6) |
| 12 | Self-healing workspace (4.1) | USN journal, batches | after 11; Dynamic Link & Path Healing for Markdown first, strictly under K1 (`propose → approve → batch → undo`) |
| 13 | Personal data governance (5.1) | D1, quarantine | parts land with D1; the rest reshaped (R7–R9) |
| 14 | AI Chief Digital Officer agent (5.2) | everything above | last |
| 15 | Digital estate management (5.3) | D1, backup drive | last; reshaped (R10) |

The vision's four phases are kept as a direction, but the order above is driven by value on the owner's PC. Encryption comes before any document content is stored (D1). Search comes before the agents that depend on it.

## 3. Binding rules

### Keep

- **K1. Propose, approve, undo.** Every change to the owner's files (from rules, models, agents or healing) is a proposal in a batch the owner approves, or an automatic batch under learn-then-take-over. Every batch has one undo. No model, agent or rule writes to disk outside this pipeline.
- **K2. Deletes always ask and go to the 30-day quarantine.** No capability deletes anything outright.
- **K3. Local only.** No cloud services, no telemetry, no outbound network from the engine or its models.
- **K4. Opt-in observation.** Anything that reads beyond the file catalog and file contents (audio, app usage, browser) is off until the owner turns it on, one capability at a time.
- **K5. Background budget.** The engine stays within 350 MB with the window closed. Heavy work waits for idle and, for the heaviest, AC power.

### Avoid

- **R1. No direct MFT parsing.** It needs admin rights or elevation. The USN journal (read without admin) plus the catalog already gives fast tracking. Revisit only if a measured need appears that the journal can't meet.
- **R3. No registry cleaning.** It gains nothing measurable and can break Windows and apps. Uninstall leftovers are handled as follows:
  - apps are removed with **their own uninstaller** (Windows' Apps list);
  - leftover **folders** are proposed for quarantine;
  - leftover **services or startup entries** are shown and can be switched off through Windows' own settings, never deleted by Aegis.
- **R8. No multi-pass shredding.** It doesn't work on SSDs (wear levelling), and it makes the quarantine's undo impossible. For data at rest, recommend BitLocker. When the owner explicitly purges quarantine, the files are deleted normally.
- **R10. No dead-man's switch that destroys data.** A missed check-in may **release** prepared instructions or keys to trustees. It never shreds, deletes or locks the owner's data automatically.
- **R11. No screen capture.** Nothing records the screen, and nothing records keystrokes or the active window. Even with encryption, the risk outweighs the value.

### Reshape

- **R2. Key handling** (follows from D1): the engine's database key is sealed to this PC (DPAPI or TPM), so the tray engine (tripwire, backup) keeps working after a reboot without anyone signing in to Aegis. Windows Hello or a PIN only opens **sensitive views** (for example documents marked sensitive, or the estate settings).
- **R4. Local models run on demand.** They load for a query or a batch job, then unload. They never stay resident in the tray engine, and must work on CPU alone.
- **R5. The librarian and cross-project intelligence work from files on disk** (saved documents and the catalog), not from watching what is on screen or being typed.
- **R6. Folderless mode compiles plain-language intents into visible rules.** These are ordinary filing rules the owner can read, edit and undo. They feed the existing propose/approve/undo pipeline. A model never decides placements per file unseen. A model may suggest where one file goes; that suggestion is still a proposal.
- **R7. Personal data is protected, not masked from the owner.** The search index stays complete, so the owner can find their own documents, account numbers included. Protection comes from four things:
  - encryption at rest (D1);
  - a "sensitive" label that hides previews and notification text;
  - leaving sensitive documents out of model prompts unless the owner asks;
  - never sending anything off the PC (K3).
- **R9. No "mathematical" zero-network claims.** Aegis promises that it makes no network connections, and keeps that promise in code: no networking crates in the engine and models, and a test that guards against them. The owner may add a firewall rule on top, but the copy never claims more than the code enforces.

## 4. Decisions for the owner

- **D1. Encryption at rest** *(Owner architectural decision D1; distinct from the parked UI deliverable `canonical/D1_Design_System_v1.0.md`).* **Approved by owner (2026-11-23)** — lands before embeddings and hybrid search work, alongside owner confirmation keeping all exclusions and reshapings (`R1–R11`, explicitly including `R2` key sealing and `R6` AI Folderless Mode):
  - **Store & vectors:** SQLCipher for the whole store, plus **`sqlite-vec`** inside the encrypted SQLite store so vectors are encrypted at rest within the 350 MB background RAM budget (K5).
  - **Key handling:** Engine key sealed to the PC via Windows DPAPI/TPM (R2), with Windows Hello gating only sensitive views.
  - **Offline recovery:** Mandatory offline 24-word or hex recovery key so Backup drive mirrors remain recoverable without the original PC.
  - **Migration note:** Converting the existing SQLite database is a one-time migration before embeddings/vector indexing lands.
- **D2. Model sizes.** Which local models are acceptable on this PC: RAM and disk to spare, CPU only or GPU. Decide in the embeddings design.
- **D3. Trustees** for estate management: who, and with what. Decide when that capability's turn comes.

## 5. For every new design

Add a short section **"North-star rules"** that lists the rules from §3 the design touches and how it keeps them. Update the table in §2.2 when an item's order or status changes.
