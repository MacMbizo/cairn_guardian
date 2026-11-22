//! Project plans (plan 2026-11-23-project-plans): each project's plan accounts for every copy,
//! runs as one batch, and can be undone.

mod common;
mod fixtures_context;

use aegis_core::context::plan::{PlanStep, ProjectPlan, StepKind, plan_for};
use aegis_core::context::store::{list_collections, record_plan_run};
use aegis_core::context::{Answer, Collection, CollectionKind, Location, ProjectKind, Relationship, answer, recognise};
use aegis_core::db::SqliteStore;
use fixtures_context::pc;
use std::path::Path;

fn projects_now(s: &SqliteStore) -> Vec<Collection> {
    list_collections(s.conn(), CollectionKind::Project).unwrap()
}

fn named(s: &SqliteStore, name: &str) -> Collection {
    projects_now(s).into_iter().find(|c| c.name == name).unwrap_or_else(|| panic!("no {name}"))
}

fn at<'a>(c: &'a Collection, p: &Path) -> &'a Location {
    let want = common::canon(p);
    c.locations.iter().find(|l| l.root_path.eq_ignore_ascii_case(&want)).unwrap_or_else(|| panic!("{want} not in {c:?}"))
}

fn same(a: &str, b: &Path) -> bool {
    a.eq_ignore_ascii_case(&common::canon(b))
}

#[test]
fn change_home_survives_recognition() {
    let (_t, mut s, home, prj) = pc();
    recognise(&mut s).unwrap();
    let fx = named(&s, "forexTrader");
    let dev = home.join("Documents/Projectsv2/Dev Set/forexTrader");
    assert_eq!(at(&fx, &dev).relationship, Relationship::Home, "the newer superset starts as home");
    let old = at(&fx, &prj.join("forexTrader")).id.clone();
    answer(&mut s, &fx.id, Some(&old), Answer::ChangeHome).unwrap();
    for _ in 0..2 {
        let fx = named(&s, "forexTrader");
        assert_eq!(at(&fx, &prj.join("forexTrader")).relationship, Relationship::Home, "{fx:?}");
        assert_ne!(at(&fx, &dev).relationship, Relationship::Home);
        recognise(&mut s).unwrap();
    }
    // A whole-card answer makes no sense for a home: it needs a folder.
    assert!(answer(&mut s, &fx.id, None, Answer::ChangeHome).is_err());
}

#[test]
fn keep_here_survives_recognition() {
    let (_t, mut s, _home, prj) = pc();
    recognise(&mut s).unwrap();
    let fx = named(&s, "forexTrader");
    let old = at(&fx, &prj.join("forexTrader")).id.clone();
    assert!(fx.locations.iter().all(|l| !l.keep_here));
    answer(&mut s, &fx.id, Some(&old), Answer::KeepHere { keep: true }).unwrap();
    recognise(&mut s).unwrap();
    let fx = named(&s, "forexTrader");
    let l = at(&fx, &prj.join("forexTrader"));
    assert!(l.keep_here, "{fx:?}");
    assert_eq!(fx.locations.iter().filter(|l| l.keep_here).count(), 1);
    answer(&mut s, &fx.id, Some(&l.id.clone()), Answer::KeepHere { keep: false }).unwrap();
    assert!(named(&s, "forexTrader").locations.iter().all(|l| !l.keep_here));
}

#[test]
fn plan_state_survives_save() {
    let (_t, mut s, _home, _prj) = pc();
    recognise(&mut s).unwrap();
    let fx = named(&s, "forexTrader");
    let fps: Vec<String> = fx.locations.iter().map(|l| l.fingerprint.clone()).collect();
    record_plan_run(s.conn(), "b1", &fx.locations[0].volume_id, &fps, "proposed").unwrap();
    recognise(&mut s).unwrap();
    assert_eq!(named(&s, "forexTrader").plan_state.as_deref(), Some("proposed:b1"));
    assert!(projects_now(&s).iter().filter(|c| c.name != "forexTrader").all(|c| c.plan_state.is_none()));
    // The latest run wins.
    record_plan_run(s.conn(), "b1", &fx.locations[0].volume_id, &fps, "done").unwrap();
    recognise(&mut s).unwrap();
    assert_eq!(named(&s, "forexTrader").plan_state.as_deref(), Some("done:b1"));
}

