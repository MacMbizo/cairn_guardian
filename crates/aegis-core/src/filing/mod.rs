//! Auto-filing (spec 2026-11-21-auto-filing-design): reads what files contain, works out what
//! they are, proposes a sensible folder structure and names per drive, moves files in batches
//! the owner approves, and learns rules that file new arrivals by themselves.

use crate::error::Result;
use rusqlite::{Connection, OptionalExtension, params};
use serde::{Deserialize, Serialize};
use ts_rs::TS;

/// What sort of thing a file is.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize, TS)]
#[serde(rename_all = "snake_case")]
#[ts(export)]
pub enum Kind {
    Document,
    Photo,
    Screenshot,
    Video,
    Music,
    Audio,
    Code,
    Installer,
    Archive,
    Font,
    System,
    Other,
}

impl Kind {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Document => "document",
            Self::Photo => "photo",
            Self::Screenshot => "screenshot",
            Self::Video => "video",
            Self::Music => "music",
            Self::Audio => "audio",
            Self::Code => "code",
            Self::Installer => "installer",
            Self::Archive => "archive",
            Self::Font => "font",
            Self::System => "system",
            Self::Other => "other",
        }
    }

    pub fn from_db(s: &str) -> Kind {
        match s {
            "document" => Self::Document,
            "photo" => Self::Photo,
            "screenshot" => Self::Screenshot,
            "video" => Self::Video,
            "music" => Self::Music,
            "audio" => Self::Audio,
            "code" => Self::Code,
            "installer" => Self::Installer,
            "archive" => Self::Archive,
            "font" => Self::Font,
            "system" => Self::System,
            _ => Self::Other,
        }
    }
}

/// What Aegis read and concluded about one file.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, TS)]
#[ts(export)]
pub struct FileMeta {
    pub file_id: String,
    pub kind: Kind,
    pub category: Option<String>,
    pub doc_type: Option<String>,
    pub who: Option<String>,
    /// `YYYY-MM-DD`.
    pub doc_date: Option<String>,
    /// Where `doc_date` came from: only content, name and photo dates are trusted for naming.
    pub date_source: classify::DateSource,
    pub title: Option<String>,
    #[ts(type = "number | null")]
    pub exif_taken_at: Option<i64>,
    pub camera: Option<String>,
    pub artist: Option<String>,
    pub album: Option<String>,
    pub confidence: f64,
    pub evidence: Vec<String>,
    #[ts(type = "number | null")]
    pub read_at: Option<i64>,
    pub read_error: Option<String>,
}

impl FileMeta {
    /// Empty metadata for a file of `kind`.
    pub fn new(file_id: &str, kind: Kind) -> Self {
        Self {
            file_id: file_id.to_string(),
            kind,
            category: None,
            doc_type: None,
            who: None,
            doc_date: None,
            date_source: classify::DateSource::None,
            title: None,
            exif_taken_at: None,
            camera: None,
            artist: None,
            album: None,
            confidence: 0.0,
            evidence: Vec::new(),
            read_at: None,
            read_error: None,
        }
    }
}

pub fn save_meta(conn: &Connection, m: &FileMeta) -> Result<()> {
    conn.execute(
        "INSERT INTO file_meta (file_id, kind, category, doc_type, who, doc_date, title, exif_taken_at, camera,
                                artist, album, confidence, evidence_json, read_at, read_error, date_source)
         VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12, ?13, ?14, ?15, ?16)
         ON CONFLICT(file_id) DO UPDATE SET
            kind = excluded.kind, category = excluded.category, doc_type = excluded.doc_type,
            who = excluded.who, doc_date = excluded.doc_date, title = excluded.title,
            exif_taken_at = excluded.exif_taken_at, camera = excluded.camera, artist = excluded.artist,
            album = excluded.album, confidence = excluded.confidence, evidence_json = excluded.evidence_json,
            read_at = excluded.read_at, read_error = excluded.read_error, date_source = excluded.date_source",
        params![
            m.file_id,
            m.kind.as_str(),
            m.category,
            m.doc_type,
            m.who,
            m.doc_date,
            m.title,
            m.exif_taken_at,
            m.camera,
            m.artist,
            m.album,
            m.confidence,
            serde_json::to_string(&m.evidence)?,
            m.read_at,
            m.read_error,
            m.date_source.as_str()
        ],
    )?;
    Ok(())
}

pub fn load_meta(conn: &Connection, file_id: &str) -> Result<Option<FileMeta>> {
    let row = conn
        .query_row(
            "SELECT kind, category, doc_type, who, doc_date, title, exif_taken_at, camera, artist, album,
                    confidence, evidence_json, read_at, read_error, date_source
             FROM file_meta WHERE file_id = ?1",
            [file_id],
            |r| {
                let evidence: String = r.get(11)?;
                Ok(FileMeta {
                    file_id: file_id.to_string(),
                    kind: Kind::from_db(&r.get::<_, String>(0)?),
                    category: r.get(1)?,
                    doc_type: r.get(2)?,
                    who: r.get(3)?,
                    doc_date: r.get(4)?,
                    date_source: classify::DateSource::from_db(r.get::<_, Option<String>>(14)?.as_deref()),
                    title: r.get(5)?,
                    exif_taken_at: r.get(6)?,
                    camera: r.get(7)?,
                    artist: r.get(8)?,
                    album: r.get(9)?,
                    confidence: r.get(10)?,
                    evidence: serde_json::from_str(&evidence).unwrap_or_default(),
                    read_at: r.get(12)?,
                    read_error: r.get(13)?,
                })
            },
        )
        .optional()?;
    Ok(row)
}

/// Where filing applies: the PC's own folders, or one drive.
#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize, TS)]
#[serde(tag = "type", rename_all = "snake_case")]
#[ts(export)]
pub enum Scope {
    Pc,
    Volume { volume_id: String },
}

impl Scope {
    /// Stable key used in tables: `"pc"` or the volume id.
    pub fn key(&self) -> String {
        match self {
            Self::Pc => "pc".into(),
            Self::Volume { volume_id } => volume_id.clone(),
        }
    }
}

/// Which structure the owner chose for a scope.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "snake_case")]
#[ts(export)]
pub enum BlueprintChoice {
    /// The owner's own folders, cleaned up.
    Tidy,
    /// The standard structure.
    Fresh,
}

impl BlueprintChoice {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Tidy => "tidy",
            Self::Fresh => "fresh",
        }
    }
}

pub fn set_blueprint(conn: &Connection, scope: &Scope, choice: BlueprintChoice, now: i64) -> Result<()> {
    conn.execute(
        "INSERT INTO filing_blueprints (scope_key, choice, chosen_at) VALUES (?1, ?2, ?3)
         ON CONFLICT(scope_key) DO UPDATE SET choice = excluded.choice, chosen_at = excluded.chosen_at",
        params![scope.key(), choice.as_str(), now],
    )?;
    Ok(())
}

pub fn blueprint_of(conn: &Connection, scope: &Scope) -> Result<Option<BlueprintChoice>> {
    let c: Option<String> = conn
        .query_row("SELECT choice FROM filing_blueprints WHERE scope_key = ?1", [scope.key()], |r| r.get(0))
        .optional()?;
    Ok(c.map(|c| if c == "tidy" { BlueprintChoice::Tidy } else { BlueprintChoice::Fresh }))
}

pub mod extract;

pub mod classify;
pub mod reader;
pub mod orgs;
pub mod naming;
pub mod blueprint;
