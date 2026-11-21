//! Remembered answers to duplicate questions, keyed by drive passport + relative path so they
//! survive rescans and drive-letter changes (spec §7.5).

use super::{DupeGroup, grouped_report, location_identity};
use crate::db::SqliteStore;
use crate::db::actions::propose_dupe_quarantine_pairs;
use crate::db::audit::append_audit_in_tx;
use crate::error::{AegisError, Result};
use crate::{new_id, now_ms};
use rusqlite::{Connection, params};
use serde::{Deserialize, Serialize};
use ts_rs::TS;

/// The owner's answer to "what is this copy?".
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(tag = "choice", rename_all = "snake_case")]
#[ts(export)]
pub enum DupeChoice {
    /// The non-kept copy is a backup: move it onto the Backup drive (or drop it if already there).
    ItsABackup,
    /// Both copies are wanted; never ask again for this pair.
    KeepBoth,
    /// Quarantine the copy at this location.
    DeleteThisCopy { location_index: usize },
    /// Make the copy at this location the Backup drive's copy instead of deleting it.
    MakeThisTheBackupCopy { location_index: usize },
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, TS)]
#[ts(export)]
pub struct DecisionOutcome {
    pub decision_id: String,
    pub batch_id: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, TS)]
#[ts(export)]
pub struct DupeDecisionItem {
    pub id: String,
    /// `backup` | `keep_both` | `delete`
    pub decision: String,
    pub a_rel_path: String,
    pub b_rel_path: String,
    pub a_drive: Option<String>,
    pub b_drive: Option<String>,
    #[ts(type = "number")]
    pub decided_at: i64,
}

fn find_group(conn: &Connection, group_id: &str) -> Result<DupeGroup> {
    grouped_report(conn)?
        .groups
        .into_iter()
        .find(|g| g.group_id == group_id)
        .ok_or_else(|| AegisError::NotFound(format!("duplicate group {group_id}")))
}

fn record(store: &mut SqliteStore, group: &DupeGroup, other: usize, decision: &str) -> Result<String> {
    let a = location_identity(store.conn(), &group.locations[group.keeper_index])?;
    let b = location_identity(store.conn(), &group.locations[other])?;
    let (a, b) = if a <= b { (a, b) } else { (b, a) };
    let (ap, ar) = a.split_once('|').unwrap_or((&a, ""));
    let (bp, br) = b.split_once('|').unwrap_or((&b, ""));
    let id = new_id();
    let now = now_ms();
    store.with_full_sync_tx(|tx| {
        tx.execute(
            "INSERT INTO dupe_decisions (id, a_passport, a_rel_path, b_passport, b_rel_path, decision, decided_at)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7)",
            params![id, ap, ar, bp, br, decision, now],
        )?;
        append_audit_in_tx(
            tx,
            "user",
            "dupe.decided",
            &group.group_id,
            &serde_json::json!({ "decision": decision, "a": a, "b": b }).to_string(),
        )?;
        Ok(())
    })?;
    Ok(id)
}

/// Quarantine pairs `(survivor, member)` for every file at `victim`, each kept by its copy at
/// the group's keeper (or, when the victim *is* the keeper, at the next location).
fn pairs_for(group: &DupeGroup, victim: usize) -> Vec<(String, String)> {
    let keep_at = if victim == group.keeper_index {
        (0..group.locations.len()).find(|&i| i != victim).unwrap_or(victim)
    } else {
        group.keeper_index
    };
    group
        .members
        .iter()
        .filter(|m| m.location_index == victim)
        .filter_map(|m| {
            group
                .members
                .iter()
                .find(|k| k.location_index == keep_at && k.blake3_hex == m.blake3_hex)
                .map(|k| (k.file_id.clone(), m.file_id.clone()))
        })
        .collect()
}

