//! Pure-Rust deterministic entity/decision/action extraction, `.vtt`/`.srt` transcript
//! normalization, and optional on-demand local LLM prompt + output parsing (`K3`, `R9`).

use std::collections::HashMap;

use crate::filing::orgs::{ORGS, has_word};

use super::EntityType;

const MAX_CANDIDATES_PER_FILE: usize = 40;

const CORP_SUFFIXES: &[&str] = &[
    "(pty) ltd",
    "pty ltd",
    "ltd",
    "limited",
    "inc",
    "incorporated",
    "llc",
    "llp",
    "corp",
    "corporation",
    "holdings",
    "group",
    "foundation",
    "university",
    "institute",
    "laboratories",
    "labs",
    "studio",
    "partners",
    "technologies",
    "solutions",
    "consulting",
    "associates",
    "bank",
];

const NON_PERSON_SPEAKERS: &[&str] = &[
    "action",
    "agenda",
    "answer",
    "approved",
    "assignee",
    "attendees",
    "author",
    "background",
    "budget",
    "client",
    "company",
    "conclusion",
    "context",
    "date",
    "decision",
    "decided",
    "deadline",
    "description",
    "details",
    "draft",
    "due",
    "email",
    "error",
    "example",
    "executive",
    "follow",
    "followup",
    "goal",
    "guest",
    "host",
    "important",
    "initiative",
    "introduction",
    "location",
    "meeting",
    "minutes",
    "minutes of",
    "next",
    "note",
    "notes",
    "objective",
    "organization",
    "organisation",
    "overview",
    "owner",
    "participants",
    "partner",
    "phase",
    "present",
    "priority",
    "problem",
    "project",
    "purpose",
    "question",
    "reference",
    "requirements",
    "resolved",
    "result",
    "review",
    "risk",
    "section",
    "solution",
    "speaker",
    "status",
    "step",
    "subject",
    "summary",
    "task",
    "timeline",
    "time",
    "title",
    "todo",
    "topic",
    "transcript",
    "update",
    "vendor",
    "version",
    "warning",
    "webvtt",
    "workstream",
];

/// Extracted typed candidate prior to `K1` provisional persistence.
#[derive(Debug, Clone, PartialEq)]
pub struct ExtractedCandidate {
    pub entity_type: EntityType,
    pub name: String,
    pub summary: String,
    pub assignee: Option<String>,
    pub due_date: Option<String>,
    pub project_hint: Option<String>,
    pub evidence_quote: String,
    pub confidence: f64,
    pub source_kind: String,
}

/// Normalizes an entity name into a deterministic lowercase key for alias lookup and deduplication.
pub fn canonical_key(entity_type: EntityType, name: &str) -> String {
    let mut s = name.trim().to_ascii_lowercase();
    if matches!(entity_type, EntityType::Decision | EntityType::ActionItem) {
        for pfx in [
            "- [ ] ", "* [ ] ", "[ ] ", "- [x] ", "* [x] ", "[x] ", "action item:", "action:",
            "todo:", "follow-up:", "follow up:", "next step:", "task:", "decision:", "decided:",
            "agreed:", "resolved:", "approved:",
        ] {
            if let Some(rest) = s.strip_prefix(pfx) {
                s = rest.trim().to_string();
                break;
            }
        }
    }
    let cleaned: String = s
        .chars()
        .map(|c| {
            if c.is_alphanumeric() || matches!(c, ' ' | '-' | '/' | '.') {
                c
            } else {
                ' '
            }
        })
        .collect();
    let collapsed = cleaned.split_whitespace().collect::<Vec<_>>().join(" ");
    collapsed.chars().take(160).collect()
}

/// Strips corporate entity suffixes to compute a secondary lookup alias (e.g. `Acme Corp` -> `acme`).
pub fn org_stem_key(name: &str) -> Option<String> {
    let mut base = canonical_key(EntityType::Organization, name);
    for suffix in CORP_SUFFIXES {
        if base.ends_with(suffix) && base.len() > suffix.len() + 1 {
            let trimmed = base[..base.len() - suffix.len()].trim();
            if trimmed.chars().filter(|c| c.is_alphanumeric()).count() >= 3 {
                base = trimmed.to_string();
                break;
            }
        }
    }
    if base.is_empty() { None } else { Some(base) }
}

