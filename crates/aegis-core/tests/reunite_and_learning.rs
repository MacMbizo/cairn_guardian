//! Put back together, learned aliases, and weekly-note projects summary (spec §11, §12, §8.5; Plan 4).

mod common;
mod fixtures_context;

use aegis_core::context::plan::{Planner, StepKind, propose, run};
use aegis_core::context::reunite::{
    is_reunite_batch, propose_reunite, reunite_plans, run_reunite, undo_reunite,
};
use aegis_core::context::store::{list_collections, list_members};
use aegis_core::context::{
    Answer, Collection, CollectionKind, DecisionKind, Relationship, answer, recognise,
};
use aegis_core::db::SqliteStore;
use aegis_core::db::actions::{approve_batch, execute_batch};
use aegis_core::engine::note::{WeeklyNote, build as build_note};
use aegis_core::filing::batches::{after_execute, next_batch, recent_batches};
use aegis_core::filing::blueprint::plan as filing_plan;
use aegis_core::filing::reader::{classify_unread, read_pending};
use aegis_core::filing::{BlueprintChoice, Scope, change_blueprint};
use aegis_core::scanner::scan_root;
use common::{canon, put};
use fixtures_context::private_quarantine;
use std::path::{Path, PathBuf};
use std::time::{Duration, SystemTime};
use tempfile::TempDir;

fn setup_pc() -> (TempDir, SqliteStore, PathBuf, PathBuf, PathBuf, PathBuf) {
    let t = TempDir::new().unwrap();
    let me = t.path().join("me");
    let pics = me.join("Pictures");
    let docs = me.join("Documents");
    let dl = me.join("Downloads");
    let prj = t.path().join("Prj");
    for d in [&pics, &docs, &dl, &prj] {
        std::fs::create_dir_all(d).unwrap();
    }
    let s = SqliteStore::open_in_memory().unwrap();
    for r in [&pics, &docs, &dl, &prj] {
        s.add_root(r).unwrap();
    }
    private_quarantine(&s, t.path());
    (t, s, pics, dl, docs, prj)
}

fn rescan(s: &mut SqliteStore, dirs: &[&Path]) {
    for d in dirs {
        scan_root(s, d, &[1]).unwrap();
    }
    read_pending(s, 5_000, Duration::from_secs(30)).unwrap();
    classify_unread(s, 5_000).unwrap();
}

fn file_all_fresh(s: &mut SqliteStore) -> String {
    change_blueprint(s, &Scope::Pc, BlueprintChoice::Fresh).unwrap();
    let mut last = None;
    let mut n = 0usize;
    while let Some(b) = next_batch(s, &Scope::Pc).unwrap() {
        n += 1;
        assert!(n <= 10, "filing loop exceeded 10 batches");
        approve_batch(s, &b, None).unwrap();
        execute_batch(s, &b).unwrap();
        after_execute(s, &b).unwrap();
        last = Some(b);
    }
    last.expect("expected at least one Fresh filing batch")
}

fn cols_of(s: &SqliteStore, kind: CollectionKind) -> Vec<Collection> {
    list_collections(s.conn(), kind).unwrap()
}

fn set_mtime_year(path: &Path, unix_secs: u64) {
    let f = std::fs::File::options().write(true).open(path).unwrap();
    f.set_modified(SystemTime::UNIX_EPOCH + Duration::from_secs(unix_secs)).unwrap();
}

