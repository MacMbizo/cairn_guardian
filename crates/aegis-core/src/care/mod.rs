//! Windows System Care (`Project 4`): reclaimable cleanup, read-only installed app & startup
//! inventory (`R3`), SMART drive health (`R1`), disk space & root growth trends, and `Screen::Care`
//! health monitoring (`K1`, `K2`, `K3`, `K5`, `R8`, `R9`).

pub mod apps;
pub mod cleanup;
pub mod smart;
pub mod startup;
pub mod trends;
pub mod watch;

use crate::contracts::TargetKind;
use crate::db::files::normalize_path;
use serde::{Deserialize, Serialize};
use std::collections::HashSet;
use std::path::{Component, Path, PathBuf};
use ts_rs::TS;

pub const SYSTEM_CARE_AGENT: &str = "system_care";

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize, TS)]
#[serde(rename_all = "snake_case")]
#[ts(export)]
pub enum CleanupCategory {
    TempFiles,
    CrashDumps,
    BrowserCache,
    DevCache,
    BuildCache,
    DownloadsJanitor,
    DeadFiles,
    UninstallLeftover,
}

impl CleanupCategory {
    pub fn label(self) -> &'static str {
        match self {
            Self::TempFiles => "Temporary files",
            Self::CrashDumps => "Crash dumps & error reports",
            Self::BrowserCache => "Browser caches",
            Self::DevCache => "Developer package caches",
            Self::BuildCache => "Project build caches",
            Self::DownloadsJanitor => "Downloads tidy-ups",
            Self::DeadFiles => "Dead large files (untracked & 180d+ old)",
            Self::UninstallLeftover => "App leftover folders",
        }
    }

    pub fn description(self) -> &'static str {
        match self {
            Self::TempFiles => "Stale Windows and user temporary files untouched for at least 24 hours.",
            Self::CrashDumps => "Windows crash dumps and Error Reporting queues from past crashes.",
            Self::BrowserCache => "Disposable HTTP/code/GPU caches inside local browser profiles (bookmarks and logins are untouched).",
            Self::DevCache => "Re-downloadable package caches (pip, npm, uv, NuGet).",
            Self::BuildCache => "Detected project build/dependency folders (safe_build_cache carve-out).",
            Self::DownloadsJanitor => "Partial downloads, duplicate download copies, and aged installers.",
            Self::DeadFiles => "Large catalogued files (100 MiB+, 180d+ unchanged) outside projects, backups, and atomic units.",
            Self::UninstallLeftover => "Aged app-profile folders left behind without a matching installed app.",
        }
    }

    pub fn sort_order(self) -> u8 {
        match self {
            Self::TempFiles => 0,
            Self::CrashDumps => 1,
            Self::BrowserCache => 2,
            Self::DevCache => 3,
            Self::BuildCache => 4,
            Self::DownloadsJanitor => 5,
            Self::DeadFiles => 6,
            Self::UninstallLeftover => 7,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, TS)]
#[ts(export)]
pub struct CleanupCandidate {
    pub id: String,
    pub category: CleanupCategory,
    pub group_label: String,
    pub path: String,
    pub target_kind: TargetKind,
    #[ts(type = "number")]
    pub bytes: i64,
    #[ts(type = "number")]
    pub file_count: i64,
    #[ts(type = "number")]
    pub age_days: i64,
    pub reason: String,
    pub file_id: Option<String>,
    pub safety_note: String,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, TS)]
#[ts(export)]
pub struct CleanupGroupSummary {
    pub category: CleanupCategory,
    pub label: String,
    pub description: String,
    #[ts(type = "number")]
    pub total_bytes: i64,
    #[ts(type = "number")]
    pub item_count: usize,
    pub items: Vec<CleanupCandidate>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, TS)]
#[ts(export)]
pub struct CleanupScanReport {
    #[ts(type = "number")]
    pub scanned_at: i64,
    #[ts(type = "number")]
    pub total_reclaimable_bytes: i64,
    pub groups: Vec<CleanupGroupSummary>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, TS)]
#[ts(export)]
pub struct InstalledApp {
    pub id: String,
    pub name: String,
    pub publisher: Option<String>,
    pub version: Option<String>,
    pub install_date: Option<String>,
    pub install_location: Option<String>,
    #[ts(type = "number")]
    pub estimated_bytes: i64,
    pub has_uninstaller: bool,
    #[ts(type = "number | null")]
    pub last_used_ms: Option<i64>,
    #[ts(type = "number | null")]
    pub days_since_used: Option<i64>,
    pub unused_likely: bool,
    pub scope: String,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, TS)]
#[ts(export)]
pub struct LeftoverFolder {
    pub path: String,
    pub folder_name: String,
    pub parent_label: String,
    #[ts(type = "number")]
    pub bytes: i64,
    #[ts(type = "number")]
    pub file_count: i64,
    #[ts(type = "number")]
    pub age_days: i64,
    pub reason: String,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, TS)]
#[ts(export)]
pub struct StartupEntry {
    pub id: String,
    pub name: String,
    pub command: String,
    pub source: String,
    pub enabled: bool,
    pub target_path: Option<String>,
    pub target_exists: bool,
    pub orphaned: bool,
    pub impact_hint: String,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, TS)]