/// Normalizes `.vtt`/`.srt` cue files or raw meeting notes into readable speaker lines.
/// Returns `(normalized_text, looks_like_transcript_or_meeting)`.
pub fn normalize_transcript_text(ext: &str, raw: &str) -> (String, bool) {
    let ext_low = ext.trim().trim_start_matches('.').to_ascii_lowercase();
    let is_caption_file = matches!(ext_low.as_str(), "vtt" | "srt");
    let has_vtt_header = raw.trim_start().starts_with("WEBVTT");

    let mut out_lines = Vec::new();
    let mut transcript_markers = is_caption_file || has_vtt_header;

    for raw_line in raw.replace("\r\n", "\n").lines() {
        let line = raw_line.trim();
        if line.is_empty() {
            continue;
        }
        if has_vtt_header && (line == "WEBVTT" || line.starts_with("NOTE ") || line == "NOTE") {
            continue;
        }
        // Skip caption cue numbers (`1`, `2`, ...) and timestamp ranges (`00:01:02.000 --> 00:01:05.000`).
        if (is_caption_file || has_vtt_header)
            && (line.chars().all(|c| c.is_ascii_digit()) || line.contains("-->"))
        {
            transcript_markers = true;
            continue;
        }

        // Convert WebVTT `<v Alice Mokoena>text</v>` voice tags cleanly into `Alice Mokoena: text`.
        let mut cleaned = convert_vtt_voice_tags(line);
        // Strip leading `[00:12:34]` or `(00:12)` transcript timestamps.
        if let Some(rest) = strip_leading_timestamp(&cleaned) {
            transcript_markers = true;
            cleaned = rest.to_string();
        }
        let cleaned = cleaned.trim().to_string();
        if !cleaned.is_empty() {
            if contains_meeting_marker(&cleaned) {
                transcript_markers = true;
            }
            out_lines.push(cleaned);
        }
    }

    (out_lines.join("\n"), transcript_markers)
}

fn convert_vtt_voice_tags(line: &str) -> String {
    let trimmed = line.trim();
    if let Some(after_v) = trimmed.strip_prefix("<v ")
        && let Some(close_idx) = after_v.find('>')
    {
        let speaker = after_v[..close_idx].trim();
        let rest = after_v[close_idx + 1..]
            .replace("</v>", "")
            .trim()
            .to_string();
        if !speaker.is_empty() && !rest.is_empty() {
            return format!("{speaker}: {rest}");
        }
    }
    // Strip basic caption HTML tags (`<b>`, `<i>`, `<c>`, `</v>`).
    let mut out = String::with_capacity(trimmed.len());
    let mut in_tag = false;
    for c in trimmed.chars() {
        match (in_tag, c) {
            (false, '<') => in_tag = true,
            (true, '>') => in_tag = false,
            (false, c) => out.push(c),
            (true, _) => {}
        }
    }
    out
}

fn strip_leading_timestamp(s: &str) -> Option<&str> {
    let trimmed = s.trim_start();
    let (open, close) = if trimmed.starts_with('[') {
        ('[', ']')
    } else if trimmed.starts_with('(') {
        ('(', ')')
    } else {
        return None;
    };
    let end = trimmed.find(close)?;
    let inner = trimmed[open.len_utf8()..end].trim();
    if inner.contains(':') && inner.chars().all(|c| c.is_ascii_digit() || matches!(c, ':' | '.')) {
        Some(trimmed[end + close.len_utf8()..].trim_start())
    } else {
        None
    }
}

fn contains_meeting_marker(line: &str) -> bool {
    let low = line.to_ascii_lowercase();
    low.starts_with("attendees:")
        || low.starts_with("participants:")
        || low.starts_with("meeting notes")
        || low.starts_with("transcript")
        || low.contains("action item:")
        || low.starts_with("decision:")
}

