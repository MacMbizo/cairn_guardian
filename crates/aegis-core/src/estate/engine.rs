//! Core Digital Estate Management engine: readiness scoring, trustee & packet CRUD (`R2`/`R7`),
//! non-secret Shamir shard lifecycle, quorum verification, non-destructive heartbeat (`R10`),
//! and offline Continuity Kit export (`5.3`, `D1`, `D3`, `R8`, `R9`, `R10`).

use std::collections::HashMap;
use std::fs;
use std::path::Path;

use rusqlite::{Connection, OptionalExtension, params};

use crate::db::SqliteStore;
use crate::db::audit::append_audit_in_tx;
use crate::db::crypto::DbKey;
use crate::db::files::SENSITIVE_MASK;
use crate::error::{AegisError, Result};
use crate::format_date_ms;
use crate::governance::posture::VaultPostureContext;
use crate::{new_id, now_ms};

use super::shamir::{combine_shards, parse_shard, split_key};
use super::{
    EstateCheckinRecord, EstateContinuityExport, EstateDashboard, EstateGeneratedShard,
    EstateHeartbeatConfig, EstateHeartbeatState, EstateHeartbeatStatus, EstatePacket,
    EstatePacketInput, EstatePacketKind, EstateShardRecord, EstateShardSetBundle,
    EstateShardVerifyResult, EstateTrustee, EstateTrusteeInput, TrusteeRole,
};

pub const DAY_MS: i64 = 86_400_000;
const HEARTBEAT_SETTING_KEY: &str = "estate.heartbeat_config";

pub const R10_POLICY_SUMMARY: &str = "Non-destructive policy (R10): A missed check-in transitions status so trustees can receive prepared continuity packets & recovery instructions, and NEVER shreds, deletes, quarantines, or locks your files or databases.";
pub const R8_POLICY_SUMMARY: &str = "Multi-pass shredding disabled (R8): Cairn relies on SQLCipher D1 database encryption and Windows BitLocker for data at rest.";

fn readiness_grade(score: f64) -> String {
    if score >= 90.0 {
        "A".into()
    } else if score >= 80.0 {
        "A-".into()
    } else if score >= 70.0 {
        "B".into()
    } else if score >= 55.0 {
        "C".into()
    } else {
        "D".into()
    }
}

fn load_heartbeat_config(conn: &Connection, now: i64) -> Result<EstateHeartbeatConfig> {
    let raw: Option<String> = conn
        .query_row(
            "SELECT value_json FROM settings WHERE key = ?1 AND scope = 'global'",
            [HEARTBEAT_SETTING_KEY],
            |r| r.get(0),
        )
        .optional()?;
    if let Some(json) = raw
        && let Ok(mut cfg) = serde_json::from_str::<EstateHeartbeatConfig>(&json)
    {
        cfg.non_destructive_policy = R10_POLICY_SUMMARY.into();
        return Ok(cfg);
    }
    Ok(EstateHeartbeatConfig {
        enabled: false,
        interval_days: 30,
        grace_days: 14,
        last_checkin_at: now,
        non_destructive_policy: R10_POLICY_SUMMARY.into(),
    })
}

/// Evaluates current heartbeat state without ever mutating or destroying data (`R10`).
pub fn compute_heartbeat_status(cfg: &EstateHeartbeatConfig, now: i64) -> EstateHeartbeatStatus {
    if !cfg.enabled {
        return EstateHeartbeatStatus {
            config: cfg.clone(),
            state: EstateHeartbeatState::Disabled,
            next_due_at: None,
            grace_ends_at: None,
            days_remaining: None,
            r10_guarantee: R10_POLICY_SUMMARY.into(),
        };
    }

    let next_due = cfg.last_checkin_at + cfg.interval_days.clamp(7, 365) * DAY_MS;
    let grace_ends = next_due + cfg.grace_days.clamp(3, 90) * DAY_MS;

    let (state, days_remaining) = if now >= grace_ends {
        (EstateHeartbeatState::ReadyForRelease, Some(0))
    } else if now >= next_due {
        let left = ((grace_ends - now + DAY_MS - 1) / DAY_MS).max(0);
        (EstateHeartbeatState::GracePeriod, Some(left))
    } else {
        let left = ((next_due - now + DAY_MS - 1) / DAY_MS).max(0);
        let st = if left <= 7 {
            EstateHeartbeatState::Warning
        } else {
            EstateHeartbeatState::Active
        };
        (st, Some(left))
    };

    EstateHeartbeatStatus {
        config: cfg.clone(),
        state,
        next_due_at: Some(next_due),
        grace_ends_at: Some(grace_ends),
        days_remaining,
        r10_guarantee: R10_POLICY_SUMMARY.into(),
    }
}

