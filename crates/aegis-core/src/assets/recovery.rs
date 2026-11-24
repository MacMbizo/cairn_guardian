//! Dormancy scanning, cross-drive matching, visual header caching, `R7` sensitivity gating,
//! and `K1` reversible recovery batches for Forgotten Asset Recovery (`4.3`).

use std::collections::{BTreeMap, HashMap, HashSet};
use std::path::{Component, Path, PathBuf};

use rusqlite::{Connection, OptionalExtension, params};
use serde::{Deserialize, Serialize};
use ts_rs::TS;

use super::formats::{AssetFamily, classify_asset_family, inspect_visual_header};
use crate::care::path_contains_aegis_internal;
use crate::contracts::{ActionType, ProposedActionInput, RefilePlan, SearchFilters, SearchMode, TargetKind};
use crate::db::SqliteStore;
use crate::db::actions::{build_policy_context, propose_batch};
use crate::db::files::{SENSITIVE_MASK, is_sensitive, normalize_path, path_is_within};
use crate::db::search::search_hybrid;
use crate::error::{AegisError, Result};
use crate::filing::local_ms;
use crate::policy::{PolicyAction, PolicyVerdict, evaluate_policy};
use crate::now_ms;

const DAY_MS: i64 = 86_400_000;
const DEFAULT_MIN_DORMANCY_DAYS: i64 = 90;
const SMALL_GENERIC_ASSET_MIN_BYTES: i64 = 8_192;
const FORGOTTEN_ASSETS_AGENT: &str = "forgotten_asset_recovery";

const COLLECTION_MATCH_STOPWORDS: &[&str] = &[
    "project", "projects", "design", "designs", "asset", "assets", "files", "folder", "folders",
    "photo", "photos", "picture", "pictures", "image", "images", "video", "videos", "music",
    "audio", "final", "draft", "copy", "backup", "archive", "series", "collection", "work",
    "personal", "documents", "downloads", "desktop",
];

const EXCLUDED_SYSTEM_DIRS: &[&str] = &[
    ".git",
    ".hg",
    ".svn",
    "node_modules",
    "target",
    "__pycache__",
    ".venv",
    "venv",
    "site-packages",
    "appdata",
    "windows",
    "program files",
    "program files (x86)",
    "programdata",
    "$recycle.bin",
    "system volume information",
    "windowsapps",
];

const CLUTTER_DIR_SEGMENTS: &[&str] = &[
    "downloads",
    "desktop",
    "unsorted",
    "to sort",
    "to_sort",
    "misc",
    "incoming",
];

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize, TS)]
#[serde(rename_all = "snake_case")]
#[ts(export)]
pub enum ForgottenReason {
    BuriedInClutter,
    OfflineDrive,
    StrandedFromCollection,
    DormantOnlyCopy,
    DormantAsset,
}

impl ForgottenReason {
    pub fn label(self) -> &'static str {
        match self {
            Self::BuriedInClutter => "Buried in Downloads/clutter",
            Self::OfflineDrive => "On unplugged drive",
            Self::StrandedFromCollection => "Matches a project or set",
            Self::DormantOnlyCopy => "Dormant only copy",
            Self::DormantAsset => "Dormant asset",
        }
    }
}

#[derive(Debug, Clone, Default, Serialize, Deserialize, TS)]
#[ts(export)]
pub struct AssetScanOptions {
    #[ts(type = "number | null")]
    pub min_dormancy_days: Option<i64>,
    pub families: Option<Vec<AssetFamily>>,
    pub reasons: Option<Vec<ForgottenReason>>,
    /// `"all"`, `"online"`, or `"offline"`.
    pub drive_status: Option<String>,
    pub volume_id: Option<String>,
    pub query: Option<String>,
    pub include_dismissed: Option<bool>,
    #[ts(type = "number | null")]
    pub limit: Option<usize>,
}

#[derive(Debug, Clone, Serialize, Deserialize, TS)]
#[ts(export)]
pub struct ForgottenAsset {
    pub file_id: String,
    pub path: String,
    pub filename: String,
    pub ext: String,
    pub family: AssetFamily,
    #[ts(type = "number")]
    pub size_bytes: i64,
    #[ts(type = "number")]
    pub mtime: i64,
    #[ts(type = "number")]
    pub age_days: i64,
    pub volume_id: String,
    pub drive_label: String,
    pub drive_online: bool,
    pub reasons: Vec<ForgottenReason>,
    pub primary_reason: ForgottenReason,
    pub summary: String,
    pub only_copy: bool,
    pub sensitivity: String,
    pub snippet: Option<String>,
    #[ts(type = "number | null")]
    pub exif_taken_at: Option<i64>,
    pub camera: Option<String>,
    pub artist: Option<String>,
    pub album: Option<String>,
    #[ts(type = "number | null")]
    pub width: Option<i64>,
    #[ts(type = "number | null")]
    pub height: Option<i64>,
    pub aspect_label: Option<String>,
    pub dominant_color: Option<String>,
    pub matched_collection_id: Option<String>,
    pub matched_collection_name: Option<String>,
    pub matched_collection_kind: Option<String>,
    pub suggested_destination: Option<String>,
    pub dismissed: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize, TS)]
#[ts(export)]
pub struct AssetFamilyCount {
    pub family: AssetFamily,
    pub label: String,
    #[ts(type = "number")]
    pub count: usize,
    #[ts(type = "number")]
    pub total_bytes: i64,
}

#[derive(Debug, Clone, Serialize, Deserialize, TS)]
#[ts(export)]
pub struct AssetDriveSummary {
    pub volume_id: String,
    pub label: String,
    pub online: bool,
    #[ts(type = "number")]
    pub count: usize,
    #[ts(type = "number")]
    pub total_bytes: i64,
}

