//! Core scan, triage, reversible R7 classification batches (`K1`), privacy rules, and
//! 30-day quarantine staging for Capability #13: Personal Data Governance (`5.1`, `R7`–`R9`).

use std::collections::{BTreeMap, HashMap, HashSet};
use std::path::{Component, Path};
use std::time::Instant;

use rusqlite::{Connection, OptionalExtension, params};

use crate::care::path_contains_aegis_internal;
use crate::contracts::{ActionType, ProposedActionInput, RefilePlan, TargetKind};
use crate::db::SqliteStore;
use crate::db::actions::{build_policy_context, propose_batch};
use crate::db::audit::append_audit_in_tx;
use crate::db::files::{SENSITIVE_MASK, is_sensitive, path_is_within};
use crate::error::{AegisError, Result};
use crate::policy::{PolicyAction, PolicyVerdict, evaluate_policy};
use crate::versions::patterns::is_excluded_relative_path;
use crate::{new_id, now_ms};

use super::detector::scan_file_for_secrets;
use super::posture::{VaultPostureContext, build_privacy_posture, build_privacy_posture_with_drives};
use super::{
    DriveEncryptionStatus, GovernanceDashboard, GovernanceFinding, GovernanceFindingLocation,
    GovernanceFindingStatus, GovernanceFindingType, GovernanceLabelBatch, GovernancePrivacyRule,
    GovernancePrivacyRuleInput, GovernanceQueryOptions, GovernanceRuleAction,
    GovernanceScanOptions, GovernanceScanReport, GovernanceSeverity, PrivacyPostureReport,
    SensitiveFileSummary,
};

/// Origin and agent label for Personal Data Governance `K1` batches.
pub const GOVERNANCE_AGENT: &str = "personal_governance";
const MAX_SCAN_FILE_BYTES: u64 = 2 * 1024 * 1024;

/// Case-insensitive `*` / `?` wildcard matcher implemented purely in Rust.
pub fn wildcard_match_ci(pattern: &str, text: &str) -> bool {
    let p: Vec<char> = pattern.to_ascii_lowercase().chars().collect();
    let s: Vec<char> = text.to_ascii_lowercase().chars().collect();
    let (mut pi, mut si) = (0usize, 0usize);
    let (mut star_p, mut star_s) = (None, 0usize);

    while si < s.len() {
        if pi < p.len() && (p[pi] == '?' || p[pi] == s[si]) {
            pi += 1;
            si += 1;
        } else if pi < p.len() && p[pi] == '*' {
            star_p = Some(pi);
            star_s = si;
            pi += 1;
        } else if let Some(sp) = star_p {
            pi = sp + 1;
            star_s += 1;
            si = star_s;
        } else {
            return false;
        }
    }
    while pi < p.len() && p[pi] == '*' {
        pi += 1;
    }
    pi == p.len()
}

pub fn rule_matches_file(
    path: &str,
    name: &str,
    path_prefix: Option<&str>,
    filename_pattern: Option<&str>,
    case_sensitive: bool,
) -> bool {
    let has_prefix = path_prefix.is_some_and(|p| !p.trim().is_empty());
    let has_pat = filename_pattern.is_some_and(|p| !p.trim().is_empty());
    if !has_prefix && !has_pat {
        return false;
    }
    if let Some(pref) = path_prefix
        && !pref.trim().is_empty()
        && !path_is_within(path, pref.trim(), case_sensitive)
    {
        return false;
    }
    if let Some(pat) = filename_pattern
        && !pat.trim().is_empty()
    {
        let pat_trim = pat.trim();
        let norm_target = if pat_trim.contains('/') || pat_trim.contains('\\') {
            path.replace('\\', "/")
        } else {
            name.to_string()
        };
        let norm_pat = pat_trim.replace('\\', "/");
        if !wildcard_match_ci(&norm_pat, &norm_target) {
            return false;
        }
    }
    true
}

/// True when any enabled `exclude_from_ai` governance rule matches `path` (`R7`).
pub fn is_path_excluded_from_ai(conn: &Connection, path: &str) -> bool {
    let Ok(mut stmt) = conn.prepare(
        "SELECT path_prefix, filename_pattern
         FROM governance_privacy_rules
         WHERE enabled = 1 AND rule_action = 'exclude_from_ai'",
    ) else {
        return false;
    };
    let name = Path::new(path)
        .file_name()
        .map(|s| s.to_string_lossy().to_string())
        .unwrap_or_else(|| path.to_string());
    let Ok(rows) = stmt.query_map([], |r| {
        Ok((r.get::<_, Option<String>>(0)?, r.get::<_, Option<String>>(1)?))
    }) else {
        return false;
    };
    for r in rows.flatten() {
        if rule_matches_file(path, &name, r.0.as_deref(), r.1.as_deref(), false) {
            return true;
        }
    }
    false
}