/// Extracts typed entities, decisions, and action items deterministically from normalized text
/// and catalog context (`filename`, `collections`, `file_meta.who`).
pub fn extract_candidates_from_text(
    filename: &str,
    ext: &str,
    raw_text: &str,
    known_collections: &[String],
    file_meta_who: Option<&str>,
    owning_collection: Option<&str>,
) -> Vec<ExtractedCandidate> {
    let (text, looks_transcript) = normalize_transcript_text(ext, raw_text);
    let source_kind = if matches!(ext.to_ascii_lowercase().as_str(), "vtt" | "srt") {
        "transcript"
    } else if looks_transcript {
        "meeting_note"
    } else {
        "document"
    };

    let mut raw_candidates: Vec<ExtractedCandidate> = Vec::new();
    let lines: Vec<&str> = text.lines().collect();

    for line in &lines {
        let trimmed = line.trim();
        if trimmed.len() < 3 {
            continue;
        }
        let quote = excerpt_quote(trimmed);
        let (speaker_opt, content) = split_leading_speaker(trimmed);
        if let Some(speaker) = speaker_opt {
            raw_candidates.push(ExtractedCandidate {
                entity_type: EntityType::Person,
                name: speaker,
                summary: format!("Speaker in {filename}"),
                assignee: None,
                due_date: None,
                project_hint: owning_collection.map(str::to_string),
                evidence_quote: quote.clone(),
                confidence: 0.87,
                source_kind: source_kind.to_string(),
            });
        }

        // 1. Decisions
        if let Some((dec_text, conf)) = parse_decision_line(content) {
            let short_title = concise_title(&dec_text, 96);
            raw_candidates.push(ExtractedCandidate {
                entity_type: EntityType::Decision,
                name: short_title,
                summary: dec_text,
                assignee: None,
                due_date: extract_due_date(content),
                project_hint: owning_collection.map(str::to_string),
                evidence_quote: quote.clone(),
                confidence: conf,
                source_kind: source_kind.to_string(),
            });
        }

        // 2. Action items
        if let Some((task_text, assignee, due_date, conf)) = parse_action_line(content) {
            let short_title = concise_title(&task_text, 96);
            if let Some(ref person) = assignee
                && looks_like_person_name(person)
            {
                raw_candidates.push(ExtractedCandidate {
                    entity_type: EntityType::Person,
                    name: clean_person_name(person),
                    summary: format!("Assigned action item in {filename}"),
                    assignee: None,
                    due_date: None,
                    project_hint: owning_collection.map(str::to_string),
                    evidence_quote: quote.clone(),
                    confidence: 0.84,
                    source_kind: source_kind.to_string(),
                });
            }
            raw_candidates.push(ExtractedCandidate {
                entity_type: EntityType::ActionItem,
                name: short_title,
                summary: task_text,
                assignee: assignee.map(|p| clean_person_name(&p)),
                due_date,
                project_hint: owning_collection.map(str::to_string),
                evidence_quote: quote.clone(),
                confidence: conf,
                source_kind: source_kind.to_string(),
            });
        }

        // 3. People from headers and email contacts
        for person in parse_people_in_line(content) {
            raw_candidates.push(ExtractedCandidate {
                entity_type: EntityType::Person,
                name: person,
                summary: format!("Referenced in {filename}"),
                assignee: None,
                due_date: None,
                project_hint: owning_collection.map(str::to_string),
                evidence_quote: quote.clone(),
                confidence: 0.85,
                source_kind: source_kind.to_string(),
            });
        }

        // 4. Organizations from headers & corporate suffixes
        for org in parse_orgs_in_line(content) {
            raw_candidates.push(ExtractedCandidate {
                entity_type: EntityType::Organization,
                name: org,
                summary: format!("Organization mentioned in {filename}"),
                assignee: None,
                due_date: None,
                project_hint: owning_collection.map(str::to_string),
                evidence_quote: quote.clone(),
                confidence: 0.83,
                source_kind: source_kind.to_string(),
            });
        }

        // 5. Projects from explicit `Project:` / `"Project Alpha"` phrases
        for proj in parse_explicit_projects_in_line(content) {
            raw_candidates.push(ExtractedCandidate {
                entity_type: EntityType::Project,
                name: proj,
                summary: format!("Project referenced in {filename}"),
                assignee: None,
                due_date: None,
                project_hint: None,
                evidence_quote: quote.clone(),
                confidence: 0.86,
                source_kind: source_kind.to_string(),
            });
        }
    }

    // Whole-document curated SA/global Organization gazetteer reuse (`filing::orgs::ORGS`).
    let lower_text = text.to_ascii_lowercase();
    for &(display, patterns) in ORGS {
        if patterns.iter().any(|pat| has_word(&lower_text, pat)) {
            let line_hit = lines
                .iter()
                .find(|l| {
                    let ll = l.to_ascii_lowercase();
                    patterns.iter().any(|pat| has_word(&ll, pat))
                })
                .copied()
                .unwrap_or(filename);
            raw_candidates.push(ExtractedCandidate {
                entity_type: EntityType::Organization,
                name: display.to_string(),
                summary: format!("Matched organization reference in {filename}"),
                assignee: None,
                due_date: None,
                project_hint: owning_collection.map(str::to_string),
                evidence_quote: excerpt_quote(line_hit),
                confidence: 0.88,
                source_kind: source_kind.to_string(),
            });
        }
    }

    // Whole-document matching against `file_meta.who` from filing metadata.
    if let Some(who) = file_meta_who.map(str::trim).filter(|w| w.len() >= 2) {
        let etype = if looks_like_person_name(who) && !ORGS.iter().any(|(d, _)| d.eq_ignore_ascii_case(who)) {
            EntityType::Person
        } else {
            EntityType::Organization
        };
        raw_candidates.push(ExtractedCandidate {
            entity_type: etype,
            name: who.to_string(),
            summary: format!("Document filing metadata (`who`) on {filename}"),
            assignee: None,
            due_date: None,
            project_hint: owning_collection.map(str::to_string),
            evidence_quote: format!("Filed sender/entity metadata: {who}"),
            confidence: 0.86,
            source_kind: "metadata".to_string(),
        });
    }

    // Owning project collection or mentioned active collections on word boundaries.
    if let Some(col) = owning_collection.map(str::trim).filter(|s| s.len() >= 3) {
        raw_candidates.push(ExtractedCandidate {
            entity_type: EntityType::Project,
            name: col.to_string(),
            summary: format!("Owning project collection for {filename}"),
            assignee: None,
            due_date: None,
            project_hint: None,
            evidence_quote: format!("Located in project `{col}`"),
            confidence: 0.90,
            source_kind: source_kind.to_string(),
        });
    }
    for col in known_collections {
        let trimmed = col.trim();
        if trimmed.len() >= 4 && has_word(&lower_text, &trimmed.to_ascii_lowercase()) {
            let hit = lines
                .iter()
                .find(|l| has_word(&l.to_ascii_lowercase(), &trimmed.to_ascii_lowercase()))
                .copied()
                .unwrap_or(filename);
            raw_candidates.push(ExtractedCandidate {
                entity_type: EntityType::Project,
                name: trimmed.to_string(),
                summary: format!("Project mentioned in {filename}"),
                assignee: None,
                due_date: None,
                project_hint: None,
                evidence_quote: excerpt_quote(hit),
                confidence: 0.87,
                source_kind: source_kind.to_string(),
            });
        }
    }

    dedup_and_rank_candidates(raw_candidates)
}