#[derive(Debug, Clone, Serialize, Deserialize, TS)]
#[ts(export)]
pub struct ForgottenAssetsReport {
    #[ts(type = "number")]
    pub scanned_at: i64,
    #[ts(type = "number")]
    pub total_count: usize,
    #[ts(type = "number")]
    pub total_bytes: i64,
    #[ts(type = "number")]
    pub offline_count: usize,
    #[ts(type = "number")]
    pub buried_count: usize,
    #[ts(type = "number")]
    pub stranded_count: usize,
    #[ts(type = "number")]
    pub only_copy_count: usize,
    #[ts(type = "number")]
    pub dismissed_count: usize,
    pub by_family: Vec<AssetFamilyCount>,
    pub by_drive: Vec<AssetDriveSummary>,
    pub items: Vec<ForgottenAsset>,
}

#[derive(Debug, Clone, Serialize, Deserialize, TS)]
#[ts(export)]
pub struct AssetRecoverItem {
    pub file_id: String,
    pub dest_dir: String,
    pub reason: Option<String>,
}

#[derive(Clone)]
struct ActiveCollection {
    id: String,
    kind: String,
    name: String,
    tokens: Vec<String>,
    online_dest: Option<String>,
}

struct ActiveLocation {
    is_old_copy: bool,
    volume_id: String,
    root_path: String,
    case_sensitive: bool,
}

struct RecoverTargetRow {
    from_path: String,
    filename: String,
    state: String,
    deleted_at: Option<i64>,
    atomic_unit_id: Option<String>,
    vol_status: String,
    vol_role: Option<String>,
}

struct ContextState {
    matchable: Vec<ActiveCollection>,
    locations: Vec<ActiveLocation>,
    active_members: HashSet<String>,
    old_copy_members: HashSet<String>,
}

struct CopyCounts {
    by_hash: HashMap<Vec<u8>, usize>,
    by_name_size: HashMap<(String, i64), usize>,
}

struct DbAssetRow {
    file_id: String,
    volume_id: String,
    mount_point: String,
    path: String,
    name: String,
    ext: String,
    size_bytes: i64,
    mtime: i64,
    blake3: Option<Vec<u8>>,
    sensitivity: String,
    drive_label: String,
    drive_online: bool,
    exif_taken_at: Option<i64>,
    camera: Option<String>,
    artist: Option<String>,
    album: Option<String>,
    cached_width: Option<i64>,
    cached_height: Option<i64>,
    cached_aspect: Option<String>,
    cached_color: Option<String>,
}

