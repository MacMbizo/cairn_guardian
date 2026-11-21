//! Works out what a file is (spec §7.2): its kind, and for documents the life area, document
//! type, who it is from and its date, with a confidence and the evidence behind it.

use super::Kind;

const CODE_EXTS: [&str; 18] = [
    "rs", "py", "ts", "tsx", "js", "jsx", "java", "c", "cpp", "h", "cs", "go", "php", "rb", "vb", "ipynb", "kt", "swift",
];
const SYSTEM_EXTS: [&str; 18] = [
    "dll", "cab", "cat", "mst", "pyc", "pyi", "sys", "ini", "dat", "cfg", "resources", "manifest", "msp", "tmp", "log",
    "lnk", "db", "pdb",
];
const DOC_EXTS: [&str; 16] = [
    "pdf", "doc", "docx", "odt", "rtf", "txt", "md", "xls", "xlsx", "xlsm", "ods", "csv", "ppt", "pptx", "odp", "pages",
];
const PHOTO_EXTS: [&str; 9] = ["jpg", "jpeg", "heic", "png", "tif", "tiff", "raw", "cr2", "nef"];
const VIDEO_EXTS: [&str; 9] = ["mp4", "mov", "mkv", "avi", "wmv", "m4v", "webm", "3gp", "mts"];
const MUSIC_EXTS: [&str; 6] = ["mp3", "m4a", "flac", "ogg", "wma", "aac"];
const AUDIO_EXTS: [&str; 3] = ["wav", "m4b", "opus"];
const ARCHIVE_EXTS: [&str; 5] = ["zip", "rar", "7z", "tar", "gz"];
const FONT_EXTS: [&str; 4] = ["ttf", "otf", "woff", "woff2"];

fn looks_like_screenshot(name: &str) -> bool {
    let n = name.to_ascii_lowercase();
    n.contains("screenshot") || n.contains("screen shot") || n.starts_with("capture") || n.starts_with("snip")
}

/// The kind of a file from its extension and name. Anything inside a code project is code.
pub fn kind_of(ext: &str, name: &str, in_atomic_unit: bool) -> Kind {
    if in_atomic_unit {
        return Kind::Code;
    }
    let e = ext.to_ascii_lowercase();
    let e = e.as_str();
    if CODE_EXTS.contains(&e) {
        Kind::Code
    } else if SYSTEM_EXTS.contains(&e) {
        Kind::System
    } else if ["exe", "msi", "msix"].contains(&e) {
        Kind::Installer
    } else if ARCHIVE_EXTS.contains(&e) {
        Kind::Archive
    } else if FONT_EXTS.contains(&e) {
        Kind::Font
    } else if (e == "png" || e == "jpg" || e == "jpeg") && looks_like_screenshot(name) {
        Kind::Screenshot
    } else if PHOTO_EXTS.contains(&e) {
        Kind::Photo
    } else if VIDEO_EXTS.contains(&e) {
        Kind::Video
    } else if MUSIC_EXTS.contains(&e) {
        Kind::Music
    } else if AUDIO_EXTS.contains(&e) {
        Kind::Audio
    } else if DOC_EXTS.contains(&e) {
        Kind::Document
    } else {
        Kind::Other
    }
}

pub use super::orgs::find_org;
use super::orgs::has_word;
use serde::{Deserialize, Serialize};
use ts_rs::TS;

/// Where a file's date came from (only content, name and photo dates are trusted for naming).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "snake_case")]
#[ts(export)]
pub enum DateSource {
    Content,
    Name,
    Exif,
    Modified,
    None,
}

#[derive(Debug, Clone, PartialEq)]
pub struct Classification {
    pub kind: Kind,
    /// Life area, `"Finance/Invoices & receipts"` style.
    pub category: Option<String>,
    pub doc_type: Option<String>,
    pub who: Option<String>,
    /// `YYYY-MM-DD`.
    pub doc_date: Option<String>,
    pub date_source: DateSource,
    pub confidence: f64,
    pub type_confidence: f64,
    pub evidence: Vec<String>,
}

pub struct ClassifyInput<'a> {
    pub name: &'a str,
    pub ext: &'a str,
    pub text: Option<&'a str>,
    pub exif_taken_at: Option<i64>,
    pub mtime: i64,
    pub in_atomic_unit: bool,
    pub learned_orgs: &'a [String],
}

/// `(category, document type, [(keyword, weight)])`. Keywords are lowercase and matched on word
/// boundaries. Extend here; the scoring needs no other change.
type Entry = (&'static str, &'static str, &'static [(&'static str, f64)]);

