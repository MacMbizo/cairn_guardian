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
        keep_here: false,
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
        plan_state: None,
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
use fixtures_context::{pc, roots_of};
use std::path::{Path, PathBuf};

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
    assert_eq!(ds.child_dirs.len(), 8);
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

// ---------------------------------------------------------------------------------------------
// Projects recogniser

use aegis_core::context::projects::{ProjectCandidate, find_projects};

fn projects_of(s: &SqliteStore) -> Vec<ProjectCandidate> {
    let roots = roots_of(s);
    find_projects(&profiles(s, &roots).unwrap(), &roots)
}

fn project_at<'a>(ps: &'a [ProjectCandidate], p: &Path) -> Option<&'a ProjectCandidate> {
    let want = common::canon(p);
    ps.iter().find(|c| c.root.eq_ignore_ascii_case(&want))
}

fn roots_list(ps: &[ProjectCandidate]) -> String {
    ps.iter().map(|p| format!("{} {:.2} {:?}", p.root, p.confidence, p.evidence)).collect::<Vec<_>>().join("\n")
}

#[test]
fn deep_marker_climbs_to_project_root_not_container() {
    let (_t, s, home, _prj) = pc();
    let ps = projects_of(&s);
    let ds = home.join("Documents/Projectsv2/Dev Set");
    let ai = project_at(&ps, &ds.join("AIPrompt")).unwrap_or_else(|| panic!("AIPrompt missing:\n{}", roots_list(&ps)));
    assert_eq!(ai.sub_projects, vec![format!("template{}prompt-pigeon", std::path::MAIN_SEPARATOR)]);
    assert!(project_at(&ps, &ds.join("AIPrompt/template/prompt-pigeon")).is_none());
    assert!(project_at(&ps, &ds).is_none());
}

#[test]
fn markerless_dev_set_folders_are_projects() {
    let (_t, s, home, _prj) = pc();
    let ps = projects_of(&s);
    let ds = home.join("Documents/Projectsv2/Dev Set");
    for name in ["chatbot", "discipline_app", "Nutrition", "paperPilot", "forexTrader", "DrivingSchool", "AgentsM"] {
        let p = project_at(&ps, &ds.join(name)).unwrap_or_else(|| panic!("{name} missing:\n{}", roots_list(&ps)));
        assert!(p.confidence >= 0.60, "{name}: {}", p.confidence);
        assert!(!p.evidence.is_empty(), "{name} has no evidence");
    }
    let disc = project_at(&ps, &ds.join("discipline_app")).unwrap();
    assert!(disc.evidence.iter().any(|e| e.contains("PRD")), "{:?}", disc.evidence);
}

#[test]
fn containers_are_not_projects() {
    let (_t, s, home, prj) = pc();
    let ps = projects_of(&s);
    let v2 = home.join("Documents/Projectsv2");
    for c in [v2.join("Dev Set"), v2.clone(), prj.clone(), prj.join("DevSet"), home.join("Documents")] {
        assert!(project_at(&ps, &c).is_none(), "{} is a container:\n{}", c.display(), roots_list(&ps));
    }
    assert!(project_at(&ps, &prj.join("DevSet/mccShepherd")).is_some(), "{}", roots_list(&ps));
    assert!(project_at(&ps, &prj.join("DevSet/wellness")).is_some());
    assert!(project_at(&ps, &v2.join("F1")).is_some(), "F1 holds forexTrader's logos:\n{}", roots_list(&ps));
    for p in ["forexTrader", "DrivingSchool", "paperPilot"] {
        assert!(project_at(&ps, &prj.join(p)).is_some(), "{p} missing:\n{}", roots_list(&ps));
    }
}

#[test]
fn ordinary_folders_are_not_projects() {
    let (_t, s, home, _prj) = pc();
    let ps = projects_of(&s);
    for f in ["Downloads/misc", "Pictures/2026/2026-10", "Pictures/2026", "Documents/CVs"] {
        assert!(project_at(&ps, &home.join(f)).is_none(), "{f} is not a project:\n{}", roots_list(&ps));
    }
    let n = ps.len();
    assert_eq!(n, 14, "{}", roots_list(&ps));
}

#[test]
fn kind_is_inferred() {
    let (_t, s, home, prj) = pc();
    let ps = projects_of(&s);
    use aegis_core::context::ProjectKind;
    let ds = home.join("Documents/Projectsv2/Dev Set");
    assert_eq!(project_at(&ps, &ds.join("forexTrader")).unwrap().project_kind, ProjectKind::Software);
    assert_eq!(project_at(&ps, &prj.join("forexTrader")).unwrap().project_kind, ProjectKind::Software);
    assert_eq!(project_at(&ps, &home.join("Documents/Projectsv2/F1")).unwrap().project_kind, ProjectKind::Design);
    assert_eq!(project_at(&ps, &ds.join("Nutrition")).unwrap().project_kind, ProjectKind::Writing);
}