fn list_trustees(conn: &Connection) -> Result<Vec<EstateTrustee>> {
    let mut st = conn.prepare(
        "SELECT id, name, relationship, contact_hint, role, notes, created_at, updated_at
         FROM estate_trustees
         ORDER BY CASE role
             WHEN 'primary_executor' THEN 0
             WHEN 'co_trustee' THEN 1
             WHEN 'legal_counsel' THEN 2
             ELSE 3 END, name ASC",
    )?;
    let rows = st.query_map([], |r| {
        let role_str: String = r.get(4)?;
        Ok(EstateTrustee {
            id: r.get(0)?,
            name: r.get(1)?,
            relationship: r.get(2)?,
            contact_hint: r.get(3)?,
            role: TrusteeRole::from_db(&role_str),
            notes: r.get(5)?,
            created_at: r.get(6)?,
            updated_at: r.get(7)?,
        })
    })?;
    Ok(rows.collect::<std::result::Result<Vec<_>, _>>()?)
}

fn list_shard_records(conn: &Connection) -> Result<Vec<EstateShardRecord>> {
    let mut st = conn.prepare(
        "SELECT s.id, s.shard_set_id, s.trustee_id, t.name, s.share_index, s.threshold_k,
                s.total_n, s.key_fingerprint, s.checksum_prefix, s.delivered_at, s.verified_at,
                s.revoked_at, s.created_at
         FROM estate_shard_records s
         LEFT JOIN estate_trustees t ON t.id = s.trustee_id
         ORDER BY s.created_at DESC, s.share_index ASC",
    )?;
    let rows = st.query_map([], |r| {
        Ok(EstateShardRecord {
            id: r.get(0)?,
            shard_set_id: r.get(1)?,
            trustee_id: r.get(2)?,
            trustee_name: r.get(3)?,
            share_index: r.get::<_, i64>(4)? as u8,
            threshold_k: r.get::<_, i64>(5)? as u8,
            total_n: r.get::<_, i64>(6)? as u8,
            key_fingerprint: r.get(7)?,
            checksum_prefix: r.get(8)?,
            delivered_at: r.get(9)?,
            verified_at: r.get(10)?,
            revoked_at: r.get(11)?,
            created_at: r.get(12)?,
        })
    })?;
    Ok(rows.collect::<std::result::Result<Vec<_>, _>>()?)
}

fn list_packets(conn: &Connection, reveal_sensitive: bool) -> Result<Vec<EstatePacket>> {
    let mut st = conn.prepare(
        "SELECT p.id, p.title, p.trustee_id, t.name, p.packet_kind, p.body_markdown,
                p.include_backup_map, p.include_project_inventory, p.created_at, p.updated_at
         FROM estate_packets p
         LEFT JOIN estate_trustees t ON t.id = p.trustee_id
         ORDER BY p.updated_at DESC",
    )?;
    let rows = st.query_map([], |r| {
        let kind_str: String = r.get(4)?;
        let raw_body: String = r.get(5)?;
        let masked = !reveal_sensitive;
        let body_markdown = if masked {
            SENSITIVE_MASK.to_string()
        } else {
            raw_body
        };
        Ok(EstatePacket {
            id: r.get(0)?,
            title: r.get(1)?,
            trustee_id: r.get(2)?,
            trustee_name: r.get(3)?,
            packet_kind: EstatePacketKind::from_db(&kind_str),
            body_markdown,
            include_backup_map: r.get::<_, i64>(6)? != 0,
            include_project_inventory: r.get::<_, i64>(7)? != 0,
            masked,
            created_at: r.get(8)?,
            updated_at: r.get(9)?,
        })
    })?;
    Ok(rows.collect::<std::result::Result<Vec<_>, _>>()?)
}

fn list_checkins(conn: &Connection, limit: usize) -> Result<Vec<EstateCheckinRecord>> {
    let mut st = conn.prepare(
        "SELECT id, method, note, checked_in_at
         FROM estate_checkins
         ORDER BY checked_in_at DESC
         LIMIT ?1",
    )?;
    let rows = st.query_map(params![limit.max(1) as i64], |r| {
        Ok(EstateCheckinRecord {
            id: r.get(0)?,
            method: r.get(1)?,
            note: r.get(2)?,
            checked_in_at: r.get(3)?,
        })
    })?;
    Ok(rows.collect::<std::result::Result<Vec<_>, _>>()?)
}

