//! Migration batches (spec §8): the plan is cut into approvable batches of at most 500 moves,
//! one folder and destination at a time, one batch per scope at a time. After a batch runs,
//! Aegis remembers where every file used to be, re-indexes moved folders, tidies up folders it
//! emptied, and teaches the rules what the owner approved or turned down.

use super::blueprint::{Placement, Target, plan, scope_roots};
use super::rules::{Outcome, log_decision, record_outcome};
use super::{Scope, blueprint_of};
use crate::contracts::{ActionType, ProposedActionInput, RefilePlan, TargetKind};
use crate::db::SqliteStore;
use crate::db::actions::propose_batch;
use crate::db::files::path_is_within;
use crate::error::Result;
use crate::{new_id, now_ms};
use rusqlite::{Connection, OptionalExtension, params};
use std::collections::{BTreeMap, HashSet};
use std::path::{Path, PathBuf};

pub const BATCH_MAX: usize = 500;
pub const AGENT: &str = "filing";

pub fn origin_for(scope: &Scope) -> String {
    format!("filing:{}", scope.key())
}

fn scope_of_origin(origin: &str) -> Option<Scope> {
    let key = origin.strip_prefix("filing:").or_else(|| origin.strip_prefix("filing-auto:"))?;
    Some(if key == "pc" { Scope::Pc } else { Scope::Volume { volume_id: key.to_string() } })
}

/// The scope's filing batch that is waiting for the owner (or approved and not yet run).
pub fn open_filing_batch(conn: &Connection, scope: &Scope) -> Result<Option<String>> {
    Ok(conn
        .query_row(
            "SELECT id FROM action_batches WHERE origin = ?1 AND status IN ('draft', 'approved')
             ORDER BY created_at DESC LIMIT 1",
            [origin_for(scope)],
            |r| r.get(0),
        )
        .optional()?)
}

/// Paths the owner turned down or undid: never proposed again.
fn declined(conn: &Connection) -> Result<HashSet<String>> {
    let mut stmt = conn.prepare(
        "SELECT lower(from_path) FROM actions WHERE agent = ?1 AND status IN ('rejected', 'rolled_back')",
    )?;
    Ok(stmt.query_map([AGENT], |r| r.get(0))?.collect::<rusqlite::Result<_>>()?)
}

/// Files that already have a move waiting somewhere (any feature): one at a time per file.
fn busy_files(conn: &Connection) -> Result<HashSet<String>> {
    let mut stmt =
        conn.prepare("SELECT file_id FROM actions WHERE file_id IS NOT NULL AND status IN ('proposed', 'approved')")?;
    Ok(stmt.query_map([], |r| r.get(0))?.collect::<rusqlite::Result<_>>()?)
}

/// `name (2).ext`, `name (3).ext`, … until it is unused in this batch.
fn unique_in_batch(to: &str, taken: &mut HashSet<String>) -> String {
    if taken.insert(to.to_lowercase()) {
        return to.to_string();
    }
    let p = Path::new(to);
    let dir = p.parent().map(Path::to_path_buf).unwrap_or_default();
    let stem = p.file_stem().map(|s| s.to_string_lossy().to_string()).unwrap_or_default();
    let ext = p.extension().map(|e| format!(".{}", e.to_string_lossy())).unwrap_or_default();
    for n in 2.. {
        let cand = dir.join(format!("{stem} ({n}){ext}")).to_string_lossy().to_string();
        if taken.insert(cand.to_lowercase()) {
            return cand;
        }
    }
    unreachable!()
}

fn root_of<'a>(roots: &'a [String], path: &str) -> Option<&'a String> {
    roots.iter().filter(|r| path_is_within(path, r, false)).max_by_key(|r| r.len())
}

