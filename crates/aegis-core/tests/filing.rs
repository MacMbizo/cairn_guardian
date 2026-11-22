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
        date_source: aegis_core::filing::classify::DateSource::Content,
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
fn windows_reserved_names_are_refused() {
    for bad in ["CON", "con", "PRN", "AUX", "NUL", "COM1", "com9", "LPT1", "LPT9", "nul.txt", "Aux .mp3"] {
        let out = sanitize(bad);
        let stem = out.split('.').next().unwrap().trim().to_uppercase();
        let reserved = ["CON", "PRN", "AUX", "NUL"].contains(&stem.as_str())
            || ((stem.starts_with("COM") || stem.starts_with("LPT")) && stem.len() == 4 && stem.as_bytes()[3].is_ascii_digit());
        assert!(!reserved && !out.is_empty(), "{bad} -> {out}");
    }
    assert_eq!(sanitize("..."), "");
    assert_eq!(sanitize(" . . "), "");
    assert_eq!(sanitize("Console"), "Console");
    assert_eq!(sanitize("COM10"), "COM10");
}

#[test]
fn music_folders_from_reserved_or_dot_tags_are_safe() {
    let (_t, s, mount, scope) = drive_with(|m| {
        fixtures_filing::mp3_with_tags(&m.join("junk"), "a.mp3", "CON", "...", "Song");
    });
    let ps = plan(&s, &scope, BlueprintChoice::Fresh).unwrap();
    let to = rel(&mount, placement_for(&ps, "a.mp3").to.as_deref().unwrap());
    assert_eq!(to, "Music\\CON_\\Unknown album\\a.mp3");
}

#[test]
fn unchanged_name_is_not_proposed() {
    assert_eq!(proposed_name(&invoice_class(), "2026-03-14 Vodacom invoice", "pdf", 0), None);
}

// ---------- Task 6: blueprints ----------

use aegis_core::drives::FakeVolumeProvider;
use aegis_core::filing::blueprint::{Placement, Target, plan, summarize};
use aegis_core::filing::reader::classify_unread;

/// A catalogued fake drive whose files have been read and classified. Returns (tmp, store, mount, scope).
fn drive_with(build: impl FnOnce(&std::path::Path)) -> (tempfile::TempDir, SqliteStore, std::path::PathBuf, Scope) {
    let t = tempfile::TempDir::new().unwrap();
    let mut s = store();
    let provider = FakeVolumeProvider::new();
    let d = common::fake_drive(&t, "Data", "0007");
    build(&d.mount_point);
    let id = common::plug_and_catalog(&mut s, &provider, &d);
    read_pending(&mut s, 1_000, Duration::from_secs(10)).unwrap();
    classify_unread(&mut s, 10_000).unwrap();
    let mount = std::path::PathBuf::from(common::canon(&d.mount_point));
    (t, s, mount, Scope::Volume { volume_id: id })
}

fn placement_for<'a>(ps: &'a [Placement], file_name: &str) -> &'a Placement {
    ps.iter()
        .find(|p| matches!(p.target, Target::File { .. }) && p.from.ends_with(file_name))
        .unwrap_or_else(|| panic!("no placement for {file_name}: {ps:#?}"))
}

fn rel(mount: &std::path::Path, p: &str) -> String {
    let m = mount.to_string_lossy().to_string();
    p.strip_prefix(&m).unwrap_or(p).trim_start_matches(['\\', '/']).replace('/', "\\")
}

#[test]
fn fresh_places_documents_photos_and_installers() {
    let (_t, s, mount, scope) = drive_with(|m| {
        fixtures_filing::docx(&m.join("junk"), "scan0001.docx", "TAX INVOICE Vodacom Invoice date: 14/03/2026 VAT Amount due");
        fixtures_filing::jpeg_with_exif(&m.join("junk"), "IMG_1.jpg", "2024:03:14 10:22:05", "Pixel 7");
        common::put(&m.join("junk"), "setup.exe", b"MZ");
    });
    let ps = plan(&s, &scope, BlueprintChoice::Fresh).unwrap();
    let inv = placement_for(&ps, "scan0001.docx");
    assert_eq!(rel(&mount, inv.to.as_deref().unwrap()), "Documents\\Finance\\Invoices & receipts\\2026\\2026-03-14 Vodacom invoice.docx");
    assert_eq!(inv.new_name.as_deref(), Some("2026-03-14 Vodacom invoice.docx"));
    assert!(inv.confidence >= 0.90);
    let photo = placement_for(&ps, "IMG_1.jpg");
    assert_eq!(rel(&mount, photo.to.as_deref().unwrap()), "Photos\\2024\\2024-03\\IMG_1.jpg");
    let exe = placement_for(&ps, "setup.exe");
    assert_eq!(rel(&mount, exe.to.as_deref().unwrap()), "Software\\Installers\\setup.exe");
}

#[test]
fn code_projects_stay_whole_and_untouched() {
    let (_t, s, _mount, scope) = drive_with(|m| {
        std::fs::create_dir_all(m.join("projects").join("app").join(".git")).unwrap();
        common::put(&m.join("projects").join("app"), "main.rs", b"fn main(){}");
        fixtures_filing::docx(&m.join("projects").join("app"), "notes.docx", "TAX INVOICE Vodacom");
    });
    let ps = plan(&s, &scope, BlueprintChoice::Fresh).unwrap();
    assert!(
        !ps.iter().any(|p| matches!(p.target, Target::File { .. }) && p.from.contains("app")),
        "nothing inside a code project is placed on its own"
    );
    let unit = ps.iter().find(|p| matches!(p.target, Target::Group { .. }) && p.from.ends_with("app")).unwrap();
    assert_eq!(unit.to, None, "code projects stay where they are");
}

#[test]
fn course_folder_kept_together() {
    let (_t, s, mount, scope) = drive_with(|m| {
        let c = m.join("self development").join("Learn Rust");
        for i in 1..=6 {
            common::put(&c, &format!("{i:02} Lesson.mp4"), b"vid");
            common::put(&c, &format!("{i:02} Lesson.srt"), b"1");
        }
    });
    let ps = plan(&s, &scope, BlueprintChoice::Fresh).unwrap();
    let g = ps.iter().find(|p| matches!(p.target, Target::Group { .. }) && p.from.ends_with("Learn Rust")).unwrap();
    assert_eq!(rel(&mount, g.to.as_deref().unwrap()), "Videos\\Courses\\Learn Rust");
    assert!(!ps.iter().any(|p| matches!(p.target, Target::File { .. }) && p.from.contains("Learn Rust")));
}

