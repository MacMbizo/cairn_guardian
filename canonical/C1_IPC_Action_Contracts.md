# C1 — IPC, Events & Action Contract (v1.2)

**ID:** C1 · **Version:** 1.2 · **Status:** Authoritative for MVP FE/BE Split
**Owner:** Staff Backend & Desktop Lead · **Last Updated:** 2026-11-20
**Related Docs:** `A2_PRD_v2.1.md` (v2.1.2), `B4_Data_Vector_Schema_v2.3.md` (v2.3.1), `C4_Policy_Engine_Confidence_Spec.md` (v1.1)

> **v1.2 changes:** Added missing MVP commands (`roots.*`, `ignore.*`, `scan.coverage_report`, `file.set_lifecycle`, and `quarantine.space_summary` from promoted `V1.5-01`); clarified Local REST/GraphQL API is `V1.5` (`V1.5-04`) and `findings.action` `'vaulted'` is `V2 reserved`; updated `actions.resolve_conflict` + §4 Rollback Conflict Resolution to enforce pre-flight across the entire batch before disk mutation and remove fictional pre-edit content backups on `modified` conflicts; documented `target_kind` (`file|dir|atomic_unit`) support.

---

## 1. Transport
- **Commands:** Tauri `invoke()` typed via `ts-rs` / `specta` Rust→TS codegen.
- **Events:** `tauri::event` versioned channels (`v: 1`).
- **Local API (V1.5 — `V1.5-04`):** REST/GraphQL on `127.0.0.1:4777`, bearer token in OS keychain, loopback-only enforced.

## 2. Command Surface (~60 Commands)

```ts
// Scan, Roots, Ignore, Volumes & Index
roots.list(): RootConfig[]
roots.add(path: string): RootConfig
roots.remove(path: string): void
roots.toggle(path: string, enabled: boolean): void
ignore.get(root?: string): string
ignore.set(content: string, root?: string): void
scan.start(roots?: string[], tiers?: (1|2|3)[]): JobId
scan.pause(job: JobId): void
scan.resume(job: JobId): void
scan.cancel(job: JobId): void
scan.status(job?: JobId): ScanStatus            // incl. coverage_pct, tier progress, skipped counts
scan.coverage_report(job?: JobId): CoverageReport // clickable denied/placeholder/locked paths (D3 §2)
volumes.list(): VolumeInfo[]
index.rebuild(root?: string): JobId
index.stats(): IndexStats                       // files, chunks, vectors, coverage

// Search (B6 §2: quarantined items excluded by default; SQLite authoritative post-filter)
search.query(
  q: string,
  filters?: SearchFilters & { include_quarantined?: boolean },
  limit?: number
): SearchPage                                   // returns match_evidence + highlighted chunks
search.suggest(prefix: string): string[]

// Files, Lifecycle, Atomic Units & Graph
file.get(id: FileId): FileDetail                // chunks preview, atomic_unit, graph neighborhood
file.set_lifecycle(id: FileId, state: 'active'|'warm'|'cold'|'archive'): void // FR-009
project.list(): Project[]
project.get(id: ProjectId): ProjectDetail
atomic_unit.list(volumeId?: string): AtomicUnit[]
atomic_unit.override(
  target: { unitId: string } | { fileId: FileId },
  allowSplit: boolean
): void                                         // journaled in audit_log
graph.neighborhood(entityId: string, depth?: number): Subgraph
collection.list(): SmartQuery[]                 // V1.5
collection.create(def: SmartQuery): SmartQuery  // V1.5

// Duplicates
dedupe.report(groupBy?: 'exact'|'near'|'semantic'|'version'): DupeReport
dedupe.propose(keepers: Record<string, FileId>): BatchId

// Actions, Simulator & Quarantine
actions.propose(plan: RefilePlan): BatchId      // supports target_kind: 'file'|'dir'|'atomic_unit'
actions.simulate(batch: BatchId): Simulation    // dry-run tree diff + policy verdicts (C4)
actions.approve(batch: BatchId, selection?: ActionId[]): void
actions.execute(batch: BatchId): ExecuteResult  // runs approved actions through policy gate
actions.reject(batch: BatchId, reason?: string): void
actions.rollback(batch: BatchId): RollbackResult // pre-flights ALL actions in batch before FS mutation
actions.resolve_conflict(
  actionId: ActionId,
  choice: 'revert_keeping_edits'|'copy_back_keeping_both'|'restore_as_rename'|'skip'|'abort_batch'
): RollbackResult
actions.list(status?: ActionStatus): Action[]
undo.last(n?: number): UndoResult

quarantine.list(volumeId?: string): QuarantineEntry[]
quarantine.space_summary(volumeId?: string): QuarantineSpaceSummary // Staged vs. Freed + 10% quota
quarantine.restore(entryIds: string[]): BatchId
quarantine.purge(entryIds: string[], confirmedPermanent: boolean): BatchId

// Health, Findings & Playbooks
health.get(): HealthBreakdown                   // 5 dims + top 10 offending items per dim
playbook.disk_rescue(): DiskRescueReport        // FR-011.2(a) incl. cache_dirs
findings.scan(path: string): JobId              // FR-011.2(b) Pre-Share Secret Audit
findings.list(filter?: FindingFilter): Finding[]
findings.action(
  id: FindingId,
  verdict: 'false_positive'|'resolved'|'accepted_risk'|'vaulted' // 'vaulted' = V2 reserved (disabled in MVP UI)
): void

// Rules, Audit & Settings
rules.list(): Rule[]
rules.create(def: RuleDef): Rule
rules.update(id: RuleId, def: RuleDef): Rule
rules.delete(id: RuleId): void
rules.graduate(actionId: ActionId): Rule        // FR-005.3 "Teach Aegis once"
audit.list(limit?: number, offset?: number): AuditEntry[]
audit.export(destPath: string): void

settings.get(scope?: string): SettingsMap
settings.set(key: string, value: unknown, scope?: string): void
onboarding.state(): OnboardingState
onboarding.complete(config: OnboardingConfig): void
models.status(): ModelStatus[]
models.download(tier: 'eco'|'standard'|'pro'): JobId
hw.profile(): 'eco'|'standard'|'pro'
doctor.run(): DoctorReport
```

