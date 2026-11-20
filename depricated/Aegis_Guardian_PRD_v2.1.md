# Aegis Guardian
## Product Requirements Document — v2.1 (Lean, Build-Ready)

> **SUPERSEDED** — canonical PRD v2.1 is `A2_PRD_v2.1.md` (extracted from `critical_analysis-v2.md` §Document 3; see `suite_reconciliation.md` §2). This file is retained as a leaner reading copy; its unique content is folded into A2 or the companion specs. Do not cite FR numbers from this file — use A2's numbering.

Version: 2.1
Status: MVP Specification — supersedes v2.0
Changes from v2.0: resolved all audit contradictions (see `solution_hierarchy.md` Tier 0–2); added Atomic Project Units, Progressive Indexing, hardware tiers, per-volume quarantine, proposal-to-rule, crisis playbooks; reconciled goal/metric targets; promoted `volume_id+file_index` to P0.

Companion docs: `B1_B2_Architecture.md`, `B4_Data_Vector_Schema.md`, `B8_ADRs.md`, `C1_IPC_Action_Contracts.md`, `F1_F2_Eval_Safety_Plan.md`.

---

## 1. Executive Summary

Aegis Guardian is an AI-native, local-first desktop platform that builds a semantic **digital twin** of a user's computing environment to search by meaning, deduplicate, detect exposed secrets/PII, manage file lifecycles, and safely reorganise files through a previewable, journaled, reversible automation pipeline.

Core beliefs: **meaning over location · reversibility over automation · local-first privacy · explainability · human sovereignty.**

## 2. Problem Statement

Users accumulate years of unstructured assets across drives, NAS, and cloud-sync folders. Conventional tools fail at: retrieval by meaning, structural decay, silent duplication, lifecycle neglect, security blind spots, and tool fragmentation. Aegis is the single intelligent layer that subsumes them.

## 3. Goals and Non-Goals

### 3.1 Goals

| # | Goal | MVP (v1.0) target | V2 target |
|---|------|-------------------|-----------|
| G1 | Find anything by meaning | ≥75% top-3 hit rate | ≥90% |
| G2 | Surface & resolve duplication | ≥85% dedupe recall; ≥30% storage reclaim in pilots | ≥95% recall |
| G3 | Safe reviewable reorganisation | ≥60% proposal acceptance; 100% reversible | ≥80% |
| G4 | Complete auditability | 100% mutations journaled + reversible | + tamper-evident signed checkpoints |
| G5 | Detect sensitive data | ≥95% secret recall on eval corpus | ≥99% |

### 3.2 Non-Goals

Not a backup service, not an editor/office suite, not an email client, not multi-user collaboration (MVP), not guaranteed-perfect classification. Domain depth arrives via plugins/built-in domains, not core bloat.

## 4. Personas

Researcher · Developer · Trader · Knowledge Worker · Student · Household Admin. Core engine is domain-agnostic; Research/Developer/Trading are V1.5 built-in domains → V2 plugins.

## 5. Product Principles

1. Understand before modifying. 2. Always explain decisions. 3. Maintain reversibility. 4. Humans in control (graduated autonomy dial). 5. Privacy by design. 6. Fail safe, not silent (honest coverage). 7. Performance is a feature.

## 6. System Architecture (summary — full spec in B1/B2)

```
Tauri 2.x desktop (React+TS+Tailwind)
        │ IPC commands/events (C1 contract)
Rust core engine (single native binary, ADR-001)
  ├─ Discovery/Watchers (notify + OS fast-paths)
  ├─ Extraction (format parsers, all-Rust MVP set)
  ├─ Classification/Embeddings (ONNX via ort/fastembed)
  ├─ Knowledge Graph + FTS5 (SQLite WAL) + Qdrant embedded
  ├─ Automation Engine (plan→simulate→execute→audit→rollback)
  └─ Agent Orchestrator (V2; proposals-only even then)
SQLite WAL · Qdrant embedded · journal/quarantine on volume roots
```

**Hardware tiers (ADR-004):** Eco (8GB, CPU: bge-small + FTS5, no LLM) · Standard (16GB: bge-small/bge-m3 quantised + on-demand 1.5–3B LLM evicted after 5min idle) · Pro (GPU/32GB+: full stack + reranker + 7–8B LLM). Memory budget ≤4GB RSS during scan on Standard tier — see B1/B2 §memory table.

## 7. Functional Requirements

Priority: **P0** MVP · **P1** v1.5 · **P2** v2 · **P3** v3.

