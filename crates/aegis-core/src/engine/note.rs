//! The weekly note (spec §9): one calm page a week saying what Aegis did, what needs the
//! owner (at most three things), how safe their files are, and where space is going.
//!
//! A note is keyed by the date of the note day it belongs to ("2026-11-22"). It's due once that
//! day's note hour has passed; a PC that was off then gets it at the next chance. A new install
//! waits for its first note day.

use super::reminders::{Reminder, demoted_kinds, list_open, mark_announced};
use super::space::{day_index, forecast};
use super::{Level, bytes_text};
use crate::backup::priority::coverage_summary;
use crate::error::Result;
use crate::{new_id, now_ms};
use rusqlite::{Connection, OptionalExtension, params};
use serde::{Deserialize, Serialize};
use ts_rs::TS;

const DAY_MS: i64 = 86_400_000;
const FIRST_DAY_KEY: &str = "engine.first_day";
pub const NEEDS_YOU_MAX: usize = 3;

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, TS)]
#[ts(export)]
pub struct WeeklyNote {
    pub week: String,
    pub headline: String,
    pub needs_you: Vec<Reminder>,
    pub did: Vec<String>,
    pub safety: Option<String>,
    pub space: Vec<String>,
    pub tidy: Option<String>,
    #[serde(default)]
    pub projects: Option<String>,
    pub about_aegis: Vec<String>,
    pub read: bool,
}

/// The local clock, from the desktop host (the core has no time zone of its own).
#[derive(Debug, Clone)]
pub struct LocalNow {
    /// Local date, `YYYY-MM-DD`.
    pub date: String,
    /// 0 = Monday … 6 = Sunday.
    pub weekday: u8,
    pub hour: u8,
}

/// `YYYY-MM-DD` for days since 1970-01-01.
pub fn date_of(index: i64) -> String {
    let z = index + 719_468;
    let era = z.div_euclid(146_097);
    let doe = z - era * 146_097;
    let yoe = (doe - doe / 1460 + doe / 36_524 - doe / 146_096) / 365;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let d = doy - (153 * mp + 2) / 5 + 1;
    let m = if mp < 10 { mp + 3 } else { mp - 9 };
    let y = yoe + era * 400 + i64::from(m <= 2);
    format!("{y:04}-{m:02}-{d:02}")
}

/// Records the first day the engine ran (first call wins).
pub fn ensure_first_day(conn: &Connection, date: &str) -> Result<()> {
    conn.execute(
        "INSERT INTO settings (key, scope, value_json, updated_at) VALUES (?1, 'global', ?2, ?3)
         ON CONFLICT(key, scope) DO NOTHING",
        params![FIRST_DAY_KEY, serde_json::to_string(date)?, now_ms()],
    )?;
    Ok(())
}

fn first_day(conn: &Connection) -> Result<Option<i64>> {
    let raw: Option<String> = conn
        .query_row("SELECT value_json FROM settings WHERE key = ?1 AND scope = 'global'", [FIRST_DAY_KEY], |r| r.get(0))
        .optional()?;
    Ok(raw.and_then(|j| serde_json::from_str::<String>(&j).ok()).and_then(|d| day_index(&d)))
}

/// The note that should exist by now and doesn't yet, as its key; `None` when nothing is due.
pub fn due_week(conn: &Connection, local: &LocalNow, note_day: u8, note_hour: u8) -> Result<Option<String>> {
    let Some(today) = day_index(&local.date) else { return Ok(None) };
    let offset = i64::from((local.weekday + 7 - note_day % 7) % 7);
    let mut due = today - offset;
    if offset == 0 && local.hour < note_hour {
        due -= 7;
    }
    if first_day(conn)?.is_some_and(|first| due < first) {
        return Ok(None);
    }
    let key = date_of(due);
    let exists: bool =
        conn.query_row("SELECT EXISTS (SELECT 1 FROM weekly_notes WHERE week = ?1)", [&key], |r| r.get(0))?;
    Ok((!exists).then_some(key))
}