// ---------------------------------------------------------------------------------------------
// Protection in filing

#[path = "fixtures_filing/mod.rs"]
mod fixtures_filing;

use aegis_core::filing::blueprint::{Placement, plan};
use aegis_core::filing::{BlueprintChoice, Scope};

/// The owner's 14 turned-down moves of project material (2026-11-22), relative to the test's
/// temporary folder (`me` is the owner's profile, `Prj` is `C:\Prj`).
const REJECTED: [&str; 14] = [
    "me/Documents/Projectsv2/Dev Set/forexTrader/images/FX Sentinel - logo(F)_v2.png",
    "me/Documents/Projectsv2/Dev Set/forexTrader/images/favicon_io/favicon-16x16.png",
    "me/Documents/Projectsv2/F1/fx-favicon-32x32.png",
    "me/Documents/Projectsv2/Dev Set/forexTrader/set_files_v2(backup)/fx-favicon-32x32.png",
    "me/Documents/Projectsv2/Dev Set/forexTrader/images/favicon_io/favicon-32x32.png",
    "me/Documents/Projectsv2/Dev Set/paperPilot/images/logo_v3.jpg",
    "Prj/DevSet/mccShepherd/all docs/others/G5_dual_ai_human_orchestration_playbook.md",
    "Prj/DevSet/mccShepherd/all docs/archive/PRD_additions.md",
    "me/Documents/Projectsv2/Dev Set/discipline_app/set_files/PRD_additions.md",
    "me/Documents/Projectsv2/Dev Set/paperPilot/set_files/TASKS.md",
    "Prj/forexTrader/images/FX Sentinel - logo(F)_v2.png",
    "Prj/forexTrader/images/favicon_io/favicon-16x16.png",
    "Prj/forexTrader/set_files_v2(backup)/fx-favicon-32x32.png",
    "Prj/forexTrader/images/favicon_io/favicon-32x32.png",
];

fn moving(ps: &[Placement], path: &str) -> Option<String> {
    ps.iter()
        .filter(|p| p.to.is_some())
        .find(|p| aegis_core::db::files::path_is_within(path, &p.from, false))
        .map(|p| format!("{} -> {} ({})", p.from, p.to.as_deref().unwrap_or(""), p.reason))
}

#[test]
fn rejected_project_moves_are_not_proposed() {
    let (t, s, _home, _prj) = pc();
    for choice in [BlueprintChoice::Fresh, BlueprintChoice::Tidy] {
        let ps = plan(&s, &Scope::Pc, choice).unwrap();
        let bad: Vec<String> =
            REJECTED.iter().filter_map(|r| moving(&ps, &common::canon(&t.path().join(r)))).collect();
        assert!(bad.is_empty(), "{choice:?} still splits projects:\n{}", bad.join("\n"));
        let kept = ps
            .iter()
            .find(|p| p.from.eq_ignore_ascii_case(&common::canon(&t.path().join("Prj/forexTrader"))))
            .expect("forexTrader is planned as one group");
        assert_eq!(kept.reason, "Part of project forexTrader: it stays together");
    }
}

#[test]
fn loose_files_still_filed() {
    let (_t, mut s, home, _prj) = pc();
    let dl = home.join("Downloads");
    fixtures_filing::docx(&dl, "scan0001.docx", "TAX INVOICE Vodacom Invoice date: 14/03/2026 VAT Amount due");
    aegis_core::scanner::scan_root(&mut s, &dl, &[1]).unwrap();
    aegis_core::filing::reader::read_pending(&mut s, 100, std::time::Duration::from_secs(10)).unwrap();
    let ps = plan(&s, &Scope::Pc, BlueprintChoice::Fresh).unwrap();
    let inv = common::canon(&dl.join("scan0001.docx"));
    let p = ps.iter().find(|p| p.from.eq_ignore_ascii_case(&inv)).expect("invoice is planned");
    assert!(p.to.as_deref().is_some_and(|t| t.contains("Invoices")), "loose invoice still filed: {p:?}");
}