fn validate_rule_prefix(conn: &Connection, prefix: Option<&str>) -> Result<Option<String>> {
    let Some(raw) = prefix.map(str::trim).filter(|s| !s.is_empty()) else {
        return Ok(None);
    };
    let p = Path::new(raw);
    if p.components().any(|c| matches!(c, Component::ParentDir)) {
        return Err(AegisError::PermissionDenied(
            "Privacy rule path must not contain '..' traversal.".into(),
        ));
    }
    if path_contains_aegis_internal(p) {
        return Err(AegisError::PermissionDenied(
            "Privacy rule path cannot target Cairn internals.".into(),
        ));
    }
    let mut st = conn.prepare(
        "SELECT mount_point, COALESCE(case_sensitive, 0) FROM volumes WHERE role = 'backup'",
    )?;
    for row in st.query_map([], |r| Ok((r.get::<_, String>(0)?, r.get::<_, i64>(1)? != 0)))? {
        let (mount, cs) = row?;
        if path_is_within(raw, &mount, cs) {
            return Err(AegisError::PermissionDenied(
                "Privacy rules cannot target a backup volume.".into(),
            ));
        }
    }
    Ok(Some(raw.to_string()))
}

struct LiveScanCandidate {
    file_id: String,
    path: String,
    name: String,
    ext: String,
    mtime: i64,
    size: i64,
    mount_point: String,
    case_sensitive: bool,
    sensitivity: String,
    in_atomic_unit: bool,
}

/// Scans live local non-backup files for PII & secrets, persisting redacted findings in SQLCipher (`D1`).
pub fn scan_personal_data(
    conn: &Connection,
    opts: &GovernanceScanOptions,
    reveal_sensitive: bool,
) -> Result<GovernanceScanReport> {
    if opts.include_sensitive && !reveal_sensitive {
        return Err(AegisError::PermissionDenied(
            "Windows Hello / Windows user verification is required before scanning sensitive files."
                .into(),
        ));
    }

    let started = Instant::now();
    let now = now_ms();
    let reveal_unlocked = opts.include_sensitive && reveal_sensitive;

    // 1. Clean out locations belonging to quarantined, deleted, or backup-role files (`K2`, `K4`).
    conn.execute(
        "DELETE FROM security_finding_locations
         WHERE file_id IN (
             SELECT f.id FROM files f
             LEFT JOIN volumes v ON v.id = f.volume_id
             WHERE f.deleted_at IS NOT NULL
                OR f.state IN ('quarantine', 'missing')
                OR COALESCE(v.role, 'unassigned') = 'backup'
         )",
        [],
    )?;
    let orphan_resolved = conn.execute(
        "UPDATE security_findings
         SET status = 'resolved', resolved_at = ?1
         WHERE status = 'open'
           AND NOT EXISTS (
               SELECT 1 FROM security_finding_locations l WHERE l.finding_id = security_findings.id
           )",
        params![now],
    )? as i64;

    let mut candidates = Vec::new();
    {
        let mut stmt = conn.prepare(
            "SELECT f.id, f.path, f.name, COALESCE(f.ext, ''), f.mtime, f.size,
                    v.mount_point, COALESCE(v.case_sensitive, 0),
                    COALESCE(f.sensitivity, 'normal'), f.atomic_unit_id IS NOT NULL
             FROM files f
             JOIN volumes v ON v.id = f.volume_id
             WHERE f.deleted_at IS NULL
               AND f.state NOT IN ('quarantine', 'missing')
               AND f.container_file_id IS NULL
               AND COALESCE(v.role, 'unassigned') != 'backup'
               AND v.status = 'online'
               AND f.cloud_state = 'local'",
        )?;
        for r in stmt.query_map([], |row| {
            Ok(LiveScanCandidate {
                file_id: row.get(0)?,
                path: row.get(1)?,
                name: row.get(2)?,
                ext: row.get(3)?,
                mtime: row.get(4)?,
                size: row.get(5)?,
                mount_point: row.get(6)?,
                case_sensitive: row.get::<_, i64>(7)? != 0,
                sensitivity: row.get(8)?,
                in_atomic_unit: row.get::<_, i64>(9)? != 0,
            })
        })? {
            let c = r?;
            if is_excluded_relative_path(&c.path, &c.mount_point) {
                continue;
            }
            if let Some(scope) = opts.scope_root.as_deref()
                && !path_is_within(&c.path, scope, c.case_sensitive)
            {
                continue;
            }
            candidates.push(c);
        }
    }

    let mut files_inspected = 0i64;
    let mut files_skipped_unchanged = 0i64;
    let mut excluded_sensitive_count = 0i64;
    let mut findings_discovered = 0i64;
    let max_files = opts.max_files.unwrap_or(usize::MAX);

    for cand in candidates {
        if (files_inspected as usize) >= max_files {
            break;
        }
        if is_sensitive(&cand.sensitivity) && !reveal_unlocked {
            excluded_sensitive_count += 1;
            continue;
        }

        if !opts.force_rescan {
            let watermark: Option<(i64, i64)> = conn
                .query_row(
                    "SELECT content_mtime, content_size FROM governance_file_scans WHERE file_id = ?1",
                    params![cand.file_id],
                    |r| Ok((r.get(0)?, r.get(1)?)),
                )
                .optional()?;
            if watermark == Some((cand.mtime, cand.size)) {
                files_skipped_unchanged += 1;
                continue;
            }
        }

        let disk_path = Path::new(&cand.path);
        let text_owned: Option<String> = match std::fs::metadata(disk_path) {
            Ok(meta) if meta.is_file() && meta.len() <= MAX_SCAN_FILE_BYTES => {
                std::fs::read(disk_path)
                    .ok()
                    .and_then(|bytes| String::from_utf8(bytes).ok())
            }
            _ => None,
        };

        files_inspected += 1;
        let hits = scan_file_for_secrets(
            &cand.path,
            &cand.name,
            &cand.ext,
            text_owned.as_deref(),
            cand.in_atomic_unit,
        );

        // Replace stale location entries for this re-scanned file so resolved secrets clear cleanly.
        conn.execute(
            "DELETE FROM security_finding_locations WHERE file_id = ?1",
            params![cand.file_id],
        )?;

        for hit in &hits {
            let existing: Option<(String, String, f64)> = conn
                .query_row(
                    "SELECT id, status, context_risk_score FROM security_findings WHERE secret_fingerprint = ?1",
                    params![hit.fingerprint.as_slice()],
                    |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?)),
                )
                .optional()?;

            let finding_id = match existing {
                Some((fid, status, old_risk)) => {
                    let next_risk = old_risk.max(hit.context_risk_score);
                    if status == "resolved" {
                        conn.execute(
                            "UPDATE security_findings
                             SET status = 'open', resolved_at = NULL, context_risk_score = ?1
                             WHERE id = ?2",
                            params![next_risk, fid],
                        )?;
                    } else if (next_risk - old_risk).abs() > f64::EPSILON {
                        conn.execute(
                            "UPDATE security_findings SET context_risk_score = ?1 WHERE id = ?2",
                            params![next_risk, fid],
                        )?;
                    }
                    fid
                }
                None => {
                    let fid = new_id();
                    conn.execute(
                        "INSERT INTO security_findings
                            (id, secret_fingerprint, finding_type, base_severity, context_risk_score, status, created_at)
                         VALUES (?1, ?2, ?3, ?4, ?5, 'open', ?6)",
                        params![
                            fid,
                            hit.fingerprint.as_slice(),
                            hit.finding_type.as_str(),
                            hit.severity.as_str(),
                            hit.context_risk_score,
                            now
                        ],
                    )?;
                    findings_discovered += 1;
                    fid
                }
            };

            conn.execute(
                "INSERT OR REPLACE INTO security_finding_locations
                    (finding_id, file_id, line_number, redacted_snippet)
                 VALUES (?1, ?2, ?3, ?4)",
                params![finding_id, cand.file_id, hit.line_number, hit.redacted_snippet],
            )?;
        }

        conn.execute(
            "INSERT INTO governance_file_scans (file_id, content_mtime, content_size, findings_count, scanned_at)
             VALUES (?1, ?2, ?3, ?4, ?5)
             ON CONFLICT(file_id) DO UPDATE SET
                content_mtime = excluded.content_mtime,
                content_size = excluded.content_size,
                findings_count = excluded.findings_count,
                scanned_at = excluded.scanned_at",
            params![cand.file_id, cand.mtime, cand.size, hits.len() as i64, now],
        )?;
    }

    let post_resolved = conn.execute(
        "UPDATE security_findings
         SET status = 'resolved', resolved_at = ?1
         WHERE status = 'open'
           AND NOT EXISTS (
               SELECT 1 FROM security_finding_locations l WHERE l.finding_id = security_findings.id
           )",
        params![now],
    )? as i64;

    Ok(GovernanceScanReport {
        files_inspected,
        files_skipped_unchanged,
        excluded_sensitive_count,
        findings_discovered,
        findings_resolved: orphan_resolved + post_resolved,
        duration_ms: started.elapsed().as_millis() as i64,
    })
}

