# G2 — Canonical Glossary (v1.0)

**ID:** G2 · **Version:** 1.0 · **Status:** Active Reference
**Owner:** Principal Documentation Architect · **Last Updated:** 2026-11-20

| Term | Canonical Definition |
|---|---|
| **Action Batch** | One atomic Simulator plan or undo unit (`action_batches` row) grouping 1+ `actions`. |
| **Action Proposal** | Evidence-backed, previewable request (`actions` row) to mutate filesystem/index state — the sole permitted mutation path. |
| **Atomic Project Unit** | An indivisible directory (`atomic_units` row) marked by `.git`, package manifests, `.app`, or `.aegisboundary` whose interior files cannot be individually refiled outward without explicit override (`ADR-004`). |
| **Autonomy Dial** | Per-action-class setting (`Manual | Suggest | Auto-approved | Auto`) controlling execution gates (`C4`). |
| **Cloud Placeholder** | Remote-only synced file (`SF_DATALESS` / `RECALL_ON_DATA_ACCESS`) recorded without hydrating/downloading payloads (`FR-001.9`). |
| **Coverage Honesty** | Guarantee (`G6`) that skipped, permission-denied, locked, and placeholder files are counted and surfaced explicitly rather than faked as 100%. |
| **Digital Twin** | The queryable semantic model (`files` + `chunks` + `entities` + `relationships` + vector shards) of the user's computing environment. |
| **Graduated Rule** | Deterministic rule (`rules.source = 'graduated_proposal'`) created from a user-approved Action Proposal via "Teach Aegis once" (`FR-005.3`). |
| **Hardlink Alias** | Multiple directory entries sharing the same `(volume_id, file_index)` with `nlink > 1`; reports `0` reclaimable bytes unless all-but-keeper links are removed (`ADR-003`). |
| **Journal / Audit Log** | Append-only, hash-chained `audit_log` enabling tamper-evident verification and rollback traceability. |
| **Per-Volume Quarantine** | Hidden `<VolumeRoot>/.aegis-quarantine/` directory enabling `O(1)` atomic reversible deletes with 30-day TTL (`ADR-003`). |
| **Pre-Rollback Reconciliation** | Pre-undo verification of `to_path` hash/state and `from_path` availability (`FR-005.4`, `C1` §4). |
| **Tiered Progressive Indexing** | 3-stage scan (`Tier 1` metadata → `Tier 2` active roots → `Tier 3` cold background) delivering Day-1 value in ≤2m (`ADR-002`). |
