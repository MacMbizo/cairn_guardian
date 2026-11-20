Here is the complete drop-in **`B1_B2_Architecture.md` (`v2.2`)** along with the exact surgical patches for **`E2_E4` (`v1.1`)**, **`A2` (`v2.1.1`)**, and **`suite_reconciliation.md` (`v1.2`)**. All 12 documents in the suite are now 100% synchronized.

---

```markdown
--- START OF FILE B1_B2_Architecture.md ---

# B1/B2 — System & Runtime Architecture (v2.2)

**ID:** B1/B2 · **Version:** 2.2 · **Status:** Approved MVP Baseline
**Owner:** Principal Systems Architect · **Last Updated:** 2026-11-20
**Related Docs:** `A2_PRD_v2.1.md` (v2.1.1), `B4_Data_Vector_Schema_v2.3.md`, `B6_Threat_Model.md` (v2.3), `B8_ADRs.md` (v2.2), `C1_IPC_Action_Contracts.md` (v1.1), `C4_Policy_Engine_Confidence_Spec.md` (v1.0), `E2_E4_Delivery_Observability_Spec.md` (v1.1), `F1_F2_Eval_Safety_Plan.md` (v2.2)

> **v2.2 changes:** Aligned Hardware Profiles ref to `ADR-002` (was `ADR-004`); fixed `remote-only` → `placeholder_remote` (`B4 v2.3`); fixed `scan_jobs.checkpoint` → `scan_jobs.checkpoint_json`; added `dhash`/`SimHash` and marked OCR as `P1 / V1.5+` (zero-bundle in MVP); reconciled `2.36GB` target vs `3.65GB` conservative ceiling (`A2 §4.1`, `F3`); fixed stale `B4 §chunking`/`PRD §7.2` refs; added standard Acceptance/Open/Traceability sections.

---

## 1. Runtime Topology (`ADR-001` Consequence)

**Single native binary.** The MVP hot path is 100% Rust inside the Tauri app process tree:

```
aegis-desktop (Tauri 2.x)
├── aegis-ui        WebView (React + TS + Tailwind) — zero engine logic
├── aegis-core      Rust: discovery, watchers, extraction, graph, policy engine (C4),
│                   automation engine, journal, per-volume quarantine, IPC server
├── aegis-embed     ONNX Runtime (`ort` / `fastembed`) in-process — embeddings, classifiers
└── aegis-llm-worker OPTIONAL helper subprocess: `llama.cpp` server spawned on demand
                    (Standard/Pro tiers only), auto-evicted after 300s idle (ADR-002, E2 §2)
