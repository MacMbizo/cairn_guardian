### 1. What This Document Is

`Aegis Guardian PRD v2.md` is a **hybrid Master Product & Architecture Specification (Pre-MVP, v2.0)** for **Aegis Guardian** — an AI-native, local-first desktop platform that builds a semantic "digital twin" of a user's personal computing environment to search by meaning, deduplicate, detect exposed secrets/PII, manage file lifecycles, and safely reorganise files through a previewable, journaled, reversible automation pipeline.

Rather than being a pure Product Requirements Document (PRD), it functions as an **all-in-one founding blueprint** bundling:
- **Strategy & Discovery:** Problem statement, goals/non-goals, 6 personas, 7 product principles, roadmap (MVP v1.0 → v3.0).
- **Architecture & Data Design:** Desktop stack (Tauri 2.x + React/TS + Rust + Python sidecar), ingestion pipeline, SQLite WAL + Qdrant schema, and an 8-agent orchestration framework.
- **Contracts, UX & Ops:** High-level IPC/REST/CLI/Plugin outlines, 10 MVP UI screens, security/privacy posture, NFRs, observability, metrics, risks, and 7 open questions.

**Overall Assessment:** For a Pre-MVP spec, the *product philosophy* is exceptionally strong. Principles like **reversibility over automation**, **transparent uncertainty**, **graduated autonomy**, and **action proposals instead of direct agent writes** solve the exact reasons users distrust AI file managers. However, as an *engineering-executable specification*, it currently has several **internal math/architecture contradictions** and **missing operational contracts** that will bite the team during MVP build if left unresolved.

---

### 2. Knowledge Base Snapshot

| Dimension | Current Specification State |
|---|---|
| **Domain** | Local-first personal file intelligence, semantic search, dedupe, security hygiene, safe automated filing |
| **Primary Users** | Researchers, Developers, Traders, Knowledge Workers, Students, Household Admins |
| **MVP Scope (v1.0)** | Discovery, top-format extraction, embeddings + hybrid search, basic graph, exact+near dedupe, approval-gated simulator, quarantine/undo, secrets inbox, core UI |
| **Explicit Non-Goals** | Cloud backup replacement, document editor, live email client, multi-user collaboration (MVP), guaranteed 100% classification |
| **Core Stack** | Tauri 2.x, React/TS/Tailwind, Rust core + Python sidecar (gRPC), SQLite (WAL) + Qdrant embedded, ONNX Runtime + `llama.cpp`, BLAKE3 |
| **Key Targets** | 1M+ files / 5M MVP chunks (10M target), ≤6h first scan, ≤4GB RSS on 16GB machine, p95 search <300ms (hybrid) |

---

### 3. Quick Audit (7-Axis Quality Check)

Before brainstorming additions, here is where the current v2.0 spec stands against a production-grade engineering bar:

| Axis | Score (0–5) | Rationale |
|---|---|---|
| **Completeness** | 4.0 | Broad coverage from vision to SQL DDL; lacks error/recovery matrices, OS permission flows, and IPC/API contracts. |
| **Correctness** | 3.0 | Strong stack choices, but contains **physical resource contradictions** (RAM/throughput math) and schema/priority drift. |
| **Clarity** | 4.5 | Very readable, concise tables, clear ASCII architecture, sharp non-goals. |
| **Consistency** | 3.5 | Minor drift across sections (e.g., MVP targets vs. §3.1 goals, `P1` rename tracking vs. §6.4 watcher dependency). |
| **Implementability** | 3.0 | Good for architectural alignment; engineers still lack chunking specs, IPC schemas, confidence calibration rules, and lock/OS edge cases. |
| **Traceability** | 3.0 | Uses IDs (`G1–G5`, `FR-001–FR-022`), but lacks Personas → Journeys → FRs → UI Screens → Acceptance Tests mapping. |
| **Maintainability** | 3.0 | Has version/status/supersedes; missing author/owner, last-updated date, and v1.0→v2.0 changelog. |