fn dedup_and_rank_candidates(items: Vec<ExtractedCandidate>) -> Vec<ExtractedCandidate> {
    let mut by_key: HashMap<(EntityType, String), ExtractedCandidate> = HashMap::new();
    for item in items {
        let key_str = canonical_key(item.entity_type, &item.name);
        if key_str.len() < 2 {
            continue;
        }
        let k = (item.entity_type, key_str);
        match by_key.get_mut(&k) {
            Some(existing) => {
                if item.confidence > existing.confidence {
                    existing.confidence = item.confidence;
                    existing.name = item.name;
                    existing.evidence_quote = item.evidence_quote;
                }
                if existing.assignee.is_none() {
                    existing.assignee = item.assignee;
                }
                if existing.due_date.is_none() {
                    existing.due_date = item.due_date;
                }
                if existing.project_hint.is_none() {
                    existing.project_hint = item.project_hint;
                }
                if item.summary.len() > existing.summary.len() {
                    existing.summary = item.summary;
                }
            }
            None => {
                by_key.insert(k, item);
            }
        }
    }

    let mut out: Vec<ExtractedCandidate> = by_key.into_values().collect();
    out.sort_by(|a, b| {
        b.confidence
            .total_cmp(&a.confidence)
            .then_with(|| a.entity_type.cmp(&b.entity_type))
            .then_with(|| a.name.cmp(&b.name))
    });
    out.truncate(MAX_CANDIDATES_PER_FILE);
    out
}

fn excerpt_quote(s: &str) -> String {
    let clean = s.split_whitespace().collect::<Vec<_>>().join(" ");
    let mut out: String = clean.chars().take(220).collect();
    if clean.chars().count() > 220 {
        out.push('…');
    }
    out
}

fn concise_title(s: &str, max_chars: usize) -> String {
    let clean = s.split_whitespace().collect::<Vec<_>>().join(" ");
    if clean.chars().count() <= max_chars {
        return clean;
    }
    let mut out: String = clean.chars().take(max_chars.saturating_sub(1)).collect();
    if let Some(sp) = out.rfind(' ')
        && sp >= max_chars / 2
    {
        out.truncate(sp);
    }
    out.push('…');
    out
}

fn strip_markdown_prefix(s: &str) -> &str {
    let mut cur = s.trim();
    loop {
        if let Some(rest) = cur.strip_prefix("- [ ] ")
            .or_else(|| cur.strip_prefix("* [ ] "))
            .or_else(|| cur.strip_prefix("[ ] "))
            .or_else(|| cur.strip_prefix("- "))
            .or_else(|| cur.strip_prefix("* "))
        {
            cur = rest.trim_start();
            continue;
        }
        break;
    }
    cur.trim_matches('*').trim()
}

fn parse_decision_line(line: &str) -> Option<(String, f64)> {
    let normalized = strip_markdown_prefix(line);
    let low = normalized.to_ascii_lowercase();
    for pfx in [
        "decision:",
        "decided:",
        "agreed:",
        "resolved:",
        "approved:",
        "conclusion:",
    ] {
        if let Some(idx) = low.strip_prefix(pfx).map(|_| pfx.len()) {
            let body = normalized[idx..].trim().trim_start_matches('*').trim();
            if body.len() >= 8 {
                return Some((body.to_string(), 0.93));
            }
        }
    }
    for marker in ["we decided to ", "team agreed to ", "agreed to ", "decided to "] {
        if let Some(pos) = low.find(marker) {
            let body = normalized[pos..].trim();
            if body.len() >= 12 {
                return Some((body.to_string(), 0.87));
            }
        }
    }
    None
}

fn parse_action_line(line: &str) -> Option<(String, Option<String>, Option<String>, f64)> {
    let trimmed = line.trim();
    let lower = trimmed.to_ascii_lowercase();

    // Unchecked Markdown task items: `- [ ] ...` or `* [ ] ...`
    if lower.starts_with("- [ ] ") || lower.starts_with("* [ ] ") || lower.starts_with("[ ] ") {
        let body = strip_markdown_prefix(trimmed);
        if body.len() >= 6 {
            let assignee = extract_assignee(body);
            let due = extract_due_date(body);
            return Some((body.to_string(), assignee, due, 0.92));
        }
    }

    let stripped = strip_markdown_prefix(trimmed);
    let slow = stripped.to_ascii_lowercase();
    for pfx in [
        "action item:",
        "action:",
        "todo:",
        "follow-up:",
        "follow up:",
        "next step:",
        "task:",
    ] {
        if slow.starts_with(pfx) {
            let body = stripped[pfx.len()..].trim().trim_start_matches('*').trim();
            if body.len() >= 6 {
                let assignee = extract_assignee(body);
                let due = extract_due_date(body);
                return Some((body.to_string(), assignee, due, 0.93));
            }
        }
    }

    // Transcript spoken commitment: `Alice Mokoena will send the budget by Friday.`
    if let Some(pos) = stripped.find(" will ") {
        let speaker_part = stripped[..pos].trim();
        let after_will = stripped[pos + 6..].trim();
        if looks_like_person_name(speaker_part) && after_will.len() >= 6 {
            let due = extract_due_date(stripped);
            return Some((
                stripped.to_string(),
                Some(speaker_part.to_string()),
                due,
                0.82,
            ));
        }
    }

    None
}

