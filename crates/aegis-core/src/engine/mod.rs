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