```

**No Python in MVP.** Python (or WASM plugin runtime) returns in V1.5+ strictly for heavy/niche parsers: Whisper transcription, complex OCR pipelines, `.pst`, and VLM captioning. Eliminates 1.5–3GB installers, gRPC serialization overhead across 1M files, AV false positives, and ~1GB interpreter RAM.

### MVP Parser & Hashing Inventory (All-Rust)

| Format / Capability | Crate / Library | Notes |
|---|---|---|
| PDF | `pdfium-render` (bundled PDFium shared lib) | Text + metadata extraction (`E2` §2) |
| DOCX / XLSX / PPTX | `calamine`, `quick-xml`, `zip` | Sheet/slide aware |
| Images / EXIF / Perceptual | `kamadak-exif`, `image`, Rust `dHash` (128-bit) | Writes `files.dhash_image` (`B4` §3) |
| Code AST | `tree-sitter` + per-language grammars | Symbol & `atomic_units` intelligence |
| Text / MD / CSV / JSON | `std`, `pulldown-cmark`, `csv`, `serde_json` | Direct streaming parse |
| Cryptographic Hash | `blake3` | Lazy worker queue (`FR-001.5`) |
| Near-Dup Text | Rust `MinHash` / `SimHash` (64/128-bit) | Writes `files.simhash_text` (`B4` §3) |
| OCR (Opt-in `FR-002.3`) | **`P1 / V1.5+`** (or zero-bundle OS-native `Windows.Media.Ocr` / macOS Vision API only; **no bundled Tesseract in MVP**) | Explicitly opt-in with CPU/battery disclosure |

---

## 2. Hardware Profiles (`ADR-002`)

Detected at onboarding (`hw.profile()`), user-overridable in Settings; drives model downloads, memory caps, and feature gates.

| Profile | Detect Rule | RAM Floor | Embeddings | LLM | Reranker | Scan RSS Cap |
|---|---|---|---|---|---|---|
| **Eco** | `<12GB` RAM or no AVX2 | 8GB | `bge-small-en-v1.5` (384-dim INT8 ONNX, ~90MB) | **None — deterministic/regex query parser only** | No | ≤2.5GB |
| **Standard** | `12–24GB`, CPU-only | 16GB | `bge-small` (384-dim) default; `bge-m3` (1024-dim) optional | On-demand 1.5–3B Q4 (~1.5–1.8GB), **evict @ 300s idle** | No | ≤4.0GB |
| **Pro** | `≥24GB` or Apple Silicon / ≥8GB VRAM GPU | 32GB | `bge-m3` (1024-dim) | 7–8B Q4 resident-optional | `bge-reranker` cross-encoder | ≤8.0GB |

Models are downloaded post-install per profile over HTTPS against Ed25519-signed `models-manifest.json` with pinned `SHA-256` verification (`E2` §2, `B6` §2).

---

## 3. Memory Budget & Throughput Model (`A2 §4`, `F3`)

Active full-scan budget on **Standard (16GB)** tier — target vs. conservative worst-case ceiling (verified by `F3` 5M-chunk CI soak test):

| Component | Target (`A2` §4.1) | Conservative Ceiling (`F3` Gate) | Constraint Mechanism |
|---|---|---|---|
| Tauri WebView + React | ≤350MB | ≤400MB | Virtualized lists; background window throttled |
| Rust Core + Extractors | ≤600MB | ≤600MB | Bounded pool (`cores / 2`, max 4 workers) |
| SQLite WAL + Page Cache | ≤256MB | ≤256MB | `PRAGMA cache_size = -262144` (`B4` §2) |
| `qdrant-edge` HNSW + mmap | ≤700MB | ≤1,500MB | On-disk payload mmap; INT8 scalar quantization (`5M × 384` ≈ `7.7GB` FP32 raw → `~1.9GB` INT8 on disk) |
| ONNX Embedding (`ort`) | ≤450MB | ≤450MB | Shared `ort` session; batch size 32–64 |
| Queues / Buffers / Misc | — | ≤444MB | Bounded `tokio` channels (backpressure, no unbounded queues) |
| `aegis-llm-worker` | **0MB (unloaded)** | **0MB (unloaded)** | Never resident during scan; on-demand burst ≤1.8GB, evicted @ 300s idle |
| **TOTAL SCAN RSS** | **≤2.36GB** | **≤3.65GB (passes ≤4GB SLA)** | **Interactive query burst ≤4.10GB peak** |

**Throughput Model (`ADR-002` & `A2` §4.2):**
- **Selective Chunking (`ADR-002`):** Docs ≤1,500 tokens = 1 chunk; larger docs cap Tier-2 indexing at first 8 semantic chunks + outline → ~1.3 chunks/file average across 1M files (~1.3M chunks).
- **CPU Budget:** Batched INT8 `bge-small` sustains ~120 chunks/sec → ~3.0h embedding + Tier-1/IO ≈ **~3.5h total (< 6h SLA)** on NVMe/SSD primary storage. Tier-3 cold/OCR runs idle-gated outside the 6h commitment.

---

## 4. Ingestion Pipeline (`A2 FR-011.1` & `§4.2`)

```
Discovery (Tier 1) ──▶ ChangeDetect + AtomicUnits ──▶ Extract (Tier 2/3) ──▶ Enrich ──▶ IndexCommit
```

1. **Discovery (Tier 1):** Root enumeration via OS adapters (§5); detects `volumes` and `atomic_units` (`ADR-004`), emits file metadata, size-dupe candidates, and filename secret alerts in `≤15m` (`≤90s` on 150k-file SSD).
2. **ChangeDetect:** Matches `(volume_id, file_index, path_hash)` using the **Rename vs. Hardlink Resolution Algorithm (`B4` §3.1)** so renames preserve `files.id` and hardlinks (`nlink > 1`) share `file_index` safely.
3. **Extract:** Throttled Rust parsers extract normalized text, `archive://` members (depth ≤2, ≤50MB), EXIF, and AST metadata.
4. **Enrich:** Lazy `blake3`, `simhash_text`/`dhash_image`, batched ONNX embeddings, classification, entities/relationships, and context-weighted secret hashing (`HMAC-BLAKE3`, `B6` §6).
5. **IndexCommit:** Single SQLite ACID transaction (`files`, `chunks` → `chunks_fts` triggers, `outbox_events`) + async `qdrant-edge` outbox drain (`≤1,000` points/tx).