fn extract_assignee(body: &str) -> Option<String> {
    // Pattern: `@Alice` or `@Alice_Mokoena`
    if let Some(at_pos) = body.find('@') {
        let after = &body[at_pos + 1..];
        let token: String = after
            .chars()
            .take_while(|c| c.is_alphanumeric() || matches!(c, '_' | '-' | '.'))
            .collect();
        if token.len() >= 2 && token.chars().next().is_some_and(|c| c.is_alphabetic()) {
            return Some(token.replace('_', " "));
        }
    }
    // Pattern: `(Owner: Name)` or `Owner: Name` or `Assignee: Name`
    let low = body.to_ascii_lowercase();
    for marker in ["owner:", "assignee:", "assigned to:"] {
        if let Some(pos) = low.find(marker) {
            let after = body[pos + marker.len()..].trim_start();
            let candidate: String = after
                .chars()
                .take_while(|&c| !matches!(c, ')' | ',' | ';' | '|' | '.'))
                .collect();
            let clean = candidate.trim();
            if looks_like_person_name(clean) {
                return Some(clean.to_string());
            }
        }
    }
    // Pattern: leading `Alice to ...` or `Alice Mokoena to ...`
    if let Some(to_pos) = body.find(" to ") {
        let lead = body[..to_pos].trim();
        if looks_like_person_name(lead) {
            return Some(lead.to_string());
        }
    }
    None
}

fn extract_due_date(s: &str) -> Option<String> {
    // Detect ISO YYYY-MM-DD anywhere in line.
    let bytes = s.as_bytes();
    if bytes.len() >= 10 {
        for i in 0..=bytes.len() - 10 {
            let slice = &s[i..i + 10];
            let sb = slice.as_bytes();
            if sb[4] == b'-'
                && sb[7] == b'-'
                && sb[0..4].iter().all(|b| b.is_ascii_digit())
                && sb[5..7].iter().all(|b| b.is_ascii_digit())
                && sb[8..10].iter().all(|b| b.is_ascii_digit())
            {
                return Some(slice.to_string());
            }
        }
    }
    // Detect `by Monday`..`by Friday` / `by EOD` / `by tomorrow`
    let low = s.to_ascii_lowercase();
    for phrase in [
        "by monday",
        "by tuesday",
        "by wednesday",
        "by thursday",
        "by friday",
        "by tomorrow",
        "by eod",
        "by end of week",
        "next week",
    ] {
        if let Some(idx) = low.find(phrase) {
            return Some(s[idx..idx + phrase.len()].to_string());
        }
    }
    None
}

fn split_leading_speaker(line: &str) -> (Option<String>, &str) {
    let trimmed = strip_markdown_prefix(line);
    if let Some(colon) = trimmed.find(':')
        && colon <= 40
    {
        let label = trimmed[..colon].trim();
        let after = trimmed[colon + 1..].trim();
        if !after.is_empty() && looks_like_person_name(label) && label.contains(' ') {
            return (Some(clean_person_name(label)), after);
        }
    }
    (None, line)
}

fn parse_people_in_line(line: &str) -> Vec<String> {
    let mut out = Vec::new();
    let trimmed = strip_markdown_prefix(line);
    let low = trimmed.to_ascii_lowercase();

    for pfx in [
        "attendees:",
        "participants:",
        "present:",
        "owner:",
        "author:",
        "prepared by:",
        "assignee:",
        "lead:",
    ] {
        if low.starts_with(pfx) {
            let list = trimmed[pfx.len()..].trim();
            for part in list.split([',', ';', '|']) {
                let mut clean = part
                    .split('(')
                    .next()
                    .unwrap_or("")
                    .split('<')
                    .next()
                    .unwrap_or("")
                    .trim();
                for sep in [" from ", " at ", " - "] {
                    if let Some(pos) = clean.to_ascii_lowercase().find(sep) {
                        clean = clean[..pos].trim();
                    }
                }
                if looks_like_person_name(clean) {
                    out.push(clean_person_name(clean));
                }
            }
            return out;
        }
    }

    // Speaker label at start of line (`Alice Mokoena: Let's approve the release.`).
    if let Some(colon) = trimmed.find(':')
        && colon <= 40
    {
        let label = trimmed[..colon].trim();
        let after = trimmed[colon + 1..].trim();
        if !after.is_empty() && looks_like_person_name(label) && label.contains(' ') {
            out.push(clean_person_name(label));
        }
    }

    // Email display contact `Alice Mokoena <alice@example.com>`.
    for chunk in trimmed.split('>') {
        if let Some(lt) = chunk.find('<')
            && chunk[lt + 1..].contains('@')
        {
            let before = chunk[..lt].trim().rsplit([',', ';', ':']).next().unwrap_or("").trim();
            if looks_like_person_name(before) {
                out.push(clean_person_name(before));
            }
        }
    }

    out
}

