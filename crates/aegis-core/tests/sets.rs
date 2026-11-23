//! Sets and photo subjects (spec 2026-11-22 §10, §17; Plan 3).

mod common;
mod fixtures_context;

use aegis_core::backup::plan::apply_plan;
use aegis_core::context::plan::{Planner, StepKind, plan_for, propose, run, undo};
use aegis_core::context::store::{list_collections, list_members};
use aegis_core::context::{Answer, Collection, CollectionKind, DecisionKind, ProjectKind, Relationship, answer, photo_subjects, protected_files, recognise};
use aegis_core::contracts::DriveRole;
use aegis_core::db::SqliteStore;
use aegis_core::db::actions::{approve_batch, execute_batch, propose_batch};
use aegis_core::drives::{FakeVolumeProvider, sync_mounted};
use aegis_core::filing::batches::{after_execute, next_batch, recheck_open_batch};
use aegis_core::filing::blueprint::plan as filing_plan;
use aegis_core::filing::reader::{classify_unread, read_pending};
use aegis_core::filing::{BlueprintChoice, Scope, change_blueprint};
use aegis_core::scanner::scan_root;
use common::{canon, fake_drive, plug_and_catalog, put};
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
    classify_unread(s, 5_000).unwrap();
    read_pending(s, 5_000, Duration::from_secs(30)).unwrap();
}

fn set_mtime_year(p: &Path, secs_since_epoch: u64) {
    let f = std::fs::OpenOptions::new().write(true).open(p).unwrap();
    f.set_modified(SystemTime::UNIX_EPOCH + Duration::from_secs(secs_since_epoch)).unwrap();
}

fn sets_of(s: &SqliteStore, kind: CollectionKind) -> Vec<Collection> {
    list_collections(s.conn(), kind).unwrap()
}

#[test]
fn smoothlite_and_back_cement_recognised_as_design_sets_without_false_positives() {
    let (_t, mut s, pics, dl, _docs, _prj) = setup_pc();
    // SmoothLite across Pictures, Pictures/2026/2026-10, and Downloads.
    for f in [
        "SmoothLite Front (CMYK).pdf",
        "SmoothLite Front (CMYK).rar",
        "SmoothLite Front (CMYK).zip",
        "SmoothLite Front (sleeve).png",
        "SmoothLite-Front.xcf",
        "SmoothLite-Back.png",
    ] {
        put(&pics, f, b"smoothlite artwork bytes");
    }
    put(&pics, "2026/2026-10/SmoothLite 2.jpg", b"smoothlite photo");
    put(&dl, "SmoothLite_Back_With_New_QR.pdf", b"smoothlite qr pdf");
    put(&dl, "SmoothLite_Back_With_New_QR.png", b"smoothlite qr png");

    // Multi-word DesignSet: Back Cement (multi-format artwork with shared stem).
    put(&pics, "back-cement.pdf", b"cement pdf");
    put(&pics, "back cement-4.png", b"cement png");
    put(&pics, "back-cement-50-2.png", b"cement 50 png");
    put(&dl, "back cement-3.pdf", b"cement 3 pdf");
    put(&dl, "back cement-3.png", b"cement 3 png");

    // Negative cases: generic camera/image prefixes, bare numbers, installers, Office locks, videos.
    for i in 1..=4 {
        put(&pics, &format!("2026/2026-10/IMG_20261012_{i:03}.jpg"), b"camera");
        put(&pics, &format!("2026/2026-10/image_{i:03}.jpg"), b"img");
        put(&pics, &format!("2026/2026-10/100033445{i}.jpg"), b"phone");
        put(&dl, &format!("ChromeSetup ({i}).exe"), b"exe");
        put(&dl, &format!("Age Analysis {i}.xlsx"), b"xlsx");
        put(&dl, &format!("~$TempDoc{i}.png"), b"temp");
        put(&dl, &format!("Guardians_{i}.mkv"), b"mkv");
    }

    rescan(&mut s, &[&pics, &dl]);
    let rep = recognise(&mut s).unwrap();
    assert_eq!(rep.design_sets, 2, "{rep:?}");
    assert_eq!(rep.photo_subjects, 0, "{rep:?}");

    let ds = sets_of(&s, CollectionKind::DesignSet);
    let smooth = ds.iter().find(|c| c.name == "SmoothLite").unwrap();
    assert_eq!(smooth.project_kind, Some(ProjectKind::Design));
    assert_eq!(smooth.locations.len(), 3, "{smooth:?}");
    assert!(smooth.locations.iter().all(|l| l.relationship == Relationship::SetFiles));
    assert!(ds.iter().any(|c| c.name == "Back Cement"));

    // Sets live in collection_members, never in collection_locations.
    let loc_count: i64 = s
        .conn()
        .query_row(
            "SELECT COUNT(*) FROM collection_locations l JOIN collections c ON c.id = l.collection_id WHERE c.kind != 'project'",
            [],
            |r| r.get(0),
        )
        .unwrap();
    assert_eq!(loc_count, 0);
}