/// Builds the Digital Estate readiness dashboard (`D1`, `D3`, `R2`, `R7`, `R8`, `R10`).
pub fn build_estate_dashboard(
    conn: &Connection,
    vault_ctx: &VaultPostureContext,
    reveal_sensitive: bool,
    now: i64,
) -> Result<EstateDashboard> {
    let trustees = list_trustees(conn)?;
    let shard_records = list_shard_records(conn)?;
    let packets = list_packets(conn, reveal_sensitive)?;
    let recent_checkins = list_checkins(conn, 15)?;
    let hb_cfg = load_heartbeat_config(conn, now)?;
    let heartbeat = compute_heartbeat_status(&hb_cfg, now);

    let backup_drives_count: i64 = conn.query_row(
        "SELECT COUNT(*) FROM volumes WHERE role = 'backup'",
        [],
        |r| r.get(0),
    )?;
    let backup_drives_online: i64 = conn.query_row(
        "SELECT COUNT(*) FROM volumes WHERE role = 'backup' AND status = 'online'",
        [],
        |r| r.get(0),
    )?;

    let active_shards: Vec<&EstateShardRecord> = shard_records
        .iter()
        .filter(|s| s.revoked_at.is_none())
        .collect();
    let quorum_ok = active_shards
        .first()
        .is_some_and(|s| active_shards.len() >= usize::from(s.threshold_k));
    let any_verified = active_shards.iter().any(|s| s.verified_at.is_some());

    let mut score = 0.0f64;
    if vault_ctx.db_encrypted {
        score += 20.0;
    }
    if vault_ctx.recovery_confirmed {
        score += 15.0;
    }
    if trustees.len() >= 2 {
        score += 20.0;
    } else if trustees.len() == 1 {
        score += 10.0;
    }
    if quorum_ok {
        score += 20.0;
    }
    if any_verified {
        score += 5.0;
    }
    if !packets.is_empty() {
        score += 10.0;
    }
    if hb_cfg.enabled {
        score += 5.0;
    }
    if backup_drives_count > 0 {
        score += 5.0;
    }
    let readiness_score = score.clamp(0.0, 100.0);

    Ok(EstateDashboard {
        readiness_score,
        readiness_grade: readiness_grade(readiness_score),
        db_encrypted: vault_ctx.db_encrypted,
        recovery_confirmed: vault_ctx.recovery_confirmed,
        backup_drives_count,
        backup_drives_online,
        heartbeat,
        trustees,
        shard_records,
        packets,
        packets_masked: !reveal_sensitive,
        recent_checkins,
        r10_policy_summary: R10_POLICY_SUMMARY.into(),
        r8_policy_summary: R8_POLICY_SUMMARY.into(),
    })
}

pub fn save_estate_trustee(
    store: &mut SqliteStore,
    input: &EstateTrusteeInput,
) -> Result<EstateTrustee> {
    let name = input.name.trim().to_string();
    if name.is_empty() {
        return Err(AegisError::InvalidInput("Trustee name must not be empty.".into()));
    }
    let now = now_ms();
    let id = input
        .id
        .as_deref()
        .map(str::trim)
        .filter(|s| !s.is_empty())
        .map(str::to_string)
        .unwrap_or_else(new_id);

    let created_at = store.with_full_sync_tx(|tx| {
        let existing_created: Option<i64> = tx
            .query_row(
                "SELECT created_at FROM estate_trustees WHERE id = ?1",
                params![id],
                |r| r.get(0),
            )
            .optional()?;
        let c_at = existing_created.unwrap_or(now);
        tx.execute(
            "INSERT INTO estate_trustees
             (id, name, relationship, contact_hint, role, notes, created_at, updated_at)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8)
             ON CONFLICT(id) DO UPDATE SET
               name = excluded.name,
               relationship = excluded.relationship,
               contact_hint = excluded.contact_hint,
               role = excluded.role,
               notes = excluded.notes,
               updated_at = excluded.updated_at",
            params![
                id,
                name,
                input.relationship.trim(),
                input.contact_hint.trim(),
                input.role.as_str(),
                input.notes.trim(),
                c_at,
                now
            ],
        )?;
        append_audit_in_tx(
            tx,
            "user",
            "estate.save_trustee",
            &id,
            &serde_json::json!({ "name": name, "role": input.role.as_str() }).to_string(),
        )?;
        Ok(c_at)
    })?;

    Ok(EstateTrustee {
        id,
        name,
        relationship: input.relationship.trim().to_string(),
        contact_hint: input.contact_hint.trim().to_string(),
        role: input.role,
        notes: input.notes.trim().to_string(),
        created_at,
        updated_at: now,
    })
}