const DICTIONARY: &[Entry] = &[
    ("Finance/Invoices & receipts", "invoice", &[("tax invoice", 3.0), ("invoice", 2.0), ("invoice date", 2.0), ("invoice no", 2.0), ("vat", 1.5), ("amount due", 2.0), ("total due", 1.5), ("bill to", 1.5)]),
    ("Finance/Invoices & receipts", "receipt", &[("receipt", 2.5), ("payment received", 2.0), ("till slip", 2.0), ("thank you for your purchase", 2.0), ("change due", 1.5)]),
    ("Finance/Invoices & receipts", "quote", &[("quotation", 2.5), ("quote no", 2.0), ("valid until", 1.0), ("estimate", 1.5)]),
    ("Finance/Banking", "statement", &[("bank statement", 3.0), ("statement of account", 2.5), ("account number", 1.5), ("opening balance", 2.0), ("closing balance", 2.0), ("available balance", 1.5), ("transaction history", 2.0), ("branch code", 1.5)]),
    ("Finance/Tax", "assessment", &[("notice of assessment", 3.0), ("assessment", 2.0), ("ita34", 3.0), ("income tax", 2.0), ("tax year", 1.5), ("sars", 2.0)]),
    ("Finance/Tax", "tax return", &[("tax return", 3.0), ("itr12", 3.0), ("irp5", 3.0), ("it3", 2.0), ("efiling", 2.0), ("tax certificate", 2.5)]),
    ("Finance/Payslips", "payslip", &[("payslip", 3.0), ("pay slip", 3.0), ("salary advice", 3.0), ("gross pay", 2.0), ("nett pay", 2.0), ("net pay", 2.0), ("paye", 2.0), ("uif", 1.0), ("earnings", 1.0), ("deductions", 1.0)]),
    ("Finance/Insurance", "policy", &[("policy schedule", 3.0), ("policy number", 2.0), ("premium", 2.0), ("insured", 1.5), ("cover", 0.5), ("excess", 1.0)]),
    ("Finance/Insurance", "claim", &[("claim number", 2.5), ("claim form", 2.5), ("claim", 1.0)]),
    ("Finance/Investments", "statement", &[("investment statement", 3.0), ("unit trust", 2.5), ("portfolio", 1.5), ("tax-free savings", 2.5), ("retirement annuity", 2.5), ("dividend", 1.5)]),
    ("Personal/ID & certificates", "ID", &[("identity document", 3.0), ("id number", 2.0), ("passport", 2.5), ("smart id", 3.0), ("birth certificate", 3.0), ("marriage certificate", 3.0), ("drivers licence", 2.5), ("driver's licence", 2.5)]),
    ("Personal/ID & certificates", "certificate", &[("certificate of", 2.0), ("certificate", 1.0), ("hereby certifies", 2.5), ("awarded to", 2.0)]),
    ("Personal/Health", "medical", &[("medical aid", 2.5), ("prescription", 2.5), ("patient", 2.0), ("diagnosis", 2.0), ("doctor", 1.0), ("pathology", 2.0), ("hospital", 1.5), ("icd-10", 2.5)]),
    ("Personal/Home", "lease", &[("lease agreement", 3.0), ("tenant", 2.0), ("landlord", 2.0), ("rental", 1.5), ("bond", 1.0), ("rates and taxes", 2.5), ("municipal account", 2.5), ("electricity", 1.0), ("water and sanitation", 2.0)]),
    ("Personal/Vehicles", "vehicle", &[("licence disc", 3.0), ("vehicle registration", 3.0), ("vin", 2.0), ("engine number", 2.0), ("roadworthy", 2.5), ("service history", 2.0), ("odometer", 1.5)]),
    ("Personal/Travel", "booking", &[("boarding pass", 3.0), ("itinerary", 2.5), ("booking reference", 2.5), ("e-ticket", 2.5), ("check-in", 1.0), ("reservation", 1.5)]),
    ("Work", "CV", &[("curriculum vitae", 3.0), ("cv", 2.0), ("work experience", 2.0), ("references available", 2.0), ("skills", 0.5)]),
    ("Work", "contract", &[("contract of employment", 3.0), ("employment contract", 3.0), ("employer", 1.5), ("employee", 1.5), ("terms and conditions of employment", 2.5), ("agreement", 1.0)]),
    ("Work", "timesheet", &[("timesheet", 3.0), ("hours worked", 2.0)]),
    ("Work", "report", &[("executive summary", 2.0), ("report", 1.0), ("minutes of meeting", 2.5), ("agenda", 1.5), ("action items", 1.5)]),
    ("Study", "assignment", &[("assignment", 2.5), ("student number", 2.5), ("module", 1.5), ("due date", 1.0), ("lecturer", 2.0), ("plagiarism", 2.0)]),
    ("Study", "transcript", &[("academic record", 3.0), ("transcript", 2.5), ("results", 1.0), ("credits", 1.0)]),
    ("Study", "notes", &[("lecture notes", 2.5), ("notes", 1.0), ("chapter", 1.0), ("summary", 0.5)]),
];