#### Critical Contradictions & Gaps to Resolve First (`[BLOCKER]` / `[MAJOR]`)
1. **`[BLOCKER]` Memory & Throughput Math Does Not Close (§6.3, §8.3, §13):**
   - §13 caps memory at **≤4GB RSS** during full scan, while §8.3 specifies **BGE-M3** (~1.1–2.2GB in memory) + NER + **4-bit Llama-3.1-8B / Phi-3** (~4.5–6GB RAM alone) alongside Tauri WebView, Rust, Python, and Qdrant. They cannot coexist in 4GB RSS.
   - §6.3 targets **1M files in ≤6h** at **≥50 docs/sec on CPU**. At 50 embeddings/sec, 1M files = 5.55 hours *if every file is 1 chunk*. At the **5M–10M chunks** scale in §13, 50 chunks/sec = **27.7 to 55.5 hours** on CPU.
2. **`[BLOCKER]` Cross-Volume Quarantine Trap (§7.11 FR-013):**
   - Moving deleted files to a single `.aegis/quarantine` breaks across drives/volumes: quarantining a 20GB video from an external drive `D:\` to `C:\` forces a slow 20GB copy and can fill the user's boot SSD.
3. **`[MAJOR]` Identity Tracking Priority Mismatch (§6.4 vs §7.1 FR-001.6 vs §9):**
   - `FR-001.6` marks rename/move identity tracking (`volume+file-index`) as **P1 (post-MVP)**, yet §6.4 relies on file-index/inode matching for watcher stability and §9 already puts `volume_id, file_index` in the MVP `files` table. If deferred to P1, every moved folder in MVP drops its graph edges and triggers redundant re-embedding.
4. **`[MAJOR]` Schema Drift in §9:**
   - §7.7 depends on `SQLite FTS5` for BM25 hybrid search, but §9 defines no `FTS5` virtual table or sync mechanism.
   - §7.2 indexes archive members (`archive://` URIs), which lack OS `volume_id` / `file_index`, yet §9 has no container/parent-file FK or virtual-entry model.
5. **`[MAJOR]` Goal vs. Metric Scope Drift (§3.1 vs §16):**
   - §3.1 lists `>90%` search hit rate and `≥80%` proposal acceptance as unqualified goals, while §16 sets MVP targets at `≥75%` and `≥60%`. §3.1 should state `V2` next to those targets to prevent scope/QA confusion.

---

### 4. Brainstorm: How to Make Aegis Guardian (and This Spec) Even Better

#### A. Product & UX Breakthroughs (10x User Value)

1. **"First 5 Minutes" Progressive Value (Kill the 6-Hour Cold Start)**
   - **Problem:** Asking a user to wait hours for an initial scan before search or health scores feel useful causes Day-1 churn.
   - **Idea:** Replace the single scan with a **3-Tiered Progressive Indexing Funnel**:
     - **Tier 1 (0–90 seconds):** OS-fast metadata sweep (MFT/USN Journal on NTFS, FSEvents/directory stat) → immediate **Storage Analytics**, **Exact-Size Duplicate Candidates**, **Filename Secret Alerts** (`.env`, `*.pem`, `id_rsa`), and an initial **Workspace Health Baseline**.
     - **Tier 2 (Minutes):** Prioritise high-signal working directories (`Desktop`, `Downloads`, `Documents`, active Git repos, recent 90-day modified files) for text extraction + embeddings so AI search works on *recent work* almost immediately.
     - **Tier 3 (Background / Idle / Plugged-In):** Cold archives, deep history, OCR, and media transcription.

2. **"Atomic Project Units" (Preventing Destructive Micro-Refiling)**
   - **Problem:** Semantic classifiers operating file-by-file love to suggest moving `README.md` or `dataset.csv` out of a code repository or LaTeX paper folder into a generic `Docs/` folder — breaking builds and workflows.
   - **Idea:** Promote **Atomic Units / Unbreakable Boundaries** to a first-class invariant in §7.5. Any folder containing boundary markers (`.git`, `package.json`, `pyproject.toml`, `.xcodeproj`, `.app` bundle, SQLite `-wal/-shm` companion files, multi-file LaTeX/Premiere projects) is treated as an **indivisible bundle**. Refiling can move or archive the *entire project folder*, never pluck individual files out of its interior unless explicitly overridden.