fn looks_like_person_name(raw: &str) -> bool {
    let trimmed = raw
        .trim()
        .trim_matches(|c: char| matches!(c, ',' | '.' | ';' | ':' | '"' | '\''));
    if trimmed.len() < 3 || trimmed.len() > 45 {
        return false;
    }
    let words: Vec<&str> = trimmed.split_whitespace().collect();
    if words.is_empty() || words.len() > 4 {
        return false;
    }
    let low_full = trimmed.to_ascii_lowercase();
    if NON_PERSON_SPEAKERS.contains(&low_full.as_str()) {
        return false;
    }
    if CORP_SUFFIXES
        .iter()
        .any(|sfx| low_full.ends_with(sfx) || words.iter().any(|w| w.eq_ignore_ascii_case(sfx)))
    {
        return false;
    }
    for (idx, w) in words.iter().enumerate() {
        let w_low = w.trim_end_matches('.').to_ascii_lowercase();
        if idx == 0 && matches!(w_low.as_str(), "dr" | "prof" | "mr" | "ms" | "mrs") {
            continue;
        }
        if NON_PERSON_SPEAKERS.contains(&w_low.as_str()) {
            return false;
        }
        let mut chars = w.chars();
        let Some(first) = chars.next() else {
            return false;
        };
        if !first.is_uppercase() {
            return false;
        }
        if w.len() >= 2 && w.chars().all(|c| c.is_uppercase()) && words.len() == 1 {
            // Single ALLCAPS word like `API` or `GDPR`.
            return false;
        }
        if !w.chars().all(|c| c.is_alphabetic() || matches!(c, '-' | '\'' | '.')) {
            return false;
        }
    }
    true
}

fn clean_person_name(raw: &str) -> String {
    raw.split_whitespace().collect::<Vec<_>>().join(" ")
}

fn parse_orgs_in_line(line: &str) -> Vec<String> {
    let mut out = Vec::new();
    let trimmed = strip_markdown_prefix(line);
    let low = trimmed.to_ascii_lowercase();

    for pfx in ["organization:", "organisation:", "company:", "client:", "vendor:", "partner:"] {
        if low.starts_with(pfx) {
            let rest = trimmed[pfx.len()..].trim();
            for part in rest.split([',', ';']) {
                let clean = part.trim();
                if clean.len() >= 2 && clean.len() <= 64 {
                    out.push(clean.to_string());
                }
            }
        }
    }

    // Scan windows ending with explicit corporate suffix tokens (`Ltd`, `Inc`, `Corp`, `(Pty) Ltd`, etc.).
    let tokens: Vec<&str> = trimmed.split_whitespace().collect();
    for i in 0..tokens.len() {
        let tok_clean = tokens[i]
            .trim_matches(|c: char| !c.is_alphanumeric() && c != '(' && c != ')')
            .to_ascii_lowercase();
        if CORP_SUFFIXES.contains(&tok_clean.as_str()) {
            // Walk left up to 4 Title-Case tokens.
            let mut start = i;
            let mut count = 0usize;
            while start > 0 && count < 4 {
                let prev = tokens[start - 1].trim_matches(|c: char| matches!(c, '(' | ')' | ','));
                let starts_upper = prev.chars().next().is_some_and(|c| c.is_uppercase());
                let is_connector = prev.eq_ignore_ascii_case("of")
                    || prev.eq_ignore_ascii_case("&")
                    || prev.eq_ignore_ascii_case("and");
                if starts_upper || (is_connector && start - 1 < i) {
                    start -= 1;
                    count += 1;
                } else {
                    break;
                }
            }
            if start < i {
                let org_name = tokens[start..=i]
                    .join(" ")
                    .trim_end_matches([',', ';', ':', '.'])
                    .to_string();
                if org_name.len() >= 5 && org_name.len() <= 72 {
                    out.push(org_name);
                }
            }
        }
    }

    out
}

fn parse_explicit_projects_in_line(line: &str) -> Vec<String> {
    let mut out = Vec::new();
    let trimmed = strip_markdown_prefix(line);
    let low = trimmed.to_ascii_lowercase();

    for pfx in ["project:", "initiative:", "workstream:"] {
        if low.starts_with(pfx) {
            let clean = trimmed[pfx.len()..]
                .split(['(', '|', ';'])
                .next()
                .unwrap_or("")
                .trim()
                .trim_end_matches('.');
            if clean.len() >= 3 && clean.len() <= 64 {
                out.push(clean.to_string());
            }
        }
    }

    // Inline `"Project Atlas"` phrase: `Project` + 1–2 capitalized words.
    let tokens: Vec<&str> = trimmed.split_whitespace().collect();
    for i in 0..tokens.len() {
        if tokens[i].eq_ignore_ascii_case("project") && i + 1 < tokens.len() {
            let next = tokens[i + 1].trim_matches(|c: char| !c.is_alphanumeric() && c != '-');
            let first_upper = next.chars().next().is_some_and(|c| c.is_uppercase());
            let low_next = next.to_ascii_lowercase();
            let skip_word = matches!(
                low_next.as_str(),
                "plan"
                    | "scope"
                    | "team"
                    | "notes"
                    | "status"
                    | "files"
                    | "folder"
                    | "update"
                    | "manager"
                    | "lead"
                    | "timeline"
                    | "budget"
            );
            if first_upper && !skip_word && next.len() >= 3 {
                out.push(format!("Project {next}"));
            }
        }
    }

    out
}

