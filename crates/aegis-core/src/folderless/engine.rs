//! Core execution, rule persistence, anchor extraction, dry-run matching, and `K1` batch staging
//! for AI Folderless Mode (`2.1`, reshaped `R6`) (`K1`–`K5`, `R4`, `R6`, `R7`, `R9`, `D1`).

use std::collections::{HashMap, HashSet};
use std::path::{Component, Path, PathBuf};
use std::time::Instant;

use rusqlite::{Connection, OptionalExtension, params};

use crate::assets::recovery::{open_batch_paths, path_key, resolve_unique_dest, same_parent_dir};
use crate::care::path_contains_aegis_internal;
use crate::contracts::{ActionType, ProposedActionInput, RefilePlan, TargetKind};
use crate::db::SqliteStore;
use crate::db::actions::{build_policy_context, propose_batch};
use crate::db::files::{is_sensitive, path_is_within};
use crate::error::{AegisError, Result};
use crate::filing::classify::kind_of;
use crate::filing::naming::{folder_name, sanitize};
use crate::filing::orgs::{find_org, has_word};
use crate::policy::{PolicyAction, PolicyVerdict, evaluate_policy};
use crate::versions::patterns::is_excluded_relative_path;
use crate::{format_date_ms, new_id, now_ms};

use super::{
    FolderlessBatchSummary, FolderlessMatchPreview, FolderlessPreviewReport, FolderlessRule,
    FolderlessRuleInput,
};

/// Agent identifier persisted in `actions.agent` for Folderless rule batches (`K1`).
pub const FOLDERLESS_AGENT: &str = "folderless_rule";

const DAY_MS: i64 = 86_400_000;
const MAX_FALLBACK_TEXT_BYTES: i64 = 524_288;

struct FileCandidateRow {
    file_id: String,
    mount_point: String,
    path: String,
    name: String,
    ext: String,
    size_bytes: i64,
    mtime: i64,
    cloud_state: String,
    drive_label: String,
    drive_online: bool,
    sensitivity: String,
    atomic_unit_id: Option<String>,
    meta_kind: Option<String>,
    meta_category: Option<String>,
    meta_doc_type: Option<String>,
    meta_who: Option<String>,
    meta_doc_date: Option<String>,
    meta_exif_taken_at: Option<i64>,
}

struct ContentAnchors {
    date: String,
    year: String,
    month: String,
    quarter: String,
    org: String,
    doc_type: String,
    project: String,
    amount: Option<String>,
}

/// Validates a rule destination root (`K4`, protected OS folders, no `..` traversal or Cairn internals).
pub fn validate_dest_root(dest_root: &str) -> Result<()> {
    let trimmed = dest_root.trim();
    if trimmed.is_empty() {
        return Err(AegisError::InvalidInput(
            "Destination root must not be empty.".into(),
        ));
    }
    let p = Path::new(trimmed);
    if p.components().any(|c| matches!(c, Component::ParentDir)) {
        return Err(AegisError::PermissionDenied(
            "Destination root must not contain '..' path traversal.".into(),
        ));
    }
    if path_contains_aegis_internal(p) {
        return Err(AegisError::PermissionDenied(
            "Destination root cannot point into Cairn internal directories.".into(),
        ));
    }
    let sys_temp = std::env::temp_dir().to_string_lossy().to_string();
    let in_sys_temp = path_is_within(trimmed, &sys_temp, false);
    for comp in p.components() {
        let lower = comp.as_os_str().to_string_lossy().to_ascii_lowercase();
        let is_protected = matches!(
            lower.as_str(),
            "windows"
                | "program files"
                | "program files (x86)"
                | "programdata"
                | "$recycle.bin"
                | "system volume information"
        ) || (lower == "appdata" && !in_sys_temp);
        if is_protected {
            return Err(AegisError::PermissionDenied(format!(
                "Destination root cannot target protected system folder `{lower}`."
            )));
        }
    }
    Ok(())
}

fn ensure_not_backup_destination(conn: &Connection, dest_root: &str) -> Result<()> {
    let mut st = conn.prepare(
        "SELECT mount_point, COALESCE(case_sensitive, 0) FROM volumes WHERE role = 'backup'",
    )?;
    for row in st.query_map([], |r| Ok((r.get::<_, String>(0)?, r.get::<_, i64>(1)? != 0)))? {
        let (mount, cs) = row?;
        if path_is_within(dest_root, &mount, cs) {
            return Err(AegisError::PermissionDenied(
                "Destination root cannot target a backup volume (K4).".into(),
            ));
        }
    }
    Ok(())
}

fn ensure_sensitive_allowed(include_sensitive: bool, reveal_sensitive: bool) -> Result<()> {
    if include_sensitive && !reveal_sensitive {
        return Err(AegisError::PermissionDenied(
            "Unlock sensitive previews with Windows authentication before including confidential or secret files in Folderless rules (R7)."
                .into(),
        ));
    }
    Ok(())
}

