# B8 — Architecture Decision Records (v2.3)

**ID:** B8 · **Version:** 2.3 · **Status:** Approved MVP Baseline
**Owner:** Principal Systems Architect · **Last Updated:** 2026-11-20
**Related Docs:** `A2_PRD_v2.1.md` (v2.1.2), `B4_Data_Vector_Schema_v2.3.md` (v2.3.1), `C4_Policy_Engine_Confidence_Spec.md` (v1.1)

> **v2.3 changes:** Added `target_kind ('file'|'dir'|'atomic_unit')` + `cache_dirs` support to `ADR-003`/`ADR-004` (`safe_build_cache` carve-out in `C4 v1.1`); updated `file_index` `BLOB` & `has_stable_inode` note in `ADR-003`; removed dangling `A2 P1–P7` reference.
> **v2.2 changes:** Reordered `ADR-001 → ADR-002 → ADR-003 → ADR-004`; updated `quarantine_items` → `quarantine_entries`; added hardlink (`nlink > 1`) accounting to `ADR-003`; standardized `.aegisboundary` marker name in `ADR-004`; added `files.unit_split_override` traceability.

---

## ADR-001 — Rust-First Extraction; Python Deferred to V1.5+
- **Status:** Accepted (MVP)
- **Context:** PRD v2.0 proposed an always-on Python gRPC sidecar. Bundling CPython + ML wheels + Poppler + Tesseract yields 1.5–3GB installers, ~1GB extra RSS, AV false positives, and IPC serialization overhead across 1M+ files.
- **Decision:** MVP hot path runs 100% in a single Rust binary: `ort`/`fastembed`, `tree-sitter`, `calamine`/`quick-xml`/`kamadak-exif`, `pdfium-render`, `blake3`, `notify`, `jwalk`, and `qdrant-edge`. Only `llama.cpp` runs on-demand for `Standard`/`Pro` LLM features. Python (or WASM plugins) returns in V1.5+ as an optional lazy-downloaded runtime for Whisper, heavy OCR, `.pst`, and VLM captioning.
- **Consequences:** Single native binary, ~50% lower RSS, simpler CI; weaker MVP extraction on scanned PDFs (handled cleanly via `partial` coverage reporting until V1.5).

## ADR-002 — Hardware-Tiered Inference, Idle Eviction & 3-Tier Progressive Indexing
- **Status:** Accepted (MVP)
- **Context:** v2.0's BGE-M3 + NER + 4-bit 8B LLM stack exceeds the ≤4GB RSS scan cap, and 50 emb/sec × 5–10M chunks = 28–56h on CPU.
- **Decision:**
  1. Three profiles (`Eco` <12GB RAM, `Standard` 12–24GB, `Pro` ≥24GB or ≥8GB VRAM GPU).
  2. **LLM never resident during bulk scans:** loaded on-demand for interactive query/proposal bursts, hard-evicted after 300s idle; disabled on `Eco`.
  3. **3-Tier Progressive Indexing:** Tier 1 metadata (≤15m, ≤90s on 150k SSD) → Tier 2 active roots (≤40m) → Tier 3 background throttled (≤2.5h).
  4. **Selective Chunking:** ≤1,500-token docs = 1 chunk (~1.3 chunks/file avg across 1M files); INT8 ONNX + Qdrant scalar quantization everywhere.
- **Consequences:** Memory closes ≤2.36GB target (≤3.65GB conservative ceiling) during scan; 1M-file CPU scan closes ~3.5h < 6h SLA; dashboard delivers value in ≤2m.

## ADR-003 — Per-Volume Quarantine, OS Trash Delegation & Hardlink-Safe Accounting
- **Status:** Accepted (MVP)
- **Context:** A single `.aegis/quarantine` forces slow cross-volume copies and risks filling the boot drive. Separately, NTFS/APFS/ext4 hardlinks (`nlink > 1`) share storage blocks; counting both as duplicates overstates reclaimable space.
- **Decision:**
  1. Quarantine lives at `<VolumeRoot>/.aegis-quarantine/<batch_id>/` (hidden/system attr, auto-excluded from indexing) so deletes are `O(1)` atomic same-volume renames for files, build-cache directories (`cache_dirs`), and whole `atomic_units` (`target_kind = 'file' | 'dir' | 'atomic_unit'`). OS Trash delegation allowed per user setting when same-volume semantics hold.
  2. If volume root is non-writable or unmounted: action fails cleanly (`failed:quarantine_unavailable`); user is offered an explicit journaled permanent-delete confirmation or copy-to-primary fallback *only* if `size < 25%` of destination free space — never silent permanent delete.
  3. Entries tracked in `B4.quarantine_entries` (`expires_at = +30d` TTL); expiry purge checks volume free-space guard.
  4. **Hardlinks (`nlink > 1`):** `B4.files` tracks `(volume_id, file_index, path_hash)` (`BLOB` 64/128-bit `file_index`, with `volumes.has_stable_inode = 0` fallback for `exFAT`/`SMB`) and `nlink`. Shared-inode hardlink groups report **0 reclaimable bytes** unless all-but-keeper links are removed.
- **Consequences:** Instant reversible deletes across multi-TB drives; 100% honest disk-reclaim reporting (`G6`).

## ADR-004 — Atomic Project Units (Indivisible Directories)
- **Status:** Accepted (MVP)
- **Context:** File-level semantic refiling can propose moving `README.md` or `data.csv` out of a Git repo or multi-file project, silently breaking builds.
- **Decision:** A directory is registered in `B4.atomic_units` if it contains any canonical boundary marker:
  `.git` · `package.json` · `pyproject.toml` · `Cargo.toml` · `go.mod` · `pom.xml` · `build.gradle` · `.xcodeproj` · `*.sln`/`*.csproj` · `.app`/`.photoslibrary` bundles · SQLite `-wal`/`-shm` companion triplets · LaTeX (`\documentclass` + `.bib`) · Premiere `.prproj` bundles · user-defined **`.aegisboundary`** marker file.
  - Proposals may move/archive/quarantine the *entire* unit root (`target_kind = 'atomic_unit'`), never pluck interior files outward.
  - Interior files remain fully indexed and searchable.
  - **`safe_build_cache` carve-out (`C4 v1.1`):** Registered build-cache directories (`cache_dirs`: `node_modules`, `target/`, `.venv/`, `.gradle/`) inside an atomic unit may be proposed for `Quarantine` in `Suggest` mode (never `Auto`) for `FR-011.2` Disk Rescue without toggling `allow_split_override`.
  - Per-file override (`files.unit_split_override = 1`) or unit override (`atomic_units.allow_split_override = 1`) must be explicitly user-confirmed and journaled. Innermost marker wins for nested units.
- **Consequences:** Eliminates destructive repo splitting with deterministic UI explainability.

## Metadata & Traceability
- **Acceptance:** Referenced by `A2`, `B4`, `C1`, `C4`; zero unresolved conflicts.
- **Open:** `OQ-12` (`pdfium` signing/notarization).
- **Traceability:** Upstream `A2` `G1–G6`, `FR-001`–`FR-013`; Downstream `B4 v2.3.1`, `C1 v1.2`, `C4 v1.1`, `F2 v2.3`.