#[test]
fn low_confidence_goes_to_sort() {
    let (_t, s, _mount, scope) = drive_with(|m| {
        common::put(m, "misc.txt", b"hello world notes");
    });
    let ps = plan(&s, &scope, BlueprintChoice::Fresh).unwrap();
    let p = placement_for(&ps, "misc.txt");
    assert!(p.to_sort);
    assert_eq!(p.to, None);
}

#[test]
fn tidy_merges_msidocuments_into_documents_and_files_loose_items() {
    let (_t, s, mount, scope) = drive_with(|m| {
        for i in 0..3 {
            common::put(&m.join("documents").join("letters"), &format!("l{i}.txt"), b"x");
        }
        fixtures_filing::docx(&m.join("msidocuments").join("letters"), "old.docx", "SARS Notice of Assessment Income Tax ITA34 Date of assessment 2025-09-30");
        fixtures_filing::docx(m, "scan9.docx", "TAX INVOICE Vodacom Invoice date: 14/03/2026 VAT Amount due");
    });
    let ps = plan(&s, &scope, BlueprintChoice::Tidy).unwrap();
    let old = placement_for(&ps, "old.docx");
    assert_eq!(rel(&mount, old.to.as_deref().unwrap()).to_lowercase(), "documents\\letters\\2025-09-30 sars assessment.docx");
    let loose = placement_for(&ps, "scan9.docx");
    assert_eq!(rel(&mount, loose.to.as_deref().unwrap()).to_lowercase(), "documents\\2026-03-14 vodacom invoice.docx");
    let rename = ps.iter().find(|p| matches!(p.target, Target::Group { .. }) && p.from.ends_with("documents")).unwrap();
    assert_eq!(rel(&mount, rename.to.as_deref().unwrap()), "Documents", "all-lowercase folder names become Title Case");
}

#[test]
fn pc_scope_maps_to_known_folders() {
    let t = tempfile::TempDir::new().unwrap();
    let home = t.path().join("me");
    for f in ["Documents", "Pictures", "Downloads"] {
        std::fs::create_dir_all(home.join(f)).unwrap();
    }
    fixtures_filing::jpeg_with_exif(&home.join("Downloads"), "IMG_9.jpg", "2023:07:01 09:00:00", "Pixel 7");
    let mut s = store();
    for f in ["Documents", "Pictures", "Downloads"] {
        s.add_root(&home.join(f)).unwrap();
        scan_root(&mut s, &home.join(f), &[1]).unwrap();
    }
    read_pending(&mut s, 100, Duration::from_secs(10)).unwrap();
    let ps = plan(&s, &Scope::Pc, BlueprintChoice::Fresh).unwrap();
    let photo = placement_for(&ps, "IMG_9.jpg");
    let pictures = common::canon(&home.join("Pictures"));
    assert_eq!(photo.to.as_deref().unwrap(), format!("{pictures}\\2023\\2023-07\\IMG_9.jpg"));
}

#[test]
fn backup_drive_is_refused() {
    let (_t, s, _mount, scope) = drive_with(|m| {
        common::put(m, "a.txt", b"x");
    });
    let Scope::Volume { volume_id } = &scope else { unreachable!() };
    s.conn().execute("UPDATE volumes SET role = 'backup' WHERE id = ?1", [volume_id]).unwrap();
    assert!(plan(&s, &scope, BlueprintChoice::Fresh).is_err());
}

#[test]
fn summary_counts_and_examples() {
    let (_t, s, _mount, scope) = drive_with(|m| {
        fixtures_filing::docx(m, "scan1.docx", "TAX INVOICE Vodacom Invoice date: 14/03/2026 VAT Amount due");
        common::put(m, "setup.exe", b"MZ");
        common::put(m, "misc.txt", b"hello world notes");
    });
    let ps = plan(&s, &scope, BlueprintChoice::Fresh).unwrap();
    let sum = summarize(&ps, BlueprintChoice::Fresh, &s).unwrap();
    assert_eq!((sum.moves, sum.renames, sum.to_sort), (2, 1, 1));
    assert!(sum.examples.len() <= 6 && !sum.examples.is_empty());
    let top: Vec<&str> = sum.tree.iter().map(|n| n.name.as_str()).collect();
    assert!(top.contains(&"Documents") && top.contains(&"Software"), "{top:?}");
}

// ---------- Task 8: rules ----------

use aegis_core::filing::rules::{Outcome, automatic_rule_for, delete_rule, list_rules, record_outcome, stop_automatic};

const INV: &str = "document|Finance/Invoices & receipts|Vodacom";
const DEST: &str = "Documents\\Finance\\Invoices & receipts\\{year}";

#[test]
fn three_approvals_graduate() {
    let s = store();
    let c = s.conn();
    for i in 1..=2 {
        let r = record_outcome(c, "v1", INV, DEST, true, Outcome::Approved, i).unwrap();
        assert_eq!(r.state, "asking");
        assert_eq!(r.approvals, i);
    }
    let r = record_outcome(c, "v1", INV, DEST, true, Outcome::Approved, 3).unwrap();
    assert_eq!(r.state, "automatic");
    assert_eq!(r.kind, Kind::Document);
    assert_eq!(r.who.as_deref(), Some("Vodacom"));
    assert!(automatic_rule_for(c, "v1", INV).unwrap().is_some());
}

#[test]
fn one_rejection_resets() {
    let s = store();
    let c = s.conn();
    for i in 1..=3 {
        record_outcome(c, "v1", INV, DEST, true, Outcome::Approved, i).unwrap();
    }
    let r = record_outcome(c, "v1", INV, DEST, true, Outcome::Rejected, 4).unwrap();
    assert_eq!((r.state.as_str(), r.approvals, r.rejections), ("asking", 0, 1));
    assert!(automatic_rule_for(c, "v1", INV).unwrap().is_none());
}