3. **Proposal-to-Rule Graduation ("Teach Aegis Once")**
   - **Problem:** Users hate reviewing the same kind of proposal repeatedly, yet jumping straight to opaque ML "Auto" mode feels scary.
   - **Idea:** Whenever a user approves a proposal in the Simulator or Approvals Inbox, offer a one-click **"Always do this for similar files"** action that synthesises a transparent, human-editable deterministic rule in the `rules` table (e.g., *PDFs matching "Brokerage Statement" in Downloads → `Finance/Statements/{YYYY}`*). Deterministic rules earn user trust faster than black-box embeddings.

4. **Crisis / Intent Playbooks (High-Emotion Entry Points)**
   - **Idea:** Most users look for a tool like Aegis during a specific moment of pain. Add 3 guided MVP/V1.5 workflows driven by the existing engine:
     - **Disk Rescue:** "I need 50GB free right now" → ranks exact dupes + `node_modules`/build artifacts + oversized stale downloads.
     - **Pre-Share / Pre-Backup Security Audit:** Scan a specific folder before zipping, uploading, or handing a laptop to IT.
     - **New Machine Migration Prep:** Identify active projects, orphaned config/dotfiles, and dead clutter before migrating.

5. **Local Preference & Taxonomy Calibration**
   - **Idea:** Every user's mental model differs (one puts tax PDFs in `Finance/Taxes`, another in `Legal/2026`). Store approval/rejection pairs locally as **few-shot routing exemplars** (in Qdrant) so the Curator agent adapts to the *user's existing directory taxonomy* instead of imposing a generic Aegis folder tree.

---

#### B. Technical & Architectural Upgrades

1. **Rethink the Python Sidecar Dependency (Rust-First Extraction)**
   - **Trap:** Bundling a Python runtime + ONNX/PyTorch + Poppler + Tesseract + Tree-sitter inside a Tauri desktop app typically produces **1.5GB–3GB installers**, fragile IPC, zombie process bugs, and Windows Defender false positives.
   - **Better Architecture:** Move the MVP hot path **100% into Rust**:
     - Embeddings & small classifiers: `ort` (ONNX Runtime Rust bindings) or `fastembed-rs` + `candle`.
     - Code AST: `tree-sitter` has official Rust bindings.
     - Office docs / Text / CSV / JSON / EXIF: Rust crates (`calamine`, `quick-xml`, `kamadak-exif`, `pulldown-cmark`).
     - PDF: `pdfium-render` (fast, battle-tested C++ library with clean Rust bindings).
     - Reserve Python (or a lazy-downloaded optional plugin runtime) **strictly for V1.5/V2 heavy/niche parsers** (Whisper, complex OCR pipelines, `.pst`, VLM captioning).
   - **Payoff:** Single native binary, half the RAM footprint, zero gRPC serialization tax on 1M files, vastly simpler cross-platform CI/CD.

2. **Hardware-Tiered Inference & Model Lifecycle Scheduler**
   - Fix the 4GB RSS contradiction explicitly by defining **3 Hardware Profiles** detected at onboarding:
     - **Eco / 8GB RAM (CPU-only):** BM25 (FTS5) + lightweight 384-dim embedding (`bge-small-en-v1.5` / `all-MiniLM-L6-v2`, ~90MB ONNX) + rule/regex query parser. **Zero local LLM resident.**
     - **Standard / 16GB RAM:** `bge-small` or quantised `bge-m3` + on-demand small LLM (1.5B–3B Q4, ~1.5–2GB) loaded *only* during interactive query/proposal bursts and **evicted after 5 min idle** so scanning stays ≤4GB RSS.
     - **Pro / GPU / Apple Silicon 32GB+:** Full BGE-M3 + cross-encoder reranker + 7B–8B LLM.
   - Also adopt **Selective / Hierarchical Chunking**: only chunk documents >1,500 tokens, and cap initial indexing to metadata + first N chunks + summary embedding during Tier-2 scan so 1M files finish within the 6h budget even on CPU.

