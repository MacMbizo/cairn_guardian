# Aegis Guardian
## Product Requirements Document — Comprehensive Specification

Version: 2.0
Status: Pre-MVP Specification
Supersedes: "Aegis Guardian Full PRD.md" (Vision v1.0)

---

## 1. Executive Summary

Aegis Guardian is an AI-native, local-first desktop platform that discovers, understands, organises, secures, archives, and maintains an entire personal computing environment. Unlike file managers, cleanup utilities, or knowledge-base tools, it builds a semantic model — a **digital twin** — of the user's files, projects, people, and relationships, and uses that model to continuously improve the environment.

The product is built around five beliefs:

1. **Meaning over location.** Files should be retrievable by what they *are*, not where they happen to live.
2. **Reversibility over automation.** Every mutation is journaled, previewable, and undoable.
3. **Local-first privacy.** Indexing, classification, and inference run on-device by default. Cloud is an explicit, opt-in, encrypted-augmentation layer only.
4. **Explainability.** Every automated decision stores its reason, confidence, evidence, and recovery path.
5. **Human sovereignty.** The system proposes; the user disposes — with graduated autonomy the user can dial up over time.

---

## 2. Problem Statement

Users accumulate years of unstructured digital assets: documents, code, media, downloads, archives, screenshots, emails, datasets, and backups scattered across internal drives, external drives, NAS, and cloud sync folders. The failure modes are well known:

- **Retrieval failure.** The user knows a file exists but cannot remember its name, location, or folder. Conventional search is filename-only; semantic search is absent.
- **Structural decay.** Folder hierarchies designed years ago no longer match how the user thinks. Moving files is risky, so they rot in place.
- **Silent duplication.** Downloads, sync conflicts (`file (1).pdf`), and copied projects consume storage and corrupt search results.
- **Lifecycle neglect.** Stale projects, orphaned assets, and abandoned archives accumulate with no signal about what is still relevant.
- **Security blind spots.** API keys, identity documents, and financial records sit in plaintext across dozens of folders with no inventory.
- **Tool fragmentation.** Cleanup tools, search tools, dedupe tools, and note apps each solve one slice and know nothing about each other.

Aegis Guardian is the single intelligent layer that subsumes all of these.

---

## 3. Goals and Non-Goals

### 3.1 Goals (in priority order)

| # | Goal | Success measure |
|---|------|-----------------|
| G1 | Find anything by meaning, instantly | >90% top-3 hit rate on realistic natural-language queries |
| G2 | Surface and resolve duplication | ≥30% storage reclaimed on dirty real-world drives in pilot studies |
| G3 | Propose safe, reviewable reorganisation | ≥80% user acceptance rate of simulator proposals |
| G4 | Maintain a complete, auditable decision record | 100% of mutations reversible; zero unrecoverable actions |
| G5 | Detect sensitive data before it leaks | Zero known-secrets left unflagged after a full scan |

### 3.2 Non-Goals (explicitly out of scope, now and for the foreseeable future)

- **Not a cloud backup service.** Aegis indexes and organises; it does not replace backup/sync products. It may *integrate* with them.
- **Not a document editor or office suite.** It may open files in native apps; it does not edit them (V3 exception: metadata-only edits).
- **Not an email client.** It indexes exported/downloaded email artifacts, not live mailboxes (V2+ may integrate read-only IMAP).
- **Not a team collaboration platform.** Multi-user is an Enterprise roadmap item, not MVP.
- **Not guaranteed perfect classification.** The product's contract is *transparent uncertainty* — low-confidence items are surfaced for review, not silently misfiled.

---

## 4. Personas (Expanded)

| Persona | Context | Primary jobs-to-be-done |
|---------|---------|------------------------|
| **Researcher** | Academia / quant research | Literature ingestion, citation graphs, paper dedupe, gap discovery |
| **Developer** | Software engineer | Repo intelligence, dependency maps, dead-project detection, secret scanning, environment hygiene |
| **Trader** | Retail/pro quant | Statement & report grouping, broker record management, tax classification, strategy organisation |
| **Knowledge worker** | General professional | Automated filing, fast retrieval, meeting/doc clustering, archive hygiene |
| **Student** | Coursework | Lecture/assignment organisation, citation management, deadline-aware staleness signals |
| **Family / household admin** | Shared machine | Photos/docs sorting, duplicate photos, secure storage of sensitive family documents |

