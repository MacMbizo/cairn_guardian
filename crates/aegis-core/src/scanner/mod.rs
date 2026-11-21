//! Tier-1 discovery, `.aegisignore` + immutable exclusions, `atomic_units` (`ADR-004`),
//! `cache_dirs` recognition, optional Tier-2 local text chunking + BLAKE3 hashing, and the
//! post-scan sweep that retires index rows for files deleted since the last scan.

pub mod os_adapter;

use crate::contracts::{CoverageErrorItem, CoverageReport, RootConfig, ScanStatus};
use crate::db::SqliteStore;
use crate::db::files::{
    DiscoveredFileInput, delete_chunks_for_file, insert_chunk, normalize_path, path_is_within,
    soft_delete_file_in_tx, unchanged_hash, upsert_discovered_file,
};
use crate::error::{AegisError, Result};
use crate::scanner::os_adapter::{OsVolumeMeta, inspect_path_metadata, inspect_volume_for_path};
use crate::{new_id, now_ms};
use ignore::gitignore::{Gitignore, GitignoreBuilder};
use rusqlite::{Connection, OptionalExtension, params};
use std::collections::HashSet;
use std::ffi::OsString;
use std::fs::File;
use std::io::Read;
use std::path::{Path, PathBuf};

/// Files written between intermediate commits during a scan.
const SCAN_COMMIT_EVERY: i64 = 1000;
/// Files between progress callbacks.
const SCAN_PROGRESS_EVERY: i64 = 250;

/// Live counters reported to the progress callback during a scan.
#[derive(Debug, Clone)]
pub struct ScanProgress {
    pub job_id: String,
    pub root_path: String,
    pub files_seen: i64,
    pub files_indexed: i64,
    pub coverage_pct: f64,
}

/// Builds a `.aegisignore` matcher for a scan root plus optional persisted rules.
fn build_ignore_matcher(root: &Path, custom_rules: Option<&str>) -> Gitignore {
    let mut builder = GitignoreBuilder::new(root);
    let ignore_file = root.join(".aegisignore");
    if ignore_file.is_file() {
        let _ = builder.add(&ignore_file);
    }
    if let Some(raw) = custom_rules {
        for line in raw.lines() {
            let trimmed = line.trim();
            if !trimmed.is_empty() && !trimmed.starts_with('#') {
                let _ = builder.add_line(None, trimmed);
            }
        }
    }
    builder.build().unwrap_or_else(|_| Gitignore::empty())
}

fn is_immutable_excluded_dir_name(name: &str) -> bool {
    [".aegis", ".aegis-quarantine", "System Volume Information", "$RECYCLE.BIN"]
        .iter()
        .any(|n| name.eq_ignore_ascii_case(n))
}

/// Directories skipped because of where they sit: Windows `Recovery` at a volume root, and the
/// Backup drive's `_Previous versions` area (spec §7.7), which holds old copies, not user files.
fn is_positional_excluded_dir(dir: &Path, name: &str, canonical_root: &Path) -> bool {
    let parent = dir.parent();
    if name.eq_ignore_ascii_case("Recovery")
        && parent == Some(canonical_root)
        && canonical_root.parent().is_none()
    {
        return true;
    }
    name.eq_ignore_ascii_case("_Previous versions")
        && parent
            .and_then(|p| p.file_name())
            .and_then(|n| n.to_str())
            .is_some_and(|n| n.eq_ignore_ascii_case("Aegis Backup"))
}

fn detect_cache_dir_kind(dir: &Path, name: &str) -> Option<&'static str> {
    if name.eq_ignore_ascii_case("node_modules") {
        return Some("node_modules");
    }
    if name.eq_ignore_ascii_case(".venv") || name.eq_ignore_ascii_case("venv") {
        return Some("python_venv");
    }
    if name.eq_ignore_ascii_case(".gradle") {
        return Some("gradle");
    }
    if name.eq_ignore_ascii_case("target")
        && dir
            .parent()
            .is_some_and(|p| p.join("Cargo.toml").is_file() || dir.join("CACHEDIR.TAG").is_file())
    {
        return Some("rust_target");
    }
    None
}

