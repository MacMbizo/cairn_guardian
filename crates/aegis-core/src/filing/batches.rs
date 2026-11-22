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

/// (source top folder, destination area).
type GroupKey = (String, String);
pub const AGENT: &str = "filing";

pub fn origin_for(scope: &Scope) -> String {
    format!("filing:{}", scope.key())
}

fn scope_of_origin(origin: &str) -> Option<Scope> {
    let key = origin.strip_prefix("filing:").or_else(|| origin.strip_prefix("filing-auto:"))?;
    Some(if key == "pc" { Scope::Pc } else { Scope::Volume { volume_id: key.to_string() } })
}

/// One item a batch couldn't move, and why, in plain words.
#[derive(Debug, Clone, PartialEq, serde::Serialize, serde::Deserialize, ts_rs::TS)]
#[ts(export)]
pub struct FailedItem {
    /// The path it was to move from.
    pub name: String,
    pub reason: String,
}

/// What a filing batch did: filed, left alone (unticked or held), and failed with reasons.
#[derive(Debug, Clone, PartialEq, serde::Serialize, serde::Deserialize, ts_rs::TS)]
#[ts(export)]
pub struct RunReport {
    pub batch_id: String,
    #[ts(type = "number")]
    pub filed: i64,
    #[ts(type = "number")]
    pub skipped: i64,
    pub failed: Vec<FailedItem>,
}

/// An execute error, said the way the owner would put it.
pub fn plain_reason(raw: &str) -> String {
    let l = raw.to_lowercase();
    let has = |s: &str| l.contains(s);
    if has("atomic_unit_boundary") {
        "It's inside a code project, which is kept whole".into()
    } else if has("outside_configured_roots") {
        "It's outside the folders Aegis looks after".into()
    } else if has("protected_aegis_internal") {
        "It's one of Aegis's own files".into()
    } else if has("isn't plugged in") || has("offline") || has("isn't") && has("any more") {
        "The drive was unplugged".into()
    } else if has("moved since") || has("not found") || has("not_found") || has("cannot find") || has("no longer") || has("os error 2") || has("os error 3") {
        "It was moved or deleted since".into()
    } else if has("being used by another process") || has("os error 32") {
        "Another program has it open".into()
    } else if has("not enough space") || has("os error 112") {
        "There isn't enough space on the drive".into()
    } else {
        format!("Windows refused ({raw})")
    }
}

/// Plain-words summary of an executed filing batch.
pub fn run_report(res: &crate::contracts::ExecuteResult) -> RunReport {
    use crate::contracts::ActionStatus;
    RunReport {
        batch_id: res.batch_id.clone(),
        filed: res.executed_count as i64,
        skipped: res.skipped_count as i64,
        failed: res
            .actions
            .iter()
            .filter(|a| a.status == ActionStatus::Failed)
            .map(|a| FailedItem { name: a.from_path.clone(), reason: plain_reason(a.error_message.as_deref().unwrap_or("")) })
            .collect(),
    }
}

/// A filing batch that has run, for the "Recent batches" list.
#[derive(Debug, Clone, PartialEq, serde::Serialize, serde::Deserialize, ts_rs::TS)]
#[ts(export)]
pub struct RecentBatch {
    pub id: String,
    pub scope: Scope,
    /// Filed by a learned rule, without asking.
    pub automatic: bool,
    #[ts(type = "number")]
    pub at: i64,
    /// Files and folders it moved (including any since undone).
    #[ts(type = "number")]
    pub files: i64,
    /// Everything it moved has been put back.
    pub undone: bool,
    /// Something it moved is still in its new place.
    pub can_undo: bool,
}

