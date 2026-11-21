//! Content readers (spec §7.1): text from documents, EXIF from photos, tags from music. Pure
//! Rust, no network, nothing installed. Callers run these in isolation (a worker thread with a
//! timeout), because a malformed file can make a parser panic or spin.

use crate::error::{AegisError, Result};
use std::io::{BufReader, Read};
use std::path::Path;

/// Extracted text is cut at this many bytes.
pub const MAX_TEXT_BYTES: usize = 65_536;
/// Files bigger than this are not read.
pub const MAX_FILE_BYTES: u64 = 52_428_800;
/// Spreadsheets contribute at most this many non-empty cells.
const MAX_CELLS: usize = 200;

#[derive(Debug, Clone, Default, PartialEq)]
pub struct Exif {
    /// Date taken, ms since epoch, read as UTC.
    pub taken_at: Option<i64>,
    pub camera: Option<String>,
    pub has_gps: bool,
}

#[derive(Debug, Clone, Default, PartialEq)]
pub struct Tags {
    pub artist: Option<String>,
    pub album: Option<String>,
    pub title: Option<String>,
}

#[derive(Debug, Clone, Default, PartialEq)]
pub struct Extracted {
    pub text: Option<String>,
    pub exif: Option<Exif>,
    pub tags: Option<Tags>,
}

const TEXT_TYPES: [&str; 13] = [
    "pdf", "docx", "pptx", "odt", "xlsx", "xls", "ods", "txt", "md", "csv", "rtf", "odp", "xlsm",
];
const PHOTO_TYPES: [&str; 5] = ["jpg", "jpeg", "heic", "tif", "tiff"];
const AUDIO_TYPES: [&str; 3] = ["mp3", "m4a", "flac"];

/// True when Aegis can read something useful from files with this extension.
pub fn readable(ext: &str) -> bool {
    let e = ext.to_ascii_lowercase();
    TEXT_TYPES.contains(&e.as_str()) || PHOTO_TYPES.contains(&e.as_str()) || AUDIO_TYPES.contains(&e.as_str())
}

fn cap(mut s: String) -> String {
    if s.len() > MAX_TEXT_BYTES {
        let mut end = MAX_TEXT_BYTES;
        while !s.is_char_boundary(end) {
            end -= 1;
        }
        s.truncate(end);
    }
    s
}

fn collapse(s: &str) -> String {
    s.split_whitespace().collect::<Vec<_>>().join(" ")
}

/// Text of an XML part: paragraph ends become spaces, every tag is dropped, entities decoded.
fn xml_text(xml: &str) -> String {
    let mut out = String::with_capacity(xml.len() / 2);
    let mut in_tag = false;
    let mut tag = String::new();
    for c in xml.chars() {
        match (in_tag, c) {
            (false, '<') => {
                in_tag = true;
                tag.clear();
            }
            (true, '>') => {
                in_tag = false;
                // Paragraph and cell ends separate words.
                let t = tag.trim_start_matches('/');
                if tag.starts_with('/') && (t.ends_with(":p") || t == "p" || t.ends_with(":tc") || t.ends_with(":br"))
                    || t.ends_with(":tab")
                    || t.ends_with(":br/")
                {
                    out.push(' ');
                }
            }
            (true, c) => tag.push(c),
            (false, c) => out.push(c),
        }
    }
    let decoded = out
        .replace("&lt;", "<")
        .replace("&gt;", ">")
        .replace("&quot;", "\"")
        .replace("&apos;", "'")
        .replace("&amp;", "&");
    collapse(&decoded)
}

fn zip_parts_text(path: &Path, wanted: impl Fn(&str) -> bool) -> Result<String> {
    let f = std::fs::File::open(path)?;
    let mut z = zip::ZipArchive::new(BufReader::new(f)).map_err(|e| AegisError::InvalidInput(e.to_string()))?;
    let mut names: Vec<String> =
        z.file_names().filter_map(|n| n.ok().map(|c| c.into_owned())).filter(|n| wanted(n)).collect();
    names.sort_by_key(|n| {
        // slide2 before slide10
        let digits: String = n.chars().filter(|c| c.is_ascii_digit()).collect();
        (digits.parse::<u64>().unwrap_or(0), n.clone())
    });
    let mut text = String::new();
    for n in names {
        let mut part = z.by_name(&n).map_err(|e| AegisError::InvalidInput(e.to_string()))?;
        let mut xml = String::new();
        part.by_ref().take(8 * 1024 * 1024).read_to_string(&mut xml)?;
        text.push_str(&xml_text(&xml));
        text.push(' ');
        if text.len() > MAX_TEXT_BYTES {
            break;
        }
    }
    Ok(text.trim().to_string())
}

fn spreadsheet_text(path: &Path) -> Result<String> {
    use calamine::Reader;
    let mut wb = calamine::open_workbook_auto(path).map_err(|e| AegisError::InvalidInput(e.to_string()))?;
    let names = wb.sheet_names().to_vec();
    let mut parts: Vec<String> = names.clone();
    let mut cells = 0;
    for n in names {
        let Ok(range) = wb.worksheet_range(&n) else { continue };
        for row in range.rows() {
            for c in row {
                let s = c.to_string();
                if !s.trim().is_empty() {
                    parts.push(s);
                    cells += 1;
                    if cells >= MAX_CELLS {
                        return Ok(parts.join(" "));
                    }
                }
            }
        }
    }
    Ok(parts.join(" "))
}

