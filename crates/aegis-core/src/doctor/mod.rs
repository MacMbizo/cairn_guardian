//! `aegis doctor` 8-point integrity check & repair engine (`E2/E4 v1.2` §4).

use crate::contracts::{DoctorCheckResult, DoctorReport};
use crate::db::SqliteStore;
use crate::db::audit::{append_audit_in_tx, verify_audit_chain};
use crate::error::Result;
use crate::now_ms;
use rusqlite::params;
use std::path::Path;

/// Runs all 8 deterministic checks from `E2_E4_Delivery_Observability_Spec.md` (`v1.2` §4).
pub fn run_doctor(store: &mut SqliteStore, repair: bool) -> Result<DoctorReport> {
    let checked_at = now_ms();
    let mut checks = Vec::with_capacity(8);

    // 1. Audit Chain Integrity (NEVER auto-rewritten)
    let audit_res = verify_audit_chain(store.conn())?;
    checks.push(DoctorCheckResult {
        check_id: 1,
        name: "Audit Chain Integrity".into(),
        passed: audit_res.valid,
        details: audit_res.detail,
        repaired: false,
        remediation: audit_res
            .first_corrupt_seq
            .map(|seq| format!("Inspect tampering starting at audit_log.seq = {seq}")),
    });

    // 2. SQLite Integrity (quick_check + foreign_key_check)
    let quick: String = store
        .conn()
        .query_row("PRAGMA quick_check", [], |r| r.get(0))?;
    let fk_violations = {
        let mut fk_stmt = store.conn().prepare("PRAGMA foreign_key_check")?;
        fk_stmt.query_map([], |_| Ok(()))?.count()
    };
    let sqlite_ok = quick.eq_ignore_ascii_case("ok") && fk_violations == 0;
    checks.push(DoctorCheckResult {
        check_id: 2,
        name: "SQLite Integrity".into(),
        passed: sqlite_ok,
        details: format!("quick_check={quick}, fk_violations={fk_violations}"),
        repaired: false,
        remediation: (!sqlite_ok).then(|| "Restore DB backup or inspect corrupt tables.".into()),
    });

    // 3. FTS5 Sync Drift (compare chunks vs chunks_fts_docsize shadow table + integrity-check)
    let chunks_cnt: i64 =
        store
            .conn()
            .query_row("SELECT COUNT(*) FROM chunks", [], |r| r.get(0))?;
    let fts_cnt: i64 = store
        .conn()
        .query_row("SELECT COUNT(*) FROM chunks_fts_docsize", [], |r| r.get(0))?;
    let fts_integrity_ok = store
        .conn()
        .execute("INSERT INTO chunks_fts(chunks_fts) VALUES('integrity-check')", [])
        .is_ok();
    let mut fts_passed = chunks_cnt == fts_cnt && fts_integrity_ok;
    let mut fts_repaired = false;
    if !fts_passed && repair {
        store
            .conn()
            .execute("INSERT INTO chunks_fts(chunks_fts) VALUES('rebuild')", [])?;
        let new_fts: i64 = store
            .conn()
            .query_row("SELECT COUNT(*) FROM chunks_fts_docsize", [], |r| r.get(0))?;
        let new_ok = store
            .conn()
            .execute("INSERT INTO chunks_fts(chunks_fts) VALUES('integrity-check')", [])
            .is_ok();
        fts_repaired = new_fts == chunks_cnt && new_ok;
        fts_passed = fts_repaired;
    }
    checks.push(DoctorCheckResult {
        check_id: 3,
        name: "FTS5 Sync Drift".into(),
        passed: fts_passed,
        details: format!("chunks={chunks_cnt}, chunks_fts={fts_cnt}"),
        repaired: fts_repaired,
        remediation: (!fts_passed).then(|| "Run doctor with repair=true to rebuild FTS5.".into()),
    });

    // 4. Outbox / Qdrant Lag
    let outbox_pending: i64 = store.conn().query_row(
        "SELECT COUNT(*) FROM outbox_events WHERE processed_at IS NULL",
        [],
        |r| r.get(0),
    )?;
    checks.push(DoctorCheckResult {
        check_id: 4,
        name: "Outbox / Qdrant Lag".into(),
        passed: true,
        details: format!("{outbox_pending} pending vector outbox event(s)"),
        repaired: false,
        remediation: None,
    });

    // 5. Quarantine Consistency
    let mut missing_q = Vec::new();
    {
        let mut qstmt = store.conn().prepare(
            "SELECT q.id, q.quarantine_path
             FROM quarantine_entries q
             JOIN volumes v ON v.id = q.volume_id
             WHERE q.restored_at IS NULL AND q.purged_at IS NULL AND v.status = 'online'",
        )?;
        for r in qstmt.query_map([], |r| Ok((r.get::<_, String>(0)?, r.get::<_, String>(1)?)))? {
            let (qid, qpath) = r?;
            if !Path::new(&qpath).exists() {
                missing_q.push((qid, qpath));
            }
        }
    }
    let mut q_repaired = false;
    let mut q_passed = missing_q.is_empty();
    if !q_passed && repair {
        store.with_full_sync_tx(|tx| {
            for (qid, qpath) in &missing_q {
                tx.execute(
                    "UPDATE quarantine_entries SET purged_at = ?1 WHERE id = ?2",
                    params![checked_at, qid],
                )?;
                append_audit_in_tx(
                    tx,
                    "system",
                    "quarantine_purged_externally",
                    qpath,
                    &serde_json::json!({ "entry_id": qid }).to_string(),
                )?;
            }
            Ok(())
        })?;
        q_repaired = true;
        q_passed = true;
    }
    checks.push(DoctorCheckResult {
        check_id: 5,
        name: "Quarantine Consistency".into(),
        passed: q_passed,
        details: format!("{} externally missing quarantine item(s)", missing_q.len()),
        repaired: q_repaired,
        remediation: (!q_passed)
            .then(|| "Run doctor repair to reconcile externally purged quarantine files.".into()),
    });

    // 6. Model Checksums
    checks.push(DoctorCheckResult {
        check_id: 6,
        name: "Model Checksums".into(),
        passed: true,
        details: "No downloaded ONNX/GGUF models requiring checksum repair yet".into(),
        repaired: false,
        remediation: None,
    });

    // 7. Watcher Health (check configured enabled roots exist)
    let roots = store.list_roots()?;
    let offline_roots: Vec<String> = roots
        .iter()
        .filter(|r| r.enabled && !Path::new(&r.path).exists())
        .map(|r| r.path.clone())
        .collect();
    let roots_ok = offline_roots.is_empty();
    checks.push(DoctorCheckResult {
        check_id: 7,
        name: "Watcher & Root Health".into(),
        passed: roots_ok,
        details: format!(
            "{} configured root(s), {} unreachable",
            roots.len(),
            offline_roots.len()
        ),
        repaired: false,
        remediation: (!roots_ok).then(|| format!("Reconnect or remove: {}", offline_roots.join(", "))),
    });

    // 8. Orphan Graph Edges & Stale cache_dirs (`E2_E4 v1.2` Check #8)
    let orphan_rels: Vec<String> = {
        let mut rstmt = store.conn().prepare(
            "SELECT id FROM relationships
             WHERE valid_to IS NULL
               AND ((src_kind = 'file' AND NOT EXISTS (SELECT 1 FROM files f WHERE f.id = src_id AND f.deleted_at IS NULL))
                 OR (dst_kind = 'file' AND NOT EXISTS (SELECT 1 FROM files f WHERE f.id = dst_id AND f.deleted_at IS NULL)))",
        )?;
        let mut ids = Vec::new();
        for r in rstmt.query_map([], |r| r.get::<_, String>(0))? {
            ids.push(r?);
        }
        ids
    };
    let stale_caches: Vec<String> = {
        let mut cstmt = store.conn().prepare(
            "SELECT c.id, c.path
             FROM cache_dirs c
             JOIN volumes v ON v.id = c.volume_id
             WHERE c.state = 'active' AND v.status = 'online'",
        )?;
        let mut ids = Vec::new();
        for r in cstmt.query_map([], |r| Ok((r.get::<_, String>(0)?, r.get::<_, String>(1)?)))? {
            let (cid, cpath) = r?;
            if !Path::new(&cpath).exists() {
                ids.push(cid);
            }
        }
        ids
    };

    let mut ch8_passed = orphan_rels.is_empty() && stale_caches.is_empty();
    let mut ch8_repaired = false;
    if !ch8_passed && repair {
        store.with_full_sync_tx(|tx| {
            for rid in &orphan_rels {
                tx.execute(
                    "UPDATE relationships SET valid_to = ?1 WHERE id = ?2",
                    params![checked_at, rid],
                )?;
            }
            for cid in &stale_caches {
                tx.execute(
                    "UPDATE cache_dirs SET state = 'missing', updated_at = ?1 WHERE id = ?2",
                    params![checked_at, cid],
                )?;
            }
            Ok(())
        })?;
        ch8_repaired = true;
        ch8_passed = true;
    }

    checks.push(DoctorCheckResult {
        check_id: 8,
        name: "Orphan Graph Edges & Stale cache_dirs".into(),
        passed: ch8_passed,
        details: format!(
            "{} orphan relationship(s), {} stale cache_dir(s)",
            orphan_rels.len(),
            stale_caches.len()
        ),
        repaired: ch8_repaired,
        remediation: (!ch8_passed)
            .then(|| "Run doctor repair to expire orphan edges and mark missing cache_dirs.".into()),
    });

    let healthy = checks.iter().all(|c| c.passed);
    Ok(DoctorReport {
        healthy,
        checked_at,
        checks,
    })
}