Design implication: the core engine must be **domain-agnostic**; domain depth (Research Mode, Developer Mode, Trading Mode) is delivered via plugins and specialised agents, not baked into the core.

---

## 5. Product Principles

1. **Understand before modifying.** No mutation occurs until the file has been indexed, embedded, and classified with recorded confidence.
2. **Always explain decisions.** Every action carries reason, confidence, evidence, timestamp, and recovery path.
3. **Maintain reversibility.** All filesystem mutations go through a journal + transaction model. Rollback is a first-class feature, not an apology.
4. **Keep humans in control.** Autonomy is a dial (Manual → Suggest → Auto-approved → Auto), set per action class, never global-only.
5. **Privacy by design.** Local inference by default; no telemetry of file *content* ever; metadata telemetry opt-in.
6. **Fail safe, not silent.** Every scan job checkpoints, resumes, and reports partial coverage honestly.
7. **Performance is a feature.** Indexing must never make the machine feel slower; all heavy work is throttled and backgrounded.

---

## 6. System Architecture

### 6.1 High-Level Components

```
┌─────────────────────────────────────────────────────────┐
│  Desktop App (Tauri + React + TS + Tailwind)             │
│  Dashboard · Twin Viewer · Simulator · Search · Settings │
└──────────────┬──────────────────────────┬───────────────┘
               │ IPC (commands/events)    │ REST/GraphQL (localhost)
┌──────────────▼──────────────────────────▼───────────────┐
│  Core Service (Rust + Python hybrid, gRPC internally)   │
│                                                          │
│  ┌────────────┐ ┌──────────────┐ ┌───────────────────┐ │
│  │ Discovery  │ │ Content      │ │ Knowledge Graph   │ │
│  │ Engine     │ │ Intelligence │ │ Engine            │ │
│  └─────┬──────┘ └──────┬───────┘ └─────────┬─────────┘ │
│        │               │                   │           │
│  ┌─────▼───────────────▼───────────────────▼─────────┐ │
│  │ Classification · Embeddings · Dedupe · Security   │ │
│  └──────────────────────┬───────────────────────────┘ │
│  ┌──────────────────────▼───────────────────────────┐ │
│  │ Automation Engine (plan → simulate → execute)     │ │
│  │ Agent Orchestrator · Policy Engine · Journal      │ │
│  └──────────────────────┬───────────────────────────┘ │
└─────────────────────────┼──────────────────────────────┘
                          │
        ┌─────────────────┼──────────────────┐
        ▼                 ▼                  ▼
   ┌─────────┐     ┌───────────┐      ┌────────────┐
   │ SQLite/ │     │ Qdrant    │      │ PostgreSQL │
   │ WAL     │     │ (vectors) │      │ (relational│
   │ journal │     │           │      │  + audit)  │
   └─────────┘     └───────────┘      └────────────┘
```

### 6.2 Technology Decisions (locked for MVP)

| Layer | Choice | Rationale |
|-------|--------|-----------|
| Desktop shell | **Tauri 2.x** | Small binary, Rust core, native webview |
| UI | **React + TypeScript + Tailwind** | Team velocity, component ecosystem |
| Core services | **Rust** (discovery, watchers, journal, file ops) + **Python** (extraction, ML orchestration) via gRPC | Rust for I/O safety & perf; Python for parser/ML ecosystem |
| Relational DB | **SQLite (WAL)** for local-first MVP; **PostgreSQL** for sync/enterprise | Embedded, zero-admin, transactional |
| Vector DB | **Qdrant embedded** (Rust, single-binary) — Chroma acceptable fallback | Local-first, no server needed |
| Inference | **ONNX Runtime** for embeddings/classification; local LLM via **llama.cpp** optional | Quantised models, CPU/GPU flexible |
| Messaging | Internal **event bus** (Rust `tokio` channels) → persisted **outbox table** | Decouples ingestion from UI |
| Watchers | `notify` crate (ReadDirectoryChangesW / FSEvents / inotify) + polling fallback | Cross-platform real-time index updates |

> Note: PostgreSQL, gRPC, and multi-process split are MVP-optional. The MVP may run discovery+graph in a single Rust process with Python sidecar for extraction; split into microservices only when profiling demands it. **Do not prematurely distribute.**

### 6.3 Ingestion Pipeline (detailed)

```
Source → Discovery → Change Detection → Extraction → Enrichment → Index Commit
```