#[test]
fn undo_resets() {
    let s = store();
    let c = s.conn();
    for i in 1..=3 {
        record_outcome(c, "v1", INV, DEST, true, Outcome::Approved, i).unwrap();
    }
    record_outcome(c, "v1", INV, DEST, true, Outcome::Undone, 4).unwrap();
    assert!(automatic_rule_for(c, "v1", INV).unwrap().is_none());
}

#[test]
fn rule_learned_on_drive_not_automatic_on_another_until_approved_there() {
    let s = store();
    let c = s.conn();
    for i in 1..=3 {
        record_outcome(c, "v1", INV, DEST, true, Outcome::Approved, i).unwrap();
    }
    assert!(automatic_rule_for(c, "v2", INV).unwrap().is_none());
    assert!(automatic_rule_for(c, "pc", INV).unwrap().is_some(), "a drive's rules also apply on the PC");
    record_outcome(c, "v2", INV, DEST, true, Outcome::Approved, 5).unwrap();
    assert!(automatic_rule_for(c, "v2", INV).unwrap().is_some());
    assert_eq!(list_rules(c).unwrap().len(), 1, "one rule, several places");
}

#[test]
fn pc_rules_apply_to_pc_and_can_be_stopped_or_deleted() {
    let s = store();
    let c = s.conn();
    for i in 1..=3 {
        record_outcome(c, "pc", INV, DEST, true, Outcome::Approved, i).unwrap();
    }
    assert!(automatic_rule_for(c, "pc", INV).unwrap().is_some());
    assert!(automatic_rule_for(c, "v1", INV).unwrap().is_none());
    let id = list_rules(c).unwrap()[0].id.clone();
    stop_automatic(c, &id).unwrap();
    assert!(automatic_rule_for(c, "pc", INV).unwrap().is_none());
    delete_rule(c, &id).unwrap();
    assert!(list_rules(c).unwrap().is_empty());
}

// ---------- Task 7: batches ----------

use aegis_core::db::actions::{approve_batch, execute_batch, rollback_batch};
use aegis_core::filing::batches::{BATCH_MAX, after_execute, after_rollback, next_batch, open_filing_batch};

fn actions_of(s: &SqliteStore, batch: &str) -> Vec<(String, String, Option<String>, String)> {
    let mut st = s
        .conn()
        .prepare("SELECT id, from_path, to_path, status FROM actions WHERE batch_id = ?1 ORDER BY from_path")
        .unwrap();
    st.query_map([batch], |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?, r.get(3)?)))
        .unwrap()
        .map(|r| r.unwrap())
        .collect()
}

fn invoice_text() -> &'static str {
    "TAX INVOICE Vodacom Invoice date: 14/03/2026 VAT Amount due"
}

#[test]
fn first_batch_is_loose_root_items_capped_at_500() {
    let (_t, mut s, _mount, scope) = drive_with(|m| {
        for i in 0..620 {
            common::put(m, &format!("setup{i}.exe"), b"MZ");
        }
        common::put(&m.join("old").join("deep"), "tool.exe", b"MZ");
    });
    set_blueprint(s.conn(), &scope, BlueprintChoice::Fresh, 1).unwrap();
    let b = next_batch(&mut s, &scope).unwrap().unwrap();
    assert_eq!(actions_of(&s, &b).len(), BATCH_MAX);
    assert!(actions_of(&s, &b).iter().all(|a| !a.1.contains("deep")), "loose root items come first");
    assert_eq!(open_filing_batch(s.conn(), &scope).unwrap().as_deref(), Some(b.as_str()));
    assert!(next_batch(&mut s, &scope).unwrap().is_none(), "one batch at a time");
}

#[test]
fn no_batch_without_a_blueprint() {
    let (_t, mut s, _mount, scope) = drive_with(|m| {
        common::put(m, "setup.exe", b"MZ");
    });
    assert!(next_batch(&mut s, &scope).unwrap().is_none());
}

#[test]
fn same_target_name_in_one_batch_gets_suffix() {
    let (_t, mut s, _mount, scope) = drive_with(|m| {
        fixtures_filing::docx(m, "scan1.docx", invoice_text());
        fixtures_filing::docx(m, "scan2.docx", invoice_text());
    });
    set_blueprint(s.conn(), &scope, BlueprintChoice::Fresh, 1).unwrap();
    let b = next_batch(&mut s, &scope).unwrap().unwrap();
    let tos: Vec<String> = actions_of(&s, &b).into_iter().filter_map(|a| a.2).collect();
    assert_eq!(tos.len(), 2);
    assert_ne!(tos[0].to_lowercase(), tos[1].to_lowercase());
    assert!(tos.iter().any(|t| t.ends_with("2026-03-14 Vodacom invoice (2).docx")), "{tos:?}");
    approve_batch(&mut s, &b, None).unwrap();
    execute_batch(&mut s, &b).unwrap();
    after_execute(&mut s, &b).unwrap();
    assert!(actions_of(&s, &b).iter().all(|a| a.3 == "executed"));
}

#[test]
fn executed_batch_records_history_and_old_name_is_findable() {
    let (_t, mut s, mount, scope) = drive_with(|m| {
        fixtures_filing::docx(m, "scan1.docx", invoice_text());
    });
    set_blueprint(s.conn(), &scope, BlueprintChoice::Fresh, 1).unwrap();
    let b = next_batch(&mut s, &scope).unwrap().unwrap();
    approve_batch(&mut s, &b, None).unwrap();
    execute_batch(&mut s, &b).unwrap();
    after_execute(&mut s, &b).unwrap();
    let (old, new): (String, String) = s
        .conn()
        .query_row("SELECT old_path, new_path FROM file_history WHERE batch_id = ?1", [&b], |r| Ok((r.get(0)?, r.get(1)?)))
        .unwrap();
    assert!(old.ends_with("scan1.docx"));
    assert!(new.ends_with("2026-03-14 Vodacom invoice.docx"));
    assert!(std::path::Path::new(&new).is_file());
    assert!(!mount.join("scan1.docx").exists());
}