fn rule_from_row(r: &rusqlite::Row<'_>) -> rusqlite::Result<FolderlessRule> {
    let parse_vec = |raw: String| -> Vec<String> { serde_json::from_str(&raw).unwrap_or_default() };
    Ok(FolderlessRule {
        id: r.get(0)?,
        name: r.get(1)?,
        intent_text: r.get(2)?,
        compiled_by: r.get(3)?,
        enabled: r.get::<_, i64>(4)? != 0,
        scope_root: r.get(5)?,
        match_kinds: parse_vec(r.get(6)?),
        match_exts: parse_vec(r.get(7)?),
        match_categories: parse_vec(r.get(8)?),
        match_doc_types: parse_vec(r.get(9)?),
        match_keywords: parse_vec(r.get(10)?),
        match_orgs: parse_vec(r.get(11)?),
        min_age_days: r.get(12)?,
        dest_root: r.get(13)?,
        path_template: r.get(14)?,
        rename_template: r.get(15)?,
        approvals: r.get(16)?,
        rejections: r.get(17)?,
        state: r.get(18)?,
        created_at: r.get(19)?,
        updated_at: r.get(20)?,
    })
}

const RULE_COLS: &str = "id, name, intent_text, compiled_by, enabled, scope_root, \
    match_kinds_json, match_exts_json, match_categories_json, match_doc_types_json, \
    match_keywords_json, match_orgs_json, min_age_days, dest_root, path_template, \
    rename_template, approvals, rejections, state, created_at, updated_at";

/// Lists all saved Folderless rules inside SQLCipher `aegis.db` (`R6`, `D1`).
pub fn list_folderless_rules(conn: &Connection) -> Result<Vec<FolderlessRule>> {
    let mut stmt = conn.prepare(&format!(
        "SELECT {RULE_COLS} FROM folderless_rules ORDER BY enabled DESC, updated_at DESC, name ASC"
    ))?;
    let rows = stmt.query_map([], rule_from_row)?;
    rows.collect::<rusqlite::Result<_>>().map_err(Into::into)
}

/// Fetches a single Folderless rule by ID.
pub fn get_folderless_rule(conn: &Connection, id: &str) -> Result<FolderlessRule> {
    conn.query_row(
        &format!("SELECT {RULE_COLS} FROM folderless_rules WHERE id = ?1"),
        params![id],
        rule_from_row,
    )
    .optional()?
    .ok_or_else(|| AegisError::NotFound(format!("folderless rule {id}")))
}

fn clean_list(items: &[String], lowercase: bool) -> Vec<String> {
    let mut out = Vec::new();
    for item in items {
        let trimmed = item.trim();
        if trimmed.is_empty() {
            continue;
        }
        let val = if lowercase {
            trimmed.to_ascii_lowercase()
        } else {
            trimmed.to_string()
        };
        if !out.contains(&val) {
            out.push(val);
        }
    }
    out
}

/// Inserts or updates a visible Folderless filing rule after validating safety guards (`R6`, `D1`).
pub fn save_folderless_rule(conn: &Connection, input: &FolderlessRuleInput) -> Result<FolderlessRule> {
    let name = input.name.trim();
    if name.is_empty() {
        return Err(AegisError::InvalidInput("Rule name must not be empty.".into()));
    }
    let intent_text = input.intent_text.trim();
    if intent_text.is_empty() {
        return Err(AegisError::InvalidInput(
            "Rule intent text must not be empty.".into(),
        ));
    }
    validate_dest_root(&input.dest_root)?;
    ensure_not_backup_destination(conn, &input.dest_root)?;

    let path_tmpl = input.path_template.trim().replace('/', "\\");
    if path_tmpl.contains("..") {
        return Err(AegisError::PermissionDenied(
            "Path template cannot contain '..' traversal.".into(),
        ));
    }
    let rename_tmpl = input
        .rename_template
        .as_deref()
        .map(str::trim)
        .filter(|s| !s.is_empty())
        .map(str::to_string);
    if let Some(ref rt) = rename_tmpl
        && (rt.contains("..") || rt.contains(['\\', '/']))
    {
        return Err(AegisError::PermissionDenied(
            "Rename template must be a filename template without directory separators.".into(),
        ));
    }

    let now = now_ms();
    let existing = match input.id.as_deref().filter(|s| !s.trim().is_empty()) {
        Some(eid) => conn
            .query_row(
                &format!("SELECT {RULE_COLS} FROM folderless_rules WHERE id = ?1"),
                params![eid],
                rule_from_row,
            )
            .optional()?,
        None => None,
    };

    let id = existing
        .as_ref()
        .map(|r| r.id.clone())
        .unwrap_or_else(new_id);
    let created_at = existing.as_ref().map(|r| r.created_at).unwrap_or(now);
    let approvals = existing.as_ref().map(|r| r.approvals).unwrap_or(0);
    let rejections = existing.as_ref().map(|r| r.rejections).unwrap_or(0);
    let state = existing
        .as_ref()
        .map(|r| r.state.clone())
        .unwrap_or_else(|| "asking".into());

    let compiled_by = input
        .compiled_by
        .as_deref()
        .map(str::trim)
        .filter(|s| !s.is_empty())
        .unwrap_or("deterministic")
        .to_string();
    let enabled = input.enabled.unwrap_or(true);
    let scope_root = input
        .scope_root
        .as_deref()
        .map(str::trim)
        .filter(|s| !s.is_empty())
        .map(str::to_string);
    let match_kinds = clean_list(&input.match_kinds, true);
    let match_exts = clean_list(&input.match_exts, true);
    let match_categories = clean_list(&input.match_categories, false);
    let match_doc_types = clean_list(&input.match_doc_types, true);
    let match_keywords = clean_list(&input.match_keywords, true);
    let match_orgs = clean_list(&input.match_orgs, false);

    conn.execute(
        "INSERT INTO folderless_rules (
            id, name, intent_text, compiled_by, enabled, scope_root,
            match_kinds_json, match_exts_json, match_categories_json,
            match_doc_types_json, match_keywords_json, match_orgs_json,
            min_age_days, dest_root, path_template, rename_template,
            approvals, rejections, state, created_at, updated_at
         ) VALUES (
            ?1, ?2, ?3, ?4, ?5, ?6,
            ?7, ?8, ?9, ?10, ?11, ?12,
            ?13, ?14, ?15, ?16,
            ?17, ?18, ?19, ?20, ?21
         )
         ON CONFLICT(id) DO UPDATE SET
            name = excluded.name,
            intent_text = excluded.intent_text,
            compiled_by = excluded.compiled_by,
            enabled = excluded.enabled,
            scope_root = excluded.scope_root,
            match_kinds_json = excluded.match_kinds_json,
            match_exts_json = excluded.match_exts_json,
            match_categories_json = excluded.match_categories_json,
            match_doc_types_json = excluded.match_doc_types_json,
            match_keywords_json = excluded.match_keywords_json,
            match_orgs_json = excluded.match_orgs_json,
            min_age_days = excluded.min_age_days,
            dest_root = excluded.dest_root,
            path_template = excluded.path_template,
            rename_template = excluded.rename_template,
            updated_at = excluded.updated_at",
        params![
            id,
            name,
            intent_text,
            compiled_by,
            i64::from(enabled),
            scope_root,
            serde_json::to_string(&match_kinds)?,
            serde_json::to_string(&match_exts)?,
            serde_json::to_string(&match_categories)?,
            serde_json::to_string(&match_doc_types)?,
            serde_json::to_string(&match_keywords)?,
            serde_json::to_string(&match_orgs)?,
            input.min_age_days,
            input.dest_root.trim(),
            path_tmpl,
            rename_tmpl,
            approvals,
            rejections,
            state,
            created_at,
            now
        ],
    )?;

    get_folderless_rule(conn, &id)
}

