//! Space recorder and fill-up forecast (spec §6.6). One sample per volume and PC root per day,
//! so the forecast (and Project 4's disk trends) have real history.

use super::{Finding, FindingAction, Level, Screen, Watch};
use crate::db::SqliteStore;
use crate::db::files::path_is_within;
use crate::error::Result;
use rusqlite::{Connection, params};
use serde::{Deserialize, Serialize};
use ts_rs::TS;

/// Days of history the forecast looks at.
pub const WINDOW_DAYS: i64 = 28;
/// Samples needed before Cairn predicts anything.
pub const MIN_SAMPLES: usize = 7;
/// Forecasts this far out are not worth mentioning.
pub const SHOW_UNDER_WEEKS: f64 = 26.0;
/// A forecast this close becomes a reminder.
pub const REMIND_UNDER_WEEKS: f64 = 3.0;

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, TS)]
#[ts(export)]
pub struct Forecast {
    pub volume_id: String,
    pub weeks_left: f64,
    pub fastest_root: Option<String>,
    #[ts(type = "number")]
    pub fastest_root_bytes_per_week: i64,
}

/// Stores today's sizes; re-recording the same day replaces that day's row.
pub fn record_day(conn: &Connection, day: &str, volumes: &[(String, i64, i64)], roots: &[(String, i64)]) -> Result<()> {
    for (id, used, total) in volumes {
        conn.execute(
            "INSERT INTO space_samples (volume_id, day, used_bytes, total_bytes) VALUES (?1, ?2, ?3, ?4)
             ON CONFLICT(volume_id, day) DO UPDATE SET used_bytes = excluded.used_bytes, total_bytes = excluded.total_bytes",
            params![id, day, used, total],
        )?;
    }
    for (root, bytes) in roots {
        conn.execute(
            "INSERT INTO root_samples (root, day, bytes) VALUES (?1, ?2, ?3)
             ON CONFLICT(root, day) DO UPDATE SET bytes = excluded.bytes",
            params![root, day, bytes],
        )?;
    }
    Ok(())
}

/// Bytes indexed under each enabled PC root.
pub fn root_sizes(store: &SqliteStore) -> Result<Vec<(String, i64)>> {
    let roots = store.list_roots()?;
    let mut out = Vec::new();
    for r in roots.into_iter().filter(|r| r.enabled) {
        let mut stmt = store.conn().prepare(
            "SELECT path, size FROM files WHERE deleted_at IS NULL AND volume_id = ?1
               AND lower(substr(path, 1, ?3)) = lower(?2)",
        )?;
        let rows = stmt.query_map(params![r.volume_id, r.path, r.path.chars().count() as i64], |row| {
            Ok((row.get::<_, String>(0)?, row.get::<_, i64>(1)?))
        })?;
        let mut total = 0;
        for row in rows {
            let (path, size) = row?;
            if path_is_within(&path, &r.path, false) {
                total += size;
            }
        }
        out.push((r.path, total));
    }
    Ok(out)
}

/// Days since 1970-01-01 for a `YYYY-MM-DD` string (proleptic Gregorian).
pub fn day_index(day: &str) -> Option<i64> {
    let mut it = day.splitn(3, '-');
    let y: i64 = it.next()?.parse().ok()?;
    let m: i64 = it.next()?.parse().ok()?;
    let d: i64 = it.next()?.parse().ok()?;
    let y = if m <= 2 { y - 1 } else { y };
    let era = y.div_euclid(400);
    let yoe = y - era * 400;
    let mp = (m + 9) % 12;
    let doy = (153 * mp + 2) / 5 + d - 1;
    let doe = yoe * 365 + yoe / 4 - yoe / 100 + doy;
    Some(era * 146_097 + doe - 719_468)
}

/// Least-squares slope of `(x, y)` points.
fn slope(points: &[(f64, f64)]) -> f64 {
    let n = points.len() as f64;
    let mx = points.iter().map(|p| p.0).sum::<f64>() / n;
    let my = points.iter().map(|p| p.1).sum::<f64>() / n;
    let num: f64 = points.iter().map(|p| (p.0 - mx) * (p.1 - my)).sum();
    let den: f64 = points.iter().map(|p| (p.0 - mx).powi(2)).sum();
    if den == 0.0 { 0.0 } else { num / den }
}