#[test]
fn named_photo_album_is_not_split_into_photo_subjects() {
    let (_t, mut s, pics, _dl, _docs, _prj) = setup_pc();
    let mugs = pics.join("Undated/mugs");
    for f in [
        "mug1.png",
        "mug2.png",
        "mug3.png",
        "TeaStrainer.png",
        "TeaStrainer3.png",
        "TeaStrainer4.png",
        "TeaStrainer5.png",
        "travelcoffee.png",
        "travelcoffee1.png",
        "travelcoffee2.png",
        "ThermalTumb.png",
        "ThermalTumb1.png",
    ] {
        let p = put(&mugs, f, b"mug photo");
        set_mtime_year(&p, 1_785_000_000);
    }

    rescan(&mut s, &[&pics]);
    let rep = recognise(&mut s).unwrap();
    assert_eq!(rep.photo_subjects, 0, "{rep:?}");
    assert!(sets_of(&s, CollectionKind::PhotoSubject).is_empty());

    let ps = filing_plan(&s, &Scope::Pc, BlueprintChoice::Fresh).unwrap();
    assert!(
        ps.iter().any(|p| p.reason.contains("Photo album: mugs")),
        "named album should stay whole: {ps:?}"
    );
}

#[test]
fn design_set_files_are_protected_in_fresh_tidy_and_recheck_open_batch() {
    let (_t, mut s, pics, dl, _docs, _prj) = setup_pc();
    let f1 = put(&pics, "SmoothLite Front (sleeve).png", b"art1");
    let f2 = put(&pics, "SmoothLite-Front.xcf", b"art2");
    let f3 = put(&pics, "2026/2026-10/SmoothLite 2.jpg", b"art3");
    let f4 = put(&dl, "SmoothLite_Back_With_New_QR.png", b"art4");
    rescan(&mut s, &[&pics, &dl]);

    // Protection works even before the first `recognise` pass runs (never_ran fallback).
    for choice in [BlueprintChoice::Fresh, BlueprintChoice::Tidy] {
        let ps = filing_plan(&s, &Scope::Pc, choice).unwrap();
        for p_path in [&f1, &f2, &f3, &f4] {
            let want = canon(p_path);
            let pl = ps.iter().find(|x| x.from.eq_ignore_ascii_case(&want)).unwrap();
            assert!(pl.to.is_none(), "{choice:?} tried to move set file {want}: {pl:?}");
            assert!(pl.reason.contains("SmoothLite"), "{pl:?}");
        }
    }

    // Re-checking a stale waiting batch supersedes any move of a DesignSet member.
    use aegis_core::contracts::{ActionType, ProposedActionInput, RefilePlan, TargetKind};
    let stale_id = s
        .conn()
        .query_row("SELECT id FROM files WHERE path = ?1", [canon(&f1)], |r| r.get::<_, String>(0))
        .unwrap();
    let batch = propose_batch(
        &mut s,
        &RefilePlan {
            origin: "filing:pc".into(),
            actions: vec![ProposedActionInput {
                agent: Some("filing".into()),
                action_type: ActionType::Move,
                target_kind: TargetKind::File,
                file_id: Some(stale_id),
                atomic_unit_id: None,
                from_path: canon(&f1),
                to_path: Some(
                    Path::new(&canon(&pics))
                        .join("2026")
                        .join("2026-08")
                        .join("SmoothLite Front (sleeve).png")
                        .to_string_lossy()
                        .into_owned(),
                ),
                reason: "old proposal".into(),
                confidence: 0.9,
                evidence_json: "{}".into(),
            }],
        },
    )
    .unwrap();
    recognise(&mut s).unwrap();
    let retired = recheck_open_batch(&mut s, &Scope::Pc).unwrap();
    assert_eq!(retired, 1);
    let status: String = s
        .conn()
        .query_row("SELECT status FROM actions WHERE batch_id = ?1", [&batch], |r| r.get(0))
        .unwrap();
    assert_eq!(status, "superseded");
}