fn finding_summary_and_action(
    ft: GovernanceFindingType,
    loc_count: usize,
) -> (String, String) {
    let base = match ft {
        GovernanceFindingType::PaymentCard => "Luhn-validated payment card sequence detected",
        GovernanceFindingType::PrivateKeyBlock => "Unencrypted private key material detected",
        GovernanceFindingType::CloudApiKey => "Cloud API key or secret token detected",
        GovernanceFindingType::GovernmentId => "Government tax or identity number detected",
        GovernanceFindingType::BankAccountOrIban => "Bank account or IBAN identifier detected",
        GovernanceFindingType::CredentialFile => "Credential store or `.env` secret file detected",
    };
    let rec = match ft {
        GovernanceFindingType::PrivateKeyBlock | GovernanceFindingType::CloudApiKey => {
            "Mark Secret or stage exposed copy to 30-day Quarantine; rotate token if shared."
        }
        GovernanceFindingType::CredentialFile => {
            "Classify Secret and exclude from AI prompts, keeping intact inside repositories."
        }
        GovernanceFindingType::PaymentCard
        | GovernanceFindingType::GovernmentId
        | GovernanceFindingType::BankAccountOrIban => {
            "Mark Confidential to mask previews and exclude from AI prompts while keeping searchable."
        }
    };
    (format!("{base} ({loc_count} location(s))"), rec.into())
}

pub fn list_privacy_rules(conn: &Connection) -> Result<Vec<GovernancePrivacyRule>> {
    let mut stmt = conn.prepare(
        "SELECT id, name, path_prefix, filename_pattern, rule_action, enabled, created_at, updated_at
         FROM governance_privacy_rules ORDER BY created_at DESC",
    )?;
    let rows = stmt.query_map([], |r| {
        let act_str: String = r.get(4)?;
        Ok(GovernancePrivacyRule {
            id: r.get(0)?,
            name: r.get(1)?,
            path_prefix: r.get(2)?,
            filename_pattern: r.get(3)?,
            rule_action: GovernanceRuleAction::from_db(&act_str)
                .unwrap_or(GovernanceRuleAction::AutoConfidential),
            enabled: r.get::<_, i64>(5)? != 0,
            created_at: r.get(6)?,
            updated_at: r.get(7)?,
        })
    })?;
    let mut out = Vec::new();
    for r in rows {
        out.push(r?);
    }
    Ok(out)
}