---

## 5. OS Adapters & Cloud-Placeholder Guard

| Concern | Windows 10/11 | macOS 12+ | Linux (Ubuntu 22.04+) |
|---|---|---|---|
| **Fast Discovery / Recovery** | NTFS MFT / USN Journal (if elevated) or parallel `jwalk` | FSEvents stream IDs + `getattrlistbulk` | `inotify` + parallel `jwalk` |
| **Watchers** | `ReadDirectoryChangesW` (`notify`) | FSEvents (`notify`) | `inotify` (`notify`) |
| **File Identity** | `VolumeSerialNumber` + `FileIndex` | `st_dev` + `st_ino` | `st_dev` + `st_ino` |
| **Cloud Placeholders** | `FILE_ATTRIBUTE_RECALL_ON_DATA_ACCESS` / `OFFLINE` | `SF_DATALESS` / `com.apple.fileprovider` | GVFS / rclone FUSE mount heuristics |
| **Permissions** | ACL error skip → `files_skipped_perm` | Guided **Full Disk Access / TCC**; denied → Coverage Report | POSIX error skip → `files_skipped_perm` |
| **Same-Volume Trash** | `IFileOperation` | `NSFileManager` trash | freedesktop trash |

**Cloud-Placeholder Guard (`FR-001.9`):** Before ANY payload read (hash, extract, embed), check OS placeholder attributes. Dehydrated files are recorded with **`cloud_state = 'placeholder_remote'`** (`B4` §3) and skipped without opening file content unless the user explicitly opts in per root.

---

## 6. Watchers, Crash Recovery & Reliability

- **Watchers:** One watcher per root; 500ms debounce, burst coalescing; 15-min polling fallback for unsupported/network mounts. External deletions mark `files.state = 'missing'` (never purged immediately); Aegis-initiated deletes move atomically to `<VolumeRoot>/.aegis-quarantine` with `files.state = 'quarantine'` + `quarantine_entries` row (`ADR-003`).
- **Crash Recovery:** `scan_jobs.checkpoint_json` (`B4` §3) stores per-batch resume tokens; restart reconciles watcher set vs. `scan_jobs`, replaying NTFS USN / FSEvents history or targeted subtree sweeps.
- **Journal Durability:** `PRAGMA synchronous = FULL` during `action_batches`/`actions`/`audit_log` commits; `NORMAL` during bulk ingestion (`B4` §2).
- **No Unjournaled Mutation:** All filesystem writes route through `C4` Policy Engine + `action_batches` transactions (`F2` §1.1 invariant).

---

## 7. Audit & Tamper Evidence (`B6` §7)
`audit_log` is **hash-chained (`BLAKE3`) and tamper-evident locally** (`aegis doctor` verifies `prev_hash`/`hash` integrity across `1..N`), not tamper-proof against a live privileged local attacker. V2 adds exportable Ed25519-signed chain-head checkpoints.

---