/// Scans the encrypted catalog read-only for forgotten creative & digital assets (`K2`, `K4`, `R7`, `D1`).
pub fn scan_forgotten_assets(
    conn: &Connection,
    options: &AssetScanOptions,
    query_vec: Option<(&str, &[f32], f64)>,
    now_ms: i64,
    reveal_sensitive: bool,
) -> Result<ForgottenAssetsReport> {
    let min_dormancy_days = options
        .min_dormancy_days
        .unwrap_or(DEFAULT_MIN_DORMANCY_DAYS)
        .max(0);
    let include_dismissed = options.include_dismissed.unwrap_or(false);
    let family_filter: Option<HashSet<AssetFamily>> =
        options.families.as_ref().filter(|v| !v.is_empty()).map(|v| v.iter().copied().collect());
    let reason_filter: Option<HashSet<ForgottenReason>> =
        options.reasons.as_ref().filter(|v| !v.is_empty()).map(|v| v.iter().copied().collect());
    let drive_status_filter = options
        .drive_status
        .as_deref()
        .map(str::trim)
        .filter(|s| !s.is_empty() && !s.eq_ignore_ascii_case("all"));
    let query_trimmed = options
        .query
        .as_deref()
        .map(str::trim)
        .filter(|s| !s.is_empty());

    let query_hits: HashMap<String, f64> = if let Some(q) = query_trimmed {
        let filters = SearchFilters {
            include_quarantined: Some(false),
            volume_id: options.volume_id.clone(),
            mode: Some(SearchMode::Hybrid),
            ..Default::default()
        };
        search_hybrid(conn, q, query_vec, &filters, 200, reveal_sensitive)?
            .hits
            .into_iter()
            .map(|h| (h.file_id, h.score))
            .collect()
    } else {
        HashMap::new()
    };

    let dismissed_ids: HashSet<String> = {
        let mut st = conn.prepare("SELECT file_id FROM asset_dismissals")?;
        st.query_map([], |r| r.get::<_, String>(0))?
            .collect::<rusqlite::Result<_>>()?
    };

    let ctx_state = load_context_state(conn)?;
    let copy_counts = build_copy_counts(conn)?;

    let rows = load_candidate_rows(conn, options.volume_id.as_deref())?;
    let mut scored: Vec<(f64, ForgottenAsset)> = Vec::new();
    let mut dismissed_count = 0usize;

    for row in rows {
        let Some(family) = classify_asset_family(&row.ext) else {
            continue;
        };
        if let Some(ref ff) = family_filter
            && !ff.contains(&family)
        {
            continue;
        }
        if !family.is_always_kept_small() && row.size_bytes < SMALL_GENERIC_ASSET_MIN_BYTES {
            continue;
        }
        let path_obj = Path::new(&row.path);
        let rel_path_obj = path_obj
            .strip_prefix(Path::new(&row.mount_point))
            .unwrap_or(path_obj);
        if path_contains_aegis_internal(path_obj) || path_has_excluded_system_dir(rel_path_obj) {
            continue;
        }
        if let Some(ds) = drive_status_filter {
            if ds.eq_ignore_ascii_case("online") && !row.drive_online {
                continue;
            }
            if ds.eq_ignore_ascii_case("offline") && row.drive_online {
                continue;
            }
        }

        let in_active_member = ctx_state.active_members.contains(&row.file_id);
        let in_old_copy_member = ctx_state.old_copy_members.contains(&row.file_id);
        let (in_active_loc, in_old_copy_loc) =
            location_membership(&row.volume_id, &row.path, &ctx_state.locations);
        let in_active_collection = in_active_member || in_active_loc;
        let in_old_copy = in_old_copy_member || in_old_copy_loc;

        let only_copy = is_only_copy_of(&row, &copy_counts);

        // Organized files on an online drive inside an active collection aren't forgotten;
        // offline organized files are only surfaced when they are the sole known copy or unmatched online.
        if in_active_collection && (row.drive_online || !only_copy) {
            continue;
        }

        let age_days = if row.mtime > 0 {
            (now_ms - row.mtime).max(0) / DAY_MS
        } else {
            0
        };
        let in_clutter = in_old_copy || path_in_clutter_folder(path_obj);

        let matched = if !in_active_collection {
            match_collection_for_path(path_obj, &row.name, &ctx_state.matchable)
        } else {
            None
        };

        let mut reasons = Vec::new();
        if !row.drive_online {
            reasons.push(ForgottenReason::OfflineDrive);
        }
        if in_old_copy || (in_clutter && (age_days >= min_dormancy_days || matched.is_some())) {
            reasons.push(ForgottenReason::BuriedInClutter);
        }
        if matched.is_some() {
            reasons.push(ForgottenReason::StrandedFromCollection);
        }
        if !in_active_collection && age_days >= min_dormancy_days {
            if only_copy {
                reasons.push(ForgottenReason::DormantOnlyCopy);
            } else {
                reasons.push(ForgottenReason::DormantAsset);
            }
        }

        if reasons.is_empty() {
            continue;
        }
        if let Some(ref rf) = reason_filter
            && !reasons.iter().any(|r| rf.contains(r))
        {
            continue;
        }

        let dismissed = dismissed_ids.contains(&row.file_id);
        if dismissed {
            dismissed_count += 1;
            if !include_dismissed {
                continue;
            }
        }

        let snippet = load_safe_snippet(conn, &row.file_id, &row.sensitivity, reveal_sensitive)?;

        let primary_reason = choose_primary_reason(&reasons, row.drive_online, matched.is_some());
        let matched_collection_id = matched.as_ref().map(|c| c.id.clone());
        let matched_collection_name = matched.as_ref().map(|c| c.name.clone());
        let matched_collection_kind = matched.as_ref().map(|c| c.kind.clone());
        let suggested_destination = matched
            .as_ref()
            .and_then(|c| c.online_dest.clone())
            .filter(|dest| !same_parent_dir(&row.path, dest));

        let summary = build_summary(
            primary_reason,
            age_days,
            &row.drive_label,
            row.drive_online,
            only_copy,
            matched_collection_name.as_deref(),
        );

        let mut asset = ForgottenAsset {
            file_id: row.file_id,
            path: row.path,
            filename: row.name,
            ext: row.ext,
            family,
            size_bytes: row.size_bytes,
            mtime: row.mtime,
            age_days,
            volume_id: row.volume_id,
            drive_label: row.drive_label,
            drive_online: row.drive_online,
            reasons,
            primary_reason,
            summary,
            only_copy,
            sensitivity: row.sensitivity,
            snippet,
            exif_taken_at: row.exif_taken_at,
            camera: row.camera,
            artist: row.artist,
            album: row.album,
            width: row.cached_width,
            height: row.cached_height,
            aspect_label: row.cached_aspect,
            dominant_color: row.cached_color,
            matched_collection_id,
            matched_collection_name,
            matched_collection_kind,
            suggested_destination,
            dismissed,
        };

        let mut rank_score = f64::from(asset.family.priority()) * 2.0
            + (asset.age_days.min(1000) as f64) * 0.01
            + if asset.only_copy { 6.0 } else { 0.0 }
            + if asset.matched_collection_id.is_some() { 5.0 } else { 0.0 }
            + if asset.reasons.contains(&ForgottenReason::BuriedInClutter) { 4.0 } else { 0.0 }
            + if !asset.drive_online { 3.0 } else { 0.0 };

        if let Some(q) = query_trimmed {
            let hybrid_sc = if !is_sensitive(&asset.sensitivity) || reveal_sensitive {
                query_hits.get(&asset.file_id).copied().unwrap_or(0.0)
            } else {
                0.0
            };
            let meta_sc = metadata_query_score(&asset, q, reveal_sensitive);
            if hybrid_sc <= 0.0 && meta_sc <= 0.0 {
                continue;
            }
            rank_score += hybrid_sc * 100.0 + meta_sc * 20.0;
        }

        if asset.dismissed {
            // Keep restored/shown dismissed items below active ones.
            asset.summary.push_str(" (Dismissed)");
            rank_score -= 500.0;
        }

        scored.push((rank_score, asset));
    }

    scored.sort_by(|a, b| {
        b.0.total_cmp(&a.0)
            .then_with(|| b.1.age_days.cmp(&a.1.age_days))
            .then_with(|| a.1.filename.cmp(&b.1.filename))
    });

    let all_items: Vec<ForgottenAsset> = scored.into_iter().map(|(_, a)| a).collect();

    let total_count = all_items.len();
    let total_bytes: i64 = all_items.iter().map(|a| a.size_bytes).sum();
    let offline_count = all_items.iter().filter(|a| !a.drive_online).count();
    let buried_count = all_items
        .iter()
        .filter(|a| a.reasons.contains(&ForgottenReason::BuriedInClutter))
        .count();
    let stranded_count = all_items
        .iter()
        .filter(|a| a.reasons.contains(&ForgottenReason::StrandedFromCollection))
        .count();
    let only_copy_count = all_items.iter().filter(|a| a.only_copy).count();

    let mut fam_acc: BTreeMap<AssetFamily, (usize, i64)> = BTreeMap::new();
    let mut drive_acc: BTreeMap<String, AssetDriveSummary> = BTreeMap::new();
    for item in &all_items {
        let fe = fam_acc.entry(item.family).or_insert((0, 0));
        fe.0 += 1;
        fe.1 += item.size_bytes;

        let de = drive_acc
            .entry(item.volume_id.clone())
            .or_insert_with(|| AssetDriveSummary {
                volume_id: item.volume_id.clone(),
                label: item.drive_label.clone(),
                online: item.drive_online,
                count: 0,
                total_bytes: 0,
            });
        de.count += 1;
        de.total_bytes += item.size_bytes;
    }

    let mut by_family: Vec<AssetFamilyCount> = fam_acc
        .into_iter()
        .map(|(family, (count, bytes))| AssetFamilyCount {
            family,
            label: family.label().to_string(),
            count,
            total_bytes: bytes,
        })
        .collect();
    by_family.sort_by(|a, b| b.count.cmp(&a.count).then_with(|| b.family.priority().cmp(&a.family.priority())));

    let mut by_drive: Vec<AssetDriveSummary> = drive_acc.into_values().collect();
    by_drive.sort_by(|a, b| b.count.cmp(&a.count).then_with(|| a.label.cmp(&b.label)));

    let limit = options.limit.unwrap_or(150).clamp(1, 500);
    let mut items = all_items;
    items.truncate(limit);

    Ok(ForgottenAssetsReport {
        scanned_at: now_ms,
        total_count,
        total_bytes,
        offline_count,
        buried_count,
        stranded_count,
        only_copy_count,
        dismissed_count,
        by_family,
        by_drive,
        items,
    })
}