/// Enables or disables a visible Folderless rule.
pub fn toggle_folderless_rule(conn: &Connection, id: &str, enabled: bool) -> Result<FolderlessRule> {
    let changed = conn.execute(
        "UPDATE folderless_rules SET enabled = ?1, updated_at = ?2 WHERE id = ?3",
        params![i64::from(enabled), now_ms(), id],
    )?;
    if changed == 0 {
        return Err(AegisError::NotFound(format!("folderless rule {id}")));
    }
    get_folderless_rule(conn, id)
}

/// Deletes a Folderless rule permanently from encrypted metadata (`R6`).
pub fn delete_folderless_rule(conn: &Connection, id: &str) -> Result<bool> {
    let changed = conn.execute("DELETE FROM folderless_rules WHERE id = ?1", params![id])?;
    Ok(changed > 0)
}

/// Converts a saved rule into `FolderlessRuleInput` for preview or editing.
pub fn rule_to_input(rule: &FolderlessRule) -> FolderlessRuleInput {
    FolderlessRuleInput {
        id: Some(rule.id.clone()),
        name: rule.name.clone(),
        intent_text: rule.intent_text.clone(),
        compiled_by: Some(rule.compiled_by.clone()),
        enabled: Some(rule.enabled),
        scope_root: rule.scope_root.clone(),
        match_kinds: rule.match_kinds.clone(),
        match_exts: rule.match_exts.clone(),
        match_categories: rule.match_categories.clone(),
        match_doc_types: rule.match_doc_types.clone(),
        match_keywords: rule.match_keywords.clone(),
        match_orgs: rule.match_orgs.clone(),
        min_age_days: rule.min_age_days,
        dest_root: rule.dest_root.clone(),
        path_template: rule.path_template.clone(),
        rename_template: rule.rename_template.clone(),
    }
}

fn extract_date_from_text(text: &str) -> Option<String> {
    let bytes = text.as_bytes();
    if bytes.len() < 10 {
        return None;
    }
    for i in 0..=bytes.len() - 10 {
        let slice = &bytes[i..i + 10];
        let sep = slice[4];
        if (sep != b'-' && sep != b'/') || slice[7] != sep {
            continue;
        }
        let digits_ok = slice[..4].iter().all(u8::is_ascii_digit)
            && slice[5..7].iter().all(u8::is_ascii_digit)
            && slice[8..10].iter().all(u8::is_ascii_digit);
        if !digits_ok {
            continue;
        }
        let year: i32 = std::str::from_utf8(&slice[0..4]).ok()?.parse().ok()?;
        let month: u32 = std::str::from_utf8(&slice[5..7]).ok()?.parse().ok()?;
        let day: u32 = std::str::from_utf8(&slice[8..10]).ok()?.parse().ok()?;
        if (1970..=2099).contains(&year) && (1..=12).contains(&month) && (1..=31).contains(&day) {
            return Some(format!("{year:04}-{month:02}-{day:02}"));
        }
    }
    None
}