#[test]
fn reunite_returns_split_project_files_preserves_subpaths_and_undoes_cleanly() {
    let (_t, mut s, pics, dl, docs, prj) = setup_pc();
    let ts_2026_08 = 1_786_000_000;

    // An earlier filing batch moved photos/files out of a folder before it grew into a project.
    let scattered = put(&docs, "MyApp/assets/banner.jpg", b"banner-bytes");
    let root_img = put(&docs, "MyApp/cover.jpg", b"cover-bytes");
    set_mtime_year(&scattered, ts_2026_08);
    set_mtime_year(&root_img, ts_2026_08);
    rescan(&mut s, &[&docs]);
    let split_batch = file_all_fresh(&mut s);
    assert!(!scattered.exists() && !root_img.exists());

    // Now MyApp becomes a code project at Documents\MyApp.
    put(&docs, "MyApp/.git/HEAD", b"ref: refs/heads/main\n");
    put(&docs, "MyApp/Cargo.toml", b"[package]\nname=\"my-app\"\nversion=\"0.1.0\"\n");
    put(&docs, "MyApp/src/main.rs", b"fn main() {}\n");
    rescan(&mut s, &[&docs, &pics, &dl, &prj]);
    recognise(&mut s).unwrap();

    let plans = reunite_plans(&s).unwrap();
    let rp = plans.iter().find(|p| p.collection_name == "MyApp").unwrap();
    assert_eq!(rp.collection_kind, CollectionKind::Project);
    assert_eq!(rp.items.len(), 2);
    assert_eq!(rp.split_batches.len(), 1);
    assert_eq!(rp.split_batches[0].batch_id, split_batch);
    assert!(rp.items.iter().any(|i| i.to.eq_ignore_ascii_case(&format!(r"{}\MyApp\assets\banner.jpg", canon(&docs)))));
    assert!(rp.items.iter().any(|i| i.to.eq_ignore_ascii_case(&format!(r"{}\MyApp\cover.jpg", canon(&docs)))));

    let batch = propose_reunite(&mut s, rp).unwrap();
    assert!(is_reunite_batch(s.conn(), &batch).unwrap());
    let out = run_reunite(&mut s, &batch).unwrap();
    assert!(out.done, "{out:?}");
    assert_eq!(out.moved_files, 2);
    assert!(scattered.is_file() && root_img.is_file());
    assert!(reunite_plans(&s).unwrap().iter().all(|p| p.collection_name != "MyApp"));

    let recent = recent_batches(s.conn(), 10).unwrap();
    assert!(recent.iter().any(|r| r.id == batch && r.project.as_deref() == Some("Put back together: MyApp") && r.can_undo));

    let rb = undo_reunite(&mut s, &batch).unwrap();
    assert!(rb.conflicts.is_empty(), "{rb:?}");
    assert!(!scattered.exists() && !root_img.exists());
    // Reunite undo restores the earlier state cleanly, so the files are offered to reunite again.
    assert!(reunite_plans(&s).unwrap().iter().any(|p| p.collection_name == "MyApp"));
}

#[test]
fn reunite_follows_project_home_that_moved_to_projects_home() {
    let (_t, mut s, pics, dl, docs, prj) = setup_pc();
    let ts_2026_08 = 1_786_000_000;

    // Need >= 2 projects in WorkBox so Alpha's home is eligible to move into Prj.
    let work = docs.join("WorkBox");
    put(&work, "Beta/.git/HEAD", b"ref: refs/heads/main\n");
    put(&work, "Beta/Cargo.toml", b"[package]\nname=\"beta\"\n");
    put(&prj, "Seed/.git/HEAD", b"ref: refs/heads/main\n");
    put(&prj, "Seed/Cargo.toml", b"[package]\nname=\"seed\"\n");

    let doc_file = put(&work, "Alpha/diagrams/flow.jpg", b"flow-bytes");
    set_mtime_year(&doc_file, ts_2026_08);
    rescan(&mut s, &[&docs, &prj]);
    file_all_fresh(&mut s);

    // Alpha is now a repo and moves via MoveHome to Prj\Alpha.
    put(&work, "Alpha/.git/HEAD", b"ref: refs/heads/main\n");
    put(&work, "Alpha/Cargo.toml", b"[package]\nname=\"alpha\"\n");
    put(&work, "Alpha/src/lib.rs", b"pub fn x() {}\n");
    rescan(&mut s, &[&docs, &pics, &dl, &prj]);
    recognise(&mut s).unwrap();

    let alpha = cols_of(&s, CollectionKind::Project).into_iter().find(|c| c.name == "Alpha").unwrap();
    let move_plan = Planner::new(&s).unwrap().plan_for(&s, &alpha).unwrap().remove(0);
    assert_eq!(move_plan.steps[0].kind, StepKind::MoveHome);
    let mb = propose(&mut s, &alpha, &move_plan).unwrap();
    assert!(run(&mut s, &mb).unwrap().done);
    assert!(prj.join("Alpha/Cargo.toml").is_file());

    // Reunite traces MoveHome back to WorkBox\Alpha and returns flow.jpg into Prj\Alpha\diagrams\flow.jpg.
    let rps = reunite_plans(&s).unwrap();
    let rp = rps.iter().find(|p| p.collection_name == "Alpha").unwrap();
    assert_eq!(rp.items.len(), 1);
    assert!(rp.items[0].to.eq_ignore_ascii_case(&format!(r"{}\Alpha\diagrams\flow.jpg", canon(&prj))));

    let rb = propose_reunite(&mut s, rp).unwrap();
    let out = run_reunite(&mut s, &rb).unwrap();
    assert!(out.done, "{out:?}");
    assert!(prj.join("Alpha/diagrams/flow.jpg").is_file());
    let _ = pics;
}