/// Inspects file headers for the given online `file_ids` and caches width/height/aspect/color in `asset_meta`.
pub fn enrich_asset_metadata(conn: &Connection, file_ids: &[String]) -> Result<usize> {
    if file_ids.is_empty() {
        return Ok(0);
    }
    let now = now_ms();
    let mut enriched = 0usize;

    for fid in file_ids.iter().take(64) {
        let row: Option<(String, Option<String>)> = conn
            .query_row(
                "SELECT f.path, COALESCE(f.ext, '')
                 FROM files f
                 JOIN volumes v ON v.id = f.volume_id
                 WHERE f.id = ?1
                   AND f.deleted_at IS NULL
                   AND f.state NOT IN ('quarantine', 'missing')
                   AND v.status = 'online'
                   AND COALESCE(v.role, '') != 'backup'",
                params![fid],
                |r| Ok((r.get(0)?, r.get(1)?)),
            )
            .optional()?;
        let Some((path, ext_opt)) = row else {
            continue;
        };
        let ext = ext_opt.unwrap_or_default();
        let Some(family) = classify_asset_family(&ext) else {
            continue;
        };
        let p = Path::new(&path);
        if !p.is_file() {
            continue;
        }
        let header = inspect_visual_header(p).unwrap_or_default();
        conn.execute(
            "INSERT INTO asset_meta (file_id, asset_family, width, height, aspect_label, dominant_color, inspected_at)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7)
             ON CONFLICT(file_id) DO UPDATE SET
               asset_family = excluded.asset_family,
               width = COALESCE(excluded.width, asset_meta.width),
               height = COALESCE(excluded.height, asset_meta.height),
               aspect_label = COALESCE(excluded.aspect_label, asset_meta.aspect_label),
               dominant_color = COALESCE(excluded.dominant_color, asset_meta.dominant_color),
               inspected_at = excluded.inspected_at",
            params![
                fid,
                family.as_str(),
                header.width,
                header.height,
                header.aspect_label,
                header.dominant_color,
                now
            ],
        )?;
        enriched += 1;
    }

    Ok(enriched)
}

/// Persists a user dismissal (`Dismiss`) so an asset no longer surfaces in default Forgotten Assets scans.
pub fn dismiss_forgotten_asset(conn: &Connection, file_id: &str, reason: &str) -> Result<()> {
    let exists: bool = conn.query_row(
        "SELECT EXISTS(SELECT 1 FROM files WHERE id = ?1 AND deleted_at IS NULL)",
        params![file_id],
        |r| r.get(0),
    )?;
    if !exists {
        return Err(AegisError::NotFound(format!("file {file_id}")));
    }
    let clean_reason = if reason.trim().is_empty() { "dismissed" } else { reason.trim() };
    conn.execute(
        "INSERT INTO asset_dismissals (file_id, reason, dismissed_at)
         VALUES (?1, ?2, ?3)
         ON CONFLICT(file_id) DO UPDATE SET reason = excluded.reason, dismissed_at = excluded.dismissed_at",
        params![file_id, clean_reason, now_ms()],
    )?;
    Ok(())
}

/// Removes a previous dismissal so the asset appears again in scans.
pub fn undismiss_forgotten_asset(conn: &Connection, file_id: &str) -> Result<()> {
    conn.execute("DELETE FROM asset_dismissals WHERE file_id = ?1", params![file_id])?;
    Ok(())
}