pub fn delete_estate_trustee(store: &mut SqliteStore, trustee_id: &str) -> Result<bool> {
    store.with_full_sync_tx(|tx| {
        let changed = tx.execute(
            "DELETE FROM estate_trustees WHERE id = ?1",
            params![trustee_id],
        )?;
        if changed > 0 {
            append_audit_in_tx(tx, "user", "estate.delete_trustee", trustee_id, "{}")?;
        }
        Ok(changed > 0)
    })
}

pub fn save_estate_packet(
    store: &mut SqliteStore,
    input: &EstatePacketInput,
    reveal_sensitive: bool,
) -> Result<EstatePacket> {
    if !reveal_sensitive {
        return Err(AegisError::PermissionDenied(
            "Windows Hello / Windows user verification is required before editing sealed succession packets (R2/R7)."
                .into(),
        ));
    }
    let title = input.title.trim().to_string();
    if title.is_empty() {
        return Err(AegisError::InvalidInput("Packet title must not be empty.".into()));
    }
    let now = now_ms();
    let id = input
        .id
        .as_deref()
        .map(str::trim)
        .filter(|s| !s.is_empty())
        .map(str::to_string)
        .unwrap_or_else(new_id);
    let trustee_id = input
        .trustee_id
        .as_deref()
        .map(str::trim)
        .filter(|s| !s.is_empty())
        .map(str::to_string);

    let (created_at, trustee_name) = store.with_full_sync_tx(|tx| {
        let existing_created: Option<i64> = tx
            .query_row(
                "SELECT created_at FROM estate_packets WHERE id = ?1",
                params![id],
                |r| r.get(0),
            )
            .optional()?;
        let c_at = existing_created.unwrap_or(now);
        tx.execute(
            "INSERT INTO estate_packets
             (id, title, trustee_id, packet_kind, body_markdown, include_backup_map,
              include_project_inventory, created_at, updated_at)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9)
             ON CONFLICT(id) DO UPDATE SET
               title = excluded.title,
               trustee_id = excluded.trustee_id,
               packet_kind = excluded.packet_kind,
               body_markdown = excluded.body_markdown,
               include_backup_map = excluded.include_backup_map,
               include_project_inventory = excluded.include_project_inventory,
               updated_at = excluded.updated_at",
            params![
                id,
                title,
                trustee_id,
                input.packet_kind.as_str(),
                input.body_markdown,
                i32::from(input.include_backup_map),
                i32::from(input.include_project_inventory),
                c_at,
                now
            ],
        )?;
        let tname: Option<String> = match trustee_id.as_deref() {
            Some(tid) => tx
                .query_row(
                    "SELECT name FROM estate_trustees WHERE id = ?1",
                    params![tid],
                    |r| r.get(0),
                )
                .optional()?,
            None => None,
        };
        append_audit_in_tx(
            tx,
            "user",
            "estate.save_packet",
            &id,
            &serde_json::json!({ "title": title, "kind": input.packet_kind.as_str() }).to_string(),
        )?;
        Ok((c_at, tname))
    })?;

    Ok(EstatePacket {
        id,
        title,
        trustee_id,
        trustee_name,
        packet_kind: input.packet_kind,
        body_markdown: input.body_markdown.clone(),
        include_backup_map: input.include_backup_map,
        include_project_inventory: input.include_project_inventory,
        masked: false,
        created_at,
        updated_at: now,
    })
}

pub fn delete_estate_packet(
    store: &mut SqliteStore,
    packet_id: &str,
    reveal_sensitive: bool,
) -> Result<bool> {
    if !reveal_sensitive {
        return Err(AegisError::PermissionDenied(
            "Windows Hello / Windows user verification is required before removing succession packets (R2/R7)."
                .into(),
        ));
    }
    store.with_full_sync_tx(|tx| {
        let changed = tx.execute("DELETE FROM estate_packets WHERE id = ?1", params![packet_id])?;
        if changed > 0 {
            append_audit_in_tx(tx, "user", "estate.delete_packet", packet_id, "{}")?;
        }
        Ok(changed > 0)
    })
}