1. **Discovery** enumerates roots (drives, dirs, cloud-sync folders, email-export dirs). Produces *file records* (metadata only, no content read yet).
2. **Change detection** compares against prior index via `(path, size, mtime, inode/file-index)` + content hash on suspicion of change. Unchanged files skip re-extraction.
3. **Extraction** (queued, throttled) reads content per format parser → text chunks, metadata sidecars (EXIF, PDF metadata, git info for repos).
4. **Enrichment** computes: content hash (BLAKE3 — faster than SHA-256, sufficient for dedupe), embeddings (chunk + doc-level), classification labels, entities, project assignment, sensitivity flags.
5. **Index commit** writes to SQLite + Qdrant atomically-ish (outbox pattern for vector writes), updates graph edges, appends audit entry.

**Throughput targets:** first full scan of 1M files ≤ 6h on mid-range hardware (metadata phase ≤ 30 min); steady-state incremental scan ≤ 60s for typical changesets; embedding throughput ≥ 50 docs/sec on CPU, ≥ 300/sec on GPU.

### 6.4 Change Monitoring (FR-010 elaboration)

- **Watchers:** one watcher per root; debounce 500ms; coalesce bursts.
- **Events handled:** create, modify, delete, rename (match by file-index/inode where possible to preserve identity across moves — critical for graph stability), attribute change.
- **Cloud-sync roots:** detect provider sync states (`.dropbox`, `~/.icloud` placeholders); treat placeholder files as "remote-only" until materialised; never trigger extraction on dehydrated OneDrive files without user consent.
- **Polling fallback:** 15-min sweep for roots where watchers unsupported/failing.
- **Crash recovery:** on restart, reconcile watchers vs. index state; full rescan only if journal indicates missed events (checksum-sampled verification).

---

## 7. Functional Requirements (Detailed)

Priority legend: **P0** = MVP, **P1** = V1.x, **P2** = V2, **P3** = V3+.

### 7.1 Discovery Engine (FR-001)

| ID | Requirement | Pri | Acceptance criteria |
|----|-------------|-----|---------------------|
| FR-001.1 | Scan configurable roots: internal drives, external, NAS/SMB mounts, cloud-sync folders | P0 | All listed root types enumerable; per-root on/off toggle |
| FR-001.2 | Respect ignore rules: gitignore-style `.aegisignore`, OS junk dirs (`node_modules` optional-inclusion toggle, `AppData` exclusion by default), user denylist | P0 | Ignored paths never indexed; rules editable in UI |
| FR-001.3 | Handle permission errors gracefully: skip + report, never crash scan | P0 | Scan completes with coverage report listing inaccessible paths |
| FR-001.4 | Detect and skip reparse points/symlinks by default (configurable) | P0 | No infinite loops; symlink policy explicit |
| FR-001.5 | Content hashing (BLAKE3) computed lazily in background queue, not during enumeration | P0 | Metadata scan unaffected by hash backlog |
| FR-001.6 | File identity stable across rename/move (volume+file-index tracking) | P1 | Renamed file retains id, embeddings, graph edges |
| FR-001.7 | Email-export ingestion (mbox, .eml, Outlook PST optional) | P2 | Attachments extracted as first-class assets |
| FR-001.8 | NAS/offline handling: mark unavailable roots stale; degrade gracefully | P1 | No scan failure when drive disconnected mid-run |

### 7.2 Content Intelligence (FR-002)

| Format | Parser | Notes |
|--------|--------|-------|
| PDF | `pypdf` + OCR fallback (Tesseract / Windows OCR) | Tables via heuristic; scanned PDFs → OCR |
| DOCX/XLSX/PPTX | `python-docx`/`openpyxl`/`python-pptx` | Preserve sheet names, slide titles |
| Images | EXIF + optional VLM captioning (V2) | OCR on screenshots |
| Audio/Video | Whisper.cpp / faster-whisper (local) | Transcription opt-in per-root (CPU-heavy) |
| Source code | Tree-sitter | AST-aware chunking, symbol extraction |
| Markdown/TXT/CSV/JSON | Direct | — |
| Archives (zip/7z) | Listing + optional member indexing | Members indexed as `archive://` URIs |

Capabilities: OCR, entity extraction (NER), topic detection, project identification, language detection, PII/sensitivity flagging. **OCR and transcription are opt-in** (resource cost disclosure in UI).

### 7.3 Knowledge Graph (FR-003)