pub fn save_privacy_rule(
    conn: &Connection,
    input: &GovernancePrivacyRuleInput,
) -> Result<GovernancePrivacyRule> {
    let name = input.name.trim();
    if name.is_empty() {
        return Err(AegisError::InvalidInput(
            "Privacy rule name must not be empty.".into(),
        ));
    }
    let clean_prefix = validate_rule_prefix(conn, input.path_prefix.as_deref())?;
    let clean_pat = input
        .filename_pattern
        .as_deref()
        .map(str::trim)
        .filter(|s| !s.is_empty())
        .map(str::to_string);
    if clean_prefix.is_none() && clean_pat.is_none() {
        return Err(AegisError::InvalidInput(
            "Provide at least a folder prefix or filename pattern for a privacy rule.".into(),
        ));
    }

    let now = now_ms();
    let enabled = input.enabled.unwrap_or(true);
    let id = input
        .id
        .as_deref()
        .map(str::trim)
        .filter(|s| !s.is_empty())
        .map(str::to_string)
        .unwrap_or_else(new_id);

    conn.execute(
        "INSERT INTO governance_privacy_rules
            (id, name, path_prefix, filename_pattern, rule_action, enabled, created_at, updated_at)
         VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?7)
         ON CONFLICT(id) DO UPDATE SET
            name = excluded.name,
            path_prefix = excluded.path_prefix,
            filename_pattern = excluded.filename_pattern,
            rule_action = excluded.rule_action,
            enabled = excluded.enabled,
            updated_at = excluded.updated_at",
        params![
            id,
            name,
            clean_prefix,
            clean_pat,
            input.rule_action.as_str(),
            i64::from(enabled),
            now
        ],
    )?;

    list_privacy_rules(conn)?
        .into_iter()
        .find(|r| r.id == id)
        .ok_or_else(|| AegisError::NotFound(format!("privacy rule {id}")))
}

pub fn delete_privacy_rule(conn: &Connection, rule_id: &str) -> Result<bool> {
    let changed = conn.execute(
        "DELETE FROM governance_privacy_rules WHERE id = ?1",
        params![rule_id],
    )?;
    Ok(changed > 0)
}