/// Generates a $K$-of-$N$ Shamir shard set over `db_key`; stores metadata only (`D1`, `R2`, `R7`).
pub fn generate_estate_shard_set(
    store: &mut SqliteStore,
    db_key: &DbKey,
    threshold_k: u8,
    total_n: u8,
    trustee_ids: &[Option<String>],
    reveal_sensitive: bool,
) -> Result<EstateShardSetBundle> {
    if !reveal_sensitive {
        return Err(AegisError::PermissionDenied(
            "Windows Hello / Windows user verification is required before generating recovery key shards (R2/R7)."
                .into(),
        ));
    }

    let shard_set_id = format!("ss-{}", &new_id()[..8]);
    let key_fp = db_key.fingerprint();
    let raw_shards = split_key(db_key, threshold_k, total_n, &shard_set_id)?;
    let now = now_ms();

    let generated = store.with_full_sync_tx(|tx| {
        // Retire prior unrevoked shard sets cleanly.
        tx.execute(
            "UPDATE estate_shard_records SET revoked_at = ?1 WHERE revoked_at IS NULL",
            params![now],
        )?;

        let mut names_map: HashMap<String, String> = HashMap::new();
        {
            let mut tst = tx.prepare("SELECT id, name FROM estate_trustees")?;
            for r in tst.query_map([], |row| {
                Ok((row.get::<_, String>(0)?, row.get::<_, String>(1)?))
            })? {
                let (tid, name) = r?;
                names_map.insert(tid, name);
            }
        }

        let mut out = Vec::with_capacity(raw_shards.len());
        for (pos, raw) in raw_shards.iter().enumerate() {
            let rec_id = new_id();
            let tid = trustee_ids
                .get(pos)
                .and_then(|o| o.as_deref())
                .map(str::trim)
                .filter(|s| !s.is_empty() && names_map.contains_key(*s))
                .map(str::to_string);
            let tname = tid.as_ref().and_then(|id| names_map.get(id).cloned());

            tx.execute(
                "INSERT INTO estate_shard_records
                 (id, shard_set_id, trustee_id, share_index, threshold_k, total_n,
                  key_fingerprint, checksum_prefix, created_at)
                 VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9)",
                params![
                    rec_id,
                    shard_set_id,
                    tid,
                    i64::from(raw.share_index),
                    i64::from(threshold_k),
                    i64::from(total_n),
                    key_fp,
                    raw.checksum_prefix,
                    now
                ],
            )?;

            out.push(EstateGeneratedShard {
                record_id: rec_id,
                trustee_id: tid,
                trustee_name: tname,
                share_index: raw.share_index,
                checksum_prefix: raw.checksum_prefix.clone(),
                shard_token: raw.shard_token.as_str().to_string(),
            });
        }

        append_audit_in_tx(
            tx,
            "user",
            "estate.generate_shards",
            &shard_set_id,
            &serde_json::json!({
                "threshold_k": threshold_k,
                "total_n": total_n,
                "key_fingerprint": key_fp,
            })
            .to_string(),
        )?;
        Ok(out)
    })?;

    Ok(EstateShardSetBundle {
        shard_set_id,
        threshold_k,
        total_n,
        key_fingerprint: key_fp,
        shards: generated,
        created_at: now,
    })
}

pub fn mark_estate_shard_delivered(store: &mut SqliteStore, shard_id: &str) -> Result<bool> {
    let now = now_ms();
    store.with_full_sync_tx(|tx| {
        let changed = tx.execute(
            "UPDATE estate_shard_records SET delivered_at = ?1 WHERE id = ?2 AND revoked_at IS NULL",
            params![now, shard_id],
        )?;
        if changed > 0 {
            append_audit_in_tx(tx, "user", "estate.mark_shard_delivered", shard_id, "{}")?;
        }
        Ok(changed > 0)
    })
}

pub fn revoke_estate_shard_set(store: &mut SqliteStore, shard_set_id: &str) -> Result<bool> {
    let now = now_ms();
    store.with_full_sync_tx(|tx| {
        let changed = tx.execute(
            "UPDATE estate_shard_records SET revoked_at = ?1 WHERE shard_set_id = ?2 AND revoked_at IS NULL",
            params![now, shard_set_id],
        )?;
        if changed > 0 {
            append_audit_in_tx(tx, "user", "estate.revoke_shard_set", shard_set_id, "{}")?;
        }
        Ok(changed > 0)
    })
}