// ---- Task 2: planning ----

fn only_plan(s: &SqliteStore, c: &Collection) -> ProjectPlan {
    let mut plans = plan_for(s, c).unwrap();
    assert_eq!(plans.len(), 1, "{plans:?}");
    plans.remove(0)
}

fn step_from<'a>(p: &'a ProjectPlan, from: &Path) -> Option<&'a PlanStep> {
    p.steps.iter().find(|st| same(&st.from, from))
}

/// A hand-made project on the PC's drive, for shapes the fixtures don't have.
fn custom(s: &SqliteStore, name: &str, locs: &[(&Path, Relationship, bool)]) -> Collection {
    let volume = named(s, "forexTrader").locations[0].volume_id.clone();
    Collection {
        id: format!("c-{name}"),
        kind: CollectionKind::Project,
        name: name.into(),
        project_kind: Some(ProjectKind::Software),
        confidence: 0.9,
        state: "candidate".into(),
        evidence: vec![],
        locations: locs
            .iter()
            .enumerate()
            .map(|(i, (p, rel, repo))| Location {
                id: format!("l-{name}-{i}"),
                collection_id: format!("c-{name}"),
                volume_id: volume.clone(),
                root_path: common::canon(p),
                relationship: *rel,
                files: 10,
                bytes: 10_000,
                // 2025-06-15, so a disambiguator says 2025.
                newest_mtime: 1_749_988_800_000,
                shared_files: 5,
                compared_by: "content".into(),
                is_repo: *repo,
                online: true,
                fingerprint: format!("fp-{name}-{i}"),
                keep_here: false,
            })
            .collect(),
        sub_projects: vec![],
        plan_state: None,
    }
}

#[test]
fn forextrader_plan_shape() {
    let (_t, mut s, home, prj) = pc();
    recognise(&mut s).unwrap();
    let p = only_plan(&s, &named(&s, "forexTrader"));
    let dev = home.join("Documents/Projectsv2/Dev Set/forexTrader");
    let target = prj.join("forexTrader");
    assert!(same(&p.home_after, &target), "{p:?}");
    let kinds: Vec<(u32, StepKind)> = p.steps.iter().map(|st| (st.step, st.kind)).collect();
    assert_eq!(kinds, vec![(1, StepKind::MoveAside), (2, StepKind::MoveHome), (3, StepKind::Quarantine)], "{p:?}");
    // The old copy steps aside so the main copy can take its name...
    assert!(same(&p.steps[0].from, &target));
    assert!(same(&p.steps[0].to, &prj.join("forexTrader (old copy)")));
    // ...the main copy moves in whole...
    assert!(same(&p.steps[1].from, &dev));
    assert!(same(&p.steps[1].to, &target));
    // ...and the old copy goes to quarantine, or into Versions if anything in it isn't in the main copy.
    assert!(same(&p.steps[2].from, &prj.join("forexTrader (old copy)")));
    let fallback = p.steps[2].fallback.as_deref().expect("a versions fallback");
    let versions = format!("{}{}Versions{}", common::canon(&target), std::path::MAIN_SEPARATOR, std::path::MAIN_SEPARATOR);
    assert!(fallback.to_lowercase().starts_with(&versions.to_lowercase()), "{fallback}");
    assert!(fallback.ends_with(" Prj"), "named after where it came from: {fallback}");
    assert!(p.waits_for.is_none());
    assert!(!p.summary.is_empty());
}

#[test]
fn drivingschool_diverged_goes_to_versions() {
    let (_t, mut s, home, prj) = pc();
    recognise(&mut s).unwrap();
    let p = only_plan(&s, &named(&s, "DrivingSchool"));
    assert!(same(&p.home_after, &prj.join("DrivingSchool")), "already in the projects home");
    assert_eq!(p.steps.len(), 1, "{p:?}");
    let st = &p.steps[0];
    assert_eq!(st.kind, StepKind::ToVersions);
    assert!(same(&st.from, &home.join("Documents/Projectsv2/Dev Set/DrivingSchool")));
    let versions = common::canon(&prj.join("DrivingSchool").join("Versions"));
    assert!(st.to.to_lowercase().starts_with(&versions.to_lowercase()), "{}", st.to);
    let leaf = Path::new(&st.to).file_name().unwrap().to_string_lossy().to_string();
    assert!(leaf.len() > 11 && leaf.as_bytes()[4] == b'-' && leaf.ends_with(" Dev Set"), "{leaf}");
}