fn extract_amount_anchor(text: &str) -> Option<String> {
    let mut prev = String::new();
    for tok in text.split_whitespace() {
        let cleaned = tok.trim_end_matches([',', ';', '.']);
        let prefix_currency = cleaned.starts_with('$')
            || (cleaned.starts_with('R') && cleaned[1..].chars().next().is_some_and(|c| c.is_ascii_digit()));
        if prefix_currency {
            let digits = cleaned.chars().filter(char::is_ascii_digit).count();
            if digits >= 2 {
                return Some(cleaned.to_string());
            }
        }
        if matches!(
            prev.to_ascii_uppercase().as_str(),
            "R" | "ZAR" | "USD" | "EUR" | "GBP" | "TOTAL" | "AMOUNT"
        ) && cleaned.chars().any(|c| c.is_ascii_digit())
            && cleaned
                .chars()
                .all(|c| c.is_ascii_digit() || c == ',' || c == '.' || c == '$')
        {
            return Some(format!("{prev} {cleaned}"));
        }
        prev = cleaned.to_string();
    }
    None
}

fn extract_header_org(text: &str) -> Option<String> {
    for raw_line in text.lines().take(18) {
        let line = raw_line.trim();
        let low = line.to_ascii_lowercase();
        for prefix in ["vendor:", "from:", "billed by:", "supplier:", "issuer:", "organization:"]
        {
            if let Some(rest) = low.strip_prefix(prefix) {
                let raw_val = &line[line.len() - rest.len()..];
                let clean = raw_val.split([',', ';', '|']).next().unwrap_or("").trim();
                if clean.len() >= 2 && clean.len() <= 50 {
                    return Some(clean.to_string());
                }
            }
        }
    }
    None
}

fn quarter_for_month(month: &str) -> &'static str {
    match month.parse::<u32>().unwrap_or(1) {
        1..=3 => "Q1",
        4..=6 => "Q2",
        7..=9 => "Q3",
        _ => "Q4",
    }
}

fn load_file_text_for_rule(conn: &Connection, f: &FileCandidateRow) -> Result<String> {
    let mut st =
        conn.prepare("SELECT text FROM chunks WHERE file_id = ?1 ORDER BY seq ASC LIMIT 4")?;
    let rows = st.query_map(params![f.file_id], |r| r.get::<_, String>(0))?;
    let mut parts = Vec::new();
    for r in rows {
        parts.push(r?);
    }
    if !parts.is_empty() {
        return Ok(parts.join("\n"));
    }

    if f.drive_online
        && f.cloud_state == "local"
        && f.size_bytes >= 0
        && f.size_bytes <= MAX_FALLBACK_TEXT_BYTES
        && matches!(
            f.ext.to_ascii_lowercase().as_str(),
            "md" | "txt" | "csv" | "vtt" | "srt" | "log" | "json"
        )
    {
        let p = Path::new(&f.path);
        if p.is_file()
            && let Ok(bytes) = std::fs::read(p)
        {
            return Ok(String::from_utf8_lossy(&bytes).into_owned());
        }
    }
    Ok(String::new())
}

fn load_kg_org_and_project_maps(conn: &Connection) -> Result<(HashMap<String, String>, HashMap<String, String>)> {
    let mut orgs = HashMap::new();
    let mut projs = HashMap::new();
    let mut st = conn.prepare(
        "SELECT r.src_id, e.entity_type, e.name
         FROM relationships r
         JOIN entities e ON e.id = r.dst_id
         WHERE r.src_kind = 'file'
           AND r.dst_kind = 'entity'
           AND r.valid_to IS NULL
           AND e.entity_type IN ('organization', 'project')",
    )?;
    for row in st.query_map([], |r| {
        Ok((
            r.get::<_, String>(0)?,
            r.get::<_, String>(1)?,
            r.get::<_, String>(2)?,
        ))
    })? {
        let (fid, etype, name) = row?;
        if etype == "organization" {
            orgs.entry(fid).or_insert(name);
        } else if etype == "project" {
            projs.entry(fid).or_insert(name);
        }
    }
    Ok((orgs, projs))
}

fn infer_doc_type(f: &FileCandidateRow, combined_low: &str) -> String {
    if let Some(ref dt) = f.meta_doc_type
        && !dt.trim().is_empty()
    {
        return dt.trim().to_string();
    }
    for (kw, label) in [
        ("invoice", "Invoice"),
        ("receipt", "Receipt"),
        ("tax", "Tax"),
        ("statement", "Statement"),
        ("contract", "Contract"),
        ("payslip", "Payslip"),
        ("transcript", "Transcript"),
        ("report", "Report"),
    ] {
        if has_word(combined_low, kw) {
            return label.to_string();
        }
    }
    f.meta_kind
        .clone()
        .unwrap_or_else(|| kind_of(&f.ext, &f.name, false).as_str().to_string())
}