/// Detects an atomic-unit marker from a directory's already-listed entry names
/// (no second `read_dir`).
fn detect_atomic_unit_marker(dir: &Path, names: &[OsString]) -> Option<(&'static str, String)> {
    // Check directory bundle extensions first (macOS .app / .photoslibrary)
    if let Some(name) = dir.file_name().and_then(|n| n.to_str())
        && (name.ends_with(".app") || name.ends_with(".photoslibrary"))
    {
        return Some(("mac_bundle", name.to_string()));
    }

    const MARKERS: &[(&str, &str)] = &[
        (".aegisboundary", "custom"),
        (".git", "git_repo"),
        ("Cargo.toml", "rust_crate"),
        ("package.json", "node_pkg"),
        ("pyproject.toml", "python_pkg"),
        ("go.mod", "go_mod"),
        ("pom.xml", "jvm_pkg"),
        ("build.gradle", "jvm_pkg"),
    ];

    for (marker, unit_type) in MARKERS {
        if names.iter().any(|n| n.as_os_str() == *marker) {
            return Some((unit_type, (*marker).to_string()));
        }
    }

    for fname in names {
        let s = fname.to_string_lossy();
        if s.ends_with(".sln") || s.ends_with(".csproj") {
            return Some(("ide_proj", "*.sln/*.csproj".to_string()));
        }
        if s.ends_with(".xcodeproj") {
            return Some(("ide_proj", "*.xcodeproj".to_string()));
        }
        if s.ends_with(".prproj") {
            return Some(("premiere_proj", "*.prproj".to_string()));
        }
    }
    None
}

fn register_atomic_unit(
    conn: &Connection,
    vol: &OsVolumeMeta,
    dir: &Path,
    unit_type: &str,
    marker: &str,
) -> Result<String> {
    let (display_root, _) = normalize_path(dir, vol.case_sensitive);
    let now = now_ms();
    let existing: Option<String> = conn
        .query_row(
            "SELECT id FROM atomic_units WHERE volume_id = ?1 AND root_path = ?2",
            params![vol.id, display_root],
            |r| r.get(0),
        )
        .optional()?;

    if let Some(id) = existing {
        conn.execute(
            "UPDATE atomic_units SET unit_type = ?1, marker_file = ?2, updated_at = ?3 WHERE id = ?4",
            params![unit_type, marker, now, id],
        )?;
        return Ok(id);
    }

    let id = new_id();
    conn.execute(
        "INSERT INTO atomic_units (id, volume_id, root_path, unit_type, marker_file, created_at, updated_at)
         VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?6)",
        params![id, vol.id, display_root, unit_type, marker, now],
    )?;
    Ok(id)
}

/// Total bytes and file count beneath `dir` (symlinks not followed).
pub fn dir_size_and_count(dir: &Path) -> (i64, i64) {
    let mut bytes: i64 = 0;
    let mut count: i64 = 0;
    let mut stack = vec![dir.to_path_buf()];
    while let Some(cur) = stack.pop() {
        let Ok(rd) = std::fs::read_dir(&cur) else {
            continue;
        };
        for entry in rd.flatten() {
            let p = entry.path();
            let Ok(meta) = std::fs::symlink_metadata(&p) else {
                continue;
            };
            if meta.file_type().is_symlink() {
                continue;
            }
            if meta.is_dir() {
                stack.push(p);
            } else {
                bytes = bytes.saturating_add(meta.len() as i64);
                count += 1;
            }
        }
    }
    (bytes, count)
}

fn upsert_cache_dir(
    conn: &Connection,
    vol: &OsVolumeMeta,
    atomic_unit_id: Option<&str>,
    dir: &Path,
    cache_kind: &str,
    source_max_mtime: Option<i64>,
) -> Result<()> {
    let (display_path, path_hash) = normalize_path(dir, vol.case_sensitive);
    let (size_bytes, file_count) = dir_size_and_count(dir);
    let now = now_ms();

    let existing: Option<String> = conn
        .query_row(
            "SELECT id FROM cache_dirs WHERE volume_id = ?1 AND path_hash = ?2 AND state = 'active'",
            params![vol.id, path_hash.as_slice()],
            |r| r.get(0),
        )
        .optional()?;

    if let Some(id) = existing {
        conn.execute(
            "UPDATE cache_dirs
             SET atomic_unit_id = ?1, path = ?2, cache_kind = ?3, size_bytes = ?4,
                 file_count = ?5, source_max_mtime = ?6, last_seen = ?7, updated_at = ?7
             WHERE id = ?8",
            params![
                atomic_unit_id,
                display_path,
                cache_kind,
                size_bytes,
                file_count,
                source_max_mtime,
                now,
                id
            ],
        )?;
    } else {
        let id = new_id();
        conn.execute(
            "INSERT INTO cache_dirs (
                id, volume_id, atomic_unit_id, path, path_hash, cache_kind,
                size_bytes, file_count, source_max_mtime, state, last_seen, created_at, updated_at
             ) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, 'active', ?10, ?10, ?10)",
            params![
                id,
                vol.id,
                atomic_unit_id,
                display_path,
                path_hash.as_slice(),
                cache_kind,
                size_bytes,
                file_count,
                source_max_mtime,
                now
            ],
        )?;
    }
    Ok(())
}

fn compute_blake3_if_local(path: &Path, cloud_state: &str) -> Result<Option<Vec<u8>>> {
    if cloud_state != "local" {
        return Ok(None);
    }
    let mut f = File::open(path)?;
    let mut hasher = blake3::Hasher::new();
    let mut buf = [0u8; 65536];
    loop {
        let n = f.read(&mut buf)?;
        if n == 0 {
            break;
        }
        hasher.update(&buf[..n]);
    }
    Ok(Some(hasher.finalize().as_bytes().to_vec()))
}

