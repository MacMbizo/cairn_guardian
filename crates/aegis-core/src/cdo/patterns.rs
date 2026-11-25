//! Cross-project code-vs-doc pulse analysis and operational velocity metrics (`5.2`, `R5`, `R7`, `R11`).

use std::collections::{HashMap, HashSet};
use std::path::Path;

use rusqlite::{Connection, params};

use super::{CdoProjectPulse, CdoVelocitySnapshot};
use crate::care::path_contains_aegis_internal;
use crate::db::files::{is_sensitive, path_is_within};
use crate::error::Result;
use crate::versions::patterns::is_excluded_relative_path;

pub const DAY_MS: i64 = 86_400_000;

fn is_code_ext(ext: &str) -> bool {
    matches!(
        ext.to_ascii_lowercase().as_str(),
        "rs" | "ts"
            | "tsx"
            | "js"
            | "jsx"
            | "py"
            | "go"
            | "java"
            | "kt"
            | "cs"
            | "cpp"
            | "c"
            | "h"
            | "hpp"
            | "rb"
            | "php"
            | "swift"
            | "scala"
            | "sql"
            | "sh"
            | "ps1"
    )
}

fn is_doc_ext(ext: &str) -> bool {
    matches!(
        ext.to_ascii_lowercase().as_str(),
        "md" | "markdown" | "txt" | "rst" | "adoc" | "pdf" | "docx" | "doc" | "odt" | "rtf" | "org"
    )
}

fn is_inbox_path(path: &str) -> bool {
    let norm = path.to_ascii_lowercase().replace('\\', "/");
    norm.contains("/downloads/") || norm.contains("/desktop/")
}

fn looks_like_invoice_or_receipt(
    name: &str,
    doc_type: Option<&str>,
    category: Option<&str>,
) -> bool {
    let n = name.to_ascii_lowercase();
    if n.contains("invoice")
        || n.contains("receipt")
        || n.contains("statement")
        || n.contains("billing")
        || n.contains("tax_")
    {
        return true;
    }
    if let Some(dt) = doc_type
        && matches!(
            dt.to_ascii_lowercase().as_str(),
            "invoice" | "receipt" | "statement" | "tax_document"
        )
    {
        return true;
    }
    if let Some(cat) = category
        && matches!(
            cat.to_ascii_lowercase().as_str(),
            "finance" | "tax" | "accounting" | "invoices"
        )
    {
        return true;
    }
    false
}

struct ProjectLocationRow {
    collection_id: String,
    name: String,
    project_kind: Option<String>,
    root_path: String,
    case_sensitive: bool,
}

struct ActiveFileRow {
    file_id: String,
    path: String,
    name: String,
    ext: String,
    mount_point: String,
    mtime: i64,
    sensitivity: String,
    in_atomic_unit: bool,
    doc_type: Option<String>,
    category: Option<String>,
}

struct ProjectAccumulator {
    collection_id: String,
    name: String,
    project_kind: Option<String>,
    code_files: i64,
    doc_files: i64,
    recent_7d_files: i64,
    recent_30d_files: i64,
    newest_code_mtime: Option<i64>,
    newest_doc_mtime: Option<i64>,
    file_ids: HashSet<String>,
    open_action_items: i64,
    decisions_count: i64,
}