#[test]
fn paperpilot_material_goes_to_docs() {
    let (_t, mut s, home, prj) = pc();
    recognise(&mut s).unwrap();
    // In the fixtures the two paperPilot folders only share a name ("possibly the same
    // project"): nothing is moved on a guess.
    let pp = named(&s, "paperPilot");
    assert!(pp.locations.iter().any(|l| l.relationship == Relationship::Unknown));
    assert!(only_plan(&s, &pp).steps.is_empty());
    // Once they are the same project, the Dev Set docs set is brought into the code's home.
    let docs = home.join("Documents/Projectsv2/Dev Set/paperPilot");
    let c = custom(&s, "paperPilot", &[(&prj.join("paperPilot"), Relationship::Home, false), (&docs, Relationship::DifferentMaterial, false)]);
    let p = only_plan(&s, &c);
    let st = step_from(&p, &docs).unwrap_or_else(|| panic!("{p:?}"));
    assert_eq!(st.kind, StepKind::ToMaterial);
    assert!(same(&st.to, &prj.join("paperPilot").join("Docs")), "mostly documents: {}", st.to);
}

#[test]
fn single_location_outside_home_moves_in() {
    let (_t, mut s, home, prj) = pc();
    recognise(&mut s).unwrap();
    let p = only_plan(&s, &named(&s, "chatbot"));
    assert_eq!(p.steps.len(), 1, "{p:?}");
    assert_eq!(p.steps[0].kind, StepKind::MoveHome);
    assert!(same(&p.steps[0].from, &home.join("Documents/Projectsv2/Dev Set/chatbot")));
    assert!(same(&p.steps[0].to, &prj.join("chatbot")));
}

#[test]
fn home_already_in_projects_home_has_no_move() {
    let (_t, mut s, _home, prj) = pc();
    recognise(&mut s).unwrap();
    let p = only_plan(&s, &named(&s, "mccShepherd"));
    assert!(p.steps.is_empty(), "{p:?}");
    assert!(same(&p.home_after, &prj.join("DevSet/mccShepherd")));
}

#[test]
fn repo_home_gets_companion_folder() {
    let (_t, mut s, home, prj) = pc();
    recognise(&mut s).unwrap();
    let repo = prj.join("Ledger");
    let copy = home.join("Documents/Projectsv2/Dev Set/Ledger");
    let c = custom(&s, "Ledger", &[(&repo, Relationship::Home, true), (&copy, Relationship::Diverged, false)]);
    let p = only_plan(&s, &c);
    let st = step_from(&p, &copy).unwrap_or_else(|| panic!("{p:?}"));
    assert_eq!(st.kind, StepKind::ToVersions);
    let companion = common::canon(&prj.join("Ledger - material"));
    assert!(st.to.to_lowercase().starts_with(&companion.to_lowercase()), "{}", st.to);
    assert!(!st.to.to_lowercase().starts_with(&format!("{}{}", common::canon(&repo).to_lowercase(), std::path::MAIN_SEPARATOR)), "never into the repo");
}

#[test]
fn repo_is_never_quarantined() {
    let (_t, mut s, home, prj) = pc();
    recognise(&mut s).unwrap();
    let main = prj.join("Ledger");
    let old = home.join("Documents/Projectsv2/Dev Set/Ledger");
    let c = custom(&s, "Ledger", &[(&main, Relationship::Home, false), (&old, Relationship::OlderSubset, true)]);
    let p = only_plan(&s, &c);
    let st = step_from(&p, &old).unwrap();
    assert_eq!(st.kind, StepKind::ToVersions, "{p:?}");
    assert!(p.steps.iter().all(|st| st.kind != StepKind::Quarantine));
}