fn is_plain_text_ext(ext: Option<&str>) -> bool {
    matches!(
        ext,
        Some("txt" | "md" | "markdown" | "rs" | "ts" | "tsx" | "js" | "jsx" | "json" | "toml" | "yaml" | "yml" | "csv" | "py" | "sql" | "html" | "css" | "log")
    )
}

/// (Re)chunks a local text file. When `content_unchanged` and chunks already exist this is a
/// no-op; otherwise stale chunks are replaced so search never serves outdated text.
fn refresh_text_chunks_if_local(
    conn: &Connection,
    file_id: &str,
    path: &Path,
    cloud_state: &str,
    size: i64,
    content_unchanged: bool,
) -> Result<()> {
    let existing_chunks: i64 = conn.query_row(
        "SELECT COUNT(*) FROM chunks WHERE file_id = ?1",
        params![file_id],
        |r| r.get(0),
    )?;
    if content_unchanged && existing_chunks > 0 {
        return Ok(());
    }
    if existing_chunks > 0 {
        delete_chunks_for_file(conn, file_id)?;
    }

    if cloud_state != "local" || size <= 0 || size > 2 * 1024 * 1024 {
        return Ok(());
    }
    let ext = path
        .extension()
        .and_then(|s| s.to_str())
        .map(|s| s.to_lowercase());
    if !is_plain_text_ext(ext.as_deref()) {
        return Ok(());
    }

    let bytes = std::fs::read(path)?;
    let text = String::from_utf8_lossy(&bytes);
    let trimmed = text.trim();
    if trimmed.is_empty() {
        return Ok(());
    }

    // Selective chunking (ADR-002): cap at first 8 ~1500-char chunks in Sprint 0
    let mut chunk_idx = 0i64;
    let mut cur = String::new();
    for line in trimmed.lines() {
        if cur.len() + line.len() + 1 > 1500 && !cur.is_empty() {
            let tokens = (cur.split_whitespace().count()) as i64;
            insert_chunk(conn, file_id, chunk_idx, &cur, tokens.max(1), chunk_idx == 0)?;
            chunk_idx += 1;
            cur.clear();
            if chunk_idx >= 8 {
                break;
            }
        }
        if !cur.is_empty() {
            cur.push('\n');
        }
        cur.push_str(line);
    }
    if !cur.is_empty() && chunk_idx < 8 {
        let tokens = (cur.split_whitespace().count()) as i64;
        insert_chunk(conn, file_id, chunk_idx, &cur, tokens.max(1), chunk_idx == 0)?;
    }

    Ok(())
}

pub fn coverage_pct_from_counts(seen: i64, skipped_perm: i64, skipped_cloud: i64) -> f64 {
    if seen <= 0 {
        return 100.0;
    }
    let covered = (seen - skipped_perm - skipped_cloud).max(0);
    let pct = (covered as f64 / seen as f64) * 100.0;
    if (skipped_perm > 0 || skipped_cloud > 0) && pct >= 100.0 {
        99.99
    } else {
        (pct * 100.0).round() / 100.0
    }
}

#[derive(Default)]
struct WalkStats {
    files_seen: i64,
    files_indexed: i64,
    files_skipped_perm: i64,
    files_skipped_cloud: i64,
    errors: Vec<CoverageErrorItem>,
    /// Directories that could not be listed: rows beneath them must not be swept as deleted.
    unreadable_dirs: Vec<String>,
    /// Files that exist but could not be (re)indexed this pass: never swept.
    unreadable_files: HashSet<String>,
}

struct WalkCtx<'a> {
    canonical_root: &'a Path,
    vol_meta: &'a OsVolumeMeta,
    matcher: &'a Gitignore,
    run_tier2: bool,
    job_id: &'a str,
    display_root: &'a str,
}