/// Name-only evidence is weaker: a file called "payslip" might not be one.
const NAME_WEIGHT: f64 = 0.6;
const NAME_ONLY_CAP: f64 = 0.75;
const ORG_BONUS: f64 = 0.15;

fn day_ok(y: i64, m: i64, d: i64) -> bool {
    if !(1900..=2100).contains(&y) || !(1..=12).contains(&m) || d < 1 {
        return false;
    }
    let leap = (y % 4 == 0 && y % 100 != 0) || y % 400 == 0;
    let max = match m {
        2 if leap => 29,
        2 => 28,
        4 | 6 | 9 | 11 => 30,
        _ => 31,
    };
    d <= max
}

fn month_of(word: &str) -> Option<i64> {
    const FULL: [&str; 12] = [
        "january", "february", "march", "april", "may", "june", "july", "august", "september", "october", "november",
        "december",
    ];
    // "mar", "march" and "sept" all count; "ma" and "marching" don't.
    let w = word.trim_end_matches('.');
    if w.len() < 3 {
        return None;
    }
    FULL.iter().position(|m| m.starts_with(w)).map(|i| i as i64 + 1)
}

/// Every plausible date in `text`, with the byte offset where it starts.
fn dates_in(text: &str) -> Vec<(usize, String)> {
    let lower = text.to_lowercase();
    // Tokens: (start offset, token) split on whitespace and commas.
    let mut tokens: Vec<(usize, &str)> = Vec::new();
    let mut start = None;
    for (i, c) in lower.char_indices() {
        if c.is_whitespace() || c == ',' {
            if let Some(s) = start.take() {
                tokens.push((s, &lower[s..i]));
            }
        } else if start.is_none() {
            start = Some(i);
        }
    }
    if let Some(s) = start {
        tokens.push((s, &lower[s..]));
    }
    let fmt = |y: i64, m: i64, d: i64| format!("{y:04}-{m:02}-{d:02}");
    let num = |s: &str| s.trim_matches(|c: char| !c.is_ascii_digit()).parse::<i64>().ok();
    let mut out = Vec::new();
    for (k, &(pos, tok)) in tokens.iter().enumerate() {
        let t = tok.trim_matches(|c: char| !c.is_alphanumeric());
        // 2024-03-14, 14/03/2024, 14.03.2024, 2024/03/14
        let parts: Vec<&str> = t.split(['-', '/', '.']).collect();
        if parts.len() == 3 && parts.iter().all(|p| !p.is_empty() && p.chars().all(|c| c.is_ascii_digit())) {
            let n: Vec<i64> = parts.iter().filter_map(|p| p.parse().ok()).collect();
            if parts[0].len() == 4 && day_ok(n[0], n[1], n[2]) {
                out.push((pos, fmt(n[0], n[1], n[2])));
            } else if parts[2].len() == 4 {
                // Day first (South Africa), unless only month-first makes sense.
                if day_ok(n[2], n[1], n[0]) {
                    out.push((pos, fmt(n[2], n[1], n[0])));
                } else if day_ok(n[2], n[0], n[1]) {
                    out.push((pos, fmt(n[2], n[0], n[1])));
                }
            }
            continue;
        }
        // 20240314
        if t.len() == 8 && t.chars().all(|c| c.is_ascii_digit()) {
            let (y, m, d) = (t[..4].parse().unwrap_or(0), t[4..6].parse().unwrap_or(0), t[6..].parse().unwrap_or(0));
            if day_ok(y, m, d) {
                out.push((pos, fmt(y, m, d)));
            }
            continue;
        }
        // 14 March 2024
        if let (Some(d), Some(next), Some(year)) = (num(t), tokens.get(k + 1), tokens.get(k + 2))
            && t.chars().all(|c| c.is_ascii_digit())
            && t.len() <= 2
            && let (Some(m), Some(y)) = (month_of(next.1), num(year.1))
            && day_ok(y, m, d)
        {
            out.push((pos, fmt(y, m, d)));
            continue;
        }
        // March 14, 2024
        if let (Some(m), Some(day), Some(year)) = (month_of(t), tokens.get(k + 1), tokens.get(k + 2))
            && let (Some(d), Some(y)) = (num(day.1), num(year.1))
            && day.1.trim_matches(|c: char| !c.is_ascii_digit()).len() <= 2
            && day_ok(y, m, d)
        {
            out.push((pos, fmt(y, m, d)));
        }
    }
    out
}

