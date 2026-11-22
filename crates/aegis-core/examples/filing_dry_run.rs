//! Dry run of auto-filing against a COPY of a real database: reads and classifies files, then
//! plans both structures for the PC and every catalogued drive and prints what would happen.
//! Nothing on disk is moved; only the copy's metadata tables are written. It refuses the live
//! database (and any database inside an app data folder).
//!
//! Usage: cargo run -p aegis-core --example filing_dry_run -- <path to copied aegis.db>

use aegis_core::db::SqliteStore;
use aegis_core::filing::blueprint::{Target, plan, summarize};
use aegis_core::filing::reader::{READ_TIMEOUT, classify_unread, read_pending};
use aegis_core::filing::{BlueprintChoice, Scope};
use std::collections::BTreeMap;
use std::time::Instant;

fn main() -> aegis_core::error::Result<()> {
    let db = std::env::args().nth(1).expect("path to a copied aegis.db");
    // It writes metadata into the database it is given: never the owner's live one.
    aegis_core::db::refuse_live_database(std::path::Path::new(&db), aegis_core::db::live_database_path().as_deref())?;
    let mut store = SqliteStore::open(std::path::Path::new(&db))?;

    let t = Instant::now();
    let classified = classify_unread(&mut store, usize::MAX)?;
    println!("classified without reading: {classified} files in {:.1}s", t.elapsed().as_secs_f64());
    let t = Instant::now();
    let rep = read_pending(&mut store, 20_000, READ_TIMEOUT)?;
    let secs = t.elapsed().as_secs_f64();
    println!(
        "read: {} ok, {} unreadable in {secs:.1}s ({:.0} files/s)",
        rep.read,
        rep.unreadable,
        (rep.read + rep.unreadable) as f64 / secs.max(0.001)
    );

    // What the reader concluded about documents.
    let mut stmt = store.conn().prepare(
        "SELECT COALESCE(category, '(unsure)'), COUNT(*), SUM(confidence >= 0.9), SUM(confidence < 0.6)
         FROM file_meta WHERE kind = 'document' GROUP BY 1 ORDER BY 2 DESC",
    )?;
    println!("\ndocuments by category (count · sure · unsure):");
    for r in stmt.query_map([], |r| Ok((r.get::<_, String>(0)?, r.get::<_, i64>(1)?, r.get::<_, i64>(2)?, r.get::<_, i64>(3)?)))? {
        let (c, n, sure, unsure) = r?;
        println!("  {c:<34} {n:>7} · {sure:>6} · {unsure:>6}");
    }
    let mut stmt = store.conn().prepare("SELECT kind, COUNT(*) FROM file_meta GROUP BY kind ORDER BY 2 DESC")?;
    let kinds: Vec<(String, i64)> = stmt.query_map([], |r| Ok((r.get(0)?, r.get(1)?)))?.collect::<rusqlite::Result<_>>()?;
    println!("kinds: {kinds:?}");

    let mut scopes = vec![(Scope::Pc, "This PC".to_string())];
    {
        let mut stmt = store.conn().prepare(
            "SELECT id, COALESCE(label, mount_point) FROM volumes WHERE passport_id IS NOT NULL AND last_catalog_complete_at IS NOT NULL",
        )?;
        for r in stmt.query_map([], |r| Ok((r.get::<_, String>(0)?, r.get::<_, String>(1)?)))? {
            let (id, label) = r?;
            scopes.push((Scope::Volume { volume_id: id }, label));
        }
    }
    for (scope, label) in scopes {
        for choice in [BlueprintChoice::Tidy, BlueprintChoice::Fresh] {
            let t = Instant::now();
            let ps = plan(&store, &scope, choice)?;
            let s = summarize(&ps, choice, &store)?;
            println!(
                "\n== {label} · {:?} ({:.1}s): moves {} · renames {} · to sort {} · stay {}",
                choice,
                t.elapsed().as_secs_f64(),
                s.moves,
                s.renames,
                s.to_sort,
                s.stays
            );
            for n in s.tree.iter().take(14) {
                let kids: Vec<String> = n.children.iter().take(5).map(|c| format!("{} ({})", c.name, c.files)).collect();
                println!("  {:<28} {:>7} files  {:>8.1} GB   {}", n.name, n.files, n.bytes as f64 / 1e9, kids.join(", "));
            }
            let groups: BTreeMap<String, usize> = ps
                .iter()
                .filter_map(|p| match &p.target {
                    Target::Group { files, .. } => Some((p.reason.split(':').next().unwrap_or("").to_string(), *files)),
                    _ => None,
                })
                .fold(BTreeMap::new(), |mut m, (k, f)| {
                    *m.entry(k).or_default() += f;
                    m
                });
            println!("  groups kept whole (files): {groups:?}");
            if choice == BlueprintChoice::Fresh {
                println!("  sample renames:");
                for p in ps.iter().filter(|p| p.new_name.is_some()).take(20) {
                    let from = std::path::Path::new(&p.from).file_name().map(|n| n.to_string_lossy().to_string()).unwrap_or_default();
                    println!("    {from}  ->  {}   [{:.2}]", p.new_name.as_deref().unwrap_or(""), p.confidence);
                }
            }
        }
    }
    Ok(())
}
