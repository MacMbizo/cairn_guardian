# C1 ↔ B4 Alignment Verification (v1.2)

**Date:** 2026-11-20 · **Result:** FULLY ALIGNED (`C1 v1.2` ↔ `B4 v2.3.1`)

| C1 v1.2 Command Group | B4 v2.3.1 Table(s) & Columns | Status |
|---|---|---|
| `roots.*`, `ignore.*` | `settings` (`scope='root:<id>\|global'`), `volumes` (`has_stable_inode`, `case_sensitive`) | ✓ Added in `C1 v1.2` |
| `scan.*`, `scan.coverage_report`, `volumes.list` | `scan_jobs` (`files_seen`, `files_skipped_perm`, `files_skipped_cloud`, `errors_json`), `volumes` | ✓ Updated in `C1 v1.2` |
| `index.*` | `files`, `chunks`, `outbox_events`, `settings` | ✓ Exact |
| `search.query/suggest` | `chunks_fts`, `qdrant:chunks`, `files.state` (authoritative post-filter), `files.sensitivity` | ✓ Updated in `B4 v2.3.1` |
| `file.get`, `file.set_lifecycle`, `project.*`, `graph.*` | `files` (`state`), `projects`, `entities`, `relationships`, `atomic_units` | ✓ Updated in `C1 v1.2` |
| `atomic_unit.list/override` | `atomic_units.allow_split_override`, `files.unit_split_override`, `audit_log` | ✓ Exact |
| `dedupe.*` | `files` (`blake3`, `simhash_text`, `dhash_image`, `nlink`), `action_batches`, `actions` | ✓ Exact |
| `actions.*`, `undo.last` | `action_batches`, `actions` (`target_kind`, nullable `file_id`, `atomic_unit_id`, `idx_actions_file_status` UNIQUE, `pre/post_blake3`, `pre/post_state_json`, `rollback_conflict`, `status`) | ✓ Updated in `B4 v2.3.1` + `C1 v1.2` |
| `quarantine.list/space_summary/restore/purge` | `quarantine_entries` (`target_kind`, nullable `file_id`, `atomic_unit_id`, `size_bytes`), `volumes.quarantine_dir`, `settings['quarantine.max_volume_pct']`, `actions`, `audit_log` | ✓ Updated in `B4 v2.3.1` + `C1 v1.2` |
| `health.get`, `playbook.disk_rescue` | `files` aggregates, `cache_dirs`, `security_findings`, `scan_jobs` | ✓ Updated in `B4 v2.3.1` |
| `findings.scan/list/action` | `security_findings` (`open\|false_positive\|accepted_risk\|vaulted\|resolved`), `security_finding_locations` (`PK(finding_id, file_id, line_number)`) | ✓ Updated in `B4 v2.3.1` |
| `rules.*`, `rules.graduate` | `rules` (`source='graduated_proposal'`), `routing_exemplars` | ✓ Exact |
| `audit.list/export` | `audit_log` (canonical length-prefixed `BLAKE3` chain) | ✓ Updated in `B4 v2.3.1` |
| `settings.*`, `onboarding.*`, `models.*`, `hw.profile`, `doctor.run` | `settings`, `volumes`, `scan_jobs`, `audit_log`, `outbox_events`, `chunks_fts`, `cache_dirs`, `relationships` | ✓ Exact |
| `collection.*` | Deferred to V1.5 (`collections` table in `B4 v2.4`) | ✓ Scoped |
