use aegis_core::contracts::{
    ActionClass, ActionType, AutonomyDial, ConflictResolutionChoice, ProposedActionInput,
    RefilePlan, RollbackConflictKind, SearchFilters, TargetKind,
};
use aegis_core::db::SqliteStore;
use aegis_core::db::actions::{
    approve_batch, exact_dedupe_report, execute_batch, propose_batch, resolve_rollback_conflict,
    rollback_batch, simulate_batch,
};
use aegis_core::db::audit::append_audit;
use aegis_core::db::files::{
    DiscoveredFileInput, insert_chunk, normalize_path, search_fts, soft_delete_file_in_tx,
    upsert_discovered_file,
};
use aegis_core::doctor::run_doctor;
use aegis_core::policy::{
    PolicyAction, PolicyContext, PolicyVerdict, RuleCandidate, RuleResolution, RuleScopeKind,
    evaluate_policy, resolve_rule_conflicts,
};
use aegis_core::quarantine::{calculate_reclaimable_bytes, list_quarantine, purge_entries, space_summary};
use aegis_core::scanner::os_adapter::{inspect_path_metadata, inspect_volume_for_path};
use aegis_core::scanner::scan_root;
use rusqlite::params;
use std::collections::HashMap;
use std::fs;
use std::time::Instant;
use tempfile::TempDir;

fn seed_volume_and_file(store: &SqliteStore, vol_id: &str, root: &std::path::Path, file_path: &std::path::Path) -> String {
    let meta = inspect_volume_for_path(root).unwrap();
    let now = aegis_core::now_ms();
    store
        .conn()
        .execute(
            "INSERT OR IGNORE INTO volumes (id, mount_point, fs_type, has_stable_inode, case_sensitive, quarantine_dir, status, last_seen)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, 'online', ?7)",
            params![
                vol_id,
                root.to_string_lossy().to_string(),
                meta.fs_type,
                i32::from(meta.has_stable_inode),
                i32::from(meta.case_sensitive),
                root.join(".aegis-quarantine").to_string_lossy().to_string(),
                now
            ],
        )
        .unwrap();

    let st = inspect_path_metadata(file_path).unwrap().unwrap();
    let input = DiscoveredFileInput {
        volume_id: vol_id.to_string(),
        has_stable_inode: true,
        case_sensitive: false,
        file_index: st.file_index,
        nlink: st.nlink,
        atomic_unit_id: None,
        path: file_path.to_path_buf(),
        size: st.size,
        mtime: st.mtime,
        ctime: st.ctime,
        blake3: Some(blake3::hash(fs::read(file_path).unwrap().as_slice()).as_bytes().to_vec()),
        cloud_state: st.cloud_state,
    };
    upsert_discovered_file(store.conn(), &input).unwrap()
}

#[test]
fn migration_speed_and_fts_sync() {
    let tmp = TempDir::new().unwrap();
    let db_path = tmp.path().join("aegis.db");

    let start = Instant::now();
    let mut store = SqliteStore::open(&db_path).unwrap();
    let elapsed = start.elapsed();
    assert!(
        elapsed.as_millis() < 200,
        "Empty DB migration took {}ms (>=200ms)",
        elapsed.as_millis()
    );

    let f1 = tmp.path().join("alpha.md");
    let f2 = tmp.path().join("beta_quarantined.md");
    fs::write(&f1, "quarterly tax ledger summary").unwrap();
    fs::write(&f2, "quarterly secret backup ledger").unwrap();

    let id1 = seed_volume_and_file(&store, "v1", tmp.path(), &f1);
    let id2 = seed_volume_and_file(&store, "v1", tmp.path(), &f2);

    insert_chunk(store.conn(), &id1, 0, "quarterly tax ledger summary", 4, true).unwrap();
    insert_chunk(store.conn(), &id2, 0, "quarterly secret backup ledger", 4, true).unwrap();

    // Mark f2 quarantined
    store
        .conn()
        .execute("UPDATE files SET state = 'quarantine' WHERE id = ?1", params![id2])
        .unwrap();

    // Default search MUST exclude quarantined items
    let page = search_fts(store.conn(), "quarterly", &SearchFilters::default(), 20).unwrap();
    assert_eq!(page.hits.len(), 1);
    assert_eq!(page.hits[0].file_id, id1);

    // Explicit include_quarantined returns both
    let page_all = search_fts(
        store.conn(),
        "quarterly",
        &SearchFilters {
            include_quarantined: Some(true),
            ..Default::default()
        },
        20,
    )
    .unwrap();
    assert_eq!(page_all.hits.len(), 2);

    // Soft-delete f1 clears its FTS chunks cleanly
    store
        .with_full_sync_tx(|tx| soft_delete_file_in_tx(tx, &id1))
        .unwrap();
    let fts_count: i64 = store
        .conn()
        .query_row("SELECT COUNT(*) FROM chunks_fts", [], |r| r.get(0))
        .unwrap();
    assert_eq!(fts_count, 1);
}

