//! Safety watches (spec §6.3, §6.4, §6.7–§6.9): only-copy risk, quarantine expiry, drive read
//! errors, catalog resume and the engine's own self-check.

use super::{Finding, FindingAction, Level, Screen, Watch, bytes_text, watch_state};
use crate::backup::backup_volume;
use crate::backup::priority::coverage_summary;
use crate::contracts::QuarantineEntry;
use crate::db::SqliteStore;
use crate::error::Result;
use crate::quarantine::list_quarantine;
use rusqlite::{Connection, OptionalExtension};
use std::path::PathBuf;

const DAY_MS: i64 = 86_400_000;
const HOUR_MS: i64 = 3_600_000;

/// Bytes at or above which a week without the Backup drive is worth a reminder.
pub const ONLY_COPY_BYTES: i64 = 1_000_000_000;
pub const ONLY_COPY_DAYS: i64 = 7;
/// Any unprotected amount is worth a reminder after this long without the Backup drive.
pub const ONLY_COPY_ANY_DAYS: i64 = 14;
/// Read errors on one drive that warrant an urgent reminder.
pub const DRIVE_ERROR_COUNT: usize = 10;

/// The only-copy reminder, or `None` when the risk is still small.
pub fn only_copy_finding(unprotected: i64, days_unseen: i64, backup_label: &str) -> Option<Finding> {
    only_copy_finding_online(unprotected, days_unseen, false, backup_label)
}

/// Like [`only_copy_finding`], but silent while the Backup drive is plugged in: the top-up
/// covers it then, and "hasn't been plugged in" would be false.
pub fn only_copy_finding_online(unprotected: i64, days_unseen: i64, online: bool, backup_label: &str) -> Option<Finding> {
    if online {
        return None;
    }
    let raise = (unprotected >= ONLY_COPY_BYTES && days_unseen >= ONLY_COPY_DAYS)
        || (unprotected > 0 && days_unseen >= ONLY_COPY_ANY_DAYS);
    raise.then(|| Finding {
        kind: "only_copy".into(),
        subject: "backup".into(),
        level: Level::Convenient,
        title: format!("{} exist only on this PC", bytes_text(unprotected)),
        detail: format!("Your Backup drive {backup_label} hasn't been plugged in for {days_unseen} days."),
        bytes: Some(unprotected),
        action: FindingAction::Open { screen: Screen::Drives },
    })
}

pub struct OnlyCopyWatch;

impl Watch for OnlyCopyWatch {
    fn name(&self) -> &'static str {
        "only_copy"
    }
    fn every_ms(&self) -> i64 {
        DAY_MS
    }
    fn needs_idle(&self) -> bool {
        false
    }
    fn kinds(&self) -> &'static [&'static str] {
        &["only_copy"]
    }
    fn run(&mut self, store: &mut SqliteStore, now: i64) -> Result<Vec<Finding>> {
        let conn = store.conn();
        let Some((id, mount, online)) = backup_volume(conn)? else { return Ok(vec![]) };
        let Some(cov) = coverage_summary(conn)? else { return Ok(vec![]) };
        let unprotected: i64 = cov.tiers.iter().filter(|t| t.tier == 1 || t.tier == 2).map(|t| t.unprotected_bytes).sum();
        let last_seen: i64 = conn.query_row("SELECT last_seen FROM volumes WHERE id = ?1", [&id], |r| r.get(0))?;
        let days = (now - last_seen).max(0) / DAY_MS;
        let label = cov.backup_label.clone().unwrap_or(mount);
        Ok(only_copy_finding_online(unprotected, days, online, &label).into_iter().collect())
    }
}

/// Quarantined items that are purged for good within the next two days, as one reminder.
pub fn quarantine_expiry_finding(entries: &[QuarantineEntry], now: i64) -> Option<Finding> {
    let soon: Vec<&QuarantineEntry> = entries
        .iter()
        .filter(|e| e.restored_at.is_none() && e.purged_at.is_none() && e.expires_at > now && e.expires_at <= now + 2 * DAY_MS)
        .collect();
    let first = soon.iter().map(|e| e.expires_at).min()?;
    let n = soon.len();
    let when = match (first - now) / DAY_MS {
        0 if first - now < DAY_MS / 2 => "today".to_string(),
        0 => "tomorrow".to_string(),
        d => format!("in {} days", d + 1),
    };
    let (noun, verb) = if n == 1 { ("file", "is") } else { ("files", "are") };
    Some(Finding {
        kind: "quarantine_expiry".into(),
        subject: "quarantine".into(),
        level: Level::Convenient,
        title: format!("{n} {noun} {verb} deleted for good {when}"),
        detail: "Keep any? They're in Disk Rescue & Quarantine.".into(),
        bytes: Some(soon.iter().map(|e| e.size_bytes).sum()),
        action: FindingAction::Open { screen: Screen::Rescue },
    })
}

