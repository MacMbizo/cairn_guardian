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

// ---------- Task 3: reader ----------

use aegis_core::filing::reader::read_pending;
use aegis_core::scanner::scan_root;
use std::time::Duration;

/// A scanned scratch folder (so files are indexed) and its store.
fn scanned(build: impl FnOnce(&std::path::Path)) -> (tempfile::TempDir, SqliteStore, std::path::PathBuf) {
    let t = tempfile::TempDir::new().unwrap();
    let root = t.path().join("Docs");
    std::fs::create_dir_all(&root).unwrap();
    build(&root);
    let mut s = store();
    s.add_root(&root).unwrap();
    scan_root(&mut s, &root, &[1]).unwrap();
    (t, s, root)
}

fn file_id_of(s: &SqliteStore, name: &str) -> String {
    s.conn().query_row("SELECT id FROM files WHERE name = ?1", [name], |r| r.get(0)).unwrap()
}

#[test]
fn reader_stores_text_and_makes_it_searchable() {
    let (_t, mut s, _root) = scanned(|r| {
        fixtures_filing::docx(r, "inv.docx", "Vodacom tax invoice");
    });
    let rep = read_pending(&mut s, 100, Duration::from_secs(10)).unwrap();
    assert_eq!(rep.read, 1);
    let page = aegis_core::db::files::search_fts(s.conn(), "Vodacom", &Default::default(), 10).unwrap();
    assert_eq!(page.hits.len(), 1);
    let m = load_meta(s.conn(), &file_id_of(&s, "inv.docx")).unwrap().unwrap();
    assert!(m.read_at.is_some());
    assert_eq!(m.kind, Kind::Document);
}

#[test]
fn bad_pdf_is_marked_unreadable_and_reader_continues() {
    let (_t, mut s, _root) = scanned(|r| {
        common::put(r, "broken.pdf", b"%PDF-1.4 this is not really a pdf \x00\x01\x02 trailer garbage");
        fixtures_filing::docx(r, "good.docx", "Statement");
    });
    let rep = read_pending(&mut s, 100, Duration::from_secs(10)).unwrap();
    assert_eq!((rep.read, rep.unreadable), (1, 1));
    let bad = load_meta(s.conn(), &file_id_of(&s, "broken.pdf")).unwrap().unwrap();
    assert!(bad.read_error.is_some());
    assert!(bad.read_at.is_some(), "not retried until the file changes");
}

#[test]
fn unchanged_files_are_not_read_twice() {
    let (_t, mut s, _root) = scanned(|r| {
        fixtures_filing::docx(r, "a.docx", "hello");
    });
    read_pending(&mut s, 100, Duration::from_secs(10)).unwrap();
    assert_eq!(read_pending(&mut s, 100, Duration::from_secs(10)).unwrap().read, 0);
}

#[test]
fn big_files_are_skipped() {
    let (_t, mut s, _root) = scanned(|r| {
        fixtures_filing::docx(r, "huge.docx", "hello");
    });
    s.conn().execute("UPDATE files SET size = 60000000", []).unwrap();
    assert_eq!(read_pending(&mut s, 100, Duration::from_secs(10)).unwrap().read, 0);
    assert!(load_meta(s.conn(), &file_id_of(&s, "huge.docx")).unwrap().is_none());
}

#[test]
fn photo_exif_is_stored() {
    let (_t, mut s, _root) = scanned(|r| {
        fixtures_filing::jpeg_with_exif(r, "IMG_1.jpg", "2024:03:14 10:22:05", "Pixel 7");
    });
    read_pending(&mut s, 100, Duration::from_secs(10)).unwrap();
    let m = load_meta(s.conn(), &file_id_of(&s, "IMG_1.jpg")).unwrap().unwrap();
    assert_eq!(m.kind, Kind::Photo);
    assert_eq!(m.exif_taken_at, Some(1_710_411_725_000));
    assert_eq!(m.camera.as_deref(), Some("Pixel 7"));
}

// ---------- Task 4: classifier ----------

use aegis_core::filing::classify::{ClassifyInput, DateSource, classify, find_date, find_org, kind_of};

fn doc(name: &str, text: Option<&str>) -> aegis_core::filing::classify::Classification {
    classify(&ClassifyInput {
        name,
        ext: name.rsplit('.').next().unwrap_or(""),
        text,
        exif_taken_at: None,
        mtime: 1_700_000_000_000,
        in_atomic_unit: false,
        learned_orgs: &[],
    })
}