/// Verifies that a pasted quorum of Shamir shards reconstructs the active vault fingerprint (`R2`, `R7`).
pub fn verify_estate_shard_quorum(
    store: &mut SqliteStore,
    shard_tokens: &[String],
    active_key_fingerprint: Option<&str>,
    reveal_sensitive: bool,
) -> Result<EstateShardVerifyResult> {
    if !reveal_sensitive {
        return Err(AegisError::PermissionDenied(
            "Windows Hello / Windows user verification is required before testing recovery shards (R2/R7)."
                .into(),
        ));
    }
    let clean: Vec<String> = shard_tokens
        .iter()
        .map(|s| s.trim().to_string())
        .filter(|s| !s.is_empty())
        .collect();
    if clean.is_empty() {
        return Err(AegisError::InvalidInput(
            "Paste at least 2 shards to verify a quorum.".into(),
        ));
    }

    let first_meta = parse_shard(&clean[0]);
    let (set_id, k, n) = match first_meta {
        Ok(ref p) => (p.shard_set_id.clone(), p.threshold_k, p.total_n),
        Err(ref e) => {
            return Ok(EstateShardVerifyResult {
                valid: false,
                shard_set_id: String::new(),
                threshold_k: 0,
                total_n: 0,
                shares_checked: clean.len() as u8,
                matches_active_vault: false,
                key_fingerprint: String::new(),
                detail: e.to_string(),
            });
        }
    };

    match combine_shards(&clean) {
        Ok(reconstructed) => {
            let fp = reconstructed.fingerprint();
            let matches_active = active_key_fingerprint.is_some_and(|a| a == fp);
            let now = now_ms();
            if matches_active {
                store.with_full_sync_tx(|tx| {
                    tx.execute(
                        "UPDATE estate_shard_records SET verified_at = ?1
                         WHERE shard_set_id = ?2 AND revoked_at IS NULL",
                        params![now, set_id],
                    )?;
                    append_audit_in_tx(
                        tx,
                        "user",
                        "estate.verify_quorum",
                        &set_id,
                        &serde_json::json!({ "matches_active_vault": true }).to_string(),
                    )?;
                    Ok(())
                })?;
            }
            let detail = if matches_active {
                format!(
                    "Quorum valid ({k}-of-{n}): reconstructed key fingerprint `{fp}` matches the active vault."
                )
            } else {
                format!(
                    "Quorum valid mathematically (fingerprint `{fp}`), but does not match active vault `{}`.",
                    active_key_fingerprint.unwrap_or("none")
                )
            };
            Ok(EstateShardVerifyResult {
                valid: matches_active || active_key_fingerprint.is_none(),
                shard_set_id: set_id,
                threshold_k: k,
                total_n: n,
                shares_checked: clean.len() as u8,
                matches_active_vault: matches_active,
                key_fingerprint: fp,
                detail,
            })
        }
        Err(e) => Ok(EstateShardVerifyResult {
            valid: false,
            shard_set_id: set_id,
            threshold_k: k,
            total_n: n,
            shares_checked: clean.len() as u8,
            matches_active_vault: false,
            key_fingerprint: String::new(),
            detail: e.to_string(),
        }),
    }
}

/// Saves non-destructive inactivity heartbeat schedule (`R10`).
pub fn save_estate_heartbeat_config(
    store: &mut SqliteStore,
    enabled: bool,
    interval_days: i64,
    grace_days: i64,
) -> Result<EstateHeartbeatStatus> {
    let now = now_ms();
    let existing = load_heartbeat_config(store.conn(), now)?;
    let cfg = EstateHeartbeatConfig {
        enabled,
        interval_days: interval_days.clamp(7, 365),
        grace_days: grace_days.clamp(3, 90),
        last_checkin_at: existing.last_checkin_at,
        non_destructive_policy: R10_POLICY_SUMMARY.into(),
    };
    let json = serde_json::to_string(&cfg)?;
    store.with_full_sync_tx(|tx| {
        tx.execute(
            "INSERT INTO settings (key, scope, value_json, updated_at)
             VALUES (?1, 'global', ?2, ?3)
             ON CONFLICT(key, scope) DO UPDATE SET value_json = excluded.value_json, updated_at = excluded.updated_at",
            params![HEARTBEAT_SETTING_KEY, json, now],
        )?;
        append_audit_in_tx(
            tx,
            "user",
            "estate.save_heartbeat",
            HEARTBEAT_SETTING_KEY,
            &serde_json::json!({
                "enabled": cfg.enabled,
                "interval_days": cfg.interval_days,
                "grace_days": cfg.grace_days,
            })
            .to_string(),
        )?;
        Ok(())
    })?;
    Ok(compute_heartbeat_status(&cfg, now))
}

