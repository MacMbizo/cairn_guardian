//! Plan 6B integration tests: hybrid FTS5 + sqlite-vec retrieval inside SQLCipher, quarantine
//! exclusion (`K2`), sensitive semantic masking (`R7`), context collection filtering/enrichment,
//! dynamic `vec_chunks` model-switch rebuilds, Doctor orphan repair, and `K5` AC-power gating.

use aegis_core::contracts::{SearchFilters, SearchMode};
use aegis_core::db::SqliteStore;
use aegis_core::db::crypto::DbKey;
use aegis_core::db::files::{
    DiscoveredFileInput, SENSITIVE_MASK, insert_chunk, set_file_sensitivity,
    upsert_discovered_file,
};
use aegis_core::db::search::search_hybrid;
use aegis_core::db::vec::{
    ensure_vec_table, pending_chunks, prune_orphan_vectors, upsert_chunk_embeddings, vec_coverage,
};
use aegis_core::doctor::run_doctor;
use aegis_core::engine::{Calm, Engine, Finding, IDLE_GRACE_MS, Watch};
use aegis_core::filing::search_facets;
use rusqlite::{Connection, params};
use std::path::PathBuf;
use tempfile::TempDir;

fn seed_vol(conn: &Connection, id: &str, label: &str, status: &str) {
    conn.execute(
        "INSERT OR IGNORE INTO volumes (id, mount_point, fs_type, label, status, last_seen)
         VALUES (?1, 'C:\\', 'ntfs', ?2, ?3, 1000)",
        params![id, label, status],
    )
    .expect("seed volume");
}

fn seed_doc(conn: &Connection, vol_id: &str, path: &str, mtime: i64, text: &str) -> String {
    let idx = blake3::hash(path.as_bytes()).as_bytes()[..16].to_vec();
    let fid = upsert_discovered_file(
        conn,
        &DiscoveredFileInput {
            volume_id: vol_id.to_string(),
            has_stable_inode: true,
            case_sensitive: false,
            file_index: Some(idx),
            nlink: 1,
            atomic_unit_id: None,
            path: PathBuf::from(path),
            size: text.len() as i64,
            mtime,
            ctime: mtime,
            blake3: None,
            cloud_state: "local".to_string(),
        },
    )
    .expect("upsert file");
    insert_chunk(conn, &fid, 0, text, 10, true).expect("insert chunk");
    fid
}

fn norm(mut v: Vec<f32>) -> Vec<f32> {
    let n: f32 = v.iter().map(|x| x * x).sum::<f32>().sqrt();
    if n > 0.0 {
        for x in &mut v {
            *x /= n;
        }
    }
    v
}

