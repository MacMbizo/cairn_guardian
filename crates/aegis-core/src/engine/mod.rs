//! Always-on engine (spec 2026-11-21-always-on-engine-design): small "watches" that each look
//! after one chore, run when due (heavy ones only while the PC is calm), and report findings
//! that become gentle reminders or lines in the weekly note.

use crate::error::Result;
use rusqlite::{Connection, OptionalExtension, params};
use serde::{Deserialize, Serialize};
use ts_rs::TS;

/// How a finding reaches the owner (spec §5).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "snake_case")]
#[ts(export)]
pub enum Level {
    /// Quiet notification at once; ignores the daily cap.
    Now,
    /// Quiet notification, at most one a day, held while the owner is busy.
    Convenient,
    /// Only in the weekly note.
    Note,
}

impl Level {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Now => "now",
            Self::Convenient => "convenient",
            Self::Note => "note",
        }
    }

    pub fn from_db(s: &str) -> Level {
        match s {
            "now" => Self::Now,
            "convenient" => Self::Convenient,
            _ => Self::Note,
        }
    }
}

/// App screens a finding can open.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "snake_case")]
#[ts(export)]
pub enum Screen {
    Home,
    Drives,
    Dupes,
    Search,
    Rescue,
    Simulator,
    Week,
    Settings,
    Filing,
    Care,
    Ask,
    Assets,
    Versions,
    Knowledge,
    Librarian,
}

/// What acting on a finding does.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(tag = "type", rename_all = "snake_case")]
#[ts(export)]
pub enum FindingAction {
    Open { screen: Screen },
    Batch { batch_id: String },
    None,
}

/// One thing a watch noticed. `(kind, subject)` identifies it across runs.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, TS)]
#[ts(export)]
pub struct Finding {
    pub kind: String,
    pub subject: String,
    pub level: Level,
    pub title: String,
    pub detail: String,
    #[ts(type = "number | null")]
    pub bytes: Option<i64>,
    pub action: FindingAction,
}

/// What the Settings screen and the Home pause chip show.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, TS)]
#[ts(export)]
pub struct EngineStatus {
    #[ts(type = "number | null")]
    pub paused_until: Option<i64>,
    pub tray: reminders::TrayState,
    pub autostart: bool,
    /// 0 = Monday … 6 = Sunday.
    pub note_day: u8,
    pub note_hour: u8,
    pub shortcut: String,
    pub shortcut_ok: bool,
    /// Reminder kinds moved to the weekly note.
    pub demoted: Vec<String>,
}

/// Bookkeeping for one watch.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct WatchState {
    pub name: String,
    pub last_run_at: Option<i64>,
    pub last_ok_at: Option<i64>,
    pub last_error: Option<String>,
}

/// The stored state of watch `name` (an empty state when it has never run).
pub fn watch_state(conn: &Connection, name: &str) -> Result<WatchState> {
    let row = conn
        .query_row(
            "SELECT last_run_at, last_ok_at, last_error FROM watch_state WHERE name = ?1",
            [name],
            |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?)),
        )
        .optional()?;
    let (last_run_at, last_ok_at, last_error) = row.unwrap_or((None, None, None));
    Ok(WatchState { name: name.to_string(), last_run_at, last_ok_at, last_error })
}

/// Records a run of watch `name`; `last_ok_at` moves only when the run succeeded.
pub fn record_run(conn: &Connection, name: &str, now: i64, error: Option<&str>) -> Result<()> {
    conn.execute(
        "INSERT INTO watch_state (name, last_run_at, last_ok_at, last_error)
         VALUES (?1, ?2, CASE WHEN ?3 IS NULL THEN ?2 END, ?3)
         ON CONFLICT(name) DO UPDATE SET
            last_run_at = excluded.last_run_at,
            last_ok_at = CASE WHEN excluded.last_error IS NULL THEN excluded.last_run_at ELSE watch_state.last_ok_at END,
            last_error = excluded.last_error",
        params![name, now, error],
    )?;
    Ok(())
}

pub mod batcher;
pub mod janitor;
pub mod note;
pub mod reminders;
pub mod space;
pub mod tripwire;
pub mod watches;

/// Owner-facing size, 1024-based like Windows Explorer and the rest of the app ("3.2 GB").
pub fn bytes_text(n: i64) -> String {
    const KB: f64 = 1024.0;
    let b = n.max(0) as f64;
    if b >= KB * KB * KB * KB {
        format!("{:.1} TB", b / (KB * KB * KB * KB))
    } else if b >= KB * KB * KB {
        format!("{:.1} GB", b / (KB * KB * KB))
    } else if b >= KB * KB {
        format!("{:.0} MB", b / (KB * KB))
    } else {
        format!("{} KB", (n.max(0) + 1023) / 1024)
    }
}