#[test]
fn photo_subjects_route_to_photos_year_subject_in_fresh_and_never_loop() {
    let (_t, mut s, pics, dl, _docs, _prj) = setup_pc();
    let ts_2026_08 = 1_786_000_000; // Aug 2026
    let p1 = put(&pics, "2026/2026-08/ipadcover.jpg", b"p1");
    let p2 = put(&pics, "2026/2026-08/ipadcover2.jpg", b"p2");
    let p3 = put(&pics, "2026/2026-08/ipadcover3.jpg", b"p3");
    let p4 = put(&dl, "ipadcover9v1.1.png", b"p4");
    let g1 = put(&pics, "2026/2026-08/IMG_001.jpg", b"g1");
    for f in [&p1, &p2, &p3, &p4, &g1] {
        set_mtime_year(f, ts_2026_08);
    }
    // First scan without `classify_unread` to verify the unclassified `.png` mtime year fallback.
    scan_root(&mut s, &pics, &[1]).unwrap();
    scan_root(&mut s, &dl, &[1]).unwrap();
    read_pending(&mut s, 5_000, Duration::from_secs(30)).unwrap();
    let rep = recognise(&mut s).unwrap();
    assert_eq!(rep.photo_subjects, 1, "{rep:?}");

    let ps = filing_plan(&s, &Scope::Pc, BlueprintChoice::Fresh).unwrap();
    let check_dest = |src: &Path, expected_leaf: &str| {
        let pl = ps.iter().find(|p| p.from.eq_ignore_ascii_case(&canon(src))).unwrap();
        let to = pl.to.as_deref().unwrap();
        assert!(to.to_lowercase().contains(r"\2026\ipadcover\"), "{to}");
        assert!(to.ends_with(expected_leaf), "{to}");
    };
    check_dest(&p1, "ipadcover.jpg");
    check_dest(&p4, "ipadcover9v1.1.png");
    let g_pl = ps.iter().find(|p| p.from.eq_ignore_ascii_case(&canon(&g1))).unwrap();
    assert!(g_pl.to.is_none(), "ungrouped photo stays in 2026-08: {g_pl:?}");

    // Now classify unread (.png) like ReaderWatch does, approve/execute Fresh, and verify no loop.
    classify_unread(&mut s, 5_000).unwrap();
    change_blueprint(&mut s, &Scope::Pc, BlueprintChoice::Fresh).unwrap();
    let mut batches = 0usize;
    while let Some(b) = next_batch(&mut s, &Scope::Pc).unwrap() {
        batches += 1;
        assert!(batches <= 10, "filing loop exceeded 10 batches");
        approve_batch(&mut s, &b, None).unwrap();
        execute_batch(&mut s, &b).unwrap();
        after_execute(&mut s, &b).unwrap();
    }
    for f in ["ipadcover.jpg", "ipadcover2.jpg", "ipadcover3.jpg", "ipadcover9v1.1.png"] {
        assert!(pics.join("2026/ipadcover").join(f).is_file(), "missing filed {f}");
    }
    recognise(&mut s).unwrap();
    let after = filing_plan(&s, &Scope::Pc, BlueprintChoice::Fresh).unwrap();
    for f in ["ipadcover.jpg", "ipadcover2.jpg", "ipadcover3.jpg", "ipadcover9v1.1.png"] {
        let moved = canon(&pics.join("2026/ipadcover").join(f));
        let pl = after.iter().find(|p| p.from.eq_ignore_ascii_case(&moved)).unwrap();
        assert!(pl.to.is_none(), "filed photo subject looped: {pl:?}");
    }
}

#[test]
fn design_set_plan_gathers_files_handles_collisions_and_undo_restores_cleanly() {
    let (_t, mut s, pics, dl, _docs, prj) = setup_pc();
    let a = put(&pics, "SmoothLite-Front.xcf", b"xcf-front");
    let b_pics = put(&pics, "SmoothLite.png", b"png-from-pictures");
    let b_dl = put(&dl, "SmoothLite.png", b"png-from-downloads");
    let d = put(&dl, "SmoothLite_QR.pdf", b"qr-pdf");
    let e = put(&pics, "2026/2026-10/SmoothLite 2.jpg", b"jpg-2026");

    rescan(&mut s, &[&pics, &dl]);
    recognise(&mut s).unwrap();

    let smooth = sets_of(&s, CollectionKind::DesignSet).into_iter().find(|c| c.name == "SmoothLite").unwrap();
    let plans = Planner::new(&s).unwrap().plan_for(&s, &smooth).unwrap();
    assert_eq!(plans.len(), 1);
    let plan = &plans[0];
    assert_eq!(plan.steps.len(), 3);
    assert!(plan.steps.iter().all(|st| st.kind == StepKind::GatherFiles));
    assert!(plan.home_after.eq_ignore_ascii_case(&canon(&prj.join("SmoothLite"))));

    let batch = propose(&mut s, &smooth, plan).unwrap();
    let out = run(&mut s, &batch).unwrap();
    assert!(out.done, "{out:?}");
    assert_eq!(out.moved_files, 5);

    let dest_dir = prj.join("SmoothLite");
    assert!(dest_dir.join("SmoothLite-Front.xcf").is_file());
    assert!(dest_dir.join("SmoothLite_QR.pdf").is_file());
    assert!(dest_dir.join("SmoothLite 2.jpg").is_file());
    assert!(dest_dir.join("SmoothLite.png").is_file());
    assert!(dest_dir.join("SmoothLite (2).png").is_file());
    for orig in [&a, &b_pics, &b_dl, &d, &e] {
        assert!(!orig.exists(), "original should have moved: {}", orig.display());
    }
    assert!(list_members(s.conn(), &smooth.id).unwrap().is_empty());

    // After gathering into C:\Prj\SmoothLite, recognition promotes it to a confirmed Design project.
    recognise(&mut s).unwrap();
    let projs = sets_of(&s, CollectionKind::Project);
    let moved_proj = projs.iter().find(|p| p.name == "SmoothLite").unwrap();
    assert_eq!(moved_proj.project_kind, Some(ProjectKind::Design));
    assert_eq!(moved_proj.state, "confirmed");

    // Undo moves all 5 files back to their exact original paths & removes empty Prj\SmoothLite.
    let rb = undo(&mut s, &batch).unwrap();
    assert!(rb.conflicts.is_empty(), "{rb:?}");
    assert_eq!(std::fs::read(&b_pics).unwrap(), b"png-from-pictures");
    assert_eq!(std::fs::read(&b_dl).unwrap(), b"png-from-downloads");
    assert!(a.is_file() && d.is_file() && e.is_file());
    assert!(!dest_dir.exists(), "empty destination folder should be cleaned up on undo");

    let hist_left: i64 = s
        .conn()
        .query_row("SELECT COUNT(*) FROM file_history WHERE batch_id = ?1", [&batch], |r| r.get(0))
        .unwrap();
    assert_eq!(hist_left, 0);

    recognise(&mut s).unwrap();
    assert!(sets_of(&s, CollectionKind::DesignSet).iter().any(|c| c.name == "SmoothLite" && c.locations.len() == 3));
}

#[test]
fn design_set_plan_toctou_digest_and_mid_batch_rollback() {
    let (_t, mut s, pics, dl, _docs, _prj) = setup_pc();
    let _f1 = put(&pics, "SmoothLite-Front.xcf", b"f1");
    let f2 = put(&dl, "SmoothLite_QR.pdf", b"f2");
    let _f3 = put(&pics, "SmoothLite.png", b"f3");
    rescan(&mut s, &[&pics, &dl]);
    recognise(&mut s).unwrap();

    let ds1 = sets_of(&s, CollectionKind::DesignSet).into_iter().find(|c| c.name == "SmoothLite").unwrap();
    let p1 = Planner::new(&s).unwrap().plan_for(&s, &ds1).unwrap().remove(0);

    // Part A: adding a new member changes the plan digest.
    let f4 = put(&dl, "SmoothLite_Extra.png", b"f4");
    rescan(&mut s, &[&dl]);
    recognise(&mut s).unwrap();
    let ds2 = sets_of(&s, CollectionKind::DesignSet).into_iter().find(|c| c.name == "SmoothLite").unwrap();
    let p2 = Planner::new(&s).unwrap().plan_for(&s, &ds2).unwrap().remove(0);
    assert_ne!(p1.digest, p2.digest);

    // Part B: delete a proposed member on disk after `propose` so execution fails mid-batch and rolls back.
    let batch = propose(&mut s, &ds2, &p2).unwrap();
    std::fs::remove_file(&f4).unwrap();
    let out = run(&mut s, &batch).unwrap();
    assert!(!out.done, "{out:?}");
    assert!(f2.is_file(), "rolled-back file must still be at its original path");
}

#[test]
fn owner_answers_on_design_set_persist_across_recognise() {
    let (_t, mut s, pics, dl, _docs, _prj) = setup_pc();
    put(&pics, "SmoothLite-Front.xcf", b"1");
    put(&pics, "SmoothLite.png", b"2");
    put(&pics, "SmoothLite-Back.png", b"3");
    put(&dl, "SmoothLite_QR.pdf", b"4");
    put(&dl, "SmoothLite_QR.png", b"5");
    rescan(&mut s, &[&pics, &dl]);
    recognise(&mut s).unwrap();

    let ds = sets_of(&s, CollectionKind::DesignSet).into_iter().find(|c| c.name == "SmoothLite").unwrap();
    assert!(answer(&mut s, &ds.id, None, Answer::ChangeHome).is_err());

    // Rename persists across recognition.
    answer(&mut s, &ds.id, None, Answer::Rename { name: "BrandX".into() }).unwrap();
    let ds = sets_of(&s, CollectionKind::DesignSet).into_iter().find(|c| c.name == "BrandX").unwrap();

    // KeepHere keeps the Downloads location out of plan steps.
    let dl_id = ds.locations.iter().find(|l| l.root_path.eq_ignore_ascii_case(&canon(&dl))).unwrap().id.clone();
    answer(&mut s, &ds.id, Some(&dl_id), Answer::KeepHere { keep: true }).unwrap();
    let ds = sets_of(&s, CollectionKind::DesignSet).into_iter().find(|c| c.name == "BrandX").unwrap();
    let plan = plan_for(&s, &ds).unwrap().remove(0);
    assert_eq!(plan.steps.len(), 1);
    assert!(plan.notes.iter().any(|n| n.contains("Keep here")), "{plan:?}");

    // NotSameAs excludes Downloads from the set while keeping Pictures intact.
    let dl_id = ds.locations.iter().find(|l| l.root_path.eq_ignore_ascii_case(&canon(&dl))).unwrap().id.clone();
    answer(&mut s, &ds.id, Some(&dl_id), Answer::NotSameAs).unwrap();
    let ds = sets_of(&s, CollectionKind::DesignSet).into_iter().find(|c| c.name == "BrandX").unwrap();
    assert_eq!(ds.locations.len(), 1);

    // LeaveAsIs empties plan steps while keeping files protected.
    answer(&mut s, &ds.id, None, Answer::LeaveAsIs).unwrap();
    let ds = sets_of(&s, CollectionKind::DesignSet).into_iter().find(|c| c.name == "BrandX").unwrap();
    assert_eq!(ds.state, "left_as_is");
    assert!(plan_for(&s, &ds).unwrap().is_empty());
    assert!(!protected_files(&s, &[canon(&pics)]).unwrap().is_empty());

    // IsProject { yes: false } drops the set and stops protecting its files.
    answer(&mut s, &ds.id, None, Answer::IsProject { yes: false }).unwrap();
    assert!(sets_of(&s, CollectionKind::DesignSet).is_empty());
    assert!(protected_files(&s, &[canon(&pics)]).unwrap().is_empty());
}

#[test]
fn learned_subjects_match_single_new_arrival_for_design_and_photo_subjects() {
    let (_t, mut s, pics, dl, _docs, prj) = setup_pc();
    let ts_2026_08 = 1_786_000_000;
    // Initial 3-file SmoothLite design set + 3-file ipadcover photo subject.
    put(&pics, "SmoothLite-Front.xcf", b"d1");
    put(&pics, "SmoothLite-Front.png", b"d2");
    put(&pics, "SmoothLite-Back.png", b"d3");
    for n in ["ipadcover.jpg", "ipadcover2.jpg", "ipadcover3.jpg"] {
        let p = put(&pics, &format!("2026/2026-08/{n}"), b"ph");
        set_mtime_year(&p, ts_2026_08);
    }
    rescan(&mut s, &[&pics, &dl]);
    recognise(&mut s).unwrap();

    // Run SmoothLite DesignSet plan (moves files to Prj\SmoothLite & records LearnedSubject design).
    let ds = sets_of(&s, CollectionKind::DesignSet).into_iter().find(|c| c.name == "SmoothLite").unwrap();
    let plan = plan_for(&s, &ds).unwrap().remove(0);
    let batch = propose(&mut s, &ds, &plan).unwrap();
    assert!(run(&mut s, &batch).unwrap().done);

    // Run Fresh filing (moves ipadcover to Photos\2026\ipadcover & records LearnedSubject photo).
    change_blueprint(&mut s, &Scope::Pc, BlueprintChoice::Fresh).unwrap();
    let mut batches = 0usize;
    while let Some(b) = next_batch(&mut s, &Scope::Pc).unwrap() {
        batches += 1;
        assert!(batches <= 10, "filing loop exceeded 10 batches");
        approve_batch(&mut s, &b, None).unwrap();
        execute_batch(&mut s, &b).unwrap();
        after_execute(&mut s, &b).unwrap();
    }

    // One single new arrival for each subject arrives in Downloads.
    let stray_design = put(&dl, "SmoothLite_Promo.png", b"d4");
    let stray_photo = put(&dl, "ipadcover_new.jpg", b"p4");
    set_mtime_year(&stray_photo, ts_2026_08);
    rescan(&mut s, &[&dl]);
    recognise(&mut s).unwrap();

    // Single stray design file is caught as DesignSet targeting existing Prj\SmoothLite!
    let ds_again = sets_of(&s, CollectionKind::DesignSet).into_iter().find(|c| c.name == "SmoothLite").unwrap();
    assert_eq!(ds_again.state, "confirmed");
    let plan_again = plan_for(&s, &ds_again).unwrap().remove(0);
    assert!(plan_again.home_after.eq_ignore_ascii_case(&canon(&prj.join("SmoothLite"))));
    let _ = stray_design;

    // Single new ipadcover photo is caught as PhotoSubject and routed by Fresh to 2026/ipadcover.
    let subjects = photo_subjects(&s, &[canon(&dl)]).unwrap();
    assert!(subjects.contains_key(&canon(&stray_photo).to_lowercase()), "{subjects:?}");
    let ps = filing_plan(&s, &Scope::Pc, BlueprintChoice::Fresh).unwrap();
    let pl = ps.iter().find(|p| p.from.eq_ignore_ascii_case(&canon(&stray_photo))).unwrap();
    assert!(pl.to.as_deref().unwrap().to_lowercase().contains(r"\2026\ipadcover\ipadcover_new.jpg"), "{pl:?}");
    let learned_rows: i64 = s
        .conn()
        .query_row(
            "SELECT COUNT(*) FROM context_decisions WHERE kind = ?1",
            [DecisionKind::LearnedSubject.as_str()],
            |r| r.get(0),
        )
        .unwrap();
    assert!(learned_rows >= 2);
}

#[test]
fn sets_respect_volume_isolation_backup_exclusion_and_drive_relettering() {
    let t = TempDir::new().unwrap();
    let mut s = SqliteStore::open_in_memory().unwrap();
    private_quarantine(&s, t.path());
    let provider = FakeVolumeProvider::default();

    let drv_a = fake_drive(&t, "DriveA", "1111-AAA");
    let drv_b = fake_drive(&t, "DriveB", "2222-BBB");
    put(&drv_a.mount_point, "Pictures/SmoothLite.xcf", b"a1");
    put(&drv_a.mount_point, "Pictures/SmoothLite.png", b"a2");
    put(&drv_a.mount_point, "Downloads/SmoothLite.pdf", b"a3");
    put(&drv_b.mount_point, "Pictures/SmoothLite.xcf", b"b1");
    put(&drv_b.mount_point, "Pictures/SmoothLite.png", b"b2");
    put(&drv_b.mount_point, "Downloads/SmoothLite.pdf", b"b3");

    let vid_a = plug_and_catalog(&mut s, &provider, &drv_a);
    let vid_b = plug_and_catalog(&mut s, &provider, &drv_b);
    private_quarantine(&s, t.path());
    // Mark DriveB as Backup drive: Backup is never read or turned into collections/sets.
    apply_plan(&mut s, &[(vid_a.clone(), DriveRole::General), (vid_b.clone(), DriveRole::Backup)]).unwrap();
    recognise(&mut s).unwrap();

    let ds = sets_of(&s, CollectionKind::DesignSet);
    assert_eq!(ds.len(), 1, "Backup drive must produce no sets: {ds:?}");
    assert!(ds[0].locations.iter().all(|l| l.volume_id == vid_a));

    // Re-letter DriveA to a new mount path and confirm `collection_members.path` rebases cleanly.
    let new_mount = t.path().join("DriveA_Relettered");
    std::fs::rename(&drv_a.mount_point, &new_mount).unwrap();
    provider.unplug(&drv_a.mount_point);
    let mut drv_a2 = drv_a;
    drv_a2.mount_point = new_mount.clone();
    provider.plug(drv_a2);
    sync_mounted(&mut s, &provider).unwrap();

    let members = list_members(s.conn(), &ds[0].id).unwrap();
    assert_eq!(members.len(), 3);
    assert!(members.iter().all(|m| m.path.to_lowercase().starts_with(&canon(&new_mount).to_lowercase())), "{members:?}");
}

#[test]
fn sets_ignore_software_bundles_media_art_duplicate_copies_and_ebook_titles() {
    let (_t, mut s, pics, dl, docs, _prj) = setup_pc();
    // Music album folders with tracks + boilerplate cover art -> 0 sets.
    for (dir, track) in [
        ("Music/Album1", "track1.mp3"),
        ("Music/Album2", "track2.flac"),
        ("Music/Album3", "track3.mp3"),
    ] {
        put(&dl, &format!("{dir}/{track}"), b"audio");
        put(&dl, &format!("{dir}/Folder.jpg"), b"art");
        put(&dl, &format!("{dir}/AlbumArtSmall.jpg"), b"art");
        put(&dl, &format!("{dir}/AlbumArt_{{1111}}_Large.jpg"), b"art");
    }

    // Movie torrent folders with videos + companion poster -> 0 sets.
    for m in ["MovieA", "MovieB", "MovieC"] {
        put(&dl, &format!("Movies/{m}/movie.mkv"), b"vid");
        put(&dl, &format!("Movies/{m}/www.YTS.MX.jpg"), b"poster");
    }

    // Software/SDK subtree with dll + UI assets + 9-patch + sprite dimensions + positional back -> 0 sets.
    put(&dl, "AndroidSDK/lib/core.dll", b"dll");
    put(&dl, "AndroidSDK/plugins/resources/back.png", b"ui");
    put(&dl, "AndroidSDK/plugins/resources/back.webp", b"ui");
    put(&dl, "AndroidSDK/plugins/device-art-resources/back.png", b"ui");
    put(&dl, "AndroidSDK/plugins/resources/phone_back.9.png", b"9p");
    put(&dl, "AndroidSDK/plugins/resources/Product_32X32.png", b"ico");
    put(&dl, "AndroidSDK/plugins/resources/Product_64X64.png", b"ico");
    put(&dl, "AndroidSDK/plugins/resources/Product_128X128.png", b"ico");

    // Unrelated long ebook titles sharing only word 0 ("Black") -> 0 sets.
    put(&docs, "Books/Black Decker Complete Home Repair Guide.jpg", b"bk");
    put(&docs, "Books/Black Decker Complete Home Repair Guide.pdf", b"bk");
    put(&docs, "Books/Black Pepper.jpg", b"pep");
    put(&docs, "Books/Black Ladies Publishing.pdf", b"lad");

    // Duplicate copies of identical filename across backup folders -> 0 sets.
    put(&docs, "Copy1/tom.jpg", b"tom");
    put(&docs, "Copy2/tom.jpg", b"tom");
    put(&docs, "Copy3/tom.jpg", b"tom");

    // Real multi-format concise DesignSet ("Back Cement") still recognised cleanly.
    put(&pics, "back-cement.pdf", b"bc1");
    put(&pics, "back cement-4.png", b"bc2");
    put(&dl, "back cement-3.pdf", b"bc3");
    put(&dl, "back cement-3.png", b"bc4");

    rescan(&mut s, &[&pics, &dl, &docs]);
    let rep = recognise(&mut s).unwrap();
    assert_eq!(rep.design_sets, 1, "{rep:?}: {:?}", sets_of(&s, CollectionKind::DesignSet));
    assert_eq!(rep.photo_subjects, 0, "{rep:?}: {:?}", sets_of(&s, CollectionKind::PhotoSubject));
    assert_eq!(sets_of(&s, CollectionKind::DesignSet)[0].name, "Back Cement");
}