#[ts(export)]
pub struct DriveHealthItem {
    pub device_id: String,
    pub model: String,
    pub media_type: String,
    pub bus_type: String,
    #[ts(type = "number")]
    pub size_bytes: i64,
    pub health_status: String,
    pub operational_status: String,
    pub drive_letters: Vec<String>,
    pub temperature_c: Option<i32>,
    pub wear_pct: Option<u32>,
    #[ts(type = "number | null")]
    pub power_on_hours: Option<u64>,
    #[ts(type = "number | null")]
    pub read_errors: Option<u64>,
    #[ts(type = "number | null")]
    pub write_errors: Option<u64>,
    pub smart_detail_available: bool,
    pub smart_note: Option<String>,
    #[ts(type = "number")]
    pub catalog_read_errors: usize,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, TS)]
#[ts(export)]
pub struct VolumeTrendPoint {
    pub day: String,
    #[ts(type = "number")]
    pub used_bytes: i64,
    #[ts(type = "number")]
    pub total_bytes: i64,
    #[ts(type = "number")]
    pub free_bytes: i64,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, TS)]
#[ts(export)]
pub struct VolumeTrend {
    pub volume_id: String,
    pub label: String,
    pub mount_point: String,
    #[ts(type = "number")]
    pub used_bytes: i64,
    #[ts(type = "number")]
    pub total_bytes: i64,
    #[ts(type = "number")]
    pub free_bytes: i64,
    pub free_pct: f64,
    #[ts(type = "number")]
    pub bytes_per_week: i64,
    pub weeks_left: Option<f64>,
    pub fastest_root: Option<String>,
    pub points: Vec<VolumeTrendPoint>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, TS)]
#[ts(export)]
pub struct RootTrend {
    pub root: String,
    pub name: String,
    #[ts(type = "number")]
    pub current_bytes: i64,
    #[ts(type = "number")]
    pub delta_7d: i64,
    #[ts(type = "number")]
    pub delta_30d: i64,
    #[ts(type = "number")]
    pub bytes_per_week: i64,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, TS)]
#[ts(export)]
pub struct DiskTrendsReport {
    pub today: String,
    pub volumes: Vec<VolumeTrend>,
    pub roots: Vec<RootTrend>,
}

/// Normalizes a filesystem path into its display form (stripping `\\?\` when present).
pub(crate) fn display_path(p: &Path) -> String {
    normalize_path(p, cfg!(not(windows))).0
}

/// True when `path` enters any Aegis internal directory (`com.aegis.guardian`, `.aegis`, `.aegis-quarantine`).
pub fn path_contains_aegis_internal(path: &Path) -> bool {
    path.components().any(|c| match c {
        Component::Normal(seg) => {
            let s = seg.to_string_lossy();
            s.eq_ignore_ascii_case(".aegis")
                || s.eq_ignore_ascii_case(".aegis-quarantine")
                || s.eq_ignore_ascii_case("com.aegis.guardian")
        }
        _ => false,
    })
}

fn dedup_safe_paths(paths: Vec<PathBuf>) -> Vec<PathBuf> {
    let mut out = Vec::new();
    let mut seen = HashSet::new();
    for p in paths {
        if path_contains_aegis_internal(&p) {
            continue;
        }
        let key = display_path(&p).to_lowercase();
        if !key.is_empty() && seen.insert(key) {
            out.push(p);
        }
    }
    out
}

/// Canonical child-only cleanup roots (`temp`, `crash`, `downloads`) allowed for `system_care`
/// quarantine proposals. Only strict descendants of these roots (never the root itself) are allowed.
pub fn allowed_cleanup_roots() -> Vec<PathBuf> {
    let cfg = cleanup::CleanupScanConfig::for_host();
    let mut paths = cfg.temp_dirs;
    paths.extend(cfg.crash_dirs);
    if let Some(dl) = cfg.downloads_dir {
        paths.push(dl);
    }
    dedup_safe_paths(paths)
}

/// Exact disposable cache leaf folders (`BrowserCache`, `DevCache`) that may themselves be proposed
/// as a `TargetKind::Dir` quarantine item by `system_care`.
pub fn allowed_disposable_cache_dirs() -> Vec<PathBuf> {
    let cfg = cleanup::CleanupScanConfig::for_host();
    let mut paths: Vec<PathBuf> = cfg.browser_cache_dirs.into_iter().map(|(_, p)| p).collect();
    paths.extend(cfg.dev_cache_dirs.into_iter().map(|(_, p)| p));
    dedup_safe_paths(paths)
}

/// Direct parent directories (`%LOCALAPPDATA%`, `%APPDATA%`) whose stale unowned child folders may be
/// proposed for 30-day quarantine after an uninstall.
pub fn allowed_leftover_parents() -> Vec<PathBuf> {
    let mut out = Vec::new();
    let mut seen = HashSet::new();
    for var in ["LOCALAPPDATA", "APPDATA"] {
        if let Some(raw) = std::env::var_os(var) {
            let p = PathBuf::from(raw);
            let key = display_path(&p).to_lowercase();
            if !key.is_empty() && seen.insert(key) {
                out.push(p);
            }
        }
    }
    out
}
