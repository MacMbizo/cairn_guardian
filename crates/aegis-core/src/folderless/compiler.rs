//! Plain-language intent compiler for AI Folderless Mode (`2.1`, reshaped `R6`).
//!
//! Compiles English organizational instructions into visible, editable `FolderlessRuleInput`
//! structs deterministically in pure Rust, with optional on-demand local LLM refinement (`R4`, `K3`).

use std::path::{Path, PathBuf};

use serde::Deserialize;

use crate::error::{AegisError, Result};
use crate::filing::naming::folder_name;
use crate::filing::orgs::{ORGS, has_word};

use super::{CompiledIntentDraft, FolderlessRuleInput};

fn title_case_phrase(s: &str) -> String {
    s.split_whitespace()
        .map(|w| {
            let mut chars = w.chars();
            match chars.next() {
                Some(first) => first.to_uppercase().collect::<String>() + &chars.as_str().to_lowercase(),
                None => String::new(),
            }
        })
        .collect::<Vec<_>>()
        .join(" ")
}

fn find_preferred_docs_base(available_roots: &[String]) -> String {
    for r in available_roots {
        let name = Path::new(r)
            .file_name()
            .and_then(|s| s.to_str())
            .unwrap_or("")
            .to_ascii_lowercase();
        if name == "documents" || name == "my documents" {
            return r.clone();
        }
    }
    available_roots
        .first()
        .cloned()
        .unwrap_or_else(|| "Documents".to_string())
}

fn find_root_by_keyword(available_roots: &[String], folder_kw: &str) -> Option<String> {
    let target = folder_kw.to_ascii_lowercase();
    for r in available_roots {
        let base = Path::new(r)
            .file_name()
            .and_then(|s| s.to_str())
            .unwrap_or("")
            .to_ascii_lowercase();
        if base == target || base == format!("my {target}") {
            return Some(r.clone());
        }
    }
    None
}

fn extract_quoted_phrases(text: &str) -> Vec<String> {
    let mut out = Vec::new();
    let mut chars = text.char_indices();
    while let Some((start, ch)) = chars.next() {
        if ch == '"' || ch == '\'' {
            let quote = ch;
            for (end, c2) in chars.by_ref() {
                if c2 == quote {
                    let inner = text[start + ch.len_utf8()..end].trim();
                    if inner.len() >= 3 && !out.contains(&inner.to_ascii_lowercase()) {
                        out.push(inner.to_ascii_lowercase());
                    }
                    break;
                }
            }
        }
    }
    out
}

fn extract_min_age_days(low: &str) -> Option<i64> {
    let words: Vec<&str> = low.split_whitespace().collect();
    for win in words.windows(3) {
        if let Ok(n) = win[0].trim_matches([',', '.']).parse::<i64>()
            && n > 0
        {
            let unit = win[1].trim_matches([',', '.']);
            let mult = if unit.starts_with("day") {
                Some(1)
            } else if unit.starts_with("week") {
                Some(7)
            } else if unit.starts_with("month") {
                Some(30)
            } else if unit.starts_with("year") {
                Some(365)
            } else {
                None
            };
            if let Some(m) = mult {
                return Some(n * m);
            }
        }
    }
    for i in 0..words.len().saturating_sub(2) {
        let w0 = words[i];
        let w1 = words[i + 1];
        if ((w0 == "older" && w1 == "than") || (w0 == "aged" && (w1 == "over" || w1 == "above")))
            && i + 3 < words.len()
            && let Ok(n) = words[i + 2].trim_matches([',', '.']).parse::<i64>()
        {
            let unit = words[i + 3].trim_matches([',', '.']);
            if unit.starts_with("day") {
                return Some(n);
            } else if unit.starts_with("week") {
                return Some(n * 7);
            } else if unit.starts_with("month") {
                return Some(n * 30);
            } else if unit.starts_with("year") {
                return Some(n * 365);
            }
        }
    }
    None
}

fn extract_explicit_abs_path(text: &str) -> Option<String> {
    for tok in text.split_whitespace() {
        let cleaned = tok.trim_matches([',', ';', '.', '"', '\'']);
        let bytes = cleaned.as_bytes();
        if bytes.len() >= 3
            && bytes[0].is_ascii_alphabetic()
            && bytes[1] == b':'
            && (bytes[2] == b'\\' || bytes[2] == b'/')
        {
            return Some(cleaned.to_string());
        }
    }
    None
}