/// Records an explicit owner check-in, resetting the non-destructive countdown (`R10`).
pub fn record_estate_checkin(
    store: &mut SqliteStore,
    method: &str,
    note: Option<&str>,
) -> Result<EstateHeartbeatStatus> {
    let now = now_ms();
    let clean_method = if method.trim().is_empty() {
        "manual"
    } else {
        method.trim()
    };
    let clean_note = note.unwrap_or("").trim();
    let id = new_id();

    let mut cfg = load_heartbeat_config(store.conn(), now)?;
    cfg.last_checkin_at = now;
    cfg.non_destructive_policy = R10_POLICY_SUMMARY.into();
    let json = serde_json::to_string(&cfg)?;

    store.with_full_sync_tx(|tx| {
        tx.execute(
            "INSERT INTO estate_checkins (id, method, note, checked_in_at) VALUES (?1, ?2, ?3, ?4)",
            params![id, clean_method, clean_note, now],
        )?;
        tx.execute(
            "INSERT INTO settings (key, scope, value_json, updated_at)
             VALUES (?1, 'global', ?2, ?3)
             ON CONFLICT(key, scope) DO UPDATE SET value_json = excluded.value_json, updated_at = excluded.updated_at",
            params![HEARTBEAT_SETTING_KEY, json, now],
        )?;
        append_audit_in_tx(
            tx,
            "user",
            "estate.checkin",
            &id,
            &serde_json::json!({ "method": clean_method }).to_string(),
        )?;
        Ok(())
    })?;

    Ok(compute_heartbeat_status(&cfg, now))
}

fn sanitize_filename_slug(s: &str) -> String {
    let mut out = String::new();
    for ch in s.chars() {
        if ch.is_ascii_alphanumeric() {
            out.push(ch.to_ascii_lowercase());
        } else if (ch.is_whitespace() || ch == '-' || ch == '_') && !out.ends_with('_') {
            out.push('_');
        }
    }
    let trimmed = out.trim_matches('_').to_string();
    if trimmed.is_empty() {
        "packet".into()
    } else {
        trimmed.chars().take(40).collect()
    }
}