/// The next batch for a scope, or `None` when there is no blueprint, a batch is already
/// waiting, or nothing is left to move.
pub fn next_batch(store: &mut SqliteStore, scope: &Scope) -> Result<Option<String>> {
    let Some(choice) = blueprint_of(store.conn(), scope)? else { return Ok(None) };
    if open_filing_batch(store.conn(), scope)?.is_some() {
        return Ok(None);
    }
    let roots: Vec<String> = scope_roots(store, scope)?.into_iter().map(|(p, _)| p).collect();
    let no = declined(store.conn())?;
    let busy = busy_files(store.conn())?;
    let moves: Vec<Placement> = plan(store, scope, choice)?
        .into_iter()
        .filter(|p| p.to.is_some() && !p.to_sort && !no.contains(&p.from.to_lowercase()))
        .filter(|p| match &p.target {
            Target::File { file_id } => !busy.contains(file_id),
            Target::Group { .. } => true,
        })
        .collect();
    if moves.is_empty() {
        return Ok(None);
    }
    // Group by (source top folder, destination area) and pick the first group in order:
    // loose items, then download dumps, then the biggest folders.
    let mut groups: BTreeMap<(String, String), (i64, Vec<Placement>)> = BTreeMap::new();
    for p in moves {
        let Some(root) = root_of(&roots, &p.from) else { continue };
        let rel = Path::new(&p.from).strip_prefix(root).map(Path::to_path_buf).unwrap_or_default();
        let src_top = if rel.components().count() <= 1 {
            String::new()
        } else {
            rel.components().next().map(|c| c.as_os_str().to_string_lossy().to_string()).unwrap_or_default()
        };
        let to = p.to.clone().unwrap_or_default();
        let dest_area = root_of(&roots, &to)
            .and_then(|r| Path::new(&to).strip_prefix(r).ok())
            .map(|rel| rel.components().take(2).map(|c| c.as_os_str().to_string_lossy().to_string()).collect::<Vec<_>>().join("\\"))
            .unwrap_or_default();
        let e = groups.entry((src_top, dest_area)).or_default();
        e.0 += p.bytes;
        e.1.push(p);
    }
    let mut ordered: Vec<((String, String), (i64, Vec<Placement>))> = groups.into_iter().collect();
    ordered.sort_by_key(|((top, _), (bytes, _))| {
        let rank = if top.is_empty() {
            0
        } else if top.to_lowercase().contains("download") {
            1
        } else {
            2
        };
        (rank, std::cmp::Reverse(*bytes))
    });
    let (_, (_, mut picked)) = ordered.into_iter().next().expect("non-empty");
    picked.truncate(BATCH_MAX);

    let mut taken = HashSet::new();
    let actions: Vec<ProposedActionInput> = picked
        .into_iter()
        .map(|p| {
            let to = unique_in_batch(p.to.as_deref().unwrap_or_default(), &mut taken);
            let dest_template = root_of(&roots, &to)
                .and_then(|r| Path::new(&to).parent()?.strip_prefix(r).ok().map(|x| x.to_string_lossy().to_string()))
                .unwrap_or_default();
            let (target_kind, file_id) = match &p.target {
                Target::File { file_id } => (TargetKind::File, Some(file_id.clone())),
                Target::Group { .. } => (TargetKind::Dir, None),
            };
            ProposedActionInput {
                agent: Some(AGENT.into()),
                action_type: ActionType::Move,
                target_kind,
                file_id,
                atomic_unit_id: None,
                from_path: p.from.clone(),
                to_path: Some(to),
                reason: p.reason.clone(),
                confidence: p.confidence,
                evidence_json: serde_json::json!({
                    "rule_key": p.rule_key,
                    "new_name": p.new_name,
                    "dest_template": dest_template,
                    "scope": scope.key(),
                })
                .to_string(),
            }
        })
        .collect();
    let batch = propose_batch(store, &RefilePlan { origin: origin_for(scope), actions })?;
    Ok(Some(batch))
}

struct ActionRow {
    id: String,
    status: String,
    file_id: Option<String>,
    target_kind: String,
    from: String,
    to: Option<String>,
    evidence: serde_json::Value,
}

fn batch_rows(conn: &Connection, batch_id: &str) -> Result<(String, Vec<ActionRow>)> {
    let origin: String = conn.query_row("SELECT origin FROM action_batches WHERE id = ?1", [batch_id], |r| r.get(0))?;
    let mut stmt = conn.prepare(
        "SELECT id, status, file_id, target_kind, from_path, to_path, evidence_json FROM actions WHERE batch_id = ?1",
    )?;
    let rows = stmt
        .query_map([batch_id], |r| {
            let ev: String = r.get(6)?;
            Ok(ActionRow {
                id: r.get(0)?,
                status: r.get(1)?,
                file_id: r.get(2)?,
                target_kind: r.get(3)?,
                from: r.get(4)?,
                to: r.get(5)?,
                evidence: serde_json::from_str(&ev).unwrap_or_default(),
            })
        })?
        .collect::<rusqlite::Result<_>>()?;
    Ok((origin, rows))
}

fn history(conn: &Connection, file_id: Option<&str>, old: &str, new: &str, batch_id: &str, now: i64) -> Result<()> {
    conn.execute(
        "INSERT INTO file_history (id, file_id, old_path, new_path, batch_id, changed_at) VALUES (?1, ?2, ?3, ?4, ?5, ?6)",
        params![new_id(), file_id, old, new, batch_id, now],
    )?;
    Ok(())
}

fn learn(conn: &Connection, scope_key: &str, a: &ActionRow, batch_id: &str, outcome: Outcome, now: i64) -> Result<()> {
    let Some(rule_key) = a.evidence.get("rule_key").and_then(|v| v.as_str()) else { return Ok(()) };
    let dest = a.evidence.get("dest_template").and_then(|v| v.as_str()).unwrap_or_default();
    let rename = a.evidence.get("new_name").is_some_and(|v| !v.is_null());
    record_outcome(conn, scope_key, rule_key, dest, rename, outcome, now)?;
    log_decision(conn, rule_key, a.file_id.as_deref(), batch_id, outcome, now)
}

