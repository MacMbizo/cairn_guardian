//! Drive roles, the drive plan and moving copies onto the Backup drive (spec §7.6, §7.8.2–3).

use super::{BACKUP_DIR, backup_dest_for, backup_root, rel_under};
use crate::contracts::{ActionType, DriveRole, ProposedActionInput, RefilePlan, TargetKind};
use crate::db::SqliteStore;
use crate::db::actions::propose_batch;
use crate::db::audit::append_audit_in_tx;
use crate::db::files::path_is_within;
use crate::dupes::grouped_report;
use crate::error::{AegisError, Result};
use crate::policy::revoke_standing_for_passport;
use crate::{new_id, now_ms};
use rusqlite::{Connection, params};
use serde::{Deserialize, Serialize};
use std::collections::{HashMap, HashSet};
use std::path::Path;
use ts_rs::TS;

/// Most files moved by one generated batch, so each batch stays reviewable.
pub const MAX_BATCH_FILES: usize = 500;

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, TS)]
#[ts(export)]
pub struct DrivePlanEntry {
    pub volume_id: String,
    pub label: Option<String>,
    pub proposed_role: DriveRole,
    #[ts(type = "number")]
    pub used_bytes: i64,
    #[ts(type = "number")]
    pub total_bytes: i64,
    /// Share of all duplicated bytes that already have a copy on this drive.
    pub backup_copy_share: f64,
    #[ts(type = "number")]
    pub duplicate_bytes: i64,
    pub online: bool,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, TS)]
#[ts(export)]
pub struct DrivePlan {
    pub entries: Vec<DrivePlanEntry>,
    pub recommended_backup: Option<String>,
    /// True once every known drive has a complete catalog.
    pub ready: bool,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, TS)]
#[ts(export)]
pub struct AppliedPlan {
    pub adopt_batch: Option<String>,
    pub stray_batch: Option<String>,
    #[ts(type = "number")]
    pub only_copies_on_backup: i64,
}

#[derive(Debug, Clone)]
struct Copy {
    id: String,
    volume_id: String,
    path: String,
    size: i64,
    mtime: i64,
}

/// Live, fingerprinted files grouped by content, across all volumes.
fn copies_by_hash(conn: &Connection) -> Result<HashMap<Vec<u8>, Vec<Copy>>> {
    let mut stmt = conn.prepare(
        "SELECT id, volume_id, path, size, mtime, blake3 FROM files
         WHERE blake3 IS NOT NULL AND deleted_at IS NULL AND size > 0
           AND state NOT IN ('quarantine','missing') AND container_file_id IS NULL",
    )?;
    let rows = stmt.query_map([], |r| {
        Ok((
            r.get::<_, Vec<u8>>(5)?,
            Copy {
                id: r.get(0)?,
                volume_id: r.get(1)?,
                path: r.get(2)?,
                size: r.get(3)?,
                mtime: r.get(4)?,
            },
        ))
    })?;
    let mut out: HashMap<Vec<u8>, Vec<Copy>> = HashMap::new();
    for r in rows {
        let (h, c) = r?;
        out.entry(h).or_default().push(c);
    }
    Ok(out)
}

/// `(id, label, role, total bytes, online, catalogued, mount point)`.
type ManagedDrive = (String, Option<String>, String, i64, bool, bool, String);

fn managed_drives(conn: &Connection) -> Result<Vec<ManagedDrive>> {
    let mut stmt = conn.prepare(
        "SELECT id, label, role, COALESCE(total_bytes, 0), status = 'online',
                last_catalog_complete_at IS NOT NULL, mount_point
         FROM volumes WHERE passport_id IS NOT NULL OR id LIKE 'fp-%' ORDER BY label, id",
    )?;
    let rows = stmt.query_map([], |r| {
        Ok((r.get(0)?, r.get(1)?, r.get(2)?, r.get(3)?, r.get(4)?, r.get(5)?, r.get(6)?))
    })?;
    Ok(rows.collect::<std::result::Result<Vec<_>, _>>()?)
}