fn build_anchors(
    f: &FileCandidateRow,
    combined_text: &str,
    combined_low: &str,
    kg_org: Option<&str>,
    kg_proj: Option<&str>,
) -> ContentAnchors {
    let date = f
        .meta_doc_date
        .clone()
        .filter(|s| s.len() >= 10)
        .or_else(|| extract_date_from_text(&format!("{}\n{}", f.name, combined_text)))
        .or_else(|| f.meta_exif_taken_at.map(format_date_ms))
        .unwrap_or_else(|| format_date_ms(f.mtime));
    let year = date.get(0..4).unwrap_or("2026").to_string();
    let month = date.get(5..7).unwrap_or("01").to_string();
    let quarter = quarter_for_month(&month).to_string();

    let org = f
        .meta_who
        .clone()
        .filter(|s| !s.trim().is_empty())
        .or_else(|| kg_org.map(str::to_string))
        .or_else(|| find_org(combined_low, &[]))
        .or_else(|| extract_header_org(combined_text))
        .unwrap_or_else(|| "General".to_string());

    let doc_type = infer_doc_type(f, combined_low);
    let project = kg_proj.unwrap_or("General").to_string();
    let amount = extract_amount_anchor(combined_text);

    ContentAnchors {
        date,
        year,
        month,
        quarter,
        org,
        doc_type,
        project,
        amount,
    }
}

fn expand_destination(
    rule: &FolderlessRuleInput,
    filename: &str,
    ext: &str,
    anchors: &ContentAnchors,
) -> (PathBuf, String, String) {
    let mut dir = PathBuf::from(rule.dest_root.trim());
    let normalized_tmpl = rule.path_template.trim().replace('/', "\\");
    for raw_seg in normalized_tmpl.split('\\').filter(|s| !s.trim().is_empty()) {
        let mut val = raw_seg.trim().to_string();
        val = val.replace("{year}", &anchors.year);
        val = val.replace("{quarter}", &anchors.quarter);
        val = val.replace("{month}", &anchors.month);
        val = val.replace("{org}", &anchors.org);
        val = val.replace("{doc_type}", &anchors.doc_type);
        val = val.replace("{project}", &anchors.project);
        val = val.replace(
            "{ext}",
            if ext.is_empty() {
                "other"
            } else {
                ext
            },
        );
        let safe_seg = folder_name(&val, "General");
        if !safe_seg.is_empty() {
            dir.push(safe_seg);
        }
    }

    let new_filename = if let Some(ref rt) = rule.rename_template
        && !rt.trim().is_empty()
    {
        let p = Path::new(filename);
        let stem = p
            .file_stem()
            .and_then(|s| s.to_str())
            .unwrap_or(filename);
        let ext_clean = p.extension().and_then(|s| s.to_str()).unwrap_or(ext);
        let mut rendered = rt.trim().to_string();
        rendered = rendered.replace("{date}", &anchors.date);
        rendered = rendered.replace("{year}", &anchors.year);
        rendered = rendered.replace("{quarter}", &anchors.quarter);
        rendered = rendered.replace("{org}", &folder_name(&anchors.org, "General"));
        rendered = rendered.replace("{doc_type}", &folder_name(&anchors.doc_type, "Doc"));
        rendered = rendered.replace("{stem}", stem);
        rendered = rendered.replace("{ext}", ext_clean);
        let clean = sanitize(&rendered);
        if clean.is_empty() {
            filename.to_string()
        } else {
            clean
        }
    } else {
        filename.to_string()
    };

    let full_to = dir.join(&new_filename);
    (
        dir,
        new_filename,
        full_to.to_string_lossy().to_string(),
    )
}

fn load_folderless_open_paths(conn: &Connection) -> Result<HashSet<String>> {
    let mut out = open_batch_paths(conn)?;
    let mut st = conn.prepare(
        "SELECT a.from_path
         FROM actions a
         JOIN action_batches b ON b.id = a.batch_id
         WHERE b.status IN ('draft', 'simulated', 'approved', 'executing')
           AND a.status IN ('proposed', 'approved')",
    )?;
    for row in st.query_map([], |r| r.get::<_, String>(0))? {
        let p = row?;
        out.insert(path_key(Path::new(&p)));
    }
    Ok(out)
}