/// Stages a `K1` reversible `Move` batch in the Simulator (`origin: "forgotten_asset_recovery"`),
/// validating every move against `PolicyContext` and rejecting offline, quarantined, or backup files.
pub fn propose_asset_recovery(store: &mut SqliteStore, items: &[AssetRecoverItem]) -> Result<String> {
    if items.is_empty() {
        return Err(AegisError::InvalidInput(
            "Select at least one asset to recover.".into(),
        ));
    }

    let policy_ctx = build_policy_context(store)?;
    let busy_paths = open_batch_paths(store.conn())?;
    let mut seen_files = HashSet::new();
    let mut reserved_dests = HashSet::new();
    let mut actions = Vec::with_capacity(items.len());

    for item in items {
        if !seen_files.insert(item.file_id.clone()) {
            continue;
        }
        let row: Option<RecoverTargetRow> = store
            .conn()
            .query_row(
                "SELECT f.path, f.name, f.state, f.deleted_at, f.atomic_unit_id,
                        COALESCE(v.status, 'offline'), v.role
                 FROM files f
                 JOIN volumes v ON v.id = f.volume_id
                 WHERE f.id = ?1",
                params![item.file_id],
                |r| {
                    Ok(RecoverTargetRow {
                        from_path: r.get(0)?,
                        filename: r.get(1)?,
                        state: r.get(2)?,
                        deleted_at: r.get(3)?,
                        atomic_unit_id: r.get(4)?,
                        vol_status: r.get(5)?,
                        vol_role: r.get(6)?,
                    })
                },
            )
            .optional()?;

        let Some(RecoverTargetRow {
            from_path,
            filename,
            state,
            deleted_at,
            atomic_unit_id,
            vol_status,
            vol_role,
        }) = row
        else {
            return Err(AegisError::NotFound(format!("file {}", item.file_id)));
        };

        if deleted_at.is_some() || state == "quarantine" || state == "missing" {
            return Err(AegisError::InvalidInput(format!(
                "Cannot recover `{filename}` because it is deleted or in quarantine."
            )));
        }
        if vol_role.as_deref() == Some("backup") {
            return Err(AegisError::PermissionDenied(
                "Backup drives are read-only and never modified (K4).".into(),
            ));
        }
        if vol_status != "online" {
            return Err(AegisError::InvalidInput(format!(
                "Cannot move `{filename}` because its drive is offline; plug the drive in first."
            )));
        }

        let from_p = Path::new(&from_path);
        if !from_p.is_file() {
            return Err(AegisError::InvalidInput(format!(
                "Source asset is missing on disk: {from_path}"
            )));
        }
        if busy_paths.contains(&path_key(from_p)) {
            return Err(AegisError::InvalidInput(format!(
                "`{filename}` already has a pending batch action in the Simulator."
            )));
        }

        let dest_dir_str = item.dest_dir.trim();
        if dest_dir_str.is_empty() {
            return Err(AegisError::InvalidInput(
                "Destination folder path must not be empty.".into(),
            ));
        }
        let dest_dir = Path::new(dest_dir_str);
        if path_contains_aegis_internal(dest_dir) {
            return Err(AegisError::PermissionDenied(
                "Cannot recover assets into Aegis internal directories.".into(),
            ));
        }
        if same_parent_dir(&from_path, dest_dir_str) {
            return Err(AegisError::InvalidInput(format!(
                "`{filename}` is already in `{dest_dir_str}`."
            )));
        }

        let dest_path = resolve_unique_dest(dest_dir, &filename, &mut reserved_dests);
        let dest_str = dest_path.to_string_lossy().to_string();

        let (unit_root, unit_allow_split) = if let Some(ref uid) = atomic_unit_id {
            store
                .conn()
                .query_row(
                    "SELECT root_path, allow_split_override FROM atomic_units WHERE id = ?1",
                    params![uid],
                    |r| Ok((Some(PathBuf::from(r.get::<_, String>(0)?)), r.get::<_, i32>(1)? != 0)),
                )
                .optional()?
                .unwrap_or((None, false))
        } else {
            (None, false)
        };

        let pa = PolicyAction {
            action_type: ActionType::Move,
            target_kind: TargetKind::File,
            agent: Some(FORGOTTEN_ASSETS_AGENT),
            from_path: from_p,
            to_path: Some(&dest_path),
            confidence: 0.92,
            atomic_unit_root: unit_root.as_deref(),
            unit_allow_split_override: unit_allow_split,
            file_unit_split_override: false,
            is_registered_cache_dir: false,
        };
        if let PolicyVerdict::Block(rule) = evaluate_policy(&pa, &policy_ctx) {
            return Err(AegisError::PermissionDenied(format!(
                "Policy blocked recovery move for `{filename}`: {rule}"
            )));
        }

        let reason = item
            .reason
            .as_deref()
            .map(str::trim)
            .filter(|s| !s.is_empty())
            .map(str::to_string)
            .unwrap_or_else(|| format!("Recover forgotten asset `{filename}` to {dest_dir_str}"));

        actions.push(ProposedActionInput {
            agent: Some(FORGOTTEN_ASSETS_AGENT.to_string()),
            action_type: ActionType::Move,
            target_kind: TargetKind::File,
            file_id: Some(item.file_id.clone()),
            atomic_unit_id,
            from_path,
            to_path: Some(dest_str),
            reason,
            confidence: 0.92,
            evidence_json: serde_json::json!({
                "source": "forgotten_asset_recovery",
                "dest_dir": dest_dir_str,
            })
            .to_string(),
        });
    }

    if actions.is_empty() {
        return Err(AegisError::InvalidInput(
            "No valid recovery moves remained to propose.".into(),
        ));
    }

    propose_batch(
        store,
        &RefilePlan {
            origin: FORGOTTEN_ASSETS_AGENT.to_string(),
            actions,
        },
    )
}

