//! Tier-1 discovery, `.aegisignore` + immutable exclusions, `atomic_units` (`ADR-004`),
//! `cache_dirs` recognition, and optional Tier-2 local text chunking + BLAKE3 hashing.

pub mod os_adapter;

use crate::contracts::{CoverageErrorItem, CoverageReport, ScanStatus};
use crate::db::SqliteStore;
use crate::db::files::{DiscoveredFileInput, insert_chunk, normalize_path, upsert_discovered_file};
use crate::error::{AegisError, Result};
use crate::scanner::os_adapter::{OsVolumeMeta, inspect_path_metadata, inspect_volume_for_path};
use crate::{new_id, now_ms};
use ignore::gitignore::{Gitignore, GitignoreBuilder};
use rusqlite::{Connection, OptionalExtension, params};
use std::collections::{HashMap, HashSet};
use std::fs::File;
use std::io::Read;
use std::path::{Path, PathBuf};

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
    name.eq_ignore_ascii_case(".aegis") || name.eq_ignore_ascii_case(".aegis-quarantine")
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

fn detect_atomic_unit_marker(dir: &Path) -> Option<(&'static str, String)> {
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
        if dir.join(marker).exists() {
            return Some((unit_type, (*marker).to_string()));
        }
    }

    if let Ok(entries) = std::fs::read_dir(dir) {
        for entry in entries.flatten() {
            let fname = entry.file_name();
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

fn summarize_cache_dir(dir: &Path) -> (i64, i64) {
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
    let (size_bytes, file_count) = summarize_cache_dir(dir);
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

fn extract_text_chunks_if_local(
    conn: &Connection,
    file_id: &str,
    path: &Path,
    cloud_state: &str,
    size: i64,
) -> Result<()> {
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

    let existing_chunks: i64 = conn.query_row(
        "SELECT COUNT(*) FROM chunks WHERE file_id = ?1",
        params![file_id],
        |r| r.get(0),
    )?;
    if existing_chunks > 0 {
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

/// Scans a configured root path (`tiers` containing `1` for Tier-1 metadata, `2` for local hash + text chunks).
pub fn scan_root(store: &mut SqliteStore, root: &Path, tiers: &[u8]) -> Result<ScanStatus> {
    let canonical_root = std::fs::canonicalize(root).unwrap_or_else(|_| root.to_path_buf());
    let vol_info = store.ensure_volume_for_path(&canonical_root)?;
    let vol_meta = inspect_volume_for_path(&canonical_root)?;
    let (display_root, _) = normalize_path(&canonical_root, vol_info.case_sensitive);

    let global_ignore = store.get_setting("ignore.global", "global")?;
    let root_ignore_key = format!("ignore.root:{}", display_root);
    let root_ignore = store.get_setting(&root_ignore_key, "global")?;
    let combined_ignore = [global_ignore, root_ignore]
        .into_iter()
        .flatten()
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

    let mut files_seen: i64 = 0;
    let mut files_indexed: i64 = 0;
    let mut files_skipped_perm: i64 = 0;
    let mut files_skipped_cloud: i64 = 0;
    let mut errors: Vec<CoverageErrorItem> = Vec::new();
    let mut visited_inodes: HashSet<Vec<u8>> = HashSet::new();
    // Maps normalized dir path -> innermost atomic_unit_id
    let mut dir_unit_map: HashMap<PathBuf, String> = HashMap::new();
    let run_tier2 = tiers.contains(&2) || tiers.contains(&3);

    // DFS explicit stack so we preserve innermost-wins atomic_unit inheritance and never descend into loops/caches
    let mut dir_stack: Vec<(PathBuf, Option<String>)> = vec![(canonical_root.clone(), None)];

    while let Some((dir, inherited_unit)) = dir_stack.pop() {
        if let Some(dir_name) = dir.file_name().and_then(|s| s.to_str()) {
            if is_immutable_excluded_dir_name(dir_name) {
                continue;
            }
            if dir != canonical_root && matcher.matched(&dir, true).is_ignore() {
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
                    store.conn(),
                    &vol_meta,
                    inherited_unit.as_deref(),
                    &dir,
                    cache_kind,
                    parent_mtime,
                )?;
                continue;
            }
        }

        // Symlink / junction loop protection (FR-001.4)
        if let Ok(Some(st)) = inspect_path_metadata(&dir) {
            if st.is_symlink && dir != canonical_root {
                continue;
            }
            if let Some(idx) = st.file_index
                && !visited_inodes.insert(idx)
            {
                continue;
            }
        }

        let current_unit = match detect_atomic_unit_marker(&dir) {
            Some((unit_type, marker)) => {
                let unit_id = register_atomic_unit(store.conn(), &vol_meta, &dir, unit_type, &marker)?;
                dir_unit_map.insert(dir.clone(), unit_id.clone());
                Some(unit_id)
            }
            None => inherited_unit.clone(),
        };

        let entries = match std::fs::read_dir(&dir) {
            Ok(rd) => rd,
            Err(e) => {
                files_seen += 1;
                files_skipped_perm += 1;
                errors.push(CoverageErrorItem {
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

        for entry in entries {
            let entry = match entry {
                Ok(e) => e,
                Err(e) => {
                    files_seen += 1;
                    files_skipped_perm += 1;
                    errors.push(CoverageErrorItem {
                        path: dir.display().to_string(),
                        code: "permission_denied".into(),
                        message: e.to_string(),
                    });
                    continue;
                }
            };
            let path = entry.path();
            let ftype = match entry.file_type() {
                Ok(ft) => ft,
                Err(e) => {
                    files_seen += 1;
                    files_skipped_perm += 1;
                    errors.push(CoverageErrorItem {
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

            if matcher.matched(&path, false).is_ignore() {
                continue;
            }

            files_seen += 1;
            let stat = match inspect_path_metadata(&path) {
                Ok(Some(st)) => st,
                Ok(None) => continue,
                Err(e) => {
                    files_skipped_perm += 1;
                    let ipc = e.to_ipc();
                    errors.push(CoverageErrorItem {
                        path: path.display().to_string(),
                        code: ipc.code,
                        message: ipc.message,
                    });
                    continue;
                }
            };

            let is_placeholder = stat.cloud_state == "placeholder_remote";
            let blake3 = if run_tier2 && !is_placeholder {
                match compute_blake3_if_local(&path, &stat.cloud_state) {
                    Ok(h) => h,
                    Err(e) => {
                        files_skipped_perm += 1;
                        let ipc = e.to_ipc();
                        errors.push(CoverageErrorItem {
                            path: path.display().to_string(),
                            code: ipc.code,
                            message: ipc.message,
                        });
                        continue;
                    }
                }
            } else {
                None
            };

            let input = DiscoveredFileInput {
                volume_id: vol_meta.id.clone(),
                has_stable_inode: vol_meta.has_stable_inode,
                case_sensitive: vol_meta.case_sensitive,
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

            let file_id = upsert_discovered_file(store.conn(), &input)?;
            if is_placeholder {
                files_skipped_cloud += 1;
            } else {
                files_indexed += 1;
                if run_tier2 {
                    let _ = extract_text_chunks_if_local(
                        store.conn(),
                        &file_id,
                        &path,
                        &stat.cloud_state,
                        stat.size,
                    );
                }
            }
        }
    }

    let finished_at = now_ms();
    let errors_json = serde_json::to_string(&errors)?;
    let status = if errors.is_empty() {
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
            files_seen,
            files_indexed,
            files_skipped_perm,
            files_skipped_cloud,
            errors_json,
            finished_at,
            job_id
        ],
    )?;

    let coverage_pct = coverage_pct_from_counts(files_seen, files_skipped_perm, files_skipped_cloud);
    Ok(ScanStatus {
        job_id,
        root_path: display_root,
        tier: max_tier,
        status: status.to_string(),
        files_seen,
        files_indexed,
        files_skipped_perm,
        files_skipped_cloud,
        coverage_pct,
        started_at,
        finished_at: Some(finished_at),
    })
}

pub fn latest_scan_status(conn: &Connection, job_id: Option<&str>) -> Result<Option<ScanStatus>> {
    let row = match job_id {
        Some(id) => conn
            .query_row(
                "SELECT id, root_path, tier, status, files_seen, files_indexed,
                        files_skipped_perm, files_skipped_cloud, started_at, finished_at
                 FROM scan_jobs WHERE id = ?1",
                params![id],
                |r| {
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
                },
            )
            .optional()?,
        None => conn
            .query_row(
                "SELECT id, root_path, tier, status, files_seen, files_indexed,
                        files_skipped_perm, files_skipped_cloud, started_at, finished_at
                 FROM scan_jobs ORDER BY started_at DESC LIMIT 1",
                [],
                |r| {
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
                },
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