/// Evaluates a Folderless rule (saved or unsaved draft) read-only across catalogued files (`K1`, `K2`, `K4`, `R6`, `R7`).
pub fn preview_folderless_rule(
    conn: &Connection,
    rule: &FolderlessRuleInput,
    include_sensitive: bool,
    reveal_sensitive: bool,
    limit: usize,
) -> Result<FolderlessPreviewReport> {
    let started = Instant::now();
    ensure_sensitive_allowed(include_sensitive, reveal_sensitive)?;
    validate_dest_root(&rule.dest_root)?;
    ensure_not_backup_destination(conn, &rule.dest_root)?;
    let allow_sensitive = include_sensitive && reveal_sensitive;
    let now = now_ms();
    let limit = limit.clamp(1, 500);

    let open_paths = load_folderless_open_paths(conn)?;
    let (kg_orgs, kg_projs) = load_kg_org_and_project_maps(conn)?;

    let mut stmt = conn.prepare(
        "SELECT f.id, v.mount_point, f.path, f.name, COALESCE(f.ext, ''),
                f.size, f.mtime, f.cloud_state,
                COALESCE(NULLIF(v.label, ''), v.mount_point),
                CASE WHEN v.status = 'online' THEN 1 ELSE 0 END,
                f.sensitivity, f.atomic_unit_id,
                m.kind, m.category, m.doc_type, m.who, m.doc_date, m.exif_taken_at
         FROM files f
         JOIN volumes v ON v.id = f.volume_id
         LEFT JOIN file_meta m ON m.file_id = f.id
         WHERE f.deleted_at IS NULL
           AND f.state NOT IN ('quarantine', 'missing')
           AND f.container_file_id IS NULL
           AND COALESCE(v.role, '') != 'backup'
         ORDER BY f.mtime DESC, f.name ASC",
    )?;
    let rows = stmt.query_map([], |r| {
        Ok(FileCandidateRow {
            file_id: r.get(0)?,
            mount_point: r.get(1)?,
            path: r.get(2)?,
            name: r.get(3)?,
            ext: r.get(4)?,
            size_bytes: r.get(5)?,
            mtime: r.get(6)?,
            cloud_state: r.get(7)?,
            drive_label: r.get(8)?,
            drive_online: r.get::<_, i64>(9)? != 0,
            sensitivity: r.get(10)?,
            atomic_unit_id: r.get(11)?,
            meta_kind: r.get(12)?,
            meta_category: r.get(13)?,
            meta_doc_type: r.get(14)?,
            meta_who: r.get(15)?,
            meta_doc_date: r.get(16)?,
            meta_exif_taken_at: r.get(17)?,
        })
    })?;

    let mut skipped_atomic_unit_count = 0usize;
    let mut excluded_sensitive_count = 0usize;
    let mut matches = Vec::new();

    for row_res in rows {
        let f = row_res?;
        if is_excluded_relative_path(&f.path, &f.mount_point) {
            continue;
        }
        if f.atomic_unit_id.is_some() {
            skipped_atomic_unit_count += 1;
            continue;
        }
        if let Some(ref scope) = rule.scope_root
            && !scope.trim().is_empty()
            && !path_is_within(&f.path, scope.trim(), false)
        {
            continue;
        }
        if let Some(min_days) = rule.min_age_days {
            let age_days = ((now - f.mtime).max(0)) / DAY_MS;
            if age_days < min_days {
                continue;
            }
        }
        if is_sensitive(&f.sensitivity) && !allow_sensitive {
            excluded_sensitive_count += 1;
            continue;
        }

        let effective_kind = f
            .meta_kind
            .clone()
            .unwrap_or_else(|| kind_of(&f.ext, &f.name, false).as_str().to_string());
        if !rule.match_kinds.is_empty()
            && !rule
                .match_kinds
                .iter()
                .any(|k| k.eq_ignore_ascii_case(&effective_kind))
        {
            continue;
        }
        if !rule.match_exts.is_empty()
            && !rule
                .match_exts
                .iter()
                .any(|e| e.eq_ignore_ascii_case(&f.ext))
        {
            continue;
        }

        let body = load_file_text_for_rule(conn, &f)?;
        let combined_text = if body.is_empty() {
            f.name.clone()
        } else {
            format!("{}\n{}", f.name, body)
        };
        let combined_low = combined_text.to_ascii_lowercase();

        if !rule.match_categories.is_empty() {
            let cat_ok = f.meta_category.as_deref().is_some_and(|mc| {
                rule.match_categories
                    .iter()
                    .any(|rc| mc.eq_ignore_ascii_case(rc) || has_word(&combined_low, &rc.to_ascii_lowercase()))
            }) || rule
                .match_categories
                .iter()
                .any(|rc| has_word(&combined_low, &rc.to_ascii_lowercase()));
            if !cat_ok {
                continue;
            }
        }

        let mut reasons = Vec::new();
        if !rule.match_doc_types.is_empty() {
            let dt_ok = rule.match_doc_types.iter().any(|dt| {
                f.meta_doc_type
                    .as_deref()
                    .is_some_and(|mdt| mdt.eq_ignore_ascii_case(dt))
                    || has_word(&combined_low, &dt.to_ascii_lowercase())
                    || has_word(&combined_low, &format!("{}s", dt.to_ascii_lowercase()))
            });
            if !dt_ok {
                continue;
            }
            reasons.push(format!("Doc type: {}", rule.match_doc_types.join(", ")));
        }

        if !rule.match_keywords.is_empty() {
            let kw_ok = rule
                .match_keywords
                .iter()
                .any(|kw| combined_low.contains(&kw.to_ascii_lowercase()));
            if !kw_ok {
                continue;
            }
            reasons.push(format!("Keyword: {}", rule.match_keywords.join(", ")));
        }

        let anchors = build_anchors(
            &f,
            &combined_text,
            &combined_low,
            kg_orgs.get(&f.file_id).map(String::as_str),
            kg_projs.get(&f.file_id).map(String::as_str),
        );

        if !rule.match_orgs.is_empty() {
            let org_ok = rule.match_orgs.iter().any(|mo| {
                anchors.org.eq_ignore_ascii_case(mo)
                    || has_word(&combined_low, &mo.to_ascii_lowercase())
            });
            if !org_ok {
                continue;
            }
            reasons.push(format!("Org: {}", anchors.org));
        }

        let (dest_dir, new_filename, to_path) =
            expand_destination(rule, &f.name, &f.ext, &anchors);
        let dest_dir_str = dest_dir.to_string_lossy().to_string();
        if same_parent_dir(&f.path, &dest_dir_str) && new_filename == f.name {
            continue;
        }

        if !rule.match_kinds.is_empty() {
            reasons.push(format!("Kind: {effective_kind}"));
        }
        if !rule.match_exts.is_empty() {
            reasons.push(format!("Ext: .{}", f.ext));
        }
        if let Some(age) = rule.min_age_days {
            reasons.push(format!("Age >= {age}d"));
        }
        if reasons.is_empty() {
            reasons.push(format!("Matches rule `{}`", rule.name));
        }

        let mut extracted_anchors = vec![
            format!("Date: {}", anchors.date),
            format!("Period: {} {}", anchors.year, anchors.quarter),
            format!("Org: {}", anchors.org),
        ];
        if let Some(ref amt) = anchors.amount {
            extracted_anchors.push(format!("Amount: {amt}"));
        }

        let confidence = if anchors.amount.is_some() && anchors.org != "General" {
            0.96
        } else if anchors.org != "General" || !body.is_empty() {
            0.91
        } else {
            0.85
        };

        let already_in_open_batch = open_paths.contains(&path_key(Path::new(&f.path)));
        matches.push(FolderlessMatchPreview {
            file_id: f.file_id,
            filename: f.name,
            from_path: f.path,
            to_path,
            new_filename,
            size_bytes: f.size_bytes,
            mtime: f.mtime,
            drive_label: f.drive_label,
            matched_reasons: reasons,
            extracted_anchors,
            confidence,
            already_in_open_batch,
        });
    }

    matches.sort_by(|a, b| {
        b.confidence
            .total_cmp(&a.confidence)
            .then_with(|| b.mtime.cmp(&a.mtime))
            .then_with(|| a.file_id.cmp(&b.file_id))
    });

    let total_matches = matches.len();
    let total_bytes: i64 = matches.iter().map(|m| m.size_bytes).sum();
    matches.truncate(limit);

    Ok(FolderlessPreviewReport {
        rule_name: rule.name.clone(),
        dest_root: rule.dest_root.clone(),
        path_template: rule.path_template.clone(),
        matches,
        total_matches,
        total_bytes,
        skipped_atomic_unit_count,
        excluded_sensitive_count,
        took_ms: started.elapsed().as_millis() as u64,
    })
}