/// Owner-facing name of a reminder kind.
pub fn kind_label(kind: &str) -> &str {
    match kind {
        "only_copy" => "files that exist only on this PC",
        "quarantine_expiry" => "quarantine about to empty",
        "space" => "running out of space",
        "janitor" => "Downloads tidy-ups",
        "drive_smart" => "drive health warnings",
        other => other,
    }
}

fn plural(n: i64, one: &str, many: &str) -> String {
    format!("{n} {}", if n == 1 { one } else { many })
}

/// Builds the note for `week` from what happened since `week_start_ms`. `today` is the local
/// date (for the forecast). Marks demoted reminder kinds as announced.
pub fn build(conn: &Connection, week: &str, week_start_ms: i64, today: &str, now: i64) -> Result<WeeklyNote> {
    let open = list_open(conn)?;
    let needs_you: Vec<Reminder> = open.iter().filter(|r| r.kind != "engine_health").take(NEEDS_YOU_MAX).cloned().collect();
    let open_count = open.iter().filter(|r| r.kind != "engine_health").count();

    let mut did = Vec::new();
    let indexed: i64 = conn.query_row(
        "SELECT COUNT(*) FROM files WHERE first_seen >= ?1 AND deleted_at IS NULL",
        [week_start_ms],
        |r| r.get(0),
    )?;
    if indexed > 0 {
        did.push(format!("Indexed {}", plural(indexed, "new file", "new files")));
    }
    let topups: i64 = conn.query_row(
        "SELECT COUNT(*) FROM backup_jobs WHERE status = 'completed' AND finished_at >= ?1",
        [week_start_ms],
        |r| r.get(0),
    )?;
    if topups > 0 {
        did.push(format!("Topped up the backup {}", plural(topups, "time", "times")));
    }
    let catalogued: i64 = conn.query_row(
        "SELECT COUNT(*) FROM volumes WHERE last_catalog_complete_at >= ?1",
        [week_start_ms],
        |r| r.get(0),
    )?;
    if catalogued > 0 {
        did.push(format!("Catalogued {}", plural(catalogued, "drive", "drives")));
    }
    let auto_filed: i64 = conn.query_row(
        "SELECT COUNT(*) FROM actions a JOIN action_batches b ON b.id = a.batch_id
         WHERE b.origin LIKE 'filing-auto:%' AND a.status = 'executed' AND b.created_at >= ?1",
        [week_start_ms],
        |r| r.get(0),
    )?;
    if auto_filed > 0 {
        did.push(format!("Filed {} by itself", plural(auto_filed, "file", "files")));
    }

    let mut fully_backed_up = false;
    let safety = match coverage_summary(conn)? {
        None => None,
        Some(cov) => {
            let unprotected: i64 =
                cov.tiers.iter().filter(|t| t.tier == 1 || t.tier == 2).map(|t| t.unprotected_bytes).sum();
            if unprotected == 0 {
                fully_backed_up = true;
                Some("Everything important is backed up.".to_string())
            } else {
                let last_seen: i64 = conn
                    .query_row(
                        "SELECT last_seen FROM volumes WHERE id = ?1",
                        [&cov.backup_volume_id],
                        |r| r.get(0),
                    )
                    .unwrap_or(now);
                let days = (now - last_seen).max(0) / DAY_MS;
                let seen = match days {
                    0 => "today".to_string(),
                    1 => "yesterday".to_string(),
                    d => format!("{d} days ago"),
                };
                Some(format!("{} exist only on this PC. Backup drive last seen {seen}.", bytes_text(unprotected)))
            }
        }
    };

    let mut space = Vec::new();
    let vols: Vec<(String, String)> = {
        let mut stmt = conn.prepare("SELECT DISTINCT s.volume_id, COALESCE(v.label, v.mount_point, s.volume_id)
                                     FROM space_samples s LEFT JOIN volumes v ON v.id = s.volume_id ORDER BY 2")?;
        stmt.query_map([], |r| Ok((r.get(0)?, r.get(1)?)))?.collect::<rusqlite::Result<_>>()?
    };
    for (id, label) in vols {
        if let Some(f) = forecast(conn, &id, today)? {
            let weeks = f.weeks_left.round().max(1.0) as i64;
            let mut line = format!("{label} fills up in about {}", plural(weeks, "week", "weeks"));
            if let Some(root) = &f.fastest_root {
                let name = std::path::Path::new(root).file_name().map_or(root.clone(), |n| n.to_string_lossy().to_string());
                line.push_str(&format!("; {name} is growing fastest"));
            }
            line.push('.');
            space.push(line);
        }
    }

    let tidy = open.iter().find(|r| r.kind == "janitor").map(|r| format!("{}, ready to review.", r.title));

    let (multi_count, min_places, max_places): (i64, Option<i64>, Option<i64>) = conn.query_row(
        "SELECT COUNT(*), MIN(n), MAX(n) FROM (
            SELECT c.id, COUNT(l.id) AS n
            FROM collections c
            JOIN collection_locations l ON l.collection_id = c.id
            WHERE c.kind = 'project' AND c.state NOT IN ('left_as_is', 'not_a_project')
            GROUP BY c.id HAVING n >= 2
        )",
        [],
        |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?)),
    )?;
    let reunite_count = crate::context::reunite_plans_conn(conn).map(|v| v.len()).unwrap_or(0);
    let mut project_parts: Vec<String> = Vec::new();
    if multi_count > 0 {
        let min_p = min_places.unwrap_or(2);
        let max_p = max_places.unwrap_or(min_p);
        let range = if min_p == max_p { format!("{min_p}") } else { format!("{min_p}\u{2013}{max_p}") };
        project_parts.push(format!(
            "{} copies in {range} places.",
            if multi_count == 1 { "1 project has".to_string() } else { format!("{multi_count} projects have") }
        ));
    }
    if reunite_count > 0 {
        project_parts.push(format!(
            "{} files from earlier batches to put back together.",
            if reunite_count == 1 { "1 group has".to_string() } else { format!("{reunite_count} groups have") }
        ));
    }
    let projects = (!project_parts.is_empty()).then(|| project_parts.join(" "));

    let mut about_aegis: Vec<String> =
        open.iter().filter(|r| r.kind == "engine_health").map(|r| format!("{}.", r.title)).collect();
    for (kind, announced) in demoted_kinds(conn)? {
        if !announced {
            about_aegis.push(format!("I stopped reminding you about {}; it's here instead.", kind_label(&kind)));
            mark_announced(conn, &kind)?;
        }
    }

    let has_now = needs_you.iter().any(|r| r.level == Level::Now);
    let headline = if has_now {
        format!("Needs you: {}.", plural(open_count as i64, "thing", "things"))
    } else if open_count == 0 && fully_backed_up {
        "A quiet week. Everything important is backed up.".to_string()
    } else if open_count == 0 {
        "A quiet week.".to_string()
    } else {
        format!("A quiet week. {} to look at when you have a minute.", plural(open_count as i64, "thing", "things"))
    };

    Ok(WeeklyNote { week: week.to_string(), headline, needs_you, did, safety, space, tidy, projects, about_aegis, read: false })
}