#[test]
fn audit_chain_tamper_detection() {
    let mut store = SqliteStore::open_in_memory().unwrap();
    append_audit(store.conn_mut(), "user", "scan.start", "/docs", "{}").unwrap();
    append_audit(store.conn_mut(), "user", "actions.approve", "b1", "{}").unwrap();
    append_audit(store.conn_mut(), "user", "actions.execute", "b1", "{}").unwrap();

    let clean_report = run_doctor(&mut store, false).unwrap();
    assert!(clean_report.checks[0].passed);

    // Tamper row 2 directly
    store
        .conn()
        .execute(
            "UPDATE audit_log SET target = '/tampered' WHERE seq = 2",
            [],
        )
        .unwrap();

    // Run doctor WITH repair=true: audit chain check must NEVER auto-rewrite and must pinpoint seq=2
    let tampered_report = run_doctor(&mut store, true).unwrap();
    assert!(!tampered_report.checks[0].passed);
    assert!(!tampered_report.checks[0].repaired);
    assert!(
        tampered_report.checks[0]
            .details
            .contains("seq 2")
    );
}

#[test]
fn policy_engine_truth_table() {
    let tmp = TempDir::new().unwrap();
    let root = tmp.path().join("workspace");
    let unit_root = root.join("repo");
    let cache_dir = unit_root.join("node_modules");
    let aegis_dir = root.join(".aegis");
    let q_root = tmp.path().join(".aegis-quarantine");
    fs::create_dir_all(&cache_dir).unwrap();
    fs::create_dir_all(&aegis_dir).unwrap();
    fs::create_dir_all(&q_root).unwrap();

    let inside_file = unit_root.join("src.rs");
    let root_file = root.join("notes.txt");
    let dest_in_root = root.join("sorted").join("src.rs");
    let dest_in_unit = unit_root.join("renamed.rs");
    let outside_file = tmp.path().join("outside.txt");
    fs::write(&inside_file, "fn main() {}").unwrap();
    fs::write(&root_file, "hello").unwrap();
    fs::write(&outside_file, "outside").unwrap();

    let mut dials = HashMap::new();
    dials.insert(ActionClass::Refile, AutonomyDial::Auto);
    let ctx = PolicyContext {
        mvp_mode: false,
        active_roots: vec![root.clone()],
        volume_quarantine_dirs: vec![q_root.clone()],
        dials: dials.clone(),
    };

    // 1. Outside configured roots blocked
    let act_out = PolicyAction {
        action_type: ActionType::Move,
        target_kind: TargetKind::File,
        agent: None,
        from_path: &outside_file,
        to_path: Some(&dest_in_root),
        confidence: 1.0,
        atomic_unit_root: None,
        unit_allow_split_override: false,
        file_unit_split_override: false,
        is_registered_cache_dir: false,
    };
    assert_eq!(
        evaluate_policy(&act_out, &ctx),
        PolicyVerdict::Block("outside_configured_roots")
    );

    // 2. Traversal `..` blocked
    let traversal_dest = root.join("..").join("escape.txt");
    let act_trav = PolicyAction {
        from_path: &root_file,
        to_path: Some(&traversal_dest),
        ..act_out
    };
    assert_eq!(
        evaluate_policy(&act_trav, &ctx),
        PolicyVerdict::Block("outside_configured_roots")
    );

    // 3. Protected .aegis internal checked on BOTH from_path and to_path
    let aegis_db = aegis_dir.join("aegis.db");
    let act_to_internal = PolicyAction {
        from_path: &root_file,
        to_path: Some(&aegis_db),
        ..act_out
    };
    assert_eq!(
        evaluate_policy(&act_to_internal, &ctx),
        PolicyVerdict::Block("protected_aegis_internal")
    );

    // 4. Atomic Unit boundary blocks interior split move outward unless override is set
    let act_split = PolicyAction {
        from_path: &inside_file,
        to_path: Some(&dest_in_root),
        atomic_unit_root: Some(&unit_root),
        ..act_out
    };
    assert_eq!(
        evaluate_policy(&act_split, &ctx),
        PolicyVerdict::Block("atomic_unit_boundary")
    );

    // Move INSIDE same atomic unit is allowed
    let act_within_unit = PolicyAction {
        to_path: Some(&dest_in_unit),
        ..act_split
    };
    assert_eq!(evaluate_policy(&act_within_unit, &ctx), PolicyVerdict::AutoEligible);

    // Per-file unit_split_override unlocks outward move
    let act_file_override = PolicyAction {
        file_unit_split_override: true,
        ..act_split
    };
    assert_eq!(
        evaluate_policy(&act_file_override, &ctx),
        PolicyVerdict::AutoEligible
    );

    // Moving the ENTIRE atomic unit is allowed
    let moved_unit = root.join("archived_repo");
    let act_whole_unit = PolicyAction {
        action_type: ActionType::Move,
        target_kind: TargetKind::AtomicUnit,
        from_path: &unit_root,
        to_path: Some(&moved_unit),
        atomic_unit_root: Some(&unit_root),
        ..act_out
    };
    assert_eq!(
        evaluate_policy(&act_whole_unit, &ctx),
        PolicyVerdict::AutoEligible
    );

    // 5. `safe_build_cache` carve-out allows node_modules quarantine (RequireApproval, never Auto)
    let q_cache_dest = q_root.join("b1").join("node_modules");
    let act_cache_q = PolicyAction {
        action_type: ActionType::Quarantine,
        target_kind: TargetKind::Dir,
        agent: Some("Housekeeper"),
        from_path: &cache_dir,
        to_path: Some(&q_cache_dest),
        confidence: 0.99,
        atomic_unit_root: Some(&unit_root),
        unit_allow_split_override: false,
        file_unit_split_override: false,
        is_registered_cache_dir: true,
    };
    assert_eq!(
        evaluate_policy(&act_cache_q, &ctx),
        PolicyVerdict::RequireApproval("quarantine_never_auto")
    );

    // 6. Purge requires explicit permanent confirmation (even inside .aegis-quarantine)
    let q_item = q_root.join("b1").join("old.txt");
    let act_purge = PolicyAction {
        action_type: ActionType::Purge,
        target_kind: TargetKind::File,
        agent: None,
        from_path: &q_item,
        to_path: None,
        confidence: 1.0,
        atomic_unit_root: None,
        unit_allow_split_override: false,
        file_unit_split_override: false,
        is_registered_cache_dir: false,
    };
    assert_eq!(
        evaluate_policy(&act_purge, &ctx),
        PolicyVerdict::RequireExplicitPermanentConfirm
    );

    // 7. Agent ceilings: Housekeeper / Security capped at Suggest even when Auto dial is on
    let act_housekeeper = PolicyAction {
        action_type: ActionType::Move,
        agent: Some("Housekeeper"),
        from_path: &root_file,
        to_path: Some(&dest_in_root),
        confidence: 0.99,
        ..act_out
    };
    assert_eq!(
        evaluate_policy(&act_housekeeper, &ctx),
        PolicyVerdict::RequireApproval("agent_ceiling_suggest")
    );

    // 8. Confidence < 0.60 holds
    let act_low = PolicyAction {
        confidence: 0.45,
        agent: Some("Curator"),
        from_path: &root_file,
        to_path: Some(&dest_in_root),
        ..act_out
    };
    assert_eq!(
        evaluate_policy(&act_low, &ctx),
        PolicyVerdict::HoldLowConfidence
    );

    // 9. MVP mode blocks AutoEligible even at confidence 0.98
    let mut mvp_ctx = ctx.clone();
    mvp_ctx.mvp_mode = true;
    let act_high = PolicyAction {
        confidence: 0.98,
        agent: Some("Curator"),
        from_path: &root_file,
        to_path: Some(&dest_in_root),
        ..act_out
    };
    assert_eq!(
        evaluate_policy(&act_high, &mvp_ctx),
        PolicyVerdict::RequireApproval("manual_or_review_gate")
    );

    // 10. Rule conflict precedence (priority ASC, project > root > global, tie => Conflict)
    let resolved = resolve_rule_conflicts(vec![
        RuleCandidate {
            id: "r_global".into(),
            priority: 100,
            scope: RuleScopeKind::Global,
        },
        RuleCandidate {
            id: "r_project".into(),
            priority: 100,
            scope: RuleScopeKind::Project,
        },
    ]);
    assert_eq!(
        resolved,
        RuleResolution::Selected(RuleCandidate {
            id: "r_project".into(),
            priority: 100,
            scope: RuleScopeKind::Project
        })
    );

    let tied = resolve_rule_conflicts(vec![
        RuleCandidate {
            id: "r1".into(),
            priority: 50,
            scope: RuleScopeKind::Root,
        },
        RuleCandidate {
            id: "r2".into(),
            priority: 50,
            scope: RuleScopeKind::Root,
        },
    ]);
    assert!(matches!(tied, RuleResolution::Conflict(_)));
}