fn extract_scope_root(intent: &str, low: &str, available_roots: &[String]) -> Option<String> {
    for kw in ["downloads", "desktop", "documents", "pictures", "videos", "music"] {
        let pat_in = format!("in {kw}");
        let pat_from = format!("from {kw}");
        let pat_in_my = format!("in my {kw}");
        let pat_from_my = format!("from my {kw}");
        if has_word(low, &pat_in)
            || has_word(low, &pat_from)
            || has_word(low, &pat_in_my)
            || has_word(low, &pat_from_my)
            || has_word(low, &format!("inside {kw}"))
        {
            return find_root_by_keyword(available_roots, kw).or_else(|| Some(title_case_phrase(kw)));
        }
    }
    if let Some(idx) = low.find("from ") {
        let after = &intent[idx + 5..];
        if let Some(p) = extract_explicit_abs_path(after) {
            return Some(p);
        }
    }
    None
}

fn extract_destination(
    intent: &str,
    low: &str,
    available_roots: &[String],
    doc_types: &[String],
    kinds: &[String],
) -> (String, Option<String>) {
    // Check explicit destination path anywhere after `to ` / `into `.
    let markers = [" into ", " to "];
    let mut dest_slice: Option<&str> = None;
    for m in markers {
        if let Some(pos) = low.find(m) {
            dest_slice = Some(&intent[pos + m.len()..]);
            break;
        }
    }

    if let Some(raw_after) = dest_slice {
        let stop_phrases = [
            ", organized by",
            ", grouped by",
            ", sorted by",
            " organized by",
            " grouped by",
            " sorted by",
            ", renamed",
            " renamed",
            ", rename",
            " rename",
            " by ",
        ];
        let low_after = raw_after.to_ascii_lowercase();
        let mut end = raw_after.len();
        for sp in stop_phrases {
            if let Some(idx) = low_after.find(sp) {
                end = end.min(idx);
            }
        }
        let candidate = raw_after[..end]
            .trim()
            .trim_end_matches([',', '.', ';', ':'])
            .trim();

        if let Some(abs) = extract_explicit_abs_path(candidate) {
            return (abs, None);
        }

        let stripped = candidate
            .strip_prefix("my ")
            .or_else(|| candidate.strip_prefix("My "))
            .or_else(|| candidate.strip_prefix("the "))
            .or_else(|| candidate.strip_prefix("The "))
            .unwrap_or(candidate)
            .trim();

        let low_stripped = stripped.to_ascii_lowercase();
        if ["downloads", "desktop", "documents", "pictures", "videos", "music"]
            .contains(&low_stripped.as_str())
            && let Some(found) = find_root_by_keyword(available_roots, &low_stripped)
        {
            return (found, None);
        }

        if !stripped.is_empty() {
            let base = find_preferred_docs_base(available_roots);
            let mut pb = PathBuf::from(&base);
            for part in stripped.split(['/', '\\']) {
                let seg = folder_name(&title_case_phrase(part), "Organized");
                if !seg.is_empty() {
                    pb.push(seg);
                }
            }
            let warn = if available_roots.is_empty() {
                Some("No active scan root configured yet — review Destination Root before saving.".to_string())
            } else {
                None
            };
            return (pb.to_string_lossy().to_string(), warn);
        }
    }

    let base = find_preferred_docs_base(available_roots);
    let sub = if let Some(dt) = doc_types.first() {
        format!("{} Archives", title_case_phrase(dt))
    } else if let Some(k) = kinds.first() {
        format!("{}s", title_case_phrase(k))
    } else {
        "Sorted Files".to_string()
    };
    let dest = Path::new(&base).join(folder_name(&sub, "Sorted Files"));
    (dest.to_string_lossy().to_string(), None)
}