### 7.1 Discovery (FR-001)

| ID | Req | Pri | Acceptance |
|----|-----|-----|-----------|
| FR-001.1 | Configurable roots: internal/external drives, NAS/SMB, cloud-sync folders | P0 | Per-root toggle; all types enumerable |
| FR-001.2 | Ignore rules: `.aegisignore` (gitignore syntax), OS junk, denylist; `node_modules` opt-in | P0 | Ignored paths never indexed |
| FR-001.3 | Graceful permission errors → coverage report, never crash | P0 | Partial coverage reported honestly |
| FR-001.4 | Symlink/reparse-point policy explicit, default skip | P0 | No loops |
| FR-001.5 | Lazy BLAKE3 hashing in background queue | P0 | Metadata scan unblocked by hash backlog |
| FR-001.6 | **Stable file identity via `volume_id+file_index` (NTFS) / inode (POSIX)** | **P0** | Renames/moves preserve id, embeddings, graph edges |
| FR-001.7 | Email-export ingestion (mbox/.eml; PST opt-in) | P2 | Attachments as first-class assets |
| FR-001.8 | Offline/NAS roots marked stale, degrade gracefully | P1 | No failure on mid-scan disconnect |

### 7.2 Progressive Indexing Funnel (NEW — S3.1)

Three tiers replace the single cold scan:

- **Tier 1 (0–90s):** OS-fast metadata sweep (NTFS MFT/USN where permitted, directory stat elsewhere) → immediate Storage Analytics, exact-size dupe candidates, filename-secret alerts (`.env`, `*.pem`, `id_rsa`), initial Health baseline. **User-visible value before any content read.**
- **Tier 2 (minutes):** prioritise high-signal dirs — Desktop, Downloads, Documents, active git repos, files modified <90d — for extraction + embeddings. AI search works on recent work early.
- **Tier 3 (background/idle/plugged-in):** cold archives, deep history, OCR, media transcription.

Acceptance: Tier-1 dashboard populated <90s on 100k-file volume; Tier-2 covers ≥80% of files accessed in last 90d within 15 min on Standard tier.

### 7.3 Content Intelligence (FR-002)

MVP parsers (all-Rust, ADR-001): PDF via `pdfium-render` (+OCR opt-in), DOCX/XLSX/PPTX via `calamine`/`quick-xml`/zip+XML, TXT/MD/CSV/JSON, images via `kamadak-exif` + OCR opt-in, source via `tree-sitter`. Audio/video transcription, PST, VLM captioning → V1.5+ optional runtime. `archive://` members indexed as virtual assets (B4 §virtual).

Capabilities: OCR (opt-in), NER, topic detection, project ID, language detection, sensitivity flagging.

### 7.4 Knowledge Graph (FR-003)

Entities: File, Folder, Project, Person, Company, Topic, Repository, Account, Event, Collection. Typed weighted temporal relationships. Storage per B4; traversal cache materialised.

### 7.5 Duplicate Intelligence (FR-004)

1. Exact: BLAKE3 groups — **hardlink-aware** (`nlink>1` shares blocks → never counted as reclaimable). 2. Near: MinHash/LSH text shingles, dHash images. 3. Semantic: embedding cosine ≥0.92. 4. Version chains: pattern+similarity+temporal → chain UI only.

Output: duplicate report, keeper heuristic (recency, path cleanliness, project membership), reclaimable-bytes honest accounting.

### 7.6 Autonomous Refiling (FR-005) — core differentiator

Pipeline: Analyse → Simulate → Approve → Execute → Audit → Rollback. Confidence routing: ≥0.9 auto-eligible (if enabled) · 0.6–0.9 review · <0.6 leave+flag.

**NEW invariants:**
- **Atomic Project Units (ADR-003):** folders containing boundary markers (`.git`, `package.json`, `pyproject.toml`, `Cargo.toml`, `.xcodeproj`, `.app`/`.app` bundles, `-wal/-shm` companions, multi-file LaTeX/Premiere) are **indivisible** — proposals may move/archive the whole unit, never interior files, unless user explicitly overrides.
- **Per-volume quarantine (ADR-002):** all deletes are O(1) renames to `<volume>/.aegis-quarantine` (TTL 30d); OS Trash delegation where reliable; copy+verify+delete only when volume non-writable.
- **Collision rules:** never silent overwrite; locked/open files skipped+reported; dry-run always available.
- **Rollback state machine (S2.3):** pre-reverse verify `(exists(to_path) ∧ blake3==post_hash ∧ !exists(from_path))`; on conflict → user chooses restore-as-rename / skip / abort batch.