pub struct QuarantineExpiryWatch;

impl Watch for QuarantineExpiryWatch {
    fn name(&self) -> &'static str {
        "quarantine_expiry"
    }
    fn every_ms(&self) -> i64 {
        DAY_MS
    }
    fn needs_idle(&self) -> bool {
        false
    }
    fn kinds(&self) -> &'static [&'static str] {
        &["quarantine_expiry"]
    }
    fn run(&mut self, store: &mut SqliteStore, now: i64) -> Result<Vec<Finding>> {
        let entries = list_quarantine(store.conn(), None)?;
        Ok(quarantine_expiry_finding(&entries, now).into_iter().collect())
    }
}

pub struct DriveErrorsWatch;

impl Watch for DriveErrorsWatch {
    fn name(&self) -> &'static str {
        "drive_errors"
    }
    fn every_ms(&self) -> i64 {
        HOUR_MS
    }
    fn needs_idle(&self) -> bool {
        false
    }
    fn kinds(&self) -> &'static [&'static str] {
        &["drive_errors"]
    }
    fn run(&mut self, store: &mut SqliteStore, _now: i64) -> Result<Vec<Finding>> {
        let conn = store.conn();
        let vols: Vec<(String, String, Option<String>)> = {
            let mut stmt = conn.prepare("SELECT id, mount_point, label FROM volumes WHERE passport_id IS NOT NULL")?;
            stmt.query_map([], |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?)))?.collect::<rusqlite::Result<_>>()?
        };
        let mut out = Vec::new();
        for (id, mount, label) in vols {
            let errors: Option<String> = conn
                .query_row(
                    "SELECT errors_json FROM scan_jobs WHERE lower(root_path) = lower(?1) AND finished_at IS NOT NULL
                     ORDER BY finished_at DESC LIMIT 1",
                    [&mount],
                    |r| r.get(0),
                )
                .optional()?;
            // Read failures only: folders Windows locks (System Volume Information, another
            // user's recycle bin) are "permission denied" and say nothing about drive health.
            let n = errors
                .and_then(|j| serde_json::from_str::<Vec<serde_json::Value>>(&j).ok())
                .map_or(0, |v| v.iter().filter(|e| e.get("code").and_then(|c| c.as_str()) != Some("permission_denied")).count());
            if n >= DRIVE_ERROR_COUNT {
                let name = label.unwrap_or(mount);
                out.push(Finding {
                    kind: "drive_errors".into(),
                    subject: id,
                    level: Level::Now,
                    title: format!("{name} had trouble reading {n} files"),
                    detail: "Copy what matters off it soon.".into(),
                    bytes: None,
                    action: FindingAction::Open { screen: Screen::Drives },
                });
            }
        }
        Ok(out)
    }
}

/// Watches the engine's own heartbeats and database growth.
pub struct SelfCheckWatch {
    heartbeats: Vec<(&'static str, i64)>,
    db_path: Option<PathBuf>,
}

const DB_SIZE_KEY: &str = "engine.db_size";

impl SelfCheckWatch {
    /// `heartbeats`: `(watch name, its cadence in ms)`; `db_path`: the database file to size.
    pub fn new(heartbeats: Vec<(&'static str, i64)>, db_path: Option<PathBuf>) -> Self {
        Self { heartbeats, db_path }
    }
}

impl Watch for SelfCheckWatch {
    fn name(&self) -> &'static str {
        "self_check"
    }
    fn every_ms(&self) -> i64 {
        HOUR_MS
    }
    fn needs_idle(&self) -> bool {
        false
    }
    fn kinds(&self) -> &'static [&'static str] {
        &["engine_health"]
    }
    fn run(&mut self, store: &mut SqliteStore, now: i64) -> Result<Vec<Finding>> {
        let mut out = Vec::new();
        for (name, every) in &self.heartbeats {
            let st = watch_state(store.conn(), name)?;
            let stale = st.last_ok_at.is_some_and(|ok| now - ok > 3 * every);
            if stale {
                out.push(Finding {
                    kind: "engine_health".into(),
                    subject: (*name).into(),
                    level: Level::Note,
                    title: format!("{} stopped working and was restarted", watch_label(name)),
                    detail: st.last_error.unwrap_or_default(),
                    bytes: None,
                    action: FindingAction::None,
                });
            }
        }
        if let Some(size) = self.db_path.as_ref().and_then(|p| std::fs::metadata(p).ok()).map(|m| m.len() as i64) {
            let base: Option<(i64, i64)> = store
                .get_setting(DB_SIZE_KEY, "global")?
                .and_then(|j| serde_json::from_str::<(i64, i64)>(&j).ok());
            match base {
                Some((was, at)) if now - at >= 7 * DAY_MS => {
                    if was > 0 && size as f64 > was as f64 * 1.2 {
                        let pct = ((size - was) * 100) / was;
                        out.push(Finding {
                            kind: "engine_health".into(),
                            subject: "db_size".into(),
                            level: Level::Note,
                            title: format!("Aegis's database grew {pct}% this week"),
                            detail: format!("It's now {}.", bytes_text(size)),
                            bytes: Some(size),
                            action: FindingAction::None,
                        });
                    }
                    store.set_setting(DB_SIZE_KEY, "global", &serde_json::to_string(&(size, now))?)?;
                }
                Some(_) => {}
                None => store.set_setting(DB_SIZE_KEY, "global", &serde_json::to_string(&(size, now))?)?,
            }
        }
        Ok(out)
    }
}