#[test]
fn hybrid_search_fuses_fts_and_vectors_inside_encrypted_db() {
    let dir = TempDir::new().unwrap();
    let db_path = dir.path().join("hybrid.db");
    let key = DbKey::generate().unwrap();
    let store = SqliteStore::open_encrypted(&db_path, &key).unwrap();
    seed_vol(store.conn(), "v1", "Main", "online");

    let f_both = seed_doc(
        store.conn(),
        "v1",
        r"C:\docs\tax_invoice.pdf",
        10_000,
        "Monthly invoice statement for electricity usage",
    );
    let f_sem_only = seed_doc(
        store.conn(),
        "v1",
        r"C:\docs\power_statement.txt",
        10_000,
        "Municipal power utility statement for kilowatt charge",
    );
    let f_unrel = seed_doc(
        store.conn(),
        "v1",
        r"C:\docs\pancakes.txt",
        10_000,
        "Fluffy buttermilk pancakes with maple syrup",
    );

    let model_id = "test-embed-4d";
    ensure_vec_table(store.conn(), model_id, 4).unwrap();
    let pend = pending_chunks(store.conn(), model_id, 4, 50).unwrap();
    assert_eq!(pend.len(), 3);

    let pairs: Vec<_> = pend
        .into_iter()
        .map(|p| {
            let v = if p.file_id == f_both {
                norm(vec![0.8, 0.6, 0.0, 0.0])
            } else if p.file_id == f_sem_only {
                norm(vec![0.95, 0.3, 0.0, 0.0])
            } else {
                norm(vec![0.0, 0.0, 1.0, 0.0])
            };
            (p, v)
        })
        .collect();
    let upserted = upsert_chunk_embeddings(store.conn(), model_id, 4, &pairs).unwrap();
    assert_eq!(upserted, 3);

    let pending_outbox: i64 = store
        .conn()
        .query_row(
            "SELECT COUNT(*) FROM outbox_events WHERE aggregate_type = 'chunk' AND processed_at IS NULL",
            [],
            |r| r.get(0),
        )
        .unwrap();
    assert_eq!(pending_outbox, 0, "upsert should mark chunk outbox events processed");

    let qvec = norm(vec![0.95, 0.3, 0.0, 0.0]);
    let page = search_hybrid(
        store.conn(),
        "invoice",
        Some((model_id, &qvec, 0.45)),
        &SearchFilters::default(),
        10,
        false,
    )
    .unwrap();

    assert_eq!(page.hits.len(), 2, "only both-hit and semantic-hit should return: {:?}", page.hits);
    assert_eq!(page.hits[0].file_id, f_both, "dual FTS+vec hit must outrank single-channel hits via RRF");
    assert_eq!(page.hits[0].match_reason, "hybrid");
    assert_eq!(page.hits[1].file_id, f_sem_only);
    assert_eq!(page.hits[1].match_reason, "semantic");
    assert!(!page.hits.iter().any(|h| h.file_id == f_unrel));

    // Keyword mode skips vector channel cleanly.
    let kw_page = search_hybrid(
        store.conn(),
        "invoice",
        Some((model_id, &qvec, 0.45)),
        &SearchFilters {
            mode: Some(SearchMode::Keyword),
            ..Default::default()
        },
        10,
        false,
    )
    .unwrap();
    assert_eq!(kw_page.hits.len(), 1);
    assert_eq!(kw_page.hits[0].file_id, f_both);
    assert_eq!(kw_page.hits[0].match_reason, "content");

    // Semantic mode excludes BM25/name channels and returns vector hits ordered by cosine distance.
    let sem_page = search_hybrid(
        store.conn(),
        "invoice",
        Some((model_id, &qvec, 0.45)),
        &SearchFilters {
            mode: Some(SearchMode::Semantic),
            ..Default::default()
        },
        10,
        false,
    )
    .unwrap();
    assert_eq!(sem_page.hits.len(), 2);
    assert_eq!(sem_page.hits[0].file_id, f_sem_only);
}

#[test]
fn quarantined_files_never_leak_into_vector_search() {
    let dir = TempDir::new().unwrap();
    let db_path = dir.path().join("quar.db");
    let key = DbKey::generate().unwrap();
    let mut store = SqliteStore::open_encrypted(&db_path, &key).unwrap();
    seed_vol(store.conn(), "v1", "Main", "online");

    let live_id = seed_doc(store.conn(), "v1", r"C:\docs\live_report.txt", 10_000, "quarterly summary");
    let quar_id = seed_doc(store.conn(), "v1", r"C:\docs\old_report.txt", 10_000, "legacy summary");

    let model_id = "test-embed-4d";
    let pend = pending_chunks(store.conn(), model_id, 4, 20).unwrap();
    let pairs: Vec<_> = pend
        .into_iter()
        .map(|p| (p, norm(vec![1.0, 0.0, 0.0, 0.0])))
        .collect();
    upsert_chunk_embeddings(store.conn(), model_id, 4, &pairs).unwrap();

    // Quarantine `quar_id` after vector indexing (`K2`).
    store
        .conn()
        .execute("UPDATE files SET state = 'quarantine' WHERE id = ?1", params![quar_id])
        .unwrap();

    let qvec = norm(vec![1.0, 0.0, 0.0, 0.0]);
    let page = search_hybrid(
        store.conn(),
        "electricity",
        Some((model_id, &qvec, 0.45)),
        &SearchFilters::default(),
        10,
        false,
    )
    .unwrap();
    assert_eq!(page.hits.len(), 1);
    assert_eq!(page.hits[0].file_id, live_id);
    assert!(!page.hits.iter().any(|h| h.file_id == quar_id));

    // Quarantined files are discoverable only when explicitly requested.
    let with_q = search_hybrid(
        store.conn(),
        "electricity",
        Some((model_id, &qvec, 0.45)),
        &SearchFilters {
            include_quarantined: Some(true),
            ..Default::default()
        },
        10,
        false,
    )
    .unwrap();
    assert_eq!(with_q.hits.len(), 2);

    // Doctor check 4 detects orphan vectors left by quarantined/deleted files and repair=true prunes them.
    let diag = run_doctor(&mut store, false).unwrap();
    let vec_check = diag.checks.iter().find(|c| c.check_id == 4).unwrap();
    assert!(!vec_check.passed, "orphan vector for quarantined file should flag doctor check 4");
    let repaired = run_doctor(&mut store, true).unwrap();
    let vec_rep = repaired.checks.iter().find(|c| c.check_id == 4).unwrap();
    assert!(vec_rep.passed && vec_rep.repaired);
    assert_eq!(prune_orphan_vectors(store.conn()).unwrap(), 0);
}