#[test]
fn name_collision_with_other_project_is_disambiguated() {
    let (_t, mut s, home, prj) = pc();
    recognise(&mut s).unwrap();
    std::fs::create_dir_all(prj.join("Ledger")).unwrap();
    let c = custom(&s, "Ledger", &[(&home.join("Documents/Projectsv2/Dev Set/Ledger"), Relationship::Home, false)]);
    let p = only_plan(&s, &c);
    assert_eq!(p.steps[0].kind, StepKind::MoveHome);
    assert!(same(&p.steps[0].to, &prj.join("Ledger (app 2025)")), "{p:?}");
    assert!(!p.steps[0].to.ends_with(" (2)"));
}

fn mount_of(s: &SqliteStore, volume: &str) -> String {
    s.conn().query_row("SELECT mount_point FROM volumes WHERE id = ?1", [volume], |r| r.get(0)).unwrap()
}

#[test]
fn no_step_crosses_volumes() {
    let (_t, mut s, _home, _prj) = pc();
    recognise(&mut s).unwrap();
    let mut steps = 0;
    for c in projects_now(&s) {
        for p in plan_for(&s, &c).unwrap() {
            let mount = mount_of(&s, &p.volume_id);
            for st in &p.steps {
                steps += 1;
                for path in [&st.from, &st.to] {
                    assert!(aegis_core::db::files::path_is_within(path, &mount, false), "{path} is not on {mount}");
                }
                assert!(c.locations.iter().any(|l| l.id == st.location_id && l.volume_id == p.volume_id));
            }
        }
    }
    assert!(steps >= 4, "the fixtures have plans");
}

#[test]
fn unplugged_location_makes_plan_wait() {
    let (_t, mut s, home, prj) = pc();
    recognise(&mut s).unwrap();
    s.conn()
        .execute(
            "INSERT INTO volumes (id, mount_point, fs_type, is_removable, is_readonly, has_stable_inode, case_sensitive, status, last_seen, label)
             VALUES ('v-x', 'X:\\', 'ntfs', 1, 0, 1, 0, 'offline', 0, 'New Volume')",
            [],
        )
        .unwrap();
    let mut c = custom(&s, "Ledger", &[(&prj.join("Ledger"), Relationship::Home, false), (&home.join("Documents/Projectsv2/Dev Set/Ledger"), Relationship::Diverged, false)]);
    let mut away = c.locations[1].clone();
    away.id = "l-away".into();
    away.volume_id = "v-x".into();
    away.root_path = r"X:\Old\Ledger".into();
    away.online = false;
    c.locations.push(away);
    let plans = plan_for(&s, &c).unwrap();
    let on_x = plans.iter().find(|p| p.volume_id == "v-x").unwrap_or_else(|| panic!("{plans:?}"));
    assert!(on_x.waits_for.as_deref().is_some_and(|w| w.contains("New Volume") && w.contains("unplugged")), "{on_x:?}");
    let on_pc = plans.iter().find(|p| p.volume_id != "v-x").unwrap();
    assert!(on_pc.waits_for.is_none(), "the PC plan doesn't wait for another drive");
}

#[test]
fn left_as_is_and_not_a_project_get_no_plan() {
    let (_t, mut s, _home, _prj) = pc();
    recognise(&mut s).unwrap();
    for state in ["left_as_is", "not_a_project"] {
        let mut fx = named(&s, "forexTrader");
        fx.state = state.into();
        assert!(plan_for(&s, &fx).unwrap().is_empty(), "{state}");
    }
}

#[test]
fn keep_here_location_gets_no_step() {
    let (_t, mut s, _home, prj) = pc();
    recognise(&mut s).unwrap();
    let mut fx = named(&s, "forexTrader");
    let target = common::canon(&prj.join("forexTrader"));
    for l in &mut fx.locations {
        if l.root_path.eq_ignore_ascii_case(&target) {
            l.keep_here = true;
        }
    }
    let p = only_plan(&s, &fx);
    assert!(p.steps.iter().all(|st| !st.from.eq_ignore_ascii_case(&target)), "{p:?}");
    let mv = p.steps.iter().find(|st| st.kind == StepKind::MoveHome).unwrap();
    assert!(!mv.to.eq_ignore_ascii_case(&target), "the kept copy keeps its name: {}", mv.to);
}
