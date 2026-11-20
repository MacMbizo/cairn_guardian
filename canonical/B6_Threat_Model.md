# B6 — Security Threat Model (v2.4)

**ID:** B6 · **Version:** 2.4 · **Status:** Baseline for MVP Security Review
**Owner:** Security Engineering Lead · **Last Updated:** 2026-11-20
**Related Docs:** `A2_PRD_v2.1.md` (v2.1.2), `B4_Data_Vector_Schema_v2.3.md` (v2.3.1), `C1_IPC_Action_Contracts.md` (v1.2), `C4_Policy_Engine_Confidence_Spec.md` (v1.1), `F1_F2_Eval_Safety_Plan.md` (v2.3)

> **v2.4 changes:** Scoped loopback HTTP API (`127.0.0.1:4777`) auth gate explicitly to `V1.5` (`V1.5-04`) instead of MVP release gate; elevated SQLite `files.state != 'quarantine'` post-filter as authoritative search-leak enforcement against async Qdrant lag (`B4 v2.3.1`).
> **v2.3 changes:** Added Model Artifact & GGUF/ONNX parser supply-chain row to STRIDE (§2); added `<VolumeRoot>/.aegis-quarantine/` to default indexing exclusions (§6.4); wired default quarantine/secret search exclusion to `C1 v1.1` `search.query`; added standard metadata/traceability sections.

---

## 1. Scope & Trust Boundaries
Protects user files, metadata, `aegis.db`, `qdrant-edge` vectors, `audit_log`, quarantine, models, API bearer token, keychain entries, and zero-egress privacy.
- **TB-1:** Filesystem (untrusted content) · **TB-2:** Rust/SQLite/Qdrant core · **TB-3:** LLM/Agent boundary · **TB-4:** Loopback API · **TB-5:** Opt-in cloud egress · **TB-6:** Plugin WASM runtime (V2).

## 2. STRIDE Analysis
| Threat | Component | Vector | Mitigation | Residual |
|---|---|---|---|---|
| **Spoofing** | Local API (TB-4, **V1.5**) | Local process calls API | Loopback bind (`127.0.0.1`), rotating per-install bearer token in OS keychain (`V1.5-04`) | Low |
| **Tampering** | `aegis.db`, journal | Malware/corruption | Hash-chained `audit_log`, `pre/post_state_json`, `aegis doctor`, WAL checksums, signed releases | Medium (tamper-evident; V2 signed checkpoints) |
| **Tampering** | Filesystem ops | Automation bug | Journaled `action_batches` only, `C4` policy gate, dry-run, per-volume quarantine | Low |
| **Tampering / RCE** | **Model Artifacts & Parsers (TB-2/TB-3)** | **Tampered ONNX/GGUF download or malformed tensor/metadata exploit** | **HTTPS-only downloads, pinned SHA-256 checksums against signed release manifest, isolated `llama.cpp` subprocess with memory cap (`ADR-002`)** | **Low–Medium** |
| **Repudiation** | Actions/audit | Denied mutation | Hash-chained `audit_log` + `origin` + `actor` + exportable log | Low |
| **Info Disclosure** | Index at rest | Device theft | Optional SQLCipher (V1.5); default relies on OS FDE — documented honestly | Medium until V1.5 |
| **Info Disclosure** | Secrets stored | Aegis leaks secrets | Zero-storage: `HMAC-BLAKE3` fingerprint + masked snippet (`AKIA...9X2Q`) only | Low |
| **Info Disclosure** | Search results | Leaking quarantined/secret files | `C1` `search.query` enforces authoritative SQLite post-filter `files.state != 'quarantine'` + `sensitivity` scoping (Qdrant filter is pre-filter optimization only, preventing async outbox lag leaks) | Low |
| **DoS** | Scanner/Extractors | Zip bombs, FIFO/device files, deep trees | Bounded pool, archive depth ≤2 / ≤50MB caps, device-file skip, token-bucket throttle | Low |
| **Elevation** | Plugins (V2) | Malicious plugin | WASM sandbox, manifest permissions, proposals-only, signed marketplace | V2 |

## 3. Prompt-Injection Surface (TB-1 → TB-3)
File contents are untrusted data: (1) delimited data blocks in prompts, (2) agents emit `proposed` actions only — never direct FS/shell execution, (3) schema-validated outputs, (4) canonicalized path checks against roots & `atomic_units`, (5) `F1` injection canaries, (6) `Housekeeper` ceiling = propose-only.

## 4. Cloud-Egress Paths (TB-5)
All content/index egress **OFF by default**: opt-in Cloud LLM passes secret/PII redaction filter + logs token count; model downloads check pinned SHA-256 over HTTPS; opt-in telemetry sends counts/timings only. Verified offline in `F2`.

## 5. Threat Model Boundaries
Out of scope: attacker with live authenticated user-session privileges or compromised OS kernel/drivers.

## 6. Secrets-Handling Invariants
1. Raw secrets never stored in SQLite, logs, vectors, or crash dumps.
2. `security_findings` uses `HMAC-BLAKE3(keychain_salt, secret)` + masked snippet.
3. `.aegis` internal dir **and** all `<VolumeRoot>/.aegis-quarantine/` directories are excluded from indexing via immutable built-in ignore rules.

## 7. Audit Threat Model
`audit_log` is **tamper-evident** locally (`aegis doctor` verifies `prev_hash`/`hash` chain); V2 adds Ed25519-signed chain checkpoints. UI/docs must never claim "tamper-proof."

## 8. Security Release Gates & Traceability
- **MVP Gates:** `F2` injection suite (0 actions), offline egress test (0 external packets), secrets grep test (0 raw secrets), `aegis doctor` detects 100% seeded chain tamper, `search.query` never leaks quarantined items even under outbox lag.
- **V1.5 Gates (`V1.5-04`):** Loopback API (`127.0.0.1:4777`) rejects non-loopback binds and missing/invalid bearer tokens (`401/403`).
- **Open:** `[OPEN]` `OQ-15` (Owner: Security Lead, V1.5): Decide threat posture for `llama.cpp` subprocess OS sandboxing (AppContainer / Seatbelt / seccomp).
- **Assumptions:** Host OS full-disk encryption (BitLocker/FileVault/LUKS) is enabled by user prior to V1.5 SQLCipher shipping.
- **Traceability:** Upstream `A2` `G4–G5`, `B8`; Downstream `B4 v2.3.1`, `C1 v1.2`, `C4 v1.1`, `F2 v2.3`.