#[test]
fn rename_hardlink_atomic_save_and_unique_action() {
    let tmp = TempDir::new().unwrap();
    let store = SqliteStore::open_in_memory().unwrap();
    let root = tmp.path();
    let vol_meta = inspect_volume_for_path(root).unwrap();
    let now = aegis_core::now_ms();
    store
        .conn()
        .execute(
            "INSERT INTO volumes (id, mount_point, fs_type, has_stable_inode, case_sensitive, quarantine_dir, status, last_seen)
             VALUES (?1, ?2, ?3, 1, 0, ?4, 'online', ?5)",
            params![
                vol_meta.id,
                root.to_string_lossy().to_string(),
                vol_meta.fs_type,
                root.join(".aegis-quarantine").to_string_lossy().to_string(),
                now
            ],
        )
        .unwrap();

    // Create file a.txt and discover it
    let a = root.join("a.txt");
    fs::write(&a, "original payload 12345").unwrap();
    let st_a = inspect_path_metadata(&a).unwrap().unwrap();
    let hash_a = blake3::hash(b"original payload 12345").as_bytes().to_vec();
    let id_a = upsert_discovered_file(
        store.conn(),
        &DiscoveredFileInput {
            volume_id: vol_meta.id.clone(),
            has_stable_inode: true,
            case_sensitive: false,
            file_index: st_a.file_index.clone(),
            nlink: st_a.nlink,
            atomic_unit_id: None,
            path: a.clone(),
            size: st_a.size,
            mtime: st_a.mtime,
            ctime: st_a.ctime,
            blake3: Some(hash_a.clone()),
            cloud_state: "local".into(),
        },
    )
    .unwrap();

    // Atomic-save pattern: rename a.txt -> a_backup.txt, write BRAND NEW a.txt
    let a_backup = root.join("a_backup.txt");
    fs::rename(&a, &a_backup).unwrap();
    fs::write(&a, "brand new atomic save contents").unwrap();

    // First encounter new a.txt at the old path (should transition old row out of live path without crashing)
    let st_new_a = inspect_path_metadata(&a).unwrap().unwrap();
    let id_new_a = upsert_discovered_file(
        store.conn(),
        &DiscoveredFileInput {
            volume_id: vol_meta.id.clone(),
            has_stable_inode: true,
            case_sensitive: false,
            file_index: st_new_a.file_index.clone(),
            nlink: st_new_a.nlink,
            atomic_unit_id: None,
            path: a.clone(),
            size: st_new_a.size,
            mtime: st_new_a.mtime,
            ctime: st_new_a.ctime,
            blake3: Some(blake3::hash(b"brand new atomic save contents").as_bytes().to_vec()),
            cloud_state: "local".into(),
        },
    )
    .unwrap();
    assert_ne!(id_new_a, id_a);

    // Then encounter a_backup.txt (the renamed original inode) -> MUST preserve id_a, NOT treat as hardlink!
    let st_backup = inspect_path_metadata(&a_backup).unwrap().unwrap();
    let id_backup = upsert_discovered_file(
        store.conn(),
        &DiscoveredFileInput {
            volume_id: vol_meta.id.clone(),
            has_stable_inode: true,
            case_sensitive: false,
            file_index: st_backup.file_index.clone(),
            nlink: st_backup.nlink,
            atomic_unit_id: None,
            path: a_backup.clone(),
            size: st_backup.size,
            mtime: st_backup.mtime,
            ctime: st_backup.ctime,
            blake3: Some(hash_a.clone()),
            cloud_state: "local".into(),
        },
    )
    .unwrap();
    assert_eq!(id_backup, id_a, "Atomic-save backup must preserve original files.id");

    // Real hardlink test: create hardlink to a.txt at a_link.txt
    let a_link = root.join("a_link.txt");
    fs::hard_link(&a, &a_link).unwrap();
    let st_link = inspect_path_metadata(&a_link).unwrap().unwrap();
    assert!(st_link.nlink >= 2);
    let id_link = upsert_discovered_file(
        store.conn(),
        &DiscoveredFileInput {
            volume_id: vol_meta.id.clone(),
            has_stable_inode: true,
            case_sensitive: false,
            file_index: st_link.file_index.clone(),
            nlink: st_link.nlink,
            atomic_unit_id: None,
            path: a_link.clone(),
            size: st_link.size,
            mtime: st_link.mtime,
            ctime: st_link.ctime,
            blake3: Some(blake3::hash(b"brand new atomic save contents").as_bytes().to_vec()),
            cloud_state: "local".into(),
        },
    )
    .unwrap();
    assert_ne!(id_link, id_new_a);

    // Hardlink-aware reclaimable bytes MUST be 0 when keeper shares inode!
    let (reclaim, adjusted) = calculate_reclaimable_bytes(
        &[
            (vol_meta.id.clone(), st_link.file_index.clone(), 2, st_link.size),
            (vol_meta.id.clone(), st_link.file_index.clone(), 2, st_link.size),
        ],
        0,
    );
    assert!(adjusted);
    assert_eq!(reclaim, 0);

    // Dedupe report also respects hardlink adjustment
    let dupes = exact_dedupe_report(store.conn()).unwrap();
    assert_eq!(dupes.total_reclaimable_bytes, 0);

    // exFAT unstable-inode fallback preserves ID across rename
    store
        .conn()
        .execute(
            "INSERT INTO volumes (id, mount_point, fs_type, has_stable_inode, case_sensitive, status, last_seen)
             VALUES ('exfat1', 'E:\\', 'exfat', 0, 0, 'online', ?1)",
            params![now],
        )
        .unwrap();
    let ex_orig = root.join("ex_old.bin");
    let ex_moved = root.join("ex_new.bin");
    fs::write(&ex_orig, "exfat test data").unwrap();
    let st_ex = inspect_path_metadata(&ex_orig).unwrap().unwrap();
    let hash_ex = blake3::hash(b"exfat test data").as_bytes().to_vec();
    let id_ex1 = upsert_discovered_file(
        store.conn(),
        &DiscoveredFileInput {
            volume_id: "exfat1".into(),
            has_stable_inode: false,
            case_sensitive: false,
            file_index: None,
            nlink: 1,
            atomic_unit_id: None,
            path: ex_orig.clone(),
            size: st_ex.size,
            mtime: st_ex.mtime,
            ctime: st_ex.ctime,
            blake3: Some(hash_ex.clone()),
            cloud_state: "local".into(),
        },
    )
    .unwrap();
    fs::rename(&ex_orig, &ex_moved).unwrap();
    let id_ex2 = upsert_discovered_file(
        store.conn(),
        &DiscoveredFileInput {
            volume_id: "exfat1".into(),
            has_stable_inode: false,
            case_sensitive: false,
            file_index: None,
            nlink: 1,
            atomic_unit_id: None,
            path: ex_moved,
            size: st_ex.size,
            mtime: st_ex.mtime,
            ctime: st_ex.ctime,
            blake3: Some(hash_ex),
            cloud_state: "local".into(),
        },
    )
    .unwrap();
    assert_eq!(id_ex1, id_ex2);

    // idx_actions_file_status MUST reject a second live (proposed/approved) action on same file_id
    let mut store_mut = store;
    let b1 = propose_batch(
        &mut store_mut,
        &RefilePlan {
            origin: "simulator".into(),
            actions: vec![ProposedActionInput {
                agent: None,
                action_type: ActionType::Move,
                target_kind: TargetKind::File,
                file_id: Some(id_a.clone()),
                atomic_unit_id: None,
                from_path: a_backup.to_string_lossy().to_string(),
                to_path: Some(root.join("dest1.txt").to_string_lossy().to_string()),
                reason: "first".into(),
                confidence: 0.95,
                evidence_json: "[]".into(),
            }],
        },
    )
    .unwrap();
    approve_batch(&mut store_mut, &b1, None).unwrap();

    let second = propose_batch(
        &mut store_mut,
        &RefilePlan {
            origin: "simulator".into(),
            actions: vec![ProposedActionInput {
                agent: None,
                action_type: ActionType::Move,
                target_kind: TargetKind::File,
                file_id: Some(id_a),
                atomic_unit_id: None,
                from_path: a_backup.to_string_lossy().to_string(),
                to_path: Some(root.join("dest2.txt").to_string_lossy().to_string()),
                reason: "duplicate live proposal".into(),
                confidence: 0.95,
                evidence_json: "[]".into(),
            }],
        },
    );
    assert!(
        second.is_err(),
        "idx_actions_file_status must reject simultaneous proposed+approved rows for same file_id"
    );
}

