# Aegis Guardian — Critical Analysis & Solution Hierarchy

> **STATUS UPDATE (2026-11-20):** All tiers executed. Suite split per audit §D into standalone docs; a second independent pass (`critical_analysis-v2.md`, "Suite Athena") produced superior A2/B4/B8 versions — reconciled in `suite_reconciliation.md`, which declares the canonical doc map, resolves the ADR-numbering conflict, and lists 8 issues (I1–I8) to patch in a v2.2 revision. Remaining gap: **B6 Security Threat Model** (P1).

**Basis:** `Aegis Guardian PRD v2.md` as audited in `critical_analysis.md`
**Purpose:** Turn the audit into an actionable, sequenced fix plan
**Date:** 2026-11-20

---

## Part 1 — Critical Analysis (Synthesis)

### 1.1 Verdict

The v2.0 spec is **exceptional as a product philosophy and a founding blueprint**, but **not yet executable as an engineering spec**. Its core safety thesis — *reversibility, transparent uncertainty, graduated autonomy, proposals-not-writes* — is genuinely differentiated and correct. The failures are concentrated in one place: **the math and contracts don't close**. Nothing about the product vision needs to change; the specification layer does.

### 1.2 The Five Findings That Matter

| # | Severity | Finding | Why it matters |
|---|----------|---------|----------------|
| F1 | **BLOCKER** | Memory/throughput math doesn't close: ≤4GB RSS can't host BGE-M3 + LLM + Qdrant + Tauri; 50 emb/sec × 5–10M chunks = 28–56h, not 6h | Every NFR and every schedule downstream of §6.3/§13 is fictional until fixed |
| F2 | **BLOCKER** | Cross-volume quarantine: single `.aegis/quarantine` forces 20GB cross-drive copies, can fill boot SSD | Breaks the #1 safety promise (reversible deletes) on exactly the users with external drives |
| F3 | **MAJOR** | Identity tracking contradiction: `volume_id+file_index` in MVP schema + watcher logic, but FR-001.6 defers it to P1 | Every rename in MVP silently destroys graph edges and re-embeds — trust-killer |
| F4 | **MAJOR** | Schema drift: no FTS5 table for §7.7 hybrid search; no model for `archive://` members lacking OS file identity; no `nlink` for hardlink-aware dedupe | Schema is the contract; gaps here propagate to every subsystem |
| F5 | **MAJOR** | Goal/metric scope drift: §3.1 claims >90% search / ≥80% acceptance unqualified; §16 sets MVP at 75%/60% | QA will fail the product against its own goals |

### 1.3 Structural Diagnosis

The monolith-PRD problem is real: one 450-line file is doing the job of **8 documents** (PRD, architecture, data spec, ADRs, API contracts, eval plan, ops runbook, roadmap). Monolith specs go stale or cause merge conflicts the moment two engineers touch different sections. The fix isn't rewriting — it's **decomposing along the seams the audit already drew**.

### 1.4 What's Already Right (don't touch)

- Product principles, non-goals, graduated autonomy, Action-Proposal-only agent writes
- Rust+Python split, Tauri, SQLite-WAL, Qdrant, hash-chained audit
- Roadmap phasing instinct (MVP excludes autonomy/plugins/cloud)
- Persona breadth and domain-plugin strategy

---

## Part 2 — Solution Hierarchy

Ordered by dependency and blast radius. Each tier must be substantially complete before the next delivers value. **Tier 0 unblocks everything; Tier 4 is upside.**

```
TIER 0 — CONTRADICTION RESOLUTION        (spec is internally consistent)
   └─ TIER 1 — ARCHITECTURE LOCK         (decisions made, ADRs written)
        └─ TIER 2 — MVP BUILD UNBLOCKERS (schema, contracts, safety invariants)
             └─ TIER 3 — TRUST & DIFFERENTIATION (the 10x features)
                  └─ TIER 4 — SCALE & ECOSYSTEM (V1.5/V2/V3 bets)
```

---

### TIER 0 — Resolve Contradictions (P0, days not weeks)

*Goal: zero unresolved contradictions between §6.3, §7.1, §8.3, §9, §13. Acceptance: memory budget table sums to ≤4GB RSS under active scan.*