/// Builds `(system_prompt, user_prompt, context_snippets)` for optional on-demand local LLM
/// extraction (`K3`, `K5`). Truncates excerpt safely so CPU generation stays snappy.
pub fn build_knowledge_llm_prompt(
    filename: &str,
    text: &str,
    owning_collection: Option<&str>,
) -> Option<(String, String, Vec<String>)> {
    let clean = text.trim();
    if clean.is_empty() {
        return None;
    }
    let excerpt: String = clean.chars().take(2_800).collect();
    let snippets: Vec<String> = clean
        .lines()
        .map(str::trim)
        .filter(|l| !l.is_empty())
        .take(8)
        .map(str::to_string)
        .collect();
    if snippets.is_empty() {
        return None;
    }

    let proj_note = owning_collection
        .map(|p| format!("Owning project context: `{p}`.\n"))
        .unwrap_or_default();

    let system_prompt = "Extract structured knowledge items from the user's local document or transcript. Return ONLY a JSON array (or tagged lines like `[DECISION] ...`) containing items with fields: type (`person`|`organization`|`project`|`decision`|`action_item`), name, summary, assignee, due_date, project_hint, evidence_quote. Never invent names or facts missing from the text.".to_string();
    let user_prompt = format!(
        "File: `{filename}`\n{proj_note}\nContent excerpt:\n{excerpt}"
    );

    Some((system_prompt, user_prompt, snippets))
}

/// Parses structured LLM output (JSON array or tagged lines `[PERSON]` / `[ORG]` / `[PROJECT]` /
/// `[DECISION]` / `[ACTION]`) into `ExtractedCandidate` items.
pub fn parse_llm_extraction_output(output: &str, default_source_kind: &str) -> Vec<ExtractedCandidate> {
    let mut parsed = parse_llm_json(output, default_source_kind);
    if parsed.is_empty() {
        parsed = parse_llm_tagged_lines(output, default_source_kind);
    }
    dedup_and_rank_candidates(parsed)
}

fn parse_llm_json(output: &str, default_source_kind: &str) -> Vec<ExtractedCandidate> {
    let trimmed = output.trim();
    let stripped = trimmed
        .strip_prefix("```json")
        .or_else(|| trimmed.strip_prefix("```"))
        .unwrap_or(trimmed);
    let stripped = stripped.strip_suffix("```").unwrap_or(stripped).trim();

    let json_slice = if let (Some(a), Some(b)) = (stripped.find('['), stripped.rfind(']'))
        && a < b
    {
        &stripped[a..=b]
    } else {
        stripped
    };

    let Ok(val) = serde_json::from_str::<serde_json::Value>(json_slice) else {
        return Vec::new();
    };
    let Some(arr) = val.as_array() else {
        return Vec::new();
    };

    let mut out = Vec::new();
    for item in arr {
        let Some(obj) = item.as_object() else {
            continue;
        };
        let raw_type = obj
            .get("type")
            .or_else(|| obj.get("entity_type"))
            .and_then(|v| v.as_str())
            .unwrap_or("");
        let Some(entity_type) = strict_parse_entity_type(raw_type) else {
            continue;
        };
        let Some(name) = obj.get("name").and_then(|v| v.as_str()).map(str::trim) else {
            continue;
        };
        if name.len() < 2 {
            continue;
        }
        let summary = obj
            .get("summary")
            .and_then(|v| v.as_str())
            .unwrap_or("")
            .trim()
            .to_string();
        let assignee = obj
            .get("assignee")
            .and_then(|v| v.as_str())
            .map(str::trim)
            .filter(|s| !s.is_empty())
            .map(str::to_string);
        let due_date = obj
            .get("due_date")
            .and_then(|v| v.as_str())
            .map(str::trim)
            .filter(|s| !s.is_empty())
            .map(str::to_string);
        let project_hint = obj
            .get("project_hint")
            .or_else(|| obj.get("project"))
            .and_then(|v| v.as_str())
            .map(str::trim)
            .filter(|s| !s.is_empty())
            .map(str::to_string);
        let evidence_quote = obj
            .get("evidence_quote")
            .or_else(|| obj.get("quote"))
            .and_then(|v| v.as_str())
            .unwrap_or(name)
            .trim()
            .to_string();
        out.push(ExtractedCandidate {
            entity_type,
            name: concise_title(name, 96),
            summary,
            assignee,
            due_date,
            project_hint,
            evidence_quote: excerpt_quote(&evidence_quote),
            confidence: 0.90,
            source_kind: default_source_kind.to_string(),
        });
    }
    out
}

