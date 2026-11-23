//! Integration tests for Context-Aware Filing Plan 5 — Step 7 modules (spec §1A, §16 step 7, §17):
//! Software packages, Owner's folders, Resources, Series, Old copies, and External-drive project plans.

mod common;
mod fixtures_context;

use aegis_core::context::plan::{Planner, StepKind, plan_for, propose, run, undo};
use aegis_core::context::store::{list_collections, list_members};
use aegis_core::context::{Collection, CollectionKind, Relationship, recognise};
use aegis_core::contracts::{ActionType, ProposedActionInput, RefilePlan, TargetKind};
use aegis_core::db::SqliteStore;
use aegis_core::db::actions::{approve_batch, execute_batch, propose_batch};
use aegis_core::drives::{FakeVolumeProvider, sync_mounted};
use aegis_core::filing::batches::{after_execute, next_batch, origin_for, recheck_open_batch};
use aegis_core::filing::blueprint::{Target, plan as filing_plan};
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
    read_pending(s, 5_000, Duration::from_secs(30)).unwrap();
    classify_unread(s, 5_000).unwrap();
}

fn cols(s: &SqliteStore, kind: CollectionKind) -> Vec<Collection> {
    list_collections(s.conn(), kind).unwrap()
}

fn age_file(p: &Path, days: u64) {
    let f = std::fs::OpenOptions::new().write(true).open(p).unwrap();
    f.set_modified(SystemTime::now() - Duration::from_secs(days * 86_400)).unwrap();
}