#[test]
fn waiting_batch_items_inside_projects_are_superseded() {
    let (t, mut s, home, _prj) = pc();
    aegis_core::filing::set_blueprint(s.conn(), &Scope::Pc, BlueprintChoice::Fresh, 0).unwrap();
    let id_of = |s: &SqliteStore, p: &Path| -> String {
        s.conn()
            .query_row("SELECT id FROM files WHERE lower(path) = lower(?1)", [common::canon(p)], |r| r.get(0))
            .unwrap()
    };
    let favicon = t.path().join(REJECTED[1]);
    let installer = home.join("Downloads/misc/zoom installer.exe");
    let place = |s: &SqliteStore, p: &Path, to: &Path| Placement {
        target: aegis_core::filing::blueprint::Target::File { file_id: id_of(s, p) },
        from: common::canon(p),
        to: Some(to.to_string_lossy().to_string()),
        new_name: None,
        confidence: 0.95,
        reason: "old plan".into(),
        rule_key: None,
        to_sort: false,
        bytes: 1,
    };
    let old = vec![
        place(&s, &favicon, &home.join("Pictures/2025/2025-06/favicon-16x16.png")),
        place(&s, &installer, &home.join("Downloads/Software/zoom installer.exe")),
    ];
    let batch = aegis_core::filing::batches::propose_placements(&mut s, &Scope::Pc, &aegis_core::filing::batches::origin_for(&Scope::Pc), old).unwrap();
    let n = aegis_core::filing::batches::recheck_open_batch(&mut s, &Scope::Pc).unwrap();
    assert_eq!(n, 1);
    let status = |s: &SqliteStore, p: &Path| -> String {
        s.conn()
            .query_row(
                "SELECT status FROM actions WHERE batch_id = ?1 AND lower(from_path) = lower(?2)",
                rusqlite::params![batch, common::canon(p)],
                |r| r.get(0),
            )
            .unwrap()
    };
    assert_eq!(status(&s, &favicon), "superseded");
    assert_eq!(status(&s, &installer), "proposed");
}

// ---------------------------------------------------------------------------------------------
// Identity and relationships

use aegis_core::context::relate::{
    ContentSummary, FileKey, choose_home, content_summary, normalized_name, relate, same_identity, version_series,
};

/// A summary holding `rels` (each 2 KB), modified at `mtime`.
fn summary(rels: impl IntoIterator<Item = String>, mtime: i64) -> ContentSummary {
    let mut s = ContentSummary::default();
    for r in rels {
        s.files.insert(r, FileKey { size: 2048, mtime, blake3: None });
        s.newest_mtime = s.newest_mtime.max(mtime);
    }
    s
}

fn named(prefix: &str, n: usize) -> Vec<String> {
    (0..n).map(|i| format!("{prefix}{i}.md")).collect()
}

fn merged(mut a: ContentSummary, b: ContentSummary) -> ContentSummary {
    a.newest_mtime = a.newest_mtime.max(b.newest_mtime);
    a.files.extend(b.files);
    a
}

#[test]
fn identical_copies() {
    let a = summary(named("f", 50), 100);
    let (rel, shared, by) = relate(&a, &a.clone());
    assert_eq!((rel, shared), (Relationship::Identical, 50));
    assert_eq!(by, "name and size");
}

#[test]
fn older_subset_like_forextrader() {
    // C:\Prj holds 356 files, all also in the Dev Set copy of 8,901 files, which is newer.
    let home = merged(summary(named("s", 356), 100), summary(named("h", 8_545), 200));
    let other = summary(named("s", 356), 100);
    assert_eq!(relate(&home, &other), (Relationship::OlderSubset, 356, "name and size".into()));
}

#[test]
fn diverged_like_drivingschool() {
    // 99 shared; 9 only in one copy, 13 only in the other (newer).
    let home = merged(summary(named("s", 99), 100), summary(named("a", 9), 150));
    let other = merged(summary(named("s", 99), 100), summary(named("b", 13), 300));
    assert_eq!(relate(&home, &other).0, Relationship::Diverged);
    assert_eq!(relate(&home, &other).1, 99);
    // A subset holding a newer version of a shared file is diverged, not an older subset.
    let mut newer = summary(named("s", 10), 100);
    newer.files.get_mut("s0.md").unwrap().mtime = 900;
    newer.newest_mtime = 900;
    let home = merged(summary(named("s", 10), 100), summary(named("h", 20), 200));
    assert_eq!(relate(&home, &newer).0, Relationship::Diverged);
}

#[test]
fn different_material_like_paperpilot() {
    let code: Vec<String> = (0..300).map(|i| format!("src{}c{i}.tsx", std::path::MAIN_SEPARATOR)).collect();
    let home = merged(summary(code, 100), summary(named("s", 2), 100));
    let other = merged(summary(named("doc", 344), 100), summary(named("s", 2), 100));
    assert_eq!(relate(&home, &other), (Relationship::DifferentMaterial, 2, "name and size".into()));
}