/// Filing batches that moved something, newest first, manual and automatic.
pub fn recent_batches(conn: &Connection, limit: usize) -> Result<Vec<RecentBatch>> {
    let mut stmt = conn.prepare(
        "SELECT b.id, b.origin, COALESCE(b.executed_at, b.created_at),
                SUM(a.status = 'executed'), SUM(a.status = 'rolled_back')
         FROM action_batches b JOIN actions a ON a.batch_id = b.id
         WHERE b.origin LIKE 'filing:%' OR b.origin LIKE 'filing-auto:%'
         GROUP BY b.id HAVING SUM(a.status IN ('executed', 'rolled_back')) > 0
         ORDER BY 3 DESC LIMIT ?1",
    )?;
    let rows = stmt.query_map([limit as i64], |r| {
        Ok((r.get::<_, String>(0)?, r.get::<_, String>(1)?, r.get::<_, i64>(2)?, r.get::<_, i64>(3)?, r.get::<_, i64>(4)?))
    })?;
    let mut out = Vec::new();
    for row in rows {
        let (id, origin, at, executed, rolled_back) = row?;
        let Some(scope) = scope_of_origin(&origin) else { continue };
        out.push(RecentBatch {
            id,
            scope,
            automatic: origin.starts_with("filing-auto:"),
            at,
            files: executed + rolled_back,
            undone: executed == 0,
            can_undo: executed > 0,
        });
    }
    Ok(out)
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

/// Paths never proposed again: the owner turned them down or undid them, the executor held
/// them back, or the move failed for a reason that won't go away by itself. Moves that failed
/// because a drive was unplugged or a file had moved on are offered again later.
fn declined(conn: &Connection) -> Result<HashSet<String>> {
    let mut stmt = conn.prepare(
        "SELECT lower(from_path) FROM actions WHERE agent = ?1 AND (
            status IN ('rejected', 'rolled_back', 'skipped')
            OR (status = 'failed' AND NOT (
                lower(COALESCE(error_message, '')) LIKE '%outside_configured_roots%'
                OR lower(COALESCE(error_message, '')) LIKE '%not found%'
                OR lower(COALESCE(error_message, '')) LIKE '%cannot find%'
                OR lower(COALESCE(error_message, '')) LIKE '%offline%'
                OR lower(COALESCE(error_message, '')) LIKE '%no longer%')))",
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

/// A planned move Aegis would still offer: it goes somewhere, is sure, and wasn't turned down.
fn offerable(p: &Placement, declined: &HashSet<String>) -> bool {
    p.to.is_some() && !p.to_sort && !declined.contains(&p.from.to_lowercase())
}

/// `(files still to move, files to sort)` from a plan, leaving out what the owner turned down.
/// Files already in a waiting batch still count: they haven't moved yet.
fn counts_of(all: &[Placement], declined: &HashSet<String>) -> (i64, i64) {
    let to_sort = all.iter().filter(|p| p.to_sort).count() as i64;
    let moving = all
        .iter()
        .filter(|p| offerable(p, declined))
        .map(|p| match &p.target {
            Target::Group { files, .. } => *files as i64,
            Target::File { .. } => 1,
        })
        .sum();
    (moving, to_sort)
}

/// Plans `scope` afresh and caches its counts (see [`counts_of`]).
pub fn recount(store: &SqliteStore, scope: &Scope, choice: super::BlueprintChoice) -> Result<(i64, i64)> {
    let counts = counts_of(&plan(store, scope, choice)?, &declined(store.conn())?);
    super::set_counts(store.conn(), scope, counts.0, counts.1)?;
    Ok(counts)
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
    let all = plan(store, scope, choice)?;
    let (moving, to_sort) = counts_of(&all, &no);
    super::set_counts(store.conn(), scope, moving, to_sort)?;
    let moves: Vec<Placement> = all
        .into_iter()
        .filter(|p| offerable(p, &no))
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
    let mut ordered: Vec<(GroupKey, (i64, Vec<Placement>))> = groups.into_iter().collect();
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
    Ok(Some(propose_placements(store, scope, &origin_for(scope), picked)?))
}

/// Proposes `placements` as one filing batch under `origin`. Destinations that would clash
/// within the batch get " (2)", " (3)" and so on.
pub fn propose_placements(store: &mut SqliteStore, scope: &Scope, origin: &str, placements: Vec<Placement>) -> Result<String> {
    let roots: Vec<String> = scope_roots(store, scope)?.into_iter().map(|(p, _)| p).collect();
    let mut taken = HashSet::new();
    let actions: Vec<ProposedActionInput> = placements
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
    propose_batch(store, &RefilePlan { origin: origin.to_string(), actions })
}

/// Paths the owner turned down or undid, lowercase (shared with the filing watch).
pub fn declined_paths(conn: &Connection) -> Result<HashSet<String>> {
    declined(conn)
}

/// Files with a move waiting anywhere (shared with the filing watch).
pub fn busy_file_ids(conn: &Connection) -> Result<HashSet<String>> {
    busy_files(conn)
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

/// What one batch taught: each rule counts once per batch (one decision by the owner), with
/// approvals applied before rejections so a mixed answer ends at "asking".
#[derive(Default)]
struct Lessons {
    approved: BTreeMap<String, (String, bool)>,
    rejected: BTreeMap<String, (String, bool)>,
}

impl Lessons {
    fn note(&mut self, conn: &Connection, a: &ActionRow, batch_id: &str, outcome: Outcome, now: i64) -> Result<()> {
        let Some(rule_key) = a.evidence.get("rule_key").and_then(|v| v.as_str()) else { return Ok(()) };
        let dest = a.evidence.get("dest_template").and_then(|v| v.as_str()).unwrap_or_default().to_string();
        let rename = a.evidence.get("new_name").is_some_and(|v| !v.is_null());
        let bucket = if outcome == Outcome::Approved { &mut self.approved } else { &mut self.rejected };
        bucket.entry(rule_key.to_string()).or_insert((dest, rename));
        log_decision(conn, rule_key, a.file_id.as_deref(), batch_id, outcome, now)
    }

    fn apply(self, conn: &Connection, scope_key: &str, rejected_as: Outcome, now: i64) -> Result<()> {
        for (key, (dest, rename)) in self.approved {
            record_outcome(conn, scope_key, &key, &dest, rename, Outcome::Approved, now)?;
        }
        for (key, (dest, rename)) in self.rejected {
            record_outcome(conn, scope_key, &key, &dest, rename, rejected_as, now)?;
        }
        Ok(())
    }
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
    let mut moved_files = 0i64;
    let mut lessons = Lessons::default();
    for a in &rows {
        match a.status.as_str() {
            "executed" => {
                if a.target_kind == "dir" {
                    let to = a.to.clone().unwrap_or_default();
                    let children = files_under(store.conn(), &a.from)?;
                    moved_files += children.len() as i64;
                    // Where each file went, worked out from the folder move itself, so it is
                    // recorded even if the drive goes away before re-indexing.
                    for (id, old, _) in &children {
                        let rest = Path::new(old).strip_prefix(&a.from).map(Path::to_path_buf).unwrap_or_default();
                        let new = Path::new(&to).join(rest).to_string_lossy().to_string();
                        history(store.conn(), Some(id), old, &new, batch_id, now)?;
                    }
                    // Re-index only while the folder is really there; otherwise the live index
                    // catches up when the drive is back (never forget files that still exist).
                    if Path::new(&to).is_dir()
                        && let Some((_, _, vol)) = children.first().cloned()
                    {
                        reindex_moved_dir(store, &vol, &a.from, &to)?;
                    }
                } else if let Some(fid) = &a.file_id {
                    let new: String = store.conn().query_row("SELECT path FROM files WHERE id = ?1", [fid], |r| r.get(0))?;
                    history(store.conn(), Some(fid), &a.from, &new, batch_id, now)?;
                    moved_files += 1;
                }
                moved_from.push(a.from.clone());
                lessons.note(store.conn(), a, batch_id, Outcome::Approved, now)?;
            }
            "proposed" => {
                // Unticked: the owner wants it left where it is.
                store.conn().execute("UPDATE actions SET status = 'rejected' WHERE id = ?1", [&a.id])?;
                lessons.note(store.conn(), a, batch_id, Outcome::Rejected, now)?;
            }
            _ => {}
        }
    }
    lessons.apply(store.conn(), &scope_key, Outcome::Rejected, now)?;
    remove_emptied(&roots, &moved_from);
    if let Some((remaining, to_sort)) = super::cached_counts(store.conn(), &scope)? {
        super::set_counts(store.conn(), &scope, remaining - moved_files, to_sort)?;
    }
    Ok(())
}

/// After a filing batch was undone: forget its history, re-index moved folders, and count the
/// undo against the rules involved.
pub fn after_rollback(store: &mut SqliteStore, batch_id: &str) -> Result<()> {
    let (origin, rows) = batch_rows(store.conn(), batch_id)?;
    let Some(scope) = scope_of_origin(&origin) else { return Ok(()) };
    let now = now_ms();
    let mut lessons = Lessons::default();
    for a in rows.iter().filter(|a| a.status == "rolled_back") {
        if a.target_kind == "dir"
            && let Some(to) = &a.to
        {
            let children = files_under(store.conn(), to)?;
            if let Some((_, _, vol)) = children.first().cloned() {
                reindex_moved_dir(store, &vol, to, &a.from)?;
            }
        }
        lessons.note(store.conn(), a, batch_id, Outcome::Undone, now)?;
    }
    lessons.apply(store.conn(), &scope.key(), Outcome::Undone, now)?;
    super::clear_counts(store.conn(), &scope)?;
    // Forget "was" records only for files that really are back where they were; anything an
    // undo couldn't move back (a conflict) keeps its record.
    store.conn().execute(
        "DELETE FROM file_history WHERE batch_id = ?1 AND EXISTS (
            SELECT 1 FROM files f WHERE f.id = file_history.file_id AND lower(f.path) = lower(file_history.old_path))",
        [batch_id],
    )?;
    Ok(())
}

/// After a filing batch was turned down (Skip, or rejected in Simulator & Actions): its rules
/// go back to asking, as one decision per rule (policy C4 section 7).
pub fn after_reject(store: &mut SqliteStore, batch_id: &str) -> Result<()> {
    let (origin, rows) = batch_rows(store.conn(), batch_id)?;
    let Some(scope) = scope_of_origin(&origin) else { return Ok(()) };
    let now = now_ms();
    let mut lessons = Lessons::default();
    for a in rows.iter().filter(|a| a.status == "rejected") {
        lessons.note(store.conn(), a, batch_id, Outcome::Rejected, now)?;
    }
    lessons.apply(store.conn(), &scope.key(), Outcome::Rejected, now)?;
    super::clear_counts(store.conn(), &scope)
}

/// "Skip this folder": turns the batch down, never offers those files again, and counts as a
/// "no" for the rules involved.
pub fn skip_batch(store: &mut SqliteStore, batch_id: &str) -> Result<()> {
    crate::db::actions::reject_batch(store, batch_id, Some("Skipped from Filing"))?;
    after_reject(store, batch_id)
}