/// Applies reversible `R7` sensitivity labels (`confidential`/`secret`/etc.) across files & linked findings (`K1`).
pub fn apply_sensitivity_labels(
    store: &mut SqliteStore,
    file_ids: &[String],
    target_sensitivity: &str,
    finding_ids: &[String],
    reason: &str,
) -> Result<GovernanceLabelBatch> {
    if !matches!(
        target_sensitivity,
        "public" | "internal" | "confidential" | "secret"
    ) {
        return Err(AegisError::InvalidInput(format!(
            "Unsupported sensitivity level: {target_sensitivity}"
        )));
    }

    let mut unique_files: Vec<String> = Vec::new();
    let mut seen_f = HashSet::new();
    for fid in file_ids {
        if seen_f.insert(fid.clone()) {
            unique_files.push(fid.clone());
        }
    }
    if unique_files.is_empty() {
        return Err(AegisError::InvalidInput(
            "Select at least one file to classify.".into(),
        ));
    }

    let batch_id = new_id();
    let now = now_ms();
    let mut linked_findings: HashSet<String> = finding_ids.iter().cloned().collect();

    store.with_full_sync_tx(|tx| {
        let mut applied_count = 0i64;
        let mut entries: Vec<(String, String)> = Vec::new();

        for fid in &unique_files {
            let prev: Option<String> = tx
                .query_row(
                    "SELECT COALESCE(f.sensitivity, 'normal')
                     FROM files f
                     JOIN volumes v ON v.id = f.volume_id
                     WHERE f.id = ?1 AND f.deleted_at IS NULL
                       AND f.state NOT IN ('quarantine', 'missing')
                       AND COALESCE(v.role, 'unassigned') != 'backup'",
                    params![fid],
                    |r| r.get(0),
                )
                .optional()?;
            let Some(prev_sens) = prev else {
                continue;
            };
            entries.push((fid.clone(), prev_sens));
            tx.execute(
                "UPDATE files SET sensitivity = ?1, updated_at = ?2 WHERE id = ?3",
                params![target_sensitivity, now, fid],
            )?;
            applied_count += 1;

            if is_sensitive(target_sensitivity) {
                let mut st = tx.prepare(
                    "SELECT DISTINCT finding_id FROM security_finding_locations WHERE file_id = ?1",
                )?;
                let rows = st.query_map([fid], |r| r.get::<_, String>(0))?;
                for r in rows {
                    linked_findings.insert(r?);
                }
            }
        }

        if applied_count == 0 {
            return Err(AegisError::InvalidInput(
                "No eligible live non-backup files found to classify.".into(),
            ));
        }

        tx.execute(
            "INSERT INTO governance_label_batches
                (id, target_sensitivity, file_count, reason, status, created_at)
             VALUES (?1, ?2, ?3, ?4, 'applied', ?5)",
            params![batch_id, target_sensitivity, applied_count, reason, now],
        )?;

        let mut finding_iter = linked_findings.into_iter().collect::<Vec<_>>();
        finding_iter.sort();
        let mut finding_idx = 0usize;

        for (fid, prev_sens) in entries {
            let mut fid_link = None;
            let mut prev_fst = None;
            if is_sensitive(target_sensitivity) && finding_idx < finding_iter.len() {
                let fn_id = &finding_iter[finding_idx];
                finding_idx += 1;
                if let Some(st) = tx
                    .query_row(
                        "SELECT status FROM security_findings WHERE id = ?1",
                        params![fn_id],
                        |r| r.get::<_, String>(0),
                    )
                    .optional()?
                {
                    tx.execute(
                        "UPDATE security_findings SET status = 'vaulted', resolved_at = ?1 WHERE id = ?2",
                        params![now, fn_id],
                    )?;
                    fid_link = Some(fn_id.clone());
                    prev_fst = Some(st);
                }
            }
            tx.execute(
                "INSERT INTO governance_label_entries
                    (id, batch_id, file_id, prev_sensitivity, new_sensitivity, finding_id, prev_finding_status)
                 VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7)",
                params![
                    new_id(),
                    batch_id,
                    fid,
                    prev_sens,
                    target_sensitivity,
                    fid_link,
                    prev_fst
                ],
            )?;
        }

        // Vault any remaining linked findings even if more findings than files.
        while finding_idx < finding_iter.len() {
            let fn_id = &finding_iter[finding_idx];
            finding_idx += 1;
            if let Some(st) = tx
                .query_row(
                    "SELECT status FROM security_findings WHERE id = ?1",
                    params![fn_id],
                    |r| r.get::<_, String>(0),
                )
                .optional()?
            {
                tx.execute(
                    "UPDATE security_findings SET status = 'vaulted', resolved_at = ?1 WHERE id = ?2",
                    params![now, fn_id],
                )?;
                let fallback_fid = &unique_files[0];
                tx.execute(
                    "INSERT INTO governance_label_entries
                        (id, batch_id, file_id, prev_sensitivity, new_sensitivity, finding_id, prev_finding_status)
                     VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7)",
                    params![
                        new_id(),
                        batch_id,
                        fallback_fid,
                        target_sensitivity,
                        target_sensitivity,
                        Some(fn_id.clone()),
                        Some(st)
                    ],
                )?;
            }
        }

        append_audit_in_tx(
            tx,
            "user",
            "governance.apply_labels",
            &batch_id,
            &serde_json::json!({
                "target_sensitivity": target_sensitivity,
                "files": applied_count
            })
            .to_string(),
        )?;

        Ok(GovernanceLabelBatch {
            id: batch_id.clone(),
            target_sensitivity: target_sensitivity.to_string(),
            file_count: applied_count,
            reason: reason.to_string(),
            status: "applied".into(),
            created_at: now,
            undone_at: None,
            can_undo: true,
        })
    })
}

/// Reverts a `governance_label_batches` batch in one click (`K1`), restoring prior labels & finding states.
pub fn undo_sensitivity_batch(
    store: &mut SqliteStore,
    batch_id: &str,
) -> Result<GovernanceLabelBatch> {
    let now = now_ms();
    store.with_full_sync_tx(|tx| {
        let row: Option<(String, i64, String, String, i64)> = tx
            .query_row(
                "SELECT target_sensitivity, file_count, reason, status, created_at
                 FROM governance_label_batches WHERE id = ?1",
                params![batch_id],
                |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?, r.get(3)?, r.get(4)?)),
            )
            .optional()?;
        let Some((target_sensitivity, file_count, reason, status, created_at)) = row else {
            return Err(AegisError::NotFound(format!("label batch {batch_id}")));
        };
        if status == "undone" {
            return Err(AegisError::InvalidInput(
                "This classification batch has already been undone.".into(),
            ));
        }

        let entries: Vec<(String, String, Option<String>, Option<String>)> = {
            let mut st = tx.prepare(
                "SELECT file_id, prev_sensitivity, finding_id, prev_finding_status
                 FROM governance_label_entries WHERE batch_id = ?1",
            )?;
            let rows = st.query_map([batch_id], |r| {
                Ok((r.get(0)?, r.get(1)?, r.get(2)?, r.get(3)?))
            })?;
            let mut out = Vec::new();
            for r in rows {
                out.push(r?);
            }
            out
        };

        for (fid, prev_sens, finding_id, prev_fst) in entries {
            tx.execute(
                "UPDATE files SET sensitivity = ?1, updated_at = ?2 WHERE id = ?3",
                params![prev_sens, now, fid],
            )?;
            if let (Some(fn_id), Some(prev_st)) = (finding_id, prev_fst) {
                let res_at = if prev_st == "open" { None } else { Some(now) };
                tx.execute(
                    "UPDATE security_findings SET status = ?1, resolved_at = ?2 WHERE id = ?3",
                    params![prev_st, res_at, fn_id],
                )?;
            }
        }

        tx.execute(
            "UPDATE governance_label_batches SET status = 'undone', undone_at = ?1 WHERE id = ?2",
            params![now, batch_id],
        )?;
        append_audit_in_tx(
            tx,
            "user",
            "governance.undo_labels",
            batch_id,
            &serde_json::json!({ "restored_files": file_count }).to_string(),
        )?;

        Ok(GovernanceLabelBatch {
            id: batch_id.to_string(),
            target_sensitivity,
            file_count,
            reason,
            status: "undone".into(),
            created_at,
            undone_at: Some(now),
            can_undo: false,
        })
    })
}