Entities: File, Folder, Project, Person, Company, Topic, Repository, Account, Event (meeting/deadline inferred), Collection (user-defined smart set).

Relationships (typed, weighted, temporal): `references`, `belongs_to`, `derived_from`, `duplicate_of`, `version_of`, `authored_by`, `mentions`, `related_to`, `supersedes`.

**Graph storage:** adjacency in SQLite tables (`entities`, `relationships` with `valid_from`/`valid_to` for temporal queries) + materialised traversal cache. Graph queries power Twin Viewer and agent reasoning.

### 7.4 Duplicate Intelligence (FR-004)

Four detection levels, run as separate passes:

1. **Exact:** BLAKE3 hash match → group, keep-best heuristic (path depth, access recency, folder cleanliness).
2. **Near-duplicate:** similarity hash (e.g., 128-bit difference hash for images; MinHash/LSH on text shingles for docs).
3. **Semantic duplicate:** embedding cosine ≥ threshold (e.g., 0.92) — catches reworded copies.
4. **Version chains:** filename patterns (`v2`, `final`, `copy`, dates) + similarity + temporal ordering → chain UI, not auto-dedupe.

Deliverable: **Duplicate report** grouped by cluster with recommended keeper + reclaimable bytes. Actions: keep-all / quarantine-losers / delete-losers — always via journal.

### 7.5 Autonomous Refiling (FR-005) — the core differentiator

Pipeline: **Analyse → Simulate → Approve → Execute → Audit → Rollback**

- **Analyse:** candidate moves generated from (a) user-defined rules, (b) project clustering, (c) curator-agent proposals. Each candidate = `{file_id, from, to, reason, confidence, evidence[]}`.
- **Simulate:** dry-run produces a full diff — every move, every folder create/delete, space delta, collision resolution. Rendered in Reorganisation Simulator UI.
- **Approve:** per-batch or per-file approval; confidence thresholds route items: ≥0.9 auto-eligible (if autonomy=Auto), 0.6–0.9 review queue, <0.6 leave-in-place + flag.
- **Execute:** transactional batch; collisions resolved by rename-policy (never overwrite without explicit approval); each op journaled with pre-state.
- **Audit:** decision record per file (see §9).
- **Rollback:** restore via journal — moves reversed, deletes restored from quarantine, renames undone. Rollback itself is journaled.

**Collision & safety rules:** never overwrite an existing file silently; never move files open/locked (detect via attempt + lock API); never touch paths outside configured roots; dry-run always available; `--dry-run` equivalent in CLI.

### 7.6 Workspace Health (FR-006)

Health score = weighted composite, each 0–100:

| Dimension | Signals |
|-----------|---------|
| Organisation | % files in classified project/folder; depth anomalies; root clutter |
| Storage | reclaimable duplicates, stale archives, oversized files, drive fullness |
| Security | unencrypted sensitive docs, exposed secrets, world-readable flags (where applicable) |
| Duplication | duplicate bytes / total bytes; version-chain sprawl |
| Searchability | % indexed-with-embeddings; OCR coverage; metadata completeness |

Scores are **explainable**: clicking a score lists the top contributing factors with links to affected items.

### 7.7 AI Search (FR-007)

- **Hybrid retrieval:** BM25 (SQLite FTS5) + vector ANN (Qdrant) fused via Reciprocal Rank Fusion; optional cross-encoder reranker (ONNX, small model).
- **Query understanding:** local LLM parses intent → structured filters (project=, type=, date range, person=) + semantic residual.
- **Result cards:** file preview/summary, why-it-matched (matched chunks highlighted), related items, open-in-native-app action.
- **Conversational refinement:** "only PDFs from 2023", "exclude duplicates".
- **Global hotkey / OS integration:** summon search from anywhere (V1); Explorer/Finder context-menu "Ask Aegis" (V1.5).
- **Examples that must work in MVP:** "show latest dissertation draft", "find all files related to portfolio optimisation", "find screenshots from risk engine".

### 7.8 Security Intelligence (FR-008)

Detection targets: API keys/tokens (regex + entropy + gitleaks-style rules), passwords/credential files, private keys/certificates (.pem, .p12, .key), identity documents (heuristic + optional classifier), financial data (statements, account numbers), PII.

Behaviour: findings go to **Security inbox** (not auto-quarantine). Per-finding actions: reveal-path, open, move-to-vault, encrypt, mark-false-positive, create-rule. Vault = encrypted container/directory (V2); MVP = flagged + relocate-to-secure-folder proposal.

