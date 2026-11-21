//! Reminders: findings that reach the owner gently (spec §5).
//!
//! One row per `(kind, subject)`, refreshed each time a watch re-raises it. Now-level rows
//! notify at once (repeating at most daily); Convenient rows share a cap of one notification a
//! day and wait while the owner is busy or Aegis is paused. Dismissing a Convenient kind three
//! times in a row demotes it to the weekly note.

use super::{Finding, FindingAction, Level};
use crate::error::Result;
use crate::new_id;
use rusqlite::{Connection, OptionalExtension, params};
use serde::{Deserialize, Serialize};
use ts_rs::TS;

const DAY_MS: i64 = 86_400_000;
/// Dismissals in a row that move a kind to the weekly note.
pub const DEMOTE_AFTER: i64 = 3;

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, TS)]
#[ts(export)]
pub struct Reminder {
    pub id: String,
    pub kind: String,
    pub subject: String,
    pub level: Level,
    pub title: String,
    pub detail: String,
    #[ts(type = "number | null")]
    pub bytes: Option<i64>,
    pub action: FindingAction,
    pub state: String,
    #[ts(type = "number")]
    pub first_raised_at: i64,
    #[ts(type = "number | null")]
    pub last_shown_at: Option<i64>,
}

/// What the tray icon shows.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "snake_case")]
#[ts(export)]
pub enum TrayState {
    Calm,
    Waiting,
    Urgent,
}

const COLS: &str = "id, kind, subject, level, title, detail, bytes, action_json, state, first_raised_at, last_shown_at";

fn row(r: &rusqlite::Row<'_>) -> rusqlite::Result<Reminder> {
    let level: String = r.get(3)?;
    let action_json: String = r.get(7)?;
    Ok(Reminder {
        id: r.get(0)?,
        kind: r.get(1)?,
        subject: r.get(2)?,
        level: Level::from_db(&level),
        title: r.get(4)?,
        detail: r.get(5)?,
        bytes: r.get(6)?,
        action: serde_json::from_str(&action_json).unwrap_or(FindingAction::None),
        state: r.get(8)?,
        first_raised_at: r.get(9)?,
        last_shown_at: r.get(10)?,
    })
}

fn is_demoted(conn: &Connection, kind: &str) -> Result<bool> {
    Ok(conn
        .query_row("SELECT demoted FROM reminder_kinds WHERE kind = ?1", [kind], |r| r.get::<_, i64>(0))
        .optional()?
        .unwrap_or(0)
        == 1)
}

/// Stores a watch's findings: upserts each by `(kind, subject)` and resolves open reminders of
/// `kinds` that the watch no longer reports.
pub fn sync_findings(conn: &Connection, kinds: &[&str], findings: &[Finding], now: i64) -> Result<()> {
    for f in findings {
        let level = if f.level == Level::Convenient && is_demoted(conn, &f.kind)? { Level::Note } else { f.level };
        conn.execute(
            "INSERT INTO reminders (id, kind, subject, level, title, detail, bytes, action_json, state,
                                    first_raised_at, last_raised_at)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, 'open', ?9, ?9)
             ON CONFLICT(kind, subject) DO UPDATE SET
                level = excluded.level, title = excluded.title, detail = excluded.detail,
                bytes = excluded.bytes, action_json = excluded.action_json,
                last_raised_at = excluded.last_raised_at,
                first_raised_at = CASE WHEN reminders.state = 'open' THEN reminders.first_raised_at ELSE excluded.first_raised_at END,
                resolved_at = NULL,
                state = 'open'",
            params![
                new_id(),
                f.kind,
                f.subject,
                level.as_str(),
                f.title,
                f.detail,
                f.bytes,
                serde_json::to_string(&f.action)?,
                now
            ],
        )?;
    }
    for kind in kinds {
        let mut stmt = conn.prepare("SELECT id, subject FROM reminders WHERE kind = ?1 AND state = 'open'")?;
        let open: Vec<(String, String)> =
            stmt.query_map([kind], |r| Ok((r.get(0)?, r.get(1)?)))?.collect::<rusqlite::Result<_>>()?;
        for (id, subject) in open {
            if !findings.iter().any(|f| &f.kind == kind && f.subject == subject) {
                conn.execute(
                    "UPDATE reminders SET state = 'resolved', resolved_at = ?2 WHERE id = ?1",
                    params![id, now],
                )?;
            }
        }
    }
    Ok(())
}

/// Reminders to notify now. Now-level: every open one not shown in the last day. Convenient:
/// at most one, only when none was shown in the last day and the owner isn't busy or paused.
pub fn to_deliver(conn: &Connection, now: i64, busy: bool, paused: bool) -> Result<Vec<Reminder>> {
    let cutoff = now - DAY_MS;
    let mut out: Vec<Reminder> = {
        let mut stmt = conn.prepare(&format!(
            "SELECT {COLS} FROM reminders WHERE state = 'open' AND level = 'now'
               AND (last_shown_at IS NULL OR last_shown_at <= ?1) ORDER BY first_raised_at"
        ))?;
        stmt.query_map([cutoff], row)?.collect::<rusqlite::Result<_>>()?
    };
    if busy || paused {
        return Ok(out);
    }
    let shown_recently: bool = conn.query_row(
        "SELECT EXISTS (SELECT 1 FROM reminders WHERE level = 'convenient' AND last_shown_at > ?1)",
        [cutoff],
        |r| r.get(0),
    )?;
    if !shown_recently {
        let next = conn
            .query_row(
                &format!(
                    "SELECT {COLS} FROM reminders WHERE state = 'open' AND level = 'convenient'
                       AND (last_shown_at IS NULL OR last_shown_at <= ?1)
                     ORDER BY last_shown_at IS NOT NULL, last_shown_at, first_raised_at LIMIT 1"
                ),
                [cutoff],
                row,
            )
            .optional()?;
        out.extend(next);
    }
    Ok(out)
}