fn walk_root(
    conn: &Connection,
    w: &WalkCtx<'_>,
    stats: &mut WalkStats,
    progress: &mut dyn FnMut(&ScanProgress),
) -> Result<()> {
    let mut visited_inodes: HashSet<Vec<u8>> = HashSet::new();
    let mut since_commit: i64 = 0;
    let mut since_progress: i64 = 0;

    // DFS explicit stack so we preserve innermost-wins atomic_unit inheritance and never descend into loops/caches
    let mut dir_stack: Vec<(PathBuf, Option<String>)> = vec![(w.canonical_root.to_path_buf(), None)];

    while let Some((dir, inherited_unit)) = dir_stack.pop() {
        if let Some(dir_name) = dir.file_name().and_then(|s| s.to_str()) {
            if is_immutable_excluded_dir_name(dir_name)
                || is_positional_excluded_dir(&dir, dir_name, w.canonical_root)
            {
                continue;
            }
            if dir != w.canonical_root && w.matcher.matched(&dir, true).is_ignore() {
                continue;
            }
            if let Some(cache_kind) = detect_cache_dir_kind(&dir, dir_name) {
                let parent_mtime = dir
                    .parent()
                    .and_then(|p| std::fs::metadata(p).ok())
                    .and_then(|m| m.modified().ok())
                    .and_then(|st| st.duration_since(std::time::UNIX_EPOCH).ok())
                    .map(|d| d.as_millis() as i64);
                upsert_cache_dir(
                    conn,
                    w.vol_meta,
                    inherited_unit.as_deref(),
                    &dir,
                    cache_kind,
                    parent_mtime,
                )?;
                continue;
            }
        }

        // Symlink / junction loop protection (FR-001.4). Cloud-file reparse points are NOT symlinks.
        if let Ok(Some(st)) = inspect_path_metadata(&dir) {
            if st.is_symlink && dir != w.canonical_root {
                continue;
            }
            if let Some(idx) = st.file_index
                && !visited_inodes.insert(idx)
            {
                continue;
            }
        }

        let entries: Vec<std::fs::DirEntry> = match std::fs::read_dir(&dir) {
            Ok(rd) => {
                let mut v = Vec::new();
                for entry in rd {
                    match entry {
                        Ok(e) => v.push(e),
                        Err(e) => {
                            stats.files_seen += 1;
                            stats.files_skipped_perm += 1;
                            stats.unreadable_dirs.push(dir.to_string_lossy().to_string());
                            stats.errors.push(CoverageErrorItem {
                                path: dir.display().to_string(),
                                code: "permission_denied".into(),
                                message: e.to_string(),
                            });
                        }
                    }
                }
                v
            }
            Err(e) => {
                stats.files_seen += 1;
                stats.files_skipped_perm += 1;
                stats.unreadable_dirs.push(dir.to_string_lossy().to_string());
                stats.errors.push(CoverageErrorItem {
                    path: dir.display().to_string(),
                    code: if e.kind() == std::io::ErrorKind::PermissionDenied {
                        "permission_denied".into()
                    } else {
                        "io_error".into()
                    },
                    message: e.to_string(),
                });
                continue;
            }
        };

        let names: Vec<OsString> = entries.iter().map(|e| e.file_name()).collect();
        let current_unit = match detect_atomic_unit_marker(&dir, &names) {
            Some((unit_type, marker)) => Some(register_atomic_unit(
                conn,
                w.vol_meta,
                &dir,
                unit_type,
                &marker,
            )?),
            None => inherited_unit.clone(),
        };

        for entry in entries {
            let path = entry.path();
            let ftype = match entry.file_type() {
                Ok(ft) => ft,
                Err(e) => {
                    stats.files_seen += 1;
                    stats.files_skipped_perm += 1;
                    stats.unreadable_files.insert(path.to_string_lossy().to_string());
                    stats.errors.push(CoverageErrorItem {
                        path: path.display().to_string(),
                        code: "permission_denied".into(),
                        message: e.to_string(),
                    });
                    continue;
                }
            };

            if ftype.is_symlink() {
                continue;
            }

            if ftype.is_dir() {
                dir_stack.push((path, current_unit.clone()));
                continue;
            }

            if w.matcher.matched(&path, false).is_ignore() {
                continue;
            }

            stats.files_seen += 1;
            let stat = match inspect_path_metadata(&path) {
                Ok(Some(st)) => st,
                Ok(None) => continue,
                Err(e) => {
                    stats.files_skipped_perm += 1;
                    stats.unreadable_files.insert(path.to_string_lossy().to_string());
                    let ipc = e.to_ipc();
                    stats.errors.push(CoverageErrorItem {
                        path: path.display().to_string(),
                        code: ipc.code,
                        message: ipc.message,
                    });
                    continue;
                }
            };

            let is_placeholder = stat.cloud_state == "placeholder_remote";
            let mut content_unchanged = false;
            let blake3 = if w.run_tier2 && !is_placeholder {
                let (_, ph) = normalize_path(&path, w.vol_meta.case_sensitive);
                match unchanged_hash(conn, &w.vol_meta.id, &ph, stat.size, stat.mtime)? {
                    Some(h) => {
                        content_unchanged = true;
                        Some(h)
                    }
                    None => match compute_blake3_if_local(&path, &stat.cloud_state) {
                        Ok(h) => h,
                        Err(e) => {
                            stats.files_skipped_perm += 1;
                            stats.unreadable_files.insert(path.to_string_lossy().to_string());
                            let ipc = e.to_ipc();
                            stats.errors.push(CoverageErrorItem {
                                path: path.display().to_string(),
                                code: ipc.code,
                                message: ipc.message,
                            });
                            continue;
                        }
                    },
                }
            } else {
                None
            };

            let input = DiscoveredFileInput {
                volume_id: w.vol_meta.id.clone(),
                has_stable_inode: w.vol_meta.has_stable_inode,
                case_sensitive: w.vol_meta.case_sensitive,
                file_index: stat.file_index,
                nlink: stat.nlink,
                atomic_unit_id: current_unit.clone(),
                path: path.clone(),
                size: stat.size,
                mtime: stat.mtime,
                ctime: stat.ctime,
                blake3,
                cloud_state: stat.cloud_state.clone(),
            };

            let file_id = upsert_discovered_file(conn, &input)?;
            if is_placeholder {
                stats.files_skipped_cloud += 1;
            } else {
                stats.files_indexed += 1;
                if w.run_tier2 {
                    let _ = refresh_text_chunks_if_local(
                        conn,
                        &file_id,
                        &path,
                        &stat.cloud_state,
                        stat.size,
                        content_unchanged,
                    );
                }
            }

            since_commit += 1;
            since_progress += 1;
            if since_commit >= SCAN_COMMIT_EVERY {
                conn.execute_batch("COMMIT; BEGIN;")?;
                since_commit = 0;
            }
            if since_progress >= SCAN_PROGRESS_EVERY {
                since_progress = 0;
                progress(&ScanProgress {
                    job_id: w.job_id.to_string(),
                    root_path: w.display_root.to_string(),
                    files_seen: stats.files_seen,
                    files_indexed: stats.files_indexed,
                    coverage_pct: coverage_pct_from_counts(
                        stats.files_seen,
                        stats.files_skipped_perm,
                        stats.files_skipped_cloud,
                    ),
                });
            }
        }
    }
    Ok(())
}

