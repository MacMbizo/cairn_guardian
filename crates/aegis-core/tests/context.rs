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

// ---------------------------------------------------------------------------------------------
// Folder profiles

mod fixtures_context;

use aegis_core::context::profile::{FolderProfile, profiles};
use std::path::{Path, PathBuf};

/// A scanned PC: `me\Documents` (with `Projectsv2\Dev Set`), `me\Downloads`, `me\Pictures` and
/// `Prj`, all roots. Returns (tmp, store, home, prj).
pub fn pc() -> (tempfile::TempDir, SqliteStore, PathBuf, PathBuf) {
    let t = tempfile::TempDir::new().unwrap();
    let home = t.path().join("me");
    let prj = t.path().join("Prj");
    for f in ["Documents", "Downloads", "Pictures"] {
        std::fs::create_dir_all(home.join(f)).unwrap();
    }
    fixtures_context::dev_set(&home.join("Documents"));
    fixtures_context::prj(&prj);
    fixtures_context::ordinary(&home);
    let mut s = SqliteStore::open_in_memory().unwrap();
    for r in [home.join("Documents"), home.join("Downloads"), home.join("Pictures"), prj.clone()] {
        s.add_root(&r).unwrap();
        aegis_core::scanner::scan_root(&mut s, &r, &[1]).unwrap();
    }
    aegis_core::filing::reader::read_pending(&mut s, 5_000, std::time::Duration::from_secs(30)).unwrap();
    (t, s, home, prj)
}

pub fn roots_of(s: &SqliteStore) -> Vec<String> {
    s.list_roots().unwrap().into_iter().map(|r| r.path).collect()
}

fn profile_at<'a>(ps: &'a [FolderProfile], p: &Path) -> &'a FolderProfile {
    let want = common::canon(p);
    ps.iter().find(|f| f.path.eq_ignore_ascii_case(&want)).unwrap_or_else(|| panic!("no profile for {want}"))
}

#[test]
fn profile_counts_exclude_build_output() {
    let (_t, s, _home, prj) = pc();
    let ps = profiles(&s, &roots_of(&s)).unwrap();
    let pp = profile_at(&ps, &prj.join("paperPilot"));
    assert_eq!(pp.files, 5, "node_modules must not count");
    assert!(pp.child_dirs.iter().all(|c| !c.to_lowercase().ends_with("node_modules")));
    assert!(pp.atomic_unit);
    assert!(pp.manifests.iter().any(|(f, _)| f == "package.json"));
    assert!(pp.has_readme);
    let pigeon = profile_at(&ps, &prj.parent().unwrap().join("me/Documents/Projectsv2/Dev Set/AIPrompt/template/prompt-pigeon"));
    assert!(pigeon.has_git);
    assert_eq!(pigeon.files, 2, ".git contents must not count");
}

#[test]
fn profile_finds_doc_and_asset_signals() {
    let (_t, s, home, _prj) = pc();
    let ps = profiles(&s, &roots_of(&s)).unwrap();
    let ds = home.join("Documents/Projectsv2/Dev Set");
    let disc = profile_at(&ps, &ds.join("discipline_app"));
    assert!(disc.doc_signals.contains(&"prd".to_string()), "{:?}", disc.doc_signals);
    assert!(disc.doc_signals.contains(&"set_files".to_string()), "{:?}", disc.doc_signals);
    assert!(disc.source_files >= 4);
    let fx = profile_at(&ps, &ds.join("forexTrader"));
    for w in ["favicon", "logo", "icon"] {
        assert!(fx.asset_signals.contains(&w.to_string()), "{w} missing from {:?}", fx.asset_signals);
    }
    assert!(fx.doc_signals.contains(&"architecture".to_string()));
    let cvs = profile_at(&ps, &home.join("Documents/CVs"));
    assert!(cvs.doc_signals.is_empty() && cvs.asset_signals.is_empty(), "{:?} {:?}", cvs.doc_signals, cvs.asset_signals);
}

#[test]
fn profile_rolls_up_subtrees() {
    let (_t, s, home, _prj) = pc();
    let ps = profiles(&s, &roots_of(&s)).unwrap();
    let ds_path = home.join("Documents/Projectsv2/Dev Set");
    let ds = profile_at(&ps, &ds_path);
    let children: i64 = ds.child_dirs.iter().map(|c| profile_at(&ps, Path::new(c)).files).sum();
    assert_eq!(ds.files, children + ds.loose_files);
    assert_eq!(ds.child_dirs.len(), 7);
    assert_eq!(ds.loose_files, 0);
    let ai = profile_at(&ps, &ds_path.join("AIPrompt"));
    assert_eq!(ai.files, 4);
    assert_eq!(ai.loose_files, 0);
    assert_eq!(ai.depth, 3);
    let driving = profile_at(&ps, &ds_path.join("DrivingSchool"));
    let src = profile_at(&ps, &ds_path.join("DrivingSchool/src"));
    assert!(driving.newest_mtime >= src.newest_mtime);
    assert_eq!(driving.kind_count(aegis_core::filing::Kind::Photo), 2);
}