pub fn mark_shown(conn: &Connection, id: &str, now: i64) -> Result<()> {
    conn.execute("UPDATE reminders SET last_shown_at = ?2 WHERE id = ?1", params![id, now])?;
    Ok(())
}

fn kind_and_level(conn: &Connection, id: &str) -> Result<Option<(String, Level)>> {
    Ok(conn
        .query_row("SELECT kind, level FROM reminders WHERE id = ?1", [id], |r| {
            Ok((r.get::<_, String>(0)?, Level::from_db(&r.get::<_, String>(1)?)))
        })
        .optional()?)
}

/// The owner closed a reminder without acting. Returns true when this demoted its kind.
pub fn dismiss(conn: &Connection, id: &str, now: i64) -> Result<bool> {
    let Some((kind, level)) = kind_and_level(conn, id)? else { return Ok(false) };
    conn.execute(
        "INSERT INTO reminder_kinds (kind, dismiss_streak) VALUES (?1, 1)
         ON CONFLICT(kind) DO UPDATE SET dismiss_streak = dismiss_streak + 1",
        [&kind],
    )?;
    if level == Level::Now {
        return Ok(false);
    }
    let (streak, demoted): (i64, i64) = conn.query_row(
        "SELECT dismiss_streak, demoted FROM reminder_kinds WHERE kind = ?1",
        [&kind],
        |r| Ok((r.get(0)?, r.get(1)?)),
    )?;
    if streak >= DEMOTE_AFTER && demoted == 0 {
        conn.execute(
            "UPDATE reminder_kinds SET demoted = 1, demoted_at = ?2, announced = 0 WHERE kind = ?1",
            params![kind, now],
        )?;
        conn.execute(
            "UPDATE reminders SET level = 'note' WHERE kind = ?1 AND state = 'open' AND level = 'convenient'",
            [&kind],
        )?;
        return Ok(true);
    }
    Ok(false)
}

/// The owner acted on a reminder: it is resolved and its kind's dismissal streak resets.
pub fn act(conn: &Connection, id: &str, now: i64) -> Result<()> {
    if let Some((kind, _)) = kind_and_level(conn, id)? {
        conn.execute(
            "INSERT INTO reminder_kinds (kind, dismiss_streak) VALUES (?1, 0)
             ON CONFLICT(kind) DO UPDATE SET dismiss_streak = 0",
            [&kind],
        )?;
    }
    conn.execute(
        "UPDATE reminders SET state = 'resolved', resolved_at = ?2 WHERE id = ?1",
        params![id, now],
    )?;
    Ok(())
}

/// "Remind me again" for a kind that was moved to the note.
pub fn undemote(conn: &Connection, kind: &str) -> Result<()> {
    conn.execute(
        "UPDATE reminder_kinds SET demoted = 0, demoted_at = NULL, dismiss_streak = 0 WHERE kind = ?1",
        [kind],
    )?;
    Ok(())
}

/// Open reminders, most urgent first, then oldest first.
pub fn list_open(conn: &Connection) -> Result<Vec<Reminder>> {
    let mut stmt = conn.prepare(&format!(
        "SELECT {COLS} FROM reminders WHERE state = 'open'
         ORDER BY CASE level WHEN 'now' THEN 0 WHEN 'convenient' THEN 1 ELSE 2 END, first_raised_at"
    ))?;
    Ok(stmt.query_map([], row)?.collect::<rusqlite::Result<_>>()?)
}

/// Demoted kinds, with whether the weekly note has already said so.
pub fn demoted_kinds(conn: &Connection) -> Result<Vec<(String, bool)>> {
    let mut stmt = conn.prepare("SELECT kind, announced FROM reminder_kinds WHERE demoted = 1 ORDER BY kind")?;
    Ok(stmt
        .query_map([], |r| Ok((r.get::<_, String>(0)?, r.get::<_, i64>(1)? == 1)))?
        .collect::<rusqlite::Result<_>>()?)
}

pub fn mark_announced(conn: &Connection, kind: &str) -> Result<()> {
    conn.execute("UPDATE reminder_kinds SET announced = 1 WHERE kind = ?1", [kind])?;
    Ok(())
}

/// The worst open level, for the tray icon.
pub fn tray_state(conn: &Connection) -> Result<TrayState> {
    let worst: Option<String> = conn
        .query_row(
            "SELECT level FROM reminders WHERE state = 'open'
             ORDER BY CASE level WHEN 'now' THEN 0 WHEN 'convenient' THEN 1 ELSE 2 END LIMIT 1",
            [],
            |r| r.get(0),
        )
        .optional()?;
    Ok(match worst.as_deref() {
        Some("now") => TrayState::Urgent,
        Some(_) => TrayState::Waiting,
        None => TrayState::Calm,
    })
}