#[test]
fn matching_by_hash_ignores_paths() {
    let mut a = ContentSummary::default();
    let mut b = ContentSummary::default();
    for i in 0..30u8 {
        a.files.insert(format!("x{i}"), FileKey { size: 5000, mtime: 1, blake3: Some(vec![i; 32]) });
        b.files.insert(format!("moved{}y{i}", std::path::MAIN_SEPARATOR), FileKey { size: 5000, mtime: 1, blake3: Some(vec![i; 32]) });
    }
    assert_eq!(relate(&a, &b), (Relationship::Identical, 30, "content".into()));
}

fn cand(root: &str, declared: &[&str]) -> ProjectCandidate {
    ProjectCandidate {
        root: root.into(),
        volume_id: "v".into(),
        confidence: 0.9,
        evidence: vec![],
        project_kind: aegis_core::context::ProjectKind::Software,
        sub_projects: vec![],
        is_repo: false,
        declared_names: declared.iter().map(|d| d.to_string()).collect(),
        files: 0,
        bytes: 0,
        newest_mtime: 0,
    }
}

#[test]
fn name_only_match_is_possible_not_assumed() {
    let a = cand(r"C:\Prj\wellness", &[]);
    let b = cand(r"C:\Users\me\Documents\Projectsv2\Dev Set\wellness_app", &[]);
    let (sa, sb) = (summary(named("a", 30), 1), summary(named("b", 20), 1));
    let id = same_identity(&a, &b, &sa, &sb, &[]).expect("names agree");
    assert!(!id.confirmed, "{id:?}");
    let other = cand(r"C:\Prj\chatbot", &[]);
    assert!(same_identity(&a, &other, &sa, &sb, &[]).is_none());
    // A manifest or shared content confirms it.
    let (ma, mb) = (cand(r"C:\Prj\x", &["mcc-wellness"]), cand(r"D:\y", &["mcc-wellness"]));
    assert!(same_identity(&ma, &mb, &sa, &sb, &[]).unwrap().confirmed);
    let (ca, cb) = (summary(named("s", 25), 1), merged(summary(named("s", 25), 1), summary(named("z", 100), 1)));
    let id = same_identity(&cand(r"C:\Prj\one", &[]), &cand(r"D:\two", &[]), &ca, &cb, &[]).unwrap();
    assert!(id.confirmed && id.evidence.contains("25 files"), "{id:?}");
}

#[test]
fn build_output_is_ignored_in_relationships() {
    let (_t, s, home, prj) = pc();
    let vol = |p: &Path| -> String {
        s.conn()
            .query_row("SELECT volume_id FROM files WHERE lower(path) LIKE lower(?1) || '%' LIMIT 1", [common::canon(p)], |r| r.get(0))
            .unwrap()
    };
    let code = content_summary(&s, &vol(&prj), &common::canon(&prj.join("paperPilot"))).unwrap();
    assert_eq!(code.len(), 5, "node_modules must not count: {:?}", code.files.keys().collect::<Vec<_>>());
    let ds = home.join("Documents/Projectsv2/Dev Set");
    let docs = content_summary(&s, &vol(&ds), &common::canon(&ds.join("paperPilot"))).unwrap();
    assert_eq!(relate(&code, &docs).0, Relationship::DifferentMaterial);
    let older = content_summary(&s, &vol(&prj), &common::canon(&prj.join("forexTrader"))).unwrap();
    let newer = content_summary(&s, &vol(&ds), &common::canon(&ds.join("forexTrader"))).unwrap();
    assert_eq!(relate(&newer, &older).0, Relationship::OlderSubset);
}

#[test]
fn version_series_inv_inv2_inv3() {
    assert!(version_series("Inv", "Inv2"));
    assert!(version_series("Inv2", "Inv3"));
    assert!(!version_series("Inv", "Inv"));
    assert!(!version_series("D1", "D2"), "too short to be a series by name");
    assert!(!version_series("Inv", "Invoice"));
    for (raw, want) in [
        ("Inv2", "inv"),
        ("wellness_app", "wellness"),
        ("forexTrader", "forextrader"),
        ("set_files_v2(backup)", "setfiles"),
        ("NewDevSet", "devset"),
        ("DrivingSchool-main", "drivingschool"),
        ("budget_v1.1", "budget"),
    ] {
        assert_eq!(normalized_name(raw), want, "{raw}");
    }
}