#[test]
fn software_package_beats_readme_project_and_moves_whole_in_fresh() {
    let (_t, mut s, pics, dl, docs, prj) = setup_pc();

    put(&dl, "DDU v18.1.5.7/Display Driver Uninstaller.exe", &vec![b'e'; 2048]);
    put(&dl, "DDU v18.1.5.7/DDU.pdb", &vec![b'p'; 2048]);
    put(&dl, "DDU v18.1.5.7/readme.txt", b"Display Driver Uninstaller\nInstructions");
    put(&dl, "DDU v18.1.5.7/settings/NVIDIA/services.cfg", b"cfg=1\n");

    put(
        &dl,
        "Armoury_Crate_Uninstall_Tool/Armoury Crate Uninstall Tool V2.3.7.0/Armoury Crate Uninstall Tool.exe",
        &vec![b'a'; 2048],
    );
    put(
        &dl,
        "Armoury_Crate_Uninstall_Tool/Armoury Crate Uninstall Tool V2.3.7.0/System.Runtime.WindowsRuntime.dll",
        &vec![b'd'; 2048],
    );
    put(
        &dl,
        "Armoury_Crate_Uninstall_Tool/Armoury Crate Uninstall Tool V2.3.7.0/dll/Uninstaller.exe",
        &vec![b'u'; 2048],
    );

    put(&dl, "Microsoft Office 2024/setup.exe", &vec![b'o'; 2048]);
    put(&dl, "Microsoft Office 2024/files/x64/setup64.exe", &vec![b'6'; 2048]);
    put(&dl, "Microsoft Office 2024/files/x64/helper.dll", &vec![b'd'; 2048]);
    put(&dl, "Microsoft Office 2024/files/x86/setup32.exe", &vec![b'3'; 2048]);
    put(&dl, "Microsoft Office 2024/files/x86/helper.dll", &vec![b'd'; 2048]);
    put(&dl, "Microsoft Office 2024/Office/Data/i641025.cab", &vec![b'c'; 2048]);
    put(&dl, "Microsoft Office 2024/Office/Data/i641026.cab", &vec![b'c'; 2048]);
    put(&dl, "Microsoft Office 2024/Office/Data/i641027.cab", &vec![b'c'; 2048]);
    put(&dl, "Microsoft Office 2024/Instructions.txt", b"Install guide");

    for d in ["Chipset.exe", "Audio.exe", "Lan.exe", "Touchpad.exe"] {
        put(&dl, &format!("drivers/{d}"), &vec![b'x'; 2048]);
    }

    // Media folder must NOT become a SoftwarePackage even though it has desktop.ini.
    put(&dl, "38833FF26BA1D.UnigramPreview/Black.Doves.S02E05.mkv", b"video-1");
    put(&dl, "38833FF26BA1D.UnigramPreview/Movie.2026.mp4", b"video-2");
    put(&dl, "38833FF26BA1D.UnigramPreview/desktop.ini", b"[.ShellClassInfo]");

    // Executable helper inside a git repo stays part of the Project, not a separate SoftwarePackage.
    put(&prj, "MyApp/.git/HEAD", b"ref: refs/heads/main\n");
    put(&prj, "MyApp/src/main.rs", b"fn main() {}\n");
    put(&prj, "MyApp/tools/helper.exe", &vec![b'h'; 2048]);
    put(&prj, "MyApp/tools/helper.dll", &vec![b'l'; 2048]);

    rescan(&mut s, &[&dl, &prj, &docs, &pics]);
    recognise(&mut s).unwrap();

    let projs = cols(&s, CollectionKind::Project);
    assert!(projs.iter().all(|c| c.name != "DDU v18.1.5.7"), "DDU must not be classified as a Project");
    assert!(projs.iter().any(|c| c.name == "MyApp"));

    let sw = cols(&s, CollectionKind::SoftwarePackage);
    for expected in ["DDU v18.1.5.7", "Armoury_Crate_Uninstall_Tool", "Microsoft Office 2024", "drivers"] {
        assert!(sw.iter().any(|c| c.name == expected), "missing software package {expected}: {sw:?}");
    }
    assert!(sw.iter().all(|c| !matches!(c.name.as_str(), "x64" | "x86" | "Office")), "composite installer split: {sw:?}");
    assert!(sw.iter().all(|c| !c.name.contains("UnigramPreview")), "media folder claimed as software: {sw:?}");
    assert!(sw.iter().all(|c| c.name != "tools" && c.name != "MyApp"), "project tool escaped project: {sw:?}");

    let fresh = filing_plan(&s, &Scope::Pc, BlueprintChoice::Fresh).unwrap();
    let dl_c = canon(&dl);
    let ddu_from = canon(&dl.join("DDU v18.1.5.7"));
    let off_from = canon(&dl.join("Microsoft Office 2024"));
    let drv_from = canon(&dl.join("drivers"));

    let find_group = |from: &str| {
        fresh
            .iter()
            .find(|p| matches!(p.target, Target::Group { .. }) && p.from.eq_ignore_ascii_case(from))
            .unwrap_or_else(|| panic!("missing group placement for {from}"))
    };
    assert_eq!(find_group(&ddu_from).to.as_deref().unwrap().to_lowercase(), format!(r"{}\software\tools\ddu v18.1.5.7", dl_c.to_lowercase()));
    assert_eq!(find_group(&off_from).to.as_deref().unwrap().to_lowercase(), format!(r"{}\software\installers\microsoft office 2024", dl_c.to_lowercase()));
    assert_eq!(find_group(&drv_from).to.as_deref().unwrap().to_lowercase(), format!(r"{}\software\installers\drivers", dl_c.to_lowercase()));

    // Zero file-level placements inside any software package.
    for p in &fresh {
        if matches!(p.target, Target::File { .. }) {
            let low = p.from.to_lowercase();
            assert!(!low.contains("ddu v18.1.5.7") && !low.contains("microsoft office 2024") && !low.contains("armoury_crate"), "split file: {p:?}");
        }
    }

    let tidy = filing_plan(&s, &Scope::Pc, BlueprintChoice::Tidy).unwrap();
    let ddu_tidy = tidy.iter().find(|p| matches!(p.target, Target::Group { .. }) && p.from.eq_ignore_ascii_case(&ddu_from)).unwrap();
    assert!(ddu_tidy.to.is_none());
}