pub fn apply_privacy_rule(
    store: &mut SqliteStore,
    rule_id: &str,
) -> Result<GovernanceLabelBatch> {
    let rule = list_privacy_rules(store.conn())?
        .into_iter()
        .find(|r| r.id == rule_id)
        .ok_or_else(|| AegisError::NotFound(format!("privacy rule {rule_id}")))?;

    let target_sens = match rule.rule_action {
        GovernanceRuleAction::AutoConfidential => "confidential",
        GovernanceRuleAction::AutoSecret => "secret",
        GovernanceRuleAction::ExcludeFromAi => {
            return Err(AegisError::InvalidInput(
                "AI exclusion rules enforce automatically on Ask Files queries without relabeling files.".into(),
            ));
        }
    };

    let matched_ids: Vec<String> = {
        let mut st = store.conn().prepare(
            "SELECT f.id, f.path, f.name, COALESCE(v.case_sensitive, 0)
             FROM files f
             JOIN volumes v ON v.id = f.volume_id
             WHERE f.deleted_at IS NULL
               AND f.state NOT IN ('quarantine', 'missing')
               AND f.container_file_id IS NULL
               AND COALESCE(v.role, 'unassigned') != 'backup'",
        )?;
        let rows = st.query_map([], |r| {
            Ok((
                r.get::<_, String>(0)?,
                r.get::<_, String>(1)?,
                r.get::<_, String>(2)?,
                r.get::<_, i64>(3)? != 0,
            ))
        })?;
        let mut out = Vec::new();
        for r in rows {
            let (id, p, name, cs) = r?;
            if rule_matches_file(
                &p,
                &name,
                rule.path_prefix.as_deref(),
                rule.filename_pattern.as_deref(),
                cs,
            ) {
                out.push(id);
            }
        }
        out
    };

    apply_sensitivity_labels(
        store,
        &matched_ids,
        target_sens,
        &[],
        &format!("Privacy rule: {}", rule.name),
    )
}

pub fn set_finding_status(
    store: &mut SqliteStore,
    finding_id: &str,
    status: GovernanceFindingStatus,
) -> Result<()> {
    let now = now_ms();
    let res_at = if status == GovernanceFindingStatus::Open {
        None
    } else {
        Some(now)
    };
    store.with_full_sync_tx(|tx| {
        let changed = tx.execute(
            "UPDATE security_findings SET status = ?1, resolved_at = ?2 WHERE id = ?3",
            params![status.as_str(), res_at, finding_id],
        )?;
        if changed == 0 {
            return Err(AegisError::NotFound(format!("finding {finding_id}")));
        }
        append_audit_in_tx(
            tx,
            "user",
            "governance.triage_finding",
            finding_id,
            &serde_json::json!({ "status": status.as_str() }).to_string(),
        )?;
        Ok(())
    })
}