#[test]
fn home_is_repo_then_newest_then_biggest() {
    let loc = |files: i64, newest: i64, repo: bool| Location {
        is_repo: repo,
        newest_mtime: newest,
        files,
        ..location("x", "c", "v-c", r"C:\x", Relationship::Unknown)
    };
    assert_eq!(choose_home(&[loc(10, 5, false), loc(5, 9, false)]), 1);
    assert_eq!(choose_home(&[loc(10, 5, false), loc(5, 1, true)]), 1);
    assert_eq!(choose_home(&[loc(10, 5, false), loc(50, 5, false)]), 1);
}

// ---------------------------------------------------------------------------------------------
// Recognition pass and watch

use aegis_core::context::recognise;

fn collection_named<'a>(cs: &'a [Collection], name: &str) -> &'a Collection {
    cs.iter().find(|c| c.name == name).unwrap_or_else(|| {
        panic!("no {name} in {:?}", cs.iter().map(|c| (&c.name, c.locations.len())).collect::<Vec<_>>())
    })
}

fn location_under<'a>(c: &'a Collection, p: &Path) -> &'a Location {
    let want = common::canon(p);
    c.locations.iter().find(|l| l.root_path.eq_ignore_ascii_case(&want)).unwrap_or_else(|| panic!("{} not in {c:?}", want))
}

#[test]
fn recognise_groups_dev_set_and_prj_copies() {
    let (_t, mut s, home, prj) = pc();
    let rep = recognise(&mut s).unwrap();
    let cs = list_collections(s.conn(), CollectionKind::Project).unwrap();
    assert_eq!(rep.projects, cs.len());
    let ds = home.join("Documents/Projectsv2/Dev Set");
    let fx = collection_named(&cs, "forexTrader");
    assert_eq!(fx.locations.len(), 2, "{fx:?}");
    assert_eq!(location_under(fx, &ds.join("forexTrader")).relationship, Relationship::Home);
    let old = location_under(fx, &prj.join("forexTrader"));
    assert_eq!(old.relationship, Relationship::OlderSubset);
    assert_eq!(old.shared_files, 9);
    let driving = collection_named(&cs, "DrivingSchool");
    assert_eq!(driving.locations.len(), 2);
    assert_eq!(location_under(driving, &prj.join("DrivingSchool")).relationship, Relationship::Home, "the newer copy is home");
    assert_eq!(location_under(driving, &ds.join("DrivingSchool")).relationship, Relationship::Diverged);
    assert!(rep.multi_location >= 3, "{rep:?}");
    // Multi-location projects come first.
    assert!(cs[0].locations.len() > 1);
    // Saved projects now drive protection.
    let roots = roots_of(&s);
    let protected = aegis_core::context::protected_roots(&s, &roots).unwrap();
    assert!(protected.iter().any(|(p, n)| n == "forexTrader" && p.eq_ignore_ascii_case(&common::canon(&prj.join("forexTrader")))));
}

fn walk(dir: &Path) -> Vec<PathBuf> {
    let mut out = Vec::new();
    for e in std::fs::read_dir(dir).unwrap() {
        let p = e.unwrap().path();
        if p.is_dir() { out.extend(walk(&p)) } else { out.push(p) }
    }
    out
}

#[test]
fn offline_drive_locations_are_listed() {
    let (t, mut s, _home, _prj) = pc();
    let provider = aegis_core::drives::FakeVolumeProvider::new();
    let d = common::fake_drive(&t, "Ext", "0042");
    for f in ["images/favicon_io/favicon-16x16.png", "engine/fx/signals.py", "engine/fx/backtest.py", "docs/architecture.md"] {
        fixtures_context::put(&d.mount_point.join("Old Projects").join("forexTrader"), f);
    }
    fixtures_context::put(&d.mount_point.join("Old Projects").join("other"), "notes/PRD.md");
    fixtures_context::put(&d.mount_point.join("Old Projects").join("other"), "src/main.py");
    for f in walk(&d.mount_point) {
        fixtures_context::age(&f, 120);
    }
    let id = common::plug_and_catalog(&mut s, &provider, &d);
    provider.unplug(&d.mount_point);
    aegis_core::drives::sync_mounted(&mut s, &provider).unwrap();
    assert_eq!(common::status_of(&s, &id), "offline");
    recognise(&mut s).unwrap();
    let cs = list_collections(s.conn(), CollectionKind::Project).unwrap();
    let fx = collection_named(&cs, "forexTrader");
    let ext = fx.locations.iter().find(|l| l.volume_id == id).unwrap_or_else(|| panic!("drive copy missing: {fx:?}"));
    assert!(!ext.online);
    assert_eq!(ext.relationship, Relationship::OlderSubset);
}

