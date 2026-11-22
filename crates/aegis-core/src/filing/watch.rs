//! Ongoing filing (spec §10): new files that settle in an "inbox" folder (Downloads, Desktop,
//! the top of Pictures, a drive's root) are filed by rules the owner has let act on their own,
//! or proposed in a batch for the owner to approve.

use super::batches::{after_execute, busy_file_ids, declined_paths, open_filing_batch, propose_placements};
use super::blueprint::{KnownFolder, PROPOSE_AT, Placement, Target, plan, scope_roots};
use super::rules::automatic_rule_for;
use super::{Scope, blueprint_of};
use crate::db::SqliteStore;
use crate::db::actions::{approve_batch, execute_batch};
use crate::engine::{Finding, FindingAction, Level, Screen, Watch};
use crate::error::Result;
use std::path::Path;

/// A file must be unchanged this long before it is filed.
pub const SETTLE_MS: i64 = 600_000;
/// Files Aegis first saw less than this long ago are left alone.
pub const MIN_AGE_MS: i64 = 120_000;
/// Waiting items that turn the weekly-note line into a reminder.
pub const REMIND_AT: i64 = 20;
/// Only rules this sure act by themselves.
pub const AUTO_AT: f64 = 0.90;

const PARTIAL: [&str; 5] = ["crdownload", "part", "partial", "download", "tmp"];

/// Scopes with a chosen blueprint and the folders whose loose files count as new arrivals.
fn inboxes(store: &SqliteStore) -> Result<Vec<(Scope, Vec<String>)>> {
    let mut out = Vec::new();
    if blueprint_of(store.conn(), &Scope::Pc)?.is_some() {
        let dirs: Vec<String> = scope_roots(store, &Scope::Pc)?
            .into_iter()
            .filter(|(_, k)| matches!(k, Some(KnownFolder::Downloads | KnownFolder::Desktop | KnownFolder::Pictures)))
            .map(|(p, _)| p)
            .collect();
        out.push((Scope::Pc, dirs));
    }
    let vols: Vec<(String, String)> = {
        let mut stmt = store.conn().prepare(
            "SELECT v.id, v.mount_point FROM volumes v JOIN filing_blueprints b ON b.scope_key = v.id
             WHERE v.status = 'online' AND COALESCE(v.role, '') != 'backup'",
        )?;
        stmt.query_map([], |r| Ok((r.get(0)?, r.get(1)?)))?.collect::<rusqlite::Result<_>>()?
    };
    for (id, mount) in vols {
        out.push((Scope::Volume { volume_id: id }, vec![mount]));
    }
    Ok(out)
}

/// The owner has approved at least one filing batch in this scope.
fn approved_once(store: &SqliteStore, scope: &Scope) -> Result<bool> {
    Ok(store.conn().query_row(
        "SELECT EXISTS (SELECT 1 FROM action_batches WHERE origin IN (?1, ?2) AND status IN ('executed', 'partial'))",
        [format!("filing:{}", scope.key()), format!("filing-auto:{}", scope.key())],
        |r| r.get(0),
    )?)
}

/// Settled, classified files sitting loose in one of `dirs`.
fn arrivals(store: &SqliteStore, dirs: &[String], now: i64) -> Result<Vec<String>> {
    let mut stmt = store.conn().prepare(
        "SELECT f.id, f.path, COALESCE(f.ext, '') FROM files f JOIN file_meta m ON m.file_id = f.id
         WHERE f.deleted_at IS NULL AND f.state NOT IN ('quarantine', 'missing') AND f.cloud_state = 'local'
           AND f.atomic_unit_id IS NULL AND f.mtime <= ?1 AND f.first_seen <= ?2",
    )?;
    let rows: Vec<(String, String, String)> = stmt
        .query_map([now - SETTLE_MS, now - MIN_AGE_MS], |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?)))?
        .collect::<rusqlite::Result<_>>()?;
    let no = declined_paths(store.conn())?;
    let busy = busy_file_ids(store.conn())?;
    Ok(rows
        .into_iter()
        .filter(|(id, path, ext)| {
            let parent = Path::new(path).parent().map(|p| p.to_string_lossy().to_string()).unwrap_or_default();
            dirs.iter().any(|d| d.eq_ignore_ascii_case(&parent))
                && !PARTIAL.contains(&ext.to_ascii_lowercase().as_str())
                && !no.contains(&path.to_lowercase())
                && !busy.contains(id)
        })
        .map(|(id, _, _)| id)
        .collect())
}