#[test]
fn sensitive_files_mask_semantic_snippets_until_unlocked_r7() {
    let dir = TempDir::new().unwrap();
    let db_path = dir.path().join("r7_vec.db");
    let key = DbKey::generate().unwrap();
    let store = SqliteStore::open_encrypted(&db_path, &key).unwrap();
    seed_vol(store.conn(), "v1", "Main", "online");

    let pub_id = seed_doc(
        store.conn(),
        "v1",
        r"C:\docs\notes.txt",
        10_000,
        "Standard municipal public utility bulletin",
    );
    let sec_id = seed_doc(
        store.conn(),
        "v1",
        r"C:\docs\bank_pin.txt",
        10_000,
        "Account routing 998877 secret escrow key",
    );
    set_file_sensitivity(store.conn(), &sec_id, "confidential").unwrap();

    let model_id = "test-embed-4d";
    let pend = pending_chunks(store.conn(), model_id, 4, 20).unwrap();
    let pairs: Vec<_> = pend
        .into_iter()
        .map(|p| (p, norm(vec![1.0, 0.0, 0.0, 0.0])))
        .collect();
    upsert_chunk_embeddings(store.conn(), model_id, 4, &pairs).unwrap();

    let qvec = norm(vec![1.0, 0.0, 0.0, 0.0]);
    // Pure semantic query with no overlapping keyword in the secret doc.
    let locked = search_hybrid(
        store.conn(),
        "unrelated_concept",
        Some((model_id, &qvec, 0.45)),
        &SearchFilters {
            mode: Some(SearchMode::Semantic),
            ..Default::default()
        },
        10,
        false,
    )
    .unwrap();
    assert_eq!(locked.hits.len(), 2, "sensitive file must remain searchable via semantic search");
    let pub_hit = locked.hits.iter().find(|h| h.file_id == pub_id).unwrap();
    let sec_hit = locked.hits.iter().find(|h| h.file_id == sec_id).unwrap();
    assert!(pub_hit.snippet.contains("municipal public"));
    assert_eq!(sec_hit.snippet, SENSITIVE_MASK);

    // Once Windows CredUI unlocks sensitive views, true semantic excerpt is visible.
    let unlocked = search_hybrid(
        store.conn(),
        "unrelated_concept",
        Some((model_id, &qvec, 0.45)),
        &SearchFilters {
            mode: Some(SearchMode::Semantic),
            ..Default::default()
        },
        10,
        true,
    )
    .unwrap();
    let sec_unlocked = unlocked.hits.iter().find(|h| h.file_id == sec_id).unwrap();
    assert!(sec_unlocked.snippet.contains("998877"));
}