/// Evaluates code-vs-documentation freshness & Knowledge action counts for every project (`R5`, `R7`, `R11`).
pub fn analyze_project_pulses(
    conn: &Connection,
    now: i64,
    horizon_days: i64,
    allow_sensitive: bool,
) -> Result<(Vec<CdoProjectPulse>, i64)> {
    let cutoff_7d = now - 7 * DAY_MS;
    let cutoff_horizon = now - horizon_days.clamp(7, 365) * DAY_MS;

    let mut project_map: HashMap<String, ProjectAccumulator> = HashMap::new();
    let mut locations: Vec<ProjectLocationRow> = Vec::new();
    {
        let mut st = conn.prepare(
            "SELECT c.id, c.name, c.project_kind, cl.root_path, COALESCE(v.case_sensitive, 0)
             FROM collections c
             JOIN collection_locations cl ON cl.collection_id = c.id
             JOIN volumes v ON v.id = cl.volume_id
             WHERE c.kind = 'project'
               AND c.state != 'dismissed'
               AND COALESCE(v.role, 'unassigned') != 'backup'
               AND v.status = 'online'",
        )?;
        for r in st.query_map([], |row| {
            Ok(ProjectLocationRow {
                collection_id: row.get(0)?,
                name: row.get(1)?,
                project_kind: row.get(2)?,
                root_path: row.get(3)?,
                case_sensitive: row.get::<_, i64>(4)? != 0,
            })
        })? {
            let loc = r?;
            project_map
                .entry(loc.collection_id.clone())
                .or_insert_with(|| ProjectAccumulator {
                    collection_id: loc.collection_id.clone(),
                    name: loc.name.clone(),
                    project_kind: loc.project_kind.clone(),
                    code_files: 0,
                    doc_files: 0,
                    recent_7d_files: 0,
                    recent_30d_files: 0,
                    newest_code_mtime: None,
                    newest_doc_mtime: None,
                    file_ids: HashSet::new(),
                    open_action_items: 0,
                    decisions_count: 0,
                });
            locations.push(loc);
        }
    }

    if project_map.is_empty() {
        return Ok((Vec::new(), 0));
    }

    let mut member_to_project: HashMap<String, Vec<String>> = HashMap::new();
    {
        let mut mst = conn.prepare(
            "SELECT cm.file_id, cm.collection_id
             FROM collection_members cm
             JOIN collections c ON c.id = cm.collection_id
             WHERE c.kind = 'project' AND c.state != 'dismissed'",
        )?;
        for r in mst.query_map([], |row| Ok((row.get::<_, String>(0)?, row.get::<_, String>(1)?)))? {
            let (fid, cid) = r?;
            member_to_project.entry(fid).or_default().push(cid);
        }
    }

    let mut excluded_sensitive = 0i64;
    let mut file_to_project: HashMap<String, String> = HashMap::new();

    {
        let mut fst = conn.prepare(
            "SELECT f.id, f.path, f.name, COALESCE(f.ext, ''), v.mount_point, f.mtime,
                    COALESCE(f.sensitivity, 'normal'), f.atomic_unit_id IS NOT NULL,
                    fm.doc_type, fm.category
             FROM files f
             JOIN volumes v ON v.id = f.volume_id
             LEFT JOIN file_meta fm ON fm.file_id = f.id
             WHERE f.deleted_at IS NULL
               AND f.state NOT IN ('quarantine', 'missing')
               AND f.container_file_id IS NULL
               AND COALESCE(v.role, 'unassigned') != 'backup'
               AND v.status = 'online'",
        )?;
        for r in fst.query_map([], |row| {
            Ok(ActiveFileRow {
                file_id: row.get(0)?,
                path: row.get(1)?,
                name: row.get(2)?,
                ext: row.get(3)?,
                mount_point: row.get(4)?,
                mtime: row.get(5)?,
                sensitivity: row.get(6)?,
                in_atomic_unit: row.get::<_, i64>(7)? != 0,
                doc_type: row.get(8)?,
                category: row.get(9)?,
            })
        })? {
            let f = r?;
            if path_contains_aegis_internal(Path::new(&f.path))
                || is_excluded_relative_path(&f.path, &f.mount_point)
            {
                continue;
            }
            if is_sensitive(&f.sensitivity) && !allow_sensitive {
                excluded_sensitive += 1;
                continue;
            }
            let _ = (&f.in_atomic_unit, &f.doc_type, &f.category);

            let mut matched_cid = member_to_project
                .get(&f.file_id)
                .and_then(|ids| ids.first().cloned());
            if matched_cid.is_none() {
                matched_cid = locations
                    .iter()
                    .find(|loc| path_is_within(&f.path, &loc.root_path, loc.case_sensitive))
                    .map(|loc| loc.collection_id.clone());
            }
            let Some(cid) = matched_cid else {
                continue;
            };
            let Some(acc) = project_map.get_mut(&cid) else {
                continue;
            };
            if !acc.file_ids.insert(f.file_id.clone()) {
                continue;
            }
            file_to_project.insert(f.file_id.clone(), cid);

            if f.mtime >= cutoff_7d {
                acc.recent_7d_files += 1;
            }
            if f.mtime >= cutoff_horizon {
                acc.recent_30d_files += 1;
            }

            let is_code = is_code_ext(&f.ext);
            let is_doc = is_doc_ext(&f.ext) || {
                let low = f.name.to_ascii_lowercase();
                low.starts_with("readme")
                    || low.contains("changelog")
                    || low.contains("architecture")
                    || low.contains("design")
            };
            if is_code {
                acc.code_files += 1;
                acc.newest_code_mtime = Some(
                    acc.newest_code_mtime
                        .map_or(f.mtime, |prev| prev.max(f.mtime)),
                );
            } else if is_doc {
                acc.doc_files += 1;
                acc.newest_doc_mtime = Some(
                    acc.newest_doc_mtime
                        .map_or(f.mtime, |prev| prev.max(f.mtime)),
                );
            }
        }
    }

    // Attribute approved Knowledge entities linked to those allowed files (`R7`, `K4`).
    {
        let mut est = conn.prepare(
            "SELECT r.dst_id, e.id, e.entity_type, e.attrs_json
             FROM relationships r
             JOIN entities e ON e.id = r.src_id
             WHERE r.dst_kind = 'file'
               AND r.valid_to IS NULL
               AND e.entity_type IN ('action_item', 'decision')",
        )?;
        let mut seen_ent: HashSet<(String, String)> = HashSet::new();
        for r in est.query_map([], |row| {
            Ok((
                row.get::<_, String>(0)?,
                row.get::<_, String>(1)?,
                row.get::<_, String>(2)?,
                row.get::<_, String>(3)?,
            ))
        })? {
            let (fid, eid, etype, attrs_json) = r?;
            let Some(cid) = file_to_project.get(&fid) else {
                continue;
            };
            if !seen_ent.insert((cid.clone(), eid)) {
                continue;
            }
            let Some(acc) = project_map.get_mut(cid) else {
                continue;
            };
            if etype == "decision" {
                acc.decisions_count += 1;
            } else if etype == "action_item" {
                let status_tag = serde_json::from_str::<serde_json::Value>(&attrs_json)
                    .ok()
                    .and_then(|v| {
                        v.get("status")
                            .and_then(|s| s.as_str())
                            .map(|s| s.to_ascii_lowercase())
                    })
                    .unwrap_or_else(|| "open".to_string());
                if status_tag != "done" && status_tag != "closed" {
                    acc.open_action_items += 1;
                }
            }
        }
    }

    let mut pulses: Vec<CdoProjectPulse> = project_map
        .into_values()
        .map(|acc| {
            let doc_lag_days = match (acc.newest_code_mtime, acc.newest_doc_mtime) {
                (Some(c), Some(d)) => Some(((c - d) / DAY_MS).max(0)),
                _ => None,
            };
            let is_doc_lagging = acc.code_files >= 3
                && ((acc.doc_files == 0 && acc.recent_30d_files > 0)
                    || doc_lag_days.is_some_and(|days| days >= 7));
            let health_flag = if is_doc_lagging {
                "doc_lagging"
            } else if acc.open_action_items >= 4 {
                "action_heavy"
            } else if acc.recent_30d_files == 0 && acc.code_files + acc.doc_files > 0 {
                "quiet"
            } else {
                "aligned"
            };
            CdoProjectPulse {
                collection_id: acc.collection_id,
                name: acc.name,
                project_kind: acc.project_kind,
                code_files: acc.code_files,
                doc_files: acc.doc_files,
                recent_7d_files: acc.recent_7d_files,
                recent_30d_files: acc.recent_30d_files,
                newest_code_mtime: acc.newest_code_mtime,
                newest_doc_mtime: acc.newest_doc_mtime,
                doc_lag_days,
                open_action_items: acc.open_action_items,
                decisions_count: acc.decisions_count,
                health_flag: health_flag.to_string(),
            }
        })
        .collect();

    pulses.sort_by(|a, b| {
        b.recent_30d_files
            .cmp(&a.recent_30d_files)
            .then_with(|| a.name.cmp(&b.name))
    });

    Ok((pulses, excluded_sensitive))
}