fn plural(n: i64) -> &'static str {
    if n == 1 { "file" } else { "files" }
}

/// Files new arrivals by learned rules, and proposes the rest. Remembers the arrivals it has
/// already considered, so a Downloads folder full of loose files doesn't trigger a full re-plan
/// every run.
#[derive(Default)]
pub struct FilingWatch {
    seen: std::collections::HashSet<String>,
}

impl Watch for FilingWatch {
    fn name(&self) -> &'static str {
        "filing"
    }
    fn every_ms(&self) -> i64 {
        900_000
    }
    fn needs_idle(&self) -> bool {
        true
    }
    fn kinds(&self) -> &'static [&'static str] {
        &["filing"]
    }
    fn run(&mut self, store: &mut SqliteStore, now: i64) -> Result<Vec<Finding>> {
        let mut findings = Vec::new();
        for (scope, dirs) in inboxes(store)? {
            let Some(choice) = blueprint_of(store.conn(), &scope)? else { continue };
            if !approved_once(store, &scope)? {
                continue;
            }
            let mut auto: Vec<Placement> = Vec::new();
            let mut pending: Vec<Placement> = Vec::new();
            let new_ids: std::collections::HashSet<String> =
                arrivals(store, &dirs, now)?.into_iter().filter(|id| !self.seen.contains(id)).collect();
            self.seen.extend(new_ids.iter().cloned());
            // Plan the scope once and pick out the arrivals (planning per file would redo the
            // whole drive for each one).
            let planned: Vec<Placement> = if new_ids.is_empty() {
                Vec::new()
            } else {
                plan(store, &scope, choice)?
                    .into_iter()
                    .filter(|p| matches!(&p.target, Target::File { file_id } if new_ids.contains(file_id)))
                    .collect()
            };
            for p in planned {
                if p.to.is_none() || p.to_sort || p.confidence < PROPOSE_AT {
                    continue;
                }
                let automatic = p.confidence >= AUTO_AT
                    && p.rule_key.as_deref().is_some_and(|k| automatic_rule_for(store.conn(), &scope.key(), k).ok().flatten().is_some());
                if automatic { auto.push(p) } else { pending.push(p) }
            }
            if !auto.is_empty() {
                let batch = propose_placements(store, &scope, &format!("filing-auto:{}", scope.key()), auto)?;
                approve_batch(store, &batch, None)?;
                execute_batch(store, &batch)?;
                after_execute(store, &batch)?;
            }
            if !pending.is_empty() && open_filing_batch(store.conn(), &scope)?.is_none() {
                propose_placements(store, &scope, &format!("filing:{}", scope.key()), pending)?;
            }
            if let Some(open) = open_filing_batch(store.conn(), &scope)? {
                let n: i64 = store.conn().query_row(
                    "SELECT COUNT(*) FROM actions WHERE batch_id = ?1 AND status = 'proposed'",
                    [&open],
                    |r| r.get(0),
                )?;
                if n > 0 {
                    findings.push(Finding {
                        kind: "filing".into(),
                        subject: scope.key(),
                        level: if n >= REMIND_AT { Level::Convenient } else { Level::Note },
                        title: format!("{n} new {} ready to file", plural(n)),
                        detail: "Review where they go, then approve. Nothing moves until you do.".into(),
                        bytes: None,
                        action: FindingAction::Open { screen: Screen::Filing },
                    });
                }
            }
        }
        Ok(findings)
    }
}