#[test]
fn reunite_design_set_and_photo_subject_update_members_learn_and_avoid_loops() {
    let (_t, mut s, pics, dl, _docs, prj) = setup_pc();
    let ts_2026_08 = 1_786_000_000;

    // Earlier Fresh filing moves loose items to Pictures\2026\2026-08 before the full sets exist.
    let ip1 = put(&dl, "ipadcover.jpg", b"ip1");
    let ip2 = put(&dl, "ipadcover2.jpg", b"ip2");
    let sl1 = put(&dl, "SmoothLite.jpg", b"sl1");
    for f in [&ip1, &ip2, &sl1] {
        set_mtime_year(f, ts_2026_08);
    }
    rescan(&mut s, &[&dl, &pics]);
    file_all_fresh(&mut s);

    // Remaining files arrive to complete the PhotoSubject and DesignSet.
    let ip3 = put(&dl, "ipadcover3.jpg", b"ip3");
    set_mtime_year(&ip3, ts_2026_08);
    put(&dl, "SmoothLite-Front.xcf", b"sl-xcf");
    put(&dl, "SmoothLite-Box.pdf", b"sl-pdf");
    rescan(&mut s, &[&dl, &pics]);
    recognise(&mut s).unwrap();

    let rps = reunite_plans(&s).unwrap();
    let ds_rp = rps.iter().find(|p| p.collection_kind == CollectionKind::DesignSet && p.collection_name == "SmoothLite").unwrap();
    assert_eq!(ds_rp.items.len(), 1);
    assert!(ds_rp.target_dir.eq_ignore_ascii_case(&format!(r"{}\Design\SmoothLite", canon(&prj))));

    let ps_rp = rps.iter().find(|p| p.collection_kind == CollectionKind::PhotoSubject && p.collection_name == "ipadcover").unwrap();
    assert_eq!(ps_rp.items.len(), 2);
    assert!(ps_rp.target_dir.eq_ignore_ascii_case(&format!(r"{}\2026\ipadcover", canon(&pics))));

    // Reunite PhotoSubject -> moves into Pictures\2026\ipadcover, learns subject, never loops in Fresh, and undoes cleanly.
    let pb = propose_reunite(&mut s, ps_rp).unwrap();
    let pout = run_reunite(&mut s, &pb).unwrap();
    assert!(pout.done && pout.moved_files == 2);
    assert!(pics.join("2026/ipadcover/ipadcover.jpg").is_file());
    assert!(pics.join("2026/ipadcover/ipadcover2.jpg").is_file());
    let learned_photo: i64 = s
        .conn()
        .query_row(
            "SELECT COUNT(*) FROM context_decisions WHERE kind = ?1 AND lower(value) = 'photo:ipadcover'",
            [DecisionKind::LearnedSubject.as_str()],
            |r| r.get(0),
        )
        .unwrap();
    assert_eq!(learned_photo, 1);
    let fresh_after = filing_plan(&s, &Scope::Pc, BlueprintChoice::Fresh).unwrap();
    for moved in [canon(&pics.join("2026/ipadcover/ipadcover.jpg")), canon(&pics.join("2026/ipadcover/ipadcover2.jpg"))] {
        let entry = fresh_after.iter().find(|p| p.from.eq_ignore_ascii_case(&moved)).unwrap();
        assert!(entry.to.is_none(), "reunited photo subject looped: {entry:?}");
    }

    let prb = undo_reunite(&mut s, &pb).unwrap();
    assert!(prb.conflicts.is_empty(), "{prb:?}");
    assert!(!pics.join("2026/ipadcover").exists(), "empty subject folder removed on undo");

    // Reunite DesignSet -> moves SmoothLite.jpg into Prj\Design\SmoothLite and undo cleans empty Prj\Design too.
    let db = propose_reunite(&mut s, ds_rp).unwrap();
    assert!(run_reunite(&mut s, &db).unwrap().done);
    assert!(prj.join("Design/SmoothLite/SmoothLite.jpg").is_file());
    let members = list_members(s.conn(), &ds_rp.collection_id).unwrap();
    assert!(members.iter().any(|m| m.path.to_lowercase().contains(r"\design\smoothlite\smoothlite.jpg")));
    undo_reunite(&mut s, &db).unwrap();
    assert!(!prj.join("Design/SmoothLite").exists());
    assert!(!prj.join("Design").exists());
}