/// Applies the owner's answer to one duplicate group. Deletes and moves become normal batches
/// that still need approval; nothing is removed here.
pub fn decide(store: &mut SqliteStore, group_id: &str, choice: DupeChoice) -> Result<DecisionOutcome> {
    let group = find_group(store.conn(), group_id)?;
    let first_other = (0..group.locations.len())
        .find(|&i| i != group.keeper_index)
        .ok_or_else(|| AegisError::InvalidInput("group has a single location".into()))?;
    let check = |li: usize| {
        if li < group.locations.len() {
            Ok(li)
        } else {
            Err(AegisError::InvalidInput(format!("no location {li} in this group")))
        }
    };
    match choice {
        DupeChoice::KeepBoth => {
            let mut last = String::new();
            for other in (0..group.locations.len()).filter(|&i| i != group.keeper_index) {
                last = record(store, &group, other, "keep_both")?;
            }
            Ok(DecisionOutcome { decision_id: last, batch_id: None })
        }
        DupeChoice::DeleteThisCopy { location_index } => {
            let victim = check(location_index)?;
            let pairs = pairs_for(&group, victim);
            let batch = propose_dupe_quarantine_pairs(store, &pairs)?;
            let id = record(store, &group, if victim == group.keeper_index { first_other } else { victim }, "delete")?;
            Ok(DecisionOutcome { decision_id: id, batch_id: Some(batch) })
        }
        DupeChoice::ItsABackup => backup_choice(store, &group, first_other),
        DupeChoice::MakeThisTheBackupCopy { location_index } => {
            let li = check(location_index)?;
            backup_choice(store, &group, li)
        }
    }
}

fn backup_choice(store: &mut SqliteStore, group: &DupeGroup, location_index: usize) -> Result<DecisionOutcome> {
    let has_backup: bool = store.conn().query_row(
        "SELECT EXISTS(SELECT 1 FROM volumes WHERE role = 'backup')",
        [],
        |r| r.get(0),
    )?;
    if !has_backup {
        return Err(AegisError::InvalidInput("No Backup drive assigned yet.".into()));
    }
    let batch = crate::backup::plan::propose_move_to_backup(store, &group.group_id, location_index)?;
    let other = if location_index == group.keeper_index {
        (0..group.locations.len()).find(|&i| i != location_index).unwrap_or(location_index)
    } else {
        location_index
    };
    let id = record(store, group, other, "backup")?;
    Ok(DecisionOutcome { decision_id: id, batch_id: Some(batch) })
}

/// Withdraws a remembered answer, so the question is asked again. Batches it created are not
/// rolled back here (use rollback for that).
pub fn undo_decision(store: &mut SqliteStore, decision_id: &str) -> Result<()> {
    let n = store.conn().execute(
        "UPDATE dupe_decisions SET undone_at = ?2 WHERE id = ?1 AND undone_at IS NULL",
        params![decision_id, now_ms()],
    )?;
    if n == 0 {
        return Err(AegisError::NotFound(format!("decision {decision_id}")));
    }
    Ok(())
}

pub fn list_decisions(conn: &Connection) -> Result<Vec<DupeDecisionItem>> {
    let mut stmt = conn.prepare(
        "SELECT d.id, d.decision, d.a_rel_path, d.b_rel_path, va.label, vb.label, d.decided_at
         FROM dupe_decisions d
         LEFT JOIN volumes va ON va.passport_id = d.a_passport OR va.id = d.a_passport
         LEFT JOIN volumes vb ON vb.passport_id = d.b_passport OR vb.id = d.b_passport
         WHERE d.undone_at IS NULL ORDER BY d.decided_at DESC",
    )?;
    let rows = stmt.query_map([], |r| {
        Ok(DupeDecisionItem {
            id: r.get(0)?,
            decision: r.get(1)?,
            a_rel_path: r.get(2)?,
            b_rel_path: r.get(3)?,
            a_drive: r.get(4)?,
            b_drive: r.get(5)?,
            decided_at: r.get(6)?,
        })
    })?;
    Ok(rows.collect::<std::result::Result<Vec<_>, _>>()?)
}