fn load_candidate_rows(conn: &Connection, volume_id: Option<&str>) -> Result<Vec<DbAssetRow>> {
    let base_sql = "SELECT f.id, f.volume_id, v.mount_point, f.path, f.name, COALESCE(f.ext, ''),
                           f.size, f.mtime, f.blake3, f.sensitivity,
                           COALESCE(NULLIF(TRIM(v.label), ''), v.mount_point, v.id) AS drive_label,
                           COALESCE(v.status = 'online', 0) AS drive_online,
                           m.exif_taken_at, m.camera, m.artist, m.album,
                           am.width, am.height, am.aspect_label, am.dominant_color
                    FROM files f
                    JOIN volumes v ON v.id = f.volume_id
                    LEFT JOIN file_meta m ON m.file_id = f.id
                    LEFT JOIN asset_meta am ON am.file_id = f.id
                    WHERE f.deleted_at IS NULL
                      AND f.state NOT IN ('quarantine', 'missing')
                      AND f.container_file_id IS NULL
                      AND f.atomic_unit_id IS NULL
                      AND COALESCE(v.role, '') != 'backup'";

    let map_row = |r: &rusqlite::Row<'_>| -> rusqlite::Result<DbAssetRow> {
        Ok(DbAssetRow {
            file_id: r.get(0)?,
            volume_id: r.get(1)?,
            mount_point: r.get(2)?,
            path: r.get(3)?,
            name: r.get(4)?,
            ext: r.get(5)?,
            size_bytes: r.get(6)?,
            mtime: r.get(7)?,
            blake3: r.get(8)?,
            sensitivity: r.get(9)?,
            drive_label: r.get(10)?,
            drive_online: r.get::<_, i64>(11)? != 0,
            exif_taken_at: r.get(12)?,
            camera: r.get(13)?,
            artist: r.get(14)?,
            album: r.get(15)?,
            cached_width: r.get(16)?,
            cached_height: r.get(17)?,
            cached_aspect: r.get(18)?,
            cached_color: r.get(19)?,
        })
    };

    let mut out = Vec::new();
    if let Some(vid) = volume_id {
        let sql = format!("{base_sql} AND f.volume_id = ?1");
        let mut st = conn.prepare(&sql)?;
        for r in st.query_map(params![vid], map_row)? {
            out.push(r?);
        }
    } else {
        let mut st = conn.prepare(base_sql)?;
        for r in st.query_map([], map_row)? {
            out.push(r?);
        }
    }
    Ok(out)
}

fn load_context_state(conn: &Connection) -> Result<ContextState> {
    let mut loc_by_col: HashMap<String, Vec<(String, String, bool, bool)>> = HashMap::new();
    {
        let mut st = conn.prepare(
            "SELECT l.collection_id, l.volume_id, l.root_path,
                    COALESCE(v.case_sensitive, 0),
                    COALESCE(v.status = 'online', 0) AND COALESCE(v.role, '') != 'backup'
             FROM collection_locations l
             LEFT JOIN volumes v ON v.id = l.volume_id",
        )?;
        for row in st.query_map([], |r| {
            Ok((
                r.get::<_, String>(0)?,
                r.get::<_, String>(1)?,
                r.get::<_, String>(2)?,
                r.get::<_, i64>(3)? != 0,
                r.get::<_, i64>(4)? != 0,
            ))
        })? {
            let (cid, vid, rp, cs, online) = row?;
            loc_by_col.entry(cid).or_default().push((vid, rp, cs, online));
        }
    }

    let mut member_dirs_by_col: HashMap<String, Vec<String>> = HashMap::new();
    let mut active_members = HashSet::new();
    let mut old_copy_members = HashSet::new();

    #[derive(Clone)]
    struct RawCol {
        id: String,
        kind: String,
        name: String,
        project_kind: Option<String>,
    }

    let mut raw_cols = Vec::new();
    let mut old_copy_ids = HashSet::new();
    {
        let mut st = conn.prepare(
            "SELECT id, kind, name, project_kind
             FROM collections
             WHERE state NOT IN ('left_as_is', 'not_a_project', 'dismissed', 'rejected', 'no')",
        )?;
        for row in st.query_map([], |r| {
            Ok(RawCol {
                id: r.get(0)?,
                kind: r.get(1)?,
                name: r.get(2)?,
                project_kind: r.get(3)?,
            })
        })? {
            let c = row?;
            if c.kind == "old_copy" {
                old_copy_ids.insert(c.id.clone());
            }
            raw_cols.push(c);
        }
    }

    let active_col_ids: HashSet<String> = raw_cols.iter().map(|c| c.id.clone()).collect();
    {
        let mut st = conn.prepare(
            "SELECT m.collection_id, m.file_id, m.path,
                    COALESCE(v.status = 'online', 0) AND COALESCE(v.role, '') != 'backup'
             FROM collection_members m
             LEFT JOIN volumes v ON v.id = m.volume_id",
        )?;
        for row in st.query_map([], |r| {
            Ok((
                r.get::<_, String>(0)?,
                r.get::<_, String>(1)?,
                r.get::<_, String>(2)?,
                r.get::<_, i64>(3)? != 0,
            ))
        })? {
            let (cid, fid, mpath, online) = row?;
            if !active_col_ids.contains(&cid) {
                continue;
            }
            if old_copy_ids.contains(&cid) {
                old_copy_members.insert(fid);
            } else {
                active_members.insert(fid);
                if online && let Some(parent) = Path::new(&mpath).parent() {
                    member_dirs_by_col
                        .entry(cid)
                        .or_default()
                        .push(parent.to_string_lossy().to_string());
                }
            }
        }
    }

    let mut locations = Vec::new();
    let mut matchable = Vec::new();

    for c in raw_cols {
        let is_old = c.kind == "old_copy";
        if let Some(locs) = loc_by_col.get(&c.id) {
            for (vid, rp, cs, _) in locs {
                locations.push(ActiveLocation {
                    is_old_copy: is_old,
                    volume_id: vid.clone(),
                    root_path: rp.clone(),
                    case_sensitive: *cs,
                });
            }
        }

        if is_old || c.kind == "software_package" || c.project_kind.as_deref() == Some("software") {
            continue;
        }
        if !matches!(
            c.kind.as_str(),
            "project" | "design_set" | "photo_subject" | "owners_folder" | "resource" | "series"
        ) {
            continue;
        }

        let online_dest = loc_by_col
            .get(&c.id)
            .and_then(|locs| {
                locs.iter()
                    .find(|(_, _, _, online)| *online)
                    .map(|(_, rp, _, _)| rp.clone())
            })
            .or_else(|| member_dirs_by_col.get(&c.id).and_then(|dirs| dirs.first().cloned()));

        let tokens = extract_match_tokens(&c.name);
        if !tokens.is_empty() {
            matchable.push(ActiveCollection {
                id: c.id,
                kind: c.kind,
                name: c.name,
                tokens,
                online_dest,
            });
        }
    }

    Ok(ContextState {
        matchable,
        locations,
        active_members,
        old_copy_members,
    })
}

