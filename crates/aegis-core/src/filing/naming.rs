//! The naming scheme (spec §6): `<YYYY-MM-DD> <who/what> <type>.<ext>` for documents Cairn is
//! sure about, `<YYYY-MM-DD> <HH.MM.SS> Screenshot.<ext>` for screenshots, nothing for the rest.

use super::Kind;
use super::classify::{Classification, DateSource};

/// Longest name Cairn gives a file, extension included.
pub const MAX_NAME: usize = 120;
/// Date and type must both be at least this sure before a document is renamed.
pub const RENAME_CONFIDENCE: f64 = 0.80;
/// Finished documents get descriptive names; notes and data files (md, txt, csv) keep theirs.
const RENAMABLE_DOCS: [&str; 10] = ["pdf", "doc", "docx", "odt", "rtf", "xls", "xlsx", "ods", "ppt", "pptx"];

/// A name Windows accepts: forbidden characters removed, spaces collapsed, no trailing dots
/// or spaces, no reserved device name (CON, NUL, COM1 …, which get a `_`), at most
/// [`MAX_NAME`] characters with the extension kept. A name of only dots and spaces becomes "".
pub fn sanitize(name: &str) -> String {
    not_reserved(sanitize_chars(name))
}

/// Windows refuses CON, PRN, AUX, NUL, COM1–9 and LPT1–9 as a name, with or without an extension.
fn not_reserved(name: String) -> String {
    let (stem, rest) = name.split_once('.').map_or((name.as_str(), ""), |(s, _)| (s, &name[s.len()..]));
    let stem = stem.trim_end();
    let up = stem.to_ascii_uppercase();
    let device = |p: &str| up.len() == 4 && up.starts_with(p) && matches!(up.as_bytes()[3], b'1'..=b'9');
    if ["CON", "PRN", "AUX", "NUL"].contains(&up.as_str()) || device("COM") || device("LPT") {
        format!("{stem}_{rest}")
    } else {
        name
    }
}

/// [`sanitize`] for a folder made from a tag or name: `fallback` when nothing usable is left.
pub fn folder_name(raw: &str, fallback: &str) -> String {
    let s = sanitize(raw);
    if s.is_empty() { fallback.to_string() } else { s }
}

fn sanitize_chars(name: &str) -> String {
    let cleaned: String = name.chars().filter(|c| !"<>:\"/\\|?*".contains(*c) && !c.is_control()).collect();
    let collapsed = cleaned.split_whitespace().collect::<Vec<_>>().join(" ");
    let trimmed = collapsed.trim_end_matches(['.', ' ']).to_string();
    if trimmed.chars().count() <= MAX_NAME {
        return trimmed;
    }
    let (stem, ext) = match trimmed.rsplit_once('.') {
        Some((s, e)) if e.len() <= 8 && !e.contains(' ') => (s.to_string(), format!(".{e}")),
        _ => (trimmed.clone(), String::new()),
    };
    let keep = MAX_NAME.saturating_sub(ext.chars().count());
    let short: String = stem.chars().take(keep).collect();
    format!("{}{ext}", short.trim_end_matches(['.', ' ']))
}

fn sentence_case(s: &str) -> String {
    let lower = s.to_lowercase();
    let mut c = lower.chars();
    match c.next() {
        Some(f) => f.to_uppercase().collect::<String>() + c.as_str(),
        None => String::new(),
    }
}

/// What a document is about, from its old name: dates, bare numbers and "(1)" removed.
fn subject_of(stem: &str) -> String {
    let words: Vec<&str> = stem
        .split(|c: char| c == '_' || c == '-' || c.is_whitespace())
        .filter(|w| !w.is_empty())
        .filter(|w| {
            let t = w.trim_matches(|c: char| c == '(' || c == ')');
            !t.is_empty() && !t.chars().all(|c| c.is_ascii_digit() || c == '.' || c == '/')
        })
        .collect();
    sentence_case(&words.join(" "))
}

/// Time of day `HH.MM.SS` from a screenshot's name, or from `mtime` (UTC, shown in local time)
/// when it has none.
fn screenshot_time(stem: &str, mtime: i64) -> String {
    let digits: Vec<(usize, String)> = {
        let mut out = Vec::new();
        let mut cur = String::new();
        let mut start = 0;
        for (i, c) in stem.char_indices() {
            if c.is_ascii_digit() {
                if cur.is_empty() {
                    start = i;
                }
                cur.push(c);
            } else if !cur.is_empty() {
                out.push((start, std::mem::take(&mut cur)));
            }
        }
        if !cur.is_empty() {
            out.push((start, cur));
        }
        out
    };
    let valid = |h: u32, m: u32, s: u32| h < 24 && m < 60 && s < 60;
    // "101010"
    for (_, d) in &digits {
        if d.len() == 6 {
            let (h, m, s) = (d[..2].parse().unwrap_or(99), d[2..4].parse().unwrap_or(99), d[4..].parse().unwrap_or(99));
            if valid(h, m, s) {
                return format!("{h:02}.{m:02}.{s:02}");
            }
        }
    }
    // "10.10.10" / "10-10-10" / "10_10_10": three 1–2 digit groups in a row after the date.
    for w in digits.windows(3) {
        if w.iter().all(|(_, d)| d.len() <= 2) {
            let n: Vec<u32> = w.iter().map(|(_, d)| d.parse().unwrap_or(99)).collect();
            if valid(n[0], n[1], n[2]) {
                return format!("{:02}.{:02}.{:02}", n[0], n[1], n[2]);
            }
        }
    }
    let secs = (super::local_ms(mtime) / 1000).rem_euclid(86_400);
    format!("{:02}.{:02}.{:02}", secs / 3600, (secs / 60) % 60, secs % 60)
}

/// The name Cairn would give this file, or `None` to keep its name.
pub fn proposed_name(c: &Classification, original_stem: &str, ext: &str, mtime: i64) -> Option<String> {
    let ext = ext.to_lowercase();
    let name = match c.kind {
        Kind::Screenshot => {
            // A modified date says when the file was last touched, not when it was taken.
            let date = c.doc_date.as_ref().filter(|_| matches!(c.date_source, DateSource::Name | DateSource::Exif | DateSource::Content))?;
            // The time follows the date in the name; digits before it are the date itself.
            let tail = original_stem.find(date.as_str()).map_or(original_stem, |i| &original_stem[i + date.len()..]);
            let tail = if c.date_source == DateSource::Name { tail } else { original_stem };
            format!("{date} {} Screenshot.{ext}", screenshot_time(tail, mtime))
        }
        Kind::Document => {
            if !RENAMABLE_DOCS.contains(&ext.as_str()) || c.type_confidence < RENAME_CONFIDENCE
                || !matches!(c.date_source, DateSource::Content | DateSource::Name | DateSource::Exif)
            {
                return None;
            }
            let date = c.doc_date.as_ref()?;
            let doc_type = c.doc_type.as_ref()?;
            let who_or_what = c.who.clone().unwrap_or_else(|| subject_of(original_stem));
            let mentions_type = who_or_what.to_lowercase().contains(&doc_type.to_lowercase());
            let mut parts = vec![date.clone()];
            if !who_or_what.is_empty() {
                parts.push(who_or_what);
            }
            if !mentions_type {
                parts.push(doc_type.clone());
            }
            format!("{}.{ext}", parts.join(" "))
        }
        _ => return None,
    };
    let name = sanitize(&name);
    let original = format!("{original_stem}.{ext}");
    (!name.eq_ignore_ascii_case(&original)).then_some(name)
}