#[test]
fn reunite_handles_collisions_and_refuses_stale_sources() {
    let (_t, mut s, pics, dl, docs, prj) = setup_pc();
    let ts_2026_08 = 1_786_000_000;

    let orig = put(&docs, "SoloApp/img/photo.jpg", b"old-photo");
    set_mtime_year(&orig, ts_2026_08);
    rescan(&mut s, &[&docs]);
    file_all_fresh(&mut s);

    // Put a different file at the original path so reunion must suffix instead of overwriting.
    let existing = put(&docs, "SoloApp/img/photo.jpg", b"existing-new-bytes");
    put(&docs, "SoloApp/.git/HEAD", b"ref: refs/heads/main\n");
    put(&docs, "SoloApp/Cargo.toml", b"[package]\nname=\"solo-app\"\n");
    rescan(&mut s, &[&docs, &pics, &dl, &prj]);
    recognise(&mut s).unwrap();

    let rp = reunite_plans(&s).unwrap().into_iter().find(|p| p.collection_name == "SoloApp").unwrap();
    assert_eq!(rp.items.len(), 1);
    let d1 = rp.digest.clone();
    let batch = propose_reunite(&mut s, &rp).unwrap();
    assert!(run_reunite(&mut s, &batch).unwrap().done);
    assert_eq!(std::fs::read(&existing).unwrap(), b"existing-new-bytes");
    assert_eq!(std::fs::read(docs.join("SoloApp/img/photo (2).jpg")).unwrap(), b"old-photo");

    // Once reunited, re-planning returns nothing for SoloApp, and deleting source causes safe error if stale plan reused.
    assert!(reunite_plans(&s).unwrap().iter().all(|p| p.collection_name != "SoloApp"));
    assert!(!d1.is_empty());
    let mut missing_plan = rp;
    missing_plan.items[0].from = docs.join("missing-now.png").to_string_lossy().to_string();
    assert!(propose_reunite(&mut s, &missing_plan).is_err());
}

#[test]
fn same_as_teaches_normalized_name_aliases_and_not_same_as_revokes() {
    let (_t, mut s, _pics, dl, docs, prj) = setup_pc();

    // Pair 1: distinct folder names with unique >= 1 KB content, 0 shared files -> separate cards at first.
    put(&prj, "Seed/NutriP/Cargo.toml", b"[package]\nname=\"nutrip-a\"\n");
    put(&prj, "Seed/NutriP/src/main.rs", &vec![b'a'; 2048]);
    put(&docs, "Work/Nutrition/Cargo.toml", b"[package]\nname=\"nutrip-b\"\n");
    put(&docs, "Work/Nutrition/src/lib.rs", &vec![b'b'; 2048]);
    rescan(&mut s, &[&prj, &docs]);
    recognise(&mut s).unwrap();

    let cols = cols_of(&s, CollectionKind::Project);
    let c_a = cols.iter().find(|c| c.name == "NutriP").unwrap();
    let c_b = cols.iter().find(|c| c.name == "Nutrition").unwrap();
    answer(&mut s, &c_a.id, None, Answer::SameAs { other_collection_id: c_b.id.clone() }).unwrap();

    // Brand-new pair of projects elsewhere sharing the learned normalized alias ("nutrip" <-> "nutrition").
    put(&prj, "Group2/NutriP_app/package.json", b"{}");
    put(&prj, "Group2/NutriP_app/src/index.ts", &vec![b'c'; 2048]);
    put(&prj, "Group2/NutriP_app/README.md", b"# a");
    put(&dl, "Archive/Nutrition_v2/package.json", b"{}");
    put(&dl, "Archive/Nutrition_v2/src/app.ts", &vec![b'd'; 2048]);
    put(&dl, "Archive/Nutrition_v2/CHANGELOG.md", b"# c");
    rescan(&mut s, &[&prj, &dl]);
    recognise(&mut s).unwrap();

    let after_alias = cols_of(&s, CollectionKind::Project);
    let grouped = after_alias
        .iter()
        .find(|c| c.locations.iter().any(|l| l.root_path.ends_with("NutriP_app")))
        .unwrap();
    assert_eq!(grouped.locations.len(), 2);
    assert!(grouped.locations.iter().any(|l| l.root_path.ends_with("Nutrition_v2")), "alias matched second copy");
    // 0 shared files -> non-Home location starts as Unknown ("Possibly the same project?").
    let non_home = grouped.locations.iter().find(|l| l.relationship != Relationship::Home).unwrap();
    assert_eq!(non_home.relationship, Relationship::Unknown);

    // Adding a shared non-trivial file upgrades the alias match cleanly.
    let shared_payload = vec![b'x'; 2048];
    put(&prj, "Group2/NutriP_app/shared.txt", &shared_payload);
    put(&dl, "Archive/Nutrition_v2/shared.txt", &shared_payload);
    rescan(&mut s, &[&prj, &dl]);
    recognise(&mut s).unwrap();
    let confirmed_group = cols_of(&s, CollectionKind::Project)
        .into_iter()
        .find(|c| c.locations.iter().any(|l| l.root_path.ends_with("NutriP_app")))
        .unwrap();
    assert_eq!(confirmed_group.locations.len(), 2);

    // Explicit NotSameAs splits them and revokes the learned alias for future fresh pairs.
    let split_loc = confirmed_group.locations.iter().find(|l| l.root_path.ends_with("Nutrition_v2")).unwrap();
    answer(&mut s, &confirmed_group.id, Some(&split_loc.id), Answer::NotSameAs).unwrap();
    let split_cols = cols_of(&s, CollectionKind::Project);
    let n1 = split_cols.iter().find(|c| c.locations.iter().any(|l| l.root_path.ends_with("NutriP_app"))).unwrap();
    let n2 = split_cols.iter().find(|c| c.locations.iter().any(|l| l.root_path.ends_with("Nutrition_v2"))).unwrap();
    assert_ne!(n1.id, n2.id);

    // A third fresh pair after revocation no longer merges on the old alias alone.
    put(&prj, "Group3/NutriP_3/package.json", b"{}");
    put(&prj, "Group3/NutriP_3/src/a.ts", &vec![b'e'; 2048]);
    put(&docs, "Group4/Nutrition_3/package.json", b"{}");
    put(&docs, "Group4/Nutrition_3/src/b.ts", &vec![b'f'; 2048]);
    rescan(&mut s, &[&prj, &docs]);
    recognise(&mut s).unwrap();
    let final_cols = cols_of(&s, CollectionKind::Project);
    let c3 = final_cols.iter().find(|c| c.locations.iter().any(|l| l.root_path.ends_with("NutriP_3"))).unwrap();
    assert!(!c3.locations.iter().any(|l| l.root_path.ends_with("Nutrition_3")));
}