fn build_copy_counts(conn: &Connection) -> Result<CopyCounts> {
    let mut by_hash: HashMap<Vec<u8>, usize> = HashMap::new();
    let mut by_name_size: HashMap<(String, i64), usize> = HashMap::new();

    let mut st = conn.prepare(
        "SELECT LOWER(f.name), f.size, f.blake3
         FROM files f
         JOIN volumes v ON v.id = f.volume_id
         WHERE f.deleted_at IS NULL
           AND f.state NOT IN ('quarantine', 'missing')
           AND f.container_file_id IS NULL
           AND COALESCE(v.role, '') != 'backup'",
    )?;
    for row in st.query_map([], |r| {
        Ok((
            r.get::<_, String>(0)?,
            r.get::<_, i64>(1)?,
            r.get::<_, Option<Vec<u8>>>(2)?,
        ))
    })? {
        let (lname, sz, hash_opt) = row?;
        *by_name_size.entry((lname, sz)).or_insert(0) += 1;
        if let Some(h) = hash_opt
            && !h.is_empty()
        {
            *by_hash.entry(h).or_insert(0) += 1;
        }
    }
    Ok(CopyCounts {
        by_hash,
        by_name_size,
    })
}

fn is_only_copy_of(row: &DbAssetRow, counts: &CopyCounts) -> bool {
    if let Some(ref h) = row.blake3
        && let Some(&c) = counts.by_hash.get(h)
    {
        return c <= 1;
    }
    let key = (row.name.to_ascii_lowercase(), row.size_bytes);
    counts.by_name_size.get(&key).copied().unwrap_or(1) <= 1
}

fn location_membership(
    volume_id: &str,
    path: &str,
    locations: &[ActiveLocation],
) -> (bool, bool) {
    let mut in_active = false;
    let mut in_old_copy = false;
    for loc in locations {
        if loc.volume_id == volume_id && path_is_within(path, &loc.root_path, loc.case_sensitive) {
            if loc.is_old_copy {
                in_old_copy = true;
            } else {
                in_active = true;
            }
        }
    }
    (in_active, in_old_copy)
}

fn path_has_excluded_system_dir(path: &Path) -> bool {
    path.parent().is_some_and(|parent| {
        parent.components().any(|comp| match comp {
            Component::Normal(seg) => {
                let lower = seg.to_string_lossy().to_ascii_lowercase();
                EXCLUDED_SYSTEM_DIRS.contains(&lower.as_str())
            }
            _ => false,
        })
    })
}

fn path_in_clutter_folder(path: &Path) -> bool {
    let Some(parent) = path.parent() else {
        return false;
    };
    parent.components().any(|comp| match comp {
        Component::Normal(seg) => {
            let lower = seg.to_string_lossy().to_ascii_lowercase();
            CLUTTER_DIR_SEGMENTS.contains(&lower.as_str())
        }
        _ => false,
    })
}

fn extract_match_tokens(s: &str) -> Vec<String> {
    let mut seen = HashSet::new();
    let mut out = Vec::new();
    for raw in s.split(|c: char| !c.is_alphanumeric()) {
        let lower = raw.trim().to_ascii_lowercase();
        if lower.len() >= 4 && !COLLECTION_MATCH_STOPWORDS.contains(&lower.as_str()) && seen.insert(lower.clone()) {
            out.push(lower);
        }
    }
    out
}

fn match_collection_for_path<'a>(
    path: &Path,
    filename: &str,
    collections: &'a [ActiveCollection],
) -> Option<&'a ActiveCollection> {
    let stem = Path::new(filename)
        .file_stem()
        .map(|s| s.to_string_lossy().to_ascii_lowercase())
        .unwrap_or_default();
    let parent_name = path
        .parent()
        .and_then(|p| p.file_name())
        .map(|s| s.to_string_lossy().to_ascii_lowercase())
        .unwrap_or_default();

    let file_tokens: HashSet<String> = extract_match_tokens(&format!("{stem} {parent_name}"))
        .into_iter()
        .collect();

    let mut best: Option<(usize, &'a ActiveCollection)> = None;
    for col in collections {
        let col_lower = col.name.to_ascii_lowercase();
        let exact_phrase_hit =
            col_lower.len() >= 4 && (stem.contains(&col_lower) || parent_name.contains(&col_lower));
        let token_hits = col.tokens.iter().filter(|t| file_tokens.contains(*t)).count();
        let score = if exact_phrase_hit {
            token_hits + 3
        } else if token_hits == col.tokens.len() || token_hits >= 2 {
            token_hits
        } else {
            0
        };
        if score > 0 && best.as_ref().is_none_or(|(bs, _)| score > *bs) {
            best = Some((score, col));
        }
    }
    best.map(|(_, col)| col)
}

fn load_safe_snippet(
    conn: &Connection,
    file_id: &str,
    sensitivity: &str,
    reveal_sensitive: bool,
) -> Result<Option<String>> {
    if !reveal_sensitive && is_sensitive(sensitivity) {
        let has_chunks: bool = conn.query_row(
            "SELECT EXISTS(SELECT 1 FROM chunks WHERE file_id = ?1)",
            params![file_id],
            |r| r.get(0),
        )?;
        return Ok(has_chunks.then(|| SENSITIVE_MASK.to_string()));
    }
    let text: Option<String> = conn
        .query_row(
            "SELECT text FROM chunks WHERE file_id = ?1 ORDER BY seq ASC LIMIT 1",
            params![file_id],
            |r| r.get(0),
        )
        .optional()?;
    Ok(text.map(|t| {
        let normalized: String = t.split_whitespace().collect::<Vec<_>>().join(" ");
        let mut s: String = normalized.chars().take(220).collect();
        if normalized.chars().count() > 220 {
            s.push('…');
        }
        s
    }))
}