/// Stages a reversible `ActionType::Move` batch for selected matches of `rule_id` (`K1`, `K2`, `K4`, `R6`, `R7`).
pub fn propose_folderless_batch(
    store: &mut SqliteStore,
    rule_id: &str,
    file_ids: &[String],
    include_sensitive: bool,
    reveal_sensitive: bool,
) -> Result<String> {
    if file_ids.is_empty() {
        return Err(AegisError::InvalidInput(
            "Select at least one matched file to stage a reversible batch.".into(),
        ));
    }
    ensure_sensitive_allowed(include_sensitive, reveal_sensitive)?;
    let rule = get_folderless_rule(store.conn(), rule_id)?;
    let rule_input = rule_to_input(&rule);
    validate_dest_root(&rule.dest_root)?;
    ensure_not_backup_destination(store.conn(), &rule.dest_root)?;

    let preview = preview_folderless_rule(
        store.conn(),
        &rule_input,
        include_sensitive,
        reveal_sensitive,
        1_000,
    )?;
    let by_id: HashMap<String, FolderlessMatchPreview> = preview
        .matches
        .into_iter()
        .map(|m| (m.file_id.clone(), m))
        .collect();

    let policy_ctx = build_policy_context(store)?;
    let busy_paths = load_folderless_open_paths(store.conn())?;
    let mut reserved_dests = HashSet::new();
    let mut seen_ids = HashSet::new();
    let mut actions = Vec::with_capacity(file_ids.len());

    for fid in file_ids {
        if !seen_ids.insert(fid.clone()) {
            continue;
        }
        struct StageFileCheck {
            path: String,
            state: String,
            deleted_at: Option<i64>,
            sensitivity: String,
            vol_status: String,
            vol_role: Option<String>,
        }
        // Explicitly reject locked sensitive files or quarantined/backup targets even if passed manually.
        let file_check: Option<StageFileCheck> = store
            .conn()
            .query_row(
                "SELECT f.path, f.state, f.deleted_at, f.sensitivity, COALESCE(v.status, 'offline'), v.role
                 FROM files f
                 JOIN volumes v ON v.id = f.volume_id
                 WHERE f.id = ?1",
                params![fid],
                |r| {
                    Ok(StageFileCheck {
                        path: r.get(0)?,
                        state: r.get(1)?,
                        deleted_at: r.get(2)?,
                        sensitivity: r.get(3)?,
                        vol_status: r.get(4)?,
                        vol_role: r.get(5)?,
                    })
                },
            )
            .optional()?;
        let Some(fc) = file_check else {
            return Err(AegisError::NotFound(format!("file {fid}")));
        };
        if fc.deleted_at.is_some() || fc.state == "quarantine" || fc.state == "missing" {
            return Err(AegisError::InvalidInput(
                "Cannot move deleted or quarantined files (K2).".into(),
            ));
        }
        if fc.vol_role.as_deref() == Some("backup") {
            return Err(AegisError::PermissionDenied(
                "Backup volumes are read-only and never modified (K4).".into(),
            ));
        }
        if fc.vol_status != "online" {
            return Err(AegisError::InvalidInput(
                "Cannot stage move for a file on an offline volume.".into(),
            ));
        }
        if is_sensitive(&fc.sensitivity) && !(include_sensitive && reveal_sensitive) {
            return Err(AegisError::PermissionDenied(
                "Unlock sensitive previews with Windows authentication before staging confidential/secret files (R7)."
                    .into(),
            ));
        }

        let fpath = fc.path;

        let Some(m) = by_id.get(fid) else {
            return Err(AegisError::InvalidInput(format!(
                "File `{fpath}` does not match rule `{}`.",
                rule.name
            )));
        };

        let from_p = Path::new(&m.from_path);
        if !from_p.is_file() {
            return Err(AegisError::InvalidInput(format!(
                "Source file missing on disk: {}",
                m.from_path
            )));
        }
        if busy_paths.contains(&path_key(from_p)) {
            return Err(AegisError::InvalidInput(format!(
                "`{}` already has an open Simulator action.",
                m.filename
            )));
        }

        let target_p = Path::new(&m.to_path);
        let dest_parent = target_p.parent().unwrap_or_else(|| Path::new(&rule.dest_root));
        let final_dest = resolve_unique_dest(dest_parent, &m.new_filename, &mut reserved_dests);
        let final_dest_str = final_dest.to_string_lossy().to_string();

        let pa = PolicyAction {
            action_type: ActionType::Move,
            target_kind: TargetKind::File,
            agent: Some(FOLDERLESS_AGENT),
            from_path: from_p,
            to_path: Some(&final_dest),
            confidence: m.confidence,
            atomic_unit_root: None,
            unit_allow_split_override: false,
            file_unit_split_override: false,
            is_registered_cache_dir: false,
        };
        if let PolicyVerdict::Block(reason) = evaluate_policy(&pa, &policy_ctx) {
            return Err(AegisError::PermissionDenied(format!(
                "Policy blocked Folderless move for `{}`: {reason}",
                m.filename
            )));
        }

        actions.push(ProposedActionInput {
            agent: Some(FOLDERLESS_AGENT.to_string()),
            action_type: ActionType::Move,
            target_kind: TargetKind::File,
            file_id: Some(fid.clone()),
            atomic_unit_id: None,
            from_path: m.from_path.clone(),
            to_path: Some(final_dest_str),
            reason: format!(
                "Folderless rule `{}`: {}",
                rule.name,
                m.matched_reasons.join(", ")
            ),
            confidence: m.confidence,
            evidence_json: serde_json::json!({
                "source": "folderless_mode",
                "rule_id": rule.id,
                "anchors": m.extracted_anchors,
            })
            .to_string(),
        });
    }

    if actions.is_empty() {
        return Err(AegisError::InvalidInput(
            "No eligible matched files remained to stage.".into(),
        ));
    }

    let origin = format!("folderless:{}", rule.id);
    propose_batch(store, &RefilePlan { origin, actions })
}