#[test]
fn moved_away_file_fails_alone() {
    let (_t, mut s, mount, scope) = drive_with(|m| {
        common::put(m, "a-setup.exe", b"MZ");
        common::put(m, "b-setup.exe", b"MZ");
    });
    set_blueprint(s.conn(), &scope, BlueprintChoice::Fresh, 1).unwrap();
    let b = next_batch(&mut s, &scope).unwrap().unwrap();
    std::fs::rename(mount.join("a-setup.exe"), mount.join("renamed-by-owner.exe")).unwrap();
    approve_batch(&mut s, &b, None).unwrap();
    execute_batch(&mut s, &b).unwrap();
    after_execute(&mut s, &b).unwrap();
    let acts = actions_of(&s, &b);
    assert_eq!(acts.iter().filter(|a| a.3 == "failed").count(), 1);
    assert_eq!(acts.iter().filter(|a| a.3 == "executed").count(), 1);
    let n: i64 = s.conn().query_row("SELECT COUNT(*) FROM file_history WHERE batch_id = ?1", [&b], |r| r.get(0)).unwrap();
    assert_eq!(n, 1);
}

#[test]
fn partially_executed_batch_records_only_done_moves() {
    let (_t, mut s, mount, scope) = drive_with(|m| {
        common::put(m, "a-setup.exe", b"MZ");
        common::put(m, "b-setup.exe", b"MZ");
    });
    set_blueprint(s.conn(), &scope, BlueprintChoice::Fresh, 1).unwrap();
    let b = next_batch(&mut s, &scope).unwrap().unwrap();
    std::fs::remove_file(mount.join("b-setup.exe")).unwrap();
    approve_batch(&mut s, &b, None).unwrap();
    execute_batch(&mut s, &b).unwrap();
    after_execute(&mut s, &b).unwrap();
    let n: i64 = s.conn().query_row("SELECT COUNT(*) FROM file_history WHERE batch_id = ?1", [&b], |r| r.get(0)).unwrap();
    assert_eq!(n, 1);
    rollback_batch(&mut s, &b).unwrap();
    after_rollback(&mut s, &b).unwrap();
    assert!(mount.join("a-setup.exe").is_file(), "the done move is undone");
}

#[test]
fn group_move_reindexes_children() {
    let (_t, mut s, _mount, scope) = drive_with(|m| {
        let c = m.join("self development").join("Learn Rust");
        for i in 1..=6 {
            common::put(&c, &format!("{i:02} Lesson.mp4"), b"vid");
            common::put(&c, &format!("{i:02} Lesson.srt"), b"1");
        }
    });
    set_blueprint(s.conn(), &scope, BlueprintChoice::Fresh, 1).unwrap();
    let b = next_batch(&mut s, &scope).unwrap().unwrap();
    approve_batch(&mut s, &b, None).unwrap();
    execute_batch(&mut s, &b).unwrap();
    after_execute(&mut s, &b).unwrap();
    let paths: Vec<String> = {
        let mut st = s.conn().prepare("SELECT path FROM files WHERE deleted_at IS NULL AND name LIKE '%Lesson%'").unwrap();
        st.query_map([], |r| r.get(0)).unwrap().map(|r| r.unwrap()).collect()
    };
    assert_eq!(paths.len(), 12, "{paths:?}");
    assert!(paths.iter().all(|p| p.contains("Courses")), "{paths:?}");
    let h: i64 = s.conn().query_row("SELECT COUNT(*) FROM file_history WHERE batch_id = ?1", [&b], |r| r.get(0)).unwrap();
    assert_eq!(h, 12, "every file in the course has a history row");
}

#[test]
fn unticked_item_counts_as_rejection_and_is_not_proposed_again() {
    let (_t, mut s, _mount, scope) = drive_with(|m| {
        common::put(m, "a-setup.exe", b"MZ");
        common::put(m, "b-setup.exe", b"MZ");
    });
    set_blueprint(s.conn(), &scope, BlueprintChoice::Fresh, 1).unwrap();
    let b = next_batch(&mut s, &scope).unwrap().unwrap();
    let acts = actions_of(&s, &b);
    approve_batch(&mut s, &b, Some(&[acts[0].0.clone()])).unwrap();
    execute_batch(&mut s, &b).unwrap();
    after_execute(&mut s, &b).unwrap();
    assert_eq!(actions_of(&s, &b)[1].3, "rejected");
    let rule = list_rules(s.conn()).unwrap().into_iter().find(|r| r.kind == Kind::Installer).unwrap();
    assert_eq!((rule.approvals, rule.rejections), (0, 1));
    assert!(next_batch(&mut s, &scope).unwrap().is_none(), "the owner said leave it");
}

#[test]
fn rollback_counts_as_undo() {
    let (_t, mut s, _mount, scope) = drive_with(|m| {
        common::put(m, "a-setup.exe", b"MZ");
    });
    set_blueprint(s.conn(), &scope, BlueprintChoice::Fresh, 1).unwrap();
    let b = next_batch(&mut s, &scope).unwrap().unwrap();
    approve_batch(&mut s, &b, None).unwrap();
    execute_batch(&mut s, &b).unwrap();
    after_execute(&mut s, &b).unwrap();
    assert_eq!(list_rules(s.conn()).unwrap()[0].approvals, 1);
    rollback_batch(&mut s, &b).unwrap();
    after_rollback(&mut s, &b).unwrap();
    let r = &list_rules(s.conn()).unwrap()[0];
    assert_eq!((r.approvals, r.rejections), (0, 1));
    let n: i64 = s.conn().query_row("SELECT COUNT(*) FROM file_history", [], |r| r.get(0)).unwrap();
    assert_eq!(n, 0);
}

#[test]
fn tidy_does_not_rename_folders_holding_code() {
    let (_t, s, _mount, scope) = drive_with(|m| {
        std::fs::create_dir_all(m.join("projectsv2").join("app").join(".git")).unwrap();
        common::put(&m.join("projectsv2").join("app"), "main.rs", b"fn main(){}");
    });
    let ps = plan(&s, &scope, BlueprintChoice::Tidy).unwrap();
    assert!(
        !ps.iter().any(|p| p.from.ends_with("projectsv2") && p.to.is_some()),
        "renaming a folder that holds code would break the code's paths"
    );
}

