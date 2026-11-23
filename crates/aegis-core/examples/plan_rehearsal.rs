//! Rehearses project plans on COPIES of real project folders: runs each named project's plan,
//! checks the disk, undoes it, and checks that every file is back exactly as it was.
//!
//! The rehearsal folder must hold a `.aegis-rehearsal` marker file and is laid out like the PC:
//! `<dir>\me\Documents\…` and `<dir>\Prj\…`. Its own database (`<dir>\rehearsal.db`) and
//! quarantine (`<dir>\.quarantine`) are used; nothing outside the folder is written.
//!
//! Usage: cargo run -p aegis-core --example plan_rehearsal -- <dir> <project name>...

use aegis_core::context::plan::{Planner, propose, run, undo};
use aegis_core::context::store::list_collections;
use aegis_core::context::{CollectionKind, recognise};
use aegis_core::db::SqliteStore;
use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use std::time::{Instant, UNIX_EPOCH};

/// Every file under `dir` (except the rehearsal's own database and quarantine), as relative
/// path → (size, modified time).
fn snapshot(dir: &Path) -> BTreeMap<String, (u64, u128)> {
    let mut out = BTreeMap::new();
    let mut stack = vec![dir.to_path_buf()];
    while let Some(d) = stack.pop() {
        let Ok(rd) = std::fs::read_dir(&d) else { continue };
        for e in rd.flatten() {
            let p = e.path();
            let name = e.file_name().to_string_lossy().to_string();
            if d == dir && (name == ".quarantine" || name.starts_with("rehearsal.db") || name == ".aegis-rehearsal") {
                continue;
            }
            match e.metadata() {
                Ok(m) if m.is_dir() => stack.push(p),
                Ok(m) => {
                    let rel = p.strip_prefix(dir).unwrap().to_string_lossy().to_string();
                    let t = m.modified().ok().and_then(|t| t.duration_since(UNIX_EPOCH).ok()).map_or(0, |d| d.as_millis());
                    out.insert(rel, (m.len(), t));
                }
                Err(_) => {}
            }
        }
    }
    out
}

fn compare(label: &str, a: &BTreeMap<String, (u64, u128)>, b: &BTreeMap<String, (u64, u128)>) -> bool {
    let missing: Vec<&String> = a.keys().filter(|k| !b.contains_key(*k)).collect();
    let extra: Vec<&String> = b.keys().filter(|k| !a.contains_key(*k)).collect();
    let changed: Vec<&String> = a.iter().filter(|(k, v)| b.get(*k).is_some_and(|w| w != *v)).map(|(k, _)| k).collect();
    let ok = missing.is_empty() && extra.is_empty() && changed.is_empty();
    println!(
        "  {label}: {} — {} files; missing {}, extra {}, changed {}",
        if ok { "PASS" } else { "FAIL" },
        a.len(),
        missing.len(),
        extra.len(),
        changed.len()
    );
    for p in missing.iter().chain(&extra).chain(&changed).take(10) {
        println!("    {p}");
    }
    ok
}

fn main() -> aegis_core::error::Result<()> {
    let mut args = std::env::args().skip(1);
    let dir = PathBuf::from(args.next().expect("rehearsal folder"));
    let names: Vec<String> = args.collect();
    assert!(dir.join(".aegis-rehearsal").is_file(), "{} has no .aegis-rehearsal marker: refusing", dir.display());
    let db = dir.join("rehearsal.db");
    for f in ["rehearsal.db", "rehearsal.db-wal", "rehearsal.db-shm"] {
        let _ = std::fs::remove_file(dir.join(f));
    }
    let mut store = SqliteStore::open(&db)?;
    let roots = [dir.join("me").join("Documents"), dir.join("Prj")];
    let t = Instant::now();
    for r in &roots {
        store.add_root(r)?;
        aegis_core::scanner::scan_root(&mut store, r, &[1])?;
    }
    let q = dir.join(".quarantine");
    std::fs::create_dir_all(&q)?;
    store.conn().execute("UPDATE volumes SET quarantine_dir = ?1", [q.to_string_lossy().to_string()])?;
    aegis_core::filing::reader::read_pending(&mut store, 20_000, aegis_core::filing::reader::READ_TIMEOUT)?;
    let rep = recognise(&mut store)?;
    println!("scanned and recognised in {:.1}s: {} projects", t.elapsed().as_secs_f64(), rep.projects);

    let mut all_ok = true;
    for name in &names {
        let projects = list_collections(store.conn(), CollectionKind::Project)?;
        let Some(c) = projects.iter().find(|c| c.name.eq_ignore_ascii_case(name)).cloned() else {
            println!("\n{name}: not recognised");
            all_ok = false;
            continue;
        };
        println!("\n{} ({} places)", c.name, c.locations.len());
        for l in &c.locations {
            println!("  {:?}  {}  {} files", l.relationship, l.root_path, l.files);
        }
        let plan = Planner::new(&store)?.plan_for(&store, &c)?.into_iter().find(|p| !p.steps.is_empty());
        let Some(plan) = plan else {
            println!("  no plan with steps");
            continue;
        };
        println!("  plan: {}", plan.summary);
        for s in &plan.steps {
            println!("    {}. {:?}  {}  ->  {}", s.step, s.kind, s.from, s.to);
        }
        let home = c.locations.iter().find(|l| l.relationship == aegis_core::context::Relationship::Home).unwrap();
        let home_before = snapshot(Path::new(&home.root_path));
        let before = snapshot(&dir);

        let t = Instant::now();
        let batch = propose(&mut store, &c, &plan)?;
        let out = run(&mut store, &batch)?;
        println!(
            "  run in {:.1}s: done {} · {} files moved · {:.1} MB to free{}",
            t.elapsed().as_secs_f64(),
            out.done,
            out.moved_files,
            out.freed_bytes as f64 / 1e6,
            out.reason.as_deref().map(|r| format!(" · {r}")).unwrap_or_default()
        );
        if out.done {
            // Versions and material the plan brought into the home are expected extras.
            let mut after = snapshot(Path::new(&plan.home_after));
            let brought: Vec<String> = plan
                .steps
                .iter()
                .filter_map(|s| Path::new(&s.to).strip_prefix(&plan.home_after).ok())
                .map(|p| p.to_string_lossy().to_string())
                .filter(|p| !p.is_empty())
                .collect();
            after.retain(|k, _| !brought.iter().any(|b| k.starts_with(b.as_str())));
            all_ok &= compare("main copy arrived whole", &home_before, &after);
            let gone = plan.steps.iter().all(|s| !Path::new(&s.from).exists() || s.from.eq_ignore_ascii_case(&plan.home_after));
            println!("  every moved folder left its old place: {}", if gone { "PASS" } else { "FAIL" });
            all_ok &= gone;
        }
        let t = Instant::now();
        if out.done {
            let r = undo(&mut store, &batch)?;
            println!("  undo in {:.1}s: {} reverted, {} conflicts", t.elapsed().as_secs_f64(), r.reverted_count, r.conflicts.len());
        }
        all_ok &= compare("after undo, everything is back exactly", &before, &snapshot(&dir));
        // Recognise again so the next project sees the restored state.
        recognise(&mut store)?;
    }
    println!("\n{}", if all_ok { "REHEARSAL PASSED" } else { "REHEARSAL FAILED" });
    Ok(())
}