#[test]
fn owners_folders_stay_whole_route_to_life_area_and_stay_protected_after_move() {
    let (_t, mut s, pics, dl, docs, prj) = setup_pc();

    put(&docs, "CVs/CV info.docx", b"Curriculum Vitae summary");
    put(&docs, "CVs/Admin/Cover Letter.docx", b"Dear hiring manager CV");
    put(&docs, "CVs/MacDonald_CV.pdf", b"CV PDF content");

    put(&docs, "Ass. Bursar/MacDonald_Cover_Letter_Assistant_Bursar.docx", b"Invoice No 1 cover letter CV");
    put(&docs, "Ass. Bursar/MacDonald_CV_Assistant_Bursar.pdf", b"Assistant Bursar curriculum vitae");

    put(&docs, "Finance Manager/Macdonald Finance Manager CV.pdf", b"Finance Manager CV");
    put(&docs, "Finance Manager/Macdonald Cover Letter.pdf", b"Cover letter for Finance Manager");

    put(&docs, "PhD/PhD Research Concept Note.docx", b"PhD research proposal concept note");

    // Holding a .db protects in place without moving it away from its local database.
    put(&docs, "Last30Days/prompt-testing.md", b"# Assignment study notes and research summary\n");
    put(&docs, "Last30Days/context-bloat.md", b"# Lecture notes summary\n");
    put(&docs, "Last30Days/.last30days-library.db", b"SQLite format 3\0");

    // Ordinary Finance life-area subfolder should stay a life area, not become an OwnersFolder.
    put(&docs, "Finance/Invoices & receipts/2026/invoice_1.pdf", b"Tax invoice amount due");

    rescan(&mut s, &[&docs, &dl, &pics, &prj]);
    recognise(&mut s).unwrap();

    let owners = cols(&s, CollectionKind::OwnersFolder);
    for expected in ["CVs", "Ass. Bursar", "Finance Manager", "PhD", "Last30Days"] {
        assert!(owners.iter().any(|c| c.name == expected), "missing owner's folder {expected}: {owners:?}");
    }
    assert!(owners.iter().all(|c| c.name != "Finance"), "life area Finance should not be claimed: {owners:?}");

    let fresh = filing_plan(&s, &Scope::Pc, BlueprintChoice::Fresh).unwrap();
    let docs_c = canon(&docs);
    let grp = |path: &Path| {
        let f = canon(path);
        fresh
            .iter()
            .find(|p| matches!(p.target, Target::Group { .. }) && p.from.eq_ignore_ascii_case(&f))
            .unwrap_or_else(|| panic!("missing group for {}", path.display()))
            .clone()
    };

    assert_eq!(grp(&docs.join("CVs")).to.as_deref().unwrap().to_lowercase(), format!(r"{}\work\cvs", docs_c.to_lowercase()));
    assert_eq!(grp(&docs.join("Ass. Bursar")).to.as_deref().unwrap().to_lowercase(), format!(r"{}\work\ass. bursar", docs_c.to_lowercase()));
    assert_eq!(grp(&docs.join("Finance Manager")).to.as_deref().unwrap().to_lowercase(), format!(r"{}\work\finance manager", docs_c.to_lowercase()));
    assert_eq!(grp(&docs.join("PhD")).to.as_deref().unwrap().to_lowercase(), format!(r"{}\study\phd", docs_c.to_lowercase()));
    assert!(grp(&docs.join("Last30Days")).to.is_none(), "folder holding .db must stay in place");

    // Run Fresh batches to move CVs -> Documents\Work\CVs, then verify it stays recognised & home.
    change_blueprint(&mut s, &Scope::Pc, BlueprintChoice::Fresh).unwrap();
    while let Some(b) = next_batch(&mut s, &Scope::Pc).unwrap() {
        approve_batch(&mut s, &b, None).unwrap();
        execute_batch(&mut s, &b).unwrap();
        after_execute(&mut s, &b).unwrap();
    }
    let moved_cvs = docs.join("Work").join("CVs");
    assert!(moved_cvs.join("CV info.docx").is_file());
    assert!(docs.join("Last30Days").join(".last30days-library.db").is_file());

    rescan(&mut s, &[&docs]);
    recognise(&mut s).unwrap();
    let fresh_after = filing_plan(&s, &Scope::Pc, BlueprintChoice::Fresh).unwrap();
    let moved_c = canon(&moved_cvs);
    let cvs_after = fresh_after
        .iter()
        .find(|p| matches!(p.target, Target::Group { .. }) && p.from.eq_ignore_ascii_case(&moved_c))
        .expect("moved CVs stays recognised under Work");
    assert!(cvs_after.to.is_none(), "moved CVs is already in Documents\\Work\\CVs: {cvs_after:?}");
}