/// Lists recent Folderless Mode batches for transactional revert / audit UI (`K1`).
pub fn list_folderless_batches(
    conn: &Connection,
    limit: usize,
) -> Result<Vec<FolderlessBatchSummary>> {
    let mut stmt = conn.prepare(
        "SELECT b.id, b.origin, b.status, b.total_actions, b.created_at, b.executed_at,
                SUM(CASE WHEN a.status = 'executed' THEN 1 ELSE 0 END),
                SUM(CASE WHEN a.status = 'rolled_back' THEN 1 ELSE 0 END)
         FROM action_batches b
         LEFT JOIN actions a ON a.batch_id = b.id
         WHERE b.origin LIKE 'folderless:%'
         GROUP BY b.id
         ORDER BY b.created_at DESC
         LIMIT ?1",
    )?;
    let rows = stmt.query_map(params![limit.clamp(1, 100) as i64], |r| {
        let origin: String = r.get(1)?;
        let rule_id = origin
            .strip_prefix("folderless:")
            .unwrap_or("")
            .to_string();
        let executed_count: i64 = r.get(6)?;
        let rolled_back_count: i64 = r.get(7)?;
        let status: String = r.get(2)?;
        Ok(FolderlessBatchSummary {
            batch_id: r.get(0)?,
            origin,
            rule_id,
            status: status.clone(),
            total_actions: r.get(3)?,
            executed_count,
            rolled_back_count,
            can_undo: executed_count > 0 && status != "rolled_back",
            created_at: r.get(4)?,
            executed_at: r.get(5)?,
        })
    })?;
    rows.collect::<rusqlite::Result<_>>().map_err(Into::into)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn extract_date_and_amount_anchors_pure_rust() {
        assert_eq!(
            extract_date_from_text("Invoice dated 2026-08-14 due on receipt"),
            Some("2026-08-14".to_string())
        );
        assert_eq!(quarter_for_month("08"), "Q3");
        assert_eq!(
            extract_amount_anchor("Total due: R 4,250.00 incl VAT"),
            Some("R 4,250.00".to_string())
        );
        assert_eq!(
            extract_amount_anchor("Charged $1,200.50 USD"),
            Some("$1,200.50".to_string())
        );
    }
}