#[test]
fn context_collection_filter_and_enrichment_work_for_projects_and_sets() {
    let dir = TempDir::new().unwrap();
    let db_path = dir.path().join("ctx.db");
    let key = DbKey::generate().unwrap();
    let store = SqliteStore::open_encrypted(&db_path, &key).unwrap();

    seed_vol(store.conn(), "vol_c", "System (C:)", "online");
    seed_vol(store.conn(), "vol_ext", "Seagate 2TB", "offline");

    let sibling_app2 = seed_doc(
        store.conn(),
        "vol_c",
        r"C:\Prj\AlphaApp2\README.md",
        10_000,
        "Unrelated sibling deployment manual",
    );
    let design_poster = seed_doc(
        store.conn(),
        "vol_ext",
        r"E:\Design\BrandKit\poster.txt",
        10_000,
        "Vector logo poster deployment manual notes",
    );
    let alpha_readme = seed_doc(
        store.conn(),
        "vol_c",
        r"C:\Prj\AlphaApp\README.md",
        10_000,
        "Architecture guide and deployment manual",
    );

    let model_id = "test-embed-4d";
    let pend = pending_chunks(store.conn(), model_id, 4, 20).unwrap();
    let pairs: Vec<_> = pend
        .into_iter()
        .map(|p| (p, norm(vec![1.0, 0.0, 0.0, 0.0])))
        .collect();
    upsert_chunk_embeddings(store.conn(), model_id, 4, &pairs).unwrap();

    store
        .conn()
        .execute(
            "INSERT INTO collections (id, kind, name, project_kind, confidence, state, computed_at)
             VALUES ('col_proj', 'project', 'AlphaApp', 'software', 0.98, 'open', 1000),
                    ('col_set', 'design_set', 'BrandKit', 'design', 0.92, 'open', 1000),
                    ('col_dead', 'project', 'DismissedOld', 'software', 0.50, 'dismissed', 1000)",
            [],
        )
        .unwrap();
    store
        .conn()
        .execute(
            "INSERT INTO collection_locations (id, collection_id, volume_id, root_path, relationship, files, bytes, newest_mtime, shared_files, compared_by, is_repo, fingerprint)
             VALUES ('loc_1', 'col_proj', 'vol_c', 'C:\\Prj\\AlphaApp', 'home', 1, 100, 1000, 1, 'hash', 1, 'fp1')",
            [],
        )
        .unwrap();
    store
        .conn()
        .execute(
            "INSERT INTO collection_members (collection_id, file_id, volume_id, path, size, mtime)
             VALUES ('col_set', ?1, 'vol_ext', 'E:\\Design\\BrandKit\\poster.txt', 100, 1000)",
            params![design_poster],
        )
        .unwrap();

    let facets = search_facets(store.conn()).unwrap();
    assert_eq!(facets.collections.len(), 2);
    assert!(facets.collections.iter().any(|(id, name, _)| id == "col_proj" && name == "AlphaApp"));
    assert!(!facets.collections.iter().any(|(id, _, _)| id == "col_dead"));

    // Keyword query returns all 3 matching documents with collection and drive enrichment.
    let fts_all = search_hybrid(
        store.conn(),
        "deployment manual",
        None,
        &SearchFilters::default(),
        10,
        false,
    )
    .unwrap();
    assert_eq!(fts_all.hits.len(), 3);

    // Hybrid query mentioning collection name boosts AlphaApp to #1 even when equal on vector distance.
    let qvec = norm(vec![1.0, 0.0, 0.0, 0.0]);
    let all = search_hybrid(
        store.conn(),
        "AlphaApp deployment manual",
        Some((model_id, &qvec, 0.45)),
        &SearchFilters::default(),
        10,
        false,
    )
    .unwrap();
    assert_eq!(all.hits.len(), 3);
    assert_eq!(all.hits[0].file_id, alpha_readme);
    assert_eq!(all.hits[0].collection_name.as_deref(), Some("AlphaApp"));
    assert_eq!(all.hits[0].collection_kind.as_deref(), Some("project"));
    assert!(all.hits[0].drive_online);

    let poster_hit = all.hits.iter().find(|h| h.file_id == design_poster).unwrap();
    assert_eq!(poster_hit.collection_name.as_deref(), Some("BrandKit"));
    assert_eq!(poster_hit.collection_kind.as_deref(), Some("design_set"));
    assert_eq!(poster_hit.drive_label.as_deref(), Some("Seagate 2TB"));
    assert!(!poster_hit.drive_online);

    // Filter by project location respects path-component boundary (excludes AlphaApp2).
    let proj_only = search_hybrid(
        store.conn(),
        "deployment manual",
        None,
        &SearchFilters {
            collection_id: Some("col_proj".into()),
            ..Default::default()
        },
        10,
        false,
    )
    .unwrap();
    assert_eq!(proj_only.hits.len(), 1);
    assert_eq!(proj_only.hits[0].file_id, alpha_readme);
    assert!(!proj_only.hits.iter().any(|h| h.file_id == sibling_app2));

    // Filter by DesignSet collection_members returns only BrandKit member.
    let set_only = search_hybrid(
        store.conn(),
        "deployment manual",
        None,
        &SearchFilters {
            collection_id: Some("col_set".into()),
            ..Default::default()
        },
        10,
        false,
    )
    .unwrap();
    assert_eq!(set_only.hits.len(), 1);
    assert_eq!(set_only.hits[0].file_id, design_poster);
}