#[test]
fn tax_invoice_from_vodacom() {
    let c = doc(
        "scan0001.pdf",
        Some("TAX INVOICE Vodacom (Pty) Ltd Invoice date: 14/03/2026 VAT No 4190123456 Amount due R 499.00"),
    );
    assert_eq!(c.kind, Kind::Document);
    assert_eq!(c.category.as_deref(), Some("Finance/Invoices & receipts"));
    assert_eq!(c.doc_type.as_deref(), Some("invoice"));
    assert_eq!(c.who.as_deref(), Some("Vodacom"));
    assert_eq!(c.doc_date.as_deref(), Some("2026-03-14"));
    assert_eq!(c.date_source, DateSource::Content);
    assert!(c.confidence >= 0.90, "{}", c.confidence);
    assert!(c.evidence.iter().any(|e| e.contains("tax invoice")), "{:?}", c.evidence);
}

#[test]
fn sars_assessment() {
    let c = doc("ita34.pdf", Some("SARS Notice of Assessment Income Tax ITA34 Tax year 2025 Date of assessment 2025-09-30"));
    assert_eq!(c.category.as_deref(), Some("Finance/Tax"));
    assert_eq!(c.doc_type.as_deref(), Some("assessment"));
    assert_eq!(c.who.as_deref(), Some("SARS"));
    assert!(c.confidence >= 0.90);
}

#[test]
fn filename_only_is_capped() {
    let c = doc("payslip march.pdf", None);
    assert_eq!(c.category.as_deref(), Some("Finance/Payslips"));
    assert!(c.confidence <= 0.75, "{}", c.confidence);
}

#[test]
fn unclear_document_is_below_060() {
    let c = doc("misc.txt", Some("hello world notes"));
    assert!(c.confidence < 0.60, "{}", c.confidence);
}

#[test]
fn kinds() {
    assert_eq!(kind_of("dll", "a.dll", false), Kind::System);
    assert_eq!(kind_of("png", "Screenshot 2024-01-02 101010.png", false), Kind::Screenshot);
    assert_eq!(kind_of("rs", "main.rs", false), Kind::Code);
    assert_eq!(kind_of("txt", "x.txt", true), Kind::Code);
    assert_eq!(kind_of("exe", "setup.exe", false), Kind::Installer);
    assert_eq!(kind_of("jpg", "IMG_1.jpg", false), Kind::Photo);
}

#[test]
fn dates_parse_and_reject_impossible() {
    assert_eq!(find_date("due 31/02/2024 now"), None);
    assert_eq!(find_date("March 14, 2024").as_deref(), Some("2024-03-14"));
    assert_eq!(find_date("ref 20240314").as_deref(), Some("2024-03-14"));
    assert_eq!(find_date("on 14 March 2024").as_deref(), Some("2024-03-14"));
    assert_eq!(find_date("2024-03-14").as_deref(), Some("2024-03-14"));
    // The date next to "invoice date" wins over an earlier one.
    assert_eq!(find_date("printed 01/01/2026 invoice date: 14/03/2026").as_deref(), Some("2026-03-14"));
}

#[test]
fn orgs_match_on_word_boundaries_and_aliases() {
    assert_eq!(find_org("account at First National Bank", &[]).as_deref(), Some("FNB"));
    assert_eq!(find_org("the absa group", &[]).as_deref(), Some("ABSA"));
    assert_eq!(find_org("absolute value", &[]), None, "no match inside other words");
    assert_eq!(find_org("letter from Acme Widgets", &["Acme Widgets".to_string()]).as_deref(), Some("Acme Widgets"));
}

#[test]
fn photos_and_installers_are_confident_without_content() {
    let photo = classify(&ClassifyInput {
        name: "IMG_1.jpg",
        ext: "jpg",
        text: None,
        exif_taken_at: Some(1_710_411_725_000),
        mtime: 0,
        in_atomic_unit: false,
        learned_orgs: &[],
    });
    assert_eq!(photo.kind, Kind::Photo);
    assert_eq!(photo.doc_date.as_deref(), Some("2024-03-14"));
    assert_eq!(photo.date_source, DateSource::Exif);
    assert!(photo.confidence >= 0.90);
    assert!(doc("setup.exe", None).confidence >= 0.90);
}