fn extract_path_template(low: &str) -> String {
    let clause = if let Some(i) = low.find("organized by") {
        &low[i..]
    } else if let Some(i) = low.find("grouped by") {
        &low[i..]
    } else if let Some(i) = low.find("sorted by") {
        &low[i..]
    } else if let Some(i) = low.find(" by ") {
        &low[i..]
    } else {
        ""
    };

    if clause.is_empty() {
        return "{year}".to_string();
    }

    let mut ordered_hits: Vec<(usize, Vec<&'static str>)> = Vec::new();
    let specs: &[(&[&str], &[&str])] = &[
        (&["fiscal quarter", "quarter"], &["{year}", "{quarter}"]),
        (&["year"], &["{year}"]),
        (&["month"], &["{month}"]),
        (
            &[
                "company name",
                "company",
                "vendor",
                "organization",
                "organisation",
                "client",
                "sender",
            ],
            &["{org}"],
        ),
        (&["project"], &["{project}"]),
        (&["document type", "doc type", "category", "type"], &["{doc_type}"]),
        (&["extension", "format"], &["{ext}"]),
    ];

    for (patterns, tokens) in specs {
        let mut best_pos: Option<usize> = None;
        for pat in *patterns {
            if let Some(pos) = clause.find(pat) {
                best_pos = Some(best_pos.map_or(pos, |b| b.min(pos)));
            }
        }
        if let Some(pos) = best_pos {
            ordered_hits.push((pos, tokens.to_vec()));
        }
    }

    ordered_hits.sort_by_key(|(pos, _)| *pos);
    let mut final_tokens: Vec<&'static str> = Vec::new();
    for (_, toks) in ordered_hits {
        for tok in toks {
            if !final_tokens.contains(&tok) {
                final_tokens.push(tok);
            }
        }
    }

    if final_tokens.is_empty() {
        "{year}".to_string()
    } else {
        final_tokens.join("\\")
    }
}

/// Compiles a plain-language organizational intent deterministically into a visible rule draft (`R6`).
pub fn compile_intent_deterministic(
    intent_text: &str,
    available_roots: &[String],
) -> Result<CompiledIntentDraft> {
    let trimmed = intent_text.trim();
    if trimmed.is_empty() {
        return Err(AegisError::InvalidInput(
            "Enter a plain-language organization rule first.".into(),
        ));
    }
    let low = trimmed.to_ascii_lowercase();

    let scope_root = extract_scope_root(trimmed, &low, available_roots);

    let mut match_exts = Vec::new();
    for ext in [
        "pdf", "docx", "doc", "xlsx", "csv", "png", "jpg", "jpeg", "raw", "psd", "mp4", "mov",
        "zip", "md", "txt", "vtt", "srt",
    ] {
        if has_word(&low, ext) || has_word(&low, &format!("{ext}s")) || low.contains(&format!(".{ext}"))
        {
            match_exts.push(ext.to_string());
        }
    }

    let mut match_kinds = Vec::new();
    for (kw, kind) in [
        ("screenshot", "screenshot"),
        ("screenshots", "screenshot"),
        ("photo", "photo"),
        ("photos", "photo"),
        ("picture", "photo"),
        ("pictures", "photo"),
        ("image", "photo"),
        ("images", "photo"),
        ("video", "video"),
        ("videos", "video"),
        ("recording", "video"),
        ("audio", "audio"),
        ("installer", "installer"),
        ("installers", "installer"),
        ("archive", "archive"),
        ("archives", "archive"),
        ("document", "document"),
        ("documents", "document"),
    ] {
        // Avoid classifying destination phrase `"to accounting archives"` as archive zip matcher!
        if has_word(&low, kw)
            && !(kw.starts_with("archive") && (low.contains("to ") && low.find(kw) > low.find("to ")))
            && !match_kinds.contains(&kind.to_string())
        {
            match_kinds.push(kind.to_string());
        }
    }

    let mut match_doc_types = Vec::new();
    for (pat, dt) in [
        ("invoice", "invoice"),
        ("invoices", "invoice"),
        ("receipt", "receipt"),
        ("receipts", "receipt"),
        ("tax", "tax"),
        ("taxes", "tax"),
        ("statement", "statement"),
        ("statements", "statement"),
        ("contract", "contract"),
        ("contracts", "contract"),
        ("agreement", "contract"),
        ("payslip", "payslip"),
        ("payslips", "payslip"),
        ("transcript", "transcript"),
        ("transcripts", "transcript"),
        ("report", "report"),
        ("reports", "report"),
    ] {
        if has_word(&low, pat) && !match_doc_types.contains(&dt.to_string()) {
            match_doc_types.push(dt.to_string());
        }
    }

    let mut match_categories = Vec::new();
    if match_doc_types.is_empty() {
        for (kw, cat) in [
            ("finance", "Finance"),
            ("financial", "Finance"),
            ("accounting", "Finance"),
            ("legal", "Legal"),
            ("medical", "Medical"),
            ("research", "Research"),
        ] {
            // Only use as category filter when appearing before `" to "`/`" into "`
            let before_dest = low
                .find(" into ")
                .or_else(|| low.find(" to "))
                .map_or(low.as_str(), |i| &low[..i]);
            if has_word(before_dest, kw) && !match_categories.contains(&cat.to_string()) {
                match_categories.push(cat.to_string());
            }
        }
    }

    let mut match_orgs = Vec::new();
    for (canon, aliases) in ORGS {
        if aliases.iter().any(|a| has_word(&low, a))
            && !match_orgs.contains(&canon.to_string())
        {
            match_orgs.push((*canon).to_string());
        }
    }

    let mut match_keywords = extract_quoted_phrases(trimmed);
    for topic_kw in [
        "warranty",
        "insurance",
        "medical",
        "travel",
        "itinerary",
        "audit",
        "benchmark",
        "proposal",
    ] {
        if has_word(&low, topic_kw) && !match_keywords.contains(&topic_kw.to_string()) {
            match_keywords.push(topic_kw.to_string());
        }
    }

    let min_age_days = extract_min_age_days(&low);
    let (dest_root, dest_warn) =
        extract_destination(trimmed, &low, available_roots, &match_doc_types, &match_kinds);
    let path_template = extract_path_template(&low);

    let rename_template = if has_word(&low, "rename")
        || low.contains("prefix with date")
        || low.contains("with date")
    {
        if path_template.contains("{org}") {
            Some("{date}_{org}_{stem}.{ext}".to_string())
        } else {
            Some("{date}_{stem}.{ext}".to_string())
        }
    } else {
        None
    };

    let name_subject = if let Some(dt) = match_doc_types.first() {
        format!("{}s", title_case_phrase(dt))
    } else if let Some(k) = match_kinds.first() {
        format!("{}s", title_case_phrase(k))
    } else if let Some(ext) = match_exts.first() {
        format!(".{} Files", ext.to_ascii_uppercase())
    } else {
        "Custom Intent Rule".to_string()
    };
    let dest_leaf = Path::new(&dest_root)
        .file_name()
        .and_then(|s| s.to_str())
        .unwrap_or("Archive");
    let rule_name = format!("{name_subject} → {dest_leaf}");

    let mut desc_parts = Vec::new();
    if let Some(ref sr) = scope_root {
        desc_parts.push(format!("inside {sr}"));
    }
    if !match_doc_types.is_empty() {
        desc_parts.push(format!("matching types [{}]", match_doc_types.join(", ")));
    }
    if !match_kinds.is_empty() {
        desc_parts.push(format!("kinds [{}]", match_kinds.join(", ")));
    }
    if !match_exts.is_empty() {
        desc_parts.push(format!("extensions [{}]", match_exts.join(", ")));
    }
    if !match_orgs.is_empty() {
        desc_parts.push(format!("orgs [{}]", match_orgs.join(", ")));
    }
    if !match_keywords.is_empty() {
        desc_parts.push(format!("keywords [{}]", match_keywords.join(", ")));
    }
    if let Some(age) = min_age_days {
        desc_parts.push(format!("untouched for >= {age} days"));
    }
    let what = if desc_parts.is_empty() {
        "indexed files".to_string()
    } else {
        desc_parts.join(", ")
    };

    let mut warnings = Vec::new();
    if let Some(w) = dest_warn {
        warnings.push(w);
    }
    if desc_parts.is_empty() {
        warnings.push(
            "Broad rule with no type/keyword filter — review matched files carefully in Dry-Run Preview."
                .to_string(),
        );
    }

    let explanation = format!(
        "Compiled into visible rule selecting {what} and proposing reversible moves under {dest_root}\\{path_template}."
    );

    Ok(CompiledIntentDraft {
        rule: FolderlessRuleInput {
            id: None,
            name: rule_name,
            intent_text: trimmed.to_string(),
            compiled_by: Some("deterministic".to_string()),
            enabled: Some(true),
            scope_root,
            match_kinds,
            match_exts,
            match_categories,
            match_doc_types,
            match_keywords,
            match_orgs,
            min_age_days,
            dest_root,
            path_template,
            rename_template,
        },
        explanation,
        warnings,
        compiled_by: "deterministic".to_string(),
    })
}

/// Builds a compact structured prompt asking `OnDemandLlm` to refine an intent draft (`K5`, `R4`, `R6`).
pub fn build_intent_llm_prompt(
    intent_text: &str,
    deterministic: &CompiledIntentDraft,
) -> (String, String) {
    let sys = "You compile plain-language file organization rules into JSON for Cairn Guardian. \
               Respond with ONLY a single JSON object with optional keys: \
               name, match_doc_types (array), match_keywords (array), match_orgs (array), \
               path_template (using only {year}, {quarter}, {month}, {org}, {doc_type}, {project}, {ext}), \
               rename_template (or null). Preserve user safety; never emit '..'."
        .to_string();
    let baseline = serde_json::json!({
        "name": deterministic.rule.name,
        "match_doc_types": deterministic.rule.match_doc_types,
        "match_keywords": deterministic.rule.match_keywords,
        "match_orgs": deterministic.rule.match_orgs,
        "path_template": deterministic.rule.path_template,
        "rename_template": deterministic.rule.rename_template,
    });
    let user = format!(
        "User intent: \"{intent_text}\"\nDeterministic draft JSON: {baseline}\nRefine or confirm fields as JSON:"
    );
    (sys, user)
}

#[derive(Deserialize)]
struct LlmRulePatch {
    name: Option<String>,
    match_doc_types: Option<Vec<String>>,
    match_keywords: Option<Vec<String>>,
    match_orgs: Option<Vec<String>>,
    path_template: Option<String>,
    rename_template: Option<String>,
}

/// Safely merges structured Local LLM JSON refinements into a deterministic draft (`R6`).
pub fn apply_llm_refinement(
    mut base: CompiledIntentDraft,
    llm_output: &str,
) -> CompiledIntentDraft {
    let Some(start) = llm_output.find('{') else {
        return base;
    };
    let Some(end) = llm_output.rfind('}') else {
        return base;
    };
    if end <= start {
        return base;
    }
    let Ok(patch) = serde_json::from_str::<LlmRulePatch>(&llm_output[start..=end]) else {
        return base;
    };

    if let Some(n) = patch.name.map(|s| s.trim().to_string()).filter(|s| !s.is_empty()) {
        base.rule.name = n;
    }
    if let Some(dts) = patch.match_doc_types {
        let cleaned: Vec<String> = dts
            .into_iter()
            .map(|s| s.trim().to_ascii_lowercase())
            .filter(|s| !s.is_empty())
            .collect();
        if !cleaned.is_empty() {
            base.rule.match_doc_types = cleaned;
        }
    }
    if let Some(kws) = patch.match_keywords {
        let cleaned: Vec<String> = kws
            .into_iter()
            .map(|s| s.trim().to_ascii_lowercase())
            .filter(|s| s.len() >= 3)
            .collect();
        if !cleaned.is_empty() {
            base.rule.match_keywords = cleaned;
        }
    }
    if let Some(orgs) = patch.match_orgs {
        let cleaned: Vec<String> = orgs
            .into_iter()
            .map(|s| s.trim().to_string())
            .filter(|s| !s.is_empty())
            .collect();
        if !cleaned.is_empty() {
            base.rule.match_orgs = cleaned;
        }
    }
    if let Some(pt) = patch.path_template {
        let t = pt.trim();
        if !t.is_empty() && !t.contains("..") && t.contains('{') {
            base.rule.path_template = t.replace('/', "\\");
        }
    }
    if let Some(rt) = patch.rename_template {
        let t = rt.trim();
        if !t.is_empty() && !t.contains("..") && !t.contains(['\\', '/']) {
            base.rule.rename_template = Some(t.to_string());
        }
    }

    base.compiled_by = "local_llm".to_string();
    base.rule.compiled_by = Some("local_llm".to_string());
    base
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn compiles_invoice_quarter_company_and_screenshot_intents() {
        let roots = vec![
            r"C:\Users\Owner\Downloads".to_string(),
            r"C:\Users\Owner\Documents".to_string(),
        ];

        let inv = compile_intent_deterministic(
            "Route vendor invoices to Accounting Archives, organized by fiscal quarter and company name",
            &roots,
        )
        .unwrap();
        assert_eq!(inv.rule.match_doc_types, vec!["invoice"]);
        assert_eq!(inv.rule.path_template, r"{year}\{quarter}\{org}");
        assert!(inv.rule.dest_root.ends_with("Accounting Archives"));
        assert!(inv.rule.scope_root.is_none());

        let shots = compile_intent_deterministic(
            "Move screenshots in Downloads older than 30 days to C:\\Media\\Screenshots, organized by year and month",
            &roots,
        )
        .unwrap();
        assert_eq!(
            shots.rule.scope_root.as_deref(),
            Some(r"C:\Users\Owner\Downloads")
        );
        assert_eq!(shots.rule.match_kinds, vec!["screenshot"]);
        assert_eq!(shots.rule.min_age_days, Some(30));
        assert_eq!(shots.rule.dest_root, r"C:\Media\Screenshots");
        assert_eq!(shots.rule.path_template, r"{year}\{month}");
    }
}