/// The document's own date as `YYYY-MM-DD`: the one right after "date" if there is one,
/// otherwise the first plausible date.
pub fn find_date(text: &str) -> Option<String> {
    let dates = dates_in(text);
    let lower = text.to_lowercase();
    let near_label = dates.iter().find(|(pos, _)| {
        let from = pos.saturating_sub(24);
        let mut s = from;
        while !lower.is_char_boundary(s) {
            s += 1;
        }
        lower[s..*pos].contains("date")
    });
    near_label.or(dates.first()).map(|(_, d)| d.clone())
}

fn ymd(ms: i64) -> String {
    crate::engine::note::date_of(ms.div_euclid(86_400_000))
}

fn words_of(name: &str) -> String {
    let stem = name.rsplit_once('.').map_or(name, |(s, _)| s);
    stem.to_lowercase().replace(['_', '-', '.'], " ")
}

/// Classifies one file. Deterministic and explainable: every conclusion has evidence.
pub fn classify(input: &ClassifyInput) -> Classification {
    let kind = kind_of(input.ext, input.name, input.in_atomic_unit);
    let mut evidence = Vec::new();
    let text = input.text.unwrap_or("");
    let text_l = text.to_lowercase();
    let name_l = words_of(input.name);

    // Date: content, then name, then photo date, then the file's own date.
    let (doc_date, date_source) = if let Some(d) = (!text.is_empty()).then(|| find_date(text)).flatten() {
        evidence.push(format!("date {d} found in the content"));
        (Some(d), DateSource::Content)
    } else if let Some(d) = find_date(&name_l) {
        evidence.push(format!("date {d} found in the name"));
        (Some(d), DateSource::Name)
    } else if let Some(t) = input.exif_taken_at {
        (Some(ymd(t)), DateSource::Exif)
    } else if input.mtime > 0 {
        (Some(ymd(input.mtime)), DateSource::Modified)
    } else {
        (None, DateSource::None)
    };

    let who = find_org(&format!("{text} {}", input.name), input.learned_orgs);

    if kind != Kind::Document {
        let confidence = match kind {
            Kind::Photo if input.exif_taken_at.is_some() => 0.95,
            Kind::Photo => 0.75,
            Kind::Screenshot | Kind::Installer | Kind::Code | Kind::Font | Kind::Music | Kind::Archive => 0.95,
            Kind::Video | Kind::Audio => 0.85,
            Kind::System | Kind::Other | Kind::Document => 0.3,
        };
        return Classification {
            kind,
            category: None,
            doc_type: None,
            who,
            doc_date,
            date_source,
            confidence,
            type_confidence: confidence,
            evidence,
        };
    }

    // Score every dictionary entry on content and on the name.
    let mut scored: Vec<(f64, bool, usize, Vec<String>)> = DICTIONARY
        .iter()
        .enumerate()
        .map(|(i, (_, _, kws))| {
            let mut score = 0.0;
            let mut in_text = false;
            let mut ev = Vec::new();
            for (kw, w) in kws.iter() {
                if !text_l.is_empty() && has_word(&text_l, kw) {
                    score += w;
                    in_text = true;
                    ev.push(format!("content mentions '{kw}'"));
                } else if has_word(&name_l, kw) {
                    score += w * NAME_WEIGHT;
                    ev.push(format!("name mentions '{kw}'"));
                }
            }
            (score, in_text, i, ev)
        })
        .collect();
    scored.sort_by(|a, b| b.0.total_cmp(&a.0));
    let (best, best_in_text, best_i, best_ev) = scored[0].clone();
    if best <= 0.0 {
        return Classification {
            kind,
            category: None,
            doc_type: None,
            who,
            doc_date,
            date_source,
            confidence: 0.2,
            type_confidence: 0.0,
            evidence,
        };
    }
    let (category, doc_type, _) = DICTIONARY[best_i];
    let second = scored
        .iter()
        .skip(1)
        .find(|s| DICTIONARY[s.2].0 != category)
        .map_or(0.0, |s| s.0);
    let mut confidence = best / (best + second + 1.0);
    if !best_in_text {
        confidence = confidence.min(NAME_ONLY_CAP);
    }
    if let Some(w) = &who {
        evidence.push(format!("from {w}"));
        if best_in_text {
            confidence = (confidence + ORG_BONUS).min(1.0);
        }
    }
    evidence.splice(0..0, best_ev);
    Classification {
        kind,
        category: Some(category.to_string()),
        doc_type: Some(doc_type.to_string()),
        who,
        doc_date,
        date_source,
        confidence,
        type_confidence: confidence,
        evidence,
    }
}