/// Computes local file velocity, inbox invoice/receipt friction, automation leverage, and open queues (`R5`, `R7`, `R11`).
pub fn compute_velocity_snapshot(
    conn: &Connection,
    now: i64,
    horizon_days: i64,
    allow_sensitive: bool,
) -> Result<(CdoVelocitySnapshot, Vec<String>, i64)> {
    let window_days = horizon_days.clamp(7, 365);
    let cutoff_7d = now - 7 * DAY_MS;
    let cutoff_horizon = now - window_days * DAY_MS;

    let mut files_touched_7d = 0i64;
    let mut files_touched_30d = 0i64;
    let mut loose_inbox_files = 0i64;
    let mut loose_invoice_docs = 0i64;
    let mut loose_invoice_paths = Vec::new();
    let mut excluded_sensitive = 0i64;

    {
        let mut st = conn.prepare(
            "SELECT f.id, f.path, f.name, COALESCE(f.ext, ''), v.mount_point, f.mtime,
                    COALESCE(f.sensitivity, 'normal'), f.atomic_unit_id IS NOT NULL,
                    fm.doc_type, fm.category
             FROM files f
             JOIN volumes v ON v.id = f.volume_id
             LEFT JOIN file_meta fm ON fm.file_id = f.id
             WHERE f.deleted_at IS NULL
               AND f.state NOT IN ('quarantine', 'missing')
               AND f.container_file_id IS NULL
               AND COALESCE(v.role, 'unassigned') != 'backup'
               AND v.status = 'online'",
        )?;
        for r in st.query_map([], |row| {
            Ok(ActiveFileRow {
                file_id: row.get(0)?,
                path: row.get(1)?,
                name: row.get(2)?,
                ext: row.get(3)?,
                mount_point: row.get(4)?,
                mtime: row.get(5)?,
                sensitivity: row.get(6)?,
                in_atomic_unit: row.get::<_, i64>(7)? != 0,
                doc_type: row.get(8)?,
                category: row.get(9)?,
            })
        })? {
            let f = r?;
            if path_contains_aegis_internal(Path::new(&f.path))
                || is_excluded_relative_path(&f.path, &f.mount_point)
            {
                continue;
            }
            let _ = (&f.file_id, &f.ext);
            if f.mtime >= cutoff_7d {
                files_touched_7d += 1;
            }
            if f.mtime >= cutoff_horizon {
                files_touched_30d += 1;
            }
            if !f.in_atomic_unit && is_inbox_path(&f.path) {
                if is_sensitive(&f.sensitivity) && !allow_sensitive {
                    excluded_sensitive += 1;
                    continue;
                }
                loose_inbox_files += 1;
                if looks_like_invoice_or_receipt(
                    &f.name,
                    f.doc_type.as_deref(),
                    f.category.as_deref(),
                ) {
                    loose_invoice_docs += 1;
                    if loose_invoice_paths.len() < 8 {
                        loose_invoice_paths.push(f.path);
                    }
                }
            }
        }
    }

    let batches_executed_30d: i64 = conn.query_row(
        "SELECT COUNT(*) FROM action_batches WHERE status = 'executed' AND COALESCE(executed_at, created_at) >= ?1",
        params![cutoff_horizon],
        |r| r.get(0),
    )?;
    let batches_rolled_back_30d: i64 = conn.query_row(
        "SELECT COUNT(*) FROM action_batches WHERE status = 'rolled_back' AND COALESCE(rolled_back_at, created_at) >= ?1",
        params![cutoff_horizon],
        |r| r.get(0),
    )?;
    let total_batches_30d = batches_executed_30d + batches_rolled_back_30d;

    let auto_decisions: i64 = conn.query_row(
        "SELECT COUNT(*) FROM filing_decisions WHERE at >= ?1 AND outcome IN ('approved', 'auto', 'executed')",
        params![cutoff_horizon],
        |r| r.get(0),
    )?;
    let folderless_approvals: i64 = conn.query_row(
        "SELECT COALESCE(SUM(approvals), 0) FROM folderless_rules WHERE enabled = 1",
        [],
        |r| r.get(0),
    )?;
    let total_automated = auto_decisions + folderless_approvals + batches_executed_30d;
    let denominator = total_automated + loose_inbox_files + total_batches_30d.min(0);
    let filing_automation_ratio = if denominator > 0 {
        ((total_automated as f64 / denominator as f64) * 1000.0).round() / 1000.0
    } else {
        1.0
    };

    let open_action_items: i64 = {
        let mut st = conn.prepare(
            "SELECT attrs_json FROM entities WHERE entity_type = 'action_item'",
        )?;
        let rows = st.query_map([], |r| r.get::<_, String>(0))?;
        let mut count = 0i64;
        for row in rows {
            let s = row?;
            let st_tag = serde_json::from_str::<serde_json::Value>(&s)
                .ok()
                .and_then(|v| {
                    v.get("status")
                        .and_then(|x| x.as_str())
                        .map(|x| x.to_ascii_lowercase())
                })
                .unwrap_or_else(|| "open".into());
            if st_tag != "done" && st_tag != "closed" {
                count += 1;
            }
        }
        count
    };

    let pending_knowledge_proposals: i64 = conn.query_row(
        "SELECT COUNT(*) FROM knowledge_proposals WHERE status = 'pending'",
        [],
        |r| r.get(0),
    )?;
    let pending_action_batches: i64 = conn.query_row(
        "SELECT COUNT(*) FROM action_batches WHERE status IN ('draft', 'approved')",
        [],
        |r| r.get(0),
    )?;
    let active_folderless_rules: i64 = conn.query_row(
        "SELECT COUNT(*) FROM folderless_rules WHERE enabled = 1",
        [],
        |r| r.get(0),
    )?;
    let active_privacy_rules: i64 = conn.query_row(
        "SELECT COUNT(*) FROM governance_privacy_rules WHERE enabled = 1",
        [],
        |r| r.get(0),
    )?;
    let open_security_findings: i64 = conn.query_row(
        "SELECT COUNT(*) FROM security_findings WHERE status = 'open'",
        [],
        |r| r.get(0),
    )?;

    Ok((
        CdoVelocitySnapshot {
            horizon_days: window_days,
            files_touched_7d,
            files_touched_30d,
            batches_executed_30d,
            batches_rolled_back_30d,
            filing_automation_ratio,
            loose_inbox_files,
            loose_invoice_docs,
            open_action_items,
            pending_knowledge_proposals,
            pending_action_batches,
            active_folderless_rules,
            active_privacy_rules,
            open_security_findings,
            observation_mode: "Disk & encrypted catalog only (R5/R11: zero screen, keystroke, or active-window telemetry)".into(),
        },
        loose_invoice_paths,
        excluded_sensitive,
    ))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::db::SqliteStore;

    #[test]
    fn classifies_code_doc_and_invoice_heuristics() {
        assert!(is_code_ext("rs"));
        assert!(is_code_ext("TSX"));
        assert!(!is_code_ext("md"));
        assert!(is_doc_ext("md"));
        assert!(is_doc_ext("pdf"));
        assert!(looks_like_invoice_or_receipt("2026-Q3-Invoice-Acme.pdf", None, None));
        assert!(looks_like_invoice_or_receipt("scan01.pdf", Some("invoice"), None));
        assert!(!looks_like_invoice_or_receipt("holiday_photo.jpg", None, None));
    }

    #[test]
    fn empty_store_produces_clean_velocity_snapshot() {
        let store = SqliteStore::open_in_memory().unwrap();
        let (snap, paths, excluded) =
            compute_velocity_snapshot(store.conn(), 1_700_000_000_000, 30, false).unwrap();
        assert_eq!(snap.files_touched_30d, 0);
        assert_eq!(snap.loose_invoice_docs, 0);
        assert_eq!(snap.filing_automation_ratio, 1.0);
        assert!(paths.is_empty());
        assert_eq!(excluded, 0);
        assert!(snap.observation_mode.contains("R5/R11"));
    }
}