use crate::db::SqliteStore;

/// Seconds without keyboard or mouse input before the PC counts as idle.
pub const CALM_IDLE_SECS: u64 = 120;
/// On battery, heavy work runs only at or above this charge.
pub const CALM_BATTERY_PCT: u8 = 50;
/// An idle-only watch that never found a calm moment runs anyway this long after it was due.
pub const IDLE_GRACE_MS: i64 = 86_400_000;

/// What the PC is doing right now, as seen by the OS probes.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Calm {
    pub idle_secs: u64,
    pub on_ac: bool,
    pub battery_pct: Option<u8>,
    /// Full-screen app, game or presentation (or Windows "busy").
    pub busy: bool,
}

impl Calm {
    /// Idle, on mains power or a well-charged battery, and not in a game or presentation.
    pub fn is_calm(&self) -> bool {
        let power_ok = self.on_ac || self.battery_pct.is_some_and(|p| p >= CALM_BATTERY_PCT);
        self.idle_secs >= CALM_IDLE_SECS && power_ok && !self.busy
    }
}

/// One chore the engine looks after.
pub trait Watch: Send {
    fn name(&self) -> &'static str;
    fn every_ms(&self) -> i64;
    /// Heavy work: runs only while the PC is calm (or after [`IDLE_GRACE_MS`] past due).
    fn needs_idle(&self) -> bool;
    /// Heavy neural/model work (`K5`): waits for AC mains power, even beyond `IDLE_GRACE_MS`.
    fn needs_ac(&self) -> bool {
        false
    }
    /// Reminder kinds this watch owns; its open reminders not re-raised on a run are resolved.
    fn kinds(&self) -> &'static [&'static str];
    fn run(&mut self, store: &mut SqliteStore, now: i64) -> Result<Vec<Finding>>;
}

/// Whether a watch should run now. A heavy watch that has never run waits for a calm moment too
/// (the engine applies the [`IDLE_GRACE_MS`] fallback from when it first saw the watch).
pub fn is_due(every_ms: i64, last_run_at: Option<i64>, now: i64, needs_idle: bool, calm: bool) -> bool {
    let Some(last) = last_run_at else { return !needs_idle || calm };
    let elapsed = now - last;
    if elapsed < every_ms {
        return false;
    }
    !needs_idle || calm || elapsed >= every_ms + IDLE_GRACE_MS
}

/// Which watches ran on a tick, and which failed (with the error).
#[derive(Debug, Default)]
pub struct TickReport {
    pub ran: Vec<&'static str>,
    pub failed: Vec<(&'static str, String)>,
}

/// Runs due watches and turns their findings into reminders.
pub struct Engine {
    watches: Vec<Box<dyn Watch>>,
    /// When this engine first saw each never-run watch (grace for heavy first runs).
    first_seen: std::collections::HashMap<&'static str, i64>,
}

impl Engine {
    pub fn new(watches: Vec<Box<dyn Watch>>) -> Self {
        Self { watches, first_seen: Default::default() }
    }

    /// One pass over the watches. A failing watch is recorded and never stops the others.
    /// While paused, idle-only watches wait; the rest (safety) keep running.
    pub fn tick(&mut self, store: &mut SqliteStore, now: i64, calm: &Calm, paused: bool) -> Result<TickReport> {
        let mut report = TickReport::default();
        let calm_now = calm.is_calm();
        for w in self.watches.iter_mut() {
            if paused && w.needs_idle() {
                continue;
            }
            if w.needs_ac() && !calm.on_ac {
                continue;
            }
            let state = watch_state(store.conn(), w.name())?;
            let first_seen = *self.first_seen.entry(w.name()).or_insert(now);
            let grace_over = state.last_run_at.is_none() && now - first_seen >= IDLE_GRACE_MS;
            if !grace_over && !is_due(w.every_ms(), state.last_run_at, now, w.needs_idle(), calm_now) {
                continue;
            }
            report.ran.push(w.name());
            match w.run(store, now) {
                Ok(findings) => {
                    reminders::sync_findings(store.conn(), w.kinds(), &findings, now)?;
                    record_run(store.conn(), w.name(), now, None)?;
                }
                Err(e) => {
                    let msg = e.to_string();
                    record_run(store.conn(), w.name(), now, Some(&msg))?;
                    report.failed.push((w.name(), msg));
                }
            }
        }
        Ok(report)
    }
}
