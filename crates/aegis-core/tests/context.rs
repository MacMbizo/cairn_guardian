//! Context layer: collections (projects first), their locations, and the owner's answers.

mod common;

use aegis_core::context::store::{decisions, list_collections, record_decision, save_collections};
use aegis_core::context::{Collection, CollectionKind, Decision, DecisionKind, Location, ProjectKind, Relationship};
use aegis_core::db::SqliteStore;

fn store() -> SqliteStore {
    let s = SqliteStore::open_in_memory().unwrap();
    s.conn()
        .execute(
            "INSERT INTO volumes (id, mount_point, fs_type, is_removable, is_readonly, has_stable_inode, case_sensitive, status, last_seen)
             VALUES ('v-c', 'C:\\', 'ntfs', 0, 0, 1, 0, 'online', 0), ('v-d', 'D:\\', 'ntfs', 1, 0, 1, 0, 'offline', 0)",
            [],
        )
        .unwrap();
    s
}

fn location(id: &str, collection: &str, volume: &str, root: &str, rel: Relationship) -> Location {
    Location {
        id: id.into(),
        collection_id: collection.into(),
        volume_id: volume.into(),
        root_path: root.into(),
        relationship: rel,
        files: 356,
        bytes: 1_000,
        newest_mtime: 5,
        shared_files: 356,
        compared_by: "content".into(),
        is_repo: false,
        online: true,
        fingerprint: format!("fp-{id}"),
    }
}

fn forex() -> Collection {
    Collection {
        id: "c1".into(),
        kind: CollectionKind::Project,
        name: "forexTrader".into(),
        project_kind: Some(ProjectKind::Software),
        confidence: 0.92,
        state: "candidate".into(),
        evidence: vec!["has a README".into(), "source code (40 files)".into()],
        locations: vec![
            location("l1", "c1", "v-c", "C:\\Users\\me\\Documents\\Projectsv2\\Dev Set\\forexTrader", Relationship::Home),
            location("l2", "c1", "v-d", "D:\\Prj\\forexTrader", Relationship::OlderSubset),
        ],
        sub_projects: vec!["engine".into()],
    }
}

#[test]
fn collections_round_trip() {
    let s = store();
    save_collections(s.conn(), &[forex()]).unwrap();
    let got = list_collections(s.conn(), CollectionKind::Project).unwrap();
    let mut want = forex();
    // Online comes from the volume, not from what was saved.
    want.locations[1].online = false;
    assert_eq!(got, vec![want]);
}

#[test]
fn saving_again_replaces_computed_rows_but_keeps_decisions() {
    let s = store();
    save_collections(s.conn(), &[forex()]).unwrap();
    let d = Decision {
        id: "d1".into(),
        kind: DecisionKind::NotSameAs,
        fingerprint_a: "fp-l1".into(),
        fingerprint_b: Some("fp-l2".into()),
        value: "no".into(),
        at: 7,
    };
    record_decision(s.conn(), &d).unwrap();

    let mut other = forex();
    other.id = "c2".into();
    other.name = "paperPilot".into();
    other.locations = vec![location("l9", "c2", "v-c", "C:\\Prj\\paperPilot", Relationship::Home)];
    save_collections(s.conn(), &[other.clone()]).unwrap();

    let got = list_collections(s.conn(), CollectionKind::Project).unwrap();
    assert_eq!(got, vec![other]);
    assert_eq!(decisions(s.conn()).unwrap(), vec![d]);
}