/// Stages exposed secret/credential files for reversible 30-day Quarantine through Simulator (`K1`, `K2`, `R8`).
pub fn propose_governance_quarantine(
    store: &mut SqliteStore,
    file_ids: &[String],
    include_sensitive: bool,
    reveal_sensitive: bool,
) -> Result<String> {
    if include_sensitive && !reveal_sensitive {
        return Err(AegisError::PermissionDenied(
            "Windows Hello / Windows user verification is required before quarantining sensitive files."
                .into(),
        ));
    }
    if file_ids.is_empty() {
        return Err(AegisError::InvalidInput(
            "Select at least one exposed file to quarantine.".into(),
        ));
    }

    let ctx = build_policy_context(store)?;
    let allow_sens = include_sensitive && reveal_sensitive;
    let mut actions = Vec::new();
    let mut seen = HashSet::new();

    for fid in file_ids {
        if !seen.insert(fid.clone()) {
            continue;
        }
        let open_exists: bool = store.conn().query_row(
            "SELECT EXISTS(SELECT 1 FROM actions WHERE file_id = ?1 AND status IN ('proposed', 'approved'))",
            params![fid],
            |r| r.get(0),
        )?;
        if open_exists {
            continue;
        }

        struct QuarantineFileCandidate {
            path: String,
            sensitivity: String,
            atomic_unit_id: Option<String>,
            au_root: Option<String>,
            role: String,
        }
        let row: Option<QuarantineFileCandidate> = store
            .conn()
            .query_row(
                "SELECT f.path, COALESCE(f.sensitivity, 'normal'), f.atomic_unit_id,
                        au.root_path, COALESCE(v.role, 'unassigned')
                 FROM files f
                 JOIN volumes v ON v.id = f.volume_id
                 LEFT JOIN atomic_units au ON au.id = f.atomic_unit_id
                 WHERE f.id = ?1 AND f.deleted_at IS NULL AND f.state NOT IN ('quarantine', 'missing')",
                params![fid],
                |r| {
                    Ok(QuarantineFileCandidate {
                        path: r.get(0)?,
                        sensitivity: r.get(1)?,
                        atomic_unit_id: r.get(2)?,
                        au_root: r.get(3)?,
                        role: r.get(4)?,
                    })
                },
            )
            .optional()?;
        let Some(cand) = row else {
            continue;
        };
        let (path, sensitivity, atomic_unit_id, au_root, role) = (
            cand.path,
            cand.sensitivity,
            cand.atomic_unit_id,
            cand.au_root,
            cand.role,
        );
        if role == "backup" {
            continue;
        }
        if is_sensitive(&sensitivity) && !allow_sens {
            return Err(AegisError::PermissionDenied(
                "Unlock sensitive views via Windows authentication before quarantining sensitive files."
                    .into(),
            ));
        }
        let verdict = evaluate_policy(
            &PolicyAction {
                action_type: ActionType::Quarantine,
                target_kind: TargetKind::File,
                agent: Some(GOVERNANCE_AGENT),
                from_path: Path::new(&path),
                to_path: None,
                confidence: 0.96,
                atomic_unit_root: au_root.as_deref().map(Path::new),
                unit_allow_split_override: false,
                file_unit_split_override: false,
                is_registered_cache_dir: false,
            },
            &ctx,
        );
        if matches!(verdict, PolicyVerdict::Block(_)) {
            continue;
        }

        actions.push(ProposedActionInput {
            agent: Some(GOVERNANCE_AGENT.into()),
            action_type: ActionType::Quarantine,
            target_kind: TargetKind::File,
            file_id: Some(fid.clone()),
            atomic_unit_id,
            from_path: path,
            to_path: None,
            reason: "Exposed secret / credential file staged for reversible 30-day Quarantine"
                .into(),
            confidence: 0.96,
            evidence_json: serde_json::json!({ "source": "personal_governance" }).to_string(),
        });
    }

    if actions.is_empty() {
        return Err(AegisError::InvalidInput(
            "Selected files are blocked by policy (outside your folders, inside a project kept whole, or already waiting in a batch).".into(),
        ));
    }

    propose_batch(
        store,
        &RefilePlan {
            origin: GOVERNANCE_AGENT.into(),
            actions,
        },
    )
}

pub fn list_governance_dashboard(
    conn: &Connection,
    opts: &GovernanceQueryOptions,
    vault_ctx: &VaultPostureContext,
    reveal_sensitive: bool,
    live_probe_bitlocker: bool,
) -> Result<GovernanceDashboard> {
    list_governance_dashboard_inner(
        conn,
        opts,
        vault_ctx,
        reveal_sensitive,
        live_probe_bitlocker,
        None,
    )
}

pub fn list_governance_dashboard_with_drives(
    conn: &Connection,
    opts: &GovernanceQueryOptions,
    vault_ctx: &VaultPostureContext,
    reveal_sensitive: bool,
    drives_override: Vec<DriveEncryptionStatus>,
) -> Result<GovernanceDashboard> {
    list_governance_dashboard_inner(
        conn,
        opts,
        vault_ctx,
        reveal_sensitive,
        false,
        Some(drives_override),
    )
}