#[test]
fn resources_stay_whole_and_route_to_documents_resources() {
    let (_t, mut s, pics, dl, docs, prj) = setup_pc();

    put(&dl, "glassmorphism/glassmorphism/SKILL.md", b"---\nname: glassmorphism\n---\n");
    put(&dl, "stitch_pwa_screen_interface_design (2)/DESIGN.md", b"# Design tokens");
    put(&dl, "stitch_pwa_screen_interface_design (2)/code.html", b"<div></div>");
    put(&dl, "stitch_pwa_screen_interface_design (2)/screen.png", b"png-data-1");
    put(&dl, "stitch_pwa_screen_interface_design (2)/screen_dark.png", b"png-data-2");
    put(&dl, "stitch_pwa_screen_interface_design (2)/screen_mobile.png", b"png-data-3");

    rescan(&mut s, &[&dl, &docs, &pics, &prj]);
    recognise(&mut s).unwrap();

    let res = cols(&s, CollectionKind::Resource);
    assert!(res.iter().any(|c| c.name == "glassmorphism"), "missing outer glassmorphism skill pack: {res:?}");
    assert!(res.iter().any(|c| c.name == "stitch_pwa_screen_interface_design (2)"), "missing stitch design pack: {res:?}");
    assert!(cols(&s, CollectionKind::DesignSet).is_empty(), "resources must not be split into DesignSets");

    let fresh = filing_plan(&s, &Scope::Pc, BlueprintChoice::Fresh).unwrap();
    let docs_c = canon(&docs);
    let g_from = canon(&dl.join("glassmorphism"));
    let s_from = canon(&dl.join("stitch_pwa_screen_interface_design (2)"));
    let g_plan = fresh.iter().find(|p| matches!(p.target, Target::Group { .. }) && p.from.eq_ignore_ascii_case(&g_from)).unwrap();
    let s_plan = fresh.iter().find(|p| matches!(p.target, Target::Group { .. }) && p.from.eq_ignore_ascii_case(&s_from)).unwrap();

    assert_eq!(g_plan.to.as_deref().unwrap().to_lowercase(), format!(r"{}\resources\skills\glassmorphism", docs_c.to_lowercase()));
    assert_eq!(
        s_plan.to.as_deref().unwrap().to_lowercase(),
        format!(r"{}\resources\ui & design\stitch_pwa_screen_interface_design (2)", docs_c.to_lowercase())
    );
}