// ---------- Task 9: filing watch ----------

use aegis_core::engine::Watch;
use aegis_core::filing::watch::FilingWatch;

fn age(path: &std::path::Path, days: f64) {
    let f = std::fs::OpenOptions::new().write(true).open(path).unwrap();
    f.set_modified(std::time::SystemTime::now() - std::time::Duration::from_secs_f64(days * 86_400.0)).unwrap();
}

const INVOICE_RULE: &str = "document|Finance/Invoices & receipts|Vodacom";

/// PC folders Documents + Downloads, an aged invoice in Downloads, read and classified.
fn pc_with_invoice(aged: bool) -> (tempfile::TempDir, SqliteStore, std::path::PathBuf) {
    let t = tempfile::TempDir::new().unwrap();
    let home = t.path().join("me");
    std::fs::create_dir_all(home.join("Documents")).unwrap();
    let p = fixtures_filing::docx(&home.join("Downloads"), "scan77.docx", invoice_text());
    if aged {
        age(&p, 1.0);
    }
    let mut s = store();
    for f in ["Documents", "Downloads"] {
        s.add_root(&home.join(f)).unwrap();
        scan_root(&mut s, &home.join(f), &[1]).unwrap();
    }
    read_pending(&mut s, 100, Duration::from_secs(10)).unwrap();
    (t, s, home)
}

/// The owner has approved filing on the PC before.
fn approved_before(s: &SqliteStore) {
    s.conn()
        .execute(
            "INSERT INTO action_batches (id, origin, status, total_actions, created_at) VALUES ('prev', 'filing:pc', 'executed', 0, 0)",
            [],
        )
        .unwrap();
}

fn later() -> i64 {
    aegis_core::now_ms() + 20 * 60_000
}

#[test]
fn new_invoice_in_downloads_files_itself_when_rule_is_automatic() {
    let (_t, mut s, home) = pc_with_invoice(true);
    set_blueprint(s.conn(), &Scope::Pc, BlueprintChoice::Fresh, 1).unwrap();
    approved_before(&s);
    for i in 1..=3 {
        record_outcome(s.conn(), "pc", INVOICE_RULE, "Finance", true, Outcome::Approved, i).unwrap();
    }
    FilingWatch::default().run(&mut s, later()).unwrap();
    let dest = home.join("Documents").join("Finance").join("Invoices & receipts").join("2026").join("2026-03-14 Vodacom invoice.docx");
    assert!(dest.is_file(), "filed by itself");
    assert!(!home.join("Downloads").join("scan77.docx").exists());
    let n: i64 = s.conn().query_row("SELECT COUNT(*) FROM file_history", [], |r| r.get(0)).unwrap();
    assert_eq!(n, 1);
}

#[test]
fn waits_for_settle_time() {
    let (_t, mut s, home) = pc_with_invoice(false);
    set_blueprint(s.conn(), &Scope::Pc, BlueprintChoice::Fresh, 1).unwrap();
    approved_before(&s);
    for i in 1..=3 {
        record_outcome(s.conn(), "pc", INVOICE_RULE, "Finance", true, Outcome::Approved, i).unwrap();
    }
    FilingWatch::default().run(&mut s, aegis_core::now_ms()).unwrap();
    assert!(home.join("Downloads").join("scan77.docx").is_file());
}

#[test]
fn without_blueprint_nothing_happens() {
    let (_t, mut s, home) = pc_with_invoice(true);
    approved_before(&s);
    let f = FilingWatch::default().run(&mut s, later()).unwrap();
    assert!(f.is_empty());
    assert!(home.join("Downloads").join("scan77.docx").is_file());
    assert!(open_filing_batch(s.conn(), &Scope::Pc).unwrap().is_none());
}

#[test]
fn confident_but_not_automatic_is_proposed_not_moved() {
    let (_t, mut s, home) = pc_with_invoice(true);
    set_blueprint(s.conn(), &Scope::Pc, BlueprintChoice::Fresh, 1).unwrap();
    approved_before(&s);
    let f = FilingWatch::default().run(&mut s, later()).unwrap();
    assert!(home.join("Downloads").join("scan77.docx").is_file(), "only proposed");
    let b = open_filing_batch(s.conn(), &Scope::Pc).unwrap().expect("a batch waits for the owner");
    assert_eq!(actions_of(&s, &b).len(), 1);
    assert_eq!(f.len(), 1);
    assert_eq!(f[0].level, aegis_core::engine::Level::Note);
    assert_eq!(f[0].title, "1 new file ready to file");
}

#[test]
fn note_counts_auto_filed() {
    let (_t, mut s, _home) = pc_with_invoice(true);
    set_blueprint(s.conn(), &Scope::Pc, BlueprintChoice::Fresh, 1).unwrap();
    approved_before(&s);
    for i in 1..=3 {
        record_outcome(s.conn(), "pc", INVOICE_RULE, "Finance", true, Outcome::Approved, i).unwrap();
    }
    FilingWatch::default().run(&mut s, later()).unwrap();
    let note = aegis_core::engine::note::build(s.conn(), "2026-11-22", 0, "2026-11-22", later()).unwrap();
    assert!(note.did.contains(&"Filed 1 file by itself".to_string()), "{:?}", note.did);
}

// ---------- Task 10: search by name and former name ----------

use aegis_core::contracts::SearchFilters;
use aegis_core::db::files::search_fts;

#[test]
fn finds_by_name_without_content() {
    let (_t, s, _root) = scanned(|r| {
        common::put(r, "Holiday budget plan.bin", b"\x00\x01");
    });
    let page = search_fts(s.conn(), "holiday budget", &SearchFilters::default(), 10).unwrap();
    assert_eq!(page.hits.len(), 1);
    assert_eq!(page.hits[0].match_reason, "name");
}