#[test]
fn model_switch_rebuilds_vec_table_and_clears_stale_embeddings() {
    let dir = TempDir::new().unwrap();
    let db_path = dir.path().join("switch.db");
    let key = DbKey::generate().unwrap();
    let store = SqliteStore::open_encrypted(&db_path, &key).unwrap();
    seed_vol(store.conn(), "v1", "Main", "online");
    seed_doc(store.conn(), "v1", r"C:\docs\a.txt", 10_000, "first document");
    seed_doc(store.conn(), "v1", r"C:\docs\b.txt", 10_000, "second document");

    // 1. Model A (4-d).
    ensure_vec_table(store.conn(), "model-a-4d", 4).unwrap();
    let pend_a = pending_chunks(store.conn(), "model-a-4d", 4, 10).unwrap();
    assert_eq!(pend_a.len(), 2);
    let pairs_a: Vec<_> = pend_a.into_iter().map(|p| (p, norm(vec![1.0, 0.0, 0.0, 0.0]))).collect();
    upsert_chunk_embeddings(store.conn(), "model-a-4d", 4, &pairs_a).unwrap();
    assert_eq!(vec_coverage(store.conn(), "model-a-4d", 4).unwrap(), (2, 2));

    // 2. Switch to Model B with SAME dimension (4-d): evicts stale Model A vectors.
    let rebuilt_same = ensure_vec_table(store.conn(), "model-b-4d", 4).unwrap();
    assert!(!rebuilt_same);
    assert_eq!(vec_coverage(store.conn(), "model-b-4d", 4).unwrap(), (2, 0));
    assert_eq!(pending_chunks(store.conn(), "model-b-4d", 4, 10).unwrap().len(), 2);

    // 3. Switch to Model C with DIFFERENT dimension (8-d): rebuilds virtual table cleanly.
    let rebuilt_diff = ensure_vec_table(store.conn(), "model-c-8d", 8).unwrap();
    assert!(rebuilt_diff);
    assert_eq!(vec_coverage(store.conn(), "model-c-8d", 8).unwrap(), (2, 0));
    let pend_c = pending_chunks(store.conn(), "model-c-8d", 8, 10).unwrap();
    assert_eq!(pend_c.len(), 2);
    let pairs_c: Vec<_> = pend_c
        .into_iter()
        .map(|p| (p, norm(vec![1.0, 0.0, 0.0, 0.0, 0.0, 0.0, 0.0, 0.0])))
        .collect();
    upsert_chunk_embeddings(store.conn(), "model-c-8d", 8, &pairs_c).unwrap();
    assert_eq!(vec_coverage(store.conn(), "model-c-8d", 8).unwrap(), (2, 2));
}

struct AcGatedTestWatch {
    runs: std::sync::Arc<std::sync::atomic::AtomicUsize>,
}

impl Watch for AcGatedTestWatch {
    fn name(&self) -> &'static str {
        "embed"
    }
    fn every_ms(&self) -> i64 {
        60_000
    }
    fn needs_idle(&self) -> bool {
        true
    }
    fn needs_ac(&self) -> bool {
        true
    }
    fn kinds(&self) -> &'static [&'static str] {
        &[]
    }
    fn run(&mut self, _store: &mut SqliteStore, _now: i64) -> aegis_core::error::Result<Vec<Finding>> {
        self.runs.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
        Ok(Vec::new())
    }
}

#[test]
fn embed_watch_respects_ac_power_gating() {
    let mut store = SqliteStore::open_in_memory().unwrap();
    let runs = std::sync::Arc::new(std::sync::atomic::AtomicUsize::new(0));
    let mut engine = Engine::new(vec![Box::new(AcGatedTestWatch { runs: runs.clone() })]);

    let battery_calm = Calm {
        idle_secs: 600,
        on_ac: false,
        battery_pct: Some(95),
        busy: false,
    };
    let ac_calm = Calm {
        idle_secs: 600,
        on_ac: true,
        battery_pct: Some(95),
        busy: false,
    };

    // On battery, never runs even at t0 OR beyond 2 * IDLE_GRACE_MS.
    let rep1 = engine.tick(&mut store, 1_000, &battery_calm, false).unwrap();
    assert!(rep1.ran.is_empty());
    let rep2 = engine
        .tick(&mut store, 1_000 + 2 * IDLE_GRACE_MS, &battery_calm, false)
        .unwrap();
    assert!(rep2.ran.is_empty(), "AC-gated watch must never run on battery even past IDLE_GRACE_MS");
    assert_eq!(runs.load(std::sync::atomic::Ordering::SeqCst), 0);

    // Once connected to AC mains while idle, watch runs cleanly.
    let rep3 = engine
        .tick(&mut store, 1_000 + 2 * IDLE_GRACE_MS + 1000, &ac_calm, false)
        .unwrap();
    assert_eq!(rep3.ran, vec!["embed"]);
    assert_eq!(runs.load(std::sync::atomic::Ordering::SeqCst), 1);
}