#[test]
fn series_episodes_and_subtitles_route_by_show_and_season() {
    let (t, mut s, pics, dl, docs, prj) = setup_pc();

    let ep1 = put(&dl, "Avatar_Seven_Havens_S01E03_720p.mkv", b"ep3");
    let sub1 = put(&dl, "Avatar_Seven_Havens_S01E03_720p.srt", b"1\n00:00:01 --> 00:00:02\nhi\n");
    put(&dl, "Avatar_Seven_Havens_S01E02_720p.mkv", b"ep2");
    put(&dl, "shows/[►Average~Joe~US◄]~S02E02-HD~WebRip.mp4", b"aj2");
    put(&dl, "Average Joe 2023 S02e01 1080P Web H264.mp4", b"aj1");
    let n24 = put(&dl, "24.S07E01.720p.mkv", b"24-ep1");

    rescan(&mut s, &[&dl, &docs, &pics, &prj]);
    recognise(&mut s).unwrap();

    let series = cols(&s, CollectionKind::Series);
    assert!(series.iter().any(|c| c.name == "Avatar Seven Havens — Season 1"), "{series:?}");
    let av = series.iter().find(|c| c.name == "Avatar Seven Havens — Season 1").unwrap();
    assert_eq!(list_members(s.conn(), &av.id).unwrap().len(), 3);
    assert!(series.iter().any(|c| c.name == "Average Joe — Season 2"), "region/year normalization failed: {series:?}");
    let aj = series.iter().find(|c| c.name == "Average Joe — Season 2").unwrap();
    assert_eq!(list_members(s.conn(), &aj.id).unwrap().len(), 2);
    assert!(series.iter().any(|c| c.name == "24 — Season 7"), "numeric show title 24 failed: {series:?}");

    // Without a Videos root, Fresh leaves series safely where they are with guidance.
    let no_vid_plan = filing_plan(&s, &Scope::Pc, BlueprintChoice::Fresh).unwrap();
    let ep1_p = no_vid_plan.iter().find(|p| p.from.eq_ignore_ascii_case(&canon(&ep1))).unwrap();
    assert!(ep1_p.to.is_none());
    assert!(ep1_p.reason.contains("Videos"), "{ep1_p:?}");

    // With a Videos root enabled, episodes and subtitles route cleanly into Shows\<Show>\Season <n>.
    let vids = t.path().join("me").join("Videos");
    std::fs::create_dir_all(&vids).unwrap();
    s.add_root(&vids).unwrap();
    let with_vid_plan = filing_plan(&s, &Scope::Pc, BlueprintChoice::Fresh).unwrap();
    let vids_c = canon(&vids);
    let dest_for = |src: &Path| {
        with_vid_plan
            .iter()
            .find(|p| p.from.eq_ignore_ascii_case(&canon(src)))
            .unwrap()
            .to
            .as_deref()
            .unwrap()
            .to_lowercase()
    };
    assert_eq!(dest_for(&ep1), format!(r"{}\shows\avatar seven havens\season 1\avatar_seven_havens_s01e03_720p.mkv", vids_c.to_lowercase()));
    assert_eq!(dest_for(&sub1), format!(r"{}\shows\avatar seven havens\season 1\avatar_seven_havens_s01e03_720p.srt", vids_c.to_lowercase()));
    assert_eq!(dest_for(&n24), format!(r"{}\shows\24\season 7\24.s07e01.720p.mkv", vids_c.to_lowercase()));
}