/// Retires index rows under `root` that were not seen in this pass (deleted or now ignored
/// outside Aegis), except beneath directories/files that could not be read this time.
/// Returns the number of rows retired.
fn sweep_unseen(
    conn: &Connection,
    vol: &OsVolumeMeta,
    display_root: &str,
    started_at: i64,
    stats: &WalkStats,
) -> Result<usize> {
    let protected = |path: &str| -> bool {
        stats.unreadable_files.iter().any(|f| path_is_within(path, f, vol.case_sensitive))
            || stats
                .unreadable_dirs
                .iter()
                .any(|d| path_is_within(path, d, vol.case_sensitive))
    };

    let stale_files: Vec<(String, String)> = {
        let mut stmt = conn.prepare(
            "SELECT id, path FROM files
             WHERE volume_id = ?1 AND last_seen < ?2 AND deleted_at IS NULL
               AND container_file_id IS NULL AND state NOT IN ('quarantine','missing')",
        )?;
        let rows = stmt.query_map(params![vol.id, started_at], |r| Ok((r.get(0)?, r.get(1)?)))?;
        let mut out = Vec::new();
        for r in rows {
            let (id, path): (String, String) = r?;
            if path_is_within(&path, display_root, vol.case_sensitive) && !protected(&path) {
                out.push((id, path));
            }
        }
        out
    };
    for (id, _) in &stale_files {
        soft_delete_file_in_tx(conn, id)?;
    }

    let stale_caches: Vec<String> = {
        let mut stmt = conn.prepare(
            "SELECT id, path FROM cache_dirs
             WHERE volume_id = ?1 AND last_seen < ?2 AND state = 'active'",
        )?;
        let rows = stmt.query_map(params![vol.id, started_at], |r| {
            Ok((r.get::<_, String>(0)?, r.get::<_, String>(1)?))
        })?;
        let mut out = Vec::new();
        for r in rows {
            let (id, path) = r?;
            if path_is_within(&path, display_root, vol.case_sensitive) && !protected(&path) {
                out.push(id);
            }
        }
        out
    };
    let now = now_ms();
    for id in &stale_caches {
        conn.execute(
            "UPDATE cache_dirs SET state = 'missing', updated_at = ?1 WHERE id = ?2",
            params![now, id],
        )?;
    }
    Ok(stale_files.len() + stale_caches.len())
}

/// Scans a configured root path (`tiers` containing `1` for Tier-1 metadata, `2` for local hash + text chunks).
pub fn scan_root(store: &mut SqliteStore, root: &Path, tiers: &[u8]) -> Result<ScanStatus> {
    scan_root_with_progress(store, root, tiers, &mut |_| {})
}

/// Like [`scan_root`], reporting live counters to `progress` every few hundred files.
pub fn scan_root_with_progress(
    store: &mut SqliteStore,
    root: &Path,
    tiers: &[u8],
    progress: &mut dyn FnMut(&ScanProgress),
) -> Result<ScanStatus> {
    scan_root_inner(store, root, tiers, None, progress)
}