3. **Per-Volume Quarantine + Hardlink-Aware Dedupe**
   - **Per-Volume Quarantine:** Store quarantined items in `<VolumeMount>/.aegis-quarantine` (or delegate to OS Trash where reliable) so deletes are **O(1) atomic metadata renames** with zero cross-drive copying. Fall back to copy+verify+delete only if volume root is non-writable.
   - **Hardlink Awareness:** On NTFS/APFS/ext4, files with `link_count > 1` share the same underlying data blocks (`file_index`/`inode`). Add `nlink` to `files` and **never count hardlinks as reclaimable duplicate storage** — otherwise Aegis will lie about how much disk space the user will get back (breaking the "Coverage/Metrics Honesty" principle).

4. **OS-Specific Fast-Path Enumeration**
   - Standard recursive directory walking (`readdir` / `FindFirstFile`) over 1M files on Windows/NAS is slow.
   - Add an OS-adapter strategy: use **NTFS MFT / USN Journal** (when elevated or permitted) on Windows for 10x faster initial metadata discovery + missed-event reconciliation after crashes; use **FSEvents** history IDs on macOS; fall back cleanly to `notify` + `jwalk` (parallel walker) on Linux/NAS.

5. **Pre-Rollback Conflict Reconciliation**
   - What if Aegis moved `A → B` on Monday, the user edited `B` or created a new file at `A` on Tuesday, and hits **Rollback** on Wednesday?
   - Define a **Rollback State Machine**: before reversing an `actions` batch, verify `(exists(to_path), blake3(to_path) == post_hash, !exists(from_path))`. If modified or collided, present a safe 3-way choice (*Restore anyway with rename / Skip this file / Abort batch*) rather than corrupting user edits.

---

#### C. Security, Privacy & Trust Hardening

1. **False-Positive-Resistant Secret Intelligence (§7.8)**
   - Running gitleaks regex + entropy across a developer's machine (`tests/`, `node_modules/`, lockfiles, `*.svg`, JWT examples in docs) will flood the Security Inbox with thousands of false alarms, causing users to ignore real leaks.
   - **Upgrade:** Add a **Risk-Weighted Triage Model**:
     - *Context multiplier:* Key in `~/Downloads/keys.txt` or `.env` = **Critical**; key inside `**/test*/fixtures/**` or `README.md` example = **Info/Suppressed**.
     - *Deduplication:* Group identical secret hashes across 15 backup copies into **1 logical finding** with 15 locations.
     - *Provider checksums:* Prioritise format-validated tokens (`ghp_`, `github_pat_`, `sk_live_`, AWS key pairs) over generic high-entropy strings.

2. **OS Permission & Privacy UX (macOS TCC, Windows SmartScreen, Cloud Dehydration)**
   - Add explicit **Permission Onboarding Flows**: macOS requires **Full Disk Access** or per-folder TCC prompts (if denied, Aegis must map those to "Permission Denied" in the Coverage Report, not silent 0-file counts).
   - **Cloud Placeholder Safety:** Explicitly handle macOS `dataless` iCloud files (`SF_DATALESS`) and Windows OneDrive `FILE_ATTRIBUTE_RECALL_ON_DATA_ACCESS` **before** opening file handles — otherwise merely opening a file to compute BLAKE3 triggers the OS to silently download gigabytes from the cloud!

3. **Tamper-Evident Audit Log Clarification**
   - Right now `audit_log` is in SQLite alongside `actions` on the local disk. If malware or a local attacker modifies the DB, hash-chaining alone is only tamper-*evident*, not tamper-*proof*. Clarify the threat model in §12: hash-chaining detects accidental DB corruption and non-chain edits during `aegis doctor`, and optional signed checkpoints / external export provide forensic assurance.

---