### 7.9 Archive & Lifecycle (FR-009)

Lifecycle states: `Active → Warm → Cold → Archive → (Quarantine)`.

- Transition rules: configurable per project/domain (e.g., no-access 180d + closed project → Cold).
- Cold/Archive = move to user-designated archive root (or compressed bundle option) — **never delete**; deletion only via explicit duplicate/stale flows with approval.
- Lifecycle transitions are journaled and reversible.
- "Resurrection": accessing an archived item via search offers to restore it to Active.

### 7.10 Continuous Monitoring (FR-010)

Covered in §6.4. Additional: project-change detection (git activity → project health updates), storage-pressure events (drive >90% → proactive report).

### 7.11 Additional Functional Areas (new in v2.0)

| ID | Feature | Pri | Notes |
|----|---------|-----|-------|
| FR-011 | **Onboarding & first-scan wizard** | P0 | Consent for roots, resource budget (battery/performance), ignore-rule templates, progress with honest ETA |
| FR-012 | **Undo stack / time-machine** | P0 | Global undo of last N action batches across all subsystems |
| FR-013 | **Quarantine zone** | P0 | All "deletes" are moves to `.aegis/quarantine` with TTL (default 30d) + restore UI |
| FR-014 | **CLI (`aegis`)** | P1 | `aegis search`, `aegis scan`, `aegis dedupe`, `aegis doctor`; same engine, headless mode |
| FR-015 | **Smart Collections** | P1 | Saved queries ("all Aegis screenshots", "unreviewed duplicates") as live folders |
| FR-016 | **Plugin SDK** | P2 | Sandboxed plugin API: register agents, parsers, dashboard widgets, rules. Manifest + permission model |
| FR-017 | **Scheduled housekeeping** | P1 | Cron-like recurring plans: weekly dedupe report, monthly lifecycle review |
| FR-018 | **Storage analytics** | P1 | Treemap of largest folders/files, growth trends, "what changed this week" |
| FR-019 | **Notifications & digest** | P1 | Daily/weekly summary: new duplicates, security findings, health delta — quiet by default |
| FR-020 | **Multi-device sync** | P2 | Encrypted sync of index+graph+rules (not file content) across a user's machines; conflict = last-writer per entity with merge log |
| FR-021 | **Team/Enterprise graph** | P3 | Shared org graph, RBAC, compliance exports |
| FR-022 | **Accessibility & i18n** | P1 | Keyboard-first UI, screen-reader labels; i18n framework (EN first) |

---

## 8. AI Agent Framework

### 8.1 Orchestrator

Single **Agent Orchestrator** owns: task queue, tool registry, policy checks, budget limits (tokens/time), and human-approval gates. Agents are specialists invoked by the orchestrator or by user request; agents never mutate files directly — they emit **Action Proposals** consumed by the Automation Engine (§7.5).

### 8.2 Agent Roster

| Agent | Responsibility | Tools | Autonomy ceiling |
|-------|---------------|-------|------------------|
| **Librarian** | Classify, tag, summarise assets | search, read-metadata, embed | Propose-only |
| **Curator** | Design folder structures & filing plans | graph-query, simulate | Auto-eligible only ≥0.9 conf, user-enabled |
| **Housekeeper** | Clutter reports, stale detection, storage reclaim proposals | dedupe, lifecycle | Propose-only (deletes never auto) |
| **Archivist** | Lifecycle transitions, archive bundling | lifecycle, compress | Auto-eligible for Warm→Cold only |
| **Security** | Secret/PII scanning triage, vault recommendations | secret-scan, graph | Propose-only |
| **Developer** | Repo analysis, dependency mapping, dead-project detection | git, package-manifests, tree-sitter | Propose-only |
| **Research** (plugin) | Paper ingestion, citation extraction, gap analysis | pdf, doi-resolver(opt-in cloud) | Propose-only |
| **Trader** (plugin) | Statement grouping, tax classification | pdf/xlsx parsers, rules | Propose-only |

### 8.3 Model Strategy

| Task | Model approach | Runtime |
|------|---------------|---------|
| Embeddings | BGE-M3 / multilingual-e5 (quantised) | ONNX, CPU-viable |
| Classification/NER | Small fine-tuned encoder or zero-shot classifier | ONNX |
| Summarisation/chat/rerank | Phi-3 / Llama-3.1-8B quantised (4-bit); Mistral acceptable | llama.cpp, GPU optional |
| Optional cloud | User-supplied API key (Anthropic/OpenAI/local endpoint) for heavy reasoning | Explicit opt-in, redaction layer strips detected secrets/PII before egress |