/// Recommends which drive should be the Backup drive: the one already holding the largest
/// share of duplicated content and the most free space, so the least data has to move.
pub fn recommend_plan(conn: &Connection) -> Result<DrivePlan> {
    let drives = managed_drives(conn)?;
    let by_hash = copies_by_hash(conn)?;
    // A drive "holds backup copies" when it has the extra (non-original) copy of something.
    let mut dup_bytes: HashMap<&str, i64> = HashMap::new();
    let mut extra_bytes: HashMap<&str, i64> = HashMap::new();
    let mut total_dup = 0i64;
    for copies in by_hash.values() {
        let mut vols: Vec<&str> = copies.iter().map(|c| c.volume_id.as_str()).collect();
        vols.sort_unstable();
        vols.dedup();
        if vols.len() < 2 {
            continue;
        }
        let size = copies[0].size;
        total_dup += size;
        for v in &vols {
            *dup_bytes.entry(v).or_insert(0) += size;
        }
        let original = copies
            .iter()
            .min_by_key(|c| (c.mtime, looks_like_backup(&c.path), c.path.len()))
            .map(|c| c.volume_id.as_str());
        for v in vols.into_iter().filter(|v| Some(*v) != original) {
            *extra_bytes.entry(v).or_insert(0) += size;
        }
    }
    let mut entries = Vec::new();
    let mut best: Option<(f64, String)> = None;
    for (id, label, role, total, online, catalogued, _) in &drives {
        let used: i64 = conn.query_row(
            "SELECT COALESCE(SUM(size), 0) FROM files WHERE volume_id = ?1 AND deleted_at IS NULL",
            [id],
            |r| r.get(0),
        )?;
        let dups = dup_bytes.get(id.as_str()).copied().unwrap_or(0);
        let extra = extra_bytes.get(id.as_str()).copied().unwrap_or(0);
        let share = if total_dup > 0 { extra as f64 / total_dup as f64 } else { 0.0 };
        let free = if *total > 0 { ((*total - used).max(0)) as f64 / *total as f64 } else { 0.0 };
        let score = 0.6 * share + 0.4 * free;
        if *catalogued && best.as_ref().is_none_or(|(s, _)| score > *s) {
            best = Some((score, id.clone()));
        }
        entries.push(DrivePlanEntry {
            volume_id: id.clone(),
            label: label.clone(),
            proposed_role: DriveRole::from_db(role).unwrap_or(DriveRole::Unassigned),
            used_bytes: used,
            total_bytes: *total,
            backup_copy_share: share,
            duplicate_bytes: dups,
            online: *online,
        });
    }
    let ready = !drives.is_empty() && drives.iter().all(|d| d.5);
    let existing_backup = entries.iter().find(|e| e.proposed_role == DriveRole::Backup).map(|e| e.volume_id.clone());
    let recommended = if ready { existing_backup.or(best.map(|b| b.1)) } else { None };
    for e in entries.iter_mut() {
        if e.proposed_role == DriveRole::Unassigned {
            e.proposed_role = if Some(&e.volume_id) == recommended.as_ref() {
                DriveRole::Backup
            } else {
                DriveRole::General
            };
        }
    }
    Ok(DrivePlan {
        entries,
        recommended_backup: recommended,
        ready,
    })
}

/// Folder names people give copies ("Backup", "Copy of", "old", "bak").
fn looks_like_backup(path: &str) -> bool {
    let p = path.to_lowercase();
    ["backup", "copy", "bak", "\\old", "/old", "archive"].iter().any(|w| p.contains(w))
}

fn in_backup_area(path: &str, mount: &str) -> bool {
    path_is_within(path, &backup_root(Path::new(mount)).to_string_lossy(), false)
}

/// Best copy to keep among `copies`: not on the Backup drive, oldest, shortest path.
fn keeper<'a>(copies: &'a [Copy], backup: &str) -> Option<&'a Copy> {
    copies
        .iter()
        .filter(|c| c.volume_id != backup)
        .min_by_key(|c| (c.mtime, looks_like_backup(&c.path), c.path.len()))
}

fn parent_rel(conn: &Connection, c: &Copy) -> Result<String> {
    let mount: String = conn.query_row("SELECT mount_point FROM volumes WHERE id = ?1", [&c.volume_id], |r| r.get(0))?;
    let parent = Path::new(&c.path).parent().map(|p| p.to_string_lossy().to_string()).unwrap_or_default();
    Ok(rel_under(&parent, &mount))
}

fn backup_evidence(conn: &Connection, keeper: &Copy, backup_id: &str) -> Result<serde_json::Value> {
    Ok(serde_json::json!({
        "backup_copy": {
            "source_file_id": keeper.id,
            "backup_volume_id": backup_id,
            "source_volume_id": keeper.volume_id,
            "source_rel_folder": parent_rel(conn, keeper)?,
        }
    }))
}