fn list_governance_dashboard_inner(
    conn: &Connection,
    opts: &GovernanceQueryOptions,
    vault_ctx: &VaultPostureContext,
    reveal_sensitive: bool,
    live_probe_bitlocker: bool,
    drives_override: Option<Vec<DriveEncryptionStatus>>,
) -> Result<GovernanceDashboard> {
    if opts.include_sensitive && !reveal_sensitive {
        return Err(AegisError::PermissionDenied(
            "Windows Hello / Windows user verification is required before viewing unmasked sensitive findings."
                .into(),
        ));
    }

    let allow_unmasked = opts.include_sensitive && reveal_sensitive;
    let posture: PrivacyPostureReport = match drives_override {
        Some(drives) => build_privacy_posture_with_drives(conn, vault_ctx, drives)?,
        None => build_privacy_posture(conn, vault_ctx, live_probe_bitlocker)?,
    };

    let mut excluded_sensitive_count = 0i64;
    let mut loc_map: HashMap<String, Vec<GovernanceFindingLocation>> = HashMap::new();
    {
        let mut lst = conn.prepare(
            "SELECT l.finding_id, l.file_id, f.path, f.name, COALESCE(v.label, v.mount_point),
                    COALESCE(f.sensitivity, 'normal'), l.line_number, l.redacted_snippet,
                    f.atomic_unit_id IS NOT NULL
             FROM security_finding_locations l
             JOIN files f ON f.id = l.file_id
             JOIN volumes v ON v.id = f.volume_id
             WHERE f.deleted_at IS NULL
               AND f.state NOT IN ('quarantine', 'missing')
               AND COALESCE(v.role, 'unassigned') != 'backup'
             ORDER BY l.line_number ASC",
        )?;
        for r in lst.query_map([], |row| {
            Ok((
                row.get::<_, String>(0)?,
                row.get::<_, String>(1)?,
                row.get::<_, String>(2)?,
                row.get::<_, String>(3)?,
                row.get::<_, String>(4)?,
                row.get::<_, String>(5)?,
                row.get::<_, i64>(6)?,
                row.get::<_, String>(7)?,
                row.get::<_, i64>(8)? != 0,
            ))
        })? {
            let (finding_id, file_id, file_path, file_name, drive_label, sens, line_number, snip, in_au) =
                r?;
            let is_sens = is_sensitive(&sens);
            let display_snip = if is_sens && !allow_unmasked {
                excluded_sensitive_count += 1;
                SENSITIVE_MASK.to_string()
            } else {
                snip
            };
            loc_map
                .entry(finding_id)
                .or_default()
                .push(GovernanceFindingLocation {
                    file_id,
                    file_path,
                    file_name,
                    drive_label,
                    sensitivity: sens,
                    line_number,
                    redacted_snippet: display_snip,
                    in_atomic_unit: in_au,
                });
        }
    }

    let mut findings = Vec::new();
    {
        let mut fst = conn.prepare(
            "SELECT id, finding_type, base_severity, context_risk_score, status, created_at, resolved_at
             FROM security_findings
             ORDER BY CASE status WHEN 'open' THEN 0 WHEN 'vaulted' THEN 1 ELSE 2 END,
                      context_risk_score DESC, created_at DESC",
        )?;
        for r in fst.query_map([], |row| {
            Ok((
                row.get::<_, String>(0)?,
                row.get::<_, String>(1)?,
                row.get::<_, String>(2)?,
                row.get::<_, f64>(3)?,
                row.get::<_, String>(4)?,
                row.get::<_, i64>(5)?,
                row.get::<_, Option<i64>>(6)?,
            ))
        })? {
            let (id, ft_s, sev_s, risk, st_s, created_at, resolved_at) = r?;
            let finding_type = GovernanceFindingType::from_db(&ft_s);
            let severity = GovernanceSeverity::from_db(&sev_s);
            let status = GovernanceFindingStatus::from_db(&st_s)
                .unwrap_or(GovernanceFindingStatus::Open);
            if let Some(sf) = opts.status_filter
                && sf != status
            {
                continue;
            }
            if let Some(svf) = opts.severity_filter
                && svf != severity
            {
                continue;
            }
            let locations = loc_map.remove(&id).unwrap_or_default();
            if locations.is_empty() && status == GovernanceFindingStatus::Open {
                continue;
            }
            let (summary, recommended_action) =
                finding_summary_and_action(finding_type, locations.len());
            findings.push(GovernanceFinding {
                id,
                finding_type,
                severity,
                context_risk_score: (risk * 100.0).round() / 100.0,
                status,
                created_at,
                resolved_at,
                summary,
                recommended_action,
                locations,
            });
        }
    }

    let mut open_counts: BTreeMap<String, i64> = BTreeMap::new();
    {
        let mut oc = conn.prepare(
            "SELECT l.file_id, COUNT(DISTINCT f.id)
             FROM security_finding_locations l
             JOIN security_findings f ON f.id = l.finding_id
             WHERE f.status = 'open'
             GROUP BY l.file_id",
        )?;
        for r in oc.query_map([], |row| Ok((row.get::<_, String>(0)?, row.get::<_, i64>(1)?)))? {
            let (fid, cnt) = r?;
            open_counts.insert(fid, cnt);
        }
    }

    let mut sensitive_files = Vec::new();
    {
        let mut sst = conn.prepare(
            "SELECT f.id, f.path, f.name, COALESCE(v.label, v.mount_point),
                    f.sensitivity, f.size, f.mtime
             FROM files f
             JOIN volumes v ON v.id = f.volume_id
             WHERE f.deleted_at IS NULL
               AND f.state NOT IN ('quarantine', 'missing')
               AND COALESCE(v.role, 'unassigned') != 'backup'
               AND COALESCE(f.sensitivity, 'normal') IN ('confidential', 'secret')
             ORDER BY f.mtime DESC LIMIT 200",
        )?;
        for r in sst.query_map([], |row| {
            Ok((
                row.get::<_, String>(0)?,
                row.get::<_, String>(1)?,
                row.get::<_, String>(2)?,
                row.get::<_, String>(3)?,
                row.get::<_, String>(4)?,
                row.get::<_, i64>(5)?,
                row.get::<_, i64>(6)?,
            ))
        })? {
            let (file_id, path, name, drive_label, sensitivity, size_bytes, mtime) = r?;
            let open_findings_count = open_counts.get(&file_id).copied().unwrap_or(0);
            sensitive_files.push(SensitiveFileSummary {
                file_id,
                path,
                name,
                drive_label,
                sensitivity,
                size_bytes,
                mtime,
                open_findings_count,
            });
        }
    }

    let rules = list_privacy_rules(conn)?;

    let mut label_batches = Vec::new();
    {
        let mut bst = conn.prepare(
            "SELECT id, target_sensitivity, file_count, reason, status, created_at, undone_at
             FROM governance_label_batches ORDER BY created_at DESC LIMIT 20",
        )?;
        for r in bst.query_map([], |row| {
            let status: String = row.get(4)?;
            Ok(GovernanceLabelBatch {
                id: row.get(0)?,
                target_sensitivity: row.get(1)?,
                file_count: row.get(2)?,
                reason: row.get(3)?,
                can_undo: status == "applied",
                status,
                created_at: row.get(5)?,
                undone_at: row.get(6)?,
            })
        })? {
            label_batches.push(r?);
        }
    }

    Ok(GovernanceDashboard {
        posture,
        findings,
        sensitive_files,
        rules,
        label_batches,
        excluded_sensitive_count,
    })
}