**Privacy guardrail:** any cloud call passes through a redaction filter; audit-logged; disabled by default.

### 8.4 Agent Safety Model

- Every tool call passes the **Policy Engine**: allowed-roots check, action-class check, autonomy-level check, budget check.
- Agents operate on the **index**, not raw disk, except via Automation Engine ops.
- Prompt-injection defence: file *contents* are treated as untrusted data — delimited, never executed; tool outputs sanitised.
- Rate limits + daily token budgets per agent; circuit-breaker on repeated failures.

---

## 9. Data Model (Detailed)

SQLite core (WAL mode). All tables carry `created_at`, `updated_at`; soft-delete via `deleted_at` where applicable.

```sql
files (
  id TEXT PK,               -- stable uuid; survives rename via file-index mapping
  volume_id TEXT, file_index INTEGER,  -- OS-level identity
  path TEXT, path_hash BLOB,
  name TEXT, ext TEXT, mime TEXT,
  size INTEGER, mtime INTEGER, ctime INTEGER,
  blake3 BLOB,              -- nullable until hashed
  state TEXT,               -- active|warm|cold|archive|quarantine|missing
  project_id TEXT NULL,
  classification TEXT, confidence REAL,
  sensitivity TEXT,         -- public|internal|confidential|secret
  lifecycle_state TEXT,
  first_seen INTEGER, last_seen INTEGER,
  access_count INTEGER, last_access INTEGER,
  watcher_alive BOOLEAN
)

chunks (
  id TEXT PK, file_id TEXT FK,
  seq INTEGER, text TEXT, token_count INTEGER,
  vector_ref TEXT            -- qdrant point id
)

entities (id, type, name, attrs JSON, embedding_ref)
relationships (id, src_id, dst_id, rel_type, weight REAL,
               evidence JSON, valid_from, valid_to)

projects (id, name, description, domain, repo_url,
          health_score, created_at)

actions (                    -- every proposed/executed mutation
  id TEXT PK, batch_id TEXT, agent TEXT NULL,   -- NULL = user/rule
  action_type TEXT,          -- move|copy|rename|quarantine|delete|archive|encrypt
  file_id TEXT, from_path TEXT, to_path TEXT,
  reason TEXT, confidence REAL, evidence JSON,
  status TEXT,               -- proposed|approved|executed|failed|rolled_back
  created_at, executed_at, rolled_back_at
)

audit_log (id, ts, actor,        -- user|agent:<name>|system
           action, target, detail JSON, prev_hash, hash)  -- hash-chained

scan_jobs (id, root, started_at, finished_at, status,
           files_seen, files_changed, errors JSON,
           checkpoint JSON)       -- resume token

rules (id, name, scope_json, condition_json, action_json,
       priority INTEGER, enabled BOOLEAN, source TEXT)  -- user|plugin|builtin

security_findings (id, file_id, finding_type, severity,
                   evidence JSON, status, resolved_at)

settings (key PK, value JSON, scope TEXT)  -- global|project|root
```

Qdrant collections: `chunks` (dim per chosen embedding model), `entities`, `projects`. Payload indexes on `file_id`, `project_id`, `sensitivity` for filtered search.

---

## 10. API & Interfaces

### 10.1 Internal IPC (Tauri commands)
Typed command surface (~40 commands): `scan.start/stop/status`, `search.query`, `simulator.preview/apply/rollback`, `actions.list/approve/reject`, `health.get`, `findings.*`, `settings.*`, `agents.run`, event stream via `tauri::event` for progress.

### 10.2 Local REST/GraphQL (V1+)
`localhost:4777` only, bearer-token auth (token in app config, rotatable). GraphQL schema exposes: `search`, `file`, `project`, `graph(neighborhood)`, `actions`, `health`, `findings`. REST for plugin callbacks. Never bound to non-loopback without explicit user config + warning.

### 10.3 Plugin SDK (V2)
Manifest (`aegis-plugin.json`): id, version, permissions (`fs:read-index`, `propose-actions`, `ui:dashboard`, `net:optional`), entry points. Sandboxed: plugins run in WASM or restricted process; can register parsers, agents, widgets, rules. No raw filesystem write access — only via Action Proposals.