/// Owner-facing name of an engine part.
fn watch_label(name: &str) -> String {
    match name {
        "live_index" => "Live file watching".into(),
        other => {
            let mut c = other.replace('_', " ");
            if let Some(f) = c.get(0..1) {
                c.replace_range(0..1, &f.to_uppercase());
            }
            c
        }
    }
}

fn requested_key(volume_id: &str) -> String {
    format!("catalog.requested:{volume_id}")
}

/// Remembers that the owner asked for this drive to be catalogued, so an interrupted catalog
/// may resume by itself (keyed by drive, never by drive letter).
pub fn mark_catalog_requested(conn: &Connection, volume_id: &str) -> Result<()> {
    conn.execute(
        "INSERT INTO settings (key, scope, value_json, updated_at) VALUES (?1, 'global', 'true', ?2)
         ON CONFLICT(key, scope) DO NOTHING",
        rusqlite::params![requested_key(volume_id), crate::now_ms()],
    )?;
    Ok(())
}

/// Drives the owner asked to catalog whose catalog never finished, and that are plugged in
/// now: the engine resumes them (spec §6.7).
pub fn catalogs_to_resume(conn: &Connection) -> Result<Vec<String>> {
    let mut stmt = conn.prepare(
        "SELECT v.id FROM volumes v
         WHERE v.status = 'online' AND v.passport_id IS NOT NULL AND v.last_catalog_complete_at IS NULL
           AND EXISTS (SELECT 1 FROM settings s WHERE s.scope = 'global' AND s.key = 'catalog.requested:' || v.id)
         ORDER BY v.id",
    )?;
    Ok(stmt.query_map([], |r| r.get(0))?.collect::<rusqlite::Result<_>>()?)
}

/// Keeps a tripwire reminder open for as long as a folder's backup is locked, so the owner can
/// always find "It was me" — even when the alert fired while the window was closed.
pub struct TripwireLockWatch;

impl Watch for TripwireLockWatch {
    fn name(&self) -> &'static str {
        "tripwire_locks"
    }
    fn every_ms(&self) -> i64 {
        0
    }
    fn needs_idle(&self) -> bool {
        false
    }
    fn kinds(&self) -> &'static [&'static str] {
        &["tripwire"]
    }
    fn run(&mut self, store: &mut SqliteStore, _now: i64) -> Result<Vec<Finding>> {
        let locks = crate::backup::guard::locked_folders(store.conn())?;
        let open: Vec<(String, String, String)> = {
            let mut stmt = store
                .conn()
                .prepare("SELECT subject, title, detail FROM reminders WHERE kind = 'tripwire'")?;
            stmt.query_map([], |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?)))?.collect::<rusqlite::Result<_>>()?
        };
        Ok(locks
            .into_iter()
            .map(|folder| {
                let name = std::path::Path::new(&folder)
                    .file_name()
                    .map(|n| n.to_string_lossy().to_string())
                    .unwrap_or_else(|| folder.clone());
                // Keep the alert's own wording when there is one.
                let (title, detail) = open
                    .iter()
                    .find(|(s, _, _)| s == &folder)
                    .map(|(_, t, d)| (t.clone(), d.clone()))
                    .unwrap_or_else(|| {
                        (
                            format!("Backup of {name} is paused"),
                            "Lots of files there changed at once. Check them, then tell Aegis if it was you.".into(),
                        )
                    });
                Finding {
                    kind: "tripwire".into(),
                    subject: folder,
                    level: Level::Now,
                    title,
                    detail,
                    bytes: None,
                    action: FindingAction::Open { screen: Screen::Drives },
                }
            })
            .collect())
    }
}