#[test]
fn backup_drive_is_ignored() {
    let (t, mut s, _home, _prj) = pc();
    let provider = aegis_core::drives::FakeVolumeProvider::new();
    let d = common::fake_drive(&t, "Backup", "0043");
    for f in ["images/favicon_io/favicon-16x16.png", "engine/fx/signals.py", "docs/PRD.md", "README.md"] {
        fixtures_context::put(&d.mount_point.join("Aegis Backup").join("forexTrader"), f);
    }
    let id = common::plug_and_catalog(&mut s, &provider, &d);
    s.conn().execute("UPDATE volumes SET role = 'backup' WHERE id = ?1", [&id]).unwrap();
    recognise(&mut s).unwrap();
    let cs = list_collections(s.conn(), CollectionKind::Project).unwrap();
    assert!(cs.iter().flat_map(|c| &c.locations).all(|l| l.volume_id != id));
}

#[test]
fn recognise_is_fast_on_a_big_catalog() {
    let t = tempfile::TempDir::new().unwrap();
    let root = t.path().join("big");
    std::fs::create_dir_all(&root).unwrap();
    let mut s = SqliteStore::open_in_memory().unwrap();
    let cfg = s.add_root(&root).unwrap();
    let base = common::canon(&root);
    let tx = s.conn_mut().transaction().unwrap();
    {
        let mut stmt = tx
            .prepare(
                "INSERT INTO files (id, volume_id, path, path_hash, name, ext, size, mtime, ctime, cloud_state, state, first_seen, last_seen, created_at, updated_at)
                 VALUES (?1, ?2, ?3, ?4, ?5, ?6, 4096, ?7, 0, 'local', 'active', 0, 0, 0, 0)",
            )
            .unwrap();
        let sep = std::path::MAIN_SEPARATOR;
        for i in 0..200_000usize {
            let proj = i / 500;
            let (dir, name, ext) = match i % 5 {
                0 => (format!("docs{sep}part{}", i % 7), format!("PRD {i}.md"), "md"),
                1 => ("images".to_string(), format!("logo {i}.png"), "png"),
                _ => (format!("src{sep}mod{}", i % 11), format!("file{i}.ts"), "ts"),
            };
            let path = format!("{base}{sep}group{}{sep}proj{proj}{sep}{dir}{sep}{name}", proj % 8);
            let hash = blake3::hash(path.as_bytes());
            stmt.execute(rusqlite::params![format!("f{i}"), cfg.volume_id, path, hash.as_bytes().to_vec(), name, ext, i as i64])
                .unwrap();
        }
    }
    tx.commit().unwrap();
    let started = std::time::Instant::now();
    let rep = recognise(&mut s).unwrap();
    let took = started.elapsed();
    assert_eq!(rep.projects, 400, "{rep:?}");
    assert!(took.as_secs() < 15, "recognition took {took:?}");
}

#[test]
fn context_watch_runs_only_when_the_catalog_changed() {
    use aegis_core::engine::Watch;
    let (_t, mut s, home, _prj) = pc();
    let mut w = aegis_core::context::watch::ContextWatch;
    w.run(&mut s, 0).unwrap();
    let first = s.get_setting(aegis_core::context::recognise::RECOGNISED_AT, "context").unwrap();
    assert!(first.is_some());
    s.conn().execute("DELETE FROM collections", []).unwrap();
    w.run(&mut s, 0).unwrap();
    assert!(list_collections(s.conn(), CollectionKind::Project).unwrap().is_empty(), "nothing changed, so no new pass");
    fixtures_context::put(&home.join("Documents/Projectsv2/Dev Set/chatbot"), "more notes.md");
    aegis_core::scanner::scan_root(&mut s, &home.join("Documents"), &[1]).unwrap();
    w.run(&mut s, 0).unwrap();
    assert!(!list_collections(s.conn(), CollectionKind::Project).unwrap().is_empty());
}

// ---------------------------------------------------------------------------------------------
// The owner's answers

use aegis_core::context::{Answer, answer};

fn projects_now(s: &SqliteStore) -> Vec<Collection> {
    list_collections(s.conn(), CollectionKind::Project).unwrap()
}