### 7.7 Workspace Health (FR-006)

Composite 0–100 across Organisation, Storage, Security, Duplication, Searchability — every score explainable to contributing factors. Tier-1 baseline available <90s.

### 7.8 AI Search (FR-007)

Hybrid retrieval: FTS5 BM25 + Qdrant ANN fused via RRF; optional cross-encoder reranker (Pro tier). Query understanding: regex/rule parser on Eco; small LLM on Standard/Pro. Result cards with match explanations. Global hotkey V1; OS context-menu V1.5. MVP queries must pass: "latest dissertation draft", "files related to portfolio optimisation", "Aegis risk engine screenshots".

### 7.9 Security Intelligence (FR-008)

Detection: API keys/tokens, passwords, certs/keys, identity docs, financial data, PII. **Risk-weighted triage (S2.4):** context multipliers (`~/Downloads/keys.txt`→Critical; `test/fixtures`/README examples→Info/suppressed), identical-secret-hash dedupe across N copies → 1 finding, provider-checksum validation (`ghp_`, `sk_live_`, AWS pairs) over raw entropy. Output: Security inbox with reveal/open/move-to-secure/false-positive actions. Vault = V2.

### 7.10 Archive & Lifecycle (FR-009)

Active→Warm→Cold→Archive (+Quarantine). Configurable per-domain rules; archive = relocate to archive root (never delete); lifecycle ops journaled+reversible; search-access on archived item offers restore.

### 7.11 Continuous Monitoring (FR-010)

Watchers per root (debounced 500ms), identity-stable rename/move handling via file-index, cloud-placeholder awareness (never trigger hydration reads — B1/B2 §cloud), polling fallback 15min, crash reconciliation vs journal.

### 7.12 Additional Features

| ID | Feature | Pri |
|----|---------|-----|
| FR-011 | Onboarding wizard: roots, ignore templates, perf budget, honest-ETA funnel progress | P0 |
| FR-012 | Global undo stack across subsystems | P0 |
| FR-013 | Quarantine zone (per-volume) + restore UI | P0 |
| FR-014 | CLI `aegis` (search/scan/dedupe/health/doctor) | P1 |
| FR-015 | Smart Collections (saved live queries) | P1 |
| FR-016 | Plugin SDK (WASM-sandboxed, permission manifest) | P2 |
| FR-017 | Scheduled housekeeping | P1 |
| FR-018 | Storage analytics (treemap, growth, weekly delta) | P1 |
| FR-019 | Digest notifications (opt-in, quiet default) | P1 |
| FR-020 | Multi-device encrypted index sync | P2 |
| FR-021 | Team/enterprise graph + RBAC | P3 |
| FR-022 | Accessibility + i18n framework | P1 |
| FR-023 | **Proposal-to-Rule graduation** ("teach Aegis once") | P1 |
| FR-024 | **Crisis playbooks**: Disk Rescue, Pre-Share Audit, Migration Prep | P1 |

## 8. Agent Framework (V2; contracts now)

Orchestrator owns queue/tools/policy/budgets/approval gates. Agents emit **Action Proposals only** — never write. Roster & autonomy ceilings per v2.0 §8.2 (all propose-only except Archivist Warm→Cold auto-eligible when enabled). Models per hardware tier (§6). Cloud LLM opt-in with pre-egress secret/PII redaction + audit.

## 9. Data Model

Authoritative schema: **`B4_Data_Vector_Schema.md`**. Summary: `files` (incl. `volume_id`, `file_index`, `nlink`, `state`, `sensitivity`), `chunks`, FTS5 virtual table, `entities`/`relationships` (temporal), `projects`, `actions` (proposal lifecycle), hash-chained `audit_log`, `scan_jobs` (checkpoint/resume), `rules`, `security_findings`, `settings`. Virtual assets (`archive://`, email attachments) modelled per B4 §virtual.

## 10. APIs & Interfaces

Authoritative contracts: **`C1_IPC_Action_Contracts.md`**. Local REST/GraphQL on `localhost` only + bearer token (V1). Plugin SDK V2.

## 11. UI (MVP screens)

Onboarding · Dashboard · Search · Files (Aegis-classified view + raw toggle) · Duplicates · Simulator · Approvals inbox · Security inbox · Settings. Progressive disclosure (simple vs power mode); inline "why?" on every score/proposal; honest coverage states. Graph viewer V1.5; Twin Viewer V3.