### 10.4 CLI
`aegis` binary: `search "query" [--json]`, `scan [--root]`, `dedupe report|apply`, `health`, `actions approve <id>`, `doctor` (integrity checks). Shares engine via local socket.

---

## 11. User Interface

### 11.1 Screens (MVP)

1. **Onboarding wizard** — welcome, root selection, ignore templates, performance budget, first scan progress (real ETA from measured throughput).
2. **Dashboard** — health score ring + dimension breakdown, security alerts, pending approvals count, storage reclaimable, "since last visit" digest.
3. **Search** — omnibox with NL + faceted filters, result cards with match explanations, preview pane, keyboard navigation.
4. **Files/Browse** — virtual view organised by *Aegis classification* (Projects / People / Topics / Types), not raw paths; toggle to raw filesystem view.
5. **Duplicates** — cluster list, keeper recommendation, storage delta, bulk actions via quarantine.
6. **Simulator** — before/after tree diff, per-file rationale on hover, confidence badges, approve-all / selective approve, rollback button post-execution.
7. **Approvals inbox** — all pending proposals (refile, lifecycle, security) with reason/evidence/confidence.
8. **Security inbox** — findings triage.
9. **Graph (basic V1)** — 2D force graph of project neighbourhood; V3 full Twin Viewer (3D, temporal scrubber).
10. **Settings** — roots, ignore rules, autonomy dial per action class, models, cloud opt-in, telemetry, updates.

### 11.2 UX Principles
- **Progressive disclosure:** simple mode (dashboard+search) vs. power mode (simulator, graph, rules editor).
- **Explainability inline:** every score, badge, and proposal has a "why?" affordance.
- **Keyboard-first:** global hotkey for search; vim-style list nav optional.
- **Honest states:** scans show coverage %, degraded sources flagged, never fake 100%.

---

## 12. Security & Privacy

| Requirement | Implementation |
|-------------|----------------|
| Local-first | All indexing/inference on-device; network calls only explicit opt-ins |
| Encryption at rest | AES-256-GCM for: (a) exported bundles, (b) vault dir (V2), (c) optional full-DB encryption via SQLCipher — user-passphrase mode available from V1.5 |
| Key management | Passphrase → Argon2id → DEK; keys never logged; OS keychain for stored secrets |
| RBAC | Single-user MVP = profile roles (Admin/Member); multi-user RBAC = Enterprise |
| Audit | Hash-chained `audit_log`; exportable; tamper-evident |
| Secure deletion | Quarantine → overwrite-pass delete (DoD 5220.22-M 3-pass optional); raw `unlink` only for non-sensitive |
| Supply chain | Pinned deps, signed releases, SBOM published, model checksums verified |
| Telemetry | Opt-in only; never file names/contents — counts and timings only |
| Secrets hygiene | Repo scans include self-check; no credentials in config — OS keychain only |

---

## 13. Non-Functional Requirements

| Category | Target |
|----------|--------|
| Scale | 1M+ files per device; 10M+ chunks in vector index (tested to 5M MVP) |
| First scan | ≤6h for 1M files; resumable; ETA honest |
| Incremental | ≤60s detection for typical changes; watchers real-time for active roots |
| Search latency | p95 < 300ms for hybrid query (excluding LLM parse); <2s with query understanding |
| Resource budget | Idle <1% CPU; scan throttled to configurable % CPU; optional "battery saver" profile halves throughput |
| Memory | ≤4GB RSS during full scan on 16GB machine |
| Reliability | Crash-safe journal (fsync policy), job checkpoint/resume, no partial-commit corruption; `aegis doctor` verifies |
| Uptime model | Desktop app; engine runs as background service while app closed (V1.5), watchers active |
| Compatibility | Windows 10/11, macOS 12+, Linux (Ubuntu 22.04+ primary); NTFS/APFS/ext4 + exFAT read |
| File systems | Skip/unsupported-FS reported honestly (no silent gaps) |
| Update | Signed auto-update with staged rollout + rollback channel |

---

## 14. Observability

- Structured logs (JSON) per component; `aegis doctor` summarises health.
- Metrics: scan coverage %, queue depths, embedding throughput, search latency histogram, approval rates, rollback count.
- Error taxonomy: per-root, per-parser failure buckets — surfaced in UI "coverage report".
- Crash reporting: opt-in, minidumped, no user data.

---

## 15. Roadmap