#[test]
fn is_project_no_drops_it_and_protection() {
    let (_t, mut s, home, _prj) = pc();
    recognise(&mut s).unwrap();
    let nutrition = collection_named(&projects_now(&s), "Nutrition").id.clone();
    answer(&mut s, &nutrition, None, Answer::IsProject { yes: false }).unwrap();
    assert!(projects_now(&s).iter().all(|c| c.name != "Nutrition"));
    let folder = common::canon(&home.join("Documents/Projectsv2/Dev Set/Nutrition"));
    let protected = aegis_core::context::protected_roots(&s, &roots_of(&s)).unwrap();
    assert!(protected.iter().all(|(p, _)| !p.eq_ignore_ascii_case(&folder)));
    let ps = plan(&s, &Scope::Pc, BlueprintChoice::Fresh).unwrap();
    assert!(ps.iter().all(|p| !p.from.eq_ignore_ascii_case(&folder)), "no longer kept whole");
    // The answer holds on the next pass.
    recognise(&mut s).unwrap();
    assert!(projects_now(&s).iter().all(|c| c.name != "Nutrition"));
}

#[test]
fn not_same_survives_rename_and_new_letter() {
    let (t, mut s, _home, _prj) = pc();
    let provider = aegis_core::drives::FakeVolumeProvider::new();
    let d = common::fake_drive(&t, "Ext", "0077");
    let copy = d.mount_point.join("Old").join("forexTrader");
    for f in ["images/favicon_io/favicon-16x16.png", "engine/fx/signals.py", "engine/fx/backtest.py", "docs/architecture.md"] {
        fixtures_context::put(&copy, f);
    }
    for f in walk(&d.mount_point) {
        fixtures_context::age(&f, 120);
    }
    let id = common::plug_and_catalog(&mut s, &provider, &d);
    recognise(&mut s).unwrap();
    let fx = collection_named(&projects_now(&s), "forexTrader").clone();
    assert_eq!(fx.locations.len(), 3, "{fx:?}");
    let on_drive = fx.locations.iter().find(|l| l.volume_id == id).unwrap().id.clone();
    answer(&mut s, &fx.id, Some(&on_drive), Answer::NotSameAs).unwrap();
    let apart = |s: &SqliteStore| {
        let cs = projects_now(s);
        let fx = cs.iter().find(|c| c.locations.iter().any(|l| l.relationship == Relationship::Home && l.volume_id != id && c.name == "forexTrader")).unwrap();
        assert_eq!(fx.locations.len(), 2, "{fx:?}");
        assert!(cs.iter().any(|c| c.locations.len() == 1 && c.locations[0].volume_id == id), "the drive copy stands alone");
    };
    apart(&s);

    // Renamed on the drive, and the drive comes back under another letter.
    provider.unplug(&d.mount_point);
    aegis_core::drives::sync_mounted(&mut s, &provider).unwrap();
    let new_mount = t.path().join("Ext2");
    std::fs::rename(&d.mount_point, &new_mount).unwrap();
    std::fs::rename(new_mount.join("Old").join("forexTrader"), new_mount.join("Old").join("fx old")).unwrap();
    let mut moved = d.clone();
    moved.mount_point = new_mount.clone();
    provider.plug(moved);
    aegis_core::drives::sync_mounted(&mut s, &provider).unwrap();
    aegis_core::scanner::catalog_volume(&mut s, &id, &mut |_| {}).unwrap();
    recognise(&mut s).unwrap();
    apart(&s);
}

#[test]
fn same_as_joins_name_mismatch() {
    let (_t, mut s, _home, prj) = pc();
    for f in ["NutriP PRD.md", "src/planner.py", "src/meals.py", "src/shopping.py"] {
        fixtures_context::put(&prj.join("NutriP"), f);
    }
    aegis_core::scanner::scan_root(&mut s, &prj, &[1]).unwrap();
    recognise(&mut s).unwrap();
    let cs = projects_now(&s);
    let (a, b) = (collection_named(&cs, "Nutrition").id.clone(), collection_named(&cs, "NutriP").id.clone());
    answer(&mut s, &b, None, Answer::SameAs { other_collection_id: a }).unwrap();
    for _ in 0..2 {
        let cs = projects_now(&s);
        let joined = cs.iter().find(|c| c.locations.len() == 2 && c.locations.iter().any(|l| l.root_path.ends_with("NutriP"))).unwrap_or_else(|| panic!("not joined: {cs:?}"));
        assert!(joined.locations.iter().any(|l| l.root_path.ends_with("Nutrition")));
        assert!(joined.evidence.iter().any(|e| e.contains("you said")), "{:?}", joined.evidence);
        recognise(&mut s).unwrap();
    }
}