#[test]
fn finds_by_former_name_after_filing() {
    let (_t, mut s, _mount, scope) = drive_with(|m| {
        fixtures_filing::docx(m, "scan1.docx", invoice_text());
    });
    set_blueprint(s.conn(), &scope, BlueprintChoice::Fresh, 1).unwrap();
    let b = next_batch(&mut s, &scope).unwrap().unwrap();
    approve_batch(&mut s, &b, None).unwrap();
    execute_batch(&mut s, &b).unwrap();
    after_execute(&mut s, &b).unwrap();
    let page = search_fts(s.conn(), "scan1", &SearchFilters::default(), 10).unwrap();
    assert_eq!(page.hits.len(), 1);
    assert_eq!(page.hits[0].match_reason, "former name");
    assert!(page.hits[0].name.contains("Vodacom invoice"));
    assert!(page.hits[0].was.as_deref().unwrap().ends_with("scan1.docx"));
}

#[test]
fn content_hit_ranks_first() {
    let (_t, mut s, _root) = scanned(|r| {
        fixtures_filing::docx(r, "a.docx", "the zebra crossing report");
        common::put(r, "zebra notes.bin", b"\x00");
    });
    read_pending(&mut s, 100, Duration::from_secs(10)).unwrap();
    let page = search_fts(s.conn(), "zebra", &SearchFilters::default(), 10).unwrap();
    assert_eq!(page.hits.len(), 2);
    assert_eq!(page.hits[0].match_reason, "content");
    assert_eq!(page.hits[1].match_reason, "name");
}

#[test]
fn filters_by_category_who_and_year() {
    let (_t, mut s, _root) = scanned(|r| {
        fixtures_filing::docx(r, "inv.docx", invoice_text());
        fixtures_filing::docx(r, "ita.docx", "SARS Notice of Assessment Income Tax ITA34 Date of assessment 2025-09-30");
    });
    read_pending(&mut s, 100, Duration::from_secs(10)).unwrap();
    let f = |category: Option<&str>, who: Option<&str>, year: Option<i32>| SearchFilters {
        category: category.map(str::to_string),
        who: who.map(str::to_string),
        year,
        ..Default::default()
    };
    let names = |filters: SearchFilters| -> Vec<String> {
        search_fts(s.conn(), "tax", &filters, 10).unwrap().hits.into_iter().map(|h| h.name).collect()
    };
    assert_eq!(names(f(None, None, None)).len(), 2);
    assert_eq!(names(f(Some("Finance/Tax"), None, None)), vec!["ita.docx"]);
    assert_eq!(names(f(Some("Finance"), Some("vodacom"), None)), vec!["inv.docx"]);
    assert_eq!(names(f(None, None, Some(2025))), vec!["ita.docx"]);
}

// ---------- Task 11: scope statuses ----------

#[test]
fn scope_statuses_list_pc_and_general_drives_not_backup() {
    let (_t, mut s, _mount, scope) = drive_with(|m| {
        common::put(m, "setup.exe", b"MZ");
    });
    set_blueprint(s.conn(), &scope, BlueprintChoice::Fresh, 1).unwrap();
    let st = aegis_core::filing::scope_statuses(&mut s).unwrap();
    assert_eq!(st.len(), 2, "{st:?}");
    assert_eq!(st[0].scope, Scope::Pc);
    assert_eq!(st[0].label, "This PC");
    let d = &st[1];
    assert_eq!(d.label, "Data");
    assert_eq!(d.blueprint, Some(BlueprintChoice::Fresh));
    assert_eq!(d.remaining, 1);
    let Scope::Volume { volume_id } = &scope else { unreachable!() };
    s.conn().execute("UPDATE volumes SET role = 'backup' WHERE id = ?1", [volume_id]).unwrap();
    assert_eq!(aegis_core::filing::scope_statuses(&mut s).unwrap().len(), 1);
}

#[test]
fn move_only_examples_show_the_destination_folder() {
    let (_t, s, _mount, scope) = drive_with(|m| {
        common::put(&m.join("junk"), "setup.exe", b"MZ");
    });
    let ps = plan(&s, &scope, BlueprintChoice::Fresh).unwrap();
    let sum = summarize(&ps, BlueprintChoice::Fresh, &s).unwrap();
    assert_eq!(sum.examples, vec![("setup.exe".to_string(), "Software\\Installers\\setup.exe".to_string())]);
}

// ---------- Task 12: dry-run findings ----------

#[test]
fn technical_notes_with_a_few_keywords_are_not_confident() {
    // A config document that mentions tenants and rentals is not a lease.
    let c = doc(
        "E1_environments_and_configuration_matrix.md",
        Some("Environments matrix. Each tenant gets its own database. Rental of GPU servers is billed monthly. Deployment notes follow."),
    );
    assert!(c.confidence < 0.60, "{} {:?}", c.confidence, c.category);
}

#[test]
fn sender_must_appear_near_the_top_or_in_the_name() {
    let body = format!(
        "Curriculum Vitae MacDonald\nWork experience as a project manager and administrator.\n{}\nSkills: Microsoft Word, Excel, Google Workspace.",
        "Responsible for budgets, procurement and reporting. ".repeat(40)
    );
    let c = doc("cover letter.docx", Some(&body));
    assert_eq!(c.who, None, "a product named deep in a CV is not who it is from");
    let c = doc("scan.pdf", Some("TAX INVOICE Vodacom (Pty) Ltd Invoice date: 14/03/2026 VAT Amount due"));
    assert_eq!(c.who.as_deref(), Some("Vodacom"));
}

#[test]
fn notes_and_data_files_are_never_renamed() {
    let mut c = invoice_class();
    for ext in ["md", "txt", "csv"] {
        c.kind = Kind::Document;
        assert_eq!(proposed_name(&c, "27_mcc_glossary_of_terms", ext, 0), None, "{ext}");
    }
}

#[test]
fn screenshot_time_comes_after_the_date() {
    let c = classify(&ClassifyInput {
        name: "Screenshot 2021-08-25 at 00-01-12 Inventory Management System DFD.png",
        ext: "png",
        text: None,
        exif_taken_at: None,
        mtime: 0,
        in_atomic_unit: false,
        learned_orgs: &[],
    });
    assert_eq!(
        proposed_name(&c, "Screenshot 2021-08-25 at 00-01-12 Inventory Management System DFD", "png", 0).as_deref(),
        Some("2021-08-25 00.01.12 Screenshot.png")
    );
}

