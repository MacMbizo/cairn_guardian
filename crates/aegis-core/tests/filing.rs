//! Auto-filing: reading, classification, naming, blueprints, batches, rules and search.

mod common;

use aegis_core::db::SqliteStore;
use aegis_core::filing::{BlueprintChoice, FileMeta, Kind, Scope, blueprint_of, load_meta, save_meta, set_blueprint};

fn store() -> SqliteStore {
    SqliteStore::open_in_memory().unwrap()
}

/// Inserts a bare files row so metadata has something to point at.
fn file_row(s: &SqliteStore, id: &str, path: &str) {
    s.conn()
        .execute(
            "INSERT INTO volumes (id, mount_point, fs_type, is_removable, is_readonly, has_stable_inode, case_sensitive, status, last_seen)
             VALUES ('v-test', 'C:/', 'ntfs', 0, 0, 1, 0, 'online', 0) ON CONFLICT(id) DO NOTHING",
            [],
        )
        .unwrap();
    let (norm, hash) = aegis_core::db::files::normalize_path(std::path::Path::new(path), false);
    s.conn()
        .execute(
            "INSERT INTO files (id, volume_id, path, path_hash, name, size, mtime, ctime, cloud_state, state, first_seen, last_seen, created_at, updated_at)
             VALUES (?1, 'v-test', ?2, ?3, ?4, 1, 0, 0, 'local', 'active', 0, 0, 0, 0)",
            rusqlite::params![id, norm, hash.to_vec(), std::path::Path::new(path).file_name().unwrap().to_string_lossy()],
        )
        .unwrap();
}

#[test]
fn meta_round_trips() {
    let s = store();
    file_row(&s, "f1", "C:/x/inv.pdf");
    let mut m = FileMeta {
        file_id: "f1".into(),
        kind: Kind::Document,
        category: Some("Finance/Invoices & receipts".into()),
        doc_type: Some("invoice".into()),
        who: Some("Vodacom".into()),
        doc_date: Some("2026-03-14".into()),
        title: None,
        exif_taken_at: None,
        camera: None,
        artist: None,
        album: None,
        confidence: 0.93,
        evidence: vec!["content mentions 'Tax invoice'".into()],
        read_at: Some(5),
        read_error: None,
    };
    save_meta(s.conn(), &m).unwrap();
    assert_eq!(load_meta(s.conn(), "f1").unwrap(), Some(m.clone()));
    m.confidence = 0.5;
    save_meta(s.conn(), &m).unwrap();
    assert_eq!(load_meta(s.conn(), "f1").unwrap().unwrap().confidence, 0.5);
    assert_eq!(load_meta(s.conn(), "nope").unwrap(), None);
}

#[test]
fn blueprint_choice_per_scope() {
    let s = store();
    let v1 = Scope::Volume { volume_id: "v1".into() };
    set_blueprint(s.conn(), &Scope::Pc, BlueprintChoice::Fresh, 1).unwrap();
    set_blueprint(s.conn(), &v1, BlueprintChoice::Tidy, 2).unwrap();
    assert_eq!(blueprint_of(s.conn(), &Scope::Pc).unwrap(), Some(BlueprintChoice::Fresh));
    assert_eq!(blueprint_of(s.conn(), &v1).unwrap(), Some(BlueprintChoice::Tidy));
    assert_eq!(blueprint_of(s.conn(), &Scope::Volume { volume_id: "v2".into() }).unwrap(), None);
}

// ---------- Task 2: extractors ----------

mod fixtures_filing;

use aegis_core::filing::extract::{MAX_TEXT_BYTES, extract, readable};

#[test]
fn docx_text_is_extracted() {
    let t = tempfile::TempDir::new().unwrap();
    let p = fixtures_filing::docx(t.path(), "a.docx", "Vodacom\nTax invoice\nAmount due R 499.00");
    let x = extract(&p).unwrap();
    assert!(x.text.unwrap().contains("Tax invoice"));
}

#[test]
fn xlsx_cells_are_extracted() {
    let t = tempfile::TempDir::new().unwrap();
    let p = fixtures_filing::xlsx(t.path(), "b.xlsx", &["Balance", "1234.50"]);
    let text = extract(&p).unwrap().text.unwrap();
    assert!(text.contains("Balance") && text.contains("1234.50"), "{text}");
}

#[test]
fn pdf_text_is_extracted() {
    let t = tempfile::TempDir::new().unwrap();
    let p = fixtures_filing::pdf(t.path(), "c.pdf", "Statement of account");
    let text = extract(&p).unwrap().text.unwrap();
    assert!(text.contains("Statement"), "{text:?}");
}

#[test]
fn exif_date_and_camera() {
    let t = tempfile::TempDir::new().unwrap();
    let p = fixtures_filing::jpeg_with_exif(t.path(), "d.jpg", "2024:03:14 10:22:05", "Pixel 7");
    let exif = extract(&p).unwrap().exif.unwrap();
    // 2024-03-14 10:22:05 read as UTC.
    assert_eq!(exif.taken_at, Some(1_710_411_725_000));
    assert_eq!(exif.camera.as_deref(), Some("Pixel 7"));
}

#[test]
fn mp3_tags() {
    let t = tempfile::TempDir::new().unwrap();
    let p = fixtures_filing::mp3_with_tags(t.path(), "e.mp3", "Miriam Makeba", "Pata Pata", "Pata Pata");
    let tags = extract(&p).unwrap().tags.unwrap();
    assert_eq!(tags.artist.as_deref(), Some("Miriam Makeba"));
    assert_eq!(tags.album.as_deref(), Some("Pata Pata"));
    assert_eq!(tags.title.as_deref(), Some("Pata Pata"));
}

#[test]
fn text_is_capped_at_64kb() {
    let t = tempfile::TempDir::new().unwrap();
    let p = common::put(t.path(), "big.txt", "words and more words ".repeat(10_000).as_bytes());
    assert!(extract(&p).unwrap().text.unwrap().len() <= MAX_TEXT_BYTES);
}

#[test]
fn rtf_control_words_stripped() {
    let t = tempfile::TempDir::new().unwrap();
    let p = common::put(t.path(), "f.rtf", br"{\rtf1\ansi Hello {\b world}}");
    assert_eq!(extract(&p).unwrap().text.unwrap().trim(), "Hello world");
}

#[test]
fn readable_types() {
    for e in ["pdf", "docx", "xlsx", "txt", "jpg", "mp3", "rtf"] {
        assert!(readable(e), "{e}");
    }
    assert!(!readable("exe") && !readable("mp4"));
}