/// When `volume_id` fills up at the recent rate, if that's within half a year.
pub fn forecast(conn: &Connection, volume_id: &str, today: &str) -> Result<Option<Forecast>> {
    let Some(t) = day_index(today) else { return Ok(None) };
    let from = t - (WINDOW_DAYS - 1);
    let mut stmt = conn.prepare("SELECT day, used_bytes, total_bytes FROM space_samples WHERE volume_id = ?1 ORDER BY day")?;
    let rows: Vec<(i64, i64, i64)> = stmt
        .query_map([volume_id], |r| Ok((r.get::<_, String>(0)?, r.get::<_, i64>(1)?, r.get::<_, i64>(2)?)))?
        .filter_map(|r| r.ok())
        .filter_map(|(d, u, tot)| day_index(&d).map(|i| (i, u, tot)))
        .filter(|(i, _, _)| *i >= from && *i <= t)
        .collect();
    if rows.len() < MIN_SAMPLES {
        return Ok(None);
    }
    let per_day = slope(&rows.iter().map(|(i, u, _)| (*i as f64, *u as f64)).collect::<Vec<_>>());
    if per_day <= 0.0 {
        return Ok(None);
    }
    let (_, used, total) = *rows.last().expect("non-empty");
    let weeks_left = ((total - used).max(0) as f64 / per_day) / 7.0;
    if weeks_left >= SHOW_UNDER_WEEKS {
        return Ok(None);
    }
    let (fastest_root, fastest_root_bytes_per_week) = fastest_root(conn, from, t)?;
    Ok(Some(Forecast { volume_id: volume_id.to_string(), weeks_left, fastest_root, fastest_root_bytes_per_week }))
}

/// The PC root that grew fastest over the window, with its growth per week.
fn fastest_root(conn: &Connection, from: i64, to: i64) -> Result<(Option<String>, i64)> {
    let mut stmt = conn.prepare("SELECT root, day, bytes FROM root_samples ORDER BY root, day")?;
    let rows: Vec<(String, i64, i64)> = stmt
        .query_map([], |r| Ok((r.get::<_, String>(0)?, r.get::<_, String>(1)?, r.get::<_, i64>(2)?)))?
        .filter_map(|r| r.ok())
        .filter_map(|(root, d, b)| day_index(&d).map(|i| (root, i, b)))
        .filter(|(_, i, _)| *i >= from && *i <= to)
        .collect();
    let mut best: (Option<String>, i64) = (None, 0);
    let mut i = 0;
    while i < rows.len() {
        let root = &rows[i].0;
        let j = rows[i..].iter().position(|r| &r.0 != root).map_or(rows.len(), |p| i + p);
        let (first, last) = (&rows[i], &rows[j - 1]);
        if last.1 > first.1 {
            let per_week = (last.2 - first.2) * 7 / (last.1 - first.1);
            if per_week > best.1 {
                best = (Some(root.clone()), per_week);
            }
        }
        i = j;
    }
    Ok(best)
}

pub struct SpaceWatch {
    /// `(volume id, used bytes, total bytes)` for each online volume.
    pub volumes: Box<dyn Fn() -> Vec<(String, i64, i64)> + Send>,
    /// Today's local date as `YYYY-MM-DD`.
    pub today: Box<dyn Fn() -> String + Send>,
}

impl Watch for SpaceWatch {
    fn name(&self) -> &'static str {
        "space"
    }
    fn every_ms(&self) -> i64 {
        21_600_000
    }
    fn needs_idle(&self) -> bool {
        false
    }
    fn kinds(&self) -> &'static [&'static str] {
        &["space"]
    }
    fn run(&mut self, store: &mut SqliteStore, _now: i64) -> Result<Vec<Finding>> {
        let today = (self.today)();
        let volumes = (self.volumes)();
        let roots = root_sizes(store)?;
        record_day(store.conn(), &today, &volumes, &roots)?;
        let mut out = Vec::new();
        for (id, _, _) in &volumes {
            let Some(f) = forecast(store.conn(), id, &today)? else { continue };
            if f.weeks_left >= REMIND_UNDER_WEEKS {
                continue;
            }
            let label: String = store
                .conn()
                .query_row("SELECT COALESCE(label, mount_point) FROM volumes WHERE id = ?1", [id], |r| r.get(0))
                .unwrap_or_else(|_| id.clone());
            let weeks = f.weeks_left.round().max(1.0) as i64;
            let detail = match &f.fastest_root {
                Some(root) => {
                    let name = std::path::Path::new(root).file_name().map_or(root.clone(), |n| n.to_string_lossy().to_string());
                    format!("{name} is growing fastest.")
                }
                None => "Space is running out steadily.".into(),
            };
            out.push(Finding {
                kind: "space".into(),
                subject: id.clone(),
                level: Level::Convenient,
                title: format!("{label} fills up in about {weeks} week{}", if weeks == 1 { "" } else { "s" }),
                detail,
                bytes: None,
                action: FindingAction::Open { screen: Screen::Week },
            });
        }
        Ok(out)
    }
}