## 12. Security & Privacy

Local-first; AES-256-GCM for exports/vault/optional SQLCipher DB encryption; Argon2id key derivation, OS keychain; RBAC Enterprise; hash-chained audit (+signed checkpoints V2, threat model clarified in B1/B2 §audit); secure deletion via quarantine→optional 3-pass overwrite; signed releases + SBOM; opt-in telemetry never carrying file names/content; cloud-placeholder hydration guard (B1/B2 §cloud); OS permission states surfaced in coverage report.

## 13. Non-Functional Requirements

| Category | Target |
|----------|--------|
| Scale | 1M files/device; 5M chunks MVP-tested (10M design target) |
| Tier-1 value | Dashboard+analytics <90s on 100k files |
| First full index | ≤6h / 1M files on **Standard tier** (selective chunking, per B1/B2 throughput model) |
| Incremental | ≤60s detection typical changesets; real-time watchers |
| Search | p95 <300ms hybrid (Eco: FTS5+small-embed; Pro adds rerank) |
| Memory | ≤4GB RSS active scan (Standard); Eco ≤2.5GB; Pro ≤8GB |
| Reliability | Checkpoint/resume jobs; crash-safe journal; `aegis doctor` verifies |
| OS | Win10/11, macOS 12+, Ubuntu 22.04+; NTFS/APFS/ext4 |
| Resources | Idle <1% CPU; configurable throttle; battery-saver profile |
| Update | Signed auto-update, staged rollout, rollback channel |

## 14. Observability

JSON structured logs; per-component metrics (coverage, queue depth, embed throughput, search latency, approval/rollback rates); error taxonomy per root/parser; opt-in minidump crash reporting.

## 15. Roadmap

- **v1.0 MVP:** funnel indexing, extraction (Rust set), hybrid search, basic graph, dedupe (exact+near+hardlink-aware), simulator+rollback, quarantine/undo, secrets triage, core UI, hardware tiers.
- **v1.5:** background service, hotkey/context menus, CLI, scheduled housekeeping, smart collections, storage analytics, 2D graph, DB encryption option, built-in domains, OS fast-paths (MFT/USN, FSEvents history), optional Python runtime for heavy parsers.
- **v2.0:** graduated autonomy, agent framework GA, lifecycle automation, vault, plugin SDK+marketplace, multi-device sync, cloud-LLM opt-in.
- **v3.0:** Twin Viewer, predictive housekeeping, knowledge OS, enterprise tier.

## 16. Metrics

North Star: % assets correctly understood (classification+embedding+project-or-explicit-unfiled). Targets per §3.1 table. Instrumentation: local analytics DB always-on; content telemetry never leaves device.

## 17. Risks & Mitigations

Misclassification → thresholds+approval+rollback+why. Destructive automation → journal+quarantine-only+dry-run+fuzz tests (F2). LLM hallucination → evidence-cited proposals, no agent writes. Prompt injection → content sandboxing, tool allowlists, no shell. Performance → incremental-first, throttling, honest coverage. Privacy perception → local default, visible status, per-root opt-in. Scope creep → non-goals + plugin absorption. *New:* cloud-placeholder accidental hydration → pre-open attribute check (B1/B2 §cloud). Secret-inbox flooding → risk-weighted triage (§7.9).

## 18. Open Questions

1. Business model (per-seat/perpetual/open-core) — affects licensing/telemetry code.
2. LLM bundling: ship quantised model vs user download (Standard/Pro tiers).
3. PostgreSQL timing: embedded SQLite→PG migration designed in B4; trigger = sync/enterprise.
4. Platform order beyond Windows (macOS TCC flows are P1).
5. Enterprise realism (budget/sales vs aspirational).
6. OCR/transcription defaults: per-root opt-in recommended.
7. ~~Rust-native extraction?~~ **Resolved → ADR-001: yes for MVP set.**
8. ~~8GB floor / LLM disabled?~~ **Resolved → ADR-004: Eco tier, no resident LLM.**
9. ~~Atomic Unit markers?~~ **Resolved → ADR-003.**
10. ~~Quarantine on unwritable volumes?~~ **Resolved → ADR-002.**

## 19. Glossary

Digital twin · Action Proposal · Journal · Quarantine (per-volume) · Autonomy dial · Coverage · Atomic Project Unit · Progressive Indexing Funnel · Hardware Tier.