#[test]
fn quarantine_and_preflight_rollback_conflicts() {
    let tmp = TempDir::new().unwrap();
    let root = tmp.path().join("drive");
    fs::create_dir_all(root.join("repo").join("node_modules").join("pkg")).unwrap();
    fs::write(root.join("repo").join("package.json"), "{}").unwrap();
    fs::write(
        root.join("repo").join("node_modules").join("pkg").join("index.js"),
        "module.exports = 1;",
    )
    .unwrap();
    let doc1 = root.join("doc1.txt");
    let doc2 = root.join("doc2.txt");
    fs::write(&doc1, "doc1 content").unwrap();
    fs::write(&doc2, "doc2 content").unwrap();

    let mut store = SqliteStore::open_in_memory().unwrap();
    let rc = store.add_root(&root).unwrap();
    scan_root(&mut store, &root, &[1, 2]).unwrap();

    // Verify space summary starts at 0 staged / 0 freed
    let sum0 = space_summary(&store, Some(&rc.volume_id)).unwrap();
    assert_eq!(sum0[0].staged_bytes, 0);
    assert_eq!(sum0[0].freed_bytes, 0);

    // Propose quarantine of doc1 + node_modules cache dir
    let (_, ph1) = normalize_path(&doc1, false);
    let doc1_id: String = store
        .conn()
        .query_row(
            "SELECT id FROM files WHERE path_hash = ?1 AND deleted_at IS NULL",
            params![ph1.as_slice()],
            |r| r.get(0),
        )
        .unwrap();

    let nm_path = root.join("repo").join("node_modules");
    let batch_q = propose_batch(
        &mut store,
        &RefilePlan {
            origin: "dedupe".into(),
            actions: vec![
                ProposedActionInput {
                    agent: Some("Housekeeper".into()),
                    action_type: ActionType::Quarantine,
                    target_kind: TargetKind::File,
                    file_id: Some(doc1_id),
                    atomic_unit_id: None,
                    from_path: doc1.to_string_lossy().to_string(),
                    to_path: None,
                    reason: "Duplicate".into(),
                    confidence: 0.99,
                    evidence_json: "[]".into(),
                },
                ProposedActionInput {
                    agent: Some("Housekeeper".into()),
                    action_type: ActionType::Quarantine,
                    target_kind: TargetKind::Dir,
                    file_id: None,
                    atomic_unit_id: None,
                    from_path: nm_path.to_string_lossy().to_string(),
                    to_path: None,
                    reason: "Dormant build cache".into(),
                    confidence: 0.95,
                    evidence_json: "[]".into(),
                },
            ],
        },
    )
    .unwrap();

    let sim = simulate_batch(&store, &batch_q).unwrap();
    assert_eq!(sim.blocked_count, 0, "safe_build_cache carve-out must permit node_modules quarantine");

    approve_batch(&mut store, &batch_q, None).unwrap();
    let exec_res = execute_batch(&mut store, &batch_q).unwrap();
    assert_eq!(exec_res.executed_count, 2);
    assert!(!doc1.exists());
    assert!(!nm_path.exists());

    // Quarantined doc1 MUST be excluded from default FTS search
    let s1 = search_fts(store.conn(), "doc1", &SearchFilters::default(), 10).unwrap();
    assert_eq!(s1.hits.len(), 0);

    let sum_staged = space_summary(&store, Some(&rc.volume_id)).unwrap();
    assert!(sum_staged[0].staged_bytes > 0);
    assert_eq!(sum_staged[0].freed_bytes, 0);

    // Purge doc1 permanently ONLY with confirmed_permanent = true
    let q_entries = list_quarantine(store.conn(), Some(&rc.volume_id)).unwrap();
    let doc1_entry_id = q_entries
        .iter()
        .find(|q| q.target_kind == TargetKind::File)
        .unwrap()
        .id
        .clone();
    assert!(purge_entries(&mut store, std::slice::from_ref(&doc1_entry_id), false).is_err());
    purge_entries(&mut store, std::slice::from_ref(&doc1_entry_id), true).unwrap();

    let sum_freed = space_summary(&store, Some(&rc.volume_id)).unwrap();
    assert!(sum_freed[0].freed_bytes > 0);

    // Now test Pre-flight Rollback Conflict Reconciliation on a Move batch of doc2 + doc3
    let doc3 = root.join("doc3.txt");
    fs::write(&doc3, "doc3 clean").unwrap();
    let moved2 = root.join("archive").join("doc2.txt");
    let moved3 = root.join("archive").join("doc3.txt");

    let doc2_id = seed_volume_and_file(&store, &rc.volume_id, &root, &doc2);
    let doc3_id = seed_volume_and_file(&store, &rc.volume_id, &root, &doc3);

    let batch_m = propose_batch(
        &mut store,
        &RefilePlan {
            origin: "simulator".into(),
            actions: vec![
                ProposedActionInput {
                    agent: None,
                    action_type: ActionType::Move,
                    target_kind: TargetKind::File,
                    file_id: Some(doc2_id),
                    atomic_unit_id: None,
                    from_path: doc2.to_string_lossy().to_string(),
                    to_path: Some(moved2.to_string_lossy().to_string()),
                    reason: "Refile 2".into(),
                    confidence: 1.0,
                    evidence_json: "[]".into(),
                },
                ProposedActionInput {
                    agent: None,
                    action_type: ActionType::Move,
                    target_kind: TargetKind::File,
                    file_id: Some(doc3_id),
                    atomic_unit_id: None,
                    from_path: doc3.to_string_lossy().to_string(),
                    to_path: Some(moved3.to_string_lossy().to_string()),
                    reason: "Refile 3".into(),
                    confidence: 1.0,
                    evidence_json: "[]".into(),
                },
            ],
        },
    )
    .unwrap();
    approve_batch(&mut store, &batch_m, None).unwrap();
    execute_batch(&mut store, &batch_m).unwrap();

    // Externally modify moved2 AND delete moved3 -> pre-flight must halt BOTH before touching disk!
    fs::write(&moved2, "externally edited content!").unwrap();
    fs::remove_file(&moved3).unwrap();

    let rb = rollback_batch(&mut store, &batch_m).unwrap();
    assert_eq!(rb.reverted_count, 0);
    assert_eq!(rb.conflicts.len(), 2);
    assert!(moved2.exists(), "Pre-flight must NOT mutate moved2 before resolution");

    let c2 = rb
        .conflicts
        .iter()
        .find(|c| c.conflict == RollbackConflictKind::Modified)
        .unwrap();
    let c3 = rb
        .conflicts
        .iter()
        .find(|c| c.conflict == RollbackConflictKind::MissingTarget)
        .unwrap();

    // Resolve modified via CopyBackKeepingBoth
    resolve_rollback_conflict(
        &mut store,
        &c2.action_id,
        ConflictResolutionChoice::CopyBackKeepingBoth,
    )
    .unwrap();
    assert!(doc2.exists() && moved2.exists());

    // Resolve missing_target via Skip -> marks action 'unrecoverable_missing'
    resolve_rollback_conflict(&mut store, &c3.action_id, ConflictResolutionChoice::Skip).unwrap();
    let st3: String = store
        .conn()
        .query_row(
            "SELECT status FROM actions WHERE id = ?1",
            params![c3.action_id],
            |r| r.get(0),
        )
        .unwrap();
    assert_eq!(st3, "unrecoverable_missing");
}