fn scan_root_inner(
    store: &mut SqliteStore,
    root: &Path,
    tiers: &[u8],
    volume_id: Option<&str>,
    progress: &mut dyn FnMut(&ScanProgress),
) -> Result<ScanStatus> {
    let canonical_root = std::fs::canonicalize(root).unwrap_or_else(|_| root.to_path_buf());
    let vol_info = match volume_id {
        Some(id) => store
            .list_volumes()?
            .into_iter()
            .find(|v| v.id == id)
            .ok_or_else(|| AegisError::NotFound(format!("volume {id}")))?,
        None => store.ensure_volume_for_path(&canonical_root)?,
    };
    let mut vol_meta = inspect_volume_for_path(&canonical_root)?;
    // The registered volume (passport-identified drives included) owns these files.
    vol_meta.id = vol_info.id.clone();
    vol_meta.has_stable_inode = vol_info.has_stable_inode;
    vol_meta.case_sensitive = vol_info.case_sensitive;
    let (display_root, _) = normalize_path(&canonical_root, vol_info.case_sensitive);

    let global_ignore = store.get_setting("ignore.global", "global")?;
    let root_ignore_key = format!("ignore.root:{}", display_root);
    let root_ignore = store.get_setting(&root_ignore_key, "global")?;
    let combined_ignore = [global_ignore, root_ignore]
        .into_iter()
        .flatten()
        .map(|raw| serde_json::from_str::<String>(&raw).unwrap_or(raw))
        .collect::<Vec<_>>()
        .join("\n");
    let matcher = build_ignore_matcher(
        &canonical_root,
        if combined_ignore.is_empty() {
            None
        } else {
            Some(&combined_ignore)
        },
    );

    let job_id = new_id();
    let started_at = now_ms();
    let max_tier = tiers.iter().copied().max().unwrap_or(1);

    store.conn().execute(
        "INSERT INTO scan_jobs (id, root_path, tier, status, started_at)
         VALUES (?1, ?2, ?3, 'running', ?4)",
        params![job_id, display_root, i32::from(max_tier), started_at],
    )?;

    let mut stats = WalkStats::default();
    let walk_ctx = WalkCtx {
        canonical_root: &canonical_root,
        vol_meta: &vol_meta,
        matcher: &matcher,
        run_tier2: tiers.contains(&2) || tiers.contains(&3),
        job_id: &job_id,
        display_root: &display_root,
    };

    // Batch writes into periodic transactions; always close the transaction, even on error,
    // so the connection is never left inside an open transaction.
    let conn = store.conn();
    conn.execute_batch("BEGIN;")?;
    let walk_res = walk_root(conn, &walk_ctx, &mut stats, progress).and_then(|()| {
        sweep_unseen(conn, &vol_meta, &display_root, started_at, &stats).map(|_| ())
    });
    let commit_res = conn.execute_batch("COMMIT;");
    if let Err(e) = walk_res {
        let _ = conn.execute(
            "UPDATE scan_jobs SET status = 'failed', finished_at = ?1 WHERE id = ?2",
            params![now_ms(), job_id],
        );
        return Err(e);
    }
    commit_res?;

    let finished_at = now_ms();
    let errors_json = serde_json::to_string(&stats.errors)?;
    let status = if stats.errors.is_empty() {
        "completed"
    } else {
        "degraded"
    };

    store.conn().execute(
        "UPDATE scan_jobs
         SET status = ?1, files_seen = ?2, files_indexed = ?3,
             files_skipped_perm = ?4, files_skipped_cloud = ?5,
             errors_json = ?6, finished_at = ?7
         WHERE id = ?8",
        params![
            status,
            stats.files_seen,
            stats.files_indexed,
            stats.files_skipped_perm,
            stats.files_skipped_cloud,
            errors_json,
            finished_at,
            job_id
        ],
    )?;

    let coverage_pct = coverage_pct_from_counts(
        stats.files_seen,
        stats.files_skipped_perm,
        stats.files_skipped_cloud,
    );
    progress(&ScanProgress {
        job_id: job_id.clone(),
        root_path: display_root.clone(),
        files_seen: stats.files_seen,
        files_indexed: stats.files_indexed,
        coverage_pct,
    });
    Ok(ScanStatus {
        job_id,
        root_path: display_root,
        tier: max_tier,
        status: status.to_string(),
        files_seen: stats.files_seen,
        files_indexed: stats.files_indexed,
        files_skipped_perm: stats.files_skipped_perm,
        files_skipped_cloud: stats.files_skipped_cloud,
        coverage_pct,
        started_at,
        finished_at: Some(finished_at),
    })
}