pub fn save(conn: &Connection, note: &WeeklyNote, now: i64) -> Result<()> {
    conn.execute(
        "INSERT INTO weekly_notes (id, week, built_at, json) VALUES (?1, ?2, ?3, ?4)
         ON CONFLICT(week) DO UPDATE SET built_at = excluded.built_at, json = excluded.json, read_at = NULL",
        params![new_id(), note.week, now, serde_json::to_string(note)?],
    )?;
    Ok(())
}

/// The most recent note, with whether the owner has opened it.
pub fn latest(conn: &Connection) -> Result<Option<WeeklyNote>> {
    let row: Option<(String, Option<i64>)> = conn
        .query_row("SELECT json, read_at FROM weekly_notes ORDER BY week DESC LIMIT 1", [], |r| {
            Ok((r.get(0)?, r.get(1)?))
        })
        .optional()?;
    Ok(row.and_then(|(json, read_at)| {
        serde_json::from_str::<WeeklyNote>(&json).ok().map(|mut n| {
            n.read = read_at.is_some();
            n
        })
    }))
}

pub fn mark_read(conn: &Connection, week: &str, now: i64) -> Result<()> {
    conn.execute("UPDATE weekly_notes SET read_at = ?2 WHERE week = ?1 AND read_at IS NULL", params![week, now])?;
    Ok(())
}