/// RTF to plain text: control words, control symbols and braces removed.
fn rtf_text(raw: &str) -> String {
    let mut out = String::new();
    let mut chars = raw.chars().peekable();
    while let Some(c) = chars.next() {
        match c {
            '\\' => match chars.peek() {
                Some(&n) if n.is_ascii_alphabetic() => {
                    while chars.peek().is_some_and(|c| c.is_ascii_alphabetic()) {
                        chars.next();
                    }
                    if chars.peek() == Some(&'-') {
                        chars.next();
                    }
                    while chars.peek().is_some_and(|c| c.is_ascii_digit()) {
                        chars.next();
                    }
                    if chars.peek() == Some(&' ') {
                        chars.next();
                    }
                }
                Some(&'\'') => {
                    chars.next();
                    chars.next();
                    chars.next();
                }
                Some(&n) if n == '\\' || n == '{' || n == '}' => {
                    out.push(n);
                    chars.next();
                }
                _ => {
                    chars.next();
                }
            },
            '{' | '}' => {}
            '\r' | '\n' => out.push(' '),
            c => out.push(c),
        }
    }
    collapse(&out)
}

fn plain_text(path: &Path) -> Result<String> {
    let f = std::fs::File::open(path)?;
    let mut buf = Vec::new();
    f.take(MAX_TEXT_BYTES as u64).read_to_end(&mut buf)?;
    Ok(String::from_utf8_lossy(&buf).into_owned())
}

/// `"2024:03:14 10:22:05"` → ms since epoch (UTC).
fn exif_time(s: &str) -> Option<i64> {
    let s = s.trim().trim_end_matches('\0');
    let (date, time) = s.split_once(' ')?;
    let day = crate::engine::space::day_index(&date.replace(':', "-"))?;
    let mut hms = time.split(':').map(|p| p.parse::<i64>().ok());
    let (h, m, sec) = (hms.next()??, hms.next()??, hms.next()??);
    Some(((day * 86_400) + h * 3600 + m * 60 + sec) * 1000)
}

fn exif_of(path: &Path) -> Result<Exif> {
    let f = std::fs::File::open(path)?;
    let exif = exif::Reader::new()
        .read_from_container(&mut BufReader::new(f))
        .map_err(|e| AegisError::InvalidInput(e.to_string()))?;
    let ascii = |tag: exif::Tag| -> Option<String> {
        let field = exif.get_field(tag, exif::In::PRIMARY)?;
        match &field.value {
            exif::Value::Ascii(v) => v.first().map(|b| String::from_utf8_lossy(b).trim_end_matches('\0').trim().to_string()),
            _ => None,
        }
    };
    let taken_at = ascii(exif::Tag::DateTimeOriginal)
        .or_else(|| ascii(exif::Tag::DateTime))
        .and_then(|s| exif_time(&s));
    let camera = ascii(exif::Tag::Model).filter(|s| !s.is_empty());
    let has_gps = exif.get_field(exif::Tag::GPSLatitude, exif::In::PRIMARY).is_some();
    Ok(Exif { taken_at, camera, has_gps })
}

fn tags_of(path: &Path) -> Result<Tags> {
    use lofty::file::TaggedFileExt;
    use lofty::tag::Accessor;
    let tf = lofty::probe::Probe::open(path)
        .map_err(|e| AegisError::InvalidInput(e.to_string()))?
        .options(lofty::config::ParseOptions::new().read_properties(false))
        .read()
        .map_err(|e| AegisError::InvalidInput(e.to_string()))?;
    let Some(tag) = tf.primary_tag().or_else(|| tf.first_tag()) else { return Ok(Tags::default()) };
    let s = |v: Option<std::borrow::Cow<'_, str>>| v.map(|c| c.trim().to_string()).filter(|c| !c.is_empty());
    Ok(Tags { artist: s(tag.artist()), album: s(tag.album()), title: s(tag.title()) })
}

/// Reads what can be read from `path`, by extension.
pub fn extract(path: &Path) -> Result<Extracted> {
    let ext = path.extension().and_then(|e| e.to_str()).unwrap_or_default().to_ascii_lowercase();
    let mut out = Extracted::default();
    match ext.as_str() {
        "pdf" => {
            let bytes = std::fs::read(path)?;
            let text = pdf_extract::extract_text_from_mem(&bytes).map_err(|e| AegisError::InvalidInput(e.to_string()))?;
            out.text = Some(collapse(&text));
        }
        "docx" => out.text = Some(zip_parts_text(path, |n| n == "word/document.xml")?),
        "pptx" => out.text = Some(zip_parts_text(path, |n| n.starts_with("ppt/slides/slide") && n.ends_with(".xml"))?),
        "odt" | "odp" => out.text = Some(zip_parts_text(path, |n| n == "content.xml")?),
        "xlsx" | "xlsm" | "xls" | "ods" => out.text = Some(spreadsheet_text(path)?),
        "txt" | "md" | "csv" => out.text = Some(plain_text(path)?),
        "rtf" => out.text = Some(rtf_text(&plain_text(path)?)),
        e if PHOTO_TYPES.contains(&e) => out.exif = Some(exif_of(path)?),
        e if AUDIO_TYPES.contains(&e) => out.tags = Some(tags_of(path)?),
        _ => {}
    }
    out.text = out.text.map(cap).filter(|t| !t.trim().is_empty());
    Ok(out)
}