pub fn latest_scan_status(conn: &Connection, job_id: Option<&str>) -> Result<Option<ScanStatus>> {
    let map = |r: &rusqlite::Row<'_>| -> rusqlite::Result<ScanStatus> {
        let seen: i64 = r.get(4)?;
        let skip_p: i64 = r.get(6)?;
        let skip_c: i64 = r.get(7)?;
        Ok(ScanStatus {
            job_id: r.get(0)?,
            root_path: r.get(1)?,
            tier: r.get::<_, i32>(2)? as u8,
            status: r.get(3)?,
            files_seen: seen,
            files_indexed: r.get(5)?,
            files_skipped_perm: skip_p,
            files_skipped_cloud: skip_c,
            coverage_pct: coverage_pct_from_counts(seen, skip_p, skip_c),
            started_at: r.get(8)?,
            finished_at: r.get(9)?,
        })
    };
    let row = match job_id {
        Some(id) => conn
            .query_row(
                "SELECT id, root_path, tier, status, files_seen, files_indexed,
                        files_skipped_perm, files_skipped_cloud, started_at, finished_at
                 FROM scan_jobs WHERE id = ?1",
                params![id],
                map,
            )
            .optional()?,
        None => conn
            .query_row(
                "SELECT id, root_path, tier, status, files_seen, files_indexed,
                        files_skipped_perm, files_skipped_cloud, started_at, finished_at
                 FROM scan_jobs ORDER BY started_at DESC LIMIT 1",
                [],
                map,
            )
            .optional()?,
    };
    Ok(row)
}

pub fn get_coverage_report(conn: &Connection, job_id: Option<&str>) -> Result<CoverageReport> {
    let status = latest_scan_status(conn, job_id)?
        .ok_or_else(|| AegisError::NotFound("no scan job found".into()))?;
    let errors_json: String = conn.query_row(
        "SELECT errors_json FROM scan_jobs WHERE id = ?1",
        params![status.job_id],
        |r| r.get(0),
    )?;
    let errors: Vec<CoverageErrorItem> = serde_json::from_str(&errors_json)?;
    Ok(CoverageReport {
        job_id: status.job_id,
        root_path: status.root_path,
        total_seen: status.files_seen,
        indexed: status.files_indexed,
        skipped_perm: status.files_skipped_perm,
        skipped_cloud: status.files_skipped_cloud,
        coverage_pct: status.coverage_pct,
        errors,
    })
}

/// Catalogs a whole registered drive from its root: a Tier-1 walk (metadata only, no payload
/// reads). Marks `volumes.last_catalog_complete_at` on success (spec §7.3 step 1).
pub fn catalog_volume(
    store: &mut SqliteStore,
    volume_id: &str,
    progress: &mut dyn FnMut(&ScanProgress),
) -> Result<ScanStatus> {
    let (mount, status): (String, String) = store
        .conn()
        .query_row(
            "SELECT mount_point, status FROM volumes WHERE id = ?1",
            params![volume_id],
            |r| Ok((r.get(0)?, r.get(1)?)),
        )
        .optional()?
        .ok_or_else(|| AegisError::NotFound(format!("volume {volume_id}")))?;
    if status != "online" {
        return Err(AegisError::VolumeOffline(volume_id.to_string()));
    }
    // Journal position is taken before the walk, so changes made during it are replayed later.
    let journal = crate::drives::refresh::journal_position(Path::new(&mount));
    let result = scan_root_inner(store, Path::new(&mount), &[1], Some(volume_id), progress)?;
    store.conn().execute(
        "UPDATE volumes SET last_catalog_complete_at = ?2 WHERE id = ?1",
        params![volume_id, now_ms()],
    )?;
    crate::drives::refresh::store_journal(store, volume_id, journal)?;
    Ok(result)
}

/// Scans that were still "running" when Aegis last stopped can never finish: marks them
/// interrupted. Called once when the engine starts, before any job runs. Returns rows changed.
pub fn mark_interrupted_jobs(conn: &rusqlite::Connection, now: i64) -> Result<usize> {
    Ok(conn.execute(
        "UPDATE scan_jobs SET status = 'interrupted', finished_at = ?1 WHERE status = 'running'",
        [now],
    )?)
}

/// The owner's Downloads folder (the first known folder), when it exists.
pub fn downloads_dir() -> Option<PathBuf> {
    known_folders().into_iter().next().filter(|p| p.is_dir())
}

/// The PC folders Aegis starts with (spec §7.8.1): Downloads, Documents, Desktop, Pictures, Music.
pub fn default_root_paths() -> Vec<PathBuf> {
    known_folders()
        .into_iter()
        .filter(|p| p.is_dir())
        .collect()
}

#[cfg(windows)]
fn known_folders() -> Vec<PathBuf> {
    use std::os::windows::ffi::OsStringExt;
    use windows_sys::Win32::System::Com::CoTaskMemFree;
    use windows_sys::Win32::UI::Shell::{
        FOLDERID_Desktop, FOLDERID_Documents, FOLDERID_Downloads, FOLDERID_Music,
        FOLDERID_Pictures, SHGetKnownFolderPath,
    };
    let ids = [
        FOLDERID_Downloads,
        FOLDERID_Documents,
        FOLDERID_Desktop,
        FOLDERID_Pictures,
        FOLDERID_Music,
    ];
    let mut out = Vec::new();
    for id in ids.iter() {
        let mut raw: *mut u16 = std::ptr::null_mut();
        let hr = unsafe { SHGetKnownFolderPath(id, 0, std::ptr::null_mut(), &mut raw) };
        if hr == 0 && !raw.is_null() {
            let len = (0..).take_while(|&i| unsafe { *raw.add(i) } != 0).count();
            let slice = unsafe { std::slice::from_raw_parts(raw, len) };
            out.push(PathBuf::from(std::ffi::OsString::from_wide(slice)));
        }
        if !raw.is_null() {
            unsafe { CoTaskMemFree(raw as *const _) };
        }
    }
    out
}