| ID | Solution | Resolves | Effort |
|----|----------|----------|--------|
| S0.1 | **Hardware-tiered inference profiles** (Eco 8GB / Standard 16GB / Pro GPU): `bge-small` (90MB) + FTS5-only on Eco; LLM loaded on-demand and evicted after 5min idle on Standard; full stack on Pro | F1 (RAM) | S |
| S0.2 | **Selective/hierarchical chunking**: chunk only docs >1,500 tokens; Tier-2 scan caps at metadata + first-N chunks + summary embedding | F1 (throughput) | S |
| S0.3 | **Promote `volume_id+file_index` to P0** in FR-001.6 | F3 | XS |
| S0.4 | **Per-volume quarantine**: `.aegis-quarantine` on each volume root → O(1) atomic renames, no cross-drive copy; fallback copy+verify+delete only if non-writable | F2 | S |
| S0.5 | **Reconcile §3.1 vs §16**: label >90%/≥80% as V2 targets in goals table | F5 | XS |
| S0.6 | **Hardlink awareness**: add `nlink`; never count shared blocks as reclaimable | F4-adjacent | S |

**Deliverable:** `B2` Architecture spec + patched PRD sections. No new documents needed beyond the tier-2 split below.

---

### TIER 1 — Lock Architecture via ADRs (P0, before first sprint)

*Goal: prevent mid-sprint architectural churn. Four ADRs, each one page.*

| ID | ADR | Decision to formalise | Why now |
|----|-----|----------------------|---------|
| S1.1 | **ADR-001: Rust-first extraction** | MVP hot path 100% Rust: `ort`/`fastembed` (embeddings), `tree-sitter` (AST), `calamine`/`quick-xml`/`kamadak-exif` (Office/EXIF), `pdfium-render` (PDF). Python deferred to V1.5+ optional plugins (Whisper, complex OCR, PST) | Avoids 1.5–3GB installer, gRPC tax on 1M files, Defender false positives, RAM overhead |
| S1.2 | **ADR-002: Per-volume quarantine + OS Trash delegation** | Volume-local quarantine; OS Trash where reliable; read-only/external volume policy (open question §18) | Safety-critical; cheapest to decide now |
| S1.3 | **ADR-003: Atomic Project Units** | Boundary markers (`.git`, `package.json`, `pyproject.toml`, `.xcodeproj`, `.app`, `-wal/-shm` companions, multi-file LaTeX/Premiere) make folders **indivisible** — refiling moves the whole unit, never plucks interior files | Prevents the #1 trust-destroying failure mode (breaking repos/builds) |
| S1.4 | **ADR-004: Model lifecycle scheduler** | On-demand LLM load/evict; embedding model pinned per hardware tier; model checksums verified at download | Makes the 4GB budget actually hold |

**Deliverable:** `B8` ADR set. ~4 pages total.

---

### TIER 2 — MVP Build Unblockers (P0/P1, the spec gaps engineers hit first)

*Goal: every subsystem has a contract precise enough to implement against.*

| ID | Solution | Unblocks | Priority |
|----|----------|----------|----------|
| S2.1 | **`B4` Data & Vector Schema Spec**: add FTS5 virtual table + sync trigger, `archive://` virtual-asset model (container FK, no OS identity), `nlink`, SQLite→PG migration plan, Qdrant chunking strategy (dim, payload indexes) | DB/indexer implementation | **P0** |
| S2.2 | **`C1` IPC/Event/Action Contract**: typed schemas for ~40 Tauri commands, `ActionProposal` state machine, rollback conflict rules | Frontend/backend parallel work | **P1** |
| S2.3 | **Rollback state machine**: pre-reverse verify `(exists(to), blake3==post_hash, !exists(from))`; on conflict → 3-way choice (restore-as-rename / skip / abort batch) | Data-loss edge case | **P0** |
| S2.4 | **Risk-weighted secret triage**: context multipliers (`keys.txt`=Critical, `test/fixtures`=Info), dedupe identical secret hashes across N copies into 1 finding, provider-checksum validation over raw entropy | Security inbox usability | **P1** |
| S2.5 | **OS permission & cloud-placeholder flows**: macOS TCC states → "Permission Denied" in coverage report (never silent 0); check `SF_DATALESS`/`FILE_ATTRIBUTE_RECALL_ON_DATA_ACCESS` **before** opening handles to avoid silent cloud downloads; NTFS MFT/USN fast-path + FSEvents history reconciliation | Correctness on real machines | **P1** |
| S2.6 | **`F2` Safety test plan**: fuzz path/symlink/quarantine ops; eval corpus for the §16 metrics | "Transparent uncertainty" claims | **P1** |

**Deliverable:** `B4` schema spec + `C1` contracts + patched FRs. This is the documentation suite the audit recommends splitting into A2/B1/B2/B4/B8/C1/F1-F2.

---

### TIER 3 — Trust & Differentiation (P1, the 10x features)