#[test]
fn leave_as_is_keeps_protection() {
    let (_t, mut s, _home, prj) = pc();
    recognise(&mut s).unwrap();
    let fx = collection_named(&projects_now(&s), "forexTrader").id.clone();
    answer(&mut s, &fx, None, Answer::LeaveAsIs).unwrap();
    let fx = collection_named(&projects_now(&s), "forexTrader").clone();
    assert_eq!(fx.state, "left_as_is");
    let protected = aegis_core::context::protected_roots(&s, &roots_of(&s)).unwrap();
    assert!(protected.iter().any(|(p, _)| p.eq_ignore_ascii_case(&common::canon(&prj.join("forexTrader")))));
}

#[test]
fn rename_is_remembered() {
    let (_t, mut s, _home, _prj) = pc();
    recognise(&mut s).unwrap();
    let fx = collection_named(&projects_now(&s), "forexTrader").id.clone();
    answer(&mut s, &fx, None, Answer::Rename { name: "FX Sentinel".into() }).unwrap();
    recognise(&mut s).unwrap();
    assert_eq!(collection_named(&projects_now(&s), "FX Sentinel").locations.len(), 2);
    let empty = answer(&mut s, &fx, None, Answer::Rename { name: "  ".into() });
    assert!(empty.is_err());
}

#[test]
fn fingerprints_name_one_location_not_its_copies() {
    use aegis_core::context::relate::{fingerprint, fingerprints_match};
    let full = summary(named("f", 15), 1);
    let part = summary(named("f", 9), 1);
    let here = fingerprint("forexTrader", &[], &full, "v-c", r"C:\Users\me\Documents\Dev Set\forexTrader");
    let copy = fingerprint("forexTrader", &[], &part, "v-c", r"C:\Prj\forexTrader");
    let renamed = fingerprint("fx old", &[], &full, "v-c", r"C:\Users\me\Documents\Dev Set\fx old");
    let other_drive = fingerprint("forexTrader", &[], &full, "v-d", r"D:\Prj\forexTrader");
    assert!(!fingerprints_match(&here, &copy), "a copy is another location");
    assert!(fingerprints_match(&here, &renamed), "a rename keeps its identity");
    assert!(!fingerprints_match(&here, &other_drive));
}

#[test]
fn shared_boilerplate_does_not_make_projects_one() {
    let (_t, mut s, _home, prj) = pc();
    // Four unrelated projects built from the same starter kit: 25 identical files each.
    for name in ["alpha", "bravo", "charlie", "delta"] {
        for i in 0..25 {
            fixtures_context::put_as(&prj.join(name), &format!("agents/agent{i}.md"), &format!("starter kit agent number {i}"));
        }
        fixtures_context::put(&prj.join(name), &format!("src/{name}_main.py"));
        fixtures_context::put(&prj.join(name), &format!("docs/{name} PRD.md"));
    }
    aegis_core::scanner::scan_root(&mut s, &prj, &[1]).unwrap();
    recognise(&mut s).unwrap();
    let cs = projects_now(&s);
    for name in ["alpha", "bravo", "charlie", "delta"] {
        let c = collection_named(&cs, name);
        assert_eq!(c.locations.len(), 1, "{name} was joined with others: {c:?}");
    }
}

#[test]
fn a_mixed_drive_root_lends_no_project_status() {
    let (t, mut s, _home, _prj) = pc();
    let provider = aegis_core::drives::FakeVolumeProvider::new();
    let d = common::fake_drive(&t, "Mixed", "0051");
    let m = d.mount_point.clone();
    // Two real projects at the top of the drive...
    for f in ["README.md", "src/main.py"] {
        fixtures_context::put(&m.join("webscraper"), f);
    }
    for f in ["package.json", "src/app.ts"] {
        fixtures_context::put(&m.join("shop"), f);
    }
    // ...beside the owner's ordinary folders.
    for (dir, f) in [
        ("Self Development", "Atomic Habits.pdf"),
        ("Self Development", "course notes.docx"),
        ("Self Development", "lecture 1.mp4"),
        ("Self Development", "script.py"),
        ("Movies", "film.mkv"),
        ("Movies", "film2.mkv"),
        ("Movies", "film3.mkv"),
        ("Books", "a.pdf"),
        ("Books", "b.pdf"),
        ("Books", "c.epub"),
        ("Music", "x.mp3"),
    ] {
        fixtures_context::put(&m.join(dir), f);
    }
    common::plug_and_catalog(&mut s, &provider, &d);
    recognise(&mut s).unwrap();
    let cs = projects_now(&s);
    let at = |n: &str| cs.iter().any(|c| c.locations.iter().any(|l| l.root_path.eq_ignore_ascii_case(&common::canon(&m.join(n)))));
    assert!(at("webscraper") && at("shop"));
    for n in ["Self Development", "Movies", "Books"] {
        assert!(!at(n), "{n} is not a project");
    }
}
