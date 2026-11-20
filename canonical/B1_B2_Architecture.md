# B1/B2 — System & Runtime Architecture (v2.3)

**ID:** B1/B2 · **Version:** 2.3 · **Status:** Approved MVP Baseline
**Owner:** Principal Systems Architect · **Last Updated:** 2026-11-20
**Related Docs:** `A2_PRD_v2.1.md` (v2.1.2), `B4_Data_Vector_Schema_v2.3.md` (v2.3.1), `B6_Threat_Model.md` (v2.4), `B8_ADRs.md` (v2.3), `C1_IPC_Action_Contracts.md` (v1.2), `C4_Policy_Engine_Confidence_Spec.md` (v1.1), `E2_E4_Delivery_Observability_Spec.md` (v1.2), `F1_F2_Eval_Safety_Plan.md` (v2.3)

> **v2.3 changes:** Split Target Interactive Burst (`≤4.10GB`) vs. Conservative Ceiling Interactive Burst (`≤5.30GB`) in §3 so 1.8GB LLM burst + 1.5GB `qdrant-edge` cap stays mathematically consistent; noted Sprint-0 `ort` CPU embedding throughput microbenchmark and optional 384-dim multilingual model fallback; documented MIH/LSH banding + exact shingle Jaccard verification for near-dup clustering (`FR-004.1`) and `cache_dirs` indexing (`B4 v2.3.1`).
>
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
| Near-Dup Text | Rust `MinHash` / `SimHash` (64/128-bit) + MIH/LSH bands + exact shingle Jaccard `≥0.85` check | Writes `files.simhash_text` (`B4` §3–§4); avoids `O(N^2)` scans |
| OCR (Opt-in `FR-002.3`) | **`P1 / V1.5+`** (or zero-bundle OS-native `Windows.Media.Ocr` / macOS Vision API only; **no bundled Tesseract in MVP**) | Explicitly opt-in with CPU/battery disclosure |

---

## 2. Hardware Profiles (`ADR-002`)

Detected at onboarding (`hw.profile()`), user-overridable in Settings; drives model downloads, memory caps, and feature gates.

| Profile | Detect Rule | RAM Floor | Embeddings | LLM | Reranker | Scan RSS Cap |
|---|---|---|---|---|---|---|
| **Eco** | `<12GB` RAM or no AVX2 | 8GB | `bge-small-en-v1.5` (384-dim INT8 ONNX, ~90MB; optional 384-dim multilingual fallback for non-EN) | **None — deterministic/regex query parser only** | No | ≤2.5GB |
| **Standard** | `12–24GB`, CPU-only | 16GB | `bge-small` (384-dim) default (optional 384-dim multilingual fallback); `bge-m3` (1024-dim) optional | On-demand 1.5–3B Q4 (~1.5–1.8GB), **evict @ 300s idle** | No | ≤4.0GB |
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
| **TOTAL SCAN RSS** | **≤2.36GB** | **≤3.65GB (passes ≤4GB SLA)** | Scan RSS only (LLM unloaded) |
| **INTERACTIVE BURST (w/ LLM)** | **≤4.10GB** | **≤5.30GB** | When 1.8GB `aegis-llm-worker` runs concurrently over `qdrant-edge` target (700MB) vs. conservative ceiling (1,500MB); `Standard` pauses background embed workers during interactive LLM burst |

**Throughput Model (`ADR-002` & `A2` §4.2):**
- **Selective Chunking (`ADR-002`):** Docs ≤1,500 tokens = 1 chunk; larger docs cap Tier-2 indexing at first 8 semantic chunks + outline → ~1.3 chunks/file average across 1M files (~1.3M chunks).
- **CPU Budget:** Batched INT8 `bge-small` target ~120 chunks/sec → ~3.0h embedding + Tier-1/IO ≈ **~3.5h total (< 6h SLA)** on NVMe/SSD primary storage; Sprint 0 runs an `ort` INT8 CPU microbenchmark on real chunks to validate worker count and SLA headroom before Sprint 1. Tier-3 cold/OCR runs idle-gated outside the 6h commitment.
- **Near-Dup Scaling:** `simhash_text` / `dhash_image` clustering uses MIH/LSH band tables (`B4` §4) plus exact token-shingle Jaccard `≥0.85` verification instead of `O(N^2)` pairwise SQLite `BLOB` scans.

---

## 4. Ingestion Pipeline (`A2 FR-011.1` & `§4.2`)

```
Discovery (Tier 1) ──▶ ChangeDetect + AtomicUnits/CacheDirs ──▶ Extract (Tier 2/3) ──▶ Enrich ──▶ IndexCommit
```

1. **Discovery (Tier 1):** Root enumeration via OS adapters (§5); detects `volumes`, `atomic_units` (`ADR-004`), and `cache_dirs` (`node_modules`, `target/`, `.venv/`, `.gradle/` — recording one summary row per cache dir with 0 interior `files` rows), and emits file metadata, size-dupe candidates, and filename secret alerts in `≤15m` (`≤90s` on 150k-file SSD).
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
  - `v2.3` (2026-11-20): Split Target vs. Conservative Interactive Burst (`≤4.10GB` / `≤5.30GB`), added Sprint-0 `ort` CPU microbenchmark + multilingual 384-dim fallback note, MIH/LSH near-dup banding + shingle verification, and `cache_dirs` discovery alignment (`B4 v2.3.1`).
- **Traceability:** Upstream `A2` (`v2.1.2`, `FR-001`–`FR-013`, `§4–5`), `B8` (`v2.3`, `ADR-001`–`ADR-004`); Downstream `B4 v2.3.1`, `E2_E4 v1.2`, `F3`.