## 3. `Action` Lifecycle State Machine (Synced to `B4 v2.3`)
- `action_batches.status`: `draft | approved | executing | executed | partial | rolled_back | failed`.
- `actions.status`: `proposed | approved | executed | failed | skipped | rolled_back | rejected | superseded | expired | unrecoverable_missing`.

```
proposed ──approve──▶ approved ──execute──▶ executed ──rollback──▶ rolled_back
   │                    │                      │
   ├──reject──▶ rejected┤                      ├──conflict──▶ rollback_conflict set;
   │                    └──expire──▶ expired   │   user resolves (restore_as_rename |
   │                                           │   skip | abort_batch)
   └──superseded──▶ superseded                 └──missing──▶ unrecoverable_missing
```

**Invariants:**
- `pre_blake3` + `pre_state_json` captured **before** mutation; `post_*` captured **after**.
- Partial unique index `idx_actions_file_status` (`WHERE file_id IS NOT NULL AND status IN ('proposed','approved')`) enforces at most 1 live (`proposed | approved`) action per `file_id` (while `target_kind = 'dir' | 'atomic_unit'` supports directory/unit-level operations without exploding into per-file rows).
- Deletes create `quarantine_entries` rows (`B4`); physical deletion never occurs outside `quarantine.purge(..., confirmedPermanent=true)` or TTL expiry.

## 4. Rollback Conflict Resolution (`FR-005.4`)
**Pre-flight Batch Invariant:** `actions.rollback(batch)` **pre-flights all `executed` actions in the batch before mutating any file on disk**. If any action fails verification (`exists(to_path) && blake3(to_path) == post_blake3 && !exists(from_path)`, plus metadata vs `post_state_json` for metadata ops), the batch pauses cleanly and returns all detected `rollback_conflict` entries for user resolution (`actions.resolve_conflict`) rather than leaving a half-reverted batch on disk.

| Condition | `rollback_conflict` | Resolution Options |
|---|---|---|
| All checks pass | `NULL` | Reverse op executed & journaled; `status = 'rolled_back'` |
| `to_path` modified after execution | `'modified'` | `revert_keeping_edits` (move edited `to_path` back to `from_path`), `copy_back_keeping_both` (copy edited `to_path` to `from_path` and keep `to_path`), `skip`, or `abort_batch` *(Aegis stores `pre_blake3`/metadata only — never claims a fictional pre-edit content backup)* |
| `from_path` reoccupied | `'collision'` | `restore_as_rename` (restore `to_path` under `<from_path>.restored-<ts>`), `skip`, or `abort_batch` |
| `to_path` missing on disk | `'missing_target'` | `skip` (status → `unrecoverable_missing`, honest error — never falsely marked `rolled_back`) or `abort_batch` |

## 5. Events (`v: 1`) & Error Taxonomy
- **Events:** `ScanProgress`, `IndexUpdated`, `ActionProposed`, `FindingCreated`, `HealthUpdated`.
- **Error Taxonomy (`scan_jobs.errors_json` & IPC errors):**
  `permission_denied` · `cloud_placeholder` · `unsupported_fs` · `locked_file` · `path_too_long` · `parse_failed:<format>` · `watcher_lost` · `volume_offline` · `quarantine_unavailable` · `policy_blocked:<rule>`.
  Every IPC error returns `{ code: string, message: string, retryable: boolean, remediation?: string }`.

## 6. Metadata, Open Questions, Assumptions & Traceability
- **Acceptance:** `ts-rs`/`specta` generates valid TypeScript bindings with 0 hand-maintained duplicates; all commands map 1:1 to `B4 v2.3.1` (`C1_B4_Alignment_Check.md`).
- **Assumptions:** Single local desktop user session per SQLite file lock.
- **Traceability:** Upstream `A2` (`v2.1.2`), `B4` (`v2.3.1`), `B6` (`v2.4`), `C4` (`v1.1`); Downstream Tauri Rust commands & React hooks.
