//! Tamper-evident BLAKE3 hash-chained `audit_log` (`B4 v2.3.1`, `B6` §7).

use crate::contracts::AuditEntry;
use crate::error::Result;
use crate::{new_id, now_ms};
use blake3::Hasher;
use rusqlite::{Connection, OptionalExtension, params};

const GENESIS_PREV_HASH: [u8; 32] = [0u8; 32];

fn write_lp(hasher: &mut Hasher, s: &str) {
    let bytes = s.as_bytes();
    hasher.update(&(bytes.len() as u32).to_le_bytes());
    hasher.update(bytes);
}

/// Input fields hashed into a canonical `audit_log` row (`B4 v2.3.1` §3).
pub struct AuditHashFields<'a> {
    pub seq: i64,
    pub id: &'a str,
    pub ts: i64,
    pub actor: &'a str,
    pub action: &'a str,
    pub target: &'a str,
    pub detail_json: &'a str,
    pub prev_hash: &'a [u8; 32],
}

/// Canonical length-prefixed `BLAKE3` row hash (`B4 v2.3.1` §3).
pub fn compute_audit_hash(fields: &AuditHashFields<'_>) -> [u8; 32] {
    let mut h = Hasher::new();
    h.update(&fields.seq.to_le_bytes());
    write_lp(&mut h, fields.id);
    h.update(&fields.ts.to_le_bytes());
    write_lp(&mut h, fields.actor);
    write_lp(&mut h, fields.action);
    write_lp(&mut h, fields.target);
    write_lp(&mut h, fields.detail_json);
    h.update(fields.prev_hash);
    *h.finalize().as_bytes()
}

/// Appends one row to `audit_log` inside an existing transaction.
pub fn append_audit_in_tx(
    tx: &Connection,
    actor: &str,
    action: &str,
    target: &str,
    detail_json: &str,
) -> Result<i64> {
    let last: Option<(i64, Vec<u8>)> = tx
        .query_row(
            "SELECT seq, hash FROM audit_log ORDER BY seq DESC LIMIT 1",
            [],
            |r| Ok((r.get(0)?, r.get(1)?)),
        )
        .optional()?;

    let (seq, prev_hash) = match last {
        Some((prev_seq, prev_vec)) => {
            let mut arr = [0u8; 32];
            if prev_vec.len() == 32 {
                arr.copy_from_slice(&prev_vec);
            }
            (prev_seq + 1, arr)
        }
        None => (1, GENESIS_PREV_HASH),
    };

    let id = new_id();
    let ts = now_ms();
    let hash = compute_audit_hash(&AuditHashFields {
        seq,
        id: &id,
        ts,
        actor,
        action,
        target,
        detail_json,
        prev_hash: &prev_hash,
    });

    tx.execute(
        "INSERT INTO audit_log (seq, id, ts, actor, action, target, detail_json, prev_hash, hash)
         VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9)",
        params![
            seq,
            id,
            ts,
            actor,
            action,
            target,
            detail_json,
            prev_hash.as_slice(),
            hash.as_slice()
        ],
    )?;

    Ok(seq)
}

/// Appends one row to `audit_log` inside a serialized `BEGIN IMMEDIATE` + `synchronous=FULL` tx.
pub fn append_audit(
    conn: &mut Connection,
    actor: &str,
    action: &str,
    target: &str,
    detail_json: &str,
) -> Result<i64> {
    conn.execute_batch("PRAGMA synchronous = FULL;")?;
    let res = (|| {
        let tx = conn.transaction_with_behavior(rusqlite::TransactionBehavior::Immediate)?;
        let seq = append_audit_in_tx(&tx, actor, action, target, detail_json)?;
        tx.commit()?;
        Ok(seq)
    })();
    let _ = conn.execute_batch("PRAGMA synchronous = NORMAL;");
    res
}

/// Result of verifying the entire `audit_log` hash chain (`1..N`).
#[derive(Debug, Clone)]
pub struct AuditVerifyResult {
    pub valid: bool,
    pub total_rows: usize,
    pub first_corrupt_seq: Option<i64>,
    pub detail: String,
}

/// Recomputes every `BLAKE3` link in `audit_log` without rewriting corrupted entries (`E4` Check #1).
pub fn verify_audit_chain(conn: &Connection) -> Result<AuditVerifyResult> {
    let mut stmt = conn.prepare(
        "SELECT seq, id, ts, actor, action, target, detail_json, prev_hash, hash
         FROM audit_log ORDER BY seq ASC",
    )?;
    let mut rows = stmt.query([])?;
    let mut expected_prev = GENESIS_PREV_HASH;
    let mut expected_seq: i64 = 1;
    let mut count = 0usize;

    while let Some(row) = rows.next()? {
        count += 1;
        let seq: i64 = row.get(0)?;
        let id: String = row.get(1)?;
        let ts: i64 = row.get(2)?;
        let actor: String = row.get(3)?;
        let action: String = row.get(4)?;
        let target: String = row.get(5)?;
        let detail_json: String = row.get(6)?;
        let prev_hash: Vec<u8> = row.get(7)?;
        let hash: Vec<u8> = row.get(8)?;

        if seq != expected_seq {
            return Ok(AuditVerifyResult {
                valid: false,
                total_rows: count,
                first_corrupt_seq: Some(seq),
                detail: format!("Non-contiguous seq: expected {expected_seq}, got {seq}"),
            });
        }
        if prev_hash.as_slice() != expected_prev {
            return Ok(AuditVerifyResult {
                valid: false,
                total_rows: count,
                first_corrupt_seq: Some(seq),
                detail: format!("Broken prev_hash link at seq {seq}"),
            });
        }
        let recomputed = compute_audit_hash(&AuditHashFields {
            seq,
            id: &id,
            ts,
            actor: &actor,
            action: &action,
            target: &target,
            detail_json: &detail_json,
            prev_hash: &expected_prev,
        });
        if hash.as_slice() != recomputed {
            return Ok(AuditVerifyResult {
                valid: false,
                total_rows: count,
                first_corrupt_seq: Some(seq),
                detail: format!("Hash mismatch at seq {seq}"),
            });
        }
        expected_prev = recomputed;
        expected_seq += 1;
    }

    Ok(AuditVerifyResult {
        valid: true,
        total_rows: count,
        first_corrupt_seq: None,
        detail: format!("Verified {count} audit entries"),
    })
}

pub fn list_audit(conn: &Connection, limit: i64, offset: i64) -> Result<Vec<AuditEntry>> {
    let mut stmt = conn.prepare(
        "SELECT seq, id, ts, actor, action, target, detail_json
         FROM audit_log ORDER BY seq DESC LIMIT ?1 OFFSET ?2",
    )?;
    let rows = stmt.query_map(params![limit, offset], |r| {
        Ok(AuditEntry {
            seq: r.get(0)?,
            id: r.get(1)?,
            ts: r.get(2)?,
            actor: r.get(3)?,
            action: r.get(4)?,
            target: r.get(5)?,
            detail_json: r.get(6)?,
        })
    })?;
    let mut out = Vec::new();
    for row in rows {
        out.push(row?);
    }
    Ok(out)
}