#[test]
fn reader_classifies_what_it_reads() {
    let (_t, mut s, _root) = scanned(|r| {
        fixtures_filing::docx(r, "scan.docx", "TAX INVOICE Vodacom Invoice date: 14/03/2026 VAT Amount due");
    });
    read_pending(&mut s, 100, Duration::from_secs(10)).unwrap();
    let m = load_meta(s.conn(), &file_id_of(&s, "scan.docx")).unwrap().unwrap();
    assert_eq!(m.category.as_deref(), Some("Finance/Invoices & receipts"));
    assert_eq!(m.who.as_deref(), Some("Vodacom"));
    assert_eq!(m.doc_date.as_deref(), Some("2026-03-14"));
    assert!(m.confidence >= 0.90);
    assert!(!m.evidence.is_empty());
}

#[test]
fn unreadable_kinds_get_meta_without_reading() {
    let (_t, mut s, _root) = scanned(|r| {
        common::put(r, "setup.exe", b"MZ installer");
        common::put(r, "clip.mp4", b"not really a video");
    });
    let n = aegis_core::filing::reader::classify_unread(&mut s, 100).unwrap();
    assert_eq!(n, 2);
    assert_eq!(load_meta(s.conn(), &file_id_of(&s, "setup.exe")).unwrap().unwrap().kind, Kind::Installer);
    assert_eq!(load_meta(s.conn(), &file_id_of(&s, "clip.mp4")).unwrap().unwrap().kind, Kind::Video);
    assert_eq!(aegis_core::filing::reader::classify_unread(&mut s, 100).unwrap(), 0, "done once");
}

// ---------- Task 5: naming ----------

use aegis_core::filing::classify::Classification;
use aegis_core::filing::naming::{proposed_name, sanitize};

fn invoice_class() -> Classification {
    doc("scan0001.pdf", Some("TAX INVOICE Vodacom Invoice date: 14/03/2026 VAT Amount due R 499.00"))
}

#[test]
fn invoice_name() {
    assert_eq!(proposed_name(&invoice_class(), "scan0001", "pdf", 0).as_deref(), Some("2026-03-14 Vodacom invoice.pdf"));
}

#[test]
fn no_rename_when_date_only_from_mtime() {
    let mut c = invoice_class();
    c.date_source = DateSource::Modified;
    assert_eq!(proposed_name(&c, "scan0001", "pdf", 0), None);
}

#[test]
fn no_rename_below_080() {
    let mut c = invoice_class();
    c.type_confidence = 0.79;
    assert_eq!(proposed_name(&c, "scan0001", "pdf", 0), None);
}

#[test]
fn subject_used_when_no_organisation() {
    let mut c = invoice_class();
    c.who = None;
    c.doc_type = Some("lease".into());
    assert_eq!(
        proposed_name(&c, "lease_agreement_2026 (1)", "pdf", 0).as_deref(),
        Some("2026-03-14 Lease agreement.pdf")
    );
}

#[test]
fn screenshot_name() {
    let c = classify(&ClassifyInput {
        name: "Screenshot 2024-01-02 101010.png",
        ext: "png",
        text: None,
        exif_taken_at: None,
        mtime: 0,
        in_atomic_unit: false,
        learned_orgs: &[],
    });
    assert_eq!(
        proposed_name(&c, "Screenshot 2024-01-02 101010", "png", 0).as_deref(),
        Some("2024-01-02 10.10.10 Screenshot.png")
    );
}

#[test]
fn names_are_sanitised_and_capped() {
    let mut c = invoice_class();
    c.who = Some(format!("A<B>C:D|E\"F?G*H{}", "x".repeat(300)));
    let n = proposed_name(&c, "scan", "pdf", 0).unwrap();
    assert!(n.len() <= 120, "{}", n.len());
    assert!(!n.chars().any(|ch| "<>:\"/\\|?*".contains(ch) || ch.is_control()), "{n}");
    assert!(n.ends_with(".pdf"));
    assert_eq!(sanitize("  report.  "), "report");
}

#[test]
fn unchanged_name_is_not_proposed() {
    assert_eq!(proposed_name(&invoice_class(), "2026-03-14 Vodacom invoice", "pdf", 0), None);
}