#[test]
fn scanner_atomic_units_cache_dirs_and_exclusions() {
    let tmp = TempDir::new().unwrap();
    let root = tmp.path().join("proj_root");
    fs::create_dir_all(root.join(".git")).unwrap();
    fs::create_dir_all(root.join("apps").join("web").join("node_modules").join("left-pad")).unwrap();
    fs::create_dir_all(root.join(".aegis-quarantine").join("b1")).unwrap();

    fs::write(root.join(".aegisignore"), "ignored.txt\n").unwrap();
    fs::write(root.join("ignored.txt"), "must not index").unwrap();
    fs::write(root.join(".aegis-quarantine").join("b1").join("secret.txt"), "must not index").unwrap();
    fs::write(root.join("README.md"), "workspace root readme").unwrap();
    fs::write(root.join("apps").join("web").join("package.json"), "{}").unwrap();
    fs::write(root.join("apps").join("web").join("notes.md"), "unique searchable frontend note").unwrap();
    fs::write(
        root.join("apps")
            .join("web")
            .join("node_modules")
            .join("left-pad")
            .join("index.js"),
        "module.exports = () => {};",
    )
    .unwrap();

    let mut store = SqliteStore::open_in_memory().unwrap();
    store.add_root(&root).unwrap();
    let status = scan_root(&mut store, &root, &[1, 2]).unwrap();
    assert_eq!(status.status, "completed");

    // 1. ignored.txt, .aegis-quarantine, and node_modules children must produce 0 rows in files
    let paths: Vec<(String, Option<String>)> = {
        let mut stmt = store
            .conn()
            .prepare("SELECT path, atomic_unit_id FROM files WHERE deleted_at IS NULL")
            .unwrap();
        stmt.query_map([], |r| Ok((r.get(0)?, r.get(1)?)))
            .unwrap()
            .map(|r| r.unwrap())
            .collect()
    };
    assert!(!paths.iter().any(|(p, _)| p.contains("ignored.txt")));
    assert!(!paths.iter().any(|(p, _)| p.contains(".aegis-quarantine")));
    assert!(!paths.iter().any(|(p, _)| p.contains("node_modules")));

    // 2. Innermost atomic unit wins: notes.md gets node_pkg, README.md gets git_repo
    let readme_au = paths
        .iter()
        .find(|(p, _)| p.ends_with("README.md"))
        .unwrap()
        .1
        .clone()
        .unwrap();
    let notes_au = paths
        .iter()
        .find(|(p, _)| p.ends_with("notes.md"))
        .unwrap()
        .1
        .clone()
        .unwrap();
    assert_ne!(readme_au, notes_au);

    // 3. cache_dirs recorded 1 entry for node_modules
    let cache_cnt: i64 = store
        .conn()
        .query_row("SELECT COUNT(*) FROM cache_dirs WHERE state = 'active'", [], |r| r.get(0))
        .unwrap();
    assert_eq!(cache_cnt, 1);

    // 4. FTS5 search finds notes.md
    let hits = search_fts(store.conn(), "searchable frontend", &SearchFilters::default(), 10).unwrap();
    assert_eq!(hits.hits.len(), 1);
}