/// One file to place on the Backup drive: copy it there, then quarantine the original, or
/// just quarantine it when the Backup drive already holds `existing` (a recorded copy).
struct ToBackup<'a> {
    member: &'a Copy,
    keeper: &'a Copy,
    existing: Option<String>,
    hash: &'a [u8],
}

fn quarantine_input(member: &Copy, reason: &str, evidence: serde_json::Value) -> ProposedActionInput {
    ProposedActionInput {
        agent: Some("Housekeeper".into()),
        action_type: ActionType::Quarantine,
        target_kind: TargetKind::File,
        file_id: Some(member.id.clone()),
        atomic_unit_id: None,
        from_path: member.path.clone(),
        to_path: None,
        reason: reason.into(),
        confidence: 0.99,
        evidence_json: evidence.to_string(),
    }
}

/// Builds one batch that puts the given files on the Backup drive. Per distinct content: one
/// verified copy (unless the Backup drive already holds it), then a quarantine of every listed
/// copy, each kept by that backup copy. Refused when any content would end up with fewer than
/// `MIN_COPIES` copies, counting quarantines already waiting for approval.
fn propose_to_backup(store: &mut SqliteStore, items: &[ToBackup<'_>], backup_id: &str, backup_mount: &str) -> Result<Option<String>> {
    if items.is_empty() {
        return Ok(None);
    }
    let mut by_hash: std::collections::BTreeMap<&[u8], Vec<&ToBackup<'_>>> = Default::default();
    for it in items {
        by_hash.entry(it.hash).or_default().push(it);
    }
    let mut actions = Vec::new();
    let mut survivors: Vec<(String, String)> = Vec::new(); // (member file id, survivor id)
    for (hash, group) in &by_hash {
        let existing = group.iter().find_map(|it| it.existing.clone());
        let pending: i64 = store.conn().query_row(
            "SELECT COUNT(DISTINCT a.file_id) FROM actions a JOIN files f ON f.id = a.file_id
             WHERE a.action_type = 'quarantine' AND a.status IN ('proposed','approved') AND f.blake3 = ?1",
            params![hash],
            |r| r.get(0),
        )?;
        let copies = crate::dupes::copy_count(store.conn(), hash)?;
        let added = i64::from(existing.is_none());
        if copies + added - pending - (group.len() as i64) < crate::dupes::MIN_COPIES {
            return Err(AegisError::PolicyBlocked {
                rule: "min_copies".into(),
                detail: format!(
                    "Aegis keeps at least {} copies of every file; with the deletes already waiting for approval, this would leave fewer.",
                    crate::dupes::MIN_COPIES
                ),
            });
        }
        match existing {
            Some(existing) => {
                for it in group {
                    survivors.push((it.member.id.clone(), existing.clone()));
                    actions.push(quarantine_input(it.member, "The Backup drive already holds this file", serde_json::json!({})));
                }
            }
            None => {
                let members: HashSet<&str> = group.iter().map(|it| it.member.id.as_str()).collect();
                let keeper = group
                    .iter()
                    .map(|it| it.keeper)
                    .find(|k| !members.contains(k.id.as_str()))
                    .unwrap_or(group[0].keeper);
                let source = group[0].member;
                let dest = backup_dest_for(store.conn(), &keeper.volume_id, &keeper.path, backup_mount)?;
                let token = new_id();
                let mut ev = backup_evidence(store.conn(), keeper, backup_id)?;
                ev["pair"] = serde_json::json!(token);
                ev["member_file_id"] = serde_json::json!(source.id);
                actions.push(ProposedActionInput {
                    agent: Some("Housekeeper".into()),
                    action_type: ActionType::Copy,
                    // A copy is not an action *on* a catalogued file: the source is checked
                    // through `member_file_id`, and file ids stay free for the quarantines.
                    target_kind: TargetKind::Dir,
                    file_id: None,
                    atomic_unit_id: None,
                    from_path: source.path.clone(),
                    to_path: Some(dest.to_string_lossy().to_string()),
                    reason: "Place this copy on the Backup drive".into(),
                    confidence: 0.99,
                    evidence_json: ev.to_string(),
                });
                for it in group {
                    actions.push(quarantine_input(
                        it.member,
                        "Now kept on the Backup drive",
                        serde_json::json!({ "after_copy": token }),
                    ));
                }
            }
        }
    }
    let batch = propose_batch(store, &RefilePlan { origin: "backup".into(), actions })?;
    // Copies run before the quarantines that depend on them.
    store.conn().execute(
        "UPDATE actions SET created_at = created_at + 1 WHERE batch_id = ?1 AND action_type = 'quarantine'",
        params![batch],
    )?;
    for (member, survivor) in survivors {
        store.conn().execute(
            "UPDATE actions SET survivor_file_id = ?3 WHERE batch_id = ?1 AND file_id = ?2",
            params![batch, member, survivor],
        )?;
    }
    Ok(Some(batch))
}

/// Sets drive roles (at most one Backup drive) and proposes the batches that put the plan into
/// effect: adopting copies already on the Backup drive, and moving stray copies onto it.
pub fn apply_plan(store: &mut SqliteStore, roles: &[(String, DriveRole)]) -> Result<AppliedPlan> {
    if roles.iter().filter(|(_, r)| *r == DriveRole::Backup).count() > 1 {
        return Err(AegisError::InvalidInput("Choose only one Backup drive.".into()));
    }
    let now = now_ms();
    store.with_full_sync_tx(|tx| {
        for (vid, role) in roles {
            let (old, passport): (String, Option<String>) =
                tx.query_row("SELECT role, passport_id FROM volumes WHERE id = ?1", [vid], |r| Ok((r.get(0)?, r.get(1)?)))?;
            if old == "backup" && *role != DriveRole::Backup
                && let Some(p) = passport
            {
                revoke_standing_for_passport(tx, &p, "role_changed")?;
            }
            if *role == DriveRole::Backup {
                tx.execute("UPDATE volumes SET role = 'general' WHERE role = 'backup' AND id != ?1", [vid])?;
            }
            tx.execute("UPDATE volumes SET role = ?2 WHERE id = ?1", params![vid, role.as_str()])?;
        }
        append_audit_in_tx(
            tx,
            "user",
            "drives.plan_applied",
            "volumes",
            &serde_json::json!({ "roles": roles.iter().map(|(v, r)| (v.clone(), r.as_str())).collect::<Vec<_>>(), "at": now }).to_string(),
        )?;
        Ok(())
    })?;

    let Some((backup_id, backup_mount, _)) = super::backup_volume(store.conn())? else {
        return Ok(AppliedPlan { adopt_batch: None, stray_batch: None, only_copies_on_backup: 0 });
    };
    let by_hash = copies_by_hash(store.conn())?;
    let mut hashes: Vec<&Vec<u8>> = by_hash.keys().collect();
    hashes.sort();

    // Files already on the Backup drive, outside "Aegis Backup".
    let mut adopt: Vec<(Copy, Copy)> = Vec::new(); // (file on backup, keeper elsewhere)
    let mut stray: Vec<(Copy, Copy, Vec<u8>)> = Vec::new(); // (copy to move, keeper, hash)
    // Stray copies only ever come from General drives: PC folders hold the owner's working files.
    let general: HashSet<String> = {
        let mut stmt = store.conn().prepare("SELECT id FROM volumes WHERE role = 'general'")?;
        let rows = stmt.query_map([], |r| r.get::<_, String>(0))?;
        rows.collect::<std::result::Result<_, _>>()?
    };
    for h in &hashes {
        let copies = &by_hash[*h];
        let on_backup: Vec<&Copy> = copies.iter().filter(|c| c.volume_id == backup_id).collect();
        let Some(k) = keeper(copies, &backup_id) else { continue };
        if let Some(b) = on_backup.iter().find(|c| !in_backup_area(&c.path, &backup_mount)) {
            if !on_backup.iter().any(|c| in_backup_area(&c.path, &backup_mount)) {
                adopt.push(((*b).clone(), k.clone()));
            }
            continue;
        }
        if on_backup.is_empty() {
            let others: Vec<&Copy> = copies
                .iter()
                .filter(|c| c.id != k.id && general.contains(&c.volume_id))
                .collect();
            if let Some(m) = others.iter().max_by_key(|c| (c.mtime, c.path.len())) {
                stray.push(((*m).clone(), k.clone(), (*h).clone()));
            }
        }
    }
    let only_copies_on_backup: i64 = {
        let mut stmt = store.conn().prepare(
            "SELECT path, blake3 FROM files WHERE volume_id = ?1 AND deleted_at IS NULL
               AND state NOT IN ('quarantine','missing') AND size > 0",
        )?;
        let rows = stmt.query_map([&backup_id], |r| Ok((r.get::<_, String>(0)?, r.get::<_, Option<Vec<u8>>>(1)?)))?;
        let mut n = 0;
        for r in rows {
            let (path, h) = r?;
            if in_backup_area(&path, &backup_mount) {
                continue;
            }
            let elsewhere = h
                .as_ref()
                .and_then(|h| by_hash.get(h))
                .is_some_and(|cs| cs.iter().any(|c| c.volume_id != backup_id));
            if !elsewhere {
                n += 1;
            }
        }
        n
    };

    adopt.truncate(MAX_BATCH_FILES);
    let adopt_batch = if adopt.is_empty() {
        None
    } else {
        let mut actions = Vec::new();
        for (b, k) in &adopt {
            let dest = backup_dest_for(store.conn(), &k.volume_id, &k.path, &backup_mount)?;
            actions.push(ProposedActionInput {
                agent: Some("Housekeeper".into()),
                action_type: ActionType::Move,
                target_kind: TargetKind::File,
                file_id: Some(b.id.clone()),
                atomic_unit_id: None,
                from_path: b.path.clone(),
                to_path: Some(dest.to_string_lossy().to_string()),
                reason: format!("Already a backup copy; filed under {BACKUP_DIR}"),
                confidence: 0.99,
                evidence_json: backup_evidence(store.conn(), k, &backup_id)?.to_string(),
            });
        }
        Some(propose_batch(store, &RefilePlan { origin: "backup".into(), actions })?)
    };

    stray.truncate(MAX_BATCH_FILES);
    let items: Vec<ToBackup<'_>> = stray
        .iter()
        .map(|(m, k, h)| ToBackup { member: m, keeper: k, existing: None, hash: h })
        .collect();
    let stray_batch = propose_to_backup(store, &items, &backup_id, &backup_mount)?;
    Ok(AppliedPlan { adopt_batch, stray_batch, only_copies_on_backup })
}

/// "It's a backup" / "Make this the backup copy": puts the copies at `location_index` of a
/// duplicate group onto the Backup drive, then quarantines them where they are. Needs approval.
pub fn propose_move_to_backup(store: &mut SqliteStore, group_id: &str, location_index: usize) -> Result<String> {
    let (backup_id, backup_mount, online) = super::backup_volume(store.conn())?
        .ok_or_else(|| AegisError::InvalidInput("No Backup drive assigned yet.".into()))?;
    if !online {
        return Err(AegisError::VolumeOffline("Plug in the Backup drive first.".into()));
    }
    let group = grouped_report(store.conn())?
        .groups
        .into_iter()
        .find(|g| g.group_id == group_id)
        .ok_or_else(|| AegisError::NotFound(format!("duplicate group {group_id}")))?;
    if location_index >= group.locations.len() {
        return Err(AegisError::InvalidInput(format!("no location {location_index} in this group")));
    }
    let by_hash = copies_by_hash(store.conn())?;
    let by_id: HashMap<String, (&Vec<u8>, &Copy)> = by_hash
        .iter()
        .flat_map(|(h, cs)| cs.iter().map(move |c| (c.id.clone(), (h, c))))
        .collect();
    let mut items = Vec::new();
    let here: HashSet<&str> = group
        .members
        .iter()
        .filter(|m| m.location_index == location_index)
        .map(|m| m.file_id.as_str())
        .collect();
    for m in group.members.iter().filter(|m| m.location_index == location_index) {
        let Some((h, member)) = by_id.get(&m.file_id) else { continue };
        let copies = &by_hash[*h];
        let others: Vec<Copy> = copies.iter().filter(|c| !here.contains(c.id.as_str())).cloned().collect();
        let Some(k) = keeper(&others, &backup_id) else { continue };
        let existing = copies
            .iter()
            .find(|c| c.volume_id == backup_id && in_backup_area(&c.path, &backup_mount))
            .map(|c| c.id.clone());
        let k = copies.iter().find(|c| c.id == k.id).unwrap_or(member);
        items.push(ToBackup { member, keeper: k, existing, hash: h.as_slice() });
    }
    propose_to_backup(store, &items, &backup_id, &backup_mount)?
        .ok_or_else(|| AegisError::InvalidInput("Nothing to move for this location.".into()))
}
