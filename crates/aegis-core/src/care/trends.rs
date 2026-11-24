//! Daily disk-space and folder-growth trends computed from `space_samples` and `root_samples`.

use super::{DiskTrendsReport, RootTrend, VolumeTrend, VolumeTrendPoint};
use crate::db::SqliteStore;
use crate::engine::space::{day_index, forecast, record_day, root_sizes};
use crate::error::Result;
use crate::scanner::os_adapter::disk_space_for_mount;
use std::collections::BTreeMap;
use std::path::Path;

/// Records today's volume and PC root space samples when `space_samples` does not yet hold `today`.
pub fn ensure_today_space_sample(store: &SqliteStore, today: &str) -> Result<()> {
    let exists: bool = store.conn().query_row(
        "SELECT EXISTS(SELECT 1 FROM space_samples WHERE day = ?1)",
        [today],
        |r| r.get(0),
    )?;
    if exists {
        return Ok(());
    }
    let vols: Vec<(String, i64, i64)> = store
        .list_volumes()?
        .into_iter()
        .filter(|v| v.status == "online" && v.total_bytes > 0)
        .map(|v| (v.id, v.total_bytes.saturating_sub(v.free_bytes) as i64, v.total_bytes as i64))
        .collect();
    if vols.is_empty() {
        return Ok(());
    }
    let roots = root_sizes(store)?;
    record_day(store.conn(), today, &vols, &roots)
}

fn linear_bytes_per_week(points: &[(i64, i64)]) -> i64 {
    if points.len() < 2 {
        return 0;
    }
    let n = points.len() as f64;
    let mx = points.iter().map(|p| p.0 as f64).sum::<f64>() / n;
    let my = points.iter().map(|p| p.1 as f64).sum::<f64>() / n;
    let num: f64 = points
        .iter()
        .map(|p| (p.0 as f64 - mx) * (p.1 as f64 - my))
        .sum();
    let den: f64 = points
        .iter()
        .map(|p| (p.0 as f64 - mx).powi(2))
        .sum();
    if den <= 0.0 {
        return 0;
    }
    ((num / den) * 7.0).round() as i64
}

/// Builds per-volume space trends + per-root growth deltas across the last `days` (`7..=365`).
pub fn build_disk_trends(store: &SqliteStore, today: &str, days: i64) -> Result<DiskTrendsReport> {
    let conn = store.conn();
    let window = days.clamp(7, 365);
    let today_idx = day_index(today);
    let min_idx = today_idx.map(|t| t - window);

    let volumes_db = store.list_volumes()?;
    let mut sample_map: BTreeMap<String, Vec<VolumeTrendPoint>> = BTreeMap::new();
    {
        let mut st = conn.prepare(
            "SELECT volume_id, day, used_bytes, total_bytes FROM space_samples ORDER BY volume_id, day",
        )?;
        for row in st.query_map([], |r| {
            Ok((
                r.get::<_, String>(0)?,
                r.get::<_, String>(1)?,
                r.get::<_, i64>(2)?,
                r.get::<_, i64>(3)?,
            ))
        })? {
            let (vid, day, used, total) = row?;
            if let (Some(min), Some(d_idx)) = (min_idx, day_index(&day))
                && (d_idx < min || today_idx.is_some_and(|t| d_idx > t))
            {
                continue;
            }
            sample_map.entry(vid).or_default().push(VolumeTrendPoint {
                day,
                used_bytes: used,
                total_bytes: total,
                free_bytes: (total - used).max(0),
            });
        }
    }

    let mut volumes = Vec::new();
    for v in &volumes_db {
        let points = sample_map.remove(&v.id).unwrap_or_default();
        if v.status != "online" && points.is_empty() {
            continue;
        }
        let (live_total, live_free) = if v.status == "online" {
            disk_space_for_mount(&v.mount_point)
        } else {
            (0, 0)
        };
        let (used_bytes, total_bytes, free_bytes) = if live_total > 0 {
            let tot = live_total as i64;
            let free = live_free as i64;
            ((tot - free).max(0), tot, free)
        } else if let Some(last) = points.last() {
            (last.used_bytes, last.total_bytes, last.free_bytes)
        } else {
            (0, 0, 0)
        };
        let free_pct = if total_bytes > 0 {
            ((free_bytes as f64 / total_bytes as f64) * 1000.0).round() / 10.0
        } else {
            0.0
        };
        let xy: Vec<(i64, i64)> = points
            .iter()
            .filter_map(|p| day_index(&p.day).map(|d| (d, p.used_bytes)))
            .collect();
        let fc = forecast(conn, &v.id, today)?;
        volumes.push(VolumeTrend {
            volume_id: v.id.clone(),
            label: v.label.clone().unwrap_or_else(|| v.mount_point.clone()),
            mount_point: v.mount_point.clone(),
            used_bytes,
            total_bytes,
            free_bytes,
            free_pct,
            bytes_per_week: linear_bytes_per_week(&xy),
            weeks_left: fc.as_ref().map(|f| f.weeks_left),
            fastest_root: fc.and_then(|f| f.fastest_root),
            points,
        });
    }

    let mut root_map: BTreeMap<String, Vec<(i64, i64)>> = BTreeMap::new();
    {
        let mut st = conn.prepare("SELECT root, day, bytes FROM root_samples ORDER BY root, day")?;
        for row in st.query_map([], |r| {
            Ok((r.get::<_, String>(0)?, r.get::<_, String>(1)?, r.get::<_, i64>(2)?))
        })? {
            let (root, day, bytes) = row?;
            let Some(d_idx) = day_index(&day) else { continue };
            if let Some(min) = min_idx
                && (d_idx < min || today_idx.is_some_and(|t| d_idx > t))
            {
                continue;
            }
            root_map.entry(root).or_default().push((d_idx, bytes));
        }
    }

    let mut roots = Vec::new();
    for (root, pts) in root_map {
        let Some(&(last_day, current_bytes)) = pts.last() else { continue };
        let base_for = |lookback: i64| -> i64 {
            let target = last_day - lookback;
            pts.iter()
                .filter(|(d, _)| *d >= target)
                .min_by_key(|(d, _)| *d)
                .map(|(_, b)| current_bytes - *b)
                .unwrap_or(0)
        };
        let name = Path::new(&root)
            .file_name()
            .map(|n| n.to_string_lossy().to_string())
            .filter(|s| !s.is_empty())
            .unwrap_or_else(|| root.clone());
        roots.push(RootTrend {
            root,
            name,
            current_bytes,
            delta_7d: base_for(7),
            delta_30d: base_for(30),
            bytes_per_week: linear_bytes_per_week(&pts),
        });
    }
    roots.sort_by(|a, b| b.bytes_per_week.cmp(&a.bytes_per_week).then_with(|| b.current_bytes.cmp(&a.current_bytes)));

    Ok(DiskTrendsReport {
        today: today.to_string(),
        volumes,
        roots,
    })
}