#[test]
fn doctor_repairs_fts_and_catches_orphans() {
    let tmp = TempDir::new().unwrap();
    let f = tmp.path().join("doc.md");
    fs::write(&f, "doctor test chunk text").unwrap();

    let mut store = SqliteStore::open_in_memory().unwrap();
    let fid = seed_volume_and_file(&store, "v1", tmp.path(), &f);
    insert_chunk(store.conn(), &fid, 0, "doctor test chunk text", 4, true).unwrap();

    // Seed FTS drift by removing the doc from chunks_fts index + insert an orphan relationship
    store
        .conn()
        .execute(
            "INSERT INTO chunks_fts(chunks_fts, rowid, text) VALUES ('delete', 1, 'doctor test chunk text')",
            [],
        )
        .unwrap();
    store
        .conn()
        .execute(
            "INSERT INTO relationships (id, src_id, src_kind, dst_id, dst_kind, rel_type, valid_from, created_at)
             VALUES ('rel1', 'nonexistent_file', 'file', ?1, 'file', 'references', 1, 1)",
            params![fid],
        )
        .unwrap();

    let diag = run_doctor(&mut store, false).unwrap();
    assert!(!diag.healthy);
    assert!(!diag.checks[2].passed, "Check #3 FTS5 drift must fail");
    assert!(!diag.checks[7].passed, "Check #8 orphan edge must fail");

    let repaired = run_doctor(&mut store, true).unwrap();
    assert!(repaired.healthy);
    assert!(repaired.checks[2].repaired);
    assert!(repaired.checks[7].repaired);
}