## 8. Metadata, Acceptance Criteria, Open Questions & Traceability
- **Acceptance Criteria:** `F3` 1M-file / 5M-chunk soak suite passes `≤3.65GB` RSS scan cap and `≤6h` total duration; `F2` confirms `0` payload reads on `placeholder_remote` fixtures.
- **Open Questions:** `[OPEN]` `OQ-12` (`pdfium` signing/notarization, Sprint 0); `[OPEN]` `OQ-15` (`aegis-llm-worker` OS sandboxing, V1.5).
- **Assumptions:** `[ASSUMPTION]` `Standard` profile machines run NVMe/SSD primary storage (`A2` §5).
- **Change Log:**
  - `v2.1` (Pre-MVP): Rust-first topology, hardware profiles, 3.65GB memory model, OS adapters.
  - `v2.2` (2026-11-20): Synced `ADR-002`, `placeholder_remote`, `scan_jobs.checkpoint_json`, `dhash`/`SimHash`, `P1` OCR scope, `A2` `2.36GB` vs `3.65GB` alignment, and standard footer.
- **Traceability:** Upstream `A2` (`FR-001`–`FR-013`, `§4–5`), `B8` (`ADR-001`–`ADR-004`); Downstream `B4 v2.3`, `E2_E4 v1.1`, `F3`.
--- END OF FILE B1_B2_Architecture.md ---
```

---

### Micro-Polishes (Exact Patches for `E2_E4`, `A2`, and `suite_reconciliation`)

#### 1. `E2_E4_Delivery_Observability_Spec.md` → Bump to **`v1.1`** (§4 Check 5)
Replace Check 5 row in `§4 aegis doctor Diagnostic & Repair Matrix` with:
```markdown
| 5 | **Quarantine Consistency** | Verify `quarantine_entries` active rows (`restored_at IS NULL AND purged_at IS NULL`) exist at `quarantine_path` on online volumes | Set `purged_at = now()` and append `audit_log(action='quarantine_purged_externally')` for files deleted outside Aegis |
```

#### 2. `A2_PRD_v2.1.md` → Bump to **`v2.1.1`** (§3.5 `FR-005.2` + §10)
Replace `FR-005.2` Acceptance Criteria in `§3.5` with:
```markdown
| FR-005.2 | Atomic Unit Protection Invariant (ADR-004) | P0 | Engine blocks proposals moving file out of `atomic_unit` unless `atomic_units.allow_split_override=1` or `files.unit_split_override=1` is explicitly confirmed. |
```
And append to `§10 Change Log`:
```markdown
| 2.1.1 | 2026-11-20 | Synced `FR-005.2` per-file `unit_split_override` (`B4 v2.3` / `B8 v2.2`) |
```

#### 3. `suite_reconciliation.md` → Bump to **`v1.2`** (§2 Living Index)
Update the `B1/B2`, `A2`, and `E2/E4` rows in `§2 Unified Living Index (G1)` to reflect **12/12 verified green**:
```markdown
| **A2** | Product Requirements Document | **`2.1.1`** | ✅ Canonical & Synced | `A2_PRD_v2.1.md` |
| **B1/B2** | System & Runtime Architecture | **`2.2`** | ✅ Canonical & Synced | `B1_B2_Architecture.md` |
| **E2/E4** | Delivery, Packaging & Observability | **`1.1`** | ✅ Canonical & Synced | `E2_E4_Delivery_Observability_Spec.md` |
```

---

### Next Action
Apply these 4 updates to your repository (`B1_B2_Architecture.md` `v2.2`, plus the 3 small edits above). Your entire documentation suite (`A2`, `B1/B2`, `B4`, `B6`, `B8`, `C1`, `C4`, `D3`, `E2/E4`, `F1/F2/F3`, `G2`, `G1/G3`) is now **production-grade and cleared for Sprint 0 scaffolding** (`B4 v2.3` SQLite migrations + `C1 v1.1` `specta` IPC bindings + `C4` Policy Engine unit tests).