/// Exports a strictly non-destructive Trustee Continuity Kit to `output_dir` (`R2`, `R7`, `R10`).
/// Never writes raw shard tokens or raw `DbKey` secrets into exported files.
pub fn export_estate_continuity_kit(
    conn: &Connection,
    output_dir: &Path,
    reveal_sensitive: bool,
) -> Result<EstateContinuityExport> {
    if !reveal_sensitive {
        return Err(AegisError::PermissionDenied(
            "Windows Hello / Windows user verification is required before exporting Continuity Kit packets (R2/R7)."
                .into(),
        ));
    }
    if output_dir.as_os_str().is_empty() {
        return Err(AegisError::InvalidInput(
            "Select an output folder for the Continuity Kit export.".into(),
        ));
    }

    let now = now_ms();
    let kit_dir = output_dir.join(format!("cairn-continuity-kit-{}", format_date_ms(now)));
    fs::create_dir_all(&kit_dir)?;

    let trustees = list_trustees(conn)?;
    let shard_records = list_shard_records(conn)?;
    let packets = list_packets(conn, true)?;
    let active_shards: Vec<&EstateShardRecord> = shard_records
        .iter()
        .filter(|s| s.revoked_at.is_none())
        .collect();

    let mut backup_volumes: Vec<serde_json::Value> = Vec::new();
    {
        let mut vst = conn.prepare(
            "SELECT id, COALESCE(label, mount_point), mount_point, status
             FROM volumes WHERE role = 'backup' ORDER BY mount_point",
        )?;
        for r in vst.query_map([], |row| {
            Ok(serde_json::json!({
                "id": row.get::<_, String>(0)?,
                "label": row.get::<_, String>(1)?,
                "mount_point": row.get::<_, String>(2)?,
                "status": row.get::<_, String>(3)?,
            }))
        })? {
            backup_volumes.push(r?);
        }
    }

    let mut projects_summary: Vec<serde_json::Value> = Vec::new();
    {
        let mut pst = conn.prepare(
            "SELECT c.name, c.project_kind, cl.root_path
             FROM collections c
             JOIN collection_locations cl ON cl.collection_id = c.id
             WHERE c.kind = 'project' AND c.state != 'dismissed'
             ORDER BY c.name ASC LIMIT 50",
        )?;
        for r in pst.query_map([], |row| {
            Ok(serde_json::json!({
                "name": row.get::<_, String>(0)?,
                "project_kind": row.get::<_, Option<String>>(1)?,
                "root_path": row.get::<_, String>(2)?,
            }))
        })? {
            projects_summary.push(r?);
        }
    }

    let mut files_written = Vec::new();

    // 1. README_TRUSTEE_RECOVERY.md
    let quorum_line = if let Some(first) = active_shards.first() {
        format!(
            "Active Shamir Shard Set `{}` requires **{} of {}** shares for key fingerprint `{}`.\n(Raw shard strings are NEVER written to this kit; each trustee holds their own `AEGIS-SHARD-V1:...` share separately.)",
            first.shard_set_id, first.threshold_k, first.total_n, first.key_fingerprint
        )
    } else {
        "No active Shamir shard set recorded yet — check offline 24-word BIP-39 / 64-hex recovery key.".into()
    };
    let readme = format!(
        "# Cairn Guardian — Trustee Recovery & Continuity Guide\n\n\
         Exported: `{}`\n\n\
         ## Non-Destructive Guarantee (R10)\n\
         {R10_POLICY_SUMMARY}\n\n\
         ## Key & Backup Recovery\n\
         1. {quorum_line}\n\
         2. Backup drives registered in Cairn: `{}` drive(s) (see `manifest.json`).\n\
         3. Reassemble shards offline or inside Cairn Vault Unlock / Digital Estate -> Quorum Tester to recover the SQLCipher `aegis.db` encryption key.\n",
        format_date_ms(now),
        backup_volumes.len()
    );
    let readme_path = kit_dir.join("README_TRUSTEE_RECOVERY.md");
    fs::write(&readme_path, readme)?;
    files_written.push(readme_path.to_string_lossy().to_string());

    // 2. Sealed packets
    let mut packet_manifest = Vec::new();
    for (idx, p) in packets.iter().enumerate() {
        let filename = format!("packet_{}_{}.md", idx + 1, sanitize_filename_slug(&p.title));
        let ppath = kit_dir.join(&filename);
        let content = format!(
            "# {}\nKind: {:?} · Trustee: {}\n\n{}\n",
            p.title,
            p.packet_kind,
            p.trustee_name.as_deref().unwrap_or("Unassigned"),
            p.body_markdown
        );
        fs::write(&ppath, content)?;
        files_written.push(ppath.to_string_lossy().to_string());
        packet_manifest.push(serde_json::json!({
            "id": p.id,
            "title": p.title,
            "file": filename,
            "kind": p.packet_kind.as_str(),
            "trustee_name": p.trustee_name,
        }));
    }

    // 3. manifest.json (strictly metadata, zero raw keys/shards)
    let include_bak = packets.is_empty() || packets.iter().any(|p| p.include_backup_map);
    let include_prj = packets.iter().any(|p| p.include_project_inventory);
    let manifest = serde_json::json!({
        "schema": "aegis-continuity-kit-v1",
        "exported_at": now,
        "r10_policy": R10_POLICY_SUMMARY,
        "trustees": trustees,
        "active_shard_records": active_shards,
        "packets": packet_manifest,
        "backup_drives": if include_bak { backup_volumes } else { Vec::new() },
        "projects": if include_prj { projects_summary } else { Vec::new() },
    });
    let manifest_path = kit_dir.join("manifest.json");
    fs::write(&manifest_path, serde_json::to_string_pretty(&manifest)?)?;
    files_written.push(manifest_path.to_string_lossy().to_string());

    Ok(EstateContinuityExport {
        output_dir: kit_dir.to_string_lossy().to_string(),
        manifest_path: manifest_path.to_string_lossy().to_string(),
        files_written,
        exported_at: now,
        r10_notice: R10_POLICY_SUMMARY.into(),
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn heartbeat_state_machine_transitions_without_destruction_r10() {
        let base = 1_700_000_000_000i64;
        let cfg = EstateHeartbeatConfig {
            enabled: true,
            interval_days: 30,
            grace_days: 10,
            last_checkin_at: base,
            non_destructive_policy: R10_POLICY_SUMMARY.into(),
        };
        assert_eq!(
            compute_heartbeat_status(&cfg, base + 5 * DAY_MS).state,
            EstateHeartbeatState::Active
        );
        assert_eq!(
            compute_heartbeat_status(&cfg, base + 25 * DAY_MS).state,
            EstateHeartbeatState::Warning
        );
        assert_eq!(
            compute_heartbeat_status(&cfg, base + 32 * DAY_MS).state,
            EstateHeartbeatState::GracePeriod
        );
        assert_eq!(
            compute_heartbeat_status(&cfg, base + 41 * DAY_MS).state,
            EstateHeartbeatState::ReadyForRelease
        );
    }
}
