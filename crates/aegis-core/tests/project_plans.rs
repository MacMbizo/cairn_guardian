//! Project plans (plan 2026-11-23-project-plans): each project's plan accounts for every copy,
//! runs as one batch, and can be undone.

mod common;
mod fixtures_context;

use aegis_core::context::store::{list_collections, record_plan_run};
use aegis_core::context::{Answer, Collection, CollectionKind, Location, Relationship, answer, recognise};
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