#[test]
fn old_copies_compare_archive_quarantine_and_undo() {
    let (t, mut s, pics, dl, docs, prj) = setup_pc();

    // PC files (live counterpart for cross-volume checks).
    put(&pics, "pic1.jpg", &vec![b'p'; 2048]);
    put(&pics, "pic2.jpg", &vec![b'q'; 2048]);
    put(&pics, "pic3.jpg", &vec![b'r'; 2048]);
    put(&dl, "notes.txt", &vec![b'n'; 2048]);

    // External drive X: with:
    // 1. MSIDownloads (diverged backup holding nested node_pkg + readme.md — must be OldCopy, NOT Project)
    // 2. Same-volume identical copy: X:\Documents vs X:\Copy of Documents (verified quarantine -> undo)
    // 3. Cross-volume identical copy: X:\Laptop Pictures matching C:\...\Pictures (must Archive, NEVER quarantine)
    let provider = FakeVolumeProvider::new();
    let d = fake_drive(&t, "Ext", "0077");
    put(&d.mount_point, "Documents/doc1.pdf", &vec![b'1'; 2048]);
    put(&d.mount_point, "Documents/doc2.docx", &vec![b'2'; 2048]);
    put(&d.mount_point, "Documents/doc3.txt", &vec![b'3'; 2048]);
    put(&d.mount_point, "Copy of Documents/doc1.pdf", &vec![b'1'; 2048]);
    put(&d.mount_point, "Copy of Documents/doc2.docx", &vec![b'2'; 2048]);
    put(&d.mount_point, "Copy of Documents/doc3.txt", &vec![b'3'; 2048]);

    let msi_pkg = put(&d.mount_point, "MSIDownloads/school-app/package.json", br#"{"name":"school-app"}"#);
    put(&d.mount_point, "MSIDownloads/school-app/src/index.ts", &vec![b's'; 2048]);
    put(&d.mount_point, "MSIDownloads/readme.md", b"Old downloads readme");
    put(&d.mount_point, "MSIDownloads/extra.pdf", &vec![b'e'; 2048]);
    age_file(&msi_pkg, 60);

    put(&d.mount_point, "Laptop Pictures/pic1.jpg", &vec![b'p'; 2048]);
    put(&d.mount_point, "Laptop Pictures/pic2.jpg", &vec![b'q'; 2048]);
    put(&d.mount_point, "Laptop Pictures/pic3.jpg", &vec![b'r'; 2048]);

    rescan(&mut s, &[&pics, &dl, &docs, &prj]);
    let ext_id = plug_and_catalog(&mut s, &provider, &d);
    s.conn()
        .execute("UPDATE volumes SET role = 'unassigned' WHERE id = ?1", [&ext_id])
        .unwrap();
    private_quarantine(&s, t.path());

    recognise(&mut s).unwrap();

    let old_copies = cols(&s, CollectionKind::OldCopy);
    assert!(old_copies.iter().any(|c| c.name == "MSI Downloads"), "MSIDownloads missing: {old_copies:?}");
    assert!(old_copies.iter().any(|c| c.name == "Copy of Documents"), "{old_copies:?}");
    assert!(old_copies.iter().any(|c| c.name == "Laptop Pictures"), "{old_copies:?}");

    // MSIDownloads' nested node_pkg must NOT promote D:\MSIDownloads into a Project card.
    let projects = cols(&s, CollectionKind::Project);
    assert!(
        projects.iter().all(|c| c.locations.iter().all(|l| !l.root_path.to_lowercase().contains("msidownloads"))),
        "MSIDownloads leaked into Projects: {projects:?}"
    );

    // Fresh filing on X: keeps MSIDownloads whole even though it holds an inner code project
    // (`school-app`), emitting zero file-level moves out of it.
    let ext_fresh = filing_plan(&s, &Scope::Volume { volume_id: ext_id.clone() }, BlueprintChoice::Fresh).unwrap();
    let msi_root_c = canon(&d.mount_point.join("MSIDownloads"));
    assert!(
        ext_fresh.iter().any(|p| matches!(p.target, Target::Group { .. }) && p.from.eq_ignore_ascii_case(&msi_root_c) && p.to.is_none()),
        "OldCopy MSIDownloads must stay whole as a group in Fresh: {ext_fresh:?}"
    );
    assert!(
        ext_fresh.iter().all(|p| !(matches!(p.target, Target::File { .. }) && p.from.to_lowercase().contains("msidownloads"))),
        "Fresh must not move loose files out of OldCopy MSIDownloads: {ext_fresh:?}"
    );

    let planner = Planner::new(&s).unwrap();

    // 1. Archive move + undo + empty Archive dir cleanup.
    let msi = old_copies.iter().find(|c| c.name == "MSI Downloads").unwrap();
    let msi_plan = planner.plan_for(&s, msi).unwrap().pop().unwrap();
    assert_eq!(msi_plan.steps[0].kind, StepKind::ToArchive);
    assert!(msi_plan.steps[0].to.to_lowercase().ends_with(r"\archive\msi\downloads"), "{msi_plan:?}");
    let batch_arch = propose(&mut s, msi, &msi_plan).unwrap();
    let out_arch = run(&mut s, &batch_arch).unwrap();
    assert!(out_arch.done, "{out_arch:?}");
    let archived = PathBuf::from(&msi_plan.home_after);
    assert!(archived.join("extra.pdf").is_file());
    assert!(!d.mount_point.join("MSIDownloads").exists());
    let rb_arch = undo(&mut s, &batch_arch).unwrap();
    assert!(rb_arch.conflicts.is_empty(), "{rb_arch:?}");
    assert!(d.mount_point.join("MSIDownloads").join("extra.pdf").is_file());
    assert!(!d.mount_point.join("Archive").exists(), "empty Archive should be tidied on undo");

    // 2. Cross-volume identical copy NEVER quarantines (moves to Archive).
    let lap_pics = old_copies.iter().find(|c| c.name == "Laptop Pictures").unwrap();
    let lp_plan = planner.plan_for(&s, lap_pics).unwrap().pop().unwrap();
    assert_eq!(lp_plan.steps[0].kind, StepKind::ToArchive, "cross-volume copy must never quarantine: {lp_plan:?}");

    // 3. Same-volume identical copy proposes Quarantine with Archive fallback and executes + undoes cleanly.
    let same_vol = old_copies.iter().find(|c| c.name == "Copy of Documents").unwrap();
    assert_eq!(same_vol.locations[0].relationship, Relationship::Identical);
    let sv_plan = planner.plan_for(&s, same_vol).unwrap().pop().unwrap();
    assert_eq!(sv_plan.steps[0].kind, StepKind::Quarantine, "{sv_plan:?}");
    assert!(sv_plan.steps[0].fallback.is_some());
    let batch_q = propose(&mut s, same_vol, &sv_plan).unwrap();
    let out_q = run(&mut s, &batch_q).unwrap();
    assert!(out_q.done, "{out_q:?}");
    assert!(!d.mount_point.join("Copy of Documents").exists());
    let rb_q = undo(&mut s, &batch_q).unwrap();
    assert!(rb_q.conflicts.is_empty(), "{rb_q:?}");
    assert!(d.mount_point.join("Copy of Documents").join("doc1.pdf").is_file());
}

#[test]
fn external_drive_project_plans_stay_same_volume_and_wait_when_offline() {
    let (t, mut s, pics, dl, docs, prj) = setup_pc();

    // PC copy is the newest Home for both MyTool and Duo.
    for rel in ["package.json", "src/index.ts", "README.md"] {
        put(&prj, &format!("MyTool/{rel}"), format!("mytool:{rel}:v2").as_bytes());
        put(&prj, &format!("Duo/{rel}"), format!("duo:{rel}:v2").repeat(100).as_bytes());
    }
    rescan(&mut s, &[&prj, &docs, &dl, &pics]);

    // External drive X: has:
    // - MyTool in X:\OldWork\MyTool (plus sibling OtherApp in X:\OldWork\OtherApp) and X:\Projects
    // - TWO copies of Duo on X: X:\Projects\Duo (newer on X:) and X:\OldWork\Duo (identical duplicate on X:)
    let provider = FakeVolumeProvider::new();
    let d = fake_drive(&t, "DriveX", "0088");
    std::fs::create_dir_all(d.mount_point.join("Projects")).unwrap();
    for rel in ["package.json", "src/index.ts", "README.md"] {
        let p = put(&d.mount_point, &format!("OldWork/MyTool/{rel}"), format!("mytool:{rel}:v1").as_bytes());
        age_file(&p, 90);
        let q = put(&d.mount_point, &format!("OldWork/OtherApp/{rel}"), format!("other:{rel}").as_bytes());
        age_file(&q, 90);
        let d1 = put(&d.mount_point, &format!("Projects/Duo/{rel}"), format!("duo:{rel}:v1").repeat(100).as_bytes());
        age_file(&d1, 60);
        let d2 = put(&d.mount_point, &format!("OldWork/Duo/{rel}"), format!("duo:{rel}:v1").repeat(100).as_bytes());
        age_file(&d2, 90);
    }

    let x_id = plug_and_catalog(&mut s, &provider, &d);
    s.conn().execute("UPDATE volumes SET role = 'unassigned' WHERE id = ?1", [&x_id]).unwrap();
    private_quarantine(&s, t.path());

    // Offline check first: unplug X: before running recognise -> plans still build and set waits_for.
    provider.unplug(&d.mount_point);
    sync_mounted(&mut s, &provider).unwrap();
    recognise(&mut s).unwrap();

    let projects = cols(&s, CollectionKind::Project);
    let mytool = projects.iter().find(|c| c.name == "MyTool").unwrap();
    let duo = projects.iter().find(|c| c.name == "Duo").unwrap();

    let mytool_x = plan_for(&s, mytool).unwrap().into_iter().find(|p| p.volume_id == x_id).unwrap();
    assert!(mytool_x.waits_for.as_deref().is_some_and(|w| w.contains("unplugged")), "{mytool_x:?}");
    assert_eq!(mytool_x.steps.len(), 1);
    assert_eq!(mytool_x.steps[0].kind, StepKind::MoveHome, "sole drive copy moves into X:\\Projects, never quarantines");
    assert_eq!(mytool_x.home_after.to_lowercase(), format!(r"{}\projects\mytool", canon(&d.mount_point).to_lowercase()));

    let duo_x = plan_for(&s, duo).unwrap().into_iter().find(|p| p.volume_id == x_id).unwrap();
    assert_eq!(duo_x.steps.len(), 1);
    assert_eq!(duo_x.steps[0].kind, StepKind::Quarantine, "same-drive identical duplicate on X: quarantines safely: {duo_x:?}");

    // Plug X: back in and run both plans atomically + undo MyTool cleanly.
    provider.plug(d.clone());
    sync_mounted(&mut s, &provider).unwrap();
    recognise(&mut s).unwrap();
    let mytool_online = cols(&s, CollectionKind::Project).into_iter().find(|c| c.name == "MyTool").unwrap();
    let plan_x = plan_for(&s, &mytool_online).unwrap().into_iter().find(|p| p.volume_id == x_id).unwrap();
    assert!(plan_x.waits_for.is_none());
    let batch = propose(&mut s, &mytool_online, &plan_x).unwrap();
    let out = run(&mut s, &batch).unwrap();
    assert!(out.done, "{out:?}");
    assert!(d.mount_point.join("Projects").join("MyTool").join("README.md").is_file());
    assert!(!d.mount_point.join("OldWork").join("MyTool").exists());
    let rb = undo(&mut s, &batch).unwrap();
    assert!(rb.conflicts.is_empty(), "{rb:?}");
    assert!(d.mount_point.join("OldWork").join("MyTool").join("README.md").is_file());
}

#[test]
fn recheck_open_batch_supersedes_files_inside_new_modules() {
    let (_t, mut s, pics, dl, docs, prj) = setup_pc();

    let exe = put(&dl, "DDU/DDU.exe", &vec![b'e'; 2048]);
    let cfg = put(&dl, "DDU/DDU.cfg", b"cfg=1");
    let ddu_txt = put(&dl, "DDU/changelog.txt", b"notes");
    let loose = put(&dl, "loose_note.txt", b"loose note");
    let _ = (exe, cfg);

    rescan(&mut s, &[&dl, &docs, &pics, &prj]);

    let id_for = |p: &Path| -> String {
        s.conn()
            .query_row("SELECT id FROM files WHERE lower(path) = lower(?1)", [canon(p)], |r| r.get(0))
            .unwrap()
    };
    let ddu_id = id_for(&ddu_txt);
    let loose_id = id_for(&loose);
    let dest_docs = canon(&docs);
    let batch = propose_batch(
        &mut s,
        &RefilePlan {
            origin: origin_for(&Scope::Pc),
            actions: vec![
                ProposedActionInput {
                    agent: Some("filing".into()),
                    action_type: ActionType::Move,
                    target_kind: TargetKind::File,
                    file_id: Some(ddu_id),
                    atomic_unit_id: None,
                    from_path: canon(&ddu_txt),
                    to_path: Some(format!(r"{dest_docs}\changelog.txt")),
                    reason: "old file-by-file batch".into(),
                    confidence: 0.8,
                    evidence_json: "{}".into(),
                },
                ProposedActionInput {
                    agent: Some("filing".into()),
                    action_type: ActionType::Move,
                    target_kind: TargetKind::File,
                    file_id: Some(loose_id),
                    atomic_unit_id: None,
                    from_path: canon(&loose),
                    to_path: Some(format!(r"{dest_docs}\loose_note.txt")),
                    reason: "loose note".into(),
                    confidence: 0.8,
                    evidence_json: "{}".into(),
                },
            ],
        },
    )
    .unwrap();

    recognise(&mut s).unwrap();
    let retired = recheck_open_batch(&mut s, &Scope::Pc).unwrap();
    assert_eq!(retired, 1);

    let statuses: Vec<(String, String)> = s
        .conn()
        .prepare("SELECT from_path, status FROM actions WHERE batch_id = ?1 ORDER BY from_path")
        .unwrap()
        .query_map([&batch], |r| Ok((r.get(0)?, r.get(1)?)))
        .unwrap()
        .collect::<rusqlite::Result<_>>()
        .unwrap();
    assert!(statuses.iter().any(|(p, st)| p.to_lowercase().contains("changelog.txt") && st == "superseded"));
    assert!(statuses.iter().any(|(p, st)| p.to_lowercase().contains("loose_note.txt") && st == "proposed"));
}