/// Files under `dir` as `(id, path, volume_id)`.
fn files_under(conn: &Connection, dir: &str) -> Result<Vec<(String, String, String)>> {
    let mut stmt = conn.prepare(
        "SELECT id, path, volume_id FROM files WHERE deleted_at IS NULL AND lower(substr(path, 1, ?2)) = lower(?1)",
    )?;
    let rows: Vec<(String, String, String)> = stmt
        .query_map(params![dir, dir.chars().count() as i64], |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?)))?
        .collect::<rusqlite::Result<_>>()?;
    Ok(rows.into_iter().filter(|(_, p, _)| path_is_within(p, dir, false)).collect())
}

/// A moved folder: re-index its new place first (ids follow the files), then the old one.
fn reindex_moved_dir(store: &mut SqliteStore, volume_id: &str, from: &str, to: &str) -> Result<()> {
    crate::engine::batcher::index_changed(store, volume_id, vec![vec![PathBuf::from(to)]])?;
    crate::engine::batcher::index_changed(store, volume_id, vec![vec![PathBuf::from(from)]])?;
    Ok(())
}

/// Removes folders this batch left empty, walking up from where files used to be.
fn remove_emptied(roots: &[String], from_paths: &[String]) {
    let mut dirs: Vec<PathBuf> = from_paths.iter().filter_map(|p| Path::new(p).parent().map(Path::to_path_buf)).collect();
    dirs.sort_by_key(|d| std::cmp::Reverse(d.components().count()));
    dirs.dedup();
    for d in dirs {
        let mut cur = d;
        loop {
            let s = cur.to_string_lossy().to_string();
            let at_root = roots.iter().any(|r| s.eq_ignore_ascii_case(r));
            if at_root || !roots.iter().any(|r| path_is_within(&s, r, false)) {
                break;
            }
            let empty = std::fs::read_dir(&cur).map(|mut it| it.next().is_none()).unwrap_or(false);
            if !empty || std::fs::remove_dir(&cur).is_err() {
                break;
            }
            match cur.parent() {
                Some(p) => cur = p.to_path_buf(),
                None => break,
            }
        }
    }
}

/// After a filing batch ran: history, re-indexing, empty folders and learning.
pub fn after_execute(store: &mut SqliteStore, batch_id: &str) -> Result<()> {
    let (origin, rows) = batch_rows(store.conn(), batch_id)?;
    let Some(scope) = scope_of_origin(&origin) else { return Ok(()) };
    let scope_key = scope.key();
    let roots: Vec<String> = scope_roots(store, &scope).map(|r| r.into_iter().map(|(p, _)| p).collect()).unwrap_or_default();
    let now = now_ms();
    let mut moved_from = Vec::new();
    for a in &rows {
        match a.status.as_str() {
            "executed" => {
                if a.target_kind == "dir" {
                    let to = a.to.clone().unwrap_or_default();
                    let children = files_under(store.conn(), &a.from)?;
                    if let Some((_, _, vol)) = children.first().cloned() {
                        reindex_moved_dir(store, &vol, &a.from, &to)?;
                    }
                    for (id, old, _) in children {
                        let new: Option<String> = store
                            .conn()
                            .query_row("SELECT path FROM files WHERE id = ?1 AND deleted_at IS NULL", [&id], |r| r.get(0))
                            .optional()?;
                        if let Some(new) = new {
                            history(store.conn(), Some(&id), &old, &new, batch_id, now)?;
                        }
                    }
                } else if let Some(fid) = &a.file_id {
                    let new: String = store.conn().query_row("SELECT path FROM files WHERE id = ?1", [fid], |r| r.get(0))?;
                    history(store.conn(), Some(fid), &a.from, &new, batch_id, now)?;
                }
                moved_from.push(a.from.clone());
                learn(store.conn(), &scope_key, a, batch_id, Outcome::Approved, now)?;
            }
            "proposed" => {
                // Unticked: the owner wants it left where it is.
                store.conn().execute("UPDATE actions SET status = 'rejected' WHERE id = ?1", [&a.id])?;
                learn(store.conn(), &scope_key, a, batch_id, Outcome::Rejected, now)?;
            }
            _ => {}
        }
    }
    remove_emptied(&roots, &moved_from);
    Ok(())
}

/// After a filing batch was undone: forget its history, re-index moved folders, and count the
/// undo against the rules involved.
pub fn after_rollback(store: &mut SqliteStore, batch_id: &str) -> Result<()> {
    let (origin, rows) = batch_rows(store.conn(), batch_id)?;
    let Some(scope) = scope_of_origin(&origin) else { return Ok(()) };
    let now = now_ms();
    for a in rows.iter().filter(|a| a.status == "rolled_back") {
        if a.target_kind == "dir"
            && let Some(to) = &a.to
        {
            let children = files_under(store.conn(), to)?;
            if let Some((_, _, vol)) = children.first().cloned() {
                reindex_moved_dir(store, &vol, to, &a.from)?;
            }
        }
        learn(store.conn(), &scope.key(), a, batch_id, Outcome::Undone, now)?;
    }
    store.conn().execute("DELETE FROM file_history WHERE batch_id = ?1", [batch_id])?;
    Ok(())
}