fn metadata_query_score(asset: &ForgottenAsset, query: &str, reveal_sensitive: bool) -> f64 {
    let terms = extract_match_tokens_or_short(query);
    if terms.is_empty() {
        return 0.0;
    }
    let mut hay_parts = vec![
        asset.filename.to_ascii_lowercase(),
        asset.path.to_ascii_lowercase(),
        asset.ext.to_ascii_lowercase(),
        asset.family.label().to_ascii_lowercase(),
        asset.drive_label.to_ascii_lowercase(),
    ];
    if let Some(ref c) = asset.matched_collection_name {
        hay_parts.push(c.to_ascii_lowercase());
    }
    if let Some(ref cam) = asset.camera {
        hay_parts.push(cam.to_ascii_lowercase());
    }
    if let Some(ref art) = asset.artist {
        hay_parts.push(art.to_ascii_lowercase());
    }
    if let Some(ref alb) = asset.album {
        hay_parts.push(alb.to_ascii_lowercase());
    }
    if let Some(ref asp) = asset.aspect_label {
        hay_parts.push(asp.to_ascii_lowercase());
    }
    if let Some(ref hex) = asset.dominant_color {
        hay_parts.push(hex.to_ascii_lowercase());
    }
    if (!is_sensitive(&asset.sensitivity) || reveal_sensitive)
        && let Some(ref snip) = asset.snippet
    {
        hay_parts.push(snip.to_ascii_lowercase());
    }

    let hay = hay_parts.join(" ");
    let mut hits = 0usize;
    for t in &terms {
        if hay.contains(t.as_str()) {
            hits += 1;
        }
    }
    if hits == terms.len() {
        1.0 + hits as f64
    } else if hits > 0 && terms.len() > 1 {
        hits as f64 * 0.4
    } else {
        0.0
    }
}

fn extract_match_tokens_or_short(q: &str) -> Vec<String> {
    let mut out = Vec::new();
    for raw in q.split(|c: char| !c.is_alphanumeric() && c != '#') {
        let lower = raw.trim().to_ascii_lowercase();
        if lower.len() >= 2 && !out.contains(&lower) {
            out.push(lower);
        }
    }
    out
}

fn choose_primary_reason(
    reasons: &[ForgottenReason],
    drive_online: bool,
    has_match: bool,
) -> ForgottenReason {
    if has_match && reasons.contains(&ForgottenReason::StrandedFromCollection) {
        return ForgottenReason::StrandedFromCollection;
    }
    if !drive_online && reasons.contains(&ForgottenReason::OfflineDrive) {
        return ForgottenReason::OfflineDrive;
    }
    if reasons.contains(&ForgottenReason::BuriedInClutter) {
        return ForgottenReason::BuriedInClutter;
    }
    if reasons.contains(&ForgottenReason::DormantOnlyCopy) {
        return ForgottenReason::DormantOnlyCopy;
    }
    ForgottenReason::DormantAsset
}

fn build_summary(
    primary: ForgottenReason,
    age_days: i64,
    drive_label: &str,
    drive_online: bool,
    only_copy: bool,
    matched_name: Option<&str>,
) -> String {
    let copy_note = if only_copy { " · Only copy" } else { "" };
    match primary {
        ForgottenReason::StrandedFromCollection => format!(
            "Matches `{}` ({}d old){}",
            matched_name.unwrap_or("collection"),
            age_days,
            if drive_online { "" } else { " · Drive offline" }
        ),
        ForgottenReason::OfflineDrive => format!(
            "Stored on unplugged drive {drive_label} ({age_days}d old){copy_note}"
        ),
        ForgottenReason::BuriedInClutter => {
            format!("Buried in Downloads/clutter for {age_days} days{copy_note}")
        }
        ForgottenReason::DormantOnlyCopy => {
            format!("Dormant for {age_days} days outside projects · Only known copy")
        }
        ForgottenReason::DormantAsset => format!("Untouched for {age_days} days outside active projects"),
    }
}

pub(crate) fn path_key(p: &Path) -> String {
    normalize_path(p, false).0
}

pub(crate) fn same_parent_dir(file_path: &str, dest_dir: &str) -> bool {
    let Some(parent) = Path::new(file_path).parent() else {
        return false;
    };
    path_key(parent) == path_key(Path::new(dest_dir))
}

pub(crate) fn open_batch_paths(conn: &Connection) -> Result<HashSet<String>> {
    let mut st = conn.prepare(
        "SELECT a.from_path
         FROM actions a
         JOIN action_batches b ON b.id = a.batch_id
         WHERE b.status IN ('draft', 'simulated', 'approved', 'executing')
           AND a.status IN ('pending', 'approved')",
    )?;
    let mut out = HashSet::new();
    for r in st.query_map([], |row| row.get::<_, String>(0))? {
        let p = r?;
        out.insert(path_key(Path::new(&p)));
    }
    Ok(out)
}

pub(crate) fn resolve_unique_dest(dest_dir: &Path, filename: &str, reserved: &mut HashSet<String>) -> PathBuf {
    let candidate = dest_dir.join(filename);
    let key = path_key(&candidate);
    if !candidate.exists() && !reserved.contains(&key) {
        reserved.insert(key);
        return candidate;
    }

    let stem = Path::new(filename)
        .file_stem()
        .map(|s| s.to_string_lossy().to_string())
        .unwrap_or_else(|| filename.to_string());
    let ext = Path::new(filename)
        .extension()
        .map(|s| format!(".{}", s.to_string_lossy()))
        .unwrap_or_default();

    for idx in 1..=999 {
        let alt_name = format!("{stem} ({idx}){ext}");
        let alt = dest_dir.join(alt_name);
        let alt_key = path_key(&alt);
        if !alt.exists() && !reserved.contains(&alt_key) {
            reserved.insert(alt_key);
            return alt;
        }
    }
    let fallback = dest_dir.join(format!(
        "{stem}_{}{ext}",
        local_ms(now_ms()).max(0) / 1000
    ));
    reserved.insert(path_key(&fallback));
    fallback
}