fn parse_llm_tagged_lines(output: &str, default_source_kind: &str) -> Vec<ExtractedCandidate> {
    let mut out = Vec::new();
    let tags: &[(&str, EntityType)] = &[
        ("[person]", EntityType::Person),
        ("[org]", EntityType::Organization),
        ("[organization]", EntityType::Organization),
        ("[project]", EntityType::Project),
        ("[decision]", EntityType::Decision),
        ("[action]", EntityType::ActionItem),
        ("[action_item]", EntityType::ActionItem),
    ];
    for raw_line in output.lines() {
        let trimmed = strip_markdown_prefix(raw_line);
        let low = trimmed.to_ascii_lowercase();
        for &(tag, etype) in tags {
            if low.starts_with(tag) {
                let body = trimmed[tag.len()..].trim().trim_start_matches(':').trim();
                if body.len() >= 2 {
                    out.push(ExtractedCandidate {
                        entity_type: etype,
                        name: concise_title(body, 96),
                        summary: body.to_string(),
                        assignee: extract_assignee(body),
                        due_date: extract_due_date(body),
                        project_hint: None,
                        evidence_quote: excerpt_quote(body),
                        confidence: 0.85,
                        source_kind: default_source_kind.to_string(),
                    });
                }
                break;
            }
        }
    }
    out
}

fn strict_parse_entity_type(raw: &str) -> Option<EntityType> {
    match raw.trim().to_ascii_lowercase().as_str() {
        "person" | "people" => Some(EntityType::Person),
        "organization" | "org" | "company" | "vendor" => Some(EntityType::Organization),
        "project" | "initiative" => Some(EntityType::Project),
        "decision" => Some(EntityType::Decision),
        "action_item" | "action" | "task" | "todo" => Some(EntityType::ActionItem),
        _ => None,
    }
}

/// Filters stopwords / noise when checking if a file is an un-paired transcript (`text` or caption).
pub fn is_extractable_text_ext(ext: &str) -> bool {
    matches!(
        ext.trim().trim_start_matches('.').to_ascii_lowercase().as_str(),
        "md" | "markdown" | "txt" | "vtt" | "srt" | "csv" | "tsv" | "json" | "rtf" | "log"
    )
}

pub fn looks_like_audio_or_video_ext(ext: &str) -> bool {
    matches!(
        ext.trim().trim_start_matches('.').to_ascii_lowercase().as_str(),
        "mp3" | "wav" | "m4a" | "flac" | "ogg" | "opus" | "aac" | "wma" | "mp4" | "mkv" | "webm"
            | "mov"
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn normalizes_vtt_and_extracts_all_five_entity_types() {
        let vtt = "\
WEBVTT

1
00:00:01.000 --> 00:00:04.500
<v Alice Mokoena>Attendees: Alice Mokoena, Bob Smith from Acme Corp.</v>

2
00:00:05.000 --> 00:00:09.000
<v Bob Smith>DECISION: Standardize SQLCipher encryption for Project Helios.</v>

3
00:00:10.000 --> 00:00:15.000
<v Alice Mokoena>- [ ] Alice Mokoena to ship CredUI unlock flow by 2026-12-01.</v>
";
        let (clean, is_tr) = normalize_transcript_text("vtt", vtt);
        assert!(is_tr);
        assert!(clean.contains("Alice Mokoena:"));
        assert!(!clean.contains("-->"));

        let cands = extract_candidates_from_text(
            "Sync.vtt",
            "vtt",
            vtt,
            &["Helios".to_string()],
            Some("Discovery"),
            Some("Helios"),
        );

        assert!(cands.iter().any(|c| c.entity_type == EntityType::Person && c.name == "Alice Mokoena"));
        assert!(cands.iter().any(|c| c.entity_type == EntityType::Person && c.name == "Bob Smith"));
        assert!(cands.iter().any(|c| c.entity_type == EntityType::Organization && c.name == "Acme Corp"));
        assert!(cands.iter().any(|c| c.entity_type == EntityType::Organization && c.name == "Discovery"));
        assert!(cands.iter().any(|c| c.entity_type == EntityType::Project && c.name == "Helios"));
        assert!(cands.iter().any(|c| c.entity_type == EntityType::Decision && c.summary.contains("SQLCipher")));
        let act = cands.iter().find(|c| c.entity_type == EntityType::ActionItem).unwrap();
        assert_eq!(act.assignee.as_deref(), Some("Alice Mokoena"));
        assert_eq!(act.due_date.as_deref(), Some("2026-12-01"));
    }

    #[test]
    fn parses_llm_json_and_tagged_bullets_cleanly() {
        let json_out = r#"```json
[
  {"type":"decision","name":"Use local Qwen2","summary":"Keep all RAG on device","evidence_quote":"Use Qwen2"},
  {"type":"action_item","name":"Benchmark GGUF","assignee":"Thabo Ndlovu","due_date":"2026-12-05","project_hint":"Atlas"}
]
```"#;
        let from_json = parse_llm_extraction_output(json_out, "document");
        assert_eq!(from_json.len(), 2);
        assert_eq!(from_json[0].entity_type, EntityType::Decision);
        assert_eq!(from_json[1].assignee.as_deref(), Some("Thabo Ndlovu"));

        let bullets = "\
- [PERSON] Thabo Ndlovu
- [DECISION] Archive stale branches weekly
- [ACTION] Thabo to audit backups by 2026-12-03";
        let from_bullets = parse_llm_extraction_output(bullets, "meeting_note");
        assert_eq!(from_bullets.len(), 3);
        assert!(from_bullets.iter().any(|c| c.entity_type == EntityType::Person));
        assert!(from_bullets.iter().any(|c| c.entity_type == EntityType::Decision));
        assert!(from_bullets.iter().any(|c| c.entity_type == EntityType::ActionItem));
    }
}