#### D. Documentation Architecture Upgrades (Turning This Into a Build-Ready Suite)

Right now, `Aegis Guardian PRD v2.md` is doing the job of **8 different documents** in one 450-line file. As soon as engineering starts, a monolith PRD either goes stale or causes merge conflicts.

Recommended clean split (keeping v2.0 as the root):

| Target Doc ID | Title | What Moves / Gets Created | Why Now (Priority) |
|---|---|---|---|
| **A2** | **PRD v2.1 (Lean)** | Keep §1–§5, §7, §11, §15–§19; reconcile §3.1 vs §16 targets; add Personas→FR traceability & testable Acceptance Criteria for `FR-002..FR-013`. | **P0** — Single source of truth for Product/QA |
| **B1/B2** | **System & Sidecar Architecture** | §6, §8, hardware profiles, Rust-vs-Python boundary, OS watcher/MFT/cloud-placeholder adapters, memory budget table. | **P0** — Resolves 4GB RSS & 10M-chunk blocker |
| **B4** | **Data & Vector Schema Spec** | §9 + missing `FTS5` tables, `archive://` virtual asset model, `nlink` hardlink tracking, SQLite→PG migration plan, Qdrant chunking strategy. | **P0** — Unblocks DB & indexer implementation |
| **B8** | **Architecture Decision Records (ADRs)** | Formalise the 4 key decisions: (1) Rust-first vs Python sidecar, (2) Per-volume quarantine, (3) Atomic project units, (4) Hardware-tiered model eviction. | **P0** — Prevents mid-sprint architectural churn |
| **C1** | **IPC, Events & Action Contract** | Exact typed schemas for the ~40 Tauri commands, `ActionProposal` lifecycle state machine, and rollback conflict rules. | **P1** — Unblocks Frontend/Backend parallel work |
| **F1/F2** | **Eval Corpus & Safety Test Plan** | Benchmark corpus for §16 metrics (75% hit rate, 95% secret recall) + fuzz tests for path/symlink/quarantine safety. | **P1** — Essential for "transparent uncertainty" claims |

---

### 5. Open Questions & Key Trade-offs Highlighted

Adding these to §18 (Open Questions) alongside your existing 7 items will save weeks later:
1. `[OPEN]` **Rust-native extraction vs. Python sidecar:** Can MVP formats (PDF via `pdfium`, Office via Rust, Tree-sitter, ONNX via `ort`) run 100% in Rust, deferring Python to V1.5+ optional plugins?
2. `[OPEN]` **Hardware baseline for MVP:** What is the minimum supported machine (e.g., 8GB RAM dual-core vs. 16GB SSD), and is local LLM query-parsing disabled on 8GB machines?
3. `[OPEN]` **Atomic Unit boundary rules:** Which project markers lock a directory against internal file-level refiling proposals?
4. `[OPEN]` **Quarantine on read-only/external volumes:** If an external USB drive has no `.aegis-quarantine` permission or user unmounts it, do we move to OS Trash or require explicit delete confirmation?

---

### 6. Next Actions (Smallest Shippable Steps)

1. **Immediate Action (Recommended First):** Produce **ADR-001 through ADR-004 (`B8`)** + a **Patched `B4` Data & Vector Schema Spec** to resolve the `[BLOCKER]` memory/throughput math, add `FTS5` + `nlink` + `archive://` support, lock `volume_id + file_index` into MVP P0, and specify per-volume quarantine.
   - *Acceptance Criteria:* Zero unresolved contradictions between §6.3, §7.1, §8.3, §9, and §13; memory budget table sums to ≤4GB RSS under active scan.
2. **Follow-up Action:** Produce **A2 PRD v2.1 Patch** adding testable acceptance criteria for `FR-002` through `FR-013`, Atomic Project Units, Tiered Progressive Indexing, and aligned MVP vs. V2 targets.

Would you like me to generate **Step 1 (`B8` ADRs + `B4` Complete Data & Vector Schema Spec)** or **Step 2 (`A2` PRD v2.1 upgraded with these improvements)** first?