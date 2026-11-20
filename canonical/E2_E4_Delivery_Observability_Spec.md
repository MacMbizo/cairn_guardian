# E2/E4 — Delivery, Packaging & Observability Specification (v1.2)

**ID:** E2/E4 · **Version:** 1.2 · **Status:** MVP Build Baseline
**Owner:** Release & Platform Engineering Lead · **Last Updated:** 2026-11-20
**Related Docs:** `A2_PRD_v2.1.md` (v2.1.2), `B4_Data_Vector_Schema_v2.3.md` (v2.3.1), `B6_Threat_Model.md` (v2.4), `B8_ADRs.md` (v2.3, `ADR-001`, `ADR-002`)

> **v1.2 changes:** Synchronized H1 title version (`v1.0` → `v1.2`) with metadata; added Check #8 (`Orphan Graph Edges & Stale cache_dirs`) to `aegis doctor`.

---

## 1. Purpose & Scope
Defines the Tauri build/bundling pipeline for native Rust dependencies (`pdfium`, `ort`, `llama.cpp`), release signing & model verification, structured JSON logging (with zero-secret redaction), and the full `aegis doctor` integrity check matrix.

**Audience:** Release/DevOps engineers, Rust core engineers, Support/QA.

## 2. Native Bundling, Signing & Model Distribution (`E2`)
| Artifact | Packaging Strategy | Verification & Security |
|---|---|---|
| **Tauri Binary + Rust Core** | Static link `sqlite` (bundled), `tree-sitter`, `qdrant-edge`; bundle `pdfium` shared lib + `ort` dynamic provider (`DirectML` Win / `CoreML` macOS / CPU Linux) inside app resources. | Windows EV/Trusted Signing (SmartScreen); macOS Developer ID + Hardened Runtime + Notarization (`xcrun notarytool`); Linux AppImage + `.deb`. |
| **`llama.cpp` Worker** | Bundled as separate signed helper binary (`aegis-llm-worker`) spawned on-demand by Rust core (`ADR-002`) with OS memory/CPU caps. | Isolates GGUF parser crash/memory risk (`B6` §2); killed cleanly on 300s idle or app exit. |
| **ONNX & GGUF Models** | Downloaded post-install per hardware profile (`Eco`/`Standard`/`Pro`) into `<app_data>/models/`. | HTTPS-only against Ed25519-signed `models-manifest.json`; verify exact `SHA-256` before loading into `ort` or `llama.cpp`. |
| **Auto-Updater** | Tauri v2 updater with `stable` and `beta` channels + Ed25519 signature verification. | Staged rollout (5% → 25% → 100%); previous installer cached for 1-click downgrade. |

## 3. Observability & Structured Logging (`E4`)
- **Log Location:** `<app_data>/logs/aegis.jsonl` (daily rotation, max 7 files × 20MB).
- **Schema:** `{"ts":"2026-11-20T13:45:00Z","level":"INFO","component":"scanner","job_id":"...","event":"tier1_complete","duration_ms":4210,"files_seen":142000}`.
- **Zero-Secret & Privacy Redaction Invariant (`B6` §6):** Logger middleware **must never** emit raw file payloads or matched secret substrings; paths are omitted from any opt-in diagnostic bundle unless user checks *"Include sanitized local paths"*.
- **Local Analytics Metrics (SQLite/JSONL, always on-device):** Scan tier durations, RSS high-water mark, ONNX chunks/sec, search p50/p95 latency, outbox queue depth, approval/rollback counts.

## 4. `aegis doctor` Diagnostic & Repair Matrix (`FR-014` / `B4` / `B6`)
Running `doctor.run()` (UI Settings or CLI) executes 8 deterministic checks:

| # | Check | Verification Logic | Auto-Repair (`--fix` / UI button) |
|---|---|---|---|
| 1 | **Audit Chain Integrity** | Recompute `BLAKE3` (length-prefixed canonical framing) over `audit_log` rows `1..N`; verify `prev_hash` links | **No auto-rewrite** (read-only alert + pinpoint first corrupt `seq`) |
| 2 | **SQLite Integrity** | `PRAGMA quick_check` + foreign-key check (`PRAGMA foreign_key_check`) | Vacuum / WAL checkpoint; flag manual restore if corrupt |
| 3 | **FTS5 Sync Drift** | Compare `COUNT(chunks)` vs `COUNT(chunks_fts)` | Rebuild FTS5 index (`INSERT INTO chunks_fts(chunks_fts) VALUES('rebuild')`) |
| 4 | **Outbox / Qdrant Lag** | Count `outbox_events WHERE processed_at IS NULL` + compare Qdrant point count | Replay pending `outbox_events` batches; prune orphan Qdrant points |
| 5 | **Quarantine Consistency** | Verify `quarantine_entries` active rows (`restored_at IS NULL AND purged_at IS NULL`) exist at `quarantine_path` on online volumes | Set `purged_at = now()` and append `audit_log(action='quarantine_purged_externally')` for files deleted outside Aegis |
| 6 | **Model Checksums** | Verify `SHA-256` of downloaded `.onnx`/`.gguf` files against manifest | Quarantine/delete corrupt model file + prompt re-download |
| 7 | **Watcher Health** | Check active `notify` / FSEvents / USN handles per online root | Restart dead watchers + trigger delta reconciliation sweep |
| 8 | **Orphan Graph Edges & Stale `cache_dirs`** | Detect `relationships` rows whose `src_id`/`dst_id` points to non-existent/tombstoned records (with `valid_to IS NULL`) and `cache_dirs` rows on online volumes missing on disk | Stamp `valid_to = now()` on orphaned relationships; mark missing `cache_dirs` as `state = 'missing'` |

## 5. Metadata, Open Questions, Assumptions & Traceability
- **Acceptance:** CI produces signed/notarized test builds on Win/macOS/Linux; `aegis doctor` catches 100% of seeded FTS drift, outbox lag, orphan relationships, and tampered audit rows in `F2`.
- **Open:** `[OPEN]` `OQ-12` (Owner: Platform Lead, Sprint 0): Confirm Apple Developer ID + `pdfium` dylib `@rpath` notarization test pass.
- **Traceability:** Upstream `A2` (`v2.1.2` §4–5), `B4 v2.3.1`, `B6 v2.4`, `B8 v2.3`; Downstream CI/CD workflows & `doctor.run()`.