#[test]
fn files_on_unplugged_drives_wait_to_be_read() {
    let (_t, mut s, mount, scope) = drive_with(|m| {
        fixtures_filing::docx(m, "later.docx", "Vodacom tax invoice");
    });
    // Forget what was read, then unplug the drive.
    s.conn().execute("DELETE FROM file_meta", []).unwrap();
    let Scope::Volume { volume_id } = &scope else { unreachable!() };
    s.conn().execute("UPDATE volumes SET status = 'offline' WHERE id = ?1", [volume_id]).unwrap();
    let rep = read_pending(&mut s, 100, Duration::from_secs(10)).unwrap();
    assert_eq!((rep.read, rep.unreadable), (0, 0), "nothing attempted while unplugged");
    // A file that vanished from a plugged-in drive isn't marked unreadable either.
    s.conn().execute("UPDATE volumes SET status = 'online' WHERE id = ?1", [volume_id]).unwrap();
    std::fs::remove_file(mount.join("later.docx")).unwrap();
    let rep = read_pending(&mut s, 100, Duration::from_secs(10)).unwrap();
    assert_eq!(rep.unreadable, 0);
    let id: String = s.conn().query_row("SELECT id FROM files WHERE name = 'later.docx'", [], |r| r.get(0)).unwrap();
    assert!(load_meta(s.conn(), &id).unwrap().is_none_or(|m| m.read_at.is_none()));
}

#[test]
fn scope_counts_are_cached_and_kept_current_by_batches() {
    let (_t, mut s, _mount, scope) = drive_with(|m| {
        common::put(m, "a-setup.exe", b"MZ");
        common::put(m, "b-setup.exe", b"MZ");
    });
    set_blueprint(s.conn(), &scope, BlueprintChoice::Fresh, 1).unwrap();
    let drive = |s: &mut SqliteStore| aegis_core::filing::scope_statuses(s).unwrap().into_iter().find(|x| x.scope == scope).unwrap();
    assert_eq!(drive(&mut s).remaining, 2);
    let cached: Option<String> = s
        .conn()
        .query_row("SELECT value_json FROM settings WHERE key LIKE 'filing.counts:%'", [], |r| r.get(0))
        .ok();
    assert!(cached.is_some(), "counts are cached, not re-planned on every visit");
    let b = next_batch(&mut s, &scope).unwrap().unwrap();
    approve_batch(&mut s, &b, None).unwrap();
    execute_batch(&mut s, &b).unwrap();
    after_execute(&mut s, &b).unwrap();
    assert_eq!(drive(&mut s).remaining, 0);
    // Choosing another structure starts the count afresh.
    set_blueprint(s.conn(), &scope, BlueprintChoice::Tidy, 2).unwrap();
    let none: Option<String> = s
        .conn()
        .query_row("SELECT value_json FROM settings WHERE key LIKE 'filing.counts:%'", [], |r| r.get(0))
        .ok();
    assert!(none.is_none());
}

// ---------- Final review fixes ----------

use aegis_core::filing::batches::skip_batch;

fn run_batch(s: &mut SqliteStore, scope: &Scope) -> Option<String> {
    let b = next_batch(s, scope).unwrap()?;
    approve_batch(s, &b, None).unwrap();
    execute_batch(s, &b).unwrap();
    after_execute(s, &b).unwrap();
    Some(b)
}

fn top_level(mount: &std::path::Path) -> Vec<String> {
    let mut v: Vec<String> = std::fs::read_dir(mount)
        .unwrap()
        .map(|e| e.unwrap().file_name().to_string_lossy().to_string())
        .filter(|n| !n.starts_with('.'))
        .collect();
    v.sort();
    v
}

#[test]
fn title_case_rename_really_renames_the_folder() {
    let (_t, mut s, mount, scope) = drive_with(|m| {
        common::put(&m.join("documents").join("letters"), "l1.txt", b"x");
    });
    set_blueprint(s.conn(), &scope, BlueprintChoice::Tidy, 1).unwrap();
    while run_batch(&mut s, &scope).is_some() {}
    assert_eq!(top_level(&mount), vec!["Documents"]);
    assert!(mount.join("Documents").join("letters").join("l1.txt").is_file());
}

#[test]
fn undo_with_a_conflict_keeps_history_for_what_stayed_moved() {
    let (_t, mut s, mount, scope) = drive_with(|m| {
        common::put(m, "a-setup.exe", b"MZ a");
        common::put(m, "b-setup.exe", b"MZ b");
    });
    set_blueprint(s.conn(), &scope, BlueprintChoice::Fresh, 1).unwrap();
    let b = run_batch(&mut s, &scope).unwrap();
    // The owner downloads a new a-setup.exe where the old one was.
    common::put(&mount, "a-setup.exe", b"MZ new");
    let res = rollback_batch(&mut s, &b).unwrap();
    after_rollback(&mut s, &b).unwrap();
    assert!(!res.conflicts.is_empty(), "the new download blocks undoing a-setup.exe: {res:?}");
    // Every file that is still away from its old place keeps its "was" record.
    let still_moved: i64 = s
        .conn()
        .query_row(
            "SELECT COUNT(*) FROM files WHERE deleted_at IS NULL AND name LIKE '%setup.exe' AND path LIKE '%Installers%'",
            [],
            |r| r.get(0),
        )
        .unwrap();
    let left: i64 = s.conn().query_row("SELECT COUNT(*) FROM file_history WHERE batch_id = ?1", [&b], |r| r.get(0)).unwrap();
    assert!(still_moved >= 1);
    assert_eq!(left, still_moved, "history kept exactly for files still moved");
}

#[test]
fn skipping_a_batch_sends_its_rules_back_to_asking() {
    let (_t, mut s, _mount, scope) = drive_with(|m| {
        common::put(m, "a-setup.exe", b"MZ");
    });
    for i in 1..=3 {
        record_outcome(s.conn(), &scope.key(), "installer|-|-", "Software\\Installers", false, Outcome::Approved, i).unwrap();
    }
    set_blueprint(s.conn(), &scope, BlueprintChoice::Fresh, 1).unwrap();
    let b = next_batch(&mut s, &scope).unwrap().unwrap();
    skip_batch(&mut s, &b).unwrap();
    let r = list_rules(s.conn()).unwrap().into_iter().find(|r| r.rule_key == "installer|-|-").unwrap();
    assert_eq!(r.state, "asking");
    assert!(next_batch(&mut s, &scope).unwrap().is_none(), "skipped files are not offered again");
}

