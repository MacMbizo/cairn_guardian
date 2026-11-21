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