#[test]
fn weekly_note_projects_line_summarises_multi_place_and_reunite_and_deserialises_legacy_json() {
    // Legacy WeeklyNote JSON saved before `projects` was added deserializes with `projects: None`.
    let legacy = r#"{"week":"2026-11-22","headline":"A quiet week.","needs_you":[],"did":[],"safety":null,"space":[],"tidy":null,"about_aegis":[],"read":false}"#;
    let parsed: WeeklyNote = serde_json::from_str(legacy).unwrap();
    assert!(parsed.projects.is_none());

    let (_t, mut s, pics, dl, docs, prj) = setup_pc();
    let ts_2026_08 = 1_786_000_000;
    let empty_note = build_note(s.conn(), "2026-11-23", 0, "2026-11-23", 1_795_000_000).unwrap();
    assert!(empty_note.projects.is_none());

    // Create 1 project in 2 places + 1 left_as_is project (excluded) + 1 reunite item.
    put(&prj, "RepoA/.git/HEAD", b"ref: refs/heads/main\n");
    put(&prj, "RepoA/Cargo.toml", b"[package]\nname=\"repo-a\"\n");
    put(&prj, "RepoA/src/main.rs", b"fn main(){}\n");
    put(&docs, "Copy/RepoA/.git/HEAD", b"ref: refs/heads/main\n");
    put(&docs, "Copy/RepoA/Cargo.toml", b"[package]\nname=\"repo-a\"\n");
    put(&docs, "Copy/RepoA/src/main.rs", b"fn main(){}\n");

    let split_img = put(&docs, "SplitProj/icon.jpg", b"ico");
    set_mtime_year(&split_img, ts_2026_08);
    rescan(&mut s, &[&docs, &prj]);
    file_all_fresh(&mut s);
    put(&docs, "SplitProj/.git/HEAD", b"ref: refs/heads/main\n");
    put(&docs, "SplitProj/Cargo.toml", b"[package]\nname=\"split-proj\"\n");
    rescan(&mut s, &[&docs, &pics, &dl, &prj]);
    recognise(&mut s).unwrap();

    let note = build_note(s.conn(), "2026-11-23", 0, "2026-11-23", 1_795_000_000).unwrap();
    let summary = note.projects.expect("projects summary should be populated");
    assert!(summary.contains("1 project has copies in 2 places."), "{summary}");
    assert!(summary.contains("1 group has files from earlier batches to put back together."), "{summary}");
}