#[test]
fn one_batch_counts_as_one_approval_per_rule() {
    let (_t, mut s, _mount, scope) = drive_with(|m| {
        for i in 0..4 {
            common::put(m, &format!("s{i}-setup.exe"), b"MZ");
        }
    });
    set_blueprint(s.conn(), &scope, BlueprintChoice::Fresh, 1).unwrap();
    run_batch(&mut s, &scope).unwrap();
    let r = list_rules(s.conn()).unwrap().into_iter().find(|r| r.rule_key == "installer|-|-").unwrap();
    assert_eq!((r.approvals, r.state.as_str()), (1, "asking"), "three decisions, not three files");
}

#[test]
fn tidy_merge_leaves_unsure_files_and_never_loops() {
    let (_t, mut s, _mount, scope) = drive_with(|m| {
        for i in 0..3 {
            common::put(&m.join("documents").join("letters"), &format!("l{i}.txt"), b"x");
        }
        common::put(&m.join("msidocuments").join("letters"), "old.txt", b"x");
    });
    set_blueprint(s.conn(), &scope, BlueprintChoice::Tidy, 1).unwrap();
    let ps = plan(&s, &scope, BlueprintChoice::Tidy).unwrap();
    let old = placement_for(&ps, "old.txt");
    assert!(old.to.is_none() || old.confidence >= 0.60, "unsure files are never moved");
    let mut rounds = 0;
    while run_batch(&mut s, &scope).is_some() {
        rounds += 1;
        assert!(rounds < 5, "the same batch keeps coming back");
    }
}

#[test]
fn filed_courses_are_not_moved_into_themselves() {
    let (_t, mut s, _mount, scope) = drive_with(|m| {
        for course in ["Learn Rust", "Learn Go"] {
            let c = m.join("self development").join(course);
            for i in 1..=6 {
                common::put(&c, &format!("{i:02} Lesson.mp4"), b"vid");
                common::put(&c, &format!("{i:02} Lesson.srt"), b"1");
            }
        }
    });
    set_blueprint(s.conn(), &scope, BlueprintChoice::Fresh, 1).unwrap();
    let mut rounds = 0;
    while run_batch(&mut s, &scope).is_some() {
        rounds += 1;
        assert!(rounds < 5, "batches keep coming back");
    }
    let ps = plan(&s, &scope, BlueprintChoice::Fresh).unwrap();
    assert!(
        !ps.iter().any(|p| p.to.as_deref().is_some_and(|t| t.to_lowercase().starts_with(&format!("{}\\", p.from.to_lowercase())))),
        "no group targets a place inside itself"
    );
}

#[test]
fn a_course_holding_a_code_project_stays_put() {
    let (_t, s, _mount, scope) = drive_with(|m| {
        let c = m.join("self development").join("Rust Course");
        for i in 1..=6 {
            common::put(&c, &format!("{i:02} Lesson.mp4"), b"vid");
        }
        std::fs::create_dir_all(c.join("exercises").join(".git")).unwrap();
        common::put(&c.join("exercises"), "main.rs", b"fn main(){}");
    });
    let ps = plan(&s, &scope, BlueprintChoice::Fresh).unwrap();
    let g = ps.iter().find(|p| matches!(p.target, Target::Group { .. }) && p.from.ends_with("Rust Course")).unwrap();
    assert_eq!(g.to, None, "moving it would move the code project inside");
}

#[test]
fn search_finds_aegis_style_dates() {
    let (_t, mut s, _mount, scope) = drive_with(|m| {
        fixtures_filing::docx(m, "scan1.docx", invoice_text());
    });
    set_blueprint(s.conn(), &scope, BlueprintChoice::Fresh, 1).unwrap();
    run_batch(&mut s, &scope).unwrap();
    for q in ["2026-03-14", "2026-03-14 Vodacom"] {
        assert_eq!(search_fts(s.conn(), q, &SearchFilters::default(), 10).unwrap().hits.len(), 1, "{q}");
    }
    let by_kind = SearchFilters { kind: Some("document".into()), ..Default::default() };
    assert_eq!(search_fts(s.conn(), "Vodacom", &by_kind, 10).unwrap().hits.len(), 1);
    let by_other = SearchFilters { kind: Some("photo".into()), ..Default::default() };
    assert!(search_fts(s.conn(), "Vodacom", &by_other, 10).unwrap().hits.is_empty());
    let Scope::Volume { volume_id } = &scope else { unreachable!() };
    let by_drive = SearchFilters { volume_id: Some(volume_id.clone()), ..Default::default() };
    assert_eq!(search_fts(s.conn(), "Vodacom", &by_drive, 10).unwrap().hits.len(), 1);
}

#[test]
fn a_moved_folder_whose_drive_vanished_keeps_its_index() {
    let (_t, mut s, mount, scope) = drive_with(|m| {
        let c = m.join("self development").join("Learn Rust");
        for i in 1..=6 {
            common::put(&c, &format!("{i:02} Lesson.mp4"), b"vid");
            common::put(&c, &format!("{i:02} Lesson.srt"), b"1");
        }
    });
    set_blueprint(s.conn(), &scope, BlueprintChoice::Fresh, 1).unwrap();
    let b = next_batch(&mut s, &scope).unwrap().unwrap();
    approve_batch(&mut s, &b, None).unwrap();
    execute_batch(&mut s, &b).unwrap();
    // The drive disappears before Aegis records the move.
    std::fs::rename(&mount, mount.with_extension("gone")).unwrap();
    after_execute(&mut s, &b).unwrap();
    let live: i64 = s.conn().query_row("SELECT COUNT(*) FROM files WHERE deleted_at IS NULL AND name LIKE '%Lesson%'", [], |r| r.get(0)).unwrap();
    assert_eq!(live, 12, "nothing is forgotten because the drive went away");
}