*Goal: features that make Aegis feel inevitable rather than useful. All build on Tier-2 contracts.*

| ID | Solution | Value | Priority |
|----|----------|-------|----------|
| S3.1 | **3-Tier Progressive Indexing Funnel**: T1 (0–90s) metadata-only → storage analytics, size-dupes, filename-secret alerts, health baseline; T2 (minutes) prioritise active dirs (Desktop/Downloads/Documents/git repos/recent-90d); T3 background cold/OCR/media | **Kills the 6-hour cold-start churn** — Day-1 value in 90 seconds | **P0-feature** |
| S3.2 | **Proposal-to-Rule Graduation** ("Teach Aegis Once"): approving a proposal offers synthesise-a-deterministic-rule; rules are human-editable | Trust compounds; deterministic rules beat black-box ML for user confidence | **P1** |
| S3.3 | **Crisis Playbooks**: Disk Rescue ("need 50GB now"), Pre-Share Security Audit, New-Machine Migration Prep | High-emotion entry points = adoption moments | **P1** |
| S3.4 | **Local taxonomy calibration**: approval/rejection pairs → few-shot routing exemplars so Curator learns *the user's* folder logic, not a generic tree | Personalisation moat | **P1** |
| S3.5 | **Tamper-evidence clarification + signed checkpoints**: define threat model honestly (detects corruption/non-chain edits); optional signed audit export for forensics | Trust under adversarial framing | **P2** |

---

### TIER 4 — Scale & Ecosystem (P2/P3, deferred bets)

| ID | Solution | Version |
|----|----------|---------|
| S4.1 | Plugin SDK + marketplace (WASM-sandboxed, permission manifest) | V2 |
| S4.2 | Graduated autonomy GA (auto-approved classes), full agent framework | V2 |
| S4.3 | Multi-device encrypted index sync; team/enterprise graph + RBAC | V2/V3 |
| S4.4 | Digital Twin 3D viewer, predictive housekeeping, personal knowledge OS | V3 |
| S4.5 | Optional Python/plugin runtime for heavy parsers (Whisper, VLM, PST) | V1.5+ |
| S4.6 | OS fast-path enumeration at scale (NTFS MFT/USN, FSEvents history) beyond MVP watchers | V1.5 |

---

## Part 3 — Execution Sequence (Recommended Order)

```
Week 1   → S0.1–S0.6   Patch spec contradictions (B2 + PRD errata)
Week 2   → S1.1–S1.4   Write ADR-001..004 (B8)
Week 3   → S2.1, S2.3  B4 schema spec + rollback state machine (P0 unblockers)
Week 4   → S2.2, S2.4, S2.5, S2.6   Contracts, triage, OS flows, test plan
Then     → S3.1        Progressive indexing (biggest product win, designs against settled schema)
          → S3.2–S3.4  Trust features in parallel once C1 contracts land
          → S4.x       Only when V1.5/V2 scope is actually scheduled
```

**Critical path:** `S0 → S1 → S2.1/S2.3 → S3.1`. Everything else parallelises or defers.

---

## Part 4 — Open Questions to Resolve (merged from audit §5 + PRD §18)

1. **Rust-native extraction?** Can MVP formats run 100% Rust, deferring Python to V1.5+? *(ADR-001 answers this — recommend YES)*
2. **Minimum hardware baseline?** Is 8GB/dual-core the floor, and is local-LLM query parsing disabled there? *(ADR-004 answers — recommend YES, Eco tier)*
3. **Atomic Unit boundary markers?** Which markers lock a directory against internal refiling? *(ADR-003 — propose the list in §B.2)*
4. **Quarantine on read-only/external volumes?** OS Trash vs. explicit-delete-confirm when `.aegis-quarantine` unwritable? *(ADR-002)*
5. **Business model:** Pro-per-seat vs. perpetual vs. open-core — affects telemetry/licensing code *(carried from PRD §18)*
6. **LLM bundling:** ship 2–4GB quantised model or user-download? *(carried)*
7. **Platform order:** Windows-first confirmed? macOS TCC flows are P1 not P0 *(carried)*

---

## Part 5 — What Success Looks Like

- **Spec health:** audit re-run scores Correctness ≥4.5, Implementability ≥4.0, Traceability ≥4.0
- **Engineering:** first sprint starts against `B4`+`C1` contracts with zero "wait, what did the PRD mean" stalls
- **Product:** user sees storage analytics + duplicate candidates + health baseline **within 90 seconds** of first launch (S3.1)
- **Trust:** 100% of mutations reversible *across volumes*, with pre-rollback conflict handling (S0.4 + S2.3)