#[cfg(not(windows))]
fn known_folders() -> Vec<PathBuf> {
    let Ok(home) = std::env::var("HOME") else {
        return Vec::new();
    };
    ["Downloads", "Documents", "Desktop", "Pictures", "Music"]
        .iter()
        .map(|n| Path::new(&home).join(n))
        .collect()
}

/// Seeds the starting roots on first run; does nothing once any root exists.
pub fn seed_default_roots(store: &SqliteStore) -> Result<Vec<RootConfig>> {
    if !store.list_roots()?.is_empty() {
        return Ok(Vec::new());
    }
    let mut added = Vec::new();
    for p in default_root_paths() {
        added.push(store.add_root(&p)?);
    }
    Ok(added)
}

/// True when any component of `path` is a folder the scanner never indexes.
fn under_excluded_dir(path: &Path) -> bool {
    path.components().any(|c| {
        c.as_os_str()
            .to_str()
            .is_some_and(is_immutable_excluded_dir_name)
    }) || path.ancestors().any(|a| {
        a.file_name().and_then(|n| n.to_str()).is_some_and(|n| {
            n.eq_ignore_ascii_case("_Previous versions")
                && a.parent()
                    .and_then(|p| p.file_name())
                    .and_then(|p| p.to_str())
                    .is_some_and(|p| p.eq_ignore_ascii_case("Aegis Backup"))
        })
    })
}

/// Re-indexes individual paths on a registered volume (USN-driven refresh, spec §7.4):
/// existing files are upserted (renames keep `files.id` via the file index), vanished paths
/// are soft-deleted. Metadata only, no payload reads. Returns the number of paths applied.
pub fn index_paths(store: &mut SqliteStore, volume_id: &str, paths: &[PathBuf]) -> Result<usize> {
    let vol = store
        .list_volumes()?
        .into_iter()
        .find(|v| v.id == volume_id)
        .ok_or_else(|| AegisError::NotFound(format!("volume {volume_id}")))?;
    let units: Vec<(String, String)> = {
        let mut stmt = store
            .conn()
            .prepare("SELECT id, root_path FROM atomic_units WHERE volume_id = ?1")?;
        let rows = stmt.query_map([volume_id], |r| Ok((r.get(0)?, r.get(1)?)))?;
        rows.collect::<std::result::Result<Vec<_>, _>>()?
    };
    let unit_for = |p: &str| {
        units
            .iter()
            .filter(|(_, root)| path_is_within(p, root, vol.case_sensitive))
            .max_by_key(|(_, root)| root.len())
            .map(|(id, _)| id.clone())
    };
    let mut applied = 0;
    store.with_full_sync_tx(|tx| {
        for path in paths {
            if under_excluded_dir(path) || !path_is_within(&path.to_string_lossy(), &vol.mount_point, vol.case_sensitive) {
                continue;
            }
            match inspect_path_metadata(path) {
                Ok(Some(stat)) if !stat.is_dir && !stat.is_symlink => {
                    let input = crate::db::files::DiscoveredFileInput {
                        volume_id: vol.id.clone(),
                        has_stable_inode: vol.has_stable_inode,
                        case_sensitive: vol.case_sensitive,
                        file_index: stat.file_index,
                        nlink: stat.nlink,
                        atomic_unit_id: unit_for(&path.to_string_lossy()),
                        path: path.clone(),
                        size: stat.size,
                        mtime: stat.mtime,
                        ctime: stat.ctime,
                        blake3: None,
                        cloud_state: stat.cloud_state.clone(),
                    };
                    crate::db::files::upsert_discovered_file(tx, &input)?;
                    applied += 1;
                }
                Ok(Some(_)) => {}
                Ok(None) | Err(_) if !path.exists() => {
                    let (_, ph) = normalize_path(path, vol.case_sensitive);
                    let id: Option<String> = tx
                        .query_row(
                            "SELECT id FROM files WHERE volume_id = ?1 AND path_hash = ?2 AND deleted_at IS NULL",
                            params![vol.id, ph.as_slice()],
                            |r| r.get(0),
                        )
                        .optional()?;
                    if let Some(id) = id {
                        crate::db::files::soft_delete_file_in_tx(tx, &id)?;
                        applied += 1;
                    }
                }
                _ => {}
            }
        }
        Ok(())
    })?;
    Ok(applied)
}