### MVP (v1.0) — "Understand & Find"
Discovery engine · content extraction (top formats) · embeddings + hybrid search · knowledge graph (basic) · duplicate detection (exact+near) · refiling simulator (rules + simple proposals, approval-gated) · quarantine/undo · security findings (secrets) · dashboard + search + duplicates + simulator + approvals UI · local models (embeddings + small LLM optional).

*Explicitly NOT in MVP:* autonomous execution, cloud LLM, plugins, CLI, multi-device, graph visualisation beyond lists, media transcription, archive lifecycle automation (manual states only).

### v1.5 — "Trust & Reach"
Background service mode · global hotkey + OS context menus · CLI · scheduled housekeeping · smart collections · storage analytics · full graph viewer (2D) · encryption-at-rest option · Research/Developer/Trader as **built-in domains** (pre-plugin).

### v2.0 — "Act & Extend"
Graduated autonomy (auto-approved classes) · agent framework GA · lifecycle automation · security vault · plugin SDK + marketplace · multi-device encrypted sync · cloud-LLM opt-in with redaction · email-import plugins.

### v3.0 — "Operating Layer"
Digital Twin Viewer (3D, temporal) · predictive housekeeping (forecast clutter/security risk) · personal knowledge OS (notes/tasks derived from graph) · team/enterprise tier · federated on-prem deployment.

---

## 16. Metrics & Success Criteria

**North Star:** % of digital assets correctly understood = files with (classification + embedding + project-or-explicit-unfiled) / total indexed.

| Metric | MVP target | V2 target |
|--------|-----------|-----------|
| Search top-3 hit rate (internal eval set) | ≥75% | ≥90% |
| Duplicate bytes detected / actual (recall) | ≥85% | ≥95% |
| Proposal acceptance rate | ≥60% | ≥80% |
| Rollback success rate | 100% | 100% |
| Scan coverage honesty (reported = actual) | 100% | 100% |
| Crash-free sessions | ≥99% | ≥99.5% |
| Secrets recall on test corpus | ≥95% | ≥99% |

Instrumentation: all metrics from opt-in telemetry + local analytics DB (always-on, on-device).

---

## 17. Risks & Mitigations

| Risk | Impact | Mitigation |
|------|--------|-----------|
| Misclassification → wrong filing | User trust loss | Confidence thresholds, approval gates, rollback, "why" explanations |
| Destructive bug in automation | Data loss | Journal + quarantine-only deletes + dry-run mandatory + fuzz-tested path ops |
| LLM hallucination in proposals | Bad suggestions | Proposals cite evidence; low-confidence never auto; agents can't execute |
| Prompt injection via file contents | Agent compromise | Content sandboxing, tool allowlists, no shell access, output sanitisation |
| Performance on huge drives | Unusable product | Incremental-first design, throttling, coverage-honest UI, benchmark CI |
| Privacy perception ("it reads everything") | Adoption failure | Local-only default, visible processing status, per-root opt-in, no content telemetry |
| Model licensing (Llama community licence etc.) | Legal | Prefer Apache/MIT models; legal review before bundling |
| Scope creep toward "OS replacement" | Deadline death | Non-goals enforced; plugin architecture absorbs domain demands |

---

## 18. Open Questions / Decisions Needed

1. **Business model validation:** is Pro-per-seat vs. perpetual-license vs. open-core decided? Affects telemetry, licensing code, and marketplace timing.
2. **LLM bundling:** ship quantised model (~2–4GB download) or require user download? Battery/UX tradeoff.
3. **PostgreSQL timing:** is embedded-SQLite→PG migration path designed now, or deferred to sync feature?
4. **macOS/Linux parity:** which is second platform? File-watcher and permission models differ significantly.
5. **Team features:** is Enterprise real (budget, sales) or aspirational roadmap?
6. **OCR/transcription defaults:** opt-in per-root vs. global toggle — cost disclosure UX.
7. **Domain modes:** built-in vs. plugin-first for Research/Developer/Trader.

---

## 19. Glossary

- **Digital twin:** the queryable semantic model of the user's environment.
- **Action Proposal:** structured, evidence-backed request to mutate state; the only path to filesystem change.
- **Journal:** append-only, hash-chained record enabling audit and rollback.
- **Quarantine:** reversible delete staging area with TTL.
- **Autonomy dial:** per-action-class setting (Manual/Suggest/Auto-approved/Auto).
- **Coverage:** % of discoverable files successfully indexed — reported honestly.